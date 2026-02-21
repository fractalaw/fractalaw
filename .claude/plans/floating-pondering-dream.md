# Phase E: Sync Pull/Push CLI Commands

## Context

Phases A-D are complete: DRRP schemas, data host functions, AI inference host function, and the DRRP polisher guest are all working (15 tests passing). The pipeline is:

```
fractalaw sync pull  →  fractalaw run drrp-polisher.wasm  →  fractalaw sync push
```

This phase implements the first and last steps — pulling annotations from sertantai's outbox and pushing polished results back to its inbox.

**Constraint**: Sertantai endpoints don't exist yet. The commands must be buildable and testable without a live server. We'll use unit tests with mock JSON and integration tests against the DuckDB layer.

## Expected Sertantai API

```
GET  /api/outbox/annotations?since=<ISO8601>  →  JSON array of annotations
POST /api/inbox/polished                       →  JSON array of polished entries
```

## Plan

### Step 1: Add sync module to `fractalaw-sync`

Add `reqwest`, `serde`, `serde_json`, `chrono` to `fractalaw-sync/Cargo.toml` behind a new `http` feature.

Create `fractalaw-sync/src/http.rs` with:

- `SyncClient` struct holding `reqwest::Client` and `base_url: String`
- `pull_annotations(since: Option<DateTime<Utc>>) -> Result<Vec<Annotation>>` — GET from outbox, deserialize JSON
- `push_polished(entries: Vec<PolishedEntry>) -> Result<u64>` — POST to inbox, return count accepted

Serde structs `Annotation` and `PolishedEntry` mirror the DuckDB table columns. These are plain Rust structs with `#[derive(Serialize, Deserialize)]`.

Update `fractalaw-sync/src/lib.rs` to re-export the `http` module behind the feature gate.

### Step 2: Add DuckDB sync helpers to `fractalaw-store`

In `crates/fractalaw-store/src/duck.rs`, add:

- `insert_annotations(annotations: &[Annotation]) -> Result<u64>` — INSERT each annotation row, return count inserted. Uses parameterized SQL (not string interpolation) to avoid injection.
- `get_unpushed_polished() -> Result<Vec<PolishedEntry>>` — SELECT from `polished_drrp WHERE pushed = false`
- `mark_pushed(law_name: &str, provision: &str) -> Result<()>` — UPDATE `pushed = true`
- `get_last_sync_at() -> Result<Option<DateTime<Utc>>>` — `SELECT MAX(synced_at) FROM drrp_annotations`

These methods use the existing `Annotation` and `PolishedEntry` serde structs from the sync crate (or we define shared types in `fractalaw-core` to avoid a circular dep).

**Dependency direction**: `fractalaw-store` depends on `fractalaw-core` (not sync). So the shared serde structs go in `fractalaw-core/src/drrp.rs` and both `fractalaw-sync` and `fractalaw-store` use them.

### Step 3: Add shared DRRP types to `fractalaw-core`

Create `crates/fractalaw-core/src/drrp.rs` with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub law_name: String,
    pub provision: String,
    pub drrp_type: String,
    pub source_text: String,
    pub confidence: f32,
    pub scraped_at: String,  // ISO8601 string for JSON compat
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolishedEntry {
    pub law_name: String,
    pub provision: String,
    pub drrp_type: String,
    pub holder: String,
    pub text: String,
    pub qualifier: Option<String>,
    pub clause_ref: String,
    pub confidence: f32,
    pub polished_at: String,
    pub model: String,
}
```

Add `serde` as an optional dep to `fractalaw-core` behind a `serde` feature (it's a pure-Rust crate — serde is fine).

### Step 4: Add `sync` CLI command

In `crates/fractalaw-cli/src/main.rs`:

```rust
/// Sync annotations and polished results with sertantai
Sync {
    #[command(subcommand)]
    action: SyncAction,
},

#[derive(Subcommand)]
enum SyncAction {
    /// Pull new annotations from sertantai outbox
    Pull {
        /// Sertantai base URL (e.g. http://localhost:4000)
        #[arg(long, env = "SERTANTAI_URL")]
        url: String,
    },
    /// Push polished results to sertantai inbox
    Push {
        /// Sertantai base URL
        #[arg(long, env = "SERTANTAI_URL")]
        url: String,
    },
}
```

**`sync pull` flow**:
1. Open DuckDB, ensure DRRP tables exist
2. Get `last_sync_at` from DuckDB
3. Call `SyncClient::pull_annotations(since)`
4. Insert annotations into `drrp_annotations` with `synced_at = now()`
5. Print summary: "Pulled N new annotations"

**`sync push` flow**:
1. Open DuckDB
2. Get unpushed polished entries
3. If none, print "Nothing to push" and return
4. Call `SyncClient::push_polished(entries)`
5. Mark each as pushed in DuckDB
6. Print summary: "Pushed N polished entries"

### Step 5: Add CLI dep on `fractalaw-sync`

In `crates/fractalaw-cli/Cargo.toml`, add:
```toml
fractalaw-sync = { path = "../fractalaw-sync", features = ["http"] }
```

## Files to Create/Modify

| File | Change |
|------|--------|
| `crates/fractalaw-core/Cargo.toml` | Add optional `serde` dep |
| `crates/fractalaw-core/src/drrp.rs` | New — shared Annotation/PolishedEntry types |
| `crates/fractalaw-core/src/lib.rs` | Re-export `drrp` module |
| `crates/fractalaw-sync/Cargo.toml` | Add `http` feature with reqwest, serde, serde_json, chrono |
| `crates/fractalaw-sync/src/lib.rs` | Re-export `http` module |
| `crates/fractalaw-sync/src/http.rs` | New — SyncClient with pull/push methods |
| `crates/fractalaw-store/Cargo.toml` | Add fractalaw-core serde feature |
| `crates/fractalaw-store/src/duck.rs` | Add insert_annotations, get_unpushed_polished, mark_pushed, get_last_sync_at |
| `crates/fractalaw-cli/Cargo.toml` | Add fractalaw-sync dep |
| `crates/fractalaw-cli/src/main.rs` | Add Sync { Pull, Push } commands |

## Verification

1. `cargo check --workspace` — no regressions
2. `cargo test --workspace` — all existing tests pass
3. Unit tests in `fractalaw-sync` for JSON serialization/deserialization (no live server needed)
4. Unit tests in `fractalaw-store` for insert_annotations / get_unpushed_polished / mark_pushed (in-memory DuckDB)
5. `cargo run -p fractalaw-cli -- sync pull --url http://localhost:4000` — will fail with connection refused (expected, endpoints don't exist yet), but validates CLI wiring
6. `cargo run -p fractalaw-cli -- sync push --url http://localhost:4000` — same, validates CLI wiring
