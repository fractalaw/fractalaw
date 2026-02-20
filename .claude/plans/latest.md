# Fractalaw — Project Status and Next Steps

*Updated: 2026-02-20, after #9 (persistent DuckDB) completion*

## Current State

Phase 1 and Phase 2 are complete. The three-tier data architecture is fully operational:

| Tier | Store | Tables | Rows | Status |
|------|-------|--------|------|--------|
| Hot path | DuckDB (persistent) | `legislation` | 19,318 | Phase 1, persistent in #9 |
| Analytical path | DuckDB (persistent) | `law_edges` | 1,035,305 | Phase 1, persistent in #9 |
| Semantic path | LanceDB (persistent) | `legislation_text` | 97,522 | Phase 2 |

All 97,522 legislation text rows have 384-dim `all-MiniLM-L6-v2` embeddings (100% coverage). Cross-store SQL queries work via DataFusion (FusionStore bridges DuckDB + LanceDB).

### CLI (9 commands)

| Command | Path | Description |
|---------|------|-------------|
| `stats` | DuckDB | Dataset summary |
| `law <name>` | DuckDB | Single law card + edges |
| `graph <name>` | DuckDB | Multi-hop traversal |
| `query <sql>` | DataFusion | Cross-store SQL |
| `import` | DuckDB | (Re)import Parquet into persistent DuckDB |
| `embed` | ONNX + LanceDB | Batch embedding pipeline |
| `text <name>` | LanceDB | Legislation sections by law |
| `search "<query>"` | ONNX + LanceDB | Semantic similarity search |
| `validate` | All stores | 4-check data integrity suite |

### Crate Status

| Crate | Status | Notes |
|-------|--------|-------|
| `fractalaw-core` | Done | Arrow schemas, shared types |
| `fractalaw-store` | Done | DuckDB + LanceDB + DataFusion |
| `fractalaw-ai` | Done | ONNX embedder (all-MiniLM-L6-v2) |
| `fractalaw-cli` | Done | 9 commands, wires all crates |
| `fractalaw-sync` | Placeholder | Arrow Flight / Lance delta sync / Loro CRDTs |
| `fractalaw-host` | Placeholder | Wasmtime WASI Component Model runtime |

## Completed Issues

| Issue | Title | Commit |
|-------|-------|--------|
| #11 | ONNX embeddings, semantic search, LanceDB integration | `e9ed54f` |
| #9 | Eliminate CLI cold-start latency with persistent DuckDB | `021378c` |

Classification pipeline (domain/family/sub_family) was split from #11 into #13.

## Priority Recommendation

### 1st: Issue #3 — Pre-tokenized text column

Store token IDs alongside raw text in the LAT table. Eliminates re-tokenization on every embedding/inference pass. The embedding model (`all-MiniLM-L6-v2`) and tokenizer are now fixed.

### 2nd tier: Hot-path enrichment (#7, #8) — parked

- **#7 — Denormalize penalty provisions**: Extract penalty/fine data onto the hot path. Feeds future classification.
- **#8 — Denormalize commencement status**: Add in-force/commenced status to legislation rows. Enriches the hot path with temporal validity.

### 3rd tier: Issue #13 — AI classification pipeline

Classify legislation text by domain/family/sub_family using the embedding vectors in LanceDB. Split from #11. Depends on #7 (penalty provisions) and #8 (commencement status) for full feature set.

### 3rd tier: Phase 3+ (not yet)

| Issue | Title | Phase | Blocker |
|-------|-------|-------|---------|
| #12 | WASM micro-app (regulation-importer) | Phase 3 | Needs `fractalaw-host` + WIT interfaces |
| #4 | Evaluation context snapshots | Phase 3 | Needs AI classification pipeline |
| #6 | Authority precedence model | Phase 3 | Needs multi-jurisdiction data |
| #13 | AI classification pipeline | Phase 2/3 | Needs #7, #8 |
| #5 | Structured provenance graph | Phase 2/3 | Needs AI pipeline |
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
