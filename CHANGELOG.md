# Changelog

All notable changes to Furx are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Open-source release groundwork: Apache-2.0 `LICENSE`/`NOTICE`, `CONTRIBUTING.md`, `SECURITY.md`,
  `CODE_OF_CONDUCT.md`, issue/PR templates, `OPEN-CORE.md`, `ROADMAP.md`, `SUPPORT.md`.

### Changed
- Documentation restructured for public use; build/architecture/feature detail moved under `docs/`.

## [0.2.0]

### Added
- **Council Mode** — dispatch one prompt to up to 6 models in parallel, with 5 presets and a merge step.
- **Memory Hub** — local JSON-RPC daemon with FTS5 full-text search and a knowledge graph.
- **Skills registry** (`~/.furx/skills` + `~/.claude/skills`).
- Smart paste (clipboard classifier), manual snapshots, per-pane Claude token meter.
- MCP server health + `tools/list` handshake; Grafana embed with host allowlist.
- Auto-update via Ed25519-signed manifests (Tauri updater).
- Mobile companion bridge (mDNS) and iOS/web companion scaffolding.

### Security
- BYOK hardening — provider keys live only in the OS keychain, never on disk or in telemetry.
- Append-only audit log enforced by SQLite triggers (UPDATE/DELETE blocked).

## [0.1.0]

### Added
- Initial desktop app: multi-pane PTY grid (zsh + agent CLIs), command palette, search, broadcast,
  cross-platform build pipeline (DMG · DEB · RPM · AppImage · MSI).

[Unreleased]: https://github.com/hernaninverso/furx/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hernaninverso/furx/releases/tag/v0.2.0
[0.1.0]: https://github.com/hernaninverso/furx/releases/tag/v0.1.0
