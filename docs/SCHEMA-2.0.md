# LAT Schema Critical Review

**Date**: 2026-02-16
**Scope**: Tables 3 (`legislation_text`) and 4 (`amendment_annotations`) as prototyped in Airtable and exported via `export_lat.sql`
**Method**: Empirical analysis of the exported Parquet files (99,113 content rows, 21,929 annotation rows, 453 UK laws)

---

## 1. The `section_id` Positional Encoding Is Broken

**Current format**: `{law_name}_{part}_{chapter}_{heading}_{section}_{sub}_{para}_{extent}`

Empty segments are represented as adjacent underscores. Example:
```
UK_ukpga_1974_37_1__1_1_1__UK
                 ^ ^       ^^
                 | |       |+-- trailing empty segment
                 | |       +--- extent marker
                 | +----------- empty chapter slot
                 +------------- part = 1
```

### 1.1 It Is Not Unique

1,511 duplicate `section_id` values across 99,113 rows (1.5% collision rate). The worst offender (`UK_uksi_2018_369`) has a single `section_id` repeated 13 times. Five laws have more rows than their maximum `position` value, confirming the encoding cannot distinguish rows within those laws.

Root causes:
- **Table rows**: Laws with tables produce rows where the positional encoding collapses (same part/section/paragraph values, differing only by table content). The `_tbl` and `_A`/`_B` suffixes were added as workarounds but don't cover all cases.
- **Extent variants**: Laws with parallel territorial provisions (GB vs NI versions of the same regulation) can produce the same positional encoding when the extent marker is missing or inconsistent.
- **Duplicate source rows**: Some Airtable records genuinely appear twice with identical IDs (Turkey had 554 dupes in the source; the UK has fewer but they exist).

### 1.2 It Is Opaque

The encoding is a fixed-slot positional system where the meaning of each segment depends on implicit knowledge:
- Segment 5 is "part" but only if the law uses parts
- Segment 6 is "chapter" but always empty in the current UK data (0 of 99,113 rows use it)
- Segment 7 is "heading" (a sequential counter, not heading text)
- The double-underscore `__` is not a delimiter — it's a single underscore followed by an empty segment followed by a single underscore

This means:
- You cannot parse a `section_id` without knowing the `law_name` prefix length
- The segment semantics shift depending on whether the law is an Act (uses sections) or SI (uses articles/regulations)
- The encoding carries no self-describing structure — unlike, say, a URI path

### 1.3 It Does Not Generalise

The non-UK country files revealed that each jurisdiction has completely different naming patterns:
- **Germany**: `DE_2020_ArbSchG § 1_2_3` (uses `§` and spaces)
- **Denmark**: `DK_2020_799_Products § 1` (English words and spaces)
- **Norway**: `NO_1929_05-24-4_ASEIEEESA_6a` (date-based numbering)
- **Turkey**: `TUR_1983_2872_EL_1_1__` (underscores with trailing placeholder `_`)
- **Sweden/Finland/Austria**: Various other patterns

There is no common positional grammar that works across jurisdictions. The encoding is a UK-specific Airtable artifact.

### 1.4 Recommendation: Replace With a Synthetic Key

**`section_id`** should become a composite natural key: `{law_name}:{position}`.

Example: `UK_ukpga_1974_37:5` (the 5th structural unit in document order).

Properties:
- **Guaranteed unique** per law (position is already unique per law by construction)
- **Sortable**: `ORDER BY section_id` recovers document order within a law
- **Parseable**: trivial to split on `:` to recover law_name and position
- **Jurisdiction-agnostic**: works identically for DE, NO, TUR, etc.
- **Stable under re-export**: position is assigned from document-traversal order, which is deterministic

The legacy positional encoding should be preserved as `legacy_id` (VARCHAR, nullable) for any backward-compatible lookups needed during migration. It should not be a primary key.

---

## 2. The Structural Hierarchy Columns Are Redundant But Useful

**Current design**: Eight nullable VARCHAR columns (`part`, `chapter`, `heading`, `section`, `article`, `paragraph`, `sub_paragraph`, `schedule`) carry the structural address of each row. Each row inherits its parent's values — a sub-section row has `part`, `heading`, and `section` populated from its ancestors.

### 2.1 What Works

The columns are genuinely useful for queries like:
- "Show all sub-sections of section 5" → `WHERE section = '5' AND section_type = 'sub_section'`
- "List all articles in Part 2" → `WHERE part = '2' AND section_type = 'article'`
- "Count provisions per chapter" → `GROUP BY chapter`

This is the **materialised path** pattern — it trades storage for query simplicity. In a columnar store like DuckDB/Parquet, the repeated string values compress extremely well (dictionary encoding), so the storage cost is negligible.

### 2.2 What Doesn't Work

1. **The `heading` column is a counter, not text.** Values are `1`, `2`, `3`... (63,419 rows populated). It represents "which heading group this row belongs to" — a sequential index, not the heading text itself. This is deeply confusing. The actual heading text is in the `text` column of heading-type rows. The column should be renamed to `heading_group` or similar, or the field semantics should be documented clearly.

2. **The `section` vs `article` distinction is fragile.** UK Acts use "sections"; UK SIs use "articles" or "regulations". The export maps this via the `Class` column (Regulation → article, otherwise → section). But 8,324 rows have `article` populated vs 73,812 with `section`, and the `Section||Regulation` source column is the same field for both. There's no semantic difference in the data — only a labelling convention that varies by instrument type.

3. **The columns don't capture the full hierarchy for non-UK jurisdictions.** Germany needs `sub_section` (Unterabschnitt). Norway needs `sub_chapter`. Austria has `§` as a structural level. Sweden has `regulation` as distinct from `article`. The 8-column fixed set is UK-centric.

### 2.3 Recommendation: Keep Columns But Normalise

The structural columns should be kept — they're genuinely useful for DuckDB queries and compress well. But:

1. **Rename `heading` → `heading_group`** (or `heading_idx`) to clarify it's a counter.
2. **Merge `section` and `article` into a single column** — call it `provision` or keep `article` (the more universal term). The section/article distinction is a display concern, not a data concern. The `section_type` column already captures whether it's a "section" or "article".
3. **Add `sub_chapter`** for Norway's Underkapittel. Consider whether a general `division` column could subsume `part`/`chapter`/`section` into a more flexible scheme.
4. **The `hierarchy_path` column** (`part.1/heading.2/section.3/sub.1`) is derived from these columns and should continue to be derived. It serves as a human-readable address. But it should use the normalised column names.

---

## 3. The `hierarchy_path` and `depth` Are Sound

`hierarchy_path` is a slash-separated materialised path: `part.1/heading.2/section.3/sub.1`. `depth` is the count of populated structural levels.

### 3.1 What Works

- The path is human-readable and useful for display
- It can be prefix-searched: `WHERE hierarchy_path LIKE 'part.1/%'` finds everything in Part 1
- DuckDB's string operations handle this efficiently
- The depth distribution is sensible: 0 (title) through 6, with the bulk at depth 3-4

### 3.2 Concerns

- The path is derived from the structural columns, so it's a computed column. If the structural columns change, the path must be recomputed. This is fine as long as the derivation is documented and deterministic.
- Empty string vs NULL for root-level rows (title) should be standardised. Currently `hierarchy_path = ''` for titles.

### 3.3 Recommendation

Keep as-is. Consider storing as NULL instead of empty string for root rows, for cleaner SQL (`WHERE hierarchy_path IS NOT NULL` vs `WHERE hierarchy_path != ''`).

---

## 4. The Annotation Linkage Model Is Fragile

Three different mechanisms link annotations to the sections they affect:

| Source | Mechanism | Coverage |
|--------|-----------|----------|
| C/I/E from LAT | Strip `_cx_N`/`_mx_N`/`_ex_N` suffix from annotation ID to get parent section_id | 100% of C/I/E rows |
| F-codes from LAT | Invert the `Changes` column on content rows (content row lists its F-codes; reverse the mapping) | 87% of F-code rows (588 without) |
| F-codes from AMD | `Articles` column in AMD CSV directly lists section_ids | 100% of AMD rows |

### 4.1 What Works

- C/I/E parent linkage is structurally reliable — it's encoded in the ID itself
- The `affected_sections` output (List\<Utf8\>) provides a clean FK from annotation to LAT rows
- 93% of all annotations have resolved `affected_sections` (20,358 / 21,929)

### 4.2 What Doesn't Work

1. **The F-code inversion is a workaround, not a data model.** The source data has the relationship backwards: content rows know which F-codes apply to them (via `Changes`), but F-code annotation rows don't know which sections they affect. The inversion works but:
   - 588 F-code annotations (from LAT) have no `affected_sections` because no content row references them via `Changes`
   - The linkage depends on exact string matching of codes like `F123` — if the `Changes` column has formatting variations, the join fails silently

2. **Annotation IDs are not unique.** 606 duplicate annotation IDs across 21,929 rows (2.8%). All duplicates are F-code annotations from `UK_uksi_2016_1091` — the same law appears in both LAT and AMD sources with the same annotation codes, producing duplicate `{law_name}_{code}` IDs. This is a data integrity issue: the `id` column cannot serve as a primary key.

3. **The `code` column is not unique per law.** F1 appears in 105 different laws. C36 appears 39 times for a single law (HSWA 1974). This is by design (codes are scoped to a law), but combined with point 2 above, it means neither `id` nor `(law_name, code)` is a reliable unique key for annotations.

4. **F-code annotation sequences have large gaps.** HSWA 1974 has F-codes F1 through F375 but only 161 annotation rows — 214 codes are missing. The Wildlife and Countryside Act 1981 jumps from F1 to F3018 with only 115 rows. These gaps reflect annotation codes that exist in the legislation but whose annotation text wasn't captured in the Airtable export.

### 4.3 Recommendation: Clean Keys and Explicit Source Tracking

1. **Use a synthetic annotation ID**: `{law_name}:{code_type}:{sequence}` where `sequence` is a per-law, per-code_type counter assigned during export. This guarantees uniqueness.

2. **Add a `source` column**: `'lat_cie'`, `'lat_f'`, or `'amd_f'` — making the provenance explicit. This replaces the implicit knowledge of which linkage mechanism was used.

3. **Preserve the original code** as `code` (F1, C36, etc.) for display and cross-referencing with the legislation text. But don't rely on it for joins.

4. **Accept the F-code coverage gap.** The 7% of F-code annotations without `affected_sections` is a source data limitation. Document it, don't paper over it.

---

## 5. The Content/Annotation Row Separation Is Under-Documented

The source LAT CSV files interleave content rows and annotation rows in the same table. The export separates them by `Record_Type`:

- Content rows → `legislation_text.parquet` (99,113 rows)
- C/I/E annotation rows → `amendment_annotations.parquet` (9,466 rows)
- F-code annotation rows from LAT → `amendment_annotations.parquet` (2,997 rows)
- Heading rows (e.g., `extent,heading`) → **dropped** (~5,960 rows)
- F-code annotation rows from AMD → `amendment_annotations.parquet` (11,887 rows from separate files)

### 5.1 Observation

The annotation heading rows (Record_Type = `commencement,heading`, `modification,heading`, etc.) are dropped entirely. These are labels like "Commencement Information" that group annotations in the original legislation.gov.uk rendering. Dropping them is probably correct — they carry no data beyond what `code_type` provides — but it means ~6K source rows are silently discarded.

### 5.2 The 249 NULL Rows

249 rows have NULL `section_id`, NULL `law_name`, and NULL `section_type`. These appear to be non-UK rows that leaked through the glob when the country files were still named `LAT-*.csv`. They should be filtered out (or already have been, if the renaming to `xLAT-*.csv` happened before the last export). Verify and add a `WHERE section_id IS NOT NULL` guard.

### 5.3 2,338 Content Rows Start With F-Codes

2,338 content rows (2.4%) have text that begins with `F1`, `F2`, etc. — these are sections whose original text has been entirely replaced by an amendment. The amended text starts with the F-code marker. This is correct legislation.gov.uk behavior, but it means:
- Text search for "F1" will hit both annotation text and amended content text
- The `amendment_count` column correctly counts these, but the raw text is polluted with code markers

Consider stripping the leading code marker from content text in a future phase (regex: `^[FCIE][0-9]+\s*`).

---

## 6. The `position` Column Is the Best Thing in the Schema

The `position` column — a simple 1-based integer assigned in document-traversal order — is the most reliable element:
- It's unique per law (by construction)
- It recovers published document order
- It's jurisdiction-agnostic
- It doesn't encode structural semantics that might vary

The SCHEMA.md design note about this was prescient. The only issue is that the value is assigned during export from the CSV row order, which depends on the Airtable export being in document order. This assumption holds for the current data but should be validated for any future exports.

---

## 7. Summary of Recommendations

| # | Area | Current | Recommendation | Priority |
|---|------|---------|---------------|----------|
| 1 | `section_id` | Positional encoding, 1.5% collision rate | Replace with `{law_name}:{position}`. Keep legacy encoding as `legacy_id`. | **High** |
| 2 | `heading` column | Counter (1, 2, 3...), confusing name | Rename to `heading_group` or `heading_idx` | Medium |
| 3 | `section`/`article` split | Two columns for same concept | Merge into single `provision` column; `section_type` already distinguishes | Medium |
| 4 | Annotation `id` | `{law_name}_{code}`, 2.8% collision rate | Synthetic key: `{law_name}:{code_type}:{seq}` | **High** |
| 5 | Annotation `source` | Implicit (which linkage mechanism was used) | Add explicit `source` column: `lat_cie`, `lat_f`, `amd_f` | Medium |
| 6 | NULL rows | 249 rows with NULL section_id | Filter out; add `WHERE section_id IS NOT NULL` guard | Low |
| 7 | `hierarchy_path` root | Empty string for titles | Use NULL instead | Low |
| 8 | F-code text pollution | 2,338 content rows start with `F1` etc. | Consider stripping in future phase | Low |
| 9 | Non-UK support | Fixed 8-column hierarchy, UK-specific encoding | Merge section/article; add sub_chapter; use position-based IDs | Deferred |

---

## 8. Migration Impact

Changing `section_id` from positional encoding to `{law_name}:{position}` affects:

1. **`amendment_annotations.affected_sections`** — currently stores positional `section_id` values. Would need to store `{law_name}:{position}` values instead. The linkage mechanism (C/I/E parent stripping, F-code inversion) would change to use position-based keys.

2. **`export_lat.sql`** — the section_id construction would simplify dramatically. The entire `strip_acronym`/`strip_id` machinery for section_ids would be replaced by `law_name || ':' || position`.

3. **`fractalaw-core/src/schema.rs`** — the `legislation_text_schema()` and `amendment_annotations_schema()` functions would need updating to reflect the new ID format. The structural column changes (heading rename, section/article merge) would also need updating.

4. **LanceDB vector search** — if section_ids are used as document IDs in the vector index, they'd need to be regenerated. Since embeddings aren't populated yet (Phase 2), this has zero cost now.

The best time to make these changes is now, before any downstream code depends on the current encoding.
