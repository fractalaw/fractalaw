# Session: 2026-02-19 — LAT Schema Revision and Baseline Data

## Context

Phase 1 core tasks are complete ([`02-12-26-begin.md`](02-12-26-begin.md)). The hot path (DuckDB `legislation` table, 19,318 rows) and analytical path (DuckDB `law_edges` table, 1,035,305 edges) are validated and working via the CLI. The **semantic path** — LanceDB with `legislation_text` (LAT) and `amendment_annotations` — was parked during Phase 1 because the SCHEMA-2.0 review found critical data quality issues.

This session focuses exclusively on getting LAT data into a usable state for development.

**GitHub Issue**: [#11 — Phase 2: ONNX embeddings, semantic search, and LanceDB integration](https://github.com/fractalaw/fractalaw/issues/11) (LAT cleanup is the blocker)

## The Problem

The existing LAT export (`data/legislation_text.parquet`, 99,113 rows from 453 UK laws) has known issues documented in [`docs/SCHEMA-2.0.md`](../../docs/SCHEMA-2.0.md):

### Critical Issues (must fix)

1. **`section_id` is not unique** — 1,511 duplicates across 99,113 rows (1.5% collision rate). The positional encoding (`{law_name}_{part}_{chapter}_{heading}_{section}_{sub}_{para}_{extent}`) collapses for table rows, extent variants, and some source duplicates. Cannot serve as a primary key.

2. **Annotation IDs are not unique** — 606 duplicates across 21,929 annotation rows (2.8%). All from `UK_uksi_2016_1091` appearing in both LAT and AMD sources with identical annotation codes.

3. **`section_id` doesn't generalise** — The positional encoding is a UK-specific Airtable artifact. Germany uses `§`, Norway uses date-based numbering, Turkey uses `kisim/bölüm/madde`. No common grammar across jurisdictions.

### Medium Issues (should fix)

4. **`heading` column is a counter, not text** — Values are `1`, `2`, `3`... (63,419 rows). It means "which heading group", not heading text. Rename to `heading_group`.

5. **`section`/`article` split is fragile** — UK Acts use "sections", UK SIs use "articles". Same underlying data, just a labelling convention. The `section_type` column already distinguishes them. Merge into single `provision` column.

6. **249 NULL rows** — Leaked non-UK rows with NULL `section_id`, `law_name`, `section_type`. Filter out.

7. **F-code coverage gap** — 7% of F-code annotations (588 rows) have no `affected_sections` because no content row references them via the `Changes` column. Data limitation, not a bug.

### Low Priority (defer)

8. **2,338 content rows start with F-code markers** (e.g., `F1 The text...`). Text pollution from legislation.gov.uk rendering. Consider stripping in future phase.

9. **`hierarchy_path` root uses empty string** instead of NULL. Cosmetic.

## SCHEMA-2.0 Recommendations

From [`docs/SCHEMA-2.0.md`](../../docs/SCHEMA-2.0.md) §7:

| # | Area | Recommendation | Priority |
|---|------|---------------|----------|
| 1 | `section_id` | Replace with `{law_name}:{position}` — guaranteed unique, sortable, jurisdiction-agnostic | **High** |
| 2 | `heading` column | Rename to `heading_group` or `heading_idx` | Medium |
| 3 | `section`/`article` | Merge into single `provision` column; `section_type` already distinguishes | Medium |
| 4 | Annotation `id` | Synthetic key: `{law_name}:{code_type}:{seq}` | **High** |
| 5 | Annotation `source` | Add explicit column: `lat_cie`, `lat_f`, `amd_f` | Medium |
| 6 | NULL rows | Filter out with `WHERE section_id IS NOT NULL` | Low |
| 7 | `hierarchy_path` root | Use NULL instead of empty string | Low |
| 8 | F-code markers | Strip leading `[FCIE]\d+\s*` from content text | Low |
| 9 | Non-UK hierarchy | Merge section/article; add `sub_chapter` | Deferred |

## What Exists

### Source Data (in `data/`)
- **17 LAT CSV files** (UK, by ESH domain): `LAT-OH-and-S.csv`, `LAT-Fire.csv`, `LAT-Environmental-Protection.csv`, etc. (~115K rows, 460 laws)
- **16 AMD CSV files** (UK amendments): `AMD-OH-and-S.csv`, etc. (~12K rows, 104 laws)
- **7 xLAT CSV files** (non-UK, excluded): AUT, DK, FIN, DE, NO, SWE, TUR — incompatible column schemas, renamed to `xLAT-*` to exclude from globs

### Existing Export
- `data/export_lat.sql` — DuckDB SQL transform script (current, with known issues)
- `data/legislation_text.parquet` — 99,113 rows, 27 cols, 6.8MB (from 453 UK laws)
- `data/amendment_annotations.parquet` — 21,929 rows (9,466 C/I/E + 2,997 F from LAT + 11,887 F from AMD; 140 laws)
- `data/annotation_totals.parquet` — 136 laws

### Schema Definitions
- `docs/SCHEMA.md` Table 3 (LAT, 27 cols) and Table 4 (amendment_annotations, 8 cols)
- `crates/fractalaw-core/src/schema.rs` — `legislation_text_schema()` and `amendment_annotations_schema()` (need updating after revisions)

### Architecture
- LAT lives in LanceDB (semantic path) — text search, embeddings, RAG
- DataFusion bridges DuckDB (hot/analytical) and LanceDB (semantic) in a single SQL plan
- `fractalaw-store` Task 4 (LanceDB ingestion) is blocked on this work

## Session Goal

**Get a clean, usable LAT baseline for development** — not perfect, not multi-jurisdiction, but correct enough to unblock LanceDB ingestion (Task 4) and eventually Phase 2 embeddings.

## Tasks

### 1. Revise the LAT schema
- [ ] Apply SCHEMA-2.0 recommendations: new `section_id` format, `heading` rename, `section`/`article` merge, annotation ID fix
- [ ] Update `docs/SCHEMA.md` Tables 3 and 4
- [ ] Update `crates/fractalaw-core/src/schema.rs` — `legislation_text_schema()` and `amendment_annotations_schema()`
- [ ] Update unit tests in schema.rs

### 2. Rewrite the LAT export script
- [ ] Rewrite `data/export_lat.sql` applying the revised schema
- [ ] New `section_id` = `{law_name}:{position}` (drop legacy positional encoding, keep as `legacy_id`)
- [ ] New annotation `id` = `{law_name}:{code_type}:{seq}`
- [ ] Add `source` column to annotations
- [ ] Filter NULL rows
- [ ] Validate: zero duplicate `section_id`, zero duplicate annotation `id`

### 3. Re-export LAT Parquet files
- [ ] Run revised export script
- [ ] Validate row counts match expectations (~99K content, ~22K annotations minus dupes/nulls)
- [ ] Verify FK linkage: `law_name` in LAT matches `name` in LRT (legislation.parquet)
- [ ] Verify `ORDER BY position` recovers document order

### 4. Smoke test with DuckDB
- [ ] Load `legislation_text.parquet` into DuckDB alongside existing legislation + law_edges
- [ ] Run cross-table queries: "show article text for HSWA 1974 section 2"
- [ ] Run annotation queries: "show all F-code amendments for COSHH"
- [ ] Verify `section_id` uniqueness: `SELECT section_id, count(*) HAVING count(*) > 1` returns zero rows

### 5. Update SCHEMA.md and close out
- [ ] Document any deviations from SCHEMA-2.0 recommendations
- [ ] Update migration path section in SCHEMA.md

## Reference: LAT Record Shape

From the legl Airtable prototype — each record is one structural unit of a law:

```
law_name       — parent law identifier (FK to LRT)
position       — document order (1-based integer)
section_id     — {law_name}:{position} (NEW — replaces legacy encoding)
section_type   — title, part, chapter, heading, section, article, paragraph, ...
hierarchy_path — part.1/heading.2/section.3/sub.1
depth          — count of populated hierarchy levels

part, chapter, heading_group, provision, paragraph, sub_paragraph, schedule
  (structural address — materialised path pattern)

text           — the legal text content
language       — en, de, fr, ...
extent_code    — territorial applicability at article level

amendment_count, modification_count, commencement_count, extent_count, editorial_count
  (annotation counters per section)

embedding      — FixedSizeList<Float32, 384> (null until Phase 2)
embedding_model, embedded_at
created_at, updated_at
```

## Reference: Multi-Jurisdiction Hierarchy

Every jurisdiction has different article-level terminology. The `section_type` enum abstracts this:

| `section_type` | UK Act | UK SI | DE | NO | TUR | RUS |
|---|---|---|---|---|---|---|
| `title` | title | title | title | title | title | zagolovok |
| `part` | part | part | Teil | del | kisim | chast |
| `chapter` | chapter | chapter | Kapitel | kapittel | bölüm | razdel |
| `article` | — | article/regulation | Artikel/§ | § | madde | stat'ya |
| `section` | section | — | Abschnitt | — | — | glava |
| `paragraph` | paragraph | paragraph | Absatz | ledd | fikra | abzats |
| `schedule` | schedule | schedule | Anlage | vedlegg | ek | prilozhenie |

The structural hierarchy columns capture address, `section_type` captures semantics.

## Reference: Annotation Linkage

Three mechanisms link annotations to the sections they affect:

| Source | Mechanism | Coverage |
|---|---|---|
| C/I/E from LAT | Strip `_cx_N`/`_mx_N`/`_ex_N` suffix from annotation ID to get parent `section_id` | 100% |
| F-codes from LAT | Invert the `Changes` column on content rows | ~87% (588 unlinked) |
| F-codes from AMD | `Articles` column directly lists section_ids | 100% |

The `affected_sections` (List\<Utf8\>) column carries the result. ~93% of all annotations have resolved linkage.
