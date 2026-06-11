#!/bin/sh
# Furx bundle plugin: filesystem-ls. argv: $1=tool $2=args_json
# Read-only directory listing of CWD (no network, no writes).
printf '{"tool":"%s","files":"%s"}\n' "$1" "$(ls -1A 2>/dev/null | tr '\n' ',' | sed 's/,$//')"
