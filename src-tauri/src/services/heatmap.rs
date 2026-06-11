// F17 — Heatmap actividad: events table grouped by (day, hour).
// Returns last 30 days × 24 hours grid + max cell count for color scale.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct HeatmapCell {
    pub day: String, // YYYY-MM-DD (UTC)
    pub hour: u8,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeatmapData {
    pub cells: Vec<HeatmapCell>,
    pub max_count: u32,
    pub total: u32,
    pub days: u32,
}

pub fn compute(db: &Mutex<Connection>, days: u32) -> Result<HeatmapData> {
    let days = days.clamp(1, 90);
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT substr(at,1,10) AS day, \
                CAST(substr(at,12,2) AS INTEGER) AS hour, \
                COUNT(*) AS cnt \
         FROM events \
         WHERE at >= datetime('now', ?) \
         GROUP BY day, hour \
         ORDER BY day, hour",
    )?;
    let arg = format!("-{} days", days);
    let rows = stmt
        .query_map([arg.as_str()], |r| {
            let day: String = r.get(0)?;
            let hour: i64 = r.get(1)?;
            let cnt: i64 = r.get(2)?;
            Ok(HeatmapCell {
                day,
                hour: hour as u8,
                count: cnt as u32,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let max = rows.iter().map(|c| c.count).max().unwrap_or(0);
    let total = rows.iter().map(|c| c.count).sum();
    Ok(HeatmapData {
        cells: rows,
        max_count: max,
        total,
        days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::sync::Arc;

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
    fn empty_db_returns_zeros() {
        let db = fresh_db();
        let d = compute(&db, 30).unwrap();
        assert_eq!(d.total, 0);
        assert_eq!(d.max_count, 0);
    }

    #[test]
    fn counts_recent_events() {
        let db = fresh_db();
        {
            let conn = db.lock();
            for i in 0..5 {
                conn.execute(
                    "INSERT INTO events (id, kind, actor) VALUES (?, ?, ?)",
                    params![format!("e{}", i), "test", "system"],
                )
                .unwrap();
            }
        }
        let d = compute(&db, 30).unwrap();
        assert_eq!(d.total, 5);
        assert!(d.max_count > 0);
    }
}
