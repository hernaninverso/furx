# Furx — Plugin System (MCP) & TTS "Read aloud"

Implemented across spec-kit features **001–004** (`specs/00{1..4}-*`). This doc is the
single reference for the architecture, the security model, and how to add a plugin.

> Constitution alignment: BYOK pure (keys never leave the client / never reach the
> Furx backend), local-first, fail-closed. See `.specify/memory/constitution.md`.

---

## 1. TTS "Read aloud" (US1, spec-001/T032)

Reads a pane's **finished** output aloud using the **local OS speech engine** —
macOS `say`, Windows SAPI (PowerShell `System.Speech`), Linux `spd-say`/`espeak`.
**No network, ever.**

- `src-tauri/src/services/tts.rs` — `TtsEngine` (per-OS backend), single
  **speaking-pane mutex** (one pane speaks at a time), `summarize()` (drops fenced
  code/diffs/logs, caps length) and `redact_secrets()` (strips `sk-`/`ghp_`/JWT/PEM/
  `key=value`/long-hex **before any speak** — never read a token aloud).
- Lifecycle: a watcher task owns the child and clears the slot on natural exit OR
  kill (generation-guarded), so auto-read keeps working after the first read.
- Commands: `tts_speak{summarize,preempt}`, `tts_stop`, `tts_available`,
  `tts_speaking_pane`.
- UI (`web/`): "Read aloud — última respuesta" + "Stop reading" palette actions;
  **voice-interrupt** (starting STT calls `tts_stop`); **auto-read-on-idle**
  per-pane toggle (default OFF, persisted) that reads the heuristic summary when a
  pane's buffer is stable across a ~1.2s debounce (mutex drops it if another pane
  is speaking).
- LLM-based summary is a deliberate **future opt-in** (council: heuristic + local is
  the privacy-correct default).

### Push-to-talk (US, spec-005)
**Hold ⌥Space** (Option+Space) → record while held → release → transcribe (Whisper
local) → write to the pane that was focused **at start**. No modal, no fixed 5s.

- `services/voice.rs`: `ptt_start` (spawn `sox` to a temp WAV, registry by id) /
  `ptt_stop` (SIGTERM via `kill -TERM` so the WAV header flushes — never SIGKILL —
  then `wait()` to reap → no zombie) / `ptt_cancel`. 60s watchdog for a lost keyup
  (terminates + deletes the temp). Commands `voice_ptt_start/stop/cancel`.
- `web/Shell.tsx`: ⌥Space keydown→`tts_stop`(voice-interrupt)+`ptt_start` (synchronous
  guard against key-repeat; captures the focused pane at start); keyup→stop+transcribe+
  `pty_write`; **Esc / blur / tab-hidden → cancel** (discard). "🎙 grabando" indicator.
  `voice_transcribe` deletes the temp WAV in all cases (success or failure).
- MVP is **focused-only** (no accessibility perms); a global hotkey + configurable key
  are a documented future. Unix-first (sox).

---

## 2. Plugin Host (MCP) — security model

`src-tauri/src/services/plugin_host.rs`. Plugins live in `~/.furx/plugins/<name>/`
and run **out of the main process**.

### Manifest (signed)
`manifest.json` → `SignedManifest`: `name`, `version`, `entrypoint`,
`entrypoint_sha256`, `permissions`, `signature`, `pubkey`.

- **Signature**: Ed25519. The `pubkey` must be in the binary-pinned `TRUSTED_PUBKEYS`
  (an attacker-supplied key is rejected — no embedded-key trust). Signing key in
  Keychain `furx-plugin-signing-key`; sign with `cargo run --example furx_sign`.
- **Content binding**: `entrypoint_sha256` is required and re-checked immediately
  before exec → the signature binds the exact binary (closes the TOCTOU; the
  installed dir is also hardened read-only).

### Permissions (default-DENY)
`Permissions { net, fs_read, fs_write, shell, secrets }` — empty = no access.

- **Network** (spec-004): `net:["*"]` = full network; `net:[hosts]` = per-host
  allowlist enforced by a **local CONNECT egress proxy** (`net_proxy.rs`) + a
  **sandbox that allows only loopback:proxy-port** (macOS `sandbox-exec`); the plugin
  *cannot bypass* the proxy. The proxy validates the host against the signed
  allowlist, **resolves DNS host-side** (no rebinding), **blocks internal/SSRF
  ranges** (loopback/RFC1918/link-local/metadata/CGNAT/IPv6 ULA+mapped), is **HTTPS
  CONNECT (:443) only**, and is **token-authenticated** (other local processes get
  407). `net:[]` = net-deny sandbox (sandbox-exec/firejail/`unshare -n`,
  **fail-closed** if none). Per-host on non-macOS is **fail-closed in v1**.
- **Secrets / BYOK** (spec-003): a secret crosses into the (clean) subprocess env
  **only if** the manifest declares it AND the user granted it. The grant maps the
  secret name → a Keychain reference `{service, account}`; the **value** is read at
  invoke time and **never persisted or logged** (names only).
- `fs_read`/`fs_write`/`shell` are declared + audited but **not** fully enforced by
  the subprocess runtime (true isolation = the WASM runtime); documented honestly.

### Runtimes
- **Subprocess** (`tokio::process`) for CLIs/MCP servers — clean env, cwd pinned,
  timeout + kill, OS net sandbox per policy above.
- **WASM** (`runtime_wasm.rs`, feature `wasm-runtime`, off by default per YAGNI):
  empty linker → **no host imports** → no network/fs/syscalls by construction.
  Recommended for untrusted third-party logic.

### Consent (ask-on-first-use) & kill switch
Per `(name, version)` consent in `~/.furx/plugin-grants.json` (version bump
re-prompts). `plugin_invoke` is default-deny until granted. **Revoking** consent
also drops the plugin's secret grants. All grant files are `chmod 600`; grant/revoke
are serialized by a process lock.

### Audit
Every install / invoke / reject / grant / secret-miss is written append-only via the
`audit` base — **names only, never secret values**.

---

## 3. Install flow & marketplace (spec-002)

- **Bundle**: 12 signed read-only plugins ship in `plugins/bundle/` (filesystem-ls,
  git-log/status/diff, ripgrep-search, dir-tree, word-count, find-files, disk-usage,
  date-now, env-info, http-get). Declared as a Tauri resource (`plugins-bundle`).
- **Install** (`plugin_install_bundled` → `install_bundled_to`): copy to a private
  `.staging-<uuid>` → verify signature + entrypoint hash there → **atomic rename**
  to the live path → harden read-only. Any failure removes staging; the live path is
  untouched (a rejected re-install keeps the prior valid one). Re-install resets
  consent + secret grants (no grant inheritance by name collision). Symlinks rejected.
- **Registry / marketplace**: the hosted registry API serves `GET /v1/registry`
  (`{version, pubkey, bundle[], catalog[]}`) with the signed bundle + a curated catalog;
  the desktop `PluginsView` installs, grants, manages secrets, and invokes tools.
  Bundled plugins also ship in `plugins/bundle/` with signed manifests.
- **CI**: `.github/workflows/plugin-security.yml` — cargo-audit + plugin_host tests +
  bundle signature/hash gate + SBOM.

---

## 4. Adding a plugin

1. `mkdir plugins/bundle/<name>` with `run.sh` (argv: `$1`=tool `$2`=args_json,
   prints JSON to stdout) + `manifest.json` (name == dir, declare minimal perms).
2. Sign: `FURX_SIGN_KEY=$(security find-generic-password -a "$USER" -s furx-plugin-signing-key -w) \
   cargo run --example furx_sign -- sign plugins/bundle/<name>`.
3. Submit it for inclusion in the hosted registry bundle (see CONTRIBUTING.md).
4. `cargo test --lib plugin_host` (the `all_bundle_plugins_verify` test checks every
   manifest against the pinned key).

---

## 5. Security audits

Each feature was reviewed by a frontier panel (**codex + gemini + 3 frontier models**)
before merge; ~24 real findings were fixed (signature/consent bypass, BYOK leaks,
TOCTOU, fail-open sandbox, grant hijack, races, unauthenticated proxy, tunnel
lifecycle). See each `specs/00N-*/` and the commit history.

**Known residuals (documented, accepted):** data-exfil via a subdomain of an allowed
host (no DPI); `fs/shell` not enforced in the subprocess runtime (use WASM);
per-host net + read-only hardening are macOS/Unix-first (Windows fail-closed for
per-host net in v1).
