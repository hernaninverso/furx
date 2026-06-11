//! 031 F1a · Audio opt-in para la cola de atención — NÚCLEO PURO. Sin `tts.rs`, sin spawns, sin DB,
//! sin red. Todo el gate de emisión (opt-in → dedup → rate-limit → presupuesto) + la cola serial
//! acotada viven acá, contra un `AudioSink` y un `Clock` INYECTABLES (mock en tests). El audio real y
//! el wiring al poller/settings/tecla son F1b.
//!
//! PRINCIPIO (de 030, se mantiene): el audio es un AVISO, no un secuestro. No mueve el foco del mic
//! (eso lo gobierna `attention::MicFocus`), es opt-in (default OFF total), acotado y silenciable al
//! instante. Reusa `attention::Priority` (NeedsInput > HasResult).
//!
//! Decisiones del council-review (clarify 031) + audit-3-frontera (codex/deepseek), todas acá:
//!  - **Reservar-SÓLO-si-se-encola, bajo UN lock**: el gate evalúa y RESERVA estado (dedup, timestamps,
//!    presupuesto) atómicamente, pero ÚNICAMENTE cuando el pedido realmente entra en la cola. Así
//!    `Decision::Emit` ⟺ "reservado Y encolado": nunca consume estado un pedido que no va a sonar
//!    (cierra el blocker de codex "Emit deja de significar reservado-y-encolado").
//!  - **Gates per-clase + bypass del urgente** (blocker codex/deepseek "informativo silencia urgente"):
//!    un `HasResult` NUNCA puede agotarle el cupo a un `NeedsInput`. El rate-limit global y el
//!    presupuesto se aplican a los INFORMATIVOS; el urgente los IGNORA (sólo lo acota su rate por-pane,
//!    por-clase, + la reproducción serial). El rate por-pane es por (pane, clase): el informativo de un
//!    pane no bloquea el urgente del mismo pane.
//!  - **Cola acotada SIN crecer nunca** (blocker codex "cola a 17"): si está llena, se desaloja un
//!    pedido de prioridad ESTRICTAMENTE menor; si no hay víctima válida, el entrante se RECHAZA
//!    (`QueueFull`) sin reservar nada (queda en la cola VISUAL F0). Un urgente jamás se sacrifica por
//!    un informativo.
//!  - **Limpieza de dedup al desalojar** (blocker deepseek "fuga de dedup"): al expulsar un pedido se
//!    borra su `event_id` del dedup, para que ese evento —que nunca sonó— pueda volver a admitirse.
//!  - **event_id estable** (`pane + tipo + seq`, NO timestamp de poll) + limpieza al resolver.
//!  - **callar = cancelación REAL** (acá: `sink.cancel()` + vaciar cola; matar el proceso es F1b).

use crate::services::attention::Priority;
use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Mutex;

/// Qué tipo de audio emite un aviso. Lo determina el tipo de evento (NeedsInput→TTS, HasResult→earcon),
/// no una elección por pane (US-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioKind {
    /// Frase hablada corta (eventos bloqueantes — NeedsInput).
    Tts,
    /// Tono corto ≤1s (eventos informativos — HasResult).
    Earcon,
}

impl AudioKind {
    /// Mapeo canónico tipo-de-evento → tipo-de-audio (US-3).
    pub fn for_priority(p: Priority) -> AudioKind {
        match p {
            Priority::NeedsInput => AudioKind::Tts,
            Priority::HasResult => AudioKind::Earcon,
        }
    }
}

/// Un pedido de audio ya admitido por el gate, listo para que el sink lo reproduzca.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRequest {
    pub pane_id: String,
    pub kind: AudioKind,
    pub priority: Priority,
    /// Id ESTABLE del evento (`pane + tipo + seq`); clave de dedup. Nunca un timestamp de poll (FR-5).
    pub event_id: String,
    /// Texto a hablar (vacío para earcon). Redactado/acotado por el wiring (F1b).
    pub text: String,
}

/// Resultado del gate. `Emit` reserva estado Y encola; el resto NO toca estado (sólo informa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Admitido: reservado y encolado para emitir.
    Emit,
    /// Ese pane no activó audio (opt-in OFF) — silencio total (FR-1.2).
    OptOut,
    /// Ya se emitió audio para este `event_id` (aún vigente) — ≤1 audio por evento (FR-3.2).
    Duplicate,
    /// Rate-limit global (1/500ms) — sólo aplica a informativos.
    RateLimitGlobal,
    /// Rate-limit por (pane, clase) (1/5s).
    RateLimitPane,
    /// Presupuesto de interrupciones agotado (6/min) — sólo bloquea informativos (FR-4).
    BudgetExceeded,
    /// Cola serial llena y sin pedido de prioridad menor que desalojar — el entrante no suena (queda
    /// en la cola VISUAL F0).
    QueueFull,
}

impl Decision {
    pub fn emitted(self) -> bool {
        matches!(self, Decision::Emit)
    }
}

// ── Parámetros del gate (council-review) ──────────────────────────────────────────────────────────
const RATE_GLOBAL_MS: u64 = 500; // 1 audio / 500ms global (informativos)
const RATE_PANE_MS: u64 = 5_000; // 1 audio / 5s por (pane, clase)
const BUDGET_MAX: usize = 6; // máximo de avisos informativos…
const BUDGET_WINDOW_MS: u64 = 60_000; // …por ventana deslizante de 60s
const DEDUP_TTL_MS: u64 = 30_000; // un event_id queda "vigente" 30s (o hasta resolverse/desalojarse)
const QUEUE_CAP: usize = 16; // cola serial acotada
const LOG_THROTTLE_MS: u64 = 1_000; // ≤1 log por razón por segundo

/// Fuente de tiempo monótona en milisegundos. Inyectable para tests deterministas (los rate-limits y
/// el presupuesto dependen del tiempo).
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Reloj real (monótono, `Instant` contra un origen fijo). En F1a sólo lo usa el wiring/manual; los
/// tests usan un reloj falso avanzable.
pub struct MonotonicClock {
    origin: std::time::Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
}

/// Salida de audio. Inyectable: en tests un mock que registra play/cancel; en F1b envuelve `tts.rs` con
/// cancelación real del proceso hijo. SIEMPRE se invoca FUERA del lock del gate (fire-and-forget).
pub trait AudioSink: Send + Sync {
    /// Lanza un aviso. CONTRATO: `play` debe SPAWNEAR y retornar rápido (no bloquear por la duración
    /// del audio — `tts.rs::speak` spawnea el proceso hijo y el audio corre async). El gestor llama
    /// `play` y `cancel` mutuamente excluidos (mismo `playback` lock), así que no se solapan. Errores se
    /// degradan (FR-7); nunca panic.
    fn play(&self, req: &AudioRequest);
    /// Cancela la reproducción en curso de verdad (matar el proceso hijo / cancelar el sink) — para
    /// `callar`. Se invoca bajo el `playback` lock, nunca solapado con `play`.
    fn cancel(&self);
}

/// 033 U4 — ALLOWLIST de agentes conocidos (no sólo charset): mapea el `cli_kind` (vocabulario
/// controlado) a su nombre para mostrar. Cualquier id NO conocido → `None` ⇒ frase genérica. Más
/// estricto que `agent_label_for_tts` (que sólo valida charset); se usa donde el nombre se MUESTRA de
/// forma persistente (notificaciones), para que un `cli_kind` inesperado nunca exponga texto arbitrario.
pub fn known_agent_label(cli_kind: &str) -> Option<String> {
    match cli_kind.trim().to_lowercase().as_str() {
        "codex" => Some("Codex".to_string()),
        "claude" => Some("Claude".to_string()),
        "aider" => Some("Aider".to_string()),
        "gemini" => Some("Gemini".to_string()),
        "grok" => Some("Grok".to_string()),
        _ => None,
    }
}

/// Resolutor de opt-in por pane. Inyectable: en tests un closure/set; en F1b lee `crate::settings`.
pub trait OptInResolver: Send + Sync {
    fn opted_in(&self, pane_id: &str) -> bool;
}

impl<F: Fn(&str) -> bool + Send + Sync> OptInResolver for F {
    fn opted_in(&self, pane_id: &str) -> bool {
        self(pane_id)
    }
}

struct GateState {
    /// event_id → instante de reserva (TTL `DEDUP_TTL_MS`; se limpia al resolver o desalojar).
    dedup: HashMap<String, u64>,
    /// Instante del último audio INFORMATIVO admitido (rate global aplica sólo a informativos).
    last_global_info: Option<u64>,
    /// (pane_id, clase) → instante del último audio de esa clase en ese pane.
    last_pane: HashMap<(String, Priority), u64>,
    /// Instantes de los avisos INFORMATIVOS emitidos en la ventana del presupuesto (deslizante).
    budget: VecDeque<u64>,
    /// Cola serial acotada de pedidos admitidos. Se ordena por prioridad y FIFO al sacar.
    queue: Vec<(u64, AudioRequest)>,
    /// Contador FIFO monótono para desempate a igual prioridad.
    order: u64,
    /// Generación de cancelación: la incrementa `silence` (callar). Un pedido tomado para reproducir
    /// con generación `g` SÓLO suena si la generación sigue siendo `g` al momento de reproducir — así un
    /// `callar` ocurrido entre "tomar" y "reproducir" CANCELA ese audio (audit codex BLOCKER reproducción).
    generation: u64,
    /// Logs throttled: instante del último log por razón. Acotado al set FIJO de razones (no crece).
    last_log: HashMap<&'static str, u64>,
}

/// Gestor de audio de la cola de atención: único punto serial por donde pasa TODO el audio. El gate
/// (decisión + reserva + encolado) es atómico bajo `Mutex`; la emisión (sink) ocurre fuera del lock.
pub struct AudioManager {
    state: Mutex<GateState>,
    /// Lock de REPRODUCCIÓN: serializa `sink.play` de pumps concurrentes Y el `sink.cancel` de
    /// `silence`, de modo que play y cancel NUNCA se solapan ni se interleavean entre el chequeo de
    /// generación y el spawn (audit codex: cierra la ventana "callar entre check y play"). Como `play`
    /// es spawn-rápido (contrato del sink), `silence` lo toma sin perder instantaneidad (callar espera a
    /// lo sumo un spawn).
    playback: Mutex<()>,
    sink: Box<dyn AudioSink>,
    opt_in: Box<dyn OptInResolver>,
    clock: Box<dyn Clock>,
}

impl AudioManager {
    pub fn new(
        sink: Box<dyn AudioSink>,
        opt_in: Box<dyn OptInResolver>,
        clock: Box<dyn Clock>,
    ) -> Self {
        Self {
            state: Mutex::new(GateState {
                dedup: HashMap::new(),
                last_global_info: None,
                last_pane: HashMap::new(),
                budget: VecDeque::new(),
                queue: Vec::new(),
                order: 0,
                generation: 0,
                last_log: HashMap::new(),
            }),
            playback: Mutex::new(()),
            sink,
            opt_in,
            clock,
        }
    }

    /// Recupera el lock de estado aún si otro hilo paniqueó teniéndolo (poison-recovery): un aviso de
    /// audio nunca debe tumbar el flujo.
    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lock de reproducción (poison-recovery).
    fn playback_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.playback.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Evalúa el gate y, sólo si el pedido realmente entra en la cola, RESERVA el estado — todo atómico
    /// bajo el lock (reservar-sólo-si-se-encola). NO reproduce: eso es `pump_one`/`pump_all`, fuera del
    /// lock. `Decision::Emit` ⟺ el pedido quedó encolado.
    pub fn consider(&self, req: AudioRequest) -> Decision {
        // Opt-in se resuelve primero: OFF ⇒ silencio total, ni reserva ni cola (FR-1.2).
        if !self.opt_in.opted_in(&req.pane_id) {
            return Decision::OptOut;
        }
        let now = self.clock.now_ms();
        let urgent = req.priority == Priority::NeedsInput;
        let mut st = self.lock();

        // Prune de estado vencido (anti-fuga en uptime largo).
        st.dedup.retain(|_, &mut t| now.saturating_sub(t) < DEDUP_TTL_MS);
        while st.budget.front().is_some_and(|&t| now.saturating_sub(t) >= BUDGET_WINDOW_MS) {
            st.budget.pop_front();
        }
        st.last_pane.retain(|_, &mut t| now.saturating_sub(t) < BUDGET_WINDOW_MS);

        // 1) Dedup por event_id (≤1 audio por evento aunque el poller tickee N veces). Ambas clases.
        if st.dedup.contains_key(&req.event_id) {
            Self::log_suppressed(&mut st, "duplicate", &req, now);
            return Decision::Duplicate;
        }
        // 2) Rate-limit por (pane, clase): el informativo de un pane NO bloquea su urgente, y viceversa.
        if let Some(&p) = st.last_pane.get(&(req.pane_id.clone(), req.priority)) {
            if now.saturating_sub(p) < RATE_PANE_MS {
                Self::log_suppressed(&mut st, "rate_pane", &req, now);
                return Decision::RateLimitPane;
            }
        }
        // 3) Rate-limit global + 4) presupuesto: SÓLO informativos. El urgente los ignora (bypass) — un
        //    HasResult nunca le agota el cupo a un NeedsInput (council/audit). El urgente queda acotado
        //    por su rate por-pane (paso 2) + la reproducción serial.
        if !urgent {
            if let Some(g) = st.last_global_info {
                if now.saturating_sub(g) < RATE_GLOBAL_MS {
                    Self::log_suppressed(&mut st, "rate_global", &req, now);
                    return Decision::RateLimitGlobal;
                }
            }
            if st.budget.len() >= BUDGET_MAX {
                Self::log_suppressed(&mut st, "budget", &req, now);
                return Decision::BudgetExceeded;
            }
        }

        // 5) Placement: intentar encolar (desaloja sólo prioridad ESTRICTAMENTE menor, limpiando su
        //    dedup). Si no hay lugar ni víctima válida → RECHAZAR sin reservar nada.
        if !Self::try_place(&mut st, &req) {
            Self::log_suppressed(&mut st, "queue_full", &req, now);
            return Decision::QueueFull;
        }

        // 6) Encolado OK → RESERVAR (recién acá, así Emit ⟺ reservado-y-encolado).
        st.dedup.insert(req.event_id.clone(), now);
        st.last_pane.insert((req.pane_id.clone(), req.priority), now);
        if !urgent {
            st.last_global_info = Some(now);
            st.budget.push_back(now);
        }
        Decision::Emit
    }

    /// Intenta colocar `req` en la cola. Devuelve `true` si quedó encolado.
    ///  - Hay lugar → push.
    ///  - Llena → desaloja el pedido de prioridad ESTRICTAMENTE menor más antiguo (limpiando su dedup,
    ///    porque nunca sonó) y mete el entrante. Un urgente desplaza informativos; un informativo nunca
    ///    desplaza a un urgente.
    ///  - Llena y sin víctima de prioridad menor → `false` (el entrante no entra; queda en la cola
    ///    VISUAL F0). No reserva estado el caller.
    fn try_place(st: &mut GateState, req: &AudioRequest) -> bool {
        if st.queue.len() < QUEUE_CAP {
            let order = st.order;
            st.order += 1;
            st.queue.push((order, req.clone()));
            return true;
        }
        // Víctima: la de prioridad estrictamente menor más antigua (menor `order`).
        let victim = st
            .queue
            .iter()
            .enumerate()
            .filter(|(_, (_, r))| r.priority < req.priority)
            .min_by_key(|(_, (o, _))| *o)
            .map(|(i, _)| i);
        match victim {
            Some(i) => {
                let (_, evicted) = st.queue.remove(i);
                st.dedup.remove(&evicted.event_id); // el desalojado nunca sonó → liberar su dedup
                let order = st.order;
                st.order += 1;
                st.queue.push((order, req.clone()));
                true
            }
            None => false,
        }
    }

    /// Saca el siguiente pedido a reproducir (mayor prioridad, luego FIFO) JUNTO con la generación de
    /// cancelación vigente, atómicamente bajo el lock de estado. `None` si la cola está vacía. El play
    /// NO se hace acá (va fuera del lock, en `play_if_current`).
    fn take_for_play(&self) -> Option<(AudioRequest, u64)> {
        let mut st = self.lock();
        let idx = st
            .queue
            .iter()
            .enumerate()
            .max_by(|(_, (oa, ra)), (_, (ob, rb))| ra.priority.cmp(&rb.priority).then(ob.cmp(oa)))
            .map(|(i, _)| i)?;
        let req = st.queue.remove(idx).1;
        Some((req, st.generation))
    }

    /// Reproduce `req` SÓLO si la generación sigue siendo `gen` (no hubo `callar` desde que se tomó).
    /// Devuelve `true` si reprodujo. El caller DEBE tener el `playback` lock tomado (serialización).
    fn play_if_current(&self, req: &AudioRequest, gen: u64) -> bool {
        // Re-chequeo de generación bajo el lock de estado: cierra la ventana "tomado pero aún no
        // reproducido" frente a un `callar` concurrente (audit codex BLOCKER reproducción #2).
        if self.lock().generation != gen {
            return false;
        }
        self.sink.play(req);
        true
    }

    /// Reproduce el pedido de mayor prioridad de la cola. `true` si reprodujo algo. Toma el `playback`
    /// lock para que dos pumps concurrentes nunca solapen `sink.play` (reproducción serial — audit
    /// codex BLOCKER reproducción #1).
    pub fn pump_one(&self) -> bool {
        let _play = self.playback_lock();
        match self.take_for_play() {
            Some((req, gen)) => self.play_if_current(&req, gen),
            None => false,
        }
    }

    /// Drena la cola entera en orden de prioridad (serial). Útil para el worker y los tests.
    pub fn pump_all(&self) -> usize {
        let mut n = 0;
        while self.pump_one() {
            n += 1;
        }
        n
    }

    /// `callar`: corta la reproducción en curso DE VERDAD (`sink.cancel`) + vacía la cola pendiente +
    /// avanza la `generation`. TOMA el `playback` lock, así que NO puede interleavearse entre el
    /// chequeo de generación y el spawn de un `pump_one` en curso (cierra la ventana de carrera del
    /// audit): o bien `silence` corre antes (y el pump verá la generación avanzada → no spawnea), o
    /// corre después de que el spawn ya arrancó (y `sink.cancel` mata ese proceso). Como `play` es
    /// spawn-rápido, esperar a lo sumo un spawn mantiene a callar efectivamente instantáneo. No resetea
    /// rate-limits/dedup ni el opt-in (FR-6). Es global (independiente del opt-in).
    pub fn silence(&self) {
        let _play = self.playback_lock(); // excluye a `pump_one` (su take+check+spawn)
        {
            let mut st = self.lock();
            st.queue.clear();
            st.generation = st.generation.wrapping_add(1);
        }
        self.sink.cancel();
    }

    /// Resuelve un evento: limpia su dedup para que un NUEVO bloqueo del mismo pane pueda volver a
    /// sonar (FR-5.2). Lo llama el wiring cuando el humano atiende / el pane vuelve a running.
    pub fn resolve(&self, event_id: &str) {
        self.lock().dedup.remove(event_id);
    }

    /// Cantidad de pedidos pendientes en la cola (para tests/observabilidad).
    pub fn pending(&self) -> usize {
        self.lock().queue.len()
    }

    /// Log throttled de supresión (≤1 log por razón por `LOG_THROTTLE_MS`) — no spamear en ráfagas.
    fn log_suppressed(st: &mut GateState, reason: &'static str, req: &AudioRequest, now: u64) {
        let last = st.last_log.get(reason).copied();
        if last.is_none_or(|t| now.saturating_sub(t) >= LOG_THROTTLE_MS) {
            st.last_log.insert(reason, now);
            tracing::debug!(
                "audio_attention: aviso suprimido pane={} event={} razón={}",
                req.pane_id,
                req.event_id,
                reason
            );
        }
    }
}

// ── F1b · wiring real ─────────────────────────────────────────────────────────────────────────────

/// Texto hablado para un aviso `NeedsInput`. FIJO y SIN contenido del buffer (privacidad: nunca se
/// lee en voz alta lo que el agente imprimió — la personalización con el nombre del agente es F2).
const TTS_NEEDS_INPUT: &str = "Un agente necesita tu atención.";

/// Sonido del earcon (`HasResult`, informativo). Tono corto del sistema (macOS). Si `afplay` o el
/// archivo faltan, el earcon degrada a silencio (FR-7).
const EARCON_SOUND: &str = "/System/Library/Sounds/Glass.aiff";

/// `AudioSink` real (F1b): TTS vía `tts.rs` (mutex un-hablante) para urgentes, earcon (`afplay`) para
/// informativos. Cumple el contrato del trait: `play` SPAWNEA y retorna; `cancel` mata de verdad SIN
/// bloquear (manda señal; un watcher async mata+reapea el hijo). Plataforma: `afplay` es de macOS — en
/// otras el spawn falla → degrada a silencio (FR-7), nunca panic.
pub struct TtsEarconSink {
    /// Canal para cancelar el earcon en curso (`callar`). El TTS lo cancela `tts::stop()`. Un watcher
    /// async OWNea el `Child` del earcon y lo reapea (await `wait()`) al terminar natural o por kill —
    /// así nunca queda zombie (audit codex/deepseek/aie) y `cancel` no bloquea la UI.
    earcon_kill: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// 033 U2 — resolutor de preferencias de audio (voz/rate/earcon), leído por reproducción.
    prefs: Box<dyn AudioPrefsResolver>,
}

impl Default for TtsEarconSink {
    fn default() -> Self {
        Self::new()
    }
}

impl TtsEarconSink {
    /// Construye el sink con un resolutor de prefs. `new()` (sin resolver) usa defaults (cero regresión).
    pub fn with_prefs(prefs: Box<dyn AudioPrefsResolver>) -> Self {
        Self {
            earcon_kill: Mutex::new(None),
            prefs,
        }
    }

    pub fn new() -> Self {
        Self::with_prefs(Box::new(AudioPrefs::default))
    }

    fn kill_slot(&self) -> std::sync::MutexGuard<'_, Option<tokio::sync::oneshot::Sender<()>>> {
        self.earcon_kill.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lanza el earcon: preempta el anterior (manda su kill), spawnea `afplay` con un watcher que lo
    /// reapea. Spawn-rápido. Degrada a silencio si `afplay` no existe.
    fn play_earcon(&self) {
        let prefs = self.prefs.prefs(); // 033 U2 — volumen + sonido configurables (default = el actual)
        let kill_rx = {
            let mut slot = self.kill_slot();
            if let Some(k) = slot.take() {
                let _ = k.send(()); // preempta el earcon previo (no-op si ya terminó)
            }
            let (tx, rx) = tokio::sync::oneshot::channel();
            *slot = Some(tx);
            rx
        };
        let mut cmd = tokio::process::Command::new("afplay");
        cmd.arg("-v")
            .arg(format!("{:.3}", prefs.earcon_volume.clamp(0.0, 1.0)))
            .arg(&prefs.earcon_sound)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        match cmd.spawn() {
            Ok(mut child) => {
                // Watcher: OWNea el Child y lo reapea (await `wait()`) al terminar natural o por kill.
                tauri::async_runtime::spawn(async move {
                    tokio::select! {
                        _ = child.wait() => {}
                        _ = kill_rx => {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                        }
                    }
                });
            }
            Err(e) => tracing::debug!("earcon afplay no disponible (degrada): {e}"),
        }
    }
}

impl AudioSink for TtsEarconSink {
    fn play(&self, req: &AudioRequest) {
        match req.kind {
            AudioKind::Tts => {
                // PRIVACIDAD (audit codex): nunca se lee contenido del buffer. `req.text` SÓLO puede
                // ser un nombre de agente de la WHITELIST (032 U2). El sink REVALIDA con
                // `agent_label_for_tts` (defensa en profundidad): si pasa → "<Nombre> necesita
                // atención"; si no (vacío/raro) → la frase genérica fija. El nombre crudo nunca se
                // habla; sólo la frase construida con un label whitelisteado.
                // Spawn-rápido: `tauri::async_runtime::spawn` retorna ya. `Drop`: si ya hay alguien
                // hablando NO lo preempta (el AudioManager + el mutex de tts ya serializan).
                let phrase = agent_label_for_tts(&req.text)
                    .map(|label| format!("{label} necesita atención"))
                    .unwrap_or_else(|| TTS_NEEDS_INPUT.to_string());
                // 033 U2 — voz/rate configurables (default = sistema, cero regresión).
                let prefs = self.prefs.prefs();
                let pane = req.pane_id.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::services::tts::speak_with(
                        &pane,
                        &phrase,
                        crate::services::tts::WhenBusy::Drop,
                        prefs.voice.as_deref(),
                        prefs.rate,
                    )
                    .await;
                });
            }
            AudioKind::Earcon => self.play_earcon(),
        }
    }

    fn cancel(&self) {
        crate::services::tts::stop(); // mata el TTS en curso (no bloquea)
        if let Some(k) = self.kill_slot().take() {
            let _ = k.send(()); // señala al watcher; él mata+reapea el earcon (no bloquea acá)
        }
    }
}

/// Clave de setting del opt-in de audio por pane (default OFF).
pub fn opt_in_key(pane_id: &str) -> String {
    format!("attention.audio_opt_in.{pane_id}")
}

/// 033 U2 — preferencias de audio (voz/rate del TTS + volumen/sonido del earcon). Defaults = el
/// comportamiento de F0–F2 (cero regresión). Todo clampeado/validado al leer (fail-closed).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioPrefs {
    /// Voz del TTS (macOS `say -v`). `None`/"" = voz por defecto del sistema.
    pub voice: Option<String>,
    /// Multiplicador de velocidad del TTS, clampeado 0.5..=2.0 (default 1.0).
    pub rate: f64,
    /// Volumen del earcon (`afplay -v`), clampeado 0.0..=1.0 (default 1.0).
    pub earcon_volume: f64,
    /// Ruta del sonido del earcon (default el del sistema).
    pub earcon_sound: String,
}

impl Default for AudioPrefs {
    fn default() -> Self {
        Self {
            voice: None,
            rate: 1.0,
            earcon_volume: 1.0,
            earcon_sound: EARCON_SOUND.to_string(),
        }
    }
}

/// Lee `AudioPrefs` desde `settings` con clamp/validación. Cualquier ausencia/valor inválido → default
/// (cero regresión, fail-closed). La voz se valida por la misma whitelist de charset que los nombres
/// (`[A-Za-z0-9 ._-]`); el sonido sólo se acepta si es un archivo existente.
pub fn read_audio_prefs(conn: &rusqlite::Connection) -> AudioPrefs {
    let s = |k: &str| -> Option<String> {
        match crate::settings::get(conn, k) {
            Ok(Some(serde_json::Value::String(v))) => Some(v),
            _ => None,
        }
    };
    let num = |k: &str| -> Option<f64> {
        match crate::settings::get(conn, k) {
            Ok(Some(serde_json::Value::Number(n))) => n.as_f64(),
            _ => None,
        }
    };
    let voice = s("attention.audio.voice")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        // misma whitelist de charset que los labels: nada de control/inyección en argv.
        .filter(|v| {
            v.chars().count() <= 48
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
        });
    let rate = num("attention.audio.rate")
        .filter(|r| r.is_finite())
        .map(|r| r.clamp(0.5, 2.0))
        .unwrap_or(1.0);
    let earcon_volume = num("attention.audio.earcon_volume")
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(1.0);
    // El sonido del earcon se pasa como argumento POSICIONAL a `afplay`. Endurecido (audit codex+deepseek):
    //  - ABSOLUTO (sin traversal relativo),
    //  - no empieza con `-` (evita OPTION INJECTION: un path `-x` lo tomaría afplay como flag),
    //  - sin metacaracteres de shell (defensa en profundidad, aunque el spawn es argv-only),
    //  - archivo existente.
    // Cualquier incumplimiento → default del sistema.
    let earcon_sound = s("attention.audio.earcon_sound")
        .filter(|p| {
            let path = std::path::Path::new(p);
            path.is_absolute()
                && !p.starts_with('-')
                && !p.chars().any(|c| {
                    matches!(c, ';' | '|' | '$' | '`' | '&' | '<' | '>' | '"' | '\'' | '\n' | '\r')
                })
                && path.is_file()
        })
        .unwrap_or_else(|| EARCON_SOUND.to_string());
    AudioPrefs {
        voice,
        rate,
        earcon_volume,
        earcon_sound,
    }
}

/// Resolutor de `AudioPrefs` (inyectable: en tests un closure; en wiring lee `settings`).
pub trait AudioPrefsResolver: Send + Sync {
    fn prefs(&self) -> AudioPrefs;
}

impl<F: Fn() -> AudioPrefs + Send + Sync> AudioPrefsResolver for F {
    fn prefs(&self) -> AudioPrefs {
        self()
    }
}

/// 032 U2 — WHITELIST para personalizar el TTS con el nombre del agente. Acepta SÓLO un nombre corto
/// de charset acotado (`[A-Za-z0-9 ._-]`, 1..=32 chars tras trim) y lo devuelve capitalizado. Cualquier
/// otra cosa (vacío, largo, caracteres raros, control) → `None` ⇒ se cae a la frase genérica. NUNCA se
/// arma desde el buffer; el origen es `cli_kind` (vocabulario controlado: codex/claude/aider/gemini).
/// Se usa en DOS puntos (defensa en profundidad): el poller para llenar `AudioRequest.text`, y el sink
/// para REVALIDAR antes de hablar.
pub fn agent_label_for_tts(name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 32 {
        return None;
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
    {
        return None;
    }
    // Capitalizar la primera letra ("codex" → "Codex").
    let mut chars = n.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str())
}

/// Lee el opt-in de audio de un pane desde `settings` (default false). Usado por el `OptInResolver`
/// del wiring y por el comando getter.
pub fn read_opt_in(conn: &rusqlite::Connection, pane_id: &str) -> bool {
    matches!(
        crate::settings::get(conn, &opt_in_key(pane_id)),
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

// ── Comandos Tauri (F1b) ────────────────────────────────────────────────────────────────────────

/// `callar`: corta TODO el audio de avisos en curso + vacía la cola. Global (no depende del opt-in).
/// Safe + reversible (no destruye datos; sólo silencia). Lo dispara una tecla o el menú.
#[tauri::command]
pub fn callar(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    state.audio.silence();
    Ok(())
}

/// Activa/desactiva el audio de avisos de un pane (default OFF). Persistente (settings).
#[tauri::command]
pub fn attention_audio_opt_in_set(
    state: tauri::State<'_, crate::AppState>,
    pane_id: String,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.db.lock();
    crate::settings::set(&conn, &opt_in_key(&pane_id), &serde_json::Value::Bool(enabled))
        .map_err(|e| e.to_string())
}

/// Lee el opt-in de audio de un pane (para el toggle de la UI).
#[tauri::command]
pub fn attention_audio_opt_in_get(
    state: tauri::State<'_, crate::AppState>,
    pane_id: String,
) -> bool {
    let conn = state.db.lock();
    read_opt_in(&conn, &pane_id)
}

/// 033 U2 — lee las preferencias de audio efectivas (ya clampeadas/validadas) para la UI de config.
#[tauri::command]
pub fn attention_audio_prefs_get(state: tauri::State<'_, crate::AppState>) -> AudioPrefs {
    let conn = state.db.lock();
    read_audio_prefs(&conn)
}

/// 033 U2 — persiste las preferencias de audio. Cada campo es opcional (sólo se escribe lo provisto);
/// la validación/clamp ocurre al LEER (`read_audio_prefs`), así nunca se aplica un valor peligroso.
#[tauri::command]
pub fn attention_audio_prefs_set(
    state: tauri::State<'_, crate::AppState>,
    voice: Option<String>,
    rate: Option<f64>,
    earcon_volume: Option<f64>,
    earcon_sound: Option<String>,
) -> Result<(), String> {
    let conn = state.db.lock();
    let set = |k: &str, v: serde_json::Value| crate::settings::set(&conn, k, &v);
    if let Some(v) = voice {
        set("attention.audio.voice", serde_json::Value::String(v)).map_err(|e| e.to_string())?;
    }
    if let Some(r) = rate {
        set(
            "attention.audio.rate",
            serde_json::json!(r.clamp(0.5, 2.0)),
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(v) = earcon_volume {
        set(
            "attention.audio.earcon_volume",
            serde_json::json!(v.clamp(0.0, 1.0)),
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(s) = earcon_sound {
        set("attention.audio.earcon_sound", serde_json::Value::String(s))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// Reloj falso avanzable (tests deterministas de rate-limit/presupuesto).
    #[derive(Clone, Default)]
    struct FakeClock(Arc<AtomicU64>);
    impl FakeClock {
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// Sink mock: registra play/cancel.
    #[derive(Clone, Default)]
    struct MockSink {
        played: Arc<Mutex<Vec<AudioRequest>>>,
        cancels: Arc<AtomicU64>,
    }
    impl AudioSink for MockSink {
        fn play(&self, req: &AudioRequest) {
            self.played.lock().unwrap().push(req.clone());
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn req(pane: &str, prio: Priority, ev: &str) -> AudioRequest {
        AudioRequest {
            pane_id: pane.into(),
            kind: AudioKind::for_priority(prio),
            priority: prio,
            event_id: ev.into(),
            text: String::new(),
        }
    }

    /// Construye un manager con opt-in para un set fijo de panes.
    fn mgr(opted: &'static [&'static str]) -> (AudioManager, MockSink, FakeClock) {
        let sink = MockSink::default();
        let clock = FakeClock::default();
        let m = AudioManager::new(
            Box::new(sink.clone()),
            Box::new(move |p: &str| opted.contains(&p)),
            Box::new(clock.clone()),
        );
        (m, sink, clock)
    }

    // SC-1: opt-in=false → 0 audio.
    #[test]
    fn opt_out_emits_nothing() {
        let (m, sink, _c) = mgr(&[]);
        for i in 0..10 {
            assert_eq!(m.consider(req("p1", Priority::NeedsInput, &format!("e{i}"))), Decision::OptOut);
        }
        m.pump_all();
        assert_eq!(sink.played.lock().unwrap().len(), 0);
        assert_eq!(m.pending(), 0);
    }

    // SC-2: 5 ticks del MISMO evento → exactamente 1 audio (dedup por event_id).
    #[test]
    fn dedup_same_event_one_audio() {
        let (m, sink, c) = mgr(&["p1"]);
        for tick in 0..5 {
            let d = m.consider(req("p1", Priority::NeedsInput, "p1:needs:7"));
            if tick == 0 {
                assert_eq!(d, Decision::Emit);
            } else {
                assert_eq!(d, Decision::Duplicate);
            }
            c.advance(2_000);
        }
        assert_eq!(m.pump_all(), 1);
        assert_eq!(sink.played.lock().unwrap().len(), 1);
    }

    // SC-2 cont.: tras resolver, el mismo pane puede volver a sonar (pasados los rate-limits).
    #[test]
    fn resolve_allows_resound() {
        let (m, _s, c) = mgr(&["p1"]);
        assert_eq!(m.consider(req("p1", Priority::NeedsInput, "p1:needs:1")), Decision::Emit);
        assert_eq!(m.consider(req("p1", Priority::NeedsInput, "p1:needs:1")), Decision::Duplicate);
        m.resolve("p1:needs:1");
        // dedup limpiado → ya NO es Duplicate; el urgente ignora el rate global, pero su rate por-pane
        // (5s, por-clase) sigue vigente.
        assert_eq!(m.consider(req("p1", Priority::NeedsInput, "p1:needs:1")), Decision::RateLimitPane);
        c.advance(5_000);
        assert_eq!(m.consider(req("p1", Priority::NeedsInput, "p1:needs:1")), Decision::Emit);
    }

    // SC-3: rate-limit global aplica a informativos (10 de panes distintos en 100ms → 1).
    #[test]
    fn rate_limit_global_informational() {
        let (m, _s, c) = mgr(&["a","b","c","d","e","f","g","h","i","j"]);
        let panes = ["a","b","c","d","e","f","g","h","i","j"];
        let mut emitted = 0;
        for (k, p) in panes.iter().enumerate() {
            if m.consider(req(p, Priority::HasResult, &format!("{p}:r:{k}"))).emitted() {
                emitted += 1;
            }
            c.advance(10);
        }
        assert_eq!(emitted, 1);
    }

    // SC-3 cont.: rate-limit por (pane, clase) (5 informativos del mismo pane en 1s → 1).
    #[test]
    fn rate_limit_pane() {
        let (m, _s, c) = mgr(&["p1"]);
        let mut emitted = 0;
        for k in 0..5 {
            if m.consider(req("p1", Priority::HasResult, &format!("p1:r:{k}"))).emitted() {
                emitted += 1;
            }
            c.advance(200);
        }
        assert_eq!(emitted, 1);
    }

    // SC-4: NeedsInput tras HasResult en ráfaga NO se silencia ni se entierra (prioridad).
    #[test]
    fn needs_input_beats_has_result_in_queue() {
        let (m, sink, _c) = mgr(&["a", "b"]);
        assert_eq!(m.consider(req("a", Priority::HasResult, "a:r:1")), Decision::Emit);
        // Mismo instante: el urgente IGNORA el rate global → admitido igual (council bypass).
        assert_eq!(m.consider(req("b", Priority::NeedsInput, "b:n:1")), Decision::Emit);
        m.pump_one();
        let played = sink.played.lock().unwrap();
        assert_eq!(played[0].pane_id, "b");
        assert_eq!(played[0].priority, Priority::NeedsInput);
    }

    // AUDIT (codex): un informativo NUNCA debe silenciar a un urgente vía rate global, dentro de 500ms.
    #[test]
    fn has_result_does_not_rate_block_urgent() {
        let (m, _s, c) = mgr(&["a", "a2", "b"]);
        assert_eq!(m.consider(req("a", Priority::HasResult, "a:r:1")), Decision::Emit);
        c.advance(100); // < 500ms
        // Otro informativo SÍ se bloquea por el rate global…
        assert_eq!(m.consider(req("a2", Priority::HasResult, "a2:r:1")), Decision::RateLimitGlobal);
        // …pero un urgente NO (bypass del rate global).
        assert_eq!(m.consider(req("b", Priority::NeedsInput, "b:n:1")), Decision::Emit);
    }

    // AUDIT (codex): presupuesto lleno de informativos NO silencia a un urgente.
    #[test]
    fn budget_full_of_info_still_allows_urgent() {
        let opted: &'static [&'static str] = &["i0","i1","i2","i3","i4","i5","i6","u"];
        let (m, sink, c) = mgr(opted);
        // Llenar el presupuesto con 6 informativos (separados 600ms para pasar el rate global).
        for k in 0..6 {
            assert_eq!(
                m.consider(req(&format!("i{k}"), Priority::HasResult, &format!("i{k}:r"))),
                Decision::Emit
            );
            c.advance(600);
        }
        // Un 7º informativo (pane fresco, así no choca con el rate por-pane): presupuesto agotado.
        assert_eq!(m.consider(req("i6", Priority::HasResult, "i6:r")), Decision::BudgetExceeded);
        // PERO un urgente pasa igual (bypass de presupuesto).
        assert_eq!(m.consider(req("u", Priority::NeedsInput, "u:n:1")), Decision::Emit);
        m.pump_all();
        assert!(sink.played.lock().unwrap().iter().any(|r| r.pane_id == "u"));
    }

    // AUDIT (codex): el informativo de un pane NO bloquea el urgente del MISMO pane (rate por-clase).
    #[test]
    fn pane_info_does_not_block_pane_urgent() {
        let (m, _s, c) = mgr(&["a"]);
        assert_eq!(m.consider(req("a", Priority::HasResult, "a:r:1")), Decision::Emit);
        c.advance(100); // < 5s rate por-pane, pero distinta clase
        assert_eq!(m.consider(req("a", Priority::NeedsInput, "a:n:1")), Decision::Emit);
    }

    // SC-5: presupuesto 6/min — 20 informativos terminando juntos → ≤6 audios en el minuto.
    #[test]
    fn budget_caps_six_per_minute() {
        let opted: &'static [&'static str] = &[
            "p0","p1","p2","p3","p4","p5","p6","p7","p8","p9",
            "q0","q1","q2","q3","q4","q5","q6","q7","q8","q9",
        ];
        let (m, _s, c) = mgr(opted);
        let mut emitted = 0;
        for (k, p) in opted.iter().enumerate() {
            if m.consider(req(p, Priority::HasResult, &format!("{p}:r:{k}"))).emitted() {
                emitted += 1;
            }
            c.advance(600); // 20*600ms = 12s < 60s; supera el rate global de 500ms
        }
        assert_eq!(emitted, BUDGET_MAX);
    }

    // SC-8: concurrencia — N hilos, mismo event_id → ≤1 emite (reserva-sólo-si-se-encola bajo el lock).
    #[test]
    fn concurrent_same_event_at_most_one_emit() {
        let sink = MockSink::default();
        let clock = FakeClock::default();
        let m = Arc::new(AudioManager::new(
            Box::new(sink.clone()),
            Box::new(|_: &str| true),
            Box::new(clock.clone()),
        ));
        let emits = Arc::new(AtomicU64::new(0));
        let mut handles = vec![];
        for _ in 0..16 {
            let m = m.clone();
            let emits = emits.clone();
            handles.push(std::thread::spawn(move || {
                if m.consider(req("p", Priority::NeedsInput, "p:n:42")).emitted() {
                    emits.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(emits.load(Ordering::SeqCst), 1);
    }

    // SC-6 (núcleo): callar cancela el sink y vacía la cola.
    #[test]
    fn silence_cancels_and_clears() {
        let (m, sink, _c) = mgr(&["a", "b", "c"]);
        assert_eq!(m.consider(req("a", Priority::NeedsInput, "a:n:1")), Decision::Emit);
        assert_eq!(m.consider(req("b", Priority::NeedsInput, "b:n:1")), Decision::Emit);
        assert_eq!(m.pending(), 2);
        m.silence();
        assert_eq!(m.pending(), 0);
        assert_eq!(sink.cancels.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn kind_follows_priority() {
        assert_eq!(AudioKind::for_priority(Priority::NeedsInput), AudioKind::Tts);
        assert_eq!(AudioKind::for_priority(Priority::HasResult), AudioKind::Earcon);
    }

    // 033 U2 — read_audio_prefs: defaults sin settings; clamp de rangos; rechazo de voz con charset
    // peligroso (anti-inyección en argv). Sonido inexistente → default.
    #[test]
    fn audio_prefs_clamp_and_defaults() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT, updated_at TEXT);",
        )
        .unwrap();
        // sin settings → defaults exactos (cero regresión)
        let d = read_audio_prefs(&conn);
        assert_eq!(d.voice, None);
        assert_eq!(d.rate, 1.0);
        assert_eq!(d.earcon_volume, 1.0);
        assert_eq!(d.earcon_sound, EARCON_SOUND);
        // valores fuera de rango / peligrosos
        crate::settings::set(&conn, "attention.audio.rate", &serde_json::json!(9.9)).unwrap();
        crate::settings::set(&conn, "attention.audio.voice", &serde_json::json!("a; rm -rf /")).unwrap();
        crate::settings::set(&conn, "attention.audio.earcon_volume", &serde_json::json!(5.0)).unwrap();
        crate::settings::set(&conn, "attention.audio.earcon_sound", &serde_json::json!("/no/existe.aiff")).unwrap();
        let p = read_audio_prefs(&conn);
        assert_eq!(p.rate, 2.0, "rate clampeado a 2.0");
        assert_eq!(p.voice, None, "voz con charset peligroso → rechazada (default)");
        assert_eq!(p.earcon_volume, 1.0, "volumen clampeado a 1.0");
        assert_eq!(p.earcon_sound, EARCON_SOUND, "sonido inexistente → default");
        // voz válida pasa, capitalizada NO (es la voz tal cual, charset ok)
        crate::settings::set(&conn, "attention.audio.voice", &serde_json::json!("Monica")).unwrap();
        crate::settings::set(&conn, "attention.audio.rate", &serde_json::json!(1.5)).unwrap();
        let p2 = read_audio_prefs(&conn);
        assert_eq!(p2.voice.as_deref(), Some("Monica"));
        assert_eq!(p2.rate, 1.5);
    }

    // 032 U2 — whitelist de personalización del TTS: acepta nombres limpios (capitalizados), rechaza
    // todo lo demás (vacío, largo, charset raro, control) → cae a frase genérica.
    #[test]
    fn agent_label_whitelist() {
        assert_eq!(agent_label_for_tts("codex").as_deref(), Some("Codex"));
        assert_eq!(agent_label_for_tts("  claude ").as_deref(), Some("Claude"));
        assert_eq!(agent_label_for_tts("gpt-4.1").as_deref(), Some("Gpt-4.1"));
        // rechazos → None (⇒ genérica)
        assert_eq!(agent_label_for_tts(""), None);
        assert_eq!(agent_label_for_tts("   "), None);
        assert_eq!(agent_label_for_tts("rm -rf /; echo hi"), None); // ';' '/' fuera del charset
        assert_eq!(agent_label_for_tts("a\nb"), None); // control char
        assert_eq!(agent_label_for_tts(&"x".repeat(33)), None); // largo
        // un secreto-shape NO pasa (tiene caracteres fuera del charset corto)
        assert_eq!(agent_label_for_tts("sk-ABCdef123!@#"), None);
    }

    // AUDIT (codex blocker "cola a 17"): cola llena de urgentes + informativo entrante → RECHAZADO; la
    // cola NO crece, ningún urgente se sacrifica.
    #[test]
    fn full_of_urgent_rejects_incoming_info() {
        let opted: &'static [&'static str] = &[
            "u0","u1","u2","u3","u4","u5","u6","u7","u8","u9",
            "u10","u11","u12","u13","u14","u15","info",
        ];
        let (m, _s, c) = mgr(opted);
        // Encolar 16 urgentes (cada uno de un pane distinto; el urgente ignora rate global/presupuesto).
        for k in 0..QUEUE_CAP {
            assert_eq!(
                m.consider(req(&format!("u{k}"), Priority::NeedsInput, &format!("u{k}:n"))),
                Decision::Emit
            );
            c.advance(1); // distinto instante; rate por-pane es por-pane así que no aplica
        }
        assert_eq!(m.pending(), QUEUE_CAP);
        // Un informativo entrante: no hay víctima de prioridad menor → QueueFull, cola intacta.
        assert_eq!(m.consider(req("info", Priority::HasResult, "info:r")), Decision::QueueFull);
        assert_eq!(m.pending(), QUEUE_CAP);
    }

    // AUDIT (deepseek blocker "fuga de dedup"): al desalojar un informativo por un urgente, su dedup se
    // limpia → ese evento (que nunca sonó) puede volver a admitirse.
    #[test]
    fn evicting_info_cleans_its_dedup() {
        // Cola llena de 16 informativos.
        let opted: &'static [&'static str] = &[
            "i0","i1","i2","i3","i4","i5","i6","i7","i8","i9",
            "i10","i11","i12","i13","i14","i15","u",
        ];
        let (m, _s, c) = mgr(opted);
        // Para llenar de informativos hay que esquivar el presupuesto (6/min). Manipulamos la cola
        // directamente (test de unidad de try_place), reservando dedup como lo haría consider.
        {
            let mut st = m.lock();
            for k in 0..QUEUE_CAP {
                let r = req(&format!("i{k}"), Priority::HasResult, &format!("i{k}:r"));
                st.dedup.insert(r.event_id.clone(), 0);
                let o = st.order;
                st.order += 1;
                st.queue.push((o, r));
            }
        }
        assert_eq!(m.pending(), QUEUE_CAP);
        c.advance(10);
        // Un urgente entra desplazando el informativo más viejo (i0) y limpiando su dedup.
        assert_eq!(m.consider(req("u", Priority::NeedsInput, "u:n:1")), Decision::Emit);
        assert_eq!(m.pending(), QUEUE_CAP); // sigue acotada
        // El dedup de i0 (el desalojado) quedó libre: re-considerarlo no es Duplicate.
        // (i0 ya no está opted? sí lo está.) Avanzamos > rate global para que pase como informativo.
        c.advance(600);
        let d = m.consider(req("i0", Priority::HasResult, "i0:r"));
        assert_ne!(d, Decision::Duplicate, "el dedup del desalojado debe estar limpio");
    }

    // AUDIT (codex BLOCKER reproducción #2): un `callar` entre "tomar" y "reproducir" CANCELA el audio.
    #[test]
    fn silence_between_take_and_play_drops_it() {
        let (m, sink, _c) = mgr(&["a"]);
        assert_eq!(m.consider(req("a", Priority::NeedsInput, "a:n:1")), Decision::Emit);
        // Simulamos al worker: toma el pedido (y la generación) pero todavía no reproduce.
        let (taken, gen) = m.take_for_play().expect("hay un pedido");
        // Llega `callar` justo en el medio.
        m.silence();
        // El play ya NO debe sonar (generación avanzada).
        assert!(!m.play_if_current(&taken, gen));
        assert_eq!(sink.played.lock().unwrap().len(), 0);
        assert_eq!(sink.cancels.load(Ordering::SeqCst), 1);
    }

    // AUDIT (codex BLOCKER reproducción #1): dos pumps concurrentes NUNCA solapan `sink.play`.
    #[test]
    fn concurrent_pumps_never_overlap_play() {
        // Sink que paniquea si detecta dos `play` solapados.
        #[derive(Clone, Default)]
        struct GuardSink {
            in_play: Arc<std::sync::atomic::AtomicBool>,
            count: Arc<AtomicU64>,
        }
        impl AudioSink for GuardSink {
            fn play(&self, _req: &AudioRequest) {
                assert!(
                    !self.in_play.swap(true, Ordering::SeqCst),
                    "dos play() solapados — reproducción NO serial"
                );
                // pequeña ventana para forzar contención
                for _ in 0..1000 {
                    std::hint::spin_loop();
                }
                self.count.fetch_add(1, Ordering::SeqCst);
                self.in_play.store(false, Ordering::SeqCst);
            }
            fn cancel(&self) {}
        }
        let sink = GuardSink::default();
        let clock = FakeClock::default();
        let m = Arc::new(AudioManager::new(
            Box::new(sink.clone()),
            Box::new(|_: &str| true),
            Box::new(clock.clone()),
        ));
        // Encolar muchos urgentes de panes distintos (urgente ignora rate global/presupuesto).
        for k in 0..200 {
            // cola acotada a 16: encolamos en tandas drenando, simulando producción continua.
            let _ = m.consider(req(&format!("p{k}"), Priority::NeedsInput, &format!("p{k}:n")));
            if k % 8 == 0 {
                clock.advance(10);
            }
        }
        // 8 hilos drenando en paralelo.
        let mut handles = vec![];
        for _ in 0..8 {
            let m = m.clone();
            handles.push(std::thread::spawn(move || m.pump_all()));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(!sink.in_play.load(Ordering::SeqCst));
    }

    // AUDIT (codex BLOCKER reproducción #2, fix estructural): `sink.cancel` (callar) NUNCA se solapa
    // con un `sink.play` — el `playback` lock los excluye. Cierra la ventana "callar entre check y play".
    #[test]
    fn cancel_never_overlaps_play() {
        #[derive(Clone, Default)]
        struct XSink {
            in_play: Arc<std::sync::atomic::AtomicBool>,
            plays: Arc<AtomicU64>,
            cancels: Arc<AtomicU64>,
        }
        impl AudioSink for XSink {
            fn play(&self, _req: &AudioRequest) {
                assert!(!self.in_play.swap(true, Ordering::SeqCst), "play solapado");
                for _ in 0..500 {
                    std::hint::spin_loop();
                }
                self.plays.fetch_add(1, Ordering::SeqCst);
                self.in_play.store(false, Ordering::SeqCst);
            }
            fn cancel(&self) {
                assert!(!self.in_play.load(Ordering::SeqCst), "cancel solapó un play en curso");
                self.cancels.fetch_add(1, Ordering::SeqCst);
            }
        }
        let sink = XSink::default();
        let clock = FakeClock::default();
        let m = Arc::new(AudioManager::new(
            Box::new(sink.clone()),
            Box::new(|_: &str| true),
            Box::new(clock.clone()),
        ));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = vec![];
        // Productores + pumps.
        for t in 0..4 {
            let m = m.clone();
            let stop = stop.clone();
            let clock = clock.clone();
            handles.push(std::thread::spawn(move || {
                let mut k = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    let _ = m.consider(req(&format!("p{t}_{k}"), Priority::NeedsInput, &format!("p{t}:{k}")));
                    m.pump_one();
                    k += 1;
                    if k.is_multiple_of(4) {
                        clock.advance(1);
                    }
                }
            }));
        }
        // Callares concurrentes.
        for _ in 0..4 {
            let m = m.clone();
            let stop = stop.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..2000 {
                    m.silence();
                }
                stop.store(true, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Sin panic ⇒ play y cancel nunca se solaparon.
        assert!(!sink.in_play.load(Ordering::SeqCst));
    }

    // try_place a nivel unidad: cola llena de informativos, entra urgente → desaloja 1 informativo.
    #[test]
    fn try_place_urgent_evicts_oldest_info() {
        let (m, _s, _c) = mgr(&["x", "u"]);
        let mut st = m.lock();
        for i in 0..QUEUE_CAP {
            let r = req("x", Priority::HasResult, &format!("x:r:{i}"));
            st.queue.push((i as u64, r));
            st.order = (i as u64) + 1;
        }
        let urgent = req("u", Priority::NeedsInput, "u:n:1");
        assert!(AudioManager::try_place(&mut st, &urgent));
        assert_eq!(st.queue.len(), QUEUE_CAP); // desalojó uno
        assert_eq!(st.queue.iter().filter(|(_, r)| r.priority == Priority::NeedsInput).count(), 1);
    }
}
