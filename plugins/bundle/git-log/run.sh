#!/bin/sh
# Furx bundle plugin: git-log. argv: $1=tool $2=args_json
printf '{"tool":"%s","log":"%s"}\n' "$1" "$(git log --oneline -10 2>/dev/null | tr '\n' ';' | sed 's/;$//' | sed 's/"/\\"/g')"
