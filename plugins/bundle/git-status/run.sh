#!/bin/sh
# Furx bundle plugin: git-status. argv: $1=tool $2=args_json. Read-only, no network.
out=$(git status -s 2>/dev/null | head -c 4000 | tr '\n' ';' | sed 's/"/\\"/g')
printf '{"tool":"%s","out":"%s"}\n' "$1" "$out"
