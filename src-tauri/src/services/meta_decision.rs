// services/meta_decision.rs — 020-aie-meta-orchestrator.
//
// El orquestador local (Tauri/Rust) usa el AIE free ($0, Tailscale) para REFINAR
// meta-decisiones —empezando por done-detection (US1, MVP)— cuando la heurística regex
// (`done_detection::classify`) es ambigua. El AIE es SIEMPRE *advisory*:
//
//   - Opt-in OFF por default (setting `orchestration.use_aie_for_meta`). Con OFF nunca se
//     consulta el AIE → comportamiento idéntico al actual (cero regresión, SC-001).
//   - El AIE NUNCA bloquea ni rompe el orquestador. TODO modo de fallo
//     (OFF / inalcanzable / timeout 3s / HTTP error / parse-fail / verdict inválido /
//     sanitizer-falla) ⇒ `None` ⇒ el consumidor usa la heurística regex actual (FR-003, SC-003).
//   - Sanitizer fail-closed: TODO payload pasa por `cloud_sanitizer::sanitize` ANTES de salir
//     y ANTES de calcular el cache-key (FR-004, SC-004, research §4). Si redacta algo, igual se
//     envía el texto ya redactado (el secreto NO sale); el "fail-closed" duro aplica si el
//     sanitizer mismo paniquea → se atrapa y devuelve `None`.
//   - BYOK puro (F-I): el AIE es free, NO usa API keys del user. Bearer del Keychain
//     (`aie-internal-bearer`), nunca hardcodeado ni logueado.
//   - El verdict del AIE es advisory: NUNCA auto-confirma acciones destructivas; la política de
//     auto-confirm de 012 (done_detection) manda (research §1).
//
// US2 (`rank_variants`) y US3 (`classify_task`) están implementados+testeados en el engine pero
// NO cableados en la UI en v1 (research: alcance) — YAGNI sobre el cableado, no sobre el contrato.

use async_trait::async_trait;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::services::cloud_sanitizer;

/// Resultado de una meta-decisión de done-detection. Espejo del `done_detection::Verdict`
/// (mantener `from_done`/`to_done` alineados); separado para que el contrato del engine no
/// dependa del módulo consumidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// El agente sigue trabajando.
    Running,
    /// Terminó / prompt vacío → idle (awaiting_review).
    Idle,
    /// Pide una decisión humana (trust / permiso / confirmación).
    NeedsInput,
}

impl Verdict {
    /// Mapea a una keyword del enum (parse ESTRICTO del texto del AIE).
    /// Tras `trim()`, acepta SOLO un único token EXACTO del conjunto válido (case-insensitive).
    /// Prosa, substrings ("maybe IDLE"), o múltiples palabras ("not QUESTION") ⇒ `None`
    /// (→ caller hace fallback, research §1). El prompt instruye "EXACTLY ONE WORD", así que
    /// cualquier respuesta que no sea exactamente una de las keywords es un parse-fail.
    pub fn parse_keyword(raw: &str) -> Option<Verdict> {
        let tok = raw.trim();
        // Un único token: no whitespace interno. "maybe IDLE", "not QUESTION" ⇒ None.
        if tok.is_empty() || tok.chars().any(|c| c.is_whitespace()) {
            return None;
        }
        let up = tok.to_uppercase();
        match up.as_str() {
            "QUESTION" | "NEEDS_INPUT" | "NEEDSINPUT" => Some(Verdict::NeedsInput),
            "WORKING" | "RUNNING" | "IN_PROGRESS" => Some(Verdict::Running),
            // Success/Done/Complete y Failed/Failure → la tarea ya no trabaja; el poller la lleva
            // a awaiting_review (revisión humana) — NUNCA auto-merge ni auto-fail (constitución VI,
            // research §1).
            "IDLE" | "SUCCESS" | "DONE" | "COMPLETE" | "FAILED" | "FAILURE" => Some(Verdict::Idle),
            _ => None,
        }
    }
}

/// Abstracción de las meta-decisiones del orquestador. TODOS los métodos devuelven `Option`:
/// `None` ⇒ usar la heurística autoritativa preexistente (research §1). Implementado por
/// `AieMetaDecision` (HTTP al AIE) y `HeuristicFallback` (siempre `None`, para tests / feature OFF).
#[async_trait]
pub trait MetaDecisionEngine: Send + Sync {
    /// US1 — clasifica el estado del buffer de un pane. `cli` es el CLI que corre (claude/codex/…)
    /// para dar contexto. `None` ⇒ fallback a la regex.
    async fn classify_done(&self, buffer_tail: &str, cli: &str) -> Option<Verdict>;

    /// US2 (diferido en UI) — rankea variantes best-of-N por calidad de diff. Devuelve los
    /// índices ordenados de mejor a peor. `None` ⇒ sin sugerencia (picker manual).
    async fn rank_variants(&self, objective: &str, diffs: &[String]) -> Option<Vec<usize>>;

    /// US3 (diferido en UI) — clasifica el objetivo (bugfix/feature/refactor/…). `None` ⇒ sin
    /// sugerencia (elección manual de agente).
    async fn classify_task(&self, objective: &str) -> Option<String>;
}

// ── HeuristicFallback ──────────────────────────────────────────────────────────
//
// No-op: siempre `None` ⇒ el consumidor cae a la heurística. Se inyecta cuando el feature
// está OFF y en tests que verifican el path de fallback (SC-001/SC-003).

#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicFallback;

#[async_trait]
impl MetaDecisionEngine for HeuristicFallback {
    async fn classify_done(&self, _buffer_tail: &str, _cli: &str) -> Option<Verdict> {
        None
    }
    async fn rank_variants(&self, _objective: &str, _diffs: &[String]) -> Option<Vec<usize>> {
        None
    }
    async fn classify_task(&self, _objective: &str) -> Option<String> {
        None
    }
}

// ── Transport (inyectable) ──────────────────────────────────────────────────────
//
// El POST HTTP real al AIE se factoriza detrás de un trait para que los tests verifiquen el
// sanitizer / cache / parse SIN tocar la red (mock del transporte). El AieMetaDecision real usa
// `ReqwestTransport`.

/// Una petición ya sanitizada y lista para enviar. `prompt` y `system` NUNCA contienen secretos
/// (pasaron por el sanitizer). El transporte agrega el bearer (que NO vive acá).
#[derive(Debug, Clone)]
pub struct MetaRequest {
    pub base_url: String,
    pub profile: &'static str,
    pub system: String,
    pub prompt: String,
    pub max_tokens: u32,
}

/// Transporte: hace el POST y devuelve el texto de respuesta del modelo. `None` en CUALQUIER
/// fallo (conexión / status no-2xx / parse) — el engine lo mapea a fallback. NUNCA propaga `Err`.
#[async_trait]
pub trait MetaTransport: Send + Sync {
    async fn post(&self, req: &MetaRequest, bearer: &str, timeout: Duration) -> Option<String>;
}

/// Transporte real vía reqwest contra `{base}/v1/infer` (mismo shape que council_multi /
/// done_detection::llm_verdict). Bearer en `Authorization`, `Accept: application/json`.
pub struct ReqwestTransport;

#[async_trait]
impl MetaTransport for ReqwestTransport {
    async fn post(&self, req: &MetaRequest, bearer: &str, timeout: Duration) -> Option<String> {
        let url = format!("{}/v1/infer", req.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "profile": req.profile,
            "system": req.system,
            "prompt": req.prompt,
            "max_tokens": req.max_tokens,
        });
        // `?` sobre Result→Option vía `.ok()?` — NUNCA unwrap en path de red (constitución/clippy).
        let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
        let resp = client
            .post(&url)
            .bearer_auth(bearer)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
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
            .unwrap_or("")
            .to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

// ── Cache ──────────────────────────────────────────────────────────────────────
//
// LRU in-memory: key = blake3(profile + ":" + sanitized_input). Se cachea el input SANITIZADO
// (no el crudo) para dedup correcto sin re-leakear (research §2). TTL corto (15s done-detection).
// No se persiste (sin riesgo de integridad cross-sesión, F-07).

const CACHE_CAP: usize = 256;
const DONE_TTL: Duration = Duration::from_secs(15);

/// Cap del LRU como `NonZeroUsize` INFALIBLE: nunca paniquea. `CACHE_CAP` es una constante > 0,
/// pero usamos `unwrap_or(MIN)` (en vez de `expect`) para que un futuro edit a 0 degrade a un
/// cache de 1 entrada en lugar de tumbar el proceso (constitución: el orquestador no paniquea).
const CACHE_NZ: NonZeroUsize = match NonZeroUsize::new(CACHE_CAP) {
    Some(n) => n,
    None => NonZeroUsize::MIN,
};

struct VerdictCache {
    inner: Mutex<LruCache<[u8; 32], (Verdict, Instant)>>,
    /// Keys con un POST en vuelo (singleflight). Evita que dos pollers concurrentes con la MISMA
    /// key dupliquen el POST al AIE y que un store más viejo pise uno más nuevo (audit ronda 2,
    /// codex #4). Lock independiente del `inner` para no acoplar el lookup con la reserva.
    inflight: Mutex<std::collections::HashSet<[u8; 32]>>,
}

impl VerdictCache {
    fn new() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(CACHE_NZ)),
            inflight: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Reserva la key para un POST (singleflight best-effort). `Some(guard)` ⇒ esta llamada es la
    /// dueña del POST; `None` ⇒ ya hay otro POST en vuelo para la misma key (el caller cae a la
    /// heurística ese tick — el dueño cacheará el verdict). El guard libera la reserva al Drop,
    /// incluido early-return o error del POST.
    fn try_begin(self: &Arc<Self>, key: [u8; 32]) -> Option<InFlightGuard> {
        let mut g = self.inflight.lock().ok()?;
        if g.contains(&key) {
            return None;
        }
        g.insert(key);
        Some(InFlightGuard {
            cache: Arc::clone(self),
            key,
        })
    }

    fn key(profile: &str, sanitized_input: &str) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(profile.as_bytes());
        hasher.update(b":");
        hasher.update(sanitized_input.as_bytes());
        *hasher.finalize().as_bytes()
    }

    /// Lookup ATÓMICO bajo un único lock: devuelve el verdict cacheado si existe y no venció el
    /// TTL. Vencido ⇒ lo elimina (evita servir stale en un get posterior) y devuelve `None`.
    /// Unificar el get bajo el mismo lock que el put (`store`) elimina la carrera get-luego-put.
    fn get(&self, key: &[u8; 32], ttl: Duration) -> Option<Verdict> {
        let mut guard = self.inner.lock().ok()?;
        match guard.get(key) {
            Some((verdict, stored_at)) if stored_at.elapsed() < ttl => Some(*verdict),
            Some(_) => {
                // entrada vencida → purgar bajo el mismo lock.
                guard.pop(key);
                None
            }
            None => None,
        }
    }

    /// Inserta/actualiza un verdict bajo el mismo lock que `get`.
    fn store(&self, key: [u8; 32], verdict: Verdict) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.put(key, (verdict, Instant::now()));
        }
    }
}

/// RAII: libera la reserva singleflight de `try_begin` al salir del scope (éxito, early-return o
/// error del POST). Garantiza que la key nunca queda "pegada" como in-flight.
struct InFlightGuard {
    cache: Arc<VerdictCache>,
    key: [u8; 32],
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.cache.inflight.lock() {
            g.remove(&self.key);
        }
    }
}

/// Cache LRU long-lived COMPARTIDO entre todas las instancias de `AieMetaDecision::new`.
/// Sin esto, cada `refine_verdict_with_aie` instanciaba un engine nuevo por tick → su cache de
/// instancia era SIEMPRE miss en producción (re-consultaba el AIE cada vez). Con el static, el
/// cache persiste entre ticks del poller (research §2: TTL 15s, dedup por input sanitizado).
static SHARED_CACHE: std::sync::OnceLock<Arc<VerdictCache>> = std::sync::OnceLock::new();

fn shared_cache() -> Arc<VerdictCache> {
    SHARED_CACHE
        .get_or_init(|| Arc::new(VerdictCache::new()))
        .clone()
}

// ── AuditSink (inyectable) ──────────────────────────────────────────────────────
//
// El audit se factoriza detrás de un trait pequeño para no acoplar el engine a un
// `AuditWriter` real en los unit tests (que no tienen tabla `events`). En producción se inyecta
// un `AuditWriterSink` que escribe a `bases/audit.rs` (append-only, research §5). Registra
// ACTIVIDAD (tipo, profile, verdict/None, latencia, cache_hit) — NUNCA el contenido del buffer.

pub trait MetaAudit: Send + Sync {
    fn record(
        &self,
        kind: &str,
        profile: &str,
        verdict: Option<Verdict>,
        latency_ms: u64,
        cache_hit: bool,
    );
}

/// Sink no-op (tests / cuando no hay audit writer disponible).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopAudit;
impl MetaAudit for NoopAudit {
    fn record(
        &self,
        _kind: &str,
        _profile: &str,
        _verdict: Option<Verdict>,
        _latency_ms: u64,
        _cache_hit: bool,
    ) {
    }
}

/// Sink real: escribe un evento append-only vía `bases/audit.rs`. SIN contenido del buffer.
pub struct AuditWriterSink {
    pub writer: crate::bases::audit::AuditWriter,
}

impl MetaAudit for AuditWriterSink {
    fn record(
        &self,
        kind: &str,
        profile: &str,
        verdict: Option<Verdict>,
        latency_ms: u64,
        cache_hit: bool,
    ) {
        let verdict_str = match verdict {
            Some(Verdict::Running) => "running",
            Some(Verdict::Idle) => "idle",
            Some(Verdict::NeedsInput) => "needs_input",
            None => "fallback",
        };
        let _ = self.writer.write(crate::bases::audit::EventInput {
            kind: "orch.meta_decision",
            actor: "meta_decision:aie",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({
                "decision": kind,           // done_detection | rank | classify
                "profile": profile,
                "verdict": verdict_str,
                "latency_ms": latency_ms,
                "cache_hit": cache_hit,
            }),
        });
    }
}

// ── AieMetaDecision ──────────────────────────────────────────────────────────────

const DONE_PROFILE: &str = "fast_small_free";
const BULK_PROFILE: &str = "bulk_free";
const DONE_MAX_TOKENS: u32 = 16;

const DONE_SYSTEM: &str = "You classify the terminal UI state of an autonomous coding agent. \
Reply with EXACTLY ONE WORD from: WORKING, IDLE, QUESTION. \
WORKING = the agent is still actively working (spinner, streaming, thinking). \
IDLE = the agent finished its task and is idle at an empty prompt (or reported an error and stopped). \
QUESTION = the agent is asking the user a permission/confirmation/trust question.";

/// Implementación real del engine: AIE free vía Tailscale. Sanitize → allowlist → bearer → POST
/// con timeout → parse defensivo → cache → audit. NUNCA propaga `Err`; todo fallo ⇒ `None`.
pub struct AieMetaDecision {
    base_url: String,
    bearer: Option<String>,
    timeout: Duration,
    transport: Arc<dyn MetaTransport>,
    cache: Arc<VerdictCache>,
    audit: Arc<dyn MetaAudit>,
}

impl AieMetaDecision {
    /// Construye con el transporte real (reqwest) y un audit sink. `base_url` y `bearer` los
    /// resuelve el caller (vía `aie_endpoint::resolve_url` + Keychain) — este módulo NO los
    /// hardcodea ni los loguea. El cache es el `SHARED_CACHE` long-lived: aunque el poller
    /// instancie un engine nuevo por tick, el cache PERSISTE entre ticks (research §2).
    pub fn new(base_url: String, bearer: Option<String>, audit: Arc<dyn MetaAudit>) -> Self {
        Self {
            base_url,
            bearer,
            timeout: Duration::from_secs(3),
            transport: Arc::new(ReqwestTransport),
            cache: shared_cache(),
            audit,
        }
    }

    /// Variante para tests: transporte inyectable (mock) + cache FRESCO por engine (aislamiento
    /// entre tests; no toca el `SHARED_CACHE` global).
    #[cfg(test)]
    fn with_transport(
        base_url: String,
        bearer: Option<String>,
        transport: Arc<dyn MetaTransport>,
        audit: Arc<dyn MetaAudit>,
    ) -> Self {
        Self {
            base_url,
            bearer,
            timeout: Duration::from_secs(3),
            transport,
            cache: Arc::new(VerdictCache::new()),
            audit,
        }
    }

    /// Sanitiza fail-closed: atrapa un pánico del sanitizer (defensa en profundidad, research §4)
    /// y devuelve `None` si revienta → el caller cae a la heurística sin enviar nada.
    /// Loguea SOLO el TIPO de fallo (nunca el buffer ni su contenido) para troubleshooting.
    fn sanitize_failclosed(input: &str) -> Option<String> {
        let owned = input.to_string();
        let res = std::panic::catch_unwind(move || {
            let (out, _report) = cloud_sanitizer::sanitize(&owned);
            out
        });
        if res.is_err() {
            tracing::warn!("meta_decision: sanitizer panicked — fail-closed a la heurística");
        }
        res.ok()
    }
}

#[async_trait]
impl MetaDecisionEngine for AieMetaDecision {
    async fn classify_done(&self, buffer_tail: &str, cli: &str) -> Option<Verdict> {
        let start = Instant::now();

        // 1. Endpoint allowlist (SSRF defense — el bearer NO sale a un host no permitido).
        if !crate::bases::allowlist::url_allowed(&self.base_url) {
            tracing::debug!("meta_decision: AIE endpoint fuera de allowlist");
            return None;
        }
        // 2. Bearer (BYOK-clean; sin bearer no se consulta).
        let bearer = self.bearer.as_deref()?;

        // 3. Sanitizar ANTES de armar el request y ANTES del cache-key (fail-closed).
        let sanitized = Self::sanitize_failclosed(buffer_tail)?;

        // 4. Cache lookup (key sobre el input SANITIZADO).
        let key = VerdictCache::key(DONE_PROFILE, &sanitized);
        if let Some(v) = self.cache.get(&key, DONE_TTL) {
            self.audit.record(
                "done_detection",
                DONE_PROFILE,
                Some(v),
                start.elapsed().as_millis() as u64,
                true,
            );
            return Some(v);
        }

        // 4b. Singleflight: si otro poller ya tiene un POST en vuelo para esta key, NO duplicar —
        // caer a la heurística este tick (el dueño cacheará el verdict, y un re-check posterior
        // hace cache-hit). Resuelve la race get-miss→POST→store del audit ronda 2 (codex #4): solo
        // un POST por key concurrente, y solo el dueño escribe (sin pisado viejo↦nuevo).
        let _flight = self.cache.try_begin(key)?;

        // 5. POST con timeout (el transporte ya es no-bloqueante / bounded). El `cli` da contexto.
        let cli_clean = cli
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(24)
            .collect::<String>();
        let req = MetaRequest {
            base_url: self.base_url.clone(),
            profile: DONE_PROFILE,
            system: DONE_SYSTEM.to_string(),
            prompt: format!("CLI: {cli_clean}\nTerminal tail:\n```\n{sanitized}\n```\nOne word:"),
            max_tokens: DONE_MAX_TOKENS,
        };
        let text = match self.transport.post(&req, bearer, self.timeout).await {
            Some(t) => t,
            None => {
                // Transporte devolvió None = conexión / timeout / status no-2xx / body vacío.
                // No distinguimos el subtipo acá (el transporte ya tragó el Err); logueamos el
                // tipo de fallo, NUNCA el buffer ni el bearer.
                tracing::debug!(
                    "meta_decision: AIE unreachable/timeout/non-2xx — fallback heurística"
                );
                self.audit.record(
                    "done_detection",
                    DONE_PROFILE,
                    None,
                    start.elapsed().as_millis() as u64,
                    false,
                );
                return None;
            }
        };

        // 6. Parse defensivo ESTRICTO: verdict fuera del enum esperado ⇒ None.
        let verdict = Verdict::parse_keyword(&text);
        if let Some(v) = verdict {
            self.cache.store(key, v);
        } else {
            // El AIE respondió pero no fue una keyword exacta → parse-fail (logueamos el tipo,
            // NO el texto de respuesta del modelo).
            tracing::debug!(
                "meta_decision: AIE reply no parseable a verdict — fallback heurística"
            );
        }
        self.audit.record(
            "done_detection",
            DONE_PROFILE,
            verdict,
            start.elapsed().as_millis() as u64,
            false,
        );
        verdict
    }

    async fn rank_variants(&self, objective: &str, diffs: &[String]) -> Option<Vec<usize>> {
        // US2 (engine implementado, sin cablear en UI v1). Sanitiza objetivo + diffs fail-closed.
        if diffs.is_empty() {
            return None;
        }
        if !crate::bases::allowlist::url_allowed(&self.base_url) {
            return None;
        }
        let bearer = self.bearer.as_deref()?;
        let obj = Self::sanitize_failclosed(objective)?;
        let mut blocks = String::new();
        for (i, d) in diffs.iter().enumerate() {
            let san = Self::sanitize_failclosed(d)?;
            // Cap por diff para no inflar el payload (bounded).
            let capped: String = san.chars().take(4000).collect();
            blocks.push_str(&format!("### Variant {i}\n```\n{capped}\n```\n"));
        }
        let n = diffs.len();
        let req = MetaRequest {
            base_url: self.base_url.clone(),
            profile: BULK_PROFILE,
            system: format!(
                "You rank {n} candidate code diffs by quality for the given objective. \
                 Reply ONLY with the variant indices (0-based) from best to worst, \
                 comma-separated (e.g. `2,0,1`). No prose."
            ),
            prompt: format!("Objective:\n{obj}\n\n{blocks}\nRanking:"),
            max_tokens: 64,
        };
        let text = self.transport.post(&req, bearer, self.timeout).await?;
        let ranking = parse_ranking(&text, n)?;
        Some(ranking)
    }

    async fn classify_task(&self, objective: &str) -> Option<String> {
        // US3 (engine implementado, sin cablear en UI v1).
        if !crate::bases::allowlist::url_allowed(&self.base_url) {
            return None;
        }
        let bearer = self.bearer.as_deref()?;
        let obj = Self::sanitize_failclosed(objective)?;
        let req = MetaRequest {
            base_url: self.base_url.clone(),
            profile: DONE_PROFILE,
            system: "Classify the software task into EXACTLY ONE word from: bugfix, feature, refactor, docs, test, chore. Reply with the single word only.".to_string(),
            prompt: format!("Task:\n{obj}\n\nCategory:"),
            max_tokens: 8,
        };
        let text = self.transport.post(&req, bearer, self.timeout).await?;
        parse_task_category(&text)
    }
}

// ── LocalMetaDecision (036) ───────────────────────────────────────────────────────
//
// Espejo de `AieMetaDecision` PERO 100% LOCAL: postea a un Ollama loopback
// (`http://127.0.0.1:11434`, OpenAI-compat `/v1/chat/completions`) SIN bearer. Es el P0 del
// consejo Goose-C: el meta-orquestador corre offline/local-first (BYOK puro) sin red externa ni
// Keychain. Reusa el sanitizer fail-closed, el `VerdictCache`, el singleflight, el parse defensivo
// y el `MetaAudit` — la ÚNICA superficie nueva es el transporte (sin Authorization) y un gate de
// allowlist ESTRICTAMENTE loopback (FR-007: el flag local NO es un bypass del gate anti-SSRF).

/// Allowlist loopback-only para el motor LOCAL (FR-007). MÁS estricto que `allowlist::url_allowed`
/// (que también acepta el Tailscale de the dev server y los sufijos `*.example.internal`/`example.test`): el motor
/// local SÓLO puede pegar a loopback (`127.0.0.0/8`, `::1`, `localhost`). Cualquier otra IP/host
/// —incluso una IP interna que `url_allowed` aceptaría— se RECHAZA aquí. Así, activar `local_engine`
/// nunca abre el gate SSRF a un host arbitrario. El puerto es libre dentro de loopback (Ollama por
/// default 11434, pero el usuario puede correrlo en otro puerto loopback).
fn loopback_allowed(input: &str) -> bool {
    let Ok(u) = url::Url::parse(input) else {
        return false;
    };
    // Sólo http/https (sin file://, ftp://, etc.).
    match u.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    let Some(host) = u.host() else {
        return false;
    };
    match host {
        // `localhost` literal (resuelve a loopback en toda plataforma sana).
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        // 127.0.0.0/8 entero (no sólo 127.0.0.1) — todo loopback IPv4.
        url::Host::Ipv4(ip) => ip.is_loopback(),
        // ::1 loopback IPv6.
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// Transporte real para Ollama: POST a `{base}/v1/chat/completions` (OpenAI-compat) SIN
/// `Authorization` (Ollama no usa bearer). Lleva el `model` que va en el body.
///
/// **Bearer IGNORADO a propósito (decisión de diseño, no olvido):** el trait `MetaTransport`
/// comparte la firma `post(req, bearer, timeout)` con el transporte del AIE (`ReqwestTransport`),
/// pero el motor local SIEMPRE invoca con `bearer = ""` y este transporte NUNCA lo manda al wire.
/// No se separa en un trait propio para no duplicar `MetaDecisionEngine` por un único parámetro
/// que el caller controla; el `LocalMetaDecision` es el único caller y siempre pasa `""`. Si
/// alguien reusara este transporte con un motor que requiera auth, el header simplemente no se
/// envía (degrada a 401/None) — nunca filtra un bearer ajeno.
///
/// Devuelve el texto del modelo o `None` ante CUALQUIER fallo (conexión rechazada / timeout /
/// status no-2xx / body vacío / shape inesperado) — NUNCA propaga `Err` (advisory, FR-005).
pub struct OllamaTransport {
    pub model: String,
}

#[async_trait]
impl MetaTransport for OllamaTransport {
    async fn post(&self, req: &MetaRequest, _bearer: &str, timeout: Duration) -> Option<String> {
        let url = format!("{}/v1/chat/completions", req.base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.prompt},
            ],
            "max_tokens": req.max_tokens,
            "stream": false,
            "temperature": 0,
        });
        // `.ok()?` en todo el path de red — NUNCA unwrap, NUNCA panic (constitución/clippy).
        let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
        let resp = client
            .post(&url)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        // OpenAI-compat: choices[0].message.content. Fallback a un `text` plano por robustez.
        let text = v
            .pointer("/choices/0/message/content")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("text").and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

/// Implementación LOCAL del engine: Ollama loopback, sin bearer. Sanitize → allowlist loopback →
/// POST con timeout → parse defensivo → cache → audit. NUNCA propaga `Err`; todo fallo ⇒ `None`
/// (FR-005, advisory). El cache es el `SHARED_CACHE` long-lived COMPARTIDO con el AIE, pero la
/// cache-key lleva el `model` como prefijo (no el profile AIE) → NUNCA colisiona con un verdict del
/// AIE para el mismo buffer (FR-001, plan §cache).
pub struct LocalMetaDecision {
    endpoint: String,
    model: String,
    timeout: Duration,
    transport: Arc<dyn MetaTransport>,
    cache: Arc<VerdictCache>,
    audit: Arc<dyn MetaAudit>,
}

impl LocalMetaDecision {
    /// Construye con el transporte real (Ollama, sin bearer) + audit sink. `endpoint` y `model` los
    /// resuelve el caller desde settings (`done_detection::get_ollama_endpoint/get_ollama_model`).
    /// El cache es el `SHARED_CACHE` long-lived (persiste entre ticks del poller, research §2).
    pub fn new(endpoint: String, model: String, audit: Arc<dyn MetaAudit>) -> Self {
        let transport: Arc<dyn MetaTransport> = Arc::new(OllamaTransport {
            model: model.clone(),
        });
        Self {
            endpoint,
            model,
            timeout: Duration::from_secs(3),
            transport,
            cache: shared_cache(),
            audit,
        }
    }

    /// Variante para tests: transporte inyectable (mock) + cache FRESCO por engine (aislamiento
    /// entre tests; no toca el `SHARED_CACHE` global ni el cache del AIE).
    #[cfg(test)]
    fn with_transport(
        endpoint: String,
        model: String,
        transport: Arc<dyn MetaTransport>,
        audit: Arc<dyn MetaAudit>,
    ) -> Self {
        Self {
            endpoint,
            model,
            timeout: Duration::from_secs(3),
            transport,
            cache: Arc::new(VerdictCache::new()),
            audit,
        }
    }

    /// Prefijo de cache-key para el motor local: `local:<model>@<endpoint>`. Distinto del profile
    /// AIE (`fast_small_free`, `bulk_free`) → el cache NUNCA colisiona local↔AIE para el mismo
    /// buffer. Incluye el ENDPOINT además del modelo: dos instancias de Ollama con el mismo nombre
    /// de modelo en puertos loopback distintos (p.ej. `:11434` y `:11435`) NO comparten cache
    /// (audit deepseek finding: evitar falso cache-hit entre instancias homónimas).
    fn cache_profile(&self) -> String {
        format!("local:{}@{}", self.model, self.endpoint)
    }
}

#[async_trait]
impl MetaDecisionEngine for LocalMetaDecision {
    async fn classify_done(&self, buffer_tail: &str, cli: &str) -> Option<Verdict> {
        let start = Instant::now();

        // 1. Allowlist LOOPBACK-only (SSRF defense — el motor local NO puede pegar a un host no
        //    loopback aunque la allowlist general lo aceptaría). FR-007.
        if !loopback_allowed(&self.endpoint) {
            tracing::debug!("meta_decision(local): endpoint no es loopback — fallback heurística");
            return None;
        }
        // 2. (Sin bearer — Ollama no usa Authorization.)
        // 3. Sanitizar ANTES de armar el request y ANTES del cache-key (fail-closed, FR-006).
        let sanitized = AieMetaDecision::sanitize_failclosed(buffer_tail)?;

        // 4. Cache lookup (key sobre el input SANITIZADO, prefijo `local:<model>`).
        let cache_profile = self.cache_profile();
        let key = VerdictCache::key(&cache_profile, &sanitized);
        if let Some(v) = self.cache.get(&key, DONE_TTL) {
            self.audit.record(
                "done_detection",
                &cache_profile,
                Some(v),
                start.elapsed().as_millis() as u64,
                true,
            );
            return Some(v);
        }

        // 4b. Singleflight: un solo POST por key concurrente.
        let _flight = self.cache.try_begin(key)?;

        // 5. POST con timeout (advisory: un 3B lento NUNCA bloquea el poller → None por timeout).
        let cli_clean = cli
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .take(24)
            .collect::<String>();
        let req = MetaRequest {
            base_url: self.endpoint.clone(),
            profile: DONE_PROFILE, // sin uso en el transporte local (lleva su propio model)
            system: DONE_SYSTEM.to_string(),
            prompt: format!("CLI: {cli_clean}\nTerminal tail:\n```\n{sanitized}\n```\nOne word:"),
            max_tokens: DONE_MAX_TOKENS,
        };
        let text = match self.transport.post(&req, "", self.timeout).await {
            Some(t) => t,
            None => {
                tracing::debug!(
                    "meta_decision(local): Ollama unreachable/timeout/non-2xx — fallback heurística"
                );
                self.audit.record(
                    "done_detection",
                    &cache_profile,
                    None,
                    start.elapsed().as_millis() as u64,
                    false,
                );
                return None;
            }
        };

        // 6. Parse defensivo ESTRICTO: verdict fuera del enum ⇒ None.
        let verdict = Verdict::parse_keyword(&text);
        if let Some(v) = verdict {
            self.cache.store(key, v);
        } else {
            tracing::debug!(
                "meta_decision(local): reply no parseable a verdict — fallback heurística"
            );
        }
        self.audit.record(
            "done_detection",
            &cache_profile,
            verdict,
            start.elapsed().as_millis() as u64,
            false,
        );
        verdict
    }

    async fn rank_variants(&self, objective: &str, diffs: &[String]) -> Option<Vec<usize>> {
        // US2 (engine implementado, cableado lo hace P2). Sanitiza objetivo + diffs fail-closed.
        if diffs.is_empty() {
            return None;
        }
        if !loopback_allowed(&self.endpoint) {
            return None;
        }
        let obj = AieMetaDecision::sanitize_failclosed(objective)?;
        let mut blocks = String::new();
        for (i, d) in diffs.iter().enumerate() {
            let san = AieMetaDecision::sanitize_failclosed(d)?;
            let capped: String = san.chars().take(4000).collect();
            blocks.push_str(&format!("### Variant {i}\n```\n{capped}\n```\n"));
        }
        let n = diffs.len();
        let req = MetaRequest {
            base_url: self.endpoint.clone(),
            profile: BULK_PROFILE,
            system: format!(
                "You rank {n} candidate code diffs by quality for the given objective. \
                 Reply ONLY with the variant indices (0-based) from best to worst, \
                 comma-separated (e.g. `2,0,1`). No prose."
            ),
            prompt: format!("Objective:\n{obj}\n\n{blocks}\nRanking:"),
            max_tokens: 64,
        };
        let text = self.transport.post(&req, "", self.timeout).await?;
        let ranking = parse_ranking(&text, n)?;
        Some(ranking)
    }

    async fn classify_task(&self, objective: &str) -> Option<String> {
        // US3 (engine implementado, cableado lo hace P2).
        if !loopback_allowed(&self.endpoint) {
            return None;
        }
        let obj = AieMetaDecision::sanitize_failclosed(objective)?;
        let req = MetaRequest {
            base_url: self.endpoint.clone(),
            profile: DONE_PROFILE,
            system: "Classify the software task into EXACTLY ONE word from: bugfix, feature, refactor, docs, test, chore. Reply with the single word only.".to_string(),
            prompt: format!("Task:\n{obj}\n\nCategory:"),
            max_tokens: 8,
        };
        let text = self.transport.post(&req, "", self.timeout).await?;
        parse_task_category(&text)
    }
}

/// Parse defensivo ESTRICTO del ranking del AIE: extrae TODOS los enteros que aparezcan en el
/// texto y exige que sean EXACTAMENTE una permutación de `0..n` (cada índice una sola vez, ninguno
/// fuera de rango). Cualquier índice fuera de `[0,n)` O duplicado invalida TODA la respuesta ⇒
/// `None` (audit 3-frontera finding #2: NO devolver una permutación parcial — research §1: schema
/// inválido ⇒ fallback al picker manual). Ej n=3: `"0,1,2,9"` ⇒ `None` (9 fuera de rango);
/// `"0 0 1"` ⇒ `None` (0 duplicado).
fn parse_ranking(raw: &str, n: usize) -> Option<Vec<usize>> {
    let mut seen = vec![false; n];
    let mut out = Vec::with_capacity(n);
    // `flush` devuelve `Err(())` si el número recolectado es inválido (fuera de rango / duplicado /
    // no-parseable como usize), señal para abortar TODO el parse (estricto).
    let flush = |num: &mut String, out: &mut Vec<usize>, seen: &mut [bool]| -> Result<(), ()> {
        if num.is_empty() {
            return Ok(());
        }
        let v = num.parse::<usize>().map_err(|_| ())?;
        num.clear();
        if v >= seen.len() || seen[v] {
            // fuera de rango O duplicado → respuesta inválida completa.
            return Err(());
        }
        seen[v] = true;
        out.push(v);
        Ok(())
    };
    let mut num = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
        } else {
            flush(&mut num, &mut out, &mut seen).ok()?;
        }
    }
    flush(&mut num, &mut out, &mut seen).ok()?;
    // Permutación COMPLETA de 0..n (la unicidad ya la garantiza `seen`).
    if out.len() == n {
        Some(out)
    } else {
        None
    }
}

/// Parse defensivo ESTRICTO de la categoría de tarea (US3). Tras `trim()` + lowercase, acepta SOLO
/// si la respuesta es EXACTAMENTE una de las categorías válidas (un único token, sin prosa ni
/// substring). El prompt instruye "the single word only", así que prosa ("This is a refactor.") o
/// un substring ("not a bugfix") ⇒ `None` ⇒ el caller cae a la elección manual de agente
/// (audit 3-frontera finding #3).
fn parse_task_category(raw: &str) -> Option<String> {
    let tok = raw.trim().to_lowercase();
    match tok.as_str() {
        "bugfix" | "feature" | "refactor" | "docs" | "test" | "chore" => Some(tok),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Mock transport: cuenta llamadas + captura el último body enviado ──────────
    struct MockTransport {
        reply: Mutex<Option<String>>,
        calls: AtomicUsize,
        last_prompt: Mutex<String>,
        last_system: Mutex<String>,
    }
    impl MockTransport {
        fn new(reply: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                reply: Mutex::new(reply.map(String::from)),
                calls: AtomicUsize::new(0),
                last_prompt: Mutex::new(String::new()),
                last_system: Mutex::new(String::new()),
            })
        }
        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl MetaTransport for MockTransport {
        async fn post(
            &self,
            req: &MetaRequest,
            _bearer: &str,
            _timeout: Duration,
        ) -> Option<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.last_prompt.lock().unwrap() = req.prompt.clone();
            *self.last_system.lock().unwrap() = req.system.clone();
            self.reply.lock().unwrap().clone()
        }
    }

    fn engine(reply: Option<&str>) -> (AieMetaDecision, Arc<MockTransport>) {
        let t = MockTransport::new(reply);
        // 041 FR-005 — the Tailscale host is no longer a default allowlist entry; register it as a
        // runtime origin so the AIE allowlist check passes (mirrors a user who configured it).
        let _ = crate::bases::allowlist::add_runtime_origin("http://100.64.0.10:8250");
        let e = AieMetaDecision::with_transport(
            "http://100.64.0.10:8250".to_string(),
            Some("test-bearer".to_string()),
            t.clone(),
            Arc::new(NoopAudit),
        );
        (e, t)
    }

    // ── parse_keyword ─────────────────────────────────────────────────────────────
    #[test]
    fn parse_keyword_maps_words() {
        // Token exacto (case-insensitive, con trim) ⇒ Some.
        assert_eq!(Verdict::parse_keyword("WORKING"), Some(Verdict::Running));
        assert_eq!(Verdict::parse_keyword("  idle  "), Some(Verdict::Idle));
        assert_eq!(
            Verdict::parse_keyword("Question"),
            Some(Verdict::NeedsInput)
        );
        assert_eq!(Verdict::parse_keyword("SUCCESS"), Some(Verdict::Idle));
        assert_eq!(Verdict::parse_keyword("FAILED"), Some(Verdict::Idle));
        assert_eq!(
            Verdict::parse_keyword("IN_PROGRESS"),
            Some(Verdict::Running)
        );
        assert_eq!(
            Verdict::parse_keyword("NEEDS_INPUT"),
            Some(Verdict::NeedsInput)
        );
        assert_eq!(Verdict::parse_keyword("gobbledygook"), None);
    }

    #[test]
    fn parse_keyword_is_strict_no_substring_no_prose() {
        // Substrings / prosa / múltiples palabras ⇒ None (parse estricto, audit finding #3).
        assert_eq!(Verdict::parse_keyword("maybe IDLE"), None);
        assert_eq!(Verdict::parse_keyword("not QUESTION"), None);
        assert_eq!(
            Verdict::parse_keyword("not a SUCCESS, it's a QUESTION"),
            None
        );
        assert_eq!(Verdict::parse_keyword("IDLE."), None); // puntuación pegada ≠ token exacto
        assert_eq!(Verdict::parse_keyword("WORKING IDLE"), None); // 2 tokens
        assert_eq!(Verdict::parse_keyword(""), None);
        assert_eq!(Verdict::parse_keyword("   "), None);
    }

    // ── Fallback: TODOS los modos de fallo → None (SC-003) ───────────────────────
    #[tokio::test]
    async fn heuristic_fallback_always_none() {
        let h = HeuristicFallback;
        assert_eq!(h.classify_done("> ", "claude").await, None);
        assert_eq!(h.rank_variants("obj", &["a".into()]).await, None);
        assert_eq!(h.classify_task("fix the bug").await, None);
    }

    #[tokio::test]
    async fn aie_no_bearer_is_none_no_call() {
        let t = MockTransport::new(Some("IDLE"));
        let e = AieMetaDecision::with_transport(
            "http://100.64.0.10:8250".to_string(),
            None, // sin bearer
            t.clone(),
            Arc::new(NoopAudit),
        );
        assert_eq!(e.classify_done("> ", "claude").await, None);
        assert_eq!(t.call_count(), 0, "sin bearer no se consulta el AIE");
    }

    #[tokio::test]
    async fn aie_endpoint_not_allowed_is_none_no_call() {
        let t = MockTransport::new(Some("IDLE"));
        let e = AieMetaDecision::with_transport(
            "http://evil.example.com:8250".to_string(), // fuera de allowlist
            Some("b".into()),
            t.clone(),
            Arc::new(NoopAudit),
        );
        assert_eq!(e.classify_done("> ", "claude").await, None);
        assert_eq!(t.call_count(), 0, "endpoint no permitido no se consulta");
    }

    #[tokio::test]
    async fn aie_transport_failure_is_none() {
        // El transporte devuelve None (timeout / HTTP error / parse-fail) → None, no Err.
        let (e, t) = engine(None);
        assert_eq!(e.classify_done("ambiguous tail", "claude").await, None);
        assert_eq!(t.call_count(), 1);
    }

    #[tokio::test]
    async fn aie_unparseable_reply_is_none() {
        // El AIE responde algo que no matchea el enum → None (parse defensivo).
        let (e, _t) = engine(Some("I am not sure what to say here"));
        assert_eq!(e.classify_done("ambiguous tail", "claude").await, None);
    }

    #[tokio::test]
    async fn aie_happy_path_maps_verdict() {
        let (e, _t) = engine(Some("QUESTION"));
        assert_eq!(
            e.classify_done("Do you trust this folder?", "claude").await,
            Some(Verdict::NeedsInput)
        );
    }

    // ── Sanitizer: un sk-... NO aparece en el body del request (SC-004) ──────────
    #[tokio::test]
    async fn sanitizer_redacts_secret_before_send() {
        let (e, t) = engine(Some("IDLE"));
        let leaky = "export OPENAI_KEY=sk-proj-Abc123_def-XYZ987_secretmaterial\n> ";
        let _ = e.classify_done(leaky, "codex").await;
        assert_eq!(t.call_count(), 1, "se hizo la llamada");
        let sent = t.last_prompt.lock().unwrap().clone();
        assert!(
            sent.contains("[REDACTED:sk]"),
            "el sk- debe estar redactado: {sent}"
        );
        assert!(
            !sent.contains("sk-proj-Abc123"),
            "el secreto NO debe aparecer en el body enviado al AIE: {sent}"
        );
    }

    // ── Cache: 2da consulta del mismo buffer dentro del TTL = cache-hit ──────────
    #[tokio::test]
    async fn cache_hit_avoids_second_call() {
        let (e, t) = engine(Some("WORKING"));
        let buf = "⠋ thinking\nesc to interrupt";
        let v1 = e.classify_done(buf, "claude").await;
        let v2 = e.classify_done(buf, "claude").await;
        assert_eq!(v1, Some(Verdict::Running));
        assert_eq!(v2, Some(Verdict::Running));
        assert_eq!(
            t.call_count(),
            1,
            "la 2da consulta del mismo buffer es cache-hit, no re-llama"
        );
    }

    #[tokio::test]
    async fn cache_keys_on_sanitized_input_not_raw() {
        // Dos buffers con secretos DISTINTOS pero que redactan al MISMO texto → mismo cache key.
        let (e, t) = engine(Some("IDLE"));
        let a = "key sk-proj-AAAAAAAAAAAAAAAAAA done\n> ";
        let b = "key sk-proj-BBBBBBBBBBBBBBBBBB done\n> ";
        let _ = e.classify_done(a, "claude").await;
        let _ = e.classify_done(b, "claude").await;
        assert_eq!(
            t.call_count(),
            1,
            "ambos redactan al mismo texto → 1 sola llamada (cache por input sanitizado)"
        );
    }

    // ── US2 rank_variants (engine, sin cablear UI) ───────────────────────────────
    #[tokio::test]
    async fn rank_variants_parses_permutation() {
        let (e, _t) = engine(Some("the ranking is 2,0,1"));
        let r = e
            .rank_variants("obj", &["diff a".into(), "diff b".into(), "diff c".into()])
            .await;
        assert_eq!(r, Some(vec![2, 0, 1]));
    }

    #[tokio::test]
    async fn rank_variants_incomplete_is_none() {
        // El modelo devuelve sólo 2 índices para 3 variantes → schema inválido → None.
        let (e, _t) = engine(Some("0,1"));
        let r = e
            .rank_variants("obj", &["a".into(), "b".into(), "c".into()])
            .await;
        assert_eq!(r, None);
    }

    #[tokio::test]
    async fn rank_variants_empty_is_none_no_call() {
        let (e, t) = engine(Some("0"));
        assert_eq!(e.rank_variants("obj", &[]).await, None);
        assert_eq!(t.call_count(), 0);
    }

    // ── US3 classify_task (engine, sin cablear UI) ───────────────────────────────
    #[tokio::test]
    async fn classify_task_exact_category() {
        // Token exacto (con trim) → Some.
        let (e, _t) = engine(Some("  refactor\n"));
        assert_eq!(
            e.classify_task("rename the module").await,
            Some("refactor".to_string())
        );
    }

    #[tokio::test]
    async fn classify_task_prose_is_none() {
        // ESTRICTO (audit finding #3): prosa que CONTIENE una categoría pero no es exactamente
        // un token → None (no extraemos substrings).
        let (e, _t) = engine(Some("This is a refactor."));
        assert_eq!(e.classify_task("rename the module").await, None);
    }

    #[tokio::test]
    async fn classify_task_negation_substring_is_none() {
        // "not a bugfix" contiene "bugfix" pero NO es la categoría → None.
        let (e, _t) = engine(Some("not a bugfix"));
        assert_eq!(e.classify_task("???").await, None);
    }

    #[tokio::test]
    async fn classify_task_unknown_is_none() {
        let (e, _t) = engine(Some("banana"));
        assert_eq!(e.classify_task("???").await, None);
    }

    // ── parse_ranking unit ────────────────────────────────────────────────────────
    #[test]
    fn parse_ranking_handles_noise_and_dedup() {
        // Prosa alrededor de una permutación completa y válida → OK.
        assert_eq!(
            parse_ranking("best: 1, then 0, then 2", 3),
            Some(vec![1, 0, 2])
        );
        assert_eq!(parse_ranking("2 1 0", 3), Some(vec![2, 1, 0]));
        // ESTRICTO (audit finding #2): cualquier duplicado invalida TODA la respuesta → None
        // (NO una permutación parcial).
        assert_eq!(parse_ranking("0 0 0", 3), None);
        assert_eq!(parse_ranking("0,1,0", 3), None);
        // ESTRICTO: un índice fuera de rango invalida TODA la respuesta → None — incluso si los
        // demás forman una permutación completa de 0..n. n=3, "0,1,2,9" → None (NO Some([0,1,2])).
        assert_eq!(parse_ranking("0,1,2,9", 3), None);
        assert_eq!(parse_ranking("0,1,9", 3), None);
        // Incompleto → None.
        assert_eq!(parse_ranking("0,1", 3), None);
    }

    #[test]
    fn parse_task_category_is_strict() {
        // Token exacto (con trim, case-insensitive) → Some.
        assert_eq!(parse_task_category("feature"), Some("feature".to_string()));
        assert_eq!(
            parse_task_category("  Bugfix \n"),
            Some("bugfix".to_string())
        );
        assert_eq!(parse_task_category("CHORE"), Some("chore".to_string()));
        // Prosa / substring / negación → None.
        assert_eq!(parse_task_category("This is a refactor."), None);
        assert_eq!(parse_task_category("not a bugfix"), None);
        assert_eq!(parse_task_category("feature request"), None);
        assert_eq!(parse_task_category(""), None);
        assert_eq!(parse_task_category("banana"), None);
    }

    #[test]
    fn singleflight_dedups_inflight_key() {
        let cache = Arc::new(VerdictCache::new());
        let key = VerdictCache::key("p", "buf");
        // Primera reserva: dueña del POST.
        let g1 = cache.try_begin(key);
        assert!(g1.is_some(), "la primera reserva debe tener éxito");
        // Segunda reserva concurrente de la MISMA key → None (no duplica el POST).
        assert!(
            cache.try_begin(key).is_none(),
            "una key in-flight no se vuelve a reservar"
        );
        // Una key distinta sí puede reservar en paralelo.
        let key2 = VerdictCache::key("p", "otro");
        assert!(
            cache.try_begin(key2).is_some(),
            "otra key reserva en paralelo"
        );
        // Al soltar el guard, la key se libera y se puede re-reservar.
        drop(g1);
        assert!(
            cache.try_begin(key).is_some(),
            "tras Drop del guard la key se re-reserva"
        );
    }

    // ── LocalMetaDecision (036) ───────────────────────────────────────────────────

    /// Helper: motor LOCAL con endpoint loopback (default Ollama) y transporte mock inyectado.
    fn local_engine(reply: Option<&str>) -> (LocalMetaDecision, Arc<MockTransport>) {
        let t = MockTransport::new(reply);
        let e = LocalMetaDecision::with_transport(
            "http://127.0.0.1:11434".to_string(),
            "qwen2.5:3b".to_string(),
            t.clone(),
            Arc::new(NoopAudit),
        );
        (e, t)
    }

    #[tokio::test]
    async fn local_happy_path_maps_verdict() {
        // Ollama responde una keyword exacta → verdict parseado, sin bearer.
        let (e, t) = local_engine(Some("QUESTION"));
        assert_eq!(
            e.classify_done("Do you trust this folder?", "claude").await,
            Some(Verdict::NeedsInput)
        );
        assert_eq!(t.call_count(), 1, "se consultó el modelo local");
    }

    #[tokio::test]
    async fn local_connection_refused_is_none() {
        // El transporte devuelve None (Ollama apagado = connection refused) → None, no Err, no
        // cuelgue (FR-005: degradación limpia).
        let (e, t) = local_engine(None);
        assert_eq!(e.classify_done("ambiguous tail", "claude").await, None);
        assert_eq!(t.call_count(), 1);
    }

    #[tokio::test]
    async fn local_timeout_is_none() {
        // Un modelo lento que excede el timeout se modela igual que cualquier fallo de transporte:
        // None → fallback a la heurística (el poller NUNCA se cuelga, FR-005).
        let (e, _t) = local_engine(None);
        assert_eq!(e.classify_done("⠋ thinking", "codex").await, None);
    }

    #[tokio::test]
    async fn local_unparseable_reply_is_none() {
        // El modelo respondió pero no fue una keyword exacta → parse defensivo → None (sin inventar
        // un verdict falso).
        let (e, _t) = local_engine(Some("I think it is probably done now, maybe"));
        assert_eq!(e.classify_done("ambiguous tail", "claude").await, None);
    }

    #[tokio::test]
    async fn local_sanitizer_redacts_secret_before_post() {
        // FR-006: aunque el modelo sea local, el sk- se redacta ANTES del POST (defensa en
        // profundidad — el modelo local podría loguear).
        let (e, t) = local_engine(Some("IDLE"));
        let leaky = "export OPENAI_KEY=sk-proj-Abc123_def-XYZ987_secretmaterial\n> ";
        let _ = e.classify_done(leaky, "codex").await;
        assert_eq!(t.call_count(), 1, "se hizo la llamada local");
        let sent = t.last_prompt.lock().unwrap().clone();
        assert!(
            sent.contains("[REDACTED:sk]"),
            "el sk- debe estar redactado: {sent}"
        );
        assert!(
            !sent.contains("sk-proj-Abc123"),
            "el secreto NO debe aparecer en el body posteado a Ollama: {sent}"
        );
    }

    #[tokio::test]
    async fn local_allowlist_loopback_ok_internal_ip_rejected() {
        // FR-007: loopback (127.0.0.1) se permite; una IP interna NO-loopback se RECHAZA (sin POST),
        // aunque la allowlist general (`url_allowed`) la dejaría pasar.
        // (a) loopback → consulta.
        let (e_ok, t_ok) = local_engine(Some("IDLE"));
        assert_eq!(
            e_ok.classify_done("> ", "claude").await,
            Some(Verdict::Idle)
        );
        assert_eq!(t_ok.call_count(), 1, "loopback se permite");

        // (b) IP interna no-loopback (Tailscale the dev server) — la allowlist general la acepta SOLO si el
        // user la configuró (041 FR-005: ya no es default). La registramos como runtime origin para
        // que la precondición se cumpla; el gate LOOPBACK-only del motor local igual la rechaza →
        // None sin POST (no bypass SSRF). Ese es el punto del test: el gate local es más estricto.
        let _ = crate::bases::allowlist::add_runtime_origin("http://100.64.0.10:11434");
        assert!(
            crate::bases::allowlist::url_allowed("http://100.64.0.10:11434"),
            "precondición: la allowlist general acepta el Tailscale tras configurarlo"
        );
        let t_bad = MockTransport::new(Some("IDLE"));
        let e_bad = LocalMetaDecision::with_transport(
            "http://100.64.0.10:11434".to_string(),
            "qwen2.5:3b".to_string(),
            t_bad.clone(),
            Arc::new(NoopAudit),
        );
        assert_eq!(
            e_bad.classify_done("> ", "claude").await,
            None,
            "una IP interna no-loopback se rechaza en el motor local"
        );
        assert_eq!(
            t_bad.call_count(),
            0,
            "endpoint no-loopback no se consulta (sin POST)"
        );

        // (c) un host de sufijo permitido por `url_allowed` (example.test) también se rechaza acá.
        let t_dom = MockTransport::new(Some("IDLE"));
        let e_dom = LocalMetaDecision::with_transport(
            "https://aie.example.internal".to_string(),
            "qwen2.5:3b".to_string(),
            t_dom.clone(),
            Arc::new(NoopAudit),
        );
        assert_eq!(e_dom.classify_done("> ", "claude").await, None);
        assert_eq!(t_dom.call_count(), 0);
    }

    #[test]
    fn loopback_allowed_unit() {
        // Loopback IPv4 (todo 127.0.0.0/8), localhost, ::1 → true.
        assert!(loopback_allowed("http://127.0.0.1:11434"));
        assert!(loopback_allowed("http://127.0.0.1"));
        assert!(loopback_allowed("http://127.1.2.3:11434"));
        assert!(loopback_allowed("http://localhost:11434/v1"));
        assert!(loopback_allowed("http://LocalHost:11434"));
        assert!(loopback_allowed("http://[::1]:11434"));
        // No-loopback (incluso si la allowlist general las aceptara) → false.
        assert!(!loopback_allowed("http://100.64.0.10:11434"));
        assert!(!loopback_allowed("http://10.0.0.5:11434"));
        assert!(!loopback_allowed("http://192.168.1.10:11434"));
        assert!(!loopback_allowed("https://aie.example.internal"));
        assert!(!loopback_allowed("http://evil.example.com:11434"));
        // Userinfo trick (audit deepseek finding): el host real está DESPUÉS del `@`. `url::Url`
        // parsea `127.0.0.1:11434` como userinfo y `evil.com` como host → se RECHAZA (no bypass).
        assert!(!loopback_allowed("http://127.0.0.1:11434@evil.com"));
        assert!(!loopback_allowed("http://127.0.0.1@evil.com:11434"));
        // El inverso (host loopback con userinfo benigno) sí se acepta — el host es loopback.
        assert!(loopback_allowed("http://user:pass@127.0.0.1:11434"));
        // Esquemas no-http / basura → false.
        assert!(!loopback_allowed("file:///etc/passwd"));
        assert!(!loopback_allowed("ftp://127.0.0.1"));
        assert!(!loopback_allowed("not a url"));
        assert!(!loopback_allowed(""));
    }

    #[tokio::test]
    async fn local_cache_does_not_collide_with_aie() {
        // FR-001/plan §cache: el motor local y el AIE comparten el SHARED_CACHE, pero la key del
        // local lleva prefijo `local:<model>` y la del AIE el profile (`fast_small_free`). Para el
        // MISMO buffer sanitizado las dos keys son DISTINTAS → no se sirven cruzado.
        let buf = "some sanitized terminal tail";
        let local = LocalMetaDecision::new(
            "http://127.0.0.1:11434".to_string(),
            "qwen2.5:3b".to_string(),
            Arc::new(NoopAudit),
        );
        let k_local = VerdictCache::key(&local.cache_profile(), buf);
        let k_aie = VerdictCache::key(DONE_PROFILE, buf);
        assert_ne!(
            k_local, k_aie,
            "la cache-key local NUNCA colisiona con la del AIE para el mismo buffer"
        );
        // Dos modelos locales distintos tampoco colisionan entre sí.
        let local_b = LocalMetaDecision::new(
            "http://127.0.0.1:11434".to_string(),
            "llama3.2:1b".to_string(),
            Arc::new(NoopAudit),
        );
        let k_local_b = VerdictCache::key(&local_b.cache_profile(), buf);
        assert_ne!(k_local, k_local_b, "distinto modelo → distinta key");
        // Mismo modelo, distinto endpoint loopback (puerto) → distinta key (audit deepseek finding:
        // dos instancias de Ollama homónimas en puertos distintos NO comparten cache).
        let local_c = LocalMetaDecision::new(
            "http://127.0.0.1:11435".to_string(),
            "qwen2.5:3b".to_string(),
            Arc::new(NoopAudit),
        );
        let k_local_c = VerdictCache::key(&local_c.cache_profile(), buf);
        assert_ne!(
            k_local, k_local_c,
            "mismo modelo en endpoint distinto → distinta key"
        );
    }

    #[tokio::test]
    async fn local_cache_hit_avoids_second_call() {
        // Dentro del TTL, la 2da consulta del mismo buffer es cache-hit (no re-POST).
        let (e, t) = local_engine(Some("WORKING"));
        let buf = "⠋ thinking\nesc to interrupt";
        let v1 = e.classify_done(buf, "claude").await;
        let v2 = e.classify_done(buf, "claude").await;
        assert_eq!(v1, Some(Verdict::Running));
        assert_eq!(v2, Some(Verdict::Running));
        assert_eq!(t.call_count(), 1, "la 2da consulta es cache-hit");
    }

    #[tokio::test]
    async fn local_rank_and_classify_smoke() {
        // US2/US3 disponibles aunque P2 los cablee: permutación válida + categoría exacta.
        let (e, _t) = local_engine(Some("2,0,1"));
        let r = e
            .rank_variants("obj", &["a".into(), "b".into(), "c".into()])
            .await;
        assert_eq!(r, Some(vec![2, 0, 1]));

        let (e2, _t2) = local_engine(Some(" refactor \n"));
        assert_eq!(
            e2.classify_task("rename the module").await,
            Some("refactor".to_string())
        );

        // rank con endpoint no-loopback → None (gate FR-007 también en US2).
        let t_bad = MockTransport::new(Some("0"));
        let e_bad = LocalMetaDecision::with_transport(
            "http://10.0.0.5:11434".to_string(),
            "qwen2.5:3b".to_string(),
            t_bad.clone(),
            Arc::new(NoopAudit),
        );
        assert_eq!(e_bad.rank_variants("obj", &["a".into()]).await, None);
        assert_eq!(t_bad.call_count(), 0);
    }
}
