#[allow(warnings)]
mod bindings;
mod ipc;

use bindings::fractal::app::{ai_inference, audit_log, data_mutate, data_query};
use bindings::Guest;
use serde::Deserialize;

struct DrrpPolisher;

// ── Prompt templates ──

const SYSTEM_PROMPT: &str = "\
You are a legal provision extractor specialising in UK ESH (environment, safety, health) legislation.

Given a section of legislation text and a DRRP type (duty, right, responsibility, or power), \
extract the precise provision.

Respond ONLY with a JSON object. No markdown fences, no explanation, just raw JSON:
{
  \"holder\": \"the entity that holds this duty/right/responsibility/power\",
  \"text\": \"the exact provision text, quoted from the source\",
  \"qualifier\": \"any qualifying phrase (e.g. 'so far as is reasonably practicable')\" or null,
  \"clause_ref\": \"the specific clause reference (e.g. 's.2(1)')\"
}

If the source text contains multiple provisions, extract the primary one that best matches the DRRP type.
If you cannot identify a clear provision, set holder to \"unknown\" and text to the most relevant sentence.";

fn build_user_prompt(ann: &Annotation) -> String {
    format!(
        "Law: {law}\n\
         Section: {provision}\n\
         DRRP type: {drrp_type}\n\
         Regex confidence: {confidence:.2}\n\
         \n\
         Source text:\n\
         {source_text}",
        law = ann.law_name,
        provision = ann.provision,
        drrp_type = ann.drrp_type,
        confidence = ann.confidence,
        source_text = ann.source_text,
    )
}

// ── Types ──

#[derive(Deserialize)]
struct Annotation {
    law_name: String,
    provision: String,
    drrp_type: String,
    source_text: String,
    confidence: f64,
}

#[derive(Deserialize)]
struct PolishedEntry {
    holder: String,
    text: String,
    qualifier: Option<String>,
    clause_ref: String,
}

// ── Helpers ──

fn audit(event_type: &str, detail: &str) {
    audit_log::record_event(&audit_log::AuditEntry {
        event_type: event_type.to_string(),
        resource: "drrp-polisher".to_string(),
        detail: detail.to_string(),
    });
}

fn query_i64(sql: &str) -> Result<i64, String> {
    let ipc = data_query::query(sql)
        .map_err(|e| format!("query failed: {} (code {})", e.message, e.code))?;
    ipc::extract_i64(&ipc).ok_or_else(|| "failed to parse i64 from IPC result".to_string())
}

fn query_string(sql: &str) -> Result<String, String> {
    let ipc = data_query::query(sql)
        .map_err(|e| format!("query failed: {} (code {})", e.message, e.code))?;
    ipc::extract_string(&ipc).ok_or_else(|| "failed to parse string from IPC result".to_string())
}

fn execute(sql: &str) -> Result<u64, String> {
    data_mutate::execute(sql)
        .map_err(|e| format!("execute failed: {} (code {})", e.message, e.code))
}

/// Escape a string for use in SQL single-quoted literals.
fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

// ── Guest entry point ──

impl Guest for DrrpPolisher {
    fn run() -> Result<String, String> {
        audit("app-started", "DRRP polisher run starting");

        // 1. Ensure DRRP tables exist (idempotent).
        execute(
            "CREATE TABLE IF NOT EXISTS drrp_annotations (
                law_name       VARCHAR NOT NULL,
                provision      VARCHAR NOT NULL,
                drrp_type      VARCHAR NOT NULL,
                source_text    VARCHAR NOT NULL,
                confidence     FLOAT   NOT NULL,
                scraped_at     TIMESTAMPTZ NOT NULL,
                polished       BOOLEAN NOT NULL DEFAULT false,
                synced_at      TIMESTAMPTZ NOT NULL
            )",
        )?;

        execute(
            "CREATE TABLE IF NOT EXISTS polished_drrp (
                law_name       VARCHAR NOT NULL,
                provision      VARCHAR NOT NULL,
                drrp_type      VARCHAR NOT NULL,
                holder         VARCHAR NOT NULL,
                text           VARCHAR NOT NULL,
                qualifier      VARCHAR,
                clause_ref     VARCHAR NOT NULL,
                confidence     FLOAT   NOT NULL,
                polished_at    TIMESTAMPTZ NOT NULL,
                model          VARCHAR NOT NULL,
                pushed         BOOLEAN NOT NULL DEFAULT false
            )",
        )?;

        // 2. Count unpolished annotations.
        let count =
            query_i64("SELECT count(*)::BIGINT FROM drrp_annotations WHERE polished = false")?;

        if count == 0 {
            audit("batch-empty", "no unpolished annotations found");
            return Ok("No unpolished annotations to process.".to_string());
        }

        audit("batch-start", &format!("{count} annotations to polish"));

        // 3. Process each annotation one at a time.
        let mut polished = 0u64;
        let mut errors = 0u64;
        let mut total_tokens = 0u32;

        for i in 0..count {
            match process_one(i) {
                Ok(tokens) => {
                    polished += 1;
                    total_tokens += tokens;
                }
                Err(e) => {
                    audit("polish-error", &format!("annotation {i}: {e}"));
                    errors += 1;
                }
            }
        }

        let summary = format!(
            "Polished {polished}/{count} annotations ({errors} errors, {total_tokens} tokens used)"
        );
        audit("batch-complete", &summary);
        Ok(summary)
    }
}

/// Process a single unpolished annotation. Returns tokens used on success.
fn process_one(offset: i64) -> Result<u32, String> {
    // Query annotation as JSON via DuckDB's to_json + struct_pack.
    let json_str = query_string(&format!(
        "SELECT to_json(struct_pack(
            law_name := law_name,
            provision := provision,
            drrp_type := drrp_type,
            source_text := source_text,
            confidence := confidence
        )) FROM drrp_annotations
        WHERE polished = false
        ORDER BY confidence DESC
        LIMIT 1 OFFSET {offset}"
    ))?;

    let ann: Annotation =
        serde_json::from_str(&json_str).map_err(|e| format!("parse annotation JSON: {e}"))?;

    // Call Claude to extract the precise DRRP provision.
    let user_prompt = build_user_prompt(&ann);
    let response = ai_inference::generate(&ai_inference::GenerateRequest {
        system_prompt: Some(SYSTEM_PROMPT.to_string()),
        user_prompt,
        max_tokens: 1024,
        temperature: 0.0,
    })
    .map_err(|e| format!("inference: {} (code {})", e.message, e.code))?;

    // Parse the structured JSON response from Claude.
    let entry: PolishedEntry = serde_json::from_str(&response.text).map_err(|e| {
        format!(
            "parse Claude response: {e}\nraw: {}",
            &response.text[..response.text.len().min(200)]
        )
    })?;

    // Insert polished result.
    let qualifier_sql = match &entry.qualifier {
        Some(q) => format!("'{}'", sql_escape(q)),
        None => "NULL".to_string(),
    };

    execute(&format!(
        "INSERT INTO polished_drrp (
            law_name, provision, drrp_type, holder, text, qualifier,
            clause_ref, confidence, polished_at, model, pushed
        ) VALUES (
            '{law_name}', '{provision}', '{drrp_type}', '{holder}', '{text}', {qualifier},
            '{clause_ref}', {confidence}, CURRENT_TIMESTAMP, 'claude', false
        )",
        law_name = sql_escape(&ann.law_name),
        provision = sql_escape(&ann.provision),
        drrp_type = sql_escape(&ann.drrp_type),
        holder = sql_escape(&entry.holder),
        text = sql_escape(&entry.text),
        qualifier = qualifier_sql,
        clause_ref = sql_escape(&entry.clause_ref),
        confidence = response.confidence,
    ))?;

    // Mark the source annotation as polished.
    execute(&format!(
        "UPDATE drrp_annotations SET polished = true
         WHERE law_name = '{law_name}' AND provision = '{provision}' AND polished = false",
        law_name = sql_escape(&ann.law_name),
        provision = sql_escape(&ann.provision),
    ))?;

    Ok(response.tokens_used)
}

bindings::export!(DrrpPolisher with_types_in bindings);
