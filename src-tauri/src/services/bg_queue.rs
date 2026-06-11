// 2.8 — Background job queue. Worker tokio + sqlite.
// Council V4 edge: concurrency limit via Semaphore = 8.
// Council V1: kind allowlist explícita.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const ALLOWED_KINDS: &[&str] = &[
    "pr_description",
    "standup",
    "explain",
    "council_review",
    "embeddings_index",
    "replay_bundle",
    "eval_run",
    // spec-011 — codebase-memory background indexing (FR-004). args:
    //   {"plugin":"codebase-memory","project_root":"/repo","project_key":"/repo"}
    "codebase_index",
];

#[derive(Debug, Clone, Serialize)]
pub struct BgJob {
    pub id: String,
    pub kind: String,
    pub args_json: String,
    pub status: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
}

pub fn enqueue(db: &Mutex<Connection>, kind: &str, args: serde_json::Value) -> Result<String> {
    if !ALLOWED_KINDS.contains(&kind) {
        return Err(anyhow!("kind not in allowlist: {}", kind));
    }
    let id = Uuid::new_v4().to_string();
    let args_json = serde_json::to_string(&args)?;
    let conn = db.lock();
    conn.execute(
        "INSERT INTO bg_jobs (id, kind, args_json, status) VALUES (?, ?, ?, 'pending')",
        params![id, kind, args_json],
    )?;
    Ok(id)
}

pub fn list(db: &Mutex<Connection>, limit: u32) -> Result<Vec<BgJob>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, kind, args_json, status, created_at, started_at, finished_at, output, error \
         FROM bg_jobs ORDER BY created_at DESC LIMIT ?",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(BgJob {
                id: r.get(0)?,
                kind: r.get(1)?,
                args_json: r.get(2)?,
                status: r.get(3)?,
                created_at: r.get(4)?,
                started_at: r.get(5)?,
                finished_at: r.get(6)?,
                output: r.get(7)?,
                error: r.get(8)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}

pub fn cancel(db: &Mutex<Connection>, id: &str) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "UPDATE bg_jobs SET status='cancelled', finished_at=datetime('now') \
         WHERE id=? AND status='pending'",
        params![id],
    )?;
    Ok(())
}

/// Worker loop — runs forever, polls pending jobs, spawns blocking executor.
pub async fn worker_loop(
    db: Arc<Mutex<Connection>>,
    audit: crate::bases::audit::AuditWriter,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let semaphore = Arc::new(Semaphore::new(8)); // V4 concurrency limit
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!("bg_queue worker shutdown");
                return;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
        let pending: Vec<(String, String, String)> = {
            let conn = db.lock();
            let mut stmt = match conn.prepare(
                "SELECT id, kind, args_json FROM bg_jobs WHERE status='pending' ORDER BY created_at LIMIT 8"
            ) { Ok(s) => s, Err(_) => continue };
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .ok()
                .map(|i| i.filter_map(|x| x.ok()).collect())
                .unwrap_or_default()
        };
        for (id, kind, args_json) in pending {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let db_c = db.clone();
            let audit_c = audit.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let res = run_job(db_c.clone(), &id, &kind, &args_json).await;
                let conn = db_c.lock();
                match res {
                    Ok(out) => {
                        let _ = conn.execute(
                            "UPDATE bg_jobs SET status='done', finished_at=datetime('now'), output=? WHERE id=?",
                            params![out, id],
                        );
                        audit_c
                            .write(crate::bases::audit::EventInput {
                                kind: "bg_job.done",
                                actor: "system",
                                pane_id: None,
                                card_id: None,
                                correlation_id: Some(&id),
                                payload: serde_json::json!({"kind": &kind}),
                            })
                            .ok();
                    }
                    Err(e) => {
                        let _ = conn.execute(
                            "UPDATE bg_jobs SET status='error', finished_at=datetime('now'), error=? WHERE id=?",
                            params![e.to_string(), id],
                        );
                        audit_c
                            .write(crate::bases::audit::EventInput {
                                kind: "bg_job.error",
                                actor: "system",
                                pane_id: None,
                                card_id: None,
                                correlation_id: Some(&id),
                                payload: serde_json::json!({"kind": &kind, "error": e.to_string()}),
                            })
                            .ok();
                    }
                }
            });
        }
    }
}

async fn run_job(
    db: Arc<Mutex<Connection>>,
    id: &str,
    kind: &str,
    args_json: &str,
) -> Result<String> {
    {
        let conn = db.lock();
        conn.execute(
            "UPDATE bg_jobs SET status='running', started_at=datetime('now') WHERE id=? AND status='pending'",
            params![id],
        )?;
    }
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match kind {
        "standup" => {
            // BLOQUE J (ULTRA REVIEW): the bg_queue path for standup is intentionally
            // inert — `commands::standup_today` is the real handler and is invoked
            // directly from the Standup modal. Returning a clear marker here (instead
            // of the previous "placeholder" string) makes the audit trail honest and
            // prevents callers from believing a real digest was generated.
            Err(anyhow!("standup is invoked synchronously from the UI; not eligible for bg_queue. Use commands::standup_today."))
        }
        "embeddings_index" => {
            let project = args
                .get("project_path")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let n = crate::services::embeddings::index_project(
                db.clone(),
                std::path::Path::new(project),
            )
            .await
            .map_err(|e| anyhow!("embeddings_index: {}", e))?;
            Ok(format!("indexed {} chunks", n))
        }
        "codebase_index" => {
            // spec-011 FR-004 — run the signed plugin's indexer in the background. We
            // re-load + re-verify the manifest (fail-closed) and run its declared
            // index_command (placeholder-expanded), NOT an arbitrary command from args.
            let plugin = args
                .get("plugin")
                .and_then(|x| x.as_str())
                .unwrap_or("codebase-memory");
            let project_root = args
                .get("project_root")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let project_key = args
                .get("project_key")
                .and_then(|x| x.as_str())
                .unwrap_or(project_root);
            crate::services::codebase_index::run_index(&db, plugin, project_root, project_key)
                .await
        }
        _ => Err(anyhow!("kind handler not implemented: {}", kind)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> Arc<Mutex<Connection>> {
        let mut conn = Connection::open_in_memory().unwrap();
        rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(include_str!("../../migrations/001_init.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/002_settings.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/003_layout_default.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/004_sprint_tables.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/005_cards_context.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/006_grafana_default.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/007_provider_latency.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/008_bg_jobs.sql")),
        ])
        .to_latest(&mut conn)
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn rejects_unknown_kind() {
        let db = fresh_db();
        assert!(enqueue(&db, "rm_rf", serde_json::json!({})).is_err());
    }

    #[test]
    fn enqueue_and_list() {
        let db = fresh_db();
        let id = enqueue(&db, "standup", serde_json::json!({})).unwrap();
        let jobs = list(&db, 10).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, id);
        assert_eq!(jobs[0].status, "pending");
    }

    #[test]
    fn cancel_pending() {
        let db = fresh_db();
        let id = enqueue(&db, "standup", serde_json::json!({})).unwrap();
        cancel(&db, &id).unwrap();
        let jobs = list(&db, 10).unwrap();
        assert_eq!(jobs[0].status, "cancelled");
    }
}
