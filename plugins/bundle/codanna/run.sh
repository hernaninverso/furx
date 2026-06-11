#!/bin/sh
# Furx bundle plugin: codanna (spec-013, Tier 1).
#
# bartolli/codanna (Apache-2.0) — local code-intelligence MCP server (call-graph,
# semantic search) over the repo. Offline: net:false. We do NOT bundle the Rust
# binary; it is a RUNTIME DEPENDENCY (`cargo install codanna`, or set CODANNA_BIN).
# This signed, hash-bound launcher locates `codanna` and starts its stdio MCP server
# scoped to the project root.
#
# codanna `serve` operates over the index in the current working directory. The Furx
# runtime passes $PROJECT_ROOT as $1; we cd there (read-only intelligence; codanna
# writes its index under the repo's .codanna dir, same as a normal `codanna index`).
#
# Resolution order:
#   1. $CODANNA_BIN (explicit override)
#   2. `codanna` on PATH
#   3. known caches (~/.cargo/bin, ~/.local/bin)
#
# Fail-closed: missing binary → JSON hint + non-zero exit.
set -eu

PROJECT_ROOT="${1:-.}"
case "$PROJECT_ROOT" in
  *[[:cntrl:]]*)
    printf '{"error":"invalid project root: control characters not allowed"}\n' >&2
    exit 4
    ;;
esac

find_bin() {
  if [ -n "${CODANNA_BIN:-}" ] && [ -x "${CODANNA_BIN}" ]; then
    printf '%s' "${CODANNA_BIN}"; return 0
  fi
  if command -v codanna >/dev/null 2>&1; then
    command -v codanna; return 0
  fi
  for c in "$HOME/.cargo/bin/codanna" "$HOME/.local/bin/codanna"; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

BIN="$(find_bin)" || {
  printf '{"error":"codanna binary not found","hint":"install with: cargo install codanna (or set CODANNA_BIN)"}\n' >&2
  exit 3
}

cd "$PROJECT_ROOT" || {
  printf '{"error":"cannot enter project root"}\n' >&2
  exit 5
}
# stdio MCP server with hot-reload watch over the repo.
exec "$BIN" serve --watch
