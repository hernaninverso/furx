# Security Policy

Furx is a local-first command center for running AI coding agents. Security is a
core design constraint: your provider API keys live only in the operating system's
keychain (macOS Keychain / Windows Credential Manager / libsecret on Linux), and are
**never** written to disk in plaintext, stored in any database, or sent to any
server Furx operates. We take vulnerability reports seriously and appreciate
responsible disclosure.

## Reporting a vulnerability

**Please do not open a public GitHub issue, discussion, or pull request for security
vulnerabilities.** Public disclosure before a fix is available puts all users at risk.

Instead, report privately via one of:

- **Email:** `security@furx.cloud` — preferred. Encrypt with our PGP key if you
  handle sensitive details (key fingerprint published at `/.well-known/security.txt`
  on the site).
- **GitHub Security Advisories:** use *Report a vulnerability* on the repository's
  **Security** tab to open a private advisory.

Please include, where possible:

- A description of the vulnerability and its impact.
- Steps to reproduce (proof-of-concept, affected version/commit, OS).
- Any suggested remediation.

## Response process & timeline

- **Acknowledgement:** we aim to confirm receipt within **3 business days**.
- **Triage & assessment:** an initial severity assessment within **7 business days**.
- **Fix & disclosure:** we work to ship a fix as quickly as the severity warrants
  (critical issues are prioritized). We will coordinate a disclosure timeline with
  you and credit you in the advisory unless you prefer to remain anonymous.

We ask that you give us a reasonable opportunity to remediate before any public
disclosure.

## Scope

In scope:

- **The Furx desktop application** (Tauri/Rust + React) — including the PTY
  orchestration, the append-only audit log, the Memory Hub daemon, the local mDNS
  mobile bridge, the auto-updater, and the keychain handling.
- **The Furx cloud service** — the hosted back end behind the optional, opt-in paid
  features (license verification, cloud sync of encrypted audit/skills data). Its code
  is not in this repository, but vulnerabilities in it are absolutely in scope: report
  them through the same channel.

Out of scope (report to the relevant upstream instead):

- Third-party LLM providers you connect with your own keys (Anthropic, OpenAI,
  Google, OpenRouter, etc.).
- Third-party dependencies — though we appreciate a heads-up so we can update.
- Issues that require a already-compromised host or physical access.

## Handling of user secrets (design notes)

- Provider API keys are stored exclusively in the OS keychain and are read on demand
  by the local process. They are never persisted to the local SQLite databases, never
  transmitted to any Furx-operated endpoint, and never included in telemetry or crash
  reports (which are opt-in and run through a redaction pass).
- Cloud sync (a paid, opt-in feature) uploads only **client-side-encrypted** data;
  the sync backend stores opaque ciphertext keyed by an install id and can delete it
  on request.
- The audit log is append-only (enforced by SQLite triggers) and stays on the user's
  machine unless cloud sync is explicitly enabled.

## Supported versions

Security fixes target the latest released version. We recommend always running the
most recent release; the in-app auto-updater (Ed25519-signed manifests) helps keep
you current.
