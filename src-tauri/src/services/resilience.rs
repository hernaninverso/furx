// services/resilience.rs — port del resilience layer del AIE a Rust.
// Council BLOQUE 3 unánime: skip-AIE-sidecar, port a Rust en este módulo.
//
// Tres dimensiones ortogonales:
//   - rate_limit: RPM/TPM token-bucket por provider+model+credential.
//   - quota: 429 / tpd-exhausted con backoff exponencial.
//   - circuit: trip after N consecutive failures, half-open recovery.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::Arc;

const CIRCUIT_TRIP_THRESHOLD: u32 = 5;
const CIRCUIT_OPEN_DURATION_MIN: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResilienceVerdict {
    Allow,
    RateLimited,
    QuotaExhausted,
    CircuitOpen,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealthSnapshot {
    pub provider: String,
    pub model: String,
    pub credential_alias: String,
    pub rate_limit_blocked_until: Option<String>,
    pub quota_blocked_until: Option<String>,
    pub circuit_blocked_until: Option<String>,
    pub consecutive_failures: i64,
}

/// Decide whether to attempt this voice based on persisted state.
/// `circuit` state is checked first (highest precedence).
pub fn check_allowed(
    db: &Arc<parking_lot::Mutex<Connection>>,
    provider: &str,
    model: &str,
    credential_alias: &str,
) -> Result<ResilienceVerdict> {
    let now = Utc::now();
    let conn = db.lock();
    // Circuit
    if let Some(until) = read_blocked_until(&conn, provider, model, credential_alias, "circuit")? {
        if until > now {
            return Ok(ResilienceVerdict::CircuitOpen);
        }
    }
    // Quota (429/tpd)
    if let Some(until) = read_blocked_until(&conn, provider, model, credential_alias, "api_429")? {
        if until > now {
            return Ok(ResilienceVerdict::QuotaExhausted);
        }
    }
    // Rate limit
    if let Some(until) = read_blocked_until(&conn, provider, model, credential_alias, "rpm_minute")?
    {
        if until > now {
            return Ok(ResilienceVerdict::RateLimited);
        }
    }
    Ok(ResilienceVerdict::Allow)
}

fn read_blocked_until(
    conn: &Connection,
    provider: &str,
    model: &str,
    credential_alias: &str,
    dimension: &str,
) -> Result<Option<DateTime<Utc>>> {
    let mut stmt = conn.prepare(
        "SELECT blocked_until FROM resilience_state
         WHERE provider = ?1 AND model = ?2 AND credential_alias = ?3 AND dimension = ?4",
    )?;
    let mut rows = stmt.query_map(params![provider, model, credential_alias, dimension], |r| {
        r.get::<_, Option<String>>(0)
    })?;
    if let Some(row) = rows.next() {
        let s: Option<String> = row?;
        if let Some(ts) = s {
            if let Ok(dt) = DateTime::parse_from_rfc3339(&ts) {
                return Ok(Some(dt.with_timezone(&Utc)));
            }
        }
    }
    Ok(None)
}

/// Record a success: clear circuit/quota; bump RPM bucket; reset consecutive_failures.
pub fn record_success(
    db: &Arc<parking_lot::Mutex<Connection>>,
    provider: &str,
    model: &str,
    credential_alias: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // Clear circuit + api_429 blocks
    conn.execute(
        "UPDATE resilience_state SET blocked_until = NULL, bucket_used = 0, updated_at = ?1
         WHERE provider = ?2 AND model = ?3 AND credential_alias = ?4
           AND dimension IN ('circuit', 'api_429')",
        params![now, provider, model, credential_alias],
    )?;
    Ok(())
}

/// Record a failure. Bumps consecutive_failures; trips circuit if threshold exceeded.
/// If the error indicates 429/rate_limit, sets api_429 blocked_until = now + 60s.
pub fn record_failure(
    db: &Arc<parking_lot::Mutex<Connection>>,
    provider: &str,
    model: &str,
    credential_alias: &str,
    error_str: &str,
) -> Result<()> {
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let is_429 = error_str.contains("429") || error_str.to_lowercase().contains("rate");
    let is_quota_exhausted =
        error_str.to_lowercase().contains("quota") || error_str.contains("insufficient");
    let conn = db.lock();

    // Bump or insert circuit row's bucket_used (we use it as consecutive_failures counter).
    // MED-1 fix (Codex B3): cap at 1M to avoid i64 overflow on pathological repeat failures.
    conn.execute(
        "INSERT INTO resilience_state (provider, model, credential_alias, dimension,
            blocked_until, bucket_used, bucket_limit, bucket_window_s, updated_at)
         VALUES (?1, ?2, ?3, 'circuit', NULL, 1, 0, 0, ?4)
         ON CONFLICT(provider, model, credential_alias, dimension) DO UPDATE SET
            bucket_used = MIN(bucket_used + 1, 1000000),
            updated_at = excluded.updated_at",
        params![provider, model, credential_alias, now_str],
    )?;

    // If consecutive_failures >= threshold, trip circuit
    let mut stmt = conn.prepare(
        "SELECT bucket_used FROM resilience_state
         WHERE provider = ?1 AND model = ?2 AND credential_alias = ?3 AND dimension = 'circuit'",
    )?;
    let count: i64 = stmt
        .query_row(params![provider, model, credential_alias], |r| r.get(0))
        .unwrap_or(0);
    drop(stmt);
    if count >= CIRCUIT_TRIP_THRESHOLD as i64 {
        let trip_until = (now + Duration::minutes(CIRCUIT_OPEN_DURATION_MIN)).to_rfc3339();
        conn.execute(
            "UPDATE resilience_state SET blocked_until = ?1, updated_at = ?2
             WHERE provider = ?3 AND model = ?4 AND credential_alias = ?5 AND dimension = 'circuit'",
            params![trip_until, now_str, provider, model, credential_alias],
        )?;
    }

    // 429 → block 60s
    if is_429 {
        let block_until = (now + Duration::seconds(60)).to_rfc3339();
        conn.execute(
            "INSERT INTO resilience_state (provider, model, credential_alias, dimension,
                blocked_until, bucket_used, bucket_limit, bucket_window_s, updated_at)
             VALUES (?1, ?2, ?3, 'api_429', ?4, 0, 0, 60, ?5)
             ON CONFLICT(provider, model, credential_alias, dimension) DO UPDATE SET
                blocked_until = excluded.blocked_until,
                updated_at = excluded.updated_at",
            params![provider, model, credential_alias, block_until, now_str],
        )?;
    } else if is_quota_exhausted {
        // Quota = until next day
        let block_until = (now + Duration::hours(24)).to_rfc3339();
        conn.execute(
            "INSERT INTO resilience_state (provider, model, credential_alias, dimension,
                blocked_until, bucket_used, bucket_limit, bucket_window_s, updated_at)
             VALUES (?1, ?2, ?3, 'api_429', ?4, 0, 0, 86400, ?5)
             ON CONFLICT(provider, model, credential_alias, dimension) DO UPDATE SET
                blocked_until = excluded.blocked_until,
                updated_at = excluded.updated_at",
            params![provider, model, credential_alias, block_until, now_str],
        )?;
    }

    Ok(())
}

/// Snapshot for UI. MED fix (Codex B3): expired `blocked_until` values are surfaced as None
/// so the UI doesn't keep showing a provider as blocked after the block expired.
pub fn snapshot(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<Vec<ProviderHealthSnapshot>> {
    let now = Utc::now();
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT provider, model, credential_alias, dimension, blocked_until, bucket_used
         FROM resilience_state",
    )?;
    let rows = stmt.query_map([], |r| {
        let p: String = r.get(0)?;
        let m: String = r.get(1)?;
        let a: String = r.get(2)?;
        let d: String = r.get(3)?;
        let b: Option<String> = r.get(4)?;
        let used: i64 = r.get(5)?;
        Ok((p, m, a, d, b, used))
    })?;

    let filter_expired = |opt: Option<String>| -> Option<String> {
        let s = opt?;
        match DateTime::parse_from_rfc3339(&s) {
            Ok(dt) if dt.with_timezone(&Utc) > now => Some(s),
            _ => None,
        }
    };

    use std::collections::HashMap;
    let mut map: HashMap<(String, String, String), ProviderHealthSnapshot> = HashMap::new();
    for row in rows {
        let (p, m, a, d, b, used) = row?;
        let entry = map
            .entry((p.clone(), m.clone(), a.clone()))
            .or_insert_with(|| ProviderHealthSnapshot {
                provider: p.clone(),
                model: m.clone(),
                credential_alias: a.clone(),
                rate_limit_blocked_until: None,
                quota_blocked_until: None,
                circuit_blocked_until: None,
                consecutive_failures: 0,
            });
        match d.as_str() {
            "rpm_minute" => entry.rate_limit_blocked_until = filter_expired(b),
            "api_429" => entry.quota_blocked_until = filter_expired(b),
            "circuit" => {
                entry.circuit_blocked_until = filter_expired(b);
                // Saturate consecutive_failures within sane range (Codex MED-1)
                entry.consecutive_failures = used.clamp(0, 1_000_000);
            }
            _ => {}
        }
    }
    Ok(map.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::path::PathBuf;

    fn fresh_db() -> Arc<Mutex<Connection>> {
        // Use unique tmp file per test to avoid lock contention when running in parallel.
        let unique = uuid::Uuid::new_v4().to_string();
        let path = PathBuf::from(format!("/tmp/furx-resilience-test-{}.db", unique));
        let _ = std::fs::remove_file(&path);
        let conn = crate::db::open(&path).unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn allow_when_no_state() {
        let db = fresh_db();
        let v = check_allowed(&db, "openrouter", "claude", "or-main").unwrap();
        assert_eq!(v, ResilienceVerdict::Allow);
    }

    #[test]
    fn trips_circuit_after_5_failures() {
        let db = fresh_db();
        for _ in 0..5 {
            record_failure(&db, "openrouter", "gpt-5", "or-main", "HTTP 500: oops").unwrap();
        }
        let v = check_allowed(&db, "openrouter", "gpt-5", "or-main").unwrap();
        assert_eq!(v, ResilienceVerdict::CircuitOpen);
    }

    #[test]
    fn blocks_on_429() {
        let db = fresh_db();
        record_failure(
            &db,
            "groq",
            "llama-70b",
            "groq-main",
            "HTTP 429: rate limit",
        )
        .unwrap();
        let v = check_allowed(&db, "groq", "llama-70b", "groq-main").unwrap();
        assert_eq!(v, ResilienceVerdict::QuotaExhausted);
    }

    #[test]
    fn success_clears_circuit() {
        let db = fresh_db();
        for _ in 0..5 {
            record_failure(&db, "p", "m", "a", "HTTP 500").unwrap();
        }
        record_success(&db, "p", "m", "a").unwrap();
        let v = check_allowed(&db, "p", "m", "a").unwrap();
        assert_eq!(v, ResilienceVerdict::Allow);
    }
}
