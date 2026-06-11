#!/bin/sh
# Furx bundle plugin: find-files. argv: $1=tool $2=args_json. Read-only, no network.
out=$(ls -lat 2>/dev/null | head -c 4000 | tr '\n' ';' | sed 's/"/\\"/g')
printf '{"tool":"%s","out":"%s"}\n' "$1" "$out"
