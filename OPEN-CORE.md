# Open-core boundary

Furx is **open-core**, split cleanly along one line: the **desktop client** vs the **hosted service**.

The **desktop client — everything in this repository — is [Apache-2.0](LICENSE)** and fully
reviewable: the multi-pane terminal, Council Mode, the append-only audit log, the Memory Hub, the
complete BYO-keys engine, and the license-gating code itself. You can clone, build, and run it today
and use Furx fully on your own keys, **entirely local-first — no account, no network beyond your own
providers.**

The **Furx cloud service** — the hosted back end behind the optional Pro conveniences
(cross-device sync, encrypted backups, session-replay storage) — is a hosted service and is not part
of this repository. Nothing that runs on your machine depends on it.

## Our commitments

- **The local client is fully open and local-first.** Every feature that runs on your machine —
  agents, Council, audit log, memory, keys — is in this repo, Apache-2.0, and works with no account.
- **No feature is removed from the free tier after being added.** What is free stays free.
- **The license gate is in the open.** Pro/Team/Enterprise gating is a deterministic signature check
  against a public key bundled in the binary — reviewable here, no obfuscation.
- **Pro unlocks client features that sync to the hosted service.** Those client-side features ship as
  source here (disabled without a key, not absent); the hosted service they sync to is not in this repo.
- **No telemetry of your prompts or keys, ever** — see [`SECURITY.md`](SECURITY.md).

## What is free forever (Apache-2.0)

- Multi-pane PTY grid (zsh + agent CLIs)
- Council Mode — one prompt dispatched to up to 6 models
- Append-only audit log (SQLite, UPDATE/DELETE blocked by triggers)
- Memory Hub (full-text search + knowledge graph)
- Skills registry
- Command palette, search, broadcast, smart paste, snapshots
- MCP server health + `tools/list`
- Voice → text (whisper.cpp)
- Auto-update (Ed25519-signed manifests)
- The complete **BYO-keys** engine — every provider, keys in the OS keychain

Everything a developer needs to run agents locally is here and free.

## What a license key unlocks (code visible, runtime-gated)

| Tier | Adds |
|---|---|
| **Pro** ($12/mo) | Cloud sync of `.mcp.json` + skills, session replay scrubber, Cost-Router auto-divert, cost meter, premium themes |
| **Team** ($30/seat/mo) | SSO/OIDC, admin console, shared cost dashboard, centralized audit |
| **Enterprise** ($49/seat/mo · or $2,500 perpetual self-host) | Notarized self-host build, data-residency, source-code escrow, white-label |
| **Compliance Pack** ($199 one-time) | GDPR/DPA template, encrypted backup, legal escrow audit log |

Billing is handled by [Paddle](https://paddle.com) as merchant-of-record. A 14-day Pro trial starts on
first install, no card required.

## For contributors

Please target the **free core** unless you are explicitly fixing or improving the gating logic.
Contributions that move a paid feature into the core (ungating it) won't be merged, but bug fixes and
improvements anywhere in the tree are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md).
