//! HTTP sync client for pulling/pushing DRRP data to sertantai.

use chrono::{DateTime, Utc};
use fractalaw_core::{Annotation, PolishedEntry};
use serde::Deserialize;
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Server { status: u16, body: String },
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// HTTP sync client for sertantai's outbox/inbox endpoints.
pub struct SyncClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Deserialize)]
struct PushResponse {
    accepted: u64,
}

impl SyncClient {
    /// Create a new sync client for the given sertantai base URL.
    ///
    /// `base_url` should be like `http://localhost:4000` (no trailing slash).
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Pull new annotations from sertantai's outbox.
    ///
    /// If `since` is provided, only annotations scraped after that timestamp
    /// are returned. Otherwise, all annotations are returned.
    pub async fn pull_annotations(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Annotation>, SyncError> {
        let mut url = format!("{}/api/outbox/annotations", self.base_url);
        if let Some(ts) = since {
            url.push_str(&format!("?since={}", ts.to_rfc3339()));
        }

        info!(url = %url, "pulling annotations from sertantai");
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::Server {
                status: status.as_u16(),
                body,
            });
        }

        let annotations: Vec<Annotation> = resp.json().await?;
        info!(count = annotations.len(), "pulled annotations");
        Ok(annotations)
    }

    /// Push polished entries to sertantai's inbox.
    ///
    /// Returns the number of entries accepted by the server.
    pub async fn push_polished(&self, entries: &[PolishedEntry]) -> Result<u64, SyncError> {
        let url = format!("{}/api/inbox/polished", self.base_url);

        info!(url = %url, count = entries.len(), "pushing polished entries to sertantai");
        let resp = self.client.post(&url).json(entries).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::Server {
                status: status.as_u16(),
                body,
            });
        }

        let result: PushResponse = resp.json().await?;
        info!(accepted = result.accepted, "push complete");
        Ok(result.accepted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fractalaw_core::{Annotation, PolishedEntry};

    #[test]
    fn annotation_json_roundtrip() {
        let ann = Annotation {
            law_name: "UK_ukpga_1974_37".into(),
            provision: "s.2(1)".into(),
            drrp_type: "duty".into(),
            source_text: "It shall be the duty of every employer...".into(),
            confidence: 0.85,
            scraped_at: "2026-02-21T10:00:00Z".into(),
        };
        let json = serde_json::to_string(&ann).unwrap();
        let parsed: Annotation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.law_name, "UK_ukpga_1974_37");
        assert_eq!(parsed.provision, "s.2(1)");
        assert_eq!(parsed.confidence, 0.85);
    }

    #[test]
    fn polished_entry_json_roundtrip() {
        let entry = PolishedEntry {
            law_name: "UK_ukpga_1974_37".into(),
            provision: "s.2(1)".into(),
            drrp_type: "duty".into(),
            holder: "every employer".into(),
            text: "ensure health safety and welfare".into(),
            qualifier: Some("so far as is reasonably practicable".into()),
            clause_ref: "s.2(1)".into(),
            confidence: 0.95,
            polished_at: "2026-02-21T13:00:00Z".into(),
            model: "claude-sonnet-4-5-20250929".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: PolishedEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.holder, "every employer");
        assert_eq!(
            parsed.qualifier.as_deref(),
            Some("so far as is reasonably practicable")
        );
    }

    #[test]
    fn polished_entry_null_qualifier() {
        let json = r#"{
            "law_name": "UK_ukpga_1974_37",
            "provision": "s.3",
            "drrp_type": "duty",
            "holder": "every employer",
            "text": "conduct undertaking without risk",
            "qualifier": null,
            "clause_ref": "s.3",
            "confidence": 0.90,
            "polished_at": "2026-02-21T13:00:00Z",
            "model": "claude-sonnet-4-5-20250929"
        }"#;
        let parsed: PolishedEntry = serde_json::from_str(json).unwrap();
        assert!(parsed.qualifier.is_none());
    }

    #[test]
    fn annotation_array_json_roundtrip() {
        let annotations = vec![
            Annotation {
                law_name: "UK_ukpga_1974_37".into(),
                provision: "s.2(1)".into(),
                drrp_type: "duty".into(),
                source_text: "duty of employer".into(),
                confidence: 0.85,
                scraped_at: "2026-02-21T10:00:00Z".into(),
            },
            Annotation {
                law_name: "UK_ukpga_1974_37".into(),
                provision: "s.7(a)".into(),
                drrp_type: "duty".into(),
                source_text: "duty of employee".into(),
                confidence: 0.80,
                scraped_at: "2026-02-21T10:00:00Z".into(),
            },
        ];
        let json = serde_json::to_string(&annotations).unwrap();
        let parsed: Vec<Annotation> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].provision, "s.7(a)");
    }

    #[test]
    fn sync_client_trims_trailing_slash() {
        let client = SyncClient::new("http://localhost:4000/".into());
        assert_eq!(client.base_url, "http://localhost:4000");
    }
}
