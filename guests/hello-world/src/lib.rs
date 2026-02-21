#[allow(warnings)]
mod bindings;

use bindings::fractal::app::audit_log;
use bindings::Guest;

struct HelloWorld;

impl Guest for HelloWorld {
    fn run() -> Result<String, String> {
        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "app-started".to_string(),
            resource: "hello-world".to_string(),
            detail: "Bootstrap test — first micro-app execution".to_string(),
        });

        Ok("Hello from the first Fractalaw micro-app!".to_string())
    }
}

bindings::export!(HelloWorld with_types_in bindings);
