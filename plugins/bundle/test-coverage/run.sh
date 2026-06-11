#!/bin/sh
# Furx bundle plugin: test-coverage (spec-013, Tier 1).
#
# goldbergyoni/test-coverage-mcp (MIT) — makes the agent aware of how its edits move
# test coverage (LCOV, language-agnostic). Offline: net:false (reads local lcov.info).
# We do NOT vendor the npm package; it is a RUNTIME DEPENDENCY resolved via `npx`
# (or set TEST_COVERAGE_MCP_BIN to a pre-installed binary). This signed, hash-bound
# launcher starts its stdio MCP server scoped to the project root.
#
# NOTE: `npx -y test-coverage-mcp` may reach the npm registry on FIRST run to fetch
# the package into the npx cache. That is a one-time install fetch, NOT the plugin's
# operating network use (the tool itself is offline). To keep the plugin strictly
# net:false at run time, pre-install it (`npm i -g test-coverage-mcp`) and point
# TEST_COVERAGE_MCP_BIN at it, or rely on the npx cache being warm.
#
# Fail-closed: neither a pinned binary nor npx available → JSON hint + non-zero exit.
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

if [ -n "${TEST_COVERAGE_MCP_BIN:-}" ] && [ -x "${TEST_COVERAGE_MCP_BIN}" ]; then
  exec "${TEST_COVERAGE_MCP_BIN}"
fi
if command -v test-coverage-mcp >/dev/null 2>&1; then
  exec "$(command -v test-coverage-mcp)"
fi
# npx fetches from the npm registry — a network call this net:[] plugin does NOT
# declare. Gate it behind explicit opt-in so the DEFAULT path makes no surprise network
# (audit codex+deepseek 013). Set FURX_ALLOW_NPX_INSTALL=1 to permit the one-time fetch.
if [ "${FURX_ALLOW_NPX_INSTALL:-0}" = "1" ] && command -v npx >/dev/null 2>&1; then
  exec npx -y test-coverage-mcp
fi

printf '{"error":"test-coverage-mcp not found","hint":"install: npm i -g test-coverage-mcp (or set TEST_COVERAGE_MCP_BIN). npx auto-install is gated behind FURX_ALLOW_NPX_INSTALL=1 (reaches npm registry, not declared by this net:[] plugin)"}\n' >&2
exit 3
