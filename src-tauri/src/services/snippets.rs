// 2.21 — Snippet library.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    pub source: String,
    pub created_at: String,
}

pub fn save(db: &Mutex<Connection>, title: &str, body: &str, tags: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO snippets (id, title, body, tags, source) VALUES (?, ?, ?, ?, 'manual')",
        params![id, title, body, tags],
    )?;
    Ok(id)
}

pub fn list(db: &Mutex<Connection>, q: Option<&str>) -> Result<Vec<Snippet>> {
    let conn = db.lock();
    let (sql, like): (&str, String) = match q {
        Some(q) if !q.is_empty() => (
            "SELECT id,title,body,tags,source,created_at FROM snippets WHERE title LIKE ? OR body LIKE ? OR tags LIKE ? ORDER BY created_at DESC LIMIT 100",
            format!("%{}%", q),
        ),
        _ => ("SELECT id,title,body,tags,source,created_at FROM snippets ORDER BY created_at DESC LIMIT 100", String::new()),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows: Vec<Snippet> = if like.is_empty() {
        stmt.query_map([], |r| {
            Ok(Snippet {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                tags: r.get(3)?,
                source: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect()
    } else {
        stmt.query_map([&like, &like, &like], |r| {
            Ok(Snippet {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                tags: r.get(3)?,
                source: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect()
    };
    Ok(rows)
}

pub fn delete(db: &Mutex<Connection>, id: &str) -> Result<()> {
    db.lock()
        .execute("DELETE FROM snippets WHERE id = ?", params![id])?;
    Ok(())
}
