#!/bin/sh
# Furx bundle plugin: mcp-language-server (spec-013, Tier 2).
#
# isaacphi/mcp-language-server (BSD-3-Clause) — real diagnostics/rename/definition via
# a backing LSP. We do NOT bundle the Go binary NOR the language servers; both are
# RUNTIME DEPENDENCIES. This signed, hash-bound launcher locates `mcp-language-server`
# and starts it over the project workspace, driving a user-chosen LSP.
#
# The LSP to drive MUST be provided by the user via $MCP_LSP_COMMAND (e.g.
# "gopls", "rust-analyzer", "pyright-langserver --stdio"). Extra LSP args go in
# $MCP_LSP_ARGS. We refuse to guess an LSP (fail-closed): a wrong LSP silently
# produces no diagnostics.
#
# process.exec: the LSP is a child process this server spawns. The manifest declares
# shell:false (no arbitrary shell) — the only exec is the LSP the user named.
#
# Resolution order for the server binary:
#   1. $MCP_LANGUAGE_SERVER_BIN (explicit override)
#   2. `mcp-language-server` on PATH
#   3. known caches (~/go/bin, $GOPATH/bin, ~/.local/bin)
#
# Fail-closed: missing binary OR missing $MCP_LSP_COMMAND → JSON hint + non-zero exit.
set -eu

PROJECT_ROOT="${1:-.}"
case "$PROJECT_ROOT" in
  *[[:cntrl:]]*)
    printf '{"error":"invalid project root: control characters not allowed"}\n' >&2
    exit 4
    ;;
esac

find_bin() {
  if [ -n "${MCP_LANGUAGE_SERVER_BIN:-}" ] && [ -x "${MCP_LANGUAGE_SERVER_BIN}" ]; then
    printf '%s' "${MCP_LANGUAGE_SERVER_BIN}"; return 0
  fi
  if command -v mcp-language-server >/dev/null 2>&1; then
    command -v mcp-language-server; return 0
  fi
  for c in "${GOPATH:-$HOME/go}/bin/mcp-language-server" "$HOME/go/bin/mcp-language-server" "$HOME/.local/bin/mcp-language-server"; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

BIN="$(find_bin)" || {
  printf '{"error":"mcp-language-server binary not found","hint":"install with: go install github.com/isaacphi/mcp-language-server@latest (or set MCP_LANGUAGE_SERVER_BIN)"}\n' >&2
  exit 3
}

if [ -z "${MCP_LSP_COMMAND:-}" ]; then
  printf '{"error":"no LSP configured","hint":"set MCP_LSP_COMMAND to your language server, e.g. gopls / rust-analyzer / \\"pyright-langserver --stdio\\"; extra args in MCP_LSP_ARGS"}\n' >&2
  exit 6
fi

# argv: --workspace <root> --lsp <lsp> [-- <lsp args>]
# shellcheck disable=SC2086  # MCP_LSP_ARGS is an intentional word-split arg list
if [ -n "${MCP_LSP_ARGS:-}" ]; then
  exec "$BIN" --workspace "$PROJECT_ROOT" --lsp "$MCP_LSP_COMMAND" -- $MCP_LSP_ARGS
fi
exec "$BIN" --workspace "$PROJECT_ROOT" --lsp "$MCP_LSP_COMMAND"
