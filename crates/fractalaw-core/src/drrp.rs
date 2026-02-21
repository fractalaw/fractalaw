//! Shared DRRP types for sync between fractalaw and sertantai.

use serde::{Deserialize, Serialize};

/// A rough DRRP annotation from sertantai's regex-based detection.
///
/// Pulled from sertantai's outbox and stored in `drrp_annotations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub law_name: String,
    pub provision: String,
    pub drrp_type: String,
    pub source_text: String,
    pub confidence: f32,
    /// ISO 8601 timestamp string.
    pub scraped_at: String,
}

/// An AI-refined DRRP provision produced by the drrp-polisher micro-app.
///
/// Stored in `polished_drrp` and pushed to sertantai's inbox.
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
    /// ISO 8601 timestamp string.
    pub polished_at: String,
    pub model: String,
}
