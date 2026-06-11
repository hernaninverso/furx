// 2.31 — Time-tracking auto-detectado por pane activity (events GROUP BY pane).

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PaneTime {
    pub pane_id: String,
    pub events: i64,
    pub active_minutes: i64,
}

pub fn weekly(db: &Mutex<Connection>) -> Result<Vec<PaneTime>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT pane_id, COUNT(*), \
                (julianday(MAX(at)) - julianday(MIN(at))) * 24 * 60 \
         FROM events \
         WHERE at >= datetime('now', '-7 days') AND pane_id IS NOT NULL \
         GROUP BY pane_id ORDER BY COUNT(*) DESC",
    )?;
    let rows: Vec<PaneTime> = stmt
        .query_map([], |r| {
            Ok(PaneTime {
                pane_id: r.get(0)?,
                events: r.get(1)?,
                active_minutes: r.get::<_, f64>(2)? as i64,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}
