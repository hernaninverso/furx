//! Cloud uploader task — drains LLM-trace events from a tokio mpsc channel and
//! POSTs them to api.furx.cloud asynchronously.
//!
//! Sprint #1 (2026-05-28) — council 6/6 voces approved option B (mpsc + uploader task).
//!
//! Backpressure: the channel is bounded to UPLOAD_QUEUE_CAP. When the channel is full,
//! NEW events are dropped (not oldest) and `DROPPED_EVENTS` counter is incremented.
//! Trade-off rationale: drop-oldest would require a side ring buffer (more code +
//! Arc<Mutex>); drop-newest with a visible counter gives Furx the same operational
//! signal ("something is wrong with sync") via the status dot, and the dropped event
//! was already persisted locally in the `events` table — so audit integrity is preserved.
//!
//! BYOK invariant (F-I): the event payload sent here has ALREADY been sanitized at the
//! AuditWriter call site (any field that could contain a provider API key must come from
//! sanitized strings only). The cloud_client.upload_trace also re-sanitizes server-side.

use crate::services::cloud_client::{self, TracePayload, TraceUpload};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

const UPLOAD_QUEUE_CAP: usize = 1000;
const HTTP_RETRY_BASE_DELAY_MS: u64 = 500;
const HTTP_RETRY_MAX_ATTEMPTS: u8 = 3;
/// After N consecutive failed uploads (no success in between), the uploader enters
/// a cooldown for COOLDOWN_AFTER_CONSECUTIVE_FAILS_MS before it drains the next msg.
/// (Post-audit fix V2#2 — circuit breaker so a dead server doesn't burn CPU forever.)
const CIRCUIT_BREAKER_THRESHOLD: u64 = 5;
const COOLDOWN_AFTER_CONSECUTIVE_FAILS_MS: u64 = 5 * 60 * 1000;

/// One queued upload job. Owned strings so the channel can outlive the caller.
#[derive(Debug, Clone)]
pub struct CloudUploadJob {
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
    /// Local audit_events.id — for backreference. Server keeps its own trace_id.
    pub local_event_id: Option<String>,
    // spec 001 H3 — council parent→child linkage. A council run uploads one synthetic
    // parent job (these three None) plus N child jobs (parent_trace_id = the parent's id).
    pub council_parent_trace_id: Option<String>,
    pub council_voice_alias: Option<String>,
    pub council_voice_position: Option<u32>,
}

/// Status of the uploader, exposed to the frontend via the status bar dot.
/// 0 = idle (no pending), 1 = working (drain in progress), 2 = paused/error.
pub static UPLOAD_STATE: AtomicU8 = AtomicU8::new(0);
/// Monotonic count of dropped events (drop-newest backpressure).
pub static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);
/// Monotonic count of successful uploads.
pub static SUCCEEDED_UPLOADS: AtomicU64 = AtomicU64::new(0);
/// Monotonic count of failed uploads after retries exhausted.
pub static FAILED_UPLOADS: AtomicU64 = AtomicU64::new(0);

/// Shared bounded ring buffer (L4: drop-OLDEST). When full, the oldest unsent job is
/// evicted to make room for the newest — newer traces are more relevant than stale ones.
/// (Previously an mpsc channel that dropped the NEWEST event.)
type Ring = Arc<Mutex<VecDeque<CloudUploadJob>>>;

/// Sender handle — clonable, given to AuditWriter so it can enqueue.
#[derive(Clone, Debug)]
pub struct UploaderHandle {
    queue: Ring,
    notify: Arc<Notify>,
}

/// Consumer side, moved into the uploader task (consumed once).
pub struct UploaderConsumer {
    queue: Ring,
    notify: Arc<Notify>,
}

impl UploaderHandle {
    /// Enqueue a job for upload. Non-blocking. If the buffer is full (cap reached),
    /// drops the OLDEST queued event and increments DROPPED_EVENTS, then pushes the new
    /// one. Always returns true (the new event is always accepted).
    pub fn try_enqueue(&self, job: CloudUploadJob) -> bool {
        {
            let mut q = self.queue.lock();
            while q.len() >= UPLOAD_QUEUE_CAP {
                q.pop_front();
                DROPPED_EVENTS.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(job);
        }
        self.notify.notify_one();
        true
    }
}

/// Create the ring buffer (sync — safe to call before the tokio runtime exists).
/// Returns a clonable handle for enqueueing and a consumer that must be passed into
/// `spawn_uploader_task` from inside a tokio context (e.g. the tauri setup callback).
pub fn create() -> (UploaderHandle, UploaderConsumer) {
    let queue: Ring = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
    let notify = Arc::new(Notify::new());
    (
        UploaderHandle {
            queue: queue.clone(),
            notify: notify.clone(),
        },
        UploaderConsumer { queue, notify },
    )
}

/// Spawn the uploader task. MUST be called from a tokio runtime context
/// (Tauri's `tauri::async_runtime::spawn` inside `.setup()` qualifies).
pub fn spawn_uploader_task(consumer: UploaderConsumer) {
    tauri::async_runtime::spawn(uploader_loop(consumer));
}

async fn uploader_loop(consumer: UploaderConsumer) {
    let mut consecutive_fails: u64 = 0;
    loop {
        // Drain FIFO. pop_front returns the oldest surviving job; when empty, await a notify.
        let job = { consumer.queue.lock().pop_front() };
        let Some(job) = job else {
            UPLOAD_STATE.store(0, Ordering::Relaxed);
            consumer.notify.notified().await;
            continue;
        };
        // Circuit breaker (V2#2): if we've failed N times in a row, sleep before next attempt
        // so a dead server doesn't burn CPU + bandwidth. A single success resets the counter.
        if consecutive_fails >= CIRCUIT_BREAKER_THRESHOLD {
            UPLOAD_STATE.store(2, Ordering::Relaxed);
            tracing::warn!(consecutive_fails, "cloud uploader cooldown engaged");
            tokio::time::sleep(std::time::Duration::from_millis(
                COOLDOWN_AFTER_CONSECUTIVE_FAILS_MS,
            ))
            .await;
        }
        UPLOAD_STATE.store(1, Ordering::Relaxed);
        match upload_with_retry(&job).await {
            Ok(_) => {
                SUCCEEDED_UPLOADS.fetch_add(1, Ordering::Relaxed);
                consecutive_fails = 0;
                let empty = consumer.queue.lock().is_empty();
                UPLOAD_STATE.store(if empty { 0 } else { 1 }, Ordering::Relaxed);
            }
            Err(e) => {
                FAILED_UPLOADS.fetch_add(1, Ordering::Relaxed);
                consecutive_fails += 1;
                UPLOAD_STATE.store(2, Ordering::Relaxed);
                tracing::warn!(error = %e, trace_id = ?job.trace_id, consecutive_fails, "cloud upload failed permanently");
            }
        }
    }
}

async fn upload_with_retry(job: &CloudUploadJob) -> Result<String, String> {
    let mut attempt = 0u8;
    let mut last_err = String::new();
    while attempt < HTTP_RETRY_MAX_ATTEMPTS {
        attempt += 1;
        let payload = if job.prompt.is_some() || job.response.is_some() {
            Some(TracePayload {
                prompt: job.prompt.as_deref(),
                response: job.response.as_deref(),
            })
        } else {
            None
        };
        let tu = TraceUpload {
            trace_id: job.trace_id.clone(),
            project_id: &job.project_id,
            ts: job.ts,
            model: &job.model,
            provider: &job.provider,
            tokens_in: job.tokens_in,
            tokens_out: job.tokens_out,
            latency_ms: job.latency_ms,
            cost_usd_micro: job.cost_usd_micro,
            status: &job.status,
            error_class: job.error_class.as_deref(),
            prompt_hash: None,
            response_hash: None,
            memory_refs: None,
            replay_id: job.replay_id.as_deref(),
            payload,
            council_parent_trace_id: job.council_parent_trace_id.as_deref(),
            council_voice_alias: job.council_voice_alias.as_deref(),
            council_voice_position: job.council_voice_position,
            client_sanitizer_passes: None,
        };
        match cloud_client::upload_trace(tu).await {
            Ok(r) => return Ok(r.trace_id),
            Err(e) => {
                last_err = e.to_string();
                // Don't retry on permanent errors (4xx-ish wording from cloud_client)
                if last_err.contains("project_cloud_traces_disabled")
                    || last_err.contains("not signed in")
                    || last_err.contains("unauthorized")
                {
                    return Err(last_err);
                }
                let delay = HTTP_RETRY_BASE_DELAY_MS * (1u64 << (attempt - 1));
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    }
    Err(format!(
        "upload failed after {} attempts: {}",
        HTTP_RETRY_MAX_ATTEMPTS, last_err
    ))
}

/// Snapshot for the frontend status bar.
#[derive(Debug, serde::Serialize)]
pub struct UploaderStatus {
    pub state: u8,
    pub state_label: &'static str,
    pub dropped: u64,
    pub succeeded: u64,
    pub failed: u64,
}

pub fn status_snapshot() -> UploaderStatus {
    let s = UPLOAD_STATE.load(Ordering::Relaxed);
    UploaderStatus {
        state: s,
        state_label: match s {
            0 => "idle",
            1 => "working",
            2 => "paused",
            _ => "unknown",
        },
        dropped: DROPPED_EVENTS.load(Ordering::Relaxed),
        succeeded: SUCCEEDED_UPLOADS.load(Ordering::Relaxed),
        failed: FAILED_UPLOADS.load(Ordering::Relaxed),
    }
}

/// Process-wide singleton handle. Set once at startup; read from anywhere via `take_global()`.
/// This lets services (council_multi, future llm callers) enqueue jobs without threading the
/// handle through the call stack. We use OnceLock for lock-free reads after init.
static GLOBAL_HANDLE: std::sync::OnceLock<UploaderHandle> = std::sync::OnceLock::new();

/// Install the singleton. Called once from lib.rs `create()` site. Idempotent (returns Err if set already).
pub fn install_global(h: UploaderHandle) {
    let _ = GLOBAL_HANDLE.set(h);
}

/// Borrow the singleton handle. Returns a clone if installed, None if not.
pub fn take_global() -> Option<UploaderHandle> {
    GLOBAL_HANDLE.get().cloned()
}

#[allow(dead_code)]
pub fn global_handle() -> Option<Arc<UploaderHandle>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str) -> CloudUploadJob {
        CloudUploadJob {
            trace_id: Some(id.to_string()),
            project_id: "p".into(),
            ts: 0,
            model: "m".into(),
            provider: "pr".into(),
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            cost_usd_micro: None,
            status: "success".into(),
            error_class: None,
            prompt: None,
            response: None,
            replay_id: None,
            local_event_id: None,
            council_parent_trace_id: None,
            council_voice_alias: None,
            council_voice_position: None,
        }
    }

    #[test]
    fn ring_buffer_drops_oldest_when_full() {
        let (h, consumer) = create();
        // Fill to cap + 3 over. Oldest 3 should be evicted; newest CAP survive.
        for i in 0..(UPLOAD_QUEUE_CAP + 3) {
            assert!(h.try_enqueue(job(&format!("j{i}"))));
        }
        let q = consumer.queue.lock();
        assert_eq!(
            q.len(),
            UPLOAD_QUEUE_CAP,
            "buffer capped at UPLOAD_QUEUE_CAP"
        );
        // Front is the 4th item (j3) — j0,j1,j2 were dropped (oldest-first).
        assert_eq!(q.front().unwrap().trace_id.as_deref(), Some("j3"));
        // Back is the newest.
        assert_eq!(
            q.back().unwrap().trace_id.as_deref(),
            Some(&format!("j{}", UPLOAD_QUEUE_CAP + 2)[..])
        );
    }
}
