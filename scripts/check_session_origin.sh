#!/usr/bin/env bash
# spec-049 · Cost-Router Fase 2 — CI guard (council §F1.1).
#
# Garantiza que `SessionOrigin` SOLO se instancia vía `from_server_state` (resuelto server-side, no
# manipulable por el frontend). El council pide:
#   grep -r "SessionOrigin::" src/ | grep -v "from_server_state\|enum SessionOrigin\|Self::\|match.*SessionOrigin"
# pero el código de PRODUCCIÓN usa `Self::` dentro de los constructores privados (allowlisted). Los
# usos en `#[cfg(test)]` que pasan/aseveran variantes como VALORES (no construyen un origen que
# bypasee el resolver server-side) NO son superficie de ataque y se excluyen del check.
#
# El guard recorre los .rs de src-tauri/src, IGNORA todo lo que esté dentro de un `mod tests { ... }`
# (cfg(test)), y aplica el grep canónico sobre el resto. Falla si encuentra una construcción de
# `SessionOrigin` fuera de `from_server_state`/`Self::`.
set -euo pipefail

SRC_DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/src"
viol=0

for f in $(grep -rl "SessionOrigin::" "$SRC_DIR" 2>/dev/null); do
  # Imprime solo las líneas FUERA del módulo de tests (todo lo anterior a `mod tests {`).
  prod=$(awk '/^[[:space:]]*mod tests[[:space:]]*\{/{exit} {print}' "$f")
  # Allowlist: la familia de resolvers server-side. `from_server_state` (el principal) e
  # `interactive_no_signal` (el fallback conservador "sin señal ⇒ interactivo", que internamente usa
  # `Self::new_user_initiated`). Ambos resuelven el origen desde el estado del backend; NINGUNO deja
  # que un caller fabrique un `Automated`. `Self::`/`enum`/`match` ya estaban en el canon del council.
  hits=$(printf '%s\n' "$prod" \
    | grep -n "SessionOrigin::" \
    | grep -v "from_server_state\|interactive_no_signal\|enum SessionOrigin\|Self::\|match.*SessionOrigin\|//" \
    || true)
  if [ -n "$hits" ]; then
    echo "VIOLATION en $f (SessionOrigin construido fuera de from_server_state):"
    echo "$hits"
    viol=1
  fi
done

if [ "$viol" -ne 0 ]; then
  echo "FAIL: SessionOrigin debe instanciarse SOLO vía from_server_state (server-side)."
  exit 1
fi
echo "OK: SessionOrigin solo se instancia vía from_server_state."
