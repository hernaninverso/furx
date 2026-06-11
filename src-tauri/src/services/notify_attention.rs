//! 033 U4 · Notificación NATIVA en background cuando un pane reclama atención (NeedsInput) y la ventana
//! de Furx NO tiene foco. NÚCLEO PURO + trait `Notifier` inyectable (mock en tests; impl real con
//! AppHandle + `tauri-plugin-notification`). El audio y el badge sólo alcanzan si estás mirando/
//! escuchando — la notificación te llega con la app en background.
//!
//! Principios (NON-NEGOTIABLE, de 030+):
//!  - NO mueve el foco del mic NI trae la ventana al frente (la notificación es INFORMATIVA — finding
//!    del council-review U4: "sin enfoque automático de la ventana"). Traer-al-frente-al-clic = F5.
//!  - Opt-in default OFF (setting `attention.notify.enabled`) + permiso del SO (denegado → no notifica).
//!  - Privacidad: el cuerpo es `'<Agente> necesita atención'` con el nombre por la WHITELIST de 032;
//!    NUNCA contenido del buffer.
//!  - Dedup/rate-limit: máx 1 notificación cada 30s POR AGENTE (no global).

use crate::services::audio_attention::{known_agent_label, Clock};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

const DEDUP_PER_AGENT_MS: u64 = 30_000; // 1 notif / 30s por agente
const PRUNE_MS: u64 = 120_000; // poda de entradas viejas (anti-fuga en uptime largo)
const GENERIC_BODY: &str = "Un agente necesita tu atención.";
const GENERIC_KEY: &str = "\0generic"; // clave de dedup para agentes sin nombre whitelisteado

/// Salida de notificación + estado de la ventana. Inyectable: en tests un mock; en el wiring un impl
/// con `AppHandle` + el plugin de notificaciones.
pub trait Notifier: Send + Sync {
    /// `true` si la ventana principal de Furx tiene foco (⇒ NO notificar; el badge/audio ya están a la
    /// vista). Ante incertidumbre, el impl real es conservador y devuelve `true` (no molestar).
    fn window_focused(&self) -> bool;
    /// Muestra una notificación nativa. Si el permiso está denegado / el plugin no está, degrada sin
    /// panic.
    fn notify(&self, title: &str, body: &str);
}

/// Gestor de notificaciones de atención: opt-in + foco de ventana + dedup-por-agente, sobre un
/// `Notifier`/`Clock` inyectables.
///
/// 034 U2 — CROSS-PLATFORM: este núcleo no tiene NINGÚN `cfg(target_os)`; el gate corre idéntico en
/// macOS/Windows/Linux. Las notificaciones nativas salen por `tauri-plugin-notification` (que usa
/// `notify_rust`: toast en Windows, libnotify/DBus en Linux, NSUserNotification en macOS). El TTS es
/// cross-platform (macOS `say` · Windows SAPI · Linux `spd-say`/`espeak`); sólo el earcon (`afplay`) es
/// de macOS y degrada en silencio fuera de ella (spawn falla → log), sin afectar la notificación. Por
/// eso U2 es VERIFICACIÓN, no código nuevo.
pub struct NotificationManager {
    notifier: Box<dyn Notifier>,
    clock: Box<dyn Clock>,
    /// Resuelve el opt-in (lee el setting `attention.notify.enabled`; default OFF).
    enabled: Box<dyn Fn() -> bool + Send + Sync>,
    /// agente → instante de la última notificación (dedup 30s/agente).
    last_per_agent: Mutex<HashMap<String, u64>>,
}

impl NotificationManager {
    pub fn new(
        notifier: Box<dyn Notifier>,
        clock: Box<dyn Clock>,
        enabled: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            notifier,
            clock,
            enabled,
            last_per_agent: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.last_per_agent.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Evalúa un solo evento NeedsInput (`agent` = su `cli_kind`). Wrapper de `consider_batch`.
    pub fn consider(&self, agent: Option<&str>) -> bool {
        self.consider_batch(&[agent])
    }

    /// 034 U4 — evalúa una TANDA de eventos NeedsInput (los del tick). Gate por agente (opt-in ON →
    /// ventana SIN foco → dedup por agente). Cuenta los agentes DISTINTOS admisibles (no deduplicados):
    ///  - 0 → no notifica.
    ///  - 1 → notif individual ("<Agente> necesita atención" o genérica).
    ///  - ≥2 → UNA notif RESUMEN ("N agentes necesitan atención") en vez de N toasts (anti-saturación).
    /// La reserva del dedup de TODOS los admitidos es atómica bajo un solo lock (antes de notificar).
    /// El resumen NUNCA incluye contenido del buffer (sólo el conteo); los nombres pasan por la
    /// ALLOWLIST `known_agent_label` (un `cli_kind` inesperado → genérico).
    pub fn consider_batch(&self, agents: &[Option<&str>]) -> bool {
        if !(self.enabled)() {
            return false; // opt-in OFF (default) ⇒ cero notificaciones
        }
        if agents.is_empty() {
            return false;
        }
        if self.notifier.window_focused() {
            return false; // estás mirando Furx ⇒ no molestar (badge/audio ya avisan)
        }
        let now = self.clock.now_ms();
        let mut admitted: Vec<(String, Option<String>)> = Vec::new(); // (key, label) distintos admisibles
        {
            let mut last = self.lock();
            last.retain(|_, &mut t| now.saturating_sub(t) < PRUNE_MS); // anti-fuga
            let mut seen: HashSet<String> = HashSet::new();
            for agent in agents {
                let label = agent.and_then(known_agent_label);
                let key = label.clone().unwrap_or_else(|| GENERIC_KEY.to_string());
                if !seen.insert(key.clone()) {
                    continue; // ya contamos este agente distinto en la tanda
                }
                if let Some(&t) = last.get(&key) {
                    if now.saturating_sub(t) < DEDUP_PER_AGENT_MS {
                        continue; // ya notificamos por este agente hace < 30s
                    }
                }
                admitted.push((key, label));
            }
            // RESERVAR el dedup de TODOS los admitidos antes de notificar (atómico).
            for (key, _) in &admitted {
                last.insert(key.clone(), now);
            }
        }
        match admitted.len() {
            0 => false,
            1 => {
                let body = match &admitted[0].1 {
                    Some(l) => format!("{l} necesita atención"),
                    None => GENERIC_BODY.to_string(),
                };
                self.notifier.notify("Furx", &body); // fuera del lock
                true
            }
            n => {
                let body = format!("{n} agentes necesitan atención");
                self.notifier.notify("Furx", &body); // sólo el conteo, nunca el buffer
                true
            }
        }
    }
}

// ── Impl real (wiring) — no se testea a nivel unidad (requiere AppHandle) ───────────────────────────

/// `Notifier` real: foco de la ventana `main` + toast nativo vía `tauri-plugin-notification`.
pub struct TauriNotifier {
    pub app: tauri::AppHandle,
    /// 034 U3 — resolutor del sonido custom del toast (lee `attention.notify.sound`; `None` = sonido por
    /// defecto del SO). Validado al leer. Sólo aplica al toast (NO al earcon/TTS).
    pub sound: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl Notifier for TauriNotifier {
    fn window_focused(&self) -> bool {
        use tauri::Manager;
        // Conservador: si no podemos determinar el foco, asumimos CON foco (true) → NO notificar
        // (preferimos no molestar a arriesgar spam con la app al frente).
        self.app
            .get_webview_window("main")
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(true)
    }

    fn notify(&self, title: &str, body: &str) {
        use tauri_plugin_notification::NotificationExt;
        let mut builder = self.app.notification().builder().title(title).body(body);
        // 034 U3 — sonido custom (validado); si no hay → default del SO (cero regresión).
        if let Some(s) = (self.sound)() {
            builder = builder.sound(s);
        }
        if let Err(e) = builder.show() {
            tracing::debug!("notificación de atención no mostrada (no fatal): {e}");
        }
    }
}

/// 034 U1 — clave del opt-in "traer la ventana al frente al activar la app" (default OFF).
pub const BRING_TO_FRONT_KEY: &str = "attention.notify.bring_to_front";

/// Lee el opt-in de bring-to-front (default false).
pub fn read_bring_to_front(conn: &rusqlite::Connection) -> bool {
    matches!(
        crate::settings::get(conn, BRING_TO_FRONT_KEY),
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

/// 034 U1 — al ACTIVARSE la app (macOS `RunEvent::Reopen`: clic en la notificación o en el dock),
/// trae la ventana `main` al frente (unminimize + show + set_focus) SÓLO si el usuario activó el
/// opt-in. NUNCA toca el foco del mic. Idempotente. Honestidad: `tauri-plugin-notification` 2.3.3 NO
/// expone un callback de clic POR notificación en desktop (`.show()` es fire-and-forget); en macOS el
/// SO activa la app al clickear, y este handler convierte esa activación en "traer Furx al frente"
/// cuando el usuario lo pidió. En otras plataformas (sin Reopen) el opt-in simplemente no dispara.
pub fn focus_main_if_opted(app: &tauri::AppHandle) {
    use tauri::Manager;
    let opted = {
        let state = app.state::<crate::AppState>();
        let conn = state.db.lock();
        read_bring_to_front(&conn)
    };
    if !opted {
        return; // default OFF → no traer la ventana (comportamiento actual)
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        // NUNCA se toca el foco del micrófono — sólo la ventana.
    }
}

/// 034 U1 — lee el opt-in de bring-to-front (para el toggle de la UI).
#[tauri::command]
pub fn attention_notify_bring_to_front_get(state: tauri::State<'_, crate::AppState>) -> bool {
    let conn = state.db.lock();
    read_bring_to_front(&conn)
}

/// 034 U1 — activa/desactiva traer la ventana al frente al activar la app (default OFF).
#[tauri::command]
pub fn attention_notify_bring_to_front_set(
    state: tauri::State<'_, crate::AppState>,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.db.lock();
    crate::settings::set(&conn, BRING_TO_FRONT_KEY, &serde_json::Value::Bool(enabled))
        .map_err(|e| e.to_string())
}

/// Clave del setting del sonido del toast (default "" = sonido del SO).
pub const NOTIFY_SOUND_KEY: &str = "attention.notify.sound";

/// Lee el sonido del toast desde settings, VALIDADO (charset acotado, sin `-` inicial → sin
/// option-injection si algún backend lo pasara como arg, longitud acotada). Vacío/inválido → `None`
/// (default del SO). Sólo afecta al toast, nunca al earcon/TTS.
pub fn read_notify_sound(conn: &rusqlite::Connection) -> Option<String> {
    let raw = match crate::settings::get(conn, NOTIFY_SOUND_KEY) {
        Ok(Some(serde_json::Value::String(s))) => s,
        _ => return None,
    };
    let t = raw.trim();
    if t.is_empty() || t.chars().count() > 64 || t.starts_with('-') {
        return None;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '/'))
    {
        return None;
    }
    Some(t.to_string())
}

/// Clave del setting de opt-in (default OFF).
pub const NOTIFY_ENABLED_KEY: &str = "attention.notify.enabled";

/// Lee el opt-in de notificaciones (default false).
pub fn read_notify_enabled(conn: &rusqlite::Connection) -> bool {
    matches!(
        crate::settings::get(conn, NOTIFY_ENABLED_KEY),
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

// ── Comandos Tauri ──────────────────────────────────────────────────────────────────────────────

/// Lee el opt-in de notificaciones en background (para el toggle de la UI).
#[tauri::command]
pub fn attention_notify_get_enabled(state: tauri::State<'_, crate::AppState>) -> bool {
    let conn = state.db.lock();
    read_notify_enabled(&conn)
}

/// Activa/desactiva las notificaciones en background. Al ACTIVAR, pide el permiso del SO (best-effort;
/// si se deniega, simplemente no se mostrarán notificaciones). Default OFF.
#[tauri::command]
pub fn attention_notify_set_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    enabled: bool,
) -> Result<(), String> {
    {
        let conn = state.db.lock();
        crate::settings::set(&conn, NOTIFY_ENABLED_KEY, &serde_json::Value::Bool(enabled))
            .map_err(|e| e.to_string())?;
    }
    if enabled {
        use tauri_plugin_notification::NotificationExt;
        // Pide permiso explícito (el SO muestra el prompt). Best-effort: ignoramos el resultado — si
        // se deniega, `notify` simplemente fallará en silencio.
        let _ = app.notification().request_permission();
    }
    Ok(())
}

/// 034 U3 — lee el sonido del toast configurado (crudo, para la UI; "" = default del SO).
#[tauri::command]
pub fn attention_notify_sound_get(state: tauri::State<'_, crate::AppState>) -> String {
    let conn = state.db.lock();
    match crate::settings::get(&conn, NOTIFY_SOUND_KEY) {
        Ok(Some(serde_json::Value::String(s))) => s,
        _ => String::new(),
    }
}

/// 034 U3 — guarda el sonido del toast. La VALIDACIÓN ocurre al LEER (`read_notify_sound`), nunca se
/// aplica un valor inválido (cae al default del SO).
#[tauri::command]
pub fn attention_notify_sound_set(
    state: tauri::State<'_, crate::AppState>,
    sound: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    crate::settings::set(&conn, NOTIFY_SOUND_KEY, &serde_json::Value::String(sound))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::audio_attention::Clock;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

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

    #[derive(Clone, Default)]
    struct MockNotifier {
        focused: Arc<AtomicBool>,
        notifs: Arc<Mutex<Vec<(String, String)>>>,
    }
    impl Notifier for MockNotifier {
        fn window_focused(&self) -> bool {
            self.focused.load(Ordering::SeqCst)
        }
        fn notify(&self, title: &str, body: &str) {
            self.notifs.lock().unwrap().push((title.into(), body.into()));
        }
    }

    fn mgr(enabled: bool) -> (NotificationManager, MockNotifier, FakeClock) {
        let n = MockNotifier::default();
        let c = FakeClock::default();
        let m = NotificationManager::new(
            Box::new(n.clone()),
            Box::new(c.clone()),
            Box::new(move || enabled),
        );
        (m, n, c)
    }

    // CA-4.3: opt-in OFF → 0 notificaciones.
    #[test]
    fn opt_out_no_notify() {
        let (m, n, _c) = mgr(false);
        assert!(!m.consider(Some("codex")));
        assert_eq!(n.notifs.lock().unwrap().len(), 0);
    }

    // CA-4.2: ventana con foco → 0 notificaciones.
    #[test]
    fn focused_window_no_notify() {
        let (m, n, _c) = mgr(true);
        n.focused.store(true, Ordering::SeqCst);
        assert!(!m.consider(Some("codex")));
        assert_eq!(n.notifs.lock().unwrap().len(), 0);
    }

    // CA-4.1 + CA-4.5: ventana sin foco + opt-in → 1 notificación con la frase whitelisteada.
    #[test]
    fn unfocused_optin_notifies_with_label() {
        let (m, n, _c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider(Some("codex")));
        let notifs = n.notifs.lock().unwrap();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].0, "Furx");
        assert_eq!(notifs[0].1, "Codex necesita atención");
    }

    // CA-4.4: 3 NeedsInput del mismo agente en 30s → 1 notificación (dedup por agente).
    #[test]
    fn dedup_per_agent_30s() {
        let (m, n, c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider(Some("codex")));
        c.advance(10_000);
        assert!(!m.consider(Some("codex"))); // < 30s
        c.advance(10_000);
        assert!(!m.consider(Some("codex"))); // < 30s
        assert_eq!(n.notifs.lock().unwrap().len(), 1);
        // pasados 30s, vuelve a notificar.
        c.advance(20_000);
        assert!(m.consider(Some("codex")));
        assert_eq!(n.notifs.lock().unwrap().len(), 2);
    }

    // dedup es POR AGENTE: agentes distintos dentro de 30s → una c/u.
    #[test]
    fn distinct_agents_each_notify() {
        let (m, n, _c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider(Some("codex")));
        assert!(m.consider(Some("claude")));
        assert_eq!(n.notifs.lock().unwrap().len(), 2);
    }

    // agente NO conocido → frase genérica (sin exponer texto arbitrario). Prueba que es un ALLOWLIST
    // de agentes, no sólo validación de charset: "myagent" es charset-válido pero desconocido.
    #[test]
    fn unknown_agent_generic_body() {
        let (m, n, _c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider(Some("myagent"))); // charset OK pero NO en el allowlist
        assert_eq!(n.notifs.lock().unwrap()[0].1, GENERIC_BODY);
    }

    // CA-4.1 (034) — 3 agentes DISTINTOS en una tanda → 1 notif RESUMEN (no 3 toasts) + los 3 en dedup.
    #[test]
    fn batch_aggregates_distinct_agents() {
        let (m, n, c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider_batch(&[Some("codex"), Some("claude"), Some("aider")]));
        let notifs = n.notifs.lock().unwrap();
        assert_eq!(notifs.len(), 1, "una sola notif resumen");
        assert_eq!(notifs[0].1, "3 agentes necesitan atención");
        drop(notifs);
        // los 3 quedaron en dedup: una nueva tanda inmediata no re-notifica.
        c.advance(1_000);
        assert!(!m.consider_batch(&[Some("codex"), Some("claude"), Some("aider")]));
    }

    // CA-4.2 (034) — 1 agente en la tanda → notif INDIVIDUAL con su nombre (no resumen).
    #[test]
    fn batch_single_agent_individual() {
        let (m, n, _c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider_batch(&[Some("codex")]));
        assert_eq!(n.notifs.lock().unwrap()[0].1, "Codex necesita atención");
    }

    // CA-4.3 (034) — agentes ya en dedup no cuentan para el resumen; repetidos en la tanda = 1 distinto.
    #[test]
    fn batch_respects_dedup_and_distinctness() {
        let (m, n, c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        assert!(m.consider(Some("codex"))); // codex ya notificó
        c.advance(1_000);
        // tanda: codex (en dedup) + claude (nuevo) + claude (repetido) → sólo claude admisible → individual.
        assert!(m.consider_batch(&[Some("codex"), Some("claude"), Some("claude")]));
        let notifs = n.notifs.lock().unwrap();
        assert_eq!(notifs.len(), 2);
        assert_eq!(notifs[1].1, "Claude necesita atención");
    }

    // 034 U3 — validación del sonido del toast: acepta nombres/paths limpios, rechaza vacío/largo/
    // metacaracteres/`-` inicial → default del SO (None).
    #[test]
    fn notify_sound_validation() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT, updated_at TEXT);",
        )
        .unwrap();
        assert_eq!(read_notify_sound(&conn), None); // sin setting → default
        let set = |v: &str| {
            crate::settings::set(&conn, NOTIFY_SOUND_KEY, &serde_json::json!(v)).unwrap()
        };
        set("Ping");
        assert_eq!(read_notify_sound(&conn).as_deref(), Some("Ping"));
        set("/System/Library/Sounds/Glass.aiff");
        assert_eq!(read_notify_sound(&conn).as_deref(), Some("/System/Library/Sounds/Glass.aiff"));
        set("");
        assert_eq!(read_notify_sound(&conn), None);
        set("-Ping"); // `-` inicial (option-injection) → rechazado
        assert_eq!(read_notify_sound(&conn), None);
        set("a; rm stuff"); // metacaracteres → rechazado
        assert_eq!(read_notify_sound(&conn), None);
    }

    // 034 U2 — CONTRATO cross-platform: el gate del NotificationManager es puro (sin cfg(target_os));
    // se comporta idéntico en cualquier SO. Este test fija ese contrato (corre igual en macOS/Win/Linux):
    // mismo input → mismo veredicto, sin ramas dependientes del SO.
    #[test]
    fn gate_is_platform_agnostic() {
        let (m, n, c) = mgr(true);
        n.focused.store(false, Ordering::SeqCst);
        // opt-in ON + sin foco → notifica (mismo resultado en cualquier plataforma).
        assert!(m.consider(Some("codex")));
        assert_eq!(n.notifs.lock().unwrap().len(), 1);
        // dedup determinista (basado en el Clock inyectado, no en el reloj del SO).
        c.advance(10_000);
        assert!(!m.consider(Some("codex")));
        // con foco → no notifica (regla pura, sin cfg de plataforma).
        n.focused.store(true, Ordering::SeqCst);
        c.advance(60_000);
        assert!(!m.consider(Some("codex")));
    }

    // 034 U1 — opt-in de bring-to-front: default OFF; true sólo con Bool(true).
    #[test]
    fn bring_to_front_default_off() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT, updated_at TEXT);",
        )
        .unwrap();
        assert!(!read_bring_to_front(&conn)); // default OFF
        crate::settings::set(&conn, BRING_TO_FRONT_KEY, &serde_json::json!(true)).unwrap();
        assert!(read_bring_to_front(&conn));
        crate::settings::set(&conn, BRING_TO_FRONT_KEY, &serde_json::json!(false)).unwrap();
        assert!(!read_bring_to_front(&conn));
        // valor no-bool → OFF (fail-closed)
        crate::settings::set(&conn, BRING_TO_FRONT_KEY, &serde_json::json!("yes")).unwrap();
        assert!(!read_bring_to_front(&conn));
    }

    // batch con opt-in OFF / ventana con foco → nada.
    #[test]
    fn batch_gated_by_optin_and_focus() {
        let (m1, n1, _c) = mgr(false);
        assert!(!m1.consider_batch(&[Some("codex"), Some("claude")]));
        assert_eq!(n1.notifs.lock().unwrap().len(), 0);
        let (m2, n2, _c2) = mgr(true);
        n2.focused.store(true, Ordering::SeqCst);
        assert!(!m2.consider_batch(&[Some("codex"), Some("claude")]));
        assert_eq!(n2.notifs.lock().unwrap().len(), 0);
    }
}
