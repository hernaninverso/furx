#!/bin/sh
# args: $1=tool $2=args_json ({"q":"pattern"}). Read-only search of CWD via rg (or grep).
q=$(printf '%s' "$2" | sed -n 's/.*"q":"\([^"]*\)".*/\1/p')
[ -z "$q" ] && { printf '{"error":"missing q"}\n'; exit 0; }
if command -v rg >/dev/null 2>&1; then out=$(rg -n --no-heading -m 50 -- "$q" . 2>/dev/null | head -50); else out=$(grep -rn -m 50 -- "$q" . 2>/dev/null | head -50); fi
printf '{"tool":"%s","matches":"%s"}\n' "$1" "$(printf '%s' "$out" | tr '\n' ';' | sed 's/"/\\"/g')"
