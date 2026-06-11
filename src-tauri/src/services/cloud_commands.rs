//! Tauri commands exposed to the frontend for the cloud integration.
//!
//! Naming convention: `cloud_*`. All async, all return `Result<T, String>` so the
//! frontend gets a structured `{ok|error}` shape via Tauri's invoke wrapper.
//!
//! Bring them up in the same `invoke_handler` block in `lib.rs` next to the existing
//! commands. Frontend wrapper lives in TBD `src/lib/cloud.ts`.

use crate::services::{cloud_client, cloud_uploader};

#[tauri::command]
pub async fn cloud_active_user() -> Result<Option<String>, String> {
    Ok(cloud_client::active_user())
}

/// Status of the background cloud uploader — used by the status bar dot.
#[tauri::command]
pub async fn cloud_uploader_status() -> cloud_uploader::UploaderStatus {
    cloud_uploader::status_snapshot()
}

#[tauri::command]
pub async fn cloud_request_signin(email: String) -> Result<(), String> {
    cloud_client::request_signin(&email)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_verify(token: String) -> Result<cloud_client::WhoAmIUser, String> {
    cloud_client::verify(&token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_whoami() -> Result<cloud_client::WhoAmIUser, String> {
    cloud_client::whoami().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_revoke() -> Result<(), String> {
    cloud_client::revoke().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_is_internal_mode() -> bool {
    cloud_client::is_internal_mode()
}

#[tauri::command]
pub async fn cloud_list_projects() -> Result<Vec<cloud_client::ProjectRow>, String> {
    cloud_client::list_projects()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_create_project(
    name: String,
    cloud_traces_enabled: bool,
) -> Result<cloud_client::ProjectRow, String> {
    cloud_client::create_project(&name, cloud_traces_enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cloud_set_project_traces_enabled(
    project_id: String,
    enabled: bool,
) -> Result<(), String> {
    cloud_client::set_project_cloud_traces(&project_id, enabled)
        .await
        .map_err(|e| e.to_string())
}

/// Sprint #6 — synthetic test trace, used by the "Send test trace" button to
/// validate the full producer→uploader→dashboard chain in one click. NO provider
/// call is made; the trace payload is fully synthetic so it doesn't burn provider
/// quota or leak PII. Returns the queued local_event_id so the UI can poll
/// cloud_uploader_status afterwards to confirm the upload landed.
///
/// Audit V2 fix: in-process rate limit (1 emission per 10s) so a held-down click
/// or buggy retry loop can't burn R2/D1 writes. Soft limit (returns Err with
/// retry_after); UI debounce is the primary user-facing protection.
static LAST_TEST_EMIT_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
const TEST_EMIT_MIN_GAP_MS: i64 = 10_000;

#[tauri::command]
pub async fn cloud_emit_test_trace() -> Result<String, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let prev = LAST_TEST_EMIT_MS.load(std::sync::atomic::Ordering::Relaxed);
    if prev != 0 && now_ms - prev < TEST_EMIT_MIN_GAP_MS {
        let retry_in = (TEST_EMIT_MIN_GAP_MS - (now_ms - prev)) / 1000;
        return Err(format!("rate limited — retry in {}s", retry_in.max(1)));
    }
    LAST_TEST_EMIT_MS.store(now_ms, std::sync::atomic::Ordering::Relaxed);

    let project_id = std::env::var("FURX_DEFAULT_PROJECT_ID")
        .map_err(|_| "not signed in — bootstrap default project first".to_string())?;
    let uploader = crate::services::cloud_uploader::take_global()
        .ok_or_else(|| "cloud uploader not running".to_string())?;
    let now = chrono::Utc::now().timestamp_millis();
    let local_event_id = format!("test-{}", now);
    let job = crate::services::cloud_uploader::CloudUploadJob {
        trace_id: None,
        project_id,
        ts: now,
        model: "furx-synthetic-test".to_string(),
        provider: "furx-internal".to_string(),
        tokens_in: Some(12),
        tokens_out: Some(34),
        latency_ms: Some(123),
        cost_usd_micro: Some(0),
        status: "success".to_string(),
        error_class: None,
        prompt: Some(format!("Furx test trace — synthetic prompt at ts={}", now)),
        response: Some("Furx test trace — synthetic response. If you see this in the dashboard, the full producer→uploader→API→D1 chain is working.".to_string()),
        replay_id: None,
        local_event_id: Some(local_event_id.clone()),
        council_parent_trace_id: None,
        council_voice_alias: None,
        council_voice_position: None,
    };
    if uploader.try_enqueue(job) {
        Ok(local_event_id)
    } else {
        Err("upload queue full (drop-newest engaged); retry in a few seconds".to_string())
    }
}

/// Sprint #5 — ensure a default project exists for trace uploads.
/// Called once after sign-in (and after deep-link verify). Lists projects;
/// if none exist, creates one with cloud_traces_enabled=true. Always sets
/// FURX_DEFAULT_PROJECT_ID env so producer hooks know where to send traces.
#[tauri::command]
pub async fn cloud_bootstrap_default_project() -> Result<String, String> {
    let projects = cloud_client::list_projects()
        .await
        .map_err(|e| e.to_string())?;
    let project_id = if let Some(p) = projects.into_iter().next() {
        p.id
    } else {
        let p = cloud_client::create_project("default", true)
            .await
            .map_err(|e| e.to_string())?;
        p.id
    };
    // env var is read by council_multi's producer hook; this is a per-process side-effect.
    // SAFETY: single-threaded write at sign-in time; producer reads later.
    std::env::set_var("FURX_DEFAULT_PROJECT_ID", &project_id);
    Ok(project_id)
}

#[derive(Debug, serde::Deserialize)]
pub struct CloudTraceInput {
    pub trace_id: Option<String>,
    pub project_id: String,
    pub ts: i64,
    pub model: String,
    pub provider: String,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub latency_ms: Option<u32>,
    pub cost_usd_micro: Option<u32>,
    pub status: String,
    pub error_class: Option<String>,
    pub prompt: Option<String>,
    pub response: Option<String>,
    pub replay_id: Option<String>,
}

#[tauri::command]
pub async fn cloud_upload_trace(input: CloudTraceInput) -> Result<String, String> {
    let payload = if input.prompt.is_some() || input.response.is_some() {
        Some(cloud_client::TracePayload {
            prompt: input.prompt.as_deref(),
            response: input.response.as_deref(),
        })
    } else {
        None
    };
    let tu = cloud_client::TraceUpload {
        trace_id: input.trace_id.clone(),
        project_id: &input.project_id,
        ts: input.ts,
        model: &input.model,
        provider: &input.provider,
        tokens_in: input.tokens_in,
        tokens_out: input.tokens_out,
        latency_ms: input.latency_ms,
        cost_usd_micro: input.cost_usd_micro,
        status: &input.status,
        error_class: input.error_class.as_deref(),
        prompt_hash: None,
        response_hash: None,
        memory_refs: None,
        replay_id: input.replay_id.as_deref(),
        payload,
        council_parent_trace_id: None,
        council_voice_alias: None,
        council_voice_position: None,
        client_sanitizer_passes: None,
    };
    let resp = cloud_client::upload_trace(tu)
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.trace_id)
}
// touch 1779995553

/// spec 004 F4 — fetch council voices (with response text) for the comparator pane.
#[tauri::command]
pub async fn cloud_council_compare(
    trace_id: String,
) -> Result<Vec<crate::services::cloud_client::CouncilVoice>, String> {
    cloud_client::get_council_voices(&trace_id)
        .await
        .map_err(|e| e.to_string())
}

/// spec 004 F4 — fetch a regression run's candidate/baseline outputs for the comparator.
#[tauri::command]
pub async fn cloud_regression_compare(
    run_id: String,
) -> Result<Vec<crate::services::cloud_client::RegressionCase>, String> {
    cloud_client::get_regression_outputs(&run_id)
        .await
        .map_err(|e| e.to_string())
}

/// spec 004 F4 — list recent council parent traces for the comparator's dropdown.
#[tauri::command]
pub async fn cloud_recent_councils(
) -> Result<Vec<crate::services::cloud_client::RecentCouncil>, String> {
    cloud_client::list_recent_councils()
        .await
        .map_err(|e| e.to_string())
}

// ── 050 Ola 8 P2 (FR-001) — Multi-machine sync ───────────────────────────────────────────────────

/// Estado del sync multi-máquina para la UI (gating + último resultado).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncStatus {
    /// `true` si el setting opt-in está ON.
    pub enabled: bool,
    /// `true` si hay sesión cloud (sin ella, sync no puede correr).
    pub signed_in: bool,
}

/// Estado del sync (opt-in + sesión). Read-only; no toca red.
#[tauri::command]
pub fn sync_status(state: tauri::State<'_, crate::AppState>) -> SyncStatus {
    let enabled = {
        let conn = state.db.lock();
        crate::services::multi_sync::is_enabled(&conn)
    };
    SyncStatus {
        enabled,
        signed_in: cloud_client::active_user().is_some(),
    }
}

/// Resultado de un ciclo de sync (para la UI).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncRunResult {
    pub ran: bool,
    pub upserted: usize,
    pub deleted: usize,
    /// Mensaje legible (p.ej. "sync OFF", "relay no disponible — estado local intacto").
    pub note: String,
}

/// Corre UN ciclo de sync: build payload local → exchange con el relay → merge LWW → apply local.
/// GATING + FAIL-CLOSED:
///   - Si el opt-in está OFF → no-op explícito (`ran=false`, cero regresión).
///   - Si no hay sesión o el relay falla → Err propagado como `note`, ESTADO LOCAL INTACTO (no se
///     aplica nada). El merge/apply SÓLO corre con un payload remoto válido.
#[tauri::command]
pub async fn sync_now(state: tauri::State<'_, crate::AppState>) -> Result<SyncRunResult, String> {
    use crate::services::multi_sync as ms;
    // Gate opt-in (default OFF → no-op).
    {
        let conn = state.db.lock();
        if !ms::is_enabled(&conn) {
            return Ok(SyncRunResult {
                ran: false,
                upserted: 0,
                deleted: 0,
                note: "sync multi-máquina desactivado (opt-in)".into(),
            });
        }
    }
    // Build local payload.
    let local = ms::build_local_payload(&state.db);
    let local_json = serde_json::to_value(&local).map_err(|e| e.to_string())?;
    // Exchange con el relay. FAIL-CLOSED: error → estado local intacto (no aplicamos nada).
    let remote_json = match cloud_client::sync_exchange(&local_json).await {
        Ok(v) => v,
        Err(e) => {
            return Ok(SyncRunResult {
                ran: false,
                upserted: 0,
                deleted: 0,
                note: format!("relay no disponible — estado local intacto ({e})"),
            });
        }
    };
    // Parsear el payload remoto. Si no parsea, fail-closed (no tocamos nada).
    let remote: ms::SyncPayload = match serde_json::from_value(remote_json) {
        Ok(p) => p,
        Err(e) => {
            return Ok(SyncRunResult {
                ran: false,
                upserted: 0,
                deleted: 0,
                note: format!("payload remoto inválido — estado local intacto ({e})"),
            });
        }
    };
    // Merge LWW + apply local (idempotente; sólo escribe lo estrictamente más nuevo).
    let merged = ms::merge_payloads(&local, &remote);
    let report = ms::apply_merged(&state.db, &merged);
    Ok(SyncRunResult {
        ran: true,
        upserted: report.upserted,
        deleted: report.deleted,
        note: "sync completado".into(),
    })
}
