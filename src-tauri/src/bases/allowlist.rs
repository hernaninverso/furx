// Shared host allowlist using strict URL parsing.
// Used by aie.rs, telegram.rs, mcp_health.rs.
//
// Why: ends_with(".example.test") would accept `attacker.com.example.test` if the
// attacker can register a subdomain. We parse with url::Url and compare
// hostname with explicit suffix-after-dot check.
//
// 041 FR-005 (multi-usuario) — the DEFAULT base allowlist is now ONLY loopback
// (`localhost`/`127.0.0.1`/`::1`). NO infra of el autor's (the the dev server Tailscale IP,
// `example.internal`/`example.test`/`devserver.local`) is baked in as a default anymore.
// What a user trusts beyond loopback comes from THREE additive layers, none of
// which ships another person's host in the binary:
//   1. env `FURX_ALLOWLIST_EXTRA_HOSTS` (CSV, legacy/CI knob) — unchanged.
//   2. settings `network.extra_origins` (JSON array of origin URLs) loaded
//      synchronously at bootstrap into RUNTIME_ORIGINS — this is how the wizard
//      and el autor's own Settings register their endpoints.
//   3. `add_runtime_origin(origin)` at runtime (after the wizard saves).
// Design decision (corrige al consejo GTM): we do NOT block any private/CGNAT
// range — a user's own Tailscale `100.x` host, once configured, is allowed. The
// lock is "no default points at someone else's infra", not range-blocking.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// Loopback hosts allowed *as-is* (exact match). This is the ENTIRE default base allowlist now —
/// only the local machine. Everything else is added per-user via the env / settings / runtime layers.
/// NOTE: `url::Url::host_str()` returns the IPv6 loopback bracketed as `[::1]`, so that is the form
/// we match against (a bare `::1` would never equal the parsed host).
static EXACT: Lazy<Vec<&'static str>> = Lazy::new(|| vec!["localhost", "127.0.0.1", "[::1]"]);

/// Default suffix allowlist: EMPTY. `example.internal`/`example.test`/`devserver.local` were el autor's infra
/// and are no longer defaults; a user adds their own via `*.suffix` in `FURX_ALLOWLIST_EXTRA_HOSTS`.
static SUFFIXES: Lazy<Vec<&'static str>> = Lazy::new(Vec::new);

/// 041 FR-005 — runtime-configured allowed HOSTS, derived from `settings:network.extra_origins`
/// (a JSON array of origin URLs) at bootstrap and from `add_runtime_origin` after the wizard saves.
/// We store HOSTS (not full origins) because `url_allowed` matches by host — keeping the existing
/// host-based contract (cero regresión) while making the set dynamic. Rare writes, many reads →
/// `parking_lot::RwLock<HashSet>` (not DashMap, per council).
static RUNTIME_HOSTS: Lazy<RwLock<HashSet<String>>> = Lazy::new(|| RwLock::new(HashSet::new()));

/// Load runtime allowed hosts from `settings:network.extra_origins` (JSON array of origin URLs).
/// MUST be called SYNCHRONOUSLY in `main()`/`run()` BEFORE `tauri::Builder` — otherwise the first
/// outbound call (e.g. a background AIE ping) would see an empty runtime set and be rejected. No
/// network I/O. Malformed entries are logged and skipped (a corrupted/edited settings row must not
/// crash the boot). Idempotent (re-inserts the same hosts).
pub fn init_from_settings(conn: &rusqlite::Connection) {
    let raw = match crate::settings::get(conn, "network.extra_origins") {
        Ok(Some(v)) => v,
        _ => return,
    };
    // Accept a JSON array of strings (the canonical shape) — NOT a CSV (a URL with a comma would
    // break a split). A non-array value is ignored (logged). Cap the number of entries so a
    // corrupted/edited settings row with a huge array can't slow the synchronous boot (defence in
    // depth — it's the user's own DB, but the bound keeps boot bounded).
    const MAX_RUNTIME_ORIGINS: usize = 256;
    let origins: Vec<String> = match raw {
        serde_json::Value::Array(items) => items
            .into_iter()
            .take(MAX_RUNTIME_ORIGINS)
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        other => {
            tracing::warn!(
                "allowlist: network.extra_origins is not a JSON array (got {}); ignoring",
                other
            );
            return;
        }
    };
    let mut rejected = Vec::new();
    for origin in origins {
        if add_runtime_origin(&origin).is_err() {
            rejected.push(origin);
        }
    }
    if !rejected.is_empty() {
        tracing::warn!(
            "allowlist: skipped malformed network.extra_origins entries: {:?}",
            rejected
        );
    }
}

/// Add a single origin URL to the runtime allowlist (called after the wizard saves an endpoint).
/// Parses with `url::Url` (a bare `http://` with no host is rejected), extracts the host, and
/// inserts it (lowercased) into RUNTIME_HOSTS. Returns `Err` on a malformed origin so the caller
/// can surface it. Idempotent.
pub fn add_runtime_origin(origin: &str) -> Result<(), String> {
    let url = url::Url::parse(origin).map_err(|e| format!("origin inválido '{}': {}", origin, e))?;
    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("esquema '{}' no permitido en origin '{}'", s, origin)),
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("origin sin host: {}", origin))?
        .to_ascii_lowercase();
    RUNTIME_HOSTS.write().insert(host);
    Ok(())
}

/// Test-only reset of the runtime allowlist so tests don't leak state into each other (the statics
/// are process-global). `--test-threads=1` (`.cargo/config.toml`) keeps this deterministic. Exposed
/// `pub(crate)` so dependent module tests (aie/telegram) can isolate their assertions too.
#[cfg(test)]
pub(crate) fn reset_runtime_hosts_for_test() {
    RUNTIME_HOSTS.write().clear();
}

/// BLOQUE J ext (council 5/5): user-extensible allowlist for distributable
/// installs. Read once at process start from `FURX_ALLOWLIST_EXTRA_HOSTS`
/// (CSV: `host1,host2,*.example.com`). Lines prefixed with `*.` become
/// suffix matches; bare entries become EXACT matches. Validated against a
/// strict hostname regex; malformed entries are skipped with a tracing
/// warning (Council pragmatic+data must-fix: never silently allow garbage).
/// Defaults are immutable: the EXTRA list ADDS, it cannot remove or shadow.
fn parse_host(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() || s.len() > 253 {
        return None;
    }
    // Strip optional `*.` suffix-marker — kept separately by caller.
    let body = s.strip_prefix("*.").unwrap_or(&s);
    if body.is_empty() || body.len() > 253 {
        return None;
    }
    if body.starts_with('.') || body.ends_with('.') {
        return None;
    }

    // Audit Llama MED: if every label is purely numeric, validate as IPv4 octets
    // (each must parse as a u8). `999.999.999.999` would otherwise sneak through
    // the DNS-label regex below because each "label" is alphanumeric and ≤63 chars.
    let parts: Vec<&str> = body.split('.').collect();
    let looks_ipv4 =
        parts.len() == 4 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()));
    if looks_ipv4 {
        let ok = parts.iter().all(|p| p.parse::<u8>().is_ok());
        return if ok { Some(s) } else { None };
    }

    // Otherwise validate as DNS labels (a-z 0-9 -, ≤63, no leading/trailing dash).
    let valid = parts.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    });
    if !valid {
        return None;
    }
    Some(s)
}

static EXTRA_EXACT: Lazy<Vec<String>> = Lazy::new(|| {
    let raw = std::env::var("FURX_ALLOWLIST_EXTRA_HOSTS").unwrap_or_default();
    let mut out = Vec::new();
    let mut rejected = Vec::new();
    for piece in raw.split(',') {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("*.") {
            continue;
        } // suffix entries handled by EXTRA_SUFFIX
        match parse_host(trimmed) {
            Some(h) => {
                if !EXACT.iter().any(|d| **d == h) && !out.contains(&h) {
                    out.push(h);
                }
            }
            None => rejected.push(trimmed.to_string()),
        }
    }
    if !rejected.is_empty() {
        tracing::warn!(
            "allowlist: rejected malformed FURX_ALLOWLIST_EXTRA_HOSTS entries: {:?}",
            rejected
        );
    }
    out
});

static EXTRA_SUFFIX: Lazy<Vec<String>> = Lazy::new(|| {
    let raw = std::env::var("FURX_ALLOWLIST_EXTRA_HOSTS").unwrap_or_default();
    let mut out = Vec::new();
    let mut rejected = Vec::new();
    for piece in raw.split(',') {
        let trimmed = piece.trim();
        if !trimmed.starts_with("*.") {
            continue;
        }
        let body = &trimmed[2..];
        match parse_host(body) {
            Some(h) => {
                if !SUFFIXES.iter().any(|d| **d == h) && !out.contains(&h) {
                    out.push(h);
                }
            }
            None => rejected.push(trimmed.to_string()),
        }
    }
    if !rejected.is_empty() {
        tracing::warn!(
            "allowlist: rejected malformed *.suffix entries: {:?}",
            rejected
        );
    }
    out
});

pub fn url_allowed(input: &str) -> bool {
    let Ok(u) = url::Url::parse(input) else {
        return false;
    };
    match u.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if EXACT.iter().any(|h| *h == host) {
        return true;
    }
    if EXTRA_EXACT.iter().any(|h| h == &host) {
        return true;
    }
    // 041 FR-005 — runtime hosts (wizard / Settings / bootstrap from network.extra_origins).
    if RUNTIME_HOSTS.read().contains(&host) {
        return true;
    }
    for s in SUFFIXES.iter() {
        if host == *s || host.ends_with(&format!(".{}", s)) {
            return true;
        }
    }
    for s in EXTRA_SUFFIX.iter() {
        if host == s.as_str() || host.ends_with(&format!(".{}", s)) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_loopback_by_default() {
        reset_runtime_hosts_for_test();
        assert!(url_allowed("http://localhost:8250/x"));
        assert!(url_allowed("http://127.0.0.1/"));
        assert!(url_allowed("http://[::1]:8250/"));
    }

    #[test]
    fn default_base_does_not_allow_hernan_infra() {
        // 041 FR-005 — with no runtime origins configured, NONE of el autor's old defaults are allowed.
        reset_runtime_hosts_for_test();
        assert!(!url_allowed("http://100.64.0.10:8250"));
        assert!(!url_allowed("https://aie.example.internal"));
        assert!(!url_allowed("https://scan.example.test/r"));
        assert!(!url_allowed("https://devserver.local"));
    }

    #[test]
    fn runtime_origin_allows_user_host_including_tailscale() {
        // 041 SC-004 — a host the user adds (incl. a 100.x Tailscale, NOT range-blocked) is allowed.
        reset_runtime_hosts_for_test();
        assert!(!url_allowed("http://100.99.1.2:8250"), "not allowed before adding");
        add_runtime_origin("http://100.99.1.2:8250").unwrap();
        assert!(url_allowed("http://100.99.1.2:8250"));
        // Host match is port-independent (we store the host), matching the legacy contract.
        assert!(url_allowed("http://100.99.1.2:11434/api/tags"));
        // A different host is still rejected.
        assert!(!url_allowed("http://100.99.9.9:8250"));
        reset_runtime_hosts_for_test();
    }

    #[test]
    fn add_runtime_origin_rejects_malformed() {
        assert!(add_runtime_origin("http://").is_err()); // no host
        assert!(add_runtime_origin("not a url").is_err());
        assert!(add_runtime_origin("ftp://host").is_err()); // bad scheme
        assert!(add_runtime_origin("file:///etc/passwd").is_err());
    }

    #[test]
    fn init_from_settings_loads_json_array() {
        reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             INSERT INTO settings (key,value) VALUES ('network.extra_origins',
               json('[\"http://100.64.0.10:8250\",\"https://aie.example.io:443\"]'));",
        )
        .unwrap();
        init_from_settings(&conn);
        // Both configured hosts are now allowed (the user explicitly trusts them — even a 100.x).
        assert!(url_allowed("http://100.64.0.10:8250"));
        assert!(url_allowed("https://aie.example.io/v1"));
        // A host NOT in the array is still rejected.
        assert!(!url_allowed("http://evil.example.org"));
        reset_runtime_hosts_for_test();
    }

    #[test]
    fn init_from_settings_skips_malformed_and_non_array() {
        reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             -- one good, one malformed entry in the array; and the key as a non-array elsewhere.
             INSERT INTO settings (key,value) VALUES ('network.extra_origins',
               json('[\"http://good.example.io:8250\",\"http://\",\"garbage\"]'));",
        )
        .unwrap();
        init_from_settings(&conn);
        assert!(url_allowed("http://good.example.io/x"));
        // Malformed entries didn't crash and weren't added.
        assert!(!url_allowed("http://garbage"));
        reset_runtime_hosts_for_test();
    }

    #[test]
    fn rejects_realistic_subdomain_attack() {
        // The realistic attack: attacker controls a domain (attacker.com) and puts a trusted token
        // as a prefix. With a runtime suffix-less model these are rejected outright; we also verify
        // that a configured EXACT host doesn't leak to a look-alike suffix.
        reset_runtime_hosts_for_test();
        add_runtime_origin("https://aie.example.io:443").unwrap();
        // evil.example.io.attacker.com must NOT match the configured aie.example.io exact host.
        assert!(!url_allowed("https://aie.example.io.attacker.com"));
        assert!(!url_allowed("https://100.64.0.10.attacker.com"));
        reset_runtime_hosts_for_test();
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!url_allowed("file:///etc/passwd"));
        assert!(!url_allowed("ftp://localhost"));
        assert!(!url_allowed("javascript:alert(1)"));
    }

    #[test]
    fn rejects_malformed() {
        assert!(!url_allowed(""));
        assert!(!url_allowed("not a url"));
        assert!(!url_allowed("http://"));
    }

    #[test]
    fn parse_host_accepts_valid_dns() {
        assert_eq!(parse_host("Example.COM").as_deref(), Some("example.com"));
        assert_eq!(
            parse_host("api-v2.foo.example.io").as_deref(),
            Some("api-v2.foo.example.io")
        );
        assert_eq!(parse_host("10.0.0.1").as_deref(), Some("10.0.0.1"));
        assert_eq!(
            parse_host("*.example.com").as_deref(),
            Some("*.example.com")
        );
    }

    #[test]
    fn parse_host_rejects_malformed() {
        assert!(parse_host("").is_none());
        assert!(parse_host(".leading.dot").is_none());
        assert!(parse_host("trailing.dot.").is_none());
        assert!(parse_host("has spaces").is_none());
        assert!(parse_host("under_score").is_none()); // RFC: underscores not allowed in hostnames
        assert!(parse_host("-leading-dash.com").is_none());
        assert!(parse_host("trailing-dash-.com").is_none());
        // 64-char label > 63 max
        let too_long = "a".repeat(64);
        assert!(parse_host(&format!("{}.com", too_long)).is_none());
    }

    #[test]
    fn parse_host_validates_ipv4_octet_bounds() {
        // Audit Llama MED follow-up: 4-part all-digit hosts MUST be valid IPv4.
        assert_eq!(parse_host("10.0.0.1").as_deref(), Some("10.0.0.1"));
        assert_eq!(
            parse_host("255.255.255.255").as_deref(),
            Some("255.255.255.255")
        );
        assert!(parse_host("999.0.0.1").is_none(), "octet > 255 must reject");
        assert!(
            parse_host("256.0.0.1").is_none(),
            "octet exactly 256 must reject"
        );
        // 3-part / 5-part all-digit aren't IPv4 but ARE syntactically valid DNS
        // labels (per RFC 1123 hostnames can be all-numeric). The allowlist
        // intentionally lets them through to its label-validation branch.
        assert_eq!(parse_host("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(parse_host("1.2.3.4.5").as_deref(), Some("1.2.3.4.5"));
    }
}
