#!/bin/bash
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# (Furx) notarization script (F50)
# Requiere:
#   1. Cert "$DEV_ID_CERT" instalado en Keychain (override con SIGN_ID).
#      Descargar desde https://developer.apple.com/account/resources/certificates/list
#   2. App-specific password en Keychain: `security add-generic-password -a "$USER" -s apple-notary-pwd -w "<pwd>"`
#      Generar en https://appleid.apple.com → Sign-In and Security → App-Specific Passwords
#
# Usage: ./scripts/notarize.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP=/Applications/Furx.app
SRC="$ROOT/src-tauri/target/release/bundle/macos/Furx.app"
ZIP=/tmp/Furx-notary.zip
TEAM_ID="${TEAM_ID:?set TEAM_ID to your Apple Developer Team ID}"
APPLE_ID="${APPLE_ID:?set APPLE_ID to your Apple ID email}"
DEV_ID_CERT="${SIGN_ID:-Developer ID Application: Your Name (${TEAM_ID})}"

# 1. Verify cert installed.
if ! security find-identity -v -p codesigning | grep -q "$DEV_ID_CERT"; then
  echo "❌ Cert '$DEV_ID_CERT' no instalado." >&2
  echo "   Descargar desde https://developer.apple.com/account/resources/certificates/list" >&2
  exit 1
fi

# 2. Notary credentials come from a stored notarytool keychain profile so the
#    app-specific password is NEVER passed on argv (visible in `ps`). One-time setup:
#      xcrun notarytool store-credentials "$NOTARY_PROFILE" \
#        --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password <app-specific-pwd>
NOTARY_PROFILE="${NOTARY_PROFILE:-furx-notary}"

# 3. Build fresh.
cd "$ROOT"
npm run build
npx tauri build --bundles app

# 4. Install to /Applications, then sign with Developer ID + hardened runtime.
pkill -f "Furx.app/Contents/MacOS/furx" 2>/dev/null || true
sleep 1
rm -rf "$APP"
ditto "$SRC" "$APP"
xattr -cr "$APP"
codesign --force --deep --sign "$DEV_ID_CERT" \
  --entitlements "$ROOT/src-tauri/entitlements.plist" \
  --options runtime --timestamp "$APP"
codesign -dv --verbose=2 "$APP" 2>&1 | head -10

# 5. Zip + submit + wait + staple.
ditto -c -k --keepParent "$APP" "$ZIP"
xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP"

# 6. Verify.
spctl -a -t exec -vv "$APP"
echo "✅ Notarized + stapled: $APP"
