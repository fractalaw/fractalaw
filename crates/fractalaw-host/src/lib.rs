//! Wasmtime host runtime: micro-app lifecycle, instance pooling, WIT interface bridge.

use std::path::Path;
use wasmtime::component::{Component, HasSelf, ResourceTable};
use wasmtime::{Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

#[cfg(feature = "duckdb")]
use fractalaw_store::DuckStore;

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

/// Configuration for the Claude API inference backend.
#[cfg(feature = "inference")]
pub struct InferenceConfig {
    pub api_key: String,
    pub model: String,
    pub client: reqwest::Client,
}

#[cfg(feature = "inference")]
impl InferenceConfig {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }
}

/// State held in the Wasmtime [`Store`](wasmtime::Store) for each guest execution.
pub struct HostState {
    pub audit_entries: Vec<AuditRecord>,
    pub wasi_ctx: WasiCtx,
    pub table: ResourceTable,
    #[cfg(feature = "duckdb")]
    pub duck: Option<DuckStore>,
    #[cfg(feature = "inference")]
    pub inference: Option<InferenceConfig>,
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
            #[cfg(feature = "duckdb")]
            duck: None,
            #[cfg(feature = "inference")]
            inference: None,
        }
    }

    /// Attach a DuckDB store for data-query and data-mutate host functions.
    #[cfg(feature = "duckdb")]
    pub fn with_duck(mut self, store: DuckStore) -> Self {
        self.duck = Some(store);
        self
    }

    /// Attach an inference backend for ai-inference host functions.
    #[cfg(feature = "inference")]
    pub fn with_inference(mut self, config: InferenceConfig) -> Self {
        self.inference = Some(config);
        self
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

// ── Audit log host function ──

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

// ── Data query host function ──

impl fractal::app::data_query::Host for HostState {
    async fn query(
        &mut self,
        sql: String,
    ) -> Result<Vec<u8>, fractal::app::data_query::QueryError> {
        self.query_impl(&sql)
    }
}

impl HostState {
    fn query_impl(&self, sql: &str) -> Result<Vec<u8>, fractal::app::data_query::QueryError> {
        #[cfg(feature = "duckdb")]
        {
            let duck = self
                .duck
                .as_ref()
                .ok_or(fractal::app::data_query::QueryError {
                    code: 1,
                    message: "no DuckDB store attached".into(),
                })?;
            let batches =
                duck.query_arrow(sql)
                    .map_err(|e| fractal::app::data_query::QueryError {
                        code: 2,
                        message: e.to_string(),
                    })?;
            encode_ipc(&batches).map_err(|e| fractal::app::data_query::QueryError {
                code: 3,
                message: e.to_string(),
            })
        }

        #[cfg(not(feature = "duckdb"))]
        {
            let _ = sql;
            Err(fractal::app::data_query::QueryError {
                code: 1,
                message: "DuckDB support not compiled in".into(),
            })
        }
    }
}

// ── Data mutate host function ──

impl fractal::app::data_mutate::Host for HostState {
    async fn insert(
        &mut self,
        table: String,
        data: Vec<u8>,
    ) -> Result<u64, fractal::app::data_mutate::MutateError> {
        self.insert_impl(&table, &data)
    }

    async fn execute(
        &mut self,
        sql: String,
    ) -> Result<u64, fractal::app::data_mutate::MutateError> {
        self.execute_impl(&sql)
    }
}

impl HostState {
    fn insert_impl(
        &self,
        table: &str,
        data: &[u8],
    ) -> Result<u64, fractal::app::data_mutate::MutateError> {
        #[cfg(feature = "duckdb")]
        {
            let duck = self
                .duck
                .as_ref()
                .ok_or(fractal::app::data_mutate::MutateError {
                    code: 1,
                    message: "no DuckDB store attached".into(),
                })?;
            let batches = decode_ipc(data).map_err(|e| fractal::app::data_mutate::MutateError {
                code: 2,
                message: format!("failed to decode Arrow IPC: {e}"),
            })?;
            let mut total_rows = 0u64;
            for batch in &batches {
                duck.insert_batch(table, batch).map_err(|e| {
                    fractal::app::data_mutate::MutateError {
                        code: 3,
                        message: e.to_string(),
                    }
                })?;
                total_rows += batch.num_rows() as u64;
            }
            Ok(total_rows)
        }

        #[cfg(not(feature = "duckdb"))]
        {
            let _ = (table, data);
            Err(fractal::app::data_mutate::MutateError {
                code: 1,
                message: "DuckDB support not compiled in".into(),
            })
        }
    }

    fn execute_impl(&self, sql: &str) -> Result<u64, fractal::app::data_mutate::MutateError> {
        #[cfg(feature = "duckdb")]
        {
            let duck = self
                .duck
                .as_ref()
                .ok_or(fractal::app::data_mutate::MutateError {
                    code: 1,
                    message: "no DuckDB store attached".into(),
                })?;
            duck.execute(sql)
                .map_err(|e| fractal::app::data_mutate::MutateError {
                    code: 2,
                    message: e.to_string(),
                })?;
            Ok(0)
        }

        #[cfg(not(feature = "duckdb"))]
        {
            let _ = sql;
            Err(fractal::app::data_mutate::MutateError {
                code: 1,
                message: "DuckDB support not compiled in".into(),
            })
        }
    }
}

// ── AI embeddings host function (stub) ──

impl fractal::app::ai_embeddings::Host for HostState {
    async fn embed(
        &mut self,
        _text: String,
    ) -> Result<Vec<f32>, fractal::app::ai_embeddings::AiError> {
        Err(fractal::app::ai_embeddings::AiError {
            code: 1,
            message: "embeddings not configured — use fractalaw embed CLI instead".into(),
        })
    }

    async fn embed_batch(
        &mut self,
        _texts: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, fractal::app::ai_embeddings::AiError> {
        Err(fractal::app::ai_embeddings::AiError {
            code: 1,
            message: "embeddings not configured — use fractalaw embed CLI instead".into(),
        })
    }
}

// ── AI inference host function ──

impl fractal::app::ai_inference::Host for HostState {
    async fn generate(
        &mut self,
        request: fractal::app::ai_inference::GenerateRequest,
    ) -> Result<fractal::app::ai_inference::GenerateResponse, fractal::app::ai_embeddings::AiError>
    {
        self.generate_impl(request).await
    }
}

impl HostState {
    async fn generate_impl(
        &mut self,
        request: fractal::app::ai_inference::GenerateRequest,
    ) -> Result<fractal::app::ai_inference::GenerateResponse, fractal::app::ai_embeddings::AiError>
    {
        #[cfg(feature = "inference")]
        {
            let config = self
                .inference
                .as_ref()
                .ok_or(fractal::app::ai_embeddings::AiError {
                    code: 1,
                    message: "no inference backend configured (set ANTHROPIC_API_KEY)".into(),
                })?;

            // Build Claude Messages API request body.
            let mut body = serde_json::json!({
                "model": config.model,
                "max_tokens": request.max_tokens,
                "messages": [
                    { "role": "user", "content": request.user_prompt }
                ],
            });

            if let Some(system) = &request.system_prompt {
                body["system"] = serde_json::json!(system);
            }
            if request.temperature > 0.0 {
                body["temperature"] = serde_json::json!(request.temperature);
            }

            let resp = config
                .client
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| fractal::app::ai_embeddings::AiError {
                    code: 2,
                    message: format!("HTTP request failed: {e}"),
                })?;

            let status = resp.status();
            let resp_text =
                resp.text()
                    .await
                    .map_err(|e| fractal::app::ai_embeddings::AiError {
                        code: 2,
                        message: format!("failed to read response body: {e}"),
                    })?;

            if !status.is_success() {
                return Err(fractal::app::ai_embeddings::AiError {
                    code: 2,
                    message: format!("Claude API error ({}): {}", status, resp_text),
                });
            }

            let parsed: serde_json::Value = serde_json::from_str(&resp_text).map_err(|e| {
                fractal::app::ai_embeddings::AiError {
                    code: 3,
                    message: format!("failed to parse response JSON: {e}"),
                }
            })?;

            let text = parsed["content"][0]["text"]
                .as_str()
                .ok_or(fractal::app::ai_embeddings::AiError {
                    code: 3,
                    message: format!(
                        "unexpected response structure (no content[0].text): {}",
                        &resp_text[..resp_text.len().min(200)]
                    ),
                })?
                .to_string();

            let tokens_used = parsed["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;

            tracing::info!(
                model = %config.model,
                tokens_used,
                "inference complete"
            );

            Ok(fractal::app::ai_inference::GenerateResponse {
                text,
                tokens_used,
                confidence: 1.0, // API responses don't have intrinsic confidence; guest decides
            })
        }

        #[cfg(not(feature = "inference"))]
        {
            let _ = request;
            Err(fractal::app::ai_embeddings::AiError {
                code: 1,
                message: "inference support not compiled in".into(),
            })
        }
    }
}

// ── Arrow IPC encoding/decoding ──

/// Encode Arrow RecordBatches into IPC streaming format bytes.
fn encode_ipc(
    batches: &[arrow::record_batch::RecordBatch],
) -> Result<Vec<u8>, arrow::error::ArrowError> {
    use arrow::ipc::writer::StreamWriter;

    if batches.is_empty() {
        // Return empty IPC stream with no schema — caller handles empty results.
        return Ok(Vec::new());
    }
    let schema = batches[0].schema();
    let mut buf = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buf, &schema)?;
        for batch in batches {
            writer.write(batch)?;
        }
        writer.finish()?;
    }
    Ok(buf)
}

/// Decode Arrow IPC streaming format bytes into RecordBatches.
#[cfg(feature = "duckdb")]
fn decode_ipc(
    data: &[u8],
) -> Result<Vec<arrow::record_batch::RecordBatch>, arrow::error::ArrowError> {
    use arrow::ipc::reader::StreamReader;
    use std::io::Cursor;

    if data.is_empty() {
        return Ok(Vec::new());
    }
    let reader = StreamReader::try_new(Cursor::new(data), None)?;
    reader.into_iter().collect()
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

/// Optional host resources to attach when running a micro-app.
#[derive(Default)]
pub struct RunOptions {
    #[cfg(feature = "duckdb")]
    pub duck: Option<DuckStore>,
    #[cfg(feature = "inference")]
    pub inference: Option<InferenceConfig>,
}

/// Load, instantiate, and execute a micro-app component.
///
/// Pass host resources via [`RunOptions`] to enable data and inference host functions.
pub async fn run_component(
    wasm_path: &Path,
    fuel: u64,
    opts: RunOptions,
) -> anyhow::Result<RunResult> {
    let engine = create_engine()?;
    let component = load_component(&engine, wasm_path).await?;
    let linker = create_linker(&engine)?;

    let mut state = HostState::new();
    #[cfg(feature = "duckdb")]
    if let Some(store) = opts.duck {
        state = state.with_duck(store);
    }
    #[cfg(feature = "inference")]
    if let Some(config) = opts.inference {
        state = state.with_inference(config);
    }

    let mut store = Store::new(&engine, state);
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

    /// Helper: run hello-world guest with no host resources attached.
    async fn run_hello_world(fuel: u64) -> RunResult {
        run_component(&hello_world_wasm(), fuel, RunOptions::default())
            .await
            .expect("run_component failed")
    }

    #[tokio::test]
    async fn run_returns_ok() {
        let result = run_hello_world(1_000_000_000).await;
        assert_eq!(
            result.output,
            Ok("Hello from the first Fractalaw micro-app!".to_string())
        );
    }

    #[tokio::test]
    async fn audit_entry_recorded() {
        let result = run_hello_world(1_000_000_000).await;
        assert_eq!(result.audit_entries.len(), 1);
        let entry = &result.audit_entries[0];
        assert_eq!(entry.event_type, "app-started");
        assert_eq!(entry.resource, "hello-world");
        assert_eq!(entry.detail, "Bootstrap test — first micro-app execution");
    }

    #[tokio::test]
    async fn fuel_consumed() {
        let budget = 1_000_000_000u64;
        let result = run_hello_world(budget).await;
        assert!(result.fuel_consumed > 0, "should have consumed some fuel");
        assert!(
            result.fuel_consumed < budget,
            "should not have exhausted the full budget"
        );
    }

    // ── Data host function unit tests ──

    #[cfg(feature = "duckdb")]
    mod data_tests {
        use super::*;
        use fractalaw_store::DuckStore;

        fn state_with_duck() -> HostState {
            let store = DuckStore::open().unwrap();
            store
                .execute("CREATE TABLE test_data (id INTEGER, name VARCHAR)")
                .unwrap();
            store
                .execute("INSERT INTO test_data VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma')")
                .unwrap();
            HostState::new().with_duck(store)
        }

        #[tokio::test]
        async fn query_returns_ipc_bytes() {
            use fractal::app::data_query::Host;

            let mut state = state_with_duck();
            let bytes = state
                .query("SELECT id, name FROM test_data ORDER BY id".into())
                .await
                .expect("query failed");

            assert!(!bytes.is_empty(), "IPC bytes should not be empty");

            // Decode and verify
            let batches = decode_ipc(&bytes).unwrap();
            let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
            assert_eq!(total_rows, 3);
        }

        #[tokio::test]
        async fn query_without_duck_errors() {
            use fractal::app::data_query::Host;

            let mut state = HostState::new(); // no DuckStore
            let err = state.query("SELECT 1".into()).await.unwrap_err();
            assert_eq!(err.code, 1);
            assert!(err.message.contains("no DuckDB store"));
        }

        #[tokio::test]
        async fn query_invalid_sql_errors() {
            use fractal::app::data_query::Host;

            let mut state = state_with_duck();
            let err = state
                .query("SELECT * FROM nonexistent_table".into())
                .await
                .unwrap_err();
            assert_eq!(err.code, 2);
        }

        #[tokio::test]
        async fn execute_runs_ddl() {
            use fractal::app::data_mutate::Host;

            let mut state = state_with_duck();
            state
                .execute("CREATE TABLE new_table (x INTEGER)".into())
                .await
                .expect("execute failed");

            // Verify via query
            use fractal::app::data_query::Host as QHost;
            let bytes = state
                .query("SELECT count(*)::BIGINT AS cnt FROM new_table".into())
                .await
                .expect("query failed");
            assert!(!bytes.is_empty());
        }

        #[tokio::test]
        async fn insert_arrow_ipc_roundtrip() {
            use arrow::array::{Int32Array, StringArray};
            use arrow::datatypes::{DataType, Field, Schema};
            use arrow::ipc::writer::StreamWriter;
            use arrow::record_batch::RecordBatch;
            use fractal::app::data_mutate::Host;
            use std::sync::Arc;

            let mut state = state_with_duck();

            // Build an Arrow IPC payload
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int32, true),
                Field::new("name", DataType::Utf8, true),
            ]));
            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(Int32Array::from(vec![10, 20])),
                    Arc::new(StringArray::from(vec!["delta", "epsilon"])),
                ],
            )
            .unwrap();

            let mut buf = Vec::new();
            {
                let mut writer = StreamWriter::try_new(&mut buf, &schema).unwrap();
                writer.write(&batch).unwrap();
                writer.finish().unwrap();
            }

            let rows = state
                .insert("test_data".into(), buf)
                .await
                .expect("insert failed");
            assert_eq!(rows, 2);

            // Verify total count is now 5 (3 original + 2 inserted)
            use fractal::app::data_query::Host as QHost;
            let bytes = state
                .query("SELECT count(*)::BIGINT AS cnt FROM test_data".into())
                .await
                .unwrap();
            let batches = decode_ipc(&bytes).unwrap();
            let col = batches[0]
                .column(0)
                .as_any()
                .downcast_ref::<arrow::array::Int64Array>()
                .unwrap();
            assert_eq!(col.value(0), 5);
        }

        #[tokio::test]
        async fn insert_without_duck_errors() {
            use fractal::app::data_mutate::Host;

            let mut state = HostState::new();
            let err = state.insert("test".into(), vec![]).await.unwrap_err();
            assert_eq!(err.code, 1);
        }

        // ── Integration test: data-test guest with DuckDB ──

        fn data_test_wasm() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../guests/data-test/target/wasm32-wasip1/release/data_test.wasm")
        }

        #[tokio::test]
        async fn data_test_guest_end_to_end() {
            let duck = DuckStore::open().unwrap();
            let opts = RunOptions {
                duck: Some(duck),
                #[cfg(feature = "inference")]
                inference: None,
            };
            let result = run_component(&data_test_wasm(), 1_000_000_000, opts)
                .await
                .expect("run_component with data-test guest failed");

            // Guest should return Ok with a summary message
            let output = result.output.expect("guest returned Err");
            assert!(
                output.contains("Data test passed"),
                "unexpected output: {output}"
            );
            assert!(
                output.contains("IPC bytes"),
                "should mention IPC bytes: {output}"
            );

            // Should have 3 audit entries: app-started, ddl-complete, query-complete
            assert_eq!(
                result.audit_entries.len(),
                3,
                "expected 3 audit entries, got: {:?}",
                result
                    .audit_entries
                    .iter()
                    .map(|e| &e.event_type)
                    .collect::<Vec<_>>()
            );
            assert_eq!(result.audit_entries[0].event_type, "app-started");
            assert_eq!(result.audit_entries[1].event_type, "ddl-complete");
            assert_eq!(result.audit_entries[2].event_type, "query-complete");

            assert!(result.fuel_consumed > 0);
        }

        // ── DRRP polisher integration tests ──

        fn drrp_polisher_wasm() -> PathBuf {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../guests/drrp-polisher/target/wasm32-wasip1/release/drrp_polisher.wasm")
        }

        #[tokio::test]
        async fn drrp_polisher_no_annotations() {
            let duck = DuckStore::open().unwrap();
            let opts = RunOptions {
                duck: Some(duck),
                #[cfg(feature = "inference")]
                inference: None,
            };
            let result = run_component(&drrp_polisher_wasm(), 1_000_000_000, opts)
                .await
                .expect("run_component failed");

            let output = result.output.expect("guest returned Err");
            assert!(
                output.contains("No unpolished annotations"),
                "expected empty batch message, got: {output}"
            );
        }

        #[tokio::test]
        async fn drrp_polisher_reports_inference_errors() {
            let duck = DuckStore::open().unwrap();
            // Create tables and seed a test annotation.
            duck.execute(
                "CREATE TABLE drrp_annotations (
                    law_name VARCHAR NOT NULL, provision VARCHAR NOT NULL,
                    drrp_type VARCHAR NOT NULL, source_text VARCHAR NOT NULL,
                    confidence FLOAT NOT NULL, scraped_at TIMESTAMPTZ NOT NULL,
                    polished BOOLEAN NOT NULL DEFAULT false,
                    synced_at TIMESTAMPTZ NOT NULL
                )",
            )
            .unwrap();
            duck.execute(
                "INSERT INTO drrp_annotations VALUES (
                    'UK_ukpga_1974_37', 's.2(1)', 'duty',
                    'It shall be the duty of every employer to ensure, so far as is reasonably practicable, the health, safety and welfare at work of all his employees.',
                    0.95, '2026-01-15T10:00:00Z', false, '2026-01-15T10:00:00Z'
                )",
            )
            .unwrap();

            let opts = RunOptions {
                duck: Some(duck),
                #[cfg(feature = "inference")]
                inference: None, // no API key → inference calls will error
            };
            let result = run_component(&drrp_polisher_wasm(), 1_000_000_000, opts)
                .await
                .expect("run_component failed");

            // Guest should succeed overall but report 1 error (inference not configured).
            let output = result.output.expect("guest returned Err");
            assert!(
                output.contains("1 errors"),
                "expected 1 inference error, got: {output}"
            );
            assert!(
                output.contains("Polished 0/1"),
                "expected 0 polished, got: {output}"
            );
        }
    }

    // ── AI host function unit tests ──

    mod ai_tests {
        use super::*;

        #[tokio::test]
        async fn embed_returns_not_configured() {
            use fractal::app::ai_embeddings::Host;

            let mut state = HostState::new();
            let err = state.embed("test".into()).await.unwrap_err();
            assert_eq!(err.code, 1);
            assert!(err.message.contains("not configured"));
        }

        #[tokio::test]
        async fn embed_batch_returns_not_configured() {
            use fractal::app::ai_embeddings::Host;

            let mut state = HostState::new();
            let err = state
                .embed_batch(vec!["a".into(), "b".into()])
                .await
                .unwrap_err();
            assert_eq!(err.code, 1);
        }

        #[tokio::test]
        async fn generate_without_config_errors() {
            use fractal::app::ai_inference::Host;

            let mut state = HostState::new();
            let request = fractal::app::ai_inference::GenerateRequest {
                system_prompt: None,
                user_prompt: "Hello".into(),
                max_tokens: 100,
                temperature: 0.0,
            };
            let err = state.generate(request).await.unwrap_err();
            assert_eq!(err.code, 1);
            assert!(err.message.contains("ANTHROPIC_API_KEY"));
        }
    }
}
