#!/bin/sh
# Furx bundle plugin: git-mcp (spec-013, Tier 2).
#
# cyanheads/git-mcp-server (Apache-2.0) — comprehensive LOCAL git operations
# (status/diff/log/commit/branch/worktree/tag, GPG/SSH signing) by spawning the
# system `git` with validated argv (no shell interpolation). Local only: net:false.
# We do NOT vendor the npm package; it is a RUNTIME DEPENDENCY via `npx` (or set
# GIT_MCP_SERVER_BIN). This signed, hash-bound launcher starts its stdio MCP server
# scoped to the project root.
#
# NOTE: `npx -y @cyanheads/git-mcp-server` may fetch from the npm registry on FIRST
# run (one-time install), not the plugin's operating network use. To keep it strictly
# net:false at run time, pre-install (`npm i -g @cyanheads/git-mcp-server`) and point
# GIT_MCP_SERVER_BIN at it.
#
# Fail-closed: neither a pinned binary nor npx → JSON hint + non-zero exit.
set -eu

PROJECT_ROOT="${1:-.}"
case "$PROJECT_ROOT" in
  *[[:cntrl:]]*)
    printf '{"error":"invalid project root: control characters not allowed"}\n' >&2
    exit 4
    ;;
esac

cd "$PROJECT_ROOT" || {
  printf '{"error":"cannot enter project root"}\n' >&2
  exit 5
}

if [ -n "${GIT_MCP_SERVER_BIN:-}" ] && [ -x "${GIT_MCP_SERVER_BIN}" ]; then
  exec "${GIT_MCP_SERVER_BIN}"
fi
if command -v git-mcp-server >/dev/null 2>&1; then
  exec "$(command -v git-mcp-server)"
fi
# npx fetches from the npm registry — a network call this net:[] plugin does NOT
# declare. Gate it behind explicit opt-in (audit codex+deepseek 013).
if [ "${FURX_ALLOW_NPX_INSTALL:-0}" = "1" ] && command -v npx >/dev/null 2>&1; then
  exec npx -y @cyanheads/git-mcp-server
fi

printf '{"error":"git-mcp-server not found","hint":"install: npm i -g @cyanheads/git-mcp-server (or set GIT_MCP_SERVER_BIN). npx auto-install is gated behind FURX_ALLOW_NPX_INSTALL=1 (reaches npm registry, not declared by this net:[] plugin)"}\n' >&2
exit 3
