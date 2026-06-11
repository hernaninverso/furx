// BLOQUE J (ULTRA REVIEW) — centralised AIE endpoint resolution.
//
// Until this module, 9 different services hard-coded `http://100.64.0.10:8250`
// directly into reqwest calls. That URL is the the dev server Tailscale AIE — it works
// for el autor, but breaks for any other distribution target (BYOK universal,
// PLAN_VIGENTE.md). 4 of the 9 sites already routed through
// `settings_store::get("endpoints.aie")` with a duplicated fallback literal;
// the other 5 bypassed settings entirely.
//
// This module is the single source of truth:
//   - DEFAULT_AIE_URL is the historical the dev server endpoint (so el autor's local dev
//     install keeps working out of the box on a fresh DB).
//   - resolve_url(db) reads `endpoints.aie` from the settings store and falls
//     back to DEFAULT_AIE_URL only when unset.
//   - resolve_url_or_default() is the no-db variant used from background tasks
//     that don't already hold a State<AppState>.
//
// Anyone adding a new AIE call MUST go through this helper, never via the
// literal URL — gotcha caught during BLOQUE J grep audit.

use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;

use crate::settings as settings_store;

/// 041 FR-003 — default-localhost. A fresh install points AIE at the local loopback so NO infra of
/// el autor's (or anyone's) is baked into the distributed binary. Each user configures their own AIE
/// endpoint in their local settings (`endpoints.aie`) or via `FURX_AIE_URL`; both win over this
/// default. el autor points his at the the dev server Tailscale host in HIS settings, like any other user.
pub const DEFAULT_AIE_URL: &str = "http://localhost:8250";

/// Resolve the AIE base URL from settings, falling back to DEFAULT_AIE_URL
/// when unset or empty. Strips trailing slashes for safe concat with path
/// segments (e.g. `format!("{}/v1/chat/completions", resolve_url(db))`).
pub fn resolve_url(db: &Mutex<Connection>) -> String {
    let conn = db.lock();
    // settings::get returns Result<Option<Value>>: outer Ok = query ran, inner
    // Some = key present, value = JSON. We accept only string values.
    let raw: Option<String> = settings_store::get(&conn, "endpoints.aie")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(String::from));
    drop(conn);
    let url = raw
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_AIE_URL.to_string());
    url.trim_end_matches('/').to_string()
}

/// Arc-flavoured variant for callers holding an Arc<Mutex<Connection>>
/// (background tasks, daemons).
pub fn resolve_url_arc(db: &Arc<Mutex<Connection>>) -> String {
    resolve_url(db.as_ref())
}

/// Fallback for code paths that have no DB handle (services called outside
/// a Tauri State context — e.g. memory_daemon, skills run_council). Reads
/// `FURX_AIE_URL` env first so packaging / CI can override without touching
/// the binary; only falls back to DEFAULT_AIE_URL when env is unset/empty.
pub fn resolve_url_or_default() -> String {
    if let Ok(v) = std::env::var("FURX_AIE_URL") {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_AIE_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_localhost_not_hernans_infra() {
        // 041 SC-002 — the distributed default MUST be localhost; no private/Tailscale IP of
        // el autor's may be baked in. Locking this so a refactor can't silently re-introduce it.
        assert_eq!(DEFAULT_AIE_URL, "http://localhost:8250");
        assert!(!DEFAULT_AIE_URL.contains("100.64.0.10"));
    }

    #[test]
    fn resolve_strips_trailing_slash() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             INSERT INTO settings (key, value) VALUES ('endpoints.aie', '\"http://example.com:8250/\"');"
        ).unwrap();
        let db = Mutex::new(conn);
        assert_eq!(resolve_url(&db), "http://example.com:8250");
    }

    #[test]
    fn resolve_falls_back_when_unset() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));"
        ).unwrap();
        let db = Mutex::new(conn);
        assert_eq!(resolve_url(&db), DEFAULT_AIE_URL);
    }

    #[test]
    fn resolve_falls_back_on_empty_string() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             INSERT INTO settings (key, value) VALUES ('endpoints.aie', '\"  \"');"
        ).unwrap();
        let db = Mutex::new(conn);
        assert_eq!(resolve_url(&db), DEFAULT_AIE_URL);
    }
}
