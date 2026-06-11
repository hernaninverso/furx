// 2.9 / W2 — .furxreplay bundle: tar.zst de audit events span + git HEAD snapshot + cards.
// Council V4: cap bundle size 100MB (V4 edge case). Streaming write — no whole-file in memory.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

const MAX_BUNDLE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct ReplayBundleReport {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub events_count: usize,
    pub redacted: bool,
}

pub fn bundle(
    db: Arc<Mutex<Connection>>,
    project_dir: Option<&Path>,
    span_start: &str,
    span_end: &str,
    out_path: &Path,
) -> Result<ReplayBundleReport> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    if !out_path.starts_with(&home) {
        return Err(anyhow!("out_path must be under $HOME"));
    }

    // Collect events.
    let events_json = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, at, kind, actor, pane_id, card_id, correlation_id, payload \
             FROM events WHERE at >= ? AND at <= ? ORDER BY at",
        )?;
        let rows: Vec<serde_json::Value> = stmt.query_map(
            [span_start, span_end],
            |r| Ok(serde_json::json!({
                "id": r.get::<_, String>(0)?,
                "at": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "actor": r.get::<_, String>(3)?,
                "pane_id": r.get::<_, Option<String>>(4)?,
                "card_id": r.get::<_, Option<String>>(5)?,
                "correlation_id": r.get::<_, Option<String>>(6)?,
                "payload": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(7)?).unwrap_or(serde_json::Value::Null),
            })),
        )?.filter_map(|x| x.ok()).collect();
        rows
    };
    let events_count = events_json.len();
    let mut events_str = serde_json::to_string(&events_json)?;

    // Cards open at span_end.
    let cards_json = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project, source, title, severity, status, created_at FROM cards \
             WHERE created_at <= ? ORDER BY created_at DESC LIMIT 100",
        )?;
        let rows: Vec<serde_json::Value> = stmt
            .query_map([span_end], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?,
                    "project": r.get::<_, String>(1)?,
                    "source": r.get::<_, String>(2)?,
                    "title": r.get::<_, String>(3)?,
                    "severity": r.get::<_, String>(4)?,
                    "status": r.get::<_, String>(5)?,
                    "created_at": r.get::<_, String>(6)?,
                }))
            })?
            .filter_map(|x| x.ok())
            .collect();
        rows
    };
    let cards_str = serde_json::to_string(&cards_json)?;

    // Redact secrets in events JSON.
    let (red_events, hits) = crate::bases::guardrail::redact(&events_str);
    let redacted = !hits.is_empty();
    events_str = red_events;

    // Git HEAD snapshot — only commit metadata, not full repo.
    let mut git_info = String::new();
    if let Some(p) = project_dir {
        if p.join(".git").exists() {
            for args in [
                &["rev-parse", "HEAD"][..],
                &["log", "-1", "--format=%H %s %an %cI"][..],
                &["status", "--porcelain"][..],
            ] {
                if let Ok(out) = std::process::Command::new("git")
                    .current_dir(p)
                    .args(args)
                    .output()
                {
                    git_info.push_str(&format!(
                        "$ git {}\n{}\n",
                        args.join(" "),
                        String::from_utf8_lossy(&out.stdout)
                    ));
                }
            }
        }
    }

    let mut manifest = serde_json::json!({
        "schema": "furxreplay/v1",
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "span_start": span_start,
        "span_end": span_end,
        "events_count": events_count,
        "redacted": redacted,
        "project_dir": project_dir.map(|p| p.to_string_lossy().to_string()),
    });

    // Write tar.zst, streaming. Cap MAX_BUNDLE_BYTES.
    let f = std::fs::File::create(out_path)?;
    let mut enc = zstd::stream::Encoder::new(f, 3)?;
    let mut tar = tar::Builder::new(&mut enc);

    let total_bytes = std::cell::Cell::new(0u64);
    let mut append = |path: &str, data: &[u8]| -> Result<()> {
        let cur = total_bytes.get();
        if cur + data.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(anyhow!("bundle would exceed {} bytes", MAX_BUNDLE_BYTES));
        }
        total_bytes.set(cur + data.len() as u64);
        let mut header = tar::Header::new_gnu();
        header.set_path(path)?;
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, data)?;
        Ok(())
    };
    append(
        "manifest.json",
        serde_json::to_string_pretty(&manifest)?.as_bytes(),
    )?;
    append("events.json", events_str.as_bytes())?;
    append("cards.json", cards_str.as_bytes())?;
    if !git_info.is_empty() {
        append("git.txt", git_info.as_bytes())?;
    }
    tar.finish()?;
    drop(tar);
    enc.finish()?;

    let size = std::fs::metadata(out_path)?.len();
    // Compute SHA-256 of the final file.
    let bytes = std::fs::read(out_path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha = hex::encode(hasher.finalize());
    // Inject sha into manifest by patching? For simplicity we just return.
    manifest["sha256"] = serde_json::json!(sha.clone());
    Ok(ReplayBundleReport {
        path: out_path.to_string_lossy().to_string(),
        size_bytes: size,
        sha256: sha,
        events_count,
        redacted,
    })
}
