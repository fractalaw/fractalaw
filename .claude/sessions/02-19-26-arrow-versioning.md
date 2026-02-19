# Plan: Resolve Arrow Version Split

## Context

The workspace has two arrow versions in the dependency tree:
- **arrow 57.3.0** — used by workspace (`arrow = "57"`), fractalaw-core, DataFusion 52 (`arrow = "57.1.0"`), LanceDB 0.26 (`arrow = "57.2"`)
- **arrow 56.2.0** — used only by duckdb 1.4.4 (`arrow = "56"`)

This means `duckdb::arrow::record_batch::RecordBatch` (arrow 56) is a different Rust type from `arrow::record_batch::RecordBatch` (arrow 57). The current `duck.rs` works around this by using `duckdb::arrow` re-exports, but this will cause friction as the project grows — especially when DataFusion needs to consume RecordBatches from DuckDB queries.

## Root Cause

duckdb-rs 1.4.4 (the latest published release) pins `arrow = "56"` in its Cargo.toml. The rest of the stack (DataFusion 52, LanceDB 0.26, arrow-flight 57) all use arrow 57.

## Resolution: Pin duckdb to git main

The duckdb-rs main branch **already has the arrow 57 upgrade** — commit `60fcab76` ("Upgrade to arrow 57 (#631)") merged on 2026-02-02. It just hasn't been released to crates.io yet.

**Approach:** Replace the crates.io duckdb dependency with a git dependency pinned to a specific commit on main.

### Change

File: `/var/home/jason/fractalaw/Cargo.toml`

```toml
# Before:
duckdb = { version = "1.4", features = ["bundled"] }

# After:
duckdb = { git = "https://github.com/duckdb/duckdb-rs", rev = "a2639608", features = ["bundled"] }
```

Commit `a2639608` (2026-02-04) is the latest on main — it includes the arrow 57 upgrade plus two subsequent bug fixes.

### What This Fixes

| Crate | Before | After |
|-------|--------|-------|
| workspace arrow | 57.3.0 | 57.3.0 |
| DataFusion 52 | arrow 57 | arrow 57 |
| LanceDB 0.26 | arrow 57 | arrow 57 |
| arrow-flight | 57 | 57 |
| duckdb | arrow **56** | arrow **57** |

Result: **single arrow version across the entire stack**.

### Downstream Changes

1. **`crates/fractalaw-store/src/duck.rs`** — Can switch from `duckdb::arrow::` re-exports back to direct `arrow::` imports, since both will be the same type. This is optional (re-exports still work) but cleaner.

2. **`crates/fractalaw-store/src/error.rs`** — The `Arrow` error variant can handle errors from both duckdb and workspace arrow code, since they're now the same type.

3. **`Cargo.lock`** — Will regenerate with only arrow 57 sub-crates (no more 56.x duplicates). Smaller lock file, faster builds.

### Risk Assessment

- **Git dependencies are not publishable to crates.io** — this is fine, fractalaw is not published to crates.io (AGPL private project).
- **Pinned rev is stable** — we pin to a specific commit, not a moving branch. Reproducible builds.
- **duckdb 1.4.5 release expected soon** — when it ships with arrow 57, we switch back to a crates.io version. The API is the same.
- **Bundled DuckDB C++ build** — the git version uses the same bundled build system. No change to the `CXX`/`LIBRARY_PATH` requirements.

## Verification

```bash
# 1. Update lockfile
cargo update -p duckdb

# 2. Confirm single arrow version
grep 'name = "arrow"' Cargo.lock
# Should show only one "arrow" entry at version 57.x

# 3. Build (no features)
cargo check --workspace

# 4. Build with duckdb
CXX=$(which g++) cargo check -p fractalaw-store --features duckdb

# 5. Run tests
CXX=$(which g++) LIBRARY_PATH="/home/linuxbrew/.linuxbrew/Cellar/gcc/15.2.0_1/lib/gcc/15" \
  cargo test -p fractalaw-store --features duckdb

# 6. Run all workspace tests
cargo test --workspace

# 7. Optionally simplify duck.rs imports (duckdb::arrow:: → arrow::)
```

## Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | duckdb dep: version → git+rev |
| `crates/fractalaw-store/src/duck.rs` | (optional) Simplify imports from `duckdb::arrow::` to `arrow::` |
