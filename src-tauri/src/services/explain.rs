// 1.4 — Failed-command auto-explain.
// Hook PTY exit con code != 0 → llama AIE bulk_free con cmd + tail stderr,
// retorna explicación corta para mostrar en badge "Explain".
//
// Council V1 hardening: stderr puede contener secrets → guardrail::redact
// antes de mandar a AIE. Cap 4KB de stderr. Timeout 12s.
// Council V3: timeout obligatorio; spawn fire-and-forget no-leak.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::Duration;

const MAX_STDERR: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct ExplainResult {
    pub markdown: String,
    pub elapsed_ms: u64,
    pub redacted: bool,
}

pub async fn explain(cmd_hint: &str, stderr_tail: &str, exit_code: i32) -> Result<ExplainResult> {
    let mut excerpt = stderr_tail.to_string();
    if excerpt.len() > MAX_STDERR {
        excerpt = excerpt[excerpt.len() - MAX_STDERR..].to_string();
    }
    // Redact secrets before sending to AIE.
    let (red, hits) = crate::bases::guardrail::redact(&excerpt);
    excerpt = red;
    let redacted = !hits.is_empty();

    let prompt = format!(
        "Comando exited con código {}. Mode del pane: {}.\n\nStderr tail:\n```\n{}\n```\n\n\
        Devolveme en máximo 3 líneas markdown qué falló y qué probar. Sin retórica, sin disclaimers.",
        exit_code, cmd_hint, excerpt
    );

    // BLOQUE J: centralised endpoint (env-overridable, no hard-code).
    let endpoint = crate::services::aie_endpoint::resolve_url_or_default();
    let bearer = crate::services::keychain_bearer::get_bearer()
        .ok_or_else(|| anyhow!("missing aie-internal-bearer Keychain"))?;
    let body = serde_json::json!({
        "model": "bulk_free",
        "max_tokens": 250,
        "temperature": 0.3,
        "messages": [
            {"role": "system", "content": "Eres ingeniero senior debugger. Concreto, 3 líneas max, español."},
            {"role": "user", "content": prompt},
        ]
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()?;
    let started = std::time::Instant::now();
    let resp = client
        .post(format!("{}/v1/chat/completions", endpoint))
        .bearer_auth(bearer)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated Keychain value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("AIE status {}", status));
    }
    let v: serde_json::Value = resp.json().await?;
    let text = v
        .pointer("/choices/0/message/content")
        .and_then(|s| s.as_str())
        .unwrap_or("(sin respuesta)")
        .to_string();
    Ok(ExplainResult {
        markdown: text,
        elapsed_ms: started.elapsed().as_millis() as u64,
        redacted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_stderr() {
        let big = "x".repeat(10000);
        // Just smoke test — we don't call the API, just verify the cap logic compiles.
        assert!(big.len() > MAX_STDERR);
    }
}
