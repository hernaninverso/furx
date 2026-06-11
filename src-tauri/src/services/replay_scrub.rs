// 2.7 — Session replay scrubber. Backend: provee buckets de audit events
// agrupados por minuto para que el slider del frontend pueda zoom + scrub.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ScrubBucket {
    pub ts: String, // YYYY-MM-DDTHH:MM
    pub count: u32,
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScrubData {
    pub buckets: Vec<ScrubBucket>,
    pub total: u32,
    pub first_at: Option<String>,
    pub last_at: Option<String>,
}

pub fn buckets(db: &Mutex<Connection>, hours: u32) -> Result<ScrubData> {
    let hours = hours.clamp(1, 24 * 30);
    let arg = format!("-{} hours", hours);
    let conn = db.lock();
    let (total, first, last): (i64, Option<String>, Option<String>) = {
        let mut s_total = conn.prepare(
            "SELECT COUNT(*), MIN(at), MAX(at) FROM events WHERE at >= datetime('now', ?)",
        )?;
        s_total.query_row([arg.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
    };
    let buckets: Vec<ScrubBucket> = {
        let mut s_bk = conn.prepare(
            "SELECT substr(at,1,16) AS bucket, COUNT(*), GROUP_CONCAT(DISTINCT kind) \
             FROM events WHERE at >= datetime('now', ?) \
             GROUP BY bucket ORDER BY bucket",
        )?;
        let it = s_bk.query_map([arg.as_str()], |r| {
            let kinds: String = r.get::<_, Option<String>>(2)?.unwrap_or_default();
            Ok(ScrubBucket {
                ts: r.get(0)?,
                count: r.get::<_, i64>(1)? as u32,
                kinds: kinds
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .take(5)
                    .map(String::from)
                    .collect(),
            })
        })?;
        let v: Vec<ScrubBucket> = it.filter_map(|x| x.ok()).collect();
        v
    };
    Ok(ScrubData {
        buckets,
        total: total as u32,
        first_at: first,
        last_at: last,
    })
}

pub fn events_at(db: &Mutex<Connection>, bucket_ts: &str) -> Result<Vec<serde_json::Value>> {
    let conn = db.lock();
    let prefix = format!("{}:%", &bucket_ts[..bucket_ts.len().min(16)]);
    let rows: Vec<serde_json::Value> = {
        let mut stmt = conn.prepare(
            "SELECT id, at, kind, actor, payload FROM events \
             WHERE at LIKE ? ORDER BY at LIMIT 200",
        )?;
        let it = stmt.query_map([prefix.as_str()], |r| Ok(serde_json::json!({
            "id": r.get::<_, String>(0)?,
            "at": r.get::<_, String>(1)?,
            "kind": r.get::<_, String>(2)?,
            "actor": r.get::<_, String>(3)?,
            "payload": serde_json::from_str::<serde_json::Value>(&r.get::<_, String>(4)?).unwrap_or(serde_json::Value::Null),
        })))?;
        let v: Vec<serde_json::Value> = it.filter_map(|x| x.ok()).collect();
        v
    };
    Ok(rows)
}
