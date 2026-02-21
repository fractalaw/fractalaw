#[allow(warnings)]
mod bindings;

use bindings::fractal::app::{audit_log, data_mutate, data_query};
use bindings::Guest;

struct DataTest;

impl Guest for DataTest {
    fn run() -> Result<String, String> {
        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "app-started".to_string(),
            resource: "data-test".to_string(),
            detail: "Data host function integration test".to_string(),
        });

        // Step 1: DDL — create a table
        data_mutate::execute(
            "CREATE TABLE IF NOT EXISTS test_from_guest (id INTEGER, msg VARCHAR)",
        )
        .map_err(|e| format!("DDL failed: {} (code {})", e.message, e.code))?;

        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "ddl-complete".to_string(),
            resource: "test_from_guest".to_string(),
            detail: "Created table test_from_guest".to_string(),
        });

        // Step 2: DML — insert rows
        data_mutate::execute(
            "INSERT INTO test_from_guest VALUES (1, 'hello from wasm'), (2, 'data host works')",
        )
        .map_err(|e| format!("INSERT failed: {} (code {})", e.message, e.code))?;

        // Step 3: Query — read back via Arrow IPC
        let ipc_bytes = data_query::query("SELECT id, msg FROM test_from_guest ORDER BY id")
            .map_err(|e| format!("Query failed: {} (code {})", e.message, e.code))?;

        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "query-complete".to_string(),
            resource: "test_from_guest".to_string(),
            detail: format!("Query returned {} IPC bytes", ipc_bytes.len()),
        });

        // Step 4: Clean up
        data_mutate::execute("DROP TABLE test_from_guest")
            .map_err(|e| format!("DROP failed: {} (code {})", e.message, e.code))?;

        Ok(format!(
            "Data test passed: DDL + INSERT + QUERY ({} IPC bytes) + DROP",
            ipc_bytes.len()
        ))
    }
}

bindings::export!(DataTest with_types_in bindings);
