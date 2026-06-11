// services/keychain_bearer.rs — single consolidated accessor for the AIE internal bearer.
//
// Spec 039. Before this module the secret `aie-internal-bearer` was read from ~12 places with
// duplicated logic: 6+ functions shelled `/usr/bin/security find-generic-password -a hernan -s
// aie-internal-bearer -w` (a subprocess per read, account+service hardcoded) and one used the good
// in-process pattern (`keychain::load` + `USER`). ~9 of those callers run in background loops, so
// every tick re-read the Keychain. This module replaces all of them with a single cached accessor.
//
// Design (curated by the 8-voice frontier council, see specs/039-bearer-helper/plan.md):
//   - ONE mechanism: `keychain::load` in-process (native macOS Security API via the `keyring`
//     crate). NO `/usr/bin/security` shelling — the accessor is the signed Furx binary, which
//     matches the partition list (teamid:the app's team id), so no Keychain prompt is reintroduced.
//   - Cache with TTL behind a `parking_lot::RwLock` (no poison). The common read (TTL valid) takes
//     a read lock and returns lock-free for the ~9 daemons. A miss/expiry takes a write lock and
//     double-checks TTL + backoff before fetching.
//   - Backoff short-circuit (NOT exponential): on a fetch failure we record `last_error_at`; for
//     ERROR_BACKOFF the next calls return `None` without touching the Keychain, avoiding a storm if
//     the Keychain is momentarily locked.
//   - Invalidation on-401: `invalidate_bearer_cache()` clears cache AND backoff so the next call
//     re-reads the (rotated) value. No prior version is kept (a `-v1` fallback would be an attack
//     vector with no benefit).
//   - Fail-closed: a missing/empty entry returns `None`, never panics, never blocks a daemon. The
//     bearer value is NEVER logged (only the `USER`/account is, for LaunchAgent diagnosis).
//
// Scope: STRICTLY `aie-internal-bearer`. NOT `furx-telegram-hmac`, NOT the `claude_accounts`
// per-agent entries — those are other secrets/flows with their own modules.
//
// TTL assumption: the AIE internal bearer has no expiry of its own (it's a static shared secret
// rotated by hand in the Keychain), so a 600s cache cannot serve an "expired token". When el autor
// rotates it, the caller hits a 401 and calls `invalidate_bearer_cache()` → the next read picks up
// the new value. A user other than `hernan` whose Keychain lacks the entry simply gets `None`
// (NotFound) and the feature degrades; the `hernan` account fallback covers the historical entries.

use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::RwLock;

/// The ONLY definition of the Keychain entry name (SC-004). Exposed `pub(crate)` so other modules
/// that legitimately need the service NAME (e.g. `aie_repl` as the default per-agent bearer service)
/// reference this constant instead of re-spelling the literal.
pub(crate) const AIE_BEARER_SERVICE: &str = "aie-internal-bearer";

/// How long a successfully fetched bearer is cached before re-reading the Keychain.
const TTL: Duration = Duration::from_secs(600);

/// After a failed fetch, how long `get_bearer()` short-circuits to `None` without re-reading.
const ERROR_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct CacheEntry {
    value: String,
    fetched_at: Instant,
}

/// Combined cache + backoff state under a single lock so the double-check is atomic.
#[derive(Default)]
struct State {
    entry: Option<CacheEntry>,
    last_error_at: Option<Instant>,
}

static CACHE: Lazy<RwLock<State>> = Lazy::new(|| RwLock::new(State::default()));

/// Returns the AIE internal bearer, cached for `TTL`. A cache hit takes only a read lock (the
/// background daemons never contend on a write lock once the value is warm).
///
/// Behaviour:
///   - Fresh cache entry (fetched within `TTL`) → returns it under a read lock (the common path for
///     the background daemons).
///   - Miss/expired → takes a write lock and double-checks: if another thread refreshed the entry
///     while we waited, returns that; if a recent fetch failed and we're inside `ERROR_BACKOFF`,
///     short-circuits to `None` WITHOUT calling the Keychain; otherwise fetches, caches on success
///     or records `last_error_at` on failure.
///
/// Never panics; returns `None` when the entry is missing/empty (fail-closed).
pub fn get_bearer() -> Option<String> {
    log_account_once();
    // Fast path: read lock, return if the cached entry is still within TTL.
    {
        let state = CACHE.read();
        if let Some(entry) = &state.entry {
            if entry.fetched_at.elapsed() < TTL {
                return Some(entry.value.clone());
            }
        }
    }

    // Slow path: write lock + double-check (another thread may have refreshed, or set backoff).
    let mut state = CACHE.write();
    if let Some(entry) = &state.entry {
        if entry.fetched_at.elapsed() < TTL {
            return Some(entry.value.clone());
        }
    }
    // Backoff short-circuit: a recent fetch failed; don't hammer the Keychain.
    if let Some(err_at) = state.last_error_at {
        if err_at.elapsed() < ERROR_BACKOFF {
            return None;
        }
    }

    match fetch_from_keychain() {
        Some(value) => {
            state.entry = Some(CacheEntry {
                value: value.clone(),
                fetched_at: Instant::now(),
            });
            state.last_error_at = None;
            Some(value)
        }
        None => {
            // Keep any stale entry out of the way: a failed fetch means we couldn't read the entry,
            // so we report None and arm the backoff. We do NOT clear `entry` here — but the entry is
            // already expired (we only reach here past the TTL check), so the next call re-fetches
            // after the backoff window.
            state.last_error_at = Some(Instant::now());
            None
        }
    }
}

/// Clears the cached bearer AND the error backoff. Call this when a caller gets a 401 from the AIE
/// (the rotation path): the next `get_bearer()` re-reads the new value from the Keychain.
pub fn invalidate_bearer_cache() {
    let mut state = CACHE.write();
    state.entry = None;
    state.last_error_at = None;
}

/// Resolves the Keychain account for the bearer entry. 041 FR-006 — centralizes on
/// `identity::keychain_account()` (validated env `USER`, else the legacy `hernan` account) so the
/// account resolution lives in ONE place across telegram-hmac and the bearer. Returns `Some` always
/// (keychain_account never returns empty); the secondary `hernan` read-fallback below stays for the
/// case where a VALID non-hernan USER simply has no entry yet.
fn account() -> Option<String> {
    Some(crate::services::identity::keychain_account())
}

/// F1 smoke: log the resolved `USER` account once, the first time the bearer is requested. Under
/// launchd the `USER` env var can be unset, which silently shifts reads to the legacy account; this
/// one-time line makes that diagnosable in the logs without ever touching the secret value.
fn log_account_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let user = account().unwrap_or_else(|| {
            crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT.to_string()
        });
        tracing::info!(user = %user, "keychain_bearer: resolving aie-internal-bearer for account");
    });
}

/// Reads the entry in-process via `keychain::load`. Tries the current `USER` account, then falls
/// back to the historical `hernan` account (logged). Returns `None` if neither has the entry.
#[cfg(not(test))]
fn fetch_from_keychain() -> Option<String> {
    real_fetch_from_keychain()
}

fn real_fetch_from_keychain() -> Option<String> {
    // 041 FR-006 — the primary account is `identity::keychain_account()` (validated USER, else the
    // legacy account). Defence-in-depth: `keychain::load` already returns None for an empty entry
    // (keychain.rs:26), but we filter again so the fail-closed guarantee ("missing/empty → None") is
    // explicit and local, matching the mock's `Some("") => None` semantics.
    let legacy = crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT;
    let user = account().unwrap_or_else(|| legacy.to_string());
    if let Some(v) =
        crate::services::keychain::load(AIE_BEARER_SERVICE, &user).filter(|v| !v.is_empty())
    {
        return Some(v);
    }
    // Secondary: if the resolved account isn't already the legacy one, try the historical `hernan`
    // entry so el autor's pre-existing bearer keeps working after the multi-user change (logged).
    if user != legacy {
        if let Some(v) =
            crate::services::keychain::load(AIE_BEARER_SERVICE, legacy).filter(|v| !v.is_empty())
        {
            tracing::warn!(
                user = %user,
                "aie-internal-bearer not found for USER account; using legacy 'hernan' account"
            );
            return Some(v);
        }
    }
    None
}

// ── Test hooks ────────────────────────────────────────────────────────────────────────────────
//
// FR-008: tests must not flake against the real, process-global macOS Keychain. `fetch_from_keychain`
// consults an in-process mock first when compiled for test:
//   - MOCK_BEARER = None      → fall through to the real Keychain (used by the e2e integration test).
//   - MOCK_BEARER = Some("")  → simulate NotFound (fail-closed path).
//   - MOCK_BEARER = Some(v)   → return Ok(v).
// MOCK_CALL_COUNT lets a test assert the cache/backoff actually suppressed a Keychain call.
#[cfg(test)]
static MOCK_BEARER: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
#[cfg(test)]
static MOCK_CALL_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
fn fetch_from_keychain() -> Option<String> {
    MOCK_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mock = MOCK_BEARER.lock();
    match mock.as_deref() {
        None => {
            drop(mock);
            real_fetch_from_keychain()
        }
        Some("") => None,
        Some(v) => Some(v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Reset shared state so each test starts clean. `--test-threads=1` (in .cargo/config.toml) is
    /// the second line of defence; this keeps each test independent regardless.
    fn reset(mock: Option<&str>) {
        {
            let mut state = CACHE.write();
            state.entry = None;
            state.last_error_at = None;
        }
        *MOCK_BEARER.lock() = mock.map(|s| s.to_string());
        MOCK_CALL_COUNT.store(0, Ordering::SeqCst);
    }

    #[test]
    fn cache_hit_does_not_refetch() {
        reset(Some("tok-A"));
        assert_eq!(get_bearer(), Some("tok-A".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        // Second call within TTL must serve from cache (no extra fetch).
        assert_eq!(get_bearer(), Some("tok-A".to_string()));
        assert_eq!(get_bearer(), Some("tok-A".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ttl_expiry_triggers_refetch() {
        reset(Some("tok-A"));
        assert_eq!(get_bearer(), Some("tok-A".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        // Force the cached entry to look older than TTL.
        {
            let mut state = CACHE.write();
            if let Some(e) = state.entry.as_mut() {
                e.fetched_at = Instant::now() - (TTL + Duration::from_secs(1));
            }
        }
        // Rotate the underlying value; the expired cache must re-fetch and pick it up.
        *MOCK_BEARER.lock() = Some("tok-B".to_string());
        assert_eq!(get_bearer(), Some("tok-B".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn backoff_short_circuits_without_refetch() {
        reset(Some("")); // NotFound → first get_bearer fetches and fails, arming the backoff.
        assert_eq!(get_bearer(), None);
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        // Within ERROR_BACKOFF: subsequent calls must NOT touch the Keychain (count stays at 1).
        assert_eq!(get_bearer(), None);
        assert_eq!(get_bearer(), None);
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn backoff_expiry_allows_refetch() {
        reset(Some(""));
        assert_eq!(get_bearer(), None);
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        // Age the error past ERROR_BACKOFF.
        {
            let mut state = CACHE.write();
            state.last_error_at = Some(Instant::now() - (ERROR_BACKOFF + Duration::from_secs(1)));
        }
        // The entry now resolves; the next call re-fetches.
        *MOCK_BEARER.lock() = Some("tok-C".to_string());
        assert_eq!(get_bearer(), Some("tok-C".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidate_clears_cache_and_backoff() {
        // 1) Invalidate clears a healthy cached value → forces a re-fetch.
        reset(Some("tok-A"));
        assert_eq!(get_bearer(), Some("tok-A".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        invalidate_bearer_cache();
        *MOCK_BEARER.lock() = Some("tok-rotated".to_string());
        assert_eq!(get_bearer(), Some("tok-rotated".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 2);

        // 2) Invalidate clears the backoff → a call right after re-fetches instead of short-circuiting.
        reset(Some(""));
        assert_eq!(get_bearer(), None);
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 1);
        invalidate_bearer_cache();
        *MOCK_BEARER.lock() = Some("tok-after-backoff".to_string());
        assert_eq!(get_bearer(), Some("tok-after-backoff".to_string()));
        assert_eq!(MOCK_CALL_COUNT.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fail_closed_returns_none() {
        reset(Some(""));
        assert_eq!(get_bearer(), None);
    }

    /// E2e integration against the REAL `aie-internal-bearer` entry. `MOCK_BEARER = None` makes
    /// `fetch_from_keychain` fall through to `real_fetch_from_keychain`. We don't assert presence
    /// (CI/dev Keychains may lack the entry); we assert `get_bearer()` never panics and that a
    /// present value is non-empty. This exercises `account()` + `keychain::load` end-to-end.
    #[test]
    fn integration_real_keychain_e2e() {
        reset(None);
        let got = get_bearer();
        if let Some(v) = got {
            assert!(!v.is_empty(), "a present bearer must be non-empty");
        }
    }
}
