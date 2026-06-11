// Settings — KV-store sobre la tabla `settings`. Frontend usa get/set via commands.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

pub fn get(conn: &Connection, key: &str) -> Result<Option<Value>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?")?;
    let mut rows = stmt.query(params![key])?;
    if let Some(row) = rows.next()? {
        let raw: String = row.get(0)?;
        Ok(Some(
            serde_json::from_str(&raw).unwrap_or(Value::String(raw)),
        ))
    } else {
        Ok(None)
    }
}

pub fn set(conn: &Connection, key: &str, value: &Value) -> Result<()> {
    let raw = serde_json::to_string(value)?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now')) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, raw],
    )?;
    Ok(())
}

pub fn all(conn: &Connection) -> Result<Vec<(String, Value)>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let rows = stmt
        .query_map([], |r| {
            let k: String = r.get(0)?;
            let v: String = r.get(1)?;
            Ok((k, serde_json::from_str(&v).unwrap_or(Value::String(v))))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
