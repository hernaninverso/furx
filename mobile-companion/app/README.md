# Furx Companion — native iOS/Android shell (spec 004 F5)

Separate Tauri Mobile 2.x project that wraps the companion UI as a native app.
**Kept separate from the desktop `src-tauri/`** on purpose: the desktop app is a
terminal multiplexer (spawns PTYs) that can't run in the iOS sandbox, so it must
never be `tauri ios init`'d.

> **Building for a device:** `tauri.conf.json` ships with `developmentTeam: ""` —
> set it to your own Apple Team ID (or export `APPLE_DEVELOPMENT_TEAM`) before a
> device build. Simulator builds (`--no-sign`) need neither.

- Bundle id: `cloud.furx.companion` · iOS dev team: `{{TEAM_ID}}` (Apple Developer
  Program, `{{APPLE_ID_EMAIL}}`) · min iOS 14.
- Frontend: the static PWA at `../pwa` (`frontendDist: "../../pwa"`).

## Status (what's verified)

- ✅ `tauri ios init` → Xcode project at `src-tauri/gen/apple/app.xcodeproj` (gitignored, regenerable).
- ✅ **iOS simulator build runs**: `tauri ios build --target aarch64-sim --no-sign` → `Furx Companion.app`, installed + launched on iPhone 17 Pro (iOS 26.5), **renders the pairing UI** (Estética A). Compiles + bundles all native Rust (WS transport, Keychain, notifications).
- ✅ Native Keychain (secret) + native notifications wired and built into the iOS app.
- ⏳ Device build / TestFlight: one credential away — see below.

## ⚠️ Architecture finding — native wrap needs a Rust WS transport (mixed content)

A Tauri webview is a **secure context** (`tauri://localhost`). Secure contexts
**block plaintext `ws://`** as mixed content. The bridge is plaintext WS (council
MC-2: encryption comes from loopback / Tailscale WireGuard), so the in-webview
`WebSocket` **cannot connect** from the native app.

**Fix (next F5 task — deserves its own council):** a **Rust-side WebSocket client**
in this app connects to the desktop bridge (plaintext WS is fine from native Rust —
no browser mixed-content rule) and exchanges frames with the webview over Tauri IPC
(`invoke` + `emit`). The webview keeps the UI + the pure-JS HMAC signer; only the
transport moves to Rust. (Alternative: WSS with a pinned self-signed cert on the
bridge — conflicts with the plaintext-over-Tailscale decision; not preferred.)

> The PWA over plain http (Safari / home-screen) does NOT hit this: an insecure
> origin allows `ws://`, and the signer is **pure-JS HMAC** (`furx-sign.js`, not
> `crypto.subtle`) so it works in any context. That path is functional from a real
> phone today. The native wrap is the only one that needs the Rust transport.

## Build / run

```bash
cd mobile-companion/app
# Simulator (no signing) — VERIFIED WORKING:
npx tauri ios build --target aarch64-sim --no-sign
xcrun simctl install booted "src-tauri/gen/apple/build/arm64-sim/Furx Companion.app"
xcrun simctl launch booted cloud.furx.companion
#   or: npx tauri ios dev   (live-reload in the simulator)
```

### Device build + TestFlight — AUTOMATED (signing done via the ASC API)

With the App Store Connect API key + Issuer ID, the entire signing chain was
automated (no Xcode account, no 2FA) and **a signed App Store IPA was produced**:

1. Registered bundle id `cloud.furx.companion` (POST /v1/bundleIds).
2. Created an `IOS_DISTRIBUTION` cert from a locally-generated CSR (POST
   /v1/certificates) → built a `-legacy` PKCS12 → `security import` →
   identity "iPhone Distribution: <your name> ({{TEAM_ID}})".
3. Created + installed an `IOS_APP_STORE` provisioning profile
   "Furx Companion App Store" (POST /v1/profiles → ~/Library/MobileDevice/...).
4. Set the Xcode project to **manual signing** (CODE_SIGN_STYLE=Manual, the
   distribution identity + profile) so `tauri ios build` needs no account.
5. `tauri ios build --export-method app-store-connect` → archive OK; export via a
   hand-written `exportOptions.plist` (manual signing) → **`Furx Companion.ipa`**.

**The ONE remaining step is Apple-gated (web UI only — the ASC API forbids app
creation, 403 "apps does not allow CREATE"):** create the app record once at
App Store Connect → Apps → ＋ → New App → bundle id `cloud.furx.companion`,
name "Furx Companion", SKU `furxcompanion2026`, primary language English.

Then upload (fully automatable, no 2FA):
```bash
cd mobile-companion/app/src-tauri/gen/apple
xcrun altool --upload-app -t ios -f "build/ipa/Furx Companion.ipa" \
  --apiKey {{ASC_KEY_ID}} --apiIssuer {{ASC_ISSUER_ID}}
```

<details><summary>Original blocker (resolved)</summary>

#### Device build + TestFlight — blocked on ONE credential

Present on this Mac: an Apple Development cert, your Team (`{{TEAM_ID}}`), an
App Store Connect API key `AuthKey_{{ASC_KEY_ID}}.p8` (key id `{{ASC_KEY_ID}}`), the
target iOS platform installed, a registered iPhone.

**Missing: the App Store Connect API _Issuer ID_** (a UUID, App Store Connect →
Users and Access → Integrations → Keys). With it, signing is fully automatable —
no Xcode account sign-in, no manual profiles:

```bash
KEY={{ASC_KEY_ID}}
P8=~/.appstoreconnect/private_keys/AuthKey_{{ASC_KEY_ID}}.p8
ISSUER={{ASC_ISSUER_ID}}            # the issuer UUID from App Store Connect

# 1. Archive + sign (auto-creates the distribution cert + provisioning profile,
#    registers the bundle id cloud.furx.companion):
npx tauri ios build --export-method app-store-connect \
  -- -allowProvisioningUpdates \
     -authenticationKeyID $KEY -authenticationKeyIssuerID $ISSUER \
     -authenticationKeyPath $P8

# 2. The App Store Connect app record for cloud.furx.companion must exist
#    (create via the API with the same key, or once in the web UI).

# 3. Upload the .ipa to TestFlight:
xcrun altool --upload-app -t ios -f <path-to>.ipa \
  --apiKey $KEY --apiIssuer $ISSUER
```

(Notary app-specific password, if needed for direct distribution instead, lives in
your OS Keychain — e.g. a `apple-notary-pwd` entry.)

</details>

## Native enhancements (phase 5b)
- ✅ Native Keychain for the pairing secret (vs the PWA's localStorage) — done.
- ✅ Native local notifications (`tauri-plugin-notification`) — done.
- ✅ SFSpeechRecognizer native voice — done. Local plugin `tauri-plugin-voice`
  (`start_listening`/`stop_listening`, on-device). Mic test needs a real device.
- ✅ Live Activity ("Claude is waiting for input") — done. `FurxWidgets` WidgetKit
  extension (Lock Screen + Dynamic Island), added by `ios-live-activity/add_widget_target.rb`
  (re-run after `tauri ios init`). Plugin `start_live_activity`/`stop_live_activity`.
  Signed (app+extension) + uploaded to TestFlight. Lock-Screen render needs a device.
