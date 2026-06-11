#!/bin/zsh
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# Build Furx.app + sign with stable identity + open.
# Stable signing identity = TCC grants survive across builds (vs. ad-hoc which
# changes the cdhash every build and forces macOS to re-prompt for permissions).
#
# Usage:  ./scripts/build-sign-open.sh               — build + sign + open the new bundle
#         ./scripts/build-sign-open.sh --skip-build  — only sign + open last build
#         ./scripts/build-sign-open.sh --install     — also overwrite /Applications/Furx.app

set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP="$REPO/src-tauri/target/release/bundle/macos/Furx.app"
IDENTITY="${SIGN_ID:?set SIGN_ID to your Apple Developer cert id / name in the login keychain}"
ENTITLEMENTS="$REPO/src-tauri/entitlements.plist"

SKIP_BUILD=0
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    --install)    INSTALL=1 ;;
  esac
done

cd "$REPO"

if [ "$SKIP_BUILD" -eq 0 ]; then
  echo "→ npx tauri build --bundles app"
  npx tauri build --bundles app 2>&1 | tail -5 || true   # updater key missing is non-fatal
fi

[ -d "$APP" ] || { echo "❌ no built app at $APP" >&2; exit 1; }

# Verify the cert is available.
if ! security find-identity -v -p codesigning | grep -q "$IDENTITY"; then
  echo "❌ signing identity $IDENTITY not in login.keychain" >&2
  echo "   available identities:" >&2
  security find-identity -v -p codesigning >&2
  exit 1
fi

echo "→ codesign --sign $IDENTITY --force --deep --options runtime"
xattr -cr "$APP"
codesign --force --deep --sign "$IDENTITY" \
  --entitlements "$ENTITLEMENTS" \
  --options runtime \
  "$APP" >/dev/null 2>&1

# Verify the signature
echo "→ verify"
codesign -dvv "$APP" 2>&1 | grep -E "(Identifier|Signature|TeamIdentifier|Authority=)" | head -5

# Optional: pisar /Applications/Furx.app (only with --install)
if [ "$INSTALL" -eq 1 ]; then
  DEST=/Applications/Furx.app
  echo "→ ditto → $DEST"
  rm -rf "$DEST"
  ditto "$APP" "$DEST"
  xattr -cr "$DEST"
  APP="$DEST"
fi

# Close any running instance so the new binary takes over.
pkill -x furx 2>/dev/null || true
sleep 1
echo "→ open $APP"
open "$APP"
sleep 1
pgrep -l furx | head -3
