// 2.23 — Floating quick-note overlay.
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct QuickNote {
    pub id: String,
    pub body: String,
    pub created_at: String,
}

pub fn add(db: &Mutex<Connection>, body: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    db.lock().execute(
        "INSERT INTO quick_notes (id, body) VALUES (?, ?)",
        params![id, body],
    )?;
    Ok(id)
}
pub fn list(db: &Mutex<Connection>) -> Result<Vec<QuickNote>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, body, created_at FROM quick_notes ORDER BY created_at DESC LIMIT 200",
    )?;
    let rows: Vec<QuickNote> = stmt
        .query_map([], |r| {
            Ok(QuickNote {
                id: r.get(0)?,
                body: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}
pub fn delete(db: &Mutex<Connection>, id: &str) -> Result<()> {
    db.lock()
        .execute("DELETE FROM quick_notes WHERE id = ?", params![id])?;
    Ok(())
}
