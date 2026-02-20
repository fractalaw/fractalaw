# Session: 2026-02-20 — Issue #13: AI Classification Pipeline

## Context

**GitHub Issue**: [#13 — AI classification pipeline for domain/family/sub_family](https://github.com/fractalaw/fractalaw/issues/13)

The embedding infrastructure is complete (Phase 2). All 97,522 legislation_text rows in LanceDB have 384-dim `all-MiniLM-L6-v2` embeddings and pre-tokenized token IDs. The classification pipeline consumes these embeddings to label laws by domain, family, sub_family, and subjects.

### What Exists

| Component | Location | Status |
|-----------|----------|--------|
| Embeddings | `data/lancedb/legislation_text.lance` | 97,522 rows, 384-dim, 100% coverage |
| Tokenizer | `crates/fractalaw-ai/src/embedder.rs` | `tokenize()`, `tokenize_batch()`, `id_to_token()` |
| LanceDB store | `crates/fractalaw-store/src/lance.rs` | `search_text()`, `query_legislation_text()`, `create_table_from_batches()` |
| Classification columns (law level) | `legislation` table | `domain`, `family`, `sub_family`, `subjects` — partially populated |
| LAT CSV exports | `data/LAT-*.csv` (17 files) | Family-annotated text sections from legacy system |
| AMD CSV exports | `data/AMD-*.csv` (16 files) | Amendment annotations by family |

### Classification Coverage (legislation table — law level)

| Column | Populated | Total | Coverage | Distinct Values |
|--------|-----------|-------|----------|-----------------|
| `domain` | 13,157 | 19,318 | 68% | 6 (incl. combinations) |
| `family` | 13,322 | 19,318 | 69% | 53 |
| `sub_family` | 1,449 | 19,318 | 7.5% | 43 |
| `subjects` | 7,908 | 19,318 | 41% | 3,987 combinations |

### Domain Values (3 core + combinations)

| Domain | Count |
|--------|-------|
| environment | 9,665 |
| health_safety | 2,822 |
| governance | 342 |
| human_resources | 297 |
| health_safety + environment | 31 |

### Family Distribution (top 20 of 53)

| Family | Count |
|--------|-------|
| AGRICULTURE | 1,020 |
| WILDLIFE & COUNTRYSIDE | 862 |
| ANIMALS & ANIMAL HEALTH | 830 |
| ENERGY | 740 |
| FISHERIES & FISHING | 652 |
| PLANT HEALTH | 597 |
| WASTE | 568 |
| HEALTH: Coronavirus | 554 |
| WATER & WASTEWATER | 474 |
| TOWN & COUNTRY PLANNING | 468 |
| OH&S: Occupational / Personal Safety | 451 |
| PLANNING & INFRASTRUCTURE | 404 |
| TRANSPORT: Harbours & Shipping | 403 |
| CLIMATE CHANGE | 392 |
| ENVIRONMENTAL PROTECTION | 382 |
| FOOD | 373 |
| X: No Family | 368 |
| POLLUTION | 355 |
| MARINE & RIVERINE | 297 |
| TRANSPORT: Maritime Safety | 280 |

### Subjects (md_subject) — Historical Classification Scheme

The `subjects` column (originally `md_subjects` in the PostgreSQL source) is a List\<Utf8\> containing free-text topic labels. The scheme was actively maintained from 1987–2012, then stopped mid-2013. Coverage: 7,908 of 19,318 laws (41%).

Top values show fine-grained topic labels that cut across the family taxonomy:

| Subject | Count |
|---------|-------|
| local government | 293 |
| environmentally sensitive areas | 149 |
| legislation | 147 |
| pollution | 93 |
| fisheries and aquaculture | 87 |
| vehicles | 84 |
| environmental protection | 78 |
| food standards | 66 |
| smoke | 58 |
| rural development | 50 |

These subjects provide useful sub-family-level categorization that the domain/family hierarchy misses. Reviving and extending them to post-2013 laws is an explicit goal of this pipeline.

### LAT CSV Files (17 families × ~300K total rows)

The LAT CSVs are family-annotated exports from the legacy system. Each file's name encodes the family:

| File | Rows | Family Mapping |
|------|------|----------------|
| LAT-Climate-Change.csv | 37,603 | CLIMATE CHANGE |
| LAT-Consumer-Product-Safety.csv | 11,362 | PUBLIC: Consumer / Product Safety |
| LAT-Dangerous-and-Explosive.csv | 21,905 | FIRE: Dangerous and Explosive Substances |
| LAT-Energy.csv | 37,343 | ENERGY |
| LAT-Environmental-Protection.csv | 21,093 | ENVIRONMENTAL PROTECTION |
| LAT-Fire.csv | 4,172 | FIRE |
| LAT-Gas-Electrical.csv | 9,343 | OH&S: Gas & Electrical Safety |
| LAT-Marine-Riverine.csv | 22,470 | MARINE & RIVERINE |
| LAT-Mine-Quarry.csv | 10,838 | OH&S: Mines & Quarries |
| LAT-Offshore.csv | 7,176 | OIL & GAS / OH&S: Offshore Safety |
| LAT-OH-and-S.csv | 46,600 | OH&S: Occupational / Personal Safety |
| LAT-Planning.csv | 24,694 | PLANNING & INFRASTRUCTURE |
| LAT-Pollution.csv | 12,562 | POLLUTION |
| LAT-Radiological-Safety.csv | 5,597 | NUCLEAR & RADIOLOGICAL |
| LAT-Waste.csv | 11,710 | WASTE |
| LAT-Water.csv | 17,711 | WATER & WASTEWATER |
| LAT-Wildlife-Countryside.csv | 15,833 | WILDLIFE & COUNTRYSIDE |

Columns: `ID`, `UK` (law_name), `Region`, `Class`, `flow`, `Record_Type`, `Part`, `Chapter`, `Heading`, `Section||Regulation`, `Sub_Section||Sub_Regulation`, `Paragraph`, `Dupe`, `Text`, `Amendment`, `Changes`, ...

The `UK` column matches `law_name` in the legislation_text table. The LAT CSVs confirm which laws belong to which family — useful as a second label source alongside the legislation table's `family` column.

## Classification Unit: The Law

Classification labels (domain, family, sub_family, subjects) are **per-law metadata**. They do not vary across sections/regulations within a law. A single Act or SI gets one family, one domain, one set of subjects.

The text sections in `legislation_text` (97,522 rows across ~452 laws) provide the **embedding signal** for classification. To classify a law, we aggregate its section embeddings into a single law-level embedding, then compare against labelled centroids.

```
legislation_text (97K rows, 384-dim embeddings)
    ↓ group by law_name, mean-pool
law-level embeddings (~452 laws in LanceDB, 19,318 in legislation table)
    ↓ cosine similarity to centroids
classification labels → legislation table
```

There is no expectation to label individual sections or regulations.

## Strategy: Centroid-Based Classification

### Why Centroids

The embeddings already capture semantic similarity. With 13K+ labelled laws (69% family coverage), we have abundant training signal. A centroid-based approach:

1. **Simple**: No neural training, no hyperparameters
2. **Fast**: One dot product per candidate label (53 families × 384 dims = trivial)
3. **Interpretable**: Distance to centroid is a natural confidence score
4. **Incrementally improvable**: Can upgrade to k-NN or MLP classifier later without changing the schema

### Law-Level Embedding Aggregation

For each law, compute a single representative embedding:

```
law_embedding[name] = L2_normalize(mean(section_embeddings where law_name = name))
```

This captures the overall semantic content of the law. Laws with many sections (large Acts) naturally weight toward their dominant themes.

### Hierarchical Classification

Classify in three passes:

1. **Domain** (3 core classes): Coarse bucket — environment, health_safety, governance/HR
2. **Family** (53 classes): Primary classification within domain
3. **Subjects** (multi-label): Fine-grained topic tags, multi-label assignment

Sub_family has only 7.5% coverage — too sparse for centroid training. It can be addressed later with manual curation or by deriving from family+subjects.

### Confidence Scoring

For each classification, store the cosine similarity to the assigned centroid. This enables:
- Filtering low-confidence predictions for human review
- Setting thresholds per classification level
- Tracking classification quality over time

## Tasks

### Task 1: Build Ground-Truth Label Sets

Extract labelled training data from the `legislation` table in DuckDB:

```sql
SELECT name, domain, family, sub_family, subjects
FROM legislation
```

The LAT CSV data has already been loaded into LanceDB as `legislation_text`. No CSV re-parsing needed — the legislation table is the single source of truth for classification labels.

**Output**: A `LabelSet` struct in `fractalaw-ai` containing:
- `law_family: HashMap<String, String>` — law_name → family (single-select)
- `law_sub_family: HashMap<String, String>` — law_name → sub_family (single-select)
- `law_domain: HashMap<String, Vec<String>>` — law_name → domains (multi-select)
- `law_subjects: HashMap<String, Vec<String>>` — law_name → subjects (multi-select)

Constructed from Arrow RecordBatches (what `DuckStore::query_arrow()` returns).

**Implementation**: New file `crates/fractalaw-ai/src/labels.rs`.

### Task 2: Compute Law-Level Embeddings and Centroids

Two steps: aggregate section embeddings into law-level embeddings, then compute centroids per label.

**Step 2a — Law-level embeddings**:
```
For each law_name in legislation_text:
    sections = all embeddings where law_name = name
    law_embedding = L2_normalize(mean(sections))
```

This produces one 384-dim vector per law, representing its overall semantic content.

**Step 2b — Centroids per label**:
```
centroid[family] = L2_normalize(mean(law_embeddings for all laws with that family))
```

Do the same for domain (3 centroids) and subjects (one centroid per distinct subject tag).

**Implementation**: New methods in `crates/fractalaw-ai/src/classifier.rs`.

**Edge cases**:
- `X: No Family` and `_todo` labels (573 laws) — exclude from centroid computation
- Multi-domain laws (31 entries with combined domains) — contribute to both domain centroids
- Multi-value subjects — each subject tag gets its own centroid; a law with `[pollution, water pollution]` contributes to both
- Laws in legislation table but not in legislation_text (47 LAT-only laws go the other way; some legislation laws may have no text) — skip, no embedding available

### Task 3: Implement Classifier

Build a `Classifier` struct in `crates/fractalaw-ai/src/classifier.rs`:

```rust
pub struct Classifier {
    domain_centroids: HashMap<String, Vec<f32>>,
    family_centroids: HashMap<String, Vec<f32>>,
    subject_centroids: HashMap<String, Vec<f32>>,
}

pub struct Classification {
    pub law_name: String,
    pub domain: Vec<(String, f32)>,    // multi-select: all above threshold
    pub family: String,                 // single-select: best match
    pub family_confidence: f32,
    pub subjects: Vec<(String, f32)>,   // multi-select: all above threshold
}

impl Classifier {
    /// Classify a single law from its aggregated embedding vector.
    pub fn classify(&self, law_name: &str, embedding: &[f32]) -> Classification { ... }

    /// Classify a batch of laws.
    pub fn classify_batch(&self, laws: &[(&str, &[f32])]) -> Vec<Classification> { ... }
}
```

**Cardinality**:
- `family` — **single-select**: pick the highest-scoring centroid
- `sub_family` — **single-select**: derived from family (future)
- `domain` — **multi-select**: a law can span environment + health_safety (31 examples in data). Return all above threshold.
- `subjects` — **multi-select**: fine-grained topic tags, return all above threshold

Classification logic (per law):
1. Compute cosine similarity to all domain centroids → return all above threshold → domain list + confidences
2. Compute cosine similarity to all family centroids → pick highest → single family + confidence
3. Compute cosine similarity to all subject centroids → return all above threshold (e.g., 0.3) → subjects list

### Task 4: Write Classification Results to Legislation Table

AI predictions stored in `classified_*` columns for ALL laws with embeddings — never overwrites ground-truth `domain`/`family`/`subjects`. A `classification_status` column tracks agreement:

| Status | Meaning |
|--------|---------|
| `predicted` | No ground truth — AI prediction is the only classification |
| `confirmed` | Ground truth exists, AI agrees |
| `conflict` | Ground truth exists, AI disagrees — needs human review |

This makes diffs queryable:
```sql
SELECT name, family, classified_family, classification_confidence
FROM legislation WHERE classification_status = 'conflict'
ORDER BY classification_confidence DESC
```

Add to `legislation_schema()` in `fractalaw-core/src/schema.rs`:
```rust
// 1.9 AI Classification (7)
Field::new("classified_domain", list_utf8.clone(), true),   // multi-select (matches domain)
Field::new("classified_family", DataType::Utf8, true),      // single-select (matches family)
Field::new("classified_subjects", list_utf8.clone(), true),  // multi-select (matches subjects)
Field::new("classification_confidence", DataType::Float32, true),
Field::new("classification_model", DataType::Utf8, true),
Field::new("classified_at", timestamp_ns_utc(), true),
Field::new("classification_status", DataType::Utf8, true),  // predicted | confirmed | conflict
```

These columns go on the **legislation** table (DuckDB hot path), not on legislation_text.

### Task 5: CLI `fractalaw classify` Command

Batch classification pipeline:

```
fractalaw classify [--threshold 0.3]
```

**Flow**:
1. Load label sets (Task 1) — from legislation DuckDB table + LAT CSVs
2. Load section embeddings from LanceDB, aggregate to law-level embeddings (Task 2a)
3. Compute centroids from labelled laws (Task 2b)
4. Classify unlabelled laws (Task 3)
5. Write results to legislation table in DuckDB (Task 4)
6. Print summary

**Progress output**:
```
Loading label sets...
  legislation table: 13,322 laws with family labels
  LAT CSVs: 17 files, 452 distinct laws confirmed
Aggregating section embeddings → law-level...
  452 laws with embeddings (from 97,522 sections)
Computing centroids...
  domain: 3 centroids (environment, health_safety, governance)
  family: 51 centroids (excluding X: No Family, _todo)
  subjects: 1,155 centroids
Classifying 5,996 unlabelled laws...
  domain:  5,996 classified (mean confidence 0.82)
  family:  5,996 classified (mean confidence 0.71)
  subjects: 11,410 laws with ≥1 subject (threshold 0.3) — 3,502 newly assigned
Writing to DuckDB...
Done.
```

Note: only ~452 laws currently have text in LanceDB, so classification coverage depends on which laws have embeddings. Laws without text sections cannot be classified by this pipeline.

### Task 6: Validation & Evaluation

Add classification checks to `fractalaw validate`:

1. **Classification coverage**: Count laws with `classified_family` IS NOT NULL
2. **Confidence distribution**: Report mean/median/p10 confidence per level
3. **Agreement with ground truth**: For laws with both ground-truth `family` and `classified_family`, what % agree?
4. **Subject revival**: Count post-2013 laws that now have `classified_subjects`

**Standalone evaluation** (not in validate, just for development):
- Held-out accuracy: take 20% of labelled laws, classify them using only the other 80%, measure accuracy
- Confusion matrix for domain (3×3) and top-10 families
- Subject precision: for laws with known subjects, what fraction of predicted subjects are correct?

## Dependencies

| Dependency | Status | Impact |
|------------|--------|--------|
| Embeddings (Phase 2) | Done | Required — centroids computed from embeddings |
| #7 — Penalty provisions | Parked | Nice-to-have — would add features for classification. Not blocking. |
| #8 — Commencement status | Parked | Nice-to-have — temporal features. Not blocking. |

The centroid-based approach works purely from embeddings, so #7 and #8 are not blockers. They would improve a future MLP/fine-tuned classifier that uses structured features alongside embeddings.

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `crates/fractalaw-ai/src/labels.rs` | Create | Ground-truth label extraction from legislation table + LAT CSVs |
| `crates/fractalaw-ai/src/classifier.rs` | Create | Law-level embedding aggregation, centroid computation, classification logic |
| `crates/fractalaw-ai/src/lib.rs` | Modify | Re-export `labels` and `classifier` modules |
| `crates/fractalaw-ai/Cargo.toml` | Modify | (if needed) |
| `crates/fractalaw-core/src/schema.rs` | Modify | Add 6 classification columns to **legislation** schema (not legislation_text) |
| `crates/fractalaw-cli/src/main.rs` | Modify | Add `classify` command + classification validation checks |
| `crates/fractalaw-store/src/duck.rs` | Modify | May need method to update classification columns on legislation rows |

## Progress

| Task | Status | Notes |
|------|--------|-------|
| 1. Ground-truth labels | [x] | `labels.rs` — LabelSet from Arrow batches |
| 2. Compute centroids | [x] | `classifier.rs` — aggregate_law_embeddings + Classifier::build |
| 3. Classifier | [x] | `classifier.rs` — classify/classify_batch with ClassificationStatus |
| 4. Schema columns | [x] | 7 columns in legislation_schema section 1.13 |
| 5. CLI `classify` | [x] | Full pipeline: labels→embeddings→centroids→classify→DuckDB write |
| 6. Validation | [x] | 4 classification checks added to `fractalaw validate` |

### Task 5 Results (first run)

- **452 laws** classified (those with embeddings in LanceDB)
- **302 confirmed** (67%) — AI agrees with ground truth
- **103 conflicts** (23%) — AI disagrees, flagged for review
- **47 predicted** (10%) — no ground truth, AI-only classification
- **Mean family confidence: 0.859**
- **24 family centroids**, 3 domain centroids, 141 subject centroids
