# Fractalaw Schema Design

**Version**: 0.3
**Date**: 2026-02-19
**Status**: All four tables exported to Parquet and validated

This document defines the three-tier data model for Fractalaw. It is the spec from which `fractalaw-core/src/schema.rs` will be implemented.

**Legacy reference**: [sertantai-legal LRT-SCHEMA.md](https://github.com/shotleybuilder/sertantai-legal/blob/main/docs/LRT-SCHEMA.md) (~80 columns, UK-only, PostgreSQL)
**Prototype reference**: [legl project](https://github.com/shotleybuilder/legl/tree/main/lib/legl/countries) (multi-country Airtable prototype)

---

## Architecture: Graph-Dense Columnar

Four tables across three access tiers, each optimised for a different query pattern:

| Tier | Table | Store | Access Pattern |
|------|-------|-------|---------------|
| **Hot path** | `legislation` (LRT) | DuckDB | Single-law lookup, metadata filter, relationship context — no joins needed |
| **Analytical path** | `law_edges` | DuckDB | Multi-hop graph traversal, amendment network analysis — vectorised joins |
| **Semantic path** | `legislation_text` (LAT) | LanceDB | Full-text search, embedding similarity, RAG retrieval |
| **Semantic path** | `amendment_annotations` | LanceDB | Amendment/modification/commencement detail linked to LAT sections |

**Guiding principle**: no pointer chasing. The hot path carries denormalized `List<Struct>` relationship arrays so the CPU reads one memory block per law. The analytical path is a flattened edge table for DuckDB's morsel-driven parallel joins. The semantic path stores text with embedding vectors for AI inference.

---

## Table 1: `legislation` — Legal Register Table (LRT) — Hot Path

One row per law. Contains all metadata plus denormalized relationship context.

The LRT is the **source of truth** — it's the scraping target where records are built up progressively. The edge table is derived from it.

### 1.1 Identity

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `name` | Utf8 | no | Unique law identifier. Format: `{JURISDICTION}_{type_code}_{year}_{number}` | `name` |
| `jurisdiction` | Utf8 | no | Country/region code: `UK`, `DE`, `NO`, `FIN`, `SWE`, `TUR`, `RUS`, `AUT`, `DK`, `EU`, `AU`, `NZ`, `FR` | *new* (implicit `UK` in legacy) |
| `source_authority` | Utf8 | no | Canonical legislation source for this jurisdiction | *new* |
| `source_url` | Utf8 | yes | Canonical URL at source authority | `leg_gov_uk_url` (was generated) |
| `type_code` | Utf8 | no | Jurisdiction-specific instrument code | `type_code` |
| `type_desc` | Utf8 | yes | Human-readable type description | `type_desc` |
| `type_class` | Utf8 | yes | Generalised class: `Primary`, `Secondary`, `EU_Directive`, `EU_Regulation` | `type_class` |
| `year` | Int32 | no | Year of enactment/publication | `year` |
| `number` | Utf8 | no | Instrument number (string — some have letters) | `number` |
| `old_style_number` | Utf8 | yes | Legacy numbering (UK-specific, e.g. `Eliz2/10-11/19`) | `old_style_number` |
| `title` | Utf8 | yes | Title in primary language | `title_en` |
| `language` | Utf8 | no | Primary language: `en`, `de`, `fr`, `no`, `sv`, `fi`, `tr`, `ru` | *new* (implicit `en`) |

**Source authorities by jurisdiction:**

| Jurisdiction | Source Authority | URL Pattern |
|-------------|----------------|-------------|
| UK | legislation.gov.uk | `https://www.legislation.gov.uk/{type_code}/{year}/{number}` |
| DE | gesetze-im-internet.de | `https://www.gesetze-im-internet.de/` |
| NO | lovdata.no | `https://lovdata.no/dokument/SF/forskrift/` |
| FIN | finlex.fi | `https://www.finlex.fi/` |
| SWE | rkrattsbaser.gov.se | `http://rkrattsbaser.gov.se/` |
| TUR | mevzuat.gov.tr | `https://www.mevzuat.gov.tr/` |
| RUS | pravo.gov.ru | `https://www.pravo.gov.ru/` |
| DK | retsinformation.dk | `https://www.retsinformation.dk/` |
| AUT | ris.bka.gv.at | `https://www.ris.bka.gv.at/` |
| EU | eur-lex.europa.eu | `https://eur-lex.europa.eu/` |
| AU | legislation.gov.au | `https://www.legislation.gov.au/` |
| NZ | legislation.govt.nz | `https://www.legislation.govt.nz/` |
| FR | legifrance.gouv.fr | `https://www.legifrance.gouv.fr/` |

### 1.2 Classification

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `domain` | List\<Utf8\> | yes | ESH domain tags: `environment`, `health_safety`, `planning`, etc. | `domain` (text[]) |
| `family` | Utf8 | yes | Legislation family grouping | `family` |
| `sub_family` | Utf8 | yes | Sub-family | `family_ii` |
| `si_code` | List\<Utf8\> | yes | Subject index codes (UK-specific) | `si_code` (JSONB → values[]) |
| `description` | Utf8 | yes | Summary description | `md_description` |
| `subjects` | List\<Utf8\> | yes | Subject keywords. Sourced from legislation.gov.uk metadata (1,155 distinct values across 7,908 UK records, ~71% coverage 1987–2012, tagging stopped mid-2013 with signs of resumption in 2025). Provides cross-cutting themes spanning multiple families (e.g., "pollution" across 20 families) and sub-family granularity. Post-2013 gap to be filled via LLM classification in a later phase. See [md-subjects-analysis](https://github.com/shotleybuilder/sertantai-legal/blob/main/docs/md-subjects-analysis.md). | `md_subjects` (JSONB → values[]) |

### 1.3 Dates

All date columns use Arrow `Date32` (days since epoch — sufficient for legislation, no time component needed).

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `primary_date` | Date32 | yes | Primary date of the instrument | `md_date` |
| `made_date` | Date32 | yes | Date instrument was made/signed | `md_made_date` |
| `enactment_date` | Date32 | yes | Date of enactment (primary legislation) | `md_enactment_date` |
| `in_force_date` | Date32 | yes | Date instrument came into force | `md_coming_into_force_date` |
| `valid_date` | Date32 | yes | DCT valid date | `md_dct_valid_date` |
| `modified_date` | Date32 | yes | Last modified at source | `md_modified` |
| `restrict_start_date` | Date32 | yes | Restriction start date | `md_restrict_start_date` |
| `latest_amend_date` | Date32 | yes | Date of most recent amendment to this law | `latest_amend_date` |
| `latest_rescind_date` | Date32 | yes | Date of most recent rescission of this law | `latest_rescind_date` |

**Dropped derived columns** — computed at query time via `year()` / `month()` functions in DuckDB/DataFusion:
- ~~`md_date_year`~~, ~~`md_date_month`~~ → `year(primary_date)`, `month(primary_date)`
- ~~`latest_amend_date_year`~~, ~~`latest_amend_date_month`~~ → `year(latest_amend_date)`
- ~~`latest_rescind_date_year`~~, ~~`latest_rescind_date_month`~~ → `year(latest_rescind_date)`
- ~~`latest_change_date_year`~~, ~~`latest_change_date_month`~~ → dropped (was unused in legacy)

### 1.4 Territorial Extent

A two-layer model capturing where legislation applies. Laws may apply at national level or to specific sub-national regions.

**Layer 1: Sub-national regions** — the administrative divisions within a jurisdiction where a law applies.
**Layer 2: National** — whether the law applies to the whole jurisdiction or only parts.

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `extent_code` | Utf8 | yes | Compact extent code. Jurisdiction-specific shorthand: `E+W+S+NI`, `E+W`, `Bayern`, `NSW+VIC`, `national` | `geo_extent` |
| `extent_regions` | List\<Utf8\> | yes | Resolved sub-national regions this law applies to | `geo_region` (text[]) |
| `extent_national` | Boolean | yes | `true` if the law applies to the entire jurisdiction. `false` if it applies only to specific regions. | *new* (derived: true when all regions present) |
| `extent_detail` | Utf8 | yes | Human-readable description of territorial application, including any provisioning (e.g., "All provisions except Part 3 which applies to England only") | `geo_detail` |
| `restrict_extent` | Utf8 | yes | Restriction on territorial extent | `md_restrict_extent` |

**Sub-national regions by jurisdiction:**

| Jurisdiction | Layer 1 Regions | Example `extent_code` |
|-------------|----------------|----------------------|
| UK | England (E), Wales (W), Scotland (S), Northern Ireland (NI) | `E+W+S+NI`, `E+W`, `S` |
| DE | 16 Bundeslander (Bayern, Nordrhein-Westfalen, Berlin, etc.) | `national`, `Bayern` |
| AU | States/Territories (NSW, VIC, QLD, WA, SA, TAS, ACT, NT) | `national`, `NSW+VIC` |
| NZ | — (unitary state, no sub-national layer) | `national` |
| EU | Member states (for directives/regulations with variable transposition) | `national`, specific member states |
| NO | — (generally national application) | `national` |
| FR | Metropolitan France, overseas departments/territories | `national`, `Martinique` |

Laws in jurisdictions without meaningful sub-national variation (e.g., NZ, NO) will have `extent_national = true` and `extent_regions = null`.

### 1.5 Document Statistics

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `total_paras` | Int32 | yes | Total paragraphs in the instrument | `md_total_paras` |
| `body_paras` | Int32 | yes | Body paragraphs | `md_body_paras` |
| `schedule_paras` | Int32 | yes | Schedule/annex paragraphs | `md_schedule_paras` |
| `attachment_paras` | Int32 | yes | Attachment paragraphs | `md_attachment_paras` |
| `images` | Int32 | yes | Image count | `md_images` |

### 1.6 Status

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `status` | Utf8 | yes | Enforcement status: `in_force`, `revoked`, `repealed`, `partial` | `live` (normalised from emoji format) |
| `status_source` | Utf8 | yes | How status was determined: `metadata`, `changes`, `both` | `live_source` |
| `status_conflict` | Boolean | yes | Whether status sources disagreed | `live_conflict` |
| `status_conflict_detail` | Utf8 | yes | JSON string with conflict reconciliation detail | `live_conflict_detail` (JSONB) |

### 1.7 Function

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `function` | List\<Utf8\> | yes | Functional roles: `Making`, `Amending Maker`, `Commencing`, etc. | `function` (JSONB keys → list) |
| `is_making` | Boolean | yes | This law makes/creates new provisions | `is_making` |
| `is_commencing` | Boolean | yes | This law commences provisions of another | `is_commencing` |
| `is_amending` | Boolean | yes | This law amends other laws | `is_amending` |
| `is_enacting` | Boolean | yes | This law enacts other laws | `is_enacting` |
| `is_rescinding` | Boolean | yes | This law rescinds/revokes/repeals other laws | `is_rescinding` |

### 1.8 Relationships — Denormalized Immediate Context

These are the **hot path** columns. Each is a `List<Struct>` carrying enough context about related laws to answer the common query without a join.

**Relationship struct fields:**

| Struct Field | Arrow Type | Description |
|-------------|-----------|-------------|
| `name` | Utf8 | Related law identifier (FK to LRT `legislation.name`) |
| `title` | Utf8 | Related law title (denormalized) |
| `year` | Int32 | Related law year (denormalized) |
| `count` | Int32 | Number of individual changes (amendments/rescissions) |
| `latest_date` | Date32 | Date of most recent change |

This struct is referred to as `RelatedLaw` below.

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `enacted_by` | List\<RelatedLaw\> | yes | Laws that enacted this law (parent acts) | `enacted_by` + `linked_enacted_by` + `enacted_by_meta` |
| `enacting` | List\<RelatedLaw\> | yes | Laws that this law enacts | `enacting` |
| `amending` | List\<RelatedLaw\> | yes | Laws that this law amends (🔺 this → others) | `amending` + `linked_amending` + `🔺_affects_stats_per_law` |
| `amended_by` | List\<RelatedLaw\> | yes | Laws that amend this law (🔻 others → this) | `amended_by` + `linked_amended_by` + `🔻_affected_by_stats_per_law` |
| `rescinding` | List\<RelatedLaw\> | yes | Laws that this law rescinds (🔺 this → others) | `rescinding` + `linked_rescinding` + `🔺_rescinding_stats_per_law` |
| `rescinded_by` | List\<RelatedLaw\> | yes | Laws that rescind this law (🔻 others → this) | `rescinded_by` + `linked_rescinded_by` + `🔻_rescinded_by_stats_per_law` |

**Amendment statistics** (summary counts from the `List<Struct>` arrays):

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `self_affects_count` | Int32 | yes | Count of self-amendments | `🔺🔻_stats_self_affects_count` |
| `affects_count` | Int32 | yes | Total amendments this law makes to others | `🔺_stats_affects_count` |
| `affected_laws_count` | Int32 | yes | Count of distinct laws this law amends | `🔺_stats_affected_laws_count` |
| `affected_by_count` | Int32 | yes | Total amendments made to this law by others | `🔻_stats_affected_by_count` |
| `affected_by_laws_count` | Int32 | yes | Count of distinct laws that amend this law | `🔻_stats_affected_by_laws_count` |
| `rescinding_laws_count` | Int32 | yes | Count of laws this law rescinds | `🔺_stats_rescinding_laws_count` |
| `rescinded_by_laws_count` | Int32 | yes | Count of laws that rescind this law | `🔻_stats_rescinded_by_laws_count` |

> **Note**: These summary counts are denormalized from the `List<Struct>` arrays for fast filtering (e.g., "show laws with > 10 amendments"). They could be computed at query time via `list_length(amended_by)` but storing them avoids repeated computation on 19K+ rows.

### 1.9 DRRP Taxa (Duties, Rights, Responsibilities, Powers)

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `duty_holder` | List\<Utf8\> | yes | Duty holder types: `Public`, `Ind: Person`, etc. | `duty_holder` (JSONB keys) |
| `rights_holder` | List\<Utf8\> | yes | Rights holder types | `rights_holder` (JSONB keys) |
| `responsibility_holder` | List\<Utf8\> | yes | Responsibility holder types | `responsibility_holder` (JSONB keys) |
| `power_holder` | List\<Utf8\> | yes | Power holder types | `power_holder` (JSONB keys) |
| `duty_type` | List\<Utf8\> | yes | Duty types: `Duty`, `Responsibility`, `Power`, etc. | `duty_type` (JSONB → values[]) |
| `role` | List\<Utf8\> | yes | Roles assigned by this law | `role` (text[]) |
| `role_gvt` | List\<Utf8\> | yes | Government roles: `Gvt: Minister`, `Gvt: Authority` | `role_gvt` (JSONB keys) |

**DRRP detail** — the full holder/article/clause entries from consolidated JSONB:

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `duties` | List\<DRRPEntry\> | yes | Duty entries with holder, clause, article | `duties` (JSONB) |
| `rights` | List\<DRRPEntry\> | yes | Rights entries | `rights` (JSONB) |
| `responsibilities` | List\<DRRPEntry\> | yes | Responsibility entries | `responsibilities` (JSONB) |
| `powers` | List\<DRRPEntry\> | yes | Power entries | `powers` (JSONB) |

**DRRPEntry struct:**

| Struct Field | Arrow Type | Description |
|-------------|-----------|-------------|
| `holder` | Utf8 | Holder type: `Ind: Person`, `Public`, `Gvt: Minister` |
| `duty_type` | Utf8 | `DUTY`, `RIGHT`, `RESPONSIBILITY`, `POWER` |
| `clause` | Utf8 | Text excerpt from the law |
| `article` | Utf8 | Article reference: `regulation/4`, `section/12` |

### 1.10 Annotation Totals

Denormalized counts of F/C/I/E annotation codes aggregated across all LAT sections for this law. Avoids a LAT aggregation query for common filters like "show the most heavily amended laws."

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `total_text_amendments` | Int32 | yes | Total F-code (textual amendment) annotations across all sections | *new* (sum of LAT `amendment_count`) |
| `total_modifications` | Int32 | yes | Total C-code (modification) annotations | *new* (sum of LAT `modification_count`) |
| `total_commencements` | Int32 | yes | Total I-code (commencement) annotations | *new* (sum of LAT `commencement_count`) |
| `total_extents` | Int32 | yes | Total E-code (extent/editorial) annotations | *new* (sum of LAT `extent_count` + `editorial_count`) |

> **Note**: These are derived from the LAT annotation counts at import time. A heavily amended law like the Health and Safety at Work etc. Act 1974 may have hundreds of F-codes; this surfaces that on the hot path without touching LanceDB.

### 1.11 Change Logs

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `change_log` | Utf8 | yes | JSON string of record change history | `record_change_log` (JSONB) |

**Dropped**: `amending_change_log`, `amended_by_change_log` — operational scraper state, not needed in the analytical store.

### 1.12 Timestamps

| Column | Arrow Type | Nullable | Description | Legacy (`uk_lrt`) |
|--------|-----------|----------|-------------|-------------------|
| `created_at` | Timestamp(ns, UTC) | no | Record creation time | `created_at` |
| `updated_at` | Timestamp(ns, UTC) | no | Last update time | `updated_at` |

### 1.13 Dropped Columns

These legacy columns are not carried forward:

| Legacy Column | Reason |
|--------------|--------|
| `number_int` | Generated from `number`. Use `CAST` or DataFusion UDF at query time. |
| `leg_gov_uk_url` | Replaced by `source_url`. No longer generated — stored explicitly. |
| `acronym` | Legacy field. Not useful cross-jurisdiction. |
| `tags` | Derived from title. Redundant with modern search/embeddings. Scales poorly with multi-language. |
| `md_date_year`, `md_date_month` | Derived. Use `year(primary_date)` in queries. |
| `latest_amend_date_year`, `latest_amend_date_month` | Derived. Use `year(latest_amend_date)`. |
| `latest_change_date`, `latest_change_date_year`, `latest_change_date_month` | Unused in legacy (no data). |
| `latest_rescind_date_year`, `latest_rescind_date_month` | Derived. Use `year(latest_rescind_date)`. |
| `live_description` | Duplicate of information available in `affected_by_stats_per_law`. |
| `popimar`, `popimar_details` | POPIMAR model deferred. May reintroduce in a future phase. |
| `article_role`, `role_article`, `role_gvt_article`, `article_role_gvt` | Article-level text cross-references. Move to LAT. |
| `duty_type_article`, `article_duty_type` | Covered by `duties`, `rights`, `responsibilities`, `powers` (§1.9). Drop. |
| `🔺🔻_stats_self_affects_count_per_law_detailed` | Replaced by `self_affects_count` + detail in `law_edges`. |

---

## Table 2: `law_edges` (Analytical Path)

Flattened edge table derived from the LRT's `List<Struct>` relationship columns. Each row represents one directional relationship between two laws, with article-level detail where available.

This is the **search index** for graph traversal. Rebuilt from the LRT on import.

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `source_name` | Utf8 | no | Law making the change (FK to `legislation.name`) |
| `target_name` | Utf8 | no | Law being changed (FK to `legislation.name`) |
| `edge_type` | Utf8 | no | Relationship type (see enum below) |
| `jurisdiction` | Utf8 | no | Jurisdiction of the source law |
| `article_target` | Utf8 | yes | Specific article/regulation affected: `reg. 1(2)`, `section 5(3)(a)` |
| `affect_type` | Utf8 | yes | Type of change: `words substituted`, `inserted`, `omitted`, `repealed`, `revoked` |
| `applied_status` | Utf8 | yes | Whether the change has been applied: `Yes`, `Not yet`, `In part` |
| `date` | Date32 | yes | Date of the change (if known) |

**Edge types:**

| `edge_type` value | Direction | Description |
|-------------------|-----------|-------------|
| `enacted_by` | target enacted by source | Source is the parent act that enabled target |
| `enacts` | source enacts target | Source creates/enables target |
| `amends` | source amends target | Source makes changes to target (🔺) |
| `amended_by` | target is amended by source | Target receives changes from source (🔻) |
| `rescinds` | source rescinds target | Source revokes/repeals target (🔺) |
| `rescinded_by` | target is rescinded by source | Target is revoked/repealed by source (🔻) |

**Derivation from LRT:**

Each entry in a `List<Struct>` relationship column expands to one or more edge rows:
- `legislation.amending[i]` → one edge per item, plus one edge per detail in the corresponding `*_stats_per_law` JSONB
- When `*_stats_per_law` detail is available, each `{target, affect, applied}` entry becomes a separate edge row (article-level granularity)
- When only the law-level relationship exists (no article detail), a single edge row is created with `article_target = NULL`

**Expected row counts** (from legacy ~19K LRT records):
- ~9,800 amending relationships + article-level detail rows
- ~6,300 amended_by relationships + article-level detail rows
- ~2,400 rescinding relationships
- ~5,700 rescinded_by relationships
- ~8,300 enacted_by relationships
- Estimated total: 30K-100K edge rows (depending on article-level expansion)

---

## Table 3: `legislation_text` — Legal Article Table (LAT) — Semantic Path

One row per structural unit of legal text. Lives in LanceDB for semantic search and embedding similarity.

This table is the Fractalaw evolution of the [legl Airtable prototype](https://github.com/shotleybuilder/legl). Each row represents one addressable unit of legislation text — an article, section, paragraph, schedule entry, etc. — positioned within the document's structural hierarchy.

### 3.1 Identity & Position

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `law_name` | Utf8 | no | Parent law identifier (FK to LRT `legislation.name`). Acronyms stripped from legacy Airtable IDs (see ID normalization below). |
| `section_id` | Utf8 | no | **Structural citation** — the canonical legal address of this provision. Format: `{law_name}:{citation}[{extent}]`. Stable across amendments — parliament assigns unique citations that never change. Examples: `UK_ukpga_1974_37:s.25A(1)`, `UK_uksi_2002_2677:reg.2A(1)(b)`, `UK_ukpga_1974_37:s.23[E+W]`. See design note below. |
| `sort_key` | Utf8 | no | **Normalised sort encoding** — machine-sortable string that respects legislative insertion ordering. `ORDER BY sort_key` recovers correct document order within a law. Derived from `section_id` citation. See design note below. |
| `position` | Int32 | no | **Snapshot document order index.** Monotonically increasing integer (1-based) preserving the published order of sections within a law at export time. Useful for range queries. Reassigned on re-export — not an identifier. |
| `section_type` | Utf8 | no | Structural type — see enum below |
| `hierarchy_path` | Utf8 | yes | Slash-separated path in document structure: `part.1/heading.2/section.3/sub.1`. NULL for root-level rows (e.g., `title`). |
| `depth` | Int32 | no | Count of populated structural hierarchy levels (0 = title/root, 1 = part, 2 = heading within part, etc.). |

> **Design note — three-column identity**: The `section_id` is a structural citation derived from parliament's own canonical addressing scheme. "Section 41A of the Environment Act 1995" never changes — even when further amendments insert 41B, 41C, or 41ZA. This is stable across amendments, unlike an integer position which requires renumbering when sections are inserted. The `sort_key` normalises the citation into a lexicographically-sortable format (e.g., `s.3` → `003.000.000~`, `s.3ZA` → `003.001.000~`, `s.3A` → `003.010.000~`). The `position` column remains as a convenience integer for fast range scans.
>
> When a law has parallel territorial provisions (same section number with different text for different regions — 29 laws, 719 rows in the UK dataset), the `section_id` includes an extent qualifier: `s.23[E+W]`, `s.23[NI]`, `s.23[S]`. Sections with a single territorial version (the common case) have no qualifier.

> **Design note — ID normalization**: Legacy Airtable IDs carry acronym suffixes/prefixes (e.g., `UK_ukpga_1974_37_HSWA`, `UK_CMCHA_ukpga_2007_19`). All IDs are stripped to the canonical form `{JURISDICTION}_{type_code}_{year}_{number}` during export. Three patterns are handled: `UK_ACRO_type_year_num → UK_type_year_num`, `UK_type_year_num_ACRO → UK_type_year_num`, `UK_year_num_ACRO → UK_year_num`.

### 3.2 Structural Hierarchy

Each level is nullable — only populated when relevant to this record's position. A section-level record will have `part` and `chapter` populated (its parents) but `paragraph` null. This is the **materialised path** pattern — it trades storage for query simplicity. In a columnar store (DuckDB/Parquet), the repeated string values compress extremely well via dictionary encoding.

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `part` | Utf8 | yes | Part number/letter |
| `chapter` | Utf8 | yes | Chapter number |
| `heading_group` | Utf8 | yes | Cross-heading group membership label. Value is the first section/article number under the parent cross-heading (e.g., `18` means "under the cross-heading starting at section 18"). Not a sequential counter. NULL for rows outside any cross-heading group (title, part, chapter, schedule, etc.). Scoped to `(law_name, part/schedule)` — resets at schedule boundaries. The heading **text** is in the `text` column of rows with `section_type = 'heading'`. |
| `provision` | Utf8 | yes | Section number (UK Acts) or article/regulation number (UK SIs, EU, most jurisdictions). Merges the former `section` and `article` columns — the `section_type` column distinguishes the provision type. |
| `paragraph` | Utf8 | yes | Paragraph number |
| `sub_paragraph` | Utf8 | yes | Sub-paragraph number |
| `schedule` | Utf8 | yes | Schedule/annex number |

### 3.3 Section Types

Normalised across jurisdictions. Each country's scraper maps its local terminology to this set.

| `section_type` | Description | UK Act | UK SI | DE | NO | TUR | RUS |
|----------------|-------------|--------|-------|-----|-----|-----|-----|
| `title` | Document title | title | title | title | title | title | zagolovok |
| `part` | Major division | part | part | Teil | del | kisim | chast |
| `chapter` | Chapter | chapter | chapter | Kapitel | kapittel | bölüm | razdel |
| `heading` | Section heading | heading | heading | Überschrift | — | başlık | — |
| `section` | Section | section | — | Abschnitt | — | — | glava |
| `sub_section` | Sub-section | sub-section | — | — | — | — | — |
| `article` | Article / regulation | — | article/regulation | Artikel/§ | § | madde | stat'ya |
| `sub_article` | Sub-article | — | sub-article | — | — | — | — |
| `paragraph` | Paragraph | paragraph | paragraph | Absatz | ledd | fikra | abzats |
| `sub_paragraph` | Sub-paragraph | sub-paragraph | sub-paragraph | — | — | bent | podpunkt |
| `schedule` | Schedule / annex | schedule | schedule | Anlage | vedlegg | ek | prilozhenie |
| `commencement` | Commencement provision | commencement | commencement | — | — | — | — |
| `table` | Table | table | table | Tabelle | — | — | — |
| `note` | Note / footnote / figure | note | note | — | — | — | — |
| `signed` | Signatory block | signed | signed | — | — | — | — |

### 3.4 Content

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `text` | Utf8 | no | The legal text content of this structural unit |
| `language` | Utf8 | no | Language code: `en`, `de`, `fr`, `no`, `sv`, `fi`, `tr`, `ru` |
| `extent_code` | Utf8 | yes | Territorial extent at this article level (e.g., `E+W` for a section that applies only to England and Wales). Same encoding as `legislation.extent_code`. |

### 3.5 Amendment Annotations

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `amendment_count` | Int32 | yes | Number of amendments annotated on this section |
| `modification_count` | Int32 | yes | Number of modifications |
| `commencement_count` | Int32 | yes | Number of commencement annotations |
| `extent_count` | Int32 | yes | Number of extent annotations |
| `editorial_count` | Int32 | yes | Number of editorial annotations |

### 3.6 Embeddings & AI (Schema Only — Populated in Later Phase)

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `embedding` | FixedSizeList\<Float32, 384\> | yes | Semantic embedding vector. Null until ONNX integration (Phase 2). Dimension 384 = all-MiniLM-L6-v2 or similar small model. |
| `embedding_model` | Utf8 | yes | Model used to generate embedding: `all-MiniLM-L6-v2`, etc. |
| `embedded_at` | Timestamp(ns, UTC) | yes | When embedding was generated |

### 3.7 Migration

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `legacy_id` | Utf8 | yes | Original Airtable positional encoding (`{law_name}_{part}_{heading}_{section}_{sub}_{para}_{extent}`). Preserved for backward-compatible lookups during migration. Not a primary key — has 1.5% collision rate. |

### 3.8 Metadata

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `created_at` | Timestamp(ns, UTC) | no | Record creation time |
| `updated_at` | Timestamp(ns, UTC) | no | Last update time |

---

## Table 4: `amendment_annotations` — Semantic Path

One row per legislative change annotation. Links amendment footnotes to the LAT sections they affect.

This table is the Fractalaw evolution of the legl Airtable amendments table. UK legislation annotates changes using coded footnotes — `F` (textual amendments), `C` (modifications), `I` (commencements), `E` (extent/editorial). Each annotation has descriptive text explaining the change and applies to one or more sections of the law. The legl prototype provided a better browsing experience than legislation.gov.uk by surfacing these as a structured, navigable table alongside the article text.

**Data sources** (combined during export via `data/export_lat.sql`):
1. **LAT-*.csv** — C/I/E annotations from interleaved annotation rows (record types `commencement,content`, `modification,content`, `extent,content`, etc.)
2. **LAT-*.csv** — F-code annotations from interleaved amendment rows (record types `amendment,textual`, `amendment,general`, etc.)
3. **AMD-*.csv** — Additional F-code annotations from separate amendment files (16 files, ~12K rows, 104 laws, zero overlap with LAT F-code rows)

**Legacy reference**: [`Legl.Countries.Uk.AirtableAmendment.Amendments`](https://github.com/shotleybuilder/legl/blob/main/lib/legl/countries/uk/airtable_amendment/uk_article_amendments.ex)

### 4.1 Identity

| Column | Arrow Type | Nullable | Description | Legacy (legl) |
|--------|-----------|----------|-------------|---------------|
| `id` | Utf8 | no | Synthetic unique annotation ID. Format: `{law_name}:{code_type}:{seq}` where `seq` is a per-law, per-code_type counter assigned during export. Example: `UK_ukpga_1974_37:amendment:1`. Guaranteed unique — replaces the legacy format which had a 2.8% collision rate. | `ID` |
| `law_name` | Utf8 | no | Parent law identifier (FK to LRT `legislation.name`). Acronyms stripped (see Table 3 ID normalization note). | derived from `opts.name` |
| `code` | Utf8 | no | Annotation code from legislation.gov.uk: `F1`, `F123`, `C42`, `I7`, `E3`. Extracted from the annotation text or AMD `Ef Code` column. Not unique per law (C36 appears 39 times in HSWA 1974). | `Ef Code` |
| `code_type` | Utf8 | no | Category: `amendment`, `modification`, `commencement`, `extent_editorial` | derived from source record type |
| `source` | Utf8 | no | Data provenance: `lat_cie` (C/I/E annotations from LAT CSV), `lat_f` (F-code annotations from LAT CSV), `amd_f` (F-code annotations from AMD CSV). Makes the linkage mechanism explicit. | *new* |

### 4.2 Annotation Codes

| `code_type` | Code Prefix | Source Record Types | Description |
|-------------|------------|-------------------|-------------|
| `amendment` | F | `amendment,textual` / `amendment,general` / `amendment,repeal` + AMD-*.csv | Textual amendments — words substituted, inserted, omitted, repealed. Sourced from both LAT annotation rows and separate AMD CSV files. |
| `modification` | C | `modification,content` | Modifications to how provisions apply |
| `commencement` | I | `commencement,content` | Commencement of provisions (bringing into force) |
| `extent_editorial` | E | `extent,content` / `editorial,content` / `subordinate,content` | Editorial notes, extent annotations, and subordinate legislation references |

### 4.3 Content

| Column | Arrow Type | Nullable | Description | Legacy (legl) |
|--------|-----------|----------|-------------|---------------|
| `text` | Utf8 | no | The annotation text describing the change (e.g., "S. 5(1) substituted (1.4.2015) by ...") | `Text` |
| `affected_sections` | List\<Utf8\> | yes | LAT `section_id` values affected by this annotation. Linkage mechanism depends on annotation type (see design note below). | `Articles` |

### 4.4 Metadata

| Column | Arrow Type | Nullable | Description |
|--------|-----------|----------|-------------|
| `created_at` | Timestamp(ns, UTC) | no | Record creation time |
| `updated_at` | Timestamp(ns, UTC) | no | Last update time |

> **Design note — affected_sections linkage**: The `affected_sections` list is built differently depending on the annotation source:
>
> - **C/I/E annotations** (from LAT rows): These annotation rows have positional IDs like `{parent_section_id}_cx_N`. The parent content section is recovered by stripping the suffix, giving a direct 1:1 linkage.
> - **F-code annotations from LAT**: Content rows carry a `Changes` column listing F-codes (e.g., `"F3,F2,F1"`). This is inverted during export: each code maps back to the content `section_id` that references it.
> - **F-code annotations from AMD-*.csv**: The `Articles` column contains comma-separated `section_id` cross-references directly.
>
> This enables both directions: from an annotation to the sections it touches, and from a LAT section's annotation counts (section 3.5) back to the full annotation text. ~7% of F-code annotations have no `affected_sections` due to data gaps in the source exports.

---

## Struct Definitions Summary

For reference — the named structs used in `List<Struct>` columns:

### `RelatedLaw`

Used in: `enacted_by`, `enacting`, `amending`, `amended_by`, `rescinding`, `rescinded_by`

```
{
  name:        Utf8       -- law identifier
  title:       Utf8       -- law title (denormalized)
  year:        Int32      -- law year (denormalized)
  count:       Int32      -- number of individual changes
  latest_date: Date32     -- most recent change date
}
```

### `DRRPEntry`

Used in: `duties`, `rights`, `responsibilities`, `powers`

```
{
  holder:    Utf8    -- holder type
  duty_type: Utf8    -- DUTY / RIGHT / RESPONSIBILITY / POWER
  clause:    Utf8    -- text excerpt
  article:   Utf8    -- article reference
}
```

---

## Column Counts

| Table | Scalar Columns | List Columns | Total |
|-------|---------------|-------------|-------|
| `legislation` (LRT) | 56 | 22 (12 List\<Utf8\> + 10 List\<Struct\>) | 78 |
| `law_edges` | 8 | — | 8 |
| `legislation_text` (LAT) | 28 | — | 28 |
| `amendment_annotations` | 8 | 1 (List\<Utf8\>) | 9 |

---

## Migration Path (Legacy → Fractalaw)

### Phase 1: Static export

1. **Export `uk_lrt` → Parquet** — `data/export_lrt.sql` (DuckDB)
   - Source: legacy PostgreSQL `uk_lrt` table exported to CSV
   - Maps columns per this schema
   - Sets `jurisdiction = "UK"`, `source_authority = "legislation.gov.uk"`, `language = "en"`
   - Output: `data/legislation.parquet` (19K+ rows)
   - **Status: done**

2. **Derive `law_edges` from exported LRT** — `data/export_edges.sql` (DuckDB)
   - Expand each `List<Struct>` relationship column into edge rows
   - Expand `*_stats_per_law` detail into article-level edge rows
   - Output: `data/law_edges.parquet` (1,035,305 edge rows)
   - **Status: done**

3. **Export LAT + annotations → Parquet** — `data/export_lat.sql` (DuckDB)
   - Source: 17 LAT-*.csv files (~115K rows, 460 laws) + 16 AMD-*.csv files (~12K rows, 104 laws)
   - Strips legacy acronyms from all ID columns
   - Separates content rows (→ `legislation_text`) from annotation rows (→ `amendment_annotations`)
   - Combines three annotation sources: C/I/E from LAT, F from LAT, F from AMD
   - Excludes `UK_uksi_2016_1091` (parser bug producing 606 duplicate annotation IDs)
   - Output: `data/legislation_text.parquet` (97,522 rows, 452 laws), `data/amendment_annotations.parquet` (19,451 rows, 137 laws), `data/annotation_totals.parquet` (135 laws)
   - FK match to LRT: 405/452 laws (90%); 47 unmatched are laws present in LAT but absent from LRT
   - Zero duplicate `section_id` values, zero duplicate annotation `id` values
   - **Status: done** — schema uses three-column identity (`section_id` structural citation, `sort_key`, `position`), `heading` → `heading_group`, `section`/`article` → `provision`, annotation `source` column, synthetic annotation IDs

### Deviations from SCHEMA-2.0 Review

The [SCHEMA-2.0 review](SCHEMA-2.0.md) recommended 9 changes. All high/medium priority items were implemented; some with design improvements over the original recommendation.

| # | Recommendation | Outcome |
|---|---------------|---------|
| 1 | Replace `section_id` with `{law_name}:{position}` | **Improved**: used citation-based `{law_name}:{citation}[{extent}]` instead. Citations are parliament's canonical addressing scheme — stable across amendments, unlike position which requires renumbering when sections are inserted. Added `sort_key` for machine-sortable ordering. |
| 2 | Rename `heading` → `heading_group` | **Done** |
| 3 | Merge `section`/`article` → `provision` | **Done** |
| 4 | Synthetic annotation ID | **Done**: `{law_name}:{code_type}:{seq}` — zero duplicates |
| 5 | Add annotation `source` column | **Done**: `lat_cie`, `lat_f`, `amd_f` |
| 6 | Filter 249 NULL rows | **Done** |
| 7 | Use NULL for root `hierarchy_path` | **Deferred** |
| 8 | Strip F-code markers from text | **Deferred** |
| 9 | Non-UK structural support | **Deferred** |

### Phase 3+: Multi-jurisdiction

- New scrapers write to the same schema with different `jurisdiction` values
- Same LRT, same `law_edges`, same LAT, same `amendment_annotations`
- DataFusion partition pruning by `jurisdiction` keeps queries fast
