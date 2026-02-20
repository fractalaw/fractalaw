# Session: 2026-02-19 — LAT Schema Revision and Baseline Data

## Context

Phase 1 core tasks are complete ([`02-12-26-begin.md`](02-12-26-begin.md)). The hot path (DuckDB `legislation` table, 19,318 rows) and analytical path (DuckDB `law_edges` table, 1,035,305 edges) are validated and working via the CLI. The **semantic path** — LanceDB with `legislation_text` (LAT) and `amendment_annotations` — was parked during Phase 1 because the SCHEMA-2.0 review found critical data quality issues.

This session focuses exclusively on getting LAT data into a usable state for development.

**GitHub Issue**: [#11 — Phase 2: ONNX embeddings, semantic search, and LanceDB integration](https://github.com/fractalaw/fractalaw/issues/11) (LAT cleanup is the blocker)

## The Problem

The existing LAT export (`data/legislation_text.parquet`, 99,113 rows from 453 UK laws) has known issues documented in [`docs/SCHEMA-2.0.md`](../../docs/SCHEMA-2.0.md):

### Critical Issues (must fix)

1. **`section_id` is not unique** — 1,511 duplicates across 99,113 rows (1.5% collision rate). The positional encoding (`{law_name}_{part}_{chapter}_{heading}_{section}_{sub}_{para}_{extent}`) collapses for table rows, extent variants, and some source duplicates. Cannot serve as a primary key. **Resolution**: Three-column identity design — see [Design Decision: Section Identity and Ordering](#design-decision-section-identity-and-ordering) below.

2. **Annotation IDs are not unique** — 606 duplicates across 21,929 annotation rows (2.8%). All from `UK_uksi_2016_1091` (The Electromagnetic Compatibility Regulations 2016). **Resolution**: Exclude this law from the baseline. Investigation confirmed this is a parser bug — see [Investigation: UK_uksi_2016_1091](#investigation-uksi20161091-annotation-duplicates) below.

3. **`section_id` doesn't generalise** — The positional encoding is a UK-specific Airtable artifact. Germany uses `§`, Norway uses date-based numbering, Turkey uses `kisim/bölüm/madde`. No common grammar across jurisdictions. **Resolution**: The citation-based `section_id` design generalises to all surveyed jurisdictions — see [Cross-Jurisdiction Validation](#cross-jurisdiction-validation-critical-issue-3) below.

### Medium Issues (should fix)

4. **`heading` column name is misleading** — Not a sequential counter as SCHEMA-2.0 assumed. It's a **group membership label** whose value is the first section/article number under the parent cross-heading (e.g., `18` means "under the cross-heading that starts at section 18"). Values are VARCHAR with 415 distinct values including alpha-suffixed numbers (`10A`, `25A`, `19AZA`), single letters (`A`, `D`), and dotted decimals (`1.1`, `2.1`). **Resolved**: Rename to `heading_group`. Semantics and values unchanged. See [Investigation: Heading Column](#investigation-heading-column) below.

5. **`section`/`article` split is fragile** — UK Acts use "sections", UK SIs use "articles". Same underlying data, just a labelling convention. The `section_type` column already distinguishes them. **Resolved**: Merge into single `provision` column.

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

## Design Decision: Section Identity and Ordering

### The Amendment Insertion Problem

SCHEMA-2.0 recommended replacing `section_id` with `{law_name}:{position}` where `position` is an integer. **This is wrong.** Amendments insert new sections into existing laws, and a snapshot integer breaks:

```
Before amendment:       After amendment inserting s.41A:
  position 40 → s.40     position 40 → s.40
  position 41 → s.41     position 41 → s.41
  position 42 → s.42     position 42 → s.41A  ← inserted
                          position 43 → s.42   ← renumbered
```

An integer `position` is a snapshot of document order at export time. It cannot accommodate insertions without renumbering everything downstream. The legacy positional encoding (`{law_name}_{part}_{chapter}_{heading}_{section}_{sub}_{para}_{extent}`) was an attempt to encode the structural address to avoid this problem — right instinct, bad implementation.

### UK Legal Numbering Conventions

Parliament's own solution to the insertion problem. Surveyed across 99,113 LAT rows:

| Pattern | Example | Sort position | Count in dataset |
|---|---|---|---|
| Plain numeric | `s.3` | Base | 5,105 sections |
| Single letter suffix | `s.3A`, `s.3B` | After 3, before 4 | 923 sections |
| Z-prefix (insert before A) | `s.3ZA`, `s.3ZB` | After 3, before 3A | 32 sections |
| Double letter | `s.19AA`, `s.19DZA` | Nested insertions | 114 sections |
| Sub-section insertions | `s.41(1A)`, `s.41(2A)` | After (1), before (2) | common in sub_section rows |
| Article equivalents | `reg.2A`, `art.16B` | Same pattern in SIs | 72+ article rows |

The structural citation is **parliament's canonical, permanent address**. "Section 41A of the Environment Act 1995" never changes — even when further amendments add 41B, 41C, or 41ZA before it.

### Resolution: Three-Column Design

| Column | Type | Role | Stable? |
|---|---|---|---|
| `section_id` | Utf8 | **Structural citation** — the legal address. `{law_name}:s.41A` or `{law_name}:reg.2A(1)(b)` | Yes — permanent, parliament-assigned |
| `sort_key` | Utf8 | **Normalised sort encoding** — machine-sortable string that respects insertion ordering | Yes — derived from citation, handles Z-prefixes |
| `position` | Int32 | **Snapshot index** — integer document order at export time. Useful for fast range queries. Reassigned on re-export | No — changes when sections are inserted |

**`section_id`** encodes the structural citation path:
```
UK_ukpga_1974_37:s.25A          — section 25A of HSWA 1974
UK_ukpga_1974_37:s.25A(1)       — sub-section (1) of section 25A
UK_ukpga_1995_25:s.41A          — inserted section 41A of Environment Act
UK_uksi_2002_2677:reg.2A(1)(b)  — inserted regulation 2A(1)(b) of COSHH
UK_ukpga_1995_25:sch.2.para.3   — schedule 2, paragraph 3
UK_ukpga_1974_37:s.23[E+W]      — E+W territorial version of section 23
UK_ukpga_1974_37:s.23[NI]       — NI territorial version (different text!)
UK_ukpga_1974_37:s.23(4)[S]     — Scotland version of sub-section (4)
```

The format is `{law_name}:{citation}[{extent}]` where:
- Citation uses the `section_type` to determine prefix (`s.` for section, `reg.` for regulation, `art.` for article, `sch.` for schedule, etc.)
- The `[extent]` qualifier is present only when parallel territorial provisions exist — i.e., when the same section number has different text for different regions

### Parallel Territorial Provisions

29 laws in the dataset (719 section-level rows) have parallel provisions where the same section number exists with different text for different territorial extents. Example: HSWA 1974 section 23(4) exists in three versions:
- **E+W**: references "Regulatory Reform (Fire Safety) Order 2005"
- **NI**: references "Fire Precautions Act 1971"
- **S**: references "Fire (Scotland) Act 2005"

These are substantively different legal provisions — not formatting variations. legislation.gov.uk serves them on a single page (`/section/23`) with fragment anchors (`#extent-E-W`, `#extent-S`, `#extent-N.I.`) but no separate URLs. The canonical legal citation remains "section 23" regardless of which territorial version applies — extent is a property of the provision, not part of the parliamentary numbering.

For `section_id` uniqueness, the extent qualifier is needed only when a law has parallel provisions for the same section number. The export detects this per-law and adds `[extent]` where required. Sections with a single territorial version (the common case — most of the 99K rows) have no qualifier.

**`sort_key`** normalises the citation into a lexicographically-sortable string:
```
s.3       → 003.000.000~
s.3ZA     → 003.001.000~
s.3ZB     → 003.002.000~
s.3A      → 003.010.000~
s.3AA     → 003.010.010~
s.3AB     → 003.010.020~
s.3B      → 003.020.000~
s.4       → 004.000.000~
s.23[E+W] → 023.000.000~E+W    (parallel provisions: extent as sort suffix)
s.23[NI]  → 023.000.000~NI
s.23[S]   → 023.000.000~S
```

Rules:
- Numeric base: zero-padded to 3 digits (handles up to section 999)
- Z-prefix: sorts in 001-009 range (before A at 010)
- Letter suffix: A=010, B=020, C=030... (gaps for nested insertions)
- Double letters: AA=010+010, AB=010+020
- Sub-levels: additional `.NNN` segments for paragraph/sub-paragraph
- Extent qualifier: `~{extent}` suffix for parallel territorial provisions (tilde sorts after digits/letters, so all versions of a section group together). Within a section, extent variants sort alphabetically: E+W < NI < S

**`position`** remains as a convenience integer. It's the row index in document order at export time. Useful for `LIMIT`/`OFFSET` queries and fast range scans. But it's derived and ephemeral — not an identifier.

### Why Not Just Integer Position?

- **Incremental updates** (Phase 3 regulation-importer): inserting section 41A between positions 248 and 249 requires renumbering all subsequent rows. With structural citation, you just add the row.
- **CRDT sync** (Fractalaw uses Loro): position-based ordering doesn't merge — two nodes inserting different sections at the "same position" conflict. Citation-based ordering is conflict-free because parliament assigns unique citations.
- **Human reference**: "Section 41A" is how lawyers cite it. An integer position is meaningless to users.
- **Cross-version stability**: the same section across different versions of a law should have the same `section_id`. Position may differ as other sections are added/removed.

### Why Not Just Structural Citation Without Sort Key?

The citation string doesn't sort correctly without normalisation:
```
Naive string sort:        Correct document order:
s.1                       s.1
s.10                      s.2
s.11                      s.3
s.2    ← wrong            s.3ZA
s.3                       s.3A
s.3A                      s.4
s.3ZA  ← wrong            ...
s.4                       s.10
                          s.11
```

The `sort_key` column encodes the parliamentary ordering rules into a lexicographically-sortable format. `ORDER BY sort_key` always recovers correct document order.

---

## Investigation: UK_uksi_2016_1091 Annotation Duplicates

**Law**: The Electromagnetic Compatibility Regulations 2016 (SI 2016/1091). A post-Brexit instrument transposing EU Directive 2014/30/EU, with 6 Parts and 7 Schedules. Heavily amended after 31 December 2020 to create parallel legal texts for E+W+S (Great Britain regime) and N.I. (Northern Ireland Protocol regime).

### What the legislation.gov.uk XML reveals

The underlying Crown Legislation Markup Language (CLML) XML uses **opaque hash-based commentary IDs**, not F-code numbers:

```xml
<Commentary id="key-089efbbc031597a80350b41a40f9fac0" Type="F">
  <Para><Text>Words in reg. 2(1) omitted (E.W.S.) (31.12.2020)...</Text></Para>
</Commentary>
```

The human-readable F1, F2, F3 numbering is a **presentation-layer construct** — assigned sequentially per-section when the HTML is rendered. The F-code numbers are not stable identifiers in the source data.

### Root cause: territorial duplication + source overlap

This SI has **systematic territorial duplication** — nearly every substantive amendment exists in two versions (E+W+S and N.I.), creating parallel legal texts within a single statutory instrument. For example, in Regulation 2 (Interpretation):
- **F1-F22** apply to the E+W+S version (substituting "EU market" → "market of Great Britain", "CE marking" → "UK marking")
- **F23-F35** apply to the N.I. version (using "relevant market", retaining "notified body", adding "UK(NI) indication")

The parser could not correctly handle this complexity. The same annotations appear in both the LAT CSV files and the AMD CSV files for this law, producing 606 duplicate `{law_name}_{code}` IDs.

### Decision: Exclude from baseline

**Do not migrate UK_uksi_2016_1091.** The data cannot be trusted. The parser needs investigation for laws with heavy post-Brexit territorial duplication — this is a pattern shared by hundreds of product safety SIs amended during the EU Exit transition, but only this one law appears in both LAT and AMD sources for the current dataset. Excluding it removes all 606 annotation duplicates.

The synthetic annotation ID design (`{law_name}:{code_type}:{seq}`) would also prevent this class of duplicate, but the underlying data quality issue remains. Better to fix the parser than to paper over broken data.

---

## Cross-Jurisdiction Validation (Critical Issue #3)

Examined all 7 non-UK LAT source files (`xLAT-*.csv`) to confirm the citation-based `section_id` design generalises. Every jurisdiction surveyed uses letter-suffix insertion for amendments — the pattern is universal.

### Amendment Insertion Patterns by Jurisdiction

| Jurisdiction | Symbol | Placement | Inserted provision example | Inserted chapter |
|---|---|---|---|---|
| **UK** | s./reg./art. | word before number | s.3A, s.3ZA, reg.2A | — |
| **Germany (DE)** | § | § before number (`§ 3`) | §5a | — |
| **Norway (NO)** | § | § before number (`§ 3.`) | §16 a., §16 d. through §16 h. | Kapittel 3A, 7A |
| **Turkey (TUR)** | Madde | word before number (`Madde 3`) | Madde 27/A (slash notation) + Ek Madde N (supplementary series) | — |
| **Austria (AUT)** | § | § before number (`§ 3`) | §4a, §4b, §7a | — |
| **Denmark (DK)** | § | § before number (`§ 1.`) | §72 a, §7a, §7b, §7c | Kapitel 11 a |
| **Finland (FIN)** | § | number before § (`1 §`) | 13 h § | 3a luku (Chapter 3a) |
| **Sweden (SWE)** | § | number before § (`1 §`) | 3 a § (expected; tends to reprint/consolidate) | — |

### Structural Citation Examples by Jurisdiction

The `{law_name}:{citation}` format adapts per-jurisdiction with a jurisdiction-specific citation prefix:

| Jurisdiction | Example `section_id` | Notes |
|---|---|---|
| UK | `UK_ukpga_1974_37:s.25A(1)` | `s.` for section, `reg.` for regulation |
| DE | `DE_2020_ArbSchG:§5a.Abs.1` | `§` for article, `Abs.` for paragraph |
| NO | `NO_1973_03-09-14:§16a` | `§` for section, space+period in source |
| TUR | `TUR_1983_2872:m.27/A` | `m.` for madde; slash preserved |
| TUR (supplementary) | `TUR_1983_2872:ek.5` | `ek.` for Ek Madde (supplementary article) |
| AUT | `AUT_2005_121:§4a` | Same as DE |
| DK | `DK_2020_1406:§72a.stk.2` | `stk.` for Stykke (subsection) |
| FIN | `FIN_1994_719:§13h` | Number-before-symbol convention |
| SWE | `SWE_2020_1:§3a` | Number-before-symbol convention |

### Key findings

1. **All jurisdictions use letter-suffix insertion.** The three-column design (structural citation + sort key + position) works universally because every jurisdiction has a canonical, stable way to cite a provision that accommodates amendments.

2. **Turkey is the only outlier** — it has two insertion mechanisms: slash notation (`Madde 27/A`) and a separate supplementary article series (`Ek Madde N`) that lives after the main body. Both are representable as citations. The sort key needs a rule for placing `ek.N` after the main article sequence.

3. **Sort key normalisation is jurisdiction-specific** but the structure is the same everywhere: zero-padded numeric base + letter suffix range. Each jurisdiction needs a mapping from its naming conventions to the normalised encoding, but the three-column design holds across all of them.

4. **Finland and Sweden reverse the symbol placement** (`1 §` instead of `§ 1`), but the citation in `section_id` can normalise to a consistent format regardless of source rendering.

5. **Norway shows the most aggressive amendment insertion** — runs of `§16 d` through `§16 h` inserted as a block by a single amending act, plus inserted chapters like `Kapittel 3A`. The sort key encoding handles this identically to UK's pattern.

### Conclusion

The citation-based `section_id` design generalises to all surveyed jurisdictions. The only jurisdiction-specific element is the citation prefix mapping (what prefix to use for the provision type), which is already captured by the `section_type` column. **Critical Issue #3 is resolved.**

---

## Investigation: Heading Column

SCHEMA-2.0 described the `heading` column as "a counter (1, 2, 3...)" and recommended renaming to `heading_group` or `heading_idx`. The counter characterisation was wrong — the column is more nuanced than that.

### What the column actually contains

The `heading` column is a **group membership label** cascaded from parent cross-heading rows to all their descendant content rows. Its value is the **first section/article number under that cross-heading**. For HSWA 1974 (Part I):

| heading value | cross-heading text | sections covered |
|---|---|---|
| `1` | "Preliminary" | s.1 only |
| `2` | "General duties" | s.2–s.9 |
| `18` | "Enforcement" | s.18–s.26 |
| `27` | "Obtaining and disclosure of information" | s.27–s.28 |
| `29` | "Special provisions relating to agriculture" | s.29–s.32 |
| `33` | "Provisions as to offences" | s.33–s.42 |

Values jump (1 → 2 → 18 → 27) because they track the lead section number, not a sequential index. The column is VARCHAR with **415 distinct values** including:
- Numeric: `1` through `315`
- Alpha-suffixed: `10A`, `11A`, `25A`, `19AZA`, `19BA`
- Single letters: `A`, `D`, `I`, `M`, `N`, `P`, `T` (Victorian-era Acts, NI regulations)
- Dotted decimals: `1.1`, `2.1`, `2.6`, `3.1`, `7.2` (NI safety sign regulations)
- Data artifact: `F107` (leaked Westlaw footnote reference — 1 row)

### Coverage

- **63,419 rows** (64%) have heading populated
- **35,694 rows** (36%) have heading NULL — titles, parts, chapters, schedules, signed blocks, notes, and content in laws/parts without cross-headings
- **19 laws** have zero heading-type rows at all (including Water Scotland Act 1980, 247 rows)

### Two kinds of heading-type rows

The `section_type = 'heading'` rows serve two distinct roles:

| Role | Count | heading column | section column | Description |
|---|---|---|---|---|
| **Cross-heading** | 10,316 | populated | NULL | Groups multiple sections: "General duties" spanning s.2–s.9 |
| **Section-title** | 4,182 | NULL | populated | Per-section title line preceding a single section's content |
| **Orphan** | 9 | NULL | NULL | All from UK_uksi_2001_2954 (Oil Storage Regulations) — structural grouping rows with heading column never populated |

Cross-headings cascade their value to all descendant rows. Section-title headings don't — they're standalone title lines for individual provisions (common in SIs and older regulations).

### Edge cases

- **Heading resets at schedule boundaries**: A law's body might end with heading=62, then the schedule starts fresh with heading=1. The heading column is scoped to `(law_name, part/schedule)`, not globally.
- **Consecutive heading rows**: 20 cases where two heading-type rows appear with no content between them (amendment SIs, schedule references, territorial duplication artifacts).
- **Scrambled position ordering**: Some NI SIs have rows in non-logical position order, but the heading column still correctly identifies group membership.

### Recommendation: Rename to `heading_group`

The column semantics are sound — it's a genuine group membership label that correctly identifies which cross-heading a provision falls under. The name `heading` is the problem: it reads as "heading text" when it's actually "which heading group am I in".

**Rename `heading` → `heading_group`.** No structural change, no value transformation, just a name that accurately describes the column's role. The `heading_idx` alternative is worse because it implies a sequential index, which this is not.

Document that:
- Values are the lead section/article number of the parent cross-heading (not a counter)
- Scoped to `(law_name, part/schedule)` — resets at schedule boundaries
- NULL for rows outside any cross-heading group (title, part, chapter, schedule, etc.)
- The heading **text** lives in the `text` column of rows with `section_type = 'heading'`

---

## Session Goal

**Get a clean, usable LAT baseline for development** — not perfect, not multi-jurisdiction, but correct enough to unblock LanceDB ingestion (Task 4) and eventually Phase 2 embeddings.

## Tasks

### 1. Revise the LAT schema
- [x] Apply the three-column identity design: `section_id` (structural citation), `sort_key` (normalised sort), `position` (snapshot integer) — `schema.rs` updated, LAT now 28 cols
- [x] Rename `heading` → `heading_group`, merge `section`/`article` → `provision` — 7 hierarchy cols (was 8)
- [x] New annotation `id` = `{law_name}:{code_type}:{seq}`, add `source` column — annotations now 9 cols
- [x] Update `docs/SCHEMA.md` Tables 3 and 4 — sections 3.1, 3.2, 3.7, 4.1, column counts, migration path
- [x] Update `crates/fractalaw-core/src/schema.rs` — `legislation_text_schema()` and `amendment_annotations_schema()`
- [x] Update unit tests in schema.rs — 21 tests, all passing
- Commit: `471e161` — pushed to origin/master

### 2. Build sort key generation
- [x] Implement sort key normalisation rules for UK naming conventions — `normalize_provision()` in `sort_key.rs`
- [x] Handle: plain numeric, single letter suffix (A=010..Z=260), Z-prefix (ZA=001..ZZ=026), double letter, combined patterns
- [x] Validate against known ordering: Environment Act s.40→41→41A→41B→41C→42, Z-prefix chains, double letters — 13 tests, all passing
- [x] Also: `with_extent()` for parallel territorial provisions (extent qualifier appended with `~`)
- Commit: `8d612c6` — pushed to origin/master

### 3. Rewrite the LAT export script

Broken into incremental stages. Each stage can be tested independently before moving on.

#### 3a. Stage 0–1: Macros and source loading
- [x] Keep existing source loading (Stage 0) — exclusion filter applied downstream in export queries
- [x] Keep existing macros: `strip_acronym`, `strip_id`, `map_section_type`, `map_code_type`, `extract_code`, `region_to_extent`, `count_codes`, `annotation_parent_id`, `is_content_row`
- [x] Add `normalize_provision` as DuckDB macros (5 helpers + main, matching `sort_key.rs`): `prov_base`, `prov_suffix`, `suffix_val`, `suffix_len`, `normalize_provision`
- [x] Add `provision_prefix(section_type, class)` macro — returns `s.`/`reg.`/`art.` or NULL
- [x] Add `build_citation(section_type, class, provision, sub, para, part, chapter, heading_group, schedule, pos)` macro
- [x] Add `build_sort_key(section_type, provision, heading_group, para, schedule)` macro
- [x] Modify `build_hierarchy` to output `provision.` instead of `section.` in hierarchy_path labels
- [x] Test: all macro smoke tests pass; full script runs end-to-end with modified `build_hierarchy`

#### 3b. Stage 2: Pre-computations
- [x] Keep existing `cie_counts` table
- [x] Add `parallel_provisions` table — 937 provision pairs across 53 laws (more than the session doc's ~29 estimate because sub_section rows bring in additional extent variations)
- [x] Add `content_id_map` table — 97,544 rows, all distinct section_ids (zero duplicates)
- [x] Fix: partition position numbering by `strip_acronym("UK")` not raw `"UK"` (7 laws have multiple acronym variants across CSV files)
- [x] Fix: `build_citation` schedule-prefix (`sch_prefix` helper) for headings/parts/chapters inside schedules
- [x] Fix: `build_citation` position-qualify `title`/`signed`/`commencement` (can have multiples per law)
- [x] Fix: position fallback when provision is empty for section/article types
- [x] Disambiguation: 2,206 rows (2.3%) get `#pos` suffix where structural citation alone collides (heading_group/part/chapter resets across parts within a law)
- [x] Test: HSWA citations look correct (`s.1`, `s.25A(1)`, `s.23(4)[E+W]`); zero duplicate section_ids

#### 3c. Stage 3: Export legislation_text.parquet (28 cols)
- [x] 28 columns matching `schema.rs` `legislation_text_schema()` order — verified exact match
- [x] New identity columns: `section_id` (citation-based), `sort_key` (normalised), `legacy_id` (old positional ID)
- [x] Renamed/merged: `heading` → `heading_group`, `section`/`article` → `provision`
- [x] Filters: NULL text rows excluded (249), `UK_uksi_2016_1091` excluded (~1,342 rows) → 97,522 rows from 452 laws
- [x] Extent qualifier `[extent]` on section_id for parallel territorial provisions (e.g., `s.23(4)[E+W]`)
- [x] Extent suffix `~extent` on sort_key for parallel provisions (e.g., `023.000.000~E+W`)
- [x] `#pos` disambiguation for 2,206 structural collisions (heading_group/part/chapter resets across law parts)
- [x] ORDER BY law_name, sort_key, position — correct document order confirmed
- [x] Zero `section_id` duplicates; sort_key ordering matches position for inserted sections (s.41→41A→41B→41C→42)

#### 3d. Stage 4–5: Annotations export (9 cols) ✅
- [x] Stage 4: Build `affected_sections` — remap old IDs → new citation-based `section_id` via `content_id_map` (INNER JOIN)
- [x] Stage 5: Export `amendment_annotations.parquet` with 9 columns matching `schema.rs`
- [x] Synthetic `id`: `{law_name}:{code_type}:{seq}` (per-law, per-code_type sequence number)
- [x] New `source` column: `lat_cie` (7,522), `lat_f` (588), `amd_f` (11,341)
- [x] New `affected_sections` with citation-based section_ids
- [x] `UK_uksi_2016_1091` excluded
- [x] Test: 19,451 rows from 137 laws, zero `id` duplicates

#### 3e. Stage 6: Export annotation_totals.parquet ✅
- [x] Exclude `UK_uksi_2016_1091` (added filter to all 3 CTEs)
- [x] Test: 135 laws

#### 3f. Stage 7: Verification queries ✅
- [x] All existing verification checks retained
- [x] Add: `section_id` uniqueness → 0 duplicates
- [x] Add: annotation `id` uniqueness → 0 duplicates
- [x] Add: annotation source breakdown
- [x] Add: sample annotation IDs
- [x] Add: sort key ordering vs position ordering for test laws (Environment Act 1995)
- [x] Add: sample output showing new `section_id` and `sort_key` for HSWA 1974
- [x] Add: sample showing parallel provisions with `[extent]` qualifiers

### 4. Re-export LAT Parquet files ✅
- [x] Run revised export script — clean end-to-end run
- [x] Validate row counts: 97,522 content, 19,451 annotations, 135 annotation totals
- [x] Verify FK linkage: 405/452 LAT laws matched LRT, 47 unmatched (LAT-only laws, known)
- [x] Verify `ORDER BY sort_key, position` recovers correct document order for inserted sections (s.41→41A→41B→41C→42)
- [x] UK_uksi_2016_1091 excluded from all 3 Parquet files (zero leaked rows)
- [x] Annotation affected_sections FK: 18,025/18,225 matched (200 unmatched = NULL-text rows in content_id_map but excluded from legislation_text, acceptable edge case)
- [x] Sort key reversals (9,237) are expected structural resets — headings/parts/schedules reset to 000.000.000 when crossing boundaries

### 5. Smoke test with DuckDB ✅
- [x] Load `legislation_text.parquet` into DuckDB alongside existing legislation + law_edges (all 4 tables loaded: 19,318 + 1,035,305 + 97,522 + 19,451 rows)
- [x] Cross-table query: HSWA 1974 section 25A — returns 4 rows (section + 3 sub_sections), correct citation-based section_ids
- [x] Annotation query: COSHH F-code amendments — 15 rows shown, correct `reg.` prefix for SI provisions (e.g. `UK_uksi_2002_2677:reg.2(1)`)
- [x] Cross-table join: HSWA F1 annotation → affected section text via `affected_sections` unnest + JOIN — works correctly
- [x] `section_id` uniqueness: zero duplicate rows returned
- [x] Sort order: `ORDER BY sort_key` and `ORDER BY position` produce identical ordering for Environment Act 1995 sections 40–42 (including inserted 41A, 41B, 41C)

### 6. Update SCHEMA.md and close out ✅
- [x] Three-column identity design and sort key rules already documented in SCHEMA.md §3.1 (from Task 1 schema revision)
- [x] Added deviations table from SCHEMA-2.0 recommendations (9 items: 6 done, 3 deferred)
- [x] Updated migration path: all 3 steps marked done with current row counts
- [x] Updated version 0.2 → 0.3, status → "All four tables exported to Parquet and validated"

## Reference: LAT Record Shape (Revised)

Each record is one structural unit of a law:

```
law_name       — parent law identifier (FK to LRT)
section_id     — structural citation: {law_name}:s.25A(1) (STABLE across amendments)
sort_key       — normalised sort string: 025.010.001 (lexicographic document order)
position       — snapshot integer index (1-based, reassigned on re-export)
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
legacy_id      — original Airtable positional encoding (nullable, for migration only)
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

## Progress

| Date | What | Commit |
|------|------|--------|
| 2026-02-19 | Design: three-column identity, parallel territorial provisions, cross-jurisdiction validation, heading column investigation, UK_uksi_2016_1091 exclusion | — (session doc) |
| 2026-02-19 | Task 1: Revise LAT + annotation schemas in `schema.rs` and `SCHEMA.md` | `471e161` |
| 2026-02-19 | Task 2: Sort key normalisation — `normalize_provision()` + `with_extent()`, 13 tests | `8d612c6` |
| 2026-02-19 | Tasks 3–6: Rewrite LAT export, validate, smoke test, update SCHEMA.md v0.3 | `daebf33` |

## Session Closed

**Date**: 2026-02-19
**Status**: All 6 tasks complete. LAT baseline is clean and usable for development.

**Final outputs**:
- `data/legislation_text.parquet` — 97,522 rows, 452 laws, 28 cols, zero duplicate `section_id`
- `data/amendment_annotations.parquet` — 19,451 rows, 137 laws, 9 cols, zero duplicate `id`
- `data/annotation_totals.parquet` — 135 laws
- `docs/SCHEMA.md` v0.3 — all four tables exported and validated
- `crates/fractalaw-core/src/schema.rs` — schemas match exports, 21 tests passing

**Unblocks**: Issue #11 (Phase 2: ONNX embeddings, semantic search, LanceDB integration) — LAT cleanup was the blocker, now resolved.
