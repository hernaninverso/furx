// 2.2 — Diff-aware AI review on save. On-demand: caller pasa file_path
// (validado $HOME), leemos `git diff HEAD -- <path>`, mandamos a AIE,
// recibimos comments por línea.
//
// Council V4: cap file 10MB, reject binary, reject outside $HOME.
// V3: timeout 15s, no FS watcher en backend (frontend triggers).

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const MAX_DIFF_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ReviewComment {
    pub line_hint: Option<u32>,
    pub severity: String, // "info" | "warning" | "critical"
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewResult {
    pub file_path: String,
    pub diff_lines: usize,
    pub comments: Vec<ReviewComment>,
    pub raw_response: String,
    pub elapsed_ms: u64,
}

pub async fn review(file_path: &Path) -> Result<ReviewResult> {
    let abs = file_path.canonicalize()?;
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    if !abs.starts_with(&home) {
        return Err(anyhow!("file outside $HOME"));
    }
    let parent = abs.parent().ok_or_else(|| anyhow!("no parent"))?;
    // Find repo root.
    let repo = find_repo_root(parent).ok_or_else(|| anyhow!("no git repo containing file"))?;
    let started = std::time::Instant::now();
    let rel = abs
        .strip_prefix(&repo)
        .unwrap_or(&abs)
        .to_string_lossy()
        .to_string();
    let out = Command::new("git")
        .current_dir(&repo)
        .args(["diff", "HEAD", "--", &rel])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if !out.status.success() {
        return Err(anyhow!("git diff failed"));
    }
    let mut diff = String::from_utf8_lossy(&out.stdout).to_string();
    if diff.trim().is_empty() {
        return Ok(ReviewResult {
            file_path: rel,
            diff_lines: 0,
            comments: vec![],
            raw_response: "(no changes)".into(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }
    if diff.len() > MAX_DIFF_BYTES {
        diff.truncate(MAX_DIFF_BYTES);
        diff.push_str("\n…(truncated)\n");
    }
    let diff_lines = diff.lines().count();
    // Redact secrets before sending.
    let (red, _hits) = crate::bases::guardrail::redact(&diff);
    let prompt = format!(
        "Reviewé este diff. Devuelve SOLO líneas en formato `[severity] mensaje`, una por hallazgo. \
        Severities: info|warning|critical. Sin retórica, sin código.\n\n```diff\n{}\n```",
        red
    );
    let bearer = crate::services::keychain_bearer::get_bearer()
        .ok_or_else(|| anyhow!("missing bearer"))?;
    let body = serde_json::json!({
        "model": "bulk_free", "max_tokens": 600, "temperature": 0.3,
        "messages": [
            {"role": "system", "content": "Eres senior code reviewer. Severo, conciso, español."},
            {"role": "user", "content": prompt},
        ]
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    // BLOQUE J: respect FURX_AIE_URL env / DEFAULT_AIE_URL — no hard-coded URL.
    let aie_url = format!(
        "{}/v1/chat/completions",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let resp = client
        .post(&aie_url)
        .bearer_auth(bearer)
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
        return Err(anyhow!("AIE {}", status));
    }
    let v: serde_json::Value = resp.json().await?;
    let raw = v
        .pointer("/choices/0/message/content")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let comments = parse_comments(&raw);
    Ok(ReviewResult {
        file_path: rel,
        diff_lines,
        comments,
        raw_response: raw,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn parse_comments(text: &str) -> Vec<ReviewComment> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*\[?\s*(info|warning|critical|warn|err|error)\s*\]?\s*[:\-]?\s*(.+)$")
            .unwrap()
    });
    text.lines()
        .filter_map(|l| {
            let caps = RE.captures(l.trim())?;
            let raw_sev = caps[1].to_ascii_lowercase();
            let sev = match raw_sev.as_str() {
                "warn" | "warning" => "warning",
                "err" | "error" | "critical" => "critical",
                _ => "info",
            };
            Some(ReviewComment {
                line_hint: None,
                severity: sev.to_string(),
                message: caps[2].trim().to_string(),
            })
        })
        .collect()
}

fn find_repo_root(start: &Path) -> Option<std::path::PathBuf> {
    let mut p = start.to_path_buf();
    loop {
        if p.join(".git").exists() {
            return Some(p);
        }
        if !p.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_warning_comment() {
        let r = parse_comments("[warning] potential null deref\n[info] indentation");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].severity, "warning");
        assert_eq!(r[1].severity, "info");
    }
}
