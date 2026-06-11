// C / F8 — Filesystem watcher over ~/.furx/worktrees/. On any change (5s debounce)
// auto-creates a "Merge to main?" card and emits furx:merge-suggest.
//
// Cleanup: stores the watcher + shutdown_tx in AppState. On window close,
// AppState::drop signals shutdown and the watcher unwatches all paths.

use anyhow::Result;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;
use uuid::Uuid;

pub struct MergeWatcher {
    shutdown_tx: Option<oneshot::Sender<()>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl MergeWatcher {
    pub fn start(
        app: AppHandle,
        db: Arc<parking_lot::Mutex<rusqlite::Connection>>,
        audit: crate::bases::audit::AuditWriter,
    ) -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
        let root = home.join(".furx").join("worktrees");
        std::fs::create_dir_all(&root)?;

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        // Codex MED: bounded channel + drop-oldest on overflow so worktree churn can't
        // grow memory unbounded while DB is slower than producers.
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<PathBuf>(128);

        // notify-debouncer-mini runs its own thread; we forward events via channel.
        let debouncer_tx = event_tx.clone();
        let mut debouncer =
            new_debouncer(Duration::from_secs(5), move |res: DebounceEventResult| {
                if let Ok(events) = res {
                    for ev in events {
                        // try_send drops the event if the channel is full; that's fine —
                        // the next FS event in the same worktree will re-trigger a card.
                        let _ = debouncer_tx.try_send(ev.path);
                    }
                }
            })?;
        debouncer
            .watcher()
            .watch(&root, notify::RecursiveMode::Recursive)?;

        let root_clone = root.clone();
        tauri::async_runtime::spawn(async move {
            // Keep the debouncer alive in this task; it will be dropped (unwatch all)
            // when this task exits.
            let _debouncer = debouncer;
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        tracing::info!("merge_watcher shutdown signal received");
                        break;
                    }
                    Some(path) = event_rx.recv() => {
                        if let Err(e) = handle_change(&app, &db, &audit, &root_clone, &path).await {
                            tracing::warn!("merge_watcher handle error: {}", e);
                        }
                    }
                }
            }
        });

        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
            path: root,
        })
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for MergeWatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn handle_change(
    app: &AppHandle,
    db: &Arc<parking_lot::Mutex<rusqlite::Connection>>,
    audit: &crate::bases::audit::AuditWriter,
    root: &std::path::Path,
    changed: &std::path::Path,
) -> Result<()> {
    // Compute the "worktree name" = first segment under root.
    let Ok(rel) = changed.strip_prefix(root) else {
        return Ok(());
    };
    let Some(name) = rel.components().next().and_then(|c| c.as_os_str().to_str()) else {
        return Ok(());
    };

    // Codex MED-1: el dedup matcheaba `title LIKE 'Worktree changed: {name}%'`, con DOS bugs:
    //   (a) no escapaba los wildcards SQL (`%`/`_`) del `name`, y el `%` final hacía match por
    //       PREFIJO → `foo` colisionaba con `foobar`, y un `name` con `_` actuaba como comodín.
    //   (b) deduppeaba contra cards recientes AUNQUE estuvieran cerradas → una card cerrada hace
    //       <10min suprimía la creación de un incidente NUEVO (se perdía la card).
    // Fix: título EXACTO (sin sufijo) con el name escapado y `LIKE ... ESCAPE '\'`, y el dedup
    // sólo considera cards ABIERTAS (open) — las cerradas no bloquean un incidente nuevo.
    let title = format!("Worktree changed: {}", name);
    let title_pattern = like_escape(&title);

    // Dedup: ¿hay una card ABIERTA reciente para este worktree (mismo título exacto)?
    {
        let conn = db.lock();
        let recent_open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cards \
             WHERE source = 'merge' \
               AND status = 'open' \
               AND title LIKE ? ESCAPE '\\' \
               AND created_at > datetime('now', '-10 minutes')",
                params![title_pattern],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if recent_open > 0 {
            // spec-022 P1 · US6 — auto-unsnooze ante NUEVA ACTIVIDAD: en vez de crear otra card por el
            // mismo worktree (deduplicado), refrescamos la actividad de la card abierta existente. Si
            // estaba snoozeada y sin expirar, se reabre (reopened=1) y reaparece en el inbox.
            let mut reopened = false;
            conn.execute(
                "UPDATE cards SET last_activity_at = datetime('now'), \
                 snooze_until = CASE WHEN snooze_until IS NOT NULL AND snooze_until > datetime('now') THEN NULL ELSE snooze_until END, \
                 reopened = CASE WHEN snooze_until IS NOT NULL AND snooze_until > datetime('now') THEN 1 ELSE reopened END \
                 WHERE source = 'merge' AND status = 'open' AND title LIKE ? ESCAPE '\\' \
                   AND created_at > datetime('now', '-10 minutes')",
                params![title_pattern],
            )
            .ok();
            // ¿alguna quedó reabierta? (informativo para el audit; barato).
            if let Ok(c) = conn.query_row(
                "SELECT COUNT(*) FROM cards WHERE source = 'merge' AND reopened = 1 AND status = 'open' AND title LIKE ? ESCAPE '\\'",
                params![title_pattern],
                |r| r.get::<_, i64>(0),
            ) {
                reopened = c > 0;
            }
            drop(conn);
            if reopened {
                audit
                    .write(crate::bases::audit::EventInput {
                        kind: "card.reopened",
                        actor: "watcher:merge",
                        pane_id: None,
                        card_id: None,
                        correlation_id: None,
                        payload: serde_json::json!({"worktree": name, "reason": "new_activity"}),
                    })
                    .ok();
            }
            return Ok(());
        }
    }

    let id = Uuid::new_v4().to_string();
    let cause = format!("Filesystem activity in {}", root.join(name).display());
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO cards (id, project, source, title, cause, severity, confidence) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            // Codex LOW: PLAN_CLOSE says severity=warning for merge_watcher cards.
            params![id, "furx", "merge", title, cause, "warning", 0.7_f64],
        )?;
    }
    audit
        .write(crate::bases::audit::EventInput {
            kind: "merge.suggest",
            actor: "watcher:merge",
            pane_id: None,
            card_id: Some(&id),
            correlation_id: None,
            payload: serde_json::json!({"worktree": name}),
        })
        .ok();
    // F3: surface on connected phones (filtered by the `card` toggle).
    crate::services::mobile_bridge::publish_notification(
        "card",
        &title,
        &cause,
        "warning",
        Some(id.clone()),
    );
    let _ = app.emit(
        "furx:merge-suggest",
        serde_json::json!({"card_id": id, "worktree": name}),
    );
    Ok(())
}

/// Codex MED-1: escapa los metacaracteres de `LIKE` (`\`, `%`, `_`) para que el patrón matchee
/// el `value` de forma LITERAL bajo `LIKE ? ESCAPE '\'`. Sin esto, un worktree name con `%`/`_`
/// actúa como comodín. El backslash se escapa primero para no romper los escapes siguientes.
fn like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::like_escape;
    use rusqlite::{params, Connection};

    /// Setup mínimo: tabla cards con las columnas que toca el dedup (open/closed/title/created_at).
    fn mk_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE cards (
                 id TEXT PRIMARY KEY,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 source TEXT NOT NULL,
                 title TEXT NOT NULL,
                 status TEXT NOT NULL DEFAULT 'open',
                 snooze_until TEXT,
                 last_activity_at TEXT,
                 reopened INTEGER NOT NULL DEFAULT 0
             );",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, id: &str, name: &str, status: &str) {
        conn.execute(
            "INSERT INTO cards (id, source, title, status, created_at) \
             VALUES (?, 'merge', ?, ?, datetime('now'))",
            params![id, format!("Worktree changed: {}", name), status],
        )
        .unwrap();
    }

    /// Replica exacta de la query de dedup del watcher (sólo cards ABIERTAS, título EXACTO escapado).
    fn dedup_count(conn: &Connection, name: &str) -> i64 {
        let title = format!("Worktree changed: {}", name);
        let pattern = like_escape(&title);
        conn.query_row(
            "SELECT COUNT(*) FROM cards \
             WHERE source = 'merge' AND status = 'open' \
               AND title LIKE ? ESCAPE '\\' \
               AND created_at > datetime('now', '-10 minutes')",
            params![pattern],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn like_escape_neutralizes_wildcards() {
        assert_eq!(like_escape("foo"), "foo");
        assert_eq!(like_escape("a_b"), "a\\_b");
        assert_eq!(like_escape("a%b"), "a\\%b");
        assert_eq!(like_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn prefix_does_not_collide() {
        // `foo` NO debe matchear la card de `foobar` (antes el `%` final lo hacía colisionar).
        let conn = mk_db();
        insert(&conn, "foobar", "foobar", "open");
        assert_eq!(dedup_count(&conn, "foo"), 0, "foo no colisiona con foobar");
        assert_eq!(dedup_count(&conn, "foobar"), 1, "foobar matchea su propia card");
    }

    #[test]
    fn underscore_is_literal_not_wildcard() {
        // `a_c` (con `_` literal) NO debe matchear `abc` (donde `_` sería comodín de 1 char).
        let conn = mk_db();
        insert(&conn, "abc", "abc", "open");
        assert_eq!(dedup_count(&conn, "a_c"), 0, "_ es literal, no comodín");
    }

    #[test]
    fn closed_card_does_not_suppress_new() {
        // Una card CERRADA reciente del mismo worktree NO debe deduppear (dedup_count=0 → se crea
        // un incidente nuevo).
        let conn = mk_db();
        insert(&conn, "c1", "wt", "closed");
        assert_eq!(dedup_count(&conn, "wt"), 0, "card cerrada no suprime una nueva");
    }

    #[test]
    fn open_card_deduplicates() {
        // Una card ABIERTA reciente del mismo worktree SÍ deduppea (dedup_count>0 → se refresca,
        // no se crea otra).
        let conn = mk_db();
        insert(&conn, "o1", "wt", "open");
        assert!(dedup_count(&conn, "wt") > 0, "card abierta deduppea");
    }
}
