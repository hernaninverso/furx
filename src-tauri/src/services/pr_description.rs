// 1.9 — Auto-PR description from audit log + git diff stat.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct PrDescription {
    pub branch: String,
    pub markdown: String,
    pub commits_count: usize,
    pub elapsed_ms: u64,
}

pub async fn generate(
    db: Arc<Mutex<Connection>>,
    repo: &Path,
    base: &str,
) -> Result<PrDescription> {
    if !repo.is_dir() || !repo.join(".git").exists() {
        return Err(anyhow!("not a git repo: {}", repo.display()));
    }
    if !is_safe_ref(base) {
        return Err(anyhow!("unsafe base ref: {}", base));
    }
    let started = std::time::Instant::now();
    let branch = git_out(repo, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_default();
    if branch.trim().is_empty() {
        return Err(anyhow!("HEAD is detached"));
    }
    let commits =
        git_out(repo, &["log", "--oneline", &format!("{}..HEAD", base)]).unwrap_or_default();
    let commits_count = commits.lines().count();
    let stat = git_out(repo, &["diff", "--stat", &format!("{}...HEAD", base)]).unwrap_or_default();
    let audit_summary: String = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT kind, COUNT(*) FROM events \
             WHERE at >= datetime('now', '-7 days') \
             GROUP BY kind ORDER BY COUNT(*) DESC LIMIT 15",
        )?;
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}:{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?
                ))
            })?
            .filter_map(|x| x.ok())
            .collect();
        rows.join(", ")
    };
    let prompt = format!(
        "Generá PR description en markdown (Title / Summary / Test plan).\n\n\
        Branch: {}\nBase: {}\nCommits ({}):\n{}\n\nDiff stat:\n{}\n\n\
        Audit kinds últimos 7 días: {}\n\n\
        Formato exacto:\n## Summary\n- bullets concisos\n\n## Test plan\n- checklist markdown\n",
        branch.trim(),
        base,
        commits_count,
        commits,
        stat,
        audit_summary
    );
    let markdown = call_aie(&prompt).await
        .unwrap_or_else(|e| format!("(AIE unavailable: {})\n\n## Summary\n- {} commits on {}\n\n## Test plan\n- run cargo test\n- run npm run build", e, commits_count, branch.trim()));
    Ok(PrDescription {
        branch: branch.trim().to_string(),
        markdown,
        commits_count,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn is_safe_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 128
        && !s.contains("..")
        && !s.starts_with('/')
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn git_out(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

async fn call_aie(prompt: &str) -> Result<String> {
    let bearer = crate::services::keychain_bearer::get_bearer()
        .ok_or_else(|| anyhow!("missing aie-internal-bearer"))?;
    let body = serde_json::json!({
        "model": "bulk_free", "max_tokens": 700, "temperature": 0.3,
        "messages": [
            {"role": "system", "content": "Eres PR description writer. Markdown limpio. Sin retórica."},
            {"role": "user", "content": prompt},
        ]
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?;
    // BLOQUE J: respect FURX_AIE_URL env / DEFAULT_AIE_URL.
    let aie_url = format!(
        "{}/v1/chat/completions",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let resp = client
        .post(&aie_url)
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
    Ok(v.pointer("/choices/0/message/content")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_refs() {
        assert!(!is_safe_ref(""));
        assert!(!is_safe_ref("; rm -rf /"));
        assert!(!is_safe_ref("../../escape"));
        assert!(is_safe_ref("master"));
        assert!(is_safe_ref("feature/x-y_z"));
    }
}
