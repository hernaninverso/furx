// services/mobile_bridge.rs — WebSocket TLS bridge para companion iOS/Android.
// BLOQUE 7 scaffolding. Implementación completa: BLOQUE 8 (post-iOS testflight).
//
// Design:
//   - Desktop levanta WS server en 127.0.0.1:43118 (loopback) + opcional :43119 sobre Tailscale
//     interface si está disponible. NEVER bind 0.0.0.0.
//   - HMAC SHA256 per-message con shared secret en Keychain (`furx-mobile-secret`).
//   - Mensajes JSON: { type: "ping"|"pane.snapshot"|"pty.write"|"approve.tool_call"|"voice.text", ... }
//   - replay-protection: nonce + ts skew window 60s (mismo esquema que telegram_inbound).
//   - BLOQUE J ext 2 (council): mDNS advertising via `mdns-sd` so the future
//     companion can find this desktop without manual IP/port entry. Service
//     type `_furx._tcp.local.`, instance name `Furx-<short_hostname>`, port
//     MOBILE_BRIDGE_PORT. Started lazily via `start_mdns_advertise()`; safe
//     to call before the WS server itself exists (advertising the port even
//     while empty is fine — companion will retry connect with backoff).

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, State,
    },
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Router,
};
use lru::LruCache;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle; // used by MobileBridge::start (production entry point)
use tokio::sync::broadcast;

pub const MOBILE_BRIDGE_PORT: u16 = 43118;
/// Tailscale interface bind port (opt-in, F2). Loopback uses MOBILE_BRIDGE_PORT.
pub const MOBILE_BRIDGE_TAILSCALE_PORT: u16 = 43119;
/// HMAC freshness window. Mirrors telegram_inbound's spirit but tighter (60s)
/// per the mobile spec NFR-3.
const MAX_SKEW_SECS: i64 = 60;
/// Nonce LRU dedup capacity (replay protection within the freshness window).
const NONCE_CACHE_SIZE: usize = 8192;

// ───────────────────────── Notification bus (F3) ─────────────────────────────
// Desktop → phone push. Sources (cards, Grafana webhook, audit) publish a
// `NotifEvent` to a global broadcast; each connected phone subscribes and
// forwards it as a `Notification` frame IF that source's toggle is on
// (`mobile.notify.{kind}` setting). pane-ready is detected per-connection in
// the snapshot ticker (it polls pane state already), so it doesn't use the bus.
//
// MC-8/F-IV: notifications carry NO pane content — title/body are short labels
// the user taps through to see on the desktop.

/// A desktop event worth surfacing on the phone. `kind` maps to a toggle.
#[derive(Debug, Clone)]
pub struct NotifEvent {
    pub kind: String, // "card" | "grafana" | "audit"
    pub title: String,
    pub body: String,
    pub severity: String, // "info" | "warning" | "critical"
    pub correlation_id: Option<String>,
}

/// Global broadcast bus. Capacity 256; lagging slow consumers drop oldest
/// (RecvError::Lagged) — acceptable for notifications. `send` errors when there
/// are no subscribers (no phone connected) and is ignored.
static NOTIFY_BUS: Lazy<broadcast::Sender<NotifEvent>> = Lazy::new(|| broadcast::channel(256).0);

/// Publish a notification to any connected phones. Fire-and-forget; safe to call
/// from anywhere (card creation, Grafana webhook, audit). Truncates title/body
/// defensively so a runaway producer can't push huge frames.
pub fn publish_notification(
    kind: &str,
    title: &str,
    body: &str,
    severity: &str,
    correlation_id: Option<String>,
) {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let _ = NOTIFY_BUS.send(NotifEvent {
        kind: kind.to_string(),
        title: clip(title, 120),
        body: clip(body, 240),
        severity: severity.to_string(),
        correlation_id,
    });
}

/// Default-on/off per source (MC-7 council verdict). cards/grafana/pane_ready
/// ON; audit opt-in (OFF).
fn notify_default(kind: &str) -> bool {
    matches!(kind, "card" | "grafana" | "pane_ready")
}

// ───────────────────────── 017 mobile-companion reform ───────────────────────
//
// Bottom-nav reform: the bridge transports three SSOT primitives to the phone —
// NavSpec (curated domains), CommandCatalog (registry projection), AppEvent
// (typed events with kernel seq). All three are SIGNED server→client
// (defense-in-depth, council #4). Execution from the phone goes through a SIGNED
// `ExecuteCommand` frame (T060) that re-checks authorization against the Rust
// registry at EXEC TIME (T061) — `visibility` is a DISPLAY filter, NOT authz.

/// Protocol version advertised in HelloAck. Mismatch → the PWA degrades to the
/// flat session view (FR-016). Bump on any wire-incompatible change.
pub const MOBILE_PROTOCOL_VERSION: u32 = 1;

/// 8-byte HMAC tags for the SIGNED server→client frames (017). Disjoint from the
/// client→server tags (PtyWrite/HelloMsg/Subscrib/ApprovTC/ExecCmd_) so a
/// signature can never be cross-replayed across directions/types.
const TAG_NAVSPEC: &[u8; 8] = b"NavSpec_";
const TAG_CMDCATALOG: &[u8; 8] = b"CmdCatlg";
const TAG_APPEVENT: &[u8; 8] = b"AppEvnt_";

/// Max size (bytes) of a NavSpec the desktop host may set. A runaway/forged spec
/// can't blow up the frame. 64KB matches WS_MAX_MSG_BYTES.
const MAX_NAVSPEC_BYTES: usize = 64 * 1024;

/// Per-AppEvent payload field cap (T066, F-IV): run names / ids / states are
/// short; anything longer is truncated before it leaves the desktop.
const APPEVENT_FIELD_CAP: usize = 200;

/// Min seconds between two ExecuteCommand for the SAME command_id on one
/// connection (T068 rate-limit). Cheap abuse/loop guard.
const EXEC_RATE_LIMIT_SECS: i64 = 5;

/// The NavSpec the desktop host pushed via `mobile_bridge_set_navspec`. Global
/// (one desktop host) + validated against the registry allowlist before storing
/// (T062). `None` until the host sets it; the phone then gets nav only after.
/// SOLE writer is the Tauri command (desktop host) — the mobile socket can NEVER
/// set it (it's not reachable from `handle_client_frame`).
static NAVSPEC: Lazy<Mutex<Option<serde_json::Value>>> = Lazy::new(|| Mutex::new(None));

/// A command projected to the phone. Mirror of the TS `MobileCommand`: the public
/// subset of a CommandDef. NO secrets, NO `extra`, NO deeplink.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileCommand {
    pub id: String,
    pub label: String,
    pub category: String,
    pub risk: String, // "safe" | "destructive" | "credential" | "external"
}

/// True if a command is hidden from the mobile companion regardless of its
/// `visibility` (T065 deny-list). Infra/SSH/VPN can't be meaningfully driven
/// from a phone and shouldn't be reachable. Enforced in BOTH the catalog
/// projection AND the exec gate (`authorize_mobile_command`).
fn mobile_denied(cmd: &crate::services::command_registry::CommandDef) -> bool {
    // Category-based deny-list (infra/ssh/vpn) + any command tagged
    // `mobile_visible:false` in its `extra` bag (future-proof opt-out).
    let cat = cmd.category.as_str();
    if matches!(cat, "ssh" | "vpn" | "infra" | "tmux") {
        return true;
    }
    if cmd.id.starts_with("ssh_") || cmd.id.starts_with("vpn_") {
        return true;
    }
    cmd.extra
        .get("mobile_visible")
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false)
}

/// True if this command may even be CONSIDERED for the mobile companion: visible
/// (not internal/hidden) AND not on the deny-list. DISPLAY filter only — the
/// exec gate re-derives this independently (never trusts the client).
fn mobile_eligible(cmd: &crate::services::command_registry::CommandDef) -> bool {
    use crate::services::command_registry::Visibility;
    matches!(cmd.visibility, Visibility::Primary | Visibility::Palette) && !mobile_denied(cmd)
}

/// Project the registry to the mobile catalog (FR-005/FR-008/T065). Filtered by
/// visibility + deny-list. Source of truth for what the phone may LIST.
fn mobile_command_catalog() -> Vec<MobileCommand> {
    use crate::services::command_registry::Risk;
    crate::services::command_registry::registry()
        .into_iter()
        .filter(mobile_eligible)
        .map(|c| MobileCommand {
            id: c.id,
            label: c.label,
            category: c.category,
            risk: match c.risk {
                Risk::Safe => "safe",
                Risk::Destructive => "destructive",
                Risk::Credential => "credential",
                Risk::External => "external",
            }
            .to_string(),
        })
        .collect()
}

/// 017 [T062] — validate a NavSpec the desktop host wants to set, against the
/// registry-backed allowlist of domain ids + the static view-id universe. Rejects
/// unknown domain ids (a compromised/stale frontend can't inject arbitrary nav
/// across the trust boundary). Returns the cleaned spec (the SAME shape) or Err.
fn validate_navspec(spec: &serde_json::Value) -> Result<(), String> {
    // Allowed domain ids = the 6 kernel domains (static; the TS SSOT can only ever
    // emit a subset of these). Keep in sync with SidebarGroupId.
    const ALLOWED_DOMAINS: &[&str] = &[
        "work",
        "intelligence",
        "observability",
        "infra",
        "extensions",
        "system",
    ];
    // 017 [SF-1] — view-id universe (mirror of the TS `VIEWS` SSOT in router.ts). The
    // comment promised a "static view-id universe"; enforce it so a stale/compromised
    // host can't inject an arbitrary view id, and cap field lengths (belt vs bloat/abuse).
    const ALLOWED_VIEWS: &[&str] = &[
        "panes",
        "incidents",
        "monitors",
        "audit",
        "settings",
        "saas",
        "health",
        "heatmap",
        "grafana",
        "ssh",
        "vpn",
        "latency",
        "search",
        "eval",
        "queue",
        "router",
        "replay",
        "tools",
        "memory",
        "plugins",
        "crashlog",
        "github",
    ];
    const NAV_FIELD_CAP: usize = 64; // max chars for label/view/icon
    let obj = spec.as_object().ok_or("navspec must be an object")?;
    // version must be a number.
    obj.get("version")
        .and_then(|v| v.as_u64())
        .ok_or("navspec.version missing/invalid")?;
    let domains = obj
        .get("domains")
        .and_then(|d| d.as_array())
        .ok_or("navspec.domains must be an array")?;
    if domains.len() > 16 {
        return Err("navspec.domains too large".into());
    }
    for d in domains {
        let did = d
            .get("domainId")
            .and_then(|v| v.as_str())
            .ok_or("domain.domainId missing")?;
        if !ALLOWED_DOMAINS.contains(&did) {
            return Err(format!("unknown domain id: {did}"));
        }
        // label must be a string (length-capped); items must be a (bounded) array.
        let dlabel = d
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or("domain.label missing")?;
        if dlabel.chars().count() > NAV_FIELD_CAP {
            return Err("domain.label too long".into());
        }
        let items = d
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or("domain.items must be an array")?;
        if items.len() > 64 {
            return Err("domain.items too large".into());
        }
        for it in items {
            // [SF-1] view must be a KNOWN view id, not just any string.
            let view = it
                .get("view")
                .and_then(|v| v.as_str())
                .ok_or("item.view missing")?;
            if !ALLOWED_VIEWS.contains(&view) {
                return Err(format!("unknown view id: {view}"));
            }
            let ilabel = it
                .get("label")
                .and_then(|v| v.as_str())
                .ok_or("item.label missing")?;
            if ilabel.chars().count() > NAV_FIELD_CAP {
                return Err("item.label too long".into());
            }
            // icon is optional; if present it must be a bounded string.
            if let Some(icon) = it.get("icon") {
                let icon = icon.as_str().ok_or("item.icon must be a string")?;
                if icon.chars().count() > NAV_FIELD_CAP {
                    return Err("item.icon too long".into());
                }
            }
        }
    }
    Ok(())
}

/// 017 [T010/T062] — Tauri command: the DESKTOP HOST registers the materialized
/// NavSpec (from `buildNavSpec()` in TS) at bridge init. Validated against the
/// allowlist before storing; the bridge then forwards it SIGNED to the phone.
/// The mobile socket can never call this (it's a Tauri IPC command, host-only).
#[tauri::command]
pub fn mobile_bridge_set_navspec(spec: serde_json::Value) -> Result<(), String> {
    let bytes = serde_json::to_string(&spec).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_NAVSPEC_BYTES {
        return Err("navspec too large".into());
    }
    validate_navspec(&spec)?;
    *NAVSPEC.lock() = Some(spec);
    Ok(())
}

/// Truncate every string field of an AppEvent's `data` object to APPEVENT_FIELD_CAP
/// (T066, F-IV). The kernel's AppEvent today only carries short ids/states, but
/// this is the belt: run names / paths / error text that future variants might
/// add never leak full-length to the phone.
fn redact_app_event(event: &serde_json::Value) -> serde_json::Value {
    let mut ev = event.clone();
    if let Some(data) = ev.get_mut("data") {
        redact_value(data, false);
    }
    ev
}

/// 017 [SF-2] — substrings that mark a field name as sensitive: its string value
/// is replaced wholesale (not just truncated) before leaving the desktop (F-IV).
const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "apikey",
    "api_key",
    "credential",
    "cookie",
];

/// Recursively redact an AppEvent `data` subtree: values under a sensitive-named
/// key become `[redacted]`; every other string is truncated to APPEVENT_FIELD_CAP.
/// Walks nested objects/arrays so a future deep payload can't smuggle long/secret
/// text past the top level (the old impl only handled top-level strings).
fn redact_value(v: &mut serde_json::Value, key_is_sensitive: bool) {
    match v {
        serde_json::Value::String(s) => {
            if key_is_sensitive {
                *v = serde_json::Value::String("[redacted]".to_string());
            } else if s.chars().count() > APPEVENT_FIELD_CAP {
                *s = s.chars().take(APPEVENT_FIELD_CAP).collect();
            }
        }
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                let kl = k.to_ascii_lowercase();
                let sensitive = SENSITIVE_KEY_MARKERS.iter().any(|m| kl.contains(m));
                redact_value(val, sensitive);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                redact_value(val, key_is_sensitive);
            }
        }
        _ => {}
    }
}
pub const KEYCHAIN_SVC_MOBILE: &str = "furx-mobile";
pub const KEYCHAIN_ACCT_SECRET: &str = "shared-secret";

const MDNS_SERVICE_TYPE: &str = "_furx._tcp.local.";
const MDNS_INSTANCE_PREFIX: &str = "Furx";

/// Handle to a running mDNS advertiser. Drop to stop advertising. Stored on
/// AppState so the daemon survives the entire app lifetime.
pub struct MdnsAdvertiser {
    /// mDNS daemon handle; dropped on app shutdown via the Drop impl below.
    daemon: mdns_sd::ServiceDaemon,
    fullname: String,
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        // Audit Llama MED: Drop runs on the main thread during app shutdown
        // sequence; daemon.unregister + shutdown are network ops that can
        // briefly block. Move both to a detached background thread so the
        // app exit isn't held up. Both methods are best-effort (return
        // Result we don't care about), so a runaway thread is harmless —
        // the process is dying anyway.
        let daemon = self.daemon.clone();
        let fullname = self.fullname.clone();
        std::thread::spawn(move || {
            let _ = daemon.unregister(&fullname);
            let _ = daemon.shutdown();
        });
    }
}

/// Spin up an mDNS-SD advertiser for the (Furx) mobile bridge. Idempotent in
/// the sense that calling it twice produces two daemons (the second will see
/// the first's broadcasts and de-conflict by serial suffix), but callers
/// should keep a single instance on AppState.
///
/// Returns Err if mdns-sd can't start (bind failed) — non-fatal: the rest
/// of the bridge keeps working, the user just has to enter the IP manually
/// in the companion.
pub fn start_mdns_advertise() -> Result<MdnsAdvertiser> {
    let daemon =
        mdns_sd::ServiceDaemon::new().map_err(|e| anyhow::anyhow!("mdns daemon: {}", e))?;

    // Audit Llama HIGH: sanitisation that produced "---" (all dashes from
    // emoji-only hostname) or empty would yield an invalid mDNS label.
    // Two-phase clean: replace illegal chars with '-', strip leading/trailing
    // dashes, fall back to "furx-host" if nothing valid remains.
    let hostname_raw = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_default();
    let cleaned: String = hostname_raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    let host_label = if trimmed.is_empty() {
        "furx-host".to_string()
    } else {
        trimmed
    };

    let instance_name = format!("{}-{}", MDNS_INSTANCE_PREFIX, host_label);
    let host_fqdn = format!("{}.local.", host_label);

    // Audit 3/3 HIGH: enable_addr_auto() previously bound to ALL interfaces
    // including VPN / virtual / Tailscale. mobile_bridge.txt advertises
    // loopback_only=true so the consumer knows to connect via local network
    // only; bind explicitly to 127.0.0.1 to match.
    let info = mdns_sd::ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &instance_name,
        &host_fqdn,
        "127.0.0.1",
        MOBILE_BRIDGE_PORT,
        &[
            ("version", env!("CARGO_PKG_VERSION")),
            ("loopback_only", "true"),
        ][..],
    )
    .map_err(|e| anyhow::anyhow!("mdns service info: {}", e))?;

    let fullname = format!("{}.{}", instance_name, MDNS_SERVICE_TYPE);
    daemon
        .register(info)
        .map_err(|e| anyhow::anyhow!("mdns register: {}", e))?;
    tracing::info!(
        "mDNS: advertising {} on 127.0.0.1:{}",
        fullname,
        MOBILE_BRIDGE_PORT
    );
    Ok(MdnsAdvertiser { daemon, fullname })
}

/// Pane metadata sent in `HelloAck` so the companion can list panes without a
/// snapshot round-trip. `state` is the FSM label (idle|busy|ready|error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneMeta {
    pub pane_id: String,
    pub state: String,
}

/// Open card sent in `HelloAck` so the phone can show an approve list. The
/// `correlation_id` in `ApproveToolCall` IS this card `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardMeta {
    pub id: String,
    pub title: String,
    pub severity: String,
    pub project: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobileMessage {
    // ── client → server (signed where they cause side-effects) ──────────────
    /// First frame after WS upgrade. Signed: proves the client holds the shared
    /// secret before the server does anything else. `client_id` is a free label.
    Hello {
        client_id: String,
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=HelloMsg || nonce || ts || client_id)
    },
    /// Switch which pane the server pushes `PaneSnapshot` for. Signed.
    Subscribe {
        pane_id: String,
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=Subscrib || nonce || ts || pane_id)
    },
    Ping {
        nonce: String,
        ts: i64,
    },
    PtyWrite {
        pane_id: String,
        text: String,
        source: String, // "voice" | "manual" | "approve"
        nonce: String,
        ts: i64,
        sig: String, // HMAC SHA256 of (nonce || ts || pane_id || text)
    },
    ApproveToolCall {
        correlation_id: String,
        decision: String, // "approve" | "reject" | "snooze"
        nonce: String,
        ts: i64,
        sig: String,
    },
    // 017 — request the command catalog (server replies with a signed CommandCatalog).
    // Unsigned: it's a pure read with no side effects; the reply is what's signed.
    GetCommands {
        nonce: String,
        ts: i64,
    },
    // 017 [T060] — execute a registry command by id ref (NO free-form args). FIRMED:
    // tag `ExecCmd_`, command_id in the `scope` slot of the canonical encoding
    // (source/body empty), same nonce/skew/replay protections as PtyWrite.
    ExecuteCommand {
        command_id: String,
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=ExecCmd_ || nonce || ts || command_id)
    },

    // ── server → client (unsigned: integrity comes from loopback/Tailscale
    //    transport — MC-2 council verdict; only the desktop can be the server) ─
    /// Reply to a valid `Hello`: lists live panes + open cards + server version.
    /// 017: `protocol_version` lets the PWA feature-detect; a stale PWA degrades
    /// to the flat session view on mismatch (FR-016).
    HelloAck {
        panes: Vec<PaneMeta>,
        cards: Vec<CardMeta>,
        server_version: String,
        protocol_version: u32,
        ts: i64,
    },
    Pong {
        nonce: String,
        ts: i64,
    },
    PaneSnapshot {
        pane_id: String,
        last_lines: Vec<String>,
        state: String,
        ts: i64,
    },
    /// Push notification (cards / Grafana / pane-ready). Carries NO pane content
    /// (F-IV privacy): the user taps through on the desktop to see detail.
    Notification {
        kind: String, // "card" | "grafana" | "pane_ready" | "audit"
        title: String,
        body: String,
        severity: String, // "info" | "warning" | "critical"
        correlation_id: Option<String>,
        ts: i64,
    },
    /// Server-side protocol error sent before closing the socket.
    BridgeError {
        message: String,
    },

    // ── 017 server → client, SIGNED (defense-in-depth, council #4) ────────────
    // These three carry a server-side HMAC tag + nonce + ts so a MITM (e.g. a
    // compromised Tailscale exit node) can't forge/replay them. The client
    // verifies the sig before applying. The `body` payload is JSON-serialized
    // before signing so its exact bytes are authenticated.
    //
    /// Curated bottom-nav domains derived from `navGroups` (FR-001). `spec` is the
    /// `MobileNavSpec` JSON; only public labels/ids — never dynamic/sensitive data.
    NavSpec {
        spec: serde_json::Value,
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=NavSpec_ || nonce || ts || <spec json>)
    },
    /// Projection of `command_registry` filtered by visibility + mobile deny-list
    /// (FR-005, T065). Each entry: {id,label,category,risk}. No metadata beyond that.
    CommandCatalog {
        commands: serde_json::Value, // array of MobileCommand
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=CmdCatlg || nonce || ts || <commands json>)
    },
    /// Typed kernel `AppEvent` with the SAME `{seq, ts}` envelope as the desktop bus
    /// (FR-009/FR-010). Payload is redacted + size-capped (T066, F-IV).
    AppEvent {
        event: serde_json::Value, // { tag, data }
        seq: u64,
        event_ts: i64,
        nonce: String,
        ts: i64,
        sig: String, // HMAC over (tag=AppEvnt_ || nonce || ts || <event json + seq>)
    },

    // ── 065 QR-pairing: pre-Hello (sin HMAC — el companion todavía NO tiene el secreto) ──
    /// Companion → bridge: canjea un token efímero del QR por el secreto permanente. Interceptado
    /// ANTES del Hello en `handle_socket`; nunca llega al path firmado.
    PairingRedeem {
        token: String,       // 64 hex — token efímero del QR
        device_id: String,   // UUID de localStorage (primer lanzamiento del companion)
        device_name: String, // label de UX ("iPhone"/"Android"/…)
    },
    /// Bridge → companion: entrega el secreto permanente tras un canje válido.
    PairingGrant {
        secret: String,
        session_id: String,
    },
}

#[allow(dead_code)]
/// HMAC SHA-256 verify of a signed mobile message.
///
/// BLOQUE J ext (council 5/5 must-fix → audit 3/3 MED follow-up): the previous
/// stub returned `Ok(true)` unconditionally — a fail-open path. The first
/// fix used a `.` separator, which the 3 auditors all flagged as ambiguous:
/// `pane_id="p.a"`, `text="b"` collides with `pane_id="p"`, `text="a.b"`.
/// Now: length-prefixed framing — each field is emitted as `[8 bytes LE
/// u64 length][raw bytes]`, plus a 1-byte type tag at the front. Unambiguous
/// for any byte payload, no escaping needed.
///
/// Returns:
///   - `Ok(true)`  → signature matches
///   - `Ok(false)` → message rejected (wrong sig, missing sig, malformed sig,
///                   or unsigned message type — Llama audit MED: callers
///                   should get a boolean, not an error, for "not authentic").
///   - `Err(...)`  → internal failure (HMAC key invalid).
pub fn verify_hmac(secret: &[u8], message: &MobileMessage) -> Result<bool> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use subtle::ConstantTimeEq;

    let (tag, nonce, ts, scope, source, body, sig_hex) = match message {
        MobileMessage::PtyWrite {
            nonce,
            ts,
            pane_id,
            text,
            source,
            sig,
        } => (
            b"PtyWrite",
            nonce.as_str(),
            *ts,
            pane_id.as_str(),
            source.as_str(),
            text.as_str(),
            sig.as_str(),
        ),
        MobileMessage::ApproveToolCall {
            nonce,
            ts,
            correlation_id,
            decision,
            sig,
        } => {
            // Codex ultra-review: was "Approvel" (typo); fixed to "ApprovTC" (8 bytes,
            // unambiguous, distinct from "PtyWrite"). Any consumer already in the wild
            // verifying the old tag must re-sign — there are none yet (BLOQUE 7+ work).
            (
                b"ApprovTC",
                nonce.as_str(),
                *ts,
                correlation_id.as_str(),
                "",
                decision.as_str(),
                sig.as_str(),
            )
        }
        // 004 mobile-companion: handshake + subscribe are signed client→server
        // frames. Distinct 8-byte tags keep their canonical encodings disjoint
        // from PtyWrite/ApprovTC so a signature can't be cross-replayed.
        MobileMessage::Hello {
            nonce,
            ts,
            client_id,
            sig,
        } => (
            b"HelloMsg",
            nonce.as_str(),
            *ts,
            client_id.as_str(),
            "",
            "",
            sig.as_str(),
        ),
        MobileMessage::Subscribe {
            nonce,
            ts,
            pane_id,
            sig,
        } => (
            b"Subscrib",
            nonce.as_str(),
            *ts,
            pane_id.as_str(),
            "",
            "",
            sig.as_str(),
        ),
        // 017 [T060] — ExecuteCommand: command_id ref in the `scope` slot, no args.
        // Distinct 8-byte tag keeps it disjoint from PtyWrite/Subscrib/etc.
        MobileMessage::ExecuteCommand {
            nonce,
            ts,
            command_id,
            sig,
        } => (
            b"ExecCmd_",
            nonce.as_str(),
            *ts,
            command_id.as_str(),
            "",
            "",
            sig.as_str(),
        ),
        // Unsigned client frames: liveness Ping + GetCommands (pure read). And all
        // server→client messages — the SIGNED ones (NavSpec/CommandCatalog/AppEvent)
        // are produced+signed by the server via `sign_outbound`, never verified here.
        // Audit Llama MED: return Ok(false) instead of Err so callers can treat
        // verification as a pure pass/fail boolean.
        MobileMessage::Ping { .. }
        | MobileMessage::GetCommands { .. }
        | MobileMessage::PaneSnapshot { .. }
        | MobileMessage::HelloAck { .. }
        | MobileMessage::Pong { .. }
        | MobileMessage::Notification { .. }
        | MobileMessage::BridgeError { .. }
        | MobileMessage::NavSpec { .. }
        | MobileMessage::CommandCatalog { .. }
        | MobileMessage::AppEvent { .. }
        // 065 — pairing pre-Hello: nunca pasan por verify_hmac (se interceptan en handle_socket),
        // pero el match debe ser exhaustivo. Si llegaran acá, se rechazan como no-firmados.
        | MobileMessage::PairingRedeem { .. }
        | MobileMessage::PairingGrant { .. } => {
            return Ok(false);
        }
    };
    if sig_hex.is_empty() || sig_hex.len() > 256 {
        return Ok(false);
    }
    let canonical = canonical_bytes(tag, nonce, ts, scope, source, body);
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| anyhow::anyhow!("hmac key: {}", e))?;
    mac.update(&canonical);
    let expected = mac.finalize().into_bytes();
    let provided = match hex::decode(sig_hex) {
        Ok(b) => b,
        Err(_) => return Ok(false),
    };
    if provided.len() != expected.len() {
        return Ok(false);
    }
    Ok(provided.ct_eq(&expected).into())
}

/// Length-prefixed canonical encoding (audit 3/3 MED — was `.`-separated and
/// ambiguous). Format: `tag(8) | len(8) field | len(8) field | ...` where
/// every length is little-endian u64 over the raw UTF-8 bytes of the field.
#[allow(dead_code)]
fn canonical_bytes(
    tag: &[u8; 8],
    nonce: &str,
    ts: i64,
    scope: &str,
    source: &str,
    body: &str,
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(8 + 6 * 8 + nonce.len() + scope.len() + source.len() + body.len() + 16);
    out.extend_from_slice(tag);
    let push = |out: &mut Vec<u8>, b: &[u8]| {
        out.extend_from_slice(&(b.len() as u64).to_le_bytes());
        out.extend_from_slice(b);
    };
    push(&mut out, nonce.as_bytes());
    let ts_bytes = ts.to_le_bytes();
    out.extend_from_slice(&(ts_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&ts_bytes);
    push(&mut out, scope.as_bytes());
    push(&mut out, source.as_bytes());
    push(&mut out, body.as_bytes());
    out
}

/// 017 [T063] — HMAC the canonical bytes for a SIGNED server→client frame. Same
/// length-prefixed encoding + 8-byte tag as the client frames, but produced by
/// the server. The `body` is the JSON of the payload so its exact bytes are
/// authenticated; the phone recomputes the same canonical and compares.
fn sign_outbound(secret: &[u8], tag: &[u8; 8], nonce: &str, ts: i64, body: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let canonical = canonical_bytes(tag, nonce, ts, body, "", "");
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(&canonical);
    hex::encode(mac.finalize().into_bytes())
}

/// Fresh per-frame nonce for outbound signed frames (server side). Time + random
/// (UUID v4), unique within the freshness window — mirrors furx-sign.js makeNonce().
fn outbound_nonce() -> String {
    format!("s-{}-{}", now_secs(), uuid::Uuid::new_v4().simple())
}

/// Generate/load shared secret (32 bytes hex, stored in Keychain).
/// Called by the desktop to display in Settings → Mobile so the user can paste it into the
/// companion app on first pairing.
#[allow(dead_code)]
pub fn ensure_secret() -> Result<String> {
    use crate::services::keychain;
    if let Some(s) = keychain::load(KEYCHAIN_SVC_MOBILE, KEYCHAIN_ACCT_SECRET) {
        if s.len() == 64 {
            return Ok(s);
        }
    }
    // Generate new 32-byte secret as hex
    use uuid::Uuid;
    let new_secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    keychain::save(KEYCHAIN_SVC_MOBILE, KEYCHAIN_ACCT_SECRET, &new_secret)?;
    Ok(new_secret)
}

/// Rotate the pairing secret: delete the Keychain entry and generate a fresh
/// one. Returns the new 64-hex secret. NOTE: a running bridge keeps the OLD
/// secret in memory until the app restarts — paired phones must re-pair AND the
/// user must restart Furx for the new secret to take effect. The Settings UI
/// surfaces this. (Live rotation would need a shared mutable secret handle;
/// deferred as rotation is rare.)
#[allow(dead_code)]
pub fn rotate_secret() -> Result<String> {
    use crate::services::keychain;
    // 065 — invalida los QR/short_codes pendientes (conserva grants en vuelo <30s, que cargarán el
    // secreto NUEVO en el retry — comportamiento correcto: la rotación invalida el viejo).
    crate::services::mobile_qr_pairing::clear_pending_sessions();
    keychain::delete(KEYCHAIN_SVC_MOBILE, KEYCHAIN_ACCT_SECRET);
    ensure_secret()
}

// ─────────────────────── 017 signed-frame builders + authz ───────────────────

/// Build a SIGNED `NavSpec` frame from the host-set spec (or `None` if the host
/// hasn't registered one yet). The spec is already validated (T062) at set time.
fn build_navspec_frame(secret: &[u8]) -> Option<MobileMessage> {
    let spec = NAVSPEC.lock().clone()?;
    let body = serde_json::to_string(&spec).ok()?;
    let nonce = outbound_nonce();
    let ts = now_secs();
    let sig = sign_outbound(secret, TAG_NAVSPEC, &nonce, ts, &body);
    Some(MobileMessage::NavSpec {
        spec,
        nonce,
        ts,
        sig,
    })
}

/// Build a SIGNED `CommandCatalog` frame (projection filtered by visibility +
/// deny-list, T065). Computed fresh each time — the registry is static.
fn build_catalog_frame(secret: &[u8]) -> MobileMessage {
    let cmds = mobile_command_catalog();
    let commands = serde_json::to_value(&cmds).unwrap_or(serde_json::Value::Array(vec![]));
    let body = serde_json::to_string(&commands).unwrap_or_else(|_| "[]".into());
    let nonce = outbound_nonce();
    let ts = now_secs();
    let sig = sign_outbound(secret, TAG_CMDCATALOG, &nonce, ts, &body);
    MobileMessage::CommandCatalog {
        commands,
        nonce,
        ts,
        sig,
    }
}

/// Build a SIGNED `AppEvent` frame from a kernel `EventEnvelope`. The payload is
/// REDACTED + size-capped (T066) before signing; the kernel `seq` is preserved so
/// the phone applies the SAME monotonic order as the desktop windows (FR-010).
fn build_app_event_frame(
    secret: &[u8],
    env: &crate::services::event_bus::EventEnvelope,
) -> Option<MobileMessage> {
    let raw = serde_json::to_value(&env.payload).ok()?;
    let event = redact_app_event(&raw);
    let body = serde_json::to_string(&event).ok()?;
    // The seq is part of the authenticated body (so it can't be re-numbered by a MITM).
    let signed_body = format!("{}|{}", env.seq, body);
    let nonce = outbound_nonce();
    let ts = now_secs();
    let sig = sign_outbound(secret, TAG_APPEVENT, &nonce, ts, &signed_body);
    Some(MobileMessage::AppEvent {
        event,
        seq: env.seq,
        event_ts: env.ts,
        nonce,
        ts,
        sig,
    })
}

/// 017 [T061] — AUTHORIZATION at EXEC TIME for a command requested from the phone.
/// `visibility` is a DISPLAY filter, NOT authz — a forged/stale mobile client could
/// ship ANY id, so we re-derive everything from the Rust registry SSOT here:
///   - unknown id / internal/hidden / deny-listed  → REJECTED (never reachable).
///   - Destructive / Credential / requires_confirmation → PENDING approval (kernel
///     gate): we create a pending approval + signal `ApprovalRequested`. NOT executed.
///   - Safe/External (eligible)                     → ALLOWED to dispatch.
enum MobileExecAuthz {
    /// Safe to dispatch the command id (no privileged side-effect path on mobile).
    Allowed,
    /// Queued for human approval; carries the request_id to emit ApprovalRequested.
    Pending {
        request_id: String,
        command_id: String,
    },
    /// Rejected with a reason (unknown / not eligible / denied).
    Rejected(String),
}

fn authorize_mobile_command(state: &BridgeState, command_id: &str) -> MobileExecAuthz {
    use crate::services::{capability, command_registry};
    // 1. Look up the command in the registry SSOT. Unknown → reject (fail-closed).
    let def = match command_registry::registry()
        .into_iter()
        .find(|c| c.id == command_id)
    {
        Some(d) => d,
        None => return MobileExecAuthz::Rejected("unknown command".into()),
    };
    // 2. Re-derive mobile eligibility server-side (DISPLAY filter is NOT authz).
    //    internal/hidden or deny-listed → reject even if the client asked for it.
    if !mobile_eligible(&def) {
        return MobileExecAuthz::Rejected("command not available on mobile".into());
    }
    // 3. Capability gate: Destructive/Credential/requires_confirmation → pending.
    //    Reuse the KERNEL gate (capability::check + create_pending) — same path as
    //    the desktop, no mobile-only shortcut. args_json = "{}" (id ref, no args).
    let chk = capability::check(command_id);
    if chk.requires_approval {
        // create_pending takes &Db (Arc<Mutex<Connection>>) and locks internally —
        // pass the shared handle, NOT a guard.
        match capability::create_pending(&state.db, command_id, "{}") {
            Ok(request_id) => MobileExecAuthz::Pending {
                request_id,
                command_id: command_id.to_string(),
            },
            Err(e) => MobileExecAuthz::Rejected(format!("approval gate error: {e}")),
        }
    } else {
        MobileExecAuthz::Allowed
    }
}

// ───────────────────────────── WebSocket bridge (F1) ─────────────────────────
//
// Council MC-2: WS plaintext + per-message HMAC. Loopback has no network
// exposure; the Tailscale bind (F2) is already WireGuard-encrypted. Side-effect
// frames (PtyWrite / ApproveToolCall / Subscribe / Hello) are HMAC-verified;
// server→client frames are trusted because only the desktop holds the secret
// and the transport is loopback/WireGuard.
//
// Top-risk mitigation (council, unanimous): per frame the order is
//   ts-skew check → HMAC verify → nonce insert
// so a forged frame (bad sig) is rejected BEFORE it can burn a legit nonce.

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn fmt_pane_state(s: crate::bases::state::PaneState) -> String {
    use crate::bases::state::PaneState::*;
    match s {
        Idle => "idle",
        Busy => "busy",
        Ready => "ready",
        Error => "error",
    }
    .to_string()
}

/// Type-erased event emitter. In production this wraps `AppHandle::emit`; in
/// tests it's a no-op/recording closure. Keeping it erased means the bridge
/// (and its WS handlers) are NOT generic over the Tauri runtime, so an in-crate
/// E2E test can drive the real server over TCP without a `AppHandle<Wry>`
/// (which can't be mocked — `tauri::test` only yields `AppHandle<MockRuntime>`).
type EmitFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

#[derive(Clone)]
struct BridgeState {
    secret: Arc<String>,
    pty: Arc<crate::pty::PtyManager>,
    pane_state: crate::bases::state::PaneStateModel,
    db: Arc<Mutex<rusqlite::Connection>>,
    audit: crate::bases::audit::AuditWriter,
    emit: EmitFn,
    nonces: Arc<Mutex<LruCache<String, ()>>>,
}

/// RAII handle for the running bridge. Drop / `shutdown` aborts every listener.
pub struct MobileBridge {
    handles: Vec<tauri::async_runtime::JoinHandle<()>>,
    bound: Vec<SocketAddr>,
}

impl MobileBridge {
    /// Start the bridge on the given bind addresses (loopback always; optionally
    /// a Tailscale IP — see `bridge_bind_addrs`). Refuses any unspecified
    /// (`0.0.0.0` / `::`) address as defense-in-depth (NFR-1).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        app: AppHandle,
        pty: Arc<crate::pty::PtyManager>,
        pane_state: crate::bases::state::PaneStateModel,
        db: Arc<Mutex<rusqlite::Connection>>,
        audit: crate::bases::audit::AuditWriter,
        secret: String,
        bind_addrs: Vec<SocketAddr>,
    ) -> Result<Self> {
        let emit: EmitFn = Arc::new(move |ev: &str, val: serde_json::Value| {
            use tauri::Emitter;
            let _ = app.emit(ev, val);
        });
        let bridge = Self::start_with_emit(pty, pane_state, db, audit, secret, bind_addrs, emit)?;
        // 065 — GC periódico de sesiones de pairing. Solo en el path de producción (start, con runtime
        // tauri); start_with_emit queda libre de spawns para los tests E2E in-crate.
        crate::services::mobile_qr_pairing::spawn_cleanup_task();
        Ok(bridge)
    }

    /// Runtime-agnostic core of `start` (no `AppHandle` → testable in-crate).
    #[allow(clippy::too_many_arguments)]
    fn start_with_emit(
        pty: Arc<crate::pty::PtyManager>,
        pane_state: crate::bases::state::PaneStateModel,
        db: Arc<Mutex<rusqlite::Connection>>,
        audit: crate::bases::audit::AuditWriter,
        secret: String,
        bind_addrs: Vec<SocketAddr>,
        emit: EmitFn,
    ) -> Result<Self> {
        if secret.len() != 64 {
            return Err(anyhow::anyhow!("shared secret must be 64 hex chars"));
        }
        let state = BridgeState {
            secret: Arc::new(secret),
            pty,
            pane_state,
            db,
            audit,
            emit,
            nonces: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(NONCE_CACHE_SIZE).expect("nonce cache size"),
            ))),
        };
        let router = Router::new()
            .route("/ws", get(ws_upgrade))
            // F4: serve the PWA bundle (embedded in the binary). The phone loads
            // http://<tailscale-ip>:43118/ — no app store, no build step.
            .route(
                "/",
                get(|| async { asset("text/html; charset=utf-8", PWA_INDEX) }),
            )
            .route(
                "/index.html",
                get(|| async { asset("text/html; charset=utf-8", PWA_INDEX) }),
            )
            .route(
                "/furx-sign.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_SIGN) }),
            )
            // 017 — bottom-nav modules.
            .route(
                "/protocol.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_PROTOCOL) }),
            )
            .route(
                "/nav.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_NAV) }),
            )
            .route(
                "/commands.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_COMMANDS) }),
            )
            .route(
                "/pairing.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_PAIRING) }),
            )
            .route(
                "/events.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_EVENTS) }),
            )
            .route(
                "/manifest.webmanifest",
                get(|| async { asset("application/manifest+json", PWA_MANIFEST) }),
            )
            .route(
                "/icon.svg",
                get(|| async { asset("image/svg+xml", PWA_ICON) }),
            )
            .route(
                "/sw.js",
                get(|| async { asset("text/javascript; charset=utf-8", PWA_SW) }),
            )
            // F3: Grafana alert webhook (contact point). Bearer-authed with the
            // pairing secret; publishes a notification to connected phones.
            .route("/furx/v1/grafana", post(grafana_webhook))
            // 065 — canje del short_code por token efímero (fallback de tipeo manual del pairing).
            .route("/pair-shortcode", post(shortcode_handler))
            .route("/healthz", get(|| async { "ok" }))
            // Defense in depth: cap request bodies (the Grafana POST). WS frames
            // are capped separately on the upgrade.
            .layer(DefaultBodyLimit::max(WS_MAX_MSG_BYTES))
            .with_state(state);

        let mut handles = Vec::new();
        let mut bound = Vec::new();
        for addr in bind_addrs {
            // NFR-1 hard guard: NEVER bind 0.0.0.0 / ::. The mDNS record also
            // advertises loopback_only; this is the belt to that suspenders.
            if addr.ip().is_unspecified() {
                tracing::error!("mobile_bridge refusing to bind unspecified addr {}", addr);
                continue;
            }
            // Bind synchronously so a bind error surfaces to the caller (and so
            // an ephemeral `:0` port can be learned via `local_addrs()`), then
            // hand the socket to tokio inside the serve task.
            let std_listener = match std::net::TcpListener::bind(addr) {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("mobile_bridge bind {}: {}", addr, e);
                    continue;
                }
            };
            if let Err(e) = std_listener.set_nonblocking(true) {
                tracing::error!("mobile_bridge set_nonblocking {}: {}", addr, e);
                continue;
            }
            let local = std_listener.local_addr().unwrap_or(addr);
            let router = router.clone();
            let h = tauri::async_runtime::spawn(async move {
                let listener = match tokio::net::TcpListener::from_std(std_listener) {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::error!("mobile_bridge from_std {}: {}", local, e);
                        return;
                    }
                };
                tracing::info!("mobile_bridge listening on {}", local);
                // 065 — `into_make_service_with_connect_info` expone la IP del cliente al
                // `shortcode_handler` (rate-limit por IP). El resto de rutas no la usan.
                if let Err(e) = axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await
                {
                    tracing::warn!("mobile_bridge serve {}: {}", local, e);
                }
            });
            handles.push(h);
            bound.push(local);
        }
        if handles.is_empty() {
            return Err(anyhow::anyhow!("mobile_bridge: no valid bind addresses"));
        }
        Ok(Self { handles, bound })
    }

    /// Addresses actually bound (resolves ephemeral `:0` to the chosen port).
    pub fn local_addrs(&self) -> Vec<SocketAddr> {
        self.bound.clone()
    }

    pub fn shutdown(&mut self) {
        for h in self.handles.drain(..) {
            h.abort();
        }
    }
}

impl Drop for MobileBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// F4 — PWA bundle embedded at compile time (ships inside the binary; nothing
// read from disk at runtime). Paths are relative to this source file.
const PWA_INDEX: &str = include_str!("../../../mobile-companion/pwa/index.html");
const PWA_SIGN: &str = include_str!("../../../mobile-companion/pwa/furx-sign.js");
const PWA_MANIFEST: &str = include_str!("../../../mobile-companion/pwa/manifest.webmanifest");
const PWA_ICON: &str = include_str!("../../../mobile-companion/pwa/icon.svg");
const PWA_SW: &str = include_str!("../../../mobile-companion/pwa/sw.js");
// 017 — data-driven bottom-nav modules (imported by index.html). Embedded so the
// in-binary / iOS-wrapped PWA can import them without a build step.
const PWA_PROTOCOL: &str = include_str!("../../../mobile-companion/pwa/protocol.js");
const PWA_NAV: &str = include_str!("../../../mobile-companion/pwa/nav.js");
const PWA_COMMANDS: &str = include_str!("../../../mobile-companion/pwa/commands.js");
// 065 — lógica de pairing por QR / short-code del companion.
const PWA_PAIRING: &str = include_str!("../../../mobile-companion/pwa/pairing.js");
const PWA_EVENTS: &str = include_str!("../../../mobile-companion/pwa/events.js");

fn asset(content_type: &'static str, body: &'static str) -> Response {
    use axum::response::IntoResponse;
    ([(axum::http::header::CONTENT_TYPE, content_type)], body).into_response()
}

fn ct_eq_str(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

fn map_grafana_severity(s: &str) -> &'static str {
    match s.to_ascii_lowercase().as_str() {
        "critical" | "crit" | "page" => "critical",
        "warning" | "warn" => "warning",
        _ => "info",
    }
}

/// F3 — Grafana alerting contact point (`webhook` type). Authed with the pairing
/// secret as a Bearer token (constant-time). Parses the unified-alerting payload
/// leniently and publishes ONE notification to connected phones. NO pane content.
// 065 — fallback de tipeo manual del pairing: el companion POSTea el short_code de 8 chars (Base32 sin
// ambiguos) y recibe el token efímero, que igual requiere `PairingRedeem` completo (existencia+TTL+used).
// Rate-limit 1 req/s por IP — evita flood DoS del endpoint. La entropía del token (32B) hace innecesario
// limitar intentos por token (un diccionario es inviable).
static SHORTCODE_RATE: Lazy<Mutex<std::collections::HashMap<std::net::IpAddr, std::time::Instant>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

#[derive(Deserialize)]
struct ShortcodeReq {
    code: String,
}
#[derive(Serialize)]
struct ShortcodeResp {
    token: String,
}

async fn shortcode_handler(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    axum::Json(body): axum::Json<ShortcodeReq>,
) -> Result<axum::Json<ShortcodeResp>, StatusCode> {
    {
        let mut rl = SHORTCODE_RATE.lock();
        let now = std::time::Instant::now();
        // Purga TTL en CADA request (audit codex+gemini LOW): las entradas >60s ya no aportan; mantiene
        // el mapa acotado por las IPs activas de la última ventana (en una LAN, pocas). Hard-cap
        // defensivo ante IP spoofing masivo: reset total.
        rl.retain(|_, t| now.duration_since(*t) < Duration::from_secs(60));
        if rl.len() > 8192 {
            rl.clear();
        }
        if let Some(&last) = rl.get(&addr.ip()) {
            if now.duration_since(last) < Duration::from_secs(1) {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
        }
        rl.insert(addr.ip(), now);
    }
    let code = body.code.trim().to_uppercase();
    if code.len() != 8
        || !code
            .chars()
            .all(|c| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&(c as u8)))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    match crate::services::mobile_qr_pairing::token_for_short(&code) {
        Some(token) => Ok(axum::Json(ShortcodeResp { token })),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn grafana_webhook(
    State(state): State<BridgeState>,
    headers: HeaderMap,
    body: String,
) -> Result<StatusCode, StatusCode> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", state.secret.as_str());
    if !ct_eq_str(auth, &expected) {
        return Err(StatusCode::FORBIDDEN);
    }
    if body.len() > WS_MAX_MSG_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("firing");
    let title = v
        .get("title")
        .and_then(|s| s.as_str())
        .or_else(|| {
            v.pointer("/alerts/0/labels/alertname")
                .and_then(|s| s.as_str())
        })
        .unwrap_or("Grafana alert");
    let body_txt = v
        .get("message")
        .and_then(|s| s.as_str())
        .or_else(|| {
            v.pointer("/alerts/0/annotations/summary")
                .and_then(|s| s.as_str())
        })
        .unwrap_or(status);
    let sev = v
        .pointer("/alerts/0/labels/severity")
        .and_then(|s| s.as_str())
        .map(map_grafana_severity)
        .unwrap_or(if status == "resolved" {
            "info"
        } else {
            "warning"
        });
    publish_notification("grafana", title, body_txt, sev, None);
    Ok(StatusCode::OK)
}

/// Max accepted WebSocket message/frame size. 4-frontier review (R1/R3/R4
/// converged, MED): without this an unauthenticated peer can send a huge text
/// frame and force the handler to run `serde` + `canonical_bytes` + HMAC over
/// megabytes of field data BEFORE the signature check rejects it (DoS). 64KB is
/// far above any legit command/paste yet bounds the work per frame. tungstenite
/// rejects oversized frames at the protocol layer, before our handler sees them.
const WS_MAX_MSG_BYTES: usize = 64 * 1024;

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<BridgeState>) -> Response {
    ws.max_message_size(WS_MAX_MSG_BYTES)
        .max_frame_size(WS_MAX_MSG_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, state))
}

/// Per-frame freshness + authenticity + replay check, in the council-mandated
/// order. Returns Ok only for a fresh, correctly-signed, non-replayed frame.
fn verify_frame(state: &BridgeState, msg: &MobileMessage) -> Result<(), &'static str> {
    let (nonce, ts): (&str, i64) = match msg {
        MobileMessage::Hello { nonce, ts, .. }
        | MobileMessage::Subscribe { nonce, ts, .. }
        | MobileMessage::PtyWrite { nonce, ts, .. }
        | MobileMessage::ExecuteCommand { nonce, ts, .. }
        | MobileMessage::ApproveToolCall { nonce, ts, .. } => (nonce.as_str(), *ts),
        _ => return Err("unsigned frame"),
    };
    if nonce.is_empty() || nonce.len() > 128 {
        return Err("bad nonce");
    }
    // 1. ts-skew BEFORE HMAC — cheap reject of stale frames.
    if (now_secs() - ts).unsigned_abs() > MAX_SKEW_SECS as u64 {
        return Err("ts skew");
    }
    // 2. HMAC verify BEFORE touching the nonce cache (top-risk: no nonce-burn).
    match verify_hmac(state.secret.as_bytes(), msg) {
        Ok(true) => {}
        Ok(false) => return Err("bad signature"),
        Err(_) => return Err("hmac error"),
    }
    // 3. Replay dedup AFTER the signature is proven valid.
    let mut cache = state.nonces.lock();
    if cache.get(nonce).is_some() {
        return Err("replay");
    }
    cache.put(nonce.to_string(), ());
    Ok(())
}

async fn send_msg(socket: &mut WebSocket, msg: &MobileMessage) -> bool {
    match serde_json::to_string(msg) {
        Ok(s) => socket.send(Message::Text(s)).await.is_ok(),
        Err(_) => false,
    }
}

async fn send_err(socket: &mut WebSocket, message: &str) -> bool {
    send_msg(
        socket,
        &MobileMessage::BridgeError {
            message: message.to_string(),
        },
    )
    .await
}

fn pane_meta_list(state: &BridgeState) -> Vec<PaneMeta> {
    state
        .pty
        .pane_ids()
        .into_iter()
        .map(|id| {
            let st = state
                .pane_state
                .get(&id)
                .map(|r| fmt_pane_state(r.state))
                .unwrap_or_else(|| "idle".to_string());
            PaneMeta {
                pane_id: id,
                state: st,
            }
        })
        .collect()
}

/// Open (undecided) cards for the phone's approve list. Cap 50, newest first.
fn open_cards(state: &BridgeState) -> Vec<CardMeta> {
    let conn = state.db.lock();
    let mut stmt = match conn.prepare(
        "SELECT id, title, severity, project FROM cards WHERE status='open' ORDER BY created_at DESC LIMIT 50",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |r| {
        Ok(CardMeta {
            id: r.get(0)?,
            title: r.get(1)?,
            severity: r.get(2)?,
            project: r.get(3)?,
        })
    });
    match rows {
        Ok(it) => it.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn snapshot_msg(state: &BridgeState, pane_id: &str) -> MobileMessage {
    let lines = state.pty.snapshot(pane_id);
    let st = state
        .pane_state
        .get(pane_id)
        .map(|r| fmt_pane_state(r.state))
        .unwrap_or_else(|| "idle".to_string());
    MobileMessage::PaneSnapshot {
        pane_id: pane_id.to_string(),
        last_lines: lines,
        state: st,
        ts: now_secs(),
    }
}

/// Whether a notification source is enabled (setting `mobile.notify.{kind}`,
/// falling back to the council default).
fn notify_enabled(state: &BridgeState, kind: &str) -> bool {
    let conn = state.db.lock();
    match crate::settings::get(&conn, &format!("mobile.notify.{}", kind)) {
        Ok(Some(v)) => v.as_bool().unwrap_or_else(|| notify_default(kind)),
        _ => notify_default(kind),
    }
}

/// One 2s tick: push the subscribed pane's snapshot, then detect Busy→Ready
/// transitions across ALL panes and push a "Claude waiting for input"
/// notification (if the pane_ready toggle is on). Returns false if the socket
/// closed (caller breaks the loop).
async fn push_tick(
    state: &BridgeState,
    socket: &mut WebSocket,
    subscribed: &Option<String>,
    pane_states: &mut std::collections::HashMap<String, String>,
) -> bool {
    if let Some(pane) = subscribed {
        let snap = snapshot_msg(state, pane);
        if !send_msg(socket, &snap).await {
            return false;
        }
    }
    let pane_ready_on = notify_enabled(state, "pane_ready");
    let mut seen = std::collections::HashSet::new();
    for id in state.pty.pane_ids() {
        seen.insert(id.clone());
        let cur = state
            .pane_state
            .get(&id)
            .map(|r| fmt_pane_state(r.state))
            .unwrap_or_else(|| "idle".to_string());
        let prev = pane_states.insert(id.clone(), cur.clone());
        if pane_ready_on && prev.as_deref() == Some("busy") && cur == "ready" {
            let n = MobileMessage::Notification {
                kind: "pane_ready".to_string(),
                title: "Claude waiting for input".to_string(),
                body: format!("Pane {} is ready", id),
                severity: "info".to_string(),
                correlation_id: Some(id.clone()),
                ts: now_secs(),
            };
            if !send_msg(socket, &n).await {
                return false;
            }
        }
    }
    // Forget panes that no longer exist (avoid unbounded growth).
    pane_states.retain(|k, _| seen.contains(k));
    true
}

async fn handle_socket(mut socket: WebSocket, state: BridgeState) {
    // ── handshake: first frame must be a valid signed Hello within 10s ──
    let first = match tokio::time::timeout(Duration::from_secs(10), socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => t,
        _ => {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let hello: MobileMessage = match serde_json::from_str(&first) {
        Ok(m) => m,
        Err(_) => {
            let _ = send_err(&mut socket, "bad handshake json").await;
            return;
        }
    };
    // 065 — RAMA PAIRING (pre-Hello, SIN HMAC): el companion todavía no tiene el secreto. Canjea el
    // token efímero del QR por el secreto permanente y cierra. Nunca llega al path Hello firmado.
    if let MobileMessage::PairingRedeem {
        token,
        device_id,
        device_name,
    } = &hello
    {
        use crate::services::mobile_qr_pairing::{mark_grant_sent, redeem, RedeemResult};
        match redeem(token, device_id, device_name) {
            RedeemResult::Grant { secret, session_id } => {
                let grant = MobileMessage::PairingGrant {
                    secret,
                    session_id: session_id.clone(),
                };
                if send_msg(&mut socket, &grant).await {
                    mark_grant_sent(token);
                    (state.emit)(
                        "mobile-pairing-done",
                        serde_json::json!({
                            "session_id": session_id,
                            "device_id": device_id,
                            "device_name": device_name,
                        }),
                    );
                    tracing::info!(
                        device_id = %device_id,
                        device_name = %device_name,
                        session_id = %session_id,
                        "mobile pairing successful"
                    );
                }
            }
            RedeemResult::Expired => {
                let _ = send_err(&mut socket, "token_expired").await;
            }
            RedeemResult::AlreadyUsed => {
                let _ = send_err(&mut socket, "token_already_used").await;
            }
            RedeemResult::Invalid => {
                let _ = send_err(&mut socket, "token_invalid").await;
            }
            RedeemResult::SecretLoadFailed => {
                let _ = send_err(&mut socket, "internal_error").await;
            }
        }
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    if !matches!(hello, MobileMessage::Hello { .. }) {
        let _ = send_err(&mut socket, "expected hello").await;
        return;
    }
    if let Err(e) = verify_frame(&state, &hello) {
        let _ = send_err(&mut socket, e).await;
        return;
    }
    let ack = MobileMessage::HelloAck {
        panes: pane_meta_list(&state),
        cards: open_cards(&state),
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: MOBILE_PROTOCOL_VERSION,
        ts: now_secs(),
    };
    if !send_msg(&mut socket, &ack).await {
        return;
    }
    // 017 — push the SIGNED bottom-nav structure (if the desktop host registered
    // one) + the command catalog right after the handshake, so the phone renders
    // the bottom-nav without an extra round-trip. NavSpec is omitted (not an error)
    // if the host hasn't called `mobile_bridge_set_navspec` yet.
    let secret_bytes = state.secret.as_bytes().to_vec();
    if let Some(nav) = build_navspec_frame(&secret_bytes) {
        if !send_msg(&mut socket, &nav).await {
            return;
        }
    }
    if !send_msg(&mut socket, &build_catalog_frame(&secret_bytes)).await {
        return;
    }
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind: "mobile.connect",
            actor: "mobile",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({}),
        })
        .ok();

    // ── main loop: client frames + 2s snapshot push + notification fan-out ──
    let mut subscribed: Option<String> = None;
    let mut notif_rx = NOTIFY_BUS.subscribe();
    // 017 — typed AppEvent fan-out from the kernel event bus (same seq as windows).
    // Per-connection subscription (T064: one subscription per client-id; switching
    // transport opens a fresh connection → fresh subscription, no double-sub).
    let mut event_rx = crate::services::event_bus::subscribe_envelopes();
    // 017 [T068] — per-connection exec rate-limit: last exec ts per command_id.
    let mut exec_last: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let secret_bytes = state.secret.as_bytes().to_vec();
    // Last-seen pane state per pane, for Busy→Ready ("Claude waiting") detection.
    let mut pane_states: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(t))) => {
                        if !handle_client_frame(&state, &mut socket, &mut subscribed, &mut exec_last, &secret_bytes, &t).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(p))) => { let _ = socket.send(Message::Pong(p)).await; }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // binary / pong — ignore
                    Some(Err(_)) => break,
                }
            }
            // Notification fan-out (cards / Grafana / audit), filtered by toggle.
            ev = notif_rx.recv() => {
                match ev {
                    Ok(ev) => {
                        if notify_enabled(&state, &ev.kind) {
                            let n = MobileMessage::Notification {
                                kind: ev.kind, title: ev.title, body: ev.body,
                                severity: ev.severity, correlation_id: ev.correlation_id, ts: now_secs(),
                            };
                            if !send_msg(&mut socket, &n).await { break; }
                        }
                    }
                    // Lagged (slow phone) — skip dropped events, keep going.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            // 017 — typed AppEvent fan-out: forward each kernel envelope as a SIGNED
            // AppEvent frame, payload redacted (T066), seq preserved (FR-010).
             env = event_rx.recv() => {
                match env {
                    Ok(env) => {
                        if let Some(frame) = build_app_event_frame(&secret_bytes, &env) {
                            if !send_msg(&mut socket, &frame).await { break; }
                        }
                    }
                    // Lagged: the phone re-syncs by seq on the next event (a missed
                    // event only matters if it's the LATEST; older ones are stale anyway).
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            _ = ticker.tick() => {
                if !push_tick(&state, &mut socket, &subscribed, &mut pane_states).await {
                    break;
                }
            }
        }
    }
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind: "mobile.disconnect",
            actor: "mobile",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({}),
        })
        .ok();
}

/// Process one client→server text frame. Returns false to close the connection.
#[allow(clippy::too_many_arguments)]
async fn handle_client_frame(
    state: &BridgeState,
    socket: &mut WebSocket,
    subscribed: &mut Option<String>,
    exec_last: &mut std::collections::HashMap<String, i64>,
    secret_bytes: &[u8],
    text: &str,
) -> bool {
    let msg: MobileMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(_) => return send_err(socket, "bad json").await,
    };
    match &msg {
        // Liveness — unsigned, no side effects.
        MobileMessage::Ping { nonce, .. } => {
            send_msg(
                socket,
                &MobileMessage::Pong {
                    nonce: nonce.clone(),
                    ts: now_secs(),
                },
            )
            .await
        }
        // 017 — pure read: re-send the SIGNED command catalog on demand. Unsigned
        // request (no side effect); the reply is signed.
        MobileMessage::GetCommands { .. } => {
            send_msg(socket, &build_catalog_frame(secret_bytes)).await
        }
        // 017 [T060/T061] — execute a registry command by id ref. SIGNED frame;
        // verified (skew/HMAC/replay) BEFORE any authz, then re-authorized at
        // exec-time against the registry SSOT (visibility is NOT authz).
        MobileMessage::ExecuteCommand { command_id, .. } => {
            if let Err(e) = verify_frame(state, &msg) {
                return send_err(socket, e).await;
            }
            handle_execute_command(state, socket, exec_last, command_id).await
        }
        MobileMessage::Hello { .. } => send_err(socket, "already handshaked").await,
        MobileMessage::Subscribe { pane_id, .. } => {
            if let Err(e) = verify_frame(state, &msg) {
                return send_err(socket, e).await;
            }
            // Only panes Furx owns are reachable (UC-A constraint).
            if state.pty.pane_ids().iter().any(|p| p == pane_id) {
                *subscribed = Some(pane_id.clone());
                let snap = snapshot_msg(state, pane_id);
                send_msg(socket, &snap).await
            } else {
                send_err(socket, "unknown pane").await
            }
        }
        MobileMessage::PtyWrite {
            pane_id,
            text,
            source,
            ..
        } => {
            if let Err(e) = verify_frame(state, &msg) {
                return send_err(socket, e).await;
            }
            state.pane_state.on_input(pane_id);
            match state.pty.write(pane_id, text.as_bytes()) {
                Ok(()) => {
                    let actor = format!("mobile:{}", sanitize_source(source));
                    state
                        .audit
                        .write(crate::bases::audit::EventInput {
                            kind: "mobile.pty_write",
                            actor: actor.as_str(),
                            pane_id: Some(pane_id),
                            card_id: None,
                            correlation_id: None,
                            // F-IV: do NOT log the text body (could carry secrets);
                            // only its length + source.
                            payload: serde_json::json!({"len": text.len(), "source": sanitize_source(source)}),
                        })
                        .ok();
                    true
                }
                Err(_) => send_err(socket, "pane write failed").await,
            }
        }
        MobileMessage::ApproveToolCall {
            correlation_id,
            decision,
            ..
        } => {
            if let Err(e) = verify_frame(state, &msg) {
                return send_err(socket, e).await;
            }
            apply_approval(state, socket, correlation_id, decision).await
        }
        // Server→client frame types must never arrive from a client.
        _ => send_err(socket, "unexpected message").await,
    }
}

fn sanitize_source(s: &str) -> &str {
    match s {
        "voice" | "manual" | "approve" => s,
        _ => "manual",
    }
}

/// 017 [T060/T061/T068] — handle a verified `ExecuteCommand`. Pipeline:
///   1. rate-limit (1 per EXEC_RATE_LIMIT_SECS per command_id on this connection).
///   2. authorize at EXEC TIME against the registry SSOT (`authorize_mobile_command`):
///      unknown/internal/hidden/denied → reject; Destructive/Credential/confirm →
///      pending approval (create + emit ApprovalRequested); Safe/External → allowed.
///   3. ALLOWED → route the request to the desktop host via the event emitter
///      (`furx:mobile-exec`). The desktop runs it through the SAME universal gate
///      (lib.rs dispatch_gate) — the bridge never holds a privileged invoke path,
///      so this exec-time check is defense-in-depth, not the sole gate.
///   4. ALWAYS audit-log the mobile exec attempt (id + outcome, no args/secrets).
async fn handle_execute_command(
    state: &BridgeState,
    socket: &mut WebSocket,
    exec_last: &mut std::collections::HashMap<String, i64>,
    command_id: &str,
) -> bool {
    // Bound the id length (defensive; ids are short snake_case).
    if command_id.is_empty() || command_id.len() > 128 {
        return send_err(socket, "invalid command id").await;
    }
    // 1. Rate-limit per command_id on this connection.
    let now = now_secs();
    if let Some(prev) = exec_last.get(command_id) {
        if now - prev < EXEC_RATE_LIMIT_SECS {
            return send_err(socket, "rate limited").await;
        }
    }
    exec_last.insert(command_id.to_string(), now);

    // 2. Exec-time authorization (registry SSOT — visibility is NOT authz).
    let outcome = authorize_mobile_command(state, command_id);
    let (audit_outcome, ret): (&str, bool) = match outcome {
        MobileExecAuthz::Rejected(reason) => {
            let r = send_err(socket, "command not authorized").await;
            tracing::info!("mobile exec rejected: {} ({})", command_id, reason);
            ("rejected", r)
        }
        MobileExecAuthz::Pending {
            request_id,
            command_id: cid,
        } => {
            // Emit ApprovalRequested with the kernel seq → desktop windows + other
            // phones see it (same path as lib.rs gate). publish_envelope assigns
            // the seq + fans out to EVENT_BUS (this connection's event_rx will
            // forward the signed AppEvent frame); we also push it to the windows
            // via the bridge's EmitFn.
            if let Some(env) = crate::services::event_bus::publish_envelope(
                crate::services::event_bus::AppEvent::ApprovalRequested {
                    request_id: request_id.clone(),
                    command_id: cid.clone(),
                },
            ) {
                if let Ok(val) = serde_json::to_value(&env) {
                    (state.emit)(crate::services::event_bus::BUS_CHANNEL, val);
                }
            }
            // Tell the phone its command is pending (not executed).
            let r = send_msg(
                socket,
                &MobileMessage::BridgeError {
                    message: format!("pending_approval:{request_id}"),
                },
            )
            .await;
            ("pending", r)
        }
        MobileExecAuthz::Allowed => {
            // Route to the desktop host to actually run it through the universal
            // gate. The bridge has no privileged invoke path of its own.
            (state.emit)(
                "furx:mobile-exec",
                serde_json::json!({ "command_id": command_id }),
            );
            ("dispatched", true)
        }
    };
    // 4. Audit every mobile exec attempt (id + outcome only — no args, no secrets).
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind: "mobile.execute_command",
            actor: "mobile",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({ "command_id": command_id, "outcome": audit_outcome }),
        })
        .ok();
    ret
}

/// Resolve a pending card from the phone — same contract as telegram_inbound so
/// dashboard, Telegram, and phone share ONE approval path (spec synergy note).
async fn apply_approval(
    state: &BridgeState,
    socket: &mut WebSocket,
    correlation_id: &str,
    decision: &str,
) -> bool {
    let canonical = match decision {
        "approve" | "approved" => "approved",
        "reject" | "rejected" => "rejected",
        "snooze" | "snoozed" => "snoozed",
        _ => return send_err(socket, "invalid decision").await,
    };
    if correlation_id.is_empty() || correlation_id.len() > 64 {
        return send_err(socket, "invalid correlation_id").await;
    }
    let status_col = if canonical == "snoozed" {
        "open"
    } else {
        "closed"
    };
    // Run the UPDATE and collapse to a plain bool INSIDE this block so the
    // (non-Send) MutexGuard + rusqlite params are dropped before any `.await`
    // — otherwise the handler future isn't Send and axum rejects it.
    let db_ok = {
        let conn = state.db.lock();
        match conn.execute(
            "UPDATE cards SET decision = ?, decided_at = datetime('now'), status = ? WHERE id = ?",
            rusqlite::params![canonical, status_col, correlation_id],
        ) {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("mobile approve db: {}", e);
                false
            }
        }
    };
    if !db_ok {
        return send_err(socket, "db error").await;
    }
    state
        .audit
        .write(crate::bases::audit::EventInput {
            kind: "mobile.approve",
            actor: "mobile",
            pane_id: None,
            card_id: Some(correlation_id),
            correlation_id: Some(correlation_id),
            payload: serde_json::json!({"decision": canonical}),
        })
        .ok();
    (state.emit)(
        "furx:mobile-callback",
        serde_json::json!({"card_id": correlation_id, "action": canonical}),
    );
    true
}

/// Compute the bind addresses for the bridge: loopback always; the Tailscale IP
/// too iff `tailscale_enabled`. Detection parses the CGNAT range 100.64.0.0/10
/// from local interfaces — no shell-out to the `tailscale` CLI (council MC-5).
pub fn bridge_bind_addrs(tailscale_enabled: bool) -> Vec<SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr};
    let mut addrs = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, MOBILE_BRIDGE_PORT))];
    if tailscale_enabled {
        if let Some(IpAddr::V4(ip)) = tailscale_ipv4() {
            addrs.push(SocketAddr::from((ip, MOBILE_BRIDGE_TAILSCALE_PORT)));
        } else {
            tracing::info!("mobile_bridge: tailscale enabled but no 100.64.0.0/10 iface found");
        }
    }
    addrs
}

/// True if `ip` is in the Tailscale CGNAT range 100.64.0.0/10.
fn is_tailscale_cgnat(ip: &std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    // 100.64.0.0/10 → first octet 100, second octet 64..=127.
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Find a local IPv4 in the Tailscale CGNAT range 100.64.0.0/10 by enumerating
/// interfaces via `getifaddrs` (no shell-out — council MC-5). Returns the first
/// match (a host has a single tailnet IPv4).
///
/// Known limitations (4-frontier review, accepted as non-blocking for MVP):
///   - IPv6 tailnet addrs (`fd7a:115c:a1e0::/48`) are not detected. Tailscale
///     always assigns a 100.x IPv4, so the bridge still binds.
///   - 100.64.0.0/10 is CGNAT, not Tailscale-exclusive; another CGNAT VPN could
///     match. The bind target is still a local interface address (not a public
///     one), so the blast radius is limited to that local network.
pub fn tailscale_ipv4() -> Option<std::net::IpAddr> {
    use std::net::IpAddr;
    let ifaces = if_addrs::get_if_addrs().ok()?;
    ifaces.into_iter().find_map(|iface| match iface.ip() {
        IpAddr::V4(v4) if is_tailscale_cgnat(&v4) => Some(IpAddr::V4(v4)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_serialization() {
        let msg = MobileMessage::Ping {
            nonce: "abc".into(),
            ts: 1234567890,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("ping"));
    }

    fn sign(secret: &[u8], canonical: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(canonical);
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn verify_hmac_accepts_valid_pty_write() {
        let secret = b"hunter2-shared-mobile-secret";
        let canonical = canonical_bytes(b"PtyWrite", "n1", 1700000000, "p7", "voice", "echo hi");
        let sig = sign(secret, &canonical);
        let msg = MobileMessage::PtyWrite {
            pane_id: "p7".into(),
            text: "echo hi".into(),
            source: "voice".into(),
            nonce: "n1".into(),
            ts: 1700000000,
            sig,
        };
        assert!(verify_hmac(secret, &msg).unwrap());
    }

    #[test]
    fn verify_hmac_rejects_wrong_secret() {
        let canonical = canonical_bytes(b"PtyWrite", "n1", 1700000000, "p7", "voice", "echo hi");
        let sig = sign(b"good", &canonical);
        let msg = MobileMessage::PtyWrite {
            pane_id: "p7".into(),
            text: "echo hi".into(),
            source: "voice".into(),
            nonce: "n1".into(),
            ts: 1700000000,
            sig,
        };
        assert!(!verify_hmac(b"WRONG", &msg).unwrap());
    }

    #[test]
    fn verify_hmac_rejects_tampered_text() {
        let secret = b"hunter2-shared-mobile-secret";
        // Sign for "echo hi" but ship "rm -rf /" — must NOT verify.
        let canonical = canonical_bytes(b"PtyWrite", "n1", 1700000000, "p7", "voice", "echo hi");
        let sig = sign(secret, &canonical);
        let msg = MobileMessage::PtyWrite {
            pane_id: "p7".into(),
            text: "rm -rf /".into(),
            source: "voice".into(),
            nonce: "n1".into(),
            ts: 1700000000,
            sig,
        };
        assert!(!verify_hmac(secret, &msg).unwrap());
    }

    #[test]
    fn verify_hmac_rejects_non_hex_sig() {
        let msg = MobileMessage::ApproveToolCall {
            correlation_id: "c1".into(),
            decision: "approve".into(),
            nonce: "n".into(),
            ts: 1700000000,
            sig: "not-hex-zz!!".into(),
        };
        assert!(!verify_hmac(b"k", &msg).unwrap());
    }

    #[test]
    fn verify_hmac_returns_false_for_unsigned_message_types() {
        // Audit Llama MED: was Err, now Ok(false) so callers can branch on bool.
        let msg = MobileMessage::Ping {
            nonce: "x".into(),
            ts: 1,
        };
        assert!(!verify_hmac(b"k", &msg).unwrap());
    }

    #[test]
    fn canonical_form_is_unambiguous() {
        // Audit 3/3 MED: this is the regression test for the field-shift attack
        // — split "p.a" + "b" vs "p" + "a.b" must produce different canonicals.
        let a = canonical_bytes(b"PtyWrite", "n", 1, "p.a", "voice", "b");
        let b = canonical_bytes(b"PtyWrite", "n", 1, "p", "voice", "a.b");
        assert_ne!(a, b);
    }

    #[test]
    fn cross_lang_hmac_vector() {
        // Fixed vector cross-checked against the PWA's furx-sign.js (Node).
        // If the JS canonical encoding drifts, that test fails against THIS hex.
        // secret = 64-char hex string (used as raw UTF-8 key bytes, like prod).
        let secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let canonical =
            canonical_bytes(b"PtyWrite", "n-vec", 1700000000, "p7", "manual", "echo hi");
        let sig = sign(secret.as_bytes(), &canonical);
        // Value emitted by this very test (printed below) and pinned so the JS
        // side can assert the same constant. See mobile-companion/pwa test.
        assert_eq!(
            sig, "423eadd24f252979c0d7fdc41b87f23e64f7fab9213c7ad49610b6b5883d3e22",
            "if this changed, re-pin the vector in furx-sign cross-lang test"
        );
        // Hello vector — exercises EMPTY source/body fields (length-0 prefixes),
        // cross-checked against furx-sign.js (Node).
        let hello = canonical_bytes(b"HelloMsg", "n-hello", 1700000000, "phone-1", "", "");
        assert_eq!(
            sign(secret.as_bytes(), &hello),
            "9be5c54d4ae22687f86e00803639ca7e67571938aa525f2e854510b0419c277c"
        );
    }

    #[test]
    fn tailscale_cgnat_range_boundaries() {
        use std::net::Ipv4Addr;
        assert!(is_tailscale_cgnat(&Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_tailscale_cgnat(&Ipv4Addr::new(100, 127, 255, 254)));
        assert!(!is_tailscale_cgnat(&Ipv4Addr::new(100, 63, 0, 1))); // below /10
        assert!(!is_tailscale_cgnat(&Ipv4Addr::new(100, 128, 0, 1))); // above /10
        assert!(!is_tailscale_cgnat(&Ipv4Addr::new(192, 168, 1, 1)));
    }

    #[test]
    fn notify_defaults_match_council() {
        assert!(notify_default("card"));
        assert!(notify_default("grafana"));
        assert!(notify_default("pane_ready"));
        assert!(!notify_default("audit")); // opt-in
        assert!(!notify_default("unknown"));
    }

    #[test]
    fn bind_addrs_loopback_only_when_tailscale_off() {
        let addrs = bridge_bind_addrs(false);
        assert_eq!(addrs.len(), 1);
        assert!(addrs[0].ip().is_loopback());
        assert_eq!(addrs[0].port(), MOBILE_BRIDGE_PORT);
        // AT-7 invariant: never an unspecified bind.
        assert!(!addrs[0].ip().is_unspecified());
    }

    // ───────────────────────── 017 mobile reform — unit ──────────────────────

    #[test]
    fn t020_catalog_excludes_internal_hidden_and_denied() {
        // CommandCatalog projection (T020/T065): no internal/hidden, no infra/ssh/vpn.
        let cat = mobile_command_catalog();
        assert!(!cat.is_empty(), "catalog should not be empty");
        // No deny-listed categories / id prefixes.
        for c in &cat {
            assert!(
                !matches!(c.category.as_str(), "ssh" | "vpn" | "infra" | "tmux"),
                "deny-listed category leaked: {} ({})",
                c.id,
                c.category
            );
            assert!(!c.id.starts_with("ssh_"), "ssh_ id leaked: {}", c.id);
            assert!(!c.id.starts_with("vpn_"), "vpn_ id leaked: {}", c.id);
        }
        // Cross-check against the registry: every eligible id is present, and NO
        // internal/hidden id is present.
        use crate::services::command_registry::{registry, Visibility};
        let cat_ids: std::collections::HashSet<&str> = cat.iter().map(|c| c.id.as_str()).collect();
        for def in registry() {
            if matches!(def.visibility, Visibility::Internal | Visibility::Hidden) {
                assert!(
                    !cat_ids.contains(def.id.as_str()),
                    "internal/hidden in catalog: {}",
                    def.id
                );
            }
        }
        // A known palette command (list_monitors) IS present; a known internal one
        // (pty_write) is NOT.
        assert!(
            cat_ids.contains("list_monitors"),
            "palette cmd missing from catalog"
        );
        assert!(
            !cat_ids.contains("pty_write"),
            "internal pty_write must not be in catalog"
        );
        // vpn_status is External+Palette but deny-listed for mobile → absent.
        assert!(
            !cat_ids.contains("vpn_status"),
            "vpn must be denied on mobile"
        );
    }

    #[test]
    fn t062_validate_navspec_rejects_unknown_domain() {
        // Valid spec passes.
        let good = serde_json::json!({
            "version": 1,
            "domains": [{ "domainId": "work", "label": "Trabajo", "items": [
                { "view": "panes", "label": "Paneles", "icon": "x" }
            ]}]
        });
        assert!(validate_navspec(&good).is_ok());
        // Unknown domain id → rejected (injection across trust boundary).
        let bad = serde_json::json!({
            "version": 1,
            "domains": [{ "domainId": "evil", "label": "Pwn", "items": [] }]
        });
        assert!(
            validate_navspec(&bad).is_err(),
            "unknown domain id must be rejected"
        );
        // Missing version → rejected.
        let no_ver = serde_json::json!({ "domains": [] });
        assert!(validate_navspec(&no_ver).is_err());
        // Not an object → rejected.
        assert!(validate_navspec(&serde_json::json!("nope")).is_err());
        // [SF-1] Unknown view id → rejected (even under a valid domain).
        let bad_view = serde_json::json!({
            "version": 1,
            "domains": [{ "domainId": "work", "label": "Trabajo", "items": [
                { "view": "evil_view", "label": "x" }
            ]}]
        });
        assert!(
            validate_navspec(&bad_view).is_err(),
            "unknown view id must be rejected"
        );
        // [SF-1] Over-long label → rejected (bloat/abuse cap).
        let long_label = "x".repeat(200);
        let bad_len = serde_json::json!({
            "version": 1,
            "domains": [{ "domainId": "work", "label": long_label, "items": [] }]
        });
        assert!(
            validate_navspec(&bad_len).is_err(),
            "over-long label must be rejected"
        );
    }

    #[test]
    fn t066_redact_app_event_caps_long_fields() {
        let long = "a".repeat(APPEVENT_FIELD_CAP + 500);
        let ev = serde_json::json!({ "tag": "TaskChanged", "data": { "id": long.clone(), "state": "running" } });
        let red = redact_app_event(&ev);
        let id = red["data"]["id"].as_str().unwrap();
        assert_eq!(
            id.chars().count(),
            APPEVENT_FIELD_CAP,
            "long field must be capped"
        );
        // Short field untouched.
        assert_eq!(red["data"]["state"], "running");
        // Tag preserved.
        assert_eq!(red["tag"], "TaskChanged");
        // [SF-2] sensitive-named keys are redacted wholesale, even nested, and the
        // cap applies inside nested objects/arrays (recursion).
        let nested = serde_json::json!({
            "tag": "AgentStateChanged",
            "data": {
                "auth_token": "sk-supersecretvalue",
                "nested": { "api_key": "abc", "note": long.clone() },
                "list": [ long.clone() ]
            }
        });
        let red2 = redact_app_event(&nested);
        assert_eq!(
            red2["data"]["auth_token"], "[redacted]",
            "sensitive key redacted"
        );
        assert_eq!(
            red2["data"]["nested"]["api_key"], "[redacted]",
            "nested sensitive key redacted"
        );
        assert_eq!(
            red2["data"]["nested"]["note"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            APPEVENT_FIELD_CAP,
            "nested long field capped"
        );
        assert_eq!(
            red2["data"]["list"][0].as_str().unwrap().chars().count(),
            APPEVENT_FIELD_CAP,
            "array element capped"
        );
    }

    #[test]
    fn t063_sign_outbound_matches_canonical() {
        // Outbound sign must equal HMAC over the canonical bytes (tag in scope slot
        // empty src/body), so the phone can recompute it.
        let secret = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let body = r#"{"version":1,"domains":[]}"#;
        let sig = sign_outbound(secret, TAG_NAVSPEC, "n-out", 1700000000, body);
        // Recompute manually.
        let canonical = canonical_bytes(TAG_NAVSPEC, "n-out", 1700000000, body, "", "");
        let expected = {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
            mac.update(&canonical);
            hex::encode(mac.finalize().into_bytes())
        };
        assert_eq!(sig, expected);
        // Distinct tags → distinct sigs over the same body (no cross-frame replay).
        let sig_cat = sign_outbound(secret, TAG_CMDCATALOG, "n-out", 1700000000, body);
        assert_ne!(
            sig, sig_cat,
            "different frame tags must yield different sigs"
        );
        // Cross-lang pin (matches mobile-companion/pwa/outbound-sign.test.mjs). If
        // the JS verifyOutbound encoding drifts, that test fails against THIS hex.
        assert_eq!(
            sig, "e45ba44aac4cab09319da86fbb8788ccb67056198d371fea39889dfd47b6c51e",
            "outbound NavSpec sig drifted — re-pin the JS cross-lang vector"
        );
    }

    #[test]
    fn t060_execute_command_canonical_is_distinct() {
        // ExecuteCommand uses tag ExecCmd_ with command_id in the scope slot. A
        // PtyWrite-shaped frame with the same nonce/ts/pane can't be replayed as
        // an ExecuteCommand (different tag → different canonical).
        let exec = canonical_bytes(b"ExecCmd_", "n", 1, "reset_furx", "", "");
        let pty = canonical_bytes(b"PtyWrite", "n", 1, "reset_furx", "", "");
        assert_ne!(exec, pty);
    }

    #[test]
    fn verify_hmac_accepts_valid_execute_command() {
        let secret = b"hunter2-shared-mobile-secret";
        let canonical = canonical_bytes(b"ExecCmd_", "ne", 1700000000, "list_monitors", "", "");
        let sig = sign(secret, &canonical);
        let msg = MobileMessage::ExecuteCommand {
            command_id: "list_monitors".into(),
            nonce: "ne".into(),
            ts: 1700000000,
            sig,
        };
        assert!(verify_hmac(secret, &msg).unwrap());
        // Tampered command_id (sign for list_monitors, ship reset_furx) → reject.
        let tampered = MobileMessage::ExecuteCommand {
            command_id: "reset_furx".into(),
            nonce: "ne".into(),
            ts: 1700000000,
            sig: sign(
                secret,
                &canonical_bytes(b"ExecCmd_", "ne", 1700000000, "list_monitors", "", ""),
            ),
        };
        assert!(!verify_hmac(secret, &tampered).unwrap());
    }

    #[test]
    fn signed_outbound_frames_are_not_client_verifiable() {
        // NavSpec/CommandCatalog/AppEvent are server-signed; verify_hmac (client→server
        // path) returns Ok(false) for them (they're never accepted as client frames).
        let nav = MobileMessage::NavSpec {
            spec: serde_json::json!({}),
            nonce: "n".into(),
            ts: 1,
            sig: "ab".into(),
        };
        assert!(!verify_hmac(b"k", &nav).unwrap());
        let cat = MobileMessage::CommandCatalog {
            commands: serde_json::json!([]),
            nonce: "n".into(),
            ts: 1,
            sig: "ab".into(),
        };
        assert!(!verify_hmac(b"k", &cat).unwrap());
    }
}

// ───────────────────────── E2E: real WebSocket over TCP ──────────────────────
// Drives the bridge end-to-end through a real loopback TCP WebSocket — proving
// the council "first deliverable": handshake auth, per-message HMAC (signed with
// the SAME `canonical_bytes` the server verifies), tampered rejection, replay
// rejection, and the approval DB path. No mobile app, no AppHandle (uses the
// runtime-agnostic `start_with_emit` + no-op emit).
#[cfg(test)]
mod e2e_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as TMessage;

    const SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"; // 64 hex

    fn sign(canonical: &[u8]) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(canonical);
        hex::encode(mac.finalize().into_bytes())
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    fn boot_bridge() -> (
        MobileBridge,
        std::net::SocketAddr,
        Arc<Mutex<rusqlite::Connection>>,
    ) {
        let pane_state = crate::bases::state::PaneStateModel::new();
        let pty = Arc::new(crate::pty::PtyManager::new(pane_state.clone()));
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE cards (id TEXT PRIMARY KEY, created_at TEXT DEFAULT (datetime('now')), \
             project TEXT DEFAULT 'furx', title TEXT DEFAULT '', severity TEXT DEFAULT 'info', \
             decision TEXT NOT NULL DEFAULT '', decided_at TEXT, status TEXT NOT NULL DEFAULT 'open');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cards (id, title, severity, project) VALUES ('card-1','Pending build','warning','scanner')",
            [],
        )
        .unwrap();
        // 017: the `approvals` schema so ExecuteCommand's pending-approval path
        // (capability::create_pending) works in the E2E exec-authz test.
        conn.execute_batch(include_str!("../../migrations/028_approvals.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/030_approval_consume.sql"))
            .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let audit = crate::bases::audit::AuditWriter::new(db.clone());
        let emit: EmitFn = Arc::new(|_ev, _val| {});
        let bridge = MobileBridge::start_with_emit(
            pty,
            pane_state,
            db.clone(),
            audit,
            SECRET.to_string(),
            vec![std::net::SocketAddr::from(([127, 0, 0, 1], 0))],
            emit,
        )
        .unwrap();
        let addr = bridge.local_addrs()[0];
        (bridge, addr, db)
    }

    // Next app-level JSON frame (skips WS control frames).
    async fn recv_raw<S>(ws: &mut S) -> serde_json::Value
    where
        S: StreamExt<Item = Result<TMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        loop {
            match ws.next().await {
                Some(Ok(TMessage::Text(t))) => return serde_json::from_str(&t).unwrap(),
                Some(Ok(_)) => continue, // skip ping/pong/binary control frames
                other => panic!("ws closed/error: {:?}", other),
            }
        }
    }

    // Response-expecting recv: skips UNSOLICITED server pushes (notification,
    // pane_snapshot) so request/response tests are robust to the global
    // NOTIFY_BUS (parallel tests) + the 2s snapshot ticker.
    async fn recv_json<S>(ws: &mut S) -> serde_json::Value
    where
        S: StreamExt<Item = Result<TMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        loop {
            let m = recv_raw(ws).await;
            match m["type"].as_str().unwrap_or("") {
                // 017: nav_spec / command_catalog / app_event are unsolicited server
                // pushes too — skip them in request/response tests.
                "notification" | "pane_snapshot" | "nav_spec" | "command_catalog" | "app_event" => {
                    continue
                }
                _ => return m,
            }
        }
    }

    #[tokio::test]
    async fn full_handshake_auth_tamper_replay_and_approve() {
        let (_bridge, addr, db) = boot_bridge();
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        // ── 1. handshake: signed Hello → HelloAck ──
        let ts = now();
        let hello_sig = sign(&canonical_bytes(
            b"HelloMsg",
            "n-hello",
            ts,
            "phone-1",
            "",
            "",
        ));
        let hello = serde_json::json!({
            "type": "hello", "client_id": "phone-1",
            "nonce": "n-hello", "ts": ts, "sig": hello_sig
        });
        ws.send(TMessage::Text(hello.to_string())).await.unwrap();
        let ack = recv_json(&mut ws).await;
        assert_eq!(ack["type"], "hello_ack", "expected hello_ack, got {ack}");
        // HelloAck carries the open cards for the approve list (F4.6).
        assert_eq!(ack["cards"][0]["id"], "card-1");
        assert_eq!(ack["cards"][0]["title"], "Pending build");

        // ── 2. valid PtyWrite to a missing pane: auth passes → "pane write failed"
        //       (proves the signed path reached pty.write, not a sig reject) ──
        let ts = now();
        let pw_canon = canonical_bytes(b"PtyWrite", "n-pw1", ts, "ghost", "manual", "echo hi");
        let pw_sig = sign(&pw_canon);
        let pw = serde_json::json!({
            "type": "pty_write", "pane_id": "ghost", "text": "echo hi",
            "source": "manual", "nonce": "n-pw1", "ts": ts, "sig": pw_sig
        });
        ws.send(TMessage::Text(pw.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(r["type"], "bridge_error");
        assert_eq!(
            r["message"], "pane write failed",
            "auth should pass; got {r}"
        );

        // ── 3. replay the SAME signed frame → "replay" (nonce already burned) ──
        ws.send(TMessage::Text(pw.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(
            r["message"], "replay",
            "second send must be replay; got {r}"
        );

        // ── 4. tampered: sign for "echo hi" but send "rm -rf /" → "bad signature" ──
        let ts = now();
        let good_sig = sign(&canonical_bytes(
            b"PtyWrite",
            "n-tamp",
            ts,
            "ghost",
            "manual",
            "echo hi",
        ));
        let tampered = serde_json::json!({
            "type": "pty_write", "pane_id": "ghost", "text": "rm -rf /",
            "source": "manual", "nonce": "n-tamp", "ts": ts, "sig": good_sig
        });
        ws.send(TMessage::Text(tampered.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(
            r["message"], "bad signature",
            "tamper must be rejected; got {r}"
        );

        // ── 5. stale ts (skew) → "ts skew" even with a valid sig ──
        let stale = now() - 600;
        let stale_sig = sign(&canonical_bytes(
            b"PtyWrite",
            "n-stale",
            stale,
            "ghost",
            "manual",
            "x",
        ));
        let stale_msg = serde_json::json!({
            "type": "pty_write", "pane_id": "ghost", "text": "x",
            "source": "manual", "nonce": "n-stale", "ts": stale, "sig": stale_sig
        });
        ws.send(TMessage::Text(stale_msg.to_string()))
            .await
            .unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(
            r["message"], "ts skew",
            "stale ts must be rejected; got {r}"
        );

        // ── 6. signed ApproveToolCall → updates the card (success sends nothing,
        //       so we round-trip a Ping/Pong to ensure ordering, then check DB) ──
        let ts = now();
        let appr_sig = sign(&canonical_bytes(
            b"ApprovTC",
            "n-appr",
            ts,
            "card-1",
            "",
            "approve",
        ));
        let appr = serde_json::json!({
            "type": "approve_tool_call", "correlation_id": "card-1", "decision": "approve",
            "nonce": "n-appr", "ts": ts, "sig": appr_sig
        });
        ws.send(TMessage::Text(appr.to_string())).await.unwrap();
        // ping → pong barrier (frames are processed sequentially server-side)
        let ping = serde_json::json!({"type": "ping", "nonce": "p1", "ts": now()});
        ws.send(TMessage::Text(ping.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(r["type"], "pong");
        let decision: String = db
            .lock()
            .query_row("SELECT decision FROM cards WHERE id='card-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(decision, "approved", "approval must update the card");
    }

    // Reads frames until one satisfies `pred` (skipping snapshots/pongs/other
    // notifications — the NOTIFY_BUS is global so parallel tests can bleed in).
    async fn recv_until<S, F>(ws: &mut S, pred: F) -> serde_json::Value
    where
        S: StreamExt<Item = Result<TMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
        F: Fn(&serde_json::Value) -> bool,
    {
        let deadline = tokio::time::Duration::from_secs(5);
        loop {
            let frame = tokio::time::timeout(deadline, recv_raw(ws))
                .await
                .expect("timeout waiting for frame");
            if pred(&frame) {
                return frame;
            }
        }
    }

    async fn handshake<S>(ws: &mut S)
    where
        S: StreamExt<Item = Result<TMessage, tokio_tungstenite::tungstenite::Error>>
            + futures_util::SinkExt<TMessage>
            + Unpin,
    {
        let ts = now();
        let sig = sign(&canonical_bytes(b"HelloMsg", "n-hs", ts, "t", "", ""));
        let hello =
            serde_json::json!({"type":"hello","client_id":"t","nonce":"n-hs","ts":ts,"sig":sig});
        let _ = ws.send(TMessage::Text(hello.to_string())).await;
        let ack = recv_json(ws).await;
        assert_eq!(ack["type"], "hello_ack");
        // ping->pong barrier: guarantees the server loop ran (NOTIFY_BUS subscribed).
        let ping = serde_json::json!({"type":"ping","nonce":"b","ts":now()});
        let _ = ws.send(TMessage::Text(ping.to_string())).await;
        recv_until(ws, |m| m["type"] == "pong").await;
    }

    #[tokio::test]
    async fn grafana_webhook_auth_and_fanout() {
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;

        let client = reqwest::Client::new();
        let gurl = format!("http://{}/furx/v1/grafana", addr);
        // wrong bearer → 403
        let bad = client
            .post(&gurl)
            .header("authorization", "Bearer nope")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 403);
        // correct bearer → 200, phone receives a grafana notification
        let payload = r#"{"status":"firing","alerts":[{"labels":{"alertname":"FurxTestAlertXZ","severity":"critical"},"annotations":{"summary":"cpu 99%"}}]}"#;
        let ok = client
            .post(&gurl)
            .header("authorization", format!("Bearer {}", SECRET))
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), 200);
        let n = recv_until(&mut ws, |m| {
            m["type"] == "notification" && m["title"] == "FurxTestAlertXZ"
        })
        .await;
        assert_eq!(n["kind"], "grafana");
        assert_eq!(n["severity"], "critical");
        assert_eq!(n["body"], "cpu 99%");
    }

    #[tokio::test]
    async fn card_notification_fanout() {
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;
        super::publish_notification(
            "card",
            "FurxCardTestQ7",
            "scanner · ci",
            "warning",
            Some("c9".into()),
        );
        let n = recv_until(&mut ws, |m| {
            m["type"] == "notification" && m["title"] == "FurxCardTestQ7"
        })
        .await;
        assert_eq!(n["kind"], "card");
        assert_eq!(n["correlation_id"], "c9");
    }

    #[tokio::test]
    async fn serves_pwa_bundle() {
        let (_bridge, addr, _db) = boot_bridge();
        let base = format!("http://{}", addr);
        let index = reqwest::get(format!("{}/", base))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(index.contains("Furx Mobile"), "index should be the PWA");
        assert!(
            index.contains("furx-sign.js"),
            "index should load the sign module"
        );
        let js = reqwest::get(format!("{}/furx-sign.js", base))
            .await
            .unwrap();
        assert!(js
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("javascript"));
        let body = js.text().await.unwrap();
        assert!(
            body.contains("canonicalBytes"),
            "sign module must export canonicalBytes"
        );
        // The bundled sign module must carry the SAME length-prefixed encoding
        // the server verifies (cross-checked numerically by cross_lang_hmac_vector).
        assert!(body.contains("HelloMsg") && body.contains("Subscrib"));
    }

    #[tokio::test]
    async fn rejects_unsigned_first_frame() {
        let (_bridge, addr, _db) = boot_bridge();
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        // first frame is a Ping (not Hello) → server replies error + closes
        let ping = serde_json::json!({"type": "ping", "nonce": "x", "ts": now()});
        ws.send(TMessage::Text(ping.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(r["type"], "bridge_error");
        assert_eq!(r["message"], "expected hello");
    }

    #[tokio::test]
    async fn rejects_hello_with_bad_secret() {
        let (_bridge, addr, _db) = boot_bridge();
        let url = format!("ws://{}/ws", addr);
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let ts = now();
        // sign with the WRONG secret
        let bad = {
            use hmac::{Hmac, Mac};
            use sha2::Sha256;
            let mut mac = Hmac::<Sha256>::new_from_slice(b"WRONG").unwrap();
            mac.update(&canonical_bytes(b"HelloMsg", "n", ts, "phone", "", ""));
            hex::encode(mac.finalize().into_bytes())
        };
        let hello = serde_json::json!({
            "type": "hello", "client_id": "phone", "nonce": "n", "ts": ts, "sig": bad
        });
        ws.send(TMessage::Text(hello.to_string())).await.unwrap();
        let r = recv_json(&mut ws).await;
        assert_eq!(r["type"], "bridge_error");
        assert_eq!(r["message"], "bad signature");
    }

    // ─────────────────────── 017 mobile reform — E2E ─────────────────────────

    // Verify a server-signed outbound frame (NavSpec/CommandCatalog/AppEvent) with
    // the test SECRET. body = JSON of the signed payload; for AppEvent it's "seq|json".
    fn verify_outbound(tag: &[u8; 8], frame: &serde_json::Value, body: &str) -> bool {
        let nonce = frame["nonce"].as_str().unwrap();
        let ts = frame["ts"].as_i64().unwrap();
        let provided = frame["sig"].as_str().unwrap();
        let canonical = super::canonical_bytes(tag, nonce, ts, body, "", "");
        let expect = sign(&canonical);
        provided == expect
    }

    #[tokio::test]
    async fn t013_navspec_and_catalog_signed_after_hello() {
        // Desktop host registers a NavSpec; the phone receives it SIGNED after Hello,
        // plus a signed CommandCatalog. (T010/T013/T063)
        let nav = serde_json::json!({
            "version": 1,
            "domains": [{ "domainId": "work", "label": "Trabajo",
                "items": [{ "view": "panes", "label": "Paneles", "icon": "x" }] }]
        });
        super::mobile_bridge_set_navspec(nav.clone()).unwrap();

        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        let ts = now();
        let sig = sign(&canonical_bytes(b"HelloMsg", "n-nav", ts, "phone", "", ""));
        ws.send(TMessage::Text(
            serde_json::json!({
                "type":"hello","client_id":"phone","nonce":"n-nav","ts":ts,"sig":sig
            })
            .to_string(),
        ))
        .await
        .unwrap();

        // HelloAck carries the protocol version (FR-016).
        let ack = recv_until(&mut ws, |m| m["type"] == "hello_ack").await;
        assert_eq!(ack["protocol_version"], super::MOBILE_PROTOCOL_VERSION);

        // Signed NavSpec frame arrives; signature verifies; domainId is "work".
        let navf = recv_until(&mut ws, |m| m["type"] == "nav_spec").await;
        let body = serde_json::to_string(&navf["spec"]).unwrap();
        assert!(
            verify_outbound(b"NavSpec_", &navf, &body),
            "navspec sig must verify"
        );
        assert_eq!(navf["spec"]["domains"][0]["domainId"], "work");

        // Signed CommandCatalog frame arrives; signature verifies; only eligible cmds.
        let catf = recv_until(&mut ws, |m| m["type"] == "command_catalog").await;
        let cbody = serde_json::to_string(&catf["commands"]).unwrap();
        assert!(
            verify_outbound(b"CmdCatlg", &catf, &cbody),
            "catalog sig must verify"
        );
        let ids: Vec<&str> = catf["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"list_monitors"), "palette cmd present");
        assert!(
            !ids.iter().any(|i| i.starts_with("ssh_")),
            "no ssh on mobile"
        );
    }

    #[tokio::test]
    async fn t022_t061_execute_safe_dispatches_destructive_pending() {
        // Safe command → dispatched (furx:mobile-exec); Destructive → pending approval,
        // NOT executed. Authz is re-derived server-side from the registry (T061).
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;

        // Safe (list_monitors): dispatched → server sends nothing back. Round-trip a
        // ping/pong barrier and assert NO bridge_error came before it.
        let ts = now();
        let sig = sign(&canonical_bytes(
            b"ExecCmd_",
            "x-safe",
            ts,
            "list_monitors",
            "",
            "",
        ));
        ws.send(TMessage::Text(serde_json::json!({
            "type":"execute_command","command_id":"list_monitors","nonce":"x-safe","ts":ts,"sig":sig
        }).to_string())).await.unwrap();
        // ping barrier
        ws.send(TMessage::Text(
            serde_json::json!({"type":"ping","nonce":"pb","ts":now()}).to_string(),
        ))
        .await
        .unwrap();
        let r = recv_until(&mut ws, |m| {
            m["type"] == "pong" || m["type"] == "bridge_error"
        })
        .await;
        assert_eq!(r["type"], "pong", "Safe exec must not error; got {r}");

        // Destructive (reset_furx): pending approval → bridge_error "pending_approval:..".
        let ts = now();
        let sig = sign(&canonical_bytes(
            b"ExecCmd_",
            "x-dst",
            ts,
            "reset_furx",
            "",
            "",
        ));
        ws.send(TMessage::Text(
            serde_json::json!({
                "type":"execute_command","command_id":"reset_furx","nonce":"x-dst","ts":ts,"sig":sig
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let r = recv_until(&mut ws, |m| {
            m["type"] == "bridge_error"
                && m["message"]
                    .as_str()
                    .map(|s| s.starts_with("pending_approval"))
                    .unwrap_or(false)
        })
        .await;
        assert!(
            r["message"]
                .as_str()
                .unwrap()
                .starts_with("pending_approval:"),
            "destructive must be pending; got {r}"
        );
    }

    #[tokio::test]
    async fn t061_execute_rejects_internal_and_unknown() {
        // internal command (pty_write) and unknown id → rejected "command not authorized".
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;

        for (cid, nonce) in [("pty_write", "x-int"), ("totally_made_up_cmd", "x-unk")] {
            let ts = now();
            let sig = sign(&canonical_bytes(b"ExecCmd_", nonce, ts, cid, "", ""));
            ws.send(TMessage::Text(
                serde_json::json!({
                    "type":"execute_command","command_id":cid,"nonce":nonce,"ts":ts,"sig":sig
                })
                .to_string(),
            ))
            .await
            .unwrap();
            let r = recv_until(&mut ws, |m| m["type"] == "bridge_error").await;
            assert_eq!(
                r["message"], "command not authorized",
                "id {cid} must be rejected; got {r}"
            );
        }
    }

    #[tokio::test]
    async fn t060_execute_tampered_command_id_rejected() {
        // Sign for list_monitors but ship reset_furx → bad signature (T060 integrity).
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;
        let ts = now();
        let sig = sign(&canonical_bytes(
            b"ExecCmd_",
            "x-tamp",
            ts,
            "list_monitors",
            "",
            "",
        ));
        ws.send(TMessage::Text(serde_json::json!({
            "type":"execute_command","command_id":"reset_furx","nonce":"x-tamp","ts":ts,"sig":sig
        }).to_string())).await.unwrap();
        let r = recv_until(&mut ws, |m| m["type"] == "bridge_error").await;
        assert_eq!(
            r["message"], "bad signature",
            "tampered exec must reject; got {r}"
        );
    }

    #[tokio::test]
    async fn t031_app_event_forwarded_signed_with_seq() {
        // A kernel AppEvent emitted via the event bus arrives at the phone as a SIGNED
        // app_event frame, with the SAME seq, payload redacted/intact. (T030/T031/T063)
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await; // subscribes to EVENT_BUS in the loop

        // Emit a unique TaskChanged via the bus fan-out (no AppHandle needed).
        let env = crate::services::event_bus::publish_envelope(
            crate::services::event_bus::AppEvent::TaskChanged {
                id: "t031-unique-task".into(),
                state: "running".into(),
            },
        )
        .expect("non-coalesced");

        let ef = recv_until(&mut ws, |m| {
            m["type"] == "app_event" && m["event"]["data"]["id"] == "t031-unique-task"
        })
        .await;
        // seq preserved.
        assert_eq!(
            ef["seq"].as_u64().unwrap(),
            env.seq,
            "seq must match kernel envelope"
        );
        // Signature verifies (body = "seq|json").
        let body = format!(
            "{}|{}",
            env.seq,
            serde_json::to_string(&ef["event"]).unwrap()
        );
        assert!(
            verify_outbound(b"AppEvnt_", &ef, &body),
            "app_event sig must verify"
        );
        assert_eq!(ef["event"]["tag"], "TaskChanged");
        assert_eq!(ef["event"]["data"]["state"], "running");
    }

    #[tokio::test]
    async fn get_commands_returns_signed_catalog() {
        let (_bridge, addr, _db) = boot_bridge();
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", addr))
            .await
            .unwrap();
        handshake(&mut ws).await;
        ws.send(TMessage::Text(
            serde_json::json!({
                "type":"get_commands","nonce":"gc","ts":now()
            })
            .to_string(),
        ))
        .await
        .unwrap();
        let cat = recv_until(&mut ws, |m| m["type"] == "command_catalog").await;
        let body = serde_json::to_string(&cat["commands"]).unwrap();
        assert!(verify_outbound(b"CmdCatlg", &cat, &body));
    }
}
