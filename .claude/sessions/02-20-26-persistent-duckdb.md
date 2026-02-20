# Session: 2026-02-20 — Issue #9: Eliminate CLI cold-start latency

## Context

**GitHub Issue**: [#9 — Eliminate CLI cold-start latency with persistent DuckDB](https://github.com/fractalaw/fractalaw/issues/9)

Phase 2 is complete. Every CLI command currently cold-loads two Parquet files into an in-memory DuckDB instance. This dominates wall time by 100x+.

### Measured Baseline

```
fractalaw query "SELECT 1 AS x"   →  4.4s wall time
fractalaw law UK_ukpga_1974_37    →  4.6s wall time
fractalaw stats                   →  13.0s wall time (includes analytical queries)
```

From tracing logs:
- `legislation.parquet` (19,318 rows, 78 cols): ~2.0s
- `law_edges.parquet` (1,035,305 rows, 8 cols): ~2.4s
- Actual query execution: <10ms

**Target**: <200ms total wall time for hot-path commands.

### What Exists

| Component | File | Status |
|-----------|------|--------|
| `DuckStore::open()` | `crates/fractalaw-store/src/duck.rs:24` | In-memory only |
| `DuckStore::load_all()` | `duck.rs:61` | Reads Parquet every invocation |
| CLI startup | `crates/fractalaw-cli/src/main.rs:101–102` | Always calls `open()` + `load_all()` |
| `FusionStore::new(&DuckStore)` | `crates/fractalaw-store/src/fusion.rs:154` | Takes `&DuckStore` — API won't change |
| LanceDB | `crates/fractalaw-store/src/lance.rs` | Already persistent (`data/lancedb/`) |

### DuckDB Connection API

```rust
// Current: in-memory
Connection::open_in_memory()?;

// Persistent: opens or creates a .duckdb file
Connection::open("data/fractalaw.duckdb")?;
```

The `duckdb-rs` crate (pinned at `a2639608`, bundled) supports `Connection::open(path)` for persistent databases. Same `Connection` type — all downstream code (`DuckTableProvider`, `FusionStore`, queries) works unchanged.

## Tasks

### Task 1: Add `DuckStore::open_persistent()` — [ ]

Add a second constructor that opens a persistent DuckDB database file.

**API change in `duck.rs`:**

```rust
impl DuckStore {
    /// Open an in-memory DuckDB database (existing behaviour).
    pub fn open() -> Result<Self, StoreError> { ... }

    /// Open or create a persistent DuckDB database at the given path.
    pub fn open_persistent(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    /// Check whether the expected tables already exist.
    pub fn has_tables(&self) -> bool {
        self.legislation_count().is_ok() && self.law_edges_count().is_ok()
    }
}
```

The `has_tables()` check is cheap — `SELECT count(*)` on a persistent table is metadata-only. If tables exist, skip `load_all()`. If they don't (first run or after schema change), import from Parquet.

**Files:**
- Edit: `crates/fractalaw-store/src/duck.rs`

**Tests:**
- `open_persistent` creates a `.duckdb` file
- `load_all` populates the persistent DB
- Reopen the same file → `has_tables()` returns true
- Counts match after reopen (no data loss)
- `open` (in-memory) still works as before

### Task 2: Add `fractalaw import` CLI command — [ ]

Explicit command to (re)load Parquet into the persistent DuckDB. This replaces the implicit `load_all()` in the startup path.

```rust
/// Import Parquet files into persistent DuckDB (first run or refresh)
Import,
```

**Flow:**
1. Open persistent DuckDB at `{data_dir}/fractalaw.duckdb`
2. Call `store.load_all(&data_dir)` (always — this is the explicit refresh command)
3. Print row counts

### Task 3: Update CLI startup to use persistent DuckDB — [ ]

Change `main()` from:

```rust
let store = DuckStore::open()?;
store.load_all(&data_dir)?;
```

To:

```rust
let db_path = data_dir.join("fractalaw.duckdb");
let store = DuckStore::open_persistent(&db_path)?;
if !store.has_tables() {
    eprintln!("First run — importing Parquet data...");
    store.load_all(&data_dir)?;
}
```

**Commands that need DuckDB:**
- `stats`, `law`, `graph` — direct DuckStore queries
- `query` — FusionStore (wraps DuckStore)
- `validate` — FusionStore + cross-store join

**Commands that don't need DuckDB:**
- `text`, `search` — LanceDB only
- `embed` — LanceDB + ONNX only

Consider: lazy-load DuckDB only when the command needs it. The `text` and `search` commands currently pay the 4s Parquet tax for no reason. This would be a nice secondary improvement but adds complexity — defer to a follow-up or handle in the same session if time permits.

**Files:**
- Edit: `crates/fractalaw-cli/src/main.rs`

### Task 4: Tests and validation — [ ]

- Build: `cargo check --workspace`
- Existing tests: `cargo test --workspace` (all 73+ tests pass)
- Manual test sequence:
  ```
  # First run (no .duckdb file exists)
  rm -f data/fractalaw.duckdb
  time fractalaw stats          # Should auto-import, ~4s first time
  time fractalaw stats          # Should be fast, <1s

  # Explicit re-import
  time fractalaw import         # Re-reads Parquet
  time fractalaw law UK_ukpga_1974_37   # Fast

  # LanceDB commands (should never touch DuckDB)
  time fractalaw text UK_ukpga_1974_37
  time fractalaw search "chemical exposure"
  ```

## Design Decisions

### Database file location

`{data_dir}/fractalaw.duckdb` — beside the Parquet files, same pattern as `{data_dir}/lancedb/`. The data dir is the single source of truth for all persistent state.

### Auto-import on first run vs explicit `import` command

Both. First run auto-imports (good UX — `fractalaw stats` just works). Explicit `import` command exists for:
- Schema changes (re-import after updating Parquet exports)
- Data refreshes (new Parquet files from the sister project)
- Testing/debugging

### `has_tables()` implementation

Could use DuckDB's `information_schema.tables` or just try `SELECT count(*)` and catch the error. The count approach is simpler and already implemented — `count_table()` returns `Err` if the table doesn't exist.

### Lazy DuckDB loading (stretch goal)

Commands like `text` and `search` only use LanceDB. Currently they still open DuckDB and load Parquet because `main()` does this unconditionally. Refactoring to lazy-load would save startup time for semantic-only commands. This is a nice-to-have — the persistent DuckDB fix eliminates the worst case (4s → <200ms) regardless.

## Progress

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 1. `open_persistent()` | [ ] | | |
| 2. `import` command | [ ] | | |
| 3. CLI startup change | [ ] | | |
| 4. Tests & validation | [ ] | | |
