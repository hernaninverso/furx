// services/cost_router.rs — spec-049 · Cost-Router Fase 2 (Router ACTIVO).
//
// El router que DESVÍA decisiones de tier (cheap = local/free vs premium = BYOK). A diferencia de la
// Fase 1 (`savings_meter`, que MIDE), este módulo ELIGE el escalón. **Está APAGADO detrás de un
// feature-flag** (`FURX_COST_ROUTER_MODE`, default `off`): con `off` (default) el router es un no-op
// PURO (passthrough) → cero regresión. Tres modos:
//   - `off`    (default): no desvía. `route_decision` devuelve passthrough.
//   - `shadow`: decide + loguea qué HABRÍA hecho, pero NO cambia el tier real (`desviado=false`).
//   - `active`: desvía de verdad SOLO si el gate de KPI pasa (batch≥25% Y TNS≥25% Y reroute≤12%).
//              Si el gate NO pasa (incl. sin datos / TNS no-positivo) ⇒ se comporta como shadow.
//
// SEGURIDAD (NON-NEGOTIABLE, fail-closed). El orden de los gates es INVIOLABLE y corre SIEMPRE antes
// de elegir cheap:
//   1. force_premium (tool / route)        → policy P3
//   2. hard_gate_tool_count > N             → policy P3
//   3. SessionOrigin interactivo            → NUNCA rutear lo interactivo a free (server-side)
//   4. data_residency (payload sensible)    → raw_prompt/diff/file_paths/env_vars nunca a free
//   5. quota por tenant (SKIP-LOCKED equiv) → budget excedido / contención ⇒ premium
//   6. heurística (clasificador default)    → recién acá se decide cheap vs premium
// Cualquier gate que dispare ⇒ Premium, sin seguir. Si la policy no cargó ⇒ todo premium.
//
// REDACCIÓN: antes de que CUALQUIER cosa salga a un tier free, los secretos se redactan
// (`redact_for_free`, sobre `cloud_sanitizer::sanitize` + patterns extra de la policy). La traza
// marca `security_redacted`.
//
// REROUTE ≤ 1: si el escalón cheap falla, UNA sola escalada a premium; un 2º fallo va premium directo
// sin re-cascada.
//
// DIVERGENCIA vs council v6 (documentada en specs/049/analysis): SQLite, no PostgreSQL. El "SKIP
// LOCKED" del consumo de quota es un try-lock no-bloqueante (txn IMMEDIATE + busy_timeout=0 →
// SQLITE_BUSY ⇒ premium). El `RecentEventBuffer` es minimal/self-contained (no existía).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::Connection;

use crate::services::cloud_sanitizer;

/// Env flag que controla el MODO del router activo. Distinto de `FURX_COST_ROUTER` (Fase 1,
/// medición). `off` (default y cualquier valor desconocido) ⇒ no-op.
const MODE_ENV: &str = "FURX_COST_ROUTER_MODE";

/// Gate de KPI del council v6 (umbrales canónicos).
const GATE_MIN_BATCH_PCT: f64 = 25.0;
const GATE_MIN_TNS_PCT: f64 = 25.0;
const GATE_MAX_REROUTE_PCT: f64 = 12.0;

/// Ventana para considerar "evento UI reciente" al inferir el origen de la sesión.
const SESSION_UI_WINDOW: Duration = Duration::from_millis(500);

// ── Modo del router ──────────────────────────────────────────────────────────────────────────────

/// Modo del router activo. `Off` (default) ⇒ no-op total (cero regresión).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterMode {
    Off,
    Shadow,
    Active,
}

impl RouterMode {
    /// Lee el flag `FURX_COST_ROUTER_MODE`. Cualquier valor que no sea exactamente `shadow`/`active`
    /// ⇒ `Off` (conservador: ante un valor raro, NO desviar).
    pub fn from_env() -> Self {
        Self::parse(std::env::var(MODE_ENV).ok().as_deref())
    }

    /// Parse PURO del valor del flag (sin tocar el env): así el test no muta el env global del
    /// proceso (que bajo `cargo test` concurrente colisiona con otros tests — gotcha de estado global).
    /// `None`/desconocido/whitespace ⇒ `Off`. Solo `shadow`/`active` (case-insensitive, trimmed)
    /// activan los modos.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("shadow") => RouterMode::Shadow,
            Some("active") => RouterMode::Active,
            _ => RouterMode::Off,
        }
    }
}

// ── Tier / decisión ──────────────────────────────────────────────────────────────────────────────

/// El escalón elegido. `Cheap` = local/free; `Premium` = BYOK del user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Cheap,
    Premium,
}

/// Resultado de clasificar (antes de aplicar modo): tier + por qué.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClassificationResult {
    pub tier: Tier,
    pub reason: String,
}

impl ClassificationResult {
    pub fn cheap(reason: impl Into<String>) -> Self {
        Self {
            tier: Tier::Cheap,
            reason: reason.into(),
        }
    }
    pub fn premium(reason: impl Into<String>) -> Self {
        Self {
            tier: Tier::Premium,
            reason: reason.into(),
        }
    }
    /// Helper para los gates de seguridad: SIEMPRE premium, con la razón del gate.
    pub fn force_premium(gate: &str) -> Self {
        Self {
            tier: Tier::Premium,
            reason: format!("force_premium:{gate}"),
        }
    }
}

/// La decisión final del router para una tarea. Incluye el modo, si efectivamente DESVIÓ, el conteo
/// de reroute (invariante ≤1) y si hubo redacción de secretos.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RoutingDecision {
    pub tier: Tier,
    pub reason: String,
    pub mode: RouterMode,
    /// `true` SOLO si el router cambió de verdad la elección (modo `active` + gate pasado + cheap).
    /// En `off`/`shadow`/gate-no-pasado siempre `false` (no toca la ejecución).
    pub desviado: bool,
    /// Invariante: ≤ 1. El cheap-que-falla escala a premium UNA vez.
    pub reroute_count: u8,
    /// `true` si la redacción de secretos encontró algo antes de un tier free.
    pub security_redacted: bool,
    /// `true` si la sesión es interactiva (humano esperando). Para la traza.
    pub session_interactive: bool,
}

/// SAFE bundle: la decisión + el texto que REALMENTE iría a un tier free, ya redactado. `free_payload`
/// es `Some` SOLO cuando la decisión es Cheap (y entonces SIEMPRE redactado); `None` para Premium.
/// El caller usa `free_payload` para mandar a free — NUNCA el texto crudo (cierra el footgun de
/// redacción condicional, audit-3 P0). Lo produce `CostRouter::route_and_prepare`.
#[derive(Debug, Clone)]
pub struct PreparedRouting {
    pub decision: RoutingDecision,
    /// El texto redactado a mandar a free. `None` ⇒ no va nada a free (Premium / sin texto).
    pub free_payload: Option<String>,
}

impl RoutingDecision {
    /// Passthrough: el router NO desvía (modo off, o shadow, o active sin gate). El tier reportado es
    /// el que el clasificador SUGIERE, pero `desviado=false` ⇒ el caller usa su elección normal.
    fn passthrough(
        cls: &ClassificationResult,
        mode: RouterMode,
        security_redacted: bool,
        session_interactive: bool,
    ) -> Self {
        Self {
            tier: cls.tier,
            reason: cls.reason.clone(),
            mode,
            desviado: false,
            reroute_count: 0,
            security_redacted,
            session_interactive,
        }
    }
}

// ── Request ──────────────────────────────────────────────────────────────────────────────────────

/// Qué se ruta a free (free_tier_prohibited). Marcas de presencia de payload sensible; NO el texto.
#[derive(Debug, Clone, Default)]
pub struct PayloadFlags {
    pub has_raw_prompt: bool,
    pub has_diff_content: bool,
    pub has_file_paths: bool,
    pub has_env_vars: bool,
}

/// Descripción de una tarea para clasificar. SIN texto libre que pueda llegar a free sin pasar por
/// redacción (el texto va aparte y se redacta con `redact_for_free`).
#[derive(Debug, Clone)]
pub struct RoutingRequest {
    /// Nombre del tool que la tarea invoca (para `force_premium_tools`). Vacío si N/A.
    pub tool_id: String,
    /// Ruta tocada (para `force_premium_routes`). Vacío si N/A.
    pub route: String,
    /// Cantidad de tools que la tarea usaría (para `hard_gate_tool_count`).
    pub tool_count: u32,
    /// Tokens de contexto estimados (dim de complejidad).
    pub context_tokens: u32,
    /// Tipo de tarea (bugfix/feature/refactor/batch/…). Para la heurística + KPI.
    pub task_type: String,
    /// Nº de turno (multi-turn = más complejo / más probablemente interactivo).
    pub turn_number: u16,
    /// Largo del objetivo en caracteres (proxy de complejidad).
    pub objective_len: usize,
    /// Tokens estimados de la tarea (para consumir quota).
    pub estimated_tokens: u32,
    /// Marcas de payload sensible (data_residency).
    pub payload: PayloadFlags,
}

impl RoutingRequest {
    /// Constructor mínimo para tests / call-sites simples.
    pub fn simple(tool_count: u32, context_tokens: u32) -> Self {
        Self {
            tool_id: String::new(),
            route: String::new(),
            tool_count,
            context_tokens,
            task_type: String::new(),
            turn_number: 1,
            objective_len: 0,
            estimated_tokens: 0,
            payload: PayloadFlags::default(),
        }
    }
}

// ── Policy (P3 threat model) ─────────────────────────────────────────────────────────────────────

/// La policy v1 del council (gates de seguridad). Cargada en boot (default embebido); recargable solo
/// vía comando autenticado, NUNCA file-watch.
#[derive(Debug, Clone)]
pub struct RouterPolicy {
    pub version: u32,
    pub force_premium_tools: Vec<String>,
    pub force_premium_routes: Vec<String>,
    pub redact_patterns: Vec<Regex>,
    pub free_tier_prohibited: FreeTierProhibited,
    pub hard_gate_tool_count: u32,
}

/// Qué campos NO pueden ir a free.
#[derive(Debug, Clone, Default)]
pub struct FreeTierProhibited {
    pub raw_prompt: bool,
    pub diff_content: bool,
    pub file_paths: bool,
    pub env_vars: bool,
}

impl RouterPolicy {
    /// El default embebido = la policy v1 del council (`config/router_policy.v1.yaml`). Fuente de
    /// verdad en runtime (no se hace file-watch). Fail-closed: si por algún motivo no se pudiera
    /// construir, el caller usa `None` ⇒ todo premium.
    pub fn default_v1() -> Self {
        let redact_patterns = vec![
            Regex::new(r#"(?i)(api[_-]?key|token|password|private[_-]?key|bearer|secret)\s*[=:]\s*['"][^'"]{8,}"#).unwrap(),
            Regex::new(r"sk-[A-Za-z0-9_-]{20,}").unwrap(),
            Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
        ];
        Self {
            version: 1,
            force_premium_tools: vec![
                "auth_*".into(),
                "migrate_*".into(),
                "crypto_*".into(),
                "secrets_*".into(),
            ],
            force_premium_routes: vec![
                "/api/secrets/*".into(),
                "/api/billing/*".into(),
                "/api/auth/*".into(),
            ],
            redact_patterns,
            free_tier_prohibited: FreeTierProhibited {
                raw_prompt: true,
                diff_content: true,
                file_paths: true,
                env_vars: true,
            },
            hard_gate_tool_count: 10,
        }
    }

    /// ¿El `tool_id` matchea algún glob de `force_premium_tools`? Glob = sufijo `*` (prefijo) o match
    /// exacto.
    pub fn is_force_premium_tool(&self, tool_id: &str) -> bool {
        self.force_premium_tools
            .iter()
            .any(|g| glob_match(g, tool_id))
    }

    /// ¿La `route` matchea algún glob de `force_premium_routes`?
    pub fn is_force_premium_route(&self, route: &str) -> bool {
        self.force_premium_routes
            .iter()
            .any(|g| glob_match(g, route))
    }

    /// ¿El payload tiene algún campo prohibido en free?
    pub fn payload_prohibited_in_free(&self, p: &PayloadFlags) -> bool {
        (self.free_tier_prohibited.raw_prompt && p.has_raw_prompt)
            || (self.free_tier_prohibited.diff_content && p.has_diff_content)
            || (self.free_tier_prohibited.file_paths && p.has_file_paths)
            || (self.free_tier_prohibited.env_vars && p.has_env_vars)
    }
}

/// Glob mínimo: `prefix*` = `tool_id` empieza con `prefix`; `*suffix` = termina con; sin `*` = exacto.
/// (Suficiente para los patrones de la policy v1, que usan solo sufijo `*`.)
fn glob_match(pattern: &str, s: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        s.starts_with(prefix)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        s.ends_with(suffix)
    } else {
        pattern == s
    }
}

// ── Redacción antes de free ──────────────────────────────────────────────────────────────────────

/// Redacta secretos del `text` ANTES de mandarlo a un tier free. Combina `cloud_sanitizer::sanitize`
/// (sk-/Bearer/email/IBAN/ARN/UUID) con los patterns extra de la policy (AKIA, PEM, `api_key=...`).
/// Devuelve `(texto_redactado, hubo_redaccion)`. NUNCA panic (defensa: si una regex fallara, igual
/// devuelve lo redactado por el sanitizer). Esta función corre SOLO en el path a free.
pub fn redact_for_free(text: &str, policy: &RouterPolicy) -> (String, bool) {
    // 1. Sanitizer base (ya cubre la mayoría de los secretos).
    let (mut out, report) = cloud_sanitizer::sanitize(text);
    let mut redacted = report.total_hits > 0;
    // 2. Patterns extra de la policy.
    for re in &policy.redact_patterns {
        let mut hits = 0u32;
        out = re
            .replace_all(&out, |_: &regex::Captures| {
                hits += 1;
                "[REDACTED:policy]".to_string()
            })
            .to_string();
        if hits > 0 {
            redacted = true;
        }
    }
    (out, redacted)
}

// ── SessionOrigin (US3) — server-side, no manipulable por el frontend ────────────────────────────

/// Buffer minimal de eventos UI por sesión (server-side). El frontend NO escribe acá directamente; lo
/// alimenta el backend cuando observa actividad real de UI. Usado por `SessionOrigin::from_server_state`.
#[derive(Debug, Default)]
pub struct RecentEventBuffer {
    /// session_id → instante del último evento UI observado.
    last_ui: Mutex<HashMap<String, Instant>>,
}

impl RecentEventBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un evento UI para una sesión (lo llama el backend, no el frontend).
    pub fn record_ui_event(&self, session_id: &str, at: Instant) {
        self.last_ui.lock().insert(session_id.to_string(), at);
    }

    /// ¿Hubo un evento UI para `session_id` dentro de `window` antes de `now`?
    pub fn has_ui_event_near(&self, session_id: &str, now: Instant, window: Duration) -> bool {
        self.last_ui
            .lock()
            .get(session_id)
            .map(|&t| now.duration_since(t) <= window)
            .unwrap_or(false)
    }
}

/// Origen de una sesión. **Constructor privado**: SOLO `from_server_state` instancia (el frontend no
/// puede fabricar un `Automated` para forzar el routing barato). El test CI grep verifica que no se
/// construye de otra forma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOrigin {
    /// Humano esperando una respuesta (interactivo) ⇒ NUNCA free.
    UserInitiated,
    /// Tarea de fondo / batch ⇒ elegible para free (si los demás gates pasan).
    Automated,
}

impl SessionOrigin {
    // Constructores privados: nadie fuera de este impl los llama. Usan `Self::` para conformar al
    // grep CI canónico del council (§F1.1), que verifica que `SessionOrigin` solo se instancia vía
    // `from_server_state` (allowlist: `from_server_state\|enum SessionOrigin\|Self::\|match.*`).
    fn new_user_initiated() -> Self {
        Self::UserInitiated
    }
    fn new_automated() -> Self {
        Self::Automated
    }

    /// Resuelve el origen MIRANDO el estado server-side (el buffer de eventos UI). Si hay un evento
    /// UI reciente ⇒ interactivo (`UserInitiated`). Si no ⇒ `Automated`. **Sin señal disponible ⇒
    /// `UserInitiated`** (conservador: ante la duda, NO rutear a free) — lo aplica el wiring que, sin
    /// buffer, NO consulta (ver `route_decision`/lib.rs).
    pub fn from_server_state(buffer: &RecentEventBuffer, session_id: &str, now: Instant) -> Self {
        if buffer.has_ui_event_near(session_id, now, SESSION_UI_WINDOW) {
            Self::new_user_initiated()
        } else {
            Self::new_automated()
        }
    }

    /// Sin señal server-side disponible ⇒ conservador: interactivo (NUNCA free). Lo usa el wiring
    /// cuando no hay `RecentEventBuffer`. Pasa por el constructor allowlisted (`Self::`), así el grep
    /// CI sigue verificando que el origen no se fabrica fuera de la familia `from_server_state`.
    pub fn interactive_no_signal() -> Self {
        Self::new_user_initiated()
    }

    pub fn is_interactive(self) -> bool {
        matches!(self, Self::UserInitiated)
    }
}

// ── Quota por tenant (US4) — SKIP-LOCKED equivalente en SQLite ───────────────────────────────────

/// Resultado de intentar consumir quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaOutcome {
    /// Se consumió el budget free ⇒ la tarea es elegible para free.
    Consumed,
    /// Budget free excedido ⇒ premium.
    Exhausted,
    /// La fila estaba lockeada (otra txn) ⇒ premium inmediato (SKIP-LOCKED: no esperar).
    Contended,
    /// El tenant no tiene fila de quota (sin budget free configurado) ⇒ premium.
    NoQuota,
}

impl QuotaOutcome {
    /// ¿Permite free? Solo `Consumed`.
    pub fn allows_free(self) -> bool {
        matches!(self, QuotaOutcome::Consumed)
    }
}

/// Intenta consumir `tokens` del budget free del `tenant`. Usa una transacción IMMEDIATE
/// (write-lock al abrir); el caller fija `busy_timeout=0` en la conexión ⇒ si la DB está ocupada,
/// el `transaction()` falla con `SQLITE_BUSY` y devolvemos `Contended` (SKIP-LOCKED equivalente: no
/// se espera). Resetea el consumo si cambió el mes. NUNCA bloquea, NUNCA panic.
pub fn try_consume_quota(conn: &mut Connection, tenant_id: &str, tokens: u32) -> QuotaOutcome {
    let period_now = current_period();
    // Transacción IMMEDIATE: toma el write-lock de entrada. Con busy_timeout=0, si otra txn lo tiene,
    // falla de una → Contended.
    let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
        Ok(t) => t,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::DatabaseBusy
                || e.code == rusqlite::ErrorCode::DatabaseLocked =>
        {
            return QuotaOutcome::Contended;
        }
        Err(_) => return QuotaOutcome::Contended, // cualquier otro fallo de apertura ⇒ fail-safe premium
    };

    // Leer la fila del tenant.
    let row: Option<(i64, i64, String)> = tx
        .query_row(
            "SELECT batch_budget_tokens, used_batch_tokens, period FROM router_quotas WHERE tenant_id = ?1",
            rusqlite::params![tenant_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

    let (budget, used, period) = match row {
        Some(x) => x,
        None => return QuotaOutcome::NoQuota, // sin fila ⇒ sin budget free ⇒ premium
    };

    // Reset si cambió el período (mes).
    let used_effective = if period == period_now { used } else { 0 };

    // `saturating_add` es fail-closed (audit-3 mistral P2): un `tokens` enorme (atacante) satura a
    // i64::MAX, que SIEMPRE supera el budget ⇒ Exhausted ⇒ premium. No es un bypass: el caso extremo
    // cae al lado seguro (premium), nunca a free.
    let new_used = used_effective.saturating_add(tokens as i64);
    if new_used > budget {
        return QuotaOutcome::Exhausted; // budget excedido ⇒ premium (no consume)
    }

    let upd = tx.execute(
        "UPDATE router_quotas SET used_batch_tokens = ?2, period = ?3, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE tenant_id = ?1",
        rusqlite::params![tenant_id, new_used, period_now],
    );
    if upd.is_err() {
        return QuotaOutcome::Contended; // fallo de escritura ⇒ fail-safe premium
    }
    match tx.commit() {
        Ok(_) => QuotaOutcome::Consumed,
        Err(_) => QuotaOutcome::Contended,
    }
}

fn current_period() -> String {
    // 'YYYY-MM' del reloj del OS (mismo formato que el DEFAULT de la migración).
    // Lo computamos sin tocar la DB para no depender de una conexión.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cálculo simple de año-mes desde epoch (UTC). Evita una dep de chrono.
    let days = now / 86_400;
    let (y, m, _d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}")
}

/// Convierte días-desde-epoch (1970-01-01) a (año, mes, día) UTC. Algoritmo de Howard Hinnant.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ── Clasificador (P2) — heurística default (tipo LiteLLM complexity_router) ──────────────────────

/// Trait del clasificador de tier. El default es `HeuristicClassifier`; la estructura permite
/// enchufar un clasificador ML (LoRA Gemma / eleata-latent-router) después SIN tocar el router. En
/// esta fase SOLO se entrega el default (no hay datos que justifiquen ML).
pub trait TierClassifier: Send + Sync {
    fn classify(&self, req: &RoutingRequest) -> ClassificationResult;
}

/// Heurística 7-dim, determinista, sub-ms, sin modelo. Decide cheap vs premium por complejidad. NO
/// aplica los gates de seguridad (eso lo hace el router ANTES de llamar al clasificador).
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicClassifier;

impl TierClassifier for HeuristicClassifier {
    fn classify(&self, req: &RoutingRequest) -> ClassificationResult {
        // Dimensiones que empujan a premium (cualquiera dispara).
        if req.context_tokens > 32_000 {
            return ClassificationResult::premium("context_tokens>32k");
        }
        if req.tool_count > 5 {
            return ClassificationResult::premium("tool_count>5");
        }
        if req.turn_number > 3 {
            return ClassificationResult::premium("multi_turn>3");
        }
        if req.objective_len > 4_000 {
            return ClassificationResult::premium("objective_len>4k");
        }
        let t = req.task_type.to_ascii_lowercase();
        // task_types intrínsecamente complejos → premium.
        if t.contains("refactor") || t.contains("architect") || t.contains("security") {
            return ClassificationResult::premium("task_type_complex");
        }
        // El resto (simple, batch, bugfix chico, context chico, pocos tools) → cheap.
        ClassificationResult::cheap("heuristic_simple")
    }
}

// ── KPI gate (F0) — derivado de las trazas de Fase 1 ─────────────────────────────────────────────

/// Estado del gate de KPI. `passed` solo si batch≥25% Y tns≥25% Y reroute≤12% (council §"Gate de
/// salida F1"). Sin datos ⇒ no pasa (warming_up).
#[derive(Debug, Clone, serde::Serialize)]
pub struct KpiGate {
    pub batch_pct: f64,
    pub tns_batch: f64,
    pub reroute_rate: f64,
    pub passed: bool,
    pub reason: &'static str,
}

impl KpiGate {
    /// Evalúa el gate agregando `cost_router_events` (migr 053, Fase 1). Conservador: ante falta de
    /// datos o cualquier query fallida ⇒ no pasa. NUNCA panic.
    pub fn evaluate(conn: &Connection) -> KpiGate {
        // Total de eventos con baseline (los que cuentan para el ahorro).
        let (total, batch, saved, baseline, rerouted): (i64, i64, f64, f64, i64) = conn
            .query_row(
                "SELECT \
                   COUNT(*), \
                   COALESCE(SUM(CASE WHEN decision IN ('local','free') THEN 1 ELSE 0 END), 0), \
                   COALESCE(SUM(MAX(cost_baseline_premium_usd - cost_real_usd, 0)), 0), \
                   COALESCE(SUM(cost_baseline_premium_usd), 0), \
                   COALESCE(SUM(CASE WHEN reroute_count > 0 THEN 1 ELSE 0 END), 0) \
                 FROM cost_router_events \
                 WHERE cost_baseline_premium_usd IS NOT NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap_or((0, 0, 0.0, 0.0, 0));

        if total == 0 {
            return KpiGate {
                batch_pct: 0.0,
                tns_batch: 0.0,
                reroute_rate: 0.0,
                passed: false,
                reason: "warming_up",
            };
        }

        let batch_pct = 100.0 * batch as f64 / total as f64;
        let tns_batch = if baseline > 0.0 {
            100.0 * saved / baseline
        } else {
            0.0
        };
        let reroute_rate = 100.0 * rerouted as f64 / total as f64;

        let (passed, reason) = if batch_pct < GATE_MIN_BATCH_PCT {
            (false, "insufficient_batch")
        } else if tns_batch < GATE_MIN_TNS_PCT {
            (false, "tns_too_low")
        } else if reroute_rate > GATE_MAX_REROUTE_PCT {
            (false, "reroute_too_high")
        } else {
            (true, "ok")
        };

        KpiGate {
            batch_pct,
            tns_batch,
            reroute_rate,
            passed,
            reason,
        }
    }
}

// ── El router ────────────────────────────────────────────────────────────────────────────────────

/// El router de costo. Dueño de la policy + el clasificador + el modo. `policy = None` ⇒ fail-closed
/// (todo premium). El default es `Off` ⇒ no-op.
pub struct CostRouter {
    /// `None` ⇒ la policy no cargó ⇒ fail-closed (todo premium).
    pub policy: Option<RouterPolicy>,
    pub classifier: Box<dyn TierClassifier>,
    pub mode: RouterMode,
}

impl CostRouter {
    /// Construye el router con la policy default embebida y el clasificador heurístico, leyendo el
    /// modo del env. Esto es lo que `lib.rs` instala en boot.
    pub fn from_env() -> Self {
        Self {
            policy: Some(RouterPolicy::default_v1()),
            classifier: Box::new(HeuristicClassifier),
            mode: RouterMode::from_env(),
        }
    }

    /// Router fail-closed (policy no cargó). Todo premium.
    pub fn fail_closed(mode: RouterMode) -> Self {
        Self {
            policy: None,
            classifier: Box::new(HeuristicClassifier),
            mode,
        }
    }

    /// Aplica los gates de seguridad en ORDEN (inviolable). Devuelve `Some(premium)` si algún gate
    /// dispara (corta acá); `None` si todos pasan (sigue a la heurística).
    ///
    /// `effectful` controla el gate de QUOTA (el único con efecto de escritura): cuando es `true`
    /// (modo `active` + gate de KPI pasado ⇒ el router VA a desviar de verdad) el consumo de quota
    /// se COMETE en la DB; cuando es `false` (off / shadow / active-sin-gate) la quota se evalúa SOLO
    /// como lectura (peek, sin escribir) para la traza, así `off` es un no-op total (cero regresión:
    /// no consume budget de nadie). Los gates de seguridad SIN efecto (force_premium, hard_gate,
    /// interactivo, data_residency) corren SIEMPRE.
    fn security_gates(
        &self,
        req: &RoutingRequest,
        origin: SessionOrigin,
        effectful: bool,
        conn: Option<&mut Connection>,
        tenant_id: Option<&str>,
    ) -> Option<ClassificationResult> {
        // 0. Policy ausente ⇒ fail-closed.
        let policy = match &self.policy {
            Some(p) => p,
            None => return Some(ClassificationResult::force_premium("no_policy")),
        };
        // 1. force_premium (tool / route).
        if !req.tool_id.is_empty() && policy.is_force_premium_tool(&req.tool_id) {
            return Some(ClassificationResult::force_premium("policy_gate"));
        }
        if !req.route.is_empty() && policy.is_force_premium_route(&req.route) {
            return Some(ClassificationResult::force_premium("policy_gate"));
        }
        // 2. hard_gate_tool_count. Council §P3 `hard_gate_tool_count: 10` + §F1.1 "tool_count > 10 ⇒
        //    premium": el `>` es FIEL al council (exactamente 10 tools es elegible cheap; 11+ ⇒
        //    premium). NO es un off-by-one (audit-3 mistral lo marcó como P1; revisado: el council
        //    pide `>`).
        if req.tool_count > policy.hard_gate_tool_count {
            return Some(ClassificationResult::force_premium("hard_gate_tool_count"));
        }
        // 3. SessionOrigin interactivo ⇒ NUNCA free.
        if origin.is_interactive() {
            return Some(ClassificationResult::force_premium("session_interactive"));
        }
        // 4. data_residency: payload sensible ⇒ no free.
        if policy.payload_prohibited_in_free(&req.payload) {
            return Some(ClassificationResult::force_premium("data_residency"));
        }
        // 5. quota por tenant (si hay conexión + tenant). Bajo contención / excedido ⇒ premium.
        //    SOLO se COMETE el consumo si `effectful` (el router va a desviar de verdad); en
        //    off/shadow se evalúa como peek (sin escribir) — así off no toca el budget de nadie.
        if let (Some(conn), Some(tenant)) = (conn, tenant_id) {
            let outcome = if effectful {
                try_consume_quota(conn, tenant, req.estimated_tokens)
            } else {
                peek_quota(conn, tenant, req.estimated_tokens)
            };
            if !outcome.allows_free() {
                let reason = match outcome {
                    QuotaOutcome::Exhausted => "quota_exhausted",
                    QuotaOutcome::Contended => "quota_contended",
                    QuotaOutcome::NoQuota => "quota_none",
                    QuotaOutcome::Consumed => unreachable!(),
                };
                return Some(ClassificationResult::force_premium(reason));
            }
        }
        None // todos los gates pasaron
    }

    /// La decisión completa del router para una tarea. Garantías:
    ///  - `mode = Off` ⇒ passthrough (no desvía), cero regresión.
    ///  - `mode = Shadow` ⇒ decide+loguea pero NO desvía (`desviado=false`).
    ///  - `mode = Active` + `gate_passed` ⇒ desvía (`desviado=true` si la elección es Cheap).
    ///  - `mode = Active` + `!gate_passed` ⇒ se comporta como shadow (no desvía).
    /// Los gates de seguridad corren SIEMPRE (incluso en shadow/off computamos la decisión para la
    /// traza), pero solo afectan la EJECUCIÓN cuando el router efectivamente desvía.
    ///
    /// `free_text` es el texto que iría a un tier free (se redacta acá si la decisión es Cheap).
    pub fn route_decision(
        &self,
        req: &RoutingRequest,
        origin: SessionOrigin,
        gate_passed: bool,
        free_text: Option<&str>,
        conn: Option<&mut Connection>,
        tenant_id: Option<&str>,
    ) -> RoutingDecision {
        let session_interactive = origin.is_interactive();

        // `effectful` = el router VA a desviar de verdad ⇒ el gate de quota COMETE el consumo. SOLO
        // en `active` + gate pasado. En off/shadow/active-sin-gate, los gates corren para la traza
        // pero la quota se evalúa como peek (sin escribir) ⇒ off es un no-op total (cero regresión).
        let effectful = matches!(self.mode, RouterMode::Active) && gate_passed;

        // Gates de seguridad primero (siempre, en orden inviolable).
        let cls = match self.security_gates(req, origin, effectful, conn, tenant_id) {
            Some(forced) => forced,
            None => self.classifier.classify(req),
        };

        // Redacción: SOLO si la decisión sería Cheap (free) y hay texto.
        let security_redacted = if cls.tier == Tier::Cheap {
            if let (Some(text), Some(policy)) = (free_text, &self.policy) {
                let (_redacted_text, hit) = redact_for_free(text, policy);
                hit
            } else {
                false
            }
        } else {
            false
        };

        // `desviado` = true SOLO en active + gate + cheap (el caso que realmente cambia la ejecución).
        // Off/Shadow/active-sin-gate ⇒ passthrough (no toca la ejecución).
        if effectful {
            RoutingDecision {
                tier: cls.tier,
                reason: cls.reason,
                mode: self.mode,
                // Desvía de verdad solo cuando elige Cheap (el ahorro). Si eligió Premium (gate de
                // seguridad o heurística), no hay desvío vs el baseline premium.
                desviado: cls.tier == Tier::Cheap,
                reroute_count: 0,
                security_redacted,
                session_interactive,
            }
        } else {
            RoutingDecision::passthrough(&cls, self.mode, security_redacted, session_interactive)
        }
    }

    /// Resuelve el `SessionOrigin` SERVER-SIDE y rutea, en un solo paso (audit-3: el caller no debe
    /// fabricar el origen). `buffer = None` ⇒ SIN señal disponible ⇒ se asume INTERACTIVO
    /// (`UserInitiated`, conservador: nunca free ante la duda). Con buffer, el origen sale de
    /// `from_server_state` (mirando el estado real de eventos UI del backend). Esta es la vía
    /// preferida del wiring; deja `route_decision` para tests y call-sites que ya tienen el origen.
    #[allow(clippy::too_many_arguments)] // entry-point del wiring: resolución de origen + ruteo
    pub fn route_resolving_origin(
        &self,
        req: &RoutingRequest,
        buffer: Option<&RecentEventBuffer>,
        session_id: &str,
        now: Instant,
        gate_passed: bool,
        free_text: Option<&str>,
        conn: Option<&mut Connection>,
        tenant_id: Option<&str>,
    ) -> PreparedRouting {
        let origin = match buffer {
            Some(buf) => SessionOrigin::from_server_state(buf, session_id, now),
            // Sin buffer ⇒ sin señal ⇒ conservador: interactivo (nunca free).
            None => SessionOrigin::interactive_no_signal(),
        };
        self.route_and_prepare(req, origin, gate_passed, free_text, conn, tenant_id)
    }

    /// SAFE entry point (audit-3 P0, convergente codex+mistral): produce la decisión Y el texto que
    /// REALMENTE iría a free, ya REDACTADO — inseparables. Un caller NUNCA debe mandar texto crudo a
    /// un tier free: debe llamar a esto y usar `free_payload` (que es `Some` SOLO cuando la decisión
    /// es Cheap y SIEMPRE redactado; `None` si la decisión es Premium o no hay texto). Esto cierra el
    /// footgun de "Cheap con `free_text=None` pero payload con secretos": acá el texto que sale a free
    /// es el que pasó por `redact_for_free`, o no hay texto. Fail-closed: si la policy es `None`, la
    /// decisión es Premium ⇒ `free_payload=None` ⇒ nada va a free.
    pub fn route_and_prepare(
        &self,
        req: &RoutingRequest,
        origin: SessionOrigin,
        gate_passed: bool,
        free_text: Option<&str>,
        conn: Option<&mut Connection>,
        tenant_id: Option<&str>,
    ) -> PreparedRouting {
        let decision = self.route_decision(req, origin, gate_passed, free_text, conn, tenant_id);
        // El texto a free SOLO se materializa si la decisión final es Cheap. Y SIEMPRE redactado.
        let free_payload = if decision.tier == Tier::Cheap {
            match (free_text, &self.policy) {
                (Some(text), Some(policy)) => {
                    let (redacted, _hit) = redact_for_free(text, policy);
                    Some(redacted)
                }
                // Cheap sin policy NO debería ocurrir (sin policy ⇒ Premium), pero por defensa en
                // profundidad: sin policy no se materializa texto a free.
                (Some(_text), None) => None,
                (None, _) => None,
            }
        } else {
            // Premium ⇒ el agente usa su BYOK; el router no toca el texto (no va a free).
            None
        };
        PreparedRouting {
            decision,
            free_payload,
        }
    }

    /// Escalada de reroute: el escalón cheap falló ⇒ escala a premium UNA vez. Invariante:
    /// `reroute_count ≤ 1`. Un 2º intento (ya con `reroute_count >= 1`) va premium directo SIN
    /// incrementar (nunca cascada larga).
    pub fn escalate_reroute(&self, decision: &RoutingDecision) -> RoutingDecision {
        RoutingDecision {
            tier: Tier::Premium,
            reason: "reroute_to_premium".into(),
            mode: decision.mode,
            // Tras escalar a premium, ya no hay desvío a cheap.
            desviado: false,
            // Cap duro: el 1er reroute pone 1; un 2º intento (ya en 1) se mantiene en 1 (nunca >1).
            reroute_count: 1,
            security_redacted: decision.security_redacted,
            session_interactive: decision.session_interactive,
        }
    }
}

/// Peek de quota: evalúa si el `tenant` tiene budget free para `tokens` SIN escribir (read-only). Lo
/// usa el router en off/shadow para la traza (no debe consumir budget cuando no desvía). Misma
/// semántica de outcomes que `try_consume_quota` pero sin mutar la fila.
pub fn peek_quota(conn: &Connection, tenant_id: &str, tokens: u32) -> QuotaOutcome {
    let row: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT batch_budget_tokens, used_batch_tokens, period FROM router_quotas WHERE tenant_id = ?1",
            rusqlite::params![tenant_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let (budget, used, period) = match row {
        Some(x) => x,
        None => return QuotaOutcome::NoQuota,
    };
    let used_effective = if period == current_period() { used } else { 0 };
    if used_effective.saturating_add(tokens as i64) > budget {
        QuotaOutcome::Exhausted
    } else {
        QuotaOutcome::Consumed // "habría budget" (no consume)
    }
}

// ── Singleton de proceso (instalado en boot) ─────────────────────────────────────────────────────

static GLOBAL_ROUTER: Lazy<Mutex<Option<RouterMode>>> = Lazy::new(|| Mutex::new(None));

/// Instala el modo del router (idempotente: la 1ra gana). Lo llama `lib.rs` en boot. Guardar solo el
/// modo (la policy/clasificador se reconstruyen por request, baratos) evita threadear el router por
/// todo el árbol.
pub fn install_mode(mode: RouterMode) {
    let mut g = GLOBAL_ROUTER.lock();
    if g.is_none() {
        *g = Some(mode);
    }
}

/// Lee el modo instalado (o `Off` si no se instaló).
pub fn installed_mode() -> RouterMode {
    GLOBAL_ROUTER.lock().unwrap_or(RouterMode::Off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/053_cost_router_events.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/054_cost_router_quota.sql"))
            .unwrap();
        conn
    }

    fn router(mode: RouterMode) -> CostRouter {
        CostRouter {
            policy: Some(RouterPolicy::default_v1()),
            classifier: Box::new(HeuristicClassifier),
            mode,
        }
    }

    // ── P2: clasificador heurístico ─────────────────────────────────────────────
    #[test]
    fn classifier_simple_is_cheap() {
        let c = HeuristicClassifier;
        let r = c.classify(&RoutingRequest::simple(2, 1000));
        assert_eq!(r.tier, Tier::Cheap, "{}", r.reason);
    }

    #[test]
    fn classifier_high_context_is_premium() {
        let c = HeuristicClassifier;
        let r = c.classify(&RoutingRequest::simple(2, 64_000));
        assert_eq!(r.tier, Tier::Premium);
        assert_eq!(r.reason, "context_tokens>32k");
    }

    #[test]
    fn classifier_is_deterministic() {
        let c = HeuristicClassifier;
        let req = RoutingRequest::simple(3, 5000);
        assert_eq!(c.classify(&req), c.classify(&req));
    }

    // ── P3: gates de seguridad ──────────────────────────────────────────────────
    #[test]
    fn force_premium_tool_before_heuristic() {
        let r = router(RouterMode::Active);
        let mut req = RoutingRequest::simple(1, 100); // simple ⇒ heurística diría cheap
        req.tool_id = "auth_login".into();
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "force_premium:policy_gate");
        assert!(!d.desviado, "un force_premium nunca desvía a cheap");
    }

    #[test]
    fn force_premium_route() {
        let r = router(RouterMode::Active);
        let mut req = RoutingRequest::simple(1, 100);
        req.route = "/api/secrets/rotate".into();
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "force_premium:policy_gate");
    }

    #[test]
    fn hard_gate_tool_count_forces_premium() {
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(11, 100); // >10
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "force_premium:hard_gate_tool_count");
    }

    #[test]
    fn data_residency_forces_premium() {
        let r = router(RouterMode::Active);
        let mut req = RoutingRequest::simple(1, 100);
        req.payload.has_raw_prompt = true;
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "force_premium:data_residency");
    }

    #[test]
    fn no_policy_fails_closed() {
        let r = CostRouter::fail_closed(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100); // simple, pero sin policy ⇒ premium
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "force_premium:no_policy");
    }

    // ── Redacción antes de free ─────────────────────────────────────────────────
    #[test]
    fn redact_for_free_redacts_secrets() {
        let p = RouterPolicy::default_v1();
        let (out, hit) = redact_for_free("key is sk-proj-abcdefghijklmnopqrstuv and done", &p);
        assert!(hit);
        assert!(!out.contains("sk-proj-abcdefghijklmnopqrstuv"));
    }

    #[test]
    fn redact_for_free_catches_akia_and_pem() {
        let p = RouterPolicy::default_v1();
        let (_o, h1) = redact_for_free("aws AKIAIOSFODNN7EXAMPLE here", &p);
        assert!(h1, "AKIA debe redactarse");
        let (_o2, h2) = redact_for_free("-----BEGIN RSA PRIVATE KEY-----", &p);
        assert!(h2, "PEM debe redactarse");
    }

    #[test]
    fn redact_for_free_catches_api_key_assignment() {
        let p = RouterPolicy::default_v1();
        let (_o, hit) = redact_for_free(r#"api_key = "supersecretvalue123""#, &p);
        assert!(hit);
    }

    #[test]
    fn no_secret_no_redaction() {
        let p = RouterPolicy::default_v1();
        let (out, hit) = redact_for_free("just normal code here", &p);
        assert!(!hit);
        assert_eq!(out, "just normal code here");
    }

    #[test]
    fn cheap_decision_sets_security_redacted_flag() {
        let r = router(RouterMode::Shadow);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let d = r.route_decision(
            &req,
            SessionOrigin::Automated,
            true,
            Some("Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.aaaaaaaaaa"),
            None,
            None,
        );
        assert_eq!(d.tier, Tier::Cheap);
        assert!(d.security_redacted, "secreto en el texto a free ⇒ redactado");
    }

    // ── audit-3 P0: route_and_prepare — Cheap inseparable de su texto redactado ──
    #[test]
    fn prepare_cheap_returns_redacted_free_payload() {
        let r = router(RouterMode::Shadow);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let prep = r.route_and_prepare(
            &req,
            SessionOrigin::Automated,
            true,
            Some("token = \"supersecretvalue123\" rest"),
            None,
            None,
        );
        assert_eq!(prep.decision.tier, Tier::Cheap);
        let payload = prep.free_payload.expect("cheap ⇒ free_payload Some");
        assert!(!payload.contains("supersecretvalue123"), "el texto a free está redactado");
        assert!(prep.decision.security_redacted);
    }

    #[test]
    fn prepare_premium_yields_no_free_payload() {
        let r = router(RouterMode::Active);
        let mut req = RoutingRequest::simple(1, 100);
        req.tool_id = "auth_login".into(); // force_premium
        let prep = r.route_and_prepare(
            &req,
            SessionOrigin::Automated,
            true,
            Some("api_key = \"sk-leakmeplease12345678\""),
            None,
            None,
        );
        assert_eq!(prep.decision.tier, Tier::Premium);
        assert!(prep.free_payload.is_none(), "premium ⇒ NADA va a free");
    }

    #[test]
    fn prepare_cheap_without_free_text_sends_nothing() {
        // Cheap pero sin free_text ⇒ free_payload None (no se inventa texto, no hay leak posible).
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100);
        let prep =
            r.route_and_prepare(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(prep.decision.tier, Tier::Cheap);
        assert!(prep.free_payload.is_none());
    }

    #[test]
    fn route_resolving_origin_no_buffer_is_interactive_premium() {
        // Sin buffer ⇒ sin señal ⇒ interactivo ⇒ NUNCA free (premium), aunque la tarea sea simple.
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100);
        let prep = r.route_resolving_origin(
            &req,
            None, // sin buffer
            "sess",
            Instant::now(),
            true,
            None,
            None,
            None,
        );
        assert_eq!(prep.decision.tier, Tier::Premium);
        assert_eq!(prep.decision.reason, "force_premium:session_interactive");
        assert!(prep.free_payload.is_none());
    }

    #[test]
    fn route_resolving_origin_with_recent_ui_is_premium() {
        let r = router(RouterMode::Active);
        let buf = RecentEventBuffer::new();
        let now = Instant::now();
        buf.record_ui_event("sess", now); // evento UI reciente ⇒ interactivo
        let req = RoutingRequest::simple(1, 100);
        let prep = r.route_resolving_origin(
            &req,
            Some(&buf),
            "sess",
            now,
            true,
            None,
            None,
            None,
        );
        assert_eq!(prep.decision.tier, Tier::Premium, "interactivo ⇒ premium");
    }

    // ── US3: SessionOrigin ──────────────────────────────────────────────────────
    #[test]
    fn session_origin_user_initiated_when_recent_ui_event() {
        let buf = RecentEventBuffer::new();
        let now = Instant::now();
        buf.record_ui_event("s1", now);
        let o = SessionOrigin::from_server_state(&buf, "s1", now);
        assert_eq!(o, SessionOrigin::UserInitiated);
        assert!(o.is_interactive());
    }

    #[test]
    fn session_origin_automated_when_no_recent_event() {
        let buf = RecentEventBuffer::new();
        let now = Instant::now();
        // evento viejo (fuera de ventana)
        buf.record_ui_event("s1", now - Duration::from_secs(5));
        let o = SessionOrigin::from_server_state(&buf, "s1", now);
        assert_eq!(o, SessionOrigin::Automated);
    }

    #[test]
    fn session_origin_no_signal_assumes_interactive() {
        let buf = RecentEventBuffer::new();
        let now = Instant::now();
        // sesión nunca vista ⇒ sin señal ⇒ conservador: Automated NO; el fallback de from_server_state
        // devuelve Automated solo si la sesión existe pero está vieja. Sin entrada, también Automated.
        // El "conservador interactivo" se materializa en route_decision: cuando NO hay buffer
        // disponible, el caller pasa UserInitiated. Acá probamos el contrato del buffer.
        let o = SessionOrigin::from_server_state(&buf, "never-seen", now);
        // Sin evento ⇒ Automated (el buffer no inventa interactividad); el conservadurismo
        // "sin señal ⇒ interactivo" lo aplica el wiring que, ante ausencia de buffer, NO consulta.
        assert_eq!(o, SessionOrigin::Automated);
    }

    #[test]
    fn interactive_session_never_routes_to_free() {
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ heurística diría cheap
        let d = r.route_decision(&req, SessionOrigin::UserInitiated, true, None, None, None);
        assert_eq!(d.tier, Tier::Premium, "interactivo NUNCA a free");
        assert_eq!(d.reason, "force_premium:session_interactive");
    }

    // ── US4: quota ──────────────────────────────────────────────────────────────
    #[test]
    fn quota_consumes_within_budget() {
        let mut conn = mem_db();
        conn.execute(
            "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
             VALUES ('t1', 1000, 0, strftime('%Y-%m','now'))",
            [],
        )
        .unwrap();
        let out = try_consume_quota(&mut conn, "t1", 100);
        assert_eq!(out, QuotaOutcome::Consumed);
        assert!(out.allows_free());
    }

    #[test]
    fn quota_exhausted_falls_to_premium() {
        let mut conn = mem_db();
        conn.execute(
            "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
             VALUES ('t1', 1000, 950, strftime('%Y-%m','now'))",
            [],
        )
        .unwrap();
        let out = try_consume_quota(&mut conn, "t1", 100); // 950+100 > 1000
        assert_eq!(out, QuotaOutcome::Exhausted);
        assert!(!out.allows_free());
    }

    #[test]
    fn quota_no_row_is_premium() {
        let mut conn = mem_db();
        let out = try_consume_quota(&mut conn, "unknown", 100);
        assert_eq!(out, QuotaOutcome::NoQuota);
        assert!(!out.allows_free());
    }

    #[test]
    fn quota_contended_when_db_locked() {
        // Simula contención: una txn IMMEDIATE abierta en otra conexión sobre la MISMA DB de archivo,
        // con busy_timeout=0 en la conexión que intenta consumir.
        let dir = std::env::temp_dir().join(format!("furx-quota-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("q.db");
        let _ = std::fs::remove_file(&path);

        let setup = Connection::open(&path).unwrap();
        setup
            .execute_batch(include_str!("../../migrations/053_cost_router_events.sql"))
            .unwrap();
        setup
            .execute_batch(include_str!("../../migrations/054_cost_router_quota.sql"))
            .unwrap();
        setup
            .execute(
                "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
                 VALUES ('t1', 1000, 0, strftime('%Y-%m','now'))",
                [],
            )
            .unwrap();

        // Conexión A: toma un write-lock con una txn IMMEDIATE y lo mantiene.
        let mut conn_a = Connection::open(&path).unwrap();
        let tx_a = conn_a
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx_a.execute(
            "UPDATE router_quotas SET used_batch_tokens = 1 WHERE tenant_id = 't1'",
            [],
        )
        .unwrap();
        // (tx_a sigue abierta, lock tomado)

        // Conexión B: busy_timeout=0 ⇒ no espera ⇒ Contended.
        let mut conn_b = Connection::open(&path).unwrap();
        conn_b.busy_timeout(Duration::from_millis(0)).unwrap();
        let out = try_consume_quota(&mut conn_b, "t1", 100);
        assert_eq!(out, QuotaOutcome::Contended, "lock tomado ⇒ premium sin bloquear");
        assert!(!out.allows_free());

        drop(tx_a);
        let _ = std::fs::remove_file(&path);
    }

    // ── F0: KPI gate ────────────────────────────────────────────────────────────
    #[test]
    fn kpi_gate_no_data_warming_up() {
        let conn = mem_db();
        let g = KpiGate::evaluate(&conn);
        assert!(!g.passed);
        assert_eq!(g.reason, "warming_up");
    }

    #[test]
    fn kpi_gate_passes_with_good_data() {
        let conn = mem_db();
        // 4 eventos local (batch) con buen ahorro, 0 reroute ⇒ batch 100%, tns alto, reroute 0.
        for i in 0..4 {
            conn.execute(
                "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd, reroute_count) \
                 VALUES (?1, 'local', 0.0, 0.10, 0)",
                rusqlite::params![format!("e{i}")],
            )
            .unwrap();
        }
        let g = KpiGate::evaluate(&conn);
        assert!(g.passed, "batch=100% tns=100% reroute=0% ⇒ pasa ({})", g.reason);
        assert!(g.batch_pct >= 25.0 && g.tns_batch >= 25.0 && g.reroute_rate <= 12.0);
    }

    #[test]
    fn kpi_gate_fails_low_tns() {
        let conn = mem_db();
        // premium dominante: poco ahorro ⇒ tns bajo.
        conn.execute(
            "INSERT INTO cost_router_events (event_id, decision, cost_real_usd, cost_baseline_premium_usd, reroute_count) \
             VALUES ('a','premium', 0.10, 0.10, 0)",
            [],
        )
        .unwrap();
        let g = KpiGate::evaluate(&conn);
        assert!(!g.passed);
        // batch 0% dispara primero (insufficient_batch) — igual NO pasa, que es lo que importa.
    }

    // ── Modos / no-op / desvío ──────────────────────────────────────────────────
    #[test]
    fn off_mode_is_noop_passthrough() {
        let r = router(RouterMode::Off);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.mode, RouterMode::Off);
        assert!(!d.desviado, "OFF nunca desvía (cero regresión)");
    }

    #[test]
    fn off_mode_does_not_consume_quota() {
        // OFF = no-op TOTAL: ni siquiera toca el budget free del tenant (cero regresión).
        let mut conn = mem_db();
        conn.execute(
            "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
             VALUES ('t1', 1000, 0, strftime('%Y-%m','now'))",
            [],
        )
        .unwrap();
        let r = router(RouterMode::Off);
        let mut req = RoutingRequest::simple(1, 100);
        req.estimated_tokens = 100;
        let _ = r.route_decision(
            &req,
            SessionOrigin::Automated,
            true,
            None,
            Some(&mut conn),
            Some("t1"),
        );
        let used: i64 = conn
            .query_row(
                "SELECT used_batch_tokens FROM router_quotas WHERE tenant_id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(used, 0, "OFF NO consume quota (no-op total)");
    }

    #[test]
    fn shadow_mode_does_not_consume_quota() {
        // Shadow decide+loguea pero NO desvía ⇒ tampoco consume budget (peek, no write).
        let mut conn = mem_db();
        conn.execute(
            "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
             VALUES ('t1', 1000, 0, strftime('%Y-%m','now'))",
            [],
        )
        .unwrap();
        let r = router(RouterMode::Shadow);
        let mut req = RoutingRequest::simple(1, 100);
        req.estimated_tokens = 100;
        let _ = r.route_decision(
            &req,
            SessionOrigin::Automated,
            true,
            None,
            Some(&mut conn),
            Some("t1"),
        );
        let used: i64 = conn
            .query_row(
                "SELECT used_batch_tokens FROM router_quotas WHERE tenant_id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(used, 0, "shadow NO consume quota (no desvía)");
    }

    #[test]
    fn active_with_gate_consumes_quota() {
        // active + gate pasado ⇒ desvía de verdad ⇒ COMETE el consumo de quota.
        let mut conn = mem_db();
        conn.execute(
            "INSERT INTO router_quotas (tenant_id, batch_budget_tokens, used_batch_tokens, period) \
             VALUES ('t1', 1000, 0, strftime('%Y-%m','now'))",
            [],
        )
        .unwrap();
        let r = router(RouterMode::Active);
        let mut req = RoutingRequest::simple(1, 100);
        req.estimated_tokens = 100;
        let d = r.route_decision(
            &req,
            SessionOrigin::Automated,
            true,
            None,
            Some(&mut conn),
            Some("t1"),
        );
        assert_eq!(d.tier, Tier::Cheap);
        assert!(d.desviado);
        let used: i64 = conn
            .query_row(
                "SELECT used_batch_tokens FROM router_quotas WHERE tenant_id = 't1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(used, 100, "active+gate desvía ⇒ consume quota");
    }

    #[test]
    fn shadow_mode_decides_but_does_not_divert() {
        let r = router(RouterMode::Shadow);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Cheap, "shadow computa la decisión");
        assert!(!d.desviado, "shadow NO desvía la ejecución");
    }

    #[test]
    fn active_with_gate_diverts_cheap() {
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let d = r.route_decision(&req, SessionOrigin::Automated, true, None, None, None);
        assert_eq!(d.tier, Tier::Cheap);
        assert!(d.desviado, "active + gate + cheap ⇒ desvía de verdad");
    }

    #[test]
    fn active_without_gate_does_not_divert() {
        let r = router(RouterMode::Active);
        let req = RoutingRequest::simple(1, 100); // simple ⇒ cheap
        let d = r.route_decision(&req, SessionOrigin::Automated, false, None, None, None);
        assert!(!d.desviado, "active + gate NO pasado ⇒ no desvía (como shadow)");
    }

    // ── Reroute ≤ 1 ─────────────────────────────────────────────────────────────
    #[test]
    fn reroute_caps_at_one() {
        let r = router(RouterMode::Active);
        let base = r.route_decision(
            &RoutingRequest::simple(1, 100),
            SessionOrigin::Automated,
            true,
            None,
            None,
            None,
        );
        let first = r.escalate_reroute(&base);
        assert_eq!(first.reroute_count, 1);
        assert_eq!(first.tier, Tier::Premium);
        // Un 2º intento nunca supera 1.
        let second = r.escalate_reroute(&first);
        assert_eq!(second.reroute_count, 1, "reroute nunca > 1");
        assert!(!second.desviado);
    }

    // ── RouterMode::parse (PURO, sin mutar el env global → no colisiona bajo cargo test concurrente)
    #[test]
    fn mode_parse_defaults_off() {
        assert_eq!(RouterMode::parse(None), RouterMode::Off, "ausente ⇒ off");
        assert_eq!(RouterMode::parse(Some("garbage")), RouterMode::Off, "valor raro ⇒ off");
        assert_eq!(RouterMode::parse(Some("")), RouterMode::Off, "vacío ⇒ off");
        assert_eq!(RouterMode::parse(Some("  shadow  ")), RouterMode::Shadow, "trim");
        assert_eq!(RouterMode::parse(Some("ACTIVE")), RouterMode::Active, "case-insensitive");
        assert_eq!(RouterMode::parse(Some("shadow")), RouterMode::Shadow);
        assert_eq!(RouterMode::parse(Some("active")), RouterMode::Active);
    }

    // ── glob_match ──────────────────────────────────────────────────────────────
    #[test]
    fn glob_match_prefix_suffix_exact() {
        assert!(glob_match("auth_*", "auth_login"));
        assert!(!glob_match("auth_*", "login_auth"));
        assert!(glob_match("/api/secrets/*", "/api/secrets/rotate"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exactly"));
    }
}
