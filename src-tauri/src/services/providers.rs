// services/providers.rs — BYOK provider registry.
// Implementa los 5 caminos del wizard Furx Connect.
// Council BLOQUE 1: 5/5 aprobaron schema con last_error_msg + UUID install_id + Keychain idempotente.

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::services::keychain;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenRouter,
    Cerebras,
    Groq,
    Mistral,
    SambaNova,
    GeminiStudio, // Google AI Studio free
    Anthropic,    // API paga (NO OAuth Max)
    OpenAI,
    GeminiPaid,
    Ollama,
    LMStudio,
    LlamaCpp,
    VLLM,
    LiteLLM,
    Custom,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Cerebras => "cerebras",
            Self::Groq => "groq",
            Self::Mistral => "mistral",
            Self::SambaNova => "sambanova",
            Self::GeminiStudio => "gemini_studio",
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::GeminiPaid => "gemini_paid",
            Self::Ollama => "ollama",
            Self::LMStudio => "lmstudio",
            Self::LlamaCpp => "llamacpp",
            Self::VLLM => "vllm",
            Self::LiteLLM => "litellm",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "openrouter" => Self::OpenRouter,
            "cerebras" => Self::Cerebras,
            "groq" => Self::Groq,
            "mistral" => Self::Mistral,
            "sambanova" => Self::SambaNova,
            "gemini_studio" => Self::GeminiStudio,
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAI,
            "gemini_paid" => Self::GeminiPaid,
            "ollama" => Self::Ollama,
            "lmstudio" => Self::LMStudio,
            "llamacpp" => Self::LlamaCpp,
            "vllm" => Self::VLLM,
            "litellm" => Self::LiteLLM,
            "custom" => Self::Custom,
            _ => return None,
        })
    }

    /// Default endpoint base URL (Chat Completions OpenAI-compatible).
    pub fn default_endpoint(&self) -> Option<&'static str> {
        match self {
            Self::OpenRouter => Some("https://openrouter.ai/api/v1"),
            Self::Cerebras => Some("https://api.cerebras.ai/v1"),
            Self::Groq => Some("https://api.groq.com/openai/v1"),
            Self::Mistral => Some("https://api.mistral.ai/v1"),
            Self::SambaNova => Some("https://api.sambanova.ai/v1"),
            Self::GeminiStudio | Self::GeminiPaid => {
                Some("https://generativelanguage.googleapis.com/v1beta/openai")
            }
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
            Self::OpenAI => Some("https://api.openai.com/v1"),
            Self::Ollama => Some("http://127.0.0.1:11434/v1"),
            Self::LMStudio => Some("http://127.0.0.1:1234/v1"),
            Self::LlamaCpp => Some("http://127.0.0.1:8080/v1"),
            Self::VLLM | Self::LiteLLM | Self::Custom => None, // user-provided
        }
    }

    /// Default lightweight model used by ping test.
    pub fn default_ping_model(&self) -> &'static str {
        match self {
            Self::OpenRouter => "openai/gpt-4o-mini",
            // H2: this must be a Cerebras model id (was the Groq-only
            // "llama-3.1-8b-instant", so every Cerebras ping/voice failed
            // model-not-found). Update per Cerebras' current catalog if it changes.
            Self::Cerebras => "llama-3.3-70b",
            Self::Groq => "llama-3.1-8b-instant",
            Self::Mistral => "mistral-small-latest",
            Self::SambaNova => "Meta-Llama-3.1-8B-Instruct",
            Self::GeminiStudio | Self::GeminiPaid => "gemini-2.0-flash",
            Self::Anthropic => "claude-haiku-4-5",
            Self::OpenAI => "gpt-4o-mini",
            Self::Ollama => "llama3.2:1b",
            Self::LMStudio | Self::LlamaCpp | Self::VLLM | Self::LiteLLM | Self::Custom => "auto",
        }
    }

    /// Whether the provider uses Authorization header with the user's key.
    /// Local providers (Ollama, llama.cpp default) usually don't.
    /// Whether the provider requires the user to supply a key.
    /// LOW fix (Codex B2): LiteLLM/vLLM/Custom now treat key as OPTIONAL so users with
    /// self-hosted no-auth gateways can save them. Local providers stay key-less.
    pub fn needs_key(&self) -> bool {
        !matches!(
            self,
            Self::Ollama
                | Self::LMStudio
                | Self::LlamaCpp
                | Self::VLLM
                | Self::LiteLLM
                | Self::Custom
        )
    }

    /// Whether this is a self-hosted local provider whose installed models we can't
    /// assume. The health ping for these MUST NOT require a hardcoded model name
    /// (the user may not have `default_ping_model()` pulled) — see `test_ping`.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Ollama | Self::LMStudio | Self::LlamaCpp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCredential {
    pub alias: String,
    pub provider: String,
    pub key_ref: Option<String>,
    pub endpoint_url: Option<String>,
    pub status: String, // "healthy" | "amber" | "red" | "unconfigured"
    pub last_ping_ms: Option<i64>,
    pub last_ping_at: Option<String>,
    pub last_error_msg: Option<String>,
    pub scope_workspace: Option<String>,
    pub preset_member: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    pub ok: bool,
    pub latency_ms: u64,
    pub model: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalProviderInfo {
    pub kind: String, // "ollama" | "lmstudio" | "llamacpp"
    pub alive: bool,
    pub endpoint: String,
    pub models: Vec<String>, // list of model names exposed (Ollama tags / OAI models endpoint)
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalScan {
    pub ollama: LocalProviderInfo,
    pub lmstudio: LocalProviderInfo,
    pub llamacpp: LocalProviderInfo,
    pub scanned_at: String,
}

/// Scan default local LLM endpoints (Ollama 11434, LM Studio 1234, llama.cpp 8080).
/// Council EDGE_5: schema-validate response body to avoid false-positives from non-LLM
/// services that happen to listen on those ports.
pub async fn local_scan() -> LocalScan {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let (ollama, lmstudio, llamacpp) = tokio::join!(
        scan_ollama(&client),
        scan_lmstudio(&client),
        scan_llamacpp(&client),
    );
    LocalScan {
        ollama,
        lmstudio,
        llamacpp,
        scanned_at: Utc::now().to_rfc3339(),
    }
}

// MED fix (Codex B2 + Gemini B2): bound the body read before `bytes().await` buffers it whole.
// Streams chunks via reqwest's native chunk() (no futures_util import needed), truncates at
// MAX, drops the rest. Defends against malicious endpoints returning multi-GB bodies on
// localhost / proxy.
async fn read_bounded(mut resp: reqwest::Response, max: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 * 1024);
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = std::cmp::min(chunk.len(), max.saturating_sub(buf.len()));
                if take == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..take]);
                if buf.len() >= max {
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    buf
}

async fn scan_ollama(client: &reqwest::Client) -> LocalProviderInfo {
    let url = "http://127.0.0.1:11434/api/tags";
    let endpoint = "http://127.0.0.1:11434/v1".to_string();
    let start = Instant::now();
    let result = client.get(url).send().await;
    let latency_ms = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) if resp.status().is_success() => {
            // MED fix (Codex B2): bounded read instead of resp.bytes() — avoid loading
            // an attacker-controlled body fully into memory.
            let body = read_bounded(resp, 1024 * 1024).await;
            let parsed: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
            let models: Vec<String> = parsed
                .as_ref()
                .and_then(|v| {
                    v.get("models").and_then(|x| x.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                m.get("name").and_then(|n| n.as_str()).map(String::from)
                            })
                            .collect()
                    })
                })
                .unwrap_or_default();
            // MED fix (Codex B2): schema MUST match — no latency-based heuristic.
            // Ollama responses always include a top-level "models" array (empty if no
            // models installed, but the key is present). If absent → not Ollama.
            let has_models_key = parsed.as_ref().and_then(|v| v.get("models")).is_some();
            if !has_models_key {
                return LocalProviderInfo {
                    kind: "ollama".into(),
                    alive: false,
                    endpoint,
                    models: vec![],
                    latency_ms,
                    error: Some("response missing 'models' key (not Ollama)".into()),
                };
            }
            LocalProviderInfo {
                kind: "ollama".into(),
                alive: true,
                endpoint,
                models,
                latency_ms,
                error: None,
            }
        }
        Ok(resp) => LocalProviderInfo {
            kind: "ollama".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => LocalProviderInfo {
            kind: "ollama".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms: 0,
            error: Some(format!("{}", e)),
        },
    }
}

async fn scan_lmstudio(client: &reqwest::Client) -> LocalProviderInfo {
    let url = "http://127.0.0.1:1234/v1/models";
    let endpoint = "http://127.0.0.1:1234/v1".to_string();
    let start = Instant::now();
    let result = client.get(url).send().await;
    let latency_ms = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) if resp.status().is_success() => {
            let body = read_bounded(resp, 1024 * 1024).await;
            let parsed: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
            let models: Vec<String> = parsed
                .as_ref()
                .and_then(|v| {
                    v.get("data").and_then(|x| x.as_array()).map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|n| n.as_str()).map(String::from))
                            .collect()
                    })
                })
                .unwrap_or_default();
            // Schema-validate: OpenAI-compatible /v1/models endpoint MUST have "data" array.
            let has_data_key = parsed.as_ref().and_then(|v| v.get("data")).is_some();
            LocalProviderInfo {
                kind: "lmstudio".into(),
                alive: has_data_key && !models.is_empty(),
                endpoint,
                models,
                latency_ms,
                error: if has_data_key {
                    None
                } else {
                    Some("response missing 'data' key".into())
                },
            }
        }
        Ok(resp) => LocalProviderInfo {
            kind: "lmstudio".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => LocalProviderInfo {
            kind: "lmstudio".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms: 0,
            error: Some(format!("{}", e)),
        },
    }
}

async fn scan_llamacpp(client: &reqwest::Client) -> LocalProviderInfo {
    let url = "http://127.0.0.1:8080/health";
    let endpoint = "http://127.0.0.1:8080/v1".to_string();
    let start = Instant::now();
    let result = client.get(url).send().await;
    let latency_ms = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) if resp.status().is_success() => {
            // llama.cpp /health returns {"status": "ok"} — schema validate
            let body = read_bounded(resp, 64 * 1024).await;
            let is_llamacpp = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(String::from))
                .map(|s| s == "ok" || s == "loading model")
                .unwrap_or(false);
            LocalProviderInfo {
                kind: "llamacpp".into(),
                alive: is_llamacpp,
                endpoint,
                models: vec![], // llama.cpp serves one model — name not always discoverable
                latency_ms,
                error: if is_llamacpp {
                    None
                } else {
                    Some("response missing 'status' field".into())
                },
            }
        }
        Ok(resp) => LocalProviderInfo {
            kind: "llamacpp".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms,
            error: Some(format!("HTTP {}", resp.status())),
        },
        Err(e) => LocalProviderInfo {
            kind: "llamacpp".into(),
            alive: false,
            endpoint,
            models: vec![],
            latency_ms: 0,
            error: Some(format!("{}", e)),
        },
    }
}

/// CRUD
pub fn list_all(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<Vec<ProviderCredential>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT alias, provider, key_ref, endpoint_url, status, last_ping_ms, last_ping_at,
                last_error_msg, scope_workspace, preset_member, created_at, updated_at
         FROM provider_credentials ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ProviderCredential {
            alias: r.get(0)?,
            provider: r.get(1)?,
            key_ref: r.get(2)?,
            endpoint_url: r.get(3)?,
            status: r.get(4)?,
            last_ping_ms: r.get(5)?,
            last_ping_at: r.get(6)?,
            last_error_msg: r.get(7)?,
            scope_workspace: r.get(8)?,
            preset_member: r.get(9)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn get(
    db: &Arc<parking_lot::Mutex<Connection>>,
    alias: &str,
) -> Result<Option<ProviderCredential>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT alias, provider, key_ref, endpoint_url, status, last_ping_ms, last_ping_at,
                last_error_msg, scope_workspace, preset_member, created_at, updated_at
         FROM provider_credentials WHERE alias = ?1",
    )?;
    let mut rows = stmt.query_map(params![alias], |r| {
        Ok(ProviderCredential {
            alias: r.get(0)?,
            provider: r.get(1)?,
            key_ref: r.get(2)?,
            endpoint_url: r.get(3)?,
            status: r.get(4)?,
            last_ping_ms: r.get(5)?,
            last_ping_at: r.get(6)?,
            last_error_msg: r.get(7)?,
            scope_workspace: r.get(8)?,
            preset_member: r.get(9)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PersistRequest {
    pub alias: String,
    pub provider: String,
    pub key: Option<String>, // None for local providers without auth
    pub endpoint_url: Option<String>,
    pub scope_workspace: Option<String>,
    pub preset_member: Option<String>,
}

// MED-9 fix (Codex): alias regex — restringido a [a-zA-Z0-9_-], 1-64 chars.
// Defends against newline/control-char injection in audit payloads, Keychain account names,
// and UI rendering.
fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= 64
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// Public re-export for council_multi.rs (defense-in-depth: re-validate before sending
// the user's Bearer to any URL — even one already stored in the DB).
pub fn endpoint_url_allowed_pub(kind: ProviderKind, url_str: &str) -> bool {
    endpoint_url_allowed(kind, url_str)
}

// MED-12 fix (Codex): if the user gives an `endpoint_url` override, it must match the
// expected host pattern for that ProviderKind. Otherwise a "OpenRouter" credential could
// be tested against an attacker-controlled URL, exfiltrating the Bearer token.
fn endpoint_url_allowed(kind: ProviderKind, url_str: &str) -> bool {
    let Ok(url) = url::Url::parse(url_str) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let scheme = url.scheme();

    match kind {
        ProviderKind::OpenRouter => scheme == "https" && host == "openrouter.ai",
        ProviderKind::Cerebras => scheme == "https" && host.ends_with(".cerebras.ai"),
        ProviderKind::Groq => scheme == "https" && host.ends_with(".groq.com"),
        ProviderKind::Mistral => scheme == "https" && host.ends_with(".mistral.ai"),
        ProviderKind::SambaNova => scheme == "https" && host.ends_with(".sambanova.ai"),
        ProviderKind::GeminiStudio | ProviderKind::GeminiPaid => {
            scheme == "https" && host.ends_with(".googleapis.com")
        }
        ProviderKind::Anthropic => scheme == "https" && host.ends_with(".anthropic.com"),
        ProviderKind::OpenAI => scheme == "https" && host == "api.openai.com",
        ProviderKind::Ollama | ProviderKind::LMStudio | ProviderKind::LlamaCpp => {
            // local-only — strict loopback
            host == "127.0.0.1" || host == "localhost" || host == "[::1]"
        }
        // user-provided custom/proxy endpoints — accept https public, plus loopback
        ProviderKind::VLLM | ProviderKind::LiteLLM | ProviderKind::Custom => {
            scheme == "https" || host == "127.0.0.1" || host == "localhost" || host == "[::1]"
        }
    }
}

/// Persist credential. Saves key to Keychain (if Some), upserts row in DB.
/// Council EDGE_5: -U overwrites silently → idempotente.
/// MED-6 fix (Codex): DB UPSERT first, Keychain after. If Keychain fails we delete the row
/// to avoid an orphaned credential pointing to nothing.
pub fn persist(
    db: &Arc<parking_lot::Mutex<Connection>>,
    req: PersistRequest,
) -> Result<ProviderCredential> {
    let provider_kind = ProviderKind::parse(&req.provider)
        .ok_or_else(|| anyhow!("unknown provider kind: {}", req.provider))?;

    if !valid_alias(&req.alias) {
        return Err(anyhow!(
            "invalid alias (must match ^[A-Za-z0-9_-]{{1,64}}$): {:?}",
            req.alias
        ));
    }

    // MED-12: validate endpoint_url against provider's expected host.
    if let Some(url) = req.endpoint_url.as_deref() {
        if !endpoint_url_allowed(provider_kind, url) {
            return Err(anyhow!(
                "endpoint_url {} not allowed for provider {}",
                url,
                req.provider
            ));
        }
    }

    let needs_key = provider_kind.needs_key();
    let optional_key_present = req.key.as_deref().filter(|k| !k.is_empty()).is_some();
    if needs_key {
        let k = req.key.as_deref().unwrap_or("");
        if k.is_empty() {
            return Err(anyhow!(
                "provider {} requires a non-empty key",
                req.provider
            ));
        }
        if k.len() > 4096 {
            return Err(anyhow!("key too long (max 4096 chars)"));
        }
    } else if optional_key_present {
        // Optional key — allow up to 4096 chars too
        let k = req.key.as_deref().unwrap();
        if k.len() > 4096 {
            return Err(anyhow!("key too long (max 4096 chars)"));
        }
    }

    let will_save_key = needs_key || optional_key_present;
    let key_ref = if will_save_key {
        Some(req.alias.clone())
    } else {
        None
    };
    let now = Utc::now().to_rfc3339();

    // BLOQUE J ext 2 audit (3/3 HIGH): the original flow did DB UPSERT, dropped
    // the mutex, then keychain::save, then a separate DELETE on failure. Between
    // the two locks another caller could read or even overwrite the alias →
    // rollback would clobber the WRONG row, or worse, leave a stale Keychain
    // entry pointing at a row that's already been updated.
    //
    // Fix: validate the precondition BEFORE touching anything, do the keychain
    // write FIRST (idempotent), and only then run the DB UPSERT inside a real
    // sqlite transaction. If anything fails after a successful keychain save
    // and we DON'T already have a row pointing at it, best-effort delete the
    // keychain entry (keychain::delete is idempotent).
    if will_save_key && req.key.as_ref().is_none() {
        return Err(anyhow!(
            "internal invariant: will_save_key=true but req.key is None"
        ));
    }

    // M2: snapshot any pre-existing key so a rare DB-upsert failure AFTER the keychain
    // overwrite can RESTORE the prior credential instead of leaving it keyless.
    let prior_key = keychain::load(keychain::SERVICE_PROVIDER, &req.alias);

    // Step 1 — Keychain (only if needed).
    let mut keychain_just_written = false;
    if will_save_key {
        let key = req
            .key
            .as_ref()
            .expect("checked above (compile-time after the early return)");
        if let Err(e) = keychain::save(keychain::SERVICE_PROVIDER, &req.alias, key) {
            return Err(anyhow!("keychain save failed: {}", e));
        }
        keychain_just_written = true;
    }

    // Step 2 — DB UPSERT inside a single transaction so the row state is atomic.
    // Bind result to a local before the block ends so the Transaction borrow
    // drops before `conn` (rustc E0597 — temporary lifetime in tail expression).
    let upsert_result: Result<(), anyhow::Error> = {
        let conn = db.lock();
        let inner = match conn.unchecked_transaction() {
            Err(e) => Err(anyhow!("db transaction begin: {}", e)),
            Ok(tx) => {
                let exec_result = tx.execute(
                    "INSERT INTO provider_credentials (alias, provider, key_ref, endpoint_url, status,
                        scope_workspace, preset_member, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'unconfigured', ?5, ?6, ?7, ?7)
                     ON CONFLICT(alias) DO UPDATE SET
                        provider = excluded.provider,
                        key_ref = excluded.key_ref,
                        endpoint_url = excluded.endpoint_url,
                        scope_workspace = excluded.scope_workspace,
                        preset_member = excluded.preset_member,
                        updated_at = excluded.updated_at",
                    params![
                        req.alias,
                        provider_kind.as_str(),
                        key_ref,
                        req.endpoint_url,
                        req.scope_workspace,
                        req.preset_member,
                        now,
                    ],
                );
                match exec_result {
                    Ok(_) => tx.commit().map_err(|e| anyhow!("db commit: {}", e)),
                    Err(e) => {
                        let _ = tx.rollback();
                        Err(anyhow!("db upsert: {}", e))
                    }
                }
            }
        };
        inner
    };
    if let Err(e) = upsert_result {
        if keychain_just_written {
            // M2: restore the prior key on an UPDATE (the alias pre-existed); remove the
            // orphan on an INSERT. Avoids leaving an existing credential keyless after a
            // rare DB-upsert failure that follows a successful keychain overwrite.
            match &prior_key {
                Some(prev) => {
                    let _ = keychain::save(keychain::SERVICE_PROVIDER, &req.alias, prev);
                }
                None => {
                    keychain::delete(keychain::SERVICE_PROVIDER, &req.alias);
                }
            }
        }
        return Err(e);
    }

    // Idempotent: if no key required, ensure no stale keychain entry remains.
    if !will_save_key {
        keychain::delete(keychain::SERVICE_PROVIDER, &req.alias);
    }

    get(db, &req.alias)?.ok_or_else(|| anyhow!("persist round-trip read failed"))
}

/// Delete credential.
/// MED-7 fix (Codex): DB DELETE first; Keychain delete after.
/// If DB delete fails we keep the Keychain entry intact (no orphan key without a row).
pub fn delete(db: &Arc<parking_lot::Mutex<Connection>>, alias: &str) -> Result<bool> {
    if !valid_alias(alias) {
        return Err(anyhow!("invalid alias"));
    }
    let cred = get(db, alias)?;
    let Some(c) = cred else {
        return Ok(false);
    };
    let n = {
        let conn = db.lock();
        conn.execute(
            "DELETE FROM provider_credentials WHERE alias = ?1",
            params![alias],
        )?
    };
    if n > 0 && c.key_ref.is_some() {
        // Best-effort — if Keychain delete fails the key sits orphaned but no DB row
        // points to it, so it can't be discovered or used by Furx.
        keychain::delete(keychain::SERVICE_PROVIDER, alias);
    }
    Ok(n > 0)
}

// Public re-export for council_multi.rs and future callers (no behaviour change).
pub fn redact_for_log(input: &str) -> String {
    redact_secrets(input)
}

// MED-8 fix (Codex): redact common API key patterns from error snippets before they hit
// the DB or surface in the UI. Some upstream proxies echo the request headers back in 4xx
// bodies; without this, the user's own Bearer token could end up in last_error_msg.
fn redact_secrets(input: &str) -> String {
    use regex::Regex;
    // Keep this list aligned with web/src/components/secret-patterns (and F32 guardrail).
    static PATTERNS: once_cell::sync::Lazy<Vec<Regex>> = once_cell::sync::Lazy::new(|| {
        let raw = [
            r"sk-or-v1-[A-Za-z0-9_-]{16,}",               // OpenRouter
            r"sk-ant-(?:api|admin)?-?[A-Za-z0-9_-]{16,}", // Anthropic
            r"sk-[A-Za-z0-9_-]{20,}",                     // OpenAI / generic sk- (incl. sk-proj-)
            r"AIza[0-9A-Za-z_-]{20,}",                    // Google
            r"csk-[A-Za-z0-9_-]{16,}",                    // Cerebras
            r"gsk_[A-Za-z0-9_-]{20,}",                    // Groq
            r"AKIA[0-9A-Z]{16}",                          // AWS access key
            r"Bearer\s+[A-Za-z0-9._-]{16,}",              // generic bearer header
            r"eyJ[A-Za-z0-9._-]{16,}",                    // JWT
            // labelled api_key / access_token followed by quoted long secret
            "(?i)(api[_-]?key|access[_-]?token)\\s*[:=]\\s*[\"'][^\"']{16,}[\"']",
        ];
        raw.iter().filter_map(|p| Regex::new(p).ok()).collect()
    });
    let mut out = input.to_string();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, "[REDACTED]").to_string();
    }
    out
}

fn update_ping_result(
    db: &Arc<parking_lot::Mutex<Connection>>,
    alias: &str,
    result: &PingResult,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let status = if result.ok {
        "healthy"
    } else if result.latency_ms > 0 {
        "amber" // request salió, pero falló (rate-limited, mal modelo, etc.)
    } else {
        "red" // ni conectó
    };
    let safe_err = result.error.as_deref().map(redact_secrets);
    let conn = db.lock();
    conn.execute(
        "UPDATE provider_credentials SET status = ?1, last_ping_ms = ?2, last_ping_at = ?3,
                last_error_msg = ?4, updated_at = ?3 WHERE alias = ?5",
        params![status, result.latency_ms as i64, now, safe_err, alias],
    )?;
    Ok(())
}

/// Model-agnostic liveness probe for LOCAL providers (Ollama/LMStudio/LlamaCpp).
/// Hits the model-list endpoint instead of /chat/completions, so a server that is
/// up but doesn't have the hardcoded `default_ping_model()` pulled still reports
/// healthy. Ollama → `{base}/api/tags`; OpenAI-compatible locals → `{base}/v1/models`
/// (or `{base}/models` if base already ends in /v1). A 2xx response = alive.
async fn local_health_ping(kind: ProviderKind, base: &str) -> PingResult {
    let trimmed = base.trim_end_matches('/');
    let url = if matches!(kind, ProviderKind::Ollama) {
        let root = trimmed.trim_end_matches("/v1");
        format!("{}/api/tags", root)
    } else if trimmed.ends_with("/v1") {
        format!("{}/models", trimmed)
    } else {
        format!("{}/v1/models", trimmed)
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return PingResult {
                ok: false,
                latency_ms: 0,
                model: None,
                error: Some(format!("{}", e)),
            }
        }
    };

    let start = Instant::now();
    let resp_res = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match resp_res {
        Ok(resp) => {
            let status = resp.status();
            const MAX_BODY: usize = 64 * 1024;
            let bytes = read_bounded(resp, MAX_BODY).await;
            if status.is_success() {
                PingResult {
                    ok: true,
                    latency_ms,
                    model: None,
                    error: None,
                }
            } else {
                let body_text = String::from_utf8_lossy(&bytes);
                let snippet: String = body_text.chars().take(200).collect();
                PingResult {
                    ok: false,
                    latency_ms,
                    model: None,
                    error: Some(format!("HTTP {}: {}", status, snippet)),
                }
            }
        }
        Err(e) => PingResult {
            ok: false,
            latency_ms: 0,
            model: None,
            error: Some(format!("{}", e)),
        },
    }
}

/// Ping test: 1-token chat completion. Council EDGE_2: max_tokens=1 OK en OpenRouter.
/// Para providers que se quejan con max_tokens<16 (Cerebras), usar 16.
/// Gemini HIGH SSRF fix: endpoint_url is re-validated here as defense-in-depth.
pub async fn test_ping(
    db: &Arc<parking_lot::Mutex<Connection>>,
    alias: &str,
) -> Result<PingResult> {
    if !valid_alias(alias) {
        return Err(anyhow!("invalid alias"));
    }
    let cred = get(db, alias)?.ok_or_else(|| anyhow!("alias not found: {}", alias))?;
    let kind = ProviderKind::parse(&cred.provider)
        .ok_or_else(|| anyhow!("invalid provider kind: {}", cred.provider))?;

    let base = cred
        .endpoint_url
        .clone()
        .or_else(|| kind.default_endpoint().map(|s| s.to_string()))
        .ok_or_else(|| anyhow!("no endpoint URL"))?;
    // Defense-in-depth: re-validate even if persist() did it. A stale row that survived
    // a code-level allowlist tightening should not be able to exfil.
    if !endpoint_url_allowed(kind, &base) {
        return Err(anyhow!("endpoint_url not allowed for provider {:?}", kind));
    }

    // BUG FIX (local health, 2026-06-05): local providers (Ollama/LMStudio/LlamaCpp)
    // must NOT fail health for a missing hardcoded model. `default_ping_model()` for
    // Ollama is "llama3.2:1b", which most users don't have pulled → the /chat/completions
    // ping returned HTTP 404 "model not found" and the credential was marked red even
    // though the server was alive. Instead, probe the model-list endpoint
    // (/api/tags for Ollama, /v1/models otherwise): a 200 means the server is up,
    // regardless of which models are installed.
    if kind.is_local() {
        return Ok(finish_ping(db, alias, local_health_ping(kind, &base).await).await);
    }

    let url = format!("{}/chat/completions", base.trim_end_matches('/'));

    // H1: load the key whenever one was saved for this alias — including the OPTIONAL
    // keys for self-hosted gateways (LiteLLM/vLLM/Custom). Gating on needs_key() meant
    // an authenticated proxy was pinged with no Authorization header and always 401'd.
    let key = keychain::load_provider_key(alias);

    // Council EDGE_2: Cerebras y otros providers thinking exigen max_tokens >= 16
    let max_tokens = match kind {
        ProviderKind::Cerebras => 16,
        ProviderKind::Anthropic => 4,
        _ => 1,
    };

    let body = serde_json::json!({
        "model": kind.default_ping_model(),
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": max_tokens,
        "temperature": 0.0
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let start = Instant::now();
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }

    let resp_res = req.send().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    let result = match resp_res {
        Ok(resp) => {
            let status = resp.status();
            // Gemini MED fix + Codex MED B2: bounded streaming read — never buffer the
            // whole body before truncating. 64 KB cap defends against multi-GB responses
            // from malicious / misconfigured endpoints.
            const MAX_BODY: usize = 64 * 1024;
            let bytes = read_bounded(resp, MAX_BODY).await;

            if status.is_success() {
                let v: serde_json::Value =
                    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
                let model = v
                    .get("model")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                PingResult {
                    ok: true,
                    latency_ms,
                    model,
                    error: None,
                }
            } else {
                let body_text = String::from_utf8_lossy(&bytes);
                let snippet: String = body_text.chars().take(200).collect();
                PingResult {
                    ok: false,
                    latency_ms,
                    model: None,
                    error: Some(format!("HTTP {}: {}", status, snippet)),
                }
            }
        }
        Err(e) => PingResult {
            ok: false,
            latency_ms: 0,
            model: None,
            error: Some(format!("{}", e)),
        },
    };

    Ok(finish_ping(db, alias, result).await)
}

async fn finish_ping(
    db: &Arc<parking_lot::Mutex<Connection>>,
    alias: &str,
    mut result: PingResult,
) -> PingResult {
    // Redact error before storing/returning (in case of provider-echoed headers)
    result.error = result.error.map(|e| redact_secrets(&e));
    let _ = update_ping_result(db, alias, &result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_roundtrip() {
        for k in [
            ProviderKind::OpenRouter,
            ProviderKind::Cerebras,
            ProviderKind::Ollama,
            ProviderKind::Anthropic,
            ProviderKind::Custom,
        ] {
            assert_eq!(ProviderKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(ProviderKind::parse("bogus"), None);
    }

    #[test]
    fn provider_kind_needs_key() {
        assert!(ProviderKind::OpenRouter.needs_key());
        assert!(ProviderKind::Anthropic.needs_key());
        assert!(!ProviderKind::Ollama.needs_key());
        assert!(!ProviderKind::LMStudio.needs_key());
    }

    #[test]
    fn provider_default_endpoint_known() {
        assert!(ProviderKind::OpenRouter.default_endpoint().is_some());
        assert!(ProviderKind::Cerebras.default_endpoint().is_some());
        assert!(ProviderKind::Custom.default_endpoint().is_none());
    }

    #[test]
    fn alias_validation() {
        assert!(valid_alias("openrouter-main"));
        assert!(valid_alias("anthropic_personal_42"));
        assert!(valid_alias("a"));
        assert!(!valid_alias(""));
        assert!(!valid_alias("has space"));
        assert!(!valid_alias("has\nnewline"));
        assert!(!valid_alias("has/slash"));
        assert!(!valid_alias("has:colon"));
        assert!(!valid_alias("⚠️emoji"));
        assert!(!valid_alias(&"a".repeat(65)));
    }

    #[test]
    fn endpoint_url_allowlist() {
        // OpenRouter — only canonical https
        assert!(endpoint_url_allowed(
            ProviderKind::OpenRouter,
            "https://openrouter.ai/api/v1"
        ));
        assert!(!endpoint_url_allowed(
            ProviderKind::OpenRouter,
            "https://openrouter.ai.evil.com/api/v1"
        ));
        assert!(!endpoint_url_allowed(
            ProviderKind::OpenRouter,
            "http://openrouter.ai/api/v1"
        ));
        // Anthropic — subdomain ok
        assert!(endpoint_url_allowed(
            ProviderKind::Anthropic,
            "https://api.anthropic.com/v1"
        ));
        // Ollama — loopback only
        assert!(endpoint_url_allowed(
            ProviderKind::Ollama,
            "http://127.0.0.1:11434/v1"
        ));
        assert!(!endpoint_url_allowed(
            ProviderKind::Ollama,
            "https://example.com/ollama"
        ));
    }

    #[test]
    fn redact_known_patterns() {
        let s = "HTTP 401: {\"error\": \"invalid key Bearer sk-or-v1-deadbeef12345678abcd\"}";
        let r = redact_secrets(s);
        assert!(r.contains("[REDACTED]"));
        assert!(!r.contains("sk-or-v1-deadbeef"));
    }
}
