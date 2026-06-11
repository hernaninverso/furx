# Architecture

Furx is **Tauri 2 + Rust** (desktop shell, orchestration, PTY, storage) with a **React 19 + Vite 6**
front end. Everything runs locally; there is no Furx-operated backend in the request path.

```
furx/                              (repo root)
├── web/                           react 19 · vite 6 · xterm 5.5
│   └── src/
│       ├── Shell.tsx              router + state coordinator
│       ├── Terminal.tsx           xterm + portable-pty bridge
│       ├── wizard/                Furx Connect (openrouter / claude / free / paid / local / proxy)
│       ├── components/            CouncilModal, BroadcastModal, MergeReviewModal, ToastStack, …
│       ├── hooks/                 usePolling, usePaneBuffers, useToast
│       └── views/                 GrafanaView, McpHealthView, SaasView, …
├── src-tauri/                     rust + tauri 2
│   ├── tauri.conf.json
│   ├── src/
│   │   ├── lib.rs                 setup + AppState
│   │   ├── commands.rs            ~150 Tauri commands
│   │   ├── pty.rs                 portable-pty manager
│   │   └── services/              ~60 modules (council_multi, providers, memory_daemon, …)
│   └── migrations/                ordered SQLite migrations
├── web-companion/                 next.js read-only audit replay (scaffolding)
└── .github/workflows/             ci.yml, release.yml (cross-platform matrix)
```

## Storage (all local)

- `~/.furx/furx.db` — SQLite (WAL). The `events` table is **append-only**, enforced by triggers that
  block `UPDATE`/`DELETE` (tamper-evident audit log).
- `~/.furx/memory.db` — Memory Hub: FTS5 full-text index + embeddings + knowledge graph.

Provider API keys are **never** stored in these databases — they live only in the OS keychain
(macOS Keychain / Windows Credential Manager / libsecret) and are read on demand.

## Graceful degradation

Optional backends fail soft. If an AIE gateway, Ollama, or the memory daemon is unreachable, Furx keeps
running on your BYO provider keys — Council Mode dispatches to whichever providers are healthy. The
internal AIE bearer (read from the OS keychain) is optional: when absent, AIE-routed features are simply
disabled rather than blocking the app.

## Configuration & environment overrides

Furx ships with **no remote backend wired in**. Optional integrations default to local endpoints; point
them at your own infrastructure only if you run one:

- `FURX_AIE_URL` — override the `endpoints.aie` setting. Default is local (`http://127.0.0.1:8250`) or
  empty; set this to your own AIE-compatible gateway.
- `FURX_OLLAMA_URL` — embedding backend (`services/embeddings.rs`). Defaults to local Ollama
  (`http://127.0.0.1:11434`).
- `FURX_ALLOWLIST_EXTRA_HOSTS` — CSV of additional hosts trusted for outbound calls (suffix entries:
  `*.example.com`). Built-in defaults are loopback-only.
- `APPLE_SIGNING_IDENTITY` — code-signing identity for notarized macOS builds (used by CI / your build
  scripts; not baked into the repo).
