// Audit writer helpers. La tabla `events` ya tiene triggers append-only en migration 001.
// Estos helpers garantizan que TODOS los eventos pasen por un solo punto + UUID v4 + correlación.

use crate::services::cloud_uploader::{CloudUploadJob, UploaderHandle};
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

// F25 — auto-snapshot every N "important" events, rate-limited to one per
// 2s so an audit flood (e.g. monitor poll burst) can't produce a snapshot
// storm. See ultra-review V4 SRE finding.
const AUTO_SNAPSHOT_EVERY: u64 = 100;
const AUTO_SNAPSHOT_MIN_GAP: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct AuditWriter {
    db: Arc<Mutex<Connection>>,
    important_counter: Arc<AtomicU64>,
    last_auto_snapshot: Arc<Mutex<Option<Instant>>>,
    /// Optional cloud uploader handle. None until `set_uploader()` is called from lib.rs.
    /// Events matching `is_trace_kind` are enqueued for async upload when present.
    cloud_uploader: Arc<Mutex<Option<UploaderHandle>>>,
}

#[derive(Debug, Serialize)]
pub struct EventInput<'a> {
    pub kind: &'a str,
    pub actor: &'a str,
    pub pane_id: Option<&'a str>,
    pub card_id: Option<&'a str>,
    pub correlation_id: Option<&'a str>,
    pub payload: serde_json::Value,
}

impl AuditWriter {
    pub fn new(db: Arc<Mutex<Connection>>) -> Self {
        Self {
            db,
            important_counter: Arc::new(AtomicU64::new(0)),
            last_auto_snapshot: Arc::new(Mutex::new(None)),
            cloud_uploader: Arc::new(Mutex::new(None)),
        }
    }

    /// Inject the cloud uploader handle. Called once during lib.rs setup after the
    /// uploader task has started. Idempotent (replaces prior handle).
    pub fn set_uploader(&self, h: UploaderHandle) {
        *self.cloud_uploader.lock() = Some(h);
    }

    /// 041 FR-002 — stamp `identity_source` derived from the actor string so every audit event
    /// carries trazabilidad of where the identity came from (cloud session / OS user /
    /// installation_id / system), consistently and without touching the ~70 call-sites' payloads.
    /// A producer that already set `identity_source` explicitly wins (we don't overwrite).
    fn stamp_payload(ev: &EventInput<'_>) -> Result<String> {
        let source = crate::services::identity::source_for_actor(ev.actor);
        let src_val = serde_json::Value::String(source.to_string());
        let s = match &ev.payload {
            serde_json::Value::Object(map) if !map.contains_key("identity_source") => {
                let mut map = map.clone();
                map.insert("identity_source".to_string(), src_val);
                serde_json::to_string(&serde_json::Value::Object(map))?
            }
            // `null` (a common "no extra detail" payload) is promoted to a one-field object so
            // EVERY audit event still carries `identity_source` (FR-002). We do NOT reshape a
            // non-null, non-object payload (string/number/array — none exist among the wired
            // call-sites), nor a payload that already carries the field — those go verbatim.
            serde_json::Value::Null => {
                let mut map = serde_json::Map::new();
                map.insert("identity_source".to_string(), src_val);
                serde_json::to_string(&serde_json::Value::Object(map))?
            }
            other => serde_json::to_string(other)?,
        };
        Ok(s)
    }

    pub fn write(&self, ev: EventInput<'_>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let payload_str = Self::stamp_payload(&ev)?;
        let kind = ev.kind.to_string();
        {
            let conn = self.db.lock();
            conn.execute(
                "INSERT INTO events (id, kind, actor, pane_id, card_id, correlation_id, payload) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    id,
                    ev.kind,
                    ev.actor,
                    ev.pane_id,
                    ev.card_id,
                    ev.correlation_id,
                    payload_str
                ],
            )?;
        }
        self.post_write_effects(&id, &kind, &ev);
        Ok(id)
    }

    /// 044 FR-001 — INSERT del evento sobre una conexión/transacción YA abierta por el caller, sin
    /// re-lockear `self.db` ni disparar efectos colaterales (snapshot/cloud/notify) dentro de la txn.
    /// El caller compone esto con su propia escritura (p.ej. `UPDATE cards`) en `unchecked_transaction`,
    /// de modo que si el audit falla, su cambio se REVIERTE (atomicidad card-write + audit-write).
    /// El append-only de `events` queda intacto: este path SÓLO hace INSERT (los triggers anti
    /// UPDATE/DELETE siguen vigentes). Devuelve el `event_id` para que el caller dispare los efectos
    /// post-commit con `post_write_effects` UNA VEZ que la transacción commiteó.
    ///
    /// IMPORTANTE: `conn` DEBE ser la misma conexión sobre la que el caller abrió la transacción
    /// (en este proceso `self.db` y la `state.db` del caller son el MISMO `Arc<Mutex<Connection>>`,
    /// así que el caller ya tiene el lock tomado; pasar ese `&Connection` evita el doble-lock).
    pub fn write_in_tx(&self, conn: &Connection, ev: &EventInput<'_>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let payload_str = Self::stamp_payload(ev)?;
        conn.execute(
            "INSERT INTO events (id, kind, actor, pane_id, card_id, correlation_id, payload) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                ev.kind,
                ev.actor,
                ev.pane_id,
                ev.card_id,
                ev.correlation_id,
                payload_str
            ],
        )?;
        Ok(id)
    }

    /// Efectos colaterales de un evento ya persistido: auto-snapshot rate-limited, upload a la nube
    /// (sólo trace kinds), y notificación móvil opt-in (council). Idempotentes respecto del INSERT y
    /// seguros de ejecutar DESPUÉS del commit. Separado de la escritura para que `write_in_tx` no los
    /// dispare dentro de una transacción (un snapshot/upload a mitad de txn rompería la atomicidad).
    pub fn post_write_effects(&self, id: &str, kind: &str, ev: &EventInput<'_>) {
        // F25 — auto-snapshot when crossing every Nth important event, rate-limited.
        if is_important_kind(kind) {
            let prev = self.important_counter.fetch_add(1, Ordering::Relaxed);
            if (prev + 1).is_multiple_of(AUTO_SNAPSHOT_EVERY) {
                self.try_auto_snapshot();
            }
        }
        // Sprint #1 — cloud upload hook (council 6/6 option B).
        // Only LLM-trace kinds are uploaded; this is the BYOK-clean filter.
        // The uploader handle is None until lib.rs wires it; until then events
        // remain local-only (zero behaviour change for existing installs).
        if is_trace_kind(kind) {
            if let Some(h) = self.cloud_uploader.lock().as_ref() {
                if let Some(job) = build_trace_job(id, ev) {
                    let _enqueued = h.try_enqueue(job);
                    // try_enqueue increments DROPPED_EVENTS counter internally on backpressure.
                }
            }
        }
        // spec 004 F3: opt-in mobile notification for low-frequency,
        // user-interesting events (council runs). Fire-and-forget to a global
        // bus; the mobile bridge forwards it ONLY if the `audit` toggle is on
        // (default OFF), so this is a no-op for everyone who hasn't enabled it.
        if kind.starts_with("council.") {
            crate::services::mobile_bridge::publish_notification(
                "audit",
                kind,
                ev.actor,
                "info",
                ev.correlation_id.map(|s| s.to_string()),
            );
        }
    }

    fn try_auto_snapshot(&self) {
        // Gemini MED: prevent thread storm — only spawn if previous snapshot done.
        static IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
        let mut guard = self.last_auto_snapshot.lock();
        if let Some(last) = *guard {
            if last.elapsed() < AUTO_SNAPSHOT_MIN_GAP {
                return;
            }
        }
        if IN_FLIGHT
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        *guard = Some(Instant::now());
        drop(guard);
        let db = self.db.clone();
        std::thread::spawn(move || {
            let _ = crate::services::snapshot::write(db, "auto");
            IN_FLIGHT.store(0, Ordering::SeqCst);
        });
    }
}

fn is_important_kind(kind: &str) -> bool {
    kind.starts_with("pty.spawned")
        || kind.starts_with("card.")
        || kind.starts_with("worktree.")
        || kind.starts_with("merge.")
        || kind.starts_with("telegram.")
        || kind.starts_with("reset.")
        || kind.starts_with("boot.")
        || kind.starts_with("voice.")
        || kind.starts_with("council.")
}

/// Kinds that count as an LLM trace and should be uploaded to api.furx.cloud
/// when the uploader is present and the project has opted in to cloud traces.
fn is_trace_kind(kind: &str) -> bool {
    kind.starts_with("llm.")
        || kind.starts_with("council.")
        || kind == "council.voice.completed"
        || kind == "llm.call.completed"
}

/// Map an EventInput into a CloudUploadJob. Returns None if the event doesn't
/// carry the required fields (project_id, model, provider, status) — those events
/// stay local-only.
///
/// Payload extraction strategy: the existing in-flight events stash `prompt` and
/// `response` strings under specific keys in `payload`. Until producers are updated
/// to emit a structured shape, we extract a "best-effort" set. Missing fields default
/// to sensible values (status='success', tokens=0).
fn build_trace_job(local_id: &str, ev: &EventInput<'_>) -> Option<CloudUploadJob> {
    let project_id = ev
        .payload
        .get("project_id")
        .and_then(|v| v.as_str())?
        .to_string();
    let model = ev
        .payload
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let provider = ev
        .payload
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let status = ev
        .payload
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("success")
        .to_string();
    let ts = ev
        .payload
        .get("ts")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let prompt = ev
        .payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(String::from);
    let response = ev
        .payload
        .get("response")
        .and_then(|v| v.as_str())
        .map(String::from);
    let tokens_in = ev
        .payload
        .get("tokens_in")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let tokens_out = ev
        .payload
        .get("tokens_out")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let latency_ms = ev
        .payload
        .get("latency_ms")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let cost_usd_micro = ev
        .payload
        .get("cost_usd_micro")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);
    let error_class = ev
        .payload
        .get("error_class")
        .and_then(|v| v.as_str())
        .map(String::from);
    let replay_id = ev
        .payload
        .get("replay_id")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(CloudUploadJob {
        trace_id: None, // server will assign
        project_id,
        ts,
        model,
        provider,
        tokens_in,
        tokens_out,
        latency_ms,
        cost_usd_micro,
        status,
        error_class,
        prompt,
        response,
        replay_id,
        local_event_id: Some(local_id.to_string()),
        council_parent_trace_id: None,
        council_voice_alias: None,
        council_voice_position: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writer() -> AuditWriter {
        let conn = crate::db::open(std::path::Path::new(":memory:")).expect("db");
        AuditWriter::new(Arc::new(Mutex::new(conn)))
    }

    fn read_payload(w: &AuditWriter, id: &str) -> serde_json::Value {
        let conn = w.db.lock();
        let raw: String = conn
            .query_row("SELECT payload FROM events WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn write_stamps_identity_source_from_actor() {
        let w = writer();
        // OS-style actor (no `@`) → "os".
        let id = w
            .write(EventInput {
                kind: "card.decided",
                actor: "user:ada",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"decision": "accept"}),
            })
            .unwrap();
        let p = read_payload(&w, &id);
        assert_eq!(p["identity_source"], "os");
        assert_eq!(p["decision"], "accept"); // existing fields preserved

        // installation_id actor.
        let id2 = w
            .write(EventInput {
                kind: "card.decided",
                actor: "user:local-abcd1234",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({}),
            })
            .unwrap();
        assert_eq!(read_payload(&w, &id2)["identity_source"], "installation_id");

        // cloud actor (email).
        let id3 = w
            .write(EventInput {
                kind: "card.decided",
                actor: "user:ada@furx.cloud",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({}),
            })
            .unwrap();
        assert_eq!(read_payload(&w, &id3)["identity_source"], "cloud");

        // non-user actor → "system".
        let id4 = w
            .write(EventInput {
                kind: "snapshot.taken",
                actor: "system",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({}),
            })
            .unwrap();
        assert_eq!(read_payload(&w, &id4)["identity_source"], "system");
    }

    #[test]
    fn write_promotes_null_payload_to_carry_identity_source() {
        let w = writer();
        let id = w
            .write(EventInput {
                kind: "watchdog.uninstalled",
                actor: "user:ada",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::Value::Null,
            })
            .unwrap();
        let p = read_payload(&w, &id);
        assert_eq!(p["identity_source"], "os");
        assert!(p.is_object(), "null payload must be promoted to an object");
    }

    // ── 044 FR-001 / SC-001 — atomicidad de card-write + audit-write ──────────────────────────────

    /// Helper: writer sobre una DB con TODAS las migraciones (tabla `cards` + `events` + triggers
    /// append-only). Distinto del `writer()` de arriba (que también usa `db::open` :memory:) — acá lo
    /// hacemos explícito para insertar una card y ejercitar la transacción real.
    fn full_writer() -> AuditWriter {
        let conn = crate::db::open(std::path::Path::new(":memory:")).expect("db");
        AuditWriter::new(Arc::new(Mutex::new(conn)))
    }

    fn insert_open_card(w: &AuditWriter, id: &str) {
        let conn = w.db.lock();
        conn.execute(
            "INSERT INTO cards (id, project, source, title, severity, status, decision) \
             VALUES (?, 'demo', 'monitor', 'algo se cayó', 'critical', 'open', '')",
            params![id],
        )
        .unwrap();
    }

    fn card_decision(w: &AuditWriter, id: &str) -> (String, String) {
        let conn = w.db.lock();
        conn.query_row(
            "SELECT decision, status FROM cards WHERE id = ?",
            params![id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .unwrap()
    }

    fn events_for_card(w: &AuditWriter, card_id: &str) -> i64 {
        let conn = w.db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE card_id = ?",
            params![card_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// El camino feliz: dentro de UNA transacción, UPDATE de la card + `write_in_tx` del audit y
    /// commit → la card queda decidida Y existe exactamente 1 evento `card.decided`.
    #[test]
    fn write_in_tx_commits_card_and_audit_together() {
        let w = full_writer();
        insert_open_card(&w, "card-1");
        {
            let conn = w.db.lock();
            let tx = conn.unchecked_transaction().unwrap();
            tx.execute(
                "UPDATE cards SET decision='approved', status='closed', decided_at=datetime('now') WHERE id=?",
                params!["card-1"],
            )
            .unwrap();
            w.write_in_tx(
                &tx,
                &EventInput {
                    kind: "card.decided",
                    actor: "user:ada",
                    pane_id: None,
                    card_id: Some("card-1"),
                    correlation_id: None,
                    payload: serde_json::json!({"decision": "approved"}),
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(card_decision(&w, "card-1"), ("approved".into(), "closed".into()));
        assert_eq!(events_for_card(&w, "card-1"), 1, "exactamente 1 evento auditado");
    }

    /// SC-001 (el corazón): si el audit-write FALLA dentro de la transacción, el cambio de la card se
    /// REVIERTE. Forzamos la falla con una colisión de PK en `events` (un evento pre-existente con un
    /// id fijo + un INSERT que reutiliza ese id). La card debe quedar EXACTAMENTE como antes (open, sin
    /// decisión) y el evento original intacto (append-only no se tocó).
    #[test]
    fn audit_failure_rolls_back_card_change() {
        let w = full_writer();
        insert_open_card(&w, "card-2");
        // Pre-sembramos un evento con un id fijo para forzar la colisión luego.
        let fixed_event_id = "fixed-event-id-collision";
        {
            let conn = w.db.lock();
            conn.execute(
                "INSERT INTO events (id, kind, actor, payload) VALUES (?, 'seed', 'system', '{}')",
                params![fixed_event_id],
            )
            .unwrap();
        }

        let attempt = || -> Result<(), String> {
            let conn = w.db.lock();
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE cards SET decision='approved', status='closed', decided_at=datetime('now') WHERE id=?",
                params!["card-2"],
            )
            .map_err(|e| e.to_string())?;
            // Audit INSERT que COLISIONA con el PK pre-sembrado → falla → debe abortar la txn entera.
            // (No usamos `write_in_tx` acá porque genera un UUID aleatorio; replicamos su INSERT con
            //  el id fijo para provocar la falla determinística que SC-001 exige verificar.)
            tx.execute(
                "INSERT INTO events (id, kind, actor, card_id, payload) VALUES (?, 'card.decided', 'user:ada', 'card-2', '{}')",
                params![fixed_event_id],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(())
        };
        let result = attempt();

        assert!(result.is_err(), "el audit-write colisionado debe fallar");
        // La card NO cambió: sigue open, sin decisión.
        assert_eq!(
            card_decision(&w, "card-2"),
            ("".into(), "open".into()),
            "el UPDATE de la card debe haberse revertido junto con el audit fallido"
        );
        // No se agregó ningún evento card.decided (sólo queda el seed original).
        assert_eq!(events_for_card(&w, "card-2"), 0, "no quedó audit huérfano");
    }

    /// El append-only de `events` sigue intacto tras el refactor: UPDATE/DELETE siguen abortando.
    #[test]
    fn events_remain_append_only() {
        let w = full_writer();
        let id = w
            .write(EventInput {
                kind: "card.decided",
                actor: "user:ada",
                pane_id: None,
                card_id: Some("card-x"),
                correlation_id: None,
                payload: serde_json::json!({"decision": "approved"}),
            })
            .unwrap();
        let conn = w.db.lock();
        assert!(
            conn.execute("UPDATE events SET kind='tampered' WHERE id=?", params![id])
                .is_err(),
            "UPDATE sobre events debe abortar (append-only)"
        );
        assert!(
            conn.execute("DELETE FROM events WHERE id=?", params![id])
                .is_err(),
            "DELETE sobre events debe abortar (append-only)"
        );
    }

    #[test]
    fn write_does_not_overwrite_explicit_identity_source() {
        let w = writer();
        let id = w
            .write(EventInput {
                kind: "card.decided",
                actor: "user:ada",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"identity_source": "explicit"}),
            })
            .unwrap();
        assert_eq!(read_payload(&w, &id)["identity_source"], "explicit");
    }
}
