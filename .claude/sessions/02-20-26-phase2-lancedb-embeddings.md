# Session: 2026-02-20 — Phase 2: LanceDB Ingestion and ONNX Embeddings

## Context

Phase 1 is complete ([`02-12-26-begin.md`](02-12-26-begin.md)). The LAT schema revision is complete ([`02-19-26-LAT-schema.md`](02-19-26-LAT-schema.md)). The blocker for Phase 2 — clean LAT data — is resolved.

**GitHub Issue**: [#11 — Phase 2: ONNX embeddings, semantic search, and LanceDB integration](https://github.com/fractalaw/fractalaw/issues/11)

### What Exists

| Component | Status | Details |
|-----------|--------|---------|
| `fractalaw-core/src/schema.rs` | Done | `legislation_text_schema()` (28 cols), `amendment_annotations_schema()` (9 cols) |
| `data/legislation_text.parquet` | Done | 97,522 rows, zero duplicate `section_id`s |
| `data/amendment_annotations.parquet` | Done | 19,451 rows, zero duplicate `id`s |
| `fractalaw-store` — DuckDB | Done | `DuckStore` (hot + analytical paths), `FusionStore` (DataFusion query layer) |
| `fractalaw-store` — LanceDB dep | Declared | `lancedb = "0.26"` in workspace, feature-gated, **no code** |
| `fractalaw-ai` — ONNX dep | Declared | `ort = "2.0.0-rc.11"` in workspace, feature-gated, **no code** |
| CLI | Done | 4 commands (`stats`, `law`, `graph`, `query`), loads DuckDB only |

### Three Access Tiers (Architecture)

```
┌──────────────────────────────────────────────────┐
│  HOT PATH — DuckDB: legislation (19,318 rows)    │  ← Done
│  ANALYTICAL PATH — DuckDB: law_edges (1,035,305) │  ← Done
├──────────────────────────────────────────────────┤
│  SEMANTIC PATH — LanceDB: legislation_text       │  ← This session
│                  LanceDB: amendment_annotations   │  ← This session
│                  + ONNX embeddings (384-dim)      │  ← This session
└──────────────────────────────────────────────────┘
```

## Tasks

### Task 1: Implement LanceStore in fractalaw-store — [ ]

Add `lance.rs` module to `fractalaw-store` behind `lancedb` feature gate.

**API surface:**

```rust
pub struct LanceStore {
    db: lancedb::Connection,
}

impl LanceStore {
    pub async fn open(path: &Path) -> Result<Self, StoreError>;
    pub async fn create_legislation_text(&self, parquet: &Path) -> Result<(), StoreError>;
    pub async fn create_amendment_annotations(&self, parquet: &Path) -> Result<(), StoreError>;
    pub async fn load_all(&self, data_dir: &Path) -> Result<(), StoreError>;
    pub async fn legislation_text(&self) -> Result<lancedb::Table, StoreError>;
    pub async fn amendment_annotations(&self) -> Result<lancedb::Table, StoreError>;
    pub async fn search_text(&self, query_vector: &[f32], limit: usize)
        -> Result<Vec<RecordBatch>, StoreError>;
}
```

**Key decisions:**
- Database path: `{data_dir}/lancedb/` (beside Parquet files)
- Ingest from Parquet via DuckDB → Arrow → LanceDB (reuse DuckDB reader for schema compatibility)
- Or direct: read Parquet into RecordBatch via `arrow::ipc` / `parquet` crate
- LanceDB auto-creates IVF-PQ index on `FixedSizeList<Float32, 384>` columns

**Files:**
- New: `crates/fractalaw-store/src/lance.rs`
- Edit: `crates/fractalaw-store/src/lib.rs` (register module)
- Edit: `crates/fractalaw-store/src/error.rs` (add LanceDB error variant)

**Tests:**
- Open/create database
- Ingest legislation_text.parquet → count rows
- Ingest amendment_annotations.parquet → count rows
- Table schema matches `fractalaw-core` definitions

### Task 2: Register LanceDB tables in FusionStore — [ ]

Extend `FusionStore` to register LanceDB tables as DataFusion `TableProvider`s alongside DuckDB tables. This enables cross-store SQL queries.

**Approach:**
- LanceDB's Lance format has [native DataFusion integration](https://lancedb.github.io/lance/integrations/datafusion/) via `lance::dataset::Dataset` implementing `TableProvider`
- Alternative: read from LanceDB table into RecordBatch, wrap in MemTable (simpler, same pattern as DuckDB)
- Start with MemTable approach (consistent with existing pattern), optimise later if needed

**Files:**
- Edit: `crates/fractalaw-store/src/fusion.rs`
- Edit: `crates/fractalaw-store/Cargo.toml` (fusion needs lancedb when both features enabled)

**Tests:**
- `SELECT count(*) FROM legislation_text` works
- `SELECT section_id, text FROM legislation_text WHERE law_name = 'UK_ukpga_1974_37' LIMIT 5`
- Cross-table join: `legislation JOIN legislation_text USING (law_name)` ← this is the semantic+hot path union

### Task 3: Implement ONNX embedding pipeline in fractalaw-ai — [ ]

Build the embedding generation pipeline using ONNX Runtime.

**Model**: `all-MiniLM-L6-v2` (384 dimensions, matches schema)
- ~80MB ONNX model file
- Tokenizer: WordPiece (BERT-based), max 256 tokens
- Download from Hugging Face, store in `models/` directory

**API surface:**

```rust
pub struct Embedder {
    session: ort::Session,
    tokenizer: tokenizers::Tokenizer,
}

impl Embedder {
    pub fn load(model_dir: &Path) -> Result<Self>;
    pub fn embed(&self, text: &str) -> Result<Vec<f32>>;
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}
```

**Dependencies to add:**
- `ort` (already declared) — ONNX Runtime
- `tokenizers` — Hugging Face tokenizer library (for WordPiece tokenization)

**Files:**
- Edit: `crates/fractalaw-ai/src/lib.rs`
- New: `crates/fractalaw-ai/src/embedder.rs`
- Edit: `crates/fractalaw-ai/Cargo.toml`

**Tests:**
- Load model
- Embed single text → 384-dim vector
- Embed batch → N x 384 vectors
- Vectors are normalised (L2 norm ≈ 1.0)

### Task 4: Batch-embed LAT text into LanceDB — [ ]

Read all 97,522 text rows from LanceDB, generate embeddings via ONNX, write back.

**Approach:**
- Stream rows from `legislation_text` table
- Batch embed (e.g., 256 rows at a time)
- Merge/update embedding columns: `embedding`, `embedding_model`, `embedded_at`
- LanceDB supports `merge_insert` for upsert operations

**Performance estimate:**
- all-MiniLM-L6-v2: ~1ms/text on CPU → ~97s for 97K rows (single-threaded)
- With batching (256): ~380 batches × overhead → ~2-5 minutes total

**Files:**
- Could be a function in `fractalaw-store` or a CLI command
- Likely: CLI `fractalaw embed` command that wires store + AI

### Task 5: Add CLI commands for semantic path — [ ]

Extend CLI with LAT-related commands:

```
fractalaw text <law_name>       — show sections for a law (from LanceDB)
fractalaw search "query text"   — semantic search across all legislation text
fractalaw embed                 — run batch embedding pipeline
```

**Files:**
- Edit: `crates/fractalaw-cli/src/main.rs`
- Edit: `crates/fractalaw-cli/Cargo.toml` (enable lancedb + onnx features)

### Task 6: Validation and smoke tests — [ ]

- Cross-store query: "Find all amendments to HSWA section 25A with their text and annotation details"
- Semantic search: "chemical exposure limits" returns COSHH regulations
- Embedding coverage: all non-title rows have non-null embeddings
- Row counts match Parquet source files

## Design Decisions

### LanceDB Storage Location

`{data_dir}/lancedb/` — a subdirectory of the data dir, beside the Parquet files. LanceDB creates its own Lance-format files. The Parquet files remain the source of truth for re-ingestion.

### Parquet → LanceDB Ingestion Path

Two options:

1. **Direct Parquet read**: Use `parquet` crate to read Parquet → RecordBatch → LanceDB `create_table`
2. **DuckDB bridge**: Load Parquet into DuckDB, query as Arrow, pipe to LanceDB

Option 1 is simpler and avoids the DuckDB dependency for ingestion. LanceDB natively accepts RecordBatch streams from any Arrow source.

**Decision**: Option 1 (direct Parquet read). DuckDB is for queries, not ETL.

### Embedding Model Selection

`all-MiniLM-L6-v2`:
- 384 dimensions (matches schema's `FixedSizeList<Float32, 384>`)
- ~22M parameters, ~80MB ONNX file
- Trained on legal/technical text (BERT-based)
- Widely benchmarked, good balance of speed vs quality
- Runs efficiently on CPU via ONNX Runtime

Alternative: `all-mpnet-base-v2` (768-dim, higher quality but 2x vector size). Would require schema change. Defer.

### Cross-Store Query Strategy

Phase 1 approach (MemTable bridge) works for the current dataset size:
- 97K LAT rows × 28 columns ≈ 50MB in memory (compressed text)
- Load into MemTable on FusionStore creation, DataFusion handles joins

For larger datasets, use Lance's native DataFusion `TableProvider` (zero-copy scan from Lance files). This is a future optimisation.

## Progress

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 1. LanceStore | [x] | | 8 tests pass: open, create, load_all, query, schema, reload, error |
| 2. FusionStore LAT | [x] | | 5 tests: count, query, cross-store join, 4-table check |
| 3. ONNX Embedder | [x] | | 5 tests pass: load, embed_single, embed_batch, similar_closer, empty_batch |
| 4. Batch embed | [x] | | 97,522 rows embedded; 195MB Lance table; 0 null embeddings; L2 norm ≈ 1.0 |
| 5. CLI commands | [ ] | | |
| 6. Validation | [ ] | | |
