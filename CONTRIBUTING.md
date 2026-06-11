# Contributing to Furx

Thanks for your interest in Furx — a local-first command center for running multiple
AI coding agents (Claude Code, Codex, Gemini, Aider, and any OpenAI-compatible
endpoint) side by side, with shared memory, an append-only audit log, and BYO-keys.

Furx is built with **Tauri 2 + Rust** (desktop shell, orchestration, PTY, storage)
and **React 19 + Vite 6** (UI). It is published by INVERSO HUB S.R.L. under the
[Apache License 2.0](LICENSE).

We welcome contributions of all sizes — bug reports, docs, tests, and code.

---

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By
participating you agree to uphold it. Report unacceptable behaviour to
`conduct@furx.cloud`.

---

## Open-core boundary (read this first)

Furx is **open-core**. The **desktop client** — this entire repository — is
Apache-2.0 licensed; a small set of commercial client features is gated by a
license key checked **at runtime**. The **Furx cloud service** (the hosted back
end behind the Pro sync/backup conveniences) is a hosted service and lives
outside this repo. Nothing local depends on it. See [`OPEN-CORE.md`](OPEN-CORE.md).

- **Core (Apache-2.0, contributions very welcome):** the orchestrator, multi-pane
  PTY grid, Council Mode dispatch, the cards rail, the append-only audit log, the
  Memory Hub, the skills registry, and the entire **BYO-keys** engine. Everything a
  developer needs to run agents locally is here and free, forever.
- **Paid tiers (code is in the tree, gated by a license key):** cloud sync,
  Cost-Router auto-divert, and the Team/Enterprise admin features. The gating logic
  is visible and reviewable; it simply checks a license entitlement before enabling
  the feature.

When proposing a change, please target the **core** unless you are explicitly
fixing or improving the gating logic. Contributions that move a paid feature into
the core (ungating it) will not be merged, but bug fixes and improvements to any
part of the tree are welcome.

---

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| **Rust** | stable (latest via `rustup`) | the `src-tauri/` crate targets stable Rust |
| **Node.js** | **23.6+** | required by the build; see `engines` in `package.json` |
| **npm** | bundled with Node | or use `pnpm`/`bun` if you prefer, but CI uses npm |
| **Tauri system deps** | per platform | see below |

Platform-specific system dependencies for Tauri:

- **macOS** — Xcode Command Line Tools (`xcode-select --install`). A signing
  identity is only needed for distributable/notarized builds; unsigned dev builds
  work with ad-hoc signing.
- **Linux** — WebKitGTK + GTK dev packages, e.g. on Debian/Ubuntu:
  ```bash
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```
- **Windows** — the Windows SDK + WebView2 runtime (preinstalled on Windows 11)
  and the MSVC build tools (`rustup` defaults to the MSVC toolchain).

---

## Getting started

```bash
# 1. Fork + clone
git clone https://github.com/<your-fork>/furx && cd furx

# 2. Install JS dependencies
npm install

# 3. Build + run the Rust test suite
(cd src-tauri && cargo test)

# 4. Run the app in dev mode (Vite + Tauri, hot reload)
npx tauri dev
```

If `npx tauri dev` cannot find tooling installed via Homebrew (e.g. `sox`,
`whisper-cli`), make sure those binaries are on your `PATH`.

---

## Running tests

Furx has two test suites — keep both green before opening a PR.

```bash
# Rust (Tauri backend, orchestration, storage, audit log, append-only triggers)
cd src-tauri && cargo test

# Frontend (React components, libs, PWA, a11y) via vitest + node test runners
npm test                 # full JS suite (node test runners + PWA)
npm run test:components   # vitest component/unit tests
npm run test:a11y         # accessibility checks
npm run test:watch        # vitest in watch mode while developing
```

When you add a new Tauri command, remember to also register it in the command
registry so the coverage test stays green (the test re-parses the
`generate_handler![...]` block and requires 1:1 coverage).

---

## Coding style

- **Rust:** format with `cargo fmt` and keep `cargo clippy` clean. Prefer small,
  focused modules under `src-tauri/src/services/`.
- **TypeScript/React:** keep `tsc -b` (run via `npm run build`) green — type errors
  fail CI. Match the existing style of the file you are editing.
- **Migrations:** SQLite migrations are ordered files under `src-tauri/migrations/`.
  Append a new file; never rewrite an applied one.
- **No hard-coded infrastructure.** Endpoints default to `localhost`/empty and are
  configured per-user at runtime. Do not bake any host, IP, account, or personal
  data into source or migrations.
- **Privacy & secrets:** BYO-keys is sacred — provider keys live only in the OS
  Keychain and never touch disk, the database, or any network call Furx makes on the
  user's behalf. Do not add code that logs, serializes, or transmits secrets.

---

## Commits & Developer Certificate of Origin (DCO)

Furx uses the [Developer Certificate of Origin](https://developercertificate.org/).
By signing off on a commit you certify that you wrote the patch or otherwise have
the right to submit it under the project's license.

Sign off every commit with `-s`:

```bash
git commit -s -m "fix: handle empty AIE endpoint gracefully"
```

This appends a `Signed-off-by: Your Name <you@example.com>` trailer. Commits
without a sign-off may be asked to amend before merge.

Commit message guidelines:

- Use [Conventional Commits](https://www.conventionalcommits.org/) prefixes where
  natural: `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`.
- A short imperative summary line (≤ 72 chars), then a body explaining *why* if the
  change is non-trivial.

---

## Pull requests

1. Branch off `main` (or `master`): `git checkout -b fix/short-description`.
2. Make your change, add/adjust tests, run both test suites.
3. Push and open a PR against the upstream repo. Fill in the
   [PR template](.github/PULL_REQUEST_TEMPLATE.md) checklist.
4. Keep PRs focused — one logical change per PR is much easier to review.
5. A maintainer will review. CI must be green (Rust + JS) before merge.

---

## Reporting bugs & requesting features

- **Bugs:** open an issue using the [Bug report](.github/ISSUE_TEMPLATE/bug_report.md)
  template.
- **Features / ideas:** open an issue using the
  [Feature request](.github/ISSUE_TEMPLATE/feature_request.md) template, or start a
  thread in GitHub Discussions.
- **Security vulnerabilities:** **do not** open a public issue. Follow the process
  in [SECURITY.md](SECURITY.md) and email `security@furx.cloud`.

---

## License of contributions

Unless you explicitly state otherwise, any contribution you submit for inclusion in
Furx is licensed under the [Apache License 2.0](LICENSE), with no additional terms
or conditions, per section 5 of the license.
