# Furx — Brand & Copy (single source of truth)

> Every public surface — README, `src-tauri/tauri.conf.json`, furx.cloud, the landing page,
> in-app copy, legal pages, store listings — derives its copy from this file.
> **If a surface disagrees with this file, the surface is wrong.**

## Core identity

| Field | Canonical value |
|---|---|
| Product name | `Furx` (capital F only; pronounced *"Furx, rhymes with works"*) |
| Tagline (the one) | `Run any coding agent side-by-side. No proxy.` |
| Descriptor (one-liner) | `A local-first desktop app for running coding agents side by side.` |
| Company | `INVERSO HUB S.R.L.` — appears in the imprint and copyright lines only; everywhere else the brand is "Furx" |
| Domain | `furx.cloud` |
| Core license | `Apache-2.0` (never MIT) |
| Billing | Paddle (Merchant of Record) |
| Platforms | macOS · Linux · Windows |
| Language | All public copy is English. The app UI ships English by default with Spanish as an optional locale. |

## Positioning paragraph (canonical)

> Furx is a local-first desktop app that runs real coding agents — Claude Code, Codex,
> Gemini CLI, Aider, or any OpenAI-compatible endpoint — side by side in a grid of terminal
> panes. Council Mode (⌘J) sends one prompt to up to six models in parallel and synthesizes
> a consensus you can inspect. Your API keys live in your OS keychain and every call goes
> straight from your machine to the provider — no proxy — with an append-only audit log the
> agent can't rewrite. The core is Apache-2.0: read the source.

## Three pillars ("Why Furx")

1. **Council fan-out** — one prompt → up to six models in parallel, with a synthesized
   consensus you can inspect. Not chat tabs: competing answers on the same problem.
2. **Real agents, not chat** — Furx runs the actual CLIs in real terminals that read your
   repo, edit files, and run tests. Comparison tools show you chat; Furx shows you work.
3. **Local & verifiable** — keys in the OS keychain, calls go direct to the provider,
   and an append-only audit log records what every agent did.

## Trust lines (verbatim — never paraphrase)

- `Your keys never leave your machine. No proxy.`
- `An audit trail the agent can't rewrite.`
- `The core is Apache-2.0 — read the source.`

## Voice & tone

Engineer-to-engineer. Present tense. Verbs over adjectives. Numbers over superlatives.
Every claim must be checkable in the source or falsifiable by a user. Never anthropomorphize
the agents. Second person ("your machine", "your keys").

### Ban list (never in public copy)

`AI-powered` · `supercharge` · `10x` · `seamless` · `revolutionize` · `unleash` ·
`game-changer` · `solo-founder` · `command center` as a headline · stacks of three adjectives ·
first-person founder voice or any personal reference.

### Kill list (legacy copy — purge on sight)

`BYO LLM Engine` · `One window. Many coding agents.` · `Any LLM. One memory. Your keys.` ·
`Solo-founder command center` · `soporte@` · `MIT` as the project license · `"4 CLIs"` as a phrase.

## Feature framing (canonical phrasing)

- **Agents:** "Claude Code, Codex, Gemini CLI, Aider — and any OpenAI-compatible endpoint".
  Never "4 CLIs" (the number ages badly). Always "coding agents", never "chatbots"/"AIs".
- **Council Mode (⌘J):** "sends one prompt to up to six models in parallel and synthesizes
  a consensus". Five presets — quick, cheapo, frontier, local, mix — plus templates, custom
  voices, a live cost estimator, and history. The synthesis is *inspectable*, never oracular.
- **BYOK:** 15 providers — OpenRouter, Anthropic, OpenAI, Gemini API, Google Gemini
  AI Studio, Groq, Cerebras, Mistral, SambaNova, Ollama, LM Studio, llama.cpp, vLLM,
  LiteLLM, and custom OpenAI-compatible endpoints. Keys are stored only in the OS
  keychain — never written to disk, never sent to a Furx server. Furx is never in the
  request path.
- **Audit log:** append-only SQLite; database triggers block `UPDATE` and `DELETE`.
  Frame it adversarially ("the agent can't rewrite it"), never as "we log things".
  Never claim "tamper-proof" — the claim is *append-only* and *tamper-evident*.
- **Memory Hub:** local JSON-RPC daemon, full-text search (FTS5) with embedding re-rank,
  shared across CLIs, namespaced per project. It is a sidecar — it never sits between an
  agent and a provider.
- **Also shipped:** Broadcast (⌘B), command palette (⌘K) + search, skills registry
  (`~/.furx/skills` + `~/.claude/skills`), voice-to-text (whisper.cpp), MCP plugin host
  (signed plugins), Ed25519-signed auto-update, iOS companion (read-only, in scaffolding).
- **Hotkey casing:** always `⌘J`, `⌘B`, `⌘K`.

## Open-core & pricing (facts)

The core is Apache-2.0 and free forever; commercial features are gated by a runtime license
key whose check is visible in the source (`OPEN-CORE.md` defines the exact boundary).
Pro $12/mo · Team $30/seat/mo · Enterprise $49/seat/mo or $2.5k self-host · Compliance Pack
$199 one-time. 14-day Pro trial on first install, no card. Billing is handled by Paddle as
Merchant of Record.

## Email handles (all `@furx.cloud`)

`hello@` (general) · `support@` · `security@` · `legal@` · `dpo@` · `sales@`.
`soporte@` is dead — replace on sight.

## Competitive one-liner (for copy that needs contrast)

> Worktree orchestrators parallelize *tasks*. Model comparators put one prompt before
> N models — but only as chat. Furx is the only tool that runs one prompt across many
> models *with real coding agents in real terminals*, provider-agnostic.

Name competitors only in private docs, never in public copy.
