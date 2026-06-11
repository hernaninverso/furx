//! 030 F0 · Voz como lente del inbox de atención — NÚCLEO PURO (cola con prioridad + foco
//! humano-otorgado + parseo/resolución de comandos de foco). Sin audio (F1), sin red, sin DB.
//!
//! PRINCIPIO NON-NEGOTIABLE (spec 030): los agentes PIDEN atención (encolan), NUNCA la AGARRAN. El
//! foco del micrófono es UNO SOLO y lo CONCEDE el humano explícitamente — `MicFocus` SÓLO se muta vía
//! `grant_focus`, que llamará el handler de un comando HUMANO (voz/tecla), jamás la ruta de un agente.
//!
//! Decisiones del council-review (clarify 030):
//!  - Cola con PRIORIDAD (NeedsInput bloqueante > has-result informativo), NO FIFO puro — para no
//!    enterrar lo bloqueante. A igual prioridad, FIFO por orden de llegada (`seq` monótono).
//!  - Resolución de nombres por exact + alias; sin match → `None` (nunca elige uno al azar).
//!  - F0 SIN audio (el audio opt-in es F1).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Clase de atención que reclama un pane. El orden del enum ES el orden de prioridad (mayor variante
/// = más urgente) — `NeedsInput` (bloqueante: el agente espera input) gana a `HasResult` (informativo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// El pane terminó y tiene un resultado para revisar (informativo).
    HasResult = 1,
    /// El pane está bloqueado esperando input humano (urgente).
    NeedsInput = 2,
}

/// Una entrada en la cola de atención: un pane que reclama al humano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionEntry {
    /// Id único monótono (también sirve de desempate FIFO a igual prioridad).
    pub seq: u64,
    pub pane_id: String,
    pub priority: Priority,
    /// `true` = ya consumida por el humano (no aparece en `peek_all`/`next_by_priority`).
    pub attended: bool,
}

#[derive(Default)]
struct QueueInner {
    entries: Vec<AttentionEntry>,
}

/// Cola de atención en memoria, thread-safe. Reemplaza/precede al uso del incidents inbox como
/// backing store (wiring posterior). `next_by_priority` es single-winner bajo concurrencia (lock).
pub struct AttentionQueue {
    inner: Mutex<QueueInner>,
    seq: AtomicU64,
}

impl Default for AttentionQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl AttentionQueue {
    pub fn new() -> Self {
        AttentionQueue {
            inner: Mutex::new(QueueInner::default()),
            seq: AtomicU64::new(1),
        }
    }

    /// Encola un pedido de atención. Si el MISMO pane ya está en la cola sin atender, se ACTUALIZA su
    /// prioridad al MÁXIMO (no duplica): un pane que pasa de has-result a needs-input sube, nunca baja
    /// por una señal vieja. Devuelve el `seq` de la entrada vigente.
    pub fn enqueue(&self, pane_id: &str, priority: Priority) -> u64 {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g
            .entries
            .iter_mut()
            .find(|e| !e.attended && e.pane_id == pane_id)
        {
            if priority > e.priority {
                e.priority = priority;
            }
            return e.seq;
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        g.entries.push(AttentionEntry {
            seq,
            pane_id: pane_id.to_string(),
            priority,
            attended: false,
        });
        seq
    }

    /// Devuelve y MARCA como atendida la entrada de MAYOR prioridad (a igual prioridad, la más
    /// antigua por `seq`). Single-winner: dos llamadas concurrentes nunca devuelven la misma entrada.
    pub fn next_by_priority(&self) -> Option<AttentionEntry> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Elegir índice de la mejor: max por (priority, luego menor seq).
        let best = g
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.attended)
            .min_by(|(_, a), (_, b)| {
                // mayor prioridad primero; a igual prioridad, menor seq primero
                b.priority
                    .cmp(&a.priority)
                    .then(a.seq.cmp(&b.seq))
            })
            .map(|(i, _)| i);
        let i = best?;
        g.entries[i].attended = true;
        Some(g.entries[i].clone())
    }

    /// Todas las entradas sin atender, ordenadas por prioridad desc, luego seq asc.
    pub fn peek_all(&self) -> Vec<AttentionEntry> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<AttentionEntry> =
            g.entries.iter().filter(|e| !e.attended).cloned().collect();
        out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.seq.cmp(&b.seq)));
        out
    }

    /// Marca una entrada como atendida sin consumir el foco (ack manual desde la UI/voz).
    pub fn ack(&self, seq: u64) -> bool {
        self.ack_pane(seq).is_some()
    }

    /// 033 U3 — como `ack`, pero devuelve el `pane_id` de la entrada recién atendida (para persistir el
    /// descarte). `None` si la seq no existe o ya estaba atendida.
    pub fn ack_pane(&self, seq: u64) -> Option<String> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let e = g.entries.iter_mut().find(|e| e.seq == seq && !e.attended)?;
        e.attended = true;
        Some(e.pane_id.clone())
    }

    /// Descarta (marca atendidas) las entradas de panes que ya NO están vivos (audit codex #5): un
    /// pane muerto encolado no debe poder recibir el foco vía `Next`. `live` = ids de panes vivos.
    pub fn drop_dead(&self, live: &HashSet<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for e in g.entries.iter_mut() {
            if !e.attended && !live.contains(&e.pane_id) {
                e.attended = true;
            }
        }
    }

    /// Cantidad de entradas sin atender (para badges/tests).
    pub fn pending_count(&self) -> usize {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.entries.iter().filter(|e| !e.attended).count()
    }
}

/// PRUEBA DE COMANDO HUMANO (witness). Audit codex F0: la invariante "ningún agente roba el foco" se
/// ENFORCEA por tipos, no por comentario — `grant_focus` exige este testigo, y el testigo SÓLO se
/// construye en el límite de input humano verificado (`from_human_input`, `pub(crate)`): el handler de
/// una transcripción de voz parseada o de un hotkey. La ruta de un agente no tiene un `HumanCommand` a
/// mano sin pasar por ese límite. Zero-size, no clonable arbitrariamente fuera del crate.
#[derive(Debug)]
pub struct HumanCommand(());

impl HumanCommand {
    /// Construir SÓLO desde el límite de input humano verificado. Es PRIVADO del módulo `attention`
    /// (audit codex F0: para cerrar al máximo, el constructor vive en el mismo módulo que el handler
    /// humano `attention_command` y los tests — NADIE fuera de `attention.rs` puede acuñarlo, ni
    /// siquiera otro código del crate). `execute_focus_command`/`grant_focus` exigen el witness por
    /// valor → fuera de este módulo NO se puede mover el foco.
    fn from_human_input() -> Self {
        HumanCommand(())
    }
}

/// Foco del micrófono: el pane al que va el próximo dictado/transcripción. SÓLO `grant_focus` lo
/// muta, y exige un `HumanCommand` (testigo de input humano). NINGÚN agente accede a este state.
#[derive(Default)]
pub struct MicFocus {
    pane_id: Mutex<Option<String>>,
}

impl MicFocus {
    pub fn new() -> Self {
        Self::default()
    }

    /// ÚNICO mutador del foco. Exige `HumanCommand` (witness): no se puede cambiar el foco sin un
    /// comando humano. Devuelve el pane que queda con foco.
    pub fn grant_focus(&self, pane_id: &str, _proof: HumanCommand) -> String {
        let mut g = self.pane_id.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(pane_id.to_string());
        pane_id.to_string()
    }

    /// Pane con foco actual (None si nadie).
    pub fn current(&self) -> Option<String> {
        self.pane_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

// ── Comandos de foco (voz/tecla) ──────────────────────────────────────────────

/// Comando de navegación de foco parseado de una transcripción de voz (o un atajo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusCommand {
    /// "siguiente": consumir el de mayor prioridad de la cola y darle el foco.
    Next,
    /// "andá a {nombre}": mover el foco al pane nombrado.
    GoTo(String),
    /// "quién me necesita": leer la cola (F2 lee por TTS; F0 puede mostrarla).
    WhoNeedsMe,
    /// 032 U2 — "callar"/"silencio": silenciar el audio de avisos en curso. NO mueve foco ni cola.
    Silence,
    /// 032 U3 — "léeme el resultado de {nombre}": leer en voz alta un resumen SEGURO del resultado del
    /// pane nombrado (summarize+redact). NO mueve el foco del mic.
    ReadResult(String),
}

/// Parsea una transcripción de voz a un `FocusCommand`. Tolera variantes comunes en español/inglés.
/// Devuelve `None` si no es un comando de foco (la transcripción va al pane enfocado como dictado).
pub fn parse_focus_command(transcript: &str) -> Option<FocusCommand> {
    let t = transcript.trim().to_lowercase();
    let t = t.trim_end_matches(['.', '?', '!', ',']).trim();
    // "siguiente" / "próximo" / "next"
    if matches!(t, "siguiente" | "próximo" | "proximo" | "next" | "el siguiente") {
        return Some(FocusCommand::Next);
    }
    // 032 U2 — "callar" / "silencio" / "silence": silenciar el audio de avisos. Frase EXPLÍCITA.
    if matches!(t, "callar" | "silencio" | "silence" | "callate" | "cállate") {
        return Some(FocusCommand::Silence);
    }
    // 032 U3 — "léeme el resultado de {nombre}": leer el resultado del pane nombrado. Frase EXPLÍCITA.
    for prefix in [
        "léeme el resultado de ",
        "leeme el resultado de ",
        "leéme el resultado de ",
        "lee el resultado de ",
        "leé el resultado de ",
        "read the result of ",
        "read result of ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(FocusCommand::ReadResult(name.to_string()));
            }
        }
    }
    // "quién me necesita" / "who needs me"
    if matches!(
        t,
        "quién me necesita" | "quien me necesita" | "who needs me" | "quién me necesita ahora"
    ) {
        return Some(FocusCommand::WhoNeedsMe);
    }
    // GoTo: SÓLO frases EXPLÍCITAS de foco (audit codex F0: `ve a`/`ir a` son dictado normal —
    // "ve a revisar el bug", "ir a producción" — y daban falsos positivos). Se restringe a verbos
    // inequívocos de navegación de pane. Defensa en capas: aunque algo se parsee mal, GoTo NO mueve
    // el foco si el nombre no resuelve a un pane (ver `execute_focus_command`).
    for prefix in [
        "andá a ", "anda a ", "foco a ", "cambiá a ", "cambia a ", "go to ", "focus on ",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(FocusCommand::GoTo(name.to_string()));
            }
        }
    }
    None
}

// ── Ejecución de un comando de foco (ÚNICO punto donde cambia el foco) ─────────

/// Resultado de ejecutar un comando de foco humano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FocusOutcome {
    /// El foco se movió a este pane.
    Focused(String),
    /// "andá a X" pero X no resolvió a un pane (o ambiguo) → el foco NO se movió (safety).
    NoMatch(String),
    /// "siguiente" pero la cola está vacía → el foco NO se movió.
    QueueEmpty,
    /// "quién me necesita": la cola actual por prioridad (la lee el TTS en F2; F0 la muestra).
    Listed(Vec<AttentionEntry>),
    /// 032 U2 — "callar": el audio de avisos se silenció. NO cambió el foco.
    Silenced,
    /// 032 U3 — "léeme el resultado de X": se está leyendo el resultado de este pane (label). NO movió
    /// el foco del mic.
    ReadingResult(String),
    /// 032 U3 — "léeme el resultado de X" pero el pane no tiene resultado para leer (label/nombre).
    NoResult(String),
}

/// ÚNICO punto donde el foco del micrófono cambia. EXIGE el witness `HumanCommand` en su FIRMA
/// (audit codex F0 ronda 2: antes lo acuñaba internamente y, siendo `pub`, un consumidor podía mover
/// el foco sin prueba humana — bypass del witness). Ahora `proof` se RECIBE y se mueve hacia
/// `grant_focus`; como `HumanCommand` sólo se construye con `from_human_input()` (`pub(crate)`), un
/// consumidor EXTERNO al crate no puede mover el foco, y dentro del crate sólo el handler humano
/// (voz/hotkey) lo acuña (greppable). Safety de capas: `GoTo` mueve el foco SÓLO si el nombre
/// resuelve EXACTAMENTE a un pane (sino `NoMatch`, sin tocar el foco; el `proof` se descarta).
pub fn execute_focus_command(
    cmd: &FocusCommand,
    queue: &AttentionQueue,
    focus: &MicFocus,
    panes: &[PaneRef],
    proof: HumanCommand,
) -> FocusOutcome {
    match cmd {
        FocusCommand::Next => match queue.next_by_priority() {
            Some(e) => FocusOutcome::Focused(focus.grant_focus(&e.pane_id, proof)),
            None => FocusOutcome::QueueEmpty, // `proof` se descarta sin mover el foco
        },
        FocusCommand::GoTo(name) => match resolve_pane(name, panes) {
            Some(pid) => FocusOutcome::Focused(focus.grant_focus(&pid, proof)),
            None => FocusOutcome::NoMatch(name.clone()), // `proof` se descarta
        },
        FocusCommand::WhoNeedsMe => FocusOutcome::Listed(queue.peek_all()), // sin cambio de foco
        // 032 U2 — `Silence` lo INTERCEPTA `attention_command` (tiene acceso al AudioManager) ANTES de
        // llegar acá; este arm es defensivo (exhaustividad) y NO mueve el foco (descarta `proof`).
        FocusCommand::Silence => FocusOutcome::Silenced,
        // 032 U3 — `ReadResult` también lo INTERCEPTA `attention_command` (necesita DB + tts). Arm
        // defensivo: NO mueve el foco, reporta "sin resultado" (no puede leer la DB desde acá).
        FocusCommand::ReadResult(name) => FocusOutcome::NoResult(name.clone()),
    }
}

/// Un pane navegable: su id + el label visible + alias para matchear por voz. El FRONT lo pasa en
/// `attention_command` (es quien conoce los labels derivados de perfil/cuenta).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneRef {
    pub pane_id: String,
    pub label: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Resuelve un nombre dicho ("codex", "claude a") a un `pane_id`, por exact (case-insensitive) sobre
/// el label o algún alias. Council MEDIA: sin match → `None` (nunca elige uno al azar). Si hay
/// AMBIGÜEDAD (varios matchean) → `None` también (que el humano desambigüe), nunca adivinar.
pub fn resolve_pane(name: &str, panes: &[PaneRef]) -> Option<String> {
    let needle = name.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let matches: HashSet<&str> = panes
        .iter()
        .filter(|p| {
            p.label.to_lowercase() == needle
                || p.aliases.iter().any(|a| a.to_lowercase() == needle)
        })
        .map(|p| p.pane_id.as_str())
        .collect();
    if matches.len() == 1 {
        matches.into_iter().next().map(|s| s.to_string())
    } else {
        None // 0 = no encontrado; >1 = ambiguo → el humano desambigua
    }
}

// ── 030 F0-wire: comandos Tauri ───────────────────────────────────────────────

/// El front llama esto cuando un pane reclama atención (lo deriva de los eventos `awaiting_review`/
/// `needs_input` que YA recibe para el inbox). `priority`: `"has_result"` | `"needs_input"`. Safe.
#[tauri::command]
pub fn attention_enqueue(
    state: tauri::State<'_, crate::AppState>,
    pane_id: String,
    priority: Priority,
) -> u64 {
    state.attention.enqueue(&pane_id, priority)
}

/// Lista la cola de atención por prioridad (para badges / "quién me necesita"). Safe (read).
#[tauri::command]
pub fn attention_list(state: tauri::State<'_, crate::AppState>) -> Vec<AttentionEntry> {
    state.attention.peek_all()
}

/// Marca una entrada como atendida sin cambiar el foco. Safe.
#[tauri::command]
pub fn attention_ack(state: tauri::State<'_, crate::AppState>, seq: u64) -> bool {
    // 033 U3 — además de marcar atendida en memoria, PERSISTE el descarte para que sobreviva el
    // reinicio (hasta que el pane tenga actividad nueva). Fail-closed: si la DB falla, el ack en
    // memoria igual valió.
    match state.attention.ack_pane(seq) {
        Some(pane_id) => {
            let conn = state.db.lock();
            record_dismissal(&conn, &pane_id);
            true
        }
        None => false,
    }
}

/// EJECUTA un comando de foco a partir de una transcripción de voz (o frase de un hotkey). Devuelve
/// `None` si la transcripción NO es un comando de foco (el front la trata como dictado para el pane
/// enfocado).
///
/// MODELO DE CONFIANZA (audit codex F0-wire — frontera del witness en IPC). El witness `HumanCommand`
/// garantiza, a nivel de TIPOS Rust, que NINGÚN código del BACKEND fuera de este módulo mueve el foco
/// — en particular la ruta de AGENTES es Rust-side (el poller `done_detection`, los handlers de
/// PTY/ACP, los subprocesos) y NO puede acuñar el witness ni llamar `execute_focus_command`. Acuñar
/// el witness en ESTE comando es legítimo porque es el límite de input HUMANO: en Tauri sólo el
/// WEBVIEW (la UI React trusted) puede invocar IPC, y los agentes corren como SUBPROCESOS sin acceso
/// a IPC — no pueden invocar `attention_command`. El front lo llama SÓLO desde el handler de la
/// transcripción de voz / del hotkey. (Si el threat model cambiara —ej. una 2ª webview no-trusted—,
/// habría que gatear este comando por `webview().label()=="main"`, como hace `window_byok` para los
/// comandos Credential.)
///
/// Defensa en profundidad (audit codex #5): se VALIDA el `pane_id` objetivo contra los panes VIVOS
/// del backend (tabla `panes`) — un `GoTo` sólo resuelve a un pane vivo (no a un id stale/manipulado
/// por el front), y un `Next` sólo enfoca panes vivos (las entradas muertas de la cola se descartan).
#[tauri::command]
pub fn attention_command(
    state: tauri::State<'_, crate::AppState>,
    transcript: String,
    panes: Vec<PaneRef>,
) -> Option<FocusOutcome> {
    let cmd = parse_focus_command(&transcript)?;
    // 032 U2 — "callar" por voz: silenciar el audio de avisos. Se intercepta acá (tiene `state.audio`)
    // ANTES de tocar la cola/foco. NO mueve el foco del mic ni la cola de atención.
    if matches!(cmd, FocusCommand::Silence) {
        state.audio.silence();
        return Some(FocusOutcome::Silenced);
    }
    // Set de panes VIVOS del backend (no confiar en los ids que pasó el front).
    let live: HashSet<String> = {
        let conn = state.db.lock();
        conn.prepare("SELECT id FROM panes")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<HashSet<String>>>()
            })
            .unwrap_or_default()
    };
    // GoTo: sólo panes vivos pueden resolver. Next: descartar de la cola las entradas de panes
    // muertos ANTES de elegir, para no enfocar un pane que ya no existe.
    let live_panes: Vec<PaneRef> = panes
        .into_iter()
        .filter(|p| live.contains(&p.pane_id))
        .collect();
    state.attention.drop_dead(&live);
    // 032 U3 — "léeme el resultado de {nombre}": se intercepta acá (necesita DB + tts). Resuelve el
    // pane por nombre (sólo vivos), busca su resultado, lo pasa por summarize+redact (privacidad:
    // NUNCA el crudo) y lo habla. NO mueve el foco del mic. Fail-closed: nombre no resuelto → NoMatch;
    // sin resultado / vacío tras redactar → NoResult.
    if let FocusCommand::ReadResult(name) = &cmd {
        let Some(pid) = resolve_pane(name, &live_panes) else {
            return Some(FocusOutcome::NoMatch(name.clone()));
        };
        let label = live_panes
            .iter()
            .find(|p| p.pane_id == pid)
            .map(|p| p.label.clone())
            .unwrap_or_else(|| name.clone());
        // Resultado del pane: el `result_summary` de la tarea de orquestación cuyo pane es `pid`.
        let raw = {
            let conn = state.db.lock();
            conn.prepare(
                "SELECT result_summary FROM orchestration_tasks WHERE pane_id = ?1 \
                 AND result_summary IS NOT NULL AND result_summary <> '' \
                 ORDER BY updated_at DESC LIMIT 1",
            )
            .and_then(|mut stmt| {
                stmt.query_row([&pid], |r| r.get::<_, String>(0))
                    .map(Some)
                    .or_else(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        other => Err(other),
                    })
            })
            .unwrap_or(None)
        };
        let Some(raw) = raw else {
            return Some(FocusOutcome::NoResult(label));
        };
        // summarize ya aplica redact_secrets internamente + acota longitud. Si queda vacío → no habla.
        let speech = crate::services::tts::summarize(&raw, 240);
        if speech.trim().is_empty() {
            return Some(FocusOutcome::NoResult(label));
        }
        // Lectura explícita pedida por el humano → Preempt (interrumpe lo que esté sonando). NO toca
        // el foco del mic. Spawn-rápido. 033 U2 — usa la voz/rate configuradas (consistente con el aviso).
        let prefs = {
            let conn = state.db.lock();
            crate::services::audio_attention::read_audio_prefs(&conn)
        };
        let pane_for_tts = pid.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::services::tts::speak_with(
                &pane_for_tts,
                &speech,
                crate::services::tts::WhenBusy::Preempt,
                prefs.voice.as_deref(),
                prefs.rate,
            )
            .await;
        });
        return Some(FocusOutcome::ReadingResult(label));
    }
    Some(execute_focus_command(
        &cmd,
        &state.attention,
        &state.mic_focus,
        &live_panes,
        HumanCommand::from_human_input(),
    ))
}

/// Pane con foco actual del micrófono (a dónde va el próximo dictado). Safe (read).
#[tauri::command]
pub fn attention_focused_pane(state: tauri::State<'_, crate::AppState>) -> Option<String> {
    state.mic_focus.current()
}

/// 032 U1 — núcleo PURO de la navegación por TECLA: dado el orden de la cola (ya por prioridad+FIFO) y
/// el pane enfocado actualmente, devuelve el SIGUIENTE pane a enfocar, ciclando.
///  - lista vacía → `None`.
///  - `current` ausente o no presente en la cola → el primero (mayor prioridad).
///  - `current` presente → el siguiente (envuelve al primero al llegar al final).
/// Es READ-ONLY: no ack, no mueve foco — eso lo decide el front (foco VISUAL, NUNCA el mic).
pub fn next_pane_after(order: &[String], current: Option<&str>) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let idx = current.and_then(|c| order.iter().position(|p| p == c));
    let next = match idx {
        Some(i) => (i + 1) % order.len(),
        None => 0,
    };
    Some(order[next].clone())
}

// ── 033 U3 · descartes persistentes (descartar = "no molestar hasta nueva actividad") ──────────────

/// Decisión PURA: ¿un pane sigue descartado? Sí sólo si fue descartado (`dismissed_at`) en un instante
/// >= la última actividad del task (`task_updated_at`). Si el task tuvo actividad NUEVA después del
/// > descarte → ya no está descartado (reaparece). Compara INSTANTES reales parseando RFC3339 (no
/// > lexicográfico — offsets distintos romperían el orden; audit codex). Si algún timestamp no parsea →
/// > `false` (fail-closed: reaparece, NUNCA silencia de más).
pub fn is_dismissed(dismissed_at: Option<&str>, task_updated_at: &str) -> bool {
    let Some(d) = dismissed_at else {
        return false;
    };
    match (parse_instant(d), parse_instant(task_updated_at)) {
        (Some(dismissed), Some(updated)) => dismissed >= updated, // compara instantes (offset-aware)
        _ => false, // timestamp ilegible → no suprimir (fail-closed)
    }
}

/// Parsea un timestamp a un instante UTC aceptando DOS formatos (bug fix 053): RFC3339
/// (`2026-06-05T12:00:00+00:00`, que escriben `record_dismissal` y `mark_running`) Y el default
/// de schema de SQLite `datetime('now')` (`2026-06-05 12:00:00`, separador espacio, sin offset, que
/// queda en `orchestration_tasks.updated_at` cuando un INSERT no setea la columna). Antes solo se
/// aceptaba RFC3339 → un `updated_at` con el default SQLite no parseaba → `is_dismissed` devolvía
/// `false` → el pane descartado se re-encolaba en cada poll (la cola se repoblaba sola, "descartar"
/// no pegaba nunca). El default SQLite es UTC, así que lo interpretamos como tal.
fn parse_instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|n| n.and_utc())
}

/// Persiste un descarte (upsert `pane_id` → ahora, RFC3339). Fail-closed: si la DB falla, el ack en
/// memoria igual ocurrió (no se pierde funcionalidad).
pub fn record_dismissal(conn: &rusqlite::Connection, pane_id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = conn.execute(
        "INSERT INTO attention_dismissed (pane_id, dismissed_at) VALUES (?1, ?2) \
         ON CONFLICT(pane_id) DO UPDATE SET dismissed_at = excluded.dismissed_at",
        rusqlite::params![pane_id, now],
    ) {
        tracing::debug!("attention_dismissed upsert falló (no fatal): {e}");
    }
}

/// Lee el `dismissed_at` persistido de un pane (o `None`). Fail-closed: cualquier error de DB → `None`
/// (no suprime). "Sin filas" es lo esperado; otros errores se loguean (observabilidad, audit codex).
pub fn read_dismissal(conn: &rusqlite::Connection, pane_id: &str) -> Option<String> {
    match conn.query_row(
        "SELECT dismissed_at FROM attention_dismissed WHERE pane_id = ?1",
        [pane_id],
        |r| r.get::<_, String>(0),
    ) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => {
            tracing::debug!("read_dismissal error (fail-closed a None): {e}");
            None
        }
    }
}

/// Borra el descarte de un pane (cuando reaparece por actividad nueva).
pub fn clear_dismissal(conn: &rusqlite::Connection, pane_id: &str) {
    let _ = conn.execute(
        "DELETE FROM attention_dismissed WHERE pane_id = ?1",
        [pane_id],
    );
}

/// 032 U1 — navegación por TECLA (⌘⇧N): devuelve el SIGUIENTE pane que reclama atención para ENFOCARLO
/// VISUALMENTE en el front. READ-ONLY: NO ack, NO `grant_focus` (NUNCA mueve el foco del mic — sólo la
/// voz lo concede). `current` = pane enfocado hoy (para ciclar). Safe.
#[tauri::command]
pub fn attention_next_pane(
    state: tauri::State<'_, crate::AppState>,
    current: Option<String>,
) -> Option<String> {
    let order: Vec<String> = state
        .attention
        .peek_all()
        .into_iter()
        .map(|e| e.pane_id)
        .collect();
    next_pane_after(&order, current.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // 032 U2 — "callar"/"silencio" parsea a Silence (frase explícita); otras frases no.
    #[test]
    fn parse_silence_command() {
        assert_eq!(parse_focus_command("callar"), Some(FocusCommand::Silence));
        assert_eq!(parse_focus_command("Silencio."), Some(FocusCommand::Silence));
        assert_eq!(parse_focus_command("silence"), Some(FocusCommand::Silence));
        assert_eq!(parse_focus_command("cállate"), Some(FocusCommand::Silence));
        // no es comando de callar (va como dictado)
        assert_eq!(parse_focus_command("callar al perro y seguir"), None);
    }

    // 032 U2 — execute con Silence NO mueve el foco (arm defensivo; el silenciado real lo hace
    // attention_command). El foco del mic queda intacto.
    #[test]
    fn execute_silence_does_not_move_focus() {
        let q = AttentionQueue::new();
        q.enqueue("a", Priority::NeedsInput);
        let f = MicFocus::new();
        let panes: Vec<PaneRef> = vec![];
        let out = execute_focus_command(
            &FocusCommand::Silence,
            &q,
            &f,
            &panes,
            HumanCommand::from_human_input(),
        );
        assert_eq!(out, FocusOutcome::Silenced);
        assert_eq!(f.current(), None); // el foco del mic NO cambió
    }

    // 032 U3 — parse de "léeme el resultado de {nombre}" extrae el nombre; variantes y rechazo.
    #[test]
    fn parse_read_result_command() {
        assert_eq!(
            parse_focus_command("léeme el resultado de Codex"),
            Some(FocusCommand::ReadResult("codex".to_string()))
        );
        assert_eq!(
            parse_focus_command("leeme el resultado de la tarea uno."),
            Some(FocusCommand::ReadResult("la tarea uno".to_string()))
        );
        assert_eq!(
            parse_focus_command("read the result of claude"),
            Some(FocusCommand::ReadResult("claude".to_string()))
        );
        // sin nombre → no es comando (va como dictado)
        assert_eq!(parse_focus_command("léeme el resultado de "), None);
        assert_eq!(parse_focus_command("léeme el cuento"), None);
    }

    // 032 U3 — PRIVACIDAD: el resumen que se hablaría (summarize) REDACTA secretos del resultado crudo.
    #[test]
    fn read_result_redacts_secrets() {
        let raw = "Listo. La API key es sk-ABCDEF0123456789XYZ y el deploy salió ok.";
        let speech = crate::services::tts::summarize(raw, 240);
        assert!(!speech.contains("sk-ABCDEF0123456789XYZ"), "el secreto no debe hablarse");
        assert!(speech.contains("[redacted]"), "el secreto se redacta");
    }

    // 033 U3 — lógica pura del descarte persistente.
    #[test]
    fn is_dismissed_logic() {
        // descartado DESPUÉS de la última actividad → sigue descartado.
        assert!(is_dismissed(
            Some("2026-06-03T10:00:00+00:00"),
            "2026-06-03T09:00:00+00:00"
        ));
        // igual instante → sigue descartado (>=).
        assert!(is_dismissed(
            Some("2026-06-03T09:00:00+00:00"),
            "2026-06-03T09:00:00+00:00"
        ));
        // actividad NUEVA después del descarte → reaparece.
        assert!(!is_dismissed(
            Some("2026-06-03T08:00:00+00:00"),
            "2026-06-03T09:00:00+00:00"
        ));
        // nunca descartado.
        assert!(!is_dismissed(None, "2026-06-03T09:00:00+00:00"));
        // OFFSETS DISTINTOS (audit codex): descarte 10:00+01:00 = 09:00 UTC es ANTERIOR a la actividad
        // 09:30+00:00 → reaparece (la comparación lexicográfica daría el resultado OPUESTO).
        assert!(!is_dismissed(
            Some("2026-06-03T10:00:00+01:00"),
            "2026-06-03T09:30:00+00:00"
        ));
        // mismo instante en offsets distintos → sigue descartado (>=).
        assert!(is_dismissed(
            Some("2026-06-03T10:00:00+01:00"),
            "2026-06-03T09:00:00+00:00"
        ));
        // timestamp ilegible → fail-closed a NO descartado (reaparece).
        assert!(!is_dismissed(Some("no-es-fecha"), "2026-06-03T09:00:00+00:00"));
        assert!(!is_dismissed(Some("2026-06-03T09:00:00+00:00"), "basura"));
    }

    // 053 fix — `task_updated_at` con el default de SQLite `datetime('now')` (formato
    // "YYYY-MM-DD HH:MM:SS", sin offset) DEBE parsearse y suprimir. Antes solo se aceptaba RFC3339 →
    // un updated_at con ese default no parseaba → is_dismissed=false → el pane descartado se
    // re-encolaba en cada poll ("descartar" no pegaba). Este es EL bug del usuario.
    #[test]
    fn is_dismissed_accepts_sqlite_datetime_format() {
        // descarte RFC3339 posterior a la actividad SQLite → sigue descartado (no reaparece).
        assert!(is_dismissed(
            Some("2026-06-03T10:00:00+00:00"),
            "2026-06-03 09:00:00" // formato datetime('now') de SQLite
        ));
        // actividad SQLite NUEVA después del descarte → reaparece.
        assert!(!is_dismissed(
            Some("2026-06-03T08:00:00+00:00"),
            "2026-06-03 09:00:00"
        ));
        // ambos en formato SQLite.
        assert!(is_dismissed(
            Some("2026-06-03 10:00:00"),
            "2026-06-03 09:00:00"
        ));
    }

    // 033 U3 — roundtrip de persistencia (record/read/clear) + upsert idempotente.
    #[test]
    fn dismissal_persistence_roundtrip() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attention_dismissed(pane_id TEXT PRIMARY KEY, dismissed_at TEXT NOT NULL);",
        )
        .unwrap();
        assert_eq!(read_dismissal(&conn, "p1"), None);
        record_dismissal(&conn, "p1");
        assert!(read_dismissal(&conn, "p1").is_some());
        record_dismissal(&conn, "p1"); // upsert: no duplica
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM attention_dismissed", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        clear_dismissal(&conn, "p1");
        assert_eq!(read_dismissal(&conn, "p1"), None);
    }

    // 032 U1 — next_pane_after: ciclo de navegación por tecla.
    #[test]
    fn next_pane_after_cycles() {
        let order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // sin current → primero
        assert_eq!(next_pane_after(&order, None).as_deref(), Some("a"));
        // current=a → b ; b → c ; c → envuelve a a
        assert_eq!(next_pane_after(&order, Some("a")).as_deref(), Some("b"));
        assert_eq!(next_pane_after(&order, Some("b")).as_deref(), Some("c"));
        assert_eq!(next_pane_after(&order, Some("c")).as_deref(), Some("a"));
        // current no presente → primero
        assert_eq!(next_pane_after(&order, Some("z")).as_deref(), Some("a"));
        // cola vacía → None
        assert_eq!(next_pane_after(&[], Some("a")), None);
    }

    /// SC-2: la cola devuelve NeedsInput (bloqueante) antes que HasResult, aunque llegue después.
    #[test]
    fn priority_needs_input_beats_has_result() {
        let q = AttentionQueue::new();
        q.enqueue("a", Priority::HasResult);
        q.enqueue("b", Priority::NeedsInput);
        let first = q.next_by_priority().unwrap();
        assert_eq!(first.pane_id, "b");
        assert_eq!(first.priority, Priority::NeedsInput);
        let second = q.next_by_priority().unwrap();
        assert_eq!(second.pane_id, "a");
    }

    /// A igual prioridad, FIFO por orden de llegada.
    #[test]
    fn equal_priority_is_fifo() {
        let q = AttentionQueue::new();
        q.enqueue("x", Priority::HasResult);
        q.enqueue("y", Priority::HasResult);
        assert_eq!(q.next_by_priority().unwrap().pane_id, "x");
        assert_eq!(q.next_by_priority().unwrap().pane_id, "y");
    }

    /// Encolar el MISMO pane no duplica; sube la prioridad al máximo (has-result → needs-input).
    #[test]
    fn enqueue_same_pane_dedups_and_escalates() {
        let q = AttentionQueue::new();
        q.enqueue("a", Priority::HasResult);
        q.enqueue("a", Priority::NeedsInput);
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.next_by_priority().unwrap().priority, Priority::NeedsInput);
        // y una señal vieja (has-result) NO baja la prioridad de una vigente needs-input.
        let q2 = AttentionQueue::new();
        q2.enqueue("a", Priority::NeedsInput);
        q2.enqueue("a", Priority::HasResult);
        assert_eq!(q2.next_by_priority().unwrap().priority, Priority::NeedsInput);
    }

    /// SC-1/SC-4: encolar NO toca el foco del micrófono (los agentes piden, no agarran).
    #[test]
    fn enqueue_never_changes_mic_focus() {
        let q = AttentionQueue::new();
        let focus = MicFocus::new();
        focus.grant_focus("pane-X", HumanCommand::from_human_input()); // el humano dicta en X
        // 3 panes reclaman atención.
        q.enqueue("a", Priority::HasResult);
        q.enqueue("b", Priority::HasResult);
        q.enqueue("c", Priority::NeedsInput);
        // El foco SIGUE en X — la cola no lo movió.
        assert_eq!(focus.current().as_deref(), Some("pane-X"));
        assert_eq!(q.pending_count(), 3);
    }

    /// SC-7: `next_by_priority` es single-winner bajo concurrencia (N threads, cada entrada 1 vez).
    #[test]
    fn next_by_priority_is_single_winner_under_concurrency() {
        let q = Arc::new(AttentionQueue::new());
        for i in 0..50 {
            q.enqueue(&format!("pane-{i}"), Priority::HasResult);
        }
        let mut handles = Vec::new();
        for _ in 0..8 {
            let q = Arc::clone(&q);
            handles.push(std::thread::spawn(move || {
                let mut got = Vec::new();
                while let Some(e) = q.next_by_priority() {
                    got.push(e.seq);
                }
                got
            }));
        }
        let mut all: Vec<u64> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        // Audit codex: probar NO-DUPLICACIÓN (len antes del dedup) Y NO-PÉRDIDA (len tras dedup) por
        // separado. 50 consumos totales, 50 seqs distintos → ningún seq consumido dos veces ni perdido.
        assert_eq!(all.len(), 50, "se consumieron != 50 entradas (pérdida o doble-consumo)");
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 50, "hubo seqs duplicados (doble-consumo)");
        assert_eq!(q.pending_count(), 0);
    }

    /// Audit codex #5: `drop_dead` saca de la cola los panes que ya no están vivos (Next no enfoca
    /// un pane muerto).
    #[test]
    fn drop_dead_removes_dead_panes() {
        let q = AttentionQueue::new();
        q.enqueue("vivo", Priority::HasResult);
        q.enqueue("muerto", Priority::NeedsInput); // más prioritario, pero morirá
        let live: HashSet<String> = ["vivo".to_string()].into_iter().collect();
        q.drop_dead(&live);
        // sólo queda "vivo"; Next lo devuelve (el "muerto" no roba el turno aunque era NeedsInput).
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.next_by_priority().unwrap().pane_id, "vivo");
    }

    /// `ack` marca atendida sin foco; no aparece más en peek_all.
    #[test]
    fn ack_removes_from_pending() {
        let q = AttentionQueue::new();
        let s = q.enqueue("a", Priority::HasResult);
        assert!(q.ack(s));
        assert!(!q.ack(s)); // idempotente: ya atendida
        assert!(!q.ack(999_999)); // seq inexistente → false, sin panic
        assert_eq!(q.pending_count(), 0);
        assert!(q.peek_all().is_empty());
    }

    /// `grant_focus` (con witness) es el único mutador; refleja el último otorgamiento humano.
    #[test]
    fn grant_focus_is_the_only_mutator() {
        let f = MicFocus::new();
        assert_eq!(f.current(), None);
        f.grant_focus("a", HumanCommand::from_human_input());
        assert_eq!(f.current().as_deref(), Some("a"));
        f.grant_focus("b", HumanCommand::from_human_input());
        assert_eq!(f.current().as_deref(), Some("b"));
    }

    /// Audit codex: el parser NO toma frases de dictado normal como comando (`ve a`/`ir a` salieron).
    #[test]
    fn parser_no_false_positives_on_dictation() {
        for dict in [
            "ve a revisar el bug del login",
            "ir a producción requiere rollback",
            "andate a dormir",
            "tengo que ir a la oficina",
            "el siguiente paso es testear", // "el siguiente" exacto es Next, pero esto NO
            "foco en el detalle pero no cambies de pane", // no empieza con prefijo exacto
        ] {
            assert_eq!(parse_focus_command(dict), None, "no debió parsear: {dict:?}");
        }
    }

    /// Audit codex: `execute_focus_command` con GoTo a un nombre que NO resuelve → NoMatch, sin tocar
    /// el foco (defensa de capas ante un parse dudoso).
    #[test]
    fn execute_goto_unresolved_does_not_move_focus() {
        let q = AttentionQueue::new();
        let f = MicFocus::new();
        f.grant_focus("X", HumanCommand::from_human_input());
        let panes = vec![PaneRef {
            pane_id: "p1".into(),
            label: "Codex".into(),
            aliases: vec![],
        }];
        let out = execute_focus_command(&FocusCommand::GoTo("produccion".into()), &q, &f, &panes, HumanCommand::from_human_input());
        assert_eq!(out, FocusOutcome::NoMatch("produccion".into()));
        assert_eq!(f.current().as_deref(), Some("X")); // foco intacto
        // GoTo que SÍ resuelve mueve el foco.
        let out2 = execute_focus_command(&FocusCommand::GoTo("codex".into()), &q, &f, &panes, HumanCommand::from_human_input());
        assert_eq!(out2, FocusOutcome::Focused("p1".into()));
        assert_eq!(f.current().as_deref(), Some("p1"));
    }

    /// `execute_focus_command` Next consume la cola por prioridad y mueve el foco; cola vacía → QueueEmpty.
    #[test]
    fn execute_next_consumes_and_focuses() {
        let q = AttentionQueue::new();
        let f = MicFocus::new();
        q.enqueue("a", Priority::HasResult);
        q.enqueue("b", Priority::NeedsInput);
        let out = execute_focus_command(&FocusCommand::Next, &q, &f, &[], HumanCommand::from_human_input());
        assert_eq!(out, FocusOutcome::Focused("b".into())); // bloqueante primero
        assert_eq!(f.current().as_deref(), Some("b"));
        execute_focus_command(&FocusCommand::Next, &q, &f, &[], HumanCommand::from_human_input()); // consume "a"
        assert_eq!(
            execute_focus_command(&FocusCommand::Next, &q, &f, &[], HumanCommand::from_human_input()),
            FocusOutcome::QueueEmpty
        );
    }

    /// `peek_all` devuelve ordenado por prioridad desc, luego seq asc.
    #[test]
    fn peek_all_is_ordered() {
        let q = AttentionQueue::new();
        q.enqueue("a", Priority::HasResult); // seq 1
        q.enqueue("b", Priority::NeedsInput); // seq 2
        q.enqueue("c", Priority::HasResult); // seq 3
        let all = q.peek_all();
        assert_eq!(all.iter().map(|e| e.pane_id.as_str()).collect::<Vec<_>>(), vec!["b", "a", "c"]);
    }

    #[test]
    fn parse_focus_commands() {
        assert_eq!(parse_focus_command("siguiente"), Some(FocusCommand::Next));
        assert_eq!(parse_focus_command("Próximo."), Some(FocusCommand::Next));
        assert_eq!(parse_focus_command("next"), Some(FocusCommand::Next));
        assert_eq!(
            parse_focus_command("andá a Codex"),
            Some(FocusCommand::GoTo("codex".into()))
        );
        assert_eq!(
            parse_focus_command("foco a claude a"),
            Some(FocusCommand::GoTo("claude a".into()))
        );
        assert_eq!(
            parse_focus_command("quién me necesita?"),
            Some(FocusCommand::WhoNeedsMe)
        );
        // dictado normal → None (va al pane enfocado).
        assert_eq!(parse_focus_command("arreglá el bug del login"), None);
        assert_eq!(parse_focus_command("andá a "), None); // sin nombre
    }

    #[test]
    fn resolve_pane_exact_alias_and_ambiguity() {
        let panes = vec![
            PaneRef {
                pane_id: "p1".into(),
                label: "Claude A".into(),
                aliases: vec!["claude-a".into(), "a".into()],
            },
            PaneRef {
                pane_id: "p2".into(),
                label: "Codex".into(),
                aliases: vec!["cx".into()],
            },
        ];
        assert_eq!(resolve_pane("codex", &panes).as_deref(), Some("p2"));
        assert_eq!(resolve_pane("Claude A", &panes).as_deref(), Some("p1"));
        assert_eq!(resolve_pane("cx", &panes).as_deref(), Some("p2"));
        assert_eq!(resolve_pane("noexiste", &panes), None); // sin match
        // ambigüedad: dos panes con el mismo alias → None (no adivinar).
        let amb = vec![
            PaneRef { pane_id: "p1".into(), label: "X".into(), aliases: vec!["dup".into()] },
            PaneRef { pane_id: "p2".into(), label: "Y".into(), aliases: vec!["dup".into()] },
        ];
        assert_eq!(resolve_pane("dup", &amb), None);
    }
}
