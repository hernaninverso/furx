# Furx Mobile Companion

> Viewer-first iOS/Android companion that pairs with the desktop Furx over a
> **local WebSocket bridge** (`127.0.0.1:43118` loopback always; `:43119` on the
> Tailscale interface, opt-in). From the phone: view live pane output (incl. a
> Claude Code session running inside a Furx pane), send signed text/voice
> commands, approve tool-calls, and receive push notifications.

Spec + tasks + review: `.specify/specs/004-mobile-companion/`.

## Status (spec 004)

- **F1 — WS bridge core** ✅ SHIPPED (`src-tauri/src/services/mobile_bridge.rs`).
  axum WebSocket server, signed `Hello` handshake, per-frame `ts-skew → HMAC
  verify → nonce-LRU` (no nonce-burn), `PtyWrite` → pane write, `ApproveToolCall`
  → cards path, 2s `PaneSnapshot` pusher. Loopback always, refuses `0.0.0.0`,
  Tailscale CGNAT (`100.64.0.0/10`) detection via getifaddrs. WS frame cap 64KB.
  `pty.rs` per-pane scrollback ring buffer (ANSI-stripped, 50 lines).
- **F4 — PWA** ✅ CORE (`mobile-companion/pwa/`). Self-contained static app served
  by the bridge (`include_str!`); pairing, pane viewer, command box, Web-Speech
  voice, Estética A. `furx-sign.js` HMAC signer cross-validated against Rust.
- **F2 — Tailscale opt-in** ✅ bridge-side (`mobile.tailscale_enabled` setting →
  second listener on the tailnet IP). Desktop toggle UI: see T4.10.
- **F3 — Notifications** ⏳ next sprint (cards + Grafana webhook + pane-ready,
  toggleable; OS/Web push). Protocol `Notification` frame already defined.
- **F5 — native app** ⏳ deferred (gated; PWA validates first).

## Architecture

```
Phone PWA (mobile-companion/pwa/, any mobile browser, installable)
  ├─ furx-sign.js — Web Crypto HMAC-SHA256, length-prefixed canonical (== Rust)
  ├─ WebSocket client → ws://<host>/ws
  ├─ Web Speech API (on-device transcription; only text leaves the phone)
  └─ Pairing: paste 64-hex secret from desktop Settings → Mobile

  ──── WebSocket over loopback / Tailscale WireGuard ────►

Desktop Furx
  ├─ services/mobile_bridge.rs — axum WS server (loopback + opt-in Tailscale)
  │   ├─ Hello handshake (signed, 10s timeout) → HelloAck (pane list)
  │   ├─ per-frame ts-skew(60s) → HMAC verify → nonce-LRU(8192)
  │   ├─ PtyWrite → PtyManager::write(focused pane)
  │   ├─ ApproveToolCall → cards decision (same contract as telegram_inbound)
  │   └─ 2s PaneSnapshot push (scrollback ring buffer, pane-state badge)
  └─ pty.rs scrollback ring buffer fed from the reader loop
```

## Security baseline (implemented)

- Loopback always; Tailscale interface opt-in. NEVER `0.0.0.0` (hard guard + test).
- Per-message HMAC-SHA256 (length-prefixed canonical, constant-time compare).
- Replay protection: 60s ts-skew window + LRU nonce dedup; verify BEFORE nonce
  insert (no nonce-burn). WS frame size capped at 64KB.
- Shared secret lives in the OS Keychain (`furx-mobile/shared-secret`), never
  logged nor serialized. Audit logs command length + source, never the text body.

## Next steps

1. Build & run the desktop app; open `http://<tailscale-ip>:43118/` on the phone.
2. Settings → Mobile (T4.10) to show/rotate the secret + QR + Tailscale toggle.
3. F3 notifications (Grafana contact point → `/furx/v1/grafana` HMAC webhook).
4. Apple Developer + TestFlight when promoting the PWA to a native shell (F5).
