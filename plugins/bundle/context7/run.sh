#!/bin/sh
# Furx bundle plugin: context7 (spec-013, Tier 2, OPT-IN / HOSTED).
#
# upstash/context7 (@upstash/context7-mcp, MIT) — version-specific library docs into
# the agent's context. CAVEAT (documented opt-in): the local stdio server is a THIN
# CLIENT of the HOSTED Context7 API (mcp.context7.com). So enabling it sends your
# library queries to Upstash's service. That is why it is OPT-IN with a net allowlist
# (default-deny: only context7.com + mcp.context7.com) and BYOK.
#
# BYOK (F-I): the Context7 API key is NEVER hardcoded. The Plugin Host injects
# $CONTEXT7_API_KEY into this process env ONLY if the user granted that secret from
# the OS Keychain (manifest declares secrets:["CONTEXT7_API_KEY"]). The server reads
# the env var itself; we never echo or persist it, and never pass it on argv (where
# it could leak into process listings). Anonymous use (no key) is allowed but
# rate-limited by Context7 (60 req/h shared pool).
#
# We do NOT vendor the npm package; it is a RUNTIME DEPENDENCY via `npx` (or set
# CONTEXT7_MCP_BIN). Fail-closed: neither a pinned binary nor npx → JSON hint + exit.
set -eu

if [ -n "${CONTEXT7_MCP_BIN:-}" ] && [ -x "${CONTEXT7_MCP_BIN}" ]; then
  exec "${CONTEXT7_MCP_BIN}"
fi
if command -v context7-mcp >/dev/null 2>&1; then
  exec "$(command -v context7-mcp)"
fi
# npx fetches from the npm registry — a host NOT in this plugin's net allow-list
# (context7.com only). Gate it behind explicit opt-in (audit codex+deepseek 013).
if [ "${FURX_ALLOW_NPX_INSTALL:-0}" = "1" ] && command -v npx >/dev/null 2>&1; then
  exec npx -y @upstash/context7-mcp
fi

printf '{"error":"context7 MCP not found","hint":"install: npm i -g @upstash/context7-mcp (or set CONTEXT7_MCP_BIN). npx auto-install is gated behind FURX_ALLOW_NPX_INSTALL=1 (reaches npm registry, outside this plugin net allow-list). Hosted service — opt-in, BYOK CONTEXT7_API_KEY"}\n' >&2
exit 3
