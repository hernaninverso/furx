// services/done_detection.rs — 012-pty-done-detection.
//
// Auto-detección del ciclo de vida de una tarea de orquestación (008) por POLLING del
// buffer de su pane PTY. Patrón robado de uzi / claude_code_agent_farm / vibe-kanban:
// los agentes CLI son interactivos (no emiten exit-code "done"), quedan en un prompt; el
// poller lee el buffer-tail y lo clasifica por la UI del CLI.
//
//   classify(buffer_tail, cli) -> Verdict {Running | Idle | NeedsInput}   ← PURO, testeable
//   Poller (worker tokio, 1-3s, sólo tareas `running`, debounce N ticks):
//     Idle      → set_state(awaiting_review) + collect_diff (reusa 008) + emit task.awaiting_review
//     NeedsInput→ si auto_confirm ON → pty_write(Enter) (tope N/min, auditado)
//                 si OFF             → emit agent.input_requested (010) + flag needs_input
//
// Constitución: VI auto-confirm es OPT-IN (default OFF) + tope/min (NO destructivo automático);
// el mark-ready manual de 008 SIGUE (esto es complementario). F-I BYOK: el LLM-assist usa el AIE
// ya configurado (bearer del Keychain), no toca keys.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Cuántas líneas del final del buffer mira el classifier (spec: ~12).
pub const TAIL_LINES: usize = 12;
/// Ticks Idle consecutivos antes de auto-transicionar (debounce — evita falso idle mientras
/// el agente "piensa" sin spinner). Configurable arriba si hace falta.
pub const IDLE_DEBOUNCE_TICKS: u32 = 3;
/// Tope de auto-confirms por tarea por ventana (anti loop infinito de prompt que reaparece).
pub const AUTO_CONFIRM_MAX_PER_MIN: i64 = 6;

/// Veredicto del classifier — qué está haciendo la pane según su UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// El agente está trabajando (spinner / "esc to interrupt" / actividad).
    Running,
    /// Prompt vacío sin spinner → terminó, espera input que no llega.
    Idle,
    /// Trust / permission / confirm prompt → necesita una decisión humana.
    NeedsInput,
}

/// CLI que corre en la pane — selecciona la tabla de patrones. Extensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliKind {
    Claude,
    Codex,
    Aider,
    Gemini,
    /// Desconocido / genérico — sólo patrones comunes.
    Generic,
}

impl CliKind {
    /// Mapea el `cli_kind` cacheado (008 agent profile) a la tabla de patrones.
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "claude" => CliKind::Claude,
            "codex" => CliKind::Codex,
            "aider" => CliKind::Aider,
            "gemini" => CliKind::Gemini,
            _ => CliKind::Generic,
        }
    }
}

// ── Tablas de patrones ───────────────────────────────────────────────────────
// Compiladas una vez (Lazy). Trabajan sobre texto en MINÚSCULAS para ser case-insensitive
// sin el costo de `(?i)` por match. classify() lowercasea el tail una vez.

// CSI/OSC/escapes/control — el classifier es puro y puede recibir fixtures con ANSI crudo
// (aunque PtyManager.snapshot ya lo strippea). Re-usamos el mismo set que pty.rs.
static ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"\x1b\[[0-9;?]*[ -/]*[@-~]",
        r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)",
        r"|\x1b[@-Z\\-_]",
        r"|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]"
    ))
    .expect("ansi regex")
});

fn strip_ansi(s: &str) -> String {
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Spinner / actividad — si aparece en el tail, OVERRIDE a Running (no marcar idle aunque
/// también se vea un prompt). Frames braille de Ink (claude/gemini), barras, "esc to interrupt",
/// "thinking…", tokens contando, etc. En minúsculas.
static ACTIVITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"esc to interrupt", // claude/codex footer mientras trabaja
        r"|press esc",
        r"|[⠁-⣿]",         // spinner braille (Ink) — rango U+2801..U+28FF
        r"|[\|/\-\\]\s*$", // spinner ascii al final de línea
        r"|thinking",
        r"|esc.{0,3}interrupt",
        r"|working", // codex "Working…"
        r"|generating",
        r"|streaming",
        r"|tokens?\b.*\bk?\s*$", // contador de tokens
        r"|↑\s*\d+",             // codex token counter "↑ 1234"
        r"|◓|◑|◒|◐|⠿"            // otros frames de spinner
    ))
    .expect("activity regex")
});

/// Patrones comunes a TODOS los CLIs (en minúsculas, substring match) → NeedsInput.
/// Se chequean SOLO si hay un prompt; tienen prioridad sobre el spinner (ver classify).
const COMMON_NEEDS_INPUT: &[&str] = &[
    "do you want to proceed",
    "do you want to continue",
    "allow this",
    "allow command",
    "yes, and don't ask again",
    "permission to",
    "approve this",
    "[y/n]",
    "(y/n)",
    "y/n?",
    "press enter to continue",
    "trust the files in this folder",
    "trust this folder",
    "do you trust",
    "continue? (y/n)",
];

/// Patrones de trust/permission ESPECÍFICOS por CLI (la tabla extensible del spec).
fn cli_specific(cli: CliKind) -> &'static [&'static str] {
    match cli {
        CliKind::Claude => &[
            "do you want to make this edit",
            "do you want to create",
            "❯ 1. yes",
            "1. yes",
            "would you like to",
        ],
        CliKind::Codex => &[
            "allow codex to run",
            "run this command?",
            "apply this patch?",
        ],
        CliKind::Aider => &[
            "add the files to the chat?",
            "create new file",
            "edit the files?",
            "allow edits to",
        ],
        CliKind::Gemini => &["apply this change?", "allow execution"],
        CliKind::Generic => &[],
    }
}

fn has_needs_input(lower_tail: &str, cli: CliKind) -> bool {
    COMMON_NEEDS_INPUT.iter().any(|p| lower_tail.contains(p))
        || cli_specific(cli).iter().any(|p| lower_tail.contains(p))
}

/// Prompts that are SAFE to auto-confirm with Enter: workspace-trust + informational
/// "press enter to continue" dialogs whose default action is benign. Audit codex+deepseek
/// 012 (constitution VI — parar ante destructivo): auto-confirm must NEVER send Enter to a
/// prompt that RUNS A COMMAND, APPLIES A PATCH, EDITS/CREATES/DELETES files, or is a bare
/// [y/n]/proceed/continue — those can approve a destructive action. So the auto-confirm
/// whitelist is a STRICT SUBSET of the detected needs-input prompts.
const SAFE_AUTO_CONFIRM: &[&str] = &[
    "trust the files in this folder",
    "trust this folder",
    "do you trust",
    "press enter to continue",
];

/// Is the visible prompt one we may auto-confirm? Only the SAFE workspace-trust subset.
/// Everything else (command exec / patch / edit / y-n / proceed) returns false → the human
/// decides, even when auto-confirm is ON.
fn safe_to_auto_confirm(lower_tail: &str) -> bool {
    SAFE_AUTO_CONFIRM.iter().any(|p| lower_tail.contains(p))
}

/// The lowercased visible window (last TAIL_LINES non-empty, ANSI-stripped) — the exact
/// text `classify` inspects. Shared so `process_task` can re-check the SAFE auto-confirm
/// subset against the SAME window without re-implementing the slicing.
fn tail_lower(buffer_tail: &[String]) -> String {
    let tail: Vec<String> = buffer_tail
        .iter()
        .rev()
        .take(TAIL_LINES * 2)
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let non_empty: Vec<&str> = tail
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .collect();
    let window: Vec<&str> = non_empty
        .iter()
        .rev()
        .take(TAIL_LINES)
        .rev()
        .copied()
        .collect();
    window.join("\n").to_lowercase()
}

// ── classify — PURO ────────────────────────────────────────────────────────────

/// Clasifica el buffer-tail (líneas, ya o no ANSI-stripped) de una pane.
///
/// Orden (spec FR-002):
///   1. NeedsInput tiene PRIORIDAD sobre el spinner — un trust prompt suele venir con el
///      footer "esc to interrupt" todavía visible; si pedimos confirmación, hay que pedirla
///      (no quedarnos en Running esperando para siempre).
///   2. Actividad (spinner / "esc to interrupt" / "thinking") → Running (override de idle).
///   3. Sin actividad y sin prompt de input → Idle (terminó, espera input que no llega).
///
/// Toma las últimas TAIL_LINES líneas, strippea ANSI, lowercasea una vez. Puro → unit-testeable.
pub fn classify(buffer_tail: &[String], cli: CliKind) -> Verdict {
    // Últimas ~12 líneas, descartando líneas en blanco al final (no aportan señal).
    let tail: Vec<String> = buffer_tail
        .iter()
        .rev()
        .take(TAIL_LINES * 2) // tomamos algo más por si hay blancos
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    // Texto plano del tail (las últimas TAIL_LINES no-vacías, en orden).
    let non_empty: Vec<&str> = tail
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .collect();
    let window: Vec<&str> = non_empty
        .iter()
        .rev()
        .take(TAIL_LINES)
        .rev()
        .copied()
        .collect();

    if window.is_empty() {
        // Buffer vacío / sólo blancos → todavía no arrancó o limpió la pantalla. Tratar como
        // Running (no marcar idle un arranque); el debounce + ticks siguientes deciden.
        return Verdict::Running;
    }

    let joined = window.join("\n");
    let lower = joined.to_lowercase();

    // 1. Trust / permission prompt → necesita decisión humana (prioridad sobre spinner).
    if has_needs_input(&lower, cli) {
        return Verdict::NeedsInput;
    }

    // 2. Actividad → Running (override).
    if ACTIVITY_RE.is_match(&lower) {
        return Verdict::Running;
    }

    // 3. Sin actividad, sin prompt de input → Idle.
    Verdict::Idle
}

// ── Poller ───────────────────────────────────────────────────────────────────

/// Acceso al buffer-tail de una pane. Lo implementa `pty::PtyManager` (vía `snapshot`); en
/// tests usamos un fake. Mantiene el poller desacoplado del PTY/red → testeable.
pub trait PaneBuffer: Send + Sync {
    /// Últimas líneas ANSI-stripped de la pane (vacío si no existe). ≤50 (scrollback ring).
    fn tail(&self, pane_id: &str) -> Vec<String>;
    /// Escribe bytes a la pane (auto-confirm: Enter). Err si la pane no existe.
    fn write(&self, pane_id: &str, data: &[u8]) -> Result<()>;
    /// ¿La pane sigue viva? (proceso no terminó).
    fn alive(&self, pane_id: &str) -> bool;
}

impl PaneBuffer for crate::pty::PtyManager {
    fn tail(&self, pane_id: &str) -> Vec<String> {
        self.snapshot(pane_id)
    }
    fn write(&self, pane_id: &str, data: &[u8]) -> Result<()> {
        crate::pty::PtyManager::write(self, pane_id, data)
    }
    fn alive(&self, pane_id: &str) -> bool {
        crate::pty::PtyManager::alive(self, pane_id)
    }
}

/// Estado de debounce por tarea (en memoria del poller — efímero, no persiste).
#[derive(Default)]
pub struct PollerState {
    /// task_id → cuántos ticks Idle consecutivos lleva.
    idle_streak: std::collections::HashMap<String, u32>,
    /// task_id → fingerprint del buffer en el último tick Idle (para exigir estabilidad).
    idle_fingerprint: std::collections::HashMap<String, u64>,
}

impl PollerState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un tick Idle y devuelve true si superó el debounce (Idle ESTABLE).
    /// Audit fix F4: además de N ticks consecutivos, exige que el buffer NO haya cambiado
    /// entre ticks. Un comando silencioso (p.ej. `cargo build` sin spinner) que sigue
    /// emitiendo output luce "sin actividad" pero su buffer crece — si el fingerprint
    /// cambió, reseteamos el streak (no está realmente idle), evitando un awaiting_review
    /// prematuro. Sólo cuenta como idle estable si el buffer quedó quieto IDLE_DEBOUNCE ticks.
    fn note_idle(&mut self, task_id: &str, fingerprint: u64) -> bool {
        let prev = self
            .idle_fingerprint
            .insert(task_id.to_string(), fingerprint);
        let n = self.idle_streak.entry(task_id.to_string()).or_insert(0);
        if prev != Some(fingerprint) {
            // el buffer cambió desde el último tick idle → arranca de nuevo (no estable).
            *n = 1;
            return false;
        }
        *n += 1;
        *n >= IDLE_DEBOUNCE_TICKS
    }

    /// Resetea el contador de idle (vio actividad / needs_input / transicionó).
    fn reset_idle(&mut self, task_id: &str) {
        self.idle_streak.remove(task_id);
        self.idle_fingerprint.remove(task_id);
    }
}

/// Fingerprint estable (no-cripto) del buffer-tail para detectar cambios entre ticks (F4).
fn buffer_fingerprint(buffer_tail: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    buffer_tail.hash(&mut h);
    h.finish()
}

/// ¿Está el auto-confirm habilitado para esta tarea? Lee el flag por-tarea OR el global setting.
fn auto_confirm_enabled(db: &Db, task: &crate::services::orchestration::OrchTask) -> bool {
    if task_auto_confirm_flag(db, &task.id) {
        return true;
    }
    let conn = db.lock();
    crate::settings::get(&conn, "orchestration.auto_confirm_global")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn task_auto_confirm_flag(db: &Db, task_id: &str) -> bool {
    let conn = db.lock();
    conn.query_row(
        "SELECT auto_confirm FROM orchestration_tasks WHERE id = ?1",
        rusqlite::params![task_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v != 0)
    .unwrap_or(false)
}

/// Marca/limpia el flag needs_input de una tarea (sub-estado, NO toca la state machine de 008).
pub fn set_needs_input(db: &Db, task_id: &str, needs: bool) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "UPDATE orchestration_tasks SET needs_input = ?2, updated_at = ?3 WHERE id = ?1",
        rusqlite::params![task_id, needs as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Toggle del auto-confirm por tarea (UI / command).
pub fn set_auto_confirm(db: &Db, task_id: &str, enabled: bool) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "UPDATE orchestration_tasks SET auto_confirm = ?2, updated_at = ?3 WHERE id = ?1",
        rusqlite::params![task_id, enabled as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// ¿Cuántos auto-confirms hubo para esta tarea en el último minuto? (tope anti-loop).
fn recent_auto_confirms(db: &Db, task_id: &str) -> i64 {
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM orch_auto_confirms
         WHERE task_id = ?1 AND confirmed_at > datetime('now','-1 minute')",
        rusqlite::params![task_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Registra un auto-confirm en la auditoría.
fn record_auto_confirm(db: &Db, task_id: &str, matched: &str) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "INSERT INTO orch_auto_confirms (id, task_id, matched) VALUES (?1,?2,?3)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), task_id, matched],
    )?;
    Ok(())
}

/// Resultado de procesar UNA tarea en un tick (para tests/observabilidad).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickAction {
    /// Sin cambio (running / esperando debounce).
    None,
    /// Transicionó a awaiting_review.
    AwaitingReview,
    /// Auto-confirmó (pty_write Enter).
    AutoConfirmed,
    /// Emitió agent.input_requested (auto-confirm OFF).
    InputRequested,
    /// Tope de auto-confirm/min superado → cae a InputRequested.
    AutoConfirmThrottled,
}

/// Procesa UNA tarea `running`: lee su buffer, clasifica, actúa. Devuelve la acción tomada.
/// `pane` = acceso a la pane (PtyManager o fake). PURO respecto del PTY (usa el trait).
///
/// `async` por el refinamiento AIE (020, US1): el `await` SÓLO dispara en el raro tick
/// post-debounce de un verdict regex `Idle` ambiguo Y con el feature ON. Con el feature OFF
/// (default) el helper retorna sin tocar la red, así que el costo async es nulo en el caso común.
pub async fn process_task(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
    pane: &dyn PaneBuffer,
    pstate: &mut PollerState,
    task: &crate::services::orchestration::OrchTask,
) -> TickAction {
    let Some(pane_id) = task.pane_id.as_deref() else {
        return TickAction::None; // tarea sin pane (no lanzada / detached sin id) — nada que pollear
    };

    let tail = pane.tail(pane_id);
    // 014 FR-003 log-history: persistir el snapshot del buffer (ANSI-stripped) por tarea. De-dup
    // (no inserta si el contenido no cambió) + rotación FIFO viven en append_log_history, así que
    // pollear cada 2s no spamea la tabla. El scrollback PTY se pierde al exit; esto lo conserva.
    {
        let stripped: Vec<String> = tail.iter().map(|l| strip_ansi(l)).collect();
        let _ =
            crate::services::orchestration::append_log_history(db, &task.id, "poller", &stripped);
    }
    let cli = task
        .cli_kind
        .as_deref()
        .map(CliKind::from_str)
        // fallback: deducir del mode (claude-*, codex-*, …) si no hay cli_kind cacheado.
        .unwrap_or_else(|| cli_from_mode(task.mode.as_deref()));

    let verdict = classify(&tail, cli);

    match verdict {
        Verdict::Running => {
            pstate.reset_idle(&task.id);
            // Si tenía needs_input flag y volvió a trabajar (p.ej. tras auto-confirm), limpiarlo.
            if task.needs_input != 0 {
                let _ = set_needs_input(db, &task.id, false);
            }
            TickAction::None
        }
        Verdict::Idle => {
            // Debounce: sólo transicionar tras N ticks Idle consecutivos Y con el buffer
            // estable (F4: un comando silencioso que sigue emitiendo output no es idle).
            if !pstate.note_idle(&task.id, buffer_fingerprint(&tail)) {
                return TickAction::None;
            }
            pstate.reset_idle(&task.id);

            // 020 — refinamiento AIE (US1): el verdict regex `Idle` es el caso AMBIGUO (un prompt
            // vacío "sin actividad" puede ser una pregunta phraseada fuera de la tabla, o el agente
            // pensando sin spinner). SÓLO acá —post-debounce, baja frecuencia (FR-007)— y SÓLO si
            // el feature `orchestration.use_aie_for_meta` está ON, consultamos el AIE para refinar.
            // El AIE es advisory y NUNCA bloquea: `refine_verdict_with_aie` cae al verdict regex
            // (`Idle`) ante cualquier fallo / feature OFF (FR-003). El `await` aquí está bounded por
            // el timeout de 3s del engine y sólo dispara en el raro tick post-debounce, no en el
            // hot-path por tick. Con el feature OFF (default) el helper retorna `Idle` sin red.
            let refined = refine_verdict_with_aie(db, audit, &tail, cli, Verdict::Idle).await;
            match refined {
                // El AIE confirmó/asumió Idle → transición a awaiting_review (comportamiento base).
                Verdict::Idle => handle_idle_transition(db, audit, task, pane_id),
                // El AIE detectó que el agente volvió a trabajar → NO transicionar (deja running).
                Verdict::Running => {
                    if task.needs_input != 0 {
                        let _ = set_needs_input(db, &task.id, false);
                    }
                    TickAction::None
                }
                // El AIE detectó una pregunta no captada por la regex → ruta NeedsInput (que respeta
                // la política de auto-confirm de 012: destructivo NUNCA se auto-confirma). El
                // reset_idle ya ocurrió arriba en este mismo branch.
                Verdict::NeedsInput => handle_needs_input(db, audit, pane, task, pane_id, &tail),
            }
        }
        Verdict::NeedsInput => {
            pstate.reset_idle(&task.id);
            handle_needs_input(db, audit, pane, task, pane_id, &tail)
        }
    }
}

/// Transición Idle→awaiting_review (extraída para reuso desde el refinamiento AIE Idle→Idle).
/// Reusa 008: collect diff + set_state(awaiting_review) + emite task.awaiting_review (010).
fn handle_idle_transition(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
    task: &crate::services::orchestration::OrchTask,
    pane_id: &str,
) -> TickAction {
    use crate::services::orchestration as orch;
    if let Some(wt) = task.worktree_path.as_deref() {
        let summary = orch::collect_diff(wt);
        let _ = orch::set_result_summary(db, &task.id, &summary);
    }
    if orch::set_state(db, &task.id, "awaiting_review", None).is_err() {
        // transición inválida (ya no estaba running) — nada que hacer.
        return TickAction::None;
    }
    let _ = set_needs_input(db, &task.id, false);
    let _ = audit.write(crate::bases::audit::EventInput {
        kind: "orch.auto_awaiting_review",
        actor: "poller:done_detection",
        pane_id: Some(pane_id),
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"task_id": task.id}),
    });
    // 010-furx-signals — notificar awaiting_review (mismo evento que el mark-ready manual).
    let body = orch::get_task(db, &task.id)
        .ok()
        .flatten()
        .and_then(|t| t.result_summary)
        .unwrap_or_default();
    let _ = crate::services::signals::emit_task_event(
        db,
        "task.awaiting_review",
        &task.id,
        None,
        &format!("{} · awaiting_review", task.title),
        &body,
        "warning",
    );
    TickAction::AwaitingReview
}

/// Maneja el verdict NeedsInput (extraído para reuso desde el refinamiento AIE Idle→NeedsInput).
/// La política de auto-confirm de 012 manda: sólo el subset SAFE de workspace-trust se auto-confirma
/// (destructivo NUNCA, constitución VI), una vez por aparición fresca del prompt, con tope/min.
fn handle_needs_input(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
    pane: &dyn PaneBuffer,
    task: &crate::services::orchestration::OrchTask,
    pane_id: &str,
    tail: &[String],
) -> TickAction {
    // El reset de idle ya lo hace cada caller (path Idle-refinado y path NeedsInput-regex) antes
    // de delegar acá.
    // ¿Es la PRIMERA vez que vemos este prompt? (el flag venía en 0). Sólo en la
    // transición fresca emitimos agent.input_requested — sino floodeamos el feed cada
    // tick mientras el prompt sigue visible.
    let fresh = task.needs_input == 0;
    let _ = set_needs_input(db, &task.id, true);
    // Audit fix F2 (constitution VI): only the SAFE workspace-trust subset is
    // auto-confirmable. A destructive prompt (run command / apply patch / edit /
    // [y/n] / proceed) is NEVER auto-confirmed — it falls through to the human even
    // when auto-confirm is ON.
    let safe = safe_to_auto_confirm(&tail_lower(tail));
    // Audit fix (codex HIGH): send Enter at most ONCE per prompt appearance — gate
    // the write on the FRESH transition (needs_input 0→1), not just the per-min cap.
    // While the same prompt persists, needs_input stays 1 ⇒ fresh=false ⇒ no re-send
    // (a stray Enter could fall through to a shell or the next prompt). Running/Idle
    // clears needs_input, so a genuinely new prompt re-arms a single confirm.
    if auto_confirm_enabled(db, task) && safe && fresh {
        // Tope anti-loop: N auto-confirms/min/tarea (belt-and-suspenders sobre el fresh-gate).
        if recent_auto_confirms(db, &task.id) >= AUTO_CONFIRM_MAX_PER_MIN {
            let _ = audit.write(crate::bases::audit::EventInput {
                kind: "orch.auto_confirm_throttled",
                actor: "poller:done_detection",
                pane_id: Some(pane_id),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"task_id": task.id, "max_per_min": AUTO_CONFIRM_MAX_PER_MIN}),
            });
            emit_input_requested(db, task); // throttled → que el humano decida
            return TickAction::AutoConfirmThrottled;
        }
        // F5 (race): re-check the pane is still alive right before writing — the
        // user may have cancelled/closed it since the snapshot. pane.write also errs
        // on a dead pane, but the explicit guard avoids writing into a reused id.
        if !pane.alive(pane_id) {
            return TickAction::None;
        }
        // Auto-confirm: enviar Enter (\r) UNA vez. Best-effort.
        if pane.write(pane_id, b"\r").is_ok() {
            let _ = record_auto_confirm(db, &task.id, "safe_trust_prompt");
            let _ = audit.write(crate::bases::audit::EventInput {
                kind: "orch.auto_confirmed",
                actor: "poller:done_detection",
                pane_id: Some(pane_id),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"task_id": task.id, "kind": "safe_trust_prompt"}),
            });
            TickAction::AutoConfirmed
        } else {
            TickAction::None
        }
    } else {
        // Not auto-confirmed — either auto-confirm OFF, the prompt is NOT in the safe
        // subset (destructive → human must decide), or it's not the fresh transition.
        // Emit agent.input_requested (010) on the fresh transition so the human sees
        // it (avoids flooding the feed every tick while the prompt persists).
        if fresh {
            emit_input_requested(db, task);
            let _ = audit.write(crate::bases::audit::EventInput {
                kind: "orch.input_requested",
                actor: "poller:done_detection",
                pane_id: Some(pane_id),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"task_id": task.id, "auto_confirm_safe": safe}),
            });
        }
        TickAction::InputRequested
    }
}

/// Emite `agent.input_requested` (010) — idempotencia ligera: el dispatcher de 010 es por
/// (event,channel); aquí no deduplicamos eventos (cada NeedsInput sin auto-confirm emite uno),
/// pero el poller sólo entra acá tras un reset de idle, no en cada tick si el estado no cambia.
fn emit_input_requested(db: &Db, task: &crate::services::orchestration::OrchTask) {
    let _ = crate::services::signals::emit_task_event(
        db,
        "agent.input_requested",
        &task.id,
        None,
        &format!("{} · necesita tu confirmación", task.title),
        "El agente está esperando una respuesta a un prompt de permiso/confirmación.",
        "warning",
    );
}

/// Deduce el CliKind del `mode` (008) cuando no hay `cli_kind` cacheado: "claude-A" → Claude.
fn cli_from_mode(mode: Option<&str>) -> CliKind {
    match mode {
        Some(m) => {
            let head = m.split(['-', '_', ' ']).next().unwrap_or("");
            CliKind::from_str(head)
        }
        None => CliKind::Generic,
    }
}

/// Un tick del poller: procesa TODAS las tareas `running`. Devuelve cuántas tocó (para tests).
/// `async` porque `process_task` puede `await` el refinamiento AIE (020). En el caso común
/// (feature OFF / verdict no ambiguo) NO hay await real (el helper retorna sin red).
pub async fn tick(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
    pane: &dyn PaneBuffer,
    pstate: &mut PollerState,
    attention: Option<&crate::services::attention::AttentionQueue>,
    audio: Option<&crate::services::audio_attention::AudioManager>,
    notify: Option<&crate::services::notify_attention::NotificationManager>,
) -> usize {
    use crate::services::attention::Priority;
    use crate::services::audio_attention::{AudioKind, AudioRequest};
    use crate::services::orchestration as orch;
    let tasks = match orch::list_tasks(db, None) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("done_detection tick list_tasks error: {}", e);
            return 0;
        }
    };
    let mut touched = 0;
    // 034 U4 — agentes NeedsInput admisibles del tick (para la notif AGRUPADA al final: 2+ distintos →
    // 1 notif resumen en vez de N toasts).
    let mut notify_agents: Vec<Option<String>> = Vec::new();
    for task in tasks.into_iter().filter(|t| t.state == "running") {
        let action = process_task(db, audit, pane, pstate, &task).await;
        // 030 F0-wire — la transición de done_detection es la fuente AUTORITATIVA y always-on para la
        // cola de atención (el front sólo observa cuando un panel está abierto). Encolar acá NO mueve
        // el foco (los agentes piden, no agarran) — sólo agrega a la cola, con prioridad por clase.
        if let (Some(q), Some(pid)) = (attention, task.pane_id.as_deref()) {
            let prio = match action {
                TickAction::AwaitingReview => Some(Priority::HasResult),
                TickAction::InputRequested | TickAction::AutoConfirmThrottled => {
                    Some(Priority::NeedsInput)
                }
                TickAction::AutoConfirmed | TickAction::None => None,
            };
            // 033 U3 — descarte PERSISTENTE: si el pane fue descartado (`attention_ack`) y NO tuvo
            // actividad nueva desde entonces (`dismissed_at >= task.updated_at`), se SALTEA toda la
            // superficie de atención (cola + audio + notif). Si el task tuvo actividad nueva, el
            // descarte se borra y reaparece. Fail-closed: si la DB falla, `read_dismissal` da `None` →
            // no suprime (se comporta como hoy).
            let suppressed = if prio.is_some() {
                let conn = db.lock();
                let dismissed = crate::services::attention::read_dismissal(&conn, pid);
                if crate::services::attention::is_dismissed(dismissed.as_deref(), &task.updated_at) {
                    true
                } else {
                    if dismissed.is_some() {
                        crate::services::attention::clear_dismissal(&conn, pid);
                    }
                    false
                }
            } else {
                false
            };
            if let Some(prio) = prio.filter(|_| !suppressed) {
                // `seq` es ESTABLE entre ticks (enqueue dedup por pane sin-atender) → el event_id del
                // audio no re-suena por cada tick del mismo estado; al atenderse, un nuevo enqueue da
                // un seq nuevo → el próximo bloqueo del pane sí puede volver a sonar (FR-5.2, vía
                // rotación de seq + TTL del dedup; sin acoplar `resolve` al ack).
                let seq = q.enqueue(pid, prio);
                // 031 F1b — disparo del audio desde el BACKEND (misma fuente autoritativa). El gate
                // (opt-in/dedup/rate-limit/presupuesto) lo decide el AudioManager; acá sólo proponemos.
                // Orden de locks SIN ciclo: el opt-in resolver lockea db_arc y lo SUELTA antes del lock
                // interno del AudioManager; las rutas de audio (consider/silence) NUNCA lockean la DB →
                // no hay deadlock DB↔audio (audit aie).
                if let Some(a) = audio {
                    // Tag de prioridad ESTABLE (no `{prio:?}`): un rename del enum no cambia el id.
                    let ptag = match prio {
                        Priority::NeedsInput => "n",
                        Priority::HasResult => "r",
                    };
                    // 032 U2 — personalización: el nombre del agente sale del `cli_kind` (vocabulario
                    // controlado), validado por la WHITELIST. NUNCA del buffer. Si no pasa → "" ⇒ el
                    // sink usa la frase genérica. El sink REVALIDA (defensa en profundidad).
                    let text = task
                        .cli_kind
                        .as_deref()
                        .and_then(crate::services::audio_attention::agent_label_for_tts)
                        .unwrap_or_default();
                    let req = AudioRequest {
                        pane_id: pid.to_string(),
                        kind: AudioKind::for_priority(prio),
                        priority: prio,
                        event_id: format!("{pid}:{ptag}:{seq}"),
                        text,
                    };
                    a.consider(req); // Emit ⟺ encolado; el drenaje es al final del tick
                }
                // 033 U4 / 034 U4 — notificación nativa en background: SÓLO para NeedsInput. Se ACUMULA
                // el agente y se evalúa la TANDA al final del tick (consider_batch agrupa 2+ distintos
                // en una sola notif). El gate (opt-in + ventana-sin-foco + dedup 30s/agente) lo decide
                // el NotificationManager. El nombre sale del cli_kind (allowlist, nunca el buffer).
                if prio == Priority::NeedsInput && notify.is_some() {
                    notify_agents.push(task.cli_kind.clone());
                }
            }
        }
        touched += 1;
    }
    // Drenar la cola de audio una vez por tick (reproducción serial; `play` es spawn-rápido). Se llama
    // siempre que haya AudioManager: si la cola está vacía es un no-op barato, y así nunca queda audio
    // admitido sin reproducir aunque algún camino encole sin emitir en este tick (robustez, audit
    // deepseek).
    if let Some(a) = audio {
        a.pump_all();
    }
    // 034 U4 — evaluar la TANDA de notificaciones una vez por tick: 2+ agentes distintos → 1 resumen.
    if let Some(n) = notify {
        if !notify_agents.is_empty() {
            let refs: Vec<Option<&str>> = notify_agents.iter().map(|a| a.as_deref()).collect();
            n.consider_batch(&refs);
        }
    }
    touched
}

/// Worker persistente: corre `tick` cada `interval` (1-3s). Sólo tareas running, lee buffer
/// existente (no re-spawn) → cero impacto perceptible (FR-007). El `pane` (PtyManager) se
/// comparte vía Arc; la verdad del estado vive en SQLite.
pub async fn run_poller_loop(
    db: Db,
    audit: crate::bases::audit::AuditWriter,
    pane: Arc<dyn PaneBuffer>,
    interval: std::time::Duration,
    attention: Arc<crate::services::attention::AttentionQueue>,
    audio: Arc<crate::services::audio_attention::AudioManager>,
    notify: Arc<crate::services::notify_attention::NotificationManager>,
) {
    let mut pstate = PollerState::new();
    loop {
        // Panic-isolation del tick (ahora async): `FutureExt::catch_unwind` + `AssertUnwindSafe`
        // — equivalente al `std::panic::catch_unwind` sync previo, pero para el future. Un panic
        // dentro de un tick NO mata el loop del poller (se recupera y sigue al próximo intervalo).
        use futures_util::FutureExt;
        let fut = std::panic::AssertUnwindSafe(tick(
            &db,
            &audit,
            pane.as_ref(),
            &mut pstate,
            Some(attention.as_ref()),
            Some(audio.as_ref()),
            Some(notify.as_ref()),
        ));
        if fut.catch_unwind().await.is_err() {
            tracing::warn!("done_detection poller tick panicked (recovered)");
        }
        tokio::time::sleep(interval).await;
    }
}

// ── P3 — LLM-assist OPCIONAL para buffers ambiguos ─────────────────────────────
//
// La heurística (classify) decide la gran mayoría de los casos. Para los ambiguos (la heurística
// devuelve Idle pero hay texto que parece una pregunta), un LLM barato (AIE `fast_small_free`)
// lee las últimas N líneas y devuelve {SUCCESS|QUESTION|FAILED|WORKING}. Default OFF; si falla /
// timeout / no-JSON → fallback a la heurística (NUNCA bloquea — F-III, FR-005).
//
// NO se llama desde el hot-path sync del poller (eso quema red por tick). Es un helper que la UI
// o un futuro tick "lento" puede invocar puntualmente sobre un buffer marcado ambiguo.

/// Veredicto del LLM-assist (mapea al Verdict heurístico).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmVerdict {
    Success,  // terminó OK → Idle (awaiting_review)
    Question, // pide algo → NeedsInput
    Failed,   // falló → (lo trata el caller; aquí mapeamos a Idle/awaiting para revisión humana)
    Working,  // sigue trabajando → Running
}

impl LlmVerdict {
    /// Parsea la respuesta del LLM (texto libre o JSON `{"verdict":"..."}`). Tolerante: busca
    /// la primera keyword conocida. Devuelve None si no hay match (→ caller hace fallback).
    pub fn parse(raw: &str) -> Option<LlmVerdict> {
        let up = raw.to_uppercase();
        // Preferimos un match de palabra; el orden importa poco (son mutuamente excluyentes en
        // la práctica), pero chequeamos QUESTION/FAILED antes que SUCCESS por si el modelo
        // explica ("not a SUCCESS, it's a QUESTION").
        if up.contains("QUESTION") {
            Some(LlmVerdict::Question)
        } else if up.contains("FAILED") || up.contains("FAILURE") {
            Some(LlmVerdict::Failed)
        } else if up.contains("WORKING") || up.contains("IN_PROGRESS") {
            Some(LlmVerdict::Working)
        } else if up.contains("SUCCESS") || up.contains("DONE") || up.contains("COMPLETE") {
            Some(LlmVerdict::Success)
        } else {
            None
        }
    }

    pub fn to_verdict(self) -> Verdict {
        match self {
            LlmVerdict::Working => Verdict::Running,
            LlmVerdict::Question => Verdict::NeedsInput,
            // Success y Failed → la tarea ya no trabaja; el poller la lleva a awaiting_review
            // (revisión humana) — NO auto-merge ni auto-fail (constitución VI / fuera de scope).
            LlmVerdict::Success | LlmVerdict::Failed => Verdict::Idle,
        }
    }
}

const LLM_ASSIST_SYSTEM: &str =
    "You classify the terminal UI state of an autonomous coding agent. \
Reply with EXACTLY ONE WORD from: SUCCESS, QUESTION, FAILED, WORKING. \
SUCCESS = the agent finished its task and is idle at an empty prompt. \
QUESTION = the agent is asking the user a permission/confirmation/trust question. \
FAILED = the agent reported an error and stopped. \
WORKING = the agent is still actively working (spinner, streaming, thinking).";

/// LLM-assist (opt-in). Manda el tail (últimas N líneas) al AIE `fast_small_free` y devuelve el
/// veredicto. Fallback a la heurística `classify` si el LLM falla/timeout/no-JSON. El bearer sale
/// del Keychain (BYOK), nunca se persiste. SSRF: el endpoint pasa por la allowlist de aie.rs.
///
/// Variante sin `SessionCtx`: NO persiste failure_signals (no hay a qué sesión/pane atribuir).
pub async fn classify_with_llm(db: &Db, buffer_tail: &[String], cli: CliKind) -> Verdict {
    classify_with_llm_ctx(db, buffer_tail, cli, None).await
}

/// Igual que `classify_with_llm` pero con `SessionCtx` opcional. HIGH 3 del audit 3-frontera (spec
/// 025): cuando el LLM-assist clasifica el estado como `Failed`, PERSISTE un `failure_signal` EN ESE
/// MOMENTO (modelo F0 "fallo persistido -> fix posterior"), con el tail SANEADO + artefactos
/// (path+línea), `resolved=0`. Sólo si hay `ctx` Y el setting `memory.procedural_learning` está ON
/// (default OFF). El correlador de 025 empareja luego ese fallo con un fix posterior y lo resuelve.
/// Best-effort: la persistencia NUNCA altera el verdict ni rompe el poller.
pub async fn classify_with_llm_ctx(
    db: &Db,
    buffer_tail: &[String],
    cli: CliKind,
    ctx: Option<&crate::services::memory_autocapture::SessionCtx>,
) -> Verdict {
    let heuristic = classify(buffer_tail, cli);
    let tail = buffer_tail
        .iter()
        .rev()
        .take(TAIL_LINES)
        .rev()
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .join("\n");
    // Audit fix F1 (constitution F-I BYOK): the terminal tail can contain user secrets
    // (API keys, tokens, passwords printed by a command). REDACT before it leaves the
    // process to the AIE — reuse the same redactor the TTS path uses. The classifier only
    // needs the UI *shape* (spinner / prompt / empty), not the secret values.
    let tail = crate::services::tts::redact_secrets(&tail);
    match llm_verdict(db, &tail).await {
        Some(v) => {
            // HIGH 3 (025): un `Failed` del LLM-assist se PERSISTE en tiempo real como failure_signal.
            if v == LlmVerdict::Failed {
                if let Some(c) = ctx {
                    if procedural_learning_enabled(db) {
                        // Persistir desde el buffer ANSI-stripped (procedural_gotchas vuelve a
                        // scrubear con scrub_buffer ANTES de tocar la DB; doble defensa de privacidad).
                        let stripped: Vec<String> =
                            buffer_tail.iter().map(|l| strip_ansi(l)).collect();
                        let _ = crate::services::procedural_gotchas::persist_failure_from_verdict(
                            db, c, &stripped,
                        );
                    }
                }
            }
            v.to_verdict()
        }
        None => heuristic, // fail-closed a la heurística (FR-005)
    }
}

/// ¿Está ON el setting `memory.procedural_learning`? Default OFF (opt-in). Gate de la persistencia
/// de failure_signals en tiempo real (HIGH 3).
fn procedural_learning_enabled(db: &Db) -> bool {
    let conn = db.lock();
    crate::settings::get(&conn, "memory.procedural_learning")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

async fn llm_verdict(db: &Db, tail: &str) -> Option<LlmVerdict> {
    let base = crate::services::aie_endpoint::resolve_url_arc(db);
    if !crate::bases::allowlist::url_allowed(&base) {
        tracing::debug!("done_detection llm-assist: AIE endpoint fuera de allowlist");
        return None;
    }
    let bearer = crate::services::keychain_bearer::get_bearer()?;
    let url = format!("{}/v1/infer", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "profile": "fast_small_free",
        "system": LLM_ASSIST_SYSTEM,
        "prompt": format!("Terminal tail:\n```\n{}\n```\nOne word:", tail),
        "max_tokens": 16,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    let resp = client
        .post(&url)
        .bearer_auth(bearer)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .ok()?;
    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated Keychain value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    let text = v
        .get("text")
        .and_then(|x| x.as_str())
        .or_else(|| {
            v.pointer("/choices/0/message/content")
                .and_then(|x| x.as_str())
        })
        .unwrap_or("");
    LlmVerdict::parse(text)
}

// ── 020 — MetaDecisionEngine wiring (US1, opt-in OFF por default) ───────────────
//
// Integra el trait `meta_decision::MetaDecisionEngine` en la done-detection: cuando la heurística
// regex `classify()` es AMBIGUA y el setting `orchestration.use_aie_for_meta` está ON, consulta
// el AIE free para refinar el verdict. Con OFF (default) NO se consulta nada → comportamiento
// idéntico (cero regresión, SC-001). El AIE NUNCA bloquea: ante cualquier fallo cae al verdict
// regex (FR-003). El AIE es advisory: el verdict refinado entra al MISMO process_task que la
// política de auto-confirm de 012 (un NeedsInput NUNCA se auto-confirma si es destructivo).

/// El verdict regex de `classify()` ES AMBIGUO. La heurística es confiable para Running (spinner
/// visible) y NeedsInput (prompt de permiso detectado por patrón). El caso flojo es `Idle`: un
/// prompt vacío "sin actividad y sin patrón conocido" puede en realidad ser una pregunta phraseada
/// fuera de la tabla de patrones, o el agente pensando sin spinner. SÓLO refinamos ese caso
/// (FR-007: no inflar latencia/volumen consultando cuando la regex ya es concluyente).
fn regex_verdict_is_ambiguous(v: Verdict) -> bool {
    matches!(v, Verdict::Idle)
}

/// ¿Está habilitado el feature `orchestration.use_aie_for_meta`? Default `false` (opt-in, FR-002).
pub fn aie_meta_enabled(db: &Db) -> bool {
    let conn = db.lock();
    crate::settings::get(&conn, "orchestration.use_aie_for_meta")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Mapea el `meta_decision::Verdict` (contrato del engine) al `Verdict` de done-detection.
fn from_meta(v: crate::services::meta_decision::Verdict) -> Verdict {
    use crate::services::meta_decision::Verdict as M;
    match v {
        M::Running => Verdict::Running,
        M::Idle => Verdict::Idle,
        M::NeedsInput => Verdict::NeedsInput,
    }
}

// ── 036 — selección de engine LOCAL (offline) vs AIE vs HeuristicFallback ──────────
//
// El meta-orquestador (020) ahora puede correr con un modelo LOCAL (Ollama loopback) además del AIE
// free. La selección es por 3 gates MUTUAMENTE EXCLUYENTES (sin doble inferencia) y opt-in
// conservador: `local_engine ON` → `LocalMetaDecision`; si no, `aie_meta ON` → `AieMetaDecision`;
// si no → `HeuristicFallback` (default OFF = comportamiento actual intacto, cero regresión).

const DEFAULT_OLLAMA_ENDPOINT: &str = "http://127.0.0.1:11434";
const DEFAULT_OLLAMA_MODEL: &str = "qwen2.5:3b";

/// ¿Está habilitado el motor LOCAL `orchestration.meta_decision.local_engine`? Default `false`
/// (opt-in, FR-004). Mismo patrón que `aie_meta_enabled`.
pub fn local_meta_enabled(db: &Db) -> bool {
    let conn = db.lock();
    crate::settings::get(&conn, "orchestration.meta_decision.local_engine")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Endpoint Ollama (`meta_decision.ollama_endpoint`, default `http://127.0.0.1:11434`). El gate
/// duro loopback-only vive en `LocalMetaDecision` (FR-007): un endpoint no-loopback acá NO abre el
/// SSRF — el engine lo rechaza igual y degrada a `None`.
pub fn get_ollama_endpoint(db: &Db) -> String {
    let conn = db.lock();
    crate::settings::get(&conn, "meta_decision.ollama_endpoint")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OLLAMA_ENDPOINT.to_string())
}

/// Modelo Ollama (`meta_decision.ollama_model`, default `qwen2.5:3b`).
pub fn get_ollama_model(db: &Db) -> String {
    let conn = db.lock();
    crate::settings::get(&conn, "meta_decision.ollama_model")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(str::to_string))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string())
}

/// Qué motor seleccionarían los 3 gates (para `build_meta_engine` y para test directo, FR-003).
/// `Local` > `Aie` > `Heuristic` (los gates son if/else mutuamente excluyentes — nunca doble
/// inferencia). Default (ambos OFF) → `Heuristic` (comportamiento actual intacto).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaEngineKind {
    Local,
    Aie,
    Heuristic,
}

/// Resuelve el motor por los 3 gates (sin construirlo) — testeable con DB en memoria.
pub fn select_meta_engine_kind(db: &Db) -> MetaEngineKind {
    if local_meta_enabled(db) {
        MetaEngineKind::Local
    } else if aie_meta_enabled(db) {
        MetaEngineKind::Aie
    } else {
        MetaEngineKind::Heuristic
    }
}

/// Construye el `Box<dyn MetaDecisionEngine>` según los 3 gates (FR-003). Reusado por
/// `refine_verdict_with_aie` (US1) y los comandos advisory US2/US3 — el contrato del trait es el
/// mismo, así que el caller no distingue local↔AIE↔fallback. NO hardcodea ni loguea el bearer; cada
/// engine ya sanitiza, cachea, audita y NUNCA propaga `Err` (todo fallo ⇒ `None`).
///   - `local_engine ON` → `LocalMetaDecision` (Ollama loopback, sin bearer, allowlist loopback).
///   - si no, `aie_meta ON` → `AieMetaDecision` (AIE free, bearer Keychain, allowlist general).
///   - si no → `HeuristicFallback` (siempre `None` → el caller usa su veredicto base).
pub fn build_meta_engine(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
) -> Box<dyn crate::services::meta_decision::MetaDecisionEngine> {
    let sink: std::sync::Arc<dyn crate::services::meta_decision::MetaAudit> =
        std::sync::Arc::new(crate::services::meta_decision::AuditWriterSink {
            writer: audit.clone(),
        });
    match select_meta_engine_kind(db) {
        MetaEngineKind::Local => {
            let endpoint = get_ollama_endpoint(db);
            let model = get_ollama_model(db);
            Box::new(crate::services::meta_decision::LocalMetaDecision::new(
                endpoint, model, sink,
            ))
        }
        MetaEngineKind::Aie => {
            let base = crate::services::aie_endpoint::resolve_url_arc(db);
            let bearer = crate::services::keychain_bearer::get_bearer();
            Box::new(crate::services::meta_decision::AieMetaDecision::new(
                base, bearer, sink,
            ))
        }
        MetaEngineKind::Heuristic => {
            Box::new(crate::services::meta_decision::HeuristicFallback)
        }
    }
}

/// Refina el verdict regex con el AIE cuando (a) la regex es ambigua y (b) el feature está ON.
/// Devuelve el verdict refinado, o el regex original ante CUALQUIER fallo / feature OFF / no
/// ambiguo / engine None. El AIE NUNCA bloquea ni rompe (FR-003). El sanitizer y el audit los
/// maneja el engine (`meta_decision`). Async pero bounded por el timeout de 3s del engine; el
/// `await` real sólo ocurre en el raro tick post-debounce de un `Idle` ambiguo con el feature ON.
///
/// Orden de gates (#6 del audit): la comprobación de ambigüedad va PRIMERO porque es un `matches!`
/// puro de costo cero y, cuando el verdict NO es ambiguo (la mayoría de los ticks), corta ANTES de
/// tocar el DB. Así el lock del setting SÓLO se paga en el caso ambiguo (Idle post-debounce, baja
/// frecuencia) — contención despreciable, no se cachea en memoria para no tener que invalidar cuando
/// el user togglea el setting en vivo.
///
/// 036: Gate 2 ahora habilita si CUALQUIER motor está ON (local O AIE) — `select_meta_engine_kind`
/// decide cuál. Con ambos OFF (default) → `Heuristic` → corta sin construir engine ni tocar la red
/// (cero regresión, SC-001/AC-3).
pub async fn refine_verdict_with_aie(
    db: &Db,
    audit: &crate::bases::audit::AuditWriter,
    buffer_tail: &[String],
    cli: CliKind,
    regex_verdict: Verdict,
) -> Verdict {
    // Gate 1: la regex ya es concluyente → no consultamos NI tocamos el DB (FR-007, #6).
    if !regex_verdict_is_ambiguous(regex_verdict) {
        return regex_verdict;
    }
    // Gate 2: ningún motor ON (default) → comportamiento idéntico, sin consultar (SC-001). Sólo acá,
    // tras pasar el gate de ambigüedad, se paga el `db.lock()` del setting. `Heuristic` ⇒ corta sin
    // construir el engine (y sin red): `HeuristicFallback` siempre devolvería `None` de todos modos.
    let kind = select_meta_engine_kind(db);
    if kind == MetaEngineKind::Heuristic {
        return regex_verdict;
    }

    let engine = build_meta_engine(db, audit);

    let tail = buffer_tail
        .iter()
        .rev()
        .take(TAIL_LINES)
        .rev()
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .join("\n");
    let cli_str = match cli {
        CliKind::Claude => "claude",
        CliKind::Codex => "codex",
        CliKind::Aider => "aider",
        CliKind::Gemini => "gemini",
        CliKind::Generic => "generic",
    };

    // `engine` es un `Box<dyn MetaDecisionEngine>` → los métodos del trait se invocan por el
    // vtable del trait-object sin necesidad de importar el trait (036: era concreto antes).
    let out = engine.classify_done(&tail, cli_str).await;

    // 048 Cost-Router Fase 1 (Savings Meter) — instrumentación fire-and-forget: cuando una
    // meta-decisión se resolvió en un tier local/free (en lugar de un modelo premium), registramos la
    // traza para MEDIR el ahorro. Es OBSERVACIONAL (no cambia el verdict ni la decisión de tier) y el
    // `emit_global` es no-op si el meter está OFF (default) → cero overhead/regresión. Los tokens de
    // esta meta-llamada no se exponen acá, así que la traza va sin tokens → `baseline = None` (no
    // cuenta para el ahorro agregado; la fila queda para que Fase 2 la enriquezca). NUNCA bloquea.
    if out.is_some() {
        let decision = match kind {
            MetaEngineKind::Local => Some(crate::services::savings_meter::Decision::Local),
            MetaEngineKind::Aie => Some(crate::services::savings_meter::Decision::Free),
            MetaEngineKind::Heuristic => None, // ya cortamos arriba; defensa en profundidad
        };
        if let Some(decision) = decision {
            crate::services::savings_meter::emit_global(
                crate::services::savings_meter::RoutingEvent {
                    decision,
                    model_id: None,
                    provider: None,
                    tokens_in: None,
                    tokens_out: None,
                    cost_real_usd: Some(0.0),
                    cost_baseline_premium_usd: None,
                    price_table_version: None,
                    baseline_is_default: false,
                },
            );
        }
    }

    match out {
        Some(v) => from_meta(v), // el motor (local o AIE) refina (advisory)
        None => regex_verdict,   // fallback total a la heurística
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::orchestration::{self as orch, TaskSpec};
    use std::sync::Mutex as StdMutex;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    // ── classify (PURO) — fixtures por CLI ────────────────────────────────────

    #[test]
    fn claude_spinner_is_running() {
        let buf = lines(&["● Reading files…", "⠋ Thinking", "  esc to interrupt"]);
        assert_eq!(classify(&buf, CliKind::Claude), Verdict::Running);
    }

    #[test]
    fn claude_idle_prompt_is_idle() {
        let buf = lines(&["● Done. I refactored the auth module.", "", "> "]);
        assert_eq!(classify(&buf, CliKind::Claude), Verdict::Idle);
    }

    #[test]
    fn claude_trust_prompt_is_needs_input() {
        let buf = lines(&[
            "Do you want to make this edit to auth.rs?",
            "❯ 1. Yes",
            "  2. No",
        ]);
        assert_eq!(classify(&buf, CliKind::Claude), Verdict::NeedsInput);
    }

    #[test]
    fn codex_working_is_running() {
        let buf = lines(&["Working… ↑ 1234 tokens", "esc to interrupt"]);
        assert_eq!(classify(&buf, CliKind::Codex), Verdict::Running);
    }

    #[test]
    fn codex_run_command_is_needs_input() {
        let buf = lines(&["Allow Codex to run `cargo test`?", "[y/n]"]);
        assert_eq!(classify(&buf, CliKind::Codex), Verdict::NeedsInput);
    }

    #[test]
    fn aider_add_files_is_needs_input() {
        let buf = lines(&["Add the files to the chat? (Y)es/(N)o [Yes]:"]);
        assert_eq!(classify(&buf, CliKind::Aider), Verdict::NeedsInput);
    }

    #[test]
    fn gemini_apply_change_is_needs_input() {
        let buf = lines(&["Apply this change? (y/N)"]);
        assert_eq!(classify(&buf, CliKind::Gemini), Verdict::NeedsInput);
    }

    #[test]
    fn generic_empty_prompt_is_idle() {
        let buf = lines(&["task complete", "", "$ "]);
        assert_eq!(classify(&buf, CliKind::Generic), Verdict::Idle);
    }

    #[test]
    fn needs_input_overrides_spinner() {
        // Un trust prompt con el footer "esc to interrupt" todavía visible → NeedsInput.
        let buf = lines(&["Do you want to proceed?", "❯ 1. Yes", "esc to interrupt"]);
        assert_eq!(classify(&buf, CliKind::Claude), Verdict::NeedsInput);
    }

    #[test]
    fn ansi_is_stripped_before_classify() {
        let buf = lines(&["\x1b[32m● Done\x1b[0m", "\x1b[2m> \x1b[0m"]);
        assert_eq!(classify(&buf, CliKind::Claude), Verdict::Idle);
    }

    #[test]
    fn empty_buffer_is_running_not_idle() {
        // Arranque (pantalla limpia) NO debe marcarse idle.
        assert_eq!(classify(&[], CliKind::Claude), Verdict::Running);
        assert_eq!(
            classify(&lines(&["", "  ", ""]), CliKind::Claude),
            Verdict::Running
        );
    }

    #[test]
    fn braille_spinner_running() {
        for frame in ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] {
            let buf = lines(&[&format!("{} loading", frame)]);
            assert_eq!(
                classify(&buf, CliKind::Generic),
                Verdict::Running,
                "frame {frame}"
            );
        }
    }

    #[test]
    fn llm_verdict_parses_keywords() {
        assert_eq!(LlmVerdict::parse("SUCCESS"), Some(LlmVerdict::Success));
        assert_eq!(
            LlmVerdict::parse("  question  "),
            Some(LlmVerdict::Question)
        );
        assert_eq!(
            LlmVerdict::parse("{\"verdict\":\"WORKING\"}"),
            Some(LlmVerdict::Working)
        );
        assert_eq!(
            LlmVerdict::parse("The agent FAILED with an error"),
            Some(LlmVerdict::Failed)
        );
        assert_eq!(
            LlmVerdict::parse("not a SUCCESS, it's a QUESTION"),
            Some(LlmVerdict::Question)
        );
        assert_eq!(LlmVerdict::parse("blah blah"), None);
    }

    #[test]
    fn llm_verdict_maps_to_heuristic_verdict() {
        assert_eq!(LlmVerdict::Working.to_verdict(), Verdict::Running);
        assert_eq!(LlmVerdict::Question.to_verdict(), Verdict::NeedsInput);
        assert_eq!(LlmVerdict::Success.to_verdict(), Verdict::Idle);
        assert_eq!(LlmVerdict::Failed.to_verdict(), Verdict::Idle);
    }

    #[test]
    fn cli_from_mode_maps_prefix() {
        assert_eq!(cli_from_mode(Some("claude-A")), CliKind::Claude);
        assert_eq!(cli_from_mode(Some("codex_fast")), CliKind::Codex);
        assert_eq!(cli_from_mode(Some("zsh")), CliKind::Generic);
        assert_eq!(cli_from_mode(None), CliKind::Generic);
    }

    // ── Poller — fake PaneBuffer + DB real (in-memory) ────────────────────────

    struct FakePane {
        tail: StdMutex<Vec<String>>,
        writes: StdMutex<Vec<Vec<u8>>>,
        alive: bool,
    }
    impl FakePane {
        fn new(tail: Vec<String>) -> Self {
            Self {
                tail: StdMutex::new(tail),
                writes: StdMutex::new(vec![]),
                alive: true,
            }
        }
        fn set_tail(&self, t: Vec<String>) {
            *self.tail.lock().unwrap() = t;
        }
        fn write_count(&self) -> usize {
            self.writes.lock().unwrap().len()
        }
    }
    impl PaneBuffer for FakePane {
        fn tail(&self, _pane_id: &str) -> Vec<String> {
            self.tail.lock().unwrap().clone()
        }
        fn write(&self, _pane_id: &str, data: &[u8]) -> Result<()> {
            self.writes.lock().unwrap().push(data.to_vec());
            Ok(())
        }
        fn alive(&self, _pane_id: &str) -> bool {
            self.alive
        }
    }

    fn test_db() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/023_signals.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql"))
            .unwrap();
        // 019 F3: columna paused_at (pause/resume) — el SELECT de row_to_task ya la pide.
        conn.execute_batch(include_str!(
            "../../migrations/037_orch_pause_council_history.sql"
        ))
        .unwrap();
        // settings table (para auto_confirm_global / no romper get()).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        // 033 U3 — tabla de descartes persistentes (para el gate de supresión del poller).
        conn.execute_batch(include_str!("../../migrations/046_attention_dismissed.sql"))
            .unwrap();
        // 038 F1.0 — DAG schema (dag_blocked column que `claim_for_launch` referencia + tablas).
        conn.execute_batch(include_str!("../../migrations/047_pipeline_dag.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn running_task(db: &Db, cli: &str) -> orch::OrchTask {
        let (_b, tasks) = orch::create_batch(
            db,
            "b",
            "/tmp/repo",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: Some(format!("{cli}-A")),
            }],
        )
        .unwrap();
        let id = tasks[0].id.clone();
        let pane_id = format!("orch-{id}");
        orch::mark_running(db, &id, "/tmp/repo/.wt/a", Some(&pane_id)).unwrap();
        // cachear cli_kind como hace el launch real
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE orchestration_tasks SET cli_kind=?2 WHERE id=?1",
                rusqlite::params![id, cli],
            )
            .unwrap();
        }
        orch::get_task(db, &id).unwrap().unwrap()
    }

    fn audit_for(db: &Db) -> crate::bases::audit::AuditWriter {
        // events table viene de 001; en in-memory no la tenemos, pero AuditWriter.write
        // devuelve Result y el poller ignora el error (`let _ =`). Para no romper, creamos
        // una tabla events mínima compatible con el INSERT del writer.
        {
            let conn = db.lock();
            let _ = conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS events (
                    id TEXT PRIMARY KEY, kind TEXT, actor TEXT, pane_id TEXT, card_id TEXT,
                    correlation_id TEXT, payload TEXT, created_at TEXT DEFAULT (datetime('now')));",
            );
        }
        crate::bases::audit::AuditWriter::new(db.clone())
    }

    #[tokio::test]
    async fn idle_transitions_to_awaiting_review_after_debounce() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        let pane = FakePane::new(lines(&["● Done.", "> "]));
        let mut ps = PollerState::new();

        // Primeros (N-1) ticks Idle: NO transiciona (debounce).
        for _ in 0..(IDLE_DEBOUNCE_TICKS - 1) {
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
            assert_eq!(a, TickAction::None);
            assert_eq!(
                orch::get_task(&db, &task.id).unwrap().unwrap().state,
                "running"
            );
        }
        // Tick N: transiciona.
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(a, TickAction::AwaitingReview);
        assert_eq!(
            orch::get_task(&db, &task.id).unwrap().unwrap().state,
            "awaiting_review"
        );
    }

    // ── 020 — el refinamiento AIE está CABLEADO en el flujo real (process_task), no sólo en
    // tests del helper (audit finding #1). Con el feature ON y un verdict regex Idle ambiguo,
    // process_task consulta el engine post-debounce; acá apuntamos `endpoints.aie` a un host FUERA
    // de la allowlist → el engine corta en el gate de allowlist (None, SIN red) → fallback al
    // verdict regex (Idle) → transición a awaiting_review. Prueba que el path async se ejecuta
    // sin tumbar el comportamiento base ni hacer I/O real. ──────────────────────────────────────
    #[tokio::test]
    async fn idle_with_feature_on_refines_via_process_task_then_falls_back() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('orchestration.use_aie_for_meta', 'true')",
                [],
            )
            .unwrap();
            // host NO permitido → classify_done() devuelve None en el gate de allowlist (sin red).
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('endpoints.aie', '\"http://blocked.invalid:1\"')",
                [],
            )
            .unwrap();
        }
        let pane = FakePane::new(lines(&["● Done.", "> "])); // regex → Idle
        let mut ps = PollerState::new();
        // debounce: los primeros N-1 ticks NO transicionan.
        for _ in 0..(IDLE_DEBOUNCE_TICKS - 1) {
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            assert_eq!(
                process_task(&db, &audit, &pane, &mut ps, &cur).await,
                TickAction::None
            );
        }
        // tick N: refinamiento ON pero engine None (allowlist) → fallback Idle → awaiting_review.
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(
            a,
            TickAction::AwaitingReview,
            "fallback al verdict regex Idle"
        );
        assert_eq!(
            orch::get_task(&db, &task.id).unwrap().unwrap().state,
            "awaiting_review"
        );
    }

    #[tokio::test]
    async fn debounce_resets_on_activity_no_false_idle() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        let pane = FakePane::new(lines(&["● Done.", "> "]));
        let mut ps = PollerState::new();

        // 1 tick idle…
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::None
        );
        // …luego el agente vuelve a trabajar (spinner) → resetea el streak.
        pane.set_tail(lines(&["⠋ Thinking", "esc to interrupt"]));
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::None
        );
        // …vuelve idle: debe necesitar IDLE_DEBOUNCE_TICKS COMPLETOS de nuevo.
        pane.set_tail(lines(&["● Done.", "> "]));
        for _ in 0..(IDLE_DEBOUNCE_TICKS - 1) {
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            assert_eq!(
                process_task(&db, &audit, &pane, &mut ps, &cur).await,
                TickAction::None
            );
        }
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::AwaitingReview
        );
    }

    #[tokio::test]
    async fn growing_buffer_not_idle_until_stable() {
        // Audit F4: a silent command still emitting output (buffer GROWS, no spinner) must
        // NOT be marked awaiting_review prematurely — the idle debounce requires the buffer
        // to be STABLE, not merely "no activity keyword".
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        let pane = FakePane::new(lines(&["wrote file0"]));
        let mut ps = PollerState::new();
        for i in 0..(IDLE_DEBOUNCE_TICKS + 2) {
            pane.set_tail(lines(&[&format!("wrote file{i}")])); // buffer cambia cada tick
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            assert_eq!(
                process_task(&db, &audit, &pane, &mut ps, &cur).await,
                TickAction::None,
                "buffer cambiante NO es idle-estable"
            );
        }
        // Ahora el buffer queda quieto → tras IDLE_DEBOUNCE_TICKS estables transiciona.
        pane.set_tail(lines(&["wrote final", "> "]));
        let mut last = TickAction::None;
        for _ in 0..IDLE_DEBOUNCE_TICKS {
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            last = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        }
        assert_eq!(
            last,
            TickAction::AwaitingReview,
            "buffer estable → awaiting_review"
        );
    }

    #[tokio::test]
    async fn needs_input_off_emits_signal_no_write() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "codex");
        let pane = FakePane::new(lines(&["Allow Codex to run `ls`?", "[y/n]"]));
        let mut ps = PollerState::new();

        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(a, TickAction::InputRequested);
        assert_eq!(pane.write_count(), 0, "auto-confirm OFF no escribe");
        // sigue running + flag needs_input ON
        let t = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(t.state, "running");
        assert_eq!(t.needs_input, 1);
        // emitió agent.input_requested
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_events WHERE type='agent.input_requested'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn needs_input_on_auto_confirms_writes_enter() {
        // Audit F2: auto-confirm fires ONLY for a SAFE workspace-trust prompt.
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        set_auto_confirm(&db, &task.id, true).unwrap();
        let pane = FakePane::new(lines(&[
            "Do you trust the files in this folder?",
            "Press enter to continue",
        ]));
        let mut ps = PollerState::new();

        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(a, TickAction::AutoConfirmed);
        assert_eq!(
            pane.write_count(),
            1,
            "safe trust prompt + auto-confirm ON escribe Enter"
        );
        assert_eq!(pane.writes.lock().unwrap()[0], b"\r");
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orch_auto_confirms WHERE task_id=?1",
                rusqlite::params![task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn destructive_prompt_not_auto_confirmed_even_when_on() {
        // Audit F2 (constitution VI): a command-exec / [y/n] prompt is NEVER auto-confirmed,
        // even with auto-confirm ON — it falls through to the human (no Enter written).
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "codex");
        set_auto_confirm(&db, &task.id, true).unwrap();
        let pane = FakePane::new(lines(&["Allow Codex to run `rm -rf build`?", "[y/n]"]));
        let mut ps = PollerState::new();

        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(
            a,
            TickAction::InputRequested,
            "destructive prompt → human, not auto-confirm"
        );
        assert_eq!(
            pane.write_count(),
            0,
            "NO Enter written to a destructive prompt"
        );
        let t = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(t.needs_input, 1, "still flagged needs_input for the human");
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orch_auto_confirms WHERE task_id=?1",
                rusqlite::params![task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "no auto-confirm recorded");
    }

    #[tokio::test]
    async fn safe_prompt_auto_confirmed_once_not_every_tick() {
        // Audit codex HIGH: while the SAME safe prompt persists, Enter is sent at most ONCE
        // (fresh transition), not every tick.
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        set_auto_confirm(&db, &task.id, true).unwrap();
        let pane = FakePane::new(lines(&[
            "Do you trust this folder?",
            "press enter to continue",
        ]));
        let mut ps = PollerState::new();

        // Tick 1: fresh → confirms once.
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::AutoConfirmed
        );
        // Tick 2: same prompt still visible, needs_input now 1 → NOT fresh → no second Enter.
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(a, TickAction::InputRequested);
        assert_eq!(
            pane.write_count(),
            1,
            "only ONE Enter for a persisting prompt"
        );
    }

    #[tokio::test]
    async fn auto_confirm_throttled_after_cap() {
        // With the fresh-gate, repeated confirms require repeated FRESH transitions
        // (prompt clears → reappears). Simulate that by resetting needs_input each round.
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        set_auto_confirm(&db, &task.id, true).unwrap();
        let pane = FakePane::new(lines(&[
            "Do you trust this folder?",
            "press enter to continue",
        ]));
        let mut ps = PollerState::new();

        for _ in 0..AUTO_CONFIRM_MAX_PER_MIN {
            let _ = set_needs_input(&db, &task.id, false); // simula prompt re-aparecido (fresh)
            let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
            assert_eq!(
                process_task(&db, &audit, &pane, &mut ps, &cur).await,
                TickAction::AutoConfirmed
            );
        }
        // El siguiente fresh cae a throttled → input_requested.
        let _ = set_needs_input(&db, &task.id, false);
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        let a = process_task(&db, &audit, &pane, &mut ps, &cur).await;
        assert_eq!(a, TickAction::AutoConfirmThrottled);
        assert_eq!(pane.write_count() as i64, AUTO_CONFIRM_MAX_PER_MIN);
    }

    #[tokio::test]
    async fn global_auto_confirm_setting_applies() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "codex");
        // flag por-tarea OFF, pero global ON.
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('orchestration.auto_confirm_global', 'true')",
                [],
            )
            .unwrap();
        }
        // Safe trust prompt (F2: sólo el subconjunto seguro se auto-confirma); el punto del
        // test es que el SETTING GLOBAL (no el flag por-tarea) habilita el auto-confirm.
        let pane = FakePane::new(lines(&[
            "Do you trust this folder?",
            "press enter to continue",
        ]));
        let mut ps = PollerState::new();
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::AutoConfirmed
        );
    }

    #[tokio::test]
    async fn tick_only_processes_running_tasks() {
        let db = test_db();
        let audit = audit_for(&db);
        let t1 = running_task(&db, "claude");
        // t2 queda pending (no running).
        orch::create_batch(
            &db,
            "b2",
            "/tmp/repo2",
            None,
            None,
            &[TaskSpec {
                title: "P".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let pane = FakePane::new(lines(&["⠋ working", "esc to interrupt"]));
        let mut ps = PollerState::new();
        let touched = tick(&db, &audit, &pane, &mut ps, None, None, None).await;
        assert_eq!(touched, 1, "sólo la running se procesa");
        assert_eq!(
            orch::get_task(&db, &t1.id).unwrap().unwrap().state,
            "running"
        );
    }

    /// 030 F0-wire — `tick` encola en la cola de atención cuando una tarea necesita input
    /// (NeedsInput → InputRequested con auto-confirm OFF) — fuente autoritativa y always-on. NO mueve
    /// el foco (sólo encola).
    #[tokio::test]
    async fn tick_enqueues_attention_on_needs_input() {
        use crate::services::attention::{AttentionQueue, Priority};
        let db = test_db();
        let audit = audit_for(&db);
        let t = running_task(&db, "codex");
        // Prompt de permiso (NeedsInput, NO auto-confirmable) → InputRequested con auto-confirm OFF.
        let pane = FakePane::new(lines(&["Allow Codex to run `ls`?", "[y/n]"]));
        let mut ps = PollerState::new();
        let q = AttentionQueue::new();
        let touched = tick(&db, &audit, &pane, &mut ps, Some(&q), None, None).await;
        assert_eq!(touched, 1);
        // El pane de la tarea quedó encolado con prioridad NeedsInput (bloqueante).
        assert_eq!(q.pending_count(), 1);
        let e = q.next_by_priority().unwrap();
        assert_eq!(e.priority, Priority::NeedsInput);
        assert_eq!(e.pane_id, format!("orch-{}", t.id));
        // Sin cola (None) no rompe ni encola.
        let mut ps2 = PollerState::new();
        let _ = tick(&db, &audit, &pane, &mut ps2, None, None, None).await;
    }

    /// 031 F1b — `tick` propone audio al AudioManager con opt-in ON → se reproduce un aviso (TTS para
    /// NeedsInput). El event_id lleva el pane (estable entre ticks).
    #[tokio::test]
    async fn tick_emits_audio_when_opted_in() {
        use crate::services::attention::{AttentionQueue, Priority};
        use crate::services::audio_attention::{
            AudioManager, AudioRequest, AudioSink, MonotonicClock,
        };
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct RecSink {
            plays: Arc<Mutex<Vec<AudioRequest>>>,
        }
        impl AudioSink for RecSink {
            fn play(&self, req: &AudioRequest) {
                self.plays.lock().unwrap().push(req.clone());
            }
            fn cancel(&self) {}
        }
        let db = test_db();
        let audit = audit_for(&db);
        let t = running_task(&db, "codex");
        let pane = FakePane::new(lines(&["Allow Codex to run `ls`?", "[y/n]"]));
        let q = AttentionQueue::new();
        let sink = RecSink::default();
        let mgr = AudioManager::new(
            Box::new(sink.clone()),
            Box::new(|_: &str| true), // opt-in ON
            Box::new(MonotonicClock::default()),
        );
        let mut ps = PollerState::new();
        let touched = tick(&db, &audit, &pane, &mut ps, Some(&q), Some(&mgr), None).await;
        assert_eq!(touched, 1);
        let plays = sink.plays.lock().unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].priority, Priority::NeedsInput);
        assert!(plays[0].event_id.contains(&format!("orch-{}", t.id)));
    }

    /// 031 F1b — opt-in OFF (default) ⇒ silencio total: `tick` no reproduce nada aunque encole.
    #[tokio::test]
    async fn tick_silent_when_opted_out() {
        use crate::services::attention::AttentionQueue;
        use crate::services::audio_attention::{
            AudioManager, AudioRequest, AudioSink, MonotonicClock,
        };
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct RecSink {
            plays: Arc<Mutex<Vec<AudioRequest>>>,
        }
        impl AudioSink for RecSink {
            fn play(&self, req: &AudioRequest) {
                self.plays.lock().unwrap().push(req.clone());
            }
            fn cancel(&self) {}
        }
        let db = test_db();
        let audit = audit_for(&db);
        let _t = running_task(&db, "codex");
        let pane = FakePane::new(lines(&["Allow Codex to run `ls`?", "[y/n]"]));
        let q = AttentionQueue::new();
        let sink = RecSink::default();
        let mgr = AudioManager::new(
            Box::new(sink.clone()),
            Box::new(|_: &str| false), // opt-in OFF (default real)
            Box::new(MonotonicClock::default()),
        );
        let mut ps = PollerState::new();
        let _ = tick(&db, &audit, &pane, &mut ps, Some(&q), Some(&mgr), None).await;
        // Encoló en la cola visual (F0) pero NO sonó.
        assert_eq!(q.pending_count(), 1);
        assert_eq!(sink.plays.lock().unwrap().len(), 0);
    }

    /// 033 U3 — el poller respeta el descarte PERSISTENTE: si el pane fue descartado y no tuvo
    /// actividad nueva (`dismissed_at >= task.updated_at`) NO se encola; si hubo actividad nueva
    /// (`updated_at > dismissed_at`) reaparece y el descarte se borra.
    #[tokio::test]
    async fn tick_respects_persistent_dismissal() {
        use crate::services::attention::AttentionQueue;
        let db = test_db();
        let audit = audit_for(&db);
        let t = running_task(&db, "codex");
        let pane = FakePane::new(lines(&["Allow Codex to run `ls`?", "[y/n]"]));
        let q = AttentionQueue::new();
        let pid = format!("orch-{}", t.id);
        // dismissed_at en el FUTURO (lexicográfico) → suprime el enqueue.
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO attention_dismissed(pane_id, dismissed_at) VALUES (?1, '9999-12-31T23:59:59+00:00')",
                [&pid],
            )
            .unwrap();
        }
        let mut ps = PollerState::new();
        let touched = tick(&db, &audit, &pane, &mut ps, Some(&q), None, None).await;
        assert_eq!(touched, 1);
        assert_eq!(q.pending_count(), 0, "descartado sin actividad nueva → no encola");
        // dismissed_at en el PASADO → reaparece + borra el descarte.
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE attention_dismissed SET dismissed_at='2000-01-01T00:00:00+00:00' WHERE pane_id=?1",
                [&pid],
            )
            .unwrap();
        }
        let mut ps2 = PollerState::new();
        tick(&db, &audit, &pane, &mut ps2, Some(&q), None, None).await;
        assert_eq!(q.pending_count(), 1, "actividad nueva → reaparece");
        let conn = db.lock();
        let remain: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM attention_dismissed WHERE pane_id=?1",
                [&pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remain, 0, "el descarte se borró al reaparecer");
    }

    // ── 020 — refine_verdict_with_aie: feature OFF = sin cambio (SC-001) ──────────

    #[tokio::test]
    async fn refine_with_feature_off_returns_regex_verdict() {
        // Sin el setting (default OFF), el refinamiento es un no-op: devuelve EXACTO el verdict
        // regex, sin tocar la red (cero regresión).
        let db = test_db();
        let audit = audit_for(&db);
        let buf = lines(&["● Done.", "> "]); // regex → Idle
        let out = refine_verdict_with_aie(&db, &audit, &buf, CliKind::Claude, Verdict::Idle).await;
        assert_eq!(out, Verdict::Idle, "feature OFF no cambia el verdict regex");
    }

    #[tokio::test]
    async fn refine_skips_when_regex_conclusive() {
        // Aun con el feature ON, un verdict NO ambiguo (Running/NeedsInput) NO consulta el AIE
        // (FR-007) → devuelve el regex tal cual. (No hay bearer/red en test; el punto es que el
        // gate de ambigüedad corta antes de cualquier intento.)
        let db = test_db();
        let audit = audit_for(&db);
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('orchestration.use_aie_for_meta', 'true')",
                [],
            )
            .unwrap();
        }
        let buf = lines(&["⠋ Thinking", "esc to interrupt"]);
        let out =
            refine_verdict_with_aie(&db, &audit, &buf, CliKind::Claude, Verdict::Running).await;
        assert_eq!(out, Verdict::Running, "verdict concluyente no se refina");
        let out2 =
            refine_verdict_with_aie(&db, &audit, &buf, CliKind::Codex, Verdict::NeedsInput).await;
        assert_eq!(out2, Verdict::NeedsInput);
    }

    #[test]
    fn ambiguity_gate_only_idle() {
        assert!(regex_verdict_is_ambiguous(Verdict::Idle));
        assert!(!regex_verdict_is_ambiguous(Verdict::Running));
        assert!(!regex_verdict_is_ambiguous(Verdict::NeedsInput));
    }

    #[tokio::test]
    async fn running_clears_stale_needs_input_flag() {
        let db = test_db();
        let audit = audit_for(&db);
        let task = running_task(&db, "claude");
        set_needs_input(&db, &task.id, true).unwrap();
        let pane = FakePane::new(lines(&["⠋ Thinking", "esc to interrupt"]));
        let mut ps = PollerState::new();
        let cur = orch::get_task(&db, &task.id).unwrap().unwrap();
        assert_eq!(
            process_task(&db, &audit, &pane, &mut ps, &cur).await,
            TickAction::None
        );
        assert_eq!(
            orch::get_task(&db, &task.id).unwrap().unwrap().needs_input,
            0
        );
    }

    // ── 020 US2/US3 — build_meta_engine + gate (los commands advisory descansan en esto) ──

    #[test]
    fn aie_meta_gate_off_by_default() {
        // El setting NO está seteado → default OFF. Los commands US2/US3 hacen este check
        // PRIMERO y devuelven None sin tocar la red (SC-001: comportamiento actual intacto).
        let db = test_db();
        assert!(!aie_meta_enabled(&db), "default OFF cuando no está seteado");
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('orchestration.use_aie_for_meta', 'false')",
                [],
            )
            .unwrap();
        }
        assert!(!aie_meta_enabled(&db), "explícitamente false → OFF");
    }

    #[tokio::test]
    async fn build_meta_engine_rank_and_classify_fall_back_to_none_off_allowlist() {
        // Con el feature ON pero el endpoint AIE FUERA de la allowlist, el engine corta en el gate
        // de allowlist (sin red) → rank_variants/classify_task devuelven None (FR-003). Es el mismo
        // path que toman meta_suggest_variant_ranking / meta_suggest_agent cuando el AIE no es
        // alcanzable: el comando degrada a None (sin sugerencia), sin romperse.
        let db = test_db();
        let audit = audit_for(&db);
        {
            let conn = db.lock();
            // AIE ON + endpoint bloqueado → build_meta_engine devuelve AieMetaDecision (036:
            // select_meta_engine_kind = Aie), que corta en el gate de allowlist.
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('orchestration.use_aie_for_meta', 'true')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('endpoints.aie', '\"http://blocked.invalid:1\"')",
                [],
            )
            .unwrap();
        }
        assert_eq!(select_meta_engine_kind(&db), MetaEngineKind::Aie);
        let engine = build_meta_engine(&db, &audit);
        // US2 — ranking: diffs CRUDOS al engine (él sanitiza); endpoint bloqueado → None.
        let ranking = engine
            .rank_variants("implementá X", &["diff a".into(), "diff b".into()])
            .await;
        assert_eq!(
            ranking, None,
            "AIE inalcanzable → sin sugerencia de ranking"
        );
        // US3 — clasificación: endpoint bloqueado → None.
        let agent = engine.classify_task("arreglá el bug del login").await;
        assert_eq!(agent, None, "AIE inalcanzable → sin sugerencia de agente");
    }

    // ── 036 — selección de engine por 3 gates (local → AIE → HeuristicFallback) ───────────────

    #[test]
    fn local_meta_gate_off_by_default() {
        // Default OFF (opt-in conservador, FR-004) — sin setting y con `false` explícito.
        let db = test_db();
        assert!(!local_meta_enabled(&db), "default OFF cuando no está seteado");
        {
            let conn = db.lock();
            crate::settings::set(
                &conn,
                "orchestration.meta_decision.local_engine",
                &serde_json::json!(false),
            )
            .unwrap();
        }
        assert!(!local_meta_enabled(&db), "explícitamente false → OFF");
    }

    #[test]
    fn ollama_settings_defaults_and_overrides() {
        // Defaults (sin setting) — loopback + qwen2.5:3b (FR-004).
        let db = test_db();
        assert_eq!(get_ollama_endpoint(&db), "http://127.0.0.1:11434");
        assert_eq!(get_ollama_model(&db), "qwen2.5:3b");
        {
            let conn = db.lock();
            crate::settings::set(
                &conn,
                "meta_decision.ollama_endpoint",
                &serde_json::json!("http://127.0.0.1:11999"),
            )
            .unwrap();
            crate::settings::set(
                &conn,
                "meta_decision.ollama_model",
                &serde_json::json!("llama3.2:1b"),
            )
            .unwrap();
            // Un valor en blanco vuelve al default (no se permite endpoint/model vacío).
            crate::settings::set(&conn, "meta_decision.ollama_endpoint", &serde_json::json!("  "))
                .unwrap();
        }
        // El override de modelo se respeta; el endpoint en blanco cae al default.
        assert_eq!(get_ollama_model(&db), "llama3.2:1b");
        assert_eq!(get_ollama_endpoint(&db), "http://127.0.0.1:11434");
    }

    #[test]
    fn three_gates_select_correct_engine() {
        // AC-3: con ambos OFF (default) → Heuristic (comportamiento actual intacto).
        let db = test_db();
        assert_eq!(select_meta_engine_kind(&db), MetaEngineKind::Heuristic);

        // Sólo AIE ON → Aie.
        {
            let conn = db.lock();
            crate::settings::set(&conn, "orchestration.use_aie_for_meta", &serde_json::json!(true))
                .unwrap();
        }
        assert_eq!(select_meta_engine_kind(&db), MetaEngineKind::Aie);

        // Local ON tiene PRIORIDAD sobre AIE (gates mutuamente excluyentes, sin doble inferencia).
        {
            let conn = db.lock();
            crate::settings::set(
                &conn,
                "orchestration.meta_decision.local_engine",
                &serde_json::json!(true),
            )
            .unwrap();
        }
        assert_eq!(
            select_meta_engine_kind(&db),
            MetaEngineKind::Local,
            "local ON gana aunque AIE también esté ON"
        );

        // Local ON, AIE OFF → sigue siendo Local.
        {
            let conn = db.lock();
            crate::settings::set(&conn, "orchestration.use_aie_for_meta", &serde_json::json!(false))
                .unwrap();
        }
        assert_eq!(select_meta_engine_kind(&db), MetaEngineKind::Local);
    }

    #[tokio::test]
    async fn build_local_engine_non_loopback_endpoint_is_none_no_bypass() {
        // FR-007 end-to-end por el wiring: local ON + endpoint NO-loopback (Tailscale) configurado
        // → build_meta_engine arma un LocalMetaDecision que RECHAZA el endpoint (loopback-only) →
        // classify_done devuelve None. El flag local NO abre el SSRF a un host arbitrario aunque la
        // allowlist general lo aceptara.
        let db = test_db();
        let audit = audit_for(&db);
        {
            let conn = db.lock();
            crate::settings::set(
                &conn,
                "orchestration.meta_decision.local_engine",
                &serde_json::json!(true),
            )
            .unwrap();
            crate::settings::set(
                &conn,
                "meta_decision.ollama_endpoint",
                &serde_json::json!("http://100.64.0.10:11434"),
            )
            .unwrap();
        }
        assert_eq!(select_meta_engine_kind(&db), MetaEngineKind::Local);
        let engine = build_meta_engine(&db, &audit);
        assert_eq!(
            engine.classify_done("ambiguous tail", "claude").await,
            None,
            "endpoint no-loopback → el motor local degrada a None (sin bypass SSRF)"
        );
    }

    #[tokio::test]
    async fn refine_with_local_off_is_no_change() {
        // AC-3 (cero regresión): con todos los motores OFF, refine_verdict_with_aie devuelve el
        // verdict regex sin tocar la red — idéntico al comportamiento actual.
        let db = test_db();
        let audit = audit_for(&db);
        let buf = lines(&["> "]);
        let out = refine_verdict_with_aie(&db, &audit, &buf, CliKind::Claude, Verdict::Idle).await;
        assert_eq!(out, Verdict::Idle, "todos los motores OFF → sin cambio");
    }

    // ── 025 HIGH 3 — persistencia del Failed en tiempo real (gate + integración) ──────────────

    // El gate `memory.procedural_learning` controla si done_detection persiste failure_signals.
    #[test]
    fn procedural_learning_gate_reads_setting() {
        let db = test_db();
        assert!(!procedural_learning_enabled(&db), "default OFF");
        {
            let conn = db.lock();
            crate::settings::set(&conn, "memory.procedural_learning", &serde_json::json!(true)).unwrap();
        }
        assert!(procedural_learning_enabled(&db), "ON tras setear el setting");
    }

    // Mapeo LlmVerdict::Failed -> Idle se preserva (revisión humana, no auto-fail) — sin red.
    #[test]
    fn failed_verdict_still_maps_to_idle() {
        assert_eq!(LlmVerdict::Failed.to_verdict(), Verdict::Idle);
    }

    // Integración del path de persistencia (HIGH 3): persistir un Failed desde un tail con artefacto
    // crea un failure_signal NO resuelto que luego se resuelve con un fix de la misma región.
    #[test]
    fn high3_persist_then_resolve_roundtrip() {
        use crate::services::memory_autocapture::SessionCtx;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/042_procedural_gotchas.sql"))
            .unwrap();
        let db = Arc::new(parking_lot::Mutex::new(conn));
        let ctx = SessionCtx {
            pane_id: "p1".into(),
            cli_kind: "claude".into(),
            project_key: "furx".into(),
            session_id: "s1".into(),
        };
        // done_detection vería este tail con un Failed; persiste el fallo en tiempo real.
        let tail = vec![
            "error[E0599]: no method found in src/lib.rs".to_string(),
            "the agent stopped".to_string(),
        ];
        let id = crate::services::procedural_gotchas::persist_failure_from_verdict(&db, &ctx, &tail)
            .expect("persiste con artefacto");
        // un fix posterior de la misma región lo resuelve.
        let fix = vec!["Compiled successfully in src/lib.rs".to_string()];
        let resolved = crate::services::procedural_gotchas::correlate_persisted_failures(
            &db, "s1", &fix,
        );
        assert_eq!(resolved, vec![id]);
    }
}
