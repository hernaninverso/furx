// services/identity.rs — single source of truth for the audit ACTOR and the Keychain ACCOUNT.
//
// Spec 041 (Ola 1, multi-usuario). Before this module Furx hard-coded the actor literal
// `user_colon_hernan` in ~70 audit call-sites and `hernan` as the Keychain account. That made the
// binary unusable
// for anyone else: a stranger's audit trail would be attributed to "hernan", and their Keychain
// reads would target the wrong account. This module resolves the actor and the account at runtime
// WITHOUT ever leaking el autor's identity into the distributed binary.
//
// Design (corrige al consejo GTM, ver /tmp/council-gtm-result.md):
//   - `current_actor()` is SYNC and does NO network I/O on the hot path. The cloud session is read
//     from `cloud_client::active_user()` (an in-process Keychain read, never the network) and the
//     result is cached behind a `parking_lot::RwLock<Option<...>>` with a 30s TTL so the ~70
//     call-sites don't each touch the Keychain. A cache hit is lock-free-ish (read lock).
//   - Resolution order: (1) active cloud session → `user:<email>` (source "cloud"); (2) env `USER`
//     VALIDATED (regex `^[A-Za-z0-9_-]{1,32}$` + blocklist root/daemon/nobody/www-data/"") →
//     `user:<user>` (source "os"); (3) fallback `user:local-<installation_id[..8]>` (source
//     "installation_id"). The fallback NEVER yields `user:root` or `user:hernan`.
//   - `installation_id` is the identity anchor when USER fails. It is generated once at bootstrap
//     (`ensure_installation_id`) and stashed in a process-global `OnceCell`; `installation_id()`
//     returns it. If bootstrap hasn't run yet (e.g. a unit test that calls `current_actor`
//     directly), a deterministic placeholder is used so the fallback never panics.
//   - `keychain_account()` centralizes the Keychain account resolution (FR-006): the validated env
//     `USER`, else the documented legacy `"hernan"` account (so el autor's existing entries keep
//     working). This is the ONLY place that hard-codes the legacy account for telegram-hmac.
//
// This module is PURE in F1: nobody calls it yet (dead code). F2 wires the ~70 actor call-sites,
// F5 wires the Keychain account. Keeping it isolated lets the audit-3 reason about it in isolation.

use std::time::{Duration, Instant};

use once_cell::sync::{Lazy, OnceCell};
use parking_lot::RwLock;

/// How long a resolved actor is cached before re-resolving. 30s (per council v5): the actor does not
/// change every few seconds (a session lasts hours), and 30s bounds the staleness after a
/// login/logout without forcing a Keychain read per call.
const ACTOR_TTL: Duration = Duration::from_secs(30);

/// Identity source recorded in the audit payload (`identity_source`). Trazabilidad de FR-002.
pub const SOURCE_CLOUD: &str = "cloud";
pub const SOURCE_OS: &str = "os";
pub const SOURCE_INSTALLATION_ID: &str = "installation_id";

/// Legacy Keychain account. Documented fallback (FR-006) so el autor's pre-existing entries
/// (`furx-telegram-hmac` under account `hernan`) keep resolving after this change. A fresh install
/// for another user resolves their own `USER` account; only when `USER` is unset/invalid do we fall
/// back here, and a stranger simply won't have that entry (the feature degrades, never misattributes).
pub const LEGACY_KEYCHAIN_ACCOUNT: &str = "hernan";

/// Process-global installation id, set once by `ensure_installation_id` at bootstrap.
static INSTALLATION_ID: OnceCell<String> = OnceCell::new();

/// Deterministic placeholder for `installation_id()` when bootstrap hasn't primed the OnceCell yet
/// (e.g. a unit test calling `current_actor` directly). ≥8 chars so the `take(8)` slice is always
/// full and the fallback actor (`user:local-<id8>`) never panics and never leaks a real identity.
const INSTALLATION_ID_PLACEHOLDER: &str = "local0000-unset";

/// Max length accepted for a cloud-session email before we treat it as malformed and fall through
/// to the OS/installation_id branches. A real email is well under this; the bound stops a corrupted
/// or oversized in-memory session value from being interpolated verbatim into the audit actor.
const MAX_CLOUD_EMAIL_LEN: usize = 254;

struct ActorCache {
    value: String,
    source: &'static str,
    expires_at: Instant,
}

static ACTOR_CACHE: Lazy<RwLock<Option<ActorCache>>> = Lazy::new(|| RwLock::new(None));

/// Generate-or-load the stable `installation_id` (settings key `app.installation_id`). Call ONCE in
/// `main()`/`run()` bootstrap, BEFORE any handler. Idempotent: re-running returns the stored id.
///
/// Also primes the process-global `INSTALLATION_ID` so the sync `installation_id()` accessor (used
/// by `current_actor`) never has to touch the DB on the hot path.
pub fn ensure_installation_id(conn: &rusqlite::Connection) -> String {
    if let Some(existing) = read_installation_id(conn) {
        let _ = INSTALLATION_ID.set(existing.clone());
        return existing;
    }
    let id = uuid::Uuid::new_v4().to_string();
    // Atomic first-writer-wins: `ON CONFLICT DO NOTHING` (NOT the shared `settings::set`, which is
    // DO UPDATE and would let a racing caller overwrite the row). The first INSERT that lands fixes
    // the canonical value; everyone else no-ops. We then re-read the row so the OnceCell mirrors
    // whatever actually persisted, so process memory and DB never disagree. Bootstrap is
    // single-threaded in practice; this is defence-in-depth against a future concurrent caller.
    let _ = conn.execute(
        "INSERT INTO settings (key, value, updated_at) \
         VALUES ('app.installation_id', ?1, datetime('now')) \
         ON CONFLICT(key) DO NOTHING",
        rusqlite::params![serde_json::Value::String(id.clone()).to_string()],
    );
    let canonical = read_installation_id(conn).unwrap_or(id);
    let _ = INSTALLATION_ID.set(canonical.clone());
    INSTALLATION_ID.get().cloned().unwrap_or(canonical)
}

fn read_installation_id(conn: &rusqlite::Connection) -> Option<String> {
    crate::settings::get(conn, "app.installation_id")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(String::from))
        .filter(|s| !s.is_empty())
}

/// The process-global installation id. Returns the primed value when bootstrap has run; otherwise a
/// deterministic placeholder so the actor fallback (`user:local-<id8>`) never panics and never leaks
/// a real identity. The placeholder is 8+ chars (`local0000`) so the `[..8]` slice is always valid.
pub fn installation_id() -> String {
    INSTALLATION_ID
        .get()
        .cloned()
        .unwrap_or_else(|| INSTALLATION_ID_PLACEHOLDER.to_string())
}

/// SYNC actor for the audit log. No network I/O. The cloud session is read in-process (Keychain),
/// cached for `ACTOR_TTL`. Resolution order: cloud session → validated `USER` → `user:local-<id8>`.
///
/// NEVER returns `user:hernan` (unless the running user's `USER` literally is `hernan`, which is
/// correct — el autor is just another user) and NEVER returns `user:root`.
pub fn current_actor() -> String {
    // Fast path: a fresh cached value.
    {
        let guard = ACTOR_CACHE.read();
        if let Some(ca) = guard.as_ref() {
            if Instant::now() < ca.expires_at {
                return ca.value.clone();
            }
        }
    }
    // Slow path: re-resolve under a write lock (double-check first).
    let (value, source) = resolve_actor();
    let mut guard = ACTOR_CACHE.write();
    if let Some(ca) = guard.as_ref() {
        if Instant::now() < ca.expires_at {
            return ca.value.clone();
        }
    }
    *guard = Some(ActorCache {
        value: value.clone(),
        source,
        expires_at: Instant::now() + ACTOR_TTL,
    });
    value
}

/// The source of the value currently cached by `current_actor()`. Use for the `identity_source`
/// audit field. If nothing is cached yet, reports `installation_id` (the most conservative default).
pub fn current_actor_source() -> &'static str {
    ACTOR_CACHE
        .read()
        .as_ref()
        .map(|ca| ca.source)
        .unwrap_or(SOURCE_INSTALLATION_ID)
}

/// Clears the actor cache. Call on login/logout so the next `current_actor()` re-resolves.
pub fn invalidate_actor_cache() {
    ACTOR_CACHE.write().take();
}

/// Derive the `identity_source` for a given actor STRING, so the audit writer can stamp it
/// consistently for every event regardless of call-site (and without reading the cache, which could
/// have rolled over). The actor string is unambiguous:
///   - `user:local-…`            → "installation_id"
///   - `user:<…@…>` (has `@`)    → "cloud" (an email)
///   - `user:<…>` (no `@`)       → "os"
///   - anything else (e.g. `system`, `system:…`) → "system"
/// This keeps `identity_source` correct even for the non-`current_actor()` actors like `"system"`.
pub fn source_for_actor(actor: &str) -> &'static str {
    if let Some(rest) = actor.strip_prefix("user:") {
        // Check `@` (cloud email) BEFORE the `local-` prefix: a real cloud email always contains
        // `@`, and only the synthetic installation-id fallback (`user:local-<id8>`) lacks one. This
        // ordering avoids misclassifying an email whose local-part happens to start with `local-`
        // (e.g. `local-team@furx.cloud`) as an installation_id (audit finding F2).
        if rest.contains('@') {
            SOURCE_CLOUD
        } else if rest.starts_with("local-") {
            SOURCE_INSTALLATION_ID
        } else {
            SOURCE_OS
        }
    } else {
        "system"
    }
}

/// Pure resolution (no caching) — returns (actor, source). Split out so tests can exercise the
/// resolution logic deterministically without the cache.
fn resolve_actor() -> (String, &'static str) {
    if let Some(email) = crate::services::cloud_client::active_user() {
        if let Some(clean) = sanitize_cloud_email(&email) {
            return (format!("user:{}", clean), SOURCE_CLOUD);
        }
    }
    if let Some(user) = validated_os_user() {
        return (format!("user:{}", user), SOURCE_OS);
    }
    let id = installation_id();
    // `chars().take(8)` is panic-free regardless of `id`'s length (vs slicing by byte index, which
    // would panic on a <8-char value or mid-codepoint). `id` is a UUID in production and an ≥8-char
    // placeholder before bootstrap, so this is always 8 chars.
    let short: String = id.chars().take(8).collect();
    (format!("user:local-{}", short), SOURCE_INSTALLATION_ID)
}

/// Bound + scrub a cloud-session email before it becomes the audit actor. An email's `@`/`.` make it
/// fail the OS-username regex by design, so we do NOT apply that regex here; instead we trim, reject
/// empty/oversized values, and reject any control character or whitespace (which could corrupt the
/// audit line or smuggle a newline). Returns the cleaned email, or `None` to fall through.
fn sanitize_cloud_email(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CLOUD_EMAIL_LEN {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    // Defence-in-depth: if the in-memory session value is a BARE system/service account name (not an
    // email), reject it so `current_actor()` can never emit `user:root` etc. A real email
    // (`root@x.com`) contains `@` and is NOT bare-equal to a blocklisted name, so it passes.
    if is_blocklisted_account(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Resolves the Keychain account (FR-006): validated env `USER`, else the documented legacy account.
/// Centralizes the legacy-`hernan` fallback so it lives in exactly one place.
pub fn keychain_account() -> String {
    validated_os_user().unwrap_or_else(|| LEGACY_KEYCHAIN_ACCOUNT.to_string())
}

/// Returns the env `USER` ONLY when it passes validation; otherwise `None`. Validation:
/// non-empty, ≤32 chars, charset `[A-Za-z0-9_-]`, not a system/service account.
fn validated_os_user() -> Option<String> {
    std::env::var("USER").ok().filter(|u| is_valid_os_username(u))
}

/// `^[A-Za-z0-9_-]{1,32}$` + blocklist. Rejects `root`/`daemon`/`nobody`/`www-data` and empty.
fn is_valid_os_username(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !is_blocklisted_account(s)
}

/// Bare system/service accounts that must never become an actor identity.
fn is_blocklisted_account(s: &str) -> bool {
    matches!(s, "root" | "daemon" | "nobody" | "www-data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    // The tests below mutate the process-global `USER` env var and the actor cache. The crate runs
    // tests single-threaded (`.cargo/config.toml` sets --test-threads=1) so these don't race; we
    // still reset the cache at the start of each to be independent of ordering.

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        conn
    }

    #[test]
    fn is_valid_os_username_rules() {
        assert!(is_valid_os_username("hernan"));
        assert!(is_valid_os_username("ada_lovelace"));
        assert!(is_valid_os_username("user-1"));
        assert!(is_valid_os_username(&"a".repeat(32)));
        // Rejections.
        assert!(!is_valid_os_username(""));
        assert!(!is_valid_os_username("root"));
        assert!(!is_valid_os_username("daemon"));
        assert!(!is_valid_os_username("nobody"));
        assert!(!is_valid_os_username("www-data"));
        assert!(!is_valid_os_username(&"a".repeat(33))); // >32
        assert!(!is_valid_os_username("has space"));
        assert!(!is_valid_os_username("inva/lid"));
        assert!(!is_valid_os_username("uñicode")); // non-ascii
    }

    #[test]
    fn ensure_installation_id_is_stable_and_idempotent() {
        let conn = mem_db();
        let a = ensure_installation_id(&conn);
        let b = ensure_installation_id(&conn);
        assert_eq!(a, b, "installation_id must be stable across calls");
        assert!(!a.is_empty());
        // Stored under the documented key.
        let stored = read_installation_id(&conn).unwrap();
        assert_eq!(stored, a);
    }

    #[test]
    fn installation_id_accessor_returns_non_empty_placeholder_before_bootstrap() {
        // Even if bootstrap hasn't primed the OnceCell in this test, the accessor must give a
        // slice-safe (≥8 char) value so the fallback never panics.
        let id = installation_id();
        assert!(id.len() >= 8, "installation_id must be ≥8 chars: {id}");
    }

    #[test]
    fn actor_falls_back_to_local_when_user_is_root() {
        std::env::set_var("USER", "root");
        invalidate_actor_cache();
        let (actor, source) = resolve_actor();
        // Without a cloud session, root is rejected → installation_id fallback.
        // (If a cloud session happens to exist in this dev env, source is "cloud" — still never root.)
        assert!(!actor.contains("root"), "root leaked into actor: {actor}");
        assert!(
            actor.starts_with("user:local-") || actor.starts_with("user:"),
            "unexpected actor format: {actor}"
        );
        assert!(
            source == SOURCE_INSTALLATION_ID || source == SOURCE_CLOUD,
            "unexpected source: {source}"
        );
    }

    #[test]
    fn actor_never_leaks_hernan_for_arbitrary_runner() {
        // A CI runner with USER="runner" or unset must never produce the dev identity actor.
        std::env::set_var("USER", "runner");
        invalidate_actor_cache();
        let (actor, _) = resolve_actor();
        assert!(!actor.contains("hernan"), "actor leaked dev identity: {actor}");
    }

    #[test]
    fn actor_uses_valid_os_user() {
        std::env::set_var("USER", "ada");
        invalidate_actor_cache();
        // resolve_actor short-circuits to cloud if a session exists; assert via validated_os_user
        // that "ada" is accepted, and that when there's no cloud session the os branch is taken.
        assert_eq!(validated_os_user().as_deref(), Some("ada"));
        if crate::services::cloud_client::active_user().is_none() {
            let (actor, source) = resolve_actor();
            assert_eq!(actor, "user:ada");
            assert_eq!(source, SOURCE_OS);
        }
    }

    #[test]
    fn actor_cache_serves_within_ttl() {
        std::env::set_var("USER", "ada");
        invalidate_actor_cache();
        let a1 = current_actor();
        // Mutate USER; within TTL the cached value must NOT change.
        std::env::set_var("USER", "bob");
        let a2 = current_actor();
        assert_eq!(a1, a2, "cache must serve the same value within TTL");
    }

    #[test]
    fn actor_cache_reresolves_after_ttl_expiry() {
        std::env::set_var("USER", "ada");
        invalidate_actor_cache();
        let _ = current_actor();
        // Force the cached entry to look expired.
        {
            let mut g = ACTOR_CACHE.write();
            if let Some(ca) = g.as_mut() {
                ca.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }
        std::env::set_var("USER", "bob");
        // Only assert re-resolution when there's no cloud session masking the OS branch.
        if crate::services::cloud_client::active_user().is_none() {
            let a3 = current_actor();
            assert_eq!(a3, "user:bob", "expired cache must re-resolve from env");
        }
    }

    #[test]
    fn source_for_actor_classifies() {
        assert_eq!(source_for_actor("user:ada"), SOURCE_OS);
        assert_eq!(source_for_actor("user:ada@furx.cloud"), SOURCE_CLOUD);
        assert_eq!(source_for_actor("user:local-abcd1234"), SOURCE_INSTALLATION_ID);
        assert_eq!(source_for_actor("system"), "system");
        assert_eq!(source_for_actor("system:monitor"), "system");
        // An email whose local-part starts with `local-` is still cloud (@ wins over local- prefix).
        assert_eq!(source_for_actor("user:local-team@furx.cloud"), SOURCE_CLOUD);
    }

    #[test]
    fn sanitize_cloud_email_bounds_and_scrubs() {
        assert_eq!(
            sanitize_cloud_email(" ada@furx.cloud "),
            Some("ada@furx.cloud".to_string())
        );
        assert_eq!(sanitize_cloud_email(""), None);
        assert_eq!(sanitize_cloud_email("   "), None);
        // Embedded newline / control char → rejected (no audit-line corruption).
        assert_eq!(sanitize_cloud_email("a@b.io\nactor:root"), None);
        assert_eq!(sanitize_cloud_email("a@b.io with space"), None);
        // Oversized → rejected.
        assert_eq!(sanitize_cloud_email(&"a".repeat(300)), None);
        // A BARE blocklisted account must never pass (defence-in-depth: no user:root via cloud).
        assert_eq!(sanitize_cloud_email("root"), None);
        assert_eq!(sanitize_cloud_email("www-data"), None);
        // A real email whose local-part is "root" is fine (not bare-equal to the blocklist).
        assert_eq!(
            sanitize_cloud_email("root@example.com"),
            Some("root@example.com".to_string())
        );
    }

    #[test]
    fn keychain_account_resolves_user_then_legacy() {
        std::env::set_var("USER", "ada");
        assert_eq!(keychain_account(), "ada");
        std::env::set_var("USER", "root"); // invalid → legacy
        assert_eq!(keychain_account(), LEGACY_KEYCHAIN_ACCOUNT);
        std::env::remove_var("USER");
        assert_eq!(keychain_account(), LEGACY_KEYCHAIN_ACCOUNT);
        // restore something benign
        std::env::set_var("USER", "ada");
    }
}
