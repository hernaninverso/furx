// 2.32 — Pane templates (preset prompt+mode+cwd+env).
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneTemplate {
    pub name: String,
    pub mode: String,
    pub cwd: Option<String>,
    pub env_keys: Vec<String>,
    pub initial_prompt: Option<String>,
}

pub fn save(db: &Mutex<Connection>, t: &PaneTemplate) -> Result<()> {
    let env_keys_json = serde_json::to_string(&t.env_keys)?;
    db.lock().execute(
        "INSERT INTO pane_templates (name, mode, cwd, env_keys, initial_prompt) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(name) DO UPDATE SET mode=excluded.mode, cwd=excluded.cwd, env_keys=excluded.env_keys, initial_prompt=excluded.initial_prompt",
        params![t.name, t.mode, t.cwd, env_keys_json, t.initial_prompt],
    )?;
    Ok(())
}
pub fn list(db: &Mutex<Connection>) -> Result<Vec<PaneTemplate>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT name, mode, cwd, env_keys, initial_prompt FROM pane_templates ORDER BY name",
    )?;
    let rows: Vec<PaneTemplate> = stmt
        .query_map([], |r| {
            let env_keys_json: String = r.get(3)?;
            Ok(PaneTemplate {
                name: r.get(0)?,
                mode: r.get(1)?,
                cwd: r.get(2)?,
                env_keys: serde_json::from_str(&env_keys_json).unwrap_or_default(),
                initial_prompt: r.get(4)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}
pub fn delete(db: &Mutex<Connection>, name: &str) -> Result<()> {
    db.lock()
        .execute("DELETE FROM pane_templates WHERE name = ?", params![name])?;
    Ok(())
}
