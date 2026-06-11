// 058 — clippy: silenciamos lints PEDANTES/ESTRUCTURALES que NO son bugs. Los de CORRECTNESS quedan
// activos (clippy = 0 fuera de estos). doc_*: formato de doc-comments INTERNOS (no user-facing).
// type_complexity / too_many_arguments: firmas legítimas (ej. pty_spawn, callbacks). should_implement_trait:
// parsers `from_str` propios. "Fixearlos" sería refactor de firmas / churn de docs riesgoso pre-launch.
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::doc_overindented_list_items)]
#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::should_implement_trait)]

mod bases;
mod commands;
pub mod db;
mod distribution;
mod export;
mod monitors;
mod pty;
pub mod services;
mod settings;

use bases::{audit::AuditWriter, router::InputRouter, scheduler::Scheduler, state::PaneStateModel};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub audit: AuditWriter,
    pub router: InputRouter,
    pub scheduler: Scheduler,
    pub pane_state: PaneStateModel,
    /// Background tasks (auto-scan, watchers) — aborted on app close.
    pub bg_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// Merge watcher handle — keeps RAII shutdown alive.
    pub merge_watcher: Arc<Mutex<Option<services::merge_watcher::MergeWatcher>>>,
    /// Telegram inbound server handle — RAII shutdown.
    pub telegram_inbound: Arc<Mutex<Option<services::telegram_inbound::InboundServer>>>,
    /// BLOQUE J ext 2 — mDNS advertiser handle for the mobile companion bridge.
    /// `None` if mdns-sd failed to start (non-fatal: companion can still
    /// connect manually with the IP from Settings → Mobile).
    pub mdns_advertiser: Arc<Mutex<Option<services::mobile_bridge::MdnsAdvertiser>>>,
    /// 004 mobile-companion — WS bridge server handle (RAII shutdown). `None`
    /// until started in setup; non-fatal if bind fails.
    pub mobile_bridge: Arc<Mutex<Option<services::mobile_bridge::MobileBridge>>>,
    /// 018 Fase 2 B0 — registro de leases del binding UI↔PTY (T060). Garantiza que
    /// un panel_id se renderiza en UNA sola webview a la vez (sin doble-binding) y
    /// descarta eventos de montajes desmontados. NO toca procesos.
    pub pty_leases: Arc<services::pty_lease::PtyLeaseRegistry>,
    /// 018 Fase 2 US2 (T020) — registro runtime de ventanas (label↔window_key↔panel_ids).
    /// Dueño del ciclo de vida de las ventanas detached a nivel proceso; cleanup al cerrar
    /// SIN matar procesos (constitución VI). Espejo en memoria de los handles OS vivos; el
    /// SSOT del árbol sigue siendo `LayoutConfigV1` (DB).
    pub windows: Arc<services::window_registry::WindowRegistry>,
    /// 018 Fase 2 US2 (T022) — mutex que SERIALIZA las transiciones de ventana
    /// (open_detached / close / reattach) para que un detach y un cierre simultáneos no
    /// corran a la vez sobre el árbol persistido. El read-modify-write del `LayoutConfigV1`
    /// (revisión monotónica) ya es transaccional en DB, pero este lock evita el churn de
    /// reintentos `stale_layout` y mantiene el reatado de cierre ATÓMICO de punta a punta.
    pub window_tx_lock: Arc<parking_lot::Mutex<()>>,
    /// 024 F1 — última evidencia de quality-gate por group_id (in-memory, advisory). El cache
    /// persistente (SQLite) se difiere a F2; en F0/F1 `quality_gate_get` lee de acá lo que
    /// `quality_gate_run` calculó. Map<group_id, Vec<VariantEvidence>>.
    pub quality_gate: Arc<parking_lot::Mutex<std::collections::HashMap<String, Vec<services::quality_gate::VariantEvidence>>>>,
    /// 030 F0-wire — cola de atención (panes que reclaman al humano, con prioridad) + foco del
    /// micrófono (humano-otorgado). Los agentes ENCOLAN; el foco lo CONCEDE el humano (witness).
    pub attention: Arc<services::attention::AttentionQueue>,
    pub mic_focus: Arc<services::attention::MicFocus>,
    /// 031 F1b — gestor de audio de avisos de la cola de atención (opt-in default OFF, serial,
    /// rate-limited, silenciable). El poller lo dispara; `callar` lo silencia.
    pub audio: Arc<services::audio_attention::AudioManager>,
    /// 048 Cost-Router Fase 1 (Savings Meter) — medidor del ahorro del routing local/free/premium.
    /// MIDE, NO desvía. `disabled()` cuando el kill-switch `FURX_COST_ROUTER` está OFF (default) →
    /// `emit` es no-op → cero regresión. El emitter es fire-and-forget (nunca bloquea el hot-path).
    pub savings_meter: Arc<services::savings_meter::SavingsMeter>,
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    // C2 — install panic hook + ensure crash dir exists. Idempotent.
    crate::services::crash_log::init();

    let furx_dir = dirs::home_dir().expect("no home dir").join(".furx");
    std::fs::create_dir_all(&furx_dir).expect("create ~/.furx");

    let db_path = furx_dir.join("furx.db");
    let conn = db::open(&db_path).expect("open db (migrations applied)");

    // 041 FR-001 — ensure the stable `installation_id` exists BEFORE any handler runs, so
    // `current_actor()`'s fallback (`user:local-<id8>`) has its anchor and never panics/leaks. Done
    // synchronously here, with the freshly-opened connection, ahead of `tauri::Builder`. Idempotent.
    {
        let id = services::identity::ensure_installation_id(&conn);
        tracing::info!(installation_id = %id, "041 identity: installation_id ready");
    }

    // 041 FR-005 — load the runtime allowlist from `network.extra_origins` SYNCHRONOUSLY, before any
    // handler or background task can issue an outbound call. If this ran after `tauri::Builder`, the
    // first outbound ping could see an empty runtime set and be rejected.
    bases::allowlist::init_from_settings(&conn);

    let db_arc = Arc::new(Mutex::new(conn));

    // 006 agent-profiles: siembra agentes built-in desde los modes legacy + una cuenta
    // Claude por slug. Idempotente (id determinístico) → seguro en cada arranque.
    {
        let claude_slugs: Vec<String> = services::claude_accounts::list_all(&db_arc)
            .map(|accts| {
                accts
                    .into_iter()
                    .filter(|a| a.cli_kind == "claude")
                    .map(|a| a.slug)
                    .collect()
            })
            .unwrap_or_default();
        if let Err(e) = services::agent_profiles::seed_builtins(&db_arc, &claude_slugs) {
            tracing::warn!("agent_profiles seed_builtins failed: {e}");
        }
    }

    // 009 aie-engine: escribir el REPL client (~/.furx/furx-chat.py) idempotente al boot.
    if let Err(e) = services::aie_repl::ensure_repl_script() {
        tracing::warn!("aie_repl ensure_repl_script failed: {e}");
    }

    let pane_state = PaneStateModel::new();
    let audit = AuditWriter::new(db_arc.clone());
    // Sprint #1 — wire the cloud uploader so audit events with LLM-trace kinds
    // (council.*, llm.*) flow to api.furx.cloud asynchronously when the user has
    // signed in. The CHANNEL is created here (sync); the consumer TASK is spawned
    // inside .setup() below where the Tauri tokio runtime is live. Until the user
    // signs in, jobs fail-fast in upload_with_retry ("not signed in") and don't retry.
    let (cloud_uploader_handle, cloud_uploader_rx) = services::cloud_uploader::create();
    audit.set_uploader(cloud_uploader_handle.clone());
    services::cloud_uploader::install_global(cloud_uploader_handle);
    // Move the rx into the setup closure (it can only be consumed once)
    let cloud_uploader_rx_opt = std::sync::Mutex::new(Some(cloud_uploader_rx));
    let bg_handles: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    let merge_watcher_slot: Arc<Mutex<Option<services::merge_watcher::MergeWatcher>>> =
        Arc::new(Mutex::new(None));
    let telegram_inbound_slot: Arc<Mutex<Option<services::telegram_inbound::InboundServer>>> =
        Arc::new(Mutex::new(None));
    let mdns_advertiser_slot: Arc<Mutex<Option<services::mobile_bridge::MdnsAdvertiser>>> =
        Arc::new(Mutex::new(None));
    let mobile_bridge_slot: Arc<Mutex<Option<services::mobile_bridge::MobileBridge>>> =
        Arc::new(Mutex::new(None));
    // 030 F0-wire — la cola de atención se crea acá para clonarla al poller de done_detection (la
    // fuente autoritativa de los pedidos de atención) además de meterla en AppState.
    let attention_setup = Arc::new(services::attention::AttentionQueue::new());
    // 031 F1b — gestor de audio. Sink real (TTS+earcon). Opt-in resuelto desde `settings` por pane
    // (default OFF) — la lectura ocurre ANTES del lock interno del gestor (sin nested-lock). Se clona
    // al poller (que lo dispara) y a AppState (que lo silencia vía `callar`).
    let audio_opt_in_db = db_arc.clone();
    // 033 U2 — el sink lee las prefs de audio (voz/rate/earcon) de `settings` por reproducción.
    let audio_prefs_db = db_arc.clone();
    let audio_sink = services::audio_attention::TtsEarconSink::with_prefs(Box::new(move || {
        let conn = audio_prefs_db.lock();
        services::audio_attention::read_audio_prefs(&conn)
    }));
    let audio_setup = Arc::new(services::audio_attention::AudioManager::new(
        Box::new(audio_sink),
        Box::new(move |pane_id: &str| {
            let conn = audio_opt_in_db.lock();
            services::audio_attention::read_opt_in(&conn, pane_id)
        }),
        Box::new(services::audio_attention::MonotonicClock::default()),
    ));
    // 048 Cost-Router Fase 1 (Savings Meter) — arranca el medidor SOLO si el kill-switch
    // `FURX_COST_ROUTER` está ON. OFF (default) ⇒ `disabled()` ⇒ `emit` no-op ⇒ cero regresión.
    // El worker de fondo (INSERT batch) se aborta al cerrar la app (vive en `bg_handles`).
    let savings_meter = if services::savings_meter::SavingsMeter::enabled() {
        let (meter, worker_handle) = services::savings_meter::SavingsMeter::start(db_arc.clone());
        bg_handles.lock().push(worker_handle);
        Arc::new(meter)
    } else {
        Arc::new(services::savings_meter::SavingsMeter::disabled())
    };
    // Instalar el singleton global para que el poller (done-detection) pueda emitir trazas sin
    // threadear el meter por toda la cadena de `process_task`. `emit_global` es no-op si OFF.
    services::savings_meter::install_global(savings_meter.clone());
    // 049 Cost-Router Fase 2 (Router ACTIVO) — instala el MODO del router (off/shadow/active) leído
    // de `FURX_COST_ROUTER_MODE`. DEFAULT `off` ⇒ el router es no-op total (no desvía ninguna
    // decisión de tier) ⇒ cero regresión. Distinto del kill-switch de Fase 1 (que prende la
    // medición). El router activo NO se enciende solo: aunque el modo sea `active`, el gate de KPI
    // sin datos productivos no pasa ⇒ no-op. el autor lo enciende cuando los datos pasen el gate.
    services::cost_router::install_mode(services::cost_router::RouterMode::from_env());
    let state = AppState {
        db: db_arc.clone(),
        audit: audit.clone(),
        router: InputRouter::new(),
        scheduler: Scheduler::new(),
        pane_state: pane_state.clone(),
        bg_handles: bg_handles.clone(),
        merge_watcher: merge_watcher_slot.clone(),
        telegram_inbound: telegram_inbound_slot.clone(),
        mdns_advertiser: mdns_advertiser_slot.clone(),
        mobile_bridge: mobile_bridge_slot.clone(),
        pty_leases: Arc::new(services::pty_lease::PtyLeaseRegistry::new()),
        windows: Arc::new(services::window_registry::WindowRegistry::new()),
        window_tx_lock: Arc::new(parking_lot::Mutex::new(())),
        quality_gate: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        attention: attention_setup.clone(),
        mic_focus: Arc::new(services::attention::MicFocus::new()),
        audio: audio_setup.clone(),
        savings_meter,
    };
    let pty_mgr = Arc::new(pty::PtyManager::new(pane_state.clone()));
    // 004 mobile-companion: the bridge shares the SAME PtyManager so it can
    // write to / snapshot the live panes. Clone the Arc before `.manage()` moves it.
    let pty_mgr_bridge = pty_mgr.clone();

    // Heartbeat ticker — corre la FSM cada 60s (Busy→Ready→Idle según actividad).
    let pane_state_tick = pane_state.clone();
    std::thread::Builder::new()
        .name("pane-state-ticker".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            pane_state_tick.tick();
        })
        .expect("ticker spawn");

    // Monitors store (last result per target) compartido entre el poller y el frontend.
    let monitors_state = Arc::new(Mutex::new(std::collections::HashMap::<
        String,
        monitors::MonitorResult,
    >::new()));

    tauri::Builder::default()
        // C1 — Auto-update + process plugins. (dialog added in C4.)
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Sprint #5 — register the furx:// URL scheme so magic-link emails open the
        // app directly. Single-instance is handled by the plugin on macOS/Windows;
        // Linux requires bundling a .desktop file (covered by tauri-plugin-deep-link's
        // bundler). The handler is installed in .setup() once the runtime is alive.
        .plugin(tauri_plugin_deep_link::init())
        // 010-furx-signals — desktop notifications (DesktopNotifSink). BYOK-clean, local.
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .manage(pty_mgr)
        .manage(monitors_state.clone())
        .setup(move |app| {
            // 018 Fase 2 US2 (T020) — registrar la ventana Main en el WindowRegistry al boot
            // (idempotente). Las detached se registran en `window_open_detached`. El cleanup
            // al cerrar NUNCA mata procesos (constitución VI).
            if let Some(st) = app.try_state::<AppState>() {
                st.windows.register_main();
            }
            // Defer ALL background work until after the window is shown. macOS 26.2 +
            // Tauri 2.11.2 with WKWebView is sensitive to heavy work during setup; if
            // any startup task panics, WebKit's URL scheme handler can SIGABRT the
            // whole process. We move everything into a single spawned task that waits
            // 2s for the window to paint, then starts each subsystem behind catch_unwind.
            let app_handle_setup = app.handle().clone();
            let store_setup = monitors_state.clone();
            let bg_handles_setup = bg_handles.clone();
            let merge_watcher_setup = merge_watcher_slot.clone();
            let telegram_inbound_setup = telegram_inbound_slot.clone();
            let mdns_advertiser_setup = mdns_advertiser_slot.clone();
            let mobile_bridge_setup = mobile_bridge_slot.clone();
            let pty_mgr_setup = pty_mgr_bridge.clone();
            let pane_state_setup = pane_state.clone();
            let db_arc_setup = db_arc.clone();
            let audit_setup = audit.clone();

            // Sprint #1 — spawn the cloud uploader task NOW that the tokio runtime is live.
            // The handle was already injected into AuditWriter at lib startup; the loop
            // simply waits on the channel and POSTs to api.furx.cloud as events arrive.
            if let Some(rx) = cloud_uploader_rx_opt.lock().expect("uploader rx mutex").take() {
                services::cloud_uploader::spawn_uploader_task(rx);
            }

            // Sprint #5 — register the furx://auth?token=... deep-link handler so
            // magic-link emails open Furx directly. Token is extracted, verified, and
            // the default project is bootstrapped (same flow as the manual paste path).
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let app_for_dl = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        tracing::info!("deep-link received: {}", url);
                        // L7: shared, unit-tested parser (furx://auth?token=X and the
                        // localhost-normalized variant). Returns None for wrong scheme /
                        // non-auth path / missing token.
                        let Some(token) = crate::services::cloud_client::parse_auth_token(url.as_str()) else { continue; };
                        let app_h = app_for_dl.clone();
                        tauri::async_runtime::spawn(async move {
                            match crate::services::cloud_client::verify(&token).await {
                                Ok(user) => {
                                    tracing::info!("deep-link verify ok: user={}", user.email);
                                    if let Err(e) = crate::services::cloud_commands::cloud_bootstrap_default_project().await {
                                        tracing::warn!("bootstrap default project after deep-link: {}", e);
                                    }
                                    use tauri::Emitter;
                                    let _ = app_h.emit("cloud:signed-in", &user);
                                }
                                Err(e) => {
                                    tracing::warn!("deep-link verify failed: {}", e);
                                    use tauri::Emitter;
                                    let _ = app_h.emit("cloud:signin-failed", e.to_string());
                                }
                            }
                        });
                    }
                });
            }

            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;

                // 038 F1.4 — reconcile_on_boot: re-evaluar el DAG de cada run de pipeline `running` al
                // arranque (resume tras restart sin doble-spawn; re-dispara cascadas pendientes de un
                // crash). Behind catch_unwind + spawn_blocking (toca la DB sync). Idempotente; NO lanza
                // nada (sólo desbloquea/cancela en DB).
                {
                    let db_reconcile = db_arc_setup.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            match crate::services::pipeline_scheduler::reconcile_on_boot(&db_reconcile) {
                                Ok(n) => {
                                    if n > 0 {
                                        tracing::info!("pipeline reconcile_on_boot: {n} run(s) re-evaluados");
                                    }
                                }
                                Err(e) => tracing::warn!("pipeline reconcile_on_boot falló: {e}"),
                            }
                        }));
                    })
                    .await;
                }

                // Monitor poller (045 FR-001) — supervisor tick cada 10s; cada target se chequea
                // cuando vence su `interval_s` (default 30s). Los targets salen de la DB
                // (`monitor_targets`), NO del hardcode: el usuario los agrega/quita por UI. Las
                // checks corren tras un Semaphore (cap 10) para no spawnear 1000 tasks con 1000
                // targets. `last_checked` lleva el due-time por id en memoria (no toca la DB en cada
                // tick). Si la DB no tiene targets, el ciclo es vacío y reintenta — nunca paniquea.
                let monitors_handle = {
                    let app_h = app_handle_setup.clone();
                    let store = store_setup.clone();
                    let db_mon = db_arc_setup.clone();
                    tauri::async_runtime::spawn(async move {
                        use std::collections::HashMap as StdHashMap;
                        use tokio::sync::Semaphore;
                        let sem = Arc::new(Semaphore::new(monitors::MAX_CONCURRENT_CHECKS));
                        // due_at[id] = instante en el que toca el próximo check de ese target.
                        let mut due_at: StdHashMap<String, std::time::Instant> = StdHashMap::new();
                        let mut interval = tokio::time::interval(Duration::from_secs(10));
                        loop {
                            interval.tick().await;
                            let now = std::time::Instant::now();
                            let targets = monitors::load_targets(&db_mon);
                            // Limpia due-times de targets que ya no existen (borrados por UI).
                            let live: std::collections::HashSet<&str> =
                                targets.iter().map(|t| t.id.as_str()).collect();
                            due_at.retain(|id, _| live.contains(id.as_str()));
                            for t in targets {
                                let due = *due_at.get(&t.id).unwrap_or(&now);
                                if now < due {
                                    continue;
                                }
                                // Re-agenda el próximo check de ESTE target por su interval.
                                due_at.insert(
                                    t.id.clone(),
                                    now + Duration::from_secs(t.interval_s.max(5)),
                                );
                                let sem = sem.clone();
                                let store = store.clone();
                                let app_h = app_h.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _permit = match sem.acquire().await {
                                        Ok(p) => p,
                                        Err(_) => return, // semaphore cerrado (shutdown).
                                    };
                                    let r = monitors::check(&t).await;
                                    store.lock().insert(r.id.clone(), r.clone());
                                    use tauri::Emitter;
                                    let _ = app_h.emit("monitor:result", r);
                                });
                            }
                        }
                    })
                };
                bg_handles_setup.lock().push(monitors_handle);

                // Projects auto-scan hourly.
                let scan_handle = {
                    let db_scan = db_arc_setup.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(20)).await;
                        loop {
                            let db_for_task = db_scan.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let _ = services::projects::scan(db_for_task);
                                }));
                            }).await;
                            tokio::time::sleep(Duration::from_secs(3600)).await;
                        }
                    })
                };
                bg_handles_setup.lock().push(scan_handle);

                // Merge watcher — panic-safe.
                let merge_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    services::merge_watcher::MergeWatcher::start(
                        app_handle_setup.clone(), db_arc_setup.clone(), audit_setup.clone(),
                    )
                }));
                match merge_res {
                    Ok(Ok(w)) => { *merge_watcher_setup.lock() = Some(w); }
                    Ok(Err(e)) => tracing::warn!("merge_watcher start failed: {}", e),
                    Err(_) => tracing::warn!("merge_watcher panicked during start"),
                }

                // Telegram inbound — opt-in.
                let endpoint = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    settings::get(&db_arc_setup.lock(), "endpoints.telegram_relay")
                        .ok().flatten()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default()
                })).unwrap_or_default();
                if !endpoint.is_empty() {
                    if let Some(secret) = services::telegram::read_secret() {
                        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            services::telegram_inbound::InboundServer::start(
                                app_handle_setup.clone(), db_arc_setup.clone(), audit_setup.clone(), secret,
                                Some(pty_mgr_setup.clone()),
                            )
                        }));
                        match res {
                            Ok(Ok(s)) => { *telegram_inbound_setup.lock() = Some(s); }
                            Ok(Err(e)) => tracing::warn!("telegram_inbound start failed: {}", e),
                            Err(_) => tracing::warn!("telegram_inbound panicked during start"),
                        }
                    } else {
                        tracing::info!("telegram_inbound skipped — Keychain secret missing");
                    }
                }

                // Boot restore probe.
                let sessions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    services::tmux_watchdog::list_furx_sessions()
                })).unwrap_or_default();
                if !sessions.is_empty() {
                    use tauri::Emitter;
                    let _ = app_handle_setup.emit("furx:boot-restore", serde_json::json!({"sessions": sessions}));
                }

                // B2 2.8 — bg_queue worker (per V5 council foundation).
                let (_bg_tx, bg_rx) = tokio::sync::oneshot::channel::<()>();
                let db_bg = db_arc_setup.clone();
                let audit_bg = audit_setup.clone();
                let bg_handle = tauri::async_runtime::spawn(async move {
                    services::bg_queue::worker_loop(db_bg, audit_bg, bg_rx).await;
                });
                bg_handles_setup.lock().push(bg_handle);
                std::mem::forget(_bg_tx);

                // 010-furx-signals — dispatcher persistente. Worker tokio que cada 5s
                // materializa deliveries de eventos nuevos + reintenta pending/failed con
                // backoff. La verdad vive en SQLite → sobrevive reinicios (los eventos
                // pendientes se completan al re-arrancar). Los sinks se reconstruyen cada
                // tick para tomar cambios de config/Keychain (BYOK). El DesktopNotifSink usa
                // el AppHandle para el toast nativo (reemplaza al DesktopBusSink de tests).
                let db_signals = db_arc_setup.clone();
                let app_signals = app_handle_setup.clone();
                let signals_handle = tauri::async_runtime::spawn(async move {
                    services::signals::run_router_loop(
                        db_signals,
                        std::time::Duration::from_secs(5),
                        move |db| {
                            vec![
                                Box::new(services::signals::DesktopNotifSink { app: app_signals.clone() }),
                                Box::new(services::signals::MobileSink),
                                Box::new(services::signals::TelegramSink::from_db(db)),
                                Box::new(services::signals::WebhookSink::from_db(db)),
                            ]
                        },
                    )
                    .await;
                });
                bg_handles_setup.lock().push(signals_handle);

                // 012-pty-done-detection — poller del ciclo de vida. Worker tokio que cada 2s
                // lee el buffer-tail de la pane de cada tarea `running` (008), la clasifica
                // (spinner→running, prompt-vacío→idle, trust→needs_input) y auto-transiciona a
                // awaiting_review (+diff) o emite agent.input_requested (010). Auto-confirm es
                // opt-in (default OFF). Sólo tareas running, lee buffer existente (no re-spawn)
                // → cero impacto perceptible. Comparte el MISMO PtyManager (snapshot + write).
                let db_poller = db_arc_setup.clone();
                let audit_poller = audit_setup.clone();
                let pane_poller: Arc<dyn services::done_detection::PaneBuffer> = pty_mgr_setup.clone();
                let attention_poller = attention_setup.clone();
                let audio_poller = audio_setup.clone();
                // 033 U4 — gestor de notificaciones en background (opt-in default OFF). Notifier real
                // con AppHandle (foco de ventana + plugin de notificaciones). Sólo lo usa el poller.
                let notify_db = db_arc_setup.clone();
                let notify_sound_db = db_arc_setup.clone();
                let notify_poller = Arc::new(services::notify_attention::NotificationManager::new(
                    Box::new(services::notify_attention::TauriNotifier {
                        app: app_handle_setup.clone(),
                        sound: Box::new(move || {
                            let conn = notify_sound_db.lock();
                            services::notify_attention::read_notify_sound(&conn)
                        }),
                    }),
                    Box::new(services::audio_attention::MonotonicClock::default()),
                    Box::new(move || {
                        let conn = notify_db.lock();
                        services::notify_attention::read_notify_enabled(&conn)
                    }),
                ));
                let poller_handle = tauri::async_runtime::spawn(async move {
                    services::done_detection::run_poller_loop(
                        db_poller,
                        audit_poller,
                        pane_poller,
                        Duration::from_secs(2),
                        attention_poller,
                        audio_poller,
                        notify_poller,
                    )
                    .await;
                });
                bg_handles_setup.lock().push(poller_handle);

                // FASE 2 — Memory Hub daemon (background HTTP + MCP server).
                let db_mem = db_arc_setup.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = services::memory_daemon::start(db_mem).await {
                        tracing::warn!("memory_daemon start failed: {}", e);
                    }
                });

                // BLOQUE J ext 2 — mDNS advertise the mobile bridge port so the
                // future iOS/Android companion auto-discovers this desktop on the
                // LAN. Non-fatal: app keeps running if mdns-sd can't start
                // (port 5353 busy, no interfaces, etc).
                match services::mobile_bridge::start_mdns_advertise() {
                    Ok(adv) => { *mdns_advertiser_setup.lock() = Some(adv); }
                    Err(e) => { tracing::warn!("mdns advertiser disabled: {}", e); }
                }

                // 004 mobile-companion — WS bridge. Loopback always; Tailscale
                // interface (:43119) iff `mobile.tailscale_enabled`. Non-fatal:
                // catch_unwind + skip on any failure (keychain, bind, etc).
                let bridge_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let secret = services::mobile_bridge::ensure_secret()?;
                    let tailscale_enabled = settings::get(&db_arc_setup.lock(), "mobile.tailscale_enabled")
                        .ok()
                        .flatten()
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let addrs = services::mobile_bridge::bridge_bind_addrs(tailscale_enabled);
                    services::mobile_bridge::MobileBridge::start(
                        app_handle_setup.clone(),
                        pty_mgr_setup.clone(),
                        pane_state_setup.clone(),
                        db_arc_setup.clone(),
                        audit_setup.clone(),
                        secret,
                        addrs,
                    )
                }));
                match bridge_res {
                    Ok(Ok(b)) => { *mobile_bridge_setup.lock() = Some(b); }
                    Ok(Err(e)) => tracing::warn!("mobile_bridge start failed: {}", e),
                    Err(_) => tracing::warn!("mobile_bridge panicked during start"),
                }

                // B5 2.40 — web companion mobile server loopback only.
                // Box::leak para mantener el handle vivo todo el proceso (shutdown
                // implícito al cerrar la app — Tauri runtime kills network task).
                let wc_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    services::web_companion::WebCompanion::start(db_arc_setup.clone())
                }));
                if let Ok(Ok(wc)) = wc_res {
                    Box::leak(Box::new(wc));
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let label = window.label().to_string();
                // 018 Fase 2 US2 (T022) — CIERRE DE UNA VENTANA DETACHED (X del SO).
                //
                // Una detached NO debe disparar el teardown global de la app (eso es sólo para la
                // Main). En su lugar, su cierre es TRANSACCIONAL: prevenimos el cierre, reatamos
                // sus paneles a Main (SSOT) SIN matar procesos (constitución VI), y recién después
                // dejamos cerrar la WebviewWindow. El `settle_detached_window` está serializado
                // (window_tx_lock) para no correr con un detach simultáneo, y es idempotente +
                // anti-reentrante (begin_settle): el `w.close()` re-dispara CloseRequested, pero al
                // estar ya "settling" lo dejamos cerrar sin re-procesar.
                if label != services::layout_config::MAIN_WINDOW_KEY {
                    if let Some(state) = window.try_state::<AppState>() {
                        // ¿es una detached registrada y NO settling todavía? → cierre transaccional.
                        if state.windows.contains(&label) && !state.windows.is_settling(&label) {
                            api.prevent_close();
                            let app = window.app_handle().clone();
                            let label_for_task = label.clone();
                            // La I/O de DB no debe correr en el hilo del evento → spawn.
                            tauri::async_runtime::spawn(async move {
                                let ws = services::layout_config::DEFAULT_WORKSPACE.to_string();
                                if let Some(st) = app.try_state::<AppState>() {
                                    match commands::settle_detached_window(&app, &st, &label_for_task, &ws) {
                                        // Reatado persistido → cerrar la ventana de verdad. El
                                        // CloseRequested re-entrante ve la ventana ya fuera del
                                        // registro → no re-procesa. Luego liberar la marca.
                                        Ok(true) => {
                                            if let Some(w) = app.get_webview_window(&label_for_task) {
                                                let _ = w.close();
                                            }
                                            st.windows.end_settle(&label_for_task);
                                        }
                                        // no-op (Main / ya cerrada) o la marca la posee otro settle
                                        // concurrente → no cerramos ni tocamos la marca acá.
                                        Ok(false) => {}
                                        // CRÍTICO (018 US2 audit): si el reattach NO se persistió, NO
                                        // cerramos la ventana (settle ya liberó la marca y NO la removió
                                        // del registro). La dejamos ABIERTA — un próximo CloseRequested
                                        // reintenta el reatado. Cerrar acá huérfanaría el PTY (viola VI).
                                        Err(e) => {
                                            tracing::warn!(
                                                "settle detached window {label_for_task} failed: {e}; \
                                                 ventana queda abierta para reintentar (no se cierra ni se mata PTY)"
                                            );
                                        }
                                    }
                                }
                            });
                        }
                        // Si ya está settling, NO prevenimos → el cierre procede normal.
                    }
                    return;
                }
                // CIERRE DE LA VENTANA MAIN → teardown global de la app (comportamiento previo).
                if let Some(state) = window.try_state::<AppState>() {
                    // Abort background tasks.
                    for h in state.bg_handles.lock().drain(..) {
                        h.abort();
                    }
                    // RAII shutdown of watcher + inbound server.
                    let _: Option<services::merge_watcher::MergeWatcher> = state.merge_watcher.lock().take();
                    let _: Option<services::telegram_inbound::InboundServer> = state.telegram_inbound.lock().take();
                    let _: Option<services::mobile_bridge::MobileBridge> = state.mobile_bridge.lock().take();
                }
            }
        })
        // 015 T015 — ENFORCEMENT UNIVERSAL del gate US4. Envolvemos el handler generado: TODO
        // invoke pasa por `dispatch_gate` ANTES de llegar al comando real. Un comando Destructive/
        // Credential/requires_confirmation sin un approval consumible se CORTA (crea pending +
        // emite ApprovalRequested + rechaza); con un approval consumible, se consume (single-use) y
        // se ejecuta. Aplica venga de palette/botón/plugin/móvil/deeplink — no sólo superficies que
        // optan. Safe/External pasan directo (lookup O(1), costo ~0). Fail-closed ante error/Raw.
        .invoke_handler({
            // Box<dyn Fn> fija R=Wry (el closure de generate_handler! es genérico sobre el Runtime
            // y sin esto la inferencia falla en el `let`). Una indirección por invoke, despreciable.
            let generated: Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync> = Box::new(tauri::generate_handler![
            commands::health,
            commands::list_panes,
            commands::list_cards,
            commands::list_events,
            commands::pane_state,
            commands::pty_spawn,
            commands::pty_write,
            commands::pty_resize,
            commands::pty_kill,
            commands::pty_alive,
            // 018 Fase 2 B0 (T060/T061) — lease del binding UI↔PTY (attach/detach, NO matan procesos).
            commands::pty_lease_attach,
            commands::pty_lease_detach,
            // 018 Fase 2 US2 (T021) — detach-to-window (abrir/cerrar/listar ventanas). NUNCA matan PTYs.
            commands::window_open_detached,
            commands::window_close,
            commands::window_list,
            commands::monitors_list, // 018 US3 — displays disponibles para placement multi-monitor.
            commands::list_monitors,
            // 045 FR-001 (Ola 5 P1) — monitores configurables por el usuario (CRUD sobre la DB).
            commands::monitor_add,
            commands::monitor_remove,
            commands::monitor_list,
            commands::seed_demo_cards,
            commands::decide_card,
            // spec-022 P1 · US6 — auto-unsnooze ante nueva actividad de la fuente de la card.
            commands::card_record_activity,
            commands::settings_get,
            commands::settings_set,
            commands::settings_all,
            commands::settings_registry_list,
            commands::settings_set_validated,
            // 042 FR-002 — wizard onboarding (endpoints + health-check).
            commands::setup_health_check,
            commands::wizard_save_endpoints,
            commands::mobile_secret_get,
            commands::mobile_secret_rotate,
            commands::mobile_bridge_status,
            // 065 — pairing por QR del companion.
            commands::mobile_pairing_qr_generate,
            commands::mobile_pairing_status,
            // 017 mobile-companion nav — desktop host registers the materialized NavSpec.
            services::mobile_bridge::mobile_bridge_set_navspec,
            commands::compat_check,
            commands::check_updates,
            commands::reset_furx,
            commands::guardrail_scan,
            commands::export_state,
            commands::get_layout,
            commands::save_layout,
            services::layout_config::layout_config_get,
            services::layout_config::layout_config_save,
            commands::snapshot_take,
            commands::snapshot_list,
            commands::projects_scan,
            commands::projects_list,
            commands::bootstrap_compile,
            commands::bundle_build,
            commands::worktree_ensure,
            commands::worktree_list,
            commands::card_open_in_claude,
            commands::card_context_for,
            commands::pty_spawn_in_worktree,
            commands::worktree_merge_review,
            commands::git_overview,
            commands::claude_usage_summary,
            commands::claude_usage_for_cwd,
            commands::aie_state,
            commands::search_run,
            commands::suggest_for_text,
            commands::mcp_health,
            // 045 FR-002 (Ola 5 P1) — MCP overrides (toggle SIN tocar ~/.claude.json) + auto-discovery.
            commands::mcp_set_enabled,
            commands::mcp_overrides_list,
            commands::mcp_discover,
            commands::heatmap_data,
            commands::smartpaste_classify,
            commands::smartpaste_offer,
            commands::spec_kit_install_alias,
            commands::spec_kit_alias_status,
            commands::clipboard_read,
            commands::ssh_hosts,
            commands::ssh_ping,
            commands::whisper_check,
            commands::telegram_emit_card,
            commands::tmux_watchdog_status,
            commands::tmux_watchdog_install,
            commands::tmux_watchdog_uninstall,
            commands::tmux_list_furx_sessions,
            commands::voice_download_model,
            commands::voice_capture,
            commands::voice_transcribe,
            // council_run removido (053): obsoleto, reemplazado por council_run_multi (CouncilModal).
            commands::boot_restore_attach,
            commands::boot_restore_ui,
            commands::boot_restore_full,
            commands::tmux_available,
            commands::pty_capture_history,
            commands::home_dir,
            commands::export_state_to_desktop,
            commands::crash_log_js,
            commands::crash_log_list,
            commands::crash_log_read,
            commands::crash_log_delete,
            commands::crash_log_clear,
            commands::broadcast_audit_sent,
            commands::interpane_send_audit,
            commands::vpn_status,
            commands::vpn_up,
            commands::explain_failed,
            commands::mention_parse,
            commands::standup_today,
            commands::latency_heatmap,
            commands::latency_poll_once,
            commands::pr_description,
            commands::disagreement_analyze,
            // B2
            commands::bg_enqueue, commands::bg_list, commands::bg_cancel,
            commands::embeddings_index, commands::embeddings_search,
            commands::diff_review_run, commands::agent_memory_recall,
            commands::corpus_status, commands::corpus_search, commands::corpus_deadends, commands::corpus_ledger,
            commands::dag_parse, commands::diff_detect_blocks,
            commands::eval_list_tasks, commands::eval_run_task,
            commands::replay_buckets, commands::replay_events_at, commands::replay_bundle_create,
            commands::router_snapshot, commands::yesterday_compile,
            // B4
            commands::snippets_save, commands::snippets_list, commands::snippets_delete,
            commands::http_send, commands::time_weekly,
            commands::gh_list_prs, commands::gh_list_issues,
            commands::quick_notes_add, commands::quick_notes_list, commands::quick_notes_delete,
            commands::theme_set, commands::theme_list,
            commands::pane_template_save, commands::pane_template_list, commands::pane_template_delete,
            commands::bisect_run,
            // B5
            commands::plugins_scan, commands::plugins_install,
            commands::plugins_list, commands::plugins_set_enabled,
            // spec-043 Ola 4 — Skills híbrido con verificación (F5)
            commands::skills_trust_list, commands::skills_discover_local,
            commands::skill_import_local, commands::skill_promote,
            commands::sync_run,
            // BLOQUE 1 — Wizard Furx Connect
            commands::provider_list,
            commands::provider_get,
            commands::provider_persist,
            commands::provider_delete,
            commands::provider_test,
            commands::license_check,
            commands::license_install_id,
            commands::license_is_pro,
            // BLOQUE 2 — Wizard 5 tabs + Council multi-provider
            commands::provider_local_scan,
            commands::council_run_multi,
            commands::council_templates_list,
            // 019 F3 (T031) — council history + custom-voices (F-II: free para todos los tiers)
            commands::council_history_list,
            commands::council_history_clear,
            commands::council_custom_voices_list,
            commands::council_custom_voice_add,
            commands::council_custom_voice_set_enabled,
            commands::council_custom_voice_remove,
            // BLOQUE 3 — Resilience snapshot + preset overrides
            commands::resilience_snapshot,
            commands::preset_override_set,
            commands::preset_overrides_list,
            // 006 — Agent profiles
            commands::agent_profile_list,
            commands::agent_profile_create,
            commands::agent_profile_update,
            commands::agent_profile_delete,
            commands::agent_profile_export,
            commands::agent_profile_import,
            // 008 — Orchestration
            commands::orchestration_create_batch,
            commands::orchestration_list,
            commands::orchestration_prepare_task,
            commands::orchestration_mark_ready,
            commands::orchestration_collect,
            commands::orchestration_cancel,
            commands::orchestration_set_state,
            // 038 Goose-C P1 — ejecución del DAG de pipelines (YAML → run con deps).
            commands::pipeline_run_yaml,
            commands::pipeline_cancel,
            commands::pipeline_waiting_runs,
            // 012-pty-done-detection — auto-confirm toggle
            commands::orchestration_set_auto_confirm,
            // 014-orchestration-ux — best-of-N, pairing-sync, log-history, locks, cleanup
            commands::orchestration_create_best_of_n,
            commands::orchestration_group_tasks,
            commands::orchestration_get_group,
            commands::orchestration_compare_group,
            // 024-quality-gate F1 — evidencia objetiva por variante (linters/typecheck)
            commands::quality_gate_run,
            commands::quality_gate_get,
            commands::orchestration_choose_variant,
            commands::orchestration_discard_variant,
            // 020-aie-meta-orchestrator — US2/US3 advisory (ranking best-of-N + sugerencia de agente)
            commands::meta_suggest_variant_ranking,
            commands::meta_suggest_agent,
            // 026-preference-loop — ranking enriquecido con el prior local + gobierno del prior
            commands::meta_suggest_variant_ranking_explained,
            commands::preference_prior_inspect,
            commands::preference_prior_reset,
            commands::preference_records_list,
            commands::orchestration_pairing_sync,
            commands::orchestration_log_history,
            commands::orchestration_capture_log,
            commands::orchestration_acquire_lock,
            commands::orchestration_release_lock,
            commands::orchestration_cleanup_worktrees,
            // 019 F3 (T030) — pause/resume + ETA + live-logs (tail) a producción
            commands::orchestration_pause_task,
            commands::orchestration_resume_task,
            // 047 FR-007 — "Detener agentes": pausa masiva (SIGSTOP) de las tareas corriendo.
            commands::stop_all_agents,
            commands::orchestration_eta,
            commands::orchestration_group_eta,
            commands::orchestration_tail_log,
            // 019 F1 — review hunk-level unificada (diff/review approve/reject por hunk).
            commands::review_open,
            commands::review_get,
            commands::review_hunk_decide,
            commands::review_conflicts,
            commands::review_apply,
            // 019 T024 — retención + exportabilidad del audit del flujo review (export-then-rotate, FR-005).
            commands::audit_export,
            commands::audit_retention_status,
            commands::audit_rotate,
            // 015 US5 — headless process/task lifecycle manager (procesos que sobreviven a la UI)
            commands::process_list,
            commands::process_cancel,
            commands::process_attach,
            // 010-furx-signals — config + control remoto
            commands::signals_recent_deliveries,
            commands::signals_set_telegram_secret,
            commands::signals_set_webhook_secret,
            commands::signals_set_subscription,
            commands::signals_create_pair_code,
            commands::signals_list_allowlist,
            commands::signals_add_allowlist,
            commands::signals_remove_allowlist,
            // 048 Cost-Router Fase 1 (Savings Meter) — dashboard read-only (mide, NO desvía).
            // savings_status = duplicado exacto de savings_summary — removido.
            commands::savings_summary,
            commands::savings_series,
            // 049 Cost-Router Fase 2 (Router ACTIVO) — estado read-only + recarga de policy. El
            // router está OFF detrás de `FURX_COST_ROUTER_MODE` (default) ⇒ no desvía nada.
            commands::cost_router_status,
            commands::cost_router_policy_reload,
            // 050 Ola 8 P2 (FR-003) — reliability board read-only (éxito/latencia/costo por agente/modelo, opt-in).
            commands::reliability_summary,
            // B9 — Claude Accounts multi-Max
            commands::claude_accounts_list,
            commands::claude_account_add,
            commands::claude_account_delete,
            commands::claude_account_verify,
            commands::claude_account_run_setup,
            // FASE 1 — Skills Registry
            commands::skill_refresh,
            commands::skill_list,
            commands::skill_get,
            commands::skill_set_enabled,
            commands::skill_delete,
            commands::skill_history,
            commands::skill_run,
            // 050 Ola 8 P2 (FR-005) — CRL con señalización activa: revocar key mata spans vivos.
            commands::crl_revoke_key,
            // FASE 2 — Memory Hub
            commands::memory_store,
            commands::memory_recall,
            // 045 FR-003 (Ola 5 P1) — recall con re-rank vectorial opt-in + circuit-breaker (backend fts|vector).
            commands::memory_recall_ranked,
            commands::memory_stats,
            // FASE 3 — UMP + LaunchAgent + CLI Hooks + Knowledge Graph
            commands::memory_launchagent_install,
            commands::memory_launchagent_uninstall,
            commands::memory_launchagent_status,
            commands::memory_generate_cli_hooks,
            commands::memory_graph_entities,
            // spec-023 F0/F1 — Memory Hub no-opaco + bandeja de propuestas (auto-captura).
            commands::memory_proposals_list,
            commands::memory_proposal_decide,
            commands::memory_forget_project,
            commands::memory_autocapture_settings,
            // spec-025 F1 — gobierno de lecciones procedurales (auto-aprendizaje #1).
            commands::lessons_active_list,
            commands::lessons_current_project_key,
            commands::lesson_set_active,
            commands::lesson_delete,
            // 050 Ola 8 P2 (FR-002) — gotcha feedback loop: voto de utilidad por lección (advisory, no auto-aplica).
            commands::lesson_record_feedback,
            // F2 spec 001 — cloud client integration (api.furx.cloud)
            services::cloud_commands::cloud_active_user,
            services::cloud_commands::cloud_uploader_status,
            services::cloud_commands::cloud_request_signin,
            services::cloud_commands::cloud_verify,
            services::cloud_commands::cloud_whoami,
            services::cloud_commands::cloud_revoke,
            services::cloud_commands::cloud_is_internal_mode,
            services::cloud_commands::cloud_list_projects,
            services::cloud_commands::cloud_create_project,
            services::cloud_commands::cloud_set_project_traces_enabled,
            services::cloud_commands::cloud_bootstrap_default_project,
            services::cloud_commands::cloud_emit_test_trace,
            services::cloud_commands::cloud_council_compare,
            services::cloud_commands::cloud_regression_compare,
            services::cloud_commands::cloud_recent_councils,
            services::cloud_commands::cloud_upload_trace,
            // 050 Ola 8 P2 (FR-001) — multi-machine sync (opt-in, fail-closed, LWW tiebreaker).
            services::cloud_commands::sync_status,
            services::cloud_commands::sync_now,
            commands::tts_speak,
            commands::tts_stop,
            commands::tts_available,
            commands::tts_speaking_pane,
            commands::plugin_verify,
            commands::plugin_invoke,
            commands::plugin_manifest,
            commands::plugin_grant,
            commands::plugin_revoke,
            commands::plugin_is_granted,
            commands::plugin_harden,
            commands::plugin_install_bundled,
            commands::plugin_list_bundled,
            commands::voice_ptt_start,
            commands::voice_ptt_stop,
            commands::voice_ptt_cancel,
            commands::plugin_grant_secret,
            commands::plugin_revoke_secret,
            commands::plugin_secret_refs,
            // 015 US1 — Command Registry único y tipado.
            services::command_registry::command_registry_list,
            // 015 US4 — Capability / approval gate.
            services::capability::capability_check,
            services::capability::approval_list,
            services::capability::approval_resolve,
            // 027 F2-wiring — policy-as-code: gestión de reglas custom (hardening-only).
            services::policy::policy_list_rules,
            services::policy::policy_set_rule,
            services::policy::policy_remove_rule,
            services::policy::policy_preview,
            services::policy::policy_set_custom_enabled,
            // 028 — ACP Agent Registry (definiciones declarativas de agentes ACP).
            services::acp_registry::acp_agents_list,
            services::acp_registry::acp_agents_upsert,
            services::acp_registry::acp_agents_delete,
            // 030 F0-wire — voz como lente del inbox de atención.
            services::attention::attention_enqueue,
            services::attention::attention_list,
            services::attention::attention_ack,
            services::attention::attention_command,
            services::attention::attention_focused_pane,
            services::attention::attention_next_pane,
            services::audio_attention::callar,
            services::audio_attention::attention_audio_opt_in_set,
            services::audio_attention::attention_audio_opt_in_get,
            services::audio_attention::attention_audio_prefs_get,
            services::audio_attention::attention_audio_prefs_set,
            services::notify_attention::attention_notify_get_enabled,
            services::notify_attention::attention_notify_set_enabled,
            services::notify_attention::attention_notify_sound_get,
            services::notify_attention::attention_notify_sound_set,
            services::notify_attention::attention_notify_bring_to_front_get,
            services::notify_attention::attention_notify_bring_to_front_set,
            ]);
            move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
                use crate::services::capability::{self as cap, GateDecision};
                use crate::services::window_byok::{self, WindowGate};
                let command = invoke.message.command().to_string();
                // 018 Fase 2 HIGH-2 (audit) — ENFORCEMENT BYOK POR-VENTANA (T064), capa ADICIONAL al
                // approval gate. Antes de TODO, si el comando es SENSIBLE (Risk::Credential) y la
                // webview llamante NO es Main, se DENIEGA (constitución F-I: una 2ª webview no es un
                // 2º vault; sólo Main inicia flujos de credencial). En la ola 1 sólo existe la ventana
                // "main", así que en la práctica siempre pasa — pero el wiring queda EFECTIVO en el
                // path central (no dead-code), listo para las ventanas detached de US2. Para comandos
                // no-sensibles `check_window_command` devuelve Allow (lookup O(1) en el registry).
                let window_label = invoke.message.webview().label().to_string();
                if let WindowGate::Deny(reason) = window_byok::check_window_command(&window_label, &command) {
                    invoke.resolver.reject(reason);
                    return true;
                }
                let app = invoke.message.webview().app_handle().clone();
                // FAIL-CLOSED (audit gemini/AIE/deepseek): try_state (no panic) — si por un refactor
                // AppState no estuviera managed, rechazamos en vez de crashear el handler.
                let state = match app.try_state::<AppState>() {
                    Some(s) => s,
                    None => {
                        invoke.resolver.reject("gate: estado interno no disponible");
                        return true;
                    }
                };
                // Fast-path: comando NO gateado por el default Y reglas custom NO activas → directo al
                // handler real (cero overhead). 027 F2-wiring: si `policy.custom_enabled` está ON, una
                // regla custom puede endurecer un comando Safe → NO se puede fast-pathear; va al gate.
                if !cap::is_gated_for_dispatch(&command)
                    && !crate::services::policy::store::custom_enabled(&state.db)
                {
                    return generated(invoke);
                }
                // Gateado (o custom activo): necesitamos los args (para el hash/approval).
                let args_json = match invoke.message.payload() {
                    // FAIL-CLOSED (audit AIE/deepseek MED): si la serialización falla, NO caemos a
                    // "{}" (eso podría matchear un approval de {} y ejecutar sin los args reales) —
                    // rechazamos.
                    tauri::ipc::InvokeBody::Json(v) => match serde_json::to_string(v) {
                        Ok(s) => s,
                        Err(e) => {
                            invoke.resolver.reject(format!("gate: args no serializables: {e}"));
                            return true;
                        }
                    },
                    // Fail-closed: un comando gateado con payload binario no se puede hashear ni
                    // auditar como args NO-secretos → se rechaza (no se ejecuta).
                    tauri::ipc::InvokeBody::Raw(_) => {
                        invoke.resolver.reject("comando gateado con payload binario no soportado");
                        return true;
                    }
                };
                match cap::dispatch_gate(&state.db, &command, &args_json) {
                    Ok(GateDecision::Pass) => generated(invoke),
                    // 027 F2-wiring: una regla custom DENEGÓ el comando. Terminal: no ejecuta, sin
                    // aprobación posible. El front reconoce `kind: "policy_denied"`.
                    Ok(GateDecision::Denied { command_id, rule_id }) => {
                        invoke.resolver.reject(serde_json::json!({
                            "kind": "policy_denied",
                            "command_id": command_id,
                            "rule_id": rule_id,
                        }));
                        true
                    }
                    Ok(GateDecision::Pending { request_id, command_id, risk }) => {
                        // Avisar a TODAS las ventanas que hay un pedido de aprobación (US3 bus).
                        services::event_bus::emit_event(
                            &app,
                            services::event_bus::AppEvent::ApprovalRequested {
                                request_id: request_id.clone(),
                                command_id,
                            },
                        );
                        // Rechazar el invoke con un payload que el front reconoce (re-invoca al
                        // aprobar). El comando real NO se ejecutó.
                        invoke.resolver.reject(serde_json::json!({
                            "kind": "pending_approval",
                            "request_id": request_id,
                            "risk": risk,
                        }));
                        true
                    }
                    // Fail-closed: error del gate (args no-JSON, secret en args por el guardrail
                    // BYOK, etc.) → rechazar, NUNCA ejecutar.
                    Err(e) => {
                        invoke.resolver.reject(format!("gate: {e}"));
                        true
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // 034 U1 — al ACTIVARSE la app (macOS `Reopen`: clic en la notificación o en el dock), traer la
        // ventana `main` al frente SÓLO si el usuario activó el opt-in (default OFF). NUNCA toca el mic.
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = &event {
                services::notify_attention::focus_main_if_opted(app_handle);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event); // silencia unused en plataformas sin Reopen
        });
}
