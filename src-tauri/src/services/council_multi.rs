// services/council_multi.rs — Council Mode con BYOK universal.
// Reemplaza el dispatch original al AIE (que era el endpoint hosted) por dispatch DIRECTO
// a los providers conectados por el user via Furx Connect.
//
// Council BLOQUE 2 EDGE_4: 6 calls directos en paralelo (no via AIE router) — más rápido y
// sin dependency en infra del autor. Cada call usa el endpoint OAI-compatible + el Bearer
// del Keychain del user.

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

use crate::services::keychain;
use crate::services::providers::{
    self, endpoint_url_allowed_pub, ProviderCredential, ProviderKind,
};
use crate::services::resilience::{self, ResilienceVerdict};

const VOICE_TIMEOUT_SECS: u64 = 30; // EDGE_7
const MAX_VOICES: usize = 6;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilPreset {
    Quick,    // OpenRouter key — 6 modelos del catálogo
    Cheapo,   // 6 free tiers individuales
    Frontier, // Anthropic + OpenAI + Gemini pagos
    Local,    // solo Ollama / LM Studio / llama.cpp
    Mix,      // todo lo healthy disponible
}

impl CouncilPreset {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "quick" => Self::Quick,
            "cheapo" => Self::Cheapo,
            "frontier" => Self::Frontier,
            "local" => Self::Local,
            "mix" => Self::Mix,
            _ => return None,
        })
    }

    fn matches(self, c: &ProviderCredential) -> bool {
        let kind = match ProviderKind::parse(&c.provider) {
            Some(k) => k,
            None => return false,
        };
        match self {
            Self::Quick => matches!(kind, ProviderKind::OpenRouter),
            Self::Cheapo => matches!(
                kind,
                ProviderKind::Cerebras
                    | ProviderKind::Groq
                    | ProviderKind::Mistral
                    | ProviderKind::SambaNova
                    | ProviderKind::GeminiStudio
                    | ProviderKind::OpenRouter
            ),
            Self::Frontier => matches!(
                kind,
                ProviderKind::Anthropic | ProviderKind::OpenAI | ProviderKind::GeminiPaid
            ),
            Self::Local => matches!(
                kind,
                ProviderKind::Ollama
                    | ProviderKind::LMStudio
                    | ProviderKind::LlamaCpp
                    | ProviderKind::VLLM
            ),
            Self::Mix => true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceResult {
    pub provider: String,
    pub alias: String,
    pub model: String,
    pub ok: bool,
    pub content: String,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CouncilResult {
    pub voices: Vec<VoiceResult>,
    pub synth: String,
    pub elapsed_ms: u64,
    pub preset: String,
    pub voices_attempted: usize,
    pub voices_succeeded: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CouncilRequest {
    pub prompt: String,
    pub preset: Option<String>, // "quick" | "cheapo" | "frontier" | "local" | "mix" — default "mix"
    pub max_voices: Option<usize>,
    /// Optional Council Template ("planning" | "implementation" | "review" | "debug" | "refactor"
    /// or a user-defined template name). Applied as an additional model-substring filter on top
    /// of the preset. NULL = no template filter.
    pub template: Option<String>,
}

/// Council Template row (migrations/018_council_templates.sql).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilTemplate {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub model_filter: String,
    pub max_voices: usize,
    pub sort_order: i64,
    pub built_in: bool,
}

/// Load one Council Template by name from SQLite.
pub fn load_template(
    db: &Arc<parking_lot::Mutex<Connection>>,
    name: &str,
) -> Option<CouncilTemplate> {
    let conn = db.lock();
    
    conn
        .query_row(
            "SELECT name, display_name, description, model_filter, max_voices, sort_order, built_in
             FROM council_templates WHERE name = ?1",
            params![name],
            |r| {
                Ok(CouncilTemplate {
                    name: r.get(0)?,
                    display_name: r.get(1)?,
                    description: r.get(2)?,
                    model_filter: r.get(3)?,
                    max_voices: r.get::<_, i64>(4)? as usize,
                    sort_order: r.get(5)?,
                    built_in: r.get::<_, i64>(6)? != 0,
                })
            },
        )
        .ok()
}

/// List all Council Templates ordered by sort_order. Used by UI selector.
pub fn list_templates(db: &Arc<parking_lot::Mutex<Connection>>) -> Vec<CouncilTemplate> {
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT name, display_name, description, model_filter, max_voices, sort_order, built_in
         FROM council_templates ORDER BY sort_order ASC, name ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let iter = stmt.query_map([], |r| {
        Ok(CouncilTemplate {
            name: r.get(0)?,
            display_name: r.get(1)?,
            description: r.get(2)?,
            model_filter: r.get(3)?,
            max_voices: r.get::<_, i64>(4)? as usize,
            sort_order: r.get(5)?,
            built_in: r.get::<_, i64>(6)? != 0,
        })
    });
    match iter {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Filter a built plan by model-substring filter (pipe-separated, case-insensitive).
/// Keeps only entries whose `provider:model` contains at least one substring.
/// Empty filter = no-op.
fn apply_template_filter(
    plan: Vec<(ProviderCredential, String)>,
    model_filter: &str,
) -> Vec<(ProviderCredential, String)> {
    let needles: Vec<String> = model_filter
        .split('|')
        .filter_map(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_lowercase())
            }
        })
        .collect();
    if needles.is_empty() {
        return plan;
    }
    plan.into_iter()
        .filter(|(cred, model)| {
            let haystack = format!("{}:{}", cred.provider, model).to_lowercase();
            needles.iter().any(|n| haystack.contains(n))
        })
        .collect()
}

/// Load preset_overrides map (alias → enabled) for a preset.
fn load_preset_overrides(
    db: &Arc<parking_lot::Mutex<Connection>>,
    preset: &str,
) -> std::collections::HashMap<String, bool> {
    let mut map = std::collections::HashMap::new();
    let conn = db.lock();
    let stmt_res =
        conn.prepare("SELECT provider_alias, enabled FROM preset_overrides WHERE preset = ?1");
    if let Ok(mut stmt) = stmt_res {
        let rows = stmt.query_map(params![preset], |r| {
            let alias: String = r.get(0)?;
            let enabled: i64 = r.get(1)?;
            Ok((alias, enabled != 0))
        });
        if let Ok(iter) = rows {
            for row in iter.flatten() {
                map.insert(row.0, row.1);
            }
        }
    }
    map
}

/// OpenRouter — modelo list para preset "quick" (1 key → 6 modelos).
fn openrouter_quick_models() -> &'static [&'static str] {
    &[
        "anthropic/claude-sonnet-4.6",
        "openai/gpt-5",
        "google/gemini-2.5-pro",
        "qwen/qwen3-235b-a22b",
        "deepseek/deepseek-chat-v3.1",
        "meta-llama/llama-4-maverick",
    ]
}

/// BLOQUE J ext (council 4/5 must-fix → audit 3/3 MED follow-up) — async model
/// discovery for Local-style providers (Ollama / LMStudio / llama.cpp / vLLM).
/// Audit fixes:
///   - HIGH (3/3): was `reqwest::blocking` inside an async context → would
///     block a Tokio worker thread for up to 2s per provider. Now fully
///     async; called from `run()` before `build_plan()`.
///   - HIGH (Codex): bearer was attached BEFORE validating the endpoint
///     against the allowlist — could leak the user's key to an attacker who
///     controls `endpoint_url`. Now allowlist-check first, abort otherwise.
///   - MED (3/3): cache had no TTL and would also cache empty results
///     forever, so a provider that came up AFTER first discovery never
///     recovered. Now: 5-minute TTL, empty results are not cached.
///   - MED (Codex): cache key was `(provider, alias)` — different endpoints
///     for the same alias would collide. Now `(provider, alias, endpoint)`.
const DISCOVER_TIMEOUT_SECS: u64 = 2;
const DISCOVER_CACHE_TTL_SECS: u64 = 300;

static LOCAL_MODELS_CACHE: once_cell::sync::Lazy<
    parking_lot::Mutex<std::collections::HashMap<(String, String, String), (Instant, Vec<String>)>>,
> = once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

async fn discover_local_models(c: &ProviderCredential) -> Vec<String> {
    let kind = match ProviderKind::parse(&c.provider) {
        Some(k) => k,
        None => return Vec::new(),
    };
    let endpoint = c
        .endpoint_url
        .clone()
        .unwrap_or_else(|| kind.default_endpoint().unwrap_or("").to_string());
    if endpoint.is_empty() {
        return Vec::new();
    }
    // Audit Codex HIGH: validate endpoint BEFORE attaching any auth so a
    // malformed/attacker-controlled endpoint never sees the bearer.
    if !endpoint_url_allowed_pub(kind, &endpoint) {
        tracing::warn!(
            "council_multi: Local provider {}/{} endpoint {} not in allowlist — skipping discovery",
            c.provider,
            c.alias,
            endpoint
        );
        return Vec::new();
    }

    // Cache lookup (key includes endpoint so swap survives Codex MED).
    let key = (c.provider.clone(), c.alias.clone(), endpoint.clone());
    {
        let cache = LOCAL_MODELS_CACHE.lock();
        if let Some((stored_at, models)) = cache.get(&key) {
            if stored_at.elapsed() < Duration::from_secs(DISCOVER_CACHE_TTL_SECS) {
                return models.clone();
            }
        }
    }

    let is_ollama = matches!(kind, ProviderKind::Ollama);
    let url = if is_ollama {
        let base = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .to_string();
        format!("{}/api/tags", base)
    } else {
        let base = endpoint.trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{}/models", base)
        } else {
            format!("{}/v1/models", base)
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(DISCOVER_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut req = client.get(&url);
    if let Some(bearer) = keychain::load_provider_key(&c.alias) {
        if !bearer.is_empty() {
            req = req.bearer_auth(bearer);
        }
    }
    let resp = match req.send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(
                "council_multi: Local model discovery for {}/{} got status {} — skipping voice",
                c.provider,
                c.alias,
                r.status()
            );
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(
                "council_multi: Local model discovery for {}/{} failed: {} — skipping voice",
                c.provider,
                c.alias,
                e
            );
            return Vec::new();
        }
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let models: Vec<String> = if is_ollama {
        json.get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        json.get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    // Audit MED 3/3: don't cache empty results — let the next plan retry.
    if !models.is_empty() {
        LOCAL_MODELS_CACHE
            .lock()
            .insert(key, (Instant::now(), models.clone()));
    }
    models
}

/// Build the voice plan: list of (credential, model) pairs to call.
/// Codex MED-6 fix B3: honor preset_overrides (per-preset enable/disable).
/// BLOQUE J ext: async because Local-kind credentials need on-the-fly model
/// discovery via reqwest::Client (was sync + reqwest::blocking, which would
/// block a Tokio worker thread — 3/3 reviewer HIGH).
async fn build_plan(
    creds: Vec<ProviderCredential>,
    preset: CouncilPreset,
    overrides: &std::collections::HashMap<String, bool>,
    max_voices: usize,
) -> Vec<(ProviderCredential, String)> {
    // Filter healthy or amber
    let pool: Vec<ProviderCredential> = creds
        .into_iter()
        .filter(|c| c.status == "healthy" || c.status == "amber")
        .filter(|c| preset.matches(c))
        .filter(|c| {
            // If an explicit override exists for (preset, alias), honor it.
            // Default: true (enabled).
            overrides.get(&c.alias).copied().unwrap_or(true)
        })
        .collect();

    let mut plan: Vec<(ProviderCredential, String)> = Vec::new();
    // M3: dedup by (alias, model). Keying on the model alone silently dropped a second
    // configured provider that happens to serve the same model id (e.g. gemini_studio +
    // gemini_paid both "gemini-2.0-flash") in the mix/cheapo presets.
    let mut seen: HashSet<(String, String)> = HashSet::new();

    // Special-case: preset Quick + only an OpenRouter cred → expand to 6 distinct models.
    if matches!(preset, CouncilPreset::Quick) {
        if let Some(or_cred) = pool.iter().find(|c| c.provider == "openrouter") {
            for m in openrouter_quick_models() {
                if plan.len() >= max_voices {
                    break;
                }
                if seen.insert((or_cred.alias.clone(), m.to_string())) {
                    plan.push((or_cred.clone(), m.to_string()));
                }
            }
            return plan;
        }
    }

    // Default — one voice per provider, using the provider's default ping model.
    for c in pool {
        if plan.len() >= max_voices {
            break;
        }
        let kind = match ProviderKind::parse(&c.provider) {
            Some(k) => k,
            None => continue,
        };

        // BLOQUE J ext (audit Codex MED — critical): the original branch checked
        // `model == "auto"` first, but `Ollama::default_ping_model() == "llama3.2:1b"`
        // (not "auto"), so the Local-discovery path NEVER fired for Ollama. Now:
        // for ANY Local kind, try discovery FIRST. If it succeeds, use the picked
        // model. If it returns empty (server down) AND the default is a real model
        // name (Ollama), fall through to the default — preserves prior behaviour
        // for users who have `llama3.2:1b` actually pulled.
        if matches!(
            kind,
            ProviderKind::Ollama
                | ProviderKind::LMStudio
                | ProviderKind::LlamaCpp
                | ProviderKind::VLLM
        ) {
            let discovered = discover_local_models(&c).await;
            if let Some(picked) = discovered.into_iter().next() {
                if seen.insert((c.alias.clone(), picked.clone())) {
                    plan.push((c, picked));
                }
                continue;
            }
            // Discovery empty: log once, then either skip (auto-default kinds)
            // or fall through to the static default below (Ollama).
            tracing::warn!(
                "council_multi: Local provider {}/{} discovery returned no models — falling back to default_ping_model",
                c.provider, c.alias
            );
        }

        let model = kind.default_ping_model().to_string();
        if model == "auto" {
            // True "auto" kinds (Custom / LiteLLM with no hint) really cannot
            // pick a model without help. Skip cleanly.
            continue;
        }
        if seen.insert((c.alias.clone(), model.clone())) {
            plan.push((c, model));
        }
    }
    plan
}

// M1: push each voice into a SHARED buffer as it completes, so if the outer 40s
// timeout fires and drops this future, the caller can still recover the voices that
// finished before the deadline (the old return-Vec was lost on drop → empty council).
async fn collect_voices(
    set: &mut JoinSet<VoiceResult>,
    db: &Arc<parking_lot::Mutex<Connection>>,
    quick_mode: bool,
    out: &parking_lot::Mutex<Vec<VoiceResult>>,
) {
    while let Some(joined) = set.join_next().await {
        let v = match joined {
            Ok(v) => {
                update_status_after_voice(db, &v, quick_mode);
                v
            }
            Err(e) => VoiceResult {
                provider: "?".into(),
                alias: "?".into(),
                model: "?".into(),
                ok: false,
                content: String::new(),
                latency_ms: 0,
                error: Some(format!("join: {}", e)),
            },
        };
        out.lock().push(v);
    }
}

async fn call_one_voice(
    cred: ProviderCredential,
    model: String,
    prompt: String,
    // spec 003 T3.7 — distilled system prompt from the project's active Persona
    // Pack, injected as a `system` message ahead of the user prompt. `None` when
    // no pack is applied. Affects every BYOK voice equally (council ungated, F-II).
    pack_system: Option<String>,
    // spec 001 H3 — when Some, this voice is a child of the given council parent
    // trace; we tag the uploaded trace so the dashboard can nest it. None = flat.
    council_parent_trace_id: Option<String>,
    voice_position: u32,
) -> VoiceResult {
    let kind = match ProviderKind::parse(&cred.provider) {
        Some(k) => k,
        None => {
            return VoiceResult {
                provider: cred.provider.clone(),
                alias: cred.alias.clone(),
                model,
                ok: false,
                content: String::new(),
                latency_ms: 0,
                error: Some("invalid provider kind".into()),
            };
        }
    };

    let base = cred
        .endpoint_url
        .clone()
        .or_else(|| kind.default_endpoint().map(String::from))
        .unwrap_or_default();
    if base.is_empty() {
        return VoiceResult {
            provider: cred.provider.clone(),
            alias: cred.alias.clone(),
            model,
            ok: false,
            content: String::new(),
            latency_ms: 0,
            error: Some("no endpoint URL".into()),
        };
    }
    // HIGH fix (Codex B2 audit): defense-in-depth endpoint allowlist. A stale or tampered
    // DB row could redirect a Bearer token to an attacker-controlled URL. The same check
    // runs in providers::test_ping; we re-run it here before Council dispatch.
    if !endpoint_url_allowed_pub(kind, &base) {
        return VoiceResult {
            provider: cred.provider.clone(),
            alias: cred.alias.clone(),
            model,
            ok: false,
            content: String::new(),
            latency_ms: 0,
            error: Some("endpoint_url not allowed by provider allowlist".into()),
        };
    }
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));

    // H1: send the saved key even for optional-key self-hosted gateways
    // (LiteLLM/vLLM/Custom). Gating on needs_key() left them unauthenticated → 401.
    let key = keychain::load_provider_key(&cred.alias);

    let max_tokens = match kind {
        ProviderKind::Cerebras => 1024,
        ProviderKind::Anthropic => 512,
        _ => 512,
    };
    // Inject the active Persona Pack as a leading `system` message (T3.7). The
    // OpenAI-compatible /chat/completions shape used by every provider here
    // accepts a system role; providers that treat system natively (Anthropic via
    // OpenRouter) honor it too. max_tokens caps OUTPUT, so the system prompt only
    // spends context budget, not the response budget.
    let messages = match pack_system.as_deref() {
        Some(sys) if !sys.is_empty() => serde_json::json!([
            {"role": "system", "content": sys},
            {"role": "user", "content": prompt},
        ]),
        _ => serde_json::json!([{"role": "user", "content": prompt}]),
    };
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0.3,
    });

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(VOICE_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return VoiceResult {
                provider: cred.provider.clone(),
                alias: cred.alias.clone(),
                model,
                ok: false,
                content: String::new(),
                latency_ms: 0,
                error: Some(format!("client build: {}", e)),
            };
        }
    };

    let start = Instant::now();
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }

    let result = req.send().await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(mut resp) => {
            let status = resp.status();
            const MAX_BODY: usize = 256 * 1024;
            // M4: bound the read to MAX_BODY DURING streaming — `resp.bytes()` would
            // first buffer an attacker-controlled multi-GB body fully into memory.
            let mut buf: Vec<u8> = Vec::new();
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => {
                        let room = MAX_BODY - buf.len();
                        if chunk.len() >= room {
                            buf.extend_from_slice(&chunk[..room]);
                            break;
                        }
                        buf.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        return VoiceResult {
                            provider: cred.provider.clone(),
                            alias: cred.alias.clone(),
                            model,
                            ok: false,
                            content: String::new(),
                            latency_ms,
                            error: Some(format!("body read: {}", e)),
                        };
                    }
                }
            }
            let bytes = buf;
            if status.is_success() {
                let v: serde_json::Value =
                    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
                let content = v
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|a| a.first())
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let actual_model = v
                    .get("model")
                    .and_then(|x| x.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| model.clone());
                let tokens_in = v
                    .get("usage")
                    .and_then(|u| u.get("prompt_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                let tokens_out = v
                    .get("usage")
                    .and_then(|u| u.get("completion_tokens"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                // Sprint #4 — producer hook. Best-effort enqueue via global handle.
                // V1 audit fix: project_id comes from FURX_DEFAULT_PROJECT_ID env (set after sign-in
                // by CloudAccountPanel.bootstrap_default_project), NOT a literal "default" — that name
                // doesn't exist in D1 and would 404 every upload. If the env var isn't set yet, skip
                // the enqueue entirely (the trace stays in local SQLite for later catch-up).
                // V2 audit fix: cap prompt + response at 64KB each before enqueue so we don't push
                // arbitrarily large payloads through the channel; Worker also enforces 256KB.
                if let Ok(project_id) = std::env::var("FURX_DEFAULT_PROJECT_ID") {
                    if !project_id.is_empty() {
                        if let Some(uploader) = crate::services::cloud_uploader::take_global() {
                            let cap = |s: &str| -> String {
                                const MAX: usize = 64 * 1024;
                                if s.len() <= MAX {
                                    return s.to_string();
                                }
                                let mut idx = MAX;
                                while idx < s.len() && !s.is_char_boundary(idx) {
                                    idx += 1;
                                }
                                s[..idx.min(s.len())].to_string()
                            };
                            // H3: tag this voice as a child of the council parent (if grouped).
                            let (cp, va, vp) = match &council_parent_trace_id {
                                Some(pid) => (
                                    Some(pid.clone()),
                                    Some(cred.alias.clone()),
                                    Some(voice_position),
                                ),
                                None => (None, None, None),
                            };
                            let job = crate::services::cloud_uploader::CloudUploadJob {
                                trace_id: None,
                                project_id,
                                ts: chrono::Utc::now().timestamp_millis(),
                                model: actual_model.clone(),
                                provider: cred.provider.clone(),
                                tokens_in: Some(tokens_in as u32),
                                tokens_out: Some(tokens_out as u32),
                                latency_ms: Some(latency_ms as u32),
                                cost_usd_micro: None,
                                status: "success".to_string(),
                                error_class: None,
                                prompt: Some(cap(&prompt)),
                                response: Some(cap(&content)),
                                replay_id: None,
                                local_event_id: None,
                                council_parent_trace_id: cp,
                                council_voice_alias: va,
                                council_voice_position: vp,
                            };
                            uploader.try_enqueue(job);
                        }
                    }
                }
                VoiceResult {
                    provider: cred.provider.clone(),
                    alias: cred.alias.clone(),
                    model: actual_model,
                    ok: true,
                    content,
                    latency_ms,
                    error: None,
                }
            } else {
                let snippet: String = String::from_utf8_lossy(&bytes).chars().take(300).collect();
                let err = providers::redact_for_log(&format!("HTTP {}: {}", status, snippet));
                VoiceResult {
                    provider: cred.provider.clone(),
                    alias: cred.alias.clone(),
                    model,
                    ok: false,
                    content: String::new(),
                    latency_ms,
                    error: Some(err),
                }
            }
        }
        Err(e) => VoiceResult {
            provider: cred.provider.clone(),
            alias: cred.alias.clone(),
            model,
            ok: false,
            content: String::new(),
            latency_ms,
            error: Some(format!("{}", e)),
        },
    }
}

fn update_status_after_voice(
    db: &Arc<parking_lot::Mutex<Connection>>,
    voice: &VoiceResult,
    quick_mode: bool,
) {
    // BLOQUE 3 resilience: record success/failure for circuit + quota tracking.
    if voice.ok {
        let _ = resilience::record_success(db, &voice.provider, &voice.model, &voice.alias);
    } else {
        let err = voice.error.as_deref().unwrap_or("");
        let _ = resilience::record_failure(db, &voice.provider, &voice.model, &voice.alias, err);
    }

    // LOW fix (Codex B2): in Quick mode 6 voices share the same alias (OpenRouter).
    // Each voice would race to overwrite the status — one late 429 could mark the whole
    // credential as amber even when 5 voices succeeded. Solution: in quick mode, only
    // update status to "healthy" on success; never downgrade on individual model failure.
    let err_str = voice.error.as_deref().unwrap_or("");
    let new_status = if voice.ok {
        "healthy"
    } else if quick_mode {
        // Skip downgrade in quick mode
        return;
    } else if err_str.contains("429") || err_str.contains("rate") {
        "amber"
    } else if err_str.contains("401")
        || err_str.contains("403")
        || err_str.contains("timeout")
        || err_str.contains("connect")
    {
        "red"
    } else {
        "amber"
    };
    let now = Utc::now().to_rfc3339();
    let safe_err = voice.error.as_deref().map(providers::redact_for_log);
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE provider_credentials SET status = ?1, last_ping_ms = ?2, last_ping_at = ?3,
                last_error_msg = ?4, updated_at = ?3 WHERE alias = ?5",
        params![
            new_status,
            voice.latency_ms as i64,
            now,
            safe_err,
            voice.alias
        ],
    );
}

fn synthesize(voices: &[VoiceResult], prompt: &str) -> String {
    let ok: Vec<&VoiceResult> = voices.iter().filter(|v| v.ok).collect();
    if ok.is_empty() {
        return format!(
            "**Council — sin respuestas exitosas**\n\nPrompt: `{}`\n\nIntentado: {} voces · {} fallaron.\n\nRevisá `Settings → Connect` y verificá los providers en estado verde.",
            prompt.chars().take(100).collect::<String>(),
            voices.len(),
            voices.iter().filter(|v| !v.ok).count()
        );
    }
    let mut out = String::new();
    out.push_str(&format!("# Council Mode\n\n**Prompt**: {}\n\n", prompt));
    out.push_str(&format!(
        "**{} voces respondieron** · ({} fallaron)\n\n",
        ok.len(),
        voices.len() - ok.len()
    ));
    for (i, v) in ok.iter().enumerate() {
        out.push_str(&format!(
            "## Voz {} · `{}` (alias `{}`)\n\n*latencia {} ms*\n\n{}\n\n---\n\n",
            i + 1,
            v.model,
            v.alias,
            v.latency_ms,
            v.content.trim()
        ));
    }
    if voices.len() > ok.len() {
        out.push_str("## Voces fallidas\n\n");
        for v in voices.iter().filter(|v| !v.ok) {
            out.push_str(&format!(
                "- `{}` (alias `{}`) · {}\n",
                v.model,
                v.alias,
                v.error.as_deref().unwrap_or("error sin detalle")
            ));
        }
    }
    out
}

/// BLOQUE G · F6 — internal council fallback. Used when the user has no
/// BYOK providers configured: tries the AIE remote (frontier_free) and the
/// local `codex exec` binary, both best-effort, both with bounded timeouts.
async fn internal_fallback_voices(prompt: String) -> Vec<VoiceResult> {
    use std::process::Stdio;
    use tokio::process::Command as TokioCommand;
    let mut out: Vec<VoiceResult> = Vec::new();

    // --- 1. AIE remote: POST /v1/chat/completions profile=frontier_free ---
    let aie_start = Instant::now();
    // 039 — in-process cached bearer (was a `/usr/bin/security` subprocess per call).
    if let Some(bearer) = crate::services::keychain_bearer::get_bearer() {
        {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(VOICE_TIMEOUT_SECS))
                .build();
            if let Ok(c) = client {
                let aie_url = format!(
                    "{}/v1/chat/completions",
                    crate::services::aie_endpoint::resolve_url_or_default()
                );
                let res = c
                    .post(&aie_url)
                    .header("Authorization", format!("Bearer {}", bearer))
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .json(&serde_json::json!({
                        "profile": "frontier_free",
                        "messages": [{"role": "user", "content": prompt}],
                        "max_tokens": 1024,
                    }))
                    .send()
                    .await;
                let latency = aie_start.elapsed().as_millis() as u64;
                match res {
                    // 039 — drop a stale bearer on 401 so the next call re-reads the rotated value.
                    Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        crate::services::keychain_bearer::invalidate_bearer_cache();
                        out.push(VoiceResult {
                            provider: "aie".to_string(),
                            alias: "internal-aie".to_string(),
                            model: "frontier_free".to_string(),
                            ok: false,
                            content: String::new(),
                            latency_ms: latency,
                            error: Some("aie status 401".to_string()),
                        });
                    }
                    Ok(r) => match r.json::<serde_json::Value>().await {
                        Ok(v) => {
                            let content = v
                                .get("choices")
                                .and_then(|c| c.get(0))
                                .and_then(|c| c.get("message"))
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string();
                            let model = v
                                .get("model")
                                .and_then(|x| x.as_str())
                                .unwrap_or("frontier_free")
                                .to_string();
                            out.push(VoiceResult {
                                provider: "aie".to_string(),
                                alias: "internal-aie".to_string(),
                                model,
                                ok: !content.is_empty(),
                                content,
                                latency_ms: latency,
                                error: None,
                            });
                        }
                        Err(e) => out.push(VoiceResult {
                            provider: "aie".to_string(),
                            alias: "internal-aie".to_string(),
                            model: "frontier_free".to_string(),
                            ok: false,
                            content: String::new(),
                            latency_ms: latency,
                            error: Some(format!("parse: {}", e)),
                        }),
                    },
                    Err(e) => out.push(VoiceResult {
                        provider: "aie".to_string(),
                        alias: "internal-aie".to_string(),
                        model: "frontier_free".to_string(),
                        ok: false,
                        content: String::new(),
                        latency_ms: latency,
                        error: Some(format!("aie call: {}", e)),
                    }),
                }
            }
        }
    }

    // --- 2. Codex local: `codex exec --skip-git-repo-check --color never <prompt>` ---
    let codex_start = Instant::now();
    let codex_res = tokio::time::timeout(Duration::from_secs(VOICE_TIMEOUT_SECS + 30), async {
        TokioCommand::new("codex")
            .args(["exec", "--skip-git-repo-check", "--color", "never"])
            .arg(&prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
    })
    .await;
    let codex_latency = codex_start.elapsed().as_millis() as u64;
    match codex_res {
        Ok(Ok(o)) if o.status.success() => {
            let content = String::from_utf8_lossy(&o.stdout).to_string();
            out.push(VoiceResult {
                provider: "codex_local".to_string(),
                alias: "codex".to_string(),
                model: "codex-cli".to_string(),
                ok: !content.is_empty(),
                content,
                latency_ms: codex_latency,
                error: None,
            });
        }
        Ok(Ok(o)) => {
            out.push(VoiceResult {
                provider: "codex_local".to_string(),
                alias: "codex".to_string(),
                model: "codex-cli".to_string(),
                ok: false,
                content: String::new(),
                latency_ms: codex_latency,
                error: Some(format!(
                    "codex exit {}: {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr)
                )),
            });
        }
        Ok(Err(e)) => {
            // ENOENT → codex not in PATH. Don't add a fake voice; just skip.
            tracing::debug!("codex local skipped: {}", e);
        }
        Err(_) => {
            out.push(VoiceResult {
                provider: "codex_local".to_string(),
                alias: "codex".to_string(),
                model: "codex-cli".to_string(),
                ok: false,
                content: String::new(),
                latency_ms: codex_latency,
                error: Some("codex timed out".into()),
            });
        }
    }

    out
}

pub async fn run(
    db: Arc<parking_lot::Mutex<Connection>>,
    req: CouncilRequest,
) -> Result<CouncilResult> {
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(anyhow!("empty prompt"));
    }
    if prompt.len() > 100_000 {
        return Err(anyhow!("prompt too long (max 100k chars)"));
    }
    let preset = req
        .preset
        .as_deref()
        .and_then(CouncilPreset::parse)
        .unwrap_or(CouncilPreset::Mix);
    // Resolve template (optional). If present, its max_voices caps the request's max_voices.
    let template = req
        .template
        .as_deref()
        .and_then(|name| load_template(&db, name));
    let template_cap = template
        .as_ref()
        .map(|t| t.max_voices)
        .unwrap_or(MAX_VOICES);
    let max_voices = req
        .max_voices
        .unwrap_or(MAX_VOICES)
        .min(MAX_VOICES)
        .min(template_cap);

    let creds = providers::list_all(&db)?;
    let overrides = load_preset_overrides(&db, &format!("{:?}", preset).to_lowercase());
    let mut plan = build_plan(creds.clone(), preset, &overrides, max_voices).await;

    // Apply template model-substring filter on top of preset (if any).
    if let Some(t) = template.as_ref() {
        plan = apply_template_filter(plan, &t.model_filter);
        // Re-cap after filter
        plan.truncate(max_voices);
    }

    // 019 F3 (T031) — CUSTOM-VOICES: las voces pinneadas por el user SIEMPRE participan, por
    // encima del preset/template (F-II: config, no tier-gate). Se anteponen al plan y se dedupea
    // por (alias, model) para no llamar dos veces a la misma. El cap MAX_VOICES sigue mandando (no
    // explotamos la cuenta de voces), pero las custom van PRIMERO (prioridad sobre las del preset).
    let custom = resolve_custom_voices(&db, &creds);
    if !custom.is_empty() {
        let mut merged: Vec<(ProviderCredential, String)> = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (cred, model) in custom.into_iter().chain(plan.into_iter()) {
            let key = (cred.alias.clone(), model.clone());
            if seen.insert(key) {
                merged.push((cred, model));
            }
        }
        merged.truncate(MAX_VOICES.max(max_voices));
        plan = merged;
    }

    // EDGE_3: frontier preset graceful downgrade. M6: track the EFFECTIVE preset so the
    // result reports "cheapo" (what actually ran) instead of mislabelling it "frontier".
    let mut effective_preset = preset;
    if matches!(preset, CouncilPreset::Frontier) && plan.is_empty() {
        let cheapo_overrides = load_preset_overrides(&db, "cheapo");
        plan = build_plan(creds, CouncilPreset::Cheapo, &cheapo_overrides, max_voices).await;
        effective_preset = CouncilPreset::Cheapo;
    }

    if plan.is_empty() {
        // BLOQUE G · F6 — original PLAN_CLOSE spec (4 frontier_free + codex local)
        // as a no-BYOK fallback. Returns a 2-voice CouncilResult if the user has
        // (a) the aie-internal-bearer in Keychain, OR (b) `codex` in PATH.
        // Either / both succeed → no need to bother the user with "configure
        // a provider first" before they can even kick the tires on the council.
        let start = Instant::now();
        let voices = internal_fallback_voices(prompt.clone()).await;
        if !voices.is_empty() {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let succeeded = voices.iter().filter(|v| v.ok).count();
            let synth = synthesize(&voices, &prompt);
            return Ok(CouncilResult {
                voices_attempted: voices.len(),
                voices_succeeded: succeeded,
                voices,
                synth,
                elapsed_ms,
                preset: "internal_fallback".to_string(),
            });
        }
        return Err(anyhow!(
            "no providers available for preset (configurá uno en Settings → Connect, o instalá `codex` / añade aie-internal-bearer al Keychain para el fallback interno)"
        ));
    }

    // BLOQUE 3 resilience filter: skip providers currently blocked.
    let plan: Vec<(ProviderCredential, String)> = plan
        .into_iter()
        .filter(|(cred, model)| {
            match resilience::check_allowed(&db, &cred.provider, model, &cred.alias) {
                Ok(ResilienceVerdict::Allow) => true,
                Ok(verdict) => {
                    tracing::info!(
                        "council: skipping {}/{} ({}): {:?}",
                        cred.provider,
                        model,
                        cred.alias,
                        verdict
                    );
                    false
                }
                Err(_) => true, // fail-open if state read fails
            }
        })
        .collect();

    if plan.is_empty() {
        return Err(anyhow!(
            "all providers blocked by resilience (rate-limit / quota / circuit). Retry in a few minutes."
        ));
    }

    let mut attempted = plan.len();
    let start = Instant::now();
    let quick_mode = matches!(preset, CouncilPreset::Quick);

    // spec 003 T3.7 — resolve the project's active Persona Pack once per run
    // (lazy fetch-on-run + 60s TTL). Never errors: degrades to None / cached.
    let pack_system = crate::services::active_pack::resolve_system_message(&db).await;

    // spec 001 H3 — when there are ≥2 voices we group them under one synthetic
    // council parent trace (uploaded after synthesis), so the dashboard can render
    // an expandable parent → N child rows. Single-voice runs stay flat (no parent).
    let parent_trace_id: Option<String> = if attempted >= 2 {
        Some(uuid::Uuid::new_v4().to_string())
    } else {
        None
    };

    let mut set: JoinSet<VoiceResult> = JoinSet::new();
    for (idx, (cred, model)) in plan.into_iter().enumerate() {
        let p = prompt.clone();
        let ps = pack_system.clone();
        let parent = parent_trace_id.clone();
        set.spawn(async move { call_one_voice(cred, model, p, ps, parent, idx as u32).await });
    }

    // MED fix (Gemini B2): global hard timeout 40s. Even if a voice's reqwest timeout
    // misbehaves due to DNS or runtime stalls, we cap total wait to 40 s and abort the rest.
    // M1: collect into a shared buffer so a 40s-timeout abort still returns the voices
    // that finished before the deadline (instead of discarding all of them).
    let collected: parking_lot::Mutex<Vec<VoiceResult>> = parking_lot::Mutex::new(Vec::new());
    if tokio::time::timeout(
        Duration::from_secs(40),
        collect_voices(&mut set, &db, quick_mode, &collected),
    )
    .await
    .is_err()
    {
        set.shutdown().await;
        tracing::warn!(
            "council_multi: global 40s timeout — returning the voices that completed before the deadline"
        );
    }
    let mut voices: Vec<VoiceResult> = collected.into_inner();

    // A provider can be marked healthy while its local process is temporarily
    // down. In that case a non-empty plan previously suppressed the internal
    // AIE/Codex fallback and returned an empty council. Retry through the
    // fallback only when every planned voice failed.
    if !voices.iter().any(|v| v.ok) {
        let fallback = internal_fallback_voices(prompt.clone()).await;
        attempted += fallback.len();
        voices.extend(fallback);
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let succeeded = voices.iter().filter(|v| v.ok).count();
    let synth = synthesize(&voices, &prompt);

    // spec 001 H3 — upload the synthetic council parent trace (groups the voices).
    // Children were already enqueued during call_one_voice with this parent id.
    // Same env-gate + 64KB cap as the per-voice producer; best-effort, never blocks.
    if let Some(pid) = &parent_trace_id {
        if let Ok(project_id) = std::env::var("FURX_DEFAULT_PROJECT_ID") {
            if !project_id.is_empty() {
                if let Some(uploader) = crate::services::cloud_uploader::take_global() {
                    let cap = |s: &str| -> String {
                        const MAX: usize = 64 * 1024;
                        if s.len() <= MAX {
                            return s.to_string();
                        }
                        let mut idx = MAX;
                        while idx < s.len() && !s.is_char_boundary(idx) {
                            idx += 1;
                        }
                        s[..idx.min(s.len())].to_string()
                    };
                    let job = crate::services::cloud_uploader::CloudUploadJob {
                        trace_id: Some(pid.clone()),
                        project_id,
                        ts: chrono::Utc::now().timestamp_millis(),
                        model: "council".to_string(),
                        provider: "furx-council".to_string(),
                        tokens_in: None,
                        tokens_out: None,
                        latency_ms: Some(elapsed_ms as u32),
                        cost_usd_micro: None,
                        status: if succeeded > 0 { "success" } else { "error" }.to_string(),
                        error_class: None,
                        prompt: Some(cap(&prompt)),
                        response: Some(cap(&synth)),
                        replay_id: None,
                        local_event_id: None,
                        council_parent_trace_id: None,
                        council_voice_alias: None,
                        council_voice_position: None,
                    };
                    uploader.try_enqueue(job);
                }
            }
        }
    }

    let result = CouncilResult {
        voices,
        synth,
        elapsed_ms,
        preset: format!("{:?}", effective_preset).to_lowercase(),
        voices_attempted: attempted,
        voices_succeeded: succeeded,
    };

    // 019 F3 (T031) — persistir el run en el history (best-effort, nunca tira el resultado).
    // El prompt y la síntesis se REDACTAN antes de quedar at-rest (F-I BYOK): el scrollback / la
    // pregunta pueden traer secrets que el user pegó.
    if let Err(e) = record_run(&db, &prompt, &req.template, &result) {
        tracing::warn!("council_multi: no se pudo guardar el run en history: {}", e);
    }

    // 050 Ola 8 P2 (FR-003) — reliability board: registra UNA fila por voz (éxito/latencia/modelo).
    // OPT-IN: `record` es no-op si el board está OFF (default) → cero regresión. Best-effort
    // (cualquier fallo se traga). Sin costo medible acá (council es free) → cost_usd=None. Solo
    // metadata no-secreta (provider/model/ok/latency); jamás el contenido de la voz.
    for v in &result.voices {
        crate::services::reliability::record(
            &db,
            &crate::services::reliability::Outcome {
                agent_kind: "council",
                model: if v.model.is_empty() { None } else { Some(v.model.as_str()) },
                provider: if v.provider.is_empty() { None } else { Some(v.provider.as_str()) },
                success: v.ok,
                latency_ms: Some(v.latency_ms as i64),
                cost_usd: None,
            },
        );
    }

    Ok(result)
}

/// Headless entry point used by trusted CLIs running inside Furx.
///
/// Reuses the exact same provider selection, resilience, history and AIE/Codex
/// fallback path as the desktop command. The prompt is supplied by the caller
/// after reading stdin or `--prompt`, so it never needs to be persisted in a
/// temporary file.
pub fn run_cli(
    prompt: String,
    preset: Option<String>,
    template: Option<String>,
    max_voices: Option<usize>,
) -> i32 {
    if prompt.trim().is_empty() {
        eprintln!("uso: furx council --stdin|--prompt <texto> [--preset mix] [--template planning] [--max-voices 6]");
        return 2;
    }
    let Some(home) = dirs::home_dir() else {
        eprintln!("no se encontró el home dir");
        return 2;
    };
    let db_path = home.join(".furx").join("furx.db");
    let conn = match crate::db::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("no se pudo abrir la DB ({}): {e}", db_path.display());
            return 2;
        }
    };
    let db = Arc::new(parking_lot::Mutex::new(conn));
    let req = CouncilRequest {
        prompt,
        preset,
        max_voices,
        template,
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("no se pudo iniciar el runtime del consejo: {e}");
            return 2;
        }
    };
    match runtime.block_on(run(db.clone(), req)) {
        Ok(result) => {
            let audit = crate::bases::audit::AuditWriter::new(db);
            let actor = crate::services::identity::current_actor();
            let _ = audit.write(crate::bases::audit::EventInput {
                kind: "council.run.cli",
                actor: &actor,
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({
                    "preset": result.preset,
                    "voices_attempted": result.voices_attempted,
                    "voices_succeeded": result.voices_succeeded,
                    "elapsed_ms": result.elapsed_ms,
                }),
            });
            match serde_json::to_string(&result) {
                Ok(json) => {
                    println!("{json}");
                    0
                }
                Err(e) => {
                    eprintln!("no se pudo serializar el resultado: {e}");
                    2
                }
            }
        }
        Err(e) => {
            eprintln!("council falló: {e}");
            2
        }
    }
}

// ── 019 F3 (T031) — Council HISTORY (persistir + listar los runs) ────────────
// El council es FREE para TODOS los tiers (constitución F-II): el history no es un feature
// gateado, es memoria del producto. Inmutable para consulta (append-only); no se edita un run
// pasado. prompt/synth se redactan antes de persistir (F-I BYOK).

/// Un run de council registrado (fila de `council_runs`). `voices_json` = resumen NO-secreto.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilRunRecord {
    pub id: String,
    pub ran_at: String,
    pub preset: String,
    pub template: Option<String>,
    pub prompt: String,
    pub synth: String,
    pub voices_attempted: i64,
    pub voices_succeeded: i64,
    pub elapsed_ms: i64,
    /// JSON array `[{provider, model, ok, latency_ms}]` (sin contenido de las voces, sólo metadata).
    pub voices_json: String,
}

/// Persiste un run en el history. `prompt`/`synth` se redactan; `voices_json` es metadata
/// (provider/model/ok/latency) — NUNCA el contenido de cada voz (puede ser largo y no aporta al
/// índice de history; el detalle del run vivo ya lo tiene el caller).
fn record_run(
    db: &Arc<parking_lot::Mutex<Connection>>,
    prompt: &str,
    template: &Option<String>,
    result: &CouncilResult,
) -> Result<()> {
    let voices_meta: Vec<serde_json::Value> = result
        .voices
        .iter()
        .map(|v| {
            serde_json::json!({
                "provider": v.provider,
                "model": v.model,
                "ok": v.ok,
                "latency_ms": v.latency_ms,
            })
        })
        .collect();
    let voices_json = serde_json::to_string(&voices_meta).unwrap_or_else(|_| "[]".to_string());
    let synth = crate::services::tts::redact_secrets(&result.synth);
    let prompt_red = crate::services::tts::redact_secrets(prompt);
    let id = uuid::Uuid::new_v4().to_string();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO council_runs
            (id, preset, template, prompt, synth, voices_attempted, voices_succeeded, elapsed_ms, voices_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            id,
            result.preset,
            template,
            prompt_red,
            synth,
            result.voices_attempted as i64,
            result.voices_succeeded as i64,
            result.elapsed_ms as i64,
            voices_json,
        ],
    )?;
    Ok(())
}

/// Lista los runs del history, más reciente primero, hasta `limit` (clamp 1..=200).
pub fn list_runs(
    db: &Arc<parking_lot::Mutex<Connection>>,
    limit: i64,
) -> Result<Vec<CouncilRunRecord>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, ran_at, preset, template, prompt, synth, voices_attempted, voices_succeeded,
                elapsed_ms, voices_json
         FROM council_runs ORDER BY ran_at DESC, rowid DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit.clamp(1, 200)], |r| {
            Ok(CouncilRunRecord {
                id: r.get(0)?,
                ran_at: r.get(1)?,
                preset: r.get(2)?,
                template: r.get(3)?,
                prompt: r.get(4)?,
                synth: r.get(5)?,
                voices_attempted: r.get(6)?,
                voices_succeeded: r.get(7)?,
                elapsed_ms: r.get(8)?,
                voices_json: r.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Borra TODO el history (acción del user: "limpiar historial"). Devuelve cuántos borró. Es una
/// acción destructiva del propio dato del user (no del sistema) — el command la gatea + audita.
pub fn clear_runs(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<usize> {
    let conn = db.lock();
    let n = conn.execute("DELETE FROM council_runs", [])?;
    Ok(n)
}

// ── 019 F3 (T031) — Council CUSTOM-VOICES (config, NUNCA un tier-gate) ────────
// Voces pinneadas por el user que SIEMPRE participan del council, por encima del preset/template.
// F-II: el council es free para todos; las custom-voices son configuración (qué providers conectados
// pinear), no un paywall. provider_alias referencia una credencial ya conectada (Furx Connect); la
// key vive en Keychain (BYOK) — acá sólo el alias, NUNCA el secreto.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomVoice {
    pub id: String,
    pub provider_alias: String,
    /// Modelo concreto; None = el `default_ping_model` del provider.
    pub model: Option<String>,
    pub enabled: bool,
    pub created_at: String,
}

/// Agrega (o re-activa) una voz custom. `model` None = default del provider. Idempotente por
/// (provider_alias, model): si ya existe, la deja `enabled=1` y devuelve su id. Valida que el alias
/// no esté vacío. NO valida tier (F-II). NO toca el Keychain.
pub fn add_custom_voice(
    db: &Arc<parking_lot::Mutex<Connection>>,
    provider_alias: &str,
    model: Option<&str>,
) -> Result<String> {
    let alias = provider_alias.trim();
    if alias.is_empty() {
        return Err(anyhow!("provider_alias vacío"));
    }
    let conn = db.lock();
    // ¿ya existe (alias, model)? → re-activar y devolver su id (idempotente).
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM council_custom_voices WHERE provider_alias = ?1 AND COALESCE(model,'') = COALESCE(?2,'')",
            params![alias, model],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        conn.execute(
            "UPDATE council_custom_voices SET enabled = 1 WHERE id = ?1",
            params![id],
        )?;
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO council_custom_voices (id, provider_alias, model, enabled) VALUES (?1,?2,?3,1)",
        params![id, alias, model],
    )?;
    Ok(id)
}

/// Lista las voces custom (todas, enabled o no), más nuevas primero.
pub fn list_custom_voices(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<Vec<CustomVoice>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, provider_alias, model, enabled, created_at
         FROM council_custom_voices ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CustomVoice {
                id: r.get(0)?,
                provider_alias: r.get(1)?,
                model: r.get(2)?,
                enabled: r.get::<_, i64>(3)? != 0,
                created_at: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Habilita/deshabilita una voz custom sin borrarla. Idempotente. Devuelve true si cambió una fila.
pub fn set_custom_voice_enabled(
    db: &Arc<parking_lot::Mutex<Connection>>,
    id: &str,
    enabled: bool,
) -> Result<bool> {
    let conn = db.lock();
    let n = conn.execute(
        "UPDATE council_custom_voices SET enabled = ?2 WHERE id = ?1",
        params![id, if enabled { 1 } else { 0 }],
    )?;
    Ok(n == 1)
}

/// Borra una voz custom. Idempotente (devuelve true si borró algo).
pub fn remove_custom_voice(db: &Arc<parking_lot::Mutex<Connection>>, id: &str) -> Result<bool> {
    let conn = db.lock();
    let n = conn.execute(
        "DELETE FROM council_custom_voices WHERE id = ?1",
        params![id],
    )?;
    Ok(n == 1)
}

/// Resuelve las voces custom ENABLED a pares (credencial conectada, model) listos para el plan del
/// council. Cada voz custom cuyo `provider_alias` matchee una credencial conectada (cualquier
/// estado — el user la pinó a propósito) se incluye; las que no resuelven se ignoran (best-effort,
/// la credencial se desconectó). `model` None → `default_ping_model` del provider. Esto NO filtra
/// por preset ni tier (F-II): las custom-voices SIEMPRE participan.
fn resolve_custom_voices(
    db: &Arc<parking_lot::Mutex<Connection>>,
    creds: &[ProviderCredential],
) -> Vec<(ProviderCredential, String)> {
    let custom = match list_custom_voices(db) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for cv in custom.into_iter().filter(|c| c.enabled) {
        let Some(cred) = creds.iter().find(|c| c.alias == cv.provider_alias) else {
            continue; // credencial ya no conectada → se ignora silenciosamente
        };
        let model = match cv.model {
            Some(m) if !m.trim().is_empty() => m,
            _ => match ProviderKind::parse(&cred.provider) {
                Some(k) => k.default_ping_model().to_string(),
                None => continue,
            },
        };
        out.push((cred.clone(), model));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(alias: &str, provider: &str, status: &str) -> ProviderCredential {
        ProviderCredential {
            alias: alias.into(),
            provider: provider.into(),
            key_ref: Some(alias.into()),
            endpoint_url: None,
            status: status.into(),
            last_ping_ms: Some(100),
            last_ping_at: None,
            last_error_msg: None,
            scope_workspace: None,
            preset_member: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn preset_matches() {
        assert!(CouncilPreset::Quick.matches(&cred("a", "openrouter", "healthy")));
        assert!(!CouncilPreset::Quick.matches(&cred("a", "anthropic", "healthy")));
        assert!(CouncilPreset::Frontier.matches(&cred("a", "anthropic", "healthy")));
        assert!(CouncilPreset::Local.matches(&cred("a", "ollama", "healthy")));
        assert!(CouncilPreset::Mix.matches(&cred("a", "groq", "healthy")));
    }

    #[tokio::test]
    async fn build_plan_skips_red() {
        let creds = vec![
            cred("or1", "openrouter", "red"),
            cred("ant1", "anthropic", "healthy"),
        ];
        let plan = build_plan(
            creds,
            CouncilPreset::Mix,
            &std::collections::HashMap::new(),
            6,
        )
        .await;
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0.alias, "ant1");
    }

    #[tokio::test]
    async fn build_plan_quick_expands_openrouter() {
        let creds = vec![cred("or1", "openrouter", "healthy")];
        let plan = build_plan(
            creds,
            CouncilPreset::Quick,
            &std::collections::HashMap::new(),
            6,
        )
        .await;
        assert_eq!(plan.len(), 6);
        // All should be the same credential with distinct models
        let models: HashSet<_> = plan.iter().map(|(_, m)| m.clone()).collect();
        assert_eq!(models.len(), 6);
    }

    #[test]
    fn synthesize_no_voices() {
        let s = synthesize(&[], "test");
        assert!(s.contains("sin respuestas"));
    }

    // ── 019 F3 (T031) history + custom-voices ────────────────────────────────

    fn hist_db() -> Arc<parking_lot::Mutex<Connection>> {
        // Sólo las 2 tablas del council de la migración 037 (la 3ra sentencia, ALTER de
        // orchestration_tasks, no aplica acá — esa tabla la prueba orchestration.rs). Mantenemos
        // el SQL idéntico a la migración para que un cambio de schema rompa el test, no lo esconda.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS council_runs (
                id TEXT PRIMARY KEY,
                ran_at TEXT NOT NULL DEFAULT (datetime('now')),
                preset TEXT NOT NULL DEFAULT 'mix',
                template TEXT,
                prompt TEXT NOT NULL DEFAULT '',
                synth TEXT NOT NULL DEFAULT '',
                voices_attempted INTEGER NOT NULL DEFAULT 0,
                voices_succeeded INTEGER NOT NULL DEFAULT 0,
                elapsed_ms INTEGER NOT NULL DEFAULT 0,
                voices_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE INDEX IF NOT EXISTS idx_council_runs_ran_at ON council_runs(ran_at);
            CREATE TABLE IF NOT EXISTS council_custom_voices (
                id TEXT PRIMARY KEY,
                provider_alias TEXT NOT NULL,
                model TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_council_custom_voice_uniq
                ON council_custom_voices(provider_alias, COALESCE(model, ''));",
        )
        .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    #[test]
    fn council_history_records_and_lists_redacted() {
        let db = hist_db();
        let result = CouncilResult {
            voices: vec![VoiceResult {
                provider: "groq".into(),
                alias: "g1".into(),
                model: "llama-3.3-70b".into(),
                ok: true,
                content: "respuesta".into(),
                latency_ms: 123,
                error: None,
            }],
            synth: "síntesis con token=ghp_abcdefghijklmnopqrstuvwxyz0123456789 adentro".into(),
            elapsed_ms: 456,
            preset: "mix".into(),
            voices_attempted: 1,
            voices_succeeded: 1,
        };
        record_run(
            &db,
            "prompt con sk-proj-ABCDEFGHIJKLMNOPQRSTUV pegado",
            &Some("review".to_string()),
            &result,
        )
        .unwrap();
        let runs = list_runs(&db, 10).unwrap();
        assert_eq!(runs.len(), 1);
        let r = &runs[0];
        assert_eq!(r.preset, "mix");
        assert_eq!(r.template.as_deref(), Some("review"));
        assert_eq!(r.voices_succeeded, 1);
        // F-I BYOK: ni el prompt ni la síntesis persisten el secret.
        assert!(!r.prompt.contains("sk-proj-ABCDEFGHIJKLMNOPQRSTUV"));
        assert!(!r.synth.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        // voices_json = metadata, sin el contenido de la voz.
        assert!(r.voices_json.contains("groq"));
        assert!(!r.voices_json.contains("respuesta"));
        // clear vacía el history.
        assert_eq!(clear_runs(&db).unwrap(), 1);
        assert_eq!(list_runs(&db, 10).unwrap().len(), 0);
    }

    #[test]
    fn custom_voices_crud_idempotent() {
        let db = hist_db();
        let id = add_custom_voice(&db, "my-groq", Some("llama-3.3-70b")).unwrap();
        // mismo (alias, model) → idempotente, mismo id, re-activado.
        let id2 = add_custom_voice(&db, "my-groq", Some("llama-3.3-70b")).unwrap();
        assert_eq!(id, id2);
        // alias vacío rechazado.
        assert!(add_custom_voice(&db, "  ", None).is_err());
        // distinto model = otra voz.
        let _id3 = add_custom_voice(&db, "my-groq", None).unwrap();
        assert_eq!(list_custom_voices(&db).unwrap().len(), 2);
        // disable / enable.
        assert!(set_custom_voice_enabled(&db, &id, false).unwrap());
        assert!(
            !list_custom_voices(&db)
                .unwrap()
                .iter()
                .find(|v| v.id == id)
                .unwrap()
                .enabled
        );
        // remove.
        assert!(remove_custom_voice(&db, &id).unwrap());
        assert_eq!(list_custom_voices(&db).unwrap().len(), 1);
        assert!(!remove_custom_voice(&db, &id).unwrap()); // ya borrada → false
    }

    #[test]
    fn resolve_custom_voices_only_enabled_and_connected() {
        let db = hist_db();
        // 2 custom: una con alias conectado, otra con alias inexistente.
        add_custom_voice(&db, "connected", Some("model-x")).unwrap();
        let ghost = add_custom_voice(&db, "ghost-alias", None).unwrap();
        // una disabled no participa.
        let off = add_custom_voice(&db, "connected", None).unwrap();
        set_custom_voice_enabled(&db, &off, false).unwrap();
        let creds = vec![cred("connected", "groq", "healthy")];
        let resolved = resolve_custom_voices(&db, &creds);
        // sólo la enabled + conectada con model explícito.
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].0.alias, "connected");
        assert_eq!(resolved[0].1, "model-x");
        let _ = ghost; // la voz fantasma se ignora (alias no conectado)
    }
}
