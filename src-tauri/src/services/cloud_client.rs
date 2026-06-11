//! Cloud client for api.furx.cloud — magic-link auth, session token storage in
//! Keychain, trace upload (fire-and-forget), project/trace management.
//!
//! Architecture (F-V of constitution):
//!  - `FURX_API_BASE` env (or default `https://api.furx.cloud`) selects the backend.
//!  - `FURX_INTERNAL_MODE=1` skips opt-in banners and tier checks (dogfooding).
//!  - Session token is stored in Keychain under service `furx-cloud-session`, account
//!    derived from the user's email; never touches argv.
//!
//! BYOK invariant (F-I): we NEVER send provider API keys to api.furx.cloud.
//! The trace upload sanitizes payloads client-side first (mirror of Worker pass);
//! AIE bearer is server-side only and lives in the Worker (not the client).

use crate::services::cloud_sanitizer::{sanitize, SanitizerReport};
use crate::services::keychain;
use anyhow::{anyhow, Context, Result};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_BASE: &str = "https://api.furx.cloud";
const SESSION_SERVICE: &str = "furx-cloud-session";
const ACTIVE_USER_SERVICE: &str = "furx-cloud-active-user";
const ACTIVE_USER_ACCOUNT: &str = "default";

fn api_base() -> String {
    std::env::var("FURX_API_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_string())
}

pub fn is_internal_mode() -> bool {
    std::env::var("FURX_INTERNAL_MODE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Furx-Desktop/0.1")
        .build()
        .expect("build reqwest client")
}

// ── Session storage ─────────────────────────────────────────────────────────

fn save_session(email: &str, token: &str) -> Result<()> {
    keychain::save(SESSION_SERVICE, email, token).map_err(|e| anyhow!("keychain save: {}", e))?;
    keychain::save(ACTIVE_USER_SERVICE, ACTIVE_USER_ACCOUNT, email)
        .map_err(|e| anyhow!("keychain save active: {}", e))?;
    Ok(())
}

pub fn active_user() -> Option<String> {
    keychain::load(ACTIVE_USER_SERVICE, ACTIVE_USER_ACCOUNT)
}

pub fn session_token() -> Option<String> {
    let email = active_user()?;
    keychain::load(SESSION_SERVICE, &email)
}

pub fn clear_session() {
    if let Some(email) = active_user() {
        keychain::delete(SESSION_SERVICE, &email);
    }
    keychain::delete(ACTIVE_USER_SERVICE, ACTIVE_USER_ACCOUNT);
    // Post-audit fix V3#2 — reset uploader counters so the UI status dot shows fresh state.
    // Doesn't drain the in-flight queue (those jobs will hit "not signed in" and exit early
    // via upload_with_retry's permanent-error check), but presents a clean slate to the user.
    use std::sync::atomic::Ordering;
    crate::services::cloud_uploader::DROPPED_EVENTS.store(0, Ordering::Relaxed);
    crate::services::cloud_uploader::SUCCEEDED_UPLOADS.store(0, Ordering::Relaxed);
    crate::services::cloud_uploader::FAILED_UPLOADS.store(0, Ordering::Relaxed);
    crate::services::cloud_uploader::UPLOAD_STATE.store(0, Ordering::Relaxed);
}

// ── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct VerifyReq<'a> {
    pub token: &'a str,
    pub device_name: Option<String>,
    pub device_platform: Option<&'static str>,
    pub furx_version: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
pub struct VerifyResp {
    pub token: String,
    pub user: WhoAmIUser,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WhoAmIUser {
    pub id: String,
    pub email: String,
    pub plan: String,
}

#[derive(Debug, Deserialize)]
pub struct WhoAmIResp {
    pub user: WhoAmIUser,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub cloud_traces_enabled: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct TraceUpload<'a> {
    pub trace_id: Option<String>,
    pub project_id: &'a str,
    pub ts: i64,
    pub model: &'a str,
    pub provider: &'a str,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub latency_ms: Option<u32>,
    pub cost_usd_micro: Option<u32>,
    pub status: &'a str,
    pub error_class: Option<&'a str>,
    pub prompt_hash: Option<&'a str>,
    pub response_hash: Option<&'a str>,
    pub memory_refs: Option<Vec<String>>,
    pub replay_id: Option<&'a str>,
    pub payload: Option<TracePayload<'a>>,
    pub council_parent_trace_id: Option<&'a str>,
    pub council_voice_alias: Option<&'a str>,
    pub council_voice_position: Option<u32>,
    pub client_sanitizer_passes: Option<SanitizerReport>,
}

#[derive(Debug, Serialize)]
pub struct TracePayload<'a> {
    pub prompt: Option<&'a str>,
    pub response: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct TraceUploadResp {
    pub trace_id: String,
    pub blob_uploaded: bool,
    pub worker_sanitizer_hits: u32,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: String,
    message: String,
}

// ── Public API (used by Tauri commands) ─────────────────────────────────────

/// Step 1 of magic-link flow. Sends email to the API; user receives a link.
pub async fn request_signin(email: &str) -> Result<()> {
    let resp = client()
        .post(format!("{}/v1/auth/request", api_base()))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .context("network request_signin")?;
    if !resp.status().is_success() {
        let body: ApiError = resp.json().await.unwrap_or(ApiError {
            error: "unknown".into(),
            message: "request failed".into(),
        });
        return Err(anyhow!("{}: {}", body.error, body.message));
    }
    Ok(())
}

/// Step 2 of magic-link flow. Exchanges the magic-link token for a session token,
/// stores the session in Keychain, returns the user info.
pub async fn verify(token: &str) -> Result<WhoAmIUser> {
    let req = VerifyReq {
        token,
        device_name: hostname().ok(),
        device_platform: Some(platform_name()),
        furx_version: option_env!("CARGO_PKG_VERSION"),
    };
    let resp = client()
        .post(format!("{}/v1/auth/verify", api_base()))
        .json(&req)
        .send()
        .await
        .context("network verify")?;
    if !resp.status().is_success() {
        let body: ApiError = resp.json().await.unwrap_or(ApiError {
            error: "unknown".into(),
            message: "verify failed".into(),
        });
        return Err(anyhow!("{}: {}", body.error, body.message));
    }
    let parsed: VerifyResp = resp.json().await.context("parse verify resp")?;
    save_session(&parsed.user.email, &parsed.token).context("save session")?;
    Ok(parsed.user)
}

/// Returns current user info (or error if no session). Uses Keychain session token.
pub async fn whoami() -> Result<WhoAmIUser> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!("{}/v1/auth/whoami", api_base()))
        .bearer_auth(&token)
        .send()
        .await
        .context("network whoami")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("whoami HTTP {}", resp.status()));
    }
    let parsed: WhoAmIResp = resp.json().await.context("parse whoami")?;
    Ok(parsed.user)
}

/// Revoke the current session on the server side and wipe Keychain.
pub async fn revoke() -> Result<()> {
    if let Some(token) = session_token() {
        let _ = client()
            .post(format!("{}/v1/auth/revoke", api_base()))
            .bearer_auth(&token)
            .send()
            .await;
    }
    clear_session();
    Ok(())
}

pub async fn list_projects() -> Result<Vec<ProjectRow>> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!("{}/v1/projects", api_base()))
        .bearer_auth(&token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("list projects HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await?;
    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

pub async fn create_project(name: &str, cloud_traces_enabled: bool) -> Result<ProjectRow> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .post(format!("{}/v1/projects", api_base()))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "name": name, "cloud_traces_enabled": cloud_traces_enabled }))
        .send()
        .await?;
    if !resp.status().is_success() {
        let body: ApiError = resp.json().await.unwrap_or(ApiError {
            error: "unknown".into(),
            message: "create project failed".into(),
        });
        return Err(anyhow!("{}: {}", body.error, body.message));
    }
    Ok(resp.json().await?)
}

pub async fn set_project_cloud_traces(project_id: &str, enabled: bool) -> Result<()> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .patch(format!("{}/v1/projects/{}", api_base(), project_id))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "cloud_traces_enabled": enabled }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("set project HTTP {}", resp.status()));
    }
    Ok(())
}

/// Upload one trace. Runs client-side sanitizer if payload is present.
/// Returns the trace_id assigned by the server (or echoed if client-generated).
pub async fn upload_trace(mut tu: TraceUpload<'_>) -> Result<TraceUploadResp> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;

    // If we're shipping a payload, run pass-1 sanitizer and attach the report.
    if let Some(p) = &tu.payload {
        let combined = format!(
            "{}\n---\n{}",
            p.prompt.unwrap_or(""),
            p.response.unwrap_or(""),
        );
        let (_sanitized, report) = sanitize(&combined);
        // We send the original payload (Worker re-sanitizes); we attach the report so
        // the Worker can record sanitizer_hits ledger pass=1. The Worker's pass=2 is
        // authoritative for what gets stored in R2.
        tu.client_sanitizer_passes = Some(report);
    }

    let resp = client()
        .post(format!("{}/v1/traces", api_base()))
        .bearer_auth(&token)
        .json(&tu)
        .send()
        .await
        .context("network upload trace")?;
    if !resp.status().is_success() {
        if resp.status() == StatusCode::ACCEPTED {
            // 202 with body — project hasn't opted in; treat as silent no-op
            let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            return Err(anyhow!("project_cloud_traces_disabled: {}", body));
        }
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(anyhow!("upload HTTP {}: {}", status, txt));
    }
    Ok(resp.json().await?)
}

// ── Council compare (spec 004 F4) ────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CouncilVoice {
    #[serde(default)]
    pub voice_alias: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub voice_position: i64,
    #[serde(default)]
    pub response: String,
}

/// Fetch the council voices (with response text resolved server-side) for a parent
/// council trace — for the comparator pane's "Load council" source.
pub async fn get_council_voices(parent_id: &str) -> Result<Vec<CouncilVoice>> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!(
            "{}/v1/traces/{}/children?include=response",
            api_base(),
            parent_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .context("network get_council_voices")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("get_council_voices HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.context("parse children")?;
    let arr = body
        .get("children")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RecentCouncil {
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub ts: i64,
    #[serde(default)]
    pub child_count: i64,
}

/// List recent council parent traces (child_count>0) for the comparator's "recent
/// councils" dropdown. Filters client-side from the trace list; returns up to 10.
pub async fn list_recent_councils() -> Result<Vec<RecentCouncil>> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!("{}/v1/traces?limit=50", api_base()))
        .bearer_auth(&token)
        .send()
        .await
        .context("network list_recent_councils")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("list_recent_councils HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.context("parse traces")?;
    let arr = body
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RecentCouncil>(v).ok())
        .filter(|t| t.child_count > 0)
        .take(10)
        .collect())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegressionCase {
    #[serde(default)]
    pub case_id: String,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub baseline: String,
    #[serde(default)]
    pub candidate: String,
}

/// Fetch a regression run's per-case candidate/baseline output text (resolved server-side)
/// for the comparator pane's "Load regression" source.
pub async fn get_regression_outputs(run_id: &str) -> Result<Vec<RegressionCase>> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!(
            "{}/v1/regression/runs/{}?include=outputs",
            api_base(),
            run_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .context("network get_regression_outputs")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("get_regression_outputs HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.context("parse regression run")?;
    let arr = body
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

// ── Deep-link auth token parsing (spec, Sprint #5 / L7) ──────────────────────

/// Parse a `furx://auth?token=...` deep-link URL and return the token, or None.
/// Accepts both `furx://auth?token=X` and `furx://localhost/auth?token=X` (OS
/// normalization differs). Pure + testable — the email-delivery half (magic link
/// reaching the user's inbox) needs a real Resend key and is a human/infra step.
pub fn parse_auth_token(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    if url.scheme() != "furx" {
        return None;
    }
    let is_auth = url.host_str() == Some("auth") || url.path() == "/auth" || url.path() == "auth";
    if !is_auth {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod deeplink_tests {
    use super::parse_auth_token;

    #[test]
    fn parses_host_form() {
        assert_eq!(
            parse_auth_token("furx://auth?token=abc123").as_deref(),
            Some("abc123")
        );
    }
    #[test]
    fn parses_localhost_path_form() {
        assert_eq!(
            parse_auth_token("furx://localhost/auth?token=xyz").as_deref(),
            Some("xyz")
        );
    }
    #[test]
    fn rejects_wrong_scheme() {
        assert_eq!(parse_auth_token("https://auth?token=abc"), None);
    }
    #[test]
    fn rejects_non_auth_path() {
        assert_eq!(parse_auth_token("furx://replay/123?token=abc"), None);
    }
    #[test]
    fn rejects_missing_or_empty_token() {
        assert_eq!(parse_auth_token("furx://auth"), None);
        assert_eq!(parse_auth_token("furx://auth?token="), None);
    }
    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_auth_token("not a url"), None);
    }
}

// ── Active Persona Pack (spec 003 T3.7) ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ActivePackResp {
    active: bool,
    pack_id: Option<String>,
    #[serde(default)]
    version: i64,
    system_prompt: Option<String>,
    #[serde(default)]
    examples: Vec<crate::services::active_pack::PackExample>,
}

/// Resolve the active Persona Pack for a project in one request.
/// Returns `Ok(None)` when no pack is applied (or it was rolled back / not in
/// 'applied' status). Errors only on auth/network failure — the caller treats
/// those as "keep the last cached pack" so the council never breaks.
pub async fn get_active_pack(
    project_id: &str,
) -> Result<Option<crate::services::active_pack::ActivePack>> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .get(format!(
            "{}/v1/projects/{}/active-pack",
            api_base(),
            project_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .context("network get_active_pack")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("get_active_pack HTTP {}", resp.status()));
    }
    let parsed: ActivePackResp = resp.json().await.context("parse active-pack")?;
    if !parsed.active {
        return Ok(None);
    }
    match (parsed.pack_id, parsed.system_prompt) {
        (Some(pack_id), Some(system_prompt)) if !system_prompt.is_empty() => {
            Ok(Some(crate::services::active_pack::ActivePack {
                pack_id,
                version: parsed.version,
                system_prompt,
                examples: parsed.examples,
            }))
        }
        _ => Ok(None),
    }
}

// ── Multi-machine sync (spec 050 FR-001) ─────────────────────────────────────
//
// Relay opt-in + FAIL-CLOSED de preferencias de usuario (overrides MCP / monitor targets / gotchas)
// entre las máquinas del MISMO usuario. `sync_exchange` sube el payload local y baja el remoto del
// usuario en UN request (el relay devuelve el estado consolidado del servidor para este usuario).
// Si NO hay sesión o el relay falla/no existe, devuelve Err → el caller MANTIENE el estado local
// (cero regresión). NUNCA lleva secrets (el payload sólo tiene preferencias; el Worker re-sanitiza).

/// Sube el payload local y baja el remoto del usuario en un POST a `/v1/sync`. Devuelve el payload
/// remoto (estado del servidor para este usuario). FAIL-CLOSED: error de auth/red/HTTP → Err (el
/// caller no toca el estado local). El endpoint puede no existir aún en el relay → Err (no rompe nada).
pub async fn sync_exchange(local: &serde_json::Value) -> Result<serde_json::Value> {
    let token = session_token().ok_or_else(|| anyhow!("not signed in"))?;
    let resp = client()
        .post(format!("{}/v1/sync", api_base()))
        .bearer_auth(&token)
        .json(local)
        .send()
        .await
        .context("network sync_exchange")?;
    if resp.status() == StatusCode::UNAUTHORIZED {
        clear_session();
        return Err(anyhow!("session revoked or expired"));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("sync HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.context("parse sync resp")?;
    Ok(body)
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn hostname() -> Result<String> {
    Ok(std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into()))
}

const fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "other"
    }
}
