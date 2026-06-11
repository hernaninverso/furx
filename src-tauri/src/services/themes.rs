// 2.36 — Theme/color per project.
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectTheme {
    pub project: String,
    pub accent_hex: String,
    pub label: Option<String>,
}

pub fn set(
    db: &Mutex<Connection>,
    project: &str,
    accent_hex: &str,
    label: Option<&str>,
) -> Result<()> {
    // Validate hex
    let h = accent_hex.trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(anyhow::anyhow!("invalid hex: {}", accent_hex));
    }
    let conn = db.lock();
    conn.execute(
        "INSERT INTO project_themes (project, accent_hex, label) VALUES (?, ?, ?) \
         ON CONFLICT(project) DO UPDATE SET accent_hex=excluded.accent_hex, label=excluded.label, updated_at=datetime('now')",
        params![project, accent_hex, label],
    )?;
    Ok(())
}
pub fn list(db: &Mutex<Connection>) -> Result<Vec<ProjectTheme>> {
    let conn = db.lock();
    let mut stmt =
        conn.prepare("SELECT project, accent_hex, label FROM project_themes ORDER BY project")?;
    let rows: Vec<ProjectTheme> = stmt
        .query_map([], |r| {
            Ok(ProjectTheme {
                project: r.get(0)?,
                accent_hex: r.get(1)?,
                label: r.get::<_, Option<String>>(2)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}
