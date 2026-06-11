#!/usr/bin/env bash
# Guard CI: marca legacy (teal V3) NO debe reaparecer en NINGÚN surface de código.
# Escanea TODO archivo de código tracked (git ls-files), excluyendo docs/specs/.specify
# (que citan el teal viejo como lo removido) y este propio script.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
PATTERN='#0d5560|#46c7c0|#2bd1ea|#5ec9d4|#5ce0f1|#5ce0f0|#4ea1ff|#4da3ff|#0e7490|#0d4f5c|#0e8a96|rgba\(13,\s*85,\s*96|rgba\(70,\s*199,\s*192|rgba\(43,\s*209,\s*234|--teal-pale|hexágono cyan'
FILES=$(git ls-files -- '*.ts' '*.tsx' '*.js' '*.jsx' '*.css' '*.scss' '*.html' '*.svg' '*.rs' '*.vue' '*.md' '*.sh' '*.plist' \
  ':!docs/**' ':!specs/**' ':!.specify/**' ':!scripts/check-brand.sh' ':!src-tauri/migrations/**' ':!PLAN_*.md' ':!HUMAN_ACTIONS.md' ':!skills/**' ':!web-companion/README.md' 2>/dev/null || true)
if [ -z "$FILES" ]; then echo "WARN check-brand: sin archivos tracked"; exit 0; fi
HITS=$(printf '%s\n' "$FILES" | xargs grep -IniE "$PATTERN" 2>/dev/null \
  | grep -vE 'node_modules|/\.next/|/dist/|/build/' || true)
if [ -n "$HITS" ]; then
  echo "FAIL check-brand: marca legacy (teal V3) encontrada:"
  echo "$HITS"
  exit 1
fi
echo "OK check-brand: identidad coral limpia (sin teal V3 legacy) en todo el árbol tracked."
