#!/bin/sh
# Furx bundle plugin: github-mcp (spec-013, Tier 1).
#
# github/github-mcp-server (MIT) — PR/issue/repo tools over the GitHub API. We do NOT
# bundle the Go binary; it is a RUNTIME DEPENDENCY the user provides (download the
# release binary, `go install`, or set GITHUB_MCP_BIN). This signed, hash-bound
# launcher locates the real `github-mcp-server` and starts it in stdio mode.
#
# BYOK (F-I): the GitHub token is NEVER hardcoded. The Furx Plugin Host injects
# $GITHUB_PERSONAL_ACCESS_TOKEN into this process's env ONLY if the user granted that
# secret from the OS Keychain (manifest declares secrets:["GITHUB_PERSONAL_ACCESS_TOKEN"]).
# The launcher just forwards it; no token ever touches disk or logs here.
#
# Network: the manifest declares net:["api.github.com"] (default-deny: no other host).
#
# Resolution order:
#   1. $GITHUB_MCP_BIN (explicit override)
#   2. `github-mcp-server` on PATH
#   3. known install caches (~/.local/bin, $GOPATH/bin, ~/go/bin)
#
# Fail-closed: missing binary → JSON hint + non-zero exit.
set -eu

find_bin() {
  if [ -n "${GITHUB_MCP_BIN:-}" ] && [ -x "${GITHUB_MCP_BIN}" ]; then
    printf '%s' "${GITHUB_MCP_BIN}"; return 0
  fi
  if command -v github-mcp-server >/dev/null 2>&1; then
    command -v github-mcp-server; return 0
  fi
  for c in "$HOME/.local/bin/github-mcp-server" "${GOPATH:-$HOME/go}/bin/github-mcp-server" "$HOME/go/bin/github-mcp-server"; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

BIN="$(find_bin)" || {
  printf '{"error":"github-mcp-server binary not found","hint":"install from github.com/github/github-mcp-server releases or `go install`, or set GITHUB_MCP_BIN"}\n' >&2
  exit 3
}

# stdio mode; the token (if granted) is already in this process env (BYOK). The server
# reads GITHUB_PERSONAL_ACCESS_TOKEN itself — we don't echo or persist it.
exec "$BIN" stdio
