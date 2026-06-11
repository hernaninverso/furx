#!/bin/sh
# Furx bundle plugin: serena (spec-013, Tier 1).
#
# Serena (oraios/serena, MIT) is a semantic coding toolkit exposed as an MCP server
# over an LSP. We do NOT bundle Serena's runtime (Python + per-language LSPs) — it is
# a RUNTIME DEPENDENCY the user provides (`uv tool install -p 3.13 serena-agent`).
# This signed, hash-bound launcher locates the real `serena` binary and starts it as
# an stdio MCP server scoped to the project root.
#
# Resolution order (first match wins):
#   1. $SERENA_BIN (explicit override, must be executable)
#   2. `serena` on PATH
#   3. `uvx --from serena-agent serena` (ephemeral run via uv, if uvx is present)
#   4. known install caches (~/.local/bin)
#
# The project root is passed as $1 by the Furx runtime (it expands $PROJECT_ROOT in
# the signed manifest's mcp.args). net/secrets are NOT used by this launcher: Serena
# is local-only (LSP over the repo) → the manifest declares net:false, secrets:[].
#
# Fail-closed: if no Serena runtime is found we print a JSON hint and exit non-zero
# rather than silently doing nothing.
set -eu

PROJECT_ROOT="${1:-.}"

# Reject control chars in the project root (defense-in-depth; a real path never has them).
case "$PROJECT_ROOT" in
  *[[:cntrl:]]*)
    printf '{"error":"invalid project root: control characters not allowed"}\n' >&2
    exit 4
    ;;
esac

run_serena() {
  # $1 = launcher prog, rest = its leading args; then the serena subcommand.
  exec "$@" start-mcp-server --context=claude-code --project "$PROJECT_ROOT"
}

if [ -n "${SERENA_BIN:-}" ] && [ -x "${SERENA_BIN}" ]; then
  run_serena "${SERENA_BIN}"
fi
if command -v serena >/dev/null 2>&1; then
  run_serena "$(command -v serena)"
fi
if command -v uvx >/dev/null 2>&1; then
  # uvx fetches serena-agent into an ephemeral env and runs the `serena` entrypoint.
  exec uvx --from serena-agent serena start-mcp-server --context=claude-code --project "$PROJECT_ROOT"
fi
for c in "$HOME/.local/bin/serena"; do
  [ -x "$c" ] && run_serena "$c"
done

printf '{"error":"serena runtime not found","hint":"install with: uv tool install -p 3.13 serena-agent (or set SERENA_BIN)"}\n' >&2
exit 3
