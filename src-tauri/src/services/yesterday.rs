// 2.11 / W5 — "Yesterday-el autor" bootstrap.
// Construye un context bootstrap especial para un Claude pane que sólo lee
// el último 7d del audit + mnemo recall + cards activas + MEMORY.md head.
//
// Council V5: depende de 2.3 (agent_memory.rs).

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

pub async fn compile(db: Arc<Mutex<Connection>>, pane_id: &str) -> Result<PathBuf> {
    if pane_id.is_empty()
        || pane_id.contains("..")
        || !pane_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(anyhow!("invalid pane_id: {}", pane_id));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let dir = home.join(".furx").join("contexts");
    std::fs::create_dir_all(&dir)?;
    let mut md = String::new();
    md.push_str("# Yesterday-el autor bootstrap\n\n");
    md.push_str("Tu rol: chat read-only que sólo conoce los últimos 7 días de auditoría, ");
    md.push_str(
        "MEMORY.md, mnemo recall, y cards activas. NO escribís código. NO modificás archivos. ",
    );
    md.push_str(
        "Sólo respondés preguntas sobre qué decidió el autor y qué decisiones quedan pendientes.\n\n",
    );

    // MEMORY.md head
    let memory = home.join(".claude/projects/-Users-hernan/memory/MEMORY.md");
    if let Ok(content) = std::fs::read_to_string(&memory) {
        let head: String = content.chars().take(6000).collect();
        md.push_str("## MEMORY.md (head 6KB)\n\n");
        md.push_str(&head);
        md.push_str("\n\n");
    }

    // Audit last 7 days (top kinds + recent 30 events). Collect to Vec first
    // so we can drop the Mutex guard before the await below.
    let (kinds, events, cards) = {
        let conn = db.lock();
        let kinds: Vec<(String, i64)> = conn
            .prepare(
                "SELECT kind, COUNT(*) FROM events WHERE at >= datetime('now', '-7 days') \
             GROUP BY kind ORDER BY COUNT(*) DESC LIMIT 30",
            )
            .ok()
            .and_then(|mut s| {
                let it = s
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .ok()?;
                Some(it.filter_map(|x| x.ok()).collect())
            })
            .unwrap_or_default();
        let events: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT at, kind, actor FROM events WHERE at >= datetime('now', '-7 days') \
             ORDER BY at DESC LIMIT 30",
            )
            .ok()
            .and_then(|mut s| {
                let it = s
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })
                    .ok()?;
                Some(it.filter_map(|x| x.ok()).collect())
            })
            .unwrap_or_default();
        let cards: Vec<(String, String, String, String)> = conn
            .prepare(
                "SELECT created_at, project, severity, title FROM cards WHERE status='open' \
             ORDER BY created_at DESC LIMIT 20",
            )
            .ok()
            .and_then(|mut s| {
                let it = s
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })
                    .ok()?;
                Some(it.filter_map(|x| x.ok()).collect())
            })
            .unwrap_or_default();
        (kinds, events, cards)
    };
    if !kinds.is_empty() {
        md.push_str("## Audit kinds (last 7 days)\n\n");
        for (k, c) in kinds {
            md.push_str(&format!("- {} · {}\n", k, c));
        }
        md.push('\n');
    }
    if !events.is_empty() {
        md.push_str("## Recent events (30)\n\n");
        for (t, k, a) in events {
            md.push_str(&format!("- {} `{}` ({})\n", t, k, a));
        }
        md.push('\n');
    }
    if !cards.is_empty() {
        md.push_str("## Active cards\n\n");
        for (t, p, sv, ttl) in cards {
            md.push_str(&format!("- [{}/{}] {} — {}\n", p, sv, ttl, t));
        }
        md.push('\n');
    }

    // Mnemo recall (best-effort, fire-and-forget if unavailable)
    if let Ok(mem) = crate::services::agent_memory::recall("furx").await {
        if !mem.recalled.is_empty() {
            md.push_str(&format!(
                "## Mnemo recall (furx · source: {})\n\n",
                mem.source
            ));
            md.push_str(&mem.recalled);
            md.push_str("\n\n");
        }
    }

    let path = dir.join(format!("{}-yesterday-bootstrap.md", pane_id));
    std::fs::write(&path, &md)?;
    Ok(path)
}
