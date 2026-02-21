#[allow(warnings)]
mod bindings;

use bindings::fractal::app::{ai_inference, audit_log};
use bindings::Guest;

struct InferenceTest;

impl Guest for InferenceTest {
    fn run() -> Result<String, String> {
        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "app-started".to_string(),
            resource: "inference-test".to_string(),
            detail: "AI inference integration test".to_string(),
        });

        let request = ai_inference::GenerateRequest {
            system_prompt: Some("You are a concise assistant. Reply in one short sentence.".into()),
            user_prompt: "What is 2+2?".into(),
            max_tokens: 64,
            temperature: 0.0,
        };

        let response = ai_inference::generate(&request)
            .map_err(|e| format!("Inference failed: {} (code {})", e.message, e.code))?;

        audit_log::record_event(&audit_log::AuditEntry {
            event_type: "inference-complete".to_string(),
            resource: "inference-test".to_string(),
            detail: format!(
                "tokens_used={}, confidence={}",
                response.tokens_used, response.confidence
            ),
        });

        Ok(format!(
            "Inference test passed: \"{}\" (tokens: {})",
            response.text, response.tokens_used
        ))
    }
}

bindings::export!(InferenceTest with_types_in bindings);
