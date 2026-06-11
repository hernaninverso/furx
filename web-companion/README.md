# Furx Web Companion (BLOQUE 8 scaffolding)

> Browser-based companion: **read-only audit replay + session replay scrubber + approve tool-calls**
> from anywhere. No terminal exec en web (sandbox no permite spawn de claude/codex/etc).

## Estado actual

- **Scaffolding** (BLOQUE 8): Next.js 14 app structure, audit sync schema, public replay link format.
- **TODO**:
  - `cd web-companion && npm install`.
  - Cloudflare Pages project `furx-web-companion`.
  - DNS `companion.furx.cloud` → Pages.
  - Audit sync endpoint on your own backend (Rust `mobile_bridge` reuse or new).

## Stack

- Next.js 14 (app router) + TypeScript.
- Cloudflare Pages deploy.
- Audit sync: WebSocket / SSE desde desktop (opt-in en Settings → Cloud Sync, Pro feature).
- Storage: your own Postgres `furx_audit_sync(install_id, events_jsonl, encrypted)`.

## Páginas

- `/` — landing → CTA "Pair with desktop" (paste install_id + 8-char code from desktop).
- `/audit/<install_id>` — audit log replay con scrubber (timeline + filter).
- `/replay/<replay_id>` — public sharable replay link (loom-style, expires 30d default).
- `/approve` — incoming approval requests (Live notifications).

## Security baseline (al implementar BLOQUE 9+)

- Cloud sync opt-in default OFF. User explicitly enables in Settings → Cloud Sync.
- Audit events encrypted client-side (libsodium) antes de POST al backend.
- El backend solo guarda ciphertext + install_id (puede borrar TODO con DELETE /audit/<id>).
- Public replay links: signed URL con expiry, NEVER expose secrets.
- F32 Guardrail (already in desktop) ensures no secret leaves the device.
- Codex+Gemini audit antes de ship.
