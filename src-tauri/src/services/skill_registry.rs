// spec-kit 043 · Ola 4 F2 — registry de confianza local (SQLite).
//
// Sits on the existing `plugins` table (010 + 039 UNIQUE(name) + 049 trust columns).
// Adds the SKILL trust state machine + the durable write discipline the council
// required (WAL + synchronous=FULL around critical writes, `BEGIN IMMEDIATE` with
// jittered exponential retry on SQLITE_BUSY, crash-recovery for `pending_verification`).
//
// Dead-code-first: this module is fully tested in isolation; F3 wires it into the
// live import path. It NEVER changes the legacy `plugins.rs` reconciliation (disk is
// the source of truth there); it only mutates the new trust columns keyed by name.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::time::{Duration, Instant};

use super::skill_manifest::{retry_delay_ms, TrustLevel};

/// Global wall-clock cap for the retry loop (council §3): 10s. Beyond this we give up.
const RETRY_GLOBAL_TIMEOUT: Duration = Duration::from_secs(10);

/// A row of skill trust state (the new 049 columns), keyed by plugin `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillState {
    pub name: String,
    pub trust_level: Option<TrustLevel>,
    pub inert: bool,
    pub pending_verification: bool,
    pub staging_path: Option<String>,
    pub tree_hash: Option<String>,
    pub status_message: Option<String>,
    pub last_verified_at: Option<String>,
}

fn level_to_str(l: TrustLevel) -> &'static str {
    match l {
        TrustLevel::Verified => "verified",
        TrustLevel::SandboxedPromoted => "promoted",
        TrustLevel::Sandboxed => "sandboxed",
        TrustLevel::Rejected => "rejected",
    }
}

fn str_to_level(s: &str) -> Option<TrustLevel> {
    match s {
        "verified" => Some(TrustLevel::Verified),
        "promoted" => Some(TrustLevel::SandboxedPromoted),
        "sandboxed" => Some(TrustLevel::Sandboxed),
        "rejected" => Some(TrustLevel::Rejected),
        _ => None,
    }
}

/// Max TOTAL `BEGIN IMMEDIATE` attempts (council "max 5"). Audit LOW: exactly 5,
/// counting the first try (1 initial + 4 retries = 5), not 6.
const MAX_BEGIN_ATTEMPTS: usize = 5;

/// Run `f` inside a `BEGIN IMMEDIATE` transaction with jittered exponential retry on
/// `SQLITE_BUSY`/`SQLITE_LOCKED`. Council §3: delays `[100,200,400,800,1600]` ±20ms,
/// MAX 5 total attempts, 10s global cap. `synchronous=FULL` is set for the duration so
/// the critical trust write is fully durable, then restored to the connection default.
///
/// WAL is established globally in `db::open` (`journal_mode=WAL`); this helper only
/// escalates `synchronous` per critical section (the global default stays NORMAL).
///
/// Audit fixes:
///   - HIGH: a FAILED `PRAGMA synchronous=FULL` fails CLOSED (we refuse the durable
///     write rather than proceed at a weaker durability level).
///   - MED: `busy_timeout` is forced to 0 for the loop so `BEGIN IMMEDIATE` returns
///     `SQLITE_BUSY` immediately and OUR loop owns the wall-clock cap (a nonzero
///     connection busy_timeout would otherwise let a single BEGIN block past 10s).
///     Restored afterward.
///   - LOW: `synchronous=EXTRA` (3) is preserved on restore (not silently weakened).
///
/// `f` receives the open connection (already inside the IMMEDIATE tx). On `Ok` we
/// COMMIT, on `Err` we ROLLBACK.
pub fn with_durable_immediate<T, F>(conn: &Connection, mut f: F) -> Result<T>
where
    F: FnMut(&Connection) -> Result<T>,
{
    let start = Instant::now();
    let mut rng = rand::thread_rng();

    // Snapshot the connection settings we temporarily change, to restore exactly.
    let prev_sync: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap_or(1);
    let prev_busy: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap_or(0);

    let restore = |conn: &Connection| {
        let kw = match prev_sync {
            0 => "OFF",
            2 => "FULL",
            3 => "EXTRA",
            _ => "NORMAL",
        };
        let _ = conn.pragma_update(None, "synchronous", kw);
        let _ = conn.busy_timeout(Duration::from_millis(prev_busy.max(0) as u64));
    };

    // ⟨audit HIGH⟩ Fail closed if we cannot escalate durability.
    if let Err(e) = conn.pragma_update(None, "synchronous", "FULL") {
        restore(conn);
        return Err(anyhow!("cannot set synchronous=FULL (durability gate): {e}"));
    }
    // Verify it actually took (some builds/locks can no-op a PRAGMA silently).
    let now_sync: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap_or(-1);
    if now_sync != 2 {
        restore(conn);
        return Err(anyhow!(
            "synchronous=FULL did not take (got {now_sync}) — refusing durable write"
        ));
    }
    // ⟨audit MED⟩ Own the wall-clock: make BEGIN return BUSY immediately. ⟨audit r2⟩
    // Fail closed if we can't — otherwise a single BEGIN could block past the 10s cap.
    if let Err(e) = conn.busy_timeout(Duration::from_millis(0)) {
        restore(conn);
        return Err(anyhow!("cannot set busy_timeout=0 for retry loop: {e}"));
    }

    let mut attempt = 0usize;
    loop {
        match conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => {}
            Err(e) if is_busy(&e) && can_retry(attempt, start) => {
                sleep_retry(attempt, &mut rng);
                attempt += 1;
                continue;
            }
            Err(e) => {
                restore(conn);
                return Err(anyhow!("BEGIN IMMEDIATE failed: {e}"));
            }
        }
        // We hold the write lock. Run the body.
        match f(conn) {
            Ok(v) => {
                if let Err(e) = conn.execute_batch("COMMIT") {
                    let _ = conn.execute_batch("ROLLBACK");
                    restore(conn);
                    return Err(anyhow!("COMMIT failed: {e}"));
                }
                restore(conn);
                return Ok(v);
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                restore(conn);
                return Err(e);
            }
        }
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked,
                ..
            },
            _
        )
    )
}

fn can_retry(attempt: usize, start: Instant) -> bool {
    // `attempt` is the number of BEGINs already made (0 before the first). Allow up to
    // MAX_BEGIN_ATTEMPTS TOTAL → retry while attempt+1 < MAX (so the loop makes at most
    // MAX BEGINs). Also bounded by the 10s wall-clock cap.
    attempt + 1 < MAX_BEGIN_ATTEMPTS && start.elapsed() < RETRY_GLOBAL_TIMEOUT
}

fn sleep_retry<R: rand::Rng>(attempt: usize, rng: &mut R) {
    let ms = retry_delay_ms(attempt, rng);
    std::thread::sleep(Duration::from_millis(ms));
}

/// Read the trust state row for `name`. `None` if there's no row at all.
pub fn get_state(conn: &Connection, name: &str) -> Result<Option<SkillState>> {
    let row = conn
        .query_row(
            "SELECT name, trust_level, inert, pending_verification, staging_path, \
                    tree_hash, status_message, last_verified_at \
             FROM plugins WHERE name = ?",
            params![name],
            |r| {
                Ok(SkillState {
                    name: r.get(0)?,
                    trust_level: r
                        .get::<_, Option<String>>(1)?
                        .as_deref()
                        .and_then(str_to_level),
                    inert: r.get::<_, i64>(2)? != 0,
                    pending_verification: r.get::<_, i64>(3)? != 0,
                    staging_path: r.get(4)?,
                    tree_hash: r.get(5)?,
                    status_message: r.get(6)?,
                    last_verified_at: r.get(7)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Step 4b of the import flow: begin an install. INSERTs the row in the PENDING state:
/// `inert=1`, `pending_verification=1`, `staging_path` set, no trust_level yet.
/// Durable + atomic.
///
/// CONCURRENCY CONTRACT (audit codex HIGH): replacing a leftover PENDING row is the
/// intended crash-recovery semantics (a previous install died mid-flight). It is SAFE
/// because F3's import flow holds an exclusive `flock(~/.furx/.import.lock)` for the
/// ENTIRE add — so there is never a SECOND live installer racing the same name. The
/// `BEGIN IMMEDIATE` here serializes the DB write; the flock serializes the installer.
/// A COMPLETED (non-pending) row blocks re-install (install-only).
///
/// Returns Err if a NON-pending row for `name` already exists (a completed install).
pub fn begin_install(
    conn: &Connection,
    name: &str,
    staging_path: &str,
    tree_hash: &str,
) -> Result<()> {
    with_durable_immediate(conn, |c| {
        // Install-only at the DB layer: a completed (non-pending) row blocks re-install.
        let existing: Option<(i64, i64)> = c
            .query_row(
                "SELECT pending_verification, inert FROM plugins WHERE name = ?",
                params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((pending, _)) = existing {
            if pending == 0 {
                return Err(anyhow!(
                    "Skill already installed. Use furx skill remove '{name}' first."
                ));
            }
            // A leftover PENDING row (crash mid-install) is allowed to be replaced.
        }
        let id = uuid::Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO plugins (id, name, version, enabled, manifest_json, \
                                  inert, pending_verification, staging_path, tree_hash) \
             VALUES (?, ?, '', 1, '{}', 1, 1, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET \
                inert=1, pending_verification=1, staging_path=excluded.staging_path, \
                tree_hash=excluded.tree_hash",
            params![id, name, staging_path, tree_hash],
        )?;
        Ok(())
    })
}

/// Step 4d: finalize an install after the atomic rename succeeded. Clears
/// `pending_verification`/`staging_path`, sets the resolved `trust_level`, `inert`
/// per the level, and stamps `last_verified_at`. Durable + atomic.
///
/// ⟨audit MED⟩ Guards on `pending_verification=1`: a finalize only ever transitions a
/// row that is STILL pending. If another path already finalized/replaced it between the
/// recovery sweep's SELECT and here, the UPDATE matches 0 rows and we error rather than
/// clobber a completed install with a stale verdict (lost-update protection — the
/// IMMEDIATE tx serializes, this WHERE makes it idempotent against double-finalize).
pub fn finalize_install(conn: &Connection, name: &str, level: TrustLevel) -> Result<()> {
    let now = now_rfc3339();
    with_durable_immediate(conn, |c| {
        let n = c.execute(
            "UPDATE plugins SET \
                pending_verification=0, staging_path=NULL, \
                trust_level=?, inert=?, last_verified_at=?, status_message=NULL \
             WHERE name=? AND pending_verification=1",
            params![level_to_str(level), level.inert(), now, name],
        )?;
        if n == 0 {
            return Err(anyhow!(
                "finalize_install: no PENDING row for '{name}' (already finalized or gone)"
            ));
        }
        Ok(())
    })
}

/// Promote a `Sandboxed` skill to `SandboxedPromoted` (user explicitly trusts the
/// source and signed the tree with their LOCAL_USER_KEY). Only valid from Sandboxed.
/// Makes scripts executable (`inert=0`). Durable + atomic.
pub fn promote(conn: &Connection, name: &str) -> Result<()> {
    let now = now_rfc3339();
    with_durable_immediate(conn, |c| {
        let cur: Option<String> = c
            .query_row(
                "SELECT trust_level FROM plugins WHERE name=?",
                params![name],
                |r| r.get(0),
            )
            .optional()?;
        match cur.as_deref().and_then(str_to_level) {
            Some(TrustLevel::Sandboxed) => {}
            Some(other) => {
                return Err(anyhow!(
                    "cannot promote '{name}': only Sandboxed skills can be promoted (is {})",
                    level_to_str(other)
                ))
            }
            None => return Err(anyhow!("cannot promote '{name}': no trust_level")),
        }
        c.execute(
            "UPDATE plugins SET trust_level='promoted', inert=0, last_verified_at=?, \
                    status_message='promoted by user' WHERE name=?",
            params![now, name],
        )?;
        Ok(())
    })
}

/// Mark a skill inert with a status message (e.g. re-verify mismatch, revocation, or a
/// `SandboxedPromoted` skill degraded back to `Sandboxed` by an update). Durable.
///
/// `clear_pending`: when `true`, also clears `pending_verification`/`staging_path`.
/// ⟨audit MED⟩ A TERMINAL recovery failure must clear pending so the startup sweep does
/// not retry it forever. A live re-verify mismatch on an already-finalized row passes
/// `false` (the row was never pending).
pub fn mark_inert(
    conn: &Connection,
    name: &str,
    level: TrustLevel,
    status: &str,
    clear_pending: bool,
) -> Result<()> {
    with_durable_immediate(conn, |c| {
        if clear_pending {
            c.execute(
                "UPDATE plugins SET trust_level=?, inert=1, status_message=?, \
                        pending_verification=0, staging_path=NULL WHERE name=?",
                params![level_to_str(level), status, name],
            )?;
        } else {
            c.execute(
                "UPDATE plugins SET trust_level=?, inert=1, status_message=? WHERE name=?",
                params![level_to_str(level), status, name],
            )?;
        }
        Ok(())
    })
}

/// Crash-recovery sweep at startup. For each `pending_verification=1` row, decide its
/// fate. The actual on-disk re-verification (re-hash + gate) is performed by a caller-
/// supplied `verify` closure (so this module stays decoupled from the FS/gate). The
/// closure returns the resolved `TrustLevel` if the skill exists + verifies, or `None`
/// if the plugin dir is gone (orphan → delete the row).
///
/// `max_attempts` bounds crash-recovery retries (council: 3) per row; a verify that
/// keeps erroring leaves the row `inert=1, status='recovery_failed'`.
pub fn recover_pending<F>(conn: &Connection, max_attempts: usize, mut verify: F) -> Result<()>
where
    F: FnMut(&str, Option<&str>) -> Result<Option<TrustLevel>>,
{
    let pending: Vec<(String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT name, staging_path FROM plugins WHERE pending_verification = 1",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    for (name, staging) in pending {
        let mut last_err: Option<anyhow::Error> = None;
        let mut resolved: Option<Option<TrustLevel>> = None;
        for _ in 0..max_attempts.max(1) {
            match verify(&name, staging.as_deref()) {
                Ok(r) => {
                    resolved = Some(r);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        match resolved {
            Some(Some(level)) => {
                // Re-verified: finalize.
                finalize_install(conn, &name, level)?;
            }
            Some(None) => {
                // Orphan: plugin dir gone and no staging → delete the row.
                with_durable_immediate(conn, |c| {
                    c.execute("DELETE FROM plugins WHERE name=?", params![name])?;
                    Ok(())
                })?;
                tracing::warn!("skill recovery: removed orphan row '{name}'");
            }
            None => {
                // Verify kept failing → leave inert, flagged for the UI.
                let msg = format!(
                    "recovery_failed: {}",
                    last_err
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unknown".into())
                );
                // Terminal: clear pending so the next startup sweep does not re-pick it.
                mark_inert(conn, &name, TrustLevel::Rejected, &msg, true)?;
                tracing::warn!("skill recovery: '{name}' failed after retries → inert (terminal)");
            }
        }
    }
    Ok(())
}

/// Delete a row ONLY if it is still `pending_verification=1` (an interrupted install).
/// Used by F3's inline rollback after a failed publish. The `pending=1` guard makes it
/// safe: it can never remove a COMPLETED install (e.g. a pre-existing skill of the same
/// name). Returns the number of rows deleted (0 or 1). Durable + atomic.
pub fn delete_pending_row(conn: &Connection, name: &str) -> Result<usize> {
    with_durable_immediate(conn, |c| {
        let n = c.execute(
            "DELETE FROM plugins WHERE name=? AND pending_verification=1",
            params![name],
        )?;
        Ok(n)
    })
}

/// Run-time enforcement (the REAL close to the install-window TOCTOU): before executing
/// an executable skill's scripts, re-hash the live `scripts_dir` and compare to the
/// `recorded_tree_hash` from the row. On mismatch → mark the skill `Rejected`/inert and
/// return `false` (caller must NOT execute). On match → `true`. A skill that is already
/// inert/sandboxed/rejected returns `false` without hashing (never executes).
///
/// This is what makes a post-install byte-swap harmless: every spawn re-verifies, so a
/// stale install hash is never trusted at run time.
pub fn reverify_or_inert(
    conn: &Connection,
    name: &str,
    scripts_dir: &std::path::Path,
) -> Result<bool> {
    let st = match get_state(conn, name)? {
        Some(s) => s,
        None => return Ok(false),
    };
    // Only executable levels are candidates; anything else never runs.
    match st.trust_level {
        Some(l) if l.may_execute() => {}
        _ => return Ok(false),
    }
    let recorded = match st.tree_hash {
        Some(h) => h,
        None => return Ok(false), // no recorded hash → cannot verify → don't execute
    };
    let actual = super::skill_manifest::tree_hash(scripts_dir).unwrap_or_default();
    if actual.eq_ignore_ascii_case(&recorded) {
        Ok(true)
    } else {
        let msg = format!("tree_hash mismatch at spawn: expected {recorded}, got {actual}");
        mark_inert(conn, name, TrustLevel::Rejected, &msg, false)?;
        Ok(false)
    }
}

/// `true` iff `name`'s current trust_level is one that MAY execute (Verified or
/// SandboxedPromoted) AND it is not inert. Used by the F2 fast-path to gate whether the
/// snapshot short-circuit is even allowed (a non-executable skill must never fast-path to
/// ok=true). Read-only; no mutation.
pub fn is_executable_level(conn: &Connection, name: &str) -> Result<bool> {
    match get_state(conn, name)? {
        Some(s) => Ok(!s.inert && matches!(s.trust_level, Some(l) if l.may_execute())),
        None => Ok(false),
    }
}

fn now_rfc3339() -> String {
    // RFC3339 UTC, second precision (matches the rest of the schema's datetime('now')).
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Minimal RFC3339 from epoch seconds without chrono (avoid a new dep).
    let days = secs / 86400;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Howard Hinnant's civil_from_days (days since 1970-01-01 → (y,m,d)).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // WAL is not available for :memory: but synchronous toggling still works.
        conn.execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/049_skill_trust.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn migration_049_adds_trust_columns() {
        let conn = test_conn();
        // All new columns must exist and default sanely.
        begin_install(&conn, "council", ".tmp_abc", "deadbeef").unwrap();
        let st = get_state(&conn, "council").unwrap().unwrap();
        assert!(st.pending_verification);
        assert!(st.inert, "pending install is inert");
        assert_eq!(st.staging_path.as_deref(), Some(".tmp_abc"));
        assert_eq!(st.tree_hash.as_deref(), Some("deadbeef"));
        assert!(st.trust_level.is_none());
    }

    #[test]
    fn install_lifecycle_pending_then_verified() {
        let conn = test_conn();
        begin_install(&conn, "council", ".tmp_1", "aa").unwrap();
        finalize_install(&conn, "council", TrustLevel::Verified).unwrap();
        let st = get_state(&conn, "council").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::Verified));
        assert!(!st.inert, "verified executes");
        assert!(!st.pending_verification);
        assert!(st.staging_path.is_none());
        assert!(st.last_verified_at.is_some());
    }

    #[test]
    fn sandboxed_finalize_is_inert() {
        let conn = test_conn();
        begin_install(&conn, "wild", ".tmp_2", "bb").unwrap();
        finalize_install(&conn, "wild", TrustLevel::Sandboxed).unwrap();
        let st = get_state(&conn, "wild").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::Sandboxed));
        assert!(st.inert, "sandboxed scripts are inert");
    }

    #[test]
    fn install_only_blocks_reinstall_of_completed_skill() {
        // SC-003 (DB layer): a completed install blocks begin_install for the same name.
        let conn = test_conn();
        begin_install(&conn, "council", ".tmp_a", "aa").unwrap();
        finalize_install(&conn, "council", TrustLevel::Verified).unwrap();
        let err = begin_install(&conn, "council", ".tmp_b", "bb").unwrap_err();
        assert!(
            err.to_string().contains("already installed"),
            "got: {err}"
        );
        // The original row is untouched.
        let st = get_state(&conn, "council").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::Verified));
        assert_eq!(st.tree_hash.as_deref(), Some("aa"));
    }

    #[test]
    fn leftover_pending_row_can_be_replaced() {
        // A crash mid-install leaves pending=1; a fresh begin_install may replace it.
        let conn = test_conn();
        begin_install(&conn, "council", ".tmp_old", "old").unwrap();
        // Still pending → replace allowed.
        begin_install(&conn, "council", ".tmp_new", "new").unwrap();
        let st = get_state(&conn, "council").unwrap().unwrap();
        assert_eq!(st.staging_path.as_deref(), Some(".tmp_new"));
        assert_eq!(st.tree_hash.as_deref(), Some("new"));
    }

    #[test]
    fn promote_only_from_sandboxed() {
        let conn = test_conn();
        begin_install(&conn, "s", ".t", "h").unwrap();
        finalize_install(&conn, "s", TrustLevel::Sandboxed).unwrap();
        promote(&conn, "s").unwrap();
        let st = get_state(&conn, "s").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::SandboxedPromoted));
        assert!(!st.inert, "promoted skill executes");
        // Promoting a Verified skill is a no-op error (already executes).
        begin_install(&conn, "v", ".t2", "h2").unwrap();
        finalize_install(&conn, "v", TrustLevel::Verified).unwrap();
        assert!(promote(&conn, "v").is_err(), "only Sandboxed can be promoted");
    }

    #[test]
    fn recover_finalizes_pending_that_reverifies() {
        let conn = test_conn();
        begin_install(&conn, "council", ".tmp_r", "aa").unwrap();
        // verify closure: the skill exists and re-verifies as Verified.
        recover_pending(&conn, 3, |name, staging| {
            assert_eq!(name, "council");
            assert_eq!(staging, Some(".tmp_r"));
            Ok(Some(TrustLevel::Verified))
        })
        .unwrap();
        let st = get_state(&conn, "council").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::Verified));
        assert!(!st.pending_verification);
    }

    #[test]
    fn recover_deletes_orphan_pending() {
        let conn = test_conn();
        begin_install(&conn, "gone", ".tmp_g", "aa").unwrap();
        // verify returns None → plugin dir vanished → orphan → row deleted.
        recover_pending(&conn, 3, |_, _| Ok(None)).unwrap();
        assert!(get_state(&conn, "gone").unwrap().is_none(), "orphan row removed");
    }

    #[test]
    fn recover_marks_inert_after_repeated_failure() {
        let conn = test_conn();
        begin_install(&conn, "bad", ".tmp_x", "aa").unwrap();
        let mut calls = 0;
        recover_pending(&conn, 3, |_, _| {
            calls += 1;
            Err(anyhow!("disk error"))
        })
        .unwrap();
        assert_eq!(calls, 3, "retried max_attempts times");
        let st = get_state(&conn, "bad").unwrap().unwrap();
        assert!(st.inert);
        assert_eq!(st.trust_level, Some(TrustLevel::Rejected));
        assert!(st.status_message.as_deref().unwrap().contains("recovery_failed"));
        // ⟨audit MED⟩ terminal failure clears pending so the next sweep won't re-pick it.
        assert!(
            !st.pending_verification,
            "terminal recovery failure must clear pending_verification"
        );
        assert!(st.staging_path.is_none(), "staging cleared on terminal failure");
        // A second sweep sees nothing pending → does not call verify again.
        let mut calls2 = 0;
        recover_pending(&conn, 3, |_, _| {
            calls2 += 1;
            Err(anyhow!("x"))
        })
        .unwrap();
        assert_eq!(calls2, 0, "no pending rows left to recover");
    }

    #[test]
    fn finalize_is_idempotent_against_double_call() {
        // ⟨audit MED⟩ finalize guards on pending=1: a second finalize errors (no pending
        // row), it does NOT clobber the first verdict.
        let conn = test_conn();
        begin_install(&conn, "c", ".t", "h").unwrap();
        finalize_install(&conn, "c", TrustLevel::Verified).unwrap();
        let err = finalize_install(&conn, "c", TrustLevel::Sandboxed).unwrap_err();
        assert!(err.to_string().contains("no PENDING row"), "got: {err}");
        let st = get_state(&conn, "c").unwrap().unwrap();
        assert_eq!(st.trust_level, Some(TrustLevel::Verified), "verdict unchanged");
    }

    #[test]
    fn retry_caps_at_five_total_attempts() {
        // ⟨audit LOW⟩ exactly 5 total BEGIN attempts: with a perpetually-held lock and a
        // 0 busy_timeout, the loop must give up after MAX_BEGIN_ATTEMPTS and return BUSY,
        // bounded well under the 10s cap (sum of base delays = 100+200+400+800 = 1.5s for
        // 4 retries → 5th attempt fails fast).
        let dir = std::env::temp_dir().join(format!("furx-cap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let setup = Connection::open(&path).unwrap();
        setup.pragma_update(None, "journal_mode", "WAL").unwrap();
        setup
            .execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        setup
            .execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        setup
            .execute_batch(include_str!("../../migrations/049_skill_trust.sql"))
            .unwrap();
        drop(setup);

        let holder = Connection::open(&path).unwrap();
        holder.busy_timeout(Duration::from_millis(0)).unwrap();
        let worker = Connection::open(&path).unwrap();
        worker.busy_timeout(Duration::from_millis(0)).unwrap();

        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        holder
            .execute(
                "INSERT INTO plugins (id,name,version,enabled,manifest_json) VALUES ('h','x','1',1,'{}')",
                [],
            )
            .unwrap();

        let start = Instant::now();
        let r = begin_install(&worker, "council", ".t", "h");
        let elapsed = start.elapsed();
        assert!(r.is_err(), "lock never released → must give up");
        assert!(
            r.unwrap_err().to_string().contains("BEGIN IMMEDIATE failed"),
            "should surface a BUSY-exhausted error"
        );
        assert!(elapsed < RETRY_GLOBAL_TIMEOUT, "must give up well under the 10s cap");
        holder.execute_batch("COMMIT").unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn durable_immediate_restores_synchronous() {
        let conn = test_conn();
        conn.pragma_update(None, "synchronous", "NORMAL").unwrap();
        with_durable_immediate(&conn, |c| {
            let s: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
            assert_eq!(s, 2, "FULL (2) inside the critical section");
            Ok(())
        })
        .unwrap();
        let after: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, 1, "restored to NORMAL after the tx");
    }

    #[test]
    fn durable_immediate_rolls_back_on_error() {
        let conn = test_conn();
        let r: Result<()> = with_durable_immediate(&conn, |c| {
            c.execute(
                "INSERT INTO plugins (id,name,version,enabled,manifest_json,inert,pending_verification) \
                 VALUES ('x','rollme','1',1,'{}',1,1)",
                [],
            )?;
            Err(anyhow!("boom"))
        });
        assert!(r.is_err());
        assert!(
            get_state(&conn, "rollme").unwrap().is_none(),
            "failed tx must roll back the insert"
        );
    }

    #[test]
    fn retry_loop_succeeds_under_contention() {
        // Two connections to the SAME file DB: hold a write lock on one, run
        // with_durable_immediate on the other → it must retry then succeed once the
        // first releases. Proves the BUSY retry path (not just the happy path).
        let dir = std::env::temp_dir().join(format!("furx-reg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        let setup = Connection::open(&path).unwrap();
        setup.pragma_update(None, "journal_mode", "WAL").unwrap();
        setup
            .execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        setup
            .execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        setup
            .execute_batch(include_str!("../../migrations/049_skill_trust.sql"))
            .unwrap();
        drop(setup);

        let c_writer = Connection::open(&path).unwrap();
        c_writer.busy_timeout(Duration::from_millis(0)).unwrap();
        let c_worker = Connection::open(&path).unwrap();
        c_worker.busy_timeout(Duration::from_millis(0)).unwrap();

        // Hold an exclusive write lock on c_writer.
        c_writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        c_writer
            .execute(
                "INSERT INTO plugins (id,name,version,enabled,manifest_json) VALUES ('w','holder','1',1,'{}')",
                [],
            )
            .unwrap();

        // Run the worker call in a thread; it must hit SQLITE_BUSY and start retrying.
        let worker = std::thread::spawn(move || {
            begin_install(&c_worker, "council", ".tmp_c", "aa")
        });
        // Give the worker time to hit BUSY and enter the retry loop, then release.
        std::thread::sleep(Duration::from_millis(150));
        c_writer.execute_batch("COMMIT").unwrap();

        let res = worker.join().unwrap();
        assert!(res.is_ok(), "worker must eventually acquire the lock: {res:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
