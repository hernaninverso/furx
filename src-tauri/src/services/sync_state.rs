// 2.39 — Multi-device sync MVP via git.
// `git init` en ~/.furx/sync/, copia settings/snapshots/cards.json,
// commit + remote push si configurado.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct SyncReport {
    pub commit: Option<String>,
    pub pushed: bool,
    pub remote: Option<String>,
    pub bytes: u64,
}

fn sync_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let d = home.join(".furx").join("sync");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub fn snapshot_and_commit(db: &Mutex<Connection>, remote: Option<&str>) -> Result<SyncReport> {
    let dir = sync_dir()?;
    // Init repo idempotente.
    if !dir.join(".git").is_dir() {
        Command::new("git")
            .current_dir(&dir)
            .args(["init", "-b", "main"])
            .output()?;
    }
    // Dump settings + recent snapshot ids to files.
    {
        let conn = db.lock();
        let mut s_set = conn.prepare("SELECT key, value FROM settings")?;
        let it = s_set.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let settings: serde_json::Value = serde_json::Value::Object(
            it.filter_map(|x| x.ok())
                .map(|(k, v)| {
                    // SECURITY: redact secret-shaped values before writing a file that may be
                    // pushed to a remote. BYOK keys live in the keychain, but a setting value
                    // could still embed a token / webhook secret. Redact, then parse-or-string.
                    let v = crate::services::providers::redact_for_log(&v);
                    (
                        k,
                        serde_json::from_str(&v).unwrap_or(serde_json::Value::String(v)),
                    )
                })
                .collect(),
        );
        std::fs::write(
            dir.join("settings.json"),
            serde_json::to_string_pretty(&settings)?,
        )?;
        let mut s_snap = conn.prepare(
            "SELECT id, at, kind, schema_version FROM snapshots ORDER BY at DESC LIMIT 100",
        )?;
        let snaps: Vec<serde_json::Value> = s_snap
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, String>(0)?, "at": r.get::<_, String>(1)?,
                    "kind": r.get::<_, String>(2)?, "schema_version": r.get::<_, i64>(3)?,
                }))
            })?
            .filter_map(|x| x.ok())
            .collect();
        std::fs::write(
            dir.join("snapshots.json"),
            serde_json::to_string_pretty(&snaps)?,
        )?;
    }
    // git add + commit.
    Command::new("git")
        .current_dir(&dir)
        .args(["add", "-A"])
        .output()?;
    let commit_out = Command::new("git")
        .current_dir(&dir)
        .args([
            "-c",
            "user.email=furx@localhost",
            "-c",
            "user.name=Furx Sync",
            "commit",
            "-m",
            &format!("sync {}", chrono::Utc::now().to_rfc3339()),
            "--allow-empty",
        ])
        .output()?;
    let commit = if commit_out.status.success() {
        let head = Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD"])
            .output()?;
        Some(String::from_utf8_lossy(&head.stdout).trim().to_string())
    } else {
        None
    };

    // Push si remote configurado.
    let pushed = if let Some(r) = remote {
        if !is_safe_remote(r) {
            return Err(anyhow!("unsafe remote URL"));
        }
        // Configure origin idempotente.
        let _ = Command::new("git")
            .current_dir(&dir)
            .args(["remote", "remove", "origin"])
            .output();
        Command::new("git")
            .current_dir(&dir)
            .args(["remote", "add", "origin", r])
            .output()?;
        let push = Command::new("git")
            .current_dir(&dir)
            .args(["push", "-u", "origin", "main"])
            .output()?;
        push.status.success()
    } else {
        false
    };

    let bytes = file_size(&dir.join("settings.json")) + file_size(&dir.join("snapshots.json"));

    // Persist last sync.
    db.lock().execute(
        "INSERT INTO sync_state (key, value) VALUES ('last_sync', ?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=datetime('now')",
        params![chrono::Utc::now().to_rfc3339()],
    )?;

    Ok(SyncReport {
        commit,
        pushed,
        remote: remote.map(String::from),
        bytes,
    })
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn is_safe_remote(r: &str) -> bool {
    // Permitir ssh git@host:repo o https://host/repo
    if r.starts_with("git@") || r.starts_with("https://") || r.starts_with("ssh://") {
        !r.contains('`') && !r.contains('$') && !r.contains(';') && r.len() < 256
    } else {
        false
    }
}
