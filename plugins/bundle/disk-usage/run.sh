#!/bin/sh
# Furx bundle plugin: disk-usage. argv: $1=tool $2=args_json. Read-only, no network.
out=$(du -sh . 2>/dev/null | head -c 4000 | tr '\n' ';' | sed 's/"/\\"/g')
printf '{"tool":"%s","out":"%s"}\n' "$1" "$out"
