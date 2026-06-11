# Build from source

```bash
git clone https://github.com/hernaninverso/furx && cd furx
npm install
(cd src-tauri && cargo build --release)
npx tauri build --bundles app

# Optional: install as /Applications/Furx.app (macOS, dev cert only)
SRC=src-tauri/target/release/bundle/macos/Furx.app
DEST=/Applications/Furx.app
rm -rf "$DEST" && ditto "$SRC" "$DEST" && xattr -cr "$DEST"
open "$DEST"
```

CI builds DMG (universal) + DEB + RPM + AppImage + MSI on every tag matching `v*`.

## Prerequisites by platform

- **macOS** — Xcode Command Line Tools + a code-signing identity. A free Apple Developer ID works for
  local installs, or use ad-hoc signing (`codesign --sign -`) / `xattr -cr` on the built `.app` for
  unsigned dev builds. Notarized distributable builds need a paid Developer Program identity (configured
  in CI — see *Release process*).
- **Linux** — WebKitGTK + GTK dev packages for Tauri, e.g. on Debian/Ubuntu:
  ```bash
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```
  plus a recent Rust toolchain and Node 20+.
- **Windows** — the Windows SDK + WebView2 runtime (preinstalled on Windows 11) and the MSVC build
  tools (`rustup` defaults to the MSVC toolchain).

Environment overrides (for distributable installs) are documented in
[`architecture.md`](architecture.md#configuration--environment-overrides).

## Release process

1. Bump `src-tauri/tauri.conf.json:version` and `src-tauri/Cargo.toml:version`.
2. Open a PR, merge to the default branch.
3. `git tag v0.X.Y && git push --tags` → triggers `.github/workflows/release.yml`.
4. The workflow produces DMG + DEB + RPM + AppImage + MSI + signed updater `.sig` files.
5. Copy release notes from [`../CHANGELOG.md`](../CHANGELOG.md), attach assets, publish.

Release builds require signing secrets configured in CI (Apple notarization via
`APPLE_SIGNING_IDENTITY` + `APPLE_CERTIFICATE`, Azure Trusted Signing, Tauri updater key). Unsigned local
dev builds work without any of these.
