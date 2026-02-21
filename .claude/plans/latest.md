# Fractalaw — Project Status and Next Steps

*Updated: 2026-02-21, after #13 (AI classification pipeline) completion*

## Current State

Phase 1 and Phase 2 are complete. The three-tier data architecture is fully operational:

| Tier | Store | Tables | Rows | Status |
|------|-------|--------|------|--------|
| Hot path | DuckDB (persistent) | `legislation` | 19,318 | Phase 1, persistent in #9 |
| Analytical path | DuckDB (persistent) | `law_edges` | 1,035,305 | Phase 1, persistent in #9 |
| Semantic path | LanceDB (persistent) | `legislation_text` | 97,522 | Phase 2 |

All 97,522 legislation text rows have 384-dim `all-MiniLM-L6-v2` embeddings and pre-tokenized token IDs (100% coverage). Cross-store SQL queries work via DataFusion (FusionStore bridges DuckDB + LanceDB).

The AI classification pipeline (#13) is complete. 452 laws with embeddings have been classified by domain, family, and subjects using centroid-based cosine similarity. Results are stored in `classified_*` columns on the legislation table with a `classification_status` field (`predicted`/`confirmed`/`conflict`) for queryable diffs against ground truth. Agreement rate: 74.6% (302 confirmed, 103 conflicts, 47 predicted-only).

### CLI (11 commands)

| Command | Path | Description |
|---------|------|-------------|
| `stats` | DuckDB | Dataset summary |
| `law <name>` | DuckDB | Single law card + edges |
| `graph <name>` | DuckDB | Multi-hop traversal |
| `query <sql>` | DataFusion | Cross-store SQL |
| `import` | DuckDB | (Re)import Parquet into persistent DuckDB |
| `embed` | ONNX + LanceDB | Batch embedding + tokenization pipeline |
| `classify` | ONNX + LanceDB + DuckDB | Centroid-based classification pipeline |
| `text <name>` | LanceDB | Legislation sections by law |
| `search "<query>"` | ONNX + LanceDB | Semantic similarity search |
| `tokenize "text"` | ONNX model | Display token IDs and decoded tokens |
| `validate` | All stores | 9-check data integrity suite (incl. 4 classification checks) |

### Crate Status

| Crate | Status | Notes |
|-------|--------|-------|
| `fractalaw-core` | Done | Arrow schemas, shared types |
| `fractalaw-store` | Done | DuckDB + LanceDB + DataFusion |
| `fractalaw-ai` | Done | ONNX embedder + classifier (labels, centroids, classification) |
| `fractalaw-cli` | Done | 11 commands, wires all crates |
| `fractalaw-sync` | Placeholder | Arrow Flight / Lance delta sync / Loro CRDTs |
| `fractalaw-host` | Placeholder | Wasmtime WASI Component Model runtime |

## Completed Issues

| Issue | Title | Commit |
|-------|-------|--------|
| #13 | Centroid-based AI classification pipeline | `5c451b9` |
| #11 | ONNX embeddings, semantic search, LanceDB integration | `e9ed54f` |
| #9 | Eliminate CLI cold-start latency with persistent DuckDB | `021378c` |
| #3 | Pre-tokenized text column in LAT | `13afdf8` |

## Priority Recommendation

### 1st tier: Phase 3 — Wasmtime host runtime + first micro-apps

Phase 2 (AI integration) is complete. The approach is validated. Phase 3 is the MicroApp Runtime — building the Wasmtime host that lets WASM components use the data and AI infrastructure we've built.

See `.claude/plans/micro-apps.md` for the full brainstorm of 22 micro-app ideas across hub-side (AI refinement, batch processing), edge-side (field tools, offline search), and bridge (sync/transform) categories.

**Suggested session sequence:**

1. **Wasmtime bootstrap** — Engine, pooling allocator, fuel metering, basic component loading. First host function: `fractal:audit/log`.
2. **Data host functions** — `fractal:data/query` + `fractal:data/mutate` bridging to DataFusion/DuckDB/LanceDB. First real micro-app: Elixir-to-Fractalaw Bridge.
3. **AI host functions** — `fractal:ai/embeddings` + `fractal:ai/classify` bridging to `fractalaw-ai`. Field Research Tool + Incident Classifier.
4. **Generative AI + events** — `fractal:ai/inference` + `fractal:events/emit`. DRRP Polisher + Regulatory Change Monitor. Event-driven composition.
5. **App Supervisor** — Registry, Lifecycle Manager, Router. Hot-swap. Fleet management.

### 2nd tier: Parked (revisit after Phase 3)

| Issue | Title | Notes |
|-------|-------|-------|
| #14 | Classification improvements | Approach validated at 74.6% agreement. Polish blocked on LAT data coverage — revisit when more text is ingested via the bridge. |
| #7 | Denormalize penalty provisions | Would add structured features for future classifier improvements. |
| #8 | Denormalize commencement status | Enriches hot path with temporal validity. |
| #12 | regulation-importer micro-app | Reframed as sync bridge from sister Elixir app. Blocked on host runtime + Elixir pipeline completion. |

### 3rd tier: Phase 3+ (not yet)

| Issue | Title | Phase | Blocker |
|-------|-------|-------|---------|
| #4 | Evaluation context snapshots | Phase 3 | Needs micro-app runtime producing evaluation contexts |
| #6 | Authority precedence model | Phase 3 | Needs multi-jurisdiction data |
| #5 | Structured provenance graph | Phase 2/3 | Needs classification + provenance data |
| #1 | Bitmask feature flags | Phase 2+ | Needs real query patterns to emerge |
| #10 | Multi-jurisdiction expansion | Phase 3+ | Needs non-UK data sources (EUR-Lex) |
| #2 | Flat-pack compilation | Phase 4 | Needs sync infrastructure |

## Known Gaps

- **47 LAT-only laws**: legislation_text contains 452 distinct law_names, but only 405 match the legislation table. 47 are SI/NI instruments not in the core legislation.parquet export. Not a bug — LAT covers a broader set.
- **Amendment annotations not in LanceDB**: Only `legislation_text` was loaded into LanceDB. The 19,451 amendment_annotations rows remain in Parquet only. These don't need embeddings (they're structured metadata, not free text) but could be loaded into LanceDB for unified access if needed.

## Session Log

| Date | Session | Summary |
|------|---------|---------|
| 2026-02-12 | `02-12-26-begin.md` | Phase 1: DuckDB hot/analytical paths, DataFusion, CLI (7 tasks) |
| 2026-02-19 | `02-19-26-LAT-schema.md` | LAT schema revision: citation-based identity, sort key normalisation |
| 2026-02-20 | `02-20-26-phase2-lancedb-embeddings.md` | Phase 2: LanceDB, ONNX embeddings, semantic search, CLI commands (6 tasks) |
| 2026-02-20 | `02-20-26-persistent-duckdb.md` | Issue #9: Persistent DuckDB, 8x speedup, `import` command (4 tasks) |
| 2026-02-20 | `02-20-26-pre-tokenized-text.md` | Issue #3: Pre-tokenized text columns, `tokenize` command (5 tasks) |
| 2026-02-20 | `02-20-26-ai-classification.md` | Issue #13: Centroid-based AI classification pipeline (6 tasks) |
