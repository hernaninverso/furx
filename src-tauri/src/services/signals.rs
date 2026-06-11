// services/signals.rs — 010-furx-signals.
//
// Capa de eventos + notificaciones multi-canal. Council 2026-05-29: CONSTRUIR
// (no adoptar Apprise/ntfy/Novu — violan BYOK). Núcleo:
//   - SignalEvent (+ priority/tags/actions estilo ntfy) persistido en `signal_events`.
//   - emit_signal(db, ev): inserta el evento (productores: 008 transiciones, agent input,
//     council). NO bloquea: el dispatcher despacha aparte.
//   - trait Sink + DesktopSink / MobileSink / TelegramSink / WebhookSink.
//   - SignalRouter: worker tokio persistente que lee eventos no despachados → crea una
//     `signal_deliveries` por canal habilitado (filtro por signal_subscriptions) →
//     despacha en paralelo → reintenta pending/failed con backoff. Sobrevive reinicios
//     (la verdad vive en SQLite; el worker es stateless).
//
// BYOK: los tokens/secretos van SÓLO al Keychain; este módulo nunca los persiste en SQLite
// ni los manda al backend.

use anyhow::{anyhow, Result};
use chrono::{Duration as ChronoDuration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Tipos de evento emitidos por los productores (FR-004).
pub const EVENT_TYPES: &[&str] = &[
    "task.done",
    "task.failed",
    "task.awaiting_review",
    "agent.input_requested",
    "council.ready",
];

/// Canales soportados en v1 (el trait Sink habilita más después).
pub const CHANNELS: &[&str] = &["desktop", "mobile", "telegram", "webhook"];

const MAX_ATTEMPTS: i64 = 5;
/// Backoff base (segundos): 2^attempt * BASE, cap a 1h.
const BACKOFF_BASE_SECS: i64 = 5;
const BACKOFF_CAP_SECS: i64 = 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvent {
    pub id: String,
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default = "default_severity")]
    pub severity: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// JSON extra estilo ntfy: priority/tags/actions. Opcional.
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

fn default_severity() -> String {
    "info".to_string()
}

impl SignalEvent {
    /// Constructor mínimo con id generado.
    pub fn new(event_type: &str, severity: &str, title: &str, body: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            project_key: None,
            task_id: None,
            agent_id: None,
            event_type: event_type.to_string(),
            severity: severity.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            payload: None,
        }
    }

    pub fn with_task(mut self, task_id: &str) -> Self {
        self.task_id = Some(task_id.to_string());
        self
    }

    pub fn with_project(mut self, project_key: &str) -> Self {
        self.project_key = Some(project_key.to_string());
        self
    }
}

fn valid_severity(s: &str) -> bool {
    matches!(s, "info" | "warning" | "critical")
}

fn severity_rank(s: &str) -> u8 {
    match s {
        "warning" => 1,
        "critical" => 2,
        _ => 0, // info / unknown
    }
}

/// Inserta un evento. NO crea deliveries (eso lo hace el router). Devuelve el id.
/// Valida tipo/severity defensivamente (productores internos, pero fail-closed).
pub fn emit_signal(db: &Db, ev: &SignalEvent) -> Result<String> {
    if ev.event_type.trim().is_empty() {
        return Err(anyhow!("signal event_type vacío"));
    }
    let severity = if valid_severity(&ev.severity) {
        ev.severity.as_str()
    } else {
        "info"
    };
    let payload_str = match &ev.payload {
        Some(v) => Some(serde_json::to_string(v)?),
        None => None,
    };
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO signal_events (id, project_key, task_id, agent_id, type, severity, title, body, payload)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            ev.id,
            ev.project_key,
            ev.task_id,
            ev.agent_id,
            ev.event_type,
            severity,
            clip(&ev.title, 200),
            clip(&ev.body, 1000),
            payload_str,
        ],
    )?;
    Ok(ev.id.clone())
}

/// Atajo: arma + emite un evento de tarea (008). project_key opcional para ownership.
pub fn emit_task_event(
    db: &Db,
    event_type: &str,
    task_id: &str,
    project_key: Option<&str>,
    title: &str,
    body: &str,
    severity: &str,
) -> Result<String> {
    let mut ev = SignalEvent::new(event_type, severity, title, body).with_task(task_id);
    if let Some(pk) = project_key {
        ev = ev.with_project(pk);
    }
    emit_signal(db, &ev)
}

// ── Subscriptions / filtros (FR-008) ────────────────────────────────────────

/// Default de un canal cuando no hay fila explícita en signal_subscriptions.
/// desktop/telegram ON; mobile ON (reusa pairing); webhook OFF (opt-in explícito).
fn channel_default_enabled(channel: &str) -> bool {
    !matches!(channel, "webhook")
}

/// ¿Está habilitado `channel` para este `event_type`/`severity`?
/// Mira la fila exacta (type,channel), luego ('*',channel), luego el default del canal.
/// Filtra por min_severity.
pub fn channel_enabled_for(
    conn: &Connection,
    channel: &str,
    event_type: &str,
    severity: &str,
) -> bool {
    let lookup = |etype: &str| -> Option<(bool, String)> {
        conn.query_row(
            "SELECT enabled, min_severity FROM signal_subscriptions WHERE event_type = ?1 AND channel = ?2",
            params![etype, channel],
            |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, String>(1)?)),
        )
        .ok()
    };
    let (enabled, min_sev) = match lookup(event_type).or_else(|| lookup("*")) {
        Some(row) => row,
        None => (channel_default_enabled(channel), "info".to_string()),
    };
    enabled && severity_rank(severity) >= severity_rank(&min_sev)
}

/// Set/clear de un filtro de canal (Settings → Integrations).
pub fn set_subscription(
    db: &Db,
    event_type: &str,
    channel: &str,
    enabled: bool,
    min_severity: &str,
) -> Result<()> {
    if !CHANNELS.contains(&channel) {
        return Err(anyhow!("canal desconocido: {}", channel));
    }
    let min_sev = if valid_severity(min_severity) {
        min_severity
    } else {
        "info"
    };
    let conn = db.lock();
    conn.execute(
        "INSERT INTO signal_subscriptions (event_type, channel, enabled, min_severity)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(event_type, channel) DO UPDATE SET enabled=excluded.enabled, min_severity=excluded.min_severity",
        params![event_type, channel, enabled as i64, min_sev],
    )?;
    Ok(())
}

// ── Sink trait + implementaciones ────────────────────────────────────────────

/// Resultado de un intento de entrega.
#[derive(Debug)]
pub enum SinkOutcome {
    Sent,
    Skipped(String),
    Failed(String),
}

/// Un canal de salida. Async: los sinks de red (telegram/webhook) hacen I/O.
#[async_trait::async_trait]
pub trait Sink: Send + Sync {
    /// Identificador del canal ("desktop"|"mobile"|"telegram"|"webhook").
    fn channel(&self) -> &'static str;
    /// ¿Está configurado/disponible este sink? (ej. token presente). Si no, las
    /// deliveries se marcan `skipped` (no `failed`, no consume retries).
    fn available(&self) -> bool {
        true
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome;
}

/// DesktopSink — publica al bus de notificaciones nativo (mobile_bridge ya tiene un
/// `publish_notification` que sirve a UI/phone). Para el toast/notificación nativa de
/// escritorio el wiring real con tauri-plugin-notification se hace en el AppHandle-sink
/// (ver `DesktopNotifSink`). Este sink base es el path testeable sin Tauri.
pub struct DesktopBusSink;

#[async_trait::async_trait]
impl Sink for DesktopBusSink {
    fn channel(&self) -> &'static str {
        "desktop"
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome {
        // Publica también al bus de phones/UI; el toast de escritorio lo dispara el
        // DesktopNotifSink (con AppHandle) cuando está disponible.
        crate::services::mobile_bridge::publish_notification(
            "signal",
            &ev.title,
            &ev.body,
            &ev.severity,
            Some(ev.id.clone()),
        );
        SinkOutcome::Sent
    }
}

/// DesktopNotifSink — toast nativo de escritorio vía tauri-plugin-notification + el bus de
/// phones/UI. Requiere AppHandle (sólo disponible runtime; en tests usamos DesktopBusSink).
pub struct DesktopNotifSink {
    pub app: tauri::AppHandle,
}

#[async_trait::async_trait]
impl Sink for DesktopNotifSink {
    fn channel(&self) -> &'static str {
        "desktop"
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome {
        use tauri_plugin_notification::NotificationExt;
        // Bus para phones/UI en vivo.
        crate::services::mobile_bridge::publish_notification(
            "signal",
            &ev.title,
            &ev.body,
            &ev.severity,
            Some(ev.id.clone()),
        );
        // Toast nativo. Si el plugin no está disponible/permitido, lo tratamos como Sent
        // (el bus ya entregó); no queremos quemar retries por una notif de UI.
        match self
            .app
            .notification()
            .builder()
            .title(if ev.title.is_empty() {
                "Furx"
            } else {
                &ev.title
            })
            .body(&ev.body)
            .show()
        {
            Ok(_) => SinkOutcome::Sent,
            Err(e) => {
                tracing::debug!("desktop notification show failed (non-fatal): {}", e);
                SinkOutcome::Sent
            }
        }
    }
}

/// MobileSink — broadcast al companion vía mobile_bridge (reusa pairing/HMAC del bridge,
/// NO crea identidad nueva — FR-003/US3).
pub struct MobileSink;

#[async_trait::async_trait]
impl Sink for MobileSink {
    fn channel(&self) -> &'static str {
        "mobile"
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome {
        crate::services::mobile_bridge::publish_notification(
            "signal",
            &ev.title,
            &ev.body,
            &ev.severity,
            Some(ev.id.clone()),
        );
        SinkOutcome::Sent
    }
}

/// TelegramSink — POST firmado (HMAC+nonce) al relay del usuario (telegram.rs).
/// Lee endpoint de settings + secreto del Keychain (BYOK). Si falta cualquiera → skipped.
pub struct TelegramSink {
    pub endpoint: Option<String>,
    pub secret: Option<String>,
}

impl TelegramSink {
    /// Construye leyendo endpoint de settings + secreto del Keychain.
    pub fn from_db(db: &Db) -> Self {
        let endpoint = {
            let conn = db.lock();
            crate::settings::get(&conn, "endpoints.telegram_relay")
                .ok()
                .flatten()
                .and_then(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
        };
        let secret = crate::services::telegram::read_secret();
        Self { endpoint, secret }
    }
}

#[async_trait::async_trait]
impl Sink for TelegramSink {
    fn channel(&self) -> &'static str {
        "telegram"
    }
    fn available(&self) -> bool {
        self.endpoint.is_some() && self.secret.is_some()
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome {
        let (endpoint, secret) = match (&self.endpoint, &self.secret) {
            (Some(e), Some(s)) => (e, s),
            _ => return SinkOutcome::Skipped("telegram no configurado".into()),
        };
        // Reusa el path firmado de telegram.rs (card-shaped, severity-aware).
        match crate::services::telegram::post_card(
            endpoint,
            secret,
            &ev.id,
            &ev.title,
            &ev.severity,
        )
        .await
        {
            Ok(send) if (200..300).contains(&send.status) => SinkOutcome::Sent,
            Ok(send) => SinkOutcome::Failed(format!("telegram status {}", send.status)),
            Err(e) => SinkOutcome::Failed(format!("telegram: {}", e)),
        }
    }
}

/// WebhookSink — POST genérico a una URL de la allowlist, firmado HMAC. Auth-token
/// del Keychain (BYOK). Off por default (canal opt-in).
pub struct WebhookSink {
    pub url: Option<String>,
    pub secret: Option<String>,
}

impl WebhookSink {
    pub fn from_db(db: &Db) -> Self {
        let url = {
            let conn = db.lock();
            crate::settings::get(&conn, "signals.webhook_url")
                .ok()
                .flatten()
                .and_then(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
        };
        let secret = crate::services::keychain::load("furx-signals", "webhook-secret");
        Self { url, secret }
    }
}

#[async_trait::async_trait]
impl Sink for WebhookSink {
    fn channel(&self) -> &'static str {
        "webhook"
    }
    fn available(&self) -> bool {
        self.url.is_some()
    }
    async fn deliver(&self, ev: &SignalEvent) -> SinkOutcome {
        let url = match &self.url {
            Some(u) => u,
            None => return SinkOutcome::Skipped("webhook URL no configurada".into()),
        };
        if !crate::bases::allowlist::url_allowed(url) {
            return SinkOutcome::Failed(format!("webhook URL fuera de allowlist: {}", url));
        }
        let body = match serde_json::to_string(&json!({
            "type": ev.event_type,
            "severity": ev.severity,
            "title": ev.title,
            "body": ev.body,
            "task_id": ev.task_id,
            "event_id": ev.id,
        })) {
            Ok(b) => b,
            Err(e) => return SinkOutcome::Failed(format!("webhook serialize: {}", e)),
        };
        let nonce = Uuid::new_v4().to_string();
        let ts = Utc::now().timestamp();
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => return SinkOutcome::Failed(format!("webhook client: {}", e)),
        };
        let mut req = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-Furx-Nonce", &nonce)
            .header("X-Furx-Ts", ts.to_string());
        // HMAC opcional (firma con el secreto del Keychain si está presente).
        if let Some(secret) = &self.secret {
            match crate::services::telegram::sign(secret, &nonce, ts, &body) {
                Ok(sig) => req = req.header("X-Furx-Sig", sig),
                Err(e) => return SinkOutcome::Failed(format!("webhook sign: {}", e)),
            }
        }
        match req.body(body).send().await {
            Ok(resp) if resp.status().is_success() => SinkOutcome::Sent,
            Ok(resp) => SinkOutcome::Failed(format!("webhook status {}", resp.status().as_u16())),
            Err(e) => SinkOutcome::Failed(format!("webhook: {}", e)),
        }
    }
}

// ── Dispatcher / router (FR-002) ─────────────────────────────────────────────

/// Crea las `signal_deliveries` faltantes para eventos no despachados. Idempotente:
/// `INSERT OR IGNORE` por (event_id, channel), respeta filtros. Marca `dispatched_at`
/// del evento. Devuelve cuántos eventos procesó (para tests/observabilidad).
pub fn materialize_deliveries(db: &Db, sinks: &[Box<dyn Sink>]) -> Result<usize> {
    // Snapshot de eventos pendientes (id, type, severity).
    let events: Vec<(String, String, String)> = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, type, severity FROM signal_events WHERE dispatched_at IS NULL ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    let now = Utc::now().to_rfc3339();
    for (event_id, event_type, severity) in &events {
        let conn = db.lock();
        for sink in sinks {
            let channel = sink.channel();
            if !channel_enabled_for(&conn, channel, event_type, severity) {
                // Filtrado: delivery skipped (queda registro de que el filtro lo apartó).
                conn.execute(
                    "INSERT OR IGNORE INTO signal_deliveries (event_id, channel, status, updated_at)
                     VALUES (?1,?2,'skipped',?3)",
                    params![event_id, channel, now],
                )?;
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO signal_deliveries (event_id, channel, status, updated_at)
                 VALUES (?1,?2,'pending',?3)",
                params![event_id, channel, now],
            )?;
        }
        conn.execute(
            "UPDATE signal_events SET dispatched_at = ?2 WHERE id = ?1 AND dispatched_at IS NULL",
            params![event_id, now],
        )?;
    }
    Ok(events.len())
}

fn backoff_secs(attempts: i64) -> i64 {
    let shift = attempts.clamp(0, 20) as u32;
    let v = BACKOFF_BASE_SECS.saturating_mul(2_i64.saturating_pow(shift));
    v.min(BACKOFF_CAP_SECS)
}

/// Carga un SignalEvent completo por id (para pasarlo a los sinks).
fn load_event(conn: &Connection, event_id: &str) -> Option<SignalEvent> {
    conn.query_row(
        "SELECT id, project_key, task_id, agent_id, type, severity, title, body, payload
         FROM signal_events WHERE id = ?1",
        params![event_id],
        |r| {
            let payload_str: Option<String> = r.get(8)?;
            Ok(SignalEvent {
                id: r.get(0)?,
                project_key: r.get(1)?,
                task_id: r.get(2)?,
                agent_id: r.get(3)?,
                event_type: r.get(4)?,
                severity: r.get(5)?,
                title: r.get(6)?,
                body: r.get(7)?,
                payload: payload_str.and_then(|s| serde_json::from_str(&s).ok()),
            })
        },
    )
    .ok()
}

/// Procesa las deliveries `pending`/`failed` cuyo `next_retry_at` ya venció. Despacha en
/// paralelo a los sinks. Devuelve (sent, failed, skipped) para tests.
pub async fn process_pending(db: &Db, sinks: &[Box<dyn Sink>]) -> Result<(usize, usize, usize)> {
    let now_str = Utc::now().to_rfc3339();
    // Claim del trabajo: (event_id, channel, attempts) elegibles ahora.
    let due: Vec<(String, String, i64)> = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT event_id, channel, attempts FROM signal_deliveries
             WHERE status IN ('pending','failed')
               AND attempts < ?1
               AND (next_retry_at IS NULL OR next_retry_at <= ?2)",
        )?;
        let rows = stmt
            .query_map(params![MAX_ATTEMPTS, now_str], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };

    let mut sent = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;

    for (event_id, channel, attempts) in due {
        // Buscar el sink de ese canal.
        let Some(sink) = sinks.iter().find(|s| s.channel() == channel) else {
            continue;
        };
        let ev = {
            let conn = db.lock();
            load_event(&conn, &event_id)
        };
        let Some(ev) = ev else { continue };

        if !sink.available() {
            // Canal no configurado → skipped (no quema retries).
            let conn = db.lock();
            conn.execute(
                "UPDATE signal_deliveries SET status='skipped', last_error='sink unavailable', updated_at=?3
                 WHERE event_id=?1 AND channel=?2",
                params![event_id, channel, Utc::now().to_rfc3339()],
            )?;
            skipped += 1;
            continue;
        }

        let outcome = sink.deliver(&ev).await;
        let conn = db.lock();
        let now = Utc::now().to_rfc3339();
        match outcome {
            SinkOutcome::Sent => {
                conn.execute(
                    "UPDATE signal_deliveries SET status='sent', attempts=attempts+1, last_error=NULL, next_retry_at=NULL, updated_at=?3
                     WHERE event_id=?1 AND channel=?2",
                    params![event_id, channel, now],
                )?;
                sent += 1;
            }
            SinkOutcome::Skipped(reason) => {
                conn.execute(
                    "UPDATE signal_deliveries SET status='skipped', last_error=?3, updated_at=?4
                     WHERE event_id=?1 AND channel=?2",
                    params![event_id, channel, reason, now],
                )?;
                skipped += 1;
            }
            SinkOutcome::Failed(err) => {
                let new_attempts = attempts + 1;
                if new_attempts >= MAX_ATTEMPTS {
                    conn.execute(
                        "UPDATE signal_deliveries SET status='failed', attempts=?3, last_error=?4, next_retry_at=NULL, updated_at=?5
                         WHERE event_id=?1 AND channel=?2",
                        params![event_id, channel, new_attempts, err, now],
                    )?;
                } else {
                    let next = (Utc::now() + ChronoDuration::seconds(backoff_secs(new_attempts)))
                        .to_rfc3339();
                    conn.execute(
                        "UPDATE signal_deliveries SET status='failed', attempts=?3, last_error=?4, next_retry_at=?5, updated_at=?6
                         WHERE event_id=?1 AND channel=?2",
                        params![event_id, channel, new_attempts, err, next, now],
                    )?;
                }
                failed += 1;
            }
        }
    }
    Ok((sent, failed, skipped))
}

/// Un tick del router: materializa deliveries nuevas + procesa las pendientes.
pub async fn tick(db: &Db, sinks: &[Box<dyn Sink>]) -> Result<(usize, usize, usize)> {
    materialize_deliveries(db, sinks)?;
    process_pending(db, sinks).await
}

/// Construye el set de sinks base (sin AppHandle). El DesktopNotifSink (con toast nativo)
/// se inserta en lib.rs reemplazando a DesktopBusSink cuando hay AppHandle.
pub fn build_default_sinks(db: &Db) -> Vec<Box<dyn Sink>> {
    vec![
        Box::new(DesktopBusSink),
        Box::new(MobileSink),
        Box::new(TelegramSink::from_db(db)),
        Box::new(WebhookSink::from_db(db)),
    ]
}

/// Worker persistente: corre `tick` cada `interval`. Sobrevive reinicios porque la verdad
/// vive en SQLite (los sinks se reconstruyen cada tick para tomar cambios de config/Keychain).
/// Reconstruye sinks vía `build_sinks` callback (permite inyectar el DesktopNotifSink real).
pub async fn run_router_loop<F>(db: Db, interval: std::time::Duration, build_sinks: F)
where
    F: Fn(&Db) -> Vec<Box<dyn Sink>> + Send + 'static,
{
    loop {
        let sinks = build_sinks(&db);
        if let Err(e) = tick(&db, &sinks).await {
            tracing::warn!("signals router tick error: {}", e);
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/023_signals.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// Sink de prueba: cuenta entregas y puede fallar N veces antes de tener éxito.
    struct CountingSink {
        ch: &'static str,
        delivered: Arc<AtomicUsize>,
        fail_first: usize,
        avail: bool,
    }
    impl CountingSink {
        fn new(ch: &'static str) -> Self {
            Self {
                ch,
                delivered: Arc::new(AtomicUsize::new(0)),
                fail_first: 0,
                avail: true,
            }
        }
    }
    #[async_trait::async_trait]
    impl Sink for CountingSink {
        fn channel(&self) -> &'static str {
            self.ch
        }
        fn available(&self) -> bool {
            self.avail
        }
        async fn deliver(&self, _ev: &SignalEvent) -> SinkOutcome {
            let n = self.delivered.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                SinkOutcome::Failed(format!("transient #{}", n))
            } else {
                SinkOutcome::Sent
            }
        }
    }

    #[tokio::test]
    async fn dispatch_to_n_sinks() {
        let db = test_db();
        let ev = SignalEvent::new("task.failed", "critical", "T1 falló", "boom");
        emit_signal(&db, &ev).unwrap();

        let s1 = CountingSink::new("desktop");
        let s2 = CountingSink::new("telegram");
        let c1 = s1.delivered.clone();
        let c2 = s2.delivered.clone();
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(s1), Box::new(s2)];

        let (sent, failed, _skipped) = tick(&db, &sinks).await.unwrap();
        assert_eq!(sent, 2);
        assert_eq!(failed, 0);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);

        // Estado en DB.
        let conn = db.lock();
        let n_sent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_deliveries WHERE status='sent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n_sent, 2);
    }

    #[tokio::test]
    async fn idempotent_event_channel() {
        // Re-correr tick NO crea deliveries duplicadas ni re-despacha lo ya enviado.
        let db = test_db();
        emit_signal(&db, &SignalEvent::new("task.done", "info", "ok", "")).unwrap();
        let s = CountingSink::new("desktop");
        let counter = s.delivered.clone();
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(s)];

        tick(&db, &sinks).await.unwrap();
        tick(&db, &sinks).await.unwrap(); // segundo tick: nada que hacer
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "no re-despacha lo ya enviado"
        );

        let conn = db.lock();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM signal_deliveries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "una sola delivery por (event,channel)");
    }

    #[tokio::test]
    async fn retry_pending_after_restart() {
        // Simula reinicio: materializamos deliveries, NO las procesamos (worker murió),
        // luego un nuevo "boot" re-corre el router y completa las pendientes.
        let db = test_db();
        emit_signal(&db, &SignalEvent::new("task.failed", "warning", "x", "")).unwrap();
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(DesktopBusSink)];
        // "Boot 1": sólo materializa (no despacha).
        materialize_deliveries(&db, &sinks).unwrap();
        {
            let conn = db.lock();
            let pending: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM signal_deliveries WHERE status='pending'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pending, 1);
        }
        // "Boot 2": el router re-corre y completa la pending.
        let (sent, _f, _s) = process_pending(&db, &sinks).await.unwrap();
        assert_eq!(sent, 1);
        let conn = db.lock();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_deliveries WHERE status='pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "la pending se completó tras el reinicio");
    }

    #[tokio::test]
    async fn failed_sink_retries_with_backoff_then_succeeds() {
        let db = test_db();
        emit_signal(&db, &SignalEvent::new("task.failed", "critical", "x", "")).unwrap();
        let mut s = CountingSink::new("desktop");
        s.fail_first = 1; // falla 1 vez, después OK
        let counter = s.delivered.clone();
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(s)];

        // Tick 1: falla → queda failed con next_retry_at futuro.
        let (sent1, failed1, _) = tick(&db, &sinks).await.unwrap();
        assert_eq!(sent1, 0);
        assert_eq!(failed1, 1);
        {
            let conn = db.lock();
            let (status, attempts, has_next): (String, i64, bool) = conn
                .query_row(
                    "SELECT status, attempts, next_retry_at IS NOT NULL FROM signal_deliveries",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
                )
                .unwrap();
            assert_eq!(status, "failed");
            assert_eq!(attempts, 1);
            assert!(has_next, "backoff fijó next_retry_at");
            // Forzamos vencimiento del backoff para el test.
            conn.execute(
                "UPDATE signal_deliveries SET next_retry_at = '2000-01-01T00:00:00Z'",
                [],
            )
            .unwrap();
        }
        // Tick 2: backoff vencido → reintenta → OK.
        let (sent2, _f, _s) = process_pending(&db, &sinks).await.unwrap();
        assert_eq!(sent2, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 2, "1 fallo + 1 éxito");
    }

    #[tokio::test]
    async fn filter_skips_disabled_channel() {
        let db = test_db();
        // Desactivar 'telegram' para task.done.
        set_subscription(&db, "task.done", "telegram", false, "info").unwrap();
        emit_signal(&db, &SignalEvent::new("task.done", "info", "ok", "")).unwrap();

        let sinks: Vec<Box<dyn Sink>> = vec![
            Box::new(CountingSink::new("desktop")),
            Box::new(CountingSink::new("telegram")),
        ];
        tick(&db, &sinks).await.unwrap();

        let conn = db.lock();
        let tg_status: String = conn
            .query_row(
                "SELECT status FROM signal_deliveries WHERE channel='telegram'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tg_status, "skipped", "telegram filtrado → skipped");
        let dk_status: String = conn
            .query_row(
                "SELECT status FROM signal_deliveries WHERE channel='desktop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dk_status, "sent");
    }

    #[tokio::test]
    async fn filter_min_severity() {
        let db = test_db();
        // desktop sólo para warning+.
        set_subscription(&db, "*", "desktop", true, "warning").unwrap();
        emit_signal(&db, &SignalEvent::new("task.done", "info", "low", "")).unwrap();
        emit_signal(
            &db,
            &SignalEvent::new("task.failed", "critical", "high", ""),
        )
        .unwrap();

        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(CountingSink::new("desktop"))];
        tick(&db, &sinks).await.unwrap();

        let conn = db.lock();
        let n_skipped: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_deliveries WHERE status='skipped'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let n_sent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM signal_deliveries WHERE status='sent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n_skipped, 1, "info filtrado");
        assert_eq!(n_sent, 1, "critical pasó");
    }

    #[tokio::test]
    async fn unavailable_sink_skips_without_burning_retries() {
        let db = test_db();
        emit_signal(&db, &SignalEvent::new("task.failed", "info", "x", "")).unwrap();
        let mut s = CountingSink::new("telegram");
        s.avail = false;
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(s)];
        let (_sent, _failed, skipped) = tick(&db, &sinks).await.unwrap();
        assert_eq!(skipped, 1);
        let conn = db.lock();
        let (status, attempts): (String, i64) = conn
            .query_row("SELECT status, attempts FROM signal_deliveries", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "skipped");
        assert_eq!(attempts, 0, "sink no disponible no consume retries");
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(0), BACKOFF_BASE_SECS);
        assert_eq!(backoff_secs(1), BACKOFF_BASE_SECS * 2);
        assert!(backoff_secs(20) <= BACKOFF_CAP_SECS);
    }
}
