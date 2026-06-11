// 1.8 — Latency heatmap LLM providers.
// Background poll cada 60s, escribe a tabla provider_latency_history.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct LatencyCell {
    pub provider: String,
    pub hour: i64,   // 0-23 UTC
    pub day: String, // YYYY-MM-DD
    pub avg_rtt_ms: f64,
    pub blocked_ratio: f64,
    pub samples: u32,
}

pub async fn poll_and_record(
    db: Arc<Mutex<Connection>>,
    endpoint: &str,
    bearer: &str,
) -> Result<usize> {
    let started = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!("{}/v1/resilience/state", endpoint.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .bearer_auth(bearer)
        .header("Accept", "application/json")
        .send()
        .await?;
    let rtt = started.elapsed().as_millis() as i64;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("AIE status {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await?;
    let rate_limits = v
        .get("rate_limits")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let conn = db.lock();
    let mut count = 0usize;
    for rl in &rate_limits {
        let provider = rl
            .get("provider")
            .and_then(|s| s.as_str())
            .unwrap_or("?")
            .to_string();
        let blocked = rl.get("blocked_until").and_then(|x| x.as_str()).is_some();
        conn.execute(
            "INSERT INTO provider_latency_history (provider, blocked, rtt_ms, note) VALUES (?, ?, ?, NULL)",
            params![provider, blocked as i64, rtt],
        ).ok();
        count += 1;
    }
    // Cap table size to last 30 days.
    conn.execute(
        "DELETE FROM provider_latency_history WHERE at < datetime('now', '-30 days')",
        [],
    )
    .ok();
    Ok(count)
}

pub fn query_heatmap(db: &Mutex<Connection>, days: u32) -> Result<Vec<LatencyCell>> {
    let days = days.clamp(1, 30);
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT provider, \
                substr(at,1,10) AS day, \
                CAST(substr(at,12,2) AS INTEGER) AS hour, \
                AVG(rtt_ms) as avg_rtt, \
                AVG(CAST(blocked AS REAL)) as blocked_ratio, \
                COUNT(*) as samples \
         FROM provider_latency_history \
         WHERE at >= datetime('now', ?) \
         GROUP BY provider, day, hour \
         ORDER BY day, hour",
    )?;
    let arg = format!("-{} days", days);
    let rows = stmt
        .query_map([arg.as_str()], |r| {
            Ok(LatencyCell {
                provider: r.get(0)?,
                day: r.get(1)?,
                hour: r.get::<_, i64>(2)?,
                avg_rtt_ms: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                blocked_ratio: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                samples: r.get::<_, i64>(5)? as u32,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(rows)
}
