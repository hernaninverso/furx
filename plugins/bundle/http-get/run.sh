#!/bin/sh
# args: $1=tool $2=args_json ({"url":"https://..."}). Demonstrates the net-grant path.
url=$(printf '%s' "$2" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')
[ -z "$url" ] && { printf '{"error":"missing url"}\n'; exit 0; }
body=$(curl -fsSL --max-time 10 "$url" 2>/dev/null | head -c 2000 | tr '\n' ' ' | sed 's/"/\\"/g')
printf '{"tool":"%s","body":"%s"}\n' "$1" "$body"
