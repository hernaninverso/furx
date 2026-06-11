#!/bin/sh
# args: $1=tool. Reports OS/arch/cwd only — never dumps environment variables.
printf '{"tool":"%s","os":"%s","arch":"%s","cwd":"%s"}\n' "$1" "$(uname -s)" "$(uname -m)" "$(pwd)"
