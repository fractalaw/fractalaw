# Plan: Build DuckDB Ingestion in fractalaw-store (Task 3)

## Context

Task 3 from the session plan: implement the DuckDB storage layer in `fractalaw-store`. This is the foundation for the hot path (single-law lookups with denormalized relationship context) and analytical path (multi-hop graph traversal via flattened edge table). The LanceDB/semantic path is parked.

The crate skeleton exists but all source files are empty. Arrow 54 and duckdb 1.2.2 are aligned in the lockfile with no version conflicts. The build compiles cleanly.

**Prerequisite**: `data/uk_lrt.jsonl` must be re-transferred from the laptop, then `duckdb < data/export_legislation.sql` re-run to regenerate the correct Parquet files (19,318 rows / 78 cols for legislation, ~1M rows / 8 cols for law_edges). Current Parquet files are prototype placeholders with the wrong schema.

## Files to Modify

| File | Action |
|------|--------|
| `crates/fractalaw-store/src/lib.rs` | Replace doc comment with module declarations and re-exports |
| `crates/fractalaw-store/src/error.rs` | Write `StoreError` enum (currently 0 bytes) |
| `crates/fractalaw-store/src/duckdb.rs` | Write `DuckStore` struct with ingestion + query methods (currently 0 bytes) |

## Reference Files (read-only)

- `crates/fractalaw-core/src/schema.rs` — Arrow schemas (`legislation_schema()`, `law_edges_schema()`)
- `crates/fractalaw-store/Cargo.toml` — dependencies already wired up
- `data/export_legislation.sql` — Parquet production pipeline

## Implementation

### 1. `error.rs` — StoreError enum (~25 lines)

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("parquet file not found: {0}")]
    ParquetNotFound(std::path::PathBuf),

    #[error("no results for query")]
    NoResults,

    #[cfg(feature = "duckdb")]
    #[error("duckdb error: {0}")]
    DuckDb(#[from] duckdb::Error),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("{0}")]
    Other(String),
}
```

### 2. `lib.rs` — Module declarations (~15 lines)

```rust
//! Storage layer: DuckDB (analytical), LanceDB (vector), DataFusion (unified query).

mod error;
pub use error::StoreError;

#[cfg(feature = "duckdb")]
mod duckdb;
#[cfg(feature = "duckdb")]
pub use duckdb::DuckStore;
```

Note: `mod duckdb` shadows the `duckdb` crate name. Use `::duckdb` or a rename if needed. Will check at compile time — if it conflicts, rename the module to `duck` or use `extern crate`.

### 3. `duckdb.rs` — DuckStore struct (~250 lines)

#### Struct and lifecycle

```rust
pub struct DuckStore {
    conn: duckdb::Connection,
}
```

- `DuckStore::open()` — `Connection::open_in_memory()?`
- `load_legislation(path: &Path)` — validates path exists, runs `CREATE TABLE legislation AS SELECT * FROM read_parquet('{path}')`
- `load_law_edges(path: &Path)` — same pattern for `law_edges` table
- `load_all(data_dir: &Path)` — convenience: calls both with `data_dir/legislation.parquet` and `data_dir/law_edges.parquet`

#### Hot path queries

- `legislation_count() -> Result<usize>` — `SELECT count(*) FROM legislation`
- `law_edges_count() -> Result<usize>` — same for edges
- `get_legislation(name: &str) -> Result<RecordBatch>` — `SELECT * FROM legislation WHERE name = ?`, returns single-row RecordBatch with all 78 cols including List<Struct> relationship arrays. Returns `StoreError::NoResults` if empty.
- `query_legislation_sql(where_clause: &str) -> Result<Vec<RecordBatch>>` — `SELECT * FROM legislation WHERE {where_clause}`, returns all matching rows as Arrow RecordBatches

#### Analytical path queries

- `edges_for_law(name: &str) -> Result<Vec<RecordBatch>>` — `SELECT * FROM law_edges WHERE source_name = ? OR target_name = ?`
- `laws_within_hops(name: &str, max_hops: u32) -> Result<Vec<RecordBatch>>` — recursive CTE:
  ```sql
  WITH RECURSIVE reachable(law_name, hop) AS (
      SELECT ?, 0
      UNION
      SELECT CASE WHEN e.source_name = r.law_name THEN e.target_name ELSE e.source_name END,
             r.hop + 1
      FROM reachable r
      JOIN law_edges e ON e.source_name = r.law_name OR e.target_name = r.law_name
      WHERE r.hop < ?
  )
  SELECT DISTINCT law_name, min(hop) as hop FROM reachable GROUP BY law_name ORDER BY hop
  ```

#### Escape hatch

- `query_arrow(sql: &str) -> Result<Vec<RecordBatch>>` — arbitrary SQL, returns Arrow RecordBatches. For Task 5 (DataFusion) and CLI integration.
- `connection(&self) -> &duckdb::Connection` — accessor for DataFusion TableProvider registration

#### Tests (~10 tests, inline)

Tests use a helper that locates `data/` relative to `CARGO_MANIFEST_DIR` (two levels up to workspace root). If Parquet files are missing or have wrong schema (the prototype files), tests that need real data are skipped with a clear message.

| Test | What it verifies |
|------|-----------------|
| `open_in_memory` | Connection works, `SELECT 1` returns 1 |
| `load_missing_file_errors` | Non-existent path → `StoreError::ParquetNotFound` |
| `load_legislation` | Loads legislation.parquet, count > 0 |
| `load_law_edges` | Loads law_edges.parquet, count > 0 |
| `load_all` | Both tables load via convenience method |
| `hot_path_get_by_name` | Fetch a known law, verify 1 row returned |
| `hot_path_no_results` | Fetch nonexistent law → `StoreError::NoResults` |
| `hot_path_filter` | WHERE clause returns filtered results |
| `analytical_edges` | Edges for a known law returns > 0 rows |
| `analytical_hops` | 2-hop traversal returns results with hop column |
| `query_arrow_escape_hatch` | Arbitrary SQL works |

Tests that depend on real data (not the prototype Parquet) will check row count / column count and skip if data doesn't match expectations (e.g., `legislation_count < 1000`).

## Implementation Order

1. Write `error.rs`
2. Update `lib.rs` with module declarations
3. `cargo check -p fractalaw-store` — verify no-feature build
4. `cargo check -p fractalaw-store --features duckdb` — verify duckdb build
5. Write `duckdb.rs` (struct + methods + tests)
6. `cargo check -p fractalaw-store --features duckdb` — verify compilation
7. `cargo test -p fractalaw-store --features duckdb` — run tests (will skip data-dependent tests until Parquet is regenerated)

## Verification

After implementation:
```bash
# Build check (no features — error.rs only)
cargo check -p fractalaw-store

# Build check (with duckdb)
cargo check -p fractalaw-store --features duckdb

# Run tests (data-dependent tests skip if prototype Parquet present)
cargo test -p fractalaw-store --features duckdb

# Full workspace check
cargo check --workspace
```

After user re-transfers `uk_lrt.jsonl` and re-runs export:
```bash
duckdb < data/export_legislation.sql
cargo test -p fractalaw-store --features duckdb
# All tests should now pass including data-dependent ones
```
