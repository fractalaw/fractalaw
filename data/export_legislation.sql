-- export_legislation.sql
-- Transforms uk_lrt.jsonl (legacy PostgreSQL export) into Fractalaw Parquet files.
-- Usage: cd fractalaw && duckdb < data/export_legislation.sql
--
-- Outputs:
--   data/legislation.parquet   — 78-column LRT hot path table
--   data/law_edges.parquet     — flattened edge table for analytical path

SET threads = 4;

-- ---------------------------------------------------------------------------
-- 0. Load raw JSONL into a staging table
-- ---------------------------------------------------------------------------

CREATE TABLE raw AS
SELECT *
FROM read_json(
    'data/uk_lrt.jsonl',
    auto_detect = true,
    maximum_object_size = 33554432
);

.print '✓ Loaded raw JSONL'

-- ---------------------------------------------------------------------------
-- 1. Helper macros
-- ---------------------------------------------------------------------------

-- Extract keys from a MAP(VARCHAR, BOOLEAN) as a VARCHAR list.
CREATE MACRO map_keys_to_list(m) AS (
    CASE WHEN m IS NULL THEN NULL
         ELSE map_keys(m)
    END
);

-- Extract year from a law name like "UK_uksi_2024_8"
CREATE MACRO year_from_name(n) AS (
    TRY_CAST(split_part(n, '_', 3) AS INTEGER)
);

-- Normalize status from emoji format to enum
CREATE MACRO normalize_status(s) AS (
    CASE
        WHEN s LIKE '%In force%' THEN 'in_force'
        WHEN s LIKE '%Revoked%' OR s LIKE '%Repealed%' OR s LIKE '%Abolished%' THEN 'revoked'
        WHEN s LIKE '%Part Revocation%' OR s LIKE '%Part Repeal%' THEN 'partial'
        WHEN s LIKE '%Planned%' THEN 'planned'
        WHEN s IS NULL OR trim(s) = '' THEN NULL
        ELSE s
    END
);

-- Strip emoji prefix from family: "💚 AGRICULTURE" → "AGRICULTURE"
CREATE MACRO strip_emoji_prefix(s) AS (
    CASE WHEN s IS NULL THEN NULL
         WHEN length(s) > 2 AND substring(s, 2, 1) = ' '
              AND unicode(s[1]) > 127
              THEN ltrim(substring(s, 3))
         ELSE s
    END
);

-- Extract function keys from the typed struct.
-- function is STRUCT with boolean fields {Revoking, Making, Amending, ...}
-- We want a list of field names where value is true.
CREATE MACRO extract_function_keys(f) AS (
    CASE WHEN f IS NULL THEN NULL ELSE
    list_filter(
        ['Making', 'Amending', 'Amending Maker', 'Commencing',
         'Enacting', 'Enacting Maker', 'Revoking', 'Revoking Maker'],
        x -> CASE x
            WHEN 'Making' THEN f."Making"
            WHEN 'Amending' THEN f."Amending"
            WHEN 'Amending Maker' THEN f."Amending Maker"
            WHEN 'Commencing' THEN f."Commencing"
            WHEN 'Enacting' THEN f."Enacting"
            WHEN 'Enacting Maker' THEN f."Enacting Maker"
            WHEN 'Revoking' THEN f."Revoking"
            WHEN 'Revoking Maker' THEN f."Revoking Maker"
            ELSE false
        END
    ) END
);

-- Build RelatedLaw struct from a simple name list (no stats detail)
CREATE MACRO names_to_related(names) AS (
    CASE WHEN names IS NULL THEN NULL
         ELSE list_transform(names, n -> {
             name: n,
             title: NULL::VARCHAR,
             year: year_from_name(n),
             count: NULL::INTEGER,
             latest_date: NULL::DATE
         })
    END
);

-- Extract DRRPEntry list from duties/rights/responsibilities/powers struct
-- Source: STRUCT(entries STRUCT(clause, holder, article, duty_type)[], ...)
CREATE MACRO extract_drrp_entries(col) AS (
    CASE WHEN col IS NULL OR col.entries IS NULL THEN NULL
         ELSE list_transform(col.entries, e -> {
             holder: e.holder,
             duty_type: e.duty_type,
             clause: e.clause,
             article: e.article
         })
    END
);

.print '✓ Created macros'

-- ---------------------------------------------------------------------------
-- 2. Build RelatedLaw arrays from stats_per_law MAPs
-- ---------------------------------------------------------------------------
-- stats_per_law: MAP(VARCHAR, STRUCT(url, name, count, title, details[]))
-- Target: [{name, title, year, count, latest_date}]

-- 🔺 affects (this law amends others)
CREATE TABLE spl_affects AS
SELECT r.name AS law_name,
       list({
           name: entry.value."name",
           title: entry.value.title,
           year: year_from_name(entry.value."name"),
           count: CAST(entry.value.count AS INTEGER),
           latest_date: NULL::DATE
       }) AS related
FROM raw r,
     LATERAL unnest(map_entries(r."🔺_affects_stats_per_law")) AS t(entry)
WHERE r."🔺_affects_stats_per_law" IS NOT NULL
GROUP BY r.name;

.print '✓ Built spl_affects'

-- 🔻 affected_by (others amend this law)
CREATE TABLE spl_affected_by AS
SELECT r.name AS law_name,
       list({
           name: entry.value."name",
           title: entry.value.title,
           year: year_from_name(entry.value."name"),
           count: CAST(entry.value.count AS INTEGER),
           latest_date: NULL::DATE
       }) AS related
FROM raw r,
     LATERAL unnest(map_entries(r."🔻_affected_by_stats_per_law")) AS t(entry)
WHERE r."🔻_affected_by_stats_per_law" IS NOT NULL
GROUP BY r.name;

.print '✓ Built spl_affected_by'

-- 🔺 rescinding (this law rescinds others)
CREATE TABLE spl_rescinding AS
SELECT r.name AS law_name,
       list({
           name: entry.value."name",
           title: entry.value.title,
           year: year_from_name(entry.value."name"),
           count: CAST(entry.value.count AS INTEGER),
           latest_date: NULL::DATE
       }) AS related
FROM raw r,
     LATERAL unnest(map_entries(r."🔺_rescinding_stats_per_law")) AS t(entry)
WHERE r."🔺_rescinding_stats_per_law" IS NOT NULL
GROUP BY r.name;

.print '✓ Built spl_rescinding'

-- 🔻 rescinded_by (others rescind this law)
CREATE TABLE spl_rescinded_by AS
SELECT r.name AS law_name,
       list({
           name: entry.value."name",
           title: entry.value.title,
           year: year_from_name(entry.value."name"),
           count: CAST(entry.value.count AS INTEGER),
           latest_date: NULL::DATE
       }) AS related
FROM raw r,
     LATERAL unnest(map_entries(r."🔻_rescinded_by_stats_per_law")) AS t(entry)
WHERE r."🔻_rescinded_by_stats_per_law" IS NOT NULL
GROUP BY r.name;

.print '✓ Built spl_rescinded_by'

-- ---------------------------------------------------------------------------
-- 3. Export legislation.parquet
-- ---------------------------------------------------------------------------

COPY (
    SELECT
        -- 1.1 Identity (12)
        r.name,
        'UK' AS jurisdiction,
        'legislation.gov.uk' AS source_authority,
        r.leg_gov_uk_url AS source_url,
        r.type_code,
        r.type_desc,
        r.type_class,
        r.year,
        r.number,
        r.old_style_number,
        r.title_en AS title,
        'en' AS "language",

        -- 1.2 Classification (6)
        r.domain,
        strip_emoji_prefix(r.family) AS family,
        r.family_ii AS sub_family,
        r.si_code."values" AS si_code,
        r.md_description AS description,
        r.md_subjects."values" AS subjects,

        -- 1.3 Dates (9)
        CAST(r.md_date AS DATE) AS primary_date,
        CAST(r.md_made_date AS DATE) AS made_date,
        CAST(r.md_enactment_date AS DATE) AS enactment_date,
        CAST(r.md_coming_into_force_date AS DATE) AS in_force_date,
        CAST(r.md_dct_valid_date AS DATE) AS valid_date,
        CAST(r.md_modified AS DATE) AS modified_date,
        CAST(r.md_restrict_start_date AS DATE) AS restrict_start_date,
        CAST(r.latest_amend_date AS DATE) AS latest_amend_date,
        CAST(r.latest_rescind_date AS DATE) AS latest_rescind_date,

        -- 1.4 Territorial Extent (5)
        r.geo_extent AS extent_code,
        r.geo_region AS extent_regions,
        CASE WHEN r.geo_region IS NOT NULL
                  AND list_sort(r.geo_region) = list_sort(['England','Wales','Scotland','Northern Ireland'])
             THEN true
             WHEN r.geo_extent = 'UK' THEN true
             WHEN r.geo_region IS NULL THEN NULL
             ELSE false
        END AS extent_national,
        r.geo_detail AS extent_detail,
        r.md_restrict_extent AS restrict_extent,

        -- 1.5 Document Statistics (5)
        CAST(r.md_total_paras AS INTEGER) AS total_paras,
        CAST(r.md_body_paras AS INTEGER) AS body_paras,
        CAST(r.md_schedule_paras AS INTEGER) AS schedule_paras,
        CAST(r.md_attachment_paras AS INTEGER) AS attachment_paras,
        CAST(r.md_images AS INTEGER) AS images,

        -- 1.6 Status (4)
        normalize_status(r.live) AS status,
        r.live_source AS status_source,
        r.live_conflict AS status_conflict,
        r.live_conflict_detail AS status_conflict_detail,

        -- 1.7 Function (6)
        extract_function_keys(r.function) AS function,
        r.is_making,
        r.is_commencing,
        r.is_amending,
        r.is_enacting,
        r.is_rescinding,

        -- 1.8 Relationships (6)
        names_to_related(r.enacted_by) AS enacted_by,
        names_to_related(r.enacting) AS enacting,
        COALESCE(sa.related, names_to_related(r.amending)) AS amending,
        COALESCE(sab.related, names_to_related(r.amended_by)) AS amended_by,
        COALESCE(sr.related, names_to_related(r.rescinding)) AS rescinding,
        COALESCE(srb.related, names_to_related(r.rescinded_by)) AS rescinded_by,

        -- 1.8 Amendment Statistics (7)
        CAST(r."🔺🔻_stats_self_affects_count" AS INTEGER) AS self_affects_count,
        CAST(r."🔺_stats_affects_count" AS INTEGER) AS affects_count,
        CAST(r."🔺_stats_affected_laws_count" AS INTEGER) AS affected_laws_count,
        CAST(r."🔻_stats_affected_by_count" AS INTEGER) AS affected_by_count,
        CAST(r."🔻_stats_affected_by_laws_count" AS INTEGER) AS affected_by_laws_count,
        CAST(r."🔺_stats_rescinding_laws_count" AS INTEGER) AS rescinding_laws_count,
        CAST(r."🔻_stats_rescinded_by_laws_count" AS INTEGER) AS rescinded_by_laws_count,

        -- 1.9 DRRP Taxa (11)
        map_keys_to_list(r.duty_holder) AS duty_holder,
        map_keys_to_list(r.rights_holder) AS rights_holder,
        map_keys_to_list(r.responsibility_holder) AS responsibility_holder,
        map_keys_to_list(r.power_holder) AS power_holder,
        r.duty_type."values" AS duty_type,
        r.role,
        map_keys_to_list(r.role_gvt) AS role_gvt,
        extract_drrp_entries(r.duties) AS duties,
        extract_drrp_entries(r.rights) AS rights,
        extract_drrp_entries(r.responsibilities) AS responsibilities,
        extract_drrp_entries(r.powers) AS powers,

        -- 1.10 Annotation Totals (4) — not in legacy data
        NULL::INTEGER AS total_text_amendments,
        NULL::INTEGER AS total_modifications,
        NULL::INTEGER AS total_commencements,
        NULL::INTEGER AS total_extents,

        -- 1.11 Change Logs (1)
        r.record_change_log::VARCHAR AS change_log,

        -- 1.12 Timestamps (2)
        CAST(r.created_at AS TIMESTAMP) AS created_at,
        CAST(r.updated_at AS TIMESTAMP) AS updated_at

    FROM raw r
    LEFT JOIN spl_affects sa ON sa.law_name = r.name
    LEFT JOIN spl_affected_by sab ON sab.law_name = r.name
    LEFT JOIN spl_rescinding sr ON sr.law_name = r.name
    LEFT JOIN spl_rescinded_by srb ON srb.law_name = r.name
)
TO 'data/legislation.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '✓ Exported legislation.parquet'

-- ---------------------------------------------------------------------------
-- 4. Export law_edges.parquet
-- ---------------------------------------------------------------------------

COPY (
    -- Amends edges (from 🔺_affects_stats_per_law)
    SELECT
        r.name AS source_name,
        entry.value."name" AS target_name,
        'amends' AS edge_type,
        'UK' AS jurisdiction,
        detail.target AS article_target,
        detail.affect AS affect_type,
        detail.applied AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(map_entries(r."🔺_affects_stats_per_law")) AS t(entry),
         LATERAL unnest(entry.value.details) AS t2(detail)
    WHERE r."🔺_affects_stats_per_law" IS NOT NULL

    UNION ALL

    -- Amended_by edges (from 🔻_affected_by_stats_per_law)
    SELECT
        entry.value."name" AS source_name,
        r.name AS target_name,
        'amended_by' AS edge_type,
        'UK' AS jurisdiction,
        detail.target AS article_target,
        detail.affect AS affect_type,
        detail.applied AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(map_entries(r."🔻_affected_by_stats_per_law")) AS t(entry),
         LATERAL unnest(entry.value.details) AS t2(detail)
    WHERE r."🔻_affected_by_stats_per_law" IS NOT NULL

    UNION ALL

    -- Rescinds edges (from 🔺_rescinding_stats_per_law)
    SELECT
        r.name AS source_name,
        entry.value."name" AS target_name,
        'rescinds' AS edge_type,
        'UK' AS jurisdiction,
        detail.target AS article_target,
        detail.affect AS affect_type,
        detail.applied AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(map_entries(r."🔺_rescinding_stats_per_law")) AS t(entry),
         LATERAL unnest(entry.value.details) AS t2(detail)
    WHERE r."🔺_rescinding_stats_per_law" IS NOT NULL

    UNION ALL

    -- Rescinded_by edges (from 🔻_rescinded_by_stats_per_law)
    SELECT
        entry.value."name" AS source_name,
        r.name AS target_name,
        'rescinded_by' AS edge_type,
        'UK' AS jurisdiction,
        detail.target AS article_target,
        detail.affect AS affect_type,
        detail.applied AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(map_entries(r."🔻_rescinded_by_stats_per_law")) AS t(entry),
         LATERAL unnest(entry.value.details) AS t2(detail)
    WHERE r."🔻_rescinded_by_stats_per_law" IS NOT NULL

    UNION ALL

    -- Enacted_by edges (from enacted_by array — law-level only)
    SELECT
        eb AS source_name,
        r.name AS target_name,
        'enacted_by' AS edge_type,
        'UK' AS jurisdiction,
        NULL AS article_target,
        NULL AS affect_type,
        NULL AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(r.enacted_by) AS t(eb)
    WHERE r.enacted_by IS NOT NULL

    UNION ALL

    -- Enacts edges (from enacting array — law-level only)
    SELECT
        r.name AS source_name,
        en AS target_name,
        'enacts' AS edge_type,
        'UK' AS jurisdiction,
        NULL AS article_target,
        NULL AS affect_type,
        NULL AS applied_status,
        NULL::DATE AS date
    FROM raw r,
         LATERAL unnest(r.enacting) AS t(en)
    WHERE r.enacting IS NOT NULL
)
TO 'data/law_edges.parquet' (FORMAT PARQUET, COMPRESSION ZSTD);

.print '✓ Exported law_edges.parquet'

-- ---------------------------------------------------------------------------
-- 5. Verify
-- ---------------------------------------------------------------------------

SELECT '── legislation.parquet ──' AS "";
SELECT count(*) AS rows,
       count(DISTINCT name) AS distinct_laws,
       count(status) AS has_status,
       count(amended_by) AS has_amended_by,
       count(duties) AS has_duties
FROM read_parquet('data/legislation.parquet');

SELECT '── law_edges.parquet ──' AS "";
SELECT count(*) AS total_edges,
       count(DISTINCT edge_type) AS edge_types
FROM read_parquet('data/law_edges.parquet');

SELECT edge_type, count(*) AS cnt
FROM read_parquet('data/law_edges.parquet')
GROUP BY edge_type
ORDER BY cnt DESC;

SELECT '── File sizes ──' AS "";
