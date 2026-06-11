#!/bin/bash
# setup-max-account.sh v2 — onboard a Claude Max OAuth token into macOS Keychain.
# REWRITE 2026-05-26 (council 5/5): arregla "siempre abre Safari" y "es confuso".
#
# Flow:
#  1. Detect available browsers (Chrome/Brave/Arc/Firefox/Edge/Safari).
#  2. Pick a per-slug browser/profile — distinct profile per slug so sessions
#     NEVER overlap.
#  3. Print one-line instructions + open URL in that browser/profile.
#  4. Run `claude setup-token` (browser-redirect OAuth), accept token paste.
#  5. Validate token format before saving.

set -euo pipefail

SLUG="${1:-}"
if [ -z "$SLUG" ]; then
  cat <<'EOF' >&2
Usage:  setup-max-account.sh <slug>     e.g.  A, B, work, personal

  Each slug binds ONE Claude Max account to a Keychain entry
  "claude-max-<slug>". Use a different slug per account. Then run
  ~/bin/claude-as-<slug> to launch Claude bound to that account.
  Multiple sessions in parallel — no logout/login.

  Override browser:  SETUP_BROWSER=Chrome  setup-max-account.sh B
EOF
  exit 1
fi

if [[ ! "$SLUG" =~ ^[A-Za-z0-9_-]{1,32}$ ]]; then
  echo "ERROR: invalid slug '$SLUG' (allowed: [A-Za-z0-9_-]{1,32})" >&2
  exit 2
fi
SERVICE="claude-max-$SLUG"

# Detect browsers in preference order (Chrome-family first — best profile support).
declare -a AVAILABLE_BROWSERS=()
declare -a BROWSER_LABELS=()
add_browser() {
  if [ -e "$1" ]; then
    AVAILABLE_BROWSERS+=("$1")
    BROWSER_LABELS+=("$2")
  fi
}
# Orden de preferencia (2026-05-26 fix):
# Chrome-family (Chrome > Brave > Edge) > Firefox > Arc > Safari.
# Arc/Safari quedan al final porque NO pueden pinnear profile via CLI sin tocar la UI
# del browser (rompe el flow de "Sign in with Google" si reusan sesion).
add_browser "/Applications/Google Chrome.app"    "Chrome"
add_browser "/Applications/Brave Browser.app"    "Brave"
add_browser "/Applications/Microsoft Edge.app"   "Edge"
add_browser "/Applications/Firefox.app"          "Firefox"
add_browser "/Applications/Arc.app"              "Arc"
add_browser "/Applications/Safari.app"           "Safari"

if [ "${#AVAILABLE_BROWSERS[@]}" -eq 0 ]; then
  echo "ERROR: no supported browser found. Install Safari or Chrome first." >&2
  exit 1
fi

# Pick browser (default: Chrome con profile-per-slug si está disponible — sesiones
# aisladas nativamente en el mismo browser, mejor UX que cambiar de browser).
# Prioridad: Chrome-family con --profile-directory (Chrome/Brave/Edge) > Firefox > Safari.
pick_idx=0
for i in "${!BROWSER_LABELS[@]}"; do
  case "${BROWSER_LABELS[$i]}" in
    Chrome|Brave|Edge) pick_idx=$i; break ;;
  esac
done
BROWSER="${AVAILABLE_BROWSERS[$pick_idx]}"
BROWSER_LABEL="${BROWSER_LABELS[$pick_idx]}"

# Env override (SETUP_BROWSER=Firefox setup-max-account.sh B)
if [ -n "${SETUP_BROWSER:-}" ]; then
  for i in "${!BROWSER_LABELS[@]}"; do
    LBL_LOW=$(echo "${BROWSER_LABELS[$i]}" | tr '[:upper:]' '[:lower:]')
    SET_LOW=$(echo "${SETUP_BROWSER}" | tr '[:upper:]' '[:lower:]')
    if [ "$LBL_LOW" = "$SET_LOW" ]; then
      BROWSER="${AVAILABLE_BROWSERS[$i]}"
      BROWSER_LABEL="${BROWSER_LABELS[$i]}"
      break
    fi
  done
fi

# Pretty UI
ALL_LABELS=$(IFS=', '; echo "${BROWSER_LABELS[*]}")
cat <<EOF

  ┌─ Furx · Add Claude Max account ──────────────────────────────────┐

    Slug:       ${SLUG}
    Keychain:   ${SERVICE}
    Browser:    ${BROWSER_LABEL}   (detected: ${ALL_LABELS})

EOF

# Si es Chrome-family + multi-slug, explicar el profile aisladito.
case "$BROWSER_LABEL" in
  Chrome|Brave|Edge)
    echo "    ${BROWSER_LABEL} profile: Claude-${SLUG}   (sesion aislada del resto)"
    ;;
  *)
    echo "    Sesion aislada — distinto browser por slug evita cookie clash."
    ;;
esac

cat <<EOF

    Cambiar browser ahora:
      Enter           → seguir con ${BROWSER_LABEL}
      1..${#BROWSER_LABELS[@]}             → elegir de la lista
      otra letra (q)  → cancelar

  └──────────────────────────────────────────────────────────────────┘

EOF

# Interactive picker (one keystroke, optional)
echo "  Lista de browsers:"
for i in "${!BROWSER_LABELS[@]}"; do
  marker="  "
  if [ "$i" = "$pick_idx" ]; then marker="* "; fi
  echo "    ${marker}$((i+1))) ${BROWSER_LABELS[$i]}"
done
echo
read -r -p "  Elegir browser [Enter=${BROWSER_LABEL}] > " choice
if [ -n "${choice:-}" ]; then
  if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#BROWSER_LABELS[@]}" ]; then
    pick_idx=$((choice - 1))
    BROWSER="${AVAILABLE_BROWSERS[$pick_idx]}"
    BROWSER_LABEL="${BROWSER_LABELS[$pick_idx]}"
    echo "  -> ${BROWSER_LABEL}"
  else
    echo "  Cancelado."
    exit 0
  fi
fi

# Overwrite warning if entry exists
if security find-generic-password -a "$USER" -s "$SERVICE" -w >/dev/null 2>&1; then
  echo "  [!] Keychain '${SERVICE}' YA existe — continuar lo SOBREESCRIBE."
  read -r -p "  Continuar? (y/N) " ans
  ans_lower=$(echo "${ans:-}" | tr '[:upper:]' '[:lower:]')
  if [ "$ans_lower" != "y" ]; then
    echo "  Abortado."; exit 0
  fi
fi

URL="https://claude.ai/login"
echo "  Abriendo ${BROWSER_LABEL} en ${URL} ..."
case "$BROWSER_LABEL" in
  Chrome|Brave|Edge)
    PROFILE_DIR="Claude-$SLUG"
    open -na "$BROWSER" --args --profile-directory="$PROFILE_DIR" "$URL"
    ;;
  Arc)
    # Arc tiene Spaces aislados pero NO se accede via CLI; usa default profile.
    open -na "$BROWSER" "$URL"
    SAFARI_WARN=0
    ARC_WARN=1
    ;;
  Firefox)
    # Firefox -P <name> abre/crea profile NAMED — cookies aisladas + Google login funciona.
    # Si el profile no existe, Firefox abre profile manager y el user lo crea con ese nombre.
    open -na "$BROWSER" --args -P "Claude-$SLUG" --no-remote "$URL" 2>/dev/null \
      || open -na "$BROWSER" "$URL"
    SAFARI_WARN=0
    ;;
  Safari)
    # IMPORTANTE (2026-05-26): NO usar private window. Google OAuth detecta private
    # y bloquea el flow "Sign in with Google". Safari NO acepta profile via CLI (solo
    # cambio manual desde menubar). Abrimos default + advertencia explícita.
    open -na "$BROWSER" "$URL"
    SAFARI_WARN=1
    ;;
esac

cat <<EOF

  Estás ahora en ${BROWSER_LABEL}:

EOF

case "$BROWSER_LABEL" in
  Chrome|Brave|Edge)
    cat <<EOF
    Profile aislado: 'Claude-${SLUG}'  (cookies separadas, Google OAuth funciona).
    Logueate normalmente — esta sesion no pisa la cuenta de tu profile default.

EOF
    ;;
  Firefox)
    cat <<EOF
    Profile: 'Claude-${SLUG}' (Firefox lo crea si no existe).
    Si te aparece el profile manager, dale Crear / Continuar con ese nombre.

EOF
    ;;
  Arc)
    cat <<EOF
    Arc no soporta CLI profile pinning. Tip: arrastra esta tab a un Space NUEVO
    en Arc (Cmd+T en un nuevo Space) para aislar la sesion manualmente.

EOF
    ;;
  Safari)
    cat <<EOF
    [!] Safari NO acepta profile via CLI — vas a usar la sesion actual del browser.

    Opciones para Safari:
      (a) Safari > File > New Profile (macOS 14+) → crear profile 'Claude ${SLUG}'
          y cambiar a él ANTES de loguearte. Mas info:
          https://support.apple.com/guide/safari/use-profiles-in-safari-ibrw7f0d3bf3
      (b) Usar Chrome/Firefox para multi-cuenta (mejor UX automatica).
      (c) Loguearte con la cuenta nueva acá (sobreescribe la sesion actual).

EOF
    ;;
esac

cat <<EOF
    1. Logueate con la cuenta Claude Max que querés ligar al slug '${SLUG}'.
       Si ya hay sesion de otra cuenta y NO estás en profile aislado → log out primero.

    2. Volvé a esta terminal y presioná Enter.

EOF
read -r -p "  Listo? Enter para continuar > "

echo
echo "  Corriendo 'claude setup-token' ..."
echo "  El CLI te va a redirigir al browser para autorizar. Después te muestra"
echo "  un token largo — copialo, volvé a la terminal y pegalo abajo."
echo

claude setup-token || {
  echo "  ERROR: 'claude setup-token' falló." >&2
  exit 1
}

echo
read -r -s -p "  Pegá el token (input oculto) > " TOKEN
echo

if [ -z "$TOKEN" ]; then
  echo "  ERROR: token vacío." >&2
  exit 1
fi

TOKEN_LEN=${#TOKEN}
if [ "$TOKEN_LEN" -lt 32 ]; then
  echo "  ERROR: token sospechosamente corto ($TOKEN_LEN chars)." >&2
  exit 1
fi

NOTE="Claude Max OAuth · slug $SLUG · $BROWSER_LABEL · $(date -u +%Y-%m-%d)"
security add-generic-password -U -a "$USER" -s "$SERVICE" -j "$NOTE" -w "$TOKEN"
TOKEN=""

# Ensure shortcut wrapper exists
WRAPPER="$HOME/bin/claude-as-$SLUG"
if [ ! -f "$WRAPPER" ]; then
  cat > "$WRAPPER" <<EOFW
#!/bin/bash
exec ~/bin/claude-as $SLUG "\$@"
EOFW
  chmod +x "$WRAPPER"
  echo "  Wrapper creado: $WRAPPER"
fi

# Auto-register en (Furx) DB (claude_accounts table) si la DB existe.
# Esto hace que el slug aparezca automaticamente en el dropdown de panes de (Furx)
# sin tener que ir al wizard "Claude Accounts" tab.
FURX_DB="$HOME/.furx/furx.db"
if [ -f "$FURX_DB" ] && command -v sqlite3 >/dev/null 2>&1; then
  NOW=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
  # Default label = slug capitalizado (puede editarse despues en (Furx) UI)
  LABEL_DEFAULT="Claude ${SLUG}"
  sqlite3 "$FURX_DB" \
    "INSERT INTO claude_accounts (slug, label, browser, status, last_verified_at, created_at, updated_at)
     VALUES ('${SLUG}', '${LABEL_DEFAULT}', '${BROWSER_LABEL}', 'verified', '${NOW}', '${NOW}', '${NOW}')
     ON CONFLICT(slug) DO UPDATE SET
       browser = excluded.browser,
       status = 'verified',
       last_verified_at = excluded.last_verified_at,
       updated_at = excluded.updated_at;" 2>/dev/null \
    && echo "  [OK] (Furx) DB:    cuenta '${SLUG}' registrada (visible en dropdown de panes)" \
    || echo "  [WARN] (Furx) DB:  no se pudo registrar en ~/.furx/furx.db (agregala manual en Furx Connect tab)"
fi

cat <<EOF

  [OK] Guardado: ${SERVICE}
  [OK] Wrapper:   ~/bin/claude-as-${SLUG}

  Ahora abri una terminal nueva (Cmd-N) y corre:
      claude-as-${SLUG}

  O abri/reiniciá Furx: el slug '${SLUG}' aparece en el dropdown de cada pane.

  Las dos sesiones corren en PARALELO. Sin logout/login.

EOF
