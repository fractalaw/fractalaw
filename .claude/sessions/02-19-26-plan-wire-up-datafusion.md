# Plan: Task 5 — DataFusion Unified Query Layer

## Context

Task 5 from the session plan (`02-12-26-begin.md`): wire up DataFusion as the unified query layer over DuckDB tables. DuckDB is the storage/ingest engine (Task 3, done); DataFusion becomes the SQL query federation layer that can query `legislation` (78 cols, 19K rows) and `law_edges` (8 cols, 1M rows) through a single `SessionContext`.

This is the bridge that lets the CLI (Task 6) and validation queries (Task 7) use standard SQL without coupling to DuckDB's query API directly. When LanceDB is added later, DataFusion will federate across both backends in one SQL plan.

Arrow version is unified at 57.3.0 — DuckDB and DataFusion share the same `RecordBatch` type.

## Architecture: DuckDB-backed TableProvider

**Custom `DuckTableProvider`** that delegates to DuckDB on every `scan()` call, rather than snapshotting data into `MemTable`. This avoids doubling memory for the 19K-row legislation table and keeps DuckDB as the single source of truth.

### Key constraint: `Connection` is Send but NOT Sync

DuckDB's `Connection` wraps `RefCell<InnerConnection>` — it's `Send` but not `Sync`. `TableProvider` requires `Send + Sync`. Solution: wrap in `Mutex<Connection>`. Each `DuckTableProvider` gets its own cloned connection via `Connection::try_clone()`.

### Scan flow

```
DataFusion SQL → scan(projection, filters, limit)
  → Build SELECT with projection + limit pushdown
  → Lock Mutex<Connection>, execute DuckDB query
  → Collect Vec<RecordBatch>
  → Wrap in MemorySourceConfig + DataSourceExec
  → Return Arc<dyn ExecutionPlan>
```

**Projection pushdown**: Map column indices to names via schema, generate `SELECT col1, col2, ...` instead of `SELECT *`.

**Limit pushdown**: Append `LIMIT N` to the DuckDB query.

**Filter pushdown**: Skipped for v1 — DataFusion applies filters post-scan. Can be added later if needed.

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `crates/fractalaw-store/src/fusion.rs` | **CREATE** | DuckTableProvider, FusionStore, UDFs (~300 lines) |
| `crates/fractalaw-store/src/lib.rs` | **MODIFY** | Add `mod fusion` with feature gate |
| `crates/fractalaw-store/src/error.rs` | **MODIFY** | Add `DataFusion` error variant |

## Implementation Details

### 1. `DuckTableProvider` (private)

```rust
struct DuckTableProvider {
    table_name: String,
    schema: SchemaRef,
    conn: Mutex<Connection>,
}
```

- Constructed by querying DuckDB for the real schema: `SELECT * FROM {table} LIMIT 0`
- `schema()` returns the introspected schema (not the canonical `fractalaw_core` schema — avoids type mismatches with DuckDB's inferred Parquet types)
- `table_type()` returns `TableType::Base`
- `scan()` builds SQL with projection/limit, executes under mutex, returns `DataSourceExec` wrapping `MemorySourceConfig`

### 2. `FusionStore` (public)

```rust
pub struct FusionStore {
    ctx: SessionContext,
}
```

- `FusionStore::new(store: &DuckStore) -> Result<Self>` — clones connections, registers both tables, registers UDFs
- `query(&self, sql: &str) -> Result<Vec<RecordBatch>>` — execute SQL, collect all batches
- `context(&self) -> &SessionContext` — direct access for advanced use

### 3. UDFs

**`law_status(code)`** — Maps status codes to display labels:
- `"in_force"` → `"In Force"`, `"not_yet_in_force"` → `"Not Yet In Force"`, `"repealed"` → `"Repealed"`, else passthrough

**`edge_type_label(code)`** — Maps edge types to display labels:
- `"amends"` → `"Amends"`, `"amended_by"` → `"Amended By"`, `"enacted_by"` → `"Enacted By"`, `"rescinds"` → `"Rescinds"`, `"rescinded_by"` → `"Rescinded By"`, else passthrough

Both use `create_udf()` with `Volatility::Immutable`, `Utf8` → `Utf8`.

### 4. Feature gating

```rust
// lib.rs
#[cfg(all(feature = "duckdb", feature = "datafusion"))]
mod fusion;
#[cfg(all(feature = "duckdb", feature = "datafusion"))]
pub use fusion::FusionStore;
```

Requires both features since FusionStore wraps DuckStore. Build with `--features duckdb,datafusion` or `--features full`.

### 5. Error handling

Add to `StoreError`:
```rust
#[cfg(feature = "datafusion")]
#[error("datafusion error: {0}")]
DataFusion(#[from] datafusion::error::DataFusionError),
```

### 6. Dependencies

`futures` is needed for `futures::stream::iter` in the scan path. Add to `fractalaw-store/Cargo.toml`:
```toml
futures = "0.3"
```

No — actually check if DataFusion re-exports what we need, or if `MemorySourceConfig` handles the streaming internally. If MemorySourceConfig takes `&[Vec<RecordBatch>]` directly (like MemTable does), we don't need futures at all. The MemTable scan path uses `MemorySourceConfig::try_new(&partitions, schema, projection)` which takes owned batches — no streaming adapter needed.

**Confirmed: No `futures` dependency needed.** We collect `Vec<RecordBatch>` synchronously from DuckDB, wrap in `MemorySourceConfig`, return `DataSourceExec`. Same pattern as MemTable.

## Test Strategy

All tests use `#[tokio::test]` and the same `require_data()` helper from `duck.rs` tests. Feature-gated with `#[cfg(test)]`.

| Test | What it validates |
|------|------------------|
| `register_tables` | FusionStore creates successfully from loaded DuckStore |
| `count_legislation` | `SELECT count(*) FROM legislation` returns 19K+ |
| `count_law_edges` | `SELECT count(*) FROM law_edges` returns 1M+ |
| `projection_pushdown` | `SELECT name, year FROM legislation LIMIT 5` returns 2 columns |
| `where_filter` | `SELECT * FROM legislation WHERE year = 2024 AND status = 'in_force'` returns rows |
| `limit_pushdown` | `SELECT * FROM legislation LIMIT 10` returns exactly 10 rows |
| `cross_table_join` | `SELECT l.name, e.edge_type FROM legislation l JOIN law_edges e ON l.name = e.source_name LIMIT 10` |
| `udf_law_status` | `SELECT law_status('in_force')` returns `"In Force"` |
| `udf_edge_type_label` | `SELECT edge_type_label('amended_by')` returns `"Amended By"` |
| `udf_passthrough` | `SELECT law_status('unknown_code')` returns `"unknown_code"` |

## Verification

```bash
# 1. Check compilation (pure Rust crates unaffected)
cargo check --workspace

# 2. Check with both features
CXX=$(which g++) cargo check -p fractalaw-store --features duckdb,datafusion

# 3. Run DataFusion tests
CXX=$(which g++) LIBRARY_PATH="/home/linuxbrew/.linuxbrew/Cellar/gcc/15.2.0_1/lib/gcc/15" \
  cargo test -p fractalaw-store --features duckdb,datafusion

# 4. Run all workspace tests (ensure nothing broke)
cargo test --workspace
```
