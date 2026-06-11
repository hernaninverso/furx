// PTY manager — wraps portable-pty with Tauri events.
// Each pane gets its own PtySession with its own writer + reader thread.

use crate::bases::state::PaneStateModel;
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;
use tauri::{AppHandle, Emitter};

// 004 mobile-companion (MC-3): per-pane scrollback ring buffer fed from the
// reader loop. The phone reads `snapshot(pane_id)` → last N ANSI-stripped lines.
// Bounded: ≤50 lines, partial line ≤8KB, so memory can't grow without bound on
// a newline-less flood (NFR-4).
const SCROLLBACK_MAX_LINES: usize = 50;
const SCROLLBACK_MAX_PARTIAL: usize = 8192;

// 023 F1 — SessionBuffer para la auto-captura de memoria. SEPARADO del scrollback de UI
// (que sigue en 50 líneas, sin inflar el render): este buffer guarda hasta 500 líneas de texto
// CRUDO (solo ANSI-stripped, SIN scrub por línea). Volátil (RAM), por pane, se purga al cierre.
// El scrub corre como ÚNICO gate autoritativo en `memory_autocapture::scrub_buffer` JUSTO antes de
// CUALQUIER egreso (AIE / propuestas / incomplete_sessions). El crudo NUNCA egresa sin scrub: ver
// el modelo de amenaza en el doc-comment de `SessionBuffer` (fix audit codex HIGH).
const SESSION_BUFFER_MAX_LINES: usize = 500;

// Strip ANSI/VT control sequences so the mobile viewer shows plain text:
// CSI (`ESC [ ... cmd`), OSC (`ESC ] ... BEL|ST`), 2-char escapes, and lone
// control chars EXCEPT tab/newline/carriage-return (handled by the line splitter).
static ANSI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"\x1b\[[0-9;?]*[ -/]*[@-~]",          // CSI
        r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", // OSC ... BEL or ST
        r"|\x1b[@-Z\\-_]",                     // 2-char escapes (e.g. ESC c)
        r"|[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]"   // control chars, keep \t \n \r
    ))
    .expect("ansi regex")
});

fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    ANSI_RE.replace_all(s, "")
}

/// Per-pane scrollback: completed lines (capped) + the in-progress partial line.
#[derive(Default)]
struct Scrollback {
    lines: VecDeque<String>,
    partial: String,
}

impl Scrollback {
    /// Feed an already-ANSI-stripped chunk. Handles `\r\n` and lone `\r`
    /// (carriage return = overwrite current line, as terminals do for progress bars).
    fn push(&mut self, chunk: &str) {
        let normalized = chunk.replace("\r\n", "\n");
        for ch in normalized.chars() {
            match ch {
                '\n' => {
                    let line = std::mem::take(&mut self.partial);
                    self.lines.push_back(line);
                    while self.lines.len() > SCROLLBACK_MAX_LINES {
                        self.lines.pop_front();
                    }
                }
                '\r' => self.partial.clear(),
                c => {
                    if self.partial.len() < SCROLLBACK_MAX_PARTIAL {
                        self.partial.push(c);
                    }
                }
            }
        }
    }

    fn snapshot(&self) -> Vec<String> {
        let mut out: Vec<String> = self.lines.iter().cloned().collect();
        if !self.partial.is_empty() {
            out.push(self.partial.clone());
        }
        out
    }
}

/// 023 F1 — contexto de captura de memoria de un pane (procedencia fina). Lo setea `pty_spawn`
/// vía `register_session_ctx` cuando el pane es un CLI de agente; sin él, no se auto-captura.
#[derive(Debug, Clone, Default)]
pub struct SessionCaptureCtx {
    pub cli_kind: String,
    pub project_key: String,
    pub session_id: String,
}

/// 023 F1 — buffer de captura de memoria por pane. Guarda hasta 500 líneas de texto **CRUDO**
/// (solo ANSI-stripped, SIN scrub por línea). `had_output` marca si el agente emitió algo (para el
/// filtro de trivialidad). Separado del scrollback de UI.
///
/// ## Modelo de amenaza (fix audit codex HIGH — privacidad de arquitectura)
/// El texto crudo NO se scrubea por línea en el reader; el scrub corre como ÚNICO gate autoritativo
/// en `memory_autocapture::scrub_buffer`, llamado JUSTO antes de CUALQUIER egreso (la destilación
/// AIE en `run_capture`, las propuestas en `memory_proposals`/`memory_entries`, y el resguardo en
/// `incomplete_sessions` vía `save_incomplete_session`). Razón: el scrub por línea **defeatea** la
/// detección de secretos PARTIDOS entre líneas — para `sk-proj-ABCDEFGHIJKLMNOP\nQRSTUVWXYZ0123`,
/// si el head (≥16 chars) ya matchea solo, la capa por-línea lo redacta a `[REDACTED:sk]`; al
/// construir la vista de-newlined desde texto ya saneado el prefijo del secreto desaparece y el
/// TAIL de la línea siguiente SOBREVIVE → leak a memoria persistente / LLM. `scrub_buffer` SÍ caza
/// el secreto partido, pero SOLO si recibe el crudo (necesita el head intacto).
///
/// El trade-off es deliberado y correcto: una ventana MINÚSCULA de crudo en RAM vs. el leak real de
/// secretos partidos a almacenamiento persistente / al AIE. Garantías del crudo:
///   - Vive SOLO en RAM (este `VecDeque`), NUNCA se persiste ni se envía sin scrub.
///   - Acotado a 500 líneas (FIFO) — no crece sin límite ni en un flood sin newline.
///   - Se purga al cerrar el pane (`session_capture.lock().remove`) y al respawn.
///   - TODO consumidor (`take_session_buffer`, `dispatch_session_capture`) pasa por `scrub_buffer`
///     antes de egresar. NINGÚN path consume las líneas crudas directamente.
#[derive(Default)]
struct SessionBuffer {
    ctx: SessionCaptureCtx,
    lines: VecDeque<String>,
    partial: String,
    had_output: bool,
}

impl SessionBuffer {
    /// Empuja un chunk CRUDO (solo ANSI-stripped por el reader; SIN scrub por línea). El scrub es
    /// responsabilidad EXCLUSIVA de `scrub_buffer` al egreso (ver modelo de amenaza del struct).
    /// Splittea por línea igual que Scrollback; cap a 500 líneas (FIFO).
    fn push_raw(&mut self, chunk: &str) {
        self.had_output = true;
        let normalized = chunk.replace("\r\n", "\n");
        for ch in normalized.chars() {
            match ch {
                '\n' => {
                    let line = std::mem::take(&mut self.partial);
                    self.lines.push_back(line);
                    while self.lines.len() > SESSION_BUFFER_MAX_LINES {
                        self.lines.pop_front();
                    }
                }
                '\r' => self.partial.clear(),
                c => {
                    if self.partial.len() < SCROLLBACK_MAX_PARTIAL {
                        self.partial.push(c);
                    }
                }
            }
        }
    }

    fn lines_snapshot(&self) -> Vec<String> {
        let mut out: Vec<String> = self.lines.iter().cloned().collect();
        if !self.partial.is_empty() {
            out.push(self.partial.clone());
        }
        out
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SpawnRequest {
    pub pane_id: String,
    pub cmd: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyOutput {
    pub pane_id: String,
    pub data: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct PtyExit {
    pub pane_id: String,
    pub code: Option<i32>,
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// 015 T014 (audit): generación (run_token) de ESTE spawn — la misma que lleva la fila del
    /// registry. El wait-thread la compara con la de la sesión vigente bajo el pane_id; si difiere
    /// (un respawn la reemplazó), sale sin tocar nada. `kill_if_spawn_id` también la usa para que
    /// la limpieza interna (spawn-fail / cancel-durante-spawn) mate SÓLO su propia generación.
    run_token: i64,
}

pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    pane_state: PaneStateModel,
    // 004 mobile-companion: scrollback ring buffer per pane (mobile snapshots).
    scrollback: Arc<Mutex<HashMap<String, Scrollback>>>,
    // 023 F1 — SessionBuffer de captura de memoria por pane (500 líneas, ya scrubeadas).
    // Sólo se llena para panes con contexto de captura registrado (CLIs de agente).
    session_capture: Arc<Mutex<HashMap<String, SessionBuffer>>>,
}

impl PtyManager {
    pub fn new(pane_state: PaneStateModel) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pane_state,
            scrollback: Arc::new(Mutex::new(HashMap::new())),
            session_capture: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 023 F1 — registra el contexto de captura de memoria de un pane (lo llama `pty_spawn` SÓLO
    /// si el pane es un CLI de agente y `memory.autocapture` está on). Sin contexto registrado, el
    /// reader NO acumula buffer de captura para ese pane (cero overhead con la feature apagada).
    pub fn register_session_ctx(&self, pane_id: &str, ctx: SessionCaptureCtx) {
        let mut map = self.session_capture.lock();
        let buf = map.entry(pane_id.to_string()).or_default();
        buf.ctx = ctx;
    }

    /// 023 F1 — toma (y REMUEVE) el SessionBuffer de un pane: el contexto, las líneas saneadas y
    /// si hubo salida del agente. `None` si no había buffer de captura (pane sin contexto). Lo
    /// llama el trigger de fin-de-sesión ANTES de purgar el pane.
    pub fn take_session_buffer(&self, pane_id: &str) -> Option<(SessionCaptureCtx, Vec<String>, bool)> {
        let mut map = self.session_capture.lock();
        map.remove(pane_id)
            .map(|b| (b.ctx.clone(), b.lines_snapshot(), b.had_output))
    }

    /// 004 mobile-companion (MC-3): last ≤50 ANSI-stripped lines of a pane, for
    /// the mobile companion `PaneSnapshot`. Empty Vec if the pane is unknown.
    pub fn snapshot(&self, pane_id: &str) -> Vec<String> {
        self.scrollback
            .lock()
            .get(pane_id)
            .map(|s| s.snapshot())
            .unwrap_or_default()
    }

    /// Live pane ids (panes Furx owns). The mobile bridge can only reach these.
    pub fn pane_ids(&self) -> Vec<String> {
        self.sessions.lock().keys().cloned().collect()
    }

    pub fn spawn(&self, req: SpawnRequest, app: AppHandle, run_token: i64) -> Result<()> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: req.rows.max(10),
            cols: req.cols.max(40),
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(&req.cmd);
        cmd.args(&req.args);
        if let Some(cwd) = &req.cwd {
            cmd.cwd(cwd);
        } else if let Some(home) = dirs::home_dir() {
            cmd.cwd(home);
        }
        // Sane TERM + propagate selected env.
        cmd.env("TERM", "xterm-256color");
        cmd.env(
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
        );
        // PATH: preserve the user's PATH so commands like `claude`, `codex`, `gemini` are found.
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave); // slave fd no longer needed after spawn

        let writer = pair.master.take_writer()?;
        let reader_master = pair.master.try_clone_reader()?;
        let sessions = Arc::clone(&self.sessions);

        // 015 T014 (audit defense-in-depth): si ya existe una sesión con este pane_id (re-spawn
        // del mismo id), la matamos+removemos ANTES de insertar la nueva. portable_pty NO mata
        // el child al dropear el handle, así que un overwrite a ciegas leakearía el OS process
        // viejo (audit HIGH, 4 voces). El caller `pty_spawn` ya reapea vía el registry; esto
        // protege a CUALQUIER otro caller de spawn. La wait-thread vieja verá la sesión ausente
        // y saldrá sin tocar el registry (la fila ya fue reasignada al run nuevo).
        if let Some(mut old) = sessions.lock().remove(&req.pane_id) {
            let _ = old.child.kill();
        }
        self.scrollback.lock().remove(&req.pane_id);
        // 023 — el SessionBuffer de captura del run anterior se descarta en el respawn (el ctx
        // del run nuevo lo re-registra `pty_spawn` después de este spawn, si corresponde).
        self.session_capture.lock().remove(&req.pane_id);

        sessions.lock().insert(
            req.pane_id.clone(),
            PtySession {
                master: pair.master,
                writer,
                child,
                run_token,
            },
        );

        // Reader thread — pumps PTY output to the frontend AND signals FSM on each chunk.
        let pane_id = req.pane_id.clone();
        let app_clone = app.clone();
        let fsm = self.pane_state.clone();
        let scrollback_reader = Arc::clone(&self.scrollback);
        let session_capture_reader = Arc::clone(&self.session_capture);
        thread::Builder::new()
            .name(format!("pty-reader-{}", pane_id))
            .spawn(move || {
                pty_reader_loop(
                    pane_id,
                    reader_master,
                    app_clone,
                    fsm,
                    scrollback_reader,
                    session_capture_reader,
                )
            })?;

        // Wait thread — emits exit event when child dies + marca FSM.forget.
        let pane_id_exit = req.pane_id.clone();
        let sessions_exit = Arc::clone(&sessions);
        let app_exit = app.clone();
        let fsm_exit = self.pane_state.clone();
        let scrollback_exit = Arc::clone(&self.scrollback);
        let session_capture_exit = Arc::clone(&self.session_capture);
        thread::Builder::new()
            .name(format!("pty-wait-{}", pane_id_exit))
            .spawn(move || {
                loop {
                    let mut guard = sessions_exit.lock();
                    let Some(sess) = guard.get_mut(&pane_id_exit) else {
                        break;
                    };
                    // 015 T014 (audit): si la sesión vigente bajo este pane_id ya NO es la mía
                    // (un respawn la reemplazó con un run_token nuevo), salgo sin tocar nada —
                    // el run nuevo lo administra SU propio wait-thread. El child viejo ya fue
                    // matado por el respawn. Verifico ANTES del try_wait, con el lock TOMADO.
                    if sess.run_token != run_token {
                        break;
                    }
                    match sess.child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.exit_code() as i32;
                            // Removemos NUESTRA sesión mientras tenemos el lock y ya verificamos
                            // run_token: garantiza que sólo sacamos del mapa la sesión propia (no
                            // la de un respawn). Recién después soltamos el lock.
                            guard.remove(&pane_id_exit);
                            drop(guard);
                            if code != 0 {
                                fsm_exit.on_error(&pane_id_exit);
                            }
                            let _ = app_exit.emit(
                                "pty:exit",
                                PtyExit {
                                    pane_id: pane_id_exit.clone(),
                                    code: Some(code),
                                },
                            );
                            // 015 T014 (US5): el PTY salió por sí mismo → marca terminal en el
                            // registry, SCOPEADO por run_token: si en la ventana desde drop(guard)
                            // un respawn reseteó la fila a un run nuevo (token N+1), este finish
                            // (token N) es no-op → no hijackea la generación nueva (audit HIGH).
                            finish_in_registry(
                                &app_exit,
                                &pane_id_exit,
                                run_token,
                                if code == 0 { "done" } else { "failed" },
                            );
                            // 023 F1 — fin de sesión: dispara la auto-captura ANTES de purgar el
                            // SessionBuffer. Gateada por settings + cli_kind dentro del dispatch.
                            dispatch_session_capture(&app_exit, &session_capture_exit, &pane_id_exit);
                            scrollback_exit.lock().remove(&pane_id_exit);
                            fsm_exit.forget(&pane_id_exit);
                            break;
                        }
                        Ok(None) => {
                            drop(guard);
                            thread::sleep(std::time::Duration::from_millis(200));
                        }
                        Err(_) => {
                            guard.remove(&pane_id_exit);
                            drop(guard);
                            fsm_exit.on_error(&pane_id_exit);
                            let _ = app_exit.emit(
                                "pty:exit",
                                PtyExit {
                                    pane_id: pane_id_exit.clone(),
                                    code: None,
                                },
                            );
                            // 015 T014 (US5): error esperando al child → terminal `failed` (scopeado).
                            finish_in_registry(&app_exit, &pane_id_exit, run_token, "failed");
                            // 023 F1 — fin de sesión (error path): igual dispara la auto-captura.
                            dispatch_session_capture(&app_exit, &session_capture_exit, &pane_id_exit);
                            scrollback_exit.lock().remove(&pane_id_exit);
                            fsm_exit.forget(&pane_id_exit);
                            break;
                        }
                    }
                }
            })?;

        Ok(())
    }

    pub fn write(&self, pane_id: &str, data: &[u8]) -> Result<()> {
        let mut guard = self.sessions.lock();
        let sess = guard
            .get_mut(pane_id)
            .ok_or_else(|| anyhow!("pane {} not found", pane_id))?;
        sess.writer.write_all(data)?;
        sess.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, pane_id: &str, rows: u16, cols: u16) -> Result<()> {
        let guard = self.sessions.lock();
        let sess = guard
            .get(pane_id)
            .ok_or_else(|| anyhow!("pane {} not found", pane_id))?;
        sess.master.resize(PtySize {
            rows: rows.max(10),
            cols: cols.max(40),
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    pub fn kill(&self, pane_id: &str) -> Result<()> {
        let mut guard = self.sessions.lock();
        if let Some(mut sess) = guard.remove(pane_id) {
            Self::terminate_child(&mut sess);
        }
        self.scrollback.lock().remove(pane_id);
        // 023 — purga el SessionBuffer de captura del pane (kill explícito no auto-captura en F0).
        self.session_capture.lock().remove(pane_id);
        Ok(())
    }

    /// 019 F3 (audit HIGH-2): mata un child que PODRÍA estar PAUSADO (SIGSTOP). Un proceso
    /// congelado NO procesa SIGHUP/SIGTERM (quedan PENDIENTES hasta un SIGCONT), y portable-pty
    /// 0.8 mata con SIGHUP (señal catchable/blockable). Si el kill-switch (orchestration_cancel →
    /// kill_attempt) intentara matar un attempt pausado a secas, el proceso quedaría inmortal/zombie
    /// congelado. Acá garantizamos que SIEMPRE muera: (1) SIGCONT al grupo para des-pendear cualquier
    /// señal retenida y reanudarlo, (2) el kill "amable" del backend (SIGHUP) por si tiene cleanup,
    /// (3) SIGKILL al grupo — que SIGSTOP NO puede bloquear — como garantía dura. Idempotente y
    /// best-effort: si el proceso ya salió, todos los pasos son no-ops (ESRCH).
    fn terminate_child(sess: &mut PtySession) {
        // (1) reanudar: un proceso bajo SIGSTOP no actúa sobre SIGHUP/SIGTERM hasta recibir SIGCONT.
        #[cfg(unix)]
        if let Some(pid) = sess.child.process_id() {
            let _ = send_unix_signal(pid as i32, PtySignal::Cont);
        }
        // (2) kill "amable" del backend (SIGHUP en portable-pty unix) — permite cleanup del proceso.
        let _ = sess.child.kill();
        // (3) garantía dura: SIGKILL al grupo (no bloqueable por SIGSTOP). Cubre el caso en que el
        //     proceso ignorara/atrapara SIGHUP o que el (1) no alcanzara a algún hijo.
        #[cfg(unix)]
        if let Some(pid) = sess.child.process_id() {
            send_unix_kill(pid as i32);
        }
    }

    /// 015 T014 (audit final) — mata el PTY del pane SÓLO si la sesión vigente es de la generación
    /// `run_token`. Lo usa la limpieza INTERNA de `pty_spawn` (fallo de spawn / cancel-durante-spawn)
    /// para no matar un run NUEVO que reusó el mismo pane_id entre medio (codex MED: la limpieza por
    /// pane_id a secas podía clobberear la generación siguiente). Para la cancelación de USUARIO se
    /// usa `kill` directo (apunta a la sesión vigente, que es justo lo que el usuario quiere cortar).
    pub fn kill_if_spawn_id(&self, pane_id: &str, run_token: i64) -> Result<()> {
        let mut guard = self.sessions.lock();
        match guard.get(pane_id) {
            Some(sess) if sess.run_token == run_token => {
                if let Some(mut sess) = guard.remove(pane_id) {
                    Self::terminate_child(&mut sess);
                }
                drop(guard);
                self.scrollback.lock().remove(pane_id);
            }
            _ => {} // la sesión vigente es de otra generación (o no hay) → no tocar.
        }
        Ok(())
    }

    pub fn alive(&self, pane_id: &str) -> bool {
        self.sessions.lock().contains_key(pane_id)
    }

    /// 019 F3 (T030) — PID del proceso raíz del PTY de un pane (su shell/agente). `None` si el
    /// pane no existe o el backend no expone el pid. Sólo lectura; el caller usa esto para
    /// SIGSTOP/SIGCONT (pause/resume), NUNCA para matar.
    pub fn process_id(&self, pane_id: &str) -> Option<u32> {
        self.sessions
            .lock()
            .get(pane_id)
            .and_then(|sess| sess.child.process_id())
    }

    /// 019 F3 (T030) — PAUSA el proceso del PTY de `pane_id` con SIGSTOP, sin matarlo. Señaliza al
    /// GRUPO de procesos (`-pid`) cuando se puede, así los hijos del agente (sub-shells, builds)
    /// también se detienen; si el pid no lidera un grupo propio, cae a señalizar el pid solo. NO es
    /// destructivo: el proceso queda congelado y se reanuda intacto con `resume`. En plataformas
    /// no-Unix es un error tipado (no hay SIGSTOP).
    ///
    /// 019 F3 (audit HIGH-1): devuelve `Ok(true)` SÓLO si había un proceso vivo y se lo detuvo;
    /// `Ok(false)` si el pane ya no tiene sesión / el proceso ya salió (no hay nada que pausar).
    /// El caller usa esto para SIGSTOP-FIRST: persiste el flag `paused_at` SÓLO ante `Ok(true)`,
    /// así nunca queda un attempt marcado "pausado" que en realidad ya terminó (o un flag sin
    /// proceso congelado detrás). `Err` = el SIGSTOP falló de verdad (no persistir el flag).
    pub fn pause(&self, pane_id: &str) -> Result<bool> {
        self.signal(pane_id, PtySignal::Stop)
    }

    /// 019 F3 (T030) — REANUDA el proceso pausado de `pane_id` con SIGCONT. Idempotente:
    /// reanudar un proceso no-pausado es inocuo. Devuelve `Ok(true)` si había un proceso vivo al
    /// que se le mandó SIGCONT, `Ok(false)` si ya no hay proceso (salió mientras estaba pausado —
    /// el caller igual limpia el flag: "reanudado" se cumple porque ya no hay nada congelado).
    pub fn resume(&self, pane_id: &str) -> Result<bool> {
        self.signal(pane_id, PtySignal::Cont)
    }

    /// Manda `sig` al proceso (grupo) del pane. `Ok(true)` = había proceso vivo y el `kill(2)`
    /// REALMENTE entregó la señal; `Ok(false)` = no hay a quién señalizar (pane sin sesión viva, o
    /// el proceso murió en la ventana entre `process_id()` y el `kill` → ESRCH). `Err` = el `kill`
    /// falló por otra causa (EPERM, etc.).
    ///
    /// 019 F3 (audit HIGH ronda 2): el `true`/`false` se basa en si el `kill` entregó la señal, NO
    /// en si `process_id()` encontró un PID antes. Race: si el proceso muere entre el `process_id()`
    /// y el `kill(SIGSTOP)`, `send_unix_signal` devuelve `Ok(false)` (ESRCH) → `pause` NO persiste el
    /// flag de un attempt que en realidad ya terminó. (Devolver `true` por haber visto el PID antes
    /// dejaba un `paused_at` sin proceso congelado detrás — la inconsistencia que este fix elimina.)
    fn signal(&self, pane_id: &str, sig: PtySignal) -> Result<bool> {
        let pid = match self.process_id(pane_id) {
            Some(p) => p as i32,
            // pane sin sesión viva (ya salió / detached sin pid) → no hay a quién señalizar.
            None => return Ok(false),
        };
        // El bool surge del resultado REAL del kill: entregada (true) vs proceso-ya-muerto (false).
        send_unix_signal(pid, sig)
    }
}

/// 019 F3 (T030) — señal de control de proceso para pause/resume (sin matar).
#[derive(Debug, Clone, Copy)]
enum PtySignal {
    Stop,
    Cont,
}

/// Manda `sig` al grupo/proceso de `pid`. Devuelve `Ok(true)` si el `kill(2)` REALMENTE entregó la
/// señal (rc 0), `Ok(false)` si el proceso ya no existe (ESRCH → no se aplicó nada), `Err` ante otro
/// errno (EPERM, etc.). 019 F3 (audit HIGH ronda 2): NO tragar ESRCH como "éxito" indistinto — el
/// caller `signal`/`pause` necesita distinguir "señal entregada" de "el proceso murió en la ventana"
/// para no persistir un flag `paused_at` sin proceso congelado detrás. (El kill-path `terminate_child`
/// descarta este bool con `let _ =`: para terminar, "proceso ya no existe" = objetivo cumplido.)
#[cfg(unix)]
fn send_unix_signal(pid: i32, sig: PtySignal) -> Result<bool> {
    let signum = match sig {
        PtySignal::Stop => libc::SIGSTOP,
        PtySignal::Cont => libc::SIGCONT,
    };
    // Señalizar al GRUPO de procesos (-pid) para alcanzar a los hijos del agente. portable_pty
    // hace setsid() en el child, así que el child ES su líder de grupo y pgid == pid. Si por algún
    // motivo el grupo no existe (ESRCH), caemos a señalizar sólo el pid.
    if unsafe { libc::kill(-pid, signum) } == 0 {
        return Ok(true);
    }
    if unsafe { libc::kill(pid, signum) } == 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        // ESRCH = el proceso (y su grupo) ya no existen → la señal NO se aplicó. Lo reportamos como
        // `Ok(false)` (no-op, NO error): el caller de pause sabrá que no hay nada congelado y NO
        // persistirá el flag. Cualquier otro errno (EPERM, …) sí es un fallo real → Err.
        if err.raw_os_error() == Some(libc::ESRCH) {
            Ok(false)
        } else {
            Err(anyhow!("kill({pid}, {signum}) falló: {err}"))
        }
    }
}

/// 019 F3 (audit HIGH-2): SIGKILL DURO al grupo de procesos (best-effort, no falla nunca).
/// SIGKILL no puede ser ignorado ni bloqueado por un proceso bajo SIGSTOP, garantizando que un
/// attempt PAUSADO muera al matarlo. Apunta primero al grupo (`-pid`) y cae al pid solo; ESRCH
/// (ya salió) es éxito silencioso.
#[cfg(unix)]
fn send_unix_kill(pid: i32) {
    if unsafe { libc::kill(-pid, libc::SIGKILL) } == 0 {
        return;
    }
    let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn send_unix_signal(_pid: i32, _sig: PtySignal) -> Result<bool> {
    Err(anyhow!(
        "pause/resume de procesos no soportado en esta plataforma"
    ))
}

/// 015 T014 (US5) — marca un proceso PTY como terminal en el registro de procesos cuando
/// el child sale por sí mismo. Decisión A1 del council: el wait thread ya tiene el
/// `AppHandle`, así que resuelve `AppState` vía `try_state` y llama `process_manager::finish`
/// directo (sin round-trip por evento ni listener global). `finish` es idempotente y NO pisa
/// un `canceled` previo (race PTY-exit-vs-cancel resuelto en el servicio).
///
/// En shutdown de la app `try_state` puede devolver `None` (la managed state ya se dropeó):
/// es un no-op deliberado — el PTY igual muere con el proceso, la invariante no se viola.
fn finish_in_registry(app: &AppHandle, pane_id: &str, run_token: i64, status: &str) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<crate::AppState>() {
        if let Err(e) =
            crate::services::process_manager::finish(&state.db, pane_id, status, Some(run_token))
        {
            // No abortamos el cierre del PTY por esto, pero lo dejamos en el log (una fila que
            // quedó `running` por un error de DB es un síntoma a investigar, no algo a tragar).
            tracing::warn!("process_manager::finish({pane_id}, {status}) falló: {e}");
        }
    }
}

/// 023 F1 — dispara la auto-captura de memoria al fin de sesión de un pane (lo llama el
/// wait-thread JUSTO antes de purgar el SessionBuffer). Sync + best-effort:
///   1. Toma (y remueve) el SessionBuffer del pane. Sin buffer (pane no-captura) → no-op.
///   2. Lee `memory.autocapture` (default OFF) + `cli_kind` del buffer. OFF o cli no-agente → no-op.
///   3. Lee los settings de captura (auto_accept, max_candidates) y delega la parte pesada
///      (scrub-bloque → AIE → propuestas) a `memory_autocapture::run_capture` en el async runtime.
/// NUNCA bloquea el cierre del pane (la red/AIE corre en una task aparte).
fn dispatch_session_capture(
    app: &AppHandle,
    session_capture: &Arc<Mutex<HashMap<String, SessionBuffer>>>,
    pane_id: &str,
) {
    use tauri::Manager;
    // (1) tomar el buffer (y removerlo). Sin buffer → este pane no era de captura.
    let (ctx, lines, had_output) = {
        let mut map = session_capture.lock();
        match map.remove(pane_id) {
            Some(b) => (b.ctx.clone(), b.lines_snapshot(), b.had_output),
            None => return,
        }
    };
    // Alcance: SOLO CLIs de agente conocidos (council v2 §4).
    if !crate::services::memory_autocapture::is_agent_cli(&ctx.cli_kind) {
        return;
    }
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    // (2)+(3) leer settings (default-OFF). Con autocapture=off → cero AIE, cero propuestas.
    let (enabled, auto_accept, max_candidates) = {
        let conn = state.db.lock();
        let read_bool = |k: &str| {
            crate::settings::get(&conn, k)
                .ok()
                .flatten()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        let enabled = read_bool("memory.autocapture");
        let auto_accept = read_bool("memory.autocapture_auto_accept");
        let max = crate::settings::get(&conn, "memory.autocapture_max_candidates")
            .ok()
            .flatten()
            .and_then(|v| v.as_f64())
            .map(|n| n as usize)
            .filter(|n| *n >= 1)
            .unwrap_or(5);
        (enabled, auto_accept, max)
    };
    if !enabled {
        return;
    }
    let db = state.db.clone();
    let capture_ctx = crate::services::memory_autocapture::SessionCtx {
        pane_id: pane_id.to_string(),
        cli_kind: ctx.cli_kind.clone(),
        project_key: ctx.project_key.clone(),
        session_id: ctx.session_id.clone(),
    };
    // 025 — clonar las líneas ANTES de moverlas a `run_capture` (las reusa la captura procedural).
    let lines_procedural = lines.clone();
    // Parte pesada (scrub-bloque + AIE + persistencia) fuera del wait-thread.
    tauri::async_runtime::spawn(async move {
        let _ = crate::services::memory_autocapture::run_capture(
            db,
            capture_ctx,
            lines,
            had_output,
            max_candidates,
            auto_accept,
        )
        .await;
    });

    // 025 F0 — además de la auto-captura de 023, disparar la detección procedural fallo->fix
    // (gated por `memory.procedural_learning`, default OFF). REUSA el MISMO SessionBuffer (ya
    // tomado arriba). No-op si el setting está off. Corre en su propia task (no bloquea el cierre).
    let procedural_enabled = {
        let conn = state.db.lock();
        crate::settings::get(&conn, "memory.procedural_learning")
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    if procedural_enabled {
        let db2 = state.db.clone();
        let pctx = crate::services::memory_autocapture::SessionCtx {
            pane_id: pane_id.to_string(),
            cli_kind: ctx.cli_kind.clone(),
            project_key: ctx.project_key.clone(),
            session_id: ctx.session_id.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let _ = crate::services::procedural_gotchas::run_procedural_capture(
                db2,
                pctx,
                lines_procedural,
                auto_accept,
            )
            .await;
        });
    }
}

fn pty_reader_loop(
    pane_id: String,
    mut reader: Box<dyn Read + Send>,
    app: AppHandle,
    fsm: PaneStateModel,
    scrollback: Arc<Mutex<HashMap<String, Scrollback>>>,
    session_capture: Arc<Mutex<HashMap<String, SessionBuffer>>>,
) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                fsm.on_output(&pane_id);
                let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
                // 004 mobile-companion: tee an ANSI-stripped copy into the
                // scrollback ring buffer for the phone. The frontend still gets
                // the raw chunk (full ANSI fidelity for xterm.js).
                {
                    let stripped = strip_ansi(&chunk);
                    let mut sb = scrollback.lock();
                    sb.entry(pane_id.clone()).or_default().push(&stripped);
                    drop(sb);
                    // 023 F1 (fix audit codex HIGH) — SessionBuffer de captura: SOLO si el pane ya
                    // tiene buffer (ctx registrado por pty_spawn = CLI de agente + autocapture on).
                    // Guardamos el texto CRUDO (solo ANSI-stripped, SIN scrub por línea). El scrub
                    // NO corre acá: corre como ÚNICO gate autoritativo en `scrub_buffer` justo antes
                    // de CUALQUIER egreso (AIE / propuestas / incomplete_sessions). Razón: el scrub
                    // por línea defeatea la detección de secretos PARTIDOS entre líneas
                    // (`sk-proj-ABCDEF\nGHIJ`: la línea 1, si el head ≥16 chars, ya quedaría
                    // `[REDACTED:sk]` → al de-newlinear desde texto saneado el prefijo desaparece y
                    // el TAIL sobrevive). El crudo vive SOLO acá: RAM volátil, acotado a 500 líneas,
                    // purgado al cerrar el pane, y NUNCA egresa sin pasar por `scrub_buffer`.
                    // Modelo de amenaza completo en `SessionBuffer` (más abajo). Si el pane no es de
                    // captura, esto es un no-op (cero overhead con la feature apagada).
                    let mut cap = session_capture.lock();
                    if let Some(b) = cap.get_mut(&pane_id) {
                        b.push_raw(&stripped);
                    }
                }
                if app
                    .emit(
                        "pty:data",
                        PtyOutput {
                            pane_id: pane_id.clone(),
                            data: chunk,
                        },
                    )
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod scrollback_tests {
    use super::*;

    #[test]
    fn strips_ansi_csi_and_osc() {
        let raw = "\x1b[31mred\x1b[0m \x1b]0;title\x07plain";
        assert_eq!(strip_ansi(raw), "red plain");
    }

    #[test]
    fn keeps_tab_and_newline() {
        let raw = "a\tb\nc";
        assert_eq!(strip_ansi(raw), "a\tb\nc");
    }

    #[test]
    fn scrollback_splits_lines_and_caps() {
        let mut sb = Scrollback::default();
        for i in 0..60 {
            sb.push(&format!("line{}\n", i));
        }
        let snap = sb.snapshot();
        // capped at 50 completed lines, no partial
        assert_eq!(snap.len(), SCROLLBACK_MAX_LINES);
        assert_eq!(snap.first().unwrap(), "line10");
        assert_eq!(snap.last().unwrap(), "line59");
    }

    #[test]
    fn scrollback_includes_partial_line() {
        let mut sb = Scrollback::default();
        sb.push("done\nin progress");
        let snap = sb.snapshot();
        assert_eq!(snap, vec!["done".to_string(), "in progress".to_string()]);
    }

    #[test]
    fn carriage_return_overwrites_line() {
        // progress bar style: "10%\r20%\r30%" should show only "30%"
        let mut sb = Scrollback::default();
        sb.push("10%\r20%\r30%");
        assert_eq!(sb.snapshot(), vec!["30%".to_string()]);
    }

    #[test]
    fn crlf_is_a_single_line_break() {
        let mut sb = Scrollback::default();
        sb.push("a\r\nb");
        assert_eq!(sb.snapshot(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn partial_line_is_byte_capped() {
        let mut sb = Scrollback::default();
        sb.push(&"x".repeat(SCROLLBACK_MAX_PARTIAL + 5000));
        assert!(sb.partial.len() <= SCROLLBACK_MAX_PARTIAL);
    }
}

// 019 F3 (audit HIGH-1/HIGH-2) — semántica de señales de control de proceso. Validan el
// comportamiento OS load-bearing: SIGKILL mata un proceso bajo SIGSTOP (HIGH-2) y los helpers de
// señal reportan correctamente (base de SIGSTOP-FIRST en pause/resume, HIGH-1).
#[cfg(all(test, unix))]
mod signal_tests {
    use super::{send_unix_kill, send_unix_signal, PtySignal};
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// Espera a que un pid deje de existir (kill(pid,0) → ESRCH), hasta `timeout`. Reapa al hijo
    /// para que no quede zombie (waitpid) cuando el proceso es nuestro child directo.
    fn wait_gone(pid: i32, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            // Reapar: si ya salió y es nuestro hijo, esto lo saca de la tabla de procesos.
            let mut status = 0i32;
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            if !alive {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// HIGH-2: un proceso PAUSADO (SIGSTOP) NO muere con SIGHUP/SIGTERM (quedan pendientes), pero
    /// SÍ con la secuencia SIGCONT + SIGKILL que usa `terminate_child`. Garantiza que matar un
    /// attempt pausado nunca deja un zombie congelado.
    #[test]
    fn sigkill_terminates_a_stopped_process() {
        let mut child = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;

        // Pausar el proceso (SIGSTOP) — como un attempt pausado.
        assert!(send_unix_signal(pid, PtySignal::Stop).is_ok());

        // Mandarle SIGTERM mientras está pausado: queda PENDIENTE, el proceso sigue vivo.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "SIGTERM no debe matar un proceso bajo SIGSTOP (queda pendiente)"
        );

        // La secuencia de terminate_child: SIGCONT (des-pendea) + SIGKILL (no bloqueable).
        let _ = send_unix_signal(pid, PtySignal::Cont);
        send_unix_kill(pid);

        assert!(
            wait_gone(pid, Duration::from_secs(3)),
            "el proceso pausado debe morir tras SIGCONT+SIGKILL (no zombie congelado)"
        );
        let _ = child.wait();
    }

    /// HIGH-1 (base, ronda 2): SIGSTOP a un pid inexistente NO es error (ESRCH) PERO tampoco es
    /// "señal entregada" → `send_unix_signal` devuelve `Ok(false)`. Esto es lo que hace que `pause`
    /// (vía `PtyManager::signal`) devuelva `Ok(false)` cuando el proceso murió en la ventana entre
    /// `process_id()` y el `kill(SIGSTOP)`, y por ende NO persista un `paused_at` inconsistente.
    /// Simula esa carrera: `process_id()` ya había visto el PID, pero el proceso muere antes del kill.
    #[test]
    fn signal_to_dead_pid_returns_ok_false() {
        let mut child = Command::new("sleep")
            .arg("0.01")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        let _ = child.wait(); // reapar → pid muerto (proceso murió tras "verse" su PID)
                              // SIGSTOP a un pid ya muerto → ESRCH → Ok(false) (no entregada, no Err).
        assert!(
            !send_unix_signal(pid, PtySignal::Stop).expect("ESRCH no debe ser Err"),
            "señal a un pid muerto NO se entrega → Ok(false), para que pause no persista el flag"
        );
    }

    /// HIGH-1 (positivo, ronda 2): a un proceso VIVO la señal SÍ se entrega → `Ok(true)`. Es la otra
    /// cara de la moneda: sólo así `pause` persiste el flag (señal realmente aplicada).
    #[test]
    fn signal_to_live_pid_returns_ok_true() {
        let mut child = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id() as i32;
        assert!(
            send_unix_signal(pid, PtySignal::Stop).expect("SIGSTOP a proceso vivo no debe fallar"),
            "señal a un proceso vivo se entrega → Ok(true)"
        );
        // limpieza: des-pendear + matar para no dejar el sleep colgado.
        let _ = send_unix_signal(pid, PtySignal::Cont);
        send_unix_kill(pid);
        let _ = child.wait();
    }
}

// 023 (fix audit codex HIGH) — INTEGRACIÓN del PATH REAL de captura: el SessionBuffer del reader
// debe guardar CRUDO para que `scrub_buffer` (el único gate de egreso) pueda cazar secretos
// PARTIDOS entre líneas. Estos tests alimentan el SessionBuffer EXACTAMENTE como lo hace el reader
// (`push_raw` por chunk → splitea por línea) y después corren el gate de egreso real
// (`memory_autocapture::scrub_buffer` sobre `lines_snapshot()`), asegurando que ni el head ni el
// tail de un secreto partido sobrevivan. Si `push_raw` volviera a pre-scrubear por línea (el bug),
// estos tests FALLAN (el tail sobreviviría) — son el guardia de regresión del path de datos real.
#[cfg(test)]
mod capture_buffer_integration_tests {
    use super::SessionBuffer;
    use crate::services::memory_autocapture::scrub_buffer;

    /// Alimenta el SessionBuffer como el reader: cada elemento = un chunk recibido del PTY (con su
    /// `\n` terminador) → `push_raw` lo splitea en líneas. Devuelve el texto de egreso ya pasado por
    /// el ÚNICO gate (`scrub_buffer` sobre el snapshot), que es lo que iría al AIE / a la DB.
    fn capture_then_scrub(chunks: &[&str]) -> String {
        let mut buf = SessionBuffer::default();
        for c in chunks {
            buf.push_raw(c);
        }
        scrub_buffer(&buf.lines_snapshot())
    }

    // `sk-...` partido: head ≥16 chars (matchea solo) en un chunk, tail en el siguiente. Reproduce el
    // leak del audit: con pre-scrub por línea el tail sobrevivía. Con buffer crudo, no.
    #[test]
    fn reader_path_no_sk_split_leak() {
        let out = capture_then_scrub(&[
            "exporting credentials\n",
            "sk-proj-ABCDEFGHIJKLMNOP\n", // head matchea solo.
            "QRSTUVWXYZ0123\n",           // tail que sobrevivía con el path viejo.
        ]);
        assert!(!out.contains("sk-proj-ABCDEFGHIJKLMNOP"), "head no sobrevive (path real reader)");
        assert!(
            !out.contains("QRSTUVWXYZ0123"),
            "el TAIL del secreto partido NO debe sobrevivir tras pasar por el buffer del reader + scrub_buffer"
        );
        assert!(!out.contains("sk-proj"), "ningún rastro del prefijo sk");
        assert!(out.contains("[REDACTED:split-secret]"));
        assert!(out.contains("exporting credentials"), "texto legítimo intacto");
    }

    // `Bearer <head>` partido: idem regresión sobre el path real del reader.
    #[test]
    fn reader_path_no_bearer_split_leak() {
        let out = capture_then_scrub(&[
            "auth header:\n",
            "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6Ik\n", // head matchea solo.
            "pXVCJ9abcdefTAIL\n",                       // tail que sobrevivía.
        ]);
        assert!(!out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6Ik"), "head del Bearer no sobrevive");
        assert!(
            !out.contains("pXVCJ9abcdefTAIL"),
            "el TAIL del Bearer partido NO debe sobrevivir por el path real del reader"
        );
        assert!(out.contains("[REDACTED:split-secret]"));
        assert!(out.contains("auth header:"));
    }

    // Texto legítimo multilínea (código) por el path real → no hay redacción espuria.
    #[test]
    fn reader_path_keeps_legit_multiline() {
        let out = capture_then_scrub(&[
            "fn add(a: i32, b: i32) -> i32 {\n",
            "    a + b\n",
            "}\n",
        ]);
        assert!(!out.contains("[REDACTED"), "no redactar texto legítimo");
        assert!(out.contains("fn add(a: i32, b: i32)"));
        assert!(out.contains("a + b"));
    }
}
