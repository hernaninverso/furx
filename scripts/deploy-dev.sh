#!/bin/bash
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# (Furx) deploy-dev — install + sign with Developer ID cert (no notarization).
#
# Why this exists: ad-hoc signing (`codesign --sign -`) creates a new identity
# on each build, so macOS TCC asks for accessibility/microphone/etc permissions
# every single time the app is rebuilt. Signing with a stable Developer ID
# Application cert keeps the identity constant across rebuilds → TCC remembers.
#
# Requires: a "Developer ID Application" cert (override via SIGN_ID / TEAM_ID)
# installed in Keychain. Same cert used by notarize.sh.
#
# Usage: ./scripts/deploy-dev.sh
# (Assumes you already ran `npx tauri build --bundles app`.)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP=/Applications/Furx.app
SRC="$ROOT/src-tauri/target/release/bundle/macos/Furx.app"
TEAM_ID="${TEAM_ID:?set TEAM_ID to your Apple Developer Team ID}"
DEV_ID_CERT="${SIGN_ID:-Developer ID Application: Your Name (${TEAM_ID})}"
ENTL="$ROOT/src-tauri/entitlements.plist"

if [ ! -d "$SRC" ]; then
  echo "❌ No bundle at $SRC — run: npx tauri build --bundles app" >&2
  exit 1
fi

if ! security find-identity -v -p codesigning | grep -q "$DEV_ID_CERT"; then
  echo "❌ Cert '$DEV_ID_CERT' no instalado en Keychain." >&2
  exit 1
fi

pkill -f "Furx.app/Contents/MacOS/furx" 2>/dev/null || true
sleep 1
ditto "$SRC" "$APP"
xattr -cr "$APP"
codesign --force --deep --options runtime \
  --entitlements "$ENTL" \
  --sign "$DEV_ID_CERT" "$APP"
codesign --verify --verbose=2 --deep --strict "$APP"

# Confirm stable identity (this is what TCC keys off).
echo '— Signing authority —'
codesign -dvv "$APP" 2>&1 | grep -E "Authority|Identifier|TeamIdentifier" | head -3

open "$APP"
sleep 2
if pgrep -f "MacOS/furx$" >/dev/null; then
  echo "✅ Furx.app instalado, firmado e ALIVE."
else
  echo "⚠️  Furx.app no levantó tras el open — chequear logs." >&2
  exit 1
fi
