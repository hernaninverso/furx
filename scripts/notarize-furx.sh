#!/usr/bin/env bash
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# 063 — Build universal + firma Developer ID + notarización + staple + verificación de Furx.app (+ DMG).
# Requisitos (una sola vez):
#   1. Cert "Developer ID Application: <Your Name> (<TEAM_ID>)" en el Keychain.
#   2. Perfil notarytool con la ASC API key (reemplazá la AuthKey/key-id/issuer por los tuyos):
#      xcrun notarytool store-credentials "furx-asc" \
#        --key ~/.appstoreconnect/private_keys/AuthKey_<KEY_ID>.p8 \
#        --key-id <KEY_ID> --issuer <ISSUER_ID>
#   3. Rust targets: rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -euo pipefail
cd "$(dirname "$0")/.."

PROFILE="${FURX_NOTARY_PROFILE:-furx-asc}"
APP="src-tauri/target/universal-apple-darwin/release/bundle/macos/Furx.app"

# 063 — build SÓLO el .app. El bundle del DMG de Tauri (`bundle_dmg.sh`) es flaky (Finder/hdiutil) y
# tumbaba todo el pipeline con `set -e` ANTES de notarizar. El .app firmado es el artefacto que se
# notariza; el DMG se arma aparte best-effort al final (no bloquea la notarización).
echo "▸ 1/6  build universal (aarch64 + x86_64) — Tauri firma con Developer ID + hardened runtime"
npm run tauri build -- --target universal-apple-darwin --bundles app
[ -d "$APP" ] || { echo "❌ no se generó $APP"; exit 1; }

echo "▸ 2/6  verificar firma + hardened runtime del .app"
codesign --verify --deep --strict --verbose=2 "$APP"
codesign -dvvv "$APP" 2>&1 | grep -iE "Authority=Developer ID|flags=.*runtime|TeamIdentifier" | head

echo "▸ 3/6  zip para notarización (ditto preserva la firma)"
ZIP="/tmp/Furx-notarize.zip"
rm -f "$ZIP"; ditto -c -k --keepParent "$APP" "$ZIP"

echo "▸ 4/6  notarytool submit --wait (ASC API key vía perfil '$PROFILE')"
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
# Si falla:  xcrun notarytool log <submission-id> --keychain-profile "$PROFILE"

echo "▸ 5/6  staple del .app (ticket embebido para uso offline)"
xcrun stapler staple "$APP"

echo "▸ 6/6  verificación final (Gatekeeper)"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=4 "$APP"

# DMG best-effort (NO bloquea): empaqueta el .app YA notarizado+stapleado en un .dmg con create-dmg si
# está, sino hdiutil. El .app adentro hereda el staple; igual stapleamos el .dmg si se crea.
echo "▸ extra  DMG (best-effort, no bloqueante)"
DMG_OUT="src-tauri/target/universal-apple-darwin/release/bundle/dmg/Furx_universal.dmg"
mkdir -p "$(dirname "$DMG_OUT")"
if command -v create-dmg >/dev/null 2>&1; then
  rm -f "$DMG_OUT"
  create-dmg --overwrite "$APP" "$(dirname "$DMG_OUT")" 2>/dev/null && \
    xcrun stapler staple "$(ls -t "$(dirname "$DMG_OUT")"/*.dmg | head -1)" 2>/dev/null || \
    echo "  (create-dmg falló — el .app notarizado igual sirve)"
else
  hdiutil create -volname "Furx" -srcfolder "$APP" -ov -format UDZO "$DMG_OUT" 2>/dev/null && \
    xcrun stapler staple "$DMG_OUT" 2>/dev/null || \
    echo "  (hdiutil DMG falló — el .app notarizado igual sirve; instalá create-dmg para un DMG lindo)"
fi

echo "✅ Furx.app notarizado + stapleado: $APP"
ls "$(dirname "$DMG_OUT")"/*.dmg 2>/dev/null && echo "   DMG: $(ls -t "$(dirname "$DMG_OUT")"/*.dmg | head -1)" || true
