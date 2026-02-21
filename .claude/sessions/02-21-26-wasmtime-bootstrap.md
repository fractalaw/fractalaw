# Session: 2026-02-21 — Phase 3, Session 1: Wasmtime Bootstrap

## Context

**Phase**: 3 (MicroApp Runtime)
**Goal**: Get `fractalaw-host` from placeholder to working WASM component execution. A "hello world" guest component calls `fractal:audit/log` and the host records the event.

**Previous phases complete**:
- Phase 1: DuckDB hot/analytical paths, DataFusion, CLI (7 commands)
- Phase 2: LanceDB embeddings, ONNX classification, semantic search (11 commands)

**Planning docs**:
- `.claude/plans/micro-apps.md` — 22 micro-app ideas, composition patterns, Phase 3 session sequence
- `.claude/plans/latest.md` — project status and priorities
- `docs/fractal-plan.md` §3.4 — MicroApp Runtime Architecture (detailed design)

### What Exists

| Component | Location | Status |
|-----------|----------|--------|
| WIT interfaces | `wit/world.wit` | Single `fractal:app@0.1.0` package, 8 interfaces + world |
| Host crate | `crates/fractalaw-host/` | Placeholder — single-line `lib.rs`, deps wired |
| Wasmtime dep | workspace `Cargo.toml` | v41, `component-model` + `async` features |
| WASM targets | rustup | `wasm32-wasip2` installed |
| Audit schema | `fractalaw-core/src/schema.rs` | `audit_log_schema()` — 10-field Arrow schema |

### What's Missing

| Component | Notes |
|-----------|-------|
| ~~`world.wit`~~ | ~~No root world definition~~ **Done** — single-package layout |
| `wasmtime::component::bindgen!` | No Rust bindings generated from WIT |
| Engine configuration | No pooling allocator, fuel metering, epoch interruption |
| Host function implementations | No bridge from WIT imports to native Rust |
| Guest component | No WASM micro-app exists yet |
| CLI `run` command | No way to load/execute a component |

### Wasmtime v41 Upgrade (done)

Bumped from v29 to v41 — clears three security advisories (RUSTSEC-2025-0046, RUSTSEC-2025-0118, RUSTSEC-2026-0006). Workspace builds and tests pass.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                 fractalaw-host                        │
│                                                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │  Engine (Wasmtime)                              │  │
│  │  - Pooling allocator (pre-allocated slots)      │  │
│  │  - Fuel metering (deterministic execution)      │  │
│  │  - Epoch interruption (wall-clock timeout)      │  │
│  │  - Cranelift AOT compilation                    │  │
│  └──────────────┬──────────────────────────────────┘  │
│                 │                                      │
│  ┌──────────────┴──────────────────────────────────┐  │
│  │  Host State                                     │  │
│  │  - audit_entries: Vec<AuditEntry>               │  │
│  │  (future: DuckStore, LanceStore, Embedder)      │  │
│  └──────────────┬──────────────────────────────────┘  │
│                 │                                      │
│  ┌──────────────┴──────────────────────────────────┐  │
│  │  Host Functions (WIT → Rust bridge)             │  │
│  │  - fractal:audit/log::record-event              │  │
│  │  (future: fractal:data/query, fractal:ai/*)     │  │
│  └─────────────────────────────────────────────────┘  │
│                                                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │  Guest Component (.wasm)                        │  │
│  │  - Imports: fractal:audit/log                   │  │
│  │  - Exports: run() -> result<string, string>     │  │
│  └─────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

### Data Flow (this session)

```
Guest calls record-event("app-started", "hello-world", "Bootstrap test")
  → WIT boundary (Arrow IPC not needed for audit — just strings)
  → Host receives AuditEntry { event_type, resource, detail }
  → Host appends to in-memory Vec<AuditEntry> (future: DuckDB/Lance)
  → Host prints to stdout / tracing
```

## Tasks

### Task 1: Create `world.wit`

Define the root world that composes the 4 WIT packages. For this session, only `fractal:audit` is implemented — the others are declared but stubbed.

```wit
// wit/world.wit
package fractal:app@0.1.0;

world micro-app {
    // Phase 3 Session 1: implemented
    import fractal:audit/log;

    // Phase 3 Session 2: data host functions
    // import fractal:data/query;
    // import fractal:data/mutate;

    // Phase 3 Session 3: AI host functions
    // import fractal:ai/embeddings;
    // import fractal:ai/classify;

    // Phase 3 Session 4: events + generative AI
    // import fractal:events/emit;
    // import fractal:ai/inference;

    // Guest entry point
    export run: func() -> result<string, string>;
}
```

**Output**: `wit/world.wit`

### Task 2: Generate Rust Bindings with `wasmtime::component::bindgen!`

Use Wasmtime's `bindgen!` macro in `fractalaw-host` to generate typed Rust host-side bindings from the WIT world.

```rust
// crates/fractalaw-host/src/lib.rs
wasmtime::component::bindgen!({
    world: "micro-app",
    path: "../../wit",
    async: true,
});
```

This generates:
- A `MicroApp` struct with methods to instantiate and call the guest's `run()` export
- Trait(s) for the host to implement (`fractal:audit/log` → `Host` trait with `record_event`)
- Typed Rust representations of all WIT types (`AuditEntry`, etc.)

**Output**: Updated `crates/fractalaw-host/src/lib.rs`

### Task 3: Implement Host State and `fractal:audit/log`

Create the host state struct and implement the `record-event` host function.

```rust
pub struct HostState {
    pub audit_entries: Vec<AuditEntry>,
    // future: duck_store, lance_store, embedder, etc.
}

struct AuditEntry {
    event_type: String,
    resource: String,
    detail: String,
    timestamp: chrono::DateTime<chrono::Utc>,
}
```

Implement the generated `Host` trait for `HostState`:

```rust
impl fractal::audit::log::Host for HostState {
    async fn record_event(&mut self, entry: fractal::audit::log::AuditEntry) {
        let audit = AuditEntry {
            event_type: entry.event_type,
            resource: entry.resource,
            detail: entry.detail,
            timestamp: chrono::Utc::now(),
        };
        tracing::info!(
            event_type = %audit.event_type,
            resource = %audit.resource,
            "Audit event recorded"
        );
        self.audit_entries.push(audit);
    }
}
```

**Output**: Host state + trait implementation in `crates/fractalaw-host/src/lib.rs`

### Task 4: Configure Wasmtime Engine

Set up the Engine with pooling allocator, fuel metering, and epoch interruption per the architecture doc (§3.4.5).

```rust
pub fn create_engine() -> anyhow::Result<Engine> {
    let mut pool = PoolingAllocationConfig::default();
    pool.total_component_instances(16);
    pool.total_memories(32);
    pool.max_memory_size(64 * 1024 * 1024); // 64MB per instance

    let mut config = Config::new();
    config.async_support(true);
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    Engine::new(&config)
}
```

Also provide a `load_component` function that compiles a `.wasm` file into a `Component`:

```rust
pub async fn load_component(engine: &Engine, path: &Path) -> anyhow::Result<Component> {
    let bytes = tokio::fs::read(path).await?;
    Component::new(engine, &bytes)
}
```

**Output**: Engine setup in `crates/fractalaw-host/src/lib.rs` (or `engine.rs`)

### Task 5: Build a Guest Component

Create a minimal WASM guest component that imports `fractal:audit/log` and exports `run`.

Options:
- **Option A**: Use `cargo-component` to build a Rust guest targeting `wasm32-wasip2`
- **Option B**: Hand-write a guest with `wit-bindgen` in a standalone crate

Option A is preferred — it's the standard Bytecode Alliance workflow.

Create `guests/hello-world/` as a standalone Cargo project (not in the workspace — it targets wasm32-wasip2, not the host architecture):

```
guests/hello-world/
├── Cargo.toml
├── src/lib.rs
└── wit/       (symlink or copy of /wit/)
```

Guest code:

```rust
wit_bindgen::generate!({
    world: "micro-app",
    path: "../wit",
});

struct HelloWorld;

impl Guest for HelloWorld {
    fn run() -> Result<String, String> {
        // Call the host's audit log
        fractal::audit::log::record_event(&AuditEntry {
            event_type: "app-started".to_string(),
            resource: "hello-world".to_string(),
            detail: "Bootstrap test — first micro-app execution".to_string(),
        });

        Ok("Hello from the first Fractalaw micro-app!".to_string())
    }
}

export!(HelloWorld);
```

Build: `cargo component build --release` → `target/wasm32-wasip2/release/hello_world.wasm`

**Output**: `guests/hello-world/` with buildable guest component

### Task 6: Wire Up End-to-End Execution

Create a public `run_component` function in `fractalaw-host` that:

1. Creates the Engine (Task 4)
2. Loads the Component (Task 4)
3. Creates a Store with HostState and fuel budget
4. Instantiates the component with host function bindings
5. Calls the guest's `run()` export
6. Returns the result + collected audit entries

```rust
pub async fn run_component(wasm_path: &Path) -> anyhow::Result<RunResult> {
    let engine = create_engine()?;
    let component = load_component(&engine, wasm_path).await?;

    let mut store = Store::new(&engine, HostState::new());
    store.set_fuel(1_000_000_000)?; // 1B fuel units (standard tier)

    let linker = create_linker(&engine)?;
    let instance = MicroApp::instantiate_async(&mut store, &component, &linker).await?;

    let result = instance.call_run(&mut store).await?;

    let state = store.into_data();
    Ok(RunResult {
        output: result,
        audit_entries: state.audit_entries,
    })
}
```

**Output**: `run_component()` in `crates/fractalaw-host/src/lib.rs`

### Task 7: Add CLI `run` Command

Add `fractalaw run <component.wasm>` to the CLI:

```rust
/// Load and execute a WASM micro-app component
Run {
    /// Path to the .wasm component file
    component: PathBuf,

    /// Fuel budget (default: 1 billion = standard tier)
    #[arg(long, default_value_t = 1_000_000_000)]
    fuel: u64,
},
```

Handler:

```rust
Command::Run { component, fuel } => {
    let result = fractalaw_host::run_component(&component).await?;
    match result.output {
        Ok(msg) => println!("{msg}"),
        Err(err) => eprintln!("Guest error: {err}"),
    }
    if !result.audit_entries.is_empty() {
        println!("\n--- Audit Trail ({} entries) ---", result.audit_entries.len());
        for entry in &result.audit_entries {
            println!("  [{}] {} — {}", entry.event_type, entry.resource, entry.detail);
        }
    }
}
```

**Output**: `run` command in `crates/fractalaw-cli/src/main.rs`

### Task 8: Test End-to-End

1. Build the guest: `cd guests/hello-world && cargo component build --release`
2. Run it: `cargo run -p fractalaw-cli -- run guests/hello-world/target/wasm32-wasip2/release/hello_world.wasm`
3. Expected output:
   ```
   Hello from the first Fractalaw micro-app!

   --- Audit Trail (1 entries) ---
     [app-started] hello-world — Bootstrap test — first micro-app execution
   ```
4. Add unit tests in `fractalaw-host` that load the guest component and verify:
   - `run()` returns `Ok("Hello from the first Fractalaw micro-app!")`
   - Exactly 1 audit entry was recorded
   - Fuel was consumed (< initial budget)

**Output**: Working end-to-end execution + tests

## Dependencies

| Dependency | Status | Impact |
|------------|--------|--------|
| wasmtime 41 | In workspace | Bumped from 29 — clears all 3 security advisories |
| wasm32-wasip2 target | Installed | Required for guest compilation |
| cargo-component | Check if installed | Required for guest build workflow |
| wit-bindgen | Needed for guest | Guest-side WIT code generation |

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `wit/world.wit` | Create | Root world definition — imports audit/log, exports run |
| `crates/fractalaw-host/src/lib.rs` | Rewrite | Engine, bindgen, host state, host functions, run_component |
| `crates/fractalaw-host/Cargo.toml` | Modify | May need additional deps (wasmtime-wasi, chrono) |
| `crates/fractalaw-cli/src/main.rs` | Modify | Add `Run` command variant |
| `guests/hello-world/Cargo.toml` | Create | Guest component project |
| `guests/hello-world/src/lib.rs` | Create | Guest implementation |
| `.gitignore` | Modify | Add `guests/*/target` |

## Success Criteria

- [x] `world.wit` defines the `micro-app` world with `audit-log` import and `run` export
- [x] `wasmtime::component::bindgen!` generates typed Rust bindings in `fractalaw-host`
- [x] Engine configured with pooling allocator, fuel metering, epoch interruption
- [x] `audit-log::record-event` host function works — guest calls it, host records it
- [x] Guest component builds with `cargo component build` targeting wasm32-wasip1
- [x] `fractalaw run <path.wasm>` loads, instantiates, and executes the guest
- [x] Audit entries are captured and displayed
- [x] Unit tests verify the full round-trip (3 tests)
- [x] `cargo check --workspace` and `cargo test --workspace` pass (112 tests, 0 failures)

## Progress

| Task | Status | Notes |
|------|--------|-------|
| 1. Create `world.wit` | [x] | Single-package `fractal:app@0.1.0` — wasm-tools 1.245 multi-package deps broken, consolidated all interfaces into one file. `%resource` escape for WIT keyword. |
| 2. Generate Rust bindings | [x] | wasmtime 41 uses per-function async config: `imports: { default: async }`. `Host` trait, `AuditEntry` record, `MicroApp::call_run()` all generated. |
| 3. Host state + audit impl | [x] | `HostState` with `Vec<AuditRecord>`, `Host` trait impl, `create_linker()` using `HasSelf<HostState>`. Added `chrono` to workspace. |
| 4. Engine configuration | [x] | `create_engine()` with pooling (16 instances, 32 memories, 64 MiB), fuel, epoch interruption. `load_component()` async. |
| 5. Guest component | [x] | `guests/hello-world/` — standalone crate, symlinks `wit/`, `cargo component build --release` → 64K wasm. Note: WASI imports added by wasip1 adapter, host needs `wasmtime-wasi`. |
| 6. End-to-end execution | [x] | `run_component()` wires engine→linker→store→instantiate→call. Added `wasmtime-wasi` for WASI p2 imports. `HostState` now holds `WasiCtx` + `ResourceTable`. `RunResult` returns output + audit entries + fuel consumed. |
| 7. CLI `run` command | [x] | `fractalaw run <component.wasm> [--fuel N]`, default 1B fuel. Prints output, audit trail, fuel consumed. |
| 8. Test end-to-end | [x] | Manual CLI test passes. 3 unit tests: `run_returns_ok`, `audit_entry_recorded`, `fuel_consumed`. Fixed epoch interruption — needed `set_epoch_deadline` + background ticker. |
