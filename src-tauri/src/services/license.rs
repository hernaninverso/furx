// services/license.rs — Pro license check + Trial 14d.
// Council EDGE_4: trial_started_at debe ser server-time, no client-time.
// Council EDGE_6: TTL offline 24h máximo.
// Council EDGE_7: check on-demand al usar Pro features (no polling background).

use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration as StdDuration;
use uuid::Uuid;

use crate::services::keychain;

// HIGH-G3 fix (Gemini): trial timestamp ALSO persisted in macOS Keychain.
// Borrar ~/.furx/furx.db ya NO resetea el trial — la entrada del Keychain sobrevive.
const KEYCHAIN_SVC_LICENSE: &str = "furx-license";
const KEYCHAIN_ACCT_TRIAL_START: &str = "trial-started-at";
const KEYCHAIN_ACCT_INSTALL_ID: &str = "install-id";

const CACHE_TTL_HOURS: i64 = 1;
const OFFLINE_GRACE_HOURS: i64 = 24;
const TRIAL_DAYS: i64 = 14;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LicenseState {
    Valid {
        tier: String,
        until: String, // ISO-8601
    },
    Trial {
        until: String,
    },
    Expired,
    Offline {
        last_check: String,
        cached_state: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicensePayload {
    pub valid: bool,
    pub tier: Option<String>, // "free" | "pro" | "team" | "enterprise"
    pub valid_until: Option<String>, // ISO-8601
    pub server_time: Option<String>, // ISO-8601 - council EDGE_4
}

/// Returns the install_id (UUID v4) for this (Furx) install.
/// HIGH-G3 fix: persisted in BOTH DB settings and Keychain — survives `rm -rf ~/.furx`.
/// Council EDGE_3: UUID v4 sin HMAC es suficiente.
pub fn install_id(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<String> {
    let key = "app.install_id";
    // L7: read under the lock, then DROP it before any keychain (blocking) IO —
    // mirrors trial_started_at(); avoids holding the DB mutex across a Keychain call.
    let existing_db: Option<String> = {
        let conn = db.lock();
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str::<String>(&s).ok())
    };

    if let Some(id) = existing_db {
        if !id.is_empty() {
            return Ok(id);
        }
    }

    // Try Keychain — survives DB wipe (lock NOT held here)
    if let Some(kc_id) = keychain::load(KEYCHAIN_SVC_LICENSE, KEYCHAIN_ACCT_INSTALL_ID) {
        if !kc_id.is_empty() {
            // Restore to DB
            let now = Utc::now().to_rfc3339();
            let value_json = serde_json::to_string(&kc_id)?;
            {
                let conn = db.lock();
                conn.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                    params![key, value_json, now],
                )?;
            }
            return Ok(kc_id);
        }
    }

    let new_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let value_json = serde_json::to_string(&new_id)?;
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![key, value_json, now],
        )?;
    }
    // Best-effort Keychain mirror (continue on failure — DB has the value)
    let _ = keychain::save(KEYCHAIN_SVC_LICENSE, KEYCHAIN_ACCT_INSTALL_ID, &new_id);
    Ok(new_id)
}

/// Returns trial_started_at (ISO-8601) — server time from first successful license check
/// if available, else falls back to local time on first install.
/// Council EDGE_4: anti clock-skew.
/// HIGH-G3 fix (Gemini): the timestamp is mirrored to Keychain so wiping `~/.furx/furx.db`
/// does NOT reset the trial. Order of precedence: DB server → DB local → Keychain → new local.
pub fn trial_started_at(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<DateTime<Utc>> {
    let key = "trial.started_at_server";
    let key_local = "trial.started_at_local";
    let conn = db.lock();
    let server: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str::<String>(&s).ok());

    if let Some(s) = server {
        // L2: a malformed stored server time must NOT propagate — that would fail-close
        // Pro permanently. Use it only if it parses; otherwise fall through to local.
        if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
            return Ok(dt.with_timezone(&Utc));
        }
        tracing::warn!("license: ignoring malformed trial.started_at_server, using local");
    }

    let local: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key_local],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str::<String>(&s).ok());

    if let Some(s) = local {
        return Ok(DateTime::parse_from_rfc3339(&s)?.with_timezone(&Utc));
    }

    // Try Keychain mirror — survives DB wipe
    drop(conn);
    if let Some(kc_ts) = keychain::load(KEYCHAIN_SVC_LICENSE, KEYCHAIN_ACCT_TRIAL_START) {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(&kc_ts) {
            let dt = parsed.with_timezone(&Utc);
            // Restore to DB local key for fast subsequent reads
            let conn = db.lock();
            let value_json = serde_json::to_string(&kc_ts)?;
            let now_str = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
                params![key_local, value_json, now_str],
            )?;
            return Ok(dt);
        }
    }

    // first run — persist local timestamp BOTH in DB and Keychain
    let now = Utc::now();
    let conn = db.lock();
    let value_json = serde_json::to_string(&now.to_rfc3339())?;
    let now_str = now.to_rfc3339();
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key_local, value_json, now_str],
    )?;
    drop(conn);
    let _ = keychain::save(KEYCHAIN_SVC_LICENSE, KEYCHAIN_ACCT_TRIAL_START, &now_str);
    Ok(now)
}

/// Anchor trial timestamp to server time on first successful check.
/// Called from check() once when the server replies with a server_time.
fn anchor_trial_to_server(
    db: &Arc<parking_lot::Mutex<Connection>>,
    server_time: &str,
) -> Result<()> {
    let key = "trial.started_at_server";
    let conn = db.lock();
    let already: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| serde_json::from_str::<String>(&s).ok());

    if already.is_some() {
        return Ok(());
    }

    // L2: never persist a server_time we can't parse — a malformed value would later
    // make trial_started_at() error and permanently fail-close Pro. Skip anchoring,
    // keep the local trial start.
    if DateTime::parse_from_rfc3339(server_time).is_err() {
        tracing::warn!("license: server returned unparseable server_time; keeping local trial start");
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    let value_json = serde_json::to_string(&server_time.to_string())?;
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key, value_json, now],
    )?;
    drop(conn);
    // Mirror to Keychain too — survives DB wipe
    let _ = keychain::save(KEYCHAIN_SVC_LICENSE, KEYCHAIN_ACCT_TRIAL_START, server_time);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    payload: LicensePayload,
    fetched_at: String,
}

fn cache_load(db: &Arc<parking_lot::Mutex<Connection>>) -> Option<CacheEntry> {
    let key = "license.cache";
    let conn = db.lock();
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .ok();
    raw.and_then(|s| serde_json::from_str::<CacheEntry>(&s).ok())
}

fn cache_save(db: &Arc<parking_lot::Mutex<Connection>>, payload: &LicensePayload) -> Result<()> {
    let key = "license.cache";
    let now = Utc::now().to_rfc3339();
    let entry = CacheEntry {
        payload: payload.clone(),
        fetched_at: now.clone(),
    };
    let json = serde_json::to_string(&entry)?;
    let conn = db.lock();
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        params![key, json, now],
    )?;
    Ok(())
}

fn endpoint_allowed(endpoint: &str) -> bool {
    // HIGH-3 fix (Codex): license endpoint MUST be an HTTPS public host.
    // Removed localhost/127.0.0.1 — they let a user spoof a Pro response trivially.
    // HTTPS-only; only the vendor license backend suffix is trusted by default.
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // Suffix match must include the leading dot or exact-eq the canonical hostname.
    // Self-hosters can point at their own license backend by overriding this list
    // in a fork; the default trusts only the vendor's license domain.
    // L1: a leading-dot suffix match already rejects look-alikes like `example.test.evil.com`
    // (it ends with `.evil.com`, not `.example.test`), so the old `!ends_with(".attacker…")`
    // clause was dead logic. Self-hosters override this list in a fork.
    let allowed = [".example.test"];
    allowed.iter().any(|s| host.ends_with(s))
}

/// Check license — first cache hit (if < CACHE_TTL_HOURS), else fetch from the dev server endpoint.
/// Falls back to "Offline" if network fails AND cache is younger than OFFLINE_GRACE_HOURS.
pub async fn check(
    db: &Arc<parking_lot::Mutex<Connection>>,
    endpoint: &str,
) -> Result<LicenseState> {
    let install = install_id(db)?;
    let trial_start = trial_started_at(db)?;
    // MED-11 fix (Codex): trial_until cached lazily; if server anchor happens during this call,
    // we re-read trial_start AFTER the anchor to ensure consistency.
    let now = Utc::now();
    let trial_until = trial_start + Duration::days(TRIAL_DAYS);
    let trial_active = now < trial_until;

    // Fresh cache hit?
    if let Some(entry) = cache_load(db) {
        if let Ok(fetched) = DateTime::parse_from_rfc3339(&entry.fetched_at) {
            // HIGH-4 fix (Codex): clock-skew can extend the TTL window arbitrarily if user
            // adelanta + atrasa el reloj. Reject cache entries whose fetched_at is in the future
            // (impossible legit) or older than 1 day (already past offline grace anyway).
            let age = now.signed_duration_since(fetched.with_timezone(&Utc));
            if age >= Duration::zero()
                && age < Duration::hours(CACHE_TTL_HOURS)
                && age < Duration::hours(OFFLINE_GRACE_HOURS)
            {
                return Ok(state_from_payload(
                    &entry.payload,
                    trial_active,
                    &trial_until,
                ));
            }
        }
    }

    // Fetch from endpoint
    if !endpoint_allowed(endpoint) {
        return Err(anyhow!("license endpoint not in allowlist: {}", endpoint));
    }
    let url = format!(
        "{}/furx/v1/license/check?install_id={}",
        endpoint.trim_end_matches('/'),
        install
    );

    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(5))
        .build()?;
    let resp_res = client.get(&url).send().await;

    match resp_res {
        Ok(resp) if resp.status().is_success() => {
            let payload: LicensePayload = resp.json().await?;
            // Anchor trial to server time once
            if let Some(server_time) = payload.server_time.as_deref() {
                let _ = anchor_trial_to_server(db, server_time);
            }
            cache_save(db, &payload)?;
            // MED-11: recomputar trial_until con el anchor recién aplicado
            let trial_start2 = trial_started_at(db).unwrap_or(trial_start);
            let trial_until2 = trial_start2 + Duration::days(TRIAL_DAYS);
            let trial_active2 = Utc::now() < trial_until2;
            Ok(state_from_payload(&payload, trial_active2, &trial_until2))
        }
        Ok(resp) => {
            tracing::warn!("license check HTTP error {}", resp.status());
            offline_state(db, trial_active, &trial_until)
        }
        Err(e) => {
            tracing::warn!("license check network error: {}", e);
            offline_state(db, trial_active, &trial_until)
        }
    }
}

fn offline_state(
    db: &Arc<parking_lot::Mutex<Connection>>,
    trial_active: bool,
    trial_until: &DateTime<Utc>,
) -> Result<LicenseState> {
    if let Some(entry) = cache_load(db) {
        if let Ok(fetched) = DateTime::parse_from_rfc3339(&entry.fetched_at) {
            let age = Utc::now().signed_duration_since(fetched.with_timezone(&Utc));
            // Council EDGE_6: dentro de 24h grace, mantenemos cached state
            if age < Duration::hours(OFFLINE_GRACE_HOURS) && entry.payload.valid {
                return Ok(state_from_payload(
                    &entry.payload,
                    trial_active,
                    trial_until,
                ));
            }
        }
    }
    // Past 24h grace o sin cache: trial si activo, sino Expired
    if trial_active {
        Ok(LicenseState::Trial {
            until: trial_until.to_rfc3339(),
        })
    } else {
        Ok(LicenseState::Offline {
            last_check: cache_load(db)
                .map(|e| e.fetched_at)
                .unwrap_or_else(|| "never".to_string()),
            cached_state: cache_load(db).map(|e| {
                if e.payload.valid {
                    "valid".to_string()
                } else {
                    "invalid".to_string()
                }
            }),
        })
    }
}

fn state_from_payload(
    payload: &LicensePayload,
    trial_active: bool,
    trial_until: &DateTime<Utc>,
) -> LicenseState {
    if payload.valid {
        // MED-10 fix (Codex): valid_until must be in the future. If backend returns
        // valid=true with a past valid_until, treat as expired/trial fallback.
        let now = Utc::now();
        if let Some(until_str) = payload.valid_until.as_deref() {
            if let Ok(until) = DateTime::parse_from_rfc3339(until_str) {
                if until.with_timezone(&Utc) > now {
                    return LicenseState::Valid {
                        tier: payload.tier.clone().unwrap_or_else(|| "pro".to_string()),
                        until: until_str.to_string(),
                    };
                }
                // valid=true but past — fall through to trial/expired
            }
        } else {
            // valid=true sin valid_until → default 30d, pero igual chequear esto futuro
            return LicenseState::Valid {
                tier: payload.tier.clone().unwrap_or_else(|| "pro".to_string()),
                until: (now + Duration::days(30)).to_rfc3339(),
            };
        }
    }
    if trial_active {
        return LicenseState::Trial {
            until: trial_until.to_rfc3339(),
        };
    }
    LicenseState::Expired
}

/// Returns true if any Pro-gated feature should be unlocked right now.
pub fn is_pro_active(state: &LicenseState) -> bool {
    matches!(
        state,
        LicenseState::Valid { .. } | LicenseState::Trial { .. }
    )
}

/// 045 FR-001 — tier ACTUAL sin tocar la red. Lee el `license.cache` que `check()` deja en DB.
/// Default conservador `"free"` cuando no hay cache (instalación fresca / nunca verificó). Esto
/// alimenta los caps por feature (ej monitores: Free 5 / Pro 50 / Team ∞) sin un round-trip por
/// llamada. El gating de seguridad sigue siendo `check()` async; esto es sólo para cuotas locales.
pub fn current_tier(db: &Arc<parking_lot::Mutex<Connection>>) -> String {
    match cache_load(db) {
        Some(entry) if entry.payload.valid => entry
            .payload
            .tier
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "free".to_string()),
        _ => "free".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_allowlist() {
        assert!(endpoint_allowed("https://aie.example.test"));
        // HIGH-3: HTTP NO permitido; sólo el dominio del vendor (.example.test) por default.
        assert!(!endpoint_allowed("https://api.example.internal"));
        assert!(!endpoint_allowed("http://100.64.0.10:8250"));
        assert!(!endpoint_allowed("http://127.0.0.1:8000"));
        assert!(!endpoint_allowed("http://localhost:8000"));
        assert!(!endpoint_allowed("http://aie.example.test"));
        assert!(!endpoint_allowed("https://evil.com"));
        assert!(!endpoint_allowed("https://attacker.example.test.evil.com"));
    }

    #[test]
    fn pro_active_logic() {
        assert!(is_pro_active(&LicenseState::Valid {
            tier: "pro".into(),
            until: "2027-01-01".into()
        }));
        assert!(is_pro_active(&LicenseState::Trial {
            until: "2027-01-01".into()
        }));
        assert!(!is_pro_active(&LicenseState::Expired));
        assert!(!is_pro_active(&LicenseState::Offline {
            last_check: "never".into(),
            cached_state: None
        }));
    }
}
