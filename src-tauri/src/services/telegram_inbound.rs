// E / F23 — Local HTTP callback server (inbound from Telegram relay).
// Binds 127.0.0.1:43117 only (NOT 0.0.0.0). HMAC verify constant-time, nonce
// LRU dedup (10min), ts skew check (5min) BEFORE parsing body (panic-safe).
//
// Only starts if endpoints.telegram_relay is configured.
// Cleanup: oneshot::Sender stored in AppState.

use anyhow::Result;
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use lru::LruCache;
use parking_lot::Mutex;
use rusqlite::params;
use serde::Deserialize;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

const BIND_ADDR: &str = "127.0.0.1:43117";
const MAX_SKEW: Duration = Duration::from_secs(300); // 5 min
const NONCE_CACHE_SIZE: usize = 4096;

#[derive(Clone)]
struct ServerState {
    secret: Arc<String>,
    nonces: Arc<Mutex<LruCache<String, ()>>>,
    db: Arc<Mutex<rusqlite::Connection>>,
    audit: crate::bases::audit::AuditWriter,
    app: AppHandle,
    /// 010-furx-signals — PtyManager para ejecutar /reply (pty_write) y /cancel (kill).
    /// Optional: si el inbound arranca sin PTY, los comandos con efecto PTY se rechazan.
    pty: Option<Arc<crate::pty::PtyManager>>,
}

pub struct InboundServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl InboundServer {
    pub fn start(
        app: AppHandle,
        db: Arc<Mutex<rusqlite::Connection>>,
        audit: crate::bases::audit::AuditWriter,
        secret: String,
        pty: Option<Arc<crate::pty::PtyManager>>,
    ) -> Result<Self> {
        let state = ServerState {
            secret: Arc::new(secret),
            nonces: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(NONCE_CACHE_SIZE).unwrap(),
            ))),
            db,
            audit,
            app,
            pty,
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let app = Router::new()
            .route("/furx/v1/callback", post(handle_callback))
            // 010-furx-signals — control remoto (US2). Mismo HMAC/nonce/skew que callback.
            .route("/furx/v1/command", post(handle_command))
            // Codex HIGH: bound payload BEFORE allocating, defense in depth on top of in-handler check.
            .layer(DefaultBodyLimit::max(8192))
            .with_state(state);
        let addr: SocketAddr = BIND_ADDR.parse()?;
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("telegram inbound bind {}: {}", addr, e);
                    return;
                }
            };
            tracing::info!("telegram inbound listening on {}", addr);
            tokio::select! {
                res = axum::serve(listener, app) => {
                    if let Err(e) = res {
                        tracing::warn!("telegram inbound serve error: {}", e);
                    }
                }
                _ = shutdown_rx => {
                    tracing::info!("telegram inbound shutdown signal");
                }
            }
        });
        Ok(Self {
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for InboundServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn handle_callback(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let nonce = headers
        .get("x-furx-nonce")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ts_str = headers
        .get("x-furx-ts")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-furx-sig")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // V5 QA — validate ts BEFORE parsing body to make this panic-safe under
    // malformed JSON floods.
    if nonce.is_empty() || nonce.len() > 128 || sig.len() != 64 || ts_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "missing headers".into()));
    }
    let ts: i64 = ts_str
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad ts".into()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if (now - ts).unsigned_abs() > MAX_SKEW.as_secs() {
        return Err((StatusCode::FORBIDDEN, "ts skew".into()));
    }

    if body.len() > 8192 {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "body > 8KB".into()));
    }

    // Codex HIGH: verify HMAC BEFORE touching the nonce cache. Otherwise an
    // unsigned attacker could burn the nonce and lock out the legitimate caller.
    let expected = crate::services::telegram::sign(&state.secret, &nonce, ts, &body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("sign: {}", e)))?;
    if !subtle_eq(sig.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::FORBIDDEN, "bad sig".into()));
    }

    // Replay protection: only insert AFTER HMAC verifies.
    {
        let mut cache = state.nonces.lock();
        if cache.get(&nonce).is_some() {
            return Err((StatusCode::FORBIDDEN, "replay".into()));
        }
        cache.put(nonce.clone(), ());
    }

    let parsed: CallbackBody = match serde_json::from_str(&body) {
        Ok(b) => b,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "bad json".into())),
    };
    // Codex MED: accept both the short Telegram action codes (per the PLAN_CLOSE
    // contract) AND the internal DB decision strings (legacy). Map both to the
    // canonical DB value.
    let decision = match parsed.action.as_str() {
        "approve" | "approved" => "approved",
        "reject" | "rejected" => "rejected",
        "snooze" | "snoozed" => "snoozed",
        "needs-changes" => "needs-changes",
        _ => return Err((StatusCode::BAD_REQUEST, "invalid action".into())),
    };
    if parsed.card_id.is_empty() || parsed.card_id.len() > 64 {
        return Err((StatusCode::BAD_REQUEST, "invalid card_id".into()));
    }

    let status_col = if decision == "snoozed" {
        "open"
    } else {
        "closed"
    };
    {
        let conn = state.db.lock();
        conn.execute(
            "UPDATE cards SET decision = ?, decided_at = datetime('now'), status = ? WHERE id = ?",
            params![decision, status_col, parsed.card_id],
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {}", e)))?;
    }
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind: "telegram.callback",
            actor: "telegram:relay",
            pane_id: None,
            card_id: Some(&parsed.card_id),
            correlation_id: Some(&nonce),
            payload: serde_json::json!({"action": decision}),
        })
        .ok();
    let _ = state.app.emit(
        "furx:telegram-callback",
        serde_json::json!({
            "card_id": parsed.card_id,
            "action": decision,
        }),
    );
    Ok(Json(serde_json::json!({"ok": true})))
}

fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[derive(Deserialize)]
struct CallbackBody {
    action: String,
    card_id: String,
}

// ── 010-furx-signals — control remoto (US2) ──────────────────────────────────

#[derive(Deserialize)]
struct CommandBody {
    chat_id: String,
    text: String,
}

/// Verifica HMAC + nonce + skew (mismo contrato que handle_callback). Devuelve Ok(()) si
/// el request es auténtico y fresco; Err((status,msg)) en cualquier fallo. Inserta el nonce
/// SÓLO tras verificar la firma (Codex HIGH: no dejar que un atacante queme nonces).
fn verify_signed(
    state: &ServerState,
    headers: &HeaderMap,
    body: &str,
) -> Result<(), (StatusCode, String)> {
    let nonce = headers
        .get("x-furx-nonce")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ts_str = headers
        .get("x-furx-ts")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let sig = headers
        .get("x-furx-sig")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if nonce.is_empty() || nonce.len() > 128 || sig.len() != 64 || ts_str.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "missing headers".into()));
    }
    let ts: i64 = ts_str
        .parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad ts".into()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    if (now - ts).unsigned_abs() > MAX_SKEW.as_secs() {
        return Err((StatusCode::FORBIDDEN, "ts skew".into()));
    }
    if body.len() > 8192 {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "body > 8KB".into()));
    }
    let expected = crate::services::telegram::sign(&state.secret, &nonce, ts, body)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("sign: {}", e)))?;
    if !subtle_eq(sig.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::FORBIDDEN, "bad sig".into()));
    }
    {
        let mut cache = state.nonces.lock();
        if cache.get(&nonce).is_some() {
            return Err((StatusCode::FORBIDDEN, "replay".into()));
        }
        cache.put(nonce, ());
    }
    Ok(())
}

/// Comando remoto entrante. El relay reenvía `{chat_id, text}` firmado. El handler:
/// (1) verifica HMAC/nonce/skew; (2) clasifica (whitelist default-deny); (3) /pair primero
/// (mecanismo de entrada); (4) valida allowlist + tarea + estado; (5) ejecuta (efecto PTY +
/// mutación de estado de 008); (6) audita TODO (recibido/rechazado). Siempre devuelve 200 con
/// un `reply` para que el relay lo muestre al usuario (incluido el rechazo) — un atacante no
/// distingue "no allowlisteado" de error porque igual no ejecuta nada.
async fn handle_command(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    verify_signed(&state, &headers, &body)?;
    let parsed: CommandBody = match serde_json::from_str(&body) {
        Ok(b) => b,
        Err(_) => return Err((StatusCode::BAD_REQUEST, "bad json".into())),
    };
    if parsed.chat_id.is_empty() || parsed.chat_id.len() > 64 || parsed.text.len() > 4096 {
        return Err((StatusCode::BAD_REQUEST, "invalid fields".into()));
    }

    use crate::services::remote_control as rc;
    let db_arc = state.db.clone();

    // (2) clasificar.
    let command = match rc::classify(&parsed.text) {
        Ok(c) => c,
        Err(_) => {
            audit_remote(
                &state,
                &parsed.chat_id,
                "unknown",
                "rejected",
                "comando no reconocido",
            );
            return Ok(Json(serde_json::json!({
                "ok": false,
                "reply": "Comando no reconocido. Disponibles: /status /cancel <task> /reply <task> <texto> /ready <task> /pair <code>",
            })));
        }
    };
    let label = rc::command_label(&command);

    // (3) /pair: mecanismo de entrada — consume el código, agrega a allowlist.
    if let rc::Command::Pair { code } = &command {
        let ok = rc::consume_pair_code(&db_arc, code, &parsed.chat_id).unwrap_or(false);
        let result = if ok { "received" } else { "rejected" };
        audit_remote(
            &state,
            &parsed.chat_id,
            label,
            result,
            if ok {
                "pareado"
            } else {
                "código inválido/expirado"
            },
        );
        let reply = if ok {
            "Pareado ✓. Ya podés mandar comandos.".to_string()
        } else {
            "Código inválido o expirado.".to_string()
        };
        return Ok(Json(serde_json::json!({ "ok": ok, "reply": reply })));
    }

    // (4) validar: allowlist + tarea + estado.
    let validated = match rc::validate(&db_arc, &parsed.chat_id, &command, None) {
        Ok(v) => v,
        Err(e) => {
            let msg = match e {
                rc::ValidationError::NotAllowed => "no autorizado",
                rc::ValidationError::TaskNotFound => "tarea no encontrada",
                rc::ValidationError::NotOwned => "tarea de otro owner",
                rc::ValidationError::InvalidState(_) => "estado inválido para el comando",
            };
            audit_remote(&state, &parsed.chat_id, label, "rejected", msg);
            // No filtrar detalle a chats no autorizados.
            let reply = if matches!(e, rc::ValidationError::NotAllowed) {
                "No autorizado.".to_string()
            } else {
                format!("Rechazado: {}.", msg)
            };
            return Ok(Json(serde_json::json!({ "ok": false, "reply": reply })));
        }
    };

    // (5) ejecutar: mutación de estado (008) + efecto PTY.
    let exec = match rc::execute(&db_arc, &command, &validated) {
        Ok(r) => r,
        Err(e) => {
            audit_remote(
                &state,
                &parsed.chat_id,
                label,
                "rejected",
                &format!("error: {}", e),
            );
            return Ok(Json(
                serde_json::json!({ "ok": false, "reply": format!("Error: {}", e) }),
            ));
        }
    };
    // Efecto PTY (necesita el PtyManager).
    match &exec.effect {
        rc::Effect::None => {}
        rc::Effect::PtyKill { pane_id } => {
            // 015 T014 (US5): /cancel de Telegram rutea por el registro de procesos (SSOT) en
            // vez de matar el PTY directo — así la fila transiciona a `canceled`, audita y emite
            // `TaskChanged` como cualquier otra cancelación de usuario (no es un 3er bypass).
            if let (Some(pty), Some(pid)) = (state.pty.as_ref(), pane_id.as_deref()) {
                if crate::commands::cancel_reap_emit(
                    &db_arc,
                    pty,
                    &state.app,
                    &state.audit,
                    pid,
                    true,
                )
                .is_err()
                {
                    let _ = pty.kill(pid); // sin fila en el registry → kill directo defensivo
                }
            }
        }
        rc::Effect::PtyWrite { pane_id, text } => match state.pty.as_ref() {
            Some(pty) => {
                if let Err(e) = pty.write(pane_id, text.as_bytes()) {
                    audit_remote(
                        &state,
                        &parsed.chat_id,
                        label,
                        "rejected",
                        &format!("pty_write: {}", e),
                    );
                    return Ok(Json(
                        serde_json::json!({ "ok": false, "reply": format!("No se pudo enviar: {}", e) }),
                    ));
                }
                state.pane_state_input(pane_id);
            }
            None => {
                audit_remote(
                    &state,
                    &parsed.chat_id,
                    label,
                    "rejected",
                    "PTY no disponible",
                );
                return Ok(Json(
                    serde_json::json!({ "ok": false, "reply": "PTY no disponible." }),
                ));
            }
        },
    }
    audit_remote(&state, &parsed.chat_id, label, "received", "ejecutado");
    let _ = state.app.emit(
        "furx:remote-command",
        serde_json::json!({
            "command": label,
            "chat_id": parsed.chat_id,
        }),
    );
    Ok(Json(serde_json::json!({ "ok": true, "reply": exec.reply })))
}

impl ServerState {
    /// Marca input en el FSM de panes (no tenemos PaneStateModel acá; best-effort no-op si
    /// el PTY no expone el modelo — el FSM se actualizará por el output del pane igualmente).
    fn pane_state_input(&self, _pane_id: &str) {
        // El PaneStateModel vive en AppState; el inbound no lo tiene. El write al PTY ya
        // genera output que el ticker/heartbeat captura. Intencionalmente no-op.
    }
}

/// Audita un comando remoto. `result` ∈ {received, rejected}. NUNCA loguea el texto libre
/// de /reply (puede contener input sensible al agente) — sólo la etiqueta del comando.
fn audit_remote(state: &ServerState, chat_id: &str, command: &str, result: &str, detail: &str) {
    let kind = if result == "rejected" {
        "remote.command.rejected"
    } else {
        "remote.command.received"
    };
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind,
            actor: "telegram:remote",
            pane_id: None,
            card_id: None,
            correlation_id: Some(chat_id),
            payload: serde_json::json!({"command": command, "result": result, "detail": detail}),
        })
        .ok();
}
