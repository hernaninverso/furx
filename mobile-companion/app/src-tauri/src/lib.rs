// Furx Companion (native iOS/Android shell) — spec 004 F5 / T5.4.
//
// A Tauri webview is a secure context, which blocks plaintext `ws://` (mixed
// content). The desktop bridge is plaintext WS (council MC-2: encryption from
// loopback / Tailscale WireGuard). So the WebSocket lives HERE in Rust (no
// browser mixed-content rule), and the webview drives it over Tauri IPC:
//   - `ws_connect(url)`  → open the socket; incoming frames are emitted to the
//                          webview as `furx:ws-message` events.
//   - `ws_send(text)`    → send a JSON frame (signed by the webview's pure-JS HMAC).
//   - `ws_disconnect()`  → close. `furx:ws-closed` fires when the socket ends.
//
// The webview keeps ALL the UI + the pure-JS HMAC signer; only transport is here.

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Default)]
struct WsState {
    /// Outgoing-frame sender for the live connection. Setting it to None drops
    /// the sender, which ends the connection task's outgoing loop → graceful close.
    tx: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Handle to the live connection task, so a reconnect / disconnect can abort
    /// the previous one deterministically (no transient double connection — 4-
    /// frontier review).
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Connection GENERATION (audit-3 del handoff pairing→sesión). UN socket global
    /// + eventos globales (`furx:ws-message`/`ws-closed`) creaban una CARRERA: al
    /// canjear el QR, el `ws_disconnect` del socket de pairing podía emitir un
    /// `ws-closed` ASÍNCRONO que llegaba DESPUÉS de que la sesión registró su
    /// `onClose` → la sesión se caía apenas conectaba (parea→parpadea→vuelve a
    /// pareo, sin panes). Cada connect/disconnect incrementa la generación; la
    /// task SOLO emite si su generación sigue siendo la actual → un socket viejo
    /// NUNCA emite un evento que la conexión nueva confunda como propio.
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// The bridge ONLY binds loopback + the Tailscale interface (100.64.0.0/10) —
/// never the LAN or a public IP. So the native client only ever connects to one
/// of those. Enforcing it here matches the bridge's bind policy, rejects an
/// arbitrary/public host, and rejects malformed URLs (4-frontier review).
fn ws_url_allowed(url: &str) -> bool {
    let rest = match url.strip_prefix("ws://") {
        Some(r) => r,
        None => return false,
    };
    let hostport = rest.split('/').next().unwrap_or("");
    // Split host from port, handling bracketed IPv6 (`[::1]:43118`).
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("")
    } else {
        hostport.rsplit_once(':').map(|(h, _)| h).unwrap_or(hostport)
    };
    if host == "127.0.0.1" || host == "::1" || host == "localhost" {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        return o[0] == 100 && (64..=127).contains(&o[1]); // Tailscale CGNAT /10
    }
    false
}

#[derive(serde::Serialize, Clone)]
struct WsPayload {
    gen: u64,
    data: String,
}

#[tauri::command]
async fn ws_connect(app: AppHandle, state: State<'_, WsState>, url: String) -> Result<u64, String> {
    if !ws_url_allowed(&url) {
        return Err("host not allowed: only ws:// to loopback or the Tailscale range (100.64.0.0/10)".into());
    }
    use std::sync::atomic::Ordering;
    // Nueva generación para ESTA conexión. Suprime los eventos de cualquier socket
    // viejo (el de pairing) que pudiera emitir tras este punto.
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let gen = state.generation.clone();

    // Abort the previous task + drop its sender so exactly one connection lives.
    if let Some(h) = state.task.lock().take() {
        h.abort();
    }
    *state.tx.lock() = None;

    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut write, mut read) = ws_stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *state.tx.lock() = Some(tx);

    let app2 = app.clone();
    let handle = tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                outgoing = rx.recv() => {
                    match outgoing {
                        Some(text) => {
                            if write.send(Message::Text(text)).await.is_err() { break; }
                        }
                        None => break, // sender dropped → ws_disconnect / new connect
                    }
                }
                incoming = read.next() => {
                    match incoming {
                        // SOLO emitir si seguimos siendo la conexión actual (no un socket viejo).
                        Some(Ok(Message::Text(t))) => {
                            if gen.load(Ordering::SeqCst) == my_gen {
                                let _ = app2.emit("furx:ws-message", WsPayload { gen: my_gen, data: t });
                            }
                        }
                        Some(Ok(Message::Ping(_))) => { /* tungstenite auto-pongs */ }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {}
                        Some(Err(_)) => break,
                    }
                }
            }
        }
        let _ = write.close().await;
        // Emitir ws-closed SOLO si esta task sigue siendo la conexión actual. Si fue
        // reemplazada (nuevo connect) o cerrada intencionalmente (ws_disconnect, que
        // incrementa la generación), su cierre NO debe disparar el onClose de la
        // conexión nueva → mata la carrera del handoff pairing→sesión.
        if gen.load(Ordering::SeqCst) == my_gen {
            let _ = app2.emit("furx:ws-closed", my_gen);
        }
    });
    *state.task.lock() = Some(handle);
    Ok(my_gen)
}

#[tauri::command]
fn ws_send(state: State<'_, WsState>, text: String) -> Result<(), String> {
    match state.tx.lock().as_ref() {
        Some(tx) => tx.send(text).map_err(|_| "connection closed".to_string()),
        None => Err("not connected".into()),
    }
}

#[tauri::command]
fn ws_disconnect(state: State<'_, WsState>) {
    // Incrementar la generación PRIMERO: invalida los eventos del socket que estamos
    // cerrando (incluido un `ws-closed` que la task pueda emitir en la carrera con el
    // abort) → la PRÓXIMA conexión (la sesión, tras el pairing) no los confunde como propios.
    state
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // Drop the sender (graceful close in the task), then abort as a safety in
    // case the task is parked on a read.
    *state.tx.lock() = None;
    if let Some(h) = state.task.lock().take() {
        h.abort();
    }
}

// ── T5.5 native pairing-secret storage in the iOS Keychain ───────────────────
// The browser PWA uses localStorage; the native app uses the OS Keychain, so the
// secret survives app reinstalls less and isn't readable by other web origins.
const KEYCHAIN_SERVICE: &str = "furx-companion";
const KEYCHAIN_ACCOUNT: &str = "pairing-secret";

#[tauri::command]
fn secret_store(value: String) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("secret must be 64 hex chars".into());
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .and_then(|e| e.set_password(&value))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn secret_load() -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .ok()?
        .get_password()
        .ok()
        .filter(|s| !s.is_empty())
}

#[tauri::command]
fn secret_clear() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already absent — idempotent
        Err(e) => Err(e.to_string()),
    }
}

// ── 065 — pairing por short-code: POST /pair-shortcode al bridge DESDE RUST NATIVO.
// El webview NO puede hacer este fetch HTTP plaintext a una IP local (ATS lo bloquea); desde Rust
// nativo no aplica ATS. Misma policy de host que `ws_url_allowed` (loopback o Tailscale CGNAT).

fn host_is_allowed(host: &str) -> bool {
    if host == "127.0.0.1" || host == "::1" || host == "localhost" {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let o = ip.octets();
        return o[0] == 100 && (64..=127).contains(&o[1]); // Tailscale CGNAT /10
    }
    false
}

#[derive(serde::Serialize)]
struct ShortcodeResult {
    token: String,
    port: u16,
}

async fn http_post_shortcode(host: &str, port: u16, code: &str) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = format!("{{\"code\":\"{code}\"}}");
    let req = format!(
        "POST /pair-shortcode HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let connect = tokio::net::TcpStream::connect((host, port));
    let mut stream = match tokio::time::timeout(std::time::Duration::from_secs(3), connect).await {
        Ok(Ok(s)) => s,
        _ => return None,
    };
    if stream.write_all(req.as_bytes()).await.is_err() {
        return None;
    }
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    let resp = String::from_utf8_lossy(&buf);
    if !(resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200")) {
        return None;
    }
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from))
}

/// Resuelve el token efímero desde un short-code de 8 chars vía el bridge. Prueba :43119 (Tailscale) y
/// :43118 (loopback); devuelve {token, port} para que el webview canjee por WS en el MISMO puerto.
#[tauri::command]
async fn pair_shortcode_token(host: String, code: String) -> Result<ShortcodeResult, String> {
    if !host_is_allowed(&host) {
        return Err("host not allowed".into());
    }
    let code = code.trim().to_uppercase();
    if code.len() != 8 || !code.bytes().all(|b| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&b)) {
        return Err("invalid code".into());
    }
    for port in [43119u16, 43118] {
        if let Some(token) = http_post_shortcode(&host, port, &code).await {
            return Ok(ShortcodeResult { token, port });
        }
    }
    Err("short_code_not_found".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(WsState::default())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_voice::init())
        .setup(|app| {
            // 065 — escáner de QR nativo (mobile-only): cámara nativa + prompt de permiso real.
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_barcode_scanner::init())?;
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ws_connect,
            ws_send,
            ws_disconnect,
            secret_store,
            secret_load,
            secret_clear,
            pair_shortcode_token
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::ws_url_allowed;

    #[test]
    fn url_allow_policy() {
        assert!(ws_url_allowed("ws://127.0.0.1:43118/ws"));
        assert!(ws_url_allowed("ws://localhost:43118/ws"));
        assert!(ws_url_allowed("ws://[::1]:43118/ws"));
        assert!(ws_url_allowed("ws://100.64.0.1:43119/ws"));   // Tailscale CGNAT
        assert!(ws_url_allowed("ws://100.127.255.254:43119/ws"));
        assert!(!ws_url_allowed("ws://100.63.0.1/ws"));        // below CGNAT
        assert!(!ws_url_allowed("ws://100.128.0.1/ws"));       // above CGNAT
        assert!(!ws_url_allowed("ws://192.168.1.5:43118/ws")); // LAN — bridge never binds it
        assert!(!ws_url_allowed("ws://evil.com/ws"));          // arbitrary host
        assert!(!ws_url_allowed("wss://127.0.0.1/ws"));        // wss not supported
        assert!(!ws_url_allowed("http://127.0.0.1/ws"));       // wrong scheme
    }
}
