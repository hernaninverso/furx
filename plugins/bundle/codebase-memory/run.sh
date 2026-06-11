#!/bin/sh
# Furx bundle plugin: codebase-memory (spec-011).
#
# This is the SIGNED, hash-bound entrypoint. Two roles:
#   (no extra args)            → launch the MCP server on stdio (what the agent CLI runs)
#   index <project_root>       → run a single indexing pass (FR-004 background indexer)
#
# It locates the real `codebase-memory-mcp` binary (first $CODEBASE_MEMORY_MCP_BIN, then
# PATH, then the known install cache). net/shell/secrets are NOT used by this launcher;
# the only side effect is read-only graph indexing of the repo into a per-project store.
#
# STORE LOCATION (honest note for v1): codebase-memory-mcp 0.6.0 writes its per-project
# graph DB under $HOME/.cache/codebase-memory-mcp/<slugified-repo-path>.db and IGNORES
# $XDG_CACHE_HOME. We still set $XDG_CACHE_HOME (forward-compat: future versions may
# honor it) and DECLARE that real path in the manifest's fs_write so the audit statement
# is truthful. The MCP server and this indexer share that same store (keyed by repo
# path) so the agent's queries see what the background index wrote.
#
# Fail-closed: if the binary is missing we print a JSON hint and exit non-zero rather
# than silently doing nothing.
set -eu

find_bin() {
  if [ -n "${CODEBASE_MEMORY_MCP_BIN:-}" ] && [ -x "${CODEBASE_MEMORY_MCP_BIN}" ]; then
    printf '%s' "${CODEBASE_MEMORY_MCP_BIN}"; return 0
  fi
  if command -v codebase-memory-mcp >/dev/null 2>&1; then
    command -v codebase-memory-mcp; return 0
  fi
  for c in "$HOME/.local/bin/codebase-memory-mcp"; do
    [ -x "$c" ] && { printf '%s' "$c"; return 0; }
  done
  return 1
}

BIN="$(find_bin)" || {
  printf '{"error":"codebase-memory-mcp binary not found","hint":"install it or set CODEBASE_MEMORY_MCP_BIN"}\n' >&2
  exit 3
}

case "${1:-}" in
  index)
    # Single indexing pass over the project root (resolved by the runtime).
    # codebase-memory-mcp 0.6.0 expects `repo_path` for index_repository.
    # Build the JSON arg safely. A JSON string literal only has three classes of
    # significant characters: " and \ (escape them), and control characters (must be
    # \u-escaped). A real repo path never contains control chars (newline/CR/tab/NUL…),
    # so REJECT those fail-closed; then escaping \ and " yields always-valid JSON.
    ROOT="${2:-.}"
    case "$ROOT" in
      *[[:cntrl:]]*)
        printf '{"error":"invalid repo_path: control characters not allowed"}\n' >&2
        exit 4
        ;;
    esac
    ESC=$(printf '%s' "$ROOT" | sed 's/\\/\\\\/g; s/"/\\"/g')
    exec "$BIN" cli index_repository "{\"repo_path\":\"$ESC\"}"
    ;;
  *)
    # Default: MCP server on stdio (the agent CLI speaks JSON-RPC to it).
    exec "$BIN"
    ;;
esac
