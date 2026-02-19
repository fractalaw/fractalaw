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
CREATE MACRO build_hierarchy(flow_val, part_val, chap_val, head_val, sec_val, sub_val, para_val) AS (
    concat_ws('/',
        CASE WHEN flow_val NOT IN ('pre', 'main', 'post', 'signed', '') AND flow_val IS NOT NULL
             THEN 'schedule.' || flow_val ELSE NULL END,
        CASE WHEN part_val IS NOT NULL AND part_val != '' THEN 'part.' || part_val ELSE NULL END,
        CASE WHEN chap_val IS NOT NULL AND chap_val != '' THEN 'chapter.' || chap_val ELSE NULL END,
        CASE WHEN head_val IS NOT NULL AND head_val != '' THEN 'heading.' || head_val ELSE NULL END,
        CASE WHEN sec_val IS NOT NULL AND sec_val != '' THEN 'section.' || sec_val ELSE NULL END,
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

.print '--- Created macros'

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
-- 3. Export legislation_text.parquet
-- ---------------------------------------------------------------------------

COPY (
    WITH content_rows AS (
        SELECT *,
            row_number() OVER (
                PARTITION BY "UK"
                ORDER BY rowid
            ) AS pos
        FROM lat_raw
        WHERE is_content_row(Record_Type)
    )
    SELECT
        -- 3.1 Identity & Position
        strip_acronym(cr."UK") AS law_name,
        CAST(cr.pos AS INTEGER) AS position,
        strip_id(cr."ID", cr."UK") AS section_id,
        map_section_type(cr.Record_Type) AS section_type,
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

        -- 3.2 Structural Hierarchy
        cr."Part" AS part,
        cr."Chapter" AS chapter,
        cr."Heading" AS heading,
        CASE WHEN cr."Class" = 'Regulation' THEN NULL ELSE cr."Section||Regulation" END AS section,
        CASE WHEN cr."Class" = 'Regulation' THEN cr."Section||Regulation" ELSE NULL END AS article,
        cr."Paragraph" AS paragraph,
        cr."Sub_Section||Sub_Regulation" AS sub_paragraph,
        CASE WHEN cr.flow NOT IN ('pre','main','post','signed','') AND cr.flow IS NOT NULL
             THEN cr.flow ELSE NULL END AS schedule,

        -- 3.4 Content
        cr."Text" AS text,
        'en' AS "language",
        region_to_extent(cr."Region") AS extent_code,

        -- 3.5 Amendment Annotations
        CAST(count_codes(cr."Changes", 'F') AS INTEGER) AS amendment_count,
        COALESCE(cie.modification_count,  NULL::INTEGER) AS modification_count,
        COALESCE(cie.commencement_count,  NULL::INTEGER) AS commencement_count,
        COALESCE(cie.extent_count,        NULL::INTEGER) AS extent_count,
        COALESCE(cie.editorial_count,     NULL::INTEGER) AS editorial_count,

        -- 3.6 Embeddings (null — populated in later phase)
        NULL::FLOAT[] AS embedding,
        NULL::VARCHAR AS embedding_model,
        NULL::TIMESTAMPTZ AS embedded_at,

        -- 3.7 Metadata
        current_timestamp AS created_at,
        current_timestamp AS updated_at

    FROM content_rows cr
    LEFT JOIN cie_counts cie ON cie.parent_id = cr."ID"
    ORDER BY strip_acronym(cr."UK"), cr.pos
)
TO 'data/legislation_text.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '--- Exported legislation_text.parquet'

-- ---------------------------------------------------------------------------
-- 4. Build affected_sections for annotations
-- ---------------------------------------------------------------------------

-- 4a. F-code affected_sections from LAT Changes column inversion.
CREATE TABLE f_affected_lat AS
SELECT
    "UK" AS law_name,
    trim(code_ref) AS code,
    list(DISTINCT strip_id("ID", "UK") ORDER BY strip_id("ID", "UK")) AS affected_sections
FROM lat_raw,
     LATERAL unnest(string_split("Changes", ',')) AS t(code_ref)
WHERE "Changes" IS NOT NULL
  AND "Changes" != ''
  AND trim(code_ref) != ''
  AND is_content_row(Record_Type)
GROUP BY "UK", trim(code_ref);

.print '--- Built F-code affected_sections from LAT'

-- 4b. F-code affected_sections from AMD Articles column.
--     Articles = comma-separated section IDs (with acronyms).
CREATE TABLE f_affected_amd AS
SELECT
    regexp_replace("ID", '_[FCIE][0-9]+$', '') AS law_name,
    "Ef Code" AS code,
    list(DISTINCT strip_id(trim(article_ref), regexp_replace("ID", '_[FCIE][0-9]+$', ''))
         ORDER BY strip_id(trim(article_ref), regexp_replace("ID", '_[FCIE][0-9]+$', ''))
    ) AS affected_sections
FROM amd_raw,
     LATERAL unnest(string_split("Articles", ',')) AS t(article_ref)
WHERE "Articles" IS NOT NULL
  AND "Articles" != ''
  AND trim(article_ref) != ''
  AND "Ef Code" IS NOT NULL
GROUP BY regexp_replace("ID", '_[FCIE][0-9]+$', ''), "Ef Code";

.print '--- Built F-code affected_sections from AMD'

-- ---------------------------------------------------------------------------
-- 5. Export amendment_annotations.parquet
-- ---------------------------------------------------------------------------
-- Three sources combined:
--   1. C/I/E annotations from LAT (commencement,content / modification,content / extent,content)
--   2. F-code annotations from LAT (amendment,general / amendment,textual / etc.)
--   3. F-code annotations from AMD (AMD-*.csv)

COPY (
    -- Source 1: C/I/E annotations from LAT
    SELECT
        strip_id(r."ID", r."UK") AS id,
        strip_acronym(r."UK") AS law_name,
        COALESCE(NULLIF(extract_code(r."Text"), ''), map_code_type(r.Record_Type)[1:1] || '0') AS code,
        map_code_type(r.Record_Type) AS code_type,
        r."Text" AS text,
        [strip_id(annotation_parent_id(r."ID"), r."UK")] AS affected_sections,
        current_timestamp AS created_at,
        current_timestamp AS updated_at
    FROM lat_raw r
    WHERE r.Record_Type IN ('modification,content', 'commencement,content',
                            'extent,content', 'editorial,content', 'subordinate,content')

    UNION ALL

    -- Source 2: F-code annotations from LAT
    SELECT
        strip_id(r."ID", r."UK") AS id,
        strip_acronym(r."UK") AS law_name,
        COALESCE(NULLIF(extract_code(r."Text"), ''), 'F' || r."Amendment") AS code,
        'amendment' AS code_type,
        r."Text" AS text,
        fa.affected_sections,
        current_timestamp AS created_at,
        current_timestamp AS updated_at
    FROM lat_raw r
    LEFT JOIN f_affected_lat fa ON fa.law_name = r."UK"
        AND fa.code = COALESCE(NULLIF(extract_code(r."Text"), ''), 'F' || r."Amendment")
    WHERE r.Record_Type LIKE 'amendment,%'

    UNION ALL

    -- Source 3: F-code annotations from AMD
    SELECT
        strip_id(a."ID", regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) AS id,
        strip_acronym(regexp_replace(a."ID", '_[FCIE][0-9]+$', '')) AS law_name,
        a."Ef Code" AS code,
        'amendment' AS code_type,
        a."Text" AS text,
        fa.affected_sections,
        current_timestamp AS created_at,
        current_timestamp AS updated_at
    FROM amd_raw a
    LEFT JOIN f_affected_amd fa ON fa.law_name = regexp_replace(a."ID", '_[FCIE][0-9]+$', '')
        AND fa.code = a."Ef Code"
    WHERE a."Ef Code" IS NOT NULL

    ORDER BY law_name, code_type, code
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
        GROUP BY strip_acronym("UK")
    ),
    amd_totals AS (
        SELECT strip_acronym(regexp_replace("ID", '_[FCIE][0-9]+$', '')) AS name,
               CAST(count(*) AS INTEGER) AS amd_text_amendments
        FROM amd_raw
        WHERE "Ef Code" IS NOT NULL
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
       count(extent_code) AS has_extent
FROM read_parquet('data/legislation_text.parquet');

SELECT section_type, count(*) AS cnt
FROM read_parquet('data/legislation_text.parquet')
GROUP BY section_type
ORDER BY cnt DESC;

.print ''
.print '=== Acronym stripping verification ==='
SELECT count(*) AS laws_with_acronym
FROM (SELECT DISTINCT law_name FROM read_parquet('data/legislation_text.parquet')
      WHERE length(law_name) - length(replace(law_name, '_', '')) > 3);

SELECT count(*) AS section_ids_with_acronym
FROM read_parquet('data/legislation_text.parquet')
WHERE regexp_matches(section_id, '_[A-Z]{2,}_');

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
.print '=== Sample: HSWA 1974 ==='
SELECT position, section_type, hierarchy_path,
       amendment_count AS f, modification_count AS c,
       commencement_count AS i, extent_count AS e,
       text[1:80] AS text_preview
FROM read_parquet('data/legislation_text.parquet')
WHERE law_name = 'UK_ukpga_1974_37'
ORDER BY position
LIMIT 15;

.print ''
.print '=== Sample: annotations for Energy Act 2013 ==='
SELECT code, code_type,
       list_transform(affected_sections, x -> x[1:40]) AS affected_preview,
       text[1:80] AS text_preview
FROM read_parquet('data/amendment_annotations.parquet')
WHERE law_name = 'UK_ukpga_2013_32'
ORDER BY code_type, code
LIMIT 10;
