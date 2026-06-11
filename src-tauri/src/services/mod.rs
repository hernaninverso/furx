// Services — feature modules added during 2026-05-25 sprint.
// Each service is a stateless module called from `commands.rs`.

pub mod acp; // 019 F4 (T040) — cliente ACP (Agent Client Protocol) MÍNIMO detrás de agents.rs (FR-012).
pub mod acp_registry; // 028 F0 — registro declarativo de agentes ACP (reemplaza el bin hardcodeado).
pub mod active_pack;
pub mod agent_memory;
pub mod corpus_memory;
pub mod platform;
pub mod agent_profiles;
pub mod agents; // 019 F1 (T010) — abstracción agent-neutral (descriptor + spawn-routing en worktree, R1/FR-011).
pub mod aie;
pub mod aie_endpoint;
pub mod aie_repl;
pub mod attempt_checkpoint; // 019 F0 (T005) — checkpoint-por-attempt + kill-switch transaccional del worktree (R4).
pub mod attention; // 030 F0 — voz como lente del inbox: cola con prioridad + foco humano-otorgado (núcleo puro).
pub mod audio_attention; // 031 F1a — audio opt-in para la cola de atención: gate de emisión + cola serial acotada (núcleo puro).
pub mod notify_attention; // 033 U4 — notificación nativa en background cuando un pane reclama y la ventana no tiene foco (núcleo puro + Notifier).
pub mod audit_retention; // 019 T024 — retención + exportabilidad del audit del flujo review (export-then-rotate, FR-005).
pub mod best_of_n; // 019 F1 (T011) — orquestación dedicada del flujo best-of-N (N attempts aislados, progreso vivo, falla parcial sin huérfanos, FR-001/002).
pub mod bg_queue;
pub mod bisect;
pub mod bootstrap;
pub mod bundle;
pub mod capability; // 015 US4 — capability/approval gate + secret provider (BYOK).
pub mod claude_accounts;
pub mod claude_usage;
#[cfg(test)]
mod claude_usage_smoke;
pub mod clipboard;
pub mod cloud_client;
pub mod cloud_commands;
pub mod cloud_sanitizer;
pub mod cloud_uploader;
pub mod codebase_index;
pub mod command_registry;
pub mod cost_router;
pub mod cost_router_v2; // 052 — Cost-Router Classifier v2 (bandit-ready): score ponderado + bandit ε-greedy + circuit breaker + canary. TODO OFF detrás de FURX_COST_ROUTER_MODE.
pub mod council;
pub mod configure; // 060 v2 — contrato "tu CLI configura Furx" (allowlist segura + dry-run + audit).
pub mod council_multi;
pub mod crash_log;
pub mod crl; // 050 Ola 8 P2 (FR-005) — CRL con señalización activa (mata spans vivos al revocar una key).
pub mod dag;
pub mod diff_preview;
pub mod diff_review;
pub mod disagreement;
pub mod done_detection;
pub mod embeddings;
pub mod eval_runner;
pub mod event_bus; // 015 US3 — state sync layer / typed event bus (Rust SSOT → all windows).
pub mod explain;
pub mod gh_panel;
pub mod heatmap;
pub mod http_client;
pub mod identity; // 041 Ola 1 — actor/account único multi-usuario (current_actor + installation_id + keychain_account).
pub mod keychain;
pub mod keychain_bearer; // 039 — consolidated cached accessor for the aie-internal-bearer secret.
pub mod layout_config; // 015 US6 — layout config versionada + multi-window-ready
pub mod license;
pub mod mcp_health;
pub mod mcp_inject;
// 045 FR-002 (Ola 5 P1) — overrides de MCP servers (DB SSOT en runtime) + auto-discovery $PATH.
pub mod mcp_overrides;
pub mod memory_autocapture; // 023 F1 — auto-captura post-sesión (scrub→destilar→propuestas).
pub mod memory_daemon;
pub mod mention;
pub mod merge_watcher;
pub mod meta_decision; // 020 — AIE advisory meta-decisions del orquestador (done-detection).
pub mod mobile_bridge;
pub mod mobile_qr_pairing; // 065 — pairing por QR del companion (token efímero, council v4)
pub mod net_proxy;
pub mod orchestration;
pub mod pairing;
pub mod pane_templates;
pub mod plugin_host;
pub mod pipeline; // 029 F0 — pipelines de orquestación declarativos (YAML → create_batch).
pub mod pipeline_scheduler; // 038 — scheduler event-driven del DAG (avance al `done` humano).
pub mod plugins;
pub mod skill_manifest; // 043 F1 — Skills híbrido: trust gate (payload/signature split, NFC tree_hash, revoked keys).
pub mod skill_registry; // 043 F2 — Skills híbrido: registry SQLite (estados + WAL/FULL + BEGIN IMMEDIATE retry + recovery).
pub mod skill_import; // 043 F3 — Skills híbrido: import TOCTOU-safe (flock + staging + gate-en-memoria + rename atómico + install-only).
pub mod skill_discovery; // 043 F4 — Skills híbrido: discovery (furx-core índice firmado + scan local Hermes/OpenClaw vía sources.user.toml).
pub mod skill_update; // 046 F1 (Ola 7 Skills P1) — update versionado: plugins/<name>/versions/<tree_hash>/ + symlink current (swap atómico) + rollback + GC.
pub mod skill_fastpath; // 046 F2 (Ola 7 Skills P1) — fast-path cache: snapshot por-archivo (rel_path,inode,mtime,size) → salta el rehash si nada cambió (fail-safe rehashea ante duda).
pub mod skill_mcp_registry; // 046 F3 (Ola 7 Skills P1) — discovery del MCP Registry oficial (server.json) como SUGERENCIAS; fail-closed (nada se instala/ejecuta sin el gate de la Ola 4).
pub mod policy; // 027 F0 — policy-as-code: motor + reglas default (cero regresión del gate).
pub mod pr_description;
pub mod preference_prior; // 026 F1 — prior local explicable (Beta por feature, cold-start, decay) por contexto.
pub mod preference_signal; // 026 F0/F1 — deriva/persiste la señal de preferencia + combina prior↔ranking advisory (020).
pub mod procedural_gotchas; // 025 F0/F1 — loop de gotchas procedurales (fallo->fix -> lección -> inyección).
pub mod process_manager;
pub mod variant_features; // 026 F0 — features objetivas por variante (diff-stat/risky-paths + adapt quality_gate 024).
pub mod projects;
pub mod provider_latency;
pub mod providers;
pub mod pty_lease; // 018 Fase 2 B0 — PtyLeaseRegistry (binding UI↔PTY único por panel_id) + cierre transaccional.
pub mod quality_gate; // 024 F0 — motor de evidencia objetiva por variante (linters/typecheck, sandbox + fail-safe).
pub mod multi_sync; // 050 Ola 8 P2 (FR-001) — multi-machine sync (LWW (updated_at, installation_id), opt-in, fail-closed).
pub mod quick_notes;
pub mod reliability; // 050 Ola 8 P2 (FR-003) — reliability board (éxito/latencia/costo por agente/modelo, opt-in, observacional).
pub mod remote_control;
pub mod replay;
pub mod replay_scrub;
pub mod resilience;
pub mod review; // 019 F0 (T002) — modelo de estado del flujo best-of-N gobernado + diff/review (núcleo puro).
pub mod review_audit; // 019 F0 (T001) — audit append-only del flujo review (vínculo audit↔change_set/hunk/approval, R2).
pub mod router_viz;
#[cfg(feature = "wasm-runtime")]
pub mod runtime_wasm;
pub mod savings_meter; // 048 — Cost-Router Fase 1 (Savings Meter): mide el ahorro del routing local/free/premium (append-only, async fire-and-forget). NO desvía.
pub mod screens; // 018 Fase 2 US3 (T030) — multi-monitor placement (núcleo puro: resolve_placement / target_screen).
pub mod search;
pub mod settings_registry;
pub mod signals;
pub mod skills;
pub mod smartpaste;
pub mod snapshot;
pub mod snippets;
pub mod ssh_config;
pub mod suggest;
pub mod sync_state;
pub mod telegram;
pub mod telegram_inbound;
pub mod themes;
pub mod time_tracking;
pub mod tmux_watchdog;
pub mod tts;
pub mod voice;
pub mod vpn;
pub mod web_companion;
pub mod whisper;
pub mod wizard; // 042 FR-002 — onboarding: validar/guardar endpoints del usuario + health-check.
pub mod window_byok; // 018 Fase 2 B0 (T064) — enforcement BYOK por-ventana + boundary asserts (FR-015).
pub mod window_reattach; // 018 Fase 2 B0 (T065) — semántica determinista close/reload/corrupción.
pub mod window_registry; // 018 Fase 2 US2 (T020) — registro runtime label↔window_key↔panel_ids (cleanup sin matar procesos).
pub mod workspace;
pub mod worktree;
pub mod yesterday; // 015 US5 — headless process/task lifecycle (procesos que sobreviven a la UI)
