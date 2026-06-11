# Features

Full reference of what Furx ships. Everything below is **free** (Apache-2.0) unless marked otherwise;
the open-core boundary is in [`../OPEN-CORE.md`](../OPEN-CORE.md).

| Feature | Status |
|---|---|
| 4-pane PTY grid (zsh + 3 CLIs, mix and match) | ✅ |
| `claude-as-*` wrappers per Claude Code account (slot A/B/…) | ✅ |
| Audit log (SQLite append-only, triggers block UPDATE/DELETE) | ✅ |
| Shared cards rail (incidents, monitors, snapshots) | ✅ |
| ⌘P search (code / memories / git) | ✅ |
| ⌘K palette (actions / search / projects) | ✅ |
| ⌘B broadcast to multiple Claude panes | ✅ |
| ⌘J Council Mode (BYOK dispatch, 5 presets) | ✅ |
| ⌘⇧V smart paste (clipboard classifier) | ✅ |
| ⌘⇧S manual snapshot | ✅ |
| Per-pane Claude token meter (reads `~/.claude/projects/.../usage.json`) | ✅ |
| MCP server health + `tools/list` handshake | ✅ |
| Grafana embed with allowlist + 60s heartbeat | ✅ |
| SSH quick-connect from `~/.ssh/config` | ✅ |
| Voice → text via whisper.cpp + streaming download | ✅ |
| Memory Hub daemon (JSON-RPC + FTS5 + knowledge graph) | ✅ |
| Skills registry (`~/.furx/skills` + `~/.claude/skills`) | ✅ |
| Crash capture (panic + JS error, rotated + PII-scrub) | ✅ |
| Auto-update (Ed25519-signed manifest, Tauri updater) | ✅ |
| Cloud sync of `.mcp.json` + skills | 🔑 Pro |
| Cost-Router auto-divert + cost meter | 🔑 Pro |
| SSO/OIDC, admin console, shared cost dashboard | 🔑 Team |
| Mobile bridge mDNS advertising (companion incoming) | 🚧 |
| iOS companion app | 🚧 scaffolding |
| Web companion (read-only audit replay) | 🚧 scaffolding |

🔑 = gated by a runtime license key (code is in the repo; see [`../OPEN-CORE.md`](../OPEN-CORE.md)).
🚧 = in progress; see [`../ROADMAP.md`](../ROADMAP.md).

Quality bar: 1279 cargo lib tests, 0 warnings, 95 Vite modules.
