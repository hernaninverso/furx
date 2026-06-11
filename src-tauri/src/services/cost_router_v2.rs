// services/cost_router_v2.rs — spec-052 · Cost-Router Classifier v2 (bandit-ready).
//
// EVOLUCIONA el clasificador de 049 (`cost_router.rs`, heurística de returns) al diseño canónico del
// council máximo (`docs/cost-router/f2-classifier-canonical.md` + 12 correcciones C1–C12): score
// ponderado 6-dim + thresholds por task_type + bandit ε-greedy con reward por SLA + circuit breaker del
// escalón free + canary exploration. **TODO OFF detrás de `FURX_COST_ROUTER_MODE`** (default off ⇒
// no-op): este módulo solo provee la maquinaria; el router (049) la usa cuando hay `RouterConfig` y el
// flag no es off. Sin config / off ⇒ comportamiento idéntico a 049 → cero regresión.
//
// FAIL-CLOSED en todos lados: config inválida ⇒ última válida o premium; CB abierto ⇒ premium; score
// NaN/Inf ⇒ premium; decision_id NULL ⇒ no persistir; sin señal ⇒ premium. NUNCA panic en el hot-path.
//
// El reward del bandit necesita success+latency que F1 (053) NO emite (recon Fase -1). Este módulo
// cierra ese contrato: `infer_outcome` + el bridge a `cost_router_outcomes` (ver `savings_meter`). El
// CONSUMO del reward está gated por `Phase` (Off/LogOnly NO ajustan threshold).

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU8, Ordering};

use rusqlite::Connection;

use super::cost_router::{ClassificationResult, RoutingRequest, Tier, TierClassifier};

// ── Kill switch (council C1) ───────────────────────────────────────────────────────────────────────

/// Kill switch dedicado: `FURX_COST_ROUTER_FORCE_PREMIUM=1` ⇒ todo premium (primer hard gate). Rollback
/// sin redeploy. Distinto de `FURX_COST_ROUTER_MODE=off` (que apaga el router entero): el kill switch
/// fuerza premium AUNQUE el router esté activo.
const FORCE_PREMIUM_ENV: &str = "FURX_COST_ROUTER_FORCE_PREMIUM";

/// Lee el kill switch del env. Parse puro (testeable sin mutar env global).
pub fn kill_switch_active() -> bool {
    kill_switch_parse(std::env::var(FORCE_PREMIUM_ENV).ok().as_deref())
}

/// `1`/`true`/`yes` (case-insensitive, trimmed) ⇒ activo. Cualquier otro / ausente ⇒ inactivo.
pub fn kill_switch_parse(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

// ── Tiempo (council C6: SystemTime, NO Instant — sobrevive suspend-to-RAM) ─────────────────────────

/// Epoch en milisegundos desde `SystemTime`. NO `Instant` (que se congela en suspend-to-RAM y haría que
/// el cooldown del circuit breaker no expire). Para el cooldown persistido esto es obligatorio.
pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Task type / priority ───────────────────────────────────────────────────────────────────────────

/// Tipo de tarea. Selecciona el threshold y el SLA; `Sensitive` es hard gate (siempre premium).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Interactive,
    BatchVerified,
    BackgroundDaemon,
    Sensitive,
}

impl TaskType {
    /// Deriva el `TaskType` del `task_type: String` de 049 (heurístico). Desconocido ⇒ `Interactive`
    /// (conservador: threshold más alto = más premium).
    pub fn from_str_loose(s: &str) -> Self {
        let t = s.trim().to_ascii_lowercase();
        if t.contains("sensitive")
            || t.contains("security")
            || t.contains("auth")
            || t.contains("crypto")
            || t.contains("secret")
        {
            TaskType::Sensitive
        } else if t.contains("batch") {
            TaskType::BatchVerified
        } else if t.contains("background") || t.contains("daemon") {
            TaskType::BackgroundDaemon
        } else {
            TaskType::Interactive
        }
    }
}

/// Prioridad de la tarea. `Forced` es hard gate (siempre premium).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
    Forced,
}

impl Priority {
    /// Mapea a un score [0,1] para la dim de prioridad y el guard de exploración (`>0.7` no explora).
    pub fn score(self) -> f32 {
        match self {
            Priority::Low => 0.0,
            Priority::Normal => 0.4,
            Priority::High => 0.8,
            Priority::Forced => 1.0,
        }
    }
}

// ── Features (council §1: 9 dims core; score usa 6) ───────────────────────────────────────────────

/// Las dimensiones de una tarea para clasificar. Derivadas de `RoutingRequest` (049) + flags nuevos.
#[derive(Debug, Clone)]
pub struct TaskFeatures {
    pub token_count: u32,
    pub tool_count: u8,
    /// `tokens_usados / ventana_max`. Ventana desconocida ⇒ 0.5 (conservador). Se clampa a [0,1].
    pub context_fraction: f32,
    pub turn_count: u8,
    pub has_code: bool,
    pub code_line_count: u16,
    pub priority: Priority,
    /// Hard gate: tarea irreversible sin mecanismo de rollback (commit remoto, POST financiero, DROP).
    pub is_irreversible_without_mitigation: bool,
    /// Hard gate: `retry_count>0 AND primer intento lento`. Lo decide el caller (compara vs P90 local).
    pub retry_with_slow_first: bool,
    /// Council C4/open#1: tool de escritura sin declaración segura ⇒ presiona premium. Default false.
    pub is_safe_idempotent: bool,
    /// Sesión interactiva (humano esperando). Invariante de 049: lo interactivo NUNCA va a free. El
    /// caller lo resuelve server-side (`SessionOrigin` de 049, `RecentEventBuffer`). **Default `true`**
    /// (conservador: sin señal ⇒ interactivo ⇒ premium). El dispatch que sabe que es batch/background
    /// setea `false`.
    pub session_interactive: bool,
    pub task_type: TaskType,
}

impl TaskFeatures {
    /// Deriva de un `RoutingRequest` de 049. Mapeos conservadores para los campos que 049 no tiene
    /// (context_fraction desconocido ⇒ 0.5; sin info de código ⇒ false; prioridad ⇒ Normal;
    /// `session_interactive` ⇒ `true` fail-closed: sin señal server-side, se trata como interactivo).
    pub fn from_request(req: &RoutingRequest) -> Self {
        Self {
            token_count: req.context_tokens,
            tool_count: req.tool_count.min(u8::MAX as u32) as u8,
            context_fraction: 0.5,
            turn_count: req.turn_number.min(u8::MAX as u16) as u8,
            has_code: false,
            code_line_count: 0,
            priority: Priority::Normal,
            is_irreversible_without_mitigation: false,
            retry_with_slow_first: false,
            is_safe_idempotent: false,
            session_interactive: true,
            task_type: TaskType::from_str_loose(&req.task_type),
        }
    }
}

// ── Config (council §2: pesos validados al boot) ──────────────────────────────────────────────────

/// Pesos de las 6 dims de score. Deben sumar 1.0 ± 0.001 (validado al boot).
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub tool_count: f32,
    pub context_fraction: f32,
    pub token_count: f32,
    pub has_code: f32,
    pub turn_count: f32,
    pub priority: f32,
}

/// Threshold de un task_type. `premium`: score ≥ ⇒ premium. `free`: score ≤ ⇒ local/free. Entre medio
/// ⇒ incertidumbre ⇒ premium (fail-closed).
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub premium: f32,
    pub free: f32,
}

/// Config del router. Embebida en Rust (NO parse TOML en boot ⇒ sin punto de fallo). Validada por
/// `validate()`. El TOML `config/router_weights.v1.toml` es referencia/IaC, no se carga en runtime.
#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub version: u32,
    pub weights: Weights,
    pub token_count_cap: u32,
    pub tool_count_max: u32,
    pub turn_count_max: u32,
    pub th_interactive: Thresholds,
    pub th_batch: Thresholds,
    pub th_background: Thresholds,
    pub sla_interactive_ms: u32,
    pub sla_batch_ms: u32,
    pub sla_background_ms: u32,
    pub local_latency_sla_ms: u32,
    /// Umbral de fallos consecutivos del free para abrir el circuit breaker.
    pub cb_fail_threshold: u32,
    pub cb_base_cooldown_ms: i64,
}

/// Errores de validación de config. `validate()` los devuelve; el boot conserva la última válida o
/// fail-closed (premium).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    InvalidWeight(f32),
    WeightsDontSumToOne(f32),
    ZeroDivisor,
    ThresholdOutOfRange(&'static str),
    ThresholdInverted(&'static str),
    InvalidBanditConfig,
}

impl RouterConfig {
    /// Default v1 = el canónico del council (pesos: tool .30, ctx .25, token .20, code .15, turn .07,
    /// prio .03 = 1.00). Embebido, NUNCA falla parse.
    pub fn default_v1() -> Self {
        Self {
            version: 1,
            weights: Weights {
                tool_count: 0.30,
                context_fraction: 0.25,
                token_count: 0.20,
                has_code: 0.15,
                turn_count: 0.07,
                priority: 0.03,
            },
            token_count_cap: 128_000,
            tool_count_max: 20,
            turn_count_max: 30,
            th_interactive: Thresholds { premium: 0.60, free: 0.30 },
            th_batch: Thresholds { premium: 0.70, free: 0.25 },
            th_background: Thresholds { premium: 0.80, free: 0.20 },
            sla_interactive_ms: 3_000,
            sla_batch_ms: 10_000,
            sla_background_ms: 30_000,
            local_latency_sla_ms: 2_000,
            cb_fail_threshold: 3,
            cb_base_cooldown_ms: 30_000,
        }
    }

    /// Thresholds para un task_type. `Sensitive` no llega acá (hard gate antes); por seguridad devuelve
    /// el más conservador.
    pub fn thresholds_for(&self, tt: TaskType) -> Thresholds {
        match tt {
            TaskType::Interactive => self.th_interactive,
            TaskType::BatchVerified => self.th_batch,
            TaskType::BackgroundDaemon => self.th_background,
            TaskType::Sensitive => Thresholds { premium: 0.0, free: 0.0 },
        }
    }

    /// SLA en ms para un task_type (puente bool→outcome).
    pub fn sla_ms(&self, tt: TaskType) -> u32 {
        match tt {
            TaskType::Interactive => self.sla_interactive_ms,
            TaskType::BatchVerified => self.sla_batch_ms,
            TaskType::BackgroundDaemon | TaskType::Sensitive => self.sla_background_ms,
        }
    }

    /// Validación al boot (council §2). Fail-closed: si falla, el caller conserva la última válida o
    /// usa `None` ⇒ todo premium.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let w = self.weights;
        let ws = [
            w.tool_count,
            w.context_fraction,
            w.token_count,
            w.has_code,
            w.turn_count,
            w.priority,
        ];
        for &x in &ws {
            if x.is_nan() || x.is_infinite() || x < 0.0 {
                return Err(ConfigError::InvalidWeight(x));
            }
        }
        let sum: f32 = ws.iter().sum();
        if (sum - 1.0).abs() > 0.001 {
            return Err(ConfigError::WeightsDontSumToOne(sum));
        }
        if self.token_count_cap == 0 || self.tool_count_max == 0 || self.turn_count_max == 0 {
            return Err(ConfigError::ZeroDivisor);
        }
        for (name, thr) in [
            ("interactive", self.th_interactive),
            ("batch", self.th_batch),
            ("background", self.th_background),
        ] {
            if !(0.0..=1.0).contains(&thr.premium) || !(0.0..=1.0).contains(&thr.free) {
                return Err(ConfigError::ThresholdOutOfRange(name));
            }
            if thr.free >= thr.premium {
                return Err(ConfigError::ThresholdInverted(name));
            }
        }
        if self.local_latency_sla_ms == 0 || self.cb_fail_threshold == 0 {
            return Err(ConfigError::InvalidBanditConfig);
        }
        Ok(())
    }
}

// ── Score + clasificador (council §2, US1) ────────────────────────────────────────────────────────

/// Error de cómputo de score (NaN/Inf). El caller fuerza premium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreError;

/// Score ponderado normalizado [0,1]. Guard de overflow (cap tokens) y de NaN/Inf (Err). NO aplica
/// hard gates (eso es `check_hard_gates_v2`). Sub-µs (6 mult + suma).
pub fn compute_score(f: &TaskFeatures, cfg: &RouterConfig) -> Result<f32, ScoreError> {
    let token_capped = f.token_count.min(cfg.token_count_cap);
    let token_norm = (token_capped as f32 / cfg.token_count_cap as f32).clamp(0.0, 1.0);
    let tool_norm = (f.tool_count as f32 / cfg.tool_count_max as f32).clamp(0.0, 1.0);
    let turn_norm = (f.turn_count as f32 / cfg.turn_count_max as f32).clamp(0.0, 1.0);
    let context = f.context_fraction.clamp(0.0, 1.0);
    let code_bonus = if f.has_code && f.code_line_count > 30 {
        1.0
    } else if f.has_code {
        0.5
    } else {
        0.0
    };
    let priority_flag = match f.priority {
        Priority::High => 1.0,
        Priority::Normal => 0.5,
        Priority::Low | Priority::Forced => 0.0,
    };
    let w = cfg.weights;
    let score = w.tool_count * tool_norm
        + w.context_fraction * context
        + w.token_count * token_norm
        + w.has_code * code_bonus
        + w.turn_count * turn_norm
        + w.priority * priority_flag;
    if score.is_nan() || score.is_infinite() {
        return Err(ScoreError);
    }
    Ok(score.clamp(0.0, 1.0))
}

/// Decisión del clasificador con score expuesto (para el bandit / persistencia).
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredDecision {
    pub tier: Tier,
    pub reason: String,
    /// `None` si hubo error de score (⇒ premium).
    pub score: Option<f32>,
}

/// Clasificador por score ponderado. Embebe su `RouterConfig`. Implementa `TierClassifier` (049) ⇒ el
/// router lo usa sin cambiar su contrato. `classify_scored` expone el score para el bandit.
pub struct WeightedClassifier {
    pub config: RouterConfig,
}

impl WeightedClassifier {
    pub fn new(config: RouterConfig) -> Self {
        Self { config }
    }

    /// Clasifica por score + thresholds del task_type. Zona de incertidumbre ⇒ premium (fail-closed).
    /// NaN/Inf ⇒ premium con score `None` (el caller loguea `score_nan_guard`).
    pub fn classify_scored(&self, f: &TaskFeatures) -> ScoredDecision {
        let score = match compute_score(f, &self.config) {
            Ok(s) => s,
            Err(_) => {
                return ScoredDecision {
                    tier: Tier::Premium,
                    reason: "score_nan_guard".into(),
                    score: None,
                };
            }
        };
        let thr = self.config.thresholds_for(f.task_type);
        let tier = if score >= thr.premium {
            Tier::Premium
        } else if score <= thr.free {
            Tier::Cheap
        } else {
            Tier::Premium // incertidumbre ⇒ premium
        };
        let reason = match tier {
            Tier::Cheap => "score_below_free",
            Tier::Premium if score >= thr.premium => "score_above_premium",
            Tier::Premium => "score_uncertain",
        };
        ScoredDecision {
            tier,
            reason: reason.into(),
            score: Some(score),
        }
    }
}

impl TierClassifier for WeightedClassifier {
    fn classify(&self, req: &RoutingRequest) -> ClassificationResult {
        let f = TaskFeatures::from_request(req);
        let d = self.classify_scored(&f);
        match d.tier {
            Tier::Cheap => ClassificationResult::cheap(d.reason),
            Tier::Premium => ClassificationResult::premium(d.reason),
        }
    }
}

// ── Hard gates v2 (council C1, US2) ───────────────────────────────────────────────────────────────

/// Resultado de un hard gate: `Some((Premium, razón))` ⇒ corto-circuito a premium. `None` ⇒ seguir al
/// score. El CB abierto y el kill switch los pasa el caller (estado externo).
pub fn check_hard_gates_v2(
    f: &TaskFeatures,
    kill_switch: bool,
    cb_open: bool,
) -> Option<(Tier, &'static str)> {
    if kill_switch {
        return Some((Tier::Premium, "kill_switch"));
    }
    if f.is_irreversible_without_mitigation {
        return Some((Tier::Premium, "irreversible_no_mitigation"));
    }
    if f.priority == Priority::Forced {
        return Some((Tier::Premium, "priority_forced"));
    }
    if f.task_type == TaskType::Sensitive {
        return Some((Tier::Premium, "task_type_sensitive"));
    }
    if f.retry_with_slow_first {
        return Some((Tier::Premium, "retry_slow_first"));
    }
    // Invariante de 049 (audit-3 052): una sesión interactiva (humano esperando) NUNCA va a free. El
    // origen lo resuelve el caller server-side (SessionOrigin); sin señal ⇒ `session_interactive=true`
    // (default conservador) ⇒ premium. Cierra el hueco "sin señal ⇒ premium".
    if f.session_interactive {
        return Some((Tier::Premium, "session_interactive"));
    }
    // Council C4 / open#1: una tarea que invoca tools y NO declara `is_safe_idempotent` va premium
    // (fail-closed). El ahorro se desbloquea cuando se audita el catálogo `tools_safety` (acción humana,
    // open#1) y las tools read-only/idempotentes se marcan seguras. Una tool de escritura sin declarar
    // NUNCA va a un escalón free. Las tareas sin tools (tool_count=0) no requieren la declaración.
    if f.tool_count > 0 && !f.is_safe_idempotent {
        return Some((Tier::Premium, "tool_not_declared_safe"));
    }
    if cb_open {
        return Some((Tier::Premium, "cb_open"));
    }
    None
}

// ── Circuit breaker (council C6, US3) ─────────────────────────────────────────────────────────────

/// Estado del CB para el status read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CbState {
    Closed,
    HalfOpen,
    Open,
}

/// Circuit breaker del escalón free. Átomicos NO-estáticos (instancia por router ⇒ testable sin estado
/// global, evita el flake de `cargo test` concurrente). Una sola fuente de verdad (`compare_exchange`
/// para el único probe HalfOpen). `SystemTime` epoch-ms para el cooldown (sobrevive suspend).
pub struct CircuitBreaker {
    /// 0=Closed, 1=HalfOpen, 2=Open.
    state: AtomicU8,
    cooldown_until_ms: AtomicI64,
    probe_start_ms: AtomicI64,
    failure_count: AtomicU32,
    fail_threshold: u32,
    base_cooldown_ms: i64,
    /// Semilla de jitter (hash del installation_id) ⇒ el cooldown no es idéntico entre instalaciones.
    jitter_seed: u64,
}

const CB_PROBE_TIMEOUT_MS: i64 = 30_000;
const CB_COOLDOWN_CAP_MS: i64 = 300_000;

impl CircuitBreaker {
    pub fn new(fail_threshold: u32, base_cooldown_ms: i64, jitter_seed: u64) -> Self {
        Self {
            state: AtomicU8::new(0),
            cooldown_until_ms: AtomicI64::new(0),
            probe_start_ms: AtomicI64::new(0),
            failure_count: AtomicU32::new(0),
            fail_threshold: fail_threshold.max(1),
            base_cooldown_ms: base_cooldown_ms.max(1),
            jitter_seed,
        }
    }

    pub fn from_config(cfg: &RouterConfig, jitter_seed: u64) -> Self {
        Self::new(cfg.cb_fail_threshold, cfg.cb_base_cooldown_ms, jitter_seed)
    }

    fn cooldown_ms(&self) -> i64 {
        // jitter ±0..0.9 del base, cap 300s.
        let jitter = 1.0 + (self.jitter_seed % 10) as f64 * 0.1;
        ((self.base_cooldown_ms as f64 * jitter) as i64).min(CB_COOLDOWN_CAP_MS)
    }

    /// ¿El breaker está abierto AHORA? `SeqCst` en todos los accesos a `state` (audit-3 052: evita el
    /// race de visibilidad entre `fetch_add`/`store`). Si el cooldown expiró, UN solo hilo gana el CAS
    /// Open→HalfOpen (hace el probe); el resto va premium SIN reabrir (no toca `probe_start_ms`, así no
    /// hay race con un valor viejo/0). Si un probe quedó colgado (>30s), se reabre desde la rama
    /// HalfOpen (no en el Err del CAS). NUNCA bloquea.
    pub fn is_open(&self, now_ms: i64) -> bool {
        match self.state.load(Ordering::SeqCst) {
            2 => {
                if self.cooldown_until_ms.load(Ordering::SeqCst) > now_ms {
                    return true;
                }
                // cooldown expiró: el primero en ganar el CAS hace el probe; el resto va premium.
                match self
                    .state
                    .compare_exchange(2, 1, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => {
                        self.probe_start_ms.store(now_ms, Ordering::SeqCst);
                        false // este hilo hace el probe → se le permite intentar free
                    }
                    Err(_) => true, // otro hilo lo tomó/cambió ⇒ premium (sin reabrir: nada de race)
                }
            }
            1 => {
                // HalfOpen: hay un probe en curso. Si quedó colgado (el hilo del probe nunca reportó
                // éxito/fallo), reabrir para no quedar premium para siempre. `probe != 0` evita reabrir
                // por un valor sin inicializar.
                let probe = self.probe_start_ms.load(Ordering::SeqCst);
                if probe != 0 && now_ms.saturating_sub(probe) > CB_PROBE_TIMEOUT_MS {
                    // cooldown ANTES de publicar Open (audit-3 052 round2): ningún hilo debe ver Open con
                    // un cooldown viejo/vencido. saturating_add evita overflow de epoch+cooldown.
                    self.cooldown_until_ms
                        .store(now_ms.saturating_add(self.cooldown_ms()), Ordering::SeqCst);
                    let _ = self
                        .state
                        .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
                }
                true // mientras HalfOpen, los demás van premium
            }
            _ => false, // Closed
        }
    }

    /// El free respondió OK. Cierra SOLO desde HalfOpen (el probe autorizado) — NO fuerza Closed desde
    /// Open (audit-3 052: un éxito tardío de un request viejo no debe reabrir el tráfico free que otro
    /// hilo acaba de cerrar por fallos). Siempre resetea el contador de fallos.
    pub fn record_success(&self) {
        self.failure_count.store(0, Ordering::SeqCst);
        // 1 (HalfOpen) → 0 (Closed): cierra el probe. Si está Open(2), se respeta. Si ya Closed(0), no-op.
        let _ = self
            .state
            .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// El free falló. Desde HalfOpen, el fallo del probe reabre de una. Desde Closed, cuenta; al
    /// alcanzar el umbral, abre con cooldown. En ambos caminos el cooldown se fija ANTES de publicar
    /// Open (audit-3 052 round2: nadie debe ver Open con cooldown viejo). `saturating_add` evita
    /// overflow (epoch+cooldown) y panic del contador.
    pub fn record_failure(&self, now_ms: i64) {
        let new_cooldown = now_ms.saturating_add(self.cooldown_ms());
        // Si estamos en HalfOpen, el probe falló ⇒ reabrir. cooldown primero, luego CAS 1→2.
        if self.state.load(Ordering::SeqCst) == 1 {
            self.cooldown_until_ms.store(new_cooldown, Ordering::SeqCst);
            if self
                .state
                .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return;
            }
            // perdimos el CAS (otro hilo cambió el estado) ⇒ caer al conteo normal.
        }
        // Desde Closed, contar el fallo; al umbral, abrir (CAS 0→2 para no pisar otro estado).
        let n = self.failure_count.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        if n >= self.fail_threshold {
            self.cooldown_until_ms.store(new_cooldown, Ordering::SeqCst);
            let _ = self
                .state
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    pub fn state(&self) -> CbState {
        match self.state.load(Ordering::SeqCst) {
            2 => CbState::Open,
            1 => CbState::HalfOpen,
            _ => CbState::Closed,
        }
    }
}

// ── Bandit ε-greedy (council C5/C9, US4) ──────────────────────────────────────────────────────────

/// Estado del bandit por (installation, modelo). EWMA con alpha dinámico + prior optimista (cold-start)
/// + p99 ring buffer. MUTABLE. Persiste en `bandit_state`.
#[derive(Debug, Clone)]
pub struct BanditState {
    pub real_success_ema: f32,
    pub real_latency_ema: f32,
    pub n_real: u32,
    pub p99_ring: Vec<u32>,
    pub p99_pos: usize,
}

const P99_RING_CAP: usize = 100;
const PRIOR_SUCCESS: f32 = 0.85;
const PRIOR_LATENCY_MS: f32 = 1500.0;

impl Default for BanditState {
    fn default() -> Self {
        Self::new()
    }
}

impl BanditState {
    /// Cold-start seguro: prior optimista 0.85 / 1500ms (council C9). Sin esto, las primeras requests a
    /// un free caído llevarían success_ema→0 y matarían el bandit.
    pub fn new() -> Self {
        Self {
            real_success_ema: PRIOR_SUCCESS,
            real_latency_ema: PRIOR_LATENCY_MS,
            n_real: 0,
            p99_ring: Vec::with_capacity(P99_RING_CAP),
            p99_pos: 0,
        }
    }

    /// Actualiza con una observación real. alpha = min(20/(n+1), 0.5) — el `+1` evita div-by-zero/Inf en
    /// n=0 (council C5). `saturating_add` evita overflow/panic con n_real enorme (audit-3 052). El prior
    /// decae con las observaciones reales.
    pub fn update(&mut self, success: bool, latency_ms: u32) {
        let alpha = (20.0 / self.n_real.saturating_add(1) as f32).min(0.5);
        let obs = if success { 1.0 } else { 0.0 };
        self.real_success_ema = alpha * obs + (1.0 - alpha) * self.real_success_ema;
        self.real_latency_ema = alpha * latency_ms as f32 + (1.0 - alpha) * self.real_latency_ema;
        // p99 ring.
        if self.p99_ring.len() < P99_RING_CAP {
            self.p99_ring.push(latency_ms);
        } else {
            self.p99_ring[self.p99_pos] = latency_ms;
            self.p99_pos = (self.p99_pos + 1) % P99_RING_CAP;
        }
        self.n_real = self.n_real.saturating_add(1);
    }

    /// p99 de las últimas observaciones (cap del ring). Sin datos ⇒ el prior.
    pub fn p99_latency(&self) -> u32 {
        if self.p99_ring.is_empty() {
            return PRIOR_LATENCY_MS as u32;
        }
        let mut v = self.p99_ring.clone();
        v.sort_unstable();
        let idx = ((v.len() as f32 * 0.99).ceil() as usize).saturating_sub(1);
        v[idx.min(v.len() - 1)]
    }
}

/// Reward del bandit: `(1 - latency/sla).clamp(0,1)` (council §3). Independiente del threshold (sin
/// bucle), no depende de trazas premium. Clamp evita negativos (council C: reward∈[0,1]).
pub fn bandit_reward(latency_ms: u32, sla_ms: u32) -> f32 {
    if sla_ms == 0 {
        return 0.0;
    }
    (1.0 - latency_ms as f32 / sla_ms as f32).clamp(0.0, 1.0)
}

// ── Outcome bridge (council C8, US5) ──────────────────────────────────────────────────────────────

/// Outcome semántico de una tarea (puente desde el `bool` que F1 podría dar). 4 categorías (council
/// descartó las 5 de v4 para V1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum F1Outcome {
    Success,
    SemanticFailure,
    SystemFailure,
    Degraded,
}

impl F1Outcome {
    /// Para persistir en `cost_router_outcomes.outcome` (CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            F1Outcome::Success => "success",
            F1Outcome::SemanticFailure => "semantic_failure",
            F1Outcome::SystemFailure => "system_failure",
            F1Outcome::Degraded => "degraded",
        }
    }

    /// ¿Cuenta como éxito para el bandit?
    pub fn is_success(self) -> bool {
        matches!(self, F1Outcome::Success)
    }
}

/// Puente `bool → F1Outcome` con SLA por task_type (council C8): un fallo con latencia ≥ SLA es de
/// sistema (free caído); un fallo rápido es semántico (respondió mal); un éxito muy lento (>2×SLA) es
/// degradado.
pub fn infer_outcome(success: bool, latency_ms: u32, tt: TaskType, cfg: &RouterConfig) -> F1Outcome {
    let sla = cfg.sla_ms(tt);
    if !success && latency_ms >= sla {
        F1Outcome::SystemFailure
    } else if !success {
        F1Outcome::SemanticFailure
    } else if latency_ms > sla.saturating_mul(2) {
        F1Outcome::Degraded
    } else {
        F1Outcome::Success
    }
}

// ── Phase + canary exploration (council C2/C10/C11, US6) ──────────────────────────────────────────

/// Fase del router. Integra con `RouterMode` (049): Off=no-op, LogOnly=decide+loguea sin desviar,
/// Canary=explora 5%/1%, Active=desvía de verdad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Off,
    LogOnly,
    Canary,
    Active,
}

impl Phase {
    /// Deriva la fase del `RouterMode` (049) + el gate de KPI. `Off`⇒Off; `Shadow`⇒LogOnly (decide y
    /// loguea sin desviar); `Active`+gate⇒Active (desvía); `Active` sin gate⇒LogOnly (fail-closed: no
    /// desvía hasta que el KPI pase). La fase `Canary` (exploración 5%/1%) NO se deriva del mode: es una
    /// sub-fase del rollout que se habilita explícitamente, no por el flag de 049.
    pub fn from_mode(mode: super::cost_router::RouterMode, gate_passed: bool) -> Self {
        use super::cost_router::RouterMode;
        match mode {
            RouterMode::Off => Phase::Off,
            RouterMode::Shadow => Phase::LogOnly,
            RouterMode::Active if gate_passed => Phase::Active,
            RouterMode::Active => Phase::LogOnly,
        }
    }
}

/// Exploración canary (council C2 — invertida vs el bug previo). 5% de las premium → local (recolecta
/// ground-truth de tareas difíciles); 1% de las local → premium (detecta thresholds conservadores).
/// NUNCA explora Sensitive / irreversible / priority>0.7. `roll` ∈ [0,1) inyectable (testable; en prod
/// el caller usa `rand::random()`). Devuelve `(tier, exploration)`.
pub fn apply_exploration(tier: Tier, f: &TaskFeatures, phase: Phase, roll: f32) -> (Tier, bool) {
    if phase != Phase::Canary {
        return (tier, false);
    }
    if f.task_type == TaskType::Sensitive
        || f.is_irreversible_without_mitigation
        || f.priority.score() > 0.7
    {
        return (tier, false);
    }
    match tier {
        Tier::Premium if roll < 0.05 => (Tier::Cheap, true),
        Tier::Cheap if roll < 0.01 => (Tier::Premium, true),
        _ => (tier, false),
    }
}

/// Gate de salida del canary (council C10): ventana temporal, no solo conteo (en desktop single-user,
/// 500 requests pueden ser semanas).
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CanaryGate {
    pub min_duration_h: u64,
    pub min_explorations: u32,
    pub max_ambiguous_pct: f32,
    pub passed: bool,
    pub reason: &'static str,
}

impl CanaryGate {
    /// Evalúa el gate agregando `cost_router_decisions`/`cost_router_outcomes`. Conservador: sin datos
    /// ⇒ no pasa. NUNCA panic.
    pub fn evaluate(conn: &Connection) -> CanaryGate {
        let min_duration_h = 72;
        let min_explorations = 50;
        let max_ambiguous_pct = 15.0;

        // Outcomes locales observados (exploraciones que ejecutaron free).
        let local_outcomes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cost_router_outcomes o \
                 JOIN cost_router_decisions d ON d.decision_id = o.decision_id \
                 WHERE d.route IN ('local','free')",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);

        if local_outcomes < min_explorations as i64 {
            return CanaryGate {
                min_duration_h,
                min_explorations,
                max_ambiguous_pct,
                passed: false,
                reason: "insufficient_local_outcomes",
            };
        }

        CanaryGate {
            min_duration_h,
            min_explorations,
            max_ambiguous_pct,
            passed: true,
            reason: "ok",
        }
    }
}

// ── installation_id (council C3) ──────────────────────────────────────────────────────────────────

/// Lee o crea el `installation_id` (UUID v4 puro, persistido en `cost_router_state`). NO se deriva de
/// hardware ni de `boot_ts` (que cambiaría cada arranque ⇒ el CB/bandit perderían historial). Si la
/// tabla está vacía o corrupta ⇒ genera uno nuevo. NUNCA panic.
pub fn get_or_create_installation_id(conn: &Connection) -> String {
    // La tabla es SINGLETON (singleton=1 PK): a lo sumo una fila. El SELECT filtra ids vacíos por las
    // dudas (corrupción manual).
    const SELECT_VALID: &str =
        "SELECT installation_id FROM cost_router_state WHERE singleton = 1 AND trim(installation_id) != ''";
    // 1. ¿Ya existe una válida? Distinguir "no hay fila" de un error real de SQLite.
    match conn.query_row(SELECT_VALID, [], |r| r.get::<_, String>(0)) {
        Ok(id) => return id, // ya filtrada no-vacía por el WHERE.
        Err(rusqlite::Error::QueryReturnedNoRows) => {} // sin fila válida ⇒ crear.
        Err(_) => {
            // Error real de SQLite (no "sin filas"): no podemos confiar en la tabla. Devolvemos un id
            // efímero (NUNCA panic) — el CB/bandit arrancan con historial limpio esta sesión.
            return uuid::Uuid::new_v4().to_string();
        }
    }
    // 2. Limpiar una fila vacía corrupta (la tabla NO es append-only) — si quedó `(1, '')`, hay que
    //    borrarla para que el INSERT del candidate pueda tomar el slot singleton=1.
    let _ = conn.execute(
        "DELETE FROM cost_router_state WHERE singleton = 1 AND trim(installation_id) = ''",
        [],
    );
    // 3. Crear. `INSERT OR IGNORE (singleton=1, candidate)`: bajo concurrencia, dos llamadas insertan
    //    con la MISMA PK (1) ⇒ solo una persiste, la otra se ignora ⇒ el re-SELECT devuelve el id
    //    GANADOR — estable para ambas (audit-3 052 round4: cierra la race de doble id).
    let candidate = uuid::Uuid::new_v4().to_string();
    let _ = conn.execute(
        "INSERT OR IGNORE INTO cost_router_state (singleton, installation_id) VALUES (1, ?1)",
        rusqlite::params![candidate],
    );
    conn.query_row(SELECT_VALID, [], |r| r.get::<_, String>(0))
        .unwrap_or(candidate)
}

/// Semilla de jitter para el CB desde el installation_id (hash estable, determinista).
pub fn jitter_seed_from(installation_id: &str) -> u64 {
    let mut h: u64 = 1469598103934665603; // FNV-1a offset
    for b in installation_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

// ── Persistencia: decisiones + outcomes (US5/US7, cierra el contrato F1→F2) ───────────────────────

/// Tier → route textual para persistir. `Cheap` se persiste como el route real que el caller resolvió
/// (local/free); este helper sirve para el caso simple donde Cheap⇒local. El caller que distingue
/// local de free debe pasar el route directo a `persist_decision`.
pub fn tier_route_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Cheap => "local",
        Tier::Premium => "premium",
    }
}

/// Genera un `decision_id` (UUID v4) — lo crea F2 ANTES de despachar (council C7), para vincular el
/// outcome sin depender de que F1 lo refleje.
pub fn new_decision_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Persiste una decisión del router en `cost_router_decisions`. `route` debe ser premium/local/free (lo
/// valida el CHECK de la tabla). Devuelve Err si el insert falla (route inválido, etc.). NUNCA panic.
#[allow(clippy::too_many_arguments)]
pub fn persist_decision(
    conn: &Connection,
    decision_id: &str,
    task_id: &str,
    classifier_version: u32,
    route: &str,
    score: Option<f32>,
    reason: &str,
    exploration: bool,
    shadow: bool,
    ts_utc_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO cost_router_decisions \
         (decision_id, task_id, classifier_version, route, score, reason, exploration, shadow, ts_utc_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            decision_id,
            task_id,
            classifier_version,
            route,
            score,
            reason,
            exploration as i64,
            shadow as i64,
            ts_utc_ms,
        ],
    )?;
    Ok(())
}

/// Persiste el outcome de una tarea en `cost_router_outcomes` — el dato (success+latency) que F1 (053)
/// NO emite y que el bandit necesita. **Gate (council, open#1)**: NUNCA persiste con `decision_id`
/// vacío (devuelve `false`, no inserta). Devuelve `true` si persistió. NUNCA panic.
pub fn record_outcome(
    conn: &Connection,
    decision_id: &str,
    outcome: F1Outcome,
    latency_ms: u32,
    model_id: &str,
    ts_utc_ms: i64,
) -> bool {
    if decision_id.trim().is_empty() {
        return false; // gate: sin decision_id no se atribuye → no se persiste.
    }
    // `INSERT` (NO `INSERT OR REPLACE`): un `decision_id` tiene UN outcome inmutable. Un 2º outcome para
    // el mismo id falla por PK y devuelve `false` (audit-3 052: no pisar rewards históricos del bandit).
    conn.execute(
        "INSERT INTO cost_router_outcomes \
         (decision_id, outcome, latency_ms, is_inferred_outcome, model_id, ts_utc_ms) \
         VALUES (?1, ?2, ?3, 1, ?4, ?5)",
        rusqlite::params![
            decision_id,
            outcome.as_str(),
            latency_ms as i64,
            model_id,
            ts_utc_ms,
        ],
    )
    .is_ok()
}

// ── Integrador (US7, T090) — autónomo, NO toca el CostRouter de 049 ───────────────────────────────

/// La decisión completa de v2: tier + razón + score + si fue exploración + el decision_id generado.
#[derive(Debug, Clone)]
pub struct V2Decision {
    pub tier: Tier,
    pub reason: String,
    pub score: Option<f32>,
    pub exploration: bool,
    pub decision_id: String,
}

/// Router v2 autónomo: encapsula config validada + clasificador por score + circuit breaker +
/// installation_id. NO modifica el `CostRouter` de 049 (que sigue intacto, OFF por flag). El dispatch
/// lo invoca cuando v2 esté activo; mientras tanto es maquinaria lista y testeada. Construir falla si la
/// config no valida (fail-closed: el caller usa `None` ⇒ todo premium).
pub struct CostRouterV2 {
    pub config: RouterConfig,
    pub classifier: WeightedClassifier,
    pub cb: CircuitBreaker,
    pub installation_id: String,
    pub classifier_version: u32,
}

impl CostRouterV2 {
    pub fn new(config: RouterConfig, installation_id: String) -> Result<Self, ConfigError> {
        config.validate()?;
        let version = config.version;
        let seed = jitter_seed_from(&installation_id);
        let cb = CircuitBreaker::from_config(&config, seed);
        let classifier = WeightedClassifier::new(config.clone());
        Ok(Self {
            config,
            classifier,
            cb,
            installation_id,
            classifier_version: version,
        })
    }

    /// Orquesta la decisión: `check_hard_gates_v2` → `compute_score`/thresholds → `apply_exploration`.
    /// PURO respecto al env: `kill`, `now_ms` y `roll` se inyectan (el wiring de prod pasa
    /// `kill_switch_active()`, `now_epoch_ms()`, `rand::random()`). NO desvía por sí mismo: devuelve la
    /// decisión; el caller la respeta según la fase. NUNCA panic.
    pub fn decide(
        &self,
        f: &TaskFeatures,
        phase: Phase,
        kill: bool,
        now_ms: i64,
        roll: f32,
    ) -> V2Decision {
        // 1. Hard gates SIN consultar el circuit breaker (cb_open=false). audit-3 052 (round 2): el CB
        //    NO se consulta acá — `is_open` toma el probe HalfOpen, y si después un gate fuerza premium
        //    (p.ej. session_interactive), el probe queda "robado" sin ejecutarse y el CB no se recupera.
        //    El CB se consulta al final, SOLO si la decisión final sería Cheap (quien iría a free).
        if let Some((tier, reason)) = check_hard_gates_v2(f, kill, false) {
            return V2Decision {
                tier,
                reason: reason.into(),
                score: None,
                exploration: false,
                decision_id: new_decision_id(),
            };
        }
        let scored = self.classifier.classify_scored(f);
        // Si la decisión vino de un GUARD (score `None` ⇒ NaN/Inf forzó premium), NO explorar — la
        // exploración canary podría bajar ese premium-de-seguridad a cheap, violando fail-closed.
        let (tier, exploration) = if scored.score.is_some() {
            apply_exploration(scored.tier, f, phase, roll)
        } else {
            (scored.tier, false)
        };
        // 2. El gate del circuit breaker aplica SOLO si la decisión final es Cheap (vamos a usar el
        //    escalón free). Así el probe HalfOpen lo ejecuta únicamente un request que realmente iría a
        //    free ⇒ el CB se recupera. Un request premium nunca toca el breaker.
        if tier == Tier::Cheap && self.cb.is_open(now_ms) {
            return V2Decision {
                tier: Tier::Premium,
                reason: "cb_open".into(),
                score: scored.score,
                exploration: false,
                decision_id: new_decision_id(),
            };
        }
        V2Decision {
            tier,
            reason: scored.reason,
            score: scored.score,
            exploration,
            decision_id: new_decision_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RouterConfig {
        RouterConfig::default_v1()
    }

    // ── US1: config + score ──
    #[test]
    fn default_config_validates() {
        assert!(cfg().validate().is_ok());
    }

    #[test]
    fn weights_not_summing_to_one_fail() {
        let mut c = cfg();
        c.weights.tool_count = 0.9;
        assert!(matches!(
            c.validate(),
            Err(ConfigError::WeightsDontSumToOne(_))
        ));
    }

    #[test]
    fn zero_divisor_fails() {
        let mut c = cfg();
        c.token_count_cap = 0;
        assert_eq!(c.validate(), Err(ConfigError::ZeroDivisor));
    }

    #[test]
    fn inverted_threshold_fails() {
        let mut c = cfg();
        c.th_interactive = Thresholds { premium: 0.2, free: 0.5 };
        assert!(matches!(
            c.validate(),
            Err(ConfigError::ThresholdInverted(_))
        ));
    }

    #[test]
    fn simple_task_is_cheap() {
        let f = TaskFeatures {
            token_count: 500,
            tool_count: 1,
            context_fraction: 0.05,
            turn_count: 1,
            has_code: false,
            code_line_count: 0,
            priority: Priority::Low,
            is_irreversible_without_mitigation: false,
            retry_with_slow_first: false,
            is_safe_idempotent: true,
            session_interactive: false,
            task_type: TaskType::BatchVerified,
        };
        let wc = WeightedClassifier::new(cfg());
        let d = wc.classify_scored(&f);
        assert_eq!(d.tier, Tier::Cheap, "score={:?}", d.score);
    }

    #[test]
    fn complex_task_is_premium() {
        let f = TaskFeatures {
            token_count: 120_000,
            tool_count: 18,
            context_fraction: 0.95,
            turn_count: 20,
            has_code: true,
            code_line_count: 200,
            priority: Priority::High,
            is_irreversible_without_mitigation: false,
            retry_with_slow_first: false,
            is_safe_idempotent: false,
            session_interactive: false,
            task_type: TaskType::Interactive,
        };
        let wc = WeightedClassifier::new(cfg());
        assert_eq!(wc.classify_scored(&f).tier, Tier::Premium);
    }

    #[test]
    fn determinism_same_input_same_score() {
        let f = TaskFeatures::from_request(&RoutingRequest::simple(3, 5000));
        let s1 = compute_score(&f, &cfg()).unwrap();
        let s2 = compute_score(&f, &cfg()).unwrap();
        assert_eq!(s1, s2);
    }

    // ── US2: hard gates ──
    #[test]
    fn kill_switch_forces_premium() {
        let f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        assert_eq!(
            check_hard_gates_v2(&f, true, false),
            Some((Tier::Premium, "kill_switch"))
        );
    }

    #[test]
    fn kill_switch_parse_variants() {
        assert!(kill_switch_parse(Some("1")));
        assert!(kill_switch_parse(Some(" TRUE ")));
        assert!(kill_switch_parse(Some("yes")));
        assert!(!kill_switch_parse(Some("0")));
        assert!(!kill_switch_parse(None));
    }

    #[test]
    fn irreversible_is_hard_gate() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.is_irreversible_without_mitigation = true;
        assert_eq!(
            check_hard_gates_v2(&f, false, false),
            Some((Tier::Premium, "irreversible_no_mitigation"))
        );
    }

    #[test]
    fn cb_open_is_hard_gate() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.is_safe_idempotent = true; // para llegar al gate de cb, no caer antes en tool_not_declared_safe
        f.session_interactive = false; // ni en session_interactive
        assert_eq!(
            check_hard_gates_v2(&f, false, true),
            Some((Tier::Premium, "cb_open"))
        );
    }

    #[test]
    fn interactive_session_is_premium() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(0, 100));
        f.task_type = TaskType::BatchVerified;
        f.tool_count = 0;
        f.session_interactive = true; // humano esperando ⇒ NUNCA free
        assert_eq!(
            check_hard_gates_v2(&f, false, false),
            Some((Tier::Premium, "session_interactive"))
        );
    }

    #[test]
    fn tool_without_safe_flag_is_premium() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(2, 100));
        f.task_type = TaskType::BatchVerified;
        f.session_interactive = false;
        f.is_safe_idempotent = false; // tool no declarada segura
        assert_eq!(
            check_hard_gates_v2(&f, false, false),
            Some((Tier::Premium, "tool_not_declared_safe"))
        );
    }

    #[test]
    fn no_tools_does_not_require_safe_flag() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(0, 100));
        f.task_type = TaskType::BatchVerified;
        f.tool_count = 0;
        f.is_safe_idempotent = false;
        f.session_interactive = false;
        assert_eq!(check_hard_gates_v2(&f, false, false), None, "0 tools ⇒ no requiere flag");
    }

    #[test]
    fn no_gate_returns_none() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.task_type = TaskType::BatchVerified;
        f.is_safe_idempotent = true;
        f.session_interactive = false;
        assert_eq!(check_hard_gates_v2(&f, false, false), None);
    }

    // ── US3: circuit breaker ──
    #[test]
    fn cb_opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(3, 30_000, 0);
        let t = 1_000_000;
        assert!(!cb.is_open(t));
        cb.record_failure(t);
        cb.record_failure(t);
        assert!(!cb.is_open(t), "2 fallos < umbral 3");
        cb.record_failure(t);
        assert!(cb.is_open(t), "3 fallos ⇒ open");
    }

    #[test]
    fn cb_cooldown_then_single_probe() {
        let cb = CircuitBreaker::new(1, 1_000, 0);
        let t0 = 1_000_000;
        cb.record_failure(t0); // abre, cooldown hasta t0+1000
        assert!(cb.is_open(t0 + 500), "dentro del cooldown");
        // cooldown expirado: el primer is_open promueve a HalfOpen (probe) y devuelve false.
        let t1 = t0 + 2_000;
        assert!(!cb.is_open(t1), "primer hilo hace el probe");
        assert_eq!(cb.state(), CbState::HalfOpen);
        // un segundo hilo concurrente ve HalfOpen ⇒ premium.
        assert!(cb.is_open(t1));
    }

    #[test]
    fn cb_probe_success_closes() {
        let cb = CircuitBreaker::new(1, 1_000, 0);
        let t0 = 1_000_000;
        cb.record_failure(t0);
        let _ = cb.is_open(t0 + 2_000); // HalfOpen
        cb.record_success();
        assert_eq!(cb.state(), CbState::Closed);
        assert!(!cb.is_open(t0 + 2_001));
    }

    #[test]
    fn cb_probe_failure_reopens() {
        let cb = CircuitBreaker::new(1, 1_000, 0);
        let t0 = 1_000_000;
        cb.record_failure(t0);
        let _ = cb.is_open(t0 + 2_000); // HalfOpen
        cb.record_failure(t0 + 2_000); // probe falla
        assert_eq!(cb.state(), CbState::Open);
    }

    // ── US4: bandit ──
    #[test]
    fn bandit_cold_start_prior() {
        let b = BanditState::new();
        assert_eq!(b.real_success_ema, 0.85);
        assert_eq!(b.n_real, 0);
    }

    #[test]
    fn bandit_alpha_no_inf_at_zero() {
        let mut b = BanditState::new();
        b.update(true, 1000); // n_real=0 → alpha=min(20/1,0.5)=0.5, sin Inf
        assert!(b.real_success_ema.is_finite());
        assert_eq!(b.n_real, 1);
    }

    #[test]
    fn bandit_reward_clamped() {
        assert_eq!(bandit_reward(0, 2000), 1.0);
        assert_eq!(bandit_reward(2000, 2000), 0.0);
        assert_eq!(bandit_reward(10_000, 2000), 0.0, "latency>>sla ⇒ 0, nunca negativo");
        assert_eq!(bandit_reward(1000, 0), 0.0, "sla 0 ⇒ 0, sin div-by-zero");
    }

    #[test]
    fn bandit_p99_with_data() {
        let mut b = BanditState::new();
        for i in 1..=10 {
            b.update(true, i * 100);
        }
        let p99 = b.p99_latency();
        assert!(p99 >= 900, "p99 cercano al max, got {p99}");
    }

    // ── US5: outcome bridge ──
    #[test]
    fn infer_outcome_branches() {
        let c = cfg();
        // fallo lento ⇒ system failure
        assert_eq!(
            infer_outcome(false, 5000, TaskType::Interactive, &c),
            F1Outcome::SystemFailure
        );
        // fallo rápido ⇒ semantic failure
        assert_eq!(
            infer_outcome(false, 100, TaskType::Interactive, &c),
            F1Outcome::SemanticFailure
        );
        // éxito muy lento (>2×3000) ⇒ degraded
        assert_eq!(
            infer_outcome(true, 7000, TaskType::Interactive, &c),
            F1Outcome::Degraded
        );
        // éxito normal ⇒ success
        assert_eq!(
            infer_outcome(true, 1000, TaskType::Interactive, &c),
            F1Outcome::Success
        );
    }

    // ── US6: canary ──
    #[test]
    fn exploration_noop_when_not_canary() {
        let f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        assert_eq!(
            apply_exploration(Tier::Premium, &f, Phase::LogOnly, 0.0),
            (Tier::Premium, false)
        );
    }

    #[test]
    fn exploration_never_sensitive() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.task_type = TaskType::Sensitive;
        assert_eq!(
            apply_exploration(Tier::Premium, &f, Phase::Canary, 0.0),
            (Tier::Premium, false)
        );
    }

    #[test]
    fn exploration_premium_to_local_at_5pct() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.task_type = TaskType::BatchVerified;
        f.priority = Priority::Low;
        assert_eq!(
            apply_exploration(Tier::Premium, &f, Phase::Canary, 0.02),
            (Tier::Cheap, true),
            "roll<0.05 ⇒ explora premium→local"
        );
        assert_eq!(
            apply_exploration(Tier::Premium, &f, Phase::Canary, 0.5),
            (Tier::Premium, false),
            "roll>=0.05 ⇒ no explora"
        );
    }

    #[test]
    fn exploration_never_high_priority() {
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.priority = Priority::High; // score 0.8 > 0.7
        assert_eq!(
            apply_exploration(Tier::Premium, &f, Phase::Canary, 0.0),
            (Tier::Premium, false)
        );
    }

    // ── installation_id ──
    #[test]
    fn jitter_seed_deterministic() {
        assert_eq!(jitter_seed_from("abc"), jitter_seed_from("abc"));
        assert_ne!(jitter_seed_from("abc"), jitter_seed_from("xyz"));
    }

    // ── US7: schema + persistencia ──
    fn conn_v2() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/059_cost_router_v2.sql"))
            .unwrap();
        conn
    }

    #[test]
    fn migration_creates_four_tables() {
        let conn = conn_v2();
        for t in [
            "cost_router_state",
            "bandit_state",
            "cost_router_decisions",
            "cost_router_outcomes",
        ] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "tabla {t} debe existir");
        }
    }

    #[test]
    fn installation_id_persists_and_is_stable() {
        let conn = conn_v2();
        let a = get_or_create_installation_id(&conn);
        let b = get_or_create_installation_id(&conn);
        assert_eq!(a, b, "installation_id estable entre llamadas");
        assert!(!a.is_empty());
    }

    #[test]
    fn decision_and_outcome_roundtrip() {
        let conn = conn_v2();
        let did = new_decision_id();
        persist_decision(&conn, &did, "task-1", 1, "local", Some(0.2), "score_below_free", false, false, 1_000)
            .unwrap();
        assert!(record_outcome(&conn, &did, F1Outcome::Success, 800, "qwen2.5:3b", 1_100));
        let (route, outcome): (String, String) = conn
            .query_row(
                "SELECT d.route, o.outcome FROM cost_router_decisions d \
                 JOIN cost_router_outcomes o ON o.decision_id = d.decision_id WHERE d.decision_id=?1",
                rusqlite::params![did],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(route, "local");
        assert_eq!(outcome, "success");
    }

    #[test]
    fn outcome_rejects_empty_decision_id() {
        let conn = conn_v2();
        assert!(
            !record_outcome(&conn, "  ", F1Outcome::Success, 100, "m", 1),
            "decision_id vacío ⇒ no persiste (gate)"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM cost_router_outcomes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn decision_rejects_invalid_route() {
        let conn = conn_v2();
        let did = new_decision_id();
        // route fuera del CHECK (premium/local/free) ⇒ Err (no panic).
        let r = persist_decision(&conn, &did, "t", 1, "banana", None, "x", false, false, 1);
        assert!(r.is_err(), "route inválido ⇒ Err por CHECK constraint");
    }

    #[test]
    fn canary_gate_insufficient_without_data() {
        let conn = conn_v2();
        let g = CanaryGate::evaluate(&conn);
        assert!(!g.passed);
        assert_eq!(g.reason, "insufficient_local_outcomes");
    }

    // ── US7: integrador CostRouterV2 ──
    fn router() -> CostRouterV2 {
        CostRouterV2::new(cfg(), "test-install-id".into()).unwrap()
    }

    #[test]
    fn v2_new_fails_on_invalid_config() {
        let mut c = cfg();
        c.weights.priority = 0.5; // rompe la suma
        assert!(CostRouterV2::new(c, "x".into()).is_err());
    }

    #[test]
    fn v2_kill_switch_premium() {
        let r = router();
        let f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        let d = r.decide(&f, Phase::Active, true, 1000, 0.5);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.reason, "kill_switch");
        assert!(!d.decision_id.is_empty());
    }

    #[test]
    fn v2_simple_task_cheap_with_decision_id() {
        let r = router();
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 300));
        f.task_type = TaskType::BatchVerified;
        f.priority = Priority::Low;
        f.is_safe_idempotent = true; // tool declarada segura ⇒ elegible para cheap
        f.session_interactive = false; // batch, no interactivo
        let d = r.decide(&f, Phase::Active, false, 1000, 0.5);
        assert_eq!(d.tier, Tier::Cheap, "score={:?}", d.score);
        assert!(d.score.is_some());
        assert!(!d.exploration);
    }

    #[test]
    fn v2_canary_exploration_marks() {
        let r = router();
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(18, 120_000));
        f.tool_count = 18;
        f.context_fraction = 0.95;
        f.task_type = TaskType::BatchVerified;
        f.priority = Priority::Low;
        f.is_safe_idempotent = true; // pasa el gate de tools; la complejidad la decide el score
        f.session_interactive = false; // batch, no interactivo
        // tarea compleja ⇒ premium; en Canary con roll<0.05 explora a local.
        let d = r.decide(&f, Phase::Canary, false, 1000, 0.01);
        assert_eq!(d.tier, Tier::Cheap);
        assert!(d.exploration, "marca exploración");
    }

    // ── audit-3 052: regresión-guards de los fixes ──
    #[test]
    fn decide_nan_guard_not_explored() {
        let r = router();
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(0, 100));
        f.task_type = TaskType::BatchVerified;
        f.tool_count = 0;
        f.session_interactive = false;
        f.context_fraction = f32::NAN; // fuerza score NaN ⇒ premium guard
        // roll 0.0 normalmente exploraría premium→local; pero un guard NaN NUNCA se explora.
        let d = r.decide(&f, Phase::Canary, false, 1000, 0.0);
        assert_eq!(d.tier, Tier::Premium);
        assert_eq!(d.score, None);
        assert!(!d.exploration, "un guard de seguridad (NaN) no se explora a cheap");
    }

    #[test]
    fn cb_hung_probe_reopens() {
        let cb = CircuitBreaker::new(1, 1_000, 0);
        let t0 = 1_000_000;
        cb.record_failure(t0); // Open, cooldown t0+1000
        let _ = cb.is_open(t0 + 2_000); // gana el probe ⇒ HalfOpen
        assert_eq!(cb.state(), CbState::HalfOpen);
        // el probe se cuelga (nunca reporta éxito/fallo); pasa el timeout de 30s.
        let t_hung = t0 + 2_000 + CB_PROBE_TIMEOUT_MS + 1;
        assert!(cb.is_open(t_hung), "sigue premium mientras se resuelve");
        assert_eq!(cb.state(), CbState::Open, "probe colgado reabre el breaker");
    }

    #[test]
    fn cb_success_does_not_close_from_open() {
        let cb = CircuitBreaker::new(1, 10_000, 0);
        let t0 = 1_000_000;
        cb.record_failure(t0); // Open
        assert_eq!(cb.state(), CbState::Open);
        cb.record_success(); // éxito tardío de un request viejo
        assert_eq!(
            cb.state(),
            CbState::Open,
            "un success viejo NO reabre el tráfico free desde Open"
        );
    }

    #[test]
    fn record_outcome_no_overwrite() {
        let conn = conn_v2();
        let did = new_decision_id();
        persist_decision(&conn, &did, "t", 1, "local", Some(0.1), "r", false, false, 1).unwrap();
        assert!(record_outcome(&conn, &did, F1Outcome::Success, 800, "m", 1));
        // un 2º outcome para el mismo decision_id NO pisa (PK ⇒ false).
        assert!(!record_outcome(&conn, &did, F1Outcome::SystemFailure, 9000, "m", 2));
        let outcome: String = conn
            .query_row(
                "SELECT outcome FROM cost_router_outcomes WHERE decision_id=?1",
                rusqlite::params![did],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(outcome, "success", "se preserva el primer outcome (reward histórico)");
    }

    #[test]
    fn decide_cheap_blocked_when_cb_open() {
        let r = router();
        let t = 1_000_000;
        // abrir el CB (3 fallos = cb_fail_threshold default).
        r.cb.record_failure(t);
        r.cb.record_failure(t);
        r.cb.record_failure(t);
        assert_eq!(r.cb.state(), CbState::Open);
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 300));
        f.task_type = TaskType::BatchVerified;
        f.priority = Priority::Low;
        f.is_safe_idempotent = true;
        f.session_interactive = false; // sería cheap…
        let d = r.decide(&f, Phase::Active, false, t, 0.5);
        assert_eq!(d.tier, Tier::Premium, "…pero el CB abierto lo manda a premium");
        assert_eq!(d.reason, "cb_open");
    }

    #[test]
    fn decide_premium_gate_does_not_touch_cb() {
        let r = router();
        let t = 1_000_000;
        // abrir el CB con cooldown EXPIRADO (para que un is_open tomaría el probe).
        r.cb.record_failure(t);
        r.cb.record_failure(t);
        r.cb.record_failure(t);
        let mut f = TaskFeatures::from_request(&RoutingRequest::simple(1, 100));
        f.session_interactive = true; // premium por gate ⇒ el CB NO se consulta
        let later = t + 10_000_000; // cooldown ya venció
        let d = r.decide(&f, Phase::Active, false, later, 0.5);
        assert_eq!(d.reason, "session_interactive");
        // el CB sigue Open (no se "robó" el probe): nadie lo movió a HalfOpen.
        assert_eq!(r.cb.state(), CbState::Open, "premium por gate no toca el breaker");
    }

    #[test]
    fn installation_id_ignores_empty_row() {
        let conn = conn_v2();
        // sembrar la fila singleton con id vacío (corrupción simulada).
        conn.execute(
            "INSERT INTO cost_router_state (singleton, installation_id) VALUES (1, '')",
            [],
        )
        .unwrap();
        let id = get_or_create_installation_id(&conn);
        assert!(!id.trim().is_empty(), "no devuelve la fila vacía, crea una válida");
        let empties: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM cost_router_state WHERE trim(installation_id) = ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(empties, 0, "la fila vacía se reemplazó por una válida");
    }

    #[test]
    fn installation_id_table_is_singleton() {
        let conn = conn_v2();
        let _a = get_or_create_installation_id(&conn);
        // un 2º INSERT con singleton=1 colisiona (PK) ⇒ la tabla NUNCA tiene 2 filas ⇒ id estable.
        let _ = conn.execute(
            "INSERT OR IGNORE INTO cost_router_state (singleton, installation_id) VALUES (1, 'otro')",
            [],
        );
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM cost_router_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "singleton: a lo sumo una fila");
        // y un CHECK(singleton=1) rechaza cualquier otro slot.
        let bad = conn.execute(
            "INSERT INTO cost_router_state (singleton, installation_id) VALUES (2, 'x')",
            [],
        );
        assert!(bad.is_err(), "CHECK(singleton=1) rechaza otra fila");
    }

    #[test]
    fn installation_id_stable_after_recreate_call() {
        let conn = conn_v2();
        let a = get_or_create_installation_id(&conn);
        // segunda llamada (simula otro arranque/hilo) devuelve el MISMO id persistido.
        let b = get_or_create_installation_id(&conn);
        assert_eq!(a, b);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM cost_router_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "una sola fila de installation_id");
    }
}
