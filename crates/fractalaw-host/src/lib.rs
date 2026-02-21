//! Wasmtime host runtime: micro-app lifecycle, instance pooling, WIT interface bridge.

use std::path::Path;
use wasmtime::component::{Component, HasSelf, ResourceTable};
use wasmtime::{Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    world: "micro-app",
    path: "../../wit",
    imports: { default: async },
    exports: { default: async },
});

/// Host-side audit entry with timestamp added by the host.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub event_type: String,
    pub resource: String,
    pub detail: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Result of running a micro-app component.
pub struct RunResult {
    pub output: Result<String, String>,
    pub audit_entries: Vec<AuditRecord>,
    pub fuel_consumed: u64,
}

/// State held in the Wasmtime [`Store`](wasmtime::Store) for each guest execution.
pub struct HostState {
    pub audit_entries: Vec<AuditRecord>,
    pub wasi_ctx: WasiCtx,
    pub table: ResourceTable,
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}

impl HostState {
    pub fn new() -> Self {
        let wasi_ctx = WasiCtxBuilder::new()
            .inherit_stdout()
            .inherit_stderr()
            .build();
        Self {
            audit_entries: Vec::new(),
            wasi_ctx,
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.table,
        }
    }
}

impl fractal::app::audit_log::Host for HostState {
    async fn record_event(&mut self, entry: fractal::app::audit_log::AuditEntry) {
        let record = AuditRecord {
            event_type: entry.event_type,
            resource: entry.resource,
            detail: entry.detail,
            timestamp: chrono::Utc::now(),
        };
        tracing::info!(
            event_type = %record.event_type,
            resource = %record.resource,
            "audit event recorded"
        );
        self.audit_entries.push(record);
    }
}

/// Create an [`Engine`] configured for micro-app execution.
///
/// - Pooling allocator with pre-allocated instance slots
/// - Fuel metering for deterministic execution budgets
/// - Epoch interruption for wall-clock timeouts
/// - Component model + async support
pub fn create_engine() -> anyhow::Result<Engine> {
    let mut pool = PoolingAllocationConfig::new();
    pool.total_component_instances(16);
    pool.total_memories(32);
    pool.max_memory_size(64 * 1024 * 1024); // 64 MiB per instance

    let mut config = Config::new();
    config.async_support(true);
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    Engine::new(&config)
}

/// Load and compile a WASM component from disk.
pub async fn load_component(engine: &Engine, path: &Path) -> anyhow::Result<Component> {
    let bytes = tokio::fs::read(path).await?;
    Component::new(engine, &bytes)
}

/// Create a [`wasmtime::component::Linker`] with host functions wired up.
pub fn create_linker(engine: &Engine) -> anyhow::Result<wasmtime::component::Linker<HostState>> {
    let mut linker = wasmtime::component::Linker::new(engine);
    // Wire up our fractal:app host functions
    MicroApp::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
    // Wire up WASI p2 interfaces (cli, io, filesystem, clocks) required by the wasip1 adapter
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    Ok(linker)
}

/// Load, instantiate, and execute a micro-app component.
pub async fn run_component(wasm_path: &Path, fuel: u64) -> anyhow::Result<RunResult> {
    let engine = create_engine()?;
    let component = load_component(&engine, wasm_path).await?;
    let linker = create_linker(&engine)?;

    let mut store = Store::new(&engine, HostState::new());
    store.set_fuel(fuel)?;
    // Allow 100 epoch ticks before interruption (= 100 seconds with 1s ticker).
    store.set_epoch_deadline(100);

    // Spawn a background task to increment the epoch every second.
    let epoch_engine = engine.clone();
    let epoch_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            epoch_engine.increment_epoch();
        }
    });

    let instance = MicroApp::instantiate_async(&mut store, &component, &linker).await?;
    let output = instance.call_run(&mut store).await?;

    epoch_handle.abort();

    let fuel_consumed = fuel.saturating_sub(store.get_fuel()?);
    let state = store.into_data();

    Ok(RunResult {
        output,
        audit_entries: state.audit_entries,
        fuel_consumed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hello_world_wasm() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../guests/hello-world/target/wasm32-wasip1/release/hello_world.wasm")
    }

    #[tokio::test]
    async fn run_returns_ok() {
        let result = run_component(&hello_world_wasm(), 1_000_000_000)
            .await
            .expect("run_component failed");

        assert_eq!(
            result.output,
            Ok("Hello from the first Fractalaw micro-app!".to_string())
        );
    }

    #[tokio::test]
    async fn audit_entry_recorded() {
        let result = run_component(&hello_world_wasm(), 1_000_000_000)
            .await
            .expect("run_component failed");

        assert_eq!(result.audit_entries.len(), 1);
        let entry = &result.audit_entries[0];
        assert_eq!(entry.event_type, "app-started");
        assert_eq!(entry.resource, "hello-world");
        assert_eq!(entry.detail, "Bootstrap test — first micro-app execution");
    }

    #[tokio::test]
    async fn fuel_consumed() {
        let budget = 1_000_000_000u64;
        let result = run_component(&hello_world_wasm(), budget)
            .await
            .expect("run_component failed");

        assert!(result.fuel_consumed > 0, "should have consumed some fuel");
        assert!(
            result.fuel_consumed < budget,
            "should not have exhausted the full budget"
        );
    }
}
