// services/savings_meter.rs — spec-048 · Cost-Router Fase 1 (Savings Meter).
//
// MIDE el ahorro del routing que Furx YA hace (local Ollama / free AIE / premium BYOK). NO desvía
// ninguna decisión de routing (eso es Fase 2). Cada decisión emite —fire-and-forget, sin bloquear el
// hot-path— una traza append-only a `cost_router_events`, y el dashboard (comandos `savings_*`)
// muestra ÚNICAMENTE lo medido (nunca proyecta).
//
// Garantías (NON-NEGOTIABLE, ver spec FR-003/FR-005/FR-009/FR-012):
//   - `emit` NUNCA bloquea: solo hace `tx.send` a un canal unbounded → worker de fondo. Si el send
//     falla (worker caído / OFF), incrementa `dropped` y sigue. NUNCA panic, NUNCA espera I/O de DB.
//   - El worker hace INSERT batch (cada 5s o al llegar a 100 filas) sobre la tabla append-only.
//   - Kill-switch `FURX_COST_ROUTER`: OFF (default) ⇒ el meter no se inicializa ⇒ `emit` es no-op ⇒
//     cero regresión (idéntico al comportamiento actual).
//   - El meter es OBSERVACIONAL: se invoca DESPUÉS de que la decisión de tier ya se tomó.
//   - La tabla NO guarda texto libre (no prompts/diffs/paths/secrets) — solo tier, modelo, tokens y
//     costos. Sin superficie de PII (spec P3).
//
// DIVERGENCIA vs council v6 (documentada en specs/048/analysis): el council asume PostgreSQL
// (ai_engine_db); Furx-cliente es SQLite (rusqlite). Append-only por triggers RAISE(ABORT) (mig 053),
// no `REVOKE`/`TRUNCATE`/roles. El test CI (Rust, abajo) verifica el rechazo real de UPDATE/DELETE.

use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Env flag que prende toda la instrumentación. OFF (ausente / no "1"/"true"/"on") ⇒ meter no-op.
const KILL_SWITCH_ENV: &str = "FURX_COST_ROUTER";

/// Cada cuánto el worker hace flush del batch acumulado (aunque no haya llegado a `BATCH_MAX`).
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// Tamaño de batch que dispara un flush inmediato.
const BATCH_MAX: usize = 100;

/// Tier que resolvió una tarea. Espejo del CHECK de `cost_router_events.decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Resuelto local (Ollama loopback) — cost_real ≈ 0.
    Local,
    /// Resuelto en un free tier (AIE / provider free) — cost_real = 0 para el user.
    Free,
    /// Resuelto en premium BYOK — el user paga (cost_real real).
    Premium,
    /// La tarea fue bloqueada (p.ej. por quota) — sin inferencia.
    Blocked,
}

impl Decision {
    /// String que va a la columna `decision` (debe matchear el CHECK de la migración).
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Local => "local",
            Decision::Free => "free",
            Decision::Premium => "premium",
            Decision::Blocked => "blocked",
        }
    }
}

/// Una traza de ahorro: el qué/cuánto de una decisión de routing. Sin texto libre (no PII).
#[derive(Debug, Clone)]
pub struct RoutingEvent {
    pub decision: Decision,
    pub model_id: Option<String>,
    pub provider: Option<String>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    /// Costo real en USD (≈0 para local/free; el costo BYOK para premium).
    pub cost_real_usd: Option<f64>,
    /// Lo que habría costado todo en premium (baseline). `None` si no se pudo calcular (sin tokens
    /// o sin tabla de precios) → la fila se guarda igual pero NO cuenta para el ahorro.
    pub cost_baseline_premium_usd: Option<f64>,
    /// Versión de la tabla de precios usada para el baseline.
    pub price_table_version: Option<String>,
    /// `true` si el baseline se calculó con el precio DEFAULT (el user no tiene premium BYOK).
    pub baseline_is_default: bool,
}

/// El medidor: dueño del extremo de envío del canal + contador de descartes. Se inyecta en
/// `AppState` envuelto en `Arc`. Cuando el kill-switch está OFF se construye con `disabled()`
/// (sin canal) → `emit` es no-op.
pub struct SavingsMeter {
    tx: Option<UnboundedSender<RoutingEvent>>,
    dropped: AtomicU64,
}

impl SavingsMeter {
    /// Lee el kill-switch. OFF por default (la feature no se prende sola).
    pub fn enabled() -> bool {
        std::env::var(KILL_SWITCH_ENV)
            .map(|v| {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "on"
            })
            .unwrap_or(false)
    }

    /// Meter desactivado (kill-switch OFF): `emit` es no-op. Cero regresión.
    pub fn disabled() -> Self {
        Self {
            tx: None,
            dropped: AtomicU64::new(0),
        }
    }

    /// Construye el meter Y arranca el worker de fondo. El worker drena el canal y hace INSERT batch
    /// sobre `db`. Devuelve `(meter, worker_handle)`; el caller (`lib.rs`) guarda el handle para
    /// abortarlo al cerrar la app. SOLO se llama si `enabled()`.
    pub fn start(db: Arc<Mutex<Connection>>) -> (Self, tauri::async_runtime::JoinHandle<()>) {
        let (tx, rx) = unbounded_channel();
        let handle = tauri::async_runtime::spawn(worker_loop(db, rx));
        (
            Self {
                tx: Some(tx),
                dropped: AtomicU64::new(0),
            },
            handle,
        )
    }

    /// Emite una traza. NUNCA bloquea: solo `tx.send`. Si el meter está OFF (`tx = None`) es un no-op
    /// PURO (no cuenta como "dropped" — un meter apagado no "pierde" eventos, simplemente no mide).
    /// Si el meter está ON pero el worker murió (canal cerrado), incrementa `dropped` (eso SÍ es una
    /// pérdida real). NUNCA panic, NUNCA I/O. (audit AIE P2: no inflar `dropped` con el meter OFF.)
    pub fn emit(&self, ev: RoutingEvent) {
        if let Some(tx) = &self.tx {
            if tx.send(ev).is_err() {
                // Meter ON pero worker caído / canal cerrado → pérdida real (best-effort).
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        // tx = None (meter OFF) → no-op puro, sin contabilizar.
    }

    /// Eventos perdidos con el meter ON (worker caído / canal cerrado). No es un libro contable: el
    /// meter mide ahorro aproximado y puede perder eventos al cerrar la app. NO cuenta los eventos
    /// emitidos con el meter OFF (esos no son "pérdidas", el medidor simplemente está apagado).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Singleton de proceso del meter. Se instala UNA vez al boot (`install_global`) y se lee desde
/// cualquier lado (`global`) — mismo patrón que `cloud_uploader::install_global`. Permite que el
/// poller de done-detection emita trazas SIN threadear el meter por toda la cadena de `process_task`
/// (que correría en el hot-path). El `emit` global es fire-and-forget; si el meter está OFF o no se
/// instaló (default), es un no-op silencioso.
static GLOBAL_METER: std::sync::OnceLock<Arc<SavingsMeter>> = std::sync::OnceLock::new();

/// Instala el meter global (idempotente: la 1ra llamada gana; las siguientes se ignoran). Se llama
/// en `lib.rs` tras construir el meter, SOLO si el kill-switch está ON (con OFF se instala el
/// `disabled()` igual, así `emit_global` es un no-op contable consistente).
pub fn install_global(meter: Arc<SavingsMeter>) {
    let _ = GLOBAL_METER.set(meter);
}

/// Emite una traza vía el meter global. No-op silencioso si el meter no se instaló (default) o está
/// OFF. NUNCA bloquea, NUNCA panic. Pensado para call-sites del hot-path (poller) que no tienen
/// `&AppState` a mano.
pub fn emit_global(ev: RoutingEvent) {
    if let Some(m) = GLOBAL_METER.get() {
        m.emit(ev);
    }
}

/// Worker de fondo: acumula eventos y los inserta en batch cada `FLUSH_INTERVAL` o al llegar a
/// `BATCH_MAX`. Corre hasta que el canal se cierra (todos los `tx` dropeados). Nunca propaga error:
/// un fallo de INSERT se loguea y se descarta el batch (best-effort).
async fn worker_loop(db: Arc<Mutex<Connection>>, mut rx: UnboundedReceiver<RoutingEvent>) {
    let mut batch: Vec<RoutingEvent> = Vec::with_capacity(BATCH_MAX);
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            maybe = rx.recv() => {
                match maybe {
                    Some(ev) => {
                        batch.push(ev);
                        if batch.len() >= BATCH_MAX {
                            flush_batch(&db, &mut batch);
                        }
                    }
                    None => {
                        // Canal cerrado → flush final y salir.
                        flush_batch(&db, &mut batch);
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                flush_batch(&db, &mut batch);
            }
        }
    }
}

/// INSERT batch dentro de una transacción. Vacía `batch` siempre (éxito o fallo). Best-effort: un
/// error de DB se loguea y el batch se descarta (no se reintenta para no acumular memoria sin bound).
fn flush_batch(db: &Arc<Mutex<Connection>>, batch: &mut Vec<RoutingEvent>) {
    if batch.is_empty() {
        return;
    }
    let drained: Vec<RoutingEvent> = std::mem::take(batch);
    let mut conn = db.lock();
    let tx = match conn.transaction() {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!("savings_meter: no se pudo abrir txn ({e}) — batch descartado");
            return;
        }
    };
    for ev in &drained {
        let id = uuid::Uuid::new_v4().to_string();
        let res = tx.execute(
            "INSERT INTO cost_router_events \
             (event_id, decision, model_id, provider, tokens_in, tokens_out, \
              cost_real_usd, cost_baseline_premium_usd, price_table_version, baseline_is_default) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                ev.decision.as_str(),
                ev.model_id,
                ev.provider,
                ev.tokens_in,
                ev.tokens_out,
                ev.cost_real_usd,
                ev.cost_baseline_premium_usd,
                ev.price_table_version,
                ev.baseline_is_default as i64,
            ],
        );
        if let Err(e) = res {
            tracing::debug!("savings_meter: INSERT falló ({e}) — fila descartada");
        }
    }
    if let Err(e) = tx.commit() {
        tracing::debug!("savings_meter: commit falló ({e}) — batch descartado");
    }
}

// ── Baseline de precios (BYOK premium) ──────────────────────────────────────────────────────────

/// Precio de un modelo (USD por millón de tokens) + metadata de versión.
#[derive(Debug, Clone)]
pub struct ModelPrice {
    pub price_in_per_mtok: f64,
    pub price_out_per_mtok: f64,
    pub price_table_version: String,
    pub is_default: bool,
}

/// Resuelve el precio premium a usar para el baseline. Si `premium_model` está configurado y existe
/// en `price_table`, usa ESE; si no, cae al precio default documentado (`is_default = 1`). Devuelve
/// `None` solo si la tabla de precios está vacía (no debería: la migración siembra un default).
pub fn resolve_premium_price(conn: &Connection, premium_model: Option<&str>) -> Option<ModelPrice> {
    // 1. Intento por el modelo premium configurado (última versión de precios).
    if let Some(model) = premium_model {
        if let Some(p) = query_price(conn, Some(model), false) {
            return Some(p);
        }
    }
    // 2. Fallback: el default documentado.
    query_price(conn, None, true)
}

/// Query de un precio. `by_default=true` ⇒ busca `is_default=1`; si no, busca por `model`. Toma la
/// `price_table_version` más reciente (orden lexicográfico de la versión "YYYY-MM" sirve).
fn query_price(conn: &Connection, model: Option<&str>, by_default: bool) -> Option<ModelPrice> {
    let (sql, bind): (&str, Option<String>) = if by_default {
        (
            "SELECT price_in_per_mtok, price_out_per_mtok, price_table_version, is_default \
             FROM price_table WHERE is_default = 1 \
             ORDER BY price_table_version DESC LIMIT 1",
            None,
        )
    } else {
        (
            "SELECT price_in_per_mtok, price_out_per_mtok, price_table_version, is_default \
             FROM price_table WHERE model = ?1 \
             ORDER BY price_table_version DESC LIMIT 1",
            model.map(String::from),
        )
    };
    let row = if let Some(m) = bind {
        conn.query_row(sql, params![m], map_price)
    } else {
        conn.query_row(sql, [], map_price)
    };
    row.ok()
}

fn map_price(r: &rusqlite::Row<'_>) -> rusqlite::Result<ModelPrice> {
    Ok(ModelPrice {
        price_in_per_mtok: r.get(0)?,
        price_out_per_mtok: r.get(1)?,
        price_table_version: r.get(2)?,
        is_default: r.get::<_, i64>(3)? != 0,
    })
}

/// Costo de `tokens_in/out` a un `ModelPrice` (USD).
pub fn cost_at_price(tokens_in: u32, tokens_out: u32, price: &ModelPrice) -> f64 {
    (tokens_in as f64 / 1_000_000.0) * price.price_in_per_mtok
        + (tokens_out as f64 / 1_000_000.0) * price.price_out_per_mtok
}

/// Construye un `RoutingEvent` para una decisión local/free, computando el baseline premium a partir
/// de los tokens. Helper de conveniencia para el cablear en `meta_decision` (cost_real ≈ 0). Si no
/// hay tokens, el baseline queda `None` (la fila no cuenta para el ahorro). NUNCA falla.
pub fn routing_event_for_local_or_free(
    conn: &Connection,
    decision: Decision,
    model_id: Option<String>,
    provider: Option<String>,
    tokens_in: Option<u32>,
    tokens_out: Option<u32>,
    premium_model: Option<&str>,
) -> RoutingEvent {
    let (baseline, version, is_default) = match (tokens_in, tokens_out) {
        (Some(ti), Some(to)) => match resolve_premium_price(conn, premium_model) {
            Some(p) => (
                Some(cost_at_price(ti, to, &p)),
                Some(p.price_table_version),
                p.is_default,
            ),
            None => (None, None, false),
        },
        // Sin tokens completos → no se inventa baseline.
        _ => (None, None, false),
    };
    RoutingEvent {
        decision,
        model_id,
        provider,
        tokens_in,
        tokens_out,
        cost_real_usd: Some(0.0), // local/free: el user no paga inferencia
        cost_baseline_premium_usd: baseline,
        price_table_version: version,
        baseline_is_default: is_default,
    }
}

// ── Summary (lectura read-only para el dashboard) ────────────────────────────────────────────────

/// Estado del meter para gatear la UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterStatus {
    /// Meter apagado (Free tier o kill-switch OFF).
    Off,
    /// < 30 días de datos: aún no se muestra el ahorro acumulado.
    WarmingUp,
    /// ≥ 30 días: el ahorro acumulado es presentable.
    Ready,
}

/// Días que el meter "calienta" antes de mostrar ahorro acumulado (council v6 Fase 3).
pub const WARMUP_DAYS: i64 = 30;

/// Resumen agregado SOLO de filas reales con baseline no-NULL. NUNCA proyecta.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SavingsSummary {
    pub status: MeterStatus,
    pub spent_real_usd: f64,
    pub baseline_premium_usd: f64,
    /// `baseline - real`, clamp a `>= 0` (el caso negativo es señal interna de Fase 2, no copy).
    pub saved_usd: f64,
    /// `saved / baseline * 100`. 0 si no hay baseline.
    pub saved_pct: f64,
    pub events_counted: i64,
    pub events_excluded_no_baseline: i64,
    pub window_days: i64,
    /// Días de datos observados (desde el evento más viejo).
    pub days_observed: i64,
    /// Sólo cuando `status = WarmingUp`: días restantes para los `WARMUP_DAYS`.
    pub eta_days: Option<i64>,
}

/// Computa el summary. `tier_meter_on` = el meter está habilitado para el tier del user (Free ⇒ false).
/// `window_days` = ventana de agregación (default 30). NO proyecta: solo agrega lo medido.
pub fn compute_summary(conn: &Connection, tier_meter_on: bool, window_days: i64) -> SavingsSummary {
    if !tier_meter_on || !SavingsMeter::enabled() {
        return SavingsSummary {
            status: MeterStatus::Off,
            spent_real_usd: 0.0,
            baseline_premium_usd: 0.0,
            saved_usd: 0.0,
            saved_pct: 0.0,
            events_counted: 0,
            events_excluded_no_baseline: 0,
            window_days,
            days_observed: 0,
            eta_days: None,
        };
    }

    // Días observados desde el evento más viejo (en toda la tabla).
    let days_observed: i64 = conn
        .query_row(
            "SELECT CAST(julianday('now') - julianday(MIN(ts)) AS INTEGER) FROM cost_router_events",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
        .unwrap_or(0)
        .max(0);

    // Agregados dentro de la ventana, solo filas con baseline no-NULL.
    let cutoff = format!("-{window_days} days");
    let (spent, baseline, counted): (f64, f64, i64) = conn
        .query_row(
            "SELECT \
               COALESCE(SUM(cost_real_usd), 0), \
               COALESCE(SUM(cost_baseline_premium_usd), 0), \
               COUNT(*) \
             FROM cost_router_events \
             WHERE cost_baseline_premium_usd IS NOT NULL \
               AND ts >= datetime('now', ?1)",
            params![cutoff],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap_or((0.0, 0.0, 0));

    let excluded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cost_router_events \
             WHERE cost_baseline_premium_usd IS NULL AND ts >= datetime('now', ?1)",
            params![cutoff],
            |r| r.get(0),
        )
        .unwrap_or(0);

    let saved = (baseline - spent).max(0.0);
    let saved_pct = if baseline > 0.0 {
        (saved / baseline) * 100.0
    } else {
        0.0
    };

    let (status, eta) = if days_observed >= WARMUP_DAYS {
        (MeterStatus::Ready, None)
    } else {
        // Clamp a >= 0 por las dudas (clock skew / carrera) — nunca eta negativo (audit P3).
        (
            MeterStatus::WarmingUp,
            Some((WARMUP_DAYS - days_observed).max(0)),
        )
    };

    SavingsSummary {
        status,
        spent_real_usd: spent,
        baseline_premium_usd: baseline,
        saved_usd: saved,
        saved_pct,
        events_counted: counted,
        events_excluded_no_baseline: excluded,
        window_days,
        days_observed,
        eta_days: eta,
    }
}

/// Un bucket de la serie temporal (día o semana). Solo lo medido.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SavingsBucket {
    pub bucket_start: String,
    pub spent_real_usd: f64,
    pub saved_usd: f64,
    pub events: i64,
}

/// Serie temporal agregada por día o semana. `bucket = "week"` agrupa por semana ISO; cualquier otro
/// valor agrupa por día. Solo filas con baseline no-NULL cuentan para `saved`.
pub fn compute_series(
    conn: &Connection,
    tier_meter_on: bool,
    bucket: &str,
    window_days: i64,
) -> Vec<SavingsBucket> {
    // Gating simétrico con `compute_summary`: Free/off o kill-switch OFF ⇒ vacío (audit: consistencia).
    if !tier_meter_on || !SavingsMeter::enabled() {
        return Vec::new();
    }
    // SQLite strftime: día = %Y-%m-%d ; semana = %Y-W%W. `bucket` va como parámetro bound (?1), no
    // interpolado → no es vector de inyección; igual lo restringimos a un set conocido (defensa en
    // profundidad + cualquier otro valor cae a "day" determinista).
    let fmt = match bucket {
        "week" => "%Y-W%W",
        _ => "%Y-%m-%d",
    };
    let cutoff = format!("-{window_days} days");
    let mut out = Vec::new();
    let mut stmt = match conn.prepare(
        "SELECT strftime(?1, ts) AS b, \
                COALESCE(SUM(cost_real_usd), 0), \
                COALESCE(SUM(MAX(cost_baseline_premium_usd - cost_real_usd, 0)), 0), \
                COUNT(*) \
         FROM cost_router_events \
         WHERE cost_baseline_premium_usd IS NOT NULL AND ts >= datetime('now', ?2) \
         GROUP BY b ORDER BY b ASC",
    ) {
        Ok(s) => s,
        Err(_) => return out,
    };
    let rows = stmt.query_map(params![fmt, cutoff], |r| {
        Ok(SavingsBucket {
            bucket_start: r.get(0)?,
            spent_real_usd: r.get(1)?,
            saved_usd: r.get(2)?,
            events: r.get(3)?,
        })
    });
    if let Ok(rows) = rows {
        for row in rows.flatten() {
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/053_cost_router_events.sql"))
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn count_rows(db: &Arc<Mutex<Connection>>) -> i64 {
        db.lock()
            .query_row("SELECT COUNT(*) FROM cost_router_events", [], |r| r.get(0))
            .unwrap()
    }

    fn sample_event(d: Decision) -> RoutingEvent {
        RoutingEvent {
            decision: d,
            model_id: Some("m".into()),
            provider: Some("p".into()),
            tokens_in: Some(1000),
            tokens_out: Some(500),
            cost_real_usd: Some(0.0),
            cost_baseline_premium_usd: Some(0.01),
            price_table_version: Some("2026-06".into()),
            baseline_is_default: true,
        }
    }

    // ── emit es no-op PURO cuando el meter está OFF (no infla `dropped`) ────────────
    #[test]
    fn disabled_meter_emit_is_pure_noop() {
        let m = SavingsMeter::disabled();
        m.emit(sample_event(Decision::Local));
        // Meter OFF ⇒ no-op puro: NO cuenta como pérdida (audit AIE P2).
        assert_eq!(m.dropped(), 0, "meter OFF no contabiliza dropped");
    }

    // ── emit con meter ON pero worker caído ⇒ dropped += 1 (pérdida real) ───────────
    #[tokio::test]
    async fn enabled_meter_with_dead_worker_counts_dropped() {
        let db = mem_db();
        let (meter, handle) = SavingsMeter::start(db);
        handle.abort(); // matamos el worker → el canal sigue abierto pero nadie drena
        // El receiver se dropea al abortar la task → el send falla.
        // (pequeña espera para que el abort tome efecto y el rx se dropee)
        tokio::task::yield_now().await;
        let _ = handle.await; // reapea
        meter.emit(sample_event(Decision::Local));
        assert_eq!(meter.dropped(), 1, "meter ON con worker muerto cuenta la pérdida");
    }

    // ── worker inserta lo emitido (flush al cerrar el canal) ────────────────────────
    #[tokio::test]
    async fn worker_inserts_emitted_events() {
        let db = mem_db();
        let (tx, rx) = unbounded_channel();
        let handle = tokio::spawn(worker_loop(db.clone(), rx));
        for _ in 0..3 {
            tx.send(sample_event(Decision::Free)).unwrap();
        }
        drop(tx); // cierra el canal → worker hace flush final y termina
        handle.await.unwrap();
        assert_eq!(count_rows(&db), 3);
    }

    // ── flush_batch directo (sin tokio): 1 fila por evento ──────────────────────────
    #[test]
    fn flush_batch_writes_rows() {
        let db = mem_db();
        let mut batch = vec![
            sample_event(Decision::Local),
            sample_event(Decision::Premium),
        ];
        flush_batch(&db, &mut batch);
        assert!(batch.is_empty(), "el batch se vacía tras el flush");
        assert_eq!(count_rows(&db), 2);
    }

    // ── baseline: default cuando no hay premium configurado ─────────────────────────
    #[test]
    fn baseline_uses_default_when_no_premium() {
        let db = mem_db();
        let conn = db.lock();
        let p = resolve_premium_price(&conn, None).expect("la migración siembra un default");
        assert!(p.is_default, "sin premium configurado → precio default");
        // 1000 in + 500 out @ default (3/15 por Mtok) = 0.003 + 0.0075 = 0.0105
        let cost = cost_at_price(1000, 500, &p);
        assert!((cost - 0.0105).abs() < 1e-9, "cost={cost}");
    }

    // ── baseline: usa el modelo premium configurado si existe en price_table ────────
    #[test]
    fn baseline_uses_configured_premium_model() {
        let db = mem_db();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO price_table (provider, model, price_in_per_mtok, price_out_per_mtok, price_table_version, is_default) \
             VALUES ('openai','gpt-5', 5.0, 20.0, '2026-06', 0)",
            [],
        )
        .unwrap();
        let p = resolve_premium_price(&conn, Some("gpt-5")).unwrap();
        assert!(!p.is_default);
        assert!((p.price_in_per_mtok - 5.0).abs() < 1e-9);
    }

    // ── routing_event helper: sin tokens → baseline None (no se inventa costo) ───────
    #[test]
    fn routing_event_no_tokens_has_no_baseline() {
        let db = mem_db();
        let conn = db.lock();
        let ev = routing_event_for_local_or_free(
            &conn,
            Decision::Local,
            Some("ollama".into()),
            Some("ollama".into()),
            None,
            None,
            None,
        );
        assert!(ev.cost_baseline_premium_usd.is_none());
    }

    // ── summary: off cuando el tier no tiene meter ──────────────────────────────────
    #[test]
    fn summary_off_when_tier_meter_off() {
        let db = mem_db();
        let conn = db.lock();
        let s = compute_summary(&conn, false, 30);
        assert!(matches!(s.status, MeterStatus::Off));
        assert_eq!(s.saved_usd, 0.0);
    }

    // ── summary: warming_up con datos recientes; excluye filas sin baseline ─────────
    #[test]
    fn summary_warming_up_and_excludes_no_baseline() {
        // Forzar enabled() para el path "on".
        std::env::set_var(KILL_SWITCH_ENV, "1");
        let db = mem_db();
        {
            let conn = db.lock();
            // Una fila con baseline (cuenta) y una sin baseline (excluida).
            conn.execute(
                "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd) \
                 VALUES ('a','local', 0.0, 0.05)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd) \
                 VALUES ('b','free', 0.0, NULL)",
                [],
            )
            .unwrap();
            let s = compute_summary(&conn, true, 30);
            assert!(matches!(s.status, MeterStatus::WarmingUp));
            assert_eq!(s.events_counted, 1);
            assert_eq!(s.events_excluded_no_baseline, 1);
            assert!((s.saved_usd - 0.05).abs() < 1e-9);
            assert_eq!(s.eta_days, Some(WARMUP_DAYS)); // 0 días observados
        }
        std::env::remove_var(KILL_SWITCH_ENV);
    }

    // ── saved_usd clamp a >= 0 (reroute más caro no muestra ahorro negativo) ────────
    #[test]
    fn summary_clamps_negative_savings_to_zero() {
        std::env::set_var(KILL_SWITCH_ENV, "1");
        let db = mem_db();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd) \
                 VALUES ('a','premium', 0.10, 0.05)",
                [],
            )
            .unwrap();
            let s = compute_summary(&conn, true, 30);
            assert_eq!(s.saved_usd, 0.0, "ahorro negativo se clampa a 0");
        }
        std::env::remove_var(KILL_SWITCH_ENV);
    }

    // ── APPEND-ONLY (test CI): UPDATE y DELETE son rechazados por trigger ────────────
    #[test]
    fn append_only_rejects_update_and_delete() {
        let db = mem_db();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO cost_router_events (event_id, decision) VALUES ('x','local')",
            [],
        )
        .unwrap();
        let upd = conn.execute(
            "UPDATE cost_router_events SET decision = 'premium' WHERE event_id = 'x'",
            [],
        );
        assert!(upd.is_err(), "UPDATE debe ser rechazado (append-only)");
        assert!(upd.unwrap_err().to_string().contains("append-only"));
        let del = conn.execute("DELETE FROM cost_router_events WHERE event_id = 'x'", []);
        assert!(del.is_err(), "DELETE debe ser rechazado (append-only)");
        assert!(del.unwrap_err().to_string().contains("append-only"));
        // La fila sigue ahí.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM cost_router_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    // ── series: agrupa por día, solo filas con baseline ─────────────────────────────
    #[test]
    fn series_groups_by_day() {
        std::env::set_var(KILL_SWITCH_ENV, "1");
        let db = mem_db();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd, ts) \
             VALUES ('a','local', 0.0, 0.02, '2026-05-01T10:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd, ts) \
             VALUES ('b','local', 0.0, 0.03, '2026-05-01T12:00:00Z')",
            [],
        )
        .unwrap();
        // Ventana amplia para que la fecha sembrada entre.
        let series = compute_series(&conn, true, "day", 100_000);
        assert_eq!(series.len(), 1, "ambas filas del 2026-05-01 → 1 bucket");
        assert_eq!(series[0].events, 2);
        assert!((series[0].saved_usd - 0.05).abs() < 1e-9);
        // Gating simétrico: tier off ⇒ vacío.
        assert!(compute_series(&conn, false, "day", 100_000).is_empty());
        std::env::remove_var(KILL_SWITCH_ENV);
    }
}
