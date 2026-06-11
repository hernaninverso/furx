// F25 — Workspace snapshots. Captura {panes, layout, cards_open, events_cursor}
// en la tabla `snapshots`. Manual via ⌘⇧S, auto cada 100 audit events importantes.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

const CURRENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub at: String,
    pub kind: String,
    pub bytes: usize,
    pub schema_version: i64,
}

#[derive(Debug, Clone, Serialize)]
struct PanePayload {
    id: String,
    mode: String,
    title: Option<String>,
    cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LayoutPayload {
    panes: serde_json::Value,
    grid_cols: String,
    grid_rows: String,
}

#[derive(Debug, Clone, Serialize)]
struct SnapshotPayload {
    schema_version: i64,
    panes: Vec<PanePayload>,
    layout: Option<LayoutPayload>,
    cards_open_count: i64,
    events_cursor: Option<String>,
}

/// Take a snapshot. `kind` must be "manual", "auto", or "startup".
pub fn write(db: Arc<Mutex<Connection>>, kind: &str) -> Result<SnapshotInfo> {
    if !matches!(kind, "manual" | "auto" | "startup") {
        return Err(anyhow::anyhow!("invalid snapshot kind: {}", kind));
    }
    let conn = db.lock();

    // panes
    let mut stmt = conn.prepare("SELECT id, mode, title, cwd FROM panes ORDER BY layout_pos")?;
    let panes: Vec<PanePayload> = stmt
        .query_map([], |r| {
            Ok(PanePayload {
                id: r.get(0)?,
                mode: r.get(1)?,
                title: r.get(2)?,
                cwd: r.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    // layout(default)
    let layout = conn
        .query_row(
            "SELECT panes, grid_cols, grid_rows FROM layouts WHERE id = 'default'",
            [],
            |r| {
                let panes_json: String = r.get(0)?;
                Ok(LayoutPayload {
                    panes: serde_json::from_str(&panes_json).unwrap_or(serde_json::Value::Null),
                    grid_cols: r.get(1)?,
                    grid_rows: r.get(2)?,
                })
            },
        )
        .ok();

    // counts
    let cards_open_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cards WHERE status = 'open'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let events_cursor: Option<String> = conn
        .query_row("SELECT MAX(at) FROM events", [], |r| r.get(0))
        .unwrap_or(None);

    let payload = SnapshotPayload {
        schema_version: CURRENT_SCHEMA_VERSION,
        panes,
        layout,
        cards_open_count,
        events_cursor,
    };
    let payload_str = serde_json::to_string(&payload)?;
    let id = Uuid::new_v4().to_string();
    let at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO snapshots (id, at, kind, payload, schema_version) VALUES (?, ?, ?, ?, ?)",
        params![id, at, kind, payload_str, CURRENT_SCHEMA_VERSION],
    )?;
    Ok(SnapshotInfo {
        id,
        at,
        kind: kind.to_string(),
        bytes: payload_str.len(),
        schema_version: CURRENT_SCHEMA_VERSION,
    })
}

pub fn list(db: &Mutex<Connection>) -> Result<Vec<SnapshotInfo>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, at, kind, length(payload), schema_version FROM snapshots ORDER BY at DESC LIMIT 100",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let bytes: i64 = r.get(3)?;
            Ok(SnapshotInfo {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                bytes: bytes as usize,
                schema_version: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
        ])
        .to_latest(&mut conn)
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn writes_a_snapshot_and_lists_it() {
        let db = fresh_db();
        let info = write(db.clone(), "manual").unwrap();
        assert_eq!(info.kind, "manual");
        assert!(info.bytes > 10);
        let v = list(&db).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, info.id);
    }

    #[test]
    fn rejects_bad_kind() {
        let db = fresh_db();
        assert!(write(db, "explode").is_err());
    }
}
