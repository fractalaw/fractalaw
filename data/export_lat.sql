-- export_lat.sql
-- Transforms LAT-*.csv and AMD-*.csv (legacy Airtable exports) into Fractalaw Parquet.
-- Usage: cd fractalaw && duckdb < data/export_lat.sql
--
-- Inputs:
--   data/LAT-*.csv  — 17 files, ~115K rows: structural content + C/I/E annotations
--   data/AMD-*.csv  — 16 files, ~12K rows: F-code (textual amendment) annotations
--
-- Outputs:
--   data/legislation_text.parquet        — LAT content rows (one row per structural unit)
--   data/amendment_annotations.parquet   — F/C/I/E annotation rows (from both sources)
--   data/annotation_totals.parquet       — per-law F/C/I/E sums for LRT backfill
--
-- ID normalization:
--   Legacy Airtable IDs carry acronym suffixes/prefixes that are stripped.
--   Three patterns:
--     UK_ACRO_typecode_year_number  → UK_typecode_year_number
--     UK_typecode_year_number_ACRO  → UK_typecode_year_number
--     UK_year_number_ACRO           → UK_year_number
--   All IDs (law_name, section_id, annotation id, affected_sections) are normalized.

SET threads = 4;

-- ---------------------------------------------------------------------------
-- 0. Load sources
-- ---------------------------------------------------------------------------

CREATE TABLE lat_raw AS
SELECT * FROM read_csv(
    'data/LAT-*.csv',
    auto_detect = true,
    header = true,
    all_varchar = true,
    union_by_name = true,
    filename = true
);

CREATE TABLE amd_raw AS
SELECT * FROM read_csv(
    'data/AMD-*.csv',
    auto_detect = true,
    header = true,
    all_varchar = true,
    union_by_name = true,
    filename = true
);

.print '--- Loaded sources'
SELECT 'LAT' AS src, count(*) AS rows, count(DISTINCT "UK") AS laws FROM lat_raw
UNION ALL
SELECT 'AMD', count(*), count(DISTINCT regexp_replace("ID", '_[FCIE][0-9]+$', '')) FROM amd_raw;

-- ---------------------------------------------------------------------------
-- 1. Macros
-- ---------------------------------------------------------------------------

-- Strip legacy acronym from a law name.
-- Handles: UK_ACRO_type_year_num, UK_type_year_num_ACRO, UK_year_num_ACRO
CREATE MACRO strip_acronym(name) AS (
    CASE
        WHEN regexp_matches(name, '^UK_[A-Z]+_[a-z]+_')
        THEN regexp_replace(name, '^(UK)_[A-Z]+_', '\1_')
        WHEN regexp_matches(name, '^UK_[a-z]+_[0-9]+_[0-9A-Za-z/\-]+_[A-Z]')
        THEN regexp_replace(name, '_[A-Z][A-Za-z0-9–]*$', '')
        WHEN regexp_matches(name, '^UK_[0-9]+_[0-9]+_[A-Z]')
        THEN regexp_replace(name, '_[A-Z][A-Za-z0-9–]*$', '')
        ELSE name
    END
);

-- Strip acronym from any ID by replacing the law_name prefix.
CREATE MACRO strip_id(id, law_name) AS (
    CASE WHEN strip_acronym(law_name) = law_name THEN id
         ELSE replace(id, law_name, strip_acronym(law_name))
    END
);

-- Content record types → schema section_type
CREATE MACRO map_section_type(rt) AS (
    CASE rt
        WHEN 'title'               THEN 'title'
        WHEN 'part'                THEN 'part'
        WHEN 'chapter'             THEN 'chapter'
        WHEN 'heading'             THEN 'heading'
        WHEN 'section'             THEN 'section'
        WHEN 'sub-section'         THEN 'sub_section'
        WHEN 'article'             THEN 'article'
        WHEN 'article,heading'     THEN 'heading'
        WHEN 'article,sub-article' THEN 'sub_article'
        WHEN 'sub-article'         THEN 'sub_article'
        WHEN 'paragraph'           THEN 'paragraph'
        WHEN 'sub-paragraph'       THEN 'sub_paragraph'
        WHEN 'schedule'            THEN 'schedule'
        WHEN 'annex'               THEN 'schedule'
        WHEN 'table'               THEN 'table'
        WHEN 'sub-table'           THEN 'table'
        WHEN 'figure'              THEN 'note'
        WHEN 'signed'              THEN 'signed'
        WHEN 'commencement'        THEN 'commencement'
        WHEN 'table,heading'       THEN 'heading'
        ELSE rt
    END
);

-- Annotation code_type from Record_Type
CREATE MACRO map_code_type(rt) AS (
    CASE
        WHEN rt LIKE 'amendment,%'        THEN 'amendment'
        WHEN rt = 'modification,content'  THEN 'modification'
        WHEN rt = 'commencement,content'  THEN 'commencement'
        WHEN rt = 'extent,content'        THEN 'extent_editorial'
        WHEN rt = 'editorial,content'     THEN 'extent_editorial'
        WHEN rt = 'subordinate,content'   THEN 'extent_editorial'
        ELSE NULL
    END
);

-- Extract annotation code from text (e.g. "F1 S. 2(1)..." → "F1")
CREATE MACRO extract_code(txt) AS (
    regexp_extract(txt, '^([FCIE][0-9]+)', 1)
);

-- Derive extent_code from Region column
CREATE MACRO region_to_extent(r) AS (
    CASE
        WHEN r IS NULL OR r = '' THEN NULL
        WHEN r LIKE '%England%Wales%Scotland%Northern Ireland%' THEN 'E+W+S+NI'
        WHEN r LIKE '%England%Wales%Scotland%'                  THEN 'E+W+S'
        WHEN r LIKE '%England%Wales%Northern Ireland%'          THEN 'E+W+NI'
        WHEN r LIKE '%England%Scotland%'                        THEN 'E+S'
        WHEN r LIKE '%England%Wales%'                           THEN 'E+W'
        WHEN r LIKE '%England%Northern Ireland%'                THEN 'E+NI'
        WHEN r = 'England'          THEN 'E'
        WHEN r = 'Wales'            THEN 'W'
        WHEN r = 'Scotland'         THEN 'S'
        WHEN r = 'Northern Ireland' THEN 'NI'
        WHEN r LIKE 'GB%'           THEN 'E+W+S'
        WHEN r LIKE 'UK%'           THEN 'E+W+S+NI'
        ELSE r
    END
);

-- Build hierarchy_path from structural columns
CREATE MACRO build_hierarchy(flow_val, part_val, chap_val, head_val, prov_val, sub_val, para_val) AS (
    concat_ws('/',
        CASE WHEN flow_val NOT IN ('pre', 'main', 'post', 'signed', '') AND flow_val IS NOT NULL
             THEN 'schedule.' || flow_val ELSE NULL END,
        CASE WHEN part_val IS NOT NULL AND part_val != '' THEN 'part.' || part_val ELSE NULL END,
        CASE WHEN chap_val IS NOT NULL AND chap_val != '' THEN 'chapter.' || chap_val ELSE NULL END,
        CASE WHEN head_val IS NOT NULL AND head_val != '' THEN 'heading.' || head_val ELSE NULL END,
        CASE WHEN prov_val IS NOT NULL AND prov_val != '' THEN 'provision.' || prov_val ELSE NULL END,
        CASE WHEN sub_val IS NOT NULL AND sub_val != '' THEN 'sub.' || sub_val ELSE NULL END,
        CASE WHEN para_val IS NOT NULL AND para_val != '' THEN 'para.' || para_val ELSE NULL END
    )
);

-- Count codes with a given prefix in a comma-separated Changes string
CREATE MACRO count_codes(changes_val, prefix) AS (
    CASE WHEN changes_val IS NULL OR changes_val = '' THEN NULL
         ELSE list_count(list_filter(
             string_split(changes_val, ','),
             x -> trim(x)[1:1] = prefix
         ))
    END
);

-- Strip annotation suffix to get parent content row ID
CREATE MACRO annotation_parent_id(id) AS (
    regexp_replace(id, '_(a|ax|cx|mx|ex|xx|px)_[0-9]+$', '')
);

-- Is this a content row (structural unit of the law)?
CREATE MACRO is_content_row(rt) AS (
    rt IS NOT NULL
    AND rt != ''
    AND rt NOT LIKE '%,content'
    AND rt NOT LIKE 'amendment,%'
    AND rt NOT LIKE 'subordinate,%'
    AND rt NOT LIKE 'editorial,%'
    AND rt NOT IN ('commencement,heading', 'modification,heading',
                   'extent,heading', 'editorial,heading', 'subordinate,heading')
);

-- ---------------------------------------------------------------------------
-- Sort-key normalisation (DuckDB reimplementation of sort_key.rs)
-- ---------------------------------------------------------------------------

-- Extract leading digits from a provision number as an integer.
-- '3A' → 3, '19DZA' → 19, '' → 0
CREATE MACRO prov_base(s) AS (
    CASE WHEN regexp_extract(upper(trim(COALESCE(s, ''))), '^(\d+)', 1) = ''
         THEN 0
         ELSE CAST(regexp_extract(upper(trim(COALESCE(s, ''))), '^(\d+)', 1) AS INTEGER)
    END
);

-- Extract the suffix (letters after leading digits), uppercased.
-- '3A' → 'A', '19DZA' → 'DZA', '42' → ''
CREATE MACRO prov_suffix(s) AS (
    regexp_replace(upper(trim(COALESCE(s, ''))), '^\d+', '')
);

-- Value of the first suffix group in a letter string.
-- Z-prefix: ZA=1, ZB=2, ..., ZZ=26
-- Plain letter: A=10, B=20, ..., Z=260
-- Empty/other: 0
CREATE MACRO suffix_val(suf) AS (
    CASE
        WHEN suf IS NULL OR length(suf) = 0 THEN 0
        WHEN length(suf) >= 2 AND substr(suf, 1, 1) = 'Z'
             AND ascii(substr(suf, 2, 1)) BETWEEN 65 AND 90
            THEN ascii(substr(suf, 2, 1)) - 64
        WHEN ascii(substr(suf, 1, 1)) BETWEEN 65 AND 90
            THEN (ascii(substr(suf, 1, 1)) - 64) * 10
        ELSE 0
    END
);

-- Characters consumed by the first suffix group.
-- Z-prefix: 2, plain letter: 1, empty/other: 0
CREATE MACRO suffix_len(suf) AS (
    CASE
        WHEN suf IS NULL OR length(suf) = 0 THEN 0
        WHEN length(suf) >= 2 AND substr(suf, 1, 1) = 'Z'
             AND ascii(substr(suf, 2, 1)) BETWEEN 65 AND 90
            THEN 2
        WHEN ascii(substr(suf, 1, 1)) BETWEEN 65 AND 90
            THEN 1
        ELSE 0
    END
);

-- Normalise a provision number into a lexicographically-sortable string.
-- Matches the Rust normalize_provision() in sort_key.rs.
-- '3' → '003.000.000', '3A' → '003.010.000', '3ZA' → '003.001.000',
-- '19DZA' → '019.040.001', '' → '000.000.000'
CREATE MACRO normalize_provision(s) AS (
    lpad(CAST(prov_base(s) AS VARCHAR), 3, '0') || '.' ||
    lpad(CAST(suffix_val(prov_suffix(s)) AS VARCHAR), 3, '0') || '.' ||
    lpad(CAST(
        suffix_val(substr(prov_suffix(s), suffix_len(prov_suffix(s)) + 1))
    AS VARCHAR), 3, '0')
);

-- ---------------------------------------------------------------------------
-- Citation and sort-key builders for the three-column identity design
-- ---------------------------------------------------------------------------

-- Provision prefix for section_id citation.
-- section/sub_section → 's.', article (Regulation) → 'reg.', article (other) → 'art.'
CREATE MACRO provision_prefix(section_type, class) AS (
    CASE
        WHEN section_type IN ('section', 'sub_section') THEN 's.'
        WHEN section_type IN ('article', 'sub_article') AND class = 'Regulation' THEN 'reg.'
        WHEN section_type IN ('article', 'sub_article') THEN 'art.'
        ELSE NULL
    END
);

-- Schedule prefix: 'sch.{N}.' for rows inside a schedule, '' otherwise.
-- The schedule container row itself (section_type='schedule') gets no prefix.
CREATE MACRO sch_prefix(section_type, schedule) AS (
    CASE WHEN schedule IS NOT NULL AND section_type != 'schedule'
         THEN 'sch.' || schedule || '.'
         ELSE ''
    END
);

-- Build citation string for section_id (without law_name prefix or extent qualifier).
-- Examples: 's.25A', 's.25A(1)', 'reg.2(1)(b)', 'h.18', 'pt.1', 'sch.2.para.3', 'title'
-- Schedule-scoped types get prefixed: 'sch.2.h.1', 'sch.2.pt.1', etc.
-- Position is used as fallback for disambiguation when structural data is insufficient.
CREATE MACRO build_citation(section_type, class, provision, sub, para,
                            part, chapter, heading_group, schedule, pos) AS (
    CASE
        -- Singleton types: position-qualified to handle multiples
        WHEN section_type = 'title' THEN 'title.' || CAST(pos AS VARCHAR)
        WHEN section_type = 'signed' THEN 'signed.' || CAST(pos AS VARCHAR)
        WHEN section_type = 'commencement' THEN 'commencement.' || CAST(pos AS VARCHAR)

        -- Schedule container row
        WHEN section_type = 'schedule' THEN
            'sch.' || COALESCE(NULLIF(schedule, ''), CAST(pos AS VARCHAR))

        -- Paragraph/sub_paragraph in a schedule: special format
        WHEN section_type IN ('paragraph', 'sub_paragraph') AND schedule IS NOT NULL THEN
            'sch.' || schedule || '.para.' || COALESCE(NULLIF(para, ''), CAST(pos AS VARCHAR))

        -- Structural containers (schedule-prefixed when inside a schedule)
        WHEN section_type = 'part' THEN
            sch_prefix(section_type, schedule) ||
            'pt.' || COALESCE(NULLIF(part, ''), CAST(pos AS VARCHAR))
        WHEN section_type = 'chapter' THEN
            sch_prefix(section_type, schedule) ||
            'ch.' || COALESCE(NULLIF(chapter, ''), CAST(pos AS VARCHAR))
        WHEN section_type = 'heading' THEN
            sch_prefix(section_type, schedule) ||
            'h.' || COALESCE(NULLIF(heading_group, ''), CAST(pos AS VARCHAR))

        -- Section/sub_section/paragraph/sub_paragraph in main body
        WHEN section_type IN ('section', 'sub_section', 'paragraph', 'sub_paragraph') THEN
            sch_prefix(section_type, schedule) ||
            's.' || CASE WHEN provision IS NOT NULL AND provision != ''
                         THEN provision ELSE CAST(pos AS VARCHAR) END ||
            CASE WHEN sub IS NOT NULL AND sub != '' THEN '(' || sub || ')' ELSE '' END ||
            CASE WHEN para IS NOT NULL AND para != '' THEN '(' || para || ')' ELSE '' END

        -- Article/sub_article (Regulation class)
        WHEN section_type IN ('article', 'sub_article') AND class = 'Regulation' THEN
            sch_prefix(section_type, schedule) ||
            'reg.' || CASE WHEN provision IS NOT NULL AND provision != ''
                           THEN provision ELSE CAST(pos AS VARCHAR) END ||
            CASE WHEN sub IS NOT NULL AND sub != '' THEN '(' || sub || ')' ELSE '' END ||
            CASE WHEN para IS NOT NULL AND para != '' THEN '(' || para || ')' ELSE '' END

        -- Article/sub_article (other class)
        WHEN section_type IN ('article', 'sub_article') THEN
            sch_prefix(section_type, schedule) ||
            'art.' || CASE WHEN provision IS NOT NULL AND provision != ''
                           THEN provision ELSE CAST(pos AS VARCHAR) END ||
            CASE WHEN sub IS NOT NULL AND sub != '' THEN '(' || sub || ')' ELSE '' END ||
            CASE WHEN para IS NOT NULL AND para != '' THEN '(' || para || ')' ELSE '' END

        -- Table, note, other: fallback to type.position
        ELSE sch_prefix(section_type, schedule) ||
             section_type || '.' || CAST(pos AS VARCHAR)
    END
);

-- Build sort_key from provision/heading/paragraph data (without extent suffix).
CREATE MACRO build_sort_key(section_type, provision, heading_group, para, schedule) AS (
    CASE
        -- Sections/articles/sub-types with a provision number
        WHEN section_type IN ('section', 'sub_section', 'article', 'sub_article',
                              'paragraph', 'sub_paragraph')
             AND provision IS NOT NULL AND provision != ''
            THEN normalize_provision(provision)
        -- Headings keyed by their heading_group label
        WHEN section_type = 'heading'
             AND heading_group IS NOT NULL AND heading_group != ''
            THEN normalize_provision(heading_group)
        -- Schedule paragraphs without a provision: use paragraph number
        WHEN section_type IN ('paragraph', 'sub_paragraph')
             AND schedule IS NOT NULL
             AND para IS NOT NULL AND para != ''
            THEN normalize_provision(para)
        -- Structural rows (title, part, chapter, schedule, signed, etc.)
        ELSE '000.000.000'
    END
);

.print '--- Created macros'

-- ---------------------------------------------------------------------------
-- Macro smoke tests
-- ---------------------------------------------------------------------------

.print ''
.print '--- normalize_provision tests (should match sort_key.rs)'
SELECT
    normalize_provision('3')     AS "3→003.000.000",
    normalize_provision('3A')    AS "3A→003.010.000",
    normalize_provision('3ZA')   AS "3ZA→003.001.000",
    normalize_provision('3ZB')   AS "3ZB→003.002.000",
    normalize_provision('3AA')   AS "3AA→003.010.010",
    normalize_provision('3AB')   AS "3AB→003.010.020",
    normalize_provision('3B')    AS "3B→003.020.000",
    normalize_provision('19DZA') AS "19DZA→019.040.001",
    normalize_provision('42')    AS "42→042.000.000",
    normalize_provision('')      AS "empty→000.000.000";

.print ''
.print '--- Sort order test (should be in strictly ascending order)'
SELECT col0 AS provision, normalize_provision(col0) AS sort_key
FROM (VALUES ('3'), ('3ZA'), ('3ZB'), ('3A'), ('3AA'), ('3AB'), ('3B'), ('4'), ('10'))
ORDER BY normalize_provision(col0);

.print ''
.print '--- build_citation tests'
SELECT
    build_citation('section',  NULL,         '25A', NULL, NULL, NULL, NULL, NULL, NULL, 10) AS "section→s.25A",
    build_citation('sub_section', NULL,      '25A', '1',  NULL, NULL, NULL, NULL, NULL, 11) AS "sub_s→s.25A(1)",
    build_citation('article',  'Regulation', '2',   '1',  'b',  NULL, NULL, NULL, NULL, 5)  AS "reg→reg.2(1)(b)",
    build_citation('article',  NULL,         '16B', NULL, NULL, NULL, NULL, NULL, NULL, 8)  AS "art→art.16B",
    build_citation('heading',  NULL,         NULL,  NULL, NULL, NULL, NULL, '18',  NULL, 20) AS "heading→h.18",
    build_citation('heading',  NULL,         NULL,  NULL, NULL, NULL, NULL, '5',   '2',  35) AS "sch_head→sch.2.h.5",
    build_citation('part',     NULL,         NULL,  NULL, NULL, '1',  NULL, NULL,  NULL, 2)  AS "part→pt.1",
    build_citation('schedule', NULL,         NULL,  NULL, NULL, NULL, NULL, NULL,  '2',  30) AS "sched→sch.2",
    build_citation('paragraph', NULL,        NULL,  NULL, '3',  NULL, NULL, NULL,  '2',  31) AS "sch_para→sch.2.para.3",
    build_citation('title',    NULL,         NULL,  NULL, NULL, NULL, NULL, NULL,  NULL, 1)  AS "title→title.1",
    build_citation('commencement', NULL,     NULL,  NULL, NULL, NULL, NULL, NULL,  NULL, 50) AS "commence→commencement.50",
    build_citation('table',    NULL,         NULL,  NULL, NULL, NULL, NULL, NULL,  NULL, 50) AS "table→table.50";

.print ''
.print '--- build_sort_key tests'
SELECT
    build_sort_key('section',   '25A', NULL, NULL, NULL) AS "s.25A→025.010.000",
    build_sort_key('heading',   NULL,  '18', NULL, NULL) AS "h.18→018.000.000",
    build_sort_key('paragraph', NULL,  NULL, '3',  '2')  AS "sch_para→003.000.000",
    build_sort_key('title',     NULL,  NULL, NULL, NULL) AS "title→000.000.000";

.print ''
.print '--- provision_prefix tests'
SELECT
    provision_prefix('section', NULL) AS "section→s.",
    provision_prefix('article', 'Regulation') AS "reg→reg.",
    provision_prefix('article', NULL) AS "art→art.",
    provision_prefix('heading', NULL) AS "heading→NULL";

-- ---------------------------------------------------------------------------
-- 2. Count C/I/E annotations per content section (by parent ID)
-- ---------------------------------------------------------------------------

CREATE TABLE cie_counts AS
SELECT
    annotation_parent_id("ID") AS parent_id,
    CAST(count(*) FILTER (WHERE Record_Type = 'modification,content')  AS INTEGER) AS modification_count,
    CAST(count(*) FILTER (WHERE Record_Type = 'commencement,content')  AS INTEGER) AS commencement_count,
    CAST(count(*) FILTER (WHERE Record_Type IN ('extent,content', 'editorial,content')) AS INTEGER) AS extent_count,
    CAST(count(*) FILTER (WHERE Record_Type = 'editorial,content')     AS INTEGER) AS editorial_count
FROM lat_raw
WHERE Record_Type IN ('modification,content', 'commencement,content', 'extent,content', 'editorial,content')
GROUP BY annotation_parent_id("ID");

.print '--- Built C/I/E counts per section'

-- ---------------------------------------------------------------------------
-- 2b. Detect parallel territorial provisions
-- ---------------------------------------------------------------------------
-- A (law_name, provision) pair is "parallel" when the same provision number
-- exists with multiple distinct extent_codes — e.g. s.23 in HSWA 1974 has
-- separate E+W, S, and NI versions. These need [extent] qualifiers in section_id.

CREATE TABLE parallel_provisions AS
SELECT
    strip_acronym("UK") AS law_name,
    "Section||Regulation" AS provision
FROM lat_raw
WHERE is_content_row(Record_Type)
  AND "Section||Regulation" IS NOT NULL
  AND "Section||Regulation" != ''
  AND region_to_extent("Region") IS NOT NULL
  AND strip_acronym("UK") != 'UK_uksi_2016_1091'
GROUP BY strip_acronym("UK"), "Section||Regulation"
HAVING count(DISTINCT region_to_extent("Region")) > 1;

.print '--- Built parallel_provisions'
SELECT count(*) AS parallel_pairs,
       count(DISTINCT law_name) AS laws_with_parallels
FROM parallel_provisions;

-- ---------------------------------------------------------------------------
-- 2c. Build content ID map (old legacy_id → new citation-based section_id)
-- ---------------------------------------------------------------------------
-- Maps every content row's old stripped ID to the new section_id.
-- Used in Stage 5 to remap affected_sections on annotations.

CREATE TABLE content_id_map AS
WITH content_rows AS (
    SELECT *,
        strip_acronym("UK") AS law_name,
        strip_id("ID", "UK") AS legacy_id,
        map_section_type(Record_Type) AS stype,
        row_number() OVER (
            PARTITION BY strip_acronym("UK")
            ORDER BY rowid
        ) AS pos
    FROM lat_raw
    WHERE is_content_row(Record_Type)
      AND strip_acronym("UK") != 'UK_uksi_2016_1091'
),
-- Build structural citation (may have duplicates for headings/parts/chapters
-- that reset across parts/schedules within the same law).
base_ids AS (
    SELECT
        cr.legacy_id,
        cr.pos,
        cr.law_name || ':' ||
        build_citation(
            cr.stype, cr."Class",
            cr."Section||Regulation", cr."Sub_Section||Sub_Regulation", cr."Paragraph",
            cr."Part", cr."Chapter", cr."Heading",
            CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL
                 THEN cr.flow ELSE NULL END,
            cr.pos
        ) ||
        CASE WHEN pp.provision IS NOT NULL
             THEN '[' || COALESCE(region_to_extent(cr."Region"), '') || ']'
             ELSE ''
        END AS base_id
    FROM content_rows cr
    LEFT JOIN parallel_provisions pp
        ON pp.law_name = cr.law_name
        AND pp.provision = cr."Section||Regulation"
),
-- Detect duplicates and disambiguate with position suffix.
counted AS (
    SELECT *,
        count(*) OVER (PARTITION BY base_id) AS id_count
    FROM base_ids
)
SELECT
    legacy_id,
    CASE WHEN id_count > 1
         THEN base_id || '#' || CAST(pos AS VARCHAR)
         ELSE base_id
    END AS section_id
FROM counted;

.print '--- Built content_id_map'
SELECT count(*) AS mapped_rows,
       count(DISTINCT section_id) AS distinct_section_ids
FROM content_id_map;

-- ---------------------------------------------------------------------------
-- 3. Export legislation_text.parquet (28 columns, matching schema.rs)
-- ---------------------------------------------------------------------------

COPY (
    WITH content_rows AS (
        SELECT *,
            strip_acronym("UK") AS law_name,
            strip_id("ID", "UK") AS legacy_id,
            map_section_type(Record_Type) AS stype,
            row_number() OVER (
                PARTITION BY strip_acronym("UK")
                ORDER BY rowid
            ) AS pos
        FROM lat_raw
        WHERE is_content_row(Record_Type)
          AND strip_acronym("UK") != 'UK_uksi_2016_1091'
          AND "Text" IS NOT NULL
    ),
    -- Build base citation and detect duplicates (mirrors content_id_map logic)
    with_citation AS (
        SELECT cr.*,
            cr.law_name || ':' ||
            build_citation(
                cr.stype, cr."Class",
                cr."Section||Regulation", cr."Sub_Section||Sub_Regulation", cr."Paragraph",
                cr."Part", cr."Chapter", cr."Heading",
                CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL
                     THEN cr.flow ELSE NULL END,
                cr.pos
            ) ||
            CASE WHEN pp.provision IS NOT NULL
                 THEN '[' || COALESCE(region_to_extent(cr."Region"), '') || ']'
                 ELSE ''
            END AS base_section_id,
            build_sort_key(
                cr.stype,
                cr."Section||Regulation", cr."Heading", cr."Paragraph",
                CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL
                     THEN cr.flow ELSE NULL END
            ) ||
            CASE WHEN pp.provision IS NOT NULL
                 THEN '~' || COALESCE(region_to_extent(cr."Region"), '')
                 ELSE ''
            END AS sort_key
        FROM content_rows cr
        LEFT JOIN parallel_provisions pp
            ON pp.law_name = cr.law_name
            AND pp.provision = cr."Section||Regulation"
    ),
    disambiguated AS (
        SELECT *,
            count(*) OVER (PARTITION BY base_section_id) AS id_count
        FROM with_citation
    )
    SELECT
        -- 3.1 Identity & Position (7)
        cr.law_name,
        CASE WHEN cr.id_count > 1
             THEN cr.base_section_id || '#' || CAST(cr.pos AS VARCHAR)
             ELSE cr.base_section_id
        END AS section_id,
        cr.sort_key,
        CAST(cr.pos AS INTEGER) AS position,
        cr.stype AS section_type,
        build_hierarchy(cr.flow, cr."Part", cr."Chapter", cr."Heading",
                        cr."Section||Regulation", cr."Sub_Section||Sub_Regulation",
                        cr."Paragraph") AS hierarchy_path,
        CAST(
            (CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Part" IS NOT NULL AND cr."Part" != '' THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Chapter" IS NOT NULL AND cr."Chapter" != '' THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Heading" IS NOT NULL AND cr."Heading" != '' THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Section||Regulation" IS NOT NULL AND cr."Section||Regulation" != '' THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Sub_Section||Sub_Regulation" IS NOT NULL AND cr."Sub_Section||Sub_Regulation" != '' THEN 1 ELSE 0 END)
            + (CASE WHEN cr."Paragraph" IS NOT NULL AND cr."Paragraph" != '' THEN 1 ELSE 0 END)
        AS INTEGER) AS depth,

        -- 3.2 Structural Hierarchy (7)
        cr."Part" AS part,
        cr."Chapter" AS chapter,
        cr."Heading" AS heading_group,
        cr."Section||Regulation" AS provision,
        cr."Paragraph" AS paragraph,
        cr."Sub_Section||Sub_Regulation" AS sub_paragraph,
        CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL
             THEN cr.flow ELSE NULL END AS schedule,

        -- 3.3 Content (3)
        cr."Text" AS text,
        'en' AS "language",
        region_to_extent(cr."Region") AS extent_code,

        -- 3.4 Amendment Annotations (5)
        CAST(count_codes(cr."Changes", 'F') AS INTEGER) AS amendment_count,
        COALESCE(cie.modification_count,  NULL::INTEGER) AS modification_count,
        COALESCE(cie.commencement_count,  NULL::INTEGER) AS commencement_count,
        COALESCE(cie.extent_count,        NULL::INTEGER) AS extent_count,
        COALESCE(cie.editorial_count,     NULL::INTEGER) AS editorial_count,

        -- 3.5 Embeddings (3 — null, populated in Phase 2)
        NULL::FLOAT[] AS embedding,
        NULL::VARCHAR AS embedding_model,
        NULL::TIMESTAMPTZ AS embedded_at,

        -- 3.6 Migration (1)
        cr.legacy_id,

        -- 3.7 Metadata (2)
        current_timestamp AS created_at,
        current_timestamp AS updated_at

    FROM disambiguated cr
    LEFT JOIN cie_counts cie ON cie.parent_id = cr."ID"
    ORDER BY cr.law_name, cr.sort_key, cr.pos
)
TO 'data/legislation_text.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '--- Exported legislation_text.parquet'

-- ---------------------------------------------------------------------------
-- 4. Build affected_sections for annotations
-- ---------------------------------------------------------------------------
-- Remap legacy section IDs → new citation-based section_ids via content_id_map.

-- 4a. F-code affected_sections from LAT Changes column inversion.
CREATE TABLE f_affected_lat AS
SELECT
    strip_acronym(r."UK") AS law_name,
    trim(code_ref) AS code,
    list(DISTINCT cim.section_id ORDER BY cim.section_id) AS affected_sections
FROM lat_raw r,
     LATERAL unnest(string_split(r."Changes", ',')) AS t(code_ref)
INNER JOIN content_id_map cim ON cim.legacy_id = strip_id(r."ID", r."UK")
WHERE r."Changes" IS NOT NULL
  AND r."Changes" != ''
  AND trim(code_ref) != ''
  AND is_content_row(r.Record_Type)
GROUP BY strip_acronym(r."UK"), trim(code_ref);

.print '--- Built F-code affected_sections from LAT'

-- 4b. F-code affected_sections from AMD Articles column.
--     Articles = comma-separated section IDs (with acronyms).
--     Remap each article_ref through content_id_map.
CREATE TABLE f_affected_amd AS
SELECT
    strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) AS law_name,
    a."Ef Code" AS code,
    list(DISTINCT cim.section_id ORDER BY cim.section_id) AS affected_sections
FROM amd_raw a,
     LATERAL unnest(string_split(a."Articles", ',')) AS t(article_ref)
INNER JOIN content_id_map cim
    ON cim.legacy_id = strip_id(trim(article_ref), regexp_replace(a."ID", '_[FCIE][0-9]+$', ''))
WHERE a."Articles" IS NOT NULL
  AND a."Articles" != ''
  AND trim(article_ref) != ''
  AND a."Ef Code" IS NOT NULL
GROUP BY strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', '')), a."Ef Code";

.print '--- Built F-code affected_sections from AMD'

-- ---------------------------------------------------------------------------
-- 5. Export amendment_annotations.parquet (9 columns)
-- ---------------------------------------------------------------------------
-- Three sources combined with synthetic IDs and source column:
--   1. C/I/E annotations from LAT → source='lat_cie'
--   2. F-code annotations from LAT → source='lat_f'
--   3. F-code annotations from AMD → source='amd_f'
-- Synthetic id: {law_name}:{code_type}:{seq}

COPY (
    WITH all_annotations AS (
        -- Source 1: C/I/E annotations from LAT
        SELECT
            strip_acronym(r."UK") AS law_name,
            COALESCE(NULLIF(extract_code(r."Text"), ''), map_code_type(r.Record_Type)[1:1] || '0') AS code,
            map_code_type(r.Record_Type) AS code_type,
            'lat_cie' AS source,
            r."Text" AS text,
            [cim.section_id] AS affected_sections,
            strip_id(r."ID", r."UK") AS _orig_id
        FROM lat_raw r
        INNER JOIN content_id_map cim
            ON cim.legacy_id = strip_id(annotation_parent_id(r."ID"), r."UK")
        WHERE r.Record_Type IN ('modification,content', 'commencement,content',
                                'extent,content', 'editorial,content', 'subordinate,content')
          AND strip_acronym(r."UK") != 'UK_uksi_2016_1091'

        UNION ALL

        -- Source 2: F-code annotations from LAT
        SELECT
            strip_acronym(r."UK") AS law_name,
            COALESCE(NULLIF(extract_code(r."Text"), ''), 'F' || r."Amendment") AS code,
            'amendment' AS code_type,
            'lat_f' AS source,
            r."Text" AS text,
            fa.affected_sections,
            strip_id(r."ID", r."UK") AS _orig_id
        FROM lat_raw r
        LEFT JOIN f_affected_lat fa
            ON fa.law_name = strip_acronym(r."UK")
            AND fa.code = COALESCE(NULLIF(extract_code(r."Text"), ''), 'F' || r."Amendment")
        WHERE r.Record_Type LIKE 'amendment,%'
          AND strip_acronym(r."UK") != 'UK_uksi_2016_1091'

        UNION ALL

        -- Source 3: F-code annotations from AMD
        SELECT
            strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) AS law_name,
            a."Ef Code" AS code,
            'amendment' AS code_type,
            'amd_f' AS source,
            a."Text" AS text,
            fa.affected_sections,
            strip_id(a."ID", regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) AS _orig_id
        FROM amd_raw a
        LEFT JOIN f_affected_amd fa
            ON fa.law_name = strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', ''))
            AND fa.code = a."Ef Code"
        WHERE a."Ef Code" IS NOT NULL
          AND strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) != 'UK_uksi_2016_1091'
    )
    SELECT
        law_name || ':' || code_type || ':' || CAST(row_number() OVER (
            PARTITION BY law_name, code_type
            ORDER BY code, source, _orig_id
        ) AS VARCHAR) AS id,
        law_name,
        code,
        code_type,
        source,
        text,
        affected_sections,
        current_timestamp AS created_at,
        current_timestamp AS updated_at
    FROM all_annotations
    ORDER BY law_name, code_type, code, source
)
TO 'data/amendment_annotations.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '--- Exported amendment_annotations.parquet'

-- ---------------------------------------------------------------------------
-- 6. Export annotation_totals.parquet (LRT backfill)
-- ---------------------------------------------------------------------------

COPY (
    WITH f_totals AS (
        SELECT strip_acronym("UK") AS name,
               CAST(sum(count_codes("Changes", 'F')) AS INTEGER) AS total_text_amendments
        FROM lat_raw
        WHERE is_content_row(Record_Type)
          AND strip_acronym("UK") != 'UK_uksi_2016_1091'
        GROUP BY strip_acronym("UK")
    ),
    cie_totals AS (
        SELECT strip_acronym("UK") AS name,
               CAST(count(*) FILTER (WHERE Record_Type = 'modification,content')  AS INTEGER) AS total_modifications,
               CAST(count(*) FILTER (WHERE Record_Type = 'commencement,content')  AS INTEGER) AS total_commencements,
               CAST(count(*) FILTER (WHERE Record_Type IN ('extent,content', 'editorial,content')) AS INTEGER) AS total_extents
        FROM lat_raw
        WHERE map_code_type(Record_Type) IS NOT NULL
          AND Record_Type NOT LIKE 'amendment,%'
          AND strip_acronym("UK") != 'UK_uksi_2016_1091'
        GROUP BY strip_acronym("UK")
    ),
    amd_totals AS (
        SELECT strip_acronym(regexp_replace("ID", '_[FCIE][0-9]+$', '')) AS name,
               CAST(count(*) AS INTEGER) AS amd_text_amendments
        FROM amd_raw
        WHERE "Ef Code" IS NOT NULL
          AND strip_acronym(regexp_replace("ID", '_[FCIE][0-9]+$', '')) != 'UK_uksi_2016_1091'
        GROUP BY strip_acronym(regexp_replace("ID", '_[FCIE][0-9]+$', ''))
    )
    SELECT
        COALESCE(f.name, c.name, a.name) AS name,
        COALESCE(f.total_text_amendments, 0) + COALESCE(a.amd_text_amendments, 0) AS total_text_amendments,
        COALESCE(c.total_modifications, 0) AS total_modifications,
        COALESCE(c.total_commencements, 0) AS total_commencements,
        COALESCE(c.total_extents, 0) AS total_extents
    FROM f_totals f
    FULL OUTER JOIN cie_totals c ON c.name = f.name
    FULL OUTER JOIN amd_totals a ON a.name = COALESCE(f.name, c.name)
    WHERE COALESCE(f.total_text_amendments, 0) + COALESCE(a.amd_text_amendments, 0) > 0
       OR COALESCE(c.total_modifications, 0) > 0
       OR COALESCE(c.total_commencements, 0) > 0
       OR COALESCE(c.total_extents, 0) > 0
)
TO 'data/annotation_totals.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '--- Exported annotation_totals.parquet'

-- ---------------------------------------------------------------------------
-- 7. Verify
-- ---------------------------------------------------------------------------

.print ''
.print '=== legislation_text.parquet ==='
SELECT count(*) AS rows,
       count(DISTINCT law_name) AS distinct_laws,
       count(DISTINCT section_type) AS section_types,
       count(text) AS has_text,
       count(amendment_count) AS has_f_count,
       count(modification_count) AS has_c_count,
       count(commencement_count) AS has_i_count,
       count(extent_code) AS has_extent,
       count(legacy_id) AS has_legacy_id,
       count(sort_key) AS has_sort_key
FROM read_parquet('data/legislation_text.parquet');

SELECT section_type, count(*) AS cnt
FROM read_parquet('data/legislation_text.parquet')
GROUP BY section_type
ORDER BY cnt DESC;

.print ''
.print '=== section_id uniqueness ==='
SELECT count(*) AS duplicate_section_ids
FROM (SELECT section_id FROM read_parquet('data/legislation_text.parquet')
      GROUP BY section_id HAVING count(*) > 1);

.print ''
.print '=== Acronym stripping verification ==='
SELECT count(*) AS laws_with_acronym
FROM (SELECT DISTINCT law_name FROM read_parquet('data/legislation_text.parquet')
      WHERE length(law_name) - length(replace(law_name, '_', '')) > 3);

.print ''
.print '=== amendment_annotations.parquet ==='
SELECT count(*) AS rows,
       count(DISTINCT law_name) AS distinct_laws,
       count(DISTINCT code_type) AS code_types,
       count(affected_sections) AS has_affected_sections
FROM read_parquet('data/amendment_annotations.parquet');

SELECT code_type, count(*) AS cnt,
       count(affected_sections) AS with_affected
FROM read_parquet('data/amendment_annotations.parquet')
GROUP BY code_type
ORDER BY cnt DESC;

.print ''
.print '=== annotation id uniqueness ==='
SELECT count(*) AS duplicate_annotation_ids
FROM (SELECT id FROM read_parquet('data/amendment_annotations.parquet')
      GROUP BY id HAVING count(*) > 1);

.print ''
.print '=== annotation source breakdown ==='
SELECT source, count(*) AS cnt
FROM read_parquet('data/amendment_annotations.parquet')
GROUP BY source
ORDER BY cnt DESC;

.print ''
.print '=== Sample annotation IDs ==='
SELECT id, law_name, code, code_type, source
FROM read_parquet('data/amendment_annotations.parquet')
WHERE law_name = 'UK_ukpga_1974_37'
ORDER BY code_type, code
LIMIT 10;

.print ''
.print '=== annotation_totals.parquet ==='
SELECT count(*) AS laws_with_annotations,
       sum(total_text_amendments) AS total_f,
       sum(total_modifications) AS total_c,
       sum(total_commencements) AS total_i,
       sum(total_extents) AS total_e
FROM read_parquet('data/annotation_totals.parquet');

.print ''
.print '=== FK match: LAT law_name → LRT name ==='
SELECT count(*) AS lat_laws,
       count(*) FILTER (WHERE EXISTS (
           SELECT 1 FROM read_parquet('data/legislation.parquet') lrt WHERE lrt.name = t.law_name
       )) AS matched_lrt,
       count(*) FILTER (WHERE NOT EXISTS (
           SELECT 1 FROM read_parquet('data/legislation.parquet') lrt WHERE lrt.name = t.law_name
       )) AS unmatched
FROM (SELECT DISTINCT law_name FROM read_parquet('data/legislation_text.parquet')) t;

.print ''
.print '=== Sample: HSWA 1974 (new identity columns) ==='
SELECT position, section_id, sort_key, section_type, provision, heading_group
FROM read_parquet('data/legislation_text.parquet')
WHERE law_name = 'UK_ukpga_1974_37'
ORDER BY sort_key, position
LIMIT 20;

.print ''
.print '=== Sample: HSWA 1974 parallel provisions ==='
SELECT position, section_id, sort_key, extent_code, text[1:60] AS text_preview
FROM read_parquet('data/legislation_text.parquet')
WHERE law_name = 'UK_ukpga_1974_37'
  AND section_id LIKE '%[%'
ORDER BY sort_key, position
LIMIT 10;

.print ''
.print '=== Sort key vs position ordering (Environment Act 1995, inserted sections) ==='
SELECT position, section_id, sort_key
FROM read_parquet('data/legislation_text.parquet')
WHERE law_name = 'UK_ukpga_1995_25'
  AND section_type = 'section'
  AND provision IN ('40', '41', '41A', '41B', '41C', '42')
ORDER BY sort_key;

.print ''
.print '=== Sample: annotations for Energy Act 2013 ==='
SELECT code, code_type,
       list_transform(affected_sections, x -> x[1:40]) AS affected_preview,
       text[1:80] AS text_preview
FROM read_parquet('data/amendment_annotations.parquet')
WHERE law_name = 'UK_ukpga_2013_32'
ORDER BY code_type, code
LIMIT 10;
