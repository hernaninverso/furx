#!/bin/bash
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# (Furx) release script — build signed bundles + generate updater manifest.
#
# Requirements (one-time setup, already done 2026-05-27):
#   1. ~/.furx/updater.key            — Tauri signer Ed25519 private key (mode 600)
#   2. ~/.furx/updater-key.passphrase — passphrase for the private key (mode 600)
#   3. Developer ID Application cert in Keychain (your name + Apple team id)
#   4. App-specific notarization password in Keychain (apple-notary-pwd)
#
# Usage: ./scripts/release.sh [vX.Y.Z]
#   If no version arg, reads from src-tauri/tauri.conf.json.
#   Tags + pushes the tag, builds, signs, notarizes, generates latest.json.
#
# Produces:
#   - src-tauri/target/release/bundle/macos/Furx.app             (firmado + notarizado)
#   - src-tauri/target/release/bundle/macos/Furx.app.tar.gz      (updater payload)
#   - src-tauri/target/release/bundle/macos/Furx.app.tar.gz.sig  (firma del updater)
#   - latest.json                                                (manifest para GH releases)

set -euo pipefail

KEY_PATH=~/.furx/updater.key
PASS_PATH=~/.furx/updater-key.passphrase

if [ ! -f "$KEY_PATH" ]; then
  echo "❌ Falta $KEY_PATH — regenerá con: npx tauri signer generate -w $KEY_PATH" >&2
  exit 1
fi
if [ ! -f "$PASS_PATH" ]; then
  echo "❌ Falta $PASS_PATH — guardá el passphrase ahí" >&2
  exit 1
fi

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

# GitHub repo slug used in the updater manifest URLs (override to point at your fork).
GH_REPO="${FURX_GH_REPO:-hernaninverso/furx}"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION="v$(python3 -c "import json; print(json.load(open('src-tauri/tauri.conf.json'))['version'])")"
fi
echo "▶ Releasing $VERSION from $ROOT"

# Codex HIGH v1: enforce 0600 on key files even if mode drifted, and scope the
# private-key env vars ONLY to the tauri-build invocation (NOT npm run build).
chmod 600 "$KEY_PATH" "$PASS_PATH"

# Frontend build runs WITHOUT signing key in env — keys only touch tauri build.
npm run build

# Codex HIGH v2: tauri build re-runs `beforeBuildCommand` (which is `npm run build`)
# unless we override it. We pass `--config` inline to set beforeBuildCommand="" so
# the signing env only ever reaches the Rust/bundler step, never npm lifecycle.
(
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_PATH")"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$PASS_PATH")"
  npx tauri build --bundles app,dmg --config '{"build":{"beforeBuildCommand":""}}'
)

APP="$ROOT/src-tauri/target/release/bundle/macos/Furx.app"
TARBALL="$APP.tar.gz"
SIG="$TARBALL.sig"

if [ ! -f "$TARBALL" ] || [ ! -f "$SIG" ]; then
  echo "❌ updater artifacts missing — verificar createUpdaterArtifacts=true en tauri.conf.json" >&2
  exit 1
fi

# Generate manifest.
SIGNATURE="$(cat "$SIG")"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"
NOTES_FILE="$ROOT/RELEASE_NOTES.md"
NOTES="$(if [ -f "$NOTES_FILE" ]; then cat "$NOTES_FILE"; else echo "See https://github.com/$GH_REPO/releases/$VERSION"; fi)"

python3 - <<PY > "$ROOT/latest.json"
import json, os, sys
version = "${VERSION#v}"
manifest = {
    "version": version,
    "notes": ${NOTES@Q},
    "pub_date": "$PUB_DATE",
    "platforms": {
        "darwin-aarch64": {
            "signature": ${SIGNATURE@Q},
            "url": f"https://github.com/${GH_REPO}/releases/download/${VERSION}/Furx.app.tar.gz"
        }
    }
}
print(json.dumps(manifest, indent=2))
PY

echo ""
echo "✅ Release artifacts ready:"
echo "   - App:      $APP"
echo "   - Tarball:  $TARBALL"
echo "   - Sig:      $SIG"
echo "   - Manifest: $ROOT/latest.json"
echo ""
echo "Next steps (human):"
echo "   1. Revisar latest.json"
echo "   2. (Opcional) Notarizar: ./scripts/notarize.sh"
echo "   3. gh release create $VERSION $APP.dmg $TARBALL $SIG --title \"Furx $VERSION\" --notes-file $NOTES_FILE"
echo "   4. gh release upload $VERSION $ROOT/latest.json --clobber"
