// services/remote_control.rs — 010-furx-signals · control remoto (US2).
//
// Comandos entrantes desde Telegram/teléfono, LIMITADOS a un set seguro (FR-006):
//   /status            — resumen de tareas
//   /cancel <task>     — pasa una tarea a canceled (mata su proceso)
//   /reply <task> <txt>— pty_write del texto al pane de la tarea (input a un agente)
//   /ready <task>      — marca una tarea awaiting_review
//   /pair <code>       — agrega el chat_id a la allowlist (challenge de un uso)
//
// NADA de shell arbitrario, edición directa ni "approve all" (Constitución VI: parar ante
// destructivo; lo destructivo dentro del agente requiere confirmación LOCAL, no por Telegram).
//
// Seguridad (FR-007): cada comando se valida — (a) origen en allowlist (o /pair válido),
// (b) task existe y pertenece al owner (project_key, 007), (c) estado de tarea válido para
// el comando. Todo comando (aceptado o rechazado) se audita.

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection};
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

const PAIR_CODE_TTL_MINS: i64 = 10;

/// Comando ya parseado y validado sintácticamente. Variantes = whitelist cerrada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Cancel { task: String },
    Reply { task: String, text: String },
    Ready { task: String },
    Pair { code: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    NotACommand,    // no empieza con '/'
    UnknownCommand, // comando fuera de la whitelist (incl. shell-ish)
    MissingArgs,    // faltan argumentos requeridos
}

/// Classifier: parsea un mensaje de texto a un `Command`. Default-deny: cualquier cosa que
/// no matchee EXACTAMENTE la whitelist se rechaza (NotACommand/UnknownCommand). No interpreta
/// shell, no concatena, no ejecuta nada acá.
pub fn classify(raw: &str) -> std::result::Result<Command, ParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    if !trimmed.starts_with('/') {
        return Err(ParseError::NotACommand);
    }
    // Primer token = comando; el resto = args. split_whitespace colapsa espacios.
    let mut it = trimmed.splitn(2, char::is_whitespace);
    let cmd = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();
    // Soportar el sufijo @botname que Telegram agrega en grupos (/cancel@FurxBot).
    let cmd = cmd.split('@').next().unwrap_or(cmd);
    match cmd {
        "/status" => Ok(Command::Status),
        "/cancel" => {
            let task = first_word(rest).ok_or(ParseError::MissingArgs)?;
            Ok(Command::Cancel { task })
        }
        "/ready" => {
            let task = first_word(rest).ok_or(ParseError::MissingArgs)?;
            Ok(Command::Ready { task })
        }
        "/pair" => {
            let code = first_word(rest).ok_or(ParseError::MissingArgs)?;
            Ok(Command::Pair { code })
        }
        "/reply" => {
            // /reply <task> <texto libre...>
            let mut parts = rest.splitn(2, char::is_whitespace);
            let task = parts
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let text = parts
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            match (task, text) {
                (Some(task), Some(text)) => Ok(Command::Reply { task, text }),
                _ => Err(ParseError::MissingArgs),
            }
        }
        _ => Err(ParseError::UnknownCommand),
    }
}

fn first_word(s: &str) -> Option<String> {
    s.split_whitespace()
        .next()
        .map(|w| w.to_string())
        .filter(|w| !w.is_empty())
}

// ── Allowlist (FR-007a) ──────────────────────────────────────────────────────

pub fn is_allowed(conn: &Connection, chat_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM signal_remote_allowlist WHERE chat_id = ?1",
        params![chat_id],
        |_| Ok(()),
    )
    .is_ok()
}

pub fn add_to_allowlist(db: &Db, chat_id: &str, label: Option<&str>, via: &str) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "INSERT INTO signal_remote_allowlist (chat_id, label, paired_via)
         VALUES (?1,?2,?3)
         ON CONFLICT(chat_id) DO UPDATE SET label=COALESCE(excluded.label, label)",
        params![chat_id, label, via],
    )?;
    Ok(())
}

pub fn remove_from_allowlist(db: &Db, chat_id: &str) -> Result<bool> {
    let conn = db.lock();
    let n = conn.execute(
        "DELETE FROM signal_remote_allowlist WHERE chat_id = ?1",
        params![chat_id],
    )?;
    Ok(n > 0)
}

// ── Pair codes (challenge local de un uso) ────────────────────────────────────

/// Genera un código de pairing de un uso, válido PAIR_CODE_TTL_MINS minutos. Lo muestra la UI
/// (Settings); el usuario lo manda por Telegram con `/pair <code>`.
pub fn create_pair_code(db: &Db) -> Result<String> {
    // Audit codex MED: 8 hex (~32 bits) era poco. Usamos el uuid completo (122 bits) →
    // brute-force inviable aunque /pair no requiera allowlist y el código viva 10 min.
    let code: String = Uuid::new_v4().simple().to_string();
    let now = Utc::now();
    let expires = (now + ChronoDuration::minutes(PAIR_CODE_TTL_MINS)).to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO signal_pair_codes (code, expires_at) VALUES (?1,?2)",
        params![code, expires],
    )?;
    Ok(code)
}

/// Consume un código de pairing: si es válido (existe, no usado, no expirado) lo marca usado
/// por `chat_id` y agrega el chat a la allowlist. Devuelve Ok(true) si pareó, Ok(false) si
/// el código es inválido/expirado/usado.
pub fn consume_pair_code(db: &Db, code: &str, chat_id: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // Marca usado SÓLO si está libre y no expirado (atómico, evita doble uso).
    let n = conn.execute(
        "UPDATE signal_pair_codes SET used_at = ?2, used_by = ?3
         WHERE code = ?1 AND used_at IS NULL AND expires_at > ?2",
        params![code, now, chat_id],
    )?;
    if n == 0 {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO signal_remote_allowlist (chat_id, paired_via)
         VALUES (?1,'pair')
         ON CONFLICT(chat_id) DO NOTHING",
        params![chat_id],
    )?;
    Ok(true)
}

// ── Validación de tarea (FR-007 b/c) ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    NotAllowed, // chat_id no en allowlist
    TaskNotFound,
    NotOwned,             // task no pertenece al owner esperado
    InvalidState(String), // estado de tarea no admite el comando
}

/// ¿El comando es legal contra el estado actual de la tarea?
/// - /cancel: pending|running|awaiting_review (no done/failed/canceled).
/// - /ready: running (running→awaiting_review, alineado con can_transition de 008).
/// - /reply: running (mandar input a un agente vivo).
pub fn state_allows(command: &Command, task_state: &str) -> bool {
    match command {
        Command::Cancel { .. } => matches!(task_state, "pending" | "running" | "awaiting_review"),
        Command::Ready { .. } => task_state == "running",
        Command::Reply { .. } => task_state == "running",
        Command::Status | Command::Pair { .. } => true,
    }
}

/// El task_id objetivo del comando (None para Status/Pair).
pub fn target_task(command: &Command) -> Option<&str> {
    match command {
        Command::Cancel { task } | Command::Ready { task } | Command::Reply { task, .. } => {
            Some(task)
        }
        Command::Status | Command::Pair { .. } => None,
    }
}

/// Validación completa de un comando entrante. `expected_owner` = project_key del owner
/// (007). Si None, no se chequea ownership (single-user). Devuelve la tarea (estado + pane_id)
/// si la validación pasa para comandos con tarea.
#[derive(Debug)]
pub struct ValidatedTask {
    pub task_id: String,
    pub state: String,
    pub pane_id: Option<String>,
}

pub fn validate(
    db: &Db,
    chat_id: &str,
    command: &Command,
    expected_owner: Option<&str>,
) -> std::result::Result<Option<ValidatedTask>, ValidationError> {
    // /pair NO requiere allowlist (es el mecanismo de entrada).
    if !matches!(command, Command::Pair { .. }) {
        let conn = db.lock();
        if !is_allowed(&conn, chat_id) {
            return Err(ValidationError::NotAllowed);
        }
    }
    let Some(task_id) = target_task(command) else {
        return Ok(None);
    };
    // NOTE: 008 (orchestration_tasks) no tiene columna project_key todavía; el ownership
    // (007) es single-user hoy. Sólo leemos state + pane_id. `expected_owner` se respetará
    // cuando 007 wire un project_key a las tareas (entonces se agrega la columna + chequeo).
    let conn = db.lock();
    let row = conn
        .query_row(
            "SELECT state, pane_id FROM orchestration_tasks WHERE id = ?1",
            params![task_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .ok();
    let (state, pane_id) = match row {
        Some(t) => t,
        None => return Err(ValidationError::TaskNotFound),
    };
    let project_key: Option<String> = None;
    if let (Some(expected), Some(actual)) = (expected_owner, project_key.as_deref()) {
        if expected != actual {
            return Err(ValidationError::NotOwned);
        }
    }
    if !state_allows(command, &state) {
        return Err(ValidationError::InvalidState(state.clone()));
    }
    Ok(Some(ValidatedTask {
        task_id: task_id.to_string(),
        state,
        pane_id,
    }))
}

// ── Ejecución ─────────────────────────────────────────────────────────────

/// Acción a ejecutar fuera del módulo (necesita el PtyManager, que vive en commands/lib).
/// El executor resuelve qué hacer; el caller materializa la parte que toca el PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Sin efecto colateral; sólo respuesta (status).
    None,
    /// Escribir `text` (+ Enter) al pane.
    PtyWrite { pane_id: String, text: String },
    /// Matar el proceso del pane (cancel).
    PtyKill { pane_id: Option<String> },
}

/// Resultado de ejecutar un comando ya validado: efecto sobre el PTY + texto de respuesta.
pub struct ExecResult {
    pub effect: Effect,
    pub reply: String,
}

/// Resumen de tareas para `/status` (no toca PTY).
pub fn status_summary(db: &Db) -> String {
    let conn = db.lock();
    let mut counts: Vec<(String, i64)> = Vec::new();
    if let Ok(mut stmt) = conn
        .prepare("SELECT state, COUNT(*) FROM orchestration_tasks GROUP BY state ORDER BY state")
    {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        {
            counts = rows.filter_map(|r| r.ok()).collect();
        }
    }
    if counts.is_empty() {
        return "Sin tareas de orquestación.".to_string();
    }
    let parts: Vec<String> = counts
        .iter()
        .map(|(s, n)| format!("{}: {}", s, n))
        .collect();
    format!("Tareas — {}", parts.join(" · "))
}

/// Aplica la mutación de estado en DB de un comando ya validado y devuelve el efecto a
/// ejecutar sobre el PTY (que el caller resuelve, porque tiene el PtyManager).
/// Para /cancel y /ready usa las transiciones validadas de 008 (orchestration::set_state).
pub fn execute(
    db: &Db,
    command: &Command,
    validated: &Option<ValidatedTask>,
) -> Result<ExecResult> {
    use crate::services::orchestration as orch;
    match command {
        Command::Status => Ok(ExecResult {
            effect: Effect::None,
            reply: status_summary(db),
        }),
        Command::Pair { .. } => {
            // El pairing se resuelve en el caller (consume_pair_code) antes de execute.
            Ok(ExecResult {
                effect: Effect::None,
                reply: "Paired ✓".to_string(),
            })
        }
        Command::Cancel { task } => {
            let v = validated.as_ref();
            let pane_id = v.and_then(|t| t.pane_id.clone());
            orch::set_state(db, task, "canceled", None)?;
            Ok(ExecResult {
                effect: Effect::PtyKill { pane_id },
                reply: format!("Tarea {} cancelada.", short(task)),
            })
        }
        Command::Ready { task } => {
            // Recolecta diff stat + awaiting_review (igual que orchestration_mark_ready).
            if let Some(t) = orch::get_task(db, task)? {
                if let Some(wt) = t.worktree_path.as_deref() {
                    let summary = orch::collect_diff(wt);
                    let _ = orch::set_result_summary(db, task, &summary);
                }
            }
            orch::set_state(db, task, "awaiting_review", None)?;
            Ok(ExecResult {
                effect: Effect::None,
                reply: format!("Tarea {} marcada para review.", short(task)),
            })
        }
        Command::Reply { task, text } => {
            let pane_id = validated
                .as_ref()
                .and_then(|t| t.pane_id.clone())
                .ok_or_else(|| anyhow::anyhow!("la tarea no tiene pane activo"))?;
            // Audit codex+deepseek HIGH: sanitizar el texto libre ANTES de escribirlo al PTY.
            // Sin esto, control chars (Ctrl-C/Ctrl-D), ANSI o \r/\n múltiples podrían matar el
            // agente, corromper el PTY o inyectar entradas extra (responder prompts). Dejamos
            // sólo caracteres imprimibles + espacios, UNA línea, y agregamos un único Enter.
            let clean: String = text
                .replace(['\r', '\n'], " ")
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            let clean = clean.trim();
            if clean.is_empty() {
                return Err(anyhow::anyhow!(
                    "el texto de /reply quedó vacío tras sanitizar"
                ));
            }
            let payload = format!("{}\n", clean);
            Ok(ExecResult {
                effect: Effect::PtyWrite {
                    pane_id,
                    text: payload,
                },
                reply: format!("Enviado a {} ✓", short(task)),
            })
        }
    }
}

fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Etiqueta corta del comando para audit/logs (no incluye texto libre de /reply).
pub fn command_label(command: &Command) -> &'static str {
    match command {
        Command::Status => "status",
        Command::Cancel { .. } => "cancel",
        Command::Reply { .. } => "reply",
        Command::Ready { .. } => "ready",
        Command::Pair { .. } => "pair",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/023_signals.sql"))
            .unwrap();
        // 012: columnas needs_input/auto_confirm/cli_kind que orchestration::row_to_task selecciona.
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql"))
            .unwrap();
        // 014: group_id/variant_index que orchestration::row_to_task ahora selecciona.
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql"))
            .unwrap();
        // 019 F3: columna paused_at (pause/resume) — el SELECT de row_to_task ya la pide.
        conn.execute_batch(include_str!(
            "../../migrations/037_orch_pause_council_history.sql"
        ))
        .unwrap();
        // 038 F1.0 — DAG schema (pipeline_run_id/dag_blocked que enrich_dag_fields lee).
        conn.execute_batch(include_str!("../../migrations/047_pipeline_dag.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn seed_task(db: &Db, id: &str, state: &str) {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO orchestration_batches (id, repo_path) VALUES ('b1','/tmp/r')",
            [],
        )
        .ok();
        conn.execute(
            "INSERT INTO orchestration_tasks (id, batch_id, title, repo_path, branch, state, pane_id)
             VALUES (?1,'b1','T','/tmp/r',?2,?3,?4)",
            params![id, format!("furx/orch/{}", id), state, format!("orch-{}", id)],
        )
        .unwrap();
    }

    #[test]
    fn classify_whitelist() {
        assert_eq!(classify("/status"), Ok(Command::Status));
        assert_eq!(classify("  /status  "), Ok(Command::Status));
        assert_eq!(classify("/status@FurxBot"), Ok(Command::Status));
        assert_eq!(
            classify("/cancel abc123"),
            Ok(Command::Cancel {
                task: "abc123".into()
            })
        );
        assert_eq!(
            classify("/ready t1"),
            Ok(Command::Ready { task: "t1".into() })
        );
        assert_eq!(
            classify("/pair 9988"),
            Ok(Command::Pair {
                code: "9988".into()
            })
        );
        assert_eq!(
            classify("/reply t1 dale que sí"),
            Ok(Command::Reply {
                task: "t1".into(),
                text: "dale que sí".into()
            })
        );
    }

    #[test]
    fn classify_rejects_non_whitelist() {
        // default-deny: nada de shell/edición/approve-all.
        assert_eq!(classify("/shell rm -rf /"), Err(ParseError::UnknownCommand));
        assert_eq!(classify("/exec ls"), Err(ParseError::UnknownCommand));
        assert_eq!(classify("/approve_all"), Err(ParseError::UnknownCommand));
        assert_eq!(classify("rm -rf /"), Err(ParseError::NotACommand));
        assert_eq!(classify("hola"), Err(ParseError::NotACommand));
        assert_eq!(classify(""), Err(ParseError::Empty));
        // args faltantes
        assert_eq!(classify("/cancel"), Err(ParseError::MissingArgs));
        assert_eq!(classify("/reply t1"), Err(ParseError::MissingArgs)); // falta texto
    }

    #[test]
    fn allowlist_roundtrip() {
        let db = test_db();
        {
            let conn = db.lock();
            assert!(!is_allowed(&conn, "12345"));
        }
        add_to_allowlist(&db, "12345", Some("mi tel"), "manual").unwrap();
        {
            let conn = db.lock();
            assert!(is_allowed(&conn, "12345"));
        }
        assert!(remove_from_allowlist(&db, "12345").unwrap());
        {
            let conn = db.lock();
            assert!(!is_allowed(&conn, "12345"));
        }
    }

    #[test]
    fn pair_code_one_time_use() {
        let db = test_db();
        let code = create_pair_code(&db).unwrap();
        // primer uso: parea
        assert!(consume_pair_code(&db, &code, "chat-A").unwrap());
        {
            let conn = db.lock();
            assert!(is_allowed(&conn, "chat-A"));
        }
        // segundo uso del mismo código: rechazado (un uso)
        assert!(!consume_pair_code(&db, &code, "chat-B").unwrap());
        {
            let conn = db.lock();
            assert!(!is_allowed(&conn, "chat-B"));
        }
        // código inexistente
        assert!(!consume_pair_code(&db, "nope", "chat-C").unwrap());
    }

    #[test]
    fn state_machine_per_command() {
        let cancel = Command::Cancel { task: "t".into() };
        let ready = Command::Ready { task: "t".into() };
        let reply = Command::Reply {
            task: "t".into(),
            text: "x".into(),
        };
        assert!(state_allows(&cancel, "running"));
        assert!(state_allows(&cancel, "pending"));
        assert!(!state_allows(&cancel, "done"));
        assert!(!state_allows(&cancel, "canceled"));
        assert!(state_allows(&ready, "running"));
        assert!(!state_allows(&ready, "done")); // /ready de una done → no
        assert!(!state_allows(&ready, "awaiting_review"));
        assert!(state_allows(&reply, "running"));
        assert!(!state_allows(&reply, "pending"));
        assert!(state_allows(&Command::Status, "anything"));
    }

    #[test]
    fn validate_rejects_unknown_chat() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        // chat NO allowlisteado → NotAllowed (no llega a mirar la tarea).
        let err = validate(
            &db,
            "stranger",
            &Command::Cancel { task: "t1".into() },
            None,
        )
        .unwrap_err();
        assert_eq!(err, ValidationError::NotAllowed);
    }

    #[test]
    fn validate_accepts_allowlisted_valid() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        add_to_allowlist(&db, "owner", None, "manual").unwrap();
        let v = validate(&db, "owner", &Command::Cancel { task: "t1".into() }, None)
            .unwrap()
            .unwrap();
        assert_eq!(v.task_id, "t1");
        assert_eq!(v.state, "running");
        assert_eq!(v.pane_id.as_deref(), Some("orch-t1"));
    }

    #[test]
    fn validate_task_not_found() {
        let db = test_db();
        add_to_allowlist(&db, "owner", None, "manual").unwrap();
        let err = validate(
            &db,
            "owner",
            &Command::Cancel {
                task: "ghost".into(),
            },
            None,
        )
        .unwrap_err();
        assert_eq!(err, ValidationError::TaskNotFound);
    }

    #[test]
    fn validate_invalid_state() {
        let db = test_db();
        seed_task(&db, "t1", "done");
        add_to_allowlist(&db, "owner", None, "manual").unwrap();
        let err = validate(&db, "owner", &Command::Ready { task: "t1".into() }, None).unwrap_err();
        assert_eq!(err, ValidationError::InvalidState("done".into()));
    }

    #[test]
    fn validate_pair_does_not_require_allowlist() {
        let db = test_db();
        // /pair se valida sin estar en la allowlist (es el mecanismo de entrada).
        let v = validate(&db, "stranger", &Command::Pair { code: "x".into() }, None).unwrap();
        assert!(v.is_none());
    }

    #[test]
    fn execute_cancel_sets_state_and_kill_effect() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        let cmd = Command::Cancel { task: "t1".into() };
        let validated = Some(ValidatedTask {
            task_id: "t1".into(),
            state: "running".into(),
            pane_id: Some("orch-t1".into()),
        });
        let res = execute(&db, &cmd, &validated).unwrap();
        assert_eq!(
            res.effect,
            Effect::PtyKill {
                pane_id: Some("orch-t1".into())
            }
        );
        let conn = db.lock();
        let state: String = conn
            .query_row(
                "SELECT state FROM orchestration_tasks WHERE id='t1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "canceled");
    }

    #[test]
    fn execute_reply_returns_pty_write_with_enter() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        let cmd = Command::Reply {
            task: "t1".into(),
            text: "decile sí".into(),
        };
        let validated = Some(ValidatedTask {
            task_id: "t1".into(),
            state: "running".into(),
            pane_id: Some("orch-t1".into()),
        });
        let res = execute(&db, &cmd, &validated).unwrap();
        assert_eq!(
            res.effect,
            Effect::PtyWrite {
                pane_id: "orch-t1".into(),
                text: "decile sí\n".into()
            }
        );
    }

    #[test]
    fn execute_ready_transitions_to_awaiting_review() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        let cmd = Command::Ready { task: "t1".into() };
        let validated = Some(ValidatedTask {
            task_id: "t1".into(),
            state: "running".into(),
            pane_id: Some("orch-t1".into()),
        });
        let res = execute(&db, &cmd, &validated).unwrap();
        assert_eq!(res.effect, Effect::None);
        let conn = db.lock();
        let state: String = conn
            .query_row(
                "SELECT state FROM orchestration_tasks WHERE id='t1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "awaiting_review");
    }

    #[test]
    fn status_summary_counts_states() {
        let db = test_db();
        seed_task(&db, "t1", "running");
        seed_task(&db, "t2", "running");
        seed_task(&db, "t3", "pending");
        let s = status_summary(&db);
        assert!(s.contains("running: 2"), "got: {}", s);
        assert!(s.contains("pending: 1"), "got: {}", s);
    }
}
