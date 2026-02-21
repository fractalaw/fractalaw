# Session: 2026-02-20 — Issue #3: Pre-tokenized text column in LAT

## Context

**GitHub Issue**: [#3 — Pre-tokenized text column in LAT](https://github.com/fractalaw/fractalaw/issues/3)

The embedding model (`all-MiniLM-L6-v2`) and its WordPiece tokenizer are now fixed. The tokenizer runs on every `embed` and `search` invocation, and would run again for any future classification pipeline. Pre-tokenizing at ingestion time trades storage for compute.

### What Exists

| Component | File | Status |
|-----------|------|--------|
| `Embedder` with tokenizer | `crates/fractalaw-ai/src/embedder.rs` | Done — `tokenizers::Tokenizer` loaded from `tokenizer.json` |
| `embed_batch()` | `embedder.rs:76` | Tokenizes internally, builds flat tensors, runs ONNX |
| Embed pipeline | `crates/fractalaw-cli/src/embed.rs` | Reads Parquet → embeds text → writes to LanceDB |
| LAT schema | `crates/fractalaw-core/src/schema.rs:171` | 28 columns, no token columns |
| LanceDB `legislation_text` | `data/lancedb/legislation_text.lance` | 97,522 rows, 384-dim embeddings |

### Current Tokenization Flow

```
embed_batch(texts)
  → tokenizer.encode_batch(texts)     ← happens every time
  → build flat i64 tensors
  → ONNX session.run()
  → mean pooling + normalize
```

The `encode_batch` call produces `Encoding` objects with `get_ids()`, `get_attention_mask()`, and `get_type_ids()`. Currently these are used once and discarded. Pre-tokenizing stores `get_ids()` in LanceDB so the tokenization step can be skipped on subsequent passes.

### Tokenizer Details

- Model: `all-MiniLM-L6-v2` (WordPiece, BERT-based)
- Vocab size: 30,522 (fits in UInt16 but UInt32 is safer for future models)
- Max sequence length: 256 tokens (truncation configured in `Embedder::load`)
- Special tokens: `[CLS]` (101), `[SEP]` (102), `[PAD]` (0)

## Tasks

### Task 1: Add `tokenize` and `tokenize_batch` to Embedder — [x]

Expose the tokenizer as a public method, returning token ID lists without running ONNX inference.

**API addition in `embedder.rs`:**

```rust
impl Embedder {
    /// Tokenize a single text, returning token IDs (including [CLS] and [SEP]).
    pub fn tokenize(&mut self, text: &str) -> anyhow::Result<Vec<u32>> {
        let encoding = self.tokenizer.encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
        Ok(encoding.get_ids().to_vec())
    }

    /// Tokenize a batch of texts, returning one token ID list per input.
    pub fn tokenize_batch(&mut self, texts: &[&str]) -> anyhow::Result<Vec<Vec<u32>>> {
        let encodings = self.tokenizer.encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
        Ok(encodings.iter().map(|e| e.get_ids().to_vec()).collect())
    }

    /// Name of the tokenizer model (for the `tokenizer_model` column).
    pub fn model_name(&self) -> &str { ... }
}
```

**Tests:**
- `tokenize` returns non-empty vec starting with `[CLS]` (101) and ending with `[SEP]` (102)
- `tokenize_batch` returns correct count
- Token count ≤ 256 for long text (truncation works)
- Empty text returns `[CLS]` + `[SEP]` only (2 tokens)

### Task 2: Add token columns to LAT schema — [x]

Add two new columns to `legislation_text_schema()` in `fractalaw-core/src/schema.rs`:

```rust
// 3.6 Pre-tokenized Text (2) — after Embeddings section
Field::new(
    "token_ids",
    DataType::List(Arc::new(Field::new("item", DataType::UInt32, false))),
    true,
),
Field::new("tokenizer_model", DataType::Utf8, true),
```

Update the schema field count test from 28 to 30.

**Placement**: After section 3.5 (Embeddings), before 3.6 (Migration) — renumber downstream sections.

### Task 3: Add tokenization to the embed pipeline — [x]

Extend `embed.rs` to populate `token_ids` and `tokenizer_model` alongside embeddings.

**Changes to `embed.rs`:**
- In `build_embedded_schema()`: fix the `token_ids` column type (Parquet source will have null lists; replace with `List<UInt32>`)
- In `replace_embedding_columns()`: add `token_ids` and `tokenizer_model` columns
- In the batch loop: call `embedder.tokenize_batch(texts)` alongside `embedder.embed_batch(texts)`

The tokenize step is essentially free compared to ONNX inference (~0.1ms vs ~1ms per text), so adding it to the existing pipeline has negligible performance impact.

### Task 4: Add `fractalaw tokenize` CLI command — [x]

Standalone tokenization command for inspection/debugging:

```
fractalaw tokenize "Health and safety at work"
```

Outputs token IDs and decoded tokens. Useful for verifying tokenization and vocabulary coverage.

### Task 5: Validation — [x]

- Schema field count test: 28 → 30
- `fractalaw embed` produces non-null `token_ids` for all rows
- Token IDs match re-tokenizing the same text
- `fractalaw validate` still passes (existing 4 checks unaffected)
- All workspace tests pass

## Design Decisions

### Column type: `List<UInt32>` not `FixedSizeList`

Token sequences are variable-length (1–256 tokens depending on text length). `List<UInt32>` is the natural Arrow type. Unlike embeddings (always 384-dim), token lists vary per row.

### UInt32 not UInt16

The current vocabulary is 30,522 (fits in UInt16). Using UInt32 for forward-compatibility with larger vocabularies (e.g., `all-mpnet-base-v2` has 30,527, and future models may use 50K+ BPE vocabularies). Storage overhead is minimal — compressed Lance/Parquet handles this efficiently.

### Include special tokens ([CLS], [SEP])

Store the full tokenized sequence including special tokens. This matches what the ONNX model expects as input, so pre-tokenized data can be fed directly to the model without re-adding specials.

### Pipeline integration vs separate pass

Tokenize during the embed pipeline (Task 3), not as a separate pass. The tokenizer is already loaded and configured with the correct truncation/padding settings. Running it alongside embedding adds ~0.1ms per text.

## Progress

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 1. `tokenize`/`tokenize_batch` | [x] | `13afdf8` | 7 new methods/tests: tokenize, tokenize_batch, id_to_token, model_name |
| 2. Schema columns | [x] | `13afdf8` | 28 → 30 fields: token_ids (List<UInt32>), tokenizer_model (Utf8) |
| 3. Embed pipeline | [x] | `13afdf8` | Tokenizes alongside embedding; fixed ListBuilder non-null inner field |
| 4. CLI `tokenize` command | [x] | `13afdf8` | `fractalaw tokenize "text"` — displays index, ID, decoded token |
| 5. Validation | [x] | `13afdf8` | 5th check: token coverage 97,522/97,522 (100%); 83 workspace tests pass |

## Session Complete

All 5 tasks done. Issue #3 shipped in commit `13afdf8`, pushed to `origin/master`.

### What Changed

| File | Changes |
|------|---------|
| `crates/fractalaw-ai/src/embedder.rs` | Added `model_name` field, `tokenize()`, `tokenize_batch()`, `id_to_token()`, `model_name()`; 7 new tests |
| `crates/fractalaw-core/src/schema.rs` | Added `token_ids: List<UInt32>` and `tokenizer_model: Utf8` to LAT schema (section 3.6); field count 28 → 30 |
| `crates/fractalaw-cli/src/embed.rs` | Added `ListBuilder`/`UInt32Builder` imports; tokenizes in batch loop; inserts token columns into output schema and batches |
| `crates/fractalaw-cli/src/main.rs` | Added `Tokenize` command + `cmd_tokenize()`; added token coverage check to validate (4 → 5 checks) |

### CLI Commands (10 total)

| Command | Store | Description |
|---------|-------|-------------|
| `stats` | DuckDB | Dataset summary |
| `law <name>` | DuckDB | Single law card + edges |
| `graph <name>` | DuckDB | Multi-hop traversal |
| `query <sql>` | DataFusion | Cross-store SQL |
| `import` | DuckDB | (Re)import Parquet into persistent DuckDB |
| `embed` | ONNX + LanceDB | Batch embedding + tokenization pipeline |
| `text <name>` | LanceDB | Legislation sections by law |
| `search "<query>"` | ONNX + LanceDB | Semantic similarity search |
| `tokenize "text"` | ONNX model | Display token IDs and decoded tokens |
| `validate` | All stores | 5-check data integrity suite |
