//! Active Persona Pack cache — spec 003 T3.7 ("push pack to Tauri client").
//!
//! When the user applies a Persona Pack in the dashboard, the Worker sets
//! `projects.active_pack_id`. This module makes the desktop client pick that up
//! and inject the pack's distilled system prompt into the next council call.
//!
//! Design (decided by the 6-voice council, 2026-05-28):
//!  - Q1 sync: LAZY fetch-on-run with a 60s in-memory TTL — off the critical path
//!    (the LLM call dominates), no wasted ~2k polls/day, naturally degrades when
//!    signed-out / offline.
//!  - Q2 storage: a global `RwLock<Option<ActivePack>>` singleton (mirrors the
//!    `cloud_uploader` pattern) + persistence in the `settings` KV table so the
//!    pack survives a restart and is available offline.
//!  - Q3 injection: a single `system` message built from `distilled_instructions`
//!    plus up to 3 compact few-shot examples (caller injects it; see council_multi).
//!  - Q5 edge-cases: rollback (active_pack_id → NULL) clears the cache + persisted
//!    row; not-signed-in / no-project / status!=applied → no pack; a network error
//!    NEVER breaks the council — we fall back to the last known pack (or none).
//!
//! BYOK invariant (F-I): the pack content is distilled text from the user's own
//! approved traces, fetched from the user's opted-in Furx project. It is NOT a
//! provider API key and never carries one. Council calls still use BYOK keys
//! client-side; injecting a system prompt does not change that.

use crate::services::cloud_client;
use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// How long an in-memory resolution is trusted before we re-check the cloud.
const TTL: Duration = Duration::from_secs(60);
/// Hard cap on the assembled system message (defense against a huge pack).
const MAX_SYSTEM_CHARS: usize = 6000;
/// Per-example input/response truncation when rendering few-shot examples.
const EXAMPLE_FIELD_CHARS: usize = 400;
const MAX_EXAMPLES: usize = 3;
/// `settings` key used to persist the resolved pack for offline / restart.
const SETTINGS_KEY: &str = "cloud.active_pack";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PackExample {
    #[serde(default)]
    pub trace_id: String,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub response: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActivePack {
    pub pack_id: String,
    #[serde(default)]
    pub version: i64,
    pub system_prompt: String,
    #[serde(default)]
    pub examples: Vec<PackExample>,
}

struct Cache {
    project_id: Option<String>,
    pack: Option<ActivePack>,
    last_fetch: Option<Instant>,
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    project_id: String,
    pack: Option<ActivePack>,
}

fn cache() -> &'static RwLock<Cache> {
    static CACHE: OnceLock<RwLock<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        RwLock::new(Cache {
            project_id: None,
            pack: None,
            last_fetch: None,
        })
    })
}

/// The currently cached pack (if any), regardless of TTL. Used by tests / UI.
pub fn current() -> Option<ActivePack> {
    cache().read().pack.clone()
}

fn store(project_id: &str, pack: Option<ActivePack>) {
    let mut c = cache().write();
    c.project_id = Some(project_id.to_string());
    c.pack = pack;
    c.last_fetch = Some(Instant::now());
}

fn fresh_for(project_id: &str) -> Option<Option<ActivePack>> {
    let c = cache().read();
    if c.project_id.as_deref() == Some(project_id) {
        if let Some(t) = c.last_fetch {
            if t.elapsed() < TTL {
                return Some(c.pack.clone());
            }
        }
    }
    None
}

fn cached_any_for(project_id: &str) -> Option<ActivePack> {
    let c = cache().read();
    if c.project_id.as_deref() == Some(project_id) {
        c.pack.clone()
    } else {
        None
    }
}

// ── Persistence (settings KV table) ─────────────────────────────────────────

fn persist(db: &Arc<Mutex<Connection>>, project_id: &str, pack: Option<&ActivePack>) {
    let value = match serde_json::to_string(&Persisted {
        project_id: project_id.to_string(),
        pack: pack.cloned(),
    }) {
        Ok(v) => v,
        Err(_) => return,
    };
    let conn = db.lock();
    let _ = conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
        params![SETTINGS_KEY, value],
    );
}

fn load_persisted(db: &Arc<Mutex<Connection>>, project_id: &str) -> Option<ActivePack> {
    let conn = db.lock();
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![SETTINGS_KEY],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    drop(conn);
    let parsed: Persisted = serde_json::from_str(&raw?).ok()?;
    if parsed.project_id == project_id {
        parsed.pack
    } else {
        None
    }
}

// ── System-message assembly (Q3) ─────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut idx = max;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    let mut out = s[..idx.min(s.len())].to_string();
    out.push('…');
    out
}

/// Build the system message injected into each council voice.
pub fn build_system_message(p: &ActivePack) -> String {
    let mut s = String::new();
    s.push_str(p.system_prompt.trim());
    let examples: Vec<&PackExample> = p
        .examples
        .iter()
        .filter(|e| !e.input.trim().is_empty() || !e.response.trim().is_empty())
        .take(MAX_EXAMPLES)
        .collect();
    if !examples.is_empty() {
        s.push_str("\n\n# Examples of the preferred response style\n");
        for (i, ex) in examples.iter().enumerate() {
            s.push_str(&format!(
                "\n## Example {}\nUser: {}\nAssistant: {}\n",
                i + 1,
                truncate(ex.input.trim(), EXAMPLE_FIELD_CHARS),
                truncate(ex.response.trim(), EXAMPLE_FIELD_CHARS),
            ));
        }
    }
    truncate(&s, MAX_SYSTEM_CHARS)
}

// ── Resolution (called at council-run time) ──────────────────────────────────

/// Resolve the system message for the active pack of the default project, or
/// `None` when no pack should be applied. NEVER returns an error — a missing
/// session, missing project, or a network failure all degrade to "no pack" or
/// the last cached pack, so the council always runs.
pub async fn resolve_system_message(db: &Arc<Mutex<Connection>>) -> Option<String> {
    let project_id = std::env::var("FURX_DEFAULT_PROJECT_ID")
        .ok()
        .filter(|s| !s.is_empty())?;

    // 1. Serve from the in-memory cache while it is fresh (TTL).
    if let Some(cached) = fresh_for(&project_id) {
        return cached.as_ref().map(build_system_message);
    }

    // 2. Not signed in → cannot hit the cloud. Use the last persisted pack (offline).
    if cloud_client::session_token().is_none() {
        return load_persisted(db, &project_id).map(|p| build_system_message(&p));
    }

    // 3. Fetch the authoritative state from the cloud.
    match cloud_client::get_active_pack(&project_id).await {
        Ok(Some(pack)) => {
            store(&project_id, Some(pack.clone()));
            persist(db, &project_id, Some(&pack));
            Some(build_system_message(&pack))
        }
        Ok(None) => {
            // No active pack / rolled back / not applied → clear cache + persistence.
            store(&project_id, None);
            persist(db, &project_id, None);
            None
        }
        Err(e) => {
            tracing::warn!(
                "active_pack: cloud fetch failed, using cached fallback: {}",
                e
            );
            // Fail-safe: never break the council. Prefer in-memory, then persisted.
            cached_any_for(&project_id)
                .or_else(|| load_persisted(db, &project_id))
                .as_ref()
                .map(build_system_message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(prompt: &str, examples: Vec<PackExample>) -> ActivePack {
        ActivePack {
            pack_id: "p1".into(),
            version: 1,
            system_prompt: prompt.into(),
            examples,
        }
    }

    #[test]
    fn system_message_without_examples_is_just_the_prompt() {
        let p = pack("Be terse and cite sources.", vec![]);
        assert_eq!(build_system_message(&p), "Be terse and cite sources.");
    }

    #[test]
    fn system_message_includes_capped_examples() {
        let exs = vec![
            PackExample {
                trace_id: "t1".into(),
                input: "hi".into(),
                response: "hello".into(),
            },
            PackExample {
                trace_id: "t2".into(),
                input: "a".into(),
                response: "b".into(),
            },
            PackExample {
                trace_id: "t3".into(),
                input: "c".into(),
                response: "d".into(),
            },
            PackExample {
                trace_id: "t4".into(),
                input: "e".into(),
                response: "f".into(),
            },
        ];
        let msg = build_system_message(&pack("System.", exs));
        assert!(msg.contains("# Examples of the preferred response style"));
        assert!(msg.contains("Example 1"));
        assert!(msg.contains("Example 3"));
        // Only MAX_EXAMPLES rendered.
        assert!(!msg.contains("Example 4"));
    }

    #[test]
    fn empty_examples_are_skipped() {
        let exs = vec![PackExample {
            trace_id: "t1".into(),
            input: "  ".into(),
            response: "".into(),
        }];
        let msg = build_system_message(&pack("Only system.", exs));
        assert_eq!(msg, "Only system.");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "áéíóú".repeat(200); // multi-byte
        let t = truncate(&s, 10);
        assert!(t.len() <= 13); // 10 bytes rounded up to boundary + '…'
        assert!(t.ends_with('…'));
    }

    #[test]
    fn system_message_is_capped() {
        let huge = "x".repeat(20_000);
        let msg = build_system_message(&pack(&huge, vec![]));
        assert!(msg.chars().count() <= MAX_SYSTEM_CHARS + 1); // +1 for the ellipsis
    }
}
