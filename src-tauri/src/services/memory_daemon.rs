// FASE 2 — Memory Hub: Universal cross-CLI memory daemon.
// Council FASE 2: axum HTTP API + MCP bridge + FTS5 + embeddings.
// Runs on localhost:43119 (HTTP) and localhost:43120 (MCP).
//
// Audit-1 (5-voice council + Codex, 2026-05-27):
// - HIGH: auth on all non-health endpoints (Keychain-backed bearer token).
// - HIGH: tokio::process for keychain lookup (no blocking I/O in async).
// - HIGH: FTS5 + entries INSERTs wrapped in BEGIN/COMMIT for atomicity.
// - MED: FTS query length cap (anti-DoS).
// - MED: reusable reqwest::Client (Lazy static).

use anyhow::{anyhow, Result};
use axum::{
    extract::Request,
    extract::State as AxState,
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use uuid::Uuid;

const MEMORY_PORT: u16 = 43119;

/// FTS5 query length cap (anti-DoS: avoid pathological expression trees).
const FTS_QUERY_MAX_CHARS: usize = 512;
const FTS_QUERY_MAX_TERMS: usize = 32;

/// Service name for the Memory Hub local bearer token in macOS Keychain.
/// Token is auto-generated on first daemon start if missing; the file path
/// at `~/.furx/memory-daemon.token` is the fallback for non-mac platforms.
const TOKEN_KEYCHAIN_SERVICE: &str = "furx-memory-daemon-token";

/// Reusable HTTP client for outbound embedding calls. Avoids per-request
/// TLS/connection-pool allocation (perf B003 / B012).
static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("reqwest client build")
});

// --- 045 FR-003: re-rank vectorial + circuit-breaker del embedder ---

/// Timeout DURO del embedding del query en el path de recall. Si el embedder (AIE/Ollama) no
/// responde en este lapso, el re-rank se saltea y cae a FTS (NUNCA cuelga el recall).
const RERANK_EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
/// El breaker ABRE tras este nº de fallos/timeouts consecutivos del embedder.
const BREAKER_THRESHOLD: u32 = 3;
/// Tras abrir, el breaker deja re-intentar una sondeo recién pasado este lapso (half-open).
const BREAKER_RESET_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Estado del circuit-breaker del embedder (Ollama/AIE). `parking_lot::RwLock` sobre estado mínimo.
/// Sin él, 100 queries × 500ms de timeout = 50s de bloqueos acumulados con el embedder caído.
struct EmbedderBreaker {
    consecutive_failures: u32,
    /// `Some(t)` = abierto desde el instante `t`. `None` = cerrado.
    opened_at: Option<std::time::Instant>,
}

static EMBEDDER_BREAKER: Lazy<parking_lot::RwLock<EmbedderBreaker>> =
    Lazy::new(|| {
        parking_lot::RwLock::new(EmbedderBreaker {
            consecutive_failures: 0,
            opened_at: None,
        })
    });

/// ¿El breaker permite intentar el embedding ahora? Cerrado ⇒ sí. Abierto pero ya pasó el lapso de
/// reset ⇒ sí (sondeo half-open). Abierto y fresco ⇒ no (cae directo a FTS sin tocar la red).
///
/// Tradeoff ACEPTADO (audit nvidia): hay un TOCTOU benigno entre este `allows()` (read-lock) y el
/// embed: en half-open, varios recalls concurrentes podrían sondear a la vez. Es inocuo — cada
/// embed es idempotente, NO comparte estado mutable, y un breaker es degradación best-effort, no un
/// semáforo. Además el recall del usuario es de hecho serial (una búsqueda a la vez). No vale meter
/// un lock duro de "single-probe" que agregaría contención sin beneficio real. El re-armado del
/// reloj mientras el embedder sigue caído es DESEADO: mantiene la caída limpia a FTS hasta que sane.
fn breaker_allows() -> bool {
    let b = EMBEDDER_BREAKER.read();
    match b.opened_at {
        None => true,
        Some(t) => t.elapsed() >= BREAKER_RESET_AFTER,
    }
}

/// Registra un éxito del embedder: cierra el breaker y resetea el contador.
fn breaker_on_success() {
    let mut b = EMBEDDER_BREAKER.write();
    b.consecutive_failures = 0;
    b.opened_at = None;
}

/// Registra un fallo/timeout: incrementa el contador y ABRE al alcanzar el umbral (re-arma el
/// reloj si ya estaba abierto y reintentó sin éxito).
fn breaker_on_failure() {
    let mut b = EMBEDDER_BREAKER.write();
    b.consecutive_failures = b.consecutive_failures.saturating_add(1);
    if b.consecutive_failures >= BREAKER_THRESHOLD {
        b.opened_at = Some(std::time::Instant::now());
    }
}

/// Coseno entre dos vectores f32. 0.0 si dims no coinciden o alguno es cero (degradación limpia).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Decodifica un BLOB de embedding (f32 little-endian, como lo guarda `compute_and_store_embedding`).
/// Devuelve `None` si el largo no es múltiplo de 4 o está vacío.
fn decode_embedding(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// Pide el embedding del query al embedder (AIE/Ollama `/v1/embeddings`, nomic-embed-text), con
/// timeout DURO. Devuelve `Ok(vec)` o `Err` (timeout/red/no-bearer/parse). NO toca el breaker (eso
/// lo decide el caller para distinguir "no se intentó" de "falló").
async fn embed_query(query: &str) -> Result<Vec<f32>> {
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or_else(|| anyhow!("no bearer"))?;
    let url = format!(
        "{}/v1/embeddings",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let fut = HTTP_CLIENT
        .post(&url)
        .header("Authorization", format!("Bearer {}", bearer))
        .json(&serde_json::json!({"model": "nomic-embed-text", "input": query}))
        .send();
    let resp = tokio::time::timeout(RERANK_EMBED_TIMEOUT, fut)
        .await
        .map_err(|_| anyhow!("embed query timeout"))?
        .map_err(|e| anyhow!("embed query failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("embed query status {status}"));
    }
    let data: serde_json::Value = tokio::time::timeout(RERANK_EMBED_TIMEOUT, resp.json())
        .await
        .map_err(|_| anyhow!("embed query body timeout"))?
        .map_err(|e| anyhow!("embed query parse: {e}"))?;
    let arr = data["data"][0]["embedding"]
        .as_array()
        .ok_or_else(|| anyhow!("embed query: no embedding in response"))?;
    let v: Vec<f32> = arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect();
    if v.is_empty() {
        return Err(anyhow!("embed query: empty embedding"));
    }
    Ok(v)
}

/// Re-ordena `entries` por coseno contra `query_emb`, usando los embeddings ya guardados en DB.
/// Entries SIN embedding (NULL/legacy) conservan su orden FTS relativo al final (score = -1). Es una
/// re-ordenación PURA (no descarta ni agrega entries) → la cobertura del baseline FTS se respeta.
fn rerank_by_embedding(
    conn: &Connection,
    entries: Vec<MemoryEntry>,
    query_emb: &[f32],
) -> Vec<MemoryEntry> {
    // Mapa id→score. Lee el embedding de cada entry (una query parametrizada por id; N entries es el
    // `limit`, chico). Sin embedding o dim-mismatch ⇒ score -1 (queda detrás de los re-rankeados).
    let mut scored: Vec<(f32, usize, MemoryEntry)> = Vec::with_capacity(entries.len());
    for (idx, e) in entries.into_iter().enumerate() {
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM memory_entries WHERE id = ?1",
                params![e.id],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .ok()
            .flatten();
        let score = blob
            .as_deref()
            .and_then(decode_embedding)
            .map(|emb| cosine(query_emb, &emb))
            .unwrap_or(-1.0);
        scored.push((score, idx, e));
    }
    // Orden estable: por score desc; ante empate, por el orden FTS original (idx asc).
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    scored.into_iter().map(|(_, _, e)| e).collect()
}

// --- Types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub source: String,
    pub source_id: Option<String>,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    /// 007 — namespace de proyecto ('__global__' por default, '__shared__' = SoT compartido).
    #[serde(default)]
    pub project_key: String,
    /// 023 — gobierno no-opaco: por qué se guardó (procedencia semántica). NULL en entries legacy.
    #[serde(default)]
    pub rationale: Option<String>,
    /// 023 — clase de memoria (episodic|procedural|project_fact|preference). Default 'episodic'.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 023 — procedencia fina: el CLI que la originó (claude|codex|gemini|aider). NULL si no aplica.
    #[serde(default)]
    pub cli_kind: Option<String>,
    /// 023 — procedencia fina: id de sesión de captura. NULL en entries legacy/manuales.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// 023 — default de `kind` para serde y stores que no lo proveen.
fn default_kind() -> String {
    "episodic".to_string()
}

#[derive(Debug, Deserialize)]
pub struct StoreRequest {
    pub content: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub tags: Option<Vec<String>>,
    /// 007 — proyecto explícito; si falta, se resuelve desde `cwd` (o '__global__').
    pub project_key: Option<String>,
    pub cwd: Option<String>,
    /// 023 — gobierno no-opaco (todos opcionales; defaults seguros si faltan).
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub cli_kind: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    pub limit: Option<usize>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    /// 007 — filtro por proyecto: None/ausente = TODOS (compat retro). include_shared
    /// agrega '__shared__' al filtro (default true).
    pub projects: Option<Vec<String>>,
    pub include_shared: Option<bool>,
    /// 045 FR-003 — pide re-rank vectorial sobre el baseline FTS5 (opt-in; default false → sólo FTS).
    #[serde(default)]
    pub rerank: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct RecallResult {
    pub entries: Vec<MemoryEntry>,
    pub total: usize,
    pub elapsed_ms: u64,
    /// 045 FR-003 — qué motor ordenó los resultados: "fts" (baseline) o "vector" (re-rank por
    /// embeddings). Si el re-rank no se pidió, o el circuit-breaker está abierto, o el embedding
    /// del query falló/timeouteó → "fts" (degradación limpia, NUNCA cuelga). La UI lo muestra como
    /// indicador de calidad. Siempre presente (Serialize-only): "fts" salvo re-rank vectorial exitoso.
    pub backend: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub limit: Option<usize>,
    /// 007 — None = todos (compat retro); include_shared default true.
    pub projects: Option<Vec<String>>,
    pub include_shared: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct MemoryStats {
    pub total_entries: usize,
    pub by_source: Vec<SourceCount>,
    /// 007 — conteo por proyecto (además del total agregado).
    pub by_project: Vec<ProjectCount>,
    pub latest: Option<MemoryEntry>,
}

#[derive(Debug, Serialize)]
pub struct SourceCount {
    pub source: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct ProjectCount {
    pub project_key: String,
    pub count: usize,
}

// --- App State ---

#[derive(Clone)]
pub struct MemDaemonState {
    pub db: Arc<Mutex<Connection>>,
    /// Local bearer token. Validated against incoming `Authorization: Bearer <t>`
    /// header on every non-health endpoint.
    pub token: Arc<String>,
}

// --- Local bearer token management ---

/// Load existing token from Keychain or fall back to ~/.furx/memory-daemon.token.
/// Generates one (32 hex chars = 128 bits) if neither exists.
pub fn ensure_local_token() -> Result<String> {
    // 1. Try Keychain first (macOS). 041 FR-006 — account is the validated current user, with a
    // legacy `hernan` read-fallback so el autor's existing token keeps loading after the change.
    #[cfg(target_os = "macos")]
    {
        let account = crate::services::identity::keychain_account();
        let read = |acct: &str| -> Option<String> {
            let out = std::process::Command::new("/usr/bin/security")
                .args([
                    "find-generic-password",
                    "-a",
                    acct,
                    "-s",
                    TOKEN_KEYCHAIN_SERVICE,
                    "-w",
                ])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        };
        if let Some(t) = read(&account) {
            return Ok(t);
        }
        if account != crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT {
            if let Some(t) = read(crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT) {
                return Ok(t);
            }
        }
    }
    // 2. Fall back to file at ~/.furx/memory-daemon.token (0600).
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home dir"))?;
    let token_file = home.join(".furx").join("memory-daemon.token");
    if let Ok(s) = std::fs::read_to_string(&token_file) {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return Ok(t);
        }
    }
    // 3. Generate a new token (32 hex chars from uuid v4) and store it.
    let new_token = Uuid::new_v4().simple().to_string();
    // Try to write to Keychain under the current account (041 FR-006 — not the hard-coded "hernan").
    #[cfg(target_os = "macos")]
    {
        let account = crate::services::identity::keychain_account();
        let _ = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                &account,
                "-s",
                TOKEN_KEYCHAIN_SERVICE,
                "-j",
                "Furx Memory Hub local bearer (auto-generated)",
                "-w",
                &new_token,
            ])
            .output();
    }
    // Always also persist to file as backup.
    if let Some(parent) = token_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(()) = std::fs::write(&token_file, &new_token) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600));
        }
    }
    Ok(new_token)
}

/// Axum middleware: validates `Authorization: Bearer <token>` on protected endpoints.
/// `/health` is intentionally unauthenticated for liveness checks.
async fn auth_middleware(
    AxState(state): AxState<MemDaemonState>,
    req: Request,
    next: Next,
) -> Result<impl IntoResponse, StatusCode> {
    let headers = req.headers();
    if let Some(hv) = headers.get("authorization") {
        if let Ok(s) = hv.to_str() {
            if let Some(t) = s.strip_prefix("Bearer ") {
                if constant_time_eq(t.trim().as_bytes(), state.token.as_bytes()) {
                    return Ok(next.run(req).await);
                }
            }
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

/// Constant-time byte comparison to avoid timing side-channels on auth.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Sanitize an FTS5 query: strip overly long input, cap term count, escape quotes.
/// Returns the OR-joined quoted-term form ready for `MATCH`.
fn sanitize_fts_query(raw: &str) -> String {
    let truncated: &str = if raw.len() > FTS_QUERY_MAX_CHARS {
        // Walk back to a UTF-8 boundary to avoid splitting multibyte chars.
        let mut idx = FTS_QUERY_MAX_CHARS;
        while idx > 0 && !raw.is_char_boundary(idx) {
            idx -= 1;
        }
        &raw[..idx]
    } else {
        raw
    };
    let terms: Vec<String> = truncated
        .split_whitespace()
        .take(FTS_QUERY_MAX_TERMS)
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect();
    terms.join(" OR ")
}

// --- 007 cross-project: namespacing helpers ---

const PROJECT_GLOBAL: &str = "__global__";
const PROJECT_SHARED: &str = "__shared__";

/// project_key puede ser un path de repo (con espacios/acentos) o un reservado. Los valores
/// van SIEMPRE por placeholders (SQL-safe), así que NO usamos allowlist de caracteres (que
/// rechazaba paths válidos — audit codex+gemini MED): sólo rechazamos vacío, control/NUL y
/// longitudes irreales (PATH_MAX macOS ~1024).
fn valid_project_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 1024 && !k.chars().any(|c| c.is_control())
}

/// Resuelve el proyecto activo desde un cwd: el `projects.path` (canonicalizado) más largo
/// que sea prefijo del cwd. Sin match → None (el caller cae a '__global__'). Council 007.
fn resolve_project_key(conn: &Connection, cwd: &str) -> Option<String> {
    let canon = std::fs::canonicalize(cwd)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| cwd.to_string());
    let mut stmt = conn.prepare("SELECT path FROM projects").ok()?;
    let paths: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ok()?
        .filter_map(|r| r.ok())
        .collect();
    paths
        .into_iter()
        .filter(|p| {
            // Canonicalizar también el path de la DB (en macOS /Users, /tmp… son symlinks;
            // comparar canon-vs-canon evita falsos negativos — audit gemini MED).
            let pc = std::fs::canonicalize(p)
                .ok()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone());
            let pc = pc.trim_end_matches('/');
            canon == pc || canon.starts_with(&format!("{}/", pc))
        })
        .max_by_key(|p| p.len()) // devuelve el projects.path crudo = la key estable
}

/// 023 — wrapper público de `resolve_project_key` para callers fuera del daemon (pty_spawn
/// resuelve el project_key del cwd para la procedencia de la auto-captura).
pub fn resolve_project_key_public(conn: &Connection, cwd: &str) -> Option<String> {
    resolve_project_key(conn, cwd)
}

/// Fragmento SQL `AND <col> IN (?,?,...)` + los valores a bindear. `projects=None` → sin
/// filtro (TODOS, compat retro). include_shared agrega '__shared__'. Valores inválidos se
/// descartan; si queda vacío tras filtrar, no se aplica filtro (devuelve todos).
fn project_filter(
    projects: &Option<Vec<String>>,
    include_shared: bool,
    col: &str,
) -> (String, Vec<String>) {
    let Some(ps) = projects else {
        return (String::new(), vec![]);
    };
    let mut vals: Vec<String> = ps
        .iter()
        .filter(|p| valid_project_key(p))
        .cloned()
        .collect();
    if include_shared && !vals.iter().any(|v| v == PROJECT_SHARED) {
        vals.push(PROJECT_SHARED.to_string());
    }
    if vals.is_empty() {
        // `projects` es Some(...) pero quedó vacío (lista vacía explícita o todo inválido) →
        // CERO filas, NO fail-open a todos los proyectos (audit codex+gemini HIGH). Esto es
        // distinto de `projects=None`, que arriba devuelve sin filtro (= todos, compat retro).
        return (" AND 0".to_string(), vec![]);
    }
    let placeholders = vec!["?"; vals.len()].join(",");
    (format!(" AND {col} IN ({placeholders})"), vals)
}

// --- Handlers ---

async fn handle_store(
    AxState(state): AxState<MemDaemonState>,
    Json(req): Json<StoreRequest>,
) -> Result<Json<MemoryEntry>, String> {
    let id = Uuid::new_v4().to_string();
    let source = req.source.unwrap_or_else(|| "manual".to_string());
    let tags_vec = req.tags.clone().unwrap_or_default();
    let tags_json = serde_json::to_string(&tags_vec).unwrap_or_else(|_| "[]".to_string());
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // 007 — resolver el proyecto: explícito → cwd → '__global__'.
    let project_key = {
        let conn = state.db.lock();
        req.project_key
            .as_deref()
            .filter(|k| valid_project_key(k))
            .map(|k| k.to_string())
            .or_else(|| {
                req.cwd
                    .as_deref()
                    .and_then(|c| resolve_project_key(&conn, c))
            })
            .unwrap_or_else(|| PROJECT_GLOBAL.to_string())
    };

    // 023 — kind con default seguro si el caller no lo provee.
    let kind = req.kind.clone().unwrap_or_else(default_kind);

    {
        // Audit-1 HIGH: wrap both INSERTs in a transaction to keep memory_entries
        // and memory_fts strictly in lock-step. If FTS insert fails (e.g. content
        // too large), we abort the entire write — no orphan row in either table.
        let conn = state.db.lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO memory_entries (id, source, source_id, content, tags, created_at, updated_at, project_key, rationale, kind, cli_kind, session_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![id, source, req.source_id, req.content, tags_json, now, now, project_key, req.rationale, kind, req.cli_kind, req.session_id],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO memory_fts (content, tags) VALUES (?, ?)",
            params![req.content, tags_json],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }

    // Compute embedding async if AIE available
    let id_clone = id.clone();
    let content_clone = req.content.clone();
    let db_clone = state.db.clone();
    tokio::spawn(async move {
        let _ = compute_and_store_embedding(&id_clone, &content_clone, &db_clone).await;
    });

    Ok(Json(MemoryEntry {
        id,
        source,
        source_id: req.source_id,
        content: req.content,
        tags: tags_vec,
        created_at: now,
        project_key,
        rationale: req.rationale,
        kind,
        cli_kind: req.cli_kind,
        session_id: req.session_id,
    }))
}

async fn handle_recall(
    AxState(state): AxState<MemDaemonState>,
    Json(req): Json<RecallRequest>,
) -> Result<Json<RecallResult>, String> {
    let limit = req.limit.unwrap_or(10).min(100);
    let include_shared = req.include_shared.unwrap_or(true);

    // PASO 1 — baseline FTS5 (siempre). El lock se TOMA y SUELTA dentro de este bloque para no
    // cruzar un `await` con la MutexGuard (no es Send) cuando hagamos el re-rank async.
    let mut entries: Vec<MemoryEntry> = {
        let conn = state.db.lock();
        // Audit-1 MED: sanitize_fts_query caps length + term count for anti-DoS.
        let fts_query = sanitize_fts_query(&req.query);
        let query_ref: String = if fts_query.is_empty() {
            req.query.clone()
        } else {
            fts_query
        };
        let mut sql = String::from(
            "SELECT e.id, e.source, e.source_id, e.content, e.tags, e.created_at, e.project_key, e.rationale, e.kind, e.cli_kind, e.session_id
             FROM memory_fts f JOIN memory_entries e ON e.rowid = f.rowid
             WHERE memory_fts MATCH ?",
        );
        let mut binds: Vec<rusqlite::types::Value> = vec![query_ref.into()];
        if let Some(ref src) = req.source {
            sql.push_str(" AND e.source = ?");
            binds.push(src.clone().into());
        }
        let (pfrag, pvals) = project_filter(&req.projects, include_shared, "e.project_key");
        sql.push_str(&pfrag);
        for v in pvals {
            binds.push(v.into());
        }
        sql.push_str(" ORDER BY rank LIMIT ?");
        binds.push((limit as i64).into());

        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows: Vec<MemoryEntry> = stmt
            .query_map(rusqlite::params_from_iter(binds), map_entry)
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };

    // PASO 2 — re-rank vectorial OPT-IN (045 FR-003), con circuit-breaker del embedder. Sólo si el
    // caller lo pidió, hay ≥2 resultados, y el breaker NO está abierto. Cualquier fallo/timeout cae
    // limpio a FTS (NUNCA cuelga; el breaker abre al 3er fallo consecutivo y reintenta a los 60s).
    let mut backend = "fts".to_string();
    if req.rerank.unwrap_or(false) && entries.len() >= 2 && breaker_allows() {
        match embed_query(&req.query).await {
            Ok(query_emb) => {
                breaker_on_success();
                let conn = state.db.lock();
                entries = rerank_by_embedding(&conn, entries, &query_emb);
                backend = "vector".to_string();
            }
            Err(e) => {
                breaker_on_failure();
                tracing::debug!("memory rerank fell back to FTS: {e}");
            }
        }
    }

    let total = entries.len();
    Ok(Json(RecallResult {
        entries,
        total,
        elapsed_ms: 0,
        backend,
    }))
}

async fn handle_search(
    AxState(state): AxState<MemDaemonState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<RecallResult>, String> {
    let _start = std::time::Instant::now();
    let limit = req.limit.unwrap_or(20).min(100);
    let include_shared = req.include_shared.unwrap_or(true);
    let conn = state.db.lock();

    let pattern = format!("%{}%", req.query.replace('%', "\\%").replace('_', "\\_"));
    let mut sql = String::from(
        "SELECT id, source, source_id, content, tags, created_at, project_key, rationale, kind, cli_kind, session_id FROM memory_entries WHERE content LIKE ?",
    );
    let mut binds: Vec<rusqlite::types::Value> = vec![pattern.into()];
    let (pfrag, pvals) = project_filter(&req.projects, include_shared, "project_key");
    sql.push_str(&pfrag);
    for v in pvals {
        binds.push(v.into());
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT ?");
    binds.push((limit as i64).into());

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let entries: Vec<MemoryEntry> = stmt
        .query_map(rusqlite::params_from_iter(binds), map_entry)
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let elapsed = _start.elapsed().as_millis() as u64;
    let total = entries.len();
    Ok(Json(RecallResult {
        entries,
        total,
        elapsed_ms: elapsed,
        // search es LIKE puro (no FTS rank, no re-rank); el baseline es "fts" por convención.
        backend: "fts".to_string(),
    }))
}

async fn handle_stats(
    AxState(state): AxState<MemDaemonState>,
) -> Result<Json<MemoryStats>, String> {
    let conn = state.db.lock();

    let total: usize = conn
        .query_row("SELECT COUNT(*) FROM memory_entries", [], |r| r.get(0))
        .unwrap_or(0);

    let mut stmt = conn
        .prepare(
            "SELECT source, COUNT(*) as cnt FROM memory_entries GROUP BY source ORDER BY cnt DESC",
        )
        .map_err(|e| e.to_string())?;
    let by_source: Vec<SourceCount> = stmt
        .query_map([], |r| {
            Ok(SourceCount {
                source: r.get(0)?,
                count: r.get::<_, i64>(1)? as usize,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    // 007 — conteo por proyecto.
    let mut pstmt = conn.prepare(
        "SELECT project_key, COUNT(*) as cnt FROM memory_entries GROUP BY project_key ORDER BY cnt DESC"
    ).map_err(|e| e.to_string())?;
    let by_project: Vec<ProjectCount> = pstmt
        .query_map([], |r| {
            Ok(ProjectCount {
                project_key: r.get(0)?,
                count: r.get::<_, i64>(1)? as usize,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let latest = conn.query_row(
        "SELECT id, source, source_id, content, tags, created_at, project_key, rationale, kind, cli_kind, session_id FROM memory_entries ORDER BY created_at DESC LIMIT 1",
        [], map_entry,
    ).ok();

    Ok(Json(MemoryStats {
        total_entries: total,
        by_source,
        by_project,
        latest,
    }))
}

fn map_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    let tags_str: String = row.get(4)?;
    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
    Ok(MemoryEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        source_id: row.get(2)?,
        content: row.get(3)?,
        tags,
        created_at: row.get(5)?,
        // 007 — todos los SELECT de map_entry incluyen project_key como columna 6.
        project_key: row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| PROJECT_GLOBAL.to_string()),
        // 023 — procedencia/kind/rationale (cols 7..10). Todos los SELECT de map_entry los incluyen
        // en ESE orden. `kind` cae a 'episodic' si viniera NULL (entries legacy pre-041).
        rationale: row.get::<_, Option<String>>(7)?,
        kind: row
            .get::<_, Option<String>>(8)?
            .unwrap_or_else(default_kind),
        cli_kind: row.get::<_, Option<String>>(9)?,
        session_id: row.get::<_, Option<String>>(10)?,
    })
}

// --- Embedding (async, best-effort) ---

async fn compute_and_store_embedding(
    id: &str,
    content: &str,
    db: &Arc<Mutex<Connection>>,
) -> Result<()> {
    // 039 — in-process cached bearer (was a `/usr/bin/security` subprocess per call).
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or_else(|| anyhow!("no bearer"))?;

    // Audit-1 MED B003: reuse the static HTTP_CLIENT instead of building a new one.
    // BLOQUE J: centralised endpoint resolution (env-overridable, no hard-code).
    let url = format!(
        "{}/v1/embeddings",
        crate::services::aie_endpoint::resolve_url_or_default()
    );
    let resp = HTTP_CLIENT
        .post(&url)
        .header("Authorization", format!("Bearer {}", bearer))
        .json(&serde_json::json!({"model": "nomic-embed-text", "input": content}))
        .send()
        .await
        .map_err(|e| anyhow!("embedding call failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("embedding status {}", status));
    }
    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("parse failed: {}", e))?;
    let embedding = data["data"][0]["embedding"].as_array();

    if let Some(vec) = embedding {
        // Convert f64 vec to f32 bytes
        let bytes: Vec<u8> = vec
            .iter()
            .filter_map(|v| v.as_f64())
            .flat_map(|v| (v as f32).to_le_bytes())
            .collect();
        let id_clone = id.to_string();
        let db = db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock();
            let _ = conn.execute(
                "UPDATE memory_entries SET embedding = ? WHERE id = ?",
                params![bytes, id_clone],
            );
        });
    }
    Ok(())
}

// --- Daemon lifecycle ---

pub async fn start(db: Arc<Mutex<Connection>>) -> Result<()> {
    // Audit-1 HIGH: every non-/health endpoint requires the local bearer token.
    let token = ensure_local_token().map_err(|e| anyhow!("token init: {}", e))?;
    let state = MemDaemonState {
        db,
        token: Arc::new(token),
    };

    // Protected sub-router: middleware checks the Authorization header before any handler runs.
    let protected = Router::new()
        .route("/store", post(handle_store))
        .route("/recall", post(handle_recall))
        .route("/search", post(handle_search))
        .route("/stats", get(handle_stats))
        // F3.1 — Universal Memory Protocol (JSON-RPC 2.0, AIMEAT-compatible)
        .route("/v1/ump", post(handle_ump_rpc))
        .route("/v1/ump/openrpc.json", get(handle_openrpc))
        // F3.4 — Knowledge Graph
        .route("/v1/graph/entities", post(handle_graph_entities))
        .route("/v1/graph/relations", post(handle_graph_relations))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .route(
            "/health",
            get(|| async {
                Json(serde_json::json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}))
            }),
        )
        .merge(protected)
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], MEMORY_PORT));
    tracing::info!("Memory Hub listening on {} (bearer auth required)", addr);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow!("bind: {}", e))?;
    axum::serve(listener, app)
        .await
        .map_err(|e| anyhow!("serve: {}", e))?;
    Ok(())
}

// === F3.1 — Universal Memory Protocol (UMP) ===
// JSON-RPC 2.0 endpoint. Compatible with AIMEAT memory_{store,recall,search,forget} subset.

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
    id: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
    id: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl RpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: Some(result),
            error: None,
            id,
        }
    }
    fn error(id: serde_json::Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
            id,
        }
    }
}

async fn handle_ump_rpc(
    AxState(state): AxState<MemDaemonState>,
    Json(req): Json<RpcRequest>,
) -> Json<RpcResponse> {
    if req.jsonrpc != "2.0" {
        return Json(RpcResponse::error(
            req.id,
            -32600,
            "invalid jsonrpc version",
        ));
    }

    match req.method.as_str() {
        "memory_store" => {
            let content = req
                .params
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let source = req
                .params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("ump");
            // 023 — procedencia/kind/rationale opcionales (gobierno no-opaco). Si alguno viene,
            // usamos store_memory_full; si no, el legacy store_memory (compat retro AIMEAT).
            let str_param = |k: &str| {
                req.params
                    .get(k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            };
            let prov = MemoryProvenance {
                project_key: str_param("project_key").unwrap_or_default(),
                source: source.to_string(),
                source_id: str_param("source_id"),
                cli_kind: str_param("cli_kind"),
                session_id: str_param("session_id"),
                rationale: str_param("rationale"),
                kind: str_param("kind"),
            };
            let has_provenance = !prov.project_key.is_empty()
                || prov.source_id.is_some()
                || prov.cli_kind.is_some()
                || prov.session_id.is_some()
                || prov.rationale.is_some()
                || prov.kind.is_some();
            let stored = if has_provenance {
                store_memory_full(&state.db, &prov, content)
            } else {
                let tags: Vec<&str> = req
                    .params
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                store_memory(&state.db, source, content, &tags)
            };
            match stored {
                Ok(id) => {
                    // Auto-extract entities for knowledge graph
                    let entities = extract_entities(content);
                    if !entities.is_empty() {
                        let conn = state.db.lock();
                        for (name, etype) in &entities {
                            let eid = uuid::Uuid::new_v4().to_string();
                            let _ = conn.execute(
                                "INSERT OR IGNORE INTO memory_entities (id, name, entity_type) VALUES (?, ?, ?)",
                                params![eid, name, etype],
                            );
                        }
                    }
                    Json(RpcResponse::success(
                        req.id,
                        serde_json::json!({"id": id, "entities": entities.len()}),
                    ))
                }
                Err(e) => Json(RpcResponse::error(req.id, -1, e.to_string())),
            }
        }
        "memory_recall" => {
            let query = req
                .params
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let limit = req
                .params
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;
            match recall_memories(&state.db, query, limit) {
                Ok(entries) => {
                    let json = serde_json::to_value(&entries).unwrap_or_default();
                    Json(RpcResponse::success(
                        req.id,
                        serde_json::json!({"entries": json, "total": entries.len()}),
                    ))
                }
                Err(e) => Json(RpcResponse::error(req.id, -1, e.to_string())),
            }
        }
        "memory_forget" => {
            let id = req.params.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let conn = state.db.lock();
            let deleted = conn
                .execute("DELETE FROM memory_entries WHERE id = ?", params![id])
                .unwrap_or(0);
            let _ = conn.execute(
                "DELETE FROM memory_fts WHERE rowid NOT IN (SELECT rowid FROM memory_entries)",
                [],
            );
            Json(RpcResponse::success(
                req.id,
                serde_json::json!({"deleted": deleted}),
            ))
        }
        "memory_forget_project" => {
            let project_key = req
                .params
                .get("project_key")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let include_shared = req
                .params
                .get("include_shared")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match forget_project(&state.db, project_key, include_shared) {
                Ok(deleted) => Json(RpcResponse::success(
                    req.id,
                    serde_json::json!({"deleted": deleted, "project_key": project_key}),
                )),
                Err(e) => Json(RpcResponse::error(req.id, -1, e.to_string())),
            }
        }
        "memory_stats" => match memory_stats(&state.db) {
            Ok(stats) => {
                let json = serde_json::to_value(&stats).unwrap_or_default();
                Json(RpcResponse::success(req.id, json))
            }
            Err(e) => Json(RpcResponse::error(req.id, -1, e.to_string())),
        },
        _ => Json(RpcResponse::error(
            req.id,
            -32601,
            format!("method not found: {}", req.method),
        )),
    }
}

async fn handle_openrpc() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openrpc": "1.0.0",
        "info": { "title": "Furx Universal Memory Protocol", "version": "1.0.0" },
        "methods": [
            { "name": "memory_store", "params": [{ "name": "content", "required": true, "schema": { "type": "string" } }, { "name": "source", "schema": { "type": "string" } }, { "name": "tags", "schema": { "type": "array", "items": { "type": "string" } } }] },
            { "name": "memory_recall", "params": [{ "name": "query", "required": true, "schema": { "type": "string" } }, { "name": "limit", "schema": { "type": "integer" } }] },
            { "name": "memory_forget", "params": [{ "name": "id", "required": true, "schema": { "type": "string" } }] },
            { "name": "memory_forget_project", "params": [{ "name": "project_key", "required": true, "schema": { "type": "string" } }, { "name": "include_shared", "schema": { "type": "boolean" } }] },
            { "name": "memory_stats", "params": [] }
        ]
    }))
}

// === F3.2 — LaunchAgent management ===

const LAUNCH_AGENT_LABEL: &str = "cloud.furx.memory-daemon";
const LAUNCH_AGENT_FILENAME: &str = "cloud.furx.memory-daemon.plist";

pub fn launchagent_plist(furx_bin: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>--memory-daemon</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>{home}/Library/Logs/furx-memory-daemon.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/Library/Logs/furx-memory-daemon.error.log</string>
</dict>
</plist>
"#,
        label = LAUNCH_AGENT_LABEL,
        bin = furx_bin,
        home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
    )
}

pub fn install_launchagent(furx_bin: &str) -> Result<()> {
    let plist = launchagent_plist(furx_bin);
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home dir"))?;
    let launch_agents = home.join("Library").join("LaunchAgents");
    std::fs::create_dir_all(&launch_agents)?;
    let plist_path = launch_agents.join(LAUNCH_AGENT_FILENAME);
    std::fs::write(&plist_path, &plist)?;

    // Bootstrap with launchctl
    let status = std::process::Command::new("launchctl")
        .args(["bootstrap", "gui/1", plist_path.to_str().unwrap()])
        .status()
        .map_err(|e| anyhow!("launchctl bootstrap: {}", e))?;
    if !status.success() {
        // May already be bootstrapped, try bootout first
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", "gui/1", plist_path.to_str().unwrap()])
            .status();
        std::process::Command::new("launchctl")
            .args(["bootstrap", "gui/1", plist_path.to_str().unwrap()])
            .status()
            .map_err(|e| anyhow!("launchctl retry bootstrap: {}", e))?;
    }
    Ok(())
}

pub fn uninstall_launchagent() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home dir"))?;
    let plist_path = home
        .join("Library")
        .join("LaunchAgents")
        .join(LAUNCH_AGENT_FILENAME);
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", "gui/1", plist_path.to_str().unwrap()])
        .status();
    let _ = std::fs::remove_file(&plist_path);
    Ok(())
}

/// True if the Memory Hub is actually serving. This is what the UI surfaces as
/// "memorias andando". The hub runs IN-PROCESS inside the app (lib.rs calls
/// `memory_daemon::start()`), so the old check — "is the LaunchAgent loaded?" —
/// reported "not running" even though the hub was fully functional. We DON'T
/// reload the LaunchAgent here (that would re-introduce the double-bind on 43119),
/// and we DON'T fall back to `launchctl list` (audit-3 Codex: a loaded-but-broken
/// LaunchAgent would report a false "running"). Single source of truth: does the
/// Memory Hub answer HTTP on 43119? (in-process OR LaunchAgent — either way).
pub fn launchagent_status() -> Result<bool> {
    Ok(hub_responding())
}

/// Sync probe: does the MEMORY HUB (specifically) answer on 127.0.0.1:MEMORY_PORT? A bare TCP
/// connect isn't enough, and "any HTTP reply" isn't either (audit-3 Codex). `/health` is the hub's
/// UNAUTHENTICATED liveness route → it returns `200 {"status":"ok","version":...}`. We GET it, read
/// the reply (in a LOOP — a single `read()` can fragment under 5 bytes), and require BOTH the HTTP
/// status line AND the hub's `"status"`/`"ok"` signature in the body. So an unrelated listener on
/// 43119 won't pass. 250ms connect + 500ms read budget.
pub fn hub_responding() -> bool {
    use std::io::{Read, Write};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), MEMORY_PORT);
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if stream
        .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return false;
    }
    // read() puede fragmentar → acumular hasta ~512 bytes o EOF/timeout.
    let mut data = Vec::with_capacity(512);
    let mut chunk = [0u8; 256];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&chunk[..n]);
                if data.len() >= 512 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if !data.starts_with(b"HTTP/") {
        return false;
    }
    // Firma del Memory Hub: el body de /health = {"status":"ok",...}. Tolera espacios en el JSON.
    let s = String::from_utf8_lossy(&data);
    s.contains("\"status\"") && s.contains("\"ok\"")
}

// === F3.3 — CLI Hooks generator ===

/// Generate Claude Code slash commands that talk to the Memory Hub.
pub fn generate_claude_commands(home: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let cmd_dir = home.join(".claude").join("commands");
    std::fs::create_dir_all(&cmd_dir)?;
    let mut created = Vec::new();

    let commands = vec![
        (
            "memory-store.md",
            r#"---
name: memory-store
description: Store a memory in the cross-CLI Memory Hub
---
Store the given text as a memory in the Universal Memory Hub (localhost:43119).
Usage: /memory-store <content> [--source claude] [--tags tag1,tag2]
Call: curl -s http://localhost:43119/v1/ump -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"memory_store","params":{"content":"'"$*"'","source":"claude"},"id":1}'
"#,
        ),
        (
            "memory-recall.md",
            r#"---
name: memory-recall
description: Recall memories from the cross-CLI Memory Hub
---
Search memories by semantic query.
Usage: /memory-recall <query> [--limit 10]
Call: curl -s http://localhost:43119/v1/ump -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"memory_recall","params":{"query":"'"$*"'","limit":10},"id":1}'
"#,
        ),
        (
            "memory-forget.md",
            r#"---
name: memory-forget
description: Delete a memory by ID from the Memory Hub
---
Usage: /memory-forget <memory-id>
"#,
        ),
        (
            "memory-stats.md",
            r#"---
name: memory-stats
description: Show Memory Hub statistics
---
Displays total memories, sources breakdown, and latest entry.
Call: curl -s http://localhost:43119/stats
"#,
        ),
    ];

    for (filename, content) in commands {
        let path = cmd_dir.join(filename);
        if !path.exists() {
            std::fs::write(&path, content)?;
            created.push(path);
        }
    }
    Ok(created)
}

/// Generate hooks.json for Claude Code auto memory capture.
pub fn generate_claude_hooks(home: &std::path::Path) -> Result<std::path::PathBuf> {
    let hooks_dir = home.join(".claude").join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;
    let hooks_path = hooks_dir.join("memory-hooks.json");
    let hooks = serde_json::json!({
        "hooks": {
            "SessionStart": {
                "command": "curl -s http://localhost:43119/v1/ump -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"method\":\"memory_recall\",\"params\":{\"query\":\"session context\",\"limit\":5},\"id\":1}'",
                "type": "command"
            },
            "Stop": [{
                "command": "echo 'Session ended'",
                "type": "command"
            }]
        }
    });
    std::fs::write(&hooks_path, serde_json::to_string_pretty(&hooks)?)?;
    Ok(hooks_path)
}

/// Generate Codex CLI memory commands.
pub fn generate_codex_commands(home: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let cmd_dir = home.join(".codex").join("commands");
    std::fs::create_dir_all(&cmd_dir)?;
    let mut updated = Vec::new();

    let commands = vec![
        ("memory-store.md", "# Memory Store\n\nStores a memory via Furx UMP. Requires the local Memory Hub to be running.\n\n```bash\ncurl -sS http://127.0.0.1:43119/v1/ump -H 'Content-Type: application/json' -H \"Authorization: Bearer $(tr -d '\\n' < \"$HOME/.furx/memory-daemon.token\")\" -d '{\"jsonrpc\":\"2.0\",\"method\":\"memory_store\",\"params\":{\"content\":\"$1\",\"source\":\"codex\"},\"id\":1}'\n```"),
        ("memory-recall.md", "# Memory Recall\n\nRecalls cross-session memories from Furx UMP. Requires the local Memory Hub to be running.\n\n```bash\ncurl -sS http://127.0.0.1:43119/v1/ump -H 'Content-Type: application/json' -H \"Authorization: Bearer $(tr -d '\\n' < \"$HOME/.furx/memory-daemon.token\")\" -d '{\"jsonrpc\":\"2.0\",\"method\":\"memory_recall\",\"params\":{\"query\":\"$1\",\"limit\":10},\"id\":1}'\n```"),
        ("council-maximo.md", "# Consejo Maximo Furx\n\nRuns the native Furx multi-provider council. It reuses BYOK providers, resilience, history, audit, and the AIE/Codex fallback. Current native maximum: 6 voices.\n\n```bash\nFURX_BIN=\"${FURX_BIN:-/Applications/Furx.app/Contents/MacOS/furx}\"\nprintf '%s' \"$1\" | \"$FURX_BIN\" council --stdin --preset mix --max-voices 6\n```\n\nUse `--template planning`, `implementation`, `review`, `debug`, or `refactor` when the task has a clear phase."),
    ];

    for (filename, content) in commands {
        let path = cmd_dir.join(filename);
        let needs_update = std::fs::read_to_string(&path)
            .map(|existing| existing != content)
            .unwrap_or(true);
        if needs_update {
            std::fs::write(&path, content)?;
            updated.push(path);
        }
    }
    Ok(updated)
}

// === F3.4 — Knowledge Graph ===
// Simple entity + relation tracking for cross-source memory connections.

#[derive(Debug, Clone, Serialize)]
pub struct GraphEntity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub metadata: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphRelation {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
    pub weight: f64,
    pub created_at: String,
}

async fn handle_graph_entities(
    AxState(state): AxState<MemDaemonState>,
) -> Result<Json<Vec<GraphEntity>>, String> {
    let conn = state.db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, entity_type, metadata, created_at FROM memory_entities ORDER BY name LIMIT 100"
    ).map_err(|e| e.to_string())?;
    let entities = stmt
        .query_map([], |r| {
            Ok(GraphEntity {
                id: r.get(0)?,
                name: r.get(1)?,
                entity_type: r.get(2)?,
                metadata: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(entities))
}

async fn handle_graph_relations(
    AxState(state): AxState<MemDaemonState>,
) -> Result<Json<Vec<GraphRelation>>, String> {
    let conn = state.db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, relation_type, weight, created_at FROM memory_relations ORDER BY weight DESC LIMIT 100"
    ).map_err(|e| e.to_string())?;
    let relations = stmt
        .query_map([], |r| {
            Ok(GraphRelation {
                id: r.get(0)?,
                source_id: r.get(1)?,
                target_id: r.get(2)?,
                relation_type: r.get(3)?,
                weight: r.get::<_, f64>(4)?,
                created_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(relations))
}

/// Extract simple entities from memory content (best-effort, regex-based).
pub fn extract_entities(content: &str) -> Vec<(String, String)> {
    let mut entities = Vec::new();
    // Capitalized multi-word phrases (potential named entities)
    for word in content.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() >= 3
            && clean
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
        {
            entities.push((clean.to_string(), "term".to_string()));
        }
    }
    // URLs
    for token in content.split_whitespace() {
        if token.starts_with("http://") || token.starts_with("https://") {
            entities.push((token.to_string(), "url".to_string()));
        }
    }
    entities.dedup();
    entities
}

/// Store a memory entry directly (for in-process (Furx) use).
pub fn store_memory(
    db: &Mutex<Connection>,
    source: &str,
    content: &str,
    tags: &[&str],
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let tags_json = serde_json::to_string(tags)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO memory_entries (id, source, content, tags, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
            params![id, source, content, tags_json, now, now],
        )?;
        let _ = conn.execute(
            "INSERT INTO memory_fts (content, tags) VALUES (?, ?)",
            params![content, tags_json],
        );
    }
    Ok(id)
}

/// 023 — procedencia + gobierno: campos que toda escritura no-opaca debe registrar.
#[derive(Debug, Clone, Default)]
pub struct MemoryProvenance {
    pub project_key: String,
    pub source: String,
    pub source_id: Option<String>,
    pub cli_kind: Option<String>,
    pub session_id: Option<String>,
    pub rationale: Option<String>,
    pub kind: Option<String>,
}

/// 023 — store in-process CON procedencia/kind/rationale (lo usa el accept de la bandeja).
/// `content` ya debe venir scrubeado (el caller re-scrubea idempotente). Atómico:
/// memory_entries + memory_fts en una transacción (igual que handle_store).
pub fn store_memory_full(db: &Mutex<Connection>, prov: &MemoryProvenance, content: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let project_key = if prov.project_key.is_empty() {
        PROJECT_GLOBAL.to_string()
    } else {
        prov.project_key.clone()
    };
    let kind = prov.kind.clone().unwrap_or_else(default_kind);
    {
        let conn = db.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO memory_entries (id, source, source_id, content, tags, created_at, updated_at, project_key, rationale, kind, cli_kind, session_id)
             VALUES (?, ?, ?, ?, '[]', ?, ?, ?, ?, ?, ?, ?)",
            params![id, prov.source, prov.source_id, content, now, now, project_key, prov.rationale, kind, prov.cli_kind, prov.session_id],
        )?;
        tx.execute(
            "INSERT INTO memory_fts (content, tags) VALUES (?, '[]')",
            params![content],
        )?;
        tx.commit()?;
    }
    Ok(id)
}

/// 023 — forget por PROYECTO: borra entries + filas FTS huérfanas del proyecto. Por defecto
/// NUNCA toca `__shared__` (SoT compartido); `include_shared=true` lo permite explícitamente.
/// Devuelve la cantidad de entries borradas. Atómico (transacción).
pub fn forget_project(db: &Mutex<Connection>, project_key: &str, include_shared: bool) -> Result<usize> {
    if !valid_project_key(project_key) {
        return Err(anyhow!("project_key inválido"));
    }
    let conn = db.lock();
    let tx = conn.unchecked_transaction()?;
    let deleted = if project_key == PROJECT_SHARED && !include_shared {
        // Pedir borrar __shared__ sin opt-in es un no-op deliberado (gobierno).
        0
    } else if include_shared {
        tx.execute(
            "DELETE FROM memory_entries WHERE project_key = ?",
            params![project_key],
        )?
    } else {
        tx.execute(
            "DELETE FROM memory_entries WHERE project_key = ? AND project_key != ?",
            params![project_key, PROJECT_SHARED],
        )?
    };
    // Reconciliar memory_fts: borrar filas FTS sin entry correspondiente (igual que memory_forget).
    tx.execute(
        "DELETE FROM memory_fts WHERE rowid NOT IN (SELECT rowid FROM memory_entries)",
        [],
    )?;
    tx.commit()?;
    Ok(deleted)
}

/// 025 — forget de UNA entry por id (lo usa `lesson_delete`). Borra la entry + reconcilia FTS.
/// Atómico. Devuelve true si borró algo.
pub fn forget_entry(db: &Mutex<Connection>, id: &str) -> Result<bool> {
    let conn = db.lock();
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute("DELETE FROM memory_entries WHERE id = ?", params![id])?;
    tx.execute(
        "DELETE FROM memory_fts WHERE rowid NOT IN (SELECT rowid FROM memory_entries)",
        [],
    )?;
    tx.commit()?;
    Ok(n > 0)
}

/// Recall memories by semantic content.
pub fn recall_memories(
    db: &Mutex<Connection>,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>> {
    let conn = db.lock();
    let query_param = query
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" OR ");

    let mut stmt = conn.prepare(
        "SELECT e.id, e.source, e.source_id, e.content, e.tags, e.created_at, e.project_key, e.rationale, e.kind, e.cli_kind, e.session_id
         FROM memory_fts f
         JOIN memory_entries e ON e.rowid = f.rowid
         WHERE memory_fts MATCH ?
         ORDER BY rank LIMIT ?",
    )?;
    let entries = stmt
        .query_map(params![query_param, limit as i64], map_entry)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(entries)
}

/// 045 FR-003 — recall con re-rank vectorial OPT-IN + circuit-breaker del embedder, devolviendo el
/// `backend` usado ("fts" | "vector") para transparencia. Es la versión async del recall del comando
/// Tauri: FTS5 baseline (siempre) y, si `rerank` y el breaker lo permite, re-ordena por coseno de
/// embeddings con timeout duro. Cualquier fallo/timeout cae limpio a FTS (NUNCA cuelga). El baseline
/// FTS sigue intacto en `recall_memories` (compat).
pub async fn recall_memories_ranked(
    db: &Arc<Mutex<Connection>>,
    query: &str,
    limit: usize,
    rerank: bool,
) -> Result<RecallResult> {
    // PASO 1 — FTS5 baseline (lock tomado y soltado antes de cualquier await).
    let mut entries: Vec<MemoryEntry> = {
        let conn = db.lock();
        let query_param = query
            .split_whitespace()
            .map(|w| format!("\"{}\"", w.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut stmt = conn.prepare(
            "SELECT e.id, e.source, e.source_id, e.content, e.tags, e.created_at, e.project_key, e.rationale, e.kind, e.cli_kind, e.session_id
             FROM memory_fts f
             JOIN memory_entries e ON e.rowid = f.rowid
             WHERE memory_fts MATCH ?
             ORDER BY rank LIMIT ?",
        )?;
        let rows: Vec<MemoryEntry> = stmt
            .query_map(params![query_param, limit as i64], map_entry)?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };

    // PASO 2 — re-rank opt-in con circuit-breaker.
    let mut backend = "fts".to_string();
    if rerank && entries.len() >= 2 && breaker_allows() {
        match embed_query(query).await {
            Ok(query_emb) => {
                breaker_on_success();
                let conn = db.lock();
                entries = rerank_by_embedding(&conn, entries, &query_emb);
                backend = "vector".to_string();
            }
            Err(e) => {
                breaker_on_failure();
                tracing::debug!("memory recall_ranked fell back to FTS: {e}");
            }
        }
    }

    let total = entries.len();
    Ok(RecallResult {
        entries,
        total,
        elapsed_ms: 0,
        backend,
    })
}

/// Get memory stats.
pub fn memory_stats(db: &Mutex<Connection>) -> Result<MemoryStats> {
    let conn = db.lock();
    let total = conn
        .query_row("SELECT COUNT(*) FROM memory_entries", [], |r| r.get(0))
        .unwrap_or(0);
    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) as cnt FROM memory_entries GROUP BY source ORDER BY cnt DESC",
    )?;
    let by_source = stmt
        .query_map([], |r| {
            Ok(SourceCount {
                source: r.get(0)?,
                count: r.get::<_, i64>(1)? as usize,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    // 007 — conteo por proyecto.
    let mut pstmt = conn.prepare(
        "SELECT project_key, COUNT(*) as cnt FROM memory_entries GROUP BY project_key ORDER BY cnt DESC"
    )?;
    let by_project = pstmt
        .query_map([], |r| {
            Ok(ProjectCount {
                project_key: r.get(0)?,
                count: r.get::<_, i64>(1)? as usize,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    let latest = conn.query_row(
        "SELECT id, source, source_id, content, tags, created_at, project_key, rationale, kind, cli_kind, session_id FROM memory_entries ORDER BY created_at DESC LIMIT 1",
        [], map_entry,
    ).ok();

    Ok(MemoryStats {
        total_entries: total,
        by_source,
        by_project,
        latest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_entries (
                id TEXT PRIMARY KEY, source TEXT, source_id TEXT, content TEXT,
                embedding BLOB, tags TEXT, created_at TEXT, updated_at TEXT,
                project_key TEXT NOT NULL DEFAULT '__global__',
                rationale TEXT, kind TEXT NOT NULL DEFAULT 'episodic',
                cli_kind TEXT, session_id TEXT
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(content, tags);
            CREATE TABLE IF NOT EXISTS projects (path TEXT PRIMARY KEY, name TEXT, shared_source TEXT);"
        ).unwrap();
        Arc::new(Mutex::new(conn))
    }

    // --- 007 cross-project tests ---

    #[test]
    fn valid_project_key_paths_and_reserved() {
        assert!(valid_project_key("__global__"));
        assert!(valid_project_key("__shared__"));
        assert!(valid_project_key("/Users/dev/furx"));
        assert!(valid_project_key("/Users/dev/My Project")); // espacios OK (parametrizado)
        assert!(valid_project_key("furx"));
        assert!(!valid_project_key(""));
        assert!(!valid_project_key("with\u{0}nul")); // control/NUL → false
        assert!(!valid_project_key(&"x".repeat(1025))); // >1024
    }

    #[test]
    fn project_filter_semantics() {
        // None = todos → sin fragmento (compat retro)
        let (frag, vals) = project_filter(&None, true, "e.project_key");
        assert!(frag.is_empty() && vals.is_empty());
        // Some([furx]) + include_shared → IN (?,?) con [furx, __shared__]
        let (frag, vals) = project_filter(&Some(vec!["furx".into()]), true, "e.project_key");
        assert!(frag.contains("IN (?,?)"));
        assert_eq!(vals, vec!["furx".to_string(), "__shared__".to_string()]);
        // include_shared=false → sólo el pedido
        let (_f, vals) = project_filter(&Some(vec!["furx".into()]), false, "e.project_key");
        assert_eq!(vals, vec!["furx".to_string()]);
        // Some explícito que queda vacío (todo inválido / lista vacía) → CERO filas, NO todos
        let (frag, v) = project_filter(&Some(vec!["".into()]), false, "e.project_key");
        assert_eq!(frag, " AND 0");
        assert!(v.is_empty());
        let (frag, _v) = project_filter(&Some(vec![]), false, "e.project_key");
        assert_eq!(frag, " AND 0");
    }

    #[test]
    fn resolve_project_key_longest_prefix() {
        let db = setup_db();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (path, name) VALUES ('/tmp/proj', 'p')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (path, name) VALUES ('/tmp/proj/deep', 'd')",
            [],
        )
        .unwrap();
        // cwd inexistente → canonicalize falla → comparación literal por prefijo
        assert_eq!(
            resolve_project_key(&conn, "/tmp/proj/deep/x").as_deref(),
            Some("/tmp/proj/deep")
        );
        assert_eq!(
            resolve_project_key(&conn, "/tmp/proj/other").as_deref(),
            Some("/tmp/proj")
        );
        assert_eq!(resolve_project_key(&conn, "/elsewhere"), None);
    }

    #[test]
    fn store_defaults_global_and_search_filters_by_project() {
        let db = setup_db();
        // dos memorias en proyectos distintos + una shared
        {
            let conn = db.lock();
            let now = "2026-01-01T00:00:00Z";
            for (id, content, pk) in [
                ("a", "alpha furx note", "furx"),
                ("b", "beta toga note", "toga"),
                ("c", "gamma shared note", "__shared__"),
            ] {
                conn.execute(
                    "INSERT INTO memory_entries (id, source, content, tags, created_at, updated_at, project_key)
                     VALUES (?, 'manual', ?, '[]', ?, ?, ?)",
                    params![id, content, now, now, pk],
                ).unwrap();
            }
        }
        // filtro por 'furx' + include_shared → furx + shared, no toga
        let (frag, vals) = project_filter(&Some(vec!["furx".into()]), true, "project_key");
        let sql = format!("SELECT id, source, source_id, content, tags, created_at, project_key, rationale, kind, cli_kind, session_id FROM memory_entries WHERE content LIKE ?{frag} ORDER BY created_at DESC");
        let conn = db.lock();
        let mut binds: Vec<rusqlite::types::Value> = vec![String::from("%note%").into()];
        for v in vals {
            binds.push(v.into());
        }
        let mut stmt = conn.prepare(&sql).unwrap();
        let got: Vec<MemoryEntry> = stmt
            .query_map(rusqlite::params_from_iter(binds), map_entry)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let pks: Vec<&str> = got.iter().map(|e| e.project_key.as_str()).collect();
        assert!(pks.contains(&"furx") && pks.contains(&"__shared__"));
        assert!(!pks.contains(&"toga"));
    }

    #[test]
    fn test_store_via_function() {
        let db = setup_db();
        let id = store_memory(&db, "test", "Hello from store fn", &["unit", "test"]).unwrap();
        assert!(!id.is_empty());
        let stats = memory_stats(&db).unwrap();
        assert_eq!(stats.total_entries, 1);
    }

    #[test]
    fn test_recall_empty() {
        let db = setup_db();
        let results = recall_memories(&db, "nonexistent", 10).unwrap();
        assert!(results.is_empty());
    }

    // --- 045 FR-003: re-rank vectorial + circuit-breaker del embedder ---

    #[test]
    fn cosine_basic() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // dims distintas o vector cero → 0.0 (degradación limpia, no NaN).
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }

    #[test]
    fn decode_embedding_roundtrip() {
        let v: Vec<f32> = vec![0.5, -1.25, 3.0];
        let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(decode_embedding(&bytes), Some(v));
        assert_eq!(decode_embedding(&[]), None); // vacío.
        assert_eq!(decode_embedding(&[1, 2, 3]), None); // no múltiplo de 4.
    }

    #[test]
    fn rerank_orders_by_cosine_and_keeps_coverage() {
        // SC-003: el re-rank es una RE-ORDENACIÓN pura (no descarta entries). Un entry sin embedding
        // queda detrás de los re-rankeados, pero NO se pierde.
        let db = setup_db();
        let now = "2026-01-01T00:00:00Z";
        // query_emb apunta a [1,0]; 'b' está alineado (cos=1), 'a' ortogonal (cos=0), 'c' sin embedding.
        let emb_a: Vec<u8> = [0.0f32, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let emb_b: Vec<u8> = [1.0f32, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        {
            let conn = db.lock();
            conn.execute("INSERT INTO memory_entries (id, source, content, embedding, tags, created_at, updated_at, project_key) VALUES ('a','manual','a',?,'[]',?,?,'__global__')", params![emb_a, now, now]).unwrap();
            conn.execute("INSERT INTO memory_entries (id, source, content, embedding, tags, created_at, updated_at, project_key) VALUES ('b','manual','b',?,'[]',?,?,'__global__')", params![emb_b, now, now]).unwrap();
            conn.execute("INSERT INTO memory_entries (id, source, content, tags, created_at, updated_at, project_key) VALUES ('c','manual','c','[]',?,?,'__global__')", params![now, now]).unwrap();
        }
        let mk = |id: &str| MemoryEntry {
            id: id.into(), source: "manual".into(), source_id: None, content: id.into(),
            tags: vec![], created_at: now.into(), project_key: "__global__".into(),
            rationale: None, kind: "episodic".into(), cli_kind: None, session_id: None,
        };
        let entries = vec![mk("a"), mk("c"), mk("b")]; // orden FTS arbitrario.
        let conn = db.lock();
        let ranked = rerank_by_embedding(&conn, entries, &[1.0, 0.0]);
        let ids: Vec<&str> = ranked.iter().map(|e| e.id.as_str()).collect();
        // 'b' (cos=1) primero, 'a' (cos=0) después, 'c' (sin embedding, score=-1) último. Los 3 siguen.
        assert_eq!(ids, vec!["b", "a", "c"]);
    }

    #[test]
    fn breaker_opens_after_threshold_and_resets() {
        // Aísla el estado global del breaker al inicio del test (otros tests no lo tocan).
        breaker_on_success();
        assert!(breaker_allows(), "cerrado al inicio");
        // 1er y 2º fallo: NO abre todavía.
        breaker_on_failure();
        breaker_on_failure();
        assert!(breaker_allows(), "2 fallos < umbral 3 → sigue cerrado");
        // 3er fallo consecutivo: ABRE → no permite (sin esperar el reset).
        breaker_on_failure();
        assert!(!breaker_allows(), "3 fallos → breaker abierto");
        // Un éxito lo cierra y resetea el contador.
        breaker_on_success();
        assert!(breaker_allows(), "success cierra el breaker");
        // limpieza para no leakear estado al resto del suite.
        breaker_on_success();
    }

    #[test]
    fn test_stats_empty() {
        let db = setup_db();
        let stats = memory_stats(&db).unwrap();
        assert_eq!(stats.total_entries, 0);
        assert!(stats.latest.is_none());
    }

    // --- Audit-1 (2026-05-27) tests ---

    #[test]
    fn fts_sanitize_truncates_long_input() {
        let raw = "a".repeat(2048);
        let q = sanitize_fts_query(&raw);
        // sanitize_fts_query returns OR-joined "word" forms. Truncation keeps a
        // single quoted token capped at FTS_QUERY_MAX_CHARS + quote overhead.
        assert!(q.len() <= FTS_QUERY_MAX_CHARS + 4, "len={}", q.len());
    }

    #[test]
    fn fts_sanitize_caps_term_count() {
        let raw = (0..100)
            .map(|i| format!("w{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let q = sanitize_fts_query(&raw);
        let terms = q.matches(" OR ").count() + if q.is_empty() { 0 } else { 1 };
        assert!(terms <= FTS_QUERY_MAX_TERMS, "terms={}", terms);
    }

    #[test]
    fn fts_sanitize_strips_inner_quotes() {
        let q = sanitize_fts_query(r#"hello "world" "#);
        assert!(!q.contains(r#""hello""world"#), "q={}", q);
        assert!(q.contains("hello"));
    }

    #[test]
    fn constant_time_eq_matches_only_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    // --- 023 — store con procedencia/kind + forget por proyecto ---

    #[test]
    fn store_full_persists_rationale_kind_and_provenance() {
        let db = setup_db();
        let prov = MemoryProvenance {
            project_key: "furx".into(),
            source: "codex".into(),
            source_id: Some("pane-3".into()),
            cli_kind: Some("codex".into()),
            session_id: Some("sess-9".into()),
            rationale: Some("decisión de diseño tomada en la sesión".into()),
            kind: Some("procedural".into()),
        };
        let id = store_memory_full(&db, &prov, "usar tabla propia memory_proposals").unwrap();
        assert!(!id.is_empty());
        // recall (FTS) devuelve la entry CON los campos nuevos.
        let entries = recall_memories(&db, "memory_proposals", 10).unwrap();
        let e = entries.iter().find(|e| e.id == id).expect("entry recalled");
        assert_eq!(e.kind, "procedural");
        assert_eq!(e.rationale.as_deref(), Some("decisión de diseño tomada en la sesión"));
        assert_eq!(e.cli_kind.as_deref(), Some("codex"));
        assert_eq!(e.session_id.as_deref(), Some("sess-9"));
        assert_eq!(e.project_key, "furx");
        assert_eq!(e.source_id.as_deref(), Some("pane-3"));
    }

    #[test]
    fn store_full_defaults_kind_to_episodic_when_absent() {
        let db = setup_db();
        let prov = MemoryProvenance {
            project_key: "furx".into(),
            source: "manual".into(),
            ..Default::default()
        };
        let id = store_memory_full(&db, &prov, "nota sin kind explicito").unwrap();
        let entries = recall_memories(&db, "nota", 10).unwrap();
        let e = entries.iter().find(|e| e.id == id).unwrap();
        assert_eq!(e.kind, "episodic", "default seguro es episodic");
        assert!(e.rationale.is_none());
    }

    #[test]
    fn forget_project_deletes_only_that_project_and_keeps_shared() {
        let db = setup_db();
        for (pk, content) in [
            ("furx", "alpha furx note"),
            ("furx", "beta furx note"),
            ("toga", "gamma toga note"),
            ("__shared__", "delta shared note"),
        ] {
            let prov = MemoryProvenance {
                project_key: pk.into(),
                source: "manual".into(),
                ..Default::default()
            };
            store_memory_full(&db, &prov, content).unwrap();
        }
        // forget furx (sin shared) → borra 2, deja toga + shared.
        let deleted = forget_project(&db, "furx", false).unwrap();
        assert_eq!(deleted, 2);
        let stats = memory_stats(&db).unwrap();
        let pks: Vec<&str> = stats.by_project.iter().map(|p| p.project_key.as_str()).collect();
        assert!(!pks.contains(&"furx"), "furx borrado");
        assert!(pks.contains(&"toga"), "toga intacto");
        assert!(pks.contains(&"__shared__"), "shared intacto");
        // FTS consistente: una búsqueda de 'furx' ya no devuelve nada de ese proyecto.
        let left = recall_memories(&db, "furx", 10).unwrap();
        assert!(left.is_empty(), "FTS reconciliado tras forget");
    }

    #[test]
    fn forget_project_shared_is_noop_without_optin() {
        let db = setup_db();
        let prov = MemoryProvenance {
            project_key: "__shared__".into(),
            source: "manual".into(),
            ..Default::default()
        };
        store_memory_full(&db, &prov, "shared sot principle").unwrap();
        // Pedir borrar __shared__ SIN include_shared → no-op (gobierno).
        let deleted = forget_project(&db, "__shared__", false).unwrap();
        assert_eq!(deleted, 0);
        // Con opt-in explícito sí borra.
        let deleted2 = forget_project(&db, "__shared__", true).unwrap();
        assert_eq!(deleted2, 1);
    }

    #[test]
    fn ensure_local_token_returns_non_empty() {
        // Smoke-test only — Keychain access may be denied in CI; the function
        // falls back to a file-backed token in that case.
        let t = ensure_local_token().expect("token");
        assert!(!t.is_empty());
        assert!(t.len() >= 16);
    }

    #[test]
    fn codex_memory_commands_are_authenticated_and_refreshed() {
        let home = std::env::temp_dir().join(format!("furx-codex-hooks-{}", Uuid::new_v4()));
        let cmd_dir = home.join(".codex").join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        let recall_path = cmd_dir.join("memory-recall.md");
        std::fs::write(&recall_path, "stale").unwrap();

        let updated = generate_codex_commands(&home).unwrap();
        assert_eq!(updated.len(), 3);

        let recall = std::fs::read_to_string(&recall_path).unwrap();
        assert!(recall.contains("Authorization: Bearer"));
        assert!(recall.contains("$HOME/.furx/memory-daemon.token"));
        let council = std::fs::read_to_string(cmd_dir.join("council-maximo.md")).unwrap();
        assert!(council.contains("council --stdin"));
        assert!(council.contains("--max-voices 6"));

        let unchanged = generate_codex_commands(&home).unwrap();
        assert!(unchanged.is_empty());

        std::fs::remove_dir_all(home).unwrap();
    }
}
