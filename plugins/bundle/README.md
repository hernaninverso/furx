# Furx bundle plugins — MCP set (spec-013)

Signed (Ed25519, pinned `TRUSTED_PUBKEYS`) plugin manifests + hash-bound `run.sh`
launchers for the recommended MCP servers. Each plugin is **opt-in per agent** (006
allow-list, default-deny) and **runtime-dependency only**: we do NOT vendor any
third-party binary — the signed `run.sh` *locates* the real binary (env override →
PATH → install cache) and fails closed with a JSON hint if it's absent (the 011
pattern). The signature covers the launcher (`entrypoint_sha256`), so the bytes Furx
runs are exactly the bytes the Furx key signed.

## License verification (P0 / T001) — all permissive, none discarded

| Plugin | Upstream | License | Stdio launch | Net | Secret (BYOK) |
|---|---|---|---|---|---|
| `serena` | oraios/serena | **MIT** | `serena start-mcp-server --context=claude-code --project <root>` | none | — |
| `github-mcp` | github/github-mcp-server | **MIT** | `github-mcp-server stdio` | `api.github.com` | `GITHUB_PERSONAL_ACCESS_TOKEN` |
| `codanna` | bartolli/codanna | **Apache-2.0** | `codanna serve --watch` | none | — |
| `test-coverage` | goldbergyoni/test-coverage-mcp | **MIT** | `npx -y test-coverage-mcp` | none¹ | — |
| `mcp-language-server` | isaacphi/mcp-language-server | **BSD-3-Clause** | `mcp-language-server --workspace <root> --lsp <lsp>` | none | — |
| `git-mcp` | cyanheads/git-mcp-server | **Apache-2.0** | `npx -y @cyanheads/git-mcp-server` | none¹ | — |
| `context7` | upstash/context7 | **MIT** | `npx -y @upstash/context7-mcp` | `context7.com`, `mcp.context7.com` | `CONTEXT7_API_KEY` (optional) |

**No plugin was discarded for license** — every bundled server is MIT / Apache-2.0
/ BSD-3-Clause (all permissive). Furx ships them as signed, hash-bound `run.sh`
launchers (no vendored binaries). The first-party `codebase-memory` plugin (the
reference signed MCP) is documented in `docs/mcp-parity.md`.

¹ `test-coverage` and `git-mcp` are **offline at run time** (read local LCOV / drive the
local `git`), but the `npx` *fallback* may hit the npm registry on the FIRST run to
populate the npx cache. That is a one-time install fetch, not the plugin's operating
network use. To keep them strictly `net:[]` at run time, pre-install
(`npm i -g …`) and point `TEST_COVERAGE_MCP_BIN` / `GIT_MCP_SERVER_BIN` at the binary.

## Runtime-dependency gaps (documented, not silently skipped)

None of the upstream binaries are present in the build/dev environment, so **all seven
ship as runtime-deps** via their launcher (the 011 pattern). The launcher resolves, in
order: `$<NAME>_BIN` override → `command -v` on PATH → known install caches → (for
node/python packages) `npx`/`python3 -m`. If nothing is found, the launcher prints a
JSON `{"error":…, "hint":"install with: …"}` to stderr and exits non-zero (fail-closed).

Install hints per plugin (also embedded in each `run.sh`):

- **serena**: `uv tool install -p 3.13 serena-agent` (or `SERENA_BIN`)
- **github-mcp**: download `github-mcp-server` release / `go install` (or `GITHUB_MCP_BIN`)
- **codanna**: `cargo install codanna` (or `CODANNA_BIN`)
- **test-coverage**: `npm i -g test-coverage-mcp` / npx (or `TEST_COVERAGE_MCP_BIN`)
- **mcp-language-server**: `go install github.com/isaacphi/mcp-language-server@latest` + an LSP via `$MCP_LSP_COMMAND` (or `MCP_LANGUAGE_SERVER_BIN`)
- **git-mcp**: `npm i -g @cyanheads/git-mcp-server` / npx (or `GIT_MCP_SERVER_BIN`)
- **context7**: `npx @upstash/context7-mcp` (or `CONTEXT7_MCP_BIN`); BYOK `CONTEXT7_API_KEY`

## Security contract (FR-002, default-deny)

- **BYOK (F-I)**: tokens live ONLY in the OS Keychain, granted per-plugin via the
  existing secret-grant (spec-003). `github-mcp` → `GITHUB_PERSONAL_ACCESS_TOKEN`;
  `context7` → `CONTEXT7_API_KEY`. Never hardcoded, never on argv, never to disk/logs.
- **net default-deny**: offline plugins declare `net:[]`. `github-mcp` is scoped to
  `api.github.com` only; `context7` to its own hosts only. **No plugin uses `net:["*"]`.**
- **shell:false** everywhere — the launcher execs only its pinned upstream binary (or
  the user-named LSP for `mcp-language-server`); no arbitrary shell.
- **Roots/readonly (T030)**: the Plugin Host now supports a structured `fs_roots`
  allowlist with a per-path `readonly` flag (from the official filesystem MCP). It is
  back-compat (skip-serialized when empty → existing signatures stay valid).

## Re-signing

After editing a manifest or `run.sh`, re-sign so `entrypoint_sha256` + signature match:

```sh
KEY=$(security find-generic-password -a "$USER" -s furx-plugin-signing-key -w)
cd src-tauri && FURX_SIGN_KEY="$KEY" cargo run --quiet --example furx_sign -- sign ../plugins/bundle/<name>
```

## Límites conocidos v1 (audit codex+deepseek 013)

- **net/fs de un MCP server = declarado + auditado, no enforced por sandbox.** El sandbox de red/fs (proxy per-host, net-deny) se aplica en el path **per-tool** (`run_tool`, specs 001-004). Un MCP server lo lanza el CLI del agente (claude/codex), no `run_tool`, así que para esos servers `net:[...]`/`fs_*`/`fs_roots` son **declaración auditada cubierta por la firma**, no un sandbox de syscalls. El enforcement real de syscalls es el path WASM (Fase 2/3). La UI lo presenta así (no como "offline/sandbox").
- **Roots/readonly (`fs_roots`) = modelo + API + containment léxico, sin enforcement de syscalls aún.** `allows_read`/`allows_write` son predicados puros, fail-closed ante `..`. La resolución de **symlinks** es responsabilidad del enforcement-layer futuro (debe pasar un path ya `canonicalize`d; el param se llama `resolved_path`).
- **Trust del binario runtime-dep (PATH).** Los launchers localizan el binario real vía `$<NAME>_BIN` → PATH → (npx gated). Un binario malicioso antes en PATH podría recibir los secrets del plugin (ej `github-mcp` con el PAT). Mitigación: preferir `$<NAME>_BIN` con ruta absoluta. No hay pin de versión/hash del binario upstream de terceros (no estaban disponibles para hashear) — gap conocido.
- **`npx` auto-install gated.** Plugins que caen a `npx` (test-coverage/git-mcp/context7) NO lo hacen por defecto: requieren `FURX_ALLOW_NPX_INSTALL=1` (npx pega al registry npm, fuera de la net allow-list declarada). Sin el flag → fail-closed con hint de pre-instalación.
