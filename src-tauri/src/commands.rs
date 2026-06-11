use crate::bases::audit::EventInput;
use crate::bases::guardrail;
use crate::bases::state::PaneState;
use crate::distribution::{self, CompatReport, ResetReport, UpdateInfo};
use crate::export::{self, ExportReport};
use crate::monitors::{self, MonitorResult};
use crate::pty::{PtyManager, SpawnRequest};
use crate::services::{
    agent_memory, aie, bg_queue, corpus_memory, bisect, bootstrap, bundle, claude_usage, clipboard,
    council as council_svc, dag, diff_preview, diff_review, disagreement, embeddings, eval_runner,
    explain, gh_panel, heatmap, http_client, mcp_health, mention, pane_templates, pr_description,
    projects, provider_latency, quick_notes, replay, replay_scrub, router_viz, search,
    settings_registry, smartpaste, snapshot, snippets, ssh_config, suggest, telegram, themes,
    time_tracking, tmux_watchdog, voice, vpn, whisper, worktree, yesterday,
};
use crate::settings as settings_store;
use crate::AppState;
use parking_lot::Mutex;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};
use uuid::Uuid;

type MonitorsStore = Arc<Mutex<HashMap<String, MonitorResult>>>;

/// 015 T014 (audit final, codex HIGH) — lock por pane que SERIALIZA el crítico de `pty_spawn`
/// (reap → register → spawn → post-check). Sin esto, dos `pty_spawn` CONCURRENTES del mismo pane
/// podían interleavear: A registra (token1) y, antes de su `pty.spawn`, B reapea la fila de A +
/// registra (token2) + spawnea; al spawnear A clobberea la sesión de B → fila `running/token2`
/// sin sesión viva (stuck). Con el lock por pane los dos spawns son SECUENCIALES (last-wins
/// limpio). Distintos panes corren en paralelo (locks distintos). Es el outermost lock del crítico
/// (luego db, luego sessions) → sin inversión de orden vs los otros paths. La key es el pane base
/// (sin el sufijo `@<ts>` del remount) para acotar el mapa y serializar todas las generaciones.
static PANE_SPAWN_LOCKS: once_cell::sync::Lazy<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

fn pane_spawn_lock(pane_id: &str) -> Arc<Mutex<()>> {
    let base = pane_id.split('@').next().unwrap_or(pane_id).to_string();
    let mut map = PANE_SPAWN_LOCKS.lock();
    map.entry(base)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ok: bool,
}

#[tauri::command]
pub fn health(state: State<'_, AppState>) -> Health {
    let db_ok = state.db.lock().execute_batch("SELECT 1").is_ok();
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ok,
    }
}

#[derive(Serialize)]
pub struct PaneInfo {
    pub id: String,
    pub layout_pos: i64,
    pub mode: String,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub state: String,
}

#[tauri::command]
pub fn list_panes(state: State<'_, AppState>) -> Result<Vec<PaneInfo>, String> {
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT id, layout_pos, mode, cwd, title, state FROM panes ORDER BY layout_pos")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PaneInfo {
                id: r.get(0)?,
                layout_pos: r.get(1)?,
                mode: r.get(2)?,
                cwd: r.get(3)?,
                title: r.get(4)?,
                state: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[derive(Serialize)]
pub struct Card {
    pub id: String,
    pub created_at: String,
    pub project: String,
    pub source: String,
    pub title: String,
    pub severity: String,
    pub status: String,
    // spec-022 P1 · US6 — campos del inbox accionable. Opcionales para compat con filas legacy.
    pub cause: Option<String>,
    pub snooze_until: Option<String>,
    pub read_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub reopened: bool,
}

#[tauri::command]
pub fn list_cards(state: State<'_, AppState>) -> Result<Vec<Card>, String> {
    let conn = state.db.lock();
    // spec-022 P1 · US6 — auto-unsnooze por EXPIRACIÓN: una card cuyo snooze venció vuelve a estar
    // accionable. Es idempotente (sólo afecta filas con snooze_until pasado) y no destructivo: limpia
    // el snooze pero conserva status/decision. El auto-unsnooze por NUEVA ACTIVIDAD lo dispara
    // `card_record_activity` (marca reopened=1). Esto corre en el path de lectura (barato, sin job).
    conn.execute(
        "UPDATE cards SET snooze_until = NULL \
         WHERE snooze_until IS NOT NULL AND snooze_until <= datetime('now') AND status = 'open'",
        [],
    )
    .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, created_at, project, source, title, severity, status, \
                    cause, snooze_until, read_at, dismissed_at, last_activity_at, reopened \
             FROM cards ORDER BY created_at DESC LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Card {
                id: r.get(0)?,
                created_at: r.get(1)?,
                project: r.get(2)?,
                source: r.get(3)?,
                title: r.get(4)?,
                severity: r.get(5)?,
                status: r.get(6)?,
                cause: r.get(7)?,
                snooze_until: r.get(8)?,
                read_at: r.get(9)?,
                dismissed_at: r.get(10)?,
                last_activity_at: r.get(11)?,
                reopened: r.get::<_, i64>(12)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[derive(Serialize)]
pub struct Event {
    pub id: String,
    pub at: String,
    pub kind: String,
    pub actor: String,
    // 047 FR-004 — campos ya presentes en la tabla `events` (append-only), expuestos al front para
    // (a) agrupar el AuditDrawer por sesión (pane_id/actor) y (b) linkear una card de incidente a su
    // evento de audit relacionado (card_id). Additive: NO cambia el comportamiento existente.
    pub pane_id: Option<String>,
    pub card_id: Option<String>,
    pub correlation_id: Option<String>,
}

#[tauri::command]
pub fn list_events(state: State<'_, AppState>, limit: Option<i64>) -> Result<Vec<Event>, String> {
    // Clamp para evitar exfil masivo desde el frontend.
    let lim = limit.unwrap_or(100).clamp(1, 1000);
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT id, at, kind, actor, pane_id, card_id, correlation_id FROM events ORDER BY at DESC LIMIT ?")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([lim], |r| {
            Ok(Event {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                actor: r.get(3)?,
                pane_id: r.get(4)?,
                card_id: r.get(5)?,
                correlation_id: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

// ── PTY commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    pane_id: String,
    mode: String,
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    agent_profile_id: Option<String>,
    session_override: Option<String>,
) -> Result<(), String> {
    // pane_id sanitization — alfanum + [_-.@] (el frontend usa "@<ts>" para remount).
    if pane_id.is_empty()
        || pane_id.len() > 64
        || !pane_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(format!("invalid pane_id: {}", pane_id));
    }
    // Backpressure: respeta global rate limit por subject "pty:spawn".
    if let Some(wait) = state.scheduler.try_acquire("pty:spawn") {
        return Err(format!("rate limited; retry in {}ms", wait.as_millis()));
    }
    // 015 T014 (audit final, codex HIGH) — SERIALIZA el crítico de este pane: dos pty_spawn
    // concurrentes del mismo pane se vuelven secuenciales (reap→register→spawn→post-check),
    // así B no puede mutar la fila entre el register de A y su spawn (last-wins limpio, sin
    // fila `running` huérfana). Se mantiene hasta el final de pty_spawn.
    let pane_lock = pane_spawn_lock(&pane_id);
    let _pane_guard = pane_lock.lock();
    // FR-004: si viene un agent profile, ÉL maneja el runtime (y seedea el cwd); si no,
    // el `mode` string legacy. Cargar el agente ANTES del cwd para usar su default_cwd.
    let agent = match agent_profile_id.as_deref() {
        Some(aid) => Some(
            crate::services::agent_profiles::get(&state.db, aid)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("agente no encontrado: {}", aid))?,
        ),
        None => None,
    };
    // cwd allowlist — solo dentro de $HOME o /tmp; nunca /, /etc, /var, etc. El cwd
    // explícito gana; si no hay, se usa el default_cwd del agente (también va por el gate).
    let cwd_in = cwd.or_else(|| agent.as_ref().and_then(|a| a.default_cwd.clone()));
    let safe_cwd = if let Some(p) = cwd_in.as_deref() {
        // Expandir `~/` (un default_cwd de agente como "~/proj" debe funcionar, no fallar
        // canonicalize). expand_tilde no-op si no hay tilde.
        let expanded = expand_tilde(p);
        let path = std::path::Path::new(&expanded)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let home = dirs::home_dir().ok_or("no home")?;
        let tmp = std::path::Path::new("/tmp");
        if !path.starts_with(&home) && !path.starts_with(tmp) {
            return Err(format!("cwd outside allowlist: {}", path.display()));
        }
        Some(path.to_string_lossy().to_string())
    } else {
        None
    };
    let (cmd, mut args, env) = match &agent {
        Some(a) => resolve_agent_runtime(a, &state.db)?,
        None => resolve_mode(&mode),
    };
    // spec-011 (FR-003/FR-004) — inject the MCP servers of the plugins in THIS agent's
    // allow-list (default-deny + signature-verified) into its MCP config, and kick off
    // background indexing for any code-graph plugins. Only runs for the agent path
    // (legacy `mode` strings have no plugin allow-list). The `--mcp-config` arg is
    // appended as a trailing CLI arg, which tmux forwards to the wrapped command.
    if let Some(a) = &agent {
        let (mcp_args, index_jobs) = build_agent_mcp_injection(a, safe_cwd.as_deref(), &state.db);
        if !mcp_args.is_empty() {
            args.extend(mcp_args);
            let _ = state.audit.write(EventInput {
                kind: "plugin.mcp.injected",
                actor: &crate::services::identity::current_actor(),
                pane_id: Some(&pane_id),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"agent_cli_kind": a.cli_kind, "plugins": a.plugins}),
            });
        }
        // FR-004 — enqueue per-project background indexing (non-blocking, off the UI).
        for (plugin, project_key) in index_jobs {
            let project_root = safe_cwd.clone().unwrap_or_default();
            let _ = bg_queue::enqueue(
                &state.db,
                "codebase_index",
                serde_json::json!({
                    "plugin": plugin,
                    "project_root": project_root,
                    "project_key": project_key,
                }),
            );
        }
    }
    // 025 F1 — inyección de lecciones procedurales aprobadas en el contexto del perfil. SOLO para
    // `cli_kind=claude` (único con system-append estable; council v2 §2). Gated por
    // `memory.procedural_inject` (default OFF). CONCATENA el bloque "Lecciones aprendidas" al
    // `--append-system-prompt` del perfil (NUNCA lo reemplaza). Audit con snapshot pre/post.
    if let Some(a) = &agent {
        inject_procedural_lessons(a, safe_cwd.as_deref(), &state.db, &state.audit, &pane_id, &mut args);
    }
    // 008 — override de la sesión tmux: por default es FURX_<mode> (resume por modo), pero
    // orquestación necesita una sesión ÚNICA por tarea (N tareas del mismo agente NO deben
    // compartir sesión). Reescribe el nombre tras el `-s` si el spawn quedó tmux-wrapped.
    if let Some(sess) = session_override.as_deref().filter(|s| !s.is_empty()) {
        // 058 (ultrareview fix) — `cmd` viene de `resolve_mode()`, que ya tmux-wrappeó y devuelve el
        // PATH COMPLETO de tmux (`which tmux` → /opt/homebrew/bin/tmux). El guard viejo `cmd == "tmux"`
        // NUNCA matcheaba el path → el override jamás se aplicaba → el spawn quedaba en FURX_<mode> pero
        // el resume buscaba FURX_<sess> → scrollback roto + panes del mismo modo compartían sesión.
        // Match por BASENAME (cubre el path completo y el literal "tmux").
        let cmd_is_tmux =
            std::path::Path::new(&cmd).file_name().and_then(|f| f.to_str()) == Some("tmux");
        if cmd_is_tmux {
            // 056 — MISMA sanitización que pty_capture_history (audit BLOCKER): así el resume encuentra
            // la sesión que el spawn crea, aunque `sess` traiga chars no-alfanuméricos (`/`, `.`, etc.).
            if let Some(i) = args.iter().position(|a| a == "-s") {
                if i + 1 < args.len() {
                    args[i + 1] = furx_session_name(sess);
                }
            }
        }
    }
    // 023 F1 — preservar el cwd para resolver el project_key de la captura de memoria (el
    // `safe_cwd` se mueve dentro de `req` al armar el SpawnRequest).
    let capture_cwd = safe_cwd.clone();
    let req = SpawnRequest {
        pane_id: pane_id.clone(),
        cmd,
        args,
        cwd: safe_cwd,
        env,
        rows,
        cols,
    };
    // 015 T014 (US5) — REAP del run anterior con el MISMO pane_id (audit HIGH, 4 voces).
    // El front remonta el mismo `pane.id` cuando cambia mode/cwd/agent (la React key incluye
    // esos campos pero el paneId es estable). Con el unmount-detach de US5, ese re-spawn caería
    // sobre un PTY VIVO → (a) el PtyManager sobreescribiría la sesión leakeando el OS process
    // viejo, (b) la nueva corrida quedaría bajo una fila stale. Reapeamos ACÁ: cancel_and_reap
    // mata el PTY viejo (sin orphan) y marca su fila `canceled`; es best-effort (si no había
    // fila/sesión, falla suave). Luego `register` (UPSERT) reinicia la fila a `running` fresco.
    // `notify=false`: el reap es interno (no una cancelación del usuario) → silencioso en bus +
    // audit, igual que un primer spawn; evita un `canceled` espurio que la UI vería como flicker.
    let _ = cancel_reap_emit(&state.db, pty.inner(), &app, &state.audit, &pane_id, false);
    // Registrar ANTES de spawnear: el wait-thread (que puede ver el exit casi inmediato) arranca
    // DENTRO de `pty.spawn`, así que la fila DEBE existir antes para que su `finish` no caiga
    // sobre una fila ausente (race resuelto por orden, council B). process_id == external_ref ==
    // pane_id. `run_token` (generación de ESTE spawn): la fila + la PtySession + su wait-thread lo
    // llevan; `finish` se scopea por él para que un wait-thread viejo no clobberee un run nuevo.
    let run_token = crate::services::process_manager::next_run_token();
    let proc_label = agent
        .as_ref()
        .map(|a| a.cli_kind.clone())
        .unwrap_or_else(|| mode.clone());
    let registered = crate::services::process_manager::register(
        &state.db,
        crate::services::process_manager::RegisterSpec {
            process_id: Some(pane_id.clone()),
            kind: crate::services::process_manager::ProcessKind::Pty,
            owner_context: Some(pane_id.clone()),
            external_ref: Some(pane_id.clone()),
            label: Some(proc_label),
            progress: None,
            run_token: Some(run_token),
        },
    )
    .map_err(|e| e.to_string())?;
    // 015 T014 (audit final, codex HIGH) — carrera de pty_spawn CONCURRENTES sobre el mismo
    // pane_id: `register` es UPSERT y un re-register de una fila `running` CONSERVA el token
    // existente (no-op). Si nuestro token NO quedó como el vigente, OTRA invocación ganó este
    // pane_id; NO debemos spawnear una 2da generación (sesión token nuestro) sobre una fila
    // ajena (token de ellos) → al salir, nuestro `finish(token)` no matchearía y la fila quedaría
    // `running` sin sesión. Abortamos ANTES de `pty.spawn` (el ganador administra el pane).
    if registered.run_token != Some(run_token) {
        return Err(format!("pane {} ya tiene un spawn en curso", pane_id));
    }
    if let Err(e) = pty.spawn(req, app.clone(), run_token) {
        // El spawn falló tras registrar → cerramos la fila como `failed` (scopeado a NUESTRA
        // generación) y matamos sólo NUESTRA half-session (`kill_if_spawn_id`): pty.spawn inserta
        // la sesión antes de arrancar los threads, un fallo de thread deja una half-session. Usamos
        // las variantes generation-specific para no clobberear un respawn que reusó el pane_id.
        let _ = pty.kill_if_spawn_id(&pane_id, run_token);
        let _ = crate::services::process_manager::finish(
            &state.db,
            &pane_id,
            "failed",
            Some(run_token),
        );
        return Err(e.to_string());
    }
    // 015 T014 (US5) — race cancel-durante-spawn (audit MED): si entre `register` y el fin de
    // `pty.spawn` llegó un `process_cancel(pane_id)` concurrente, la fila quedó `canceled` pero
    // el PTY recién nacido está vivo → orphan "canceled" pero corriendo. Lo cerramos determinista:
    // si la fila ya no está `running`, matamos SÓLO nuestra generación y abortamos.
    match crate::services::process_manager::get(&state.db, &pane_id) {
        Ok(Some(info)) if info.status != "running" => {
            let _ = pty.kill_if_spawn_id(&pane_id, run_token);
            return Err(format!("proceso {} cancelado durante el spawn", pane_id));
        }
        _ => {}
    }
    // 023 F1 — registrar el contexto de captura de memoria del pane SOLO si es un CLI de agente
    // conocido Y la auto-captura está encendida (default OFF). Sin esto, el reader NO acumula
    // SessionBuffer para el pane (cero overhead con la feature apagada). El cli_kind sale del
    // agente o del `mode` legacy; el project_key se resuelve del cwd.
    {
        let cli_kind = agent
            .as_ref()
            .map(|a| a.cli_kind.clone())
            .unwrap_or_else(|| mode.clone());
        if crate::services::memory_autocapture::is_agent_cli(&cli_kind) {
            let autocapture_on = {
                let conn = state.db.lock();
                crate::settings::get(&conn, "memory.autocapture")
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            };
            if autocapture_on {
                let project_key = capture_cwd
                    .as_deref()
                    .map(|c| {
                        let conn = state.db.lock();
                        crate::services::memory_daemon::resolve_project_key_public(&conn, c)
                            .unwrap_or_else(|| "__global__".to_string())
                    })
                    .unwrap_or_else(|| "__global__".to_string());
                pty.register_session_ctx(
                    &pane_id,
                    crate::pty::SessionCaptureCtx {
                        cli_kind: cli_kind.clone(),
                        project_key,
                        session_id: format!("{}:{}", pane_id, run_token),
                    },
                );
            }
        }
    }
    // mark CLI account as last-used on successful spawn (universal).
    match &agent {
        Some(a) => {
            if let Some(slug) = a.account_slug.as_deref().filter(|s| !s.is_empty()) {
                let _ = crate::services::claude_accounts::mark_used(&state.db, &a.cli_kind, slug);
            }
        }
        None => {
            // Order matters: longer prefixes first (openai-api before potential 'api-').
            for kind in &["openai-api", "claude", "codex", "gemini", "aider", "custom"] {
                let prefix = format!("{}-", kind);
                if let Some(slug) = mode.strip_prefix(&prefix) {
                    if !slug.is_empty() && slug != *kind {
                        let _ = crate::services::claude_accounts::mark_used(&state.db, kind, slug);
                        break;
                    }
                }
            }
        }
    }
    // Audit (no PII). Cuando un agente maneja el spawn NO logueamos su id (los built-in
    // Claude llevan el slug en el id: `builtin:claude:<slug>`) ni el `mode` stale del pane
    // — sólo cli_kind + si es built-in. Sin agente: el mode legacy.
    let _ = state.audit.write(EventInput {
        kind: "pty.spawned",
        actor: &crate::services::identity::current_actor(),
        pane_id: Some(&pane_id),
        card_id: None, correlation_id: None,
        payload: match &agent {
            Some(a) => serde_json::json!({"agent": true, "cli_kind": a.cli_kind, "agent_builtin": a.is_builtin}),
            None => serde_json::json!({"mode": mode}),
        },
    });
    Ok(())
}

#[tauri::command]
pub fn pty_write(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    // 018 Fase 2 US2 (should-fix audit ola-1): el `window_label` para el guard de lease se
    // deriva SERVER-SIDE del `WebviewWindow::label()`, NO de un parámetro del front. Un
    // caller no puede declarar el label de OTRA ventana para colar un write sobre la sesión
    // del binding vigente de esa otra ventana.
    window: tauri::WebviewWindow,
    pane_id: String,
    data: String,
    action_id: Option<String>,
    correlation_id: Option<String>,
    // 018 Fase 2 B0 (T060) — `mount_instance_id` lo declara el front (es del montaje del
    // componente, no spoofeable de forma útil: identifica QUÉ montaje de ESTA ventana escribe).
    // OPCIONAL para no romper callers legacy (Terminal.tsx pre-Fase-2 no lo manda → fail-OPEN
    // si el panel no tiene lease). Cuando llega (Leaf del WorkspaceView), se VALIDA el lease
    // contra (label-server-side, mount_instance_id): si el binding vigente del panel_id es de
    // OTRA ventana/montaje (doble-binding o componente desmontado), el write se DESCARTA
    // (fail-closed) — sin matar el PTY.
    mount_instance_id: Option<String>,
) -> Result<(), String> {
    let window_label = window.label().to_string();
    // T060 + HIGH-1 (audit) — guard de lease, fail-CLOSED universal.
    //
    //   - Caller DECLARÓ su binding (window_label + mount_instance_id): validamos contra el
    //     lease vigente. `is_current` es false si es de otra ventana/montaje o está detaching
    //     → el write de un binding stale se DESCARTA (el pane vigente, en otra ventana, recibe
    //     el input real). No es error duro.
    //   - Caller NO declaró (o sólo mandó uno de los dos) PERO EXISTE un lease para el panel:
    //     antes esto bypasseaba el guard (fail-OPEN universal → un caller con lease podía
    //     escribir sin params y saltarse la validación). Ahora se DESCARTA (fail-CLOSED): si el
    //     panel tiene binding bajo el registro de leases, sólo el binding vigente —que SÍ
    //     declara sus params— puede escribir.
    //   - NO hay lease para el panel (caller legacy, flag `newWorkspace` OFF, Terminal pre-Fase-2
    //     que no hace attach): fail-OPEN, el write procede normal. El path legacy queda intacto.
    //
    // EN NINGÚN CASO se mata/respawnea el PTY (invariante VI): el guard sólo decide si el byte
    // de input llega a la sesión o se descarta.
    match mount_instance_id.as_deref() {
        // Caller declaró su montaje (Leaf del WorkspaceView): validar el lease contra el
        // label SERVER-SIDE + ese mount. Si no es el binding vigente → descartar (fail-closed).
        Some(mid) => {
            if !state.pty_leases.is_current(&pane_id, &window_label, mid) {
                return Ok(());
            }
        }
        // Sin mount declarado (caller legacy / Terminal pre-Fase-2): si el panel TIENE lease,
        // fail-closed (descartar — sólo el binding vigente, que sí declara su mount, escribe);
        // si NO hay lease, fail-open (path legacy intacto).
        None => {
            if state.pty_leases.has_lease(&pane_id) {
                return Ok(());
            }
        }
    }
    // F3 idempotencia: si recibimos el mismo (correlation,action) dentro del TTL,
    // ignoramos el segundo write (red flaky / dobles enters).
    if let (Some(cid), Some(aid)) = (correlation_id.as_ref(), action_id.as_ref()) {
        let key = crate::bases::router::ActionKey {
            correlation_id: cid.clone(),
            action_id: aid.clone(),
        };
        if let Some(crate::bases::router::ActionOutcome::Ok(_)) = state.router.check(&key) {
            return Ok(());
        }
        state.pane_state.on_input(&pane_id);
        let res = pty
            .write(&pane_id, data.as_bytes())
            .map_err(|e| e.to_string());
        let outcome = match &res {
            Ok(_) => crate::bases::router::ActionOutcome::Ok("written".into()),
            Err(e) => crate::bases::router::ActionOutcome::Err(e.clone()),
        };
        state.router.record(key, outcome);
        return res;
    }
    state.pane_state.on_input(&pane_id);
    pty.write(&pane_id, data.as_bytes())
        .map_err(|e| e.to_string())
}

/// 018 Fase 2 B0 (T060) — el Leaf de una webview ATTACHA su binding al montarse. Liga
/// `panel_id` a (`window_label`, `mount_instance_id`). Si ya había un binding para ese
/// panel_id (p.ej. el viejo montaje aún no se desuscribió, o vive en otra ventana), lo
/// FUERZA-DETACH versionado. NUNCA toca el proceso PTY (que vive en el backend, US5).
/// Devuelve la `window_label` desplazada (si la hubo) para que el front avise a esa
/// webview que perdió el binding.
#[tauri::command]
pub fn pty_lease_attach(
    app: AppHandle,
    state: State<'_, AppState>,
    // 018 Fase 2 US2 (should-fix audit ola-1): el `window_label` se deriva SERVER-SIDE del
    // `WebviewWindow::label()` — NUNCA se confía en un parámetro del front (anti-spoof: una
    // webview no puede declarar ser "main" para robar/desplazar el binding de otra ventana).
    window: tauri::WebviewWindow,
    panel_id: String,
    mount_instance_id: String,
) -> Result<Option<String>, String> {
    use tauri::Emitter;
    let window_label = window.label().to_string();
    let out = state
        .pty_leases
        .attach_panel(&panel_id, &window_label, &mount_instance_id);
    // 018 Fase 2 US2 (should-fix audit ola-1): SEÑAL `displaced`. Si este attach desplazó un
    // binding que vivía en OTRA ventana (force-detach versionado), avisamos a ESA ventana que
    // perdió el binding del panel, para que apague su input de ese pane PROACTIVAMENTE (su
    // `pty_write` ya quedó fail-closed por `is_current`, pero el evento evita teclear "al vacío").
    // NO toca el proceso. Evento por-ventana (emit_to), payload sin secretos (sólo ids).
    if let Some(d) = &out.displaced {
        if d.window_label != window_label {
            let _ = app.emit_to(
                d.window_label.as_str(),
                "furx:lease-lost",
                serde_json::json!({ "panel_id": panel_id, "to_window": window_label }),
            );
        }
    }
    Ok(out.displaced.map(|l| l.window_label))
}

/// 018 Fase 2 B0 (T061) — el Leaf DETACHA su binding al desmontarse / al cerrar la
/// ventana. Libera SÓLO el binding UI si el lease vigente coincide con (window_label,
/// mount_instance_id); idempotente y serializado por panel_id. NUNCA mata el proceso.
/// `true` si liberó un binding propio.
#[tauri::command]
pub fn pty_lease_detach(
    state: State<'_, AppState>,
    // server-side label (anti-spoof, ver pty_lease_attach): sólo el binding de ESTA ventana
    // puede soltarse. Una webview no puede liberar el lease de otra declarando su label.
    window: tauri::WebviewWindow,
    panel_id: String,
    mount_instance_id: String,
) -> Result<bool, String> {
    let window_label = window.label().to_string();
    Ok(state
        .pty_leases
        .detach_panel_view(&panel_id, &window_label, &mount_instance_id))
}

// ── 018 Fase 2 US2 (T021) — comandos de ventana (detach-to-window) ──────────────
//
// CONSTITUCIÓN VI: NINGUNO de estos comandos mata/respawnea un PTY. Detach mueve el
// `PanelDescriptor` en el árbol y abre una WebviewWindow; close reata el subárbol a Main
// y cierra la ventana. El proceso vive en el backend (US5), desacoplado de la UI.
//
// SSOT UNIDIRECCIONAL: la mutación SIEMPRE es UI→comando→Rust valida (T062)+bump revisión
// (T063)+persiste `LayoutConfigV1`→emite UN `LayoutChanged`→las webviews re-hidratan. NUNCA
// se persiste el JSON interno de dockview.

/// Reporte de una ventana viva (para `window_list`). Espejo de `WindowEntry`.
#[derive(Serialize)]
pub struct WindowReport {
    pub label: String,
    pub window_key: String,
    pub is_main: bool,
}

/// Persiste una mutación de layout con REINTENTO ante `stale_layout` (otra ventana escribió
/// entre el read y el write): re-lee, re-aplica la transformación pura `f` sobre la revisión
/// fresca, y reintenta (hasta `tries`). `f` recibe el cfg actual y devuelve `Some((next, extra))`
/// para persistir, o `None` para abortar sin error (la precondición ya no se cumple). El bump de
/// revisión lo pone `f` (las transformaciones de window_reattach lo hacen). Devuelve el `extra`
/// (p.ej. el window_key creado) en éxito. NUNCA toca procesos.
fn persist_layout_mutation<T>(
    db: &Arc<Mutex<rusqlite::Connection>>,
    workspace_id: &str,
    tries: usize,
    mut f: impl FnMut(
        &crate::services::layout_config::LayoutConfigV1,
    ) -> Option<(crate::services::layout_config::LayoutConfigV1, T)>,
) -> Result<Option<T>, String> {
    use crate::services::layout_config;
    let mut last_err = String::new();
    for _ in 0..tries.max(1) {
        let cur = layout_config::get(db, workspace_id).map_err(|e| e.to_string())?;
        let Some((next, extra)) = f(&cur) else {
            return Ok(None); // precondición no satisfecha (p.ej. panel ya no está) → no-op limpio.
        };
        match layout_config::save(db, &next) {
            Ok(()) => return Ok(Some(extra)),
            Err(e) => {
                let msg = e.to_string();
                // stale_layout = carrera optimista: re-leer y reintentar. Otro error → propagar.
                if msg.contains("stale_layout") {
                    last_err = msg;
                    continue;
                }
                return Err(msg);
            }
        }
    }
    Err(format!("layout busy (reintentos agotados): {last_err}"))
}

/// T021 — DETACH-TO-WINDOW: saca el pane `panel_id` (Leaf de Main) a una ventana secundaria.
/// Pasos (serializados por `window_tx_lock` contra un cierre simultáneo):
///   1. Mutar el `LayoutConfigV1`: mover el Leaf de Main a una nueva `WindowLayout{Detached}`
///      (transformación pura `detach_panel_to_window`, valida + bump revisión al persistir).
///   2. Registrar la ventana en el `WindowRegistry` (label == window_key generado).
///   3. Abrir la `WebviewWindow` (misma entry `index.html` + `?window_key=` para que la webview
///      sepa qué subárbol renderizar).
///   4. Emitir `LayoutChanged` → Main re-hidrata (ya no muestra ese pane) y la nueva ventana
///      monta su subárbol + hace su propio `pty_lease_attach` con SU label → el binding migra
///      sin tocar el proceso (el force-detach versionado del lease desplaza el binding viejo).
/// Idempotente ante un panel ya detached / inexistente (devuelve `Ok(None)`).
#[tauri::command]
pub fn window_open_detached(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    panel_id: String,
    workspace_id: Option<String>,
) -> Result<Option<String>, String> {
    use crate::services::{layout_config, window_reattach};
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
    // OWNERSHIP (018 US2 audit): detach es una operación que saca un pane DE Main → sólo la ventana
    // Main puede iniciarla. El caller lo deriva Tauri server-side (anti-spoof). Una webview detached
    // no debe poder detachar panes (de Main ni de otra ventana). Simétrico al guard de `window_close`.
    if window.label() != layout_config::MAIN_WINDOW_KEY {
        return Err(format!(
            "la ventana '{}' no puede detachar paneles (sólo Main inicia detach)",
            window.label()
        ));
    }
    let ws = workspace_id.unwrap_or_else(|| layout_config::DEFAULT_WORKSPACE.to_string());
    // Serializar la transición de ventana (detach/close) de punta a punta.
    let _tx = state.window_tx_lock.lock();
    // APERTURA TRANSACCIONAL (018 US2 audit, codex r3): BUILD-ANTES-DE-PERSISTIR. El bug previo
    // persistía el detach y, si el `.build()` fallaba Y el rollback también, dejaba un
    // `WindowLayout{Detached}` SIN webview = panel huérfano/inaccesible. Lo eliminamos de raíz:
    //   1. DRY-RUN puro de `detach_panel_to_window` sobre el layout actual (leído bajo el lock →
    //      estable): valida que el pane sea detachable (en Main Y removible) Y pre-computa el
    //      `window_key`. Quitar un pane de Main NO cambia el set de window_keys, así que el key del
    //      dry-run == el que generará la persistencia real. Si devuelve None → no-op (pane ausente o
    //      único Leaf raíz no removible) SIN abrir ninguna ventana.
    let cfg = layout_config::get(&state.db, &ws).map_err(|e| e.to_string())?;
    let Some((_, window_key)) = window_reattach::detach_panel_to_window(&cfg, &panel_id) else {
        return Ok(None);
    };
    //   2. Crear la WebviewWindow PRIMERO. Si falla, NADA se persistió ni registró → estado limpio,
    //      sin huérfano posible. La entry es la misma index.html (anti-FOUC) + `?window_key=` que la
    //      webview lee para renderizar SÓLO su subárbol.
    let url = format!("index.html?window_key={window_key}");
    if let Err(e) = WebviewWindowBuilder::new(&app, &window_key, WebviewUrl::App(url.into()))
        .title(format!("Furx — {window_key}"))
        .inner_size(1100.0, 760.0)
        .build()
    {
        return Err(format!("no se pudo abrir la ventana detached: {e}"));
    }
    //   3. Persistir el detach. Si falla (o el pane desapareció entre el dry-run y ahora), cerrar la
    //      webview recién creada — NO renderiza ningún pane todavía (su window_key aún no existe en el
    //      layout → `windowFor` devuelve null) → cierre inocuo, sin huérfano. NUNCA toca procesos.
    match persist_layout_mutation(&state.db, &ws, 5, |c| {
        window_reattach::detach_panel_to_window(c, &panel_id)
    }) {
        Ok(Some(persisted_key)) => {
            // Bajo el lock, persisted_key == window_key (mismo next_detached_key).
            state
                .windows
                .register_detached(&persisted_key, &persisted_key);
            // US3 (T031): ubicar la ventana según los monitores disponibles. Un detach fresco no tiene
            // `DisplayHint` aún (rehidratación con hint = unidad diferida) → `resolve_placement` centra
            // en el primario; con un hint cuyo monitor no exista, caería al primario (fallback). El
            // núcleo `resolve_placement` es PURO/testeado; acá sólo lo aplicamos. BEST-EFFORT: un fallo
            // de placement NO invalida el detach (la ventana ya existe y el layout ya está persistido).
            if let Ok(screens) = collect_screens(&window) {
                if let Some(p) =
                    crate::services::screens::resolve_placement(None, &screens, 1100, 760)
                {
                    if let Some(w) = app.get_webview_window(&persisted_key) {
                        let _ = w.set_position(tauri::PhysicalPosition::new(p.x, p.y));
                        let _ = w.set_size(tauri::PhysicalSize::new(p.width, p.height));
                    }
                }
            }
            // Avisar a TODAS las webviews (Main re-hidrata sin el pane; la nueva monta su subárbol).
            crate::services::event_bus::emit_event(
                &app,
                crate::services::event_bus::AppEvent::LayoutChanged {
                    window_id: persisted_key.clone(),
                },
            );
            Ok(Some(persisted_key))
        }
        Ok(None) => {
            // El pane desapareció entre el dry-run y la persistencia (p.ej. se cerró) → cerrar la
            // webview y no-op (sin registro ni layout fantasma).
            if let Some(w) = app.get_webview_window(&window_key) {
                let _ = w.close();
            }
            Ok(None)
        }
        Err(e) => {
            if let Some(w) = app.get_webview_window(&window_key) {
                let _ = w.close();
            }
            Err(format!(
                "no se pudo persistir el detach de la ventana '{window_key}': {e}"
            ))
        }
    }
}

/// T022 — núcleo TRANSACCIONAL del cierre/re-attach de una ventana detached. Reusable por
/// el comando `window_close` (botón re-attach) Y por el listener `onCloseRequested` (X del SO,
/// lib.rs). Pasos, serializados por `window_tx_lock` (un detach simultáneo no corre a la vez):
///   1. `begin_settle(label)` — si ya estaba settling (cierre re-entrante), NO re-procesa.
///   2. Baja del registro (idempotente). Main → no-op (su ciclo lo maneja la app).
///   3. Reatar el subárbol de esa ventana a Main en el SSOT (`reattach_window_to_main`, puro:
///      mueve el árbol, NUNCA mata procesos), con reintento ante `stale_layout`.
///   4. Emitir `LayoutChanged` → Main re-hidrata con el pane reatado.
/// Devuelve `true` si procesó el reatado (era una detached no-settling), `false` si fue no-op.
/// NO cierra la WebviewWindow (eso lo hace el caller: el comando con `w.close()`, el listener
/// dejando proceder el cierre). NUNCA toca procesos PTY (constitución VI).
pub(crate) fn settle_detached_window(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    workspace_id: &str,
) -> Result<bool, String> {
    use crate::services::{layout_config, window_reattach};
    let _tx = state.window_tx_lock.lock();
    // Idempotencia / anti-reentrante: sólo el PRIMER settle procesa. Si `false`, OTRO settle
    // (la otra vía: botón vs X) ya es dueño de la marca → no la liberamos ni cerramos acá.
    if !state.windows.begin_settle(label) {
        return Ok(false);
    }
    // PEEK sin remover: ¿es una detached viva? (no Main, registrada).
    if !state.windows.is_live_detached(label) {
        // Main o ya cerrada → nada que reatar. Liberamos NUESTRA marca (la tomamos arriba).
        state.windows.end_settle(label);
        return Ok(false);
    }
    // TRANSACCIONAL (018 US2 audit): persistir el reattach a Main en el SSOT ANTES de remover la
    // ventana del registro. Si la persistencia falla, NO removemos (un retry podrá reatar los
    // panes), liberamos la marca y propagamos el error — la WebviewWindow queda ABIERTA (el caller
    // NO debe cerrarla ante Err), evitando un PTY vivo pero huérfano/inaccesible (constitución VI).
    if let Err(e) = persist_layout_mutation(&state.db, workspace_id, 5, |cfg| {
        Some((window_reattach::reattach_window_to_main(cfg, label), ()))
    }) {
        state.windows.end_settle(label);
        return Err(e);
    }
    // Reattach ya persistido → recién ahora removemos del registro vivo y notificamos a Main.
    // (La marca `settling` se mantiene hasta que el caller cierre la webview y llame `end_settle`.)
    state.windows.close(label);
    crate::services::event_bus::emit_event(
        app,
        crate::services::event_bus::AppEvent::LayoutChanged {
            window_id: layout_config::MAIN_WINDOW_KEY.to_string(),
        },
    );
    Ok(true)
}

/// T021/T022 — CIERRE / RE-ATTACH de una ventana detached por su `label` (botón re-attach del
/// front). Reata sus paneles a Main vía `settle_detached_window`, luego cierra la WebviewWindow
/// real. El `w.close()` dispara un `onCloseRequested` re-entrante que, al ver el label ya
/// settling, deja cerrar sin re-procesar (ver lib.rs). Idempotente. NUNCA mata procesos.
///
/// OWNERSHIP (018 US2 audit): el `label` viene del front, así que NO se confía ciegamente. El label
/// REAL del caller lo deriva Tauri server-side (`window.label()`, anti-spoof). Una ventana detached
/// SÓLO puede cerrarse/reatarse a SÍ MISMA; sólo Main puede operar sobre un label ajeno (op
/// administrativa). Así una webview detached no puede cerrar/desplazar los panes de OTRA ventana.
#[tauri::command]
pub fn window_close(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    label: String,
    workspace_id: Option<String>,
) -> Result<(), String> {
    use crate::services::{layout_config, window_byok};
    use tauri::Manager;
    let caller = window.label();
    if !window_byok::can_close_window(caller, &label) {
        return Err(format!(
            "la ventana '{caller}' no puede cerrar/reatar '{label}' (sólo Main o la propia ventana)"
        ));
    }
    let ws = workspace_id.unwrap_or_else(|| layout_config::DEFAULT_WORKSPACE.to_string());
    // En `Err`, settle ya liberó la marca y NO removió la ventana → propagamos sin cerrar
    // (la WebviewWindow queda viva para reintentar; no se huérfana ningún PTY).
    let processed = settle_detached_window(&app, &state, &label, &ws)?;
    // Sólo cerramos + limpiamos si NOSOTROS procesamos el reattach (Ok(true)). Ok(false) =
    // no-op (Main / ya cerrada) o la marca la posee otro settle concurrente (botón vs X) → no
    // la tocamos para no liberar la marca ajena ni cerrar dos veces.
    if processed {
        // Cerrar la WebviewWindow real. El `w.close()` re-dispara onCloseRequested, pero la
        // ventana ya NO está en el registro (settle la removió) → el listener no re-procesa.
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.close();
        }
        // Limpiar la marca de settling: el label `detached-N` puede reusarse en un detach futuro
        // (next_detached_key reusa N liberados); un settling stale haría que el PRÓXIMO cierre por
        // la X de ese label se saltee el reatado. (idempotente: no-op si nunca se marcó.)
        state.windows.end_settle(&label);
    }
    Ok(())
}

/// T021 — lista las ventanas vivas (label, window_key, is_main). Orden determinista.
#[tauri::command]
pub fn window_list(state: State<'_, AppState>) -> Result<Vec<WindowReport>, String> {
    Ok(state
        .windows
        .list()
        .into_iter()
        .map(|e| WindowReport {
            label: e.label,
            window_key: e.window_key,
            is_main: e.is_main,
        })
        .collect())
}

/// T030 (US3) — lista los MONITORES (displays) disponibles como `ScreenInfo` (id estable, geometría
/// física, primario). Distinto del comando `list_monitors` (monitoreo de salud de servidores). El
/// front lo usa para saber a dónde reabrir una ventana; el backend, para resolver `DisplayHint`.
/// Enumera los monitores (displays) vía la API de Tauri 2 (cuelga de una Window, no de AppHandle)
/// y los normaliza a `ScreenInfo` (px físicos). Helper reusado por `monitors_list` (comando) y por
/// `window_open_detached` (placement al abrir).
pub(crate) fn collect_screens(
    window: &tauri::WebviewWindow,
) -> Result<Vec<crate::services::screens::ScreenInfo>, String> {
    use crate::services::screens::ScreenInfo;
    // Geometría del primario (posición+tamaño) para identificarlo por geometría — discriminador único
    // por monitor, robusto ante nombres duplicados (audit codex).
    let primary = window.primary_monitor().map_err(|e| e.to_string())?;
    let primary_pos: Option<(i32, i32, u32, u32)> = primary.as_ref().map(|m| {
        let p = m.position();
        let s = m.size();
        (p.x, p.y, s.width, s.height)
    });
    let monitors = window.available_monitors().map_err(|e| e.to_string())?;
    Ok(monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let pos = m.position();
            let size = m.size();
            let name = m.name().cloned().unwrap_or_else(|| format!("monitor-{i}"));
            // ID ÚNICO dentro del snapshot: `nombre@x,y`. `Monitor::name()` NO es único (dos displays
            // idénticos comparten nombre → un `monitor_id` ambiguo elegiría el equivocado). Agregar la
            // posición desambigua (no hay dos monitores en el mismo origen) y es estable mientras el
            // arreglo físico no cambie.
            let id = format!("{name}@{},{}", pos.x, pos.y);
            // Primario: SIEMPRE por GEOMETRÍA (posición+tamaño) contra el `primary_monitor` reportado.
            // NO por nombre: dos displays idénticos comparten nombre → marcaría AMBOS como primario
            // (audit codex). La POSICIÓN es única por monitor, así que es el discriminador correcto.
            // Si el SO no reporta primario, último recurso: el primero (determinista).
            let is_primary = match primary_pos {
                Some((px, py, pw, ph)) => {
                    pos.x == px && pos.y == py && size.width == pw && size.height == ph
                }
                None => i == 0,
            };
            ScreenInfo {
                id,
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
                scale_factor: m.scale_factor(),
                is_primary,
            }
        })
        .collect())
}

#[tauri::command]
pub fn monitors_list(
    window: tauri::WebviewWindow,
) -> Result<Vec<crate::services::screens::ScreenInfo>, String> {
    collect_screens(&window)
}

#[derive(Serialize)]
pub struct PaneStateReport {
    pub state: PaneState,
    pub idle_seconds: u64,
}

#[tauri::command]
pub fn pane_state(state: State<'_, AppState>, pane_id: String) -> Option<PaneStateReport> {
    state.pane_state.get(&pane_id).map(|rec| {
        let last = rec
            .last_output_at
            .unwrap_or(rec.since)
            .max(rec.last_input_at.unwrap_or(rec.since));
        PaneStateReport {
            state: rec.state,
            idle_seconds: last.elapsed().as_secs(),
        }
    })
}

#[tauri::command]
pub fn pty_resize(
    pty: State<'_, Arc<PtyManager>>,
    pane_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    pty.resize(&pane_id, rows, cols).map_err(|e| e.to_string())
}

/// 015 T014 (US5): mata un PTY ruteando por el registro de procesos (SSOT) — marca la fila
/// `canceled` y reapea el recurso real. Si el pane no tiene fila en el registry (p.ej. un
/// spawn que falló ANTES de registrar, o un pane nunca registrado), cae a un kill directo
/// best-effort para no dejar un child huérfano (path defensivo de `Terminal.tsx` L159).
#[tauri::command]
pub fn pty_kill(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    app: tauri::AppHandle,
    pane_id: String,
) -> Result<(), String> {
    match cancel_and_reap(state.inner(), pty.inner(), &app, &pane_id) {
        Ok(_) => Ok(()),
        // Sin fila en el registry → kill directo defensivo (el recurso igual debe morir).
        Err(_) => pty.kill(&pane_id).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub fn pty_alive(pty: State<'_, Arc<PtyManager>>, pane_id: String) -> bool {
    pty.alive(&pane_id)
}

// ── Monitors ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct MonitorSnapshot {
    pub target: monitors::MonitorTarget,
    pub last: Option<MonitorResult>,
}

#[tauri::command]
pub fn list_monitors(
    state: State<'_, AppState>,
    store: State<'_, MonitorsStore>,
) -> Vec<MonitorSnapshot> {
    // 045 FR-001 — los targets salen de la DB (`monitor_targets`), ya no del hardcode.
    let targets = monitors::load_targets(&state.db);
    let snap = store.lock();
    targets
        .into_iter()
        .map(|t| {
            let last = snap.get(&t.id).cloned();
            MonitorSnapshot { target: t, last }
        })
        .collect()
}

/// 045 FR-001 — agrega un monitor configurable. Valida kind+addr (backend, no sólo UI) y aplica
/// el cap por tier (Free 5 / Pro 50 / Team+∞) ANTES de insertar. Devuelve el id generado.
#[tauri::command]
pub fn monitor_add(
    state: State<'_, AppState>,
    label: String,
    kind: String,
    addr: String,
    interval_s: Option<u64>,
) -> Result<String, String> {
    // Cap por tier (lee el tier cacheado de la DB — sin red). El cap se aplica DENTRO de
    // insert_target_capped, bajo el mismo lock que el insert → sin TOCTOU entre count e insert.
    let tier = license_svc::current_tier(&state.db);
    let cap = monitor_cap_for_tier(&tier);
    let id = monitors::insert_target_capped(
        &state.db,
        &label,
        &kind,
        &addr,
        interval_s.unwrap_or(30),
        cap,
        &tier,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "monitor.add",
        actor: "user",
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({ "id": id, "label": label, "kind": kind, "addr": addr }),
    });
    Ok(id)
}

/// 045 FR-001 — quita un monitor por id. Audita. Devuelve true si borró.
#[tauri::command]
pub fn monitor_remove(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let removed = monitors::delete_target(&state.db, &id).map_err(|e| e.to_string())?;
    if removed {
        let _ = state.audit.write(EventInput {
            kind: "monitor.remove",
            actor: "user",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({ "id": id }),
        });
    }
    Ok(removed)
}

/// 045 FR-001 — lista los targets configurados (sin el último resultado; eso lo da `list_monitors`).
#[tauri::command]
pub fn monitor_list(state: State<'_, AppState>) -> Vec<monitors::MonitorTarget> {
    monitors::load_targets(&state.db)
}

/// Cap de monitores por tier. None = ilimitado (Team/Enterprise). Council: Free 5 / Pro 50 / Team ∞.
fn monitor_cap_for_tier(tier: &str) -> Option<usize> {
    match tier {
        "team" | "enterprise" => None,
        "pro" => Some(50),
        _ => Some(5), // free (default) + cualquier valor desconocido = conservador.
    }
}

// ── Cards (incidents) ───────────────────────────────────────────────

#[tauri::command]
pub fn seed_demo_cards(state: State<'_, AppState>) -> Result<usize, String> {
    // P0b (audit 3-frontera HIGH 1): defensa not-bypasseable. El seed de demo es una
    // herramienta SÓLO de desarrollo; en builds de release se rechaza en runtime aunque
    // alguna superficie lo invoque (palette universal, deeplink, MCP). NO usamos `#[cfg]`
    // para borrar el comando: queda registrado/compilado para no romper `generate_handler!`
    // ni el test de cobertura del registry — es un guard de runtime, no condicional de build.
    if !cfg!(debug_assertions) {
        return Err("seed demo solo disponible en builds de desarrollo".into());
    }
    let demos = [
        (
            "info",
            "furx",
            "monitor",
            "Hetzner DR latency 230ms",
            "above p95 of 90ms",
        ),
        (
            "warning",
            "scanner",
            "ci",
            "Phase-B smoke 5/6 — heartbeat lag",
            "pass-3a tick missed",
        ),
        (
            "critical",
            "toga",
            "doctrine",
            "BORA fetch falla 3x consecutivas",
            "anti-bot challenger detected",
        ),
    ];
    let conn = state.db.lock();
    let mut count = 0usize;
    for (sev, project, source, title, cause) in demos {
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO cards (id, project, source, title, cause, severity, confidence) VALUES (?,?,?,?,?,?,?)",
            params![id, project, source, title, cause, sev, 0.85_f64],
        ).map_err(|e| e.to_string())?;
        // F3: notify connected phones (filtered by the `card` toggle on the bridge).
        crate::services::mobile_bridge::publish_notification(
            "card",
            title,
            &format!("{} · {}", project, cause),
            sev,
            Some(id.clone()),
        );
        count += 1;
    }
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "cards.seeded",
            actor: "system",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"count": count}),
        })
        .map_err(|e| e.to_string())?;
    Ok(count)
}

/// spec-022 P1 · US6 — valida un `snooze_until` provisto por el front. El UPDATE es parametrizado
/// (sin riesgo de inyección), pero Codex LOW señaló que esto NO validaba semánticamente: una fecha
/// imposible (mes 13) o con offset quedaba como texto comparado lexicográficamente contra
/// `datetime('now')` → la card podía re-aparecer/ocultarse a destiempo.
/// Fix: parsear a un datetime UTC REAL y devolver el formato CANÓNICO `YYYY-MM-DD HH:MM:SS` (UTC,
/// el mismo que produce `datetime('now')` y que el front genera vía `computeSnoozeUntil`). Acepta:
///   - `YYYY-MM-DD HH:MM:SS` (formato del front, ya UTC; sin zona → se asume UTC).
///   - `YYYY-MM-DDTHH:MM:SS[.fff]Z` o con offset `±HH:MM` (RFC3339 → se convierte a UTC).
/// Rechaza cualquier cosa que no parsee a un instante real. NULL/None es válido (sin snooze).
fn validate_snooze_until(raw: &str) -> Result<String, String> {
    use chrono::{DateTime, NaiveDateTime, Utc};

    let s = raw.trim();
    if s.is_empty() || s.len() > 40 {
        return Err("snooze_until vacío o demasiado largo".into());
    }

    // 1) RFC3339 / ISO-8601 con zona explícita (`Z` o `±HH:MM`) → convertir a UTC.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt
            .with_timezone(&Utc)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string());
    }

    // 2) Sin zona: aceptar separador ' ' o 'T'. Se interpreta como UTC (el front ya manda UTC).
    //    Probamos con segundos, y con fracción de segundo opcional.
    let normalized = s.replacen('T', " ", 1);
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Ok(ndt.format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }

    Err(format!("snooze_until no es una fecha/hora válida: {}", s))
}

#[tauri::command]
pub fn decide_card(
    state: State<'_, AppState>,
    card_id: String,
    decision: String,
    note: Option<String>,
    // spec-022 P1 · US6 — duración de snooze explícita (1h/4h/mañana). Sólo aplica a `snoozed`.
    // Si viene None con decision=snoozed, se cae al default legacy (1h) por compat.
    snooze_until: Option<String>,
    // 050 Ola 8 P2 (FR-004) — idempotency key (defensa extra sobre el invokeSeqRef de la Ola 3, para
    // el caso multi-instancia FUTURO). Si viene, y YA existe un evento `card.decided` con esta misma
    // key, la decisión es un NO-OP idempotente (no re-decide ni re-audita). None = comportamiento
    // legacy (sin idempotencia). El seqRef del front sigue cubriendo la doble-respuesta intra-ventana;
    // esto cubre dos instancias/ventanas que replayen la MISMA decisión.
    idempotency_key: Option<String>,
) -> Result<(), String> {
    // Whitelist decisions. `read`/`dismissed` son acciones de INBOX (no cierran la card: sólo la
    // sacan del inbox activo vía read_at/dismissed_at), el resto son decisiones de aprobación.
    let allowed = [
        "approved",
        "rejected",
        "needs-changes",
        "snoozed",
        "read",
        "dismissed",
    ];
    if !allowed.contains(&decision.as_str()) {
        return Err(format!("invalid decision: {}", decision));
    }
    // status: `read`/`dismissed`/`snoozed` mantienen la card 'open' (sigue viva, sólo cambia su
    // visibilidad en el inbox); approved/rejected/needs-changes la cierran.
    let status = if matches!(decision.as_str(), "snoozed" | "read" | "dismissed") {
        "open"
    } else {
        "closed"
    };
    // 044 FR-001 — el card-write y el audit-write van en UNA transacción SQLite: si el audit falla,
    // el cambio de la card se REVIERTE (no queda una card "decidida" sin su rastro auditado). El
    // append-only de `events` queda intacto (sólo hacemos INSERT; los triggers anti UPDATE/DELETE
    // siguen vigentes). `state.db` y `state.audit` comparten el MISMO `Arc<Mutex<Connection>>`, así
    // que el INSERT del audit va por `write_in_tx(&conn, ...)` sobre la conexión ya lockeada (sin
    // doble-lock). Los efectos colaterales (snapshot/cloud/notify) corren DESPUÉS del commit.
    let actor = crate::services::identity::current_actor();
    // 050 FR-004 — saneo de la idempotency key (defensa: no es texto libre; un token de la UI). Cap
    // de largo + charset acotado; vacío = sin key. Se valida ANTES de tocar la DB.
    let idem = idempotency_key.as_deref().and_then(|k| {
        let k = k.trim();
        if k.is_empty()
            || k.len() > 128
            || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            None
        } else {
            Some(k.to_string())
        }
    });
    let conn = state.db.lock();
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    // 050 FR-004 — corto-circuito idempotente: si ya hay un `card.decided` con esta key para esta
    // card, devolvemos Ok sin re-decidir ni re-auditar (multi-instancia futuro). El match es por
    // (card_id, idempotency_key) en el payload del evento append-only. El chequeo va DENTRO de la
    // misma transacción que el INSERT del audit (audit deepseek HIGH 1): así el SELECT-luego-INSERT
    // es atómico aun bajo dos PROCESOS concurrentes (multi-instancia) — sin esto, dos instancias
    // podrían ambas pasar el SELECT y duplicar el evento. Intra-proceso ya está serializado por el
    // Mutex<Connection>, pero la transacción cierra también la ventana cross-proceso.
    if let Some(key) = &idem {
        let seen: bool = tx
            .query_row(
                "SELECT 1 FROM events \
                 WHERE kind = 'card.decided' AND card_id = ? \
                   AND json_extract(payload, '$.idempotency_key') = ? LIMIT 1",
                params![card_id, key],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if seen {
            // No-op idempotente: cerramos la txn sin escribir (rollback implícito; no se decidió nada).
            return Ok(());
        }
    }
    match decision.as_str() {
        "snoozed" => {
            // Duración explícita validada, o default 1h si no vino (compat). Al snoozear se limpia el
            // flag `reopened` (la card vuelve a estar "quieta" hasta el próximo evento/expiración).
            let until: String = match snooze_until.as_deref() {
                Some(raw) => validate_snooze_until(raw)?,
                None => {
                    // default: ahora + 1 hora (SQLite la evalúa).
                    tx.query_row("SELECT datetime('now', '+1 hour')", [], |r| r.get(0))
                        .map_err(|e| e.to_string())?
                }
            };
            tx.execute(
                "UPDATE cards SET decision = ?, decided_at = datetime('now'), decision_note = ?, \
                 status = 'open', snooze_until = ?, reopened = 0 WHERE id = ?",
                params![decision, note, until, card_id],
            )
            .map_err(|e| e.to_string())?;
        }
        "read" => {
            tx.execute(
                "UPDATE cards SET read_at = datetime('now') WHERE id = ?",
                params![card_id],
            )
            .map_err(|e| e.to_string())?;
        }
        "dismissed" => {
            tx.execute(
                "UPDATE cards SET decision = ?, decided_at = datetime('now'), decision_note = ?, \
                 status = 'open', dismissed_at = datetime('now') WHERE id = ?",
                params![decision, note, card_id],
            )
            .map_err(|e| e.to_string())?;
        }
        _ => {
            // approved / rejected / needs-changes — cierran la card y limpian cualquier snooze.
            tx.execute(
                "UPDATE cards SET decision = ?, decided_at = datetime('now'), decision_note = ?, \
                 status = ?, snooze_until = NULL WHERE id = ?",
                params![decision, note, status, card_id],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    // Audit-write en la MISMA transacción. Si esto falla, `tx` se dropea sin commit → ROLLBACK del
    // UPDATE de la card. El `?` propaga el error tras revertir (el `drop(tx)` implícito hace rollback).
    // 050 FR-004 — la idempotency key SÓLO se agrega al payload cuando vino (audit deepseek MED 2: no
    // guardamos `"idempotency_key": null` en los eventos legacy/sin-key). Es la fuente de verdad del
    // corto-circuito de una replay futura (multi-instancia). Las queries `json_extract(...)=?` sólo
    // bindean keys no-nulas, así que un evento sin la clave nunca matchea (sin falsos positivos).
    let mut payload =
        serde_json::json!({"decision": decision, "note": note, "snooze_until": snooze_until});
    if let Some(key) = &idem {
        payload["idempotency_key"] = serde_json::Value::String(key.clone());
    }
    let ev = EventInput {
        kind: "card.decided",
        actor: &actor,
        pane_id: None,
        card_id: Some(&card_id),
        correlation_id: None,
        payload,
    };
    let event_id = state
        .audit
        .write_in_tx(&tx, &ev)
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);
    // Post-commit: efectos colaterales (auto-snapshot/cloud/notify). Fuera de la txn y del lock.
    state.audit.post_write_effects(&event_id, "card.decided", &ev);
    Ok(())
}

/// spec-022 P1 · US6 — auto-unsnooze ante NUEVA ACTIVIDAD de la fuente. Cuando un productor de cards
/// (monitor, job, watcher) re-observa la causa de una card existente, llama acá con el timestamp del
/// evento. Si la card estaba snoozeada, se reabre con badge "Reabierto" (reopened=1) y se limpia el
/// snooze; si no, sólo se refresca `last_activity_at`. Idempotente y no destructivo.
#[tauri::command]
pub fn card_record_activity(
    state: State<'_, AppState>,
    card_id: String,
) -> Result<bool, String> {
    let conn = state.db.lock();
    // ¿Estaba snoozeada y sin expirar? → reabrir.
    let was_snoozed: bool = conn
        .query_row(
            "SELECT 1 FROM cards WHERE id = ? AND status = 'open' \
             AND snooze_until IS NOT NULL AND snooze_until > datetime('now')",
            params![card_id],
            |_| Ok(true),
        )
        .unwrap_or(false);
    conn.execute(
        "UPDATE cards SET last_activity_at = datetime('now'), \
         snooze_until = CASE WHEN snooze_until IS NOT NULL AND snooze_until > datetime('now') THEN NULL ELSE snooze_until END, \
         reopened = CASE WHEN snooze_until IS NOT NULL AND snooze_until > datetime('now') THEN 1 ELSE reopened END \
         WHERE id = ?",
        params![card_id],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    if was_snoozed {
        state
            .audit
            .write(EventInput {
                kind: "card.reopened",
                actor: "system",
                pane_id: None,
                card_id: Some(&card_id),
                correlation_id: None,
                payload: serde_json::json!({"reason": "new_activity"}),
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(was_snoozed)
}

// ── Settings ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>, key: String) -> Result<Option<Value>, String> {
    let conn = state.db.lock();
    settings_store::get(&conn, &key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, key: String, value: Value) -> Result<(), String> {
    // 027 F2-wiring (audit codex BLOCKER): las keys reservadas `policy.*` (gobierno) NO se pueden
    // escribir por el setter genérico — eso bypassearía el gate + el audit y permitiría RELAJAR la
    // política custom en silencio (ej. apagar `policy.custom_enabled`). Se gestionan SÓLO vía los
    // comandos dedicados gateados+auditados de policy (p.ej. `policy_set_custom_enabled`).
    if key.starts_with("policy.") {
        return Err(format!(
            "'{key}' es una key de gobierno reservada: usá el comando dedicado de policy (gateado + auditado), no settings_set"
        ));
    }
    // Audit fix codex US7: VALIDAR contra el schema del registry acá, en el path único de
    // persistencia, para que NINGÚN call-site (ni el `settings_set` público que usan las
    // vistas viejas) pueda bypassear la validación. Keys desconocidas (legacy/ad-hoc) pasan
    // sin schema (siguen funcionando) pero igual pasan el guardrail de secretos de abajo.
    settings_registry::validate(&key, &value)
        .map_err(|e| format!("invalid value for '{key}': {e}"))?;
    // F32 Guardrail — bloquea valores con secretos detectables.
    let serialized = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    let findings = guardrail::scan(&serialized);
    if !findings.is_empty() {
        let kinds: Vec<&str> = findings.iter().map(|f| f.pattern_id).collect();
        state
            .audit
            .write(EventInput {
                kind: "guardrail.blocked",
                actor: "system",
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"key": &key, "patterns": kinds}),
            })
            .ok();
        return Err(format!(
            "settings rejected by secret guardrail: {:?}",
            kinds
        ));
    }
    let conn = state.db.lock();
    settings_store::set(&conn, &key, &value).map_err(|e| e.to_string())?;
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "settings.changed",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"key": key}),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn settings_all(state: State<'_, AppState>) -> Result<Vec<(String, Value)>, String> {
    let conn = state.db.lock();
    settings_store::all(&conn).map_err(|e| e.to_string())
}

// ── 042 FR-002 — Wizard onboarding (endpoints + health-check) ─────────

/// Health-check de los endpoints que el usuario tipea en el wizard (botón "Probar"). Async (red),
/// timeout DURO 1500ms, sin seguir redirects. No persiste nada — sólo informa reachable/latencia.
#[tauri::command]
pub async fn setup_health_check(
    aie_url: String,
    ollama_url: String,
) -> Result<crate::services::wizard::HealthPair, String> {
    crate::services::wizard::health_check(&aie_url, &ollama_url).await
}

/// Guarda los endpoints del wizard (sync): valida con `url::Url`, persiste en settings y agrega los
/// hosts a la allowlist runtime bajo el `db.lock()` (lock-ordering DB→allowlist). Un campo vacío se
/// deja en su default. Audita el cambio (sin valores de endpoint en el payload, sólo qué se tocó).
#[tauri::command]
pub fn wizard_save_endpoints(
    state: State<'_, AppState>,
    aie_url: String,
    ollama_url: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    crate::services::wizard::save_endpoints(&conn, &aie_url, &ollama_url)?;
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "wizard.endpoints_saved",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({
                "aie_set": !aie_url.trim().is_empty(),
                "ollama_set": !ollama_url.trim().is_empty(),
            }),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Settings registry (US7, spec 015) ────────────────────────────────

/// The curated, typed settings registry. The front generates its Settings UI
/// (tabs + search + controls) from this. See `services::settings_registry`.
#[tauri::command]
pub fn settings_registry_list() -> Result<Vec<settings_registry::SettingDef>, String> {
    settings_registry::settings_registry_list()
}

/// Validate `value` against the registry schema for `key`, then persist via the
/// same path as `settings_set` (secret guardrail + audit). Unknown keys skip
/// schema validation (legacy/ad-hoc settings keep working) but still pass the
/// guardrail.
#[tauri::command]
pub fn settings_set_validated(
    state: State<'_, AppState>,
    key: String,
    value: Value,
) -> Result<(), String> {
    // Validación ahora vive en `settings_set` (path único) — delegamos. Se mantiene este
    // comando explícito por claridad de API; la validación NO se puede bypassear.
    settings_set(state, key, value)
}

// ── Mobile companion (spec 004) ──────────────────────────────────────

/// The 64-hex pairing secret (Keychain `furx-mobile/shared-secret`), generated
/// on first call. Shown in Settings → Mobile so the user can paste it / scan a
/// QR into the companion. NOT a provider key (BYOK untouched).
#[tauri::command]
pub fn mobile_secret_get() -> Result<String, String> {
    crate::services::mobile_bridge::ensure_secret().map_err(|e| e.to_string())
}

/// Rotate the pairing secret. Paired phones must re-pair; the running bridge
/// keeps the old secret until Furx restarts (surfaced in the UI).
#[tauri::command]
pub fn mobile_secret_rotate() -> Result<String, String> {
    crate::services::mobile_bridge::rotate_secret().map_err(|e| e.to_string())
}

/// Bridge runtime status for Settings → Mobile: whether it's listening, on which
/// addresses, and the detected Tailscale IP (for the off-LAN pairing URL/QR).
#[tauri::command]
pub fn mobile_bridge_status(state: State<'_, AppState>) -> Result<Value, String> {
    let guard = state.mobile_bridge.lock();
    let addrs: Vec<String> = guard
        .as_ref()
        .map(|b| b.local_addrs().iter().map(|a| a.to_string()).collect())
        .unwrap_or_default();
    let tailscale_ip = crate::services::mobile_bridge::tailscale_ipv4().map(|i| i.to_string());
    Ok(serde_json::json!({
        "running": guard.is_some(),
        "addrs": addrs,
        "tailscale_ip": tailscale_ip,
        "loopback_port": crate::services::mobile_bridge::MOBILE_BRIDGE_PORT,
        "tailscale_port": crate::services::mobile_bridge::MOBILE_BRIDGE_TAILSCALE_PORT,
    }))
}

// ── Mobile pairing QR (spec 065) ─────────────────────────────────────
#[derive(Serialize)]
pub struct PairingQrData {
    pub uri: String,
    pub session_id: String,
    pub short_code: String,
    pub exp_epoch: u64,
}

/// Genera el QR de pairing: token efímero (en el URI) + short_code + session_id + expiración. El
/// secreto permanente NUNCA va en el QR. Gate: el secreto debe existir (mismo que `mobile_secret_get`).
#[tauri::command]
pub fn mobile_pairing_qr_generate(state: State<'_, AppState>) -> Result<PairingQrData, String> {
    crate::services::mobile_bridge::ensure_secret().map_err(|e| e.to_string())?;

    let port = crate::services::mobile_bridge::MOBILE_BRIDGE_PORT;
    let ts_port = crate::services::mobile_bridge::MOBILE_BRIDGE_TAILSCALE_PORT;
    // Derivar AMBOS hosts de los BINDINGS REALES (`local_addrs`), no de detección de interfaz (audit
    // codex): `h` = LAN no-loopback en :43118; `ts` = la addr Tailscale en :43119 SOLO si el bridge la
    // bindeó (mobile.tailscale_enabled). Sin esto, una iface Tailscale con el setting OFF generaba un QR
    // inalcanzable. El bridge hoy solo bindea loopback:43118 + (opcional) Tailscale:43119.
    let (lan_ips, ts_param): (Vec<String>, String) = {
        let bridge = state.mobile_bridge.lock();
        let addrs = bridge.as_ref().map(|b| b.local_addrs()).unwrap_or_default();
        let lan = addrs
            .iter()
            .filter(|a| a.port() == port && !a.ip().is_loopback())
            .take(2)
            .map(|a| a.ip().to_string())
            .collect();
        let ts = addrs
            .iter()
            .find(|a| a.port() == ts_port && !a.ip().is_loopback())
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
        (lan, ts)
    };

    // El teléfono NO alcanza loopback. Si no hay LAN real NI Tailscale bindeado, el QR no tendría host
    // alcanzable → error accionable ANTES de gastar una sesión (audit codex: sesión antes del check).
    if ts_param.is_empty() && lan_ips.is_empty() {
        return Err(
            "no hay host alcanzable para un teléfono: activá el acceso por Tailscale en Ajustes → Móvil \
             (el bridge solo escucha en loopback + Tailscale)"
                .into(),
        );
    }

    let s = crate::services::mobile_qr_pairing::generate_session().map_err(|e| e.to_string())?;

    // Hostname: máx 15 code points (maneja emoji/CJK sin cortar a mitad), percent-encoded.
    let hostname_raw = hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "Furx".to_string());
    let name_trunc: String = hostname_raw.chars().take(15).collect();
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let name_enc = utf8_percent_encode(&name_trunc, NON_ALPHANUMERIC).to_string();

    let build = |hosts: &str, name: &str| {
        let mut u = format!(
            "furx://pair?v=1&t={}&h={}&p={}&exp={}&n={}",
            s.token_hex, hosts, port, s.exp_epoch, name,
        );
        if !ts_param.is_empty() {
            u.push_str(&format!("&ts={ts_param}"));
        }
        u
    };
    // Cap de URI ≤175 GARANTIZADO (audit codex MED): reducción progresiva — 2 IPs+name → 1 IP+name →
    // 1 IP sin name → solo ts sin name. La última (token 64 + ts ~15, sin h/name) es ~125 chars < 175.
    let one_ip = lan_ips.first().cloned().unwrap_or_default();
    let candidates = [
        (lan_ips.join(","), name_enc.as_str()),
        (one_ip.clone(), name_enc.as_str()),
        (one_ip, ""),
        (String::new(), ""),
    ];
    let uri = candidates
        .into_iter()
        .map(|(h, n)| build(&h, n))
        .find(|u| u.len() <= 175)
        .ok_or("no se pudo generar un URI dentro del límite del QR")?;

    Ok(PairingQrData {
        uri,
        session_id: s.session_id,
        short_code: s.short_code,
        exp_epoch: s.exp_epoch,
    })
}

/// Estado de una sesión de pairing (poll de respaldo del evento `mobile-pairing-done`).
#[tauri::command]
pub fn mobile_pairing_status(session_id: String) -> String {
    use crate::services::mobile_qr_pairing::{session_status, SessionStatus};
    match session_status(&session_id) {
        SessionStatus::Completed => "completed".to_string(),
        SessionStatus::Pending => "pending".to_string(),
        SessionStatus::Expired => "expired".to_string(),
    }
}

// ── Distribution ─────────────────────────────────────────────────────

#[tauri::command]
pub fn compat_check() -> CompatReport {
    distribution::compat_check()
}

#[tauri::command]
pub async fn check_updates(state: State<'_, AppState>) -> Result<UpdateInfo, String> {
    // 041 FR-003 — updates endpoint resolution: env `FURX_UPDATES_URL` wins, then settings
    // `endpoints.updates`, then the documented default release repo. Migration 048 empties the
    // seeded `endpoints.updates`, so we MUST treat an empty/blank value as unset (not pass `""` to
    // the allowlist). The `hernaninverso/furx` repo stays as the documented default per
    // FR-003 (it's where el autor publishes releases; any user overrides via env/settings).
    const DEFAULT_UPDATES_URL: &str =
        "https://api.github.com/repos/hernaninverso/furx/releases/latest";
    let endpoint = {
        let env_url = std::env::var("FURX_UPDATES_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(u) = env_url {
            u
        } else {
            let conn = state.db.lock();
            settings_store::get(&conn, "endpoints.updates")
                .map_err(|e| e.to_string())?
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_UPDATES_URL.to_string())
        }
    };
    // SSRF allowlist: solo api.github.com (+ subdomain *.github.com) o https://updates.furx.* (dominio propio).
    let allowed = endpoint.starts_with("https://api.github.com/")
        || endpoint.starts_with("https://updates.furx.")
        || endpoint.starts_with("https://github.com/");
    if !allowed {
        return Err(format!("updates endpoint not in allowlist: {}", endpoint));
    }
    let current = env!("CARGO_PKG_VERSION");
    Ok(distribution::check_updates(&endpoint, current).await)
}

#[tauri::command]
pub fn reset_furx(state: State<'_, AppState>, level: String) -> Result<ResetReport, String> {
    // SUPER-WARNING: irreversible. We require an audit event BEFORE the reset.
    state
        .audit
        .write(EventInput {
            kind: "reset.requested",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"level": &level}),
        })
        .map_err(|e| e.to_string())?;
    distribution::reset(&level).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct GuardrailReport {
    pub clean: bool,
    pub findings: Vec<GuardrailFinding>,
}
#[derive(Serialize)]
pub struct GuardrailFinding {
    pub pattern_id: String,
    pub sample: String,
}

#[tauri::command]
pub fn guardrail_scan(payload: String) -> GuardrailReport {
    let findings = guardrail::scan(&payload);
    GuardrailReport {
        clean: findings.is_empty(),
        findings: findings
            .into_iter()
            .map(|f| GuardrailFinding {
                pattern_id: f.pattern_id.to_string(),
                sample: f.sample,
            })
            .collect(),
    }
}

// ── Layout persistence ───────────────────────────────────────────────

#[derive(Serialize, serde::Deserialize)]
pub struct Layout {
    pub id: String,
    pub name: String,
    pub panes: Vec<LayoutPane>,
    pub grid_cols: String,
    pub grid_rows: String,
}

#[derive(Serialize, serde::Deserialize)]
pub struct LayoutPane {
    pub id: String,
    pub mode: String,
    pub title: String,
    // BLOQUE B · Codex audit MED #2: persist cwd + bundle_path so the per-pane
    // sticky context survives layout reload. Validated/canonicalised by
    // pty_spawn's allowlist at the next spawn call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_path: Option<String>,
}

#[tauri::command]
pub fn get_layout(
    state: State<'_, AppState>,
    id: Option<String>,
) -> Result<Option<Layout>, String> {
    let lid = id.unwrap_or_else(|| "default".into());
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT id, name, panes, grid_cols, grid_rows FROM layouts WHERE id = ?")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![lid]).map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let panes_json: String = row.get(2).map_err(|e| e.to_string())?;
    let panes: Vec<LayoutPane> = serde_json::from_str(&panes_json).unwrap_or_default();
    Ok(Some(Layout {
        id: row.get(0).map_err(|e| e.to_string())?,
        name: row.get(1).map_err(|e| e.to_string())?,
        panes,
        grid_cols: row.get(3).map_err(|e| e.to_string())?,
        grid_rows: row.get(4).map_err(|e| e.to_string())?,
    }))
}

#[tauri::command]
pub fn save_layout(state: State<'_, AppState>, layout: Layout) -> Result<(), String> {
    let panes_json = serde_json::to_string(&layout.panes).map_err(|e| e.to_string())?;
    let conn = state.db.lock();
    conn.execute(
        "INSERT INTO layouts (id, name, panes, grid_cols, grid_rows) VALUES (?,?,?,?,?) \
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, panes=excluded.panes, \
            grid_cols=excluded.grid_cols, grid_rows=excluded.grid_rows, updated_at=datetime('now')",
        params![
            layout.id,
            layout.name,
            panes_json,
            layout.grid_cols,
            layout.grid_rows
        ],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "layout.saved",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"layout_id": layout.id}),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Sprint 2026-05-25 services ──────────────────────────────────────

/// F25 — Manual ⌘⇧S snapshot from UI.
#[tauri::command]
pub fn snapshot_take(
    state: State<'_, AppState>,
    kind: String,
) -> Result<snapshot::SnapshotInfo, String> {
    let info = snapshot::write(state.db.clone(), &kind).map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "snapshot.taken",
        actor: &if kind == "manual" { crate::services::identity::current_actor() } else { "system".to_string() },
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"snapshot_id": info.id, "snapshot_kind": info.kind, "bytes": info.bytes}),
    }).map_err(|e| e.to_string())?;
    Ok(info)
}

#[tauri::command]
pub fn snapshot_list(state: State<'_, AppState>) -> Result<Vec<snapshot::SnapshotInfo>, String> {
    snapshot::list(&state.db).map_err(|e| e.to_string())
}

/// F11 — Trigger project rescan (background-friendly; UI shows toast on completion).
#[tauri::command]
pub fn projects_scan(state: State<'_, AppState>) -> Result<usize, String> {
    let n = projects::scan(state.db.clone()).map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "projects.scanned",
            actor: "system",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"count": n}),
        })
        .ok();
    Ok(n)
}

#[tauri::command]
pub fn projects_list(state: State<'_, AppState>) -> Result<Vec<projects::Project>, String> {
    projects::list(&state.db).map_err(|e| e.to_string())
}

/// F1 — Compile bootstrap for a pane. Stored to ~/.furx/contexts/&lt;pane&gt;-bootstrap.md.
#[tauri::command]
pub fn bootstrap_compile(
    state: State<'_, AppState>,
    pane_id: String,
    project_dir: Option<String>,
) -> Result<String, String> {
    let proj: Option<PathBuf> = project_dir.map(PathBuf::from);
    let conn = state.db.lock();
    let path =
        bootstrap::compile_for_pane(&pane_id, proj.as_deref(), &conn).map_err(|e| e.to_string())?;
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "bootstrap.compiled",
            actor: "system",
            pane_id: Some(&pane_id),
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"path": path.to_string_lossy()}),
        })
        .ok();
    Ok(path.to_string_lossy().to_string())
}

/// F21 — Build a context bundle from explicit inputs (project dir + optional log path + note).
/// If `save_for_card_id` is set, persists the bundle to ~/.furx/contexts/card-&lt;id&gt;.md
/// AND updates `cards.context_bundle_path`.
#[derive(serde::Deserialize)]
pub struct BundleRequest {
    pub project_dir: Option<String>,
    pub log_path: Option<String>,
    pub note: Option<String>,
    pub save_for_card_id: Option<String>,
}

#[tauri::command]
pub fn bundle_build(
    state: State<'_, AppState>,
    req: BundleRequest,
) -> Result<bundle::Bundle, String> {
    let project_dir = req.project_dir.as_deref().map(std::path::Path::new);
    let log_path = req.log_path.as_deref().map(std::path::Path::new);
    let b = bundle::build(bundle::BundleInputs {
        project_dir,
        log_path,
        extra_note: req.note.as_deref(),
    });
    if let Some(card_id) = req.save_for_card_id.as_deref() {
        let path = bundle::save_for_card(card_id, &b).map_err(|e| e.to_string())?;
        let conn = state.db.lock();
        conn.execute(
            "UPDATE cards SET context_bundle_path = ? WHERE id = ?",
            params![path.to_string_lossy().to_string(), card_id],
        )
        .map_err(|e| e.to_string())?;
        drop(conn);
        state.audit.write(EventInput {
            kind: "bundle.saved",
            actor: "system",
            pane_id: None, card_id: Some(card_id), correlation_id: None,
            payload: serde_json::json!({"path": path.to_string_lossy(), "redacted": b.redacted}),
        }).ok();
    }
    Ok(b)
}

// ── F3 / F2 / F8 worktree + routing + merge ─────────────────────────

#[tauri::command]
pub fn worktree_ensure(
    state: State<'_, AppState>,
    repo_path: String,
    branch: String,
) -> Result<worktree::Worktree, String> {
    let wt =
        worktree::ensure(std::path::Path::new(&repo_path), &branch).map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "worktree.ensured",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"repo": &wt.repo_path, "branch": &wt.branch, "path": &wt.worktree_path, "created": wt.created}),
    }).ok();
    Ok(wt)
}

#[tauri::command]
pub fn worktree_list(repo_path: String) -> Result<Vec<worktree::Worktree>, String> {
    worktree::list_for_repo(std::path::Path::new(&repo_path)).map_err(|e| e.to_string())
}

/// F2 — "Open in Claude" from a card: produces a recommended pane spec.
/// The frontend uses it to addPane(claude-A) with the proper cwd + bundle path.
#[derive(Serialize)]
pub struct OpenInClaudeReco {
    pub bundle_path: Option<String>,
    pub project_dir: Option<String>,
    pub suggested_mode: String,
}

#[tauri::command]
pub fn card_open_in_claude(
    state: State<'_, AppState>,
    card_id: String,
) -> Result<OpenInClaudeReco, String> {
    let conn = state.db.lock();
    let row = conn
        .query_row(
            "SELECT project, source, title, cause, context_bundle_path FROM cards WHERE id = ?",
            params![card_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    let (project, source, title, cause, existing_bundle) = row;
    drop(conn);

    // Find project dir candidate from the cached project registry.
    let project_dir: Option<String> = {
        let conn = state.db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE name = ? LIMIT 1",
            params![project],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };

    // Build / reuse the bundle.
    let bundle_path: Option<String> = if let Some(p) = existing_bundle {
        Some(p)
    } else {
        let proj = project_dir.as_deref().map(std::path::Path::new);
        let note = format!(
            "Triggered from card `{}`\n- project: {}\n- source: {}\n- title: {}\n- cause: {}",
            card_id,
            project,
            source,
            title,
            cause.as_deref().unwrap_or("—"),
        );
        let b = bundle::build(bundle::BundleInputs {
            project_dir: proj,
            log_path: None,
            extra_note: Some(&note),
        });
        match bundle::save_for_card(&card_id, &b) {
            Ok(path) => {
                let s = path.to_string_lossy().to_string();
                let conn = state.db.lock();
                let _ = conn.execute(
                    "UPDATE cards SET context_bundle_path = ? WHERE id = ?",
                    params![s, card_id],
                );
                drop(conn);
                Some(s)
            }
            Err(_) => None,
        }
    };

    state
        .audit
        .write(EventInput {
            kind: "card.open_in_claude",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: Some(&card_id),
            correlation_id: None,
            payload: serde_json::json!({"project_dir": project_dir, "bundle_path": bundle_path}),
        })
        .ok();

    Ok(OpenInClaudeReco {
        bundle_path,
        project_dir,
        suggested_mode: "claude-A".to_string(),
    })
}

/// BLOQUE B · F3 — Spawn a PTY whose cwd is a freshly-ensured git worktree.
/// Conveniencia: resuelve la worktree por `repo_path` + `branch` y delega a
/// `pty_spawn` con `cwd` ya validado por la allowlist ($HOME / /tmp).
#[tauri::command]
pub fn pty_spawn_in_worktree(
    app: AppHandle,
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    pane_id: String,
    repo_path: String,
    branch: String,
    mode: String,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    // Codex audit B MED: validate pane_id + rate-limit BEFORE creating a
    // worktree — otherwise an invalid request can still trigger a `git worktree
    // add` and leave an orphan directory.
    if pane_id.is_empty()
        || pane_id.len() > 64
        || !pane_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(format!("invalid pane_id: {}", pane_id));
    }
    // Rate-limit BEFORE doing any work on disk.
    if let Some(wait) = state.scheduler.try_acquire("pty:spawn") {
        return Err(format!("rate limited; retry in {}ms", wait.as_millis()));
    }
    let repo = std::path::Path::new(&repo_path);
    let wt = worktree::ensure(repo, &branch).map_err(|e| e.to_string())?;
    let wt_cwd = wt.worktree_path.clone();
    // pty_spawn re-validates pane_id + cwd allowlist + mode + does the audit
    // write. If it fails AFTER worktree creation, we leave the worktree in
    // place because `created` may be re-used on retry (idempotent ensure).
    pty_spawn(
        app,
        state,
        pty,
        pane_id,
        mode,
        Some(wt_cwd.clone()),
        rows,
        cols,
        None,
        None,
    )?;
    Ok(wt_cwd)
}

/// BLOQUE B · F21 — Standalone card context bundle for a given card.
/// Frontend can call this to refresh the bundle without going through
/// `card_open_in_claude` (e.g. for re-arming an existing pane).
#[tauri::command]
pub fn card_context_for(state: State<'_, AppState>, card_id: String) -> Result<String, String> {
    let bundle = bundle::card_context(&state.db, &card_id).map_err(|e| e.to_string())?;
    let path = bundle::save_for_card(&card_id, &bundle).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

/// F8 — Merge review: returns diff stat + list of risky files (no execute).
#[derive(Serialize)]
pub struct MergeReview {
    pub branch: String,
    pub diff_stat: String,
    pub risky_paths: Vec<String>,
}

/// spec 004 F2 — read-only Git surface for a project-context pane.
#[derive(serde::Serialize)]
pub struct GitOverview {
    pub branch: String,
    pub dirty: u32,
    pub clean: bool,
    pub diff_stat: String,
}

#[tauri::command]
pub fn git_overview(repo_path: String) -> Result<GitOverview, String> {
    let cwd = std::path::Path::new(&repo_path);
    if !cwd.is_dir() {
        return Err("not a directory".into());
    }
    let run = |args: &[&str]| -> Option<String> {
        std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]).ok_or("not a git repo")?;
    let status = run(&["status", "--porcelain"]).unwrap_or_default();
    let dirty = status.lines().filter(|l| !l.trim().is_empty()).count() as u32;
    let diff_stat: String = run(&["diff", "--stat", "--stat-width=64"])
        .unwrap_or_default()
        .chars()
        .take(4000)
        .collect();
    Ok(GitOverview {
        branch,
        dirty,
        clean: dirty == 0,
        diff_stat,
    })
}

#[tauri::command]
pub fn worktree_merge_review(repo_path: String, branch: String) -> Result<MergeReview, String> {
    if !worktree::is_safe_branch_for_api(&branch) {
        return Err(format!("unsafe branch: {}", branch));
    }
    let cwd = std::path::Path::new(&repo_path);
    if !cwd.is_dir() || !cwd.join(".git").exists() {
        return Err("not a git repo".into());
    }
    let stat = std::process::Command::new("git")
        .current_dir(cwd)
        .args(["diff", "--stat", &format!("...{}", branch)])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| e.to_string())?;
    let diff_stat = String::from_utf8_lossy(&stat.stdout).to_string();
    let files = std::process::Command::new("git")
        .current_dir(cwd)
        .args(["diff", "--name-only", &format!("...{}", branch)])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| e.to_string())?;
    let risky: Vec<String> = String::from_utf8_lossy(&files.stdout)
        .lines()
        .filter(|p| {
            let lower = p.to_lowercase();
            lower.contains("migration")
                || lower.starts_with(".env")
                || lower.contains("/.env")
                || lower.contains("secret")
                || lower.contains("credential")
        })
        .map(String::from)
        .collect();
    Ok(MergeReview {
        branch,
        diff_stat,
        risky_paths: risky,
    })
}

// ── F5 / F15 strips ─────────────────────────────────────────────────

#[tauri::command]
pub fn claude_usage_summary() -> claude_usage::UsageSummary {
    claude_usage::summary()
}

/// BLOQUE E · F5 — per-pane Claude usage. Frontend calls this on every pane
/// poll if the pane has a sticky `cwd`. Returns None when the CLI hasn't
/// written a usage.json under ~/.claude/projects/<encoded-cwd> yet.
#[tauri::command]
pub fn claude_usage_for_cwd(cwd: String) -> Option<claude_usage::PaneUsage> {
    // Defensive: only accept absolute paths under $HOME / /tmp / /var.
    // No traversal, no weird control chars.
    if cwd.is_empty() || cwd.len() > 1024 {
        return None;
    }
    if !cwd.starts_with('/') {
        return None;
    }
    if cwd.contains("..") || cwd.contains('\0') {
        return None;
    }
    claude_usage::for_cwd(&cwd)
}

#[tauri::command]
pub async fn aie_state(state: State<'_, AppState>) -> Result<aie::AieStateSummary, String> {
    // BLOQUE J: centralised endpoint resolution.
    let endpoint = crate::services::aie_endpoint::resolve_url(&state.db);
    aie::fetch_state(&endpoint).await.map_err(|e| e.to_string())
}

// ── F10 ⌘P search ───────────────────────────────────────────────────

// F9 — suggested action heuristic over a chunk of PTY text.
#[tauri::command]
pub fn suggest_for_text(text: String) -> Option<suggest::Suggestion> {
    suggest::suggest(&text)
}

// F16 — MCP server health (parses ~/.claude.json mcpServers + .claude/.mcp.json).
// 045 FR-002 — anota cada server con su override de la DB (`enabled`): la DB GANA sobre el JSON en
// runtime. La probe de salud corre igual (informativo); el `enabled` le dice a la UI/inyector qué
// servers el usuario dejó activos sin tocar el JSON.
#[tauri::command]
pub async fn mcp_health(state: State<'_, AppState>) -> Result<mcp_health::McpHealthReport, String> {
    let mut report = mcp_health::check_all().await;
    let overrides = crate::services::mcp_overrides::load_overrides(&state.db);
    for s in &mut report.servers {
        s.enabled = overrides.get(&s.name).copied().unwrap_or(true);
    }
    Ok(report)
}

/// 045 FR-002 — togglea un MCP server (enabled/disabled) SIN tocar ~/.claude.json. Valida que el
/// `name` exista en la config canónica antes de persistir (nombres inventados → Err). DB = SSOT.
#[tauri::command]
pub fn mcp_set_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let known = mcp_health::list_server_names();
    crate::services::mcp_overrides::set_enabled(&state.db, &name, enabled, &known)
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "mcp.set_enabled",
        actor: "user",
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({ "name": name, "enabled": enabled }),
    });
    Ok(())
}

/// 045 FR-002 — lista los overrides persistidos (para UI/diagnóstico).
#[tauri::command]
pub fn mcp_overrides_list(
    state: State<'_, AppState>,
) -> Vec<crate::services::mcp_overrides::McpOverride> {
    crate::services::mcp_overrides::list_overrides(&state.db)
}

/// 045 FR-002 — auto-discovery: binarios `mcp-*` en $PATH ofrecidos como SUGERENCIA (NO auto-instala
/// ni auto-habilita — foco humano). Marca cuáles ya están en ~/.claude.json.
#[tauri::command]
pub fn mcp_discover() -> Vec<crate::services::mcp_overrides::DiscoveredMcp> {
    let configured = mcp_health::list_server_names();
    crate::services::mcp_overrides::discover_path(&configured)
}

// F17 — heatmap data from events table.
#[tauri::command]
pub fn heatmap_data(
    state: State<'_, AppState>,
    days: Option<u32>,
) -> Result<heatmap::HeatmapData, String> {
    heatmap::compute(&state.db, days.unwrap_or(30)).map_err(|e| e.to_string())
}

// F12 — smart-paste: classify a text blob (user-initiated, no auto-poll).
#[tauri::command]
pub fn smartpaste_classify(text: String) -> smartpaste::PasteClassification {
    smartpaste::classify(&text)
}

// F12 — read system clipboard via arboard.
#[tauri::command]
pub fn clipboard_read() -> Result<Option<String>, String> {
    clipboard::read().map_err(|e| e.to_string())
}

// F22 — list ~/.ssh/config Host entries.
#[tauri::command]
pub fn ssh_hosts() -> Result<Vec<ssh_config::SshHost>, String> {
    ssh_config::parse_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_ping(host_name: String) -> Result<ssh_config::SshHostPing, String> {
    let hosts = ssh_config::parse_config().map_err(|e| e.to_string())?;
    let host = hosts
        .into_iter()
        .find(|h| h.name == host_name)
        .ok_or("host not found in ssh config")?;
    Ok(ssh_config::ping(host).await)
}

// 021-voice-es — lee `voice.model` (`base`|`small`, default Base) desde la tabla settings.
fn voice_model_setting(state: &State<'_, AppState>) -> voice::VoiceModel {
    let conn = state.db.lock();
    let raw = settings_store::get(&conn, "voice.model")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    voice::VoiceModel::from_setting(&raw)
}

// 021-voice-es — lee `voice.language` (`es`|`auto`|`en`, default `es`) desde settings.
fn voice_language_setting(state: &State<'_, AppState>) -> String {
    let conn = state.db.lock();
    let raw = settings_store::get(&conn, "voice.language")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    voice::normalize_lang(&raw)
}

// F19 / 021-voice-es — whisper readiness check del modelo CONFIGURADO (multilingüe).
#[tauri::command]
pub fn whisper_check(state: State<'_, AppState>) -> whisper::WhisperCheck {
    whisper::check(voice_model_setting(&state))
}

// D / F19 — download model with streaming progress events.
// Frontend listens on the "voice:download-progress" event for {downloaded, total}.
#[tauri::command]
pub async fn voice_download_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<voice::DownloadResult, String> {
    state
        .audit
        .write(EventInput {
            kind: "voice.download_started",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({}),
        })
        .ok();
    // 021-voice-es — baja el modelo CONFIGURADO (base|small), no el tiny.en viejo.
    let model = voice_model_setting(&state);
    let app_for_cb = app.clone();
    let cb: voice::ProgressCb = Box::new(move |downloaded, total| {
        let _ = app_for_cb.emit(
            "voice:download-progress",
            serde_json::json!({"downloaded": downloaded, "total": total}),
        );
    });
    let r = voice::download_model_streamed(model, Some(cb))
        .await
        .map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "voice.download_completed",
            actor: "system",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"bytes": r.bytes, "path": &r.path}),
        })
        .ok();
    Ok(r)
}

// spec-005 — push-to-talk: held capture (start on ⌥Space keydown, stop on keyup).
#[tauri::command]
pub async fn voice_ptt_start() -> Result<String, String> {
    voice::ptt_start().await.map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn voice_ptt_stop(
    state: State<'_, AppState>,
    id: String,
) -> Result<voice::CaptureResult, String> {
    let r = voice::ptt_stop(&id).await.map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "voice.ptt.captured",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"bytes": r.bytes}),
        })
        .ok();
    Ok(r)
}
#[tauri::command]
pub async fn voice_ptt_cancel(id: String) {
    voice::ptt_cancel(&id).await;
}

#[tauri::command]
pub async fn voice_capture(
    state: State<'_, AppState>,
    seconds: u16,
) -> Result<voice::CaptureResult, String> {
    let r = voice::capture(seconds).await.map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "voice.captured",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"path": &r.path, "bytes": r.bytes, "seconds": r.seconds}),
        })
        .ok();
    Ok(r)
}

#[tauri::command]
pub async fn voice_transcribe(
    state: State<'_, AppState>,
    audio_path: String,
) -> Result<voice::TranscribeResult, String> {
    // Codex HIGH: voice_transcribe deletes the path after use. We restrict it to
    // canonical_parent == std::env::temp_dir() AND file name matches the strict
    // shape produced by voice_capture (`furx-voice-<uuid>.wav`). No ~/.furx.
    let p = std::path::PathBuf::from(&audio_path);
    let canonical = p.canonicalize().map_err(|e| e.to_string())?;
    let parent = canonical.parent().ok_or("no parent")?;
    let tmp_canonical = std::env::temp_dir()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if parent != tmp_canonical.as_path() {
        return Err("audio_path must live in std::env::temp_dir()".into());
    }
    let fname = canonical.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let safe = regex::Regex::new(r"^furx-voice-[0-9a-f-]{36}\.wav$").unwrap();
    if !safe.is_match(fname) {
        return Err(format!("invalid audio file name: {}", fname));
    }
    // 021-voice-es — modelo (base|small) + idioma (es|auto|en) configurados. Se leen ANTES
    // del `.await` (el guard del lock NO cruza el await). `-l es` arregla el dictado español.
    let model = voice_model_setting(&state);
    let lang = voice_language_setting(&state);
    let result = voice::transcribe(&canonical, model, &lang)
        .await
        .map_err(|e| e.to_string());
    // Always delete the temp WAV — success OR failure (no mic audio left on disk).
    let _ = std::fs::remove_file(&canonical);
    let r = result?;
    state
        .audit
        .write(EventInput {
            kind: "voice.transcribed",
            actor: "system",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"chars": r.text.len(), "elapsed_ms": r.elapsed_ms}),
        })
        .ok();
    Ok(r)
}

// ── C2 crash log commands ──────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct CrashJsInput {
    pub source: String, // "js-error" | "js-unhandled-rejection" | "manual"
    pub message: String,
    pub location: Option<String>,
    pub stack: Option<String>,
}

#[tauri::command]
pub fn crash_log_js(payload: CrashJsInput) -> Result<(), String> {
    use crate::services::crash_log::{write_entry, CrashEntry, CrashSource};
    // Codex MED v1: enforce per-field caps at the boundary (matches frontend cap).
    const MAX_MESSAGE: usize = 8 * 1024;
    const MAX_LOCATION: usize = 512;
    const MAX_STACK: usize = 8 * 1024;
    if payload.message.is_empty() || payload.message.len() > MAX_MESSAGE {
        return Err("invalid message".into());
    }
    if payload.location.as_ref().map(|s| s.len()).unwrap_or(0) > MAX_LOCATION {
        return Err("location too long".into());
    }
    if payload.stack.as_ref().map(|s| s.len()).unwrap_or(0) > MAX_STACK {
        return Err("stack too long".into());
    }
    let source = match payload.source.as_str() {
        "js-error" => CrashSource::JsError,
        "js-unhandled-rejection" => CrashSource::JsUnhandledRejection,
        "manual" => CrashSource::Manual,
        _ => return Err("invalid source".into()),
    };
    let entry = CrashEntry {
        iso_ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        version: env!("CARGO_PKG_VERSION").to_string(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        source,
        location: payload.location,
        message: payload.message,
        backtrace: payload.stack,
    };
    write_entry(&entry).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn crash_log_list() -> Vec<crate::services::crash_log::CrashSummary> {
    crate::services::crash_log::list_files()
}

#[tauri::command]
pub fn crash_log_read(filename: String) -> Result<String, String> {
    crate::services::crash_log::read_file(&filename).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn crash_log_delete(filename: String) -> Result<(), String> {
    crate::services::crash_log::delete_file(&filename).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn crash_log_clear() -> Result<usize, String> {
    crate::services::crash_log::clear_all().map_err(|e| e.to_string())
}

// AUDIT-HARDCODE 2026-05-26 — return the user's home directory so the
// frontend can derive sensible defaults instead of hardcoding `/Users/dev`.
#[tauri::command]
pub fn home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| "could not resolve home directory".to_string())
}

/// Expand a leading `~/` or `~` to the user's home directory. Returns the
/// path unchanged if no tilde is present or `$HOME` cannot be resolved.
pub fn expand_tilde(path: &str) -> String {
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod expand_tilde_tests {
    use super::expand_tilde;

    #[test]
    fn returns_absolute_unchanged() {
        assert_eq!(expand_tilde("/tmp/foo"), "/tmp/foo");
    }

    #[test]
    fn returns_relative_unchanged() {
        assert_eq!(expand_tilde("foo/bar"), "foo/bar");
    }

    #[test]
    fn expands_tilde_slash_to_home() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(expand_tilde("~/foo"), format!("{}/foo", home));
    }

    #[test]
    fn expands_bare_tilde() {
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn does_not_expand_tilde_in_middle() {
        assert_eq!(expand_tilde("/a/~/b"), "/a/~/b");
    }
}

// 058 — socket tmux DEDICADO de Furx: aísla por completo el server tmux de Furx del server por defecto
// del usuario (sin esto un `kill-server`/`set -g` tocaba las sesiones de trabajo ajenas). Centralizado
// acá para que "Furx sólo toca su propio socket" sea un INVARIANTE: cada invocación de tmux que spawnea
// un proceso pasa por `furx_tmux()`, que SIEMPRE antepone `-L furx`. Un call-site nuevo no puede olvidar
// los args y caer al socket del usuario (la clase de bug que esto ya causó dos veces: kill-server y
// capture-pane sobre el socket equivocado).
const FURX_TMUX_SOCKET: &str = "furx";

/// `tmux -L furx` listo para encadenar `.args([...])`. `bin` = "tmux" (vía PATH) o el path resuelto por
/// `which` (lo usa `spawn_furx_scroll_opts`). NO incluye los call-sites que construyen la LÍNEA de
/// comando del PTY (`wrap_with_tmux_if_available`): esos no spawnean un Command, usan `FURX_TMUX_SOCKET`.
fn furx_tmux(bin: &str) -> std::process::Command {
    let mut c = std::process::Command::new(bin);
    c.args(["-L", FURX_TMUX_SOCKET]);
    c
}

// RESTORE-FIX 2026-05-26 — expose tmux availability + capture pane history so
// the frontend can replay scrollback before live PTY data arrives.
#[tauri::command]
pub fn tmux_available() -> bool {
    std::process::Command::new("which")
        .arg("tmux")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

#[tauri::command]
pub fn pty_capture_history(mode: String, lines: Option<i64>) -> Result<String, String> {
    // 058 (ultrareview fix) — `mode` acá es el `sessionKey` del front (= orch_session || paneId). El
    // allowlist de `pane_id` en pty_spawn YA permite `.` y `@` ("@<ts>" para remount); rechazarlos acá
    // rompía el resume para esos panes. Aceptamos el MISMO charset que pane_id — `furx_session_name`
    // sanitiza todo char no-[A-Za-z0-9_] a `_` ANTES de tocar tmux, así que `.`/`@` nunca llegan crudos.
    if mode.is_empty()
        || mode.len() > 64
        || !mode
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(format!("invalid mode: {}", mode));
    }
    let session = furx_session_name(&mode); // 056 — sanitización única (audit BLOCKER)
    // Check session exists first — tmux capture-pane on a missing session emits an error.
    // 058 — `furx_tmux` aplica el socket dedicado `-L furx` (aísla del server tmux del usuario).
    let has = furx_tmux("tmux")
        .args(["has-session", "-t", &session])
        .output()
        .map_err(|e| format!("tmux not available: {}", e))?;
    if !has.status.success() {
        return Ok(String::new());
    }
    let limit = lines.unwrap_or(3000).clamp(100, 10000);
    let start = format!("-{}", limit);
    let out = furx_tmux("tmux")
        .args(["capture-pane", "-p", "-e", "-t", &session, "-S", &start, "-E", "-"])
        .output()
        .map_err(|e| format!("capture-pane failed: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// F / F24 — boot restore actions. attach is a no-op (each Terminal does
// tmux new-session -A on mount). ui restores layout from latest snapshot.
// full kills tmux server.
#[tauri::command]
pub fn boot_restore_attach(state: State<'_, AppState>) -> Result<(), String> {
    state
        .audit
        .write(EventInput {
            kind: "boot.restore.attach",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::Value::Null,
        })
        .ok();
    Ok(())
}

#[tauri::command]
pub fn boot_restore_full(state: State<'_, AppState>) -> Result<(), String> {
    // 058 (ultrareview fix) — `furx_tmux` → kill-server SOBRE EL SOCKET DEDICADO de Furx. Sin el `-L
    // furx`, `tmux kill-server` mataba el server tmux DEL USUARIO entero (todas sus sesiones de trabajo).
    let out = furx_tmux("tmux").arg("kill-server").output();
    state.audit.write(EventInput {
        kind: "boot.restore.full", actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"tmux_kill_ok": out.as_ref().map(|o| o.status.success()).unwrap_or(false)}),
    }).ok();
    Ok(())
}

#[derive(Serialize)]
pub struct RestoreUiPayload {
    pub schema_version: i64,
    pub panes: serde_json::Value,
    pub layout: serde_json::Value,
}

#[tauri::command]
pub fn boot_restore_ui(state: State<'_, AppState>) -> Result<Option<RestoreUiPayload>, String> {
    let conn = state.db.lock();
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT payload, schema_version FROM snapshots \
             WHERE kind IN ('manual','auto','startup') \
             ORDER BY at DESC LIMIT 1",
            [],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    drop(conn);
    state
        .audit
        .write(EventInput {
            kind: "boot.restore.ui",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"found": row.is_some()}),
        })
        .ok();
    let Some((payload_str, schema_version)) = row else {
        return Ok(None);
    };
    let v: serde_json::Value = serde_json::from_str(&payload_str).map_err(|e| e.to_string())?;
    Ok(Some(RestoreUiPayload {
        schema_version,
        panes: v.get("panes").cloned().unwrap_or(serde_json::Value::Null),
        layout: v.get("layout").cloned().unwrap_or(serde_json::Value::Null),
    }))
}

// 1.4 — Failed-command auto-explain.
#[tauri::command]
pub async fn explain_failed(
    state: State<'_, AppState>,
    cmd_hint: String,
    stderr_tail: String,
    exit_code: i32,
) -> Result<explain::ExplainResult, String> {
    let r = explain::explain(&cmd_hint, &stderr_tail, exit_code)
        .await
        .map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "explain.run", actor: "system",
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"exit_code": exit_code, "redacted": r.redacted, "elapsed_ms": r.elapsed_ms}),
    }).ok();
    Ok(r)
}

// 1.5 — @mention parse.
#[tauri::command]
pub fn mention_parse(input: String) -> Option<mention::MentionRoute> {
    mention::parse(&input)
}

// 1.6 — Auto-standup.
#[tauri::command]
pub async fn standup_today(state: State<'_, AppState>) -> Result<String, String> {
    use std::time::Duration;
    let (events_summary, cards) = {
        let conn = state.db.lock();
        let mut s1 = conn
            .prepare(
                "SELECT kind, COUNT(*) FROM events WHERE at >= datetime('now', '-24 hours') \
             GROUP BY kind ORDER BY COUNT(*) DESC LIMIT 20",
            )
            .map_err(|e| e.to_string())?;
        let events: Vec<(String, i64)> = s1
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|x| x.ok())
            .collect();
        let mut s2 = conn
            .prepare(
                "SELECT project, title, severity FROM cards WHERE status='open' \
             ORDER BY created_at DESC LIMIT 10",
            )
            .map_err(|e| e.to_string())?;
        let cards: Vec<(String, String, String)> = s2
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?
            .filter_map(|x| x.ok())
            .collect();
        (
            events
                .into_iter()
                .map(|(k, c)| format!("{}({})", k, c))
                .collect::<Vec<_>>()
                .join(", "),
            cards,
        )
    };
    let cards_str = cards
        .iter()
        .map(|(p, t, s)| format!("[{}/{}] {}", p, s, t))
        .collect::<Vec<_>>()
        .join("; ");
    let prompt = format!(
        "Generá un standup diario corto (español).\n\
        Eventos last 24h: {}\n\
        Cards open: {}\n\n\
        Formato:\n## Yesterday\n- bullets\n## Today\n- bullets\n## Blockers\n- bullets (puede ser 'ninguno')",
        events_summary, cards_str
    );
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or("missing aie-internal-bearer")?;
    let body = serde_json::json!({
        "model": "bulk_free", "max_tokens": 500, "temperature": 0.4,
        "messages": [{"role": "user", "content": prompt}]
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    // BLOQUE J: route through aie_endpoint helper instead of hard-coding the
    // the dev server Tailscale URL — respects Settings → endpoints.aie override.
    let aie_url = format!(
        "{}/v1/chat/completions",
        crate::services::aie_endpoint::resolve_url(&state.db)
    );
    let resp = client
        .post(&aie_url)
        .bearer_auth(bearer)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        // 039 — drop a stale bearer on 401 so the next call re-reads the rotated Keychain value.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(format!("AIE {}", status));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let md = v
        .pointer("/choices/0/message/content")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    state
        .audit
        .write(EventInput {
            kind: "standup.generated",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"chars": md.len()}),
        })
        .ok();
    Ok(md)
}

// 1.8 — Latency heatmap data + poll trigger.
#[tauri::command]
pub fn latency_heatmap(
    state: State<'_, AppState>,
    days: Option<u32>,
) -> Result<Vec<provider_latency::LatencyCell>, String> {
    provider_latency::query_heatmap(&state.db, days.unwrap_or(7)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn latency_poll_once(state: State<'_, AppState>) -> Result<usize, String> {
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or("missing aie-internal-bearer")?;
    // BLOQUE J: centralised endpoint resolution.
    let endpoint = crate::services::aie_endpoint::resolve_url(&state.db);
    provider_latency::poll_and_record(state.db.clone(), &endpoint, &bearer)
        .await
        .map_err(|e| e.to_string())
}

// 1.9 — Auto-PR description.
#[tauri::command]
pub async fn pr_description(
    state: State<'_, AppState>,
    repo_path: String,
    base: Option<String>,
) -> Result<pr_description::PrDescription, String> {
    let repo_path = expand_tilde(&repo_path);
    let base = base.unwrap_or_else(|| "master".into());
    let r = pr_description::generate(state.db.clone(), std::path::Path::new(&repo_path), &base)
        .await
        .map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "pr.description",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"branch": &r.branch, "commits": r.commits_count}),
        })
        .ok();
    Ok(r)
}

// 1.10 / W1 — disagreement analyzer.
#[tauri::command]
pub fn disagreement_analyze(
    responses: Vec<disagreement::LlmResponse>,
) -> disagreement::DisagreementReport {
    disagreement::analyze(&responses)
}

// VPN — Tailscale + WireGuard status + bring-up.
#[tauri::command]
pub async fn vpn_status() -> vpn::VpnStatus {
    vpn::status().await
}

#[tauri::command]
pub async fn vpn_up(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let r = vpn::bring_up(&name).await.map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "vpn.up",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"name": name}),
        })
        .ok();
    Ok(r)
}

// F6 — council on demand. Removido del handler+registry (053): obsoleto, reemplazado por
// council_run_multi (CouncilModal). Se conserva la fn como referencia.
#[allow(dead_code)]
#[tauri::command]
pub async fn council_run(
    state: State<'_, AppState>,
    query: String,
) -> Result<council_svc::CouncilRun, String> {
    let r = council_svc::run(&query).await.map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "council.run",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"query_chars": query.len(), "elapsed_ms": r.elapsed_ms}),
        })
        .ok();
    // 010-furx-signals — el council terminó; notificar.
    let _ = signals_svc::emit_signal(
        &state.db,
        &signals_svc::SignalEvent::new(
            "council.ready",
            "info",
            "Council listo",
            "El consejo terminó su deliberación.",
        ),
    );
    Ok(r)
}

// F23 — emit a card to the user's Telegram relay (outbound only).
#[tauri::command]
pub async fn telegram_emit_card(
    state: State<'_, AppState>,
    card_id: String,
) -> Result<telegram::TelegramSend, String> {
    let endpoint = {
        let conn = state.db.lock();
        settings_store::get(&conn, "endpoints.telegram_relay")
            .map_err(|e| e.to_string())?
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default()
    };
    if endpoint.is_empty() {
        return Err("endpoints.telegram_relay no configurado".into());
    }
    let secret = telegram::read_secret().ok_or("missing Keychain entry furx-telegram-hmac")?;
    let (title, severity) = {
        let conn = state.db.lock();
        conn.query_row(
            "SELECT title, severity FROM cards WHERE id = ?",
            params![card_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|e| e.to_string())?
    };
    let send = telegram::post_card(&endpoint, &secret, &card_id, &title, &severity)
        .await
        .map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "telegram.sent",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: Some(&card_id), correlation_id: None,
        payload: serde_json::json!({"endpoint": &send.endpoint, "status": send.status, "nonce": &send.nonce}),
    }).ok();
    Ok(send)
}

// F24 — tmux launchd watchdog control.
#[tauri::command]
pub fn tmux_watchdog_status() -> Result<tmux_watchdog::WatchdogStatus, String> {
    tmux_watchdog::status().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn tmux_watchdog_install(
    state: State<'_, AppState>,
) -> Result<tmux_watchdog::WatchdogStatus, String> {
    let s = tmux_watchdog::install().map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "watchdog.installed",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"plist_path": &s.plist_path}),
        })
        .ok();
    Ok(s)
}

#[tauri::command]
pub fn tmux_watchdog_uninstall(state: State<'_, AppState>) -> Result<(), String> {
    tmux_watchdog::uninstall().map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "watchdog.uninstalled",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::Value::Null,
        })
        .ok();
    Ok(())
}

#[tauri::command]
pub fn tmux_list_furx_sessions() -> Vec<tmux_watchdog::FurxSession> {
    tmux_watchdog::list_furx_sessions()
}

#[tauri::command]
pub fn search_run(query: String, cwd: Option<String>) -> Result<search::SearchResult, String> {
    let path = cwd.as_deref().map(std::path::Path::new);
    search::run(&query, path).map_err(|e| e.to_string())
}

#[tauri::command]
/// Wrapper sin args: exporta el estado a ~/Desktop/furx-state-<ts>.json.
/// El frontend lo invoca desde Settings sin que el user tenga que escribir un path.
pub fn export_state_to_desktop(state: State<'_, AppState>) -> Result<ExportReport, String> {
    let home = dirs::home_dir().ok_or("no home")?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // export_state genera un tar.zst (NO json) — extensión .furxexport, consistente con export.rs.
    let out_path = home.join("Desktop").join(format!("furx-state-{ts}.furxexport"));
    let report = export::export_state(&out_path).map_err(|e| e.to_string())?;
    state.audit.write(crate::bases::audit::EventInput {
        kind: "export.created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"path": &report.path, "sha256": &report.sha256, "size": report.size_bytes, "filtered": &report.filtered}),
    }).map_err(|e| e.to_string())?;
    Ok(report)
}

#[tauri::command]
pub fn export_state(state: State<'_, AppState>, out_path: String) -> Result<ExportReport, String> {
    // out_path debe estar bajo $HOME para no escribir en lugares raros.
    let p = std::path::Path::new(&out_path);
    let home = dirs::home_dir().ok_or("no home")?;
    let abs = p
        .canonicalize()
        .or_else(|_| {
            // El archivo aún no existe; chequeá el parent.
            p.parent()
                .ok_or_else(|| std::io::Error::other("no parent"))
                .and_then(|par| {
                    par.canonicalize()
                        .map(|d| d.join(p.file_name().unwrap_or_default()))
                })
        })
        .map_err(|e| e.to_string())?;
    if !abs.starts_with(&home) {
        return Err("out_path outside $HOME".into());
    }
    let report = export::export_state(&abs).map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "export.created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"path": &report.path, "sha256": &report.sha256, "size": report.size_bytes, "filtered": &report.filtered}),
    }).map_err(|e| e.to_string())?;
    Ok(report)
}

/// Resolve a (Furx) pane mode to (command, args, env). Si tmux está disponible,
/// envuelve cada CLI en `tmux new-session -A -s FURX_<modo>` para que la sesión
/// sobreviva al cierre de la app (próximo arranque la re-attachea).
fn resolve_mode(mode: &str) -> (String, Vec<String>, HashMap<String, String>) {
    let mut env = HashMap::new();
    let home = dirs::home_dir().unwrap_or_default();
    let bin = home.join("bin");
    // Si el binario no existe o falta config, abrimos zsh con un mensaje útil en vez de crash.
    // Los install-hints NO van envueltos en tmux (mensaje más legible directo).
    // B9.1 — universal CLI account modes. Format: "<cli>-<slug>" donde cli ∈
    // {claude, codex, gemini, aider} y slug es un account del usuario. Para los CLIs
    // sin account (sin slug) se acepta el legacy mode "codex"/"gemini"/"aider".
    let valid_slug = |s: &str| -> bool {
        !s.is_empty()
            && s.len() <= 32
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };

    let (raw_cmd, raw_args, use_tmux): (String, Vec<String>, bool) = match mode {
        "zsh" => ("/bin/zsh".into(), vec!["-l".into()], true),
        // claude-<slug> → wrapper ~/bin/claude-as-<slug>
        m if m.starts_with("claude-") => {
            let slug = &m[("claude-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}' (allowed: [A-Za-z0-9_-]{{1,32}})", slug),
                    "Usá A, B, work, personal, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("claude", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // codex-<slug> → wrapper ~/bin/codex-as-<slug> (env var OPENAI_API_KEY)
        m if m.starts_with("codex-") && m != "codex" => {
            let slug = &m[("codex-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}'", slug),
                    "Usá A, B, work, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("codex", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // gemini-<slug> → wrapper (env var GEMINI_API_KEY)
        m if m.starts_with("gemini-") && m != "gemini" => {
            let slug = &m[("gemini-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}'", slug),
                    "Usá A, B, work, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("gemini", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // aider-<slug>
        m if m.starts_with("aider-") && m != "aider" => {
            let slug = &m[("aider-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}'", slug),
                    "Usá A, B, work, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("aider", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // openai-api-<slug> (Codex MED fix B9.1: estaba ausente, dropdown lo ofrecía pero caía a zsh)
        m if m.starts_with("openai-api-") => {
            let slug = &m[("openai-api-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}'", slug),
                    "Usá A, B, work, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("openai-api", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // custom-<slug>
        m if m.starts_with("custom-") => {
            let slug = &m[("custom-".len())..];
            if !valid_slug(slug) {
                let (c, a) = install_hint_shell_pair(
                    &format!("Slug inválido '{}'", slug),
                    "Usá A, B, work, etc.",
                );
                (c, a, false)
            } else {
                let (c, a) = check_cli_account_setup("custom", slug, &bin);
                let is_hint = c.ends_with("zsh");
                (c, a, !is_hint)
            }
        }
        // Legacy modes (sin slug, usan default config del CLI o env existente)
        "codex" => {
            if !which_in_path("codex") {
                let (c, a) = install_hint_shell_pair(
                    "codex CLI no encontrado",
                    "Instalá con:\n  npm install -g @openai/codex\nVerificá login con: codex login",
                );
                (c, a, false)
            } else {
                ("codex".into(), vec![], true)
            }
        }
        "gemini" => {
            if !which_in_path("gemini") {
                let (c, a) = install_hint_shell_pair(
                    "gemini CLI no encontrado",
                    "Instalá con:\n  npm install -g @google/gemini-cli",
                );
                (c, a, false)
            } else {
                ("gemini".into(), vec![], true)
            }
        }
        "aider" => {
            if !which_in_path("aider") {
                let (c, a) = install_hint_shell_pair("aider no instalado", "Instalá con:\n  python3 -m pip install --user aider-chat\nO con uv:\n  uv tool install aider-chat");
                (c, a, false)
            } else {
                env.insert("AIDER_NO_AUTO_COMMITS".into(), "1".into());
                ("aider".into(), vec![], true)
            }
        }
        // 062 — Grok (xAI). Sin slug: usa su propio login (`grok login`/OAuth), como los demás legacy.
        // Sin este brazo caía al `_ => zsh` y seleccionar Grok abría una shell en silencio (audit codex).
        "grok" => {
            if !which_in_path("grok") {
                let (c, a) = install_hint_shell_pair(
                    "grok CLI no encontrado",
                    "Instalá Grok desde https://x.ai/grok\nLuego logueate con: grok login",
                );
                (c, a, false)
            } else {
                ("grok".into(), vec![], true)
            }
        }
        _ => ("/bin/zsh".into(), vec!["-l".into()], true),
    };
    if use_tmux {
        wrap_with_tmux_if_available(mode, raw_cmd, raw_args, env)
    } else {
        (raw_cmd, raw_args, env)
    }
}

/// Resolve an AgentProfile to (cmd, args, env). Sintetiza el `mode` string desde
/// (cli_kind, account_slug) y REUSA resolve_mode (cero duplicación de la lógica de
/// wrappers/tmux/Keychain). Encima inyecta model/system-prompt SÓLO para los CLIs con
/// flag conocido (claude/aider) y SÓLO si resolve_mode devolvió un CLI real — nunca
/// sobre un install-hint (que es un `/bin/zsh` desnudo). Los trailing args van al CLI
/// envuelto, esté o no bajo tmux (`tmux new-session … <cmd> <args>` ejecuta `<cmd> <args>`).
fn resolve_agent_runtime(
    agent: &crate::services::agent_profiles::AgentProfile,
    db: &std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>,
) -> Result<(String, Vec<String>, HashMap<String, String>), String> {
    // 009 — motor 'aie': el "CLI" del pane es un REPL Python (stdlib) que chatea por HTTP
    // contra el AIE. BYOK: el bearer sale del Keychain y va por env (aie_env), nunca al backend.
    if agent.engine_kind == "aie" {
        if !which_in_path("python3") {
            return Err("python3 no encontrado en PATH (requerido para el motor AIE)".to_string());
        }
        let script = crate::services::aie_repl::ensure_repl_script().map_err(|e| e.to_string())?;
        let env = crate::services::aie_repl::aie_env(agent, db);
        return Ok((
            "python3".to_string(),
            vec![script.to_string_lossy().to_string()],
            env,
        ));
    }
    if agent.engine_kind != "cli" {
        return Err(format!("engine_kind no soportado: {}", agent.engine_kind));
    }
    let mode =
        crate::services::agent_profiles::synth_mode(&agent.cli_kind, agent.account_slug.as_deref())
            .map_err(|e| e.to_string())?;
    let (cmd, mut args, env) = resolve_mode(&mode);
    let is_hint = cmd == "/bin/zsh"; // install-hint o agente zsh → no inyectar flags
    if !is_hint {
        match agent.cli_kind.as_str() {
            "claude" => {
                if let Some(m) = agent.model.as_deref().filter(|s| !s.is_empty()) {
                    args.push("--model".into());
                    args.push(m.to_string());
                }
                let sp = agent.system_prompt.trim();
                if !sp.is_empty() {
                    args.push("--append-system-prompt".into());
                    args.push(sp.to_string());
                }
            }
            "aider" => {
                if let Some(m) = agent.model.as_deref().filter(|s| !s.is_empty()) {
                    args.push("--model".into());
                    args.push(m.to_string());
                }
            }
            // codex/gemini/openai-api/custom/zsh: se guardan pero no se inyectan (sin
            // flag estable/conocido en v1 — inyectar uno equivocado rompería el spawn).
            _ => {}
        }
    }
    Ok((cmd, args, env))
}

/// 025 F1 — inyecta el bloque de lecciones procedurales aprobadas al `--append-system-prompt` del
/// perfil. Mutación in-place de `args`: localiza el arg `--append-system-prompt` (puesto por
/// `resolve_agent_runtime` para claude) y reemplaza su VALOR por `system_prompt + bloque` (concatena,
/// NUNCA reemplaza el system_prompt; FR-011). Si el perfil no tenía system_prompt (sin ese arg),
/// agrega `--append-system-prompt <bloque>`. No-op salvo claude + setting ON + lecciones activas.
/// Audit append-only con snapshot pre/post (council v2 §5 / FR-014).
fn inject_procedural_lessons(
    agent: &crate::services::agent_profiles::AgentProfile,
    safe_cwd: Option<&str>,
    db: &std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>,
    audit: &crate::bases::audit::AuditWriter,
    pane_id: &str,
    args: &mut Vec<String>,
) {
    use crate::services::procedural_gotchas as pg;
    // Solo claude tiene flag estable de system-prompt (council v2 §2).
    if agent.cli_kind != "claude" {
        // Registrar la omisión para otros CLIs (sin error; FR-013).
        if agent.cli_kind == "codex" || agent.cli_kind == "gemini" || agent.cli_kind == "aider" {
            let inject_on = {
                let conn = db.lock();
                crate::settings::get(&conn, "memory.procedural_inject")
                    .ok().flatten().and_then(|v| v.as_bool()).unwrap_or(false)
            };
            if inject_on {
                let _ = audit.write(EventInput {
                    kind: "memory.lesson.inject_skipped",
                    actor: "system:furx",
                    pane_id: Some(pane_id), card_id: None, correlation_id: None,
                    payload: serde_json::json!({
                        "cli_kind": agent.cli_kind,
                        "reason": "cli sin flag estable de system-prompt (system-append solo en claude v1)"
                    }),
                });
            }
        }
        return;
    }
    // Gate: setting ON + presupuesto de tokens.
    let (inject_on, budget) = {
        let conn = db.lock();
        let on = crate::settings::get(&conn, "memory.procedural_inject")
            .ok().flatten().and_then(|v| v.as_bool()).unwrap_or(false);
        let b = crate::settings::get(&conn, "memory.procedural_inject_max")
            .ok().flatten().and_then(|v| v.as_f64()).map(|n| n as usize)
            .filter(|n| *n >= 100)
            .unwrap_or(pg::DEFAULT_INJECT_TOKEN_BUDGET);
        (on, b)
    };
    if !inject_on {
        return;
    }
    let Some(cwd) = safe_cwd else { return; };
    let project_key = resolve_project_key_for_cwd(db, cwd);
    let active = match pg::list_active_lessons(db, &project_key) {
        Ok(a) => a,
        Err(_) => return,
    };
    let Some(block) = pg::build_lessons_block(&active, budget) else {
        return; // 0 lecciones activas -> sin addendum (spawn idéntico al actual).
    };
    // Ids de las lecciones que entraron al bloque (para el audit).
    let injected_ids: Vec<String> = active
        .iter()
        .filter(|l| l.active && block.contains(&l.content))
        .map(|l| l.entry_id.clone())
        .collect();

    // Localizar el --append-system-prompt del perfil y CONCATENAR (no reemplazar) el bloque.
    let pre = agent.system_prompt.trim().to_string();
    let post = pg::append_lessons_to_prompt(&pre, Some(&block));
    if let Some(i) = args.iter().position(|x| x == "--append-system-prompt") {
        if i + 1 < args.len() {
            args[i + 1] = post.clone();
        } else {
            args.push(post.clone());
        }
    } else {
        args.push("--append-system-prompt".into());
        args.push(post.clone());
    }
    // Audit append-only con snapshot pre/post (council v2 §5).
    let _ = audit.write(EventInput {
        kind: "memory.lesson.injected",
        actor: "system:furx",
        pane_id: Some(pane_id), card_id: None, correlation_id: None,
        payload: serde_json::json!({
            "profile": agent.id,
            "cli_kind": agent.cli_kind,
            "project_key": project_key,
            "lesson_ids": injected_ids,
            "rationale": "inyectado por política memory.procedural_inject",
            "system_prompt_pre": pre,
            "system_prompt_post": post,
        }),
    });
}

/// spec-011 — resolve the project_key for a cwd: the longest `projects.path`
/// (canonicalized) that prefixes the cwd; falls back to the cwd itself if no project
/// row matches (so the store is still per-repo). Mirrors memory_daemon::resolve_project_key
/// (007) but reads through the app DB and never fails the spawn.
fn resolve_project_key_for_cwd(
    db: &std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>,
    cwd: &str,
) -> String {
    let canon = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| cwd.to_string());
    let paths: Vec<String> = {
        let conn = db.lock();
        conn.prepare("SELECT path FROM projects")
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, String>(0))
                    .map(|rows| rows.filter_map(|x| x.ok()).collect::<Vec<_>>())
            })
            .unwrap_or_default()
    };
    paths
        .into_iter()
        .filter(|p| {
            let pc = std::fs::canonicalize(p)
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.clone());
            let pc = pc.trim_end_matches('/');
            canon == pc || canon.starts_with(&format!("{}/", pc))
        })
        .max_by_key(|p| p.len())
        .unwrap_or(canon)
}

/// spec-011 (FR-003) — given an agent and its resolved cwd, build the MCP servers from
/// the plugins in the agent's allow-list (default-deny + signature-verified), write a
/// per-agent `.mcp.json`, and return CLI args to point the agent at it + a list of
/// (plugin, index_command) to index in the background (FR-004).
///
/// Returns `(extra_args, index_jobs)`. `extra_args` is empty for CLIs we don't (yet)
/// know how to point at a custom MCP config (only `claude` supports `--mcp-config` in
/// v1); the config file is still written + auditable. `index_jobs` are plugins that
/// declare an `index_command` (we enqueue a background pass per project).
fn build_agent_mcp_injection(
    agent: &crate::services::agent_profiles::AgentProfile,
    safe_cwd: Option<&str>,
    db: &std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>,
) -> (Vec<String>, Vec<(String, String)>) {
    use crate::services::mcp_inject;
    // No cwd → can't scope the project; nothing to inject (still default-deny safe).
    let Some(cwd) = safe_cwd else {
        return (vec![], vec![]);
    };
    if agent.plugins.is_empty() {
        // Default-deny: write an EMPTY config so the agent sees no injected servers
        // (verifiable SC-003) and add no args.
        let _ = mcp_inject::write_agent_mcp_config(&agent.id, &[]);
        return (vec![], vec![]);
    }
    // spec-022 US1 — el estado DISABLED global gana sobre el allow-list del perfil. Un
    // plugin desactivado por el usuario NO se inyecta como MCP server NI se indexa,
    // aunque esté en el allow-list del agente. Filtramos ACÁ (única ruta con acceso a DB)
    // antes de resolver servers o enqueue de index jobs.
    let allow_enabled: Vec<String> = agent
        .plugins
        .iter()
        .filter(|name| {
            crate::services::plugins::is_enabled(db, name).unwrap_or(false)
        })
        .cloned()
        .collect();
    if allow_enabled.is_empty() {
        // Todos los plugins del allow-list están disabled → config vacía (default-deny).
        let _ = mcp_inject::write_agent_mcp_config(&agent.id, &[]);
        return (vec![], vec![]);
    }
    let furx_data = dirs::home_dir()
        .map(|h| h.join(".furx").to_string_lossy().into_owned())
        .unwrap_or_default();
    let project_key = resolve_project_key_for_cwd(db, cwd);
    let plugins_base = dirs::home_dir().map(|h| h.join(".furx").join("plugins"));
    let Some(plugins_base) = plugins_base else {
        return (vec![], vec![]);
    };

    // Gate by the (enabled) allow-list + signature verification + ENTRYPOINT HASH binding
    // (fail-closed: the injected command is resolved to the signed entrypoint inside the
    // installed plugin dir and its on-disk bytes are verified against entrypoint_sha256).
    let servers = mcp_inject::servers_for_allowlist_verified(
        &plugins_base,
        &allow_enabled,
        cwd,
        &project_key,
        &furx_data,
    );

    // Which of the injected (enabled) plugins declare a background index_command (FR-004)?
    let mut index_jobs = Vec::new();
    for name in &allow_enabled {
        if let Some(m) = mcp_inject::load_verified_manifest(&plugins_base, name) {
            if m.mcp
                .as_ref()
                .and_then(|s| s.index_command.as_ref())
                .is_some()
            {
                index_jobs.push((m.name.clone(), project_key.clone()));
            }
        }
    }

    // Write the per-agent MCP config (always — even empty — so it's deterministic).
    let cfg_path = match mcp_inject::write_agent_mcp_config(&agent.id, &servers) {
        Ok(p) => p,
        Err(_) => return (vec![], index_jobs), // write failed → don't break the spawn
    };

    // Only `claude` knows how to take a custom MCP config file on the CLI in v1
    // (`--mcp-config <file>`). Other CLIs: config is written + audited, no flag yet.
    let extra_args = if !servers.is_empty() && agent.cli_kind == "claude" {
        vec![
            "--mcp-config".to_string(),
            cfg_path.to_string_lossy().into_owned(),
        ]
    } else {
        vec![]
    };
    (extra_args, index_jobs)
}

/// Universal CLI account checker. Mapea (cli_kind, slug) → wrapper `~/bin/<cli>-as-<slug>`
/// + verifica Keychain entry `<prefix><slug>` con env_var apropiado.
fn check_cli_account_setup(
    cli_kind: &str,
    slug: &str,
    bin: &std::path::Path,
) -> (String, Vec<String>) {
    let wrapper = bin.join(format!("{}-as-{}", cli_kind, slug));
    if !wrapper.exists() {
        // Codex LOW fix B9.1: hint script depende del kind (claude usa setup-max-account.sh).
        let setup_cmd = if cli_kind == "claude" {
            format!("~/bin/setup-max-account.sh {}", slug)
        } else {
            format!("~/bin/setup-account.sh {} --cli {}", slug, cli_kind)
        };
        return install_hint_shell_pair(
            &format!("~/bin/{}-as-{} no encontrado", cli_kind, slug),
            &format!(
                "Esperaba {}. Crear wrapper + entrada Keychain con:\n  {}",
                wrapper.display(),
                setup_cmd
            ),
        );
    }
    // Service prefix depende del kind (espejo de claude_accounts::CliKind::default_service_prefix)
    let svc_prefix = match cli_kind {
        "claude" => "claude-max-",
        "codex" => "codex-cli-",
        "gemini" => "gemini-cli-",
        "aider" => "aider-",
        "openai-api" => "openai-api-",
        _ => "custom-",
    };
    let service = format!("{}{}", svc_prefix, slug);
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    if crate::services::keychain::load(&service, &user).is_none() {
        let setup_cmd = if cli_kind == "claude" {
            format!("~/bin/setup-max-account.sh {}", slug)
        } else {
            format!("~/bin/setup-account.sh {} --cli {}", slug, cli_kind)
        };
        return install_hint_shell_pair(
            &format!("Sin Keychain entry `{}`", service),
            &format!(
                "Crear la entrada con el token de tu cuenta {} {}:\n\n  {}",
                cli_kind, slug, setup_cmd
            ),
        );
    }
    (wrapper.to_string_lossy().to_string(), vec![])
}

/// Legacy wrapper para back-compat (algunos call sites pueden referenciarlo).
#[allow(dead_code)]
fn check_claude_setup(slug: &str, bin: &std::path::Path) -> (String, Vec<String>) {
    let wrapper = bin.join(format!("claude-as-{}", slug));
    if !wrapper.exists() {
        return install_hint_shell_pair(
            &format!("~/bin/claude-as-{} no encontrado", slug),
            &format!("Esperaba {}. Crear wrapper + entrada Keychain con:\n  ~/bin/setup-max-account.sh {}", wrapper.display(), slug),
        );
    }
    let service = format!("claude-max-{}", slug);
    // MED fix (Gemini audit B9): use the `keyring` crate (native Security API) instead of
    // shelling out to /usr/bin/security — avoids argv leaks via `ps -ef` AND keeps Keychain
    // access path consistent with services/keychain.rs::load.
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    let keychain_ok = crate::services::keychain::load(&service, &user).is_some();
    if !keychain_ok {
        return install_hint_shell_pair(
            &format!("⚠ Sin Keychain entry `{}`", service),
            // LOW fix (Gemini): interpolate actual $USER into the hint example.
            &format!("Crear la entrada con el OAuth token de tu cuenta Max {}:\n\n  ~/bin/setup-max-account.sh {}\n\nO manualmente:\n  security add-generic-password -a {} -s {} -w '<TOKEN>'\n\nObtené el token desde Claude CLI haciendo login con `claude` (queda en ~/.claude/.credentials.json campo accessToken).", slug, slug, user, service),
        );
    }
    (wrapper.to_string_lossy().to_string(), vec![])
}

fn install_hint_shell_pair(title: &str, body: &str) -> (String, Vec<String>) {
    let msg = format!(
        "clear; printf '\\n\\033[33m{}\\033[0m\\n\\n'; printf '%s\\n' '{}'; printf '\\n'; exec /bin/zsh -l",
        title.replace('\'', "'\\''"),
        body.replace('\'', "'\\''").replace('\n', "'\\n'")
    );
    ("/bin/zsh".into(), vec!["-c".into(), msg])
}

fn which_in_path(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if std::path::Path::new(dir).join(cmd).exists() {
                return true;
            }
        }
    }
    std::process::Command::new("/usr/bin/which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn wrap_with_tmux_if_available(
    mode: &str,
    cmd: String,
    args: Vec<String>,
    env: HashMap<String, String>,
) -> (String, Vec<String>, HashMap<String, String>) {
    let tmux = std::process::Command::new("/usr/bin/which")
        .arg("tmux")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(tmux_path) = tmux else {
        return (cmd, args, env);
    };

    // 056 scroll-fix — habilitar WHEEL-SCROLLBACK + scrollbar nativa a la derecha en las sesiones de
    // Furx (bug: no se podía scrollear el texto previo del diálogo, sólo el input). Best-effort,
    // detached, con RETRY (no un sleep fijo arbitrario — audit BLOCKER). Ver `spawn_furx_scroll_opts`.
    // 058 (ultrareview fix) — `set -g` ahora corre sobre el SOCKET DEDICADO `-L furx`: ya NO toca el
    // `mouse`/scrollbars de las sesiones tmux AJENAS del usuario (antes era server-global compartido).
    spawn_furx_scroll_opts(&tmux_path);

    // Build: tmux -L furx new-session -A -s FURX_<mode> <cmd> <args...>
    // 058 — socket dedicado: aísla por completo el tmux de Furx del server por defecto del usuario.
    let session = furx_session_name(mode); // 056 — sanitización única (audit BLOCKER)
    let mut new_args = vec![
        "-L".into(),
        FURX_TMUX_SOCKET.into(),
        "new-session".into(),
        "-A".into(), // attach if exists, create if not (persists across launches)
        "-s".into(),
        session,
    ];
    if !cmd.is_empty() {
        new_args.push(cmd);
        for a in args {
            new_args.push(a);
        }
    }
    (tmux_path, new_args, env)
}

/// 056 — nombre canónico de la sesión tmux de Furx. UN SOLO lugar de sanitización (audit BLOCKER): el
/// spawn (override de `-s`) y `pty_capture_history` DEBEN producir EXACTAMENTE el mismo nombre, sino el
/// resume no encuentra la sesión. Reemplaza todo char que no sea `[A-Za-z0-9_]` por `_`.
pub fn furx_session_name(raw: &str) -> String {
    let safe: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    format!("FURX_{}", safe)
}

/// 056 — aplica las opciones de scroll (mouse/copy-mode + scrollbar nativa) al server tmux de Furx.
/// Best-effort + DETACHED (no puede romper el spawn) + RETRY (sin sleep fijo arbitrario — audit
/// BLOCKER): reintenta hasta que el server tmux responda (lo levanta el `new-session` async) o ~3s.
/// Cada `set-option` es una invocación SEPARADA (un fallo de `pane-scrollbars` en tmux <3.5 NO impide
/// que `mouse on`/`history-limit` se apliquen — audit). Idempotente.
fn spawn_furx_scroll_opts(tmux_path: &str) {
    let tmux = tmux_path.to_string();
    std::thread::spawn(move || {
        // Opciones por separado: las seguras en toda versión primero; pane-scrollbars (3.5+) al final.
        let opts: [&[&str]; 4] = [
            &["mouse", "on"],
            &["history-limit", "50000"],
            &["pane-scrollbars", "on"],
            &["pane-scrollbars-position", "right"],
        ];
        for _ in 0..15 {
            // ¿el server tmux está arriba? `list-sessions` falla (status != 0) si no hay server aún.
            // 058 — `furx_tmux` aplica `-L furx`: chequea/configura el SOCKET DEDICADO (no el del usuario).
            let up = furx_tmux(&tmux)
                .arg("list-sessions")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if up {
                for o in opts {
                    // `set -g` es seguro: corre sobre el server `-L furx`, aislado del del usuario.
                    let mut cmd = furx_tmux(&tmux);
                    cmd.args(["set-option", "-g"]).args(o);
                    let _ = cmd.output();
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
}

// ════════════════════════════════════════════════════════════════════
// B2 + B4 commands (sprint plan_furx_master)
// ════════════════════════════════════════════════════════════════════

// 2.8 bg_queue
#[tauri::command]
pub fn bg_enqueue(
    state: State<'_, AppState>,
    kind: String,
    args: serde_json::Value,
) -> Result<String, String> {
    bg_queue::enqueue(&state.db, &kind, args).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn bg_list(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<bg_queue::BgJob>, String> {
    bg_queue::list(&state.db, limit.unwrap_or(50)).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn bg_cancel(state: State<'_, AppState>, id: String) -> Result<(), String> {
    bg_queue::cancel(&state.db, &id).map_err(|e| e.to_string())
}

// 2.1 embeddings
#[tauri::command]
pub async fn embeddings_index(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<usize, String> {
    let project_path = expand_tilde(&project_path);
    embeddings::index_project(state.db.clone(), std::path::Path::new(&project_path))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn embeddings_search(
    state: State<'_, AppState>,
    project_path: String,
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<embeddings::SearchHit>, String> {
    let project_path = expand_tilde(&project_path);
    embeddings::search(
        state.db.clone(),
        std::path::Path::new(&project_path),
        &query,
        top_k.unwrap_or(10),
    )
    .await
    .map_err(|e| e.to_string())
}

// 2.2 diff-aware review
#[tauri::command]
pub async fn diff_review_run(
    state: State<'_, AppState>,
    file_path: String,
) -> Result<diff_review::ReviewResult, String> {
    let r = diff_review::review(std::path::Path::new(&file_path))
        .await
        .map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "diff_review.run", actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"file": &r.file_path, "diff_lines": r.diff_lines, "comments": r.comments.len()}),
    }).ok();
    Ok(r)
}

// 2.3 agent memory
#[tauri::command]
pub async fn agent_memory_recall(project: String) -> Result<agent_memory::ProjectMemory, String> {
    agent_memory::recall(&project)
        .await
        .map_err(|e| e.to_string())
}

// Spec 066 — Furx Memory (corpus-engine). Devuelven el envelope CorpusResult (nunca Err: la UI
// distingue available/error_code). Infallible a nivel IPC.
#[tauri::command]
pub async fn corpus_status() -> corpus_memory::CorpusResult<corpus_memory::CorpusStatus> {
    corpus_memory::status().await
}

#[tauri::command]
pub async fn corpus_search(
    query: String,
    project: Option<String>,
    human_only: Option<bool>,
    limit: Option<u32>,
) -> corpus_memory::CorpusResult<corpus_memory::SearchResults> {
    corpus_memory::search(&query, project.as_deref(), human_only.unwrap_or(false), limit).await
}

#[tauri::command]
pub async fn corpus_deadends(top: Option<u32>) -> corpus_memory::CorpusResult<corpus_memory::Deadends> {
    corpus_memory::deadends(top).await
}

#[tauri::command]
pub async fn corpus_ledger(
    project: Option<String>,
    kind: Option<String>,
) -> corpus_memory::CorpusResult<corpus_memory::Ledger> {
    corpus_memory::ledger(project.as_deref(), kind.as_deref()).await
}

// 2.4 DAG
#[tauri::command]
pub fn dag_parse(repo_path: String) -> Result<Vec<dag::Dag>, String> {
    dag::parse_repo(std::path::Path::new(&repo_path)).map_err(|e| e.to_string())
}

// 2.5 diff blocks
#[tauri::command]
pub fn diff_detect_blocks(buffer: String) -> Vec<diff_preview::DiffBlock> {
    diff_preview::detect_blocks(&buffer)
}

// 2.6 eval runner
#[tauri::command]
pub fn eval_list_tasks() -> Result<Vec<eval_runner::EvalTask>, String> {
    eval_runner::list_tasks().map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn eval_run_task(
    state: State<'_, AppState>,
    task: eval_runner::EvalTask,
) -> Result<eval_runner::EvalRun, String> {
    let r = eval_runner::run(&task).await.map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "eval.run", actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"task": &r.task, "status": &r.status, "elapsed_ms": r.elapsed_ms}),
    }).ok();
    Ok(r)
}

// 2.7 replay scrub
#[tauri::command]
pub fn replay_buckets(
    state: State<'_, AppState>,
    hours: Option<u32>,
) -> Result<replay_scrub::ScrubData, String> {
    replay_scrub::buckets(&state.db, hours.unwrap_or(72)).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn replay_events_at(
    state: State<'_, AppState>,
    bucket_ts: String,
) -> Result<Vec<serde_json::Value>, String> {
    replay_scrub::events_at(&state.db, &bucket_ts).map_err(|e| e.to_string())
}

// 2.9 replay bundle
#[tauri::command]
pub fn replay_bundle_create(
    state: State<'_, AppState>,
    project_dir: Option<String>,
    span_start: String,
    span_end: String,
    out_path: String,
) -> Result<replay::ReplayBundleReport, String> {
    let home = dirs::home_dir().ok_or("no home")?;
    let p = std::path::PathBuf::from(&out_path);
    let abs = p
        .canonicalize()
        .or_else(|_| {
            p.parent()
                .ok_or_else(|| std::io::Error::other("no parent"))
                .and_then(|par| {
                    par.canonicalize()
                        .map(|d| d.join(p.file_name().unwrap_or_default()))
                })
        })
        .map_err(|e| e.to_string())?;
    if !abs.starts_with(&home) {
        return Err("out_path must be under $HOME".into());
    }
    let r = replay::bundle(
        state.db.clone(),
        project_dir.as_deref().map(std::path::Path::new),
        &span_start,
        &span_end,
        &abs,
    )
    .map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "replay.bundled", actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"path": &r.path, "bytes": r.size_bytes, "events": r.events_count, "redacted": r.redacted}),
    }).ok();
    Ok(r)
}

// 2.10 router viz
#[tauri::command]
pub async fn router_snapshot(
    state: State<'_, AppState>,
) -> Result<router_viz::CascadeSnapshot, String> {
    let bearer =
        crate::services::keychain_bearer::get_bearer().ok_or("missing aie-internal-bearer")?;
    // BLOQUE J: route through aie_endpoint helper (was hard-coded the dev server URL).
    let url = crate::services::aie_endpoint::resolve_url(&state.db);
    router_viz::fetch(&url, &bearer)
        .await
        .map_err(|e| e.to_string())
}

// 2.11 yesterday bootstrap
#[tauri::command]
pub async fn yesterday_compile(
    state: State<'_, AppState>,
    pane_id: String,
) -> Result<String, String> {
    let p = yesterday::compile(state.db.clone(), &pane_id)
        .await
        .map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "yesterday.compiled",
            actor: "system",
            pane_id: Some(&pane_id),
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"path": p.to_string_lossy()}),
        })
        .ok();
    Ok(p.to_string_lossy().to_string())
}

// B4
#[tauri::command]
pub fn snippets_save(
    state: State<'_, AppState>,
    title: String,
    body: String,
    tags: String,
) -> Result<String, String> {
    snippets::save(&state.db, &title, &body, &tags).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn snippets_list(
    state: State<'_, AppState>,
    q: Option<String>,
) -> Result<Vec<snippets::Snippet>, String> {
    snippets::list(&state.db, q.as_deref()).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn snippets_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    snippets::delete(&state.db, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn http_send(
    state: State<'_, AppState>,
    req: http_client::HttpRequest,
) -> Result<http_client::HttpResponse, String> {
    http_client::send(&state.db, req)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn time_weekly(state: State<'_, AppState>) -> Result<Vec<time_tracking::PaneTime>, String> {
    time_tracking::weekly(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gh_list_prs(repo_path: String) -> Result<Vec<gh_panel::GhItem>, String> {
    gh_panel::list_prs(std::path::Path::new(&repo_path))
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn gh_list_issues(repo_path: String) -> Result<Vec<gh_panel::GhItem>, String> {
    gh_panel::list_issues(std::path::Path::new(&repo_path))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quick_notes_add(state: State<'_, AppState>, body: String) -> Result<String, String> {
    quick_notes::add(&state.db, &body).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn quick_notes_list(state: State<'_, AppState>) -> Result<Vec<quick_notes::QuickNote>, String> {
    quick_notes::list(&state.db).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn quick_notes_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    quick_notes::delete(&state.db, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn theme_set(
    state: State<'_, AppState>,
    project: String,
    accent_hex: String,
    label: Option<String>,
) -> Result<(), String> {
    themes::set(&state.db, &project, &accent_hex, label.as_deref()).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn theme_list(state: State<'_, AppState>) -> Result<Vec<themes::ProjectTheme>, String> {
    themes::list(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pane_template_save(
    state: State<'_, AppState>,
    template: pane_templates::PaneTemplate,
) -> Result<(), String> {
    pane_templates::save(&state.db, &template).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn pane_template_list(
    state: State<'_, AppState>,
) -> Result<Vec<pane_templates::PaneTemplate>, String> {
    pane_templates::list(&state.db).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn pane_template_delete(state: State<'_, AppState>, name: String) -> Result<(), String> {
    pane_templates::delete(&state.db, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bisect_run(
    state: State<'_, AppState>,
    repo_path: String,
    good: String,
    bad: String,
    test_cmd: String,
) -> Result<bisect::BisectResult, String> {
    let repo_path = expand_tilde(&repo_path);
    bisect::run(
        &state.db,
        std::path::Path::new(&repo_path),
        &good,
        &bad,
        &test_cmd,
    )
    .map_err(|e| e.to_string())
}

// spec-kit 001 · US1 — TTS "Read aloud" commands (local OS engine, no network).
use crate::services::tts;

/// Read `text` aloud for `pane_id`. When `summarize` is true (the default for
/// auto-read-on-completion), the block is reduced to prose ≤ ~600 chars (no code/
/// logs). When `preempt`, a currently-speaking pane is cancelled; otherwise the
/// request is dropped if another pane is speaking (single-speaker rule).
#[tauri::command]
pub async fn tts_speak(
    state: State<'_, AppState>,
    pane_id: String,
    text: String,
    summarize: bool,
    preempt: bool,
) -> Result<bool, String> {
    // Always redact secrets before speaking — never read a key/token aloud, even
    // when summarize=false (codex+gemini HIGH: redaction must not be summary-gated).
    let payload = if summarize {
        tts::summarize(&text, 600)
    } else {
        tts::redact_secrets(&text)
    };
    let when = if preempt {
        tts::WhenBusy::Preempt
    } else {
        tts::WhenBusy::Drop
    };
    let started = tts::speak(&pane_id, &payload, when)
        .await
        .map_err(|e| e.to_string())?;
    if started {
        state.audit.write(EventInput {
            kind: "tts.speak",
            actor: &crate::services::identity::current_actor(), pane_id: Some(pane_id.as_str()), card_id: None, correlation_id: None,
            payload: serde_json::json!({"chars": payload.chars().count(), "summarized": summarize}),
        }).ok();
    }
    Ok(started)
}

/// Stop any current speech immediately (global Stop, or STT voice-interrupt).
#[tauri::command]
pub fn tts_stop() {
    tts::stop();
}

/// Whether a local OS speech engine is available (UI hides the toggle if not).
#[tauri::command]
pub fn tts_available() -> bool {
    tts::available()
}

/// The pane currently speaking, if any (for the UI indicator).
#[tauri::command]
pub fn tts_speaking_pane() -> Option<String> {
    tts::speaking_pane()
}

// spec-kit 001 · US2 — Plugin Host (MCP) commands.
use crate::services::plugin_host;

/// Verify a signed manifest's Ed25519 signature. Install gate: the UI calls this
/// before enabling a plugin; an invalid/absent signature must block install.
#[tauri::command]
pub fn plugin_verify(manifest: plugin_host::SignedManifest) -> bool {
    manifest.verify()
}

/// Invoke a plugin tool. Loads `~/.furx/plugins/<name>/manifest.json`, REJECTS if
/// the signature is invalid (FR-014), then runs the tool out-of-process honoring
/// the permission set (net-deny fail-closed, BYOK secret gate). v1 grants NO
/// secrets (default-deny); ask-on-first-use secret grants land in Fase 2.
#[tauri::command]
pub async fn plugin_invoke(
    state: State<'_, AppState>,
    name: String,
    tool: String,
    args_json: String,
) -> Result<plugin_host::ToolResult, String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let home = dirs::home_dir().ok_or("no home")?;
    let dir = home.join(".furx").join("plugins").join(&name);
    let manifest_path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| format!("manifest: {e}"))?;
    let manifest: plugin_host::SignedManifest =
        serde_json::from_str(&text).map_err(|e| format!("manifest parse: {e}"))?;
    if !manifest.verify() {
        state
            .audit
            .write(EventInput {
                kind: "plugin.invoke.rejected",
                actor: &crate::services::identity::current_actor(),
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"name": &name, "reason": "invalid signature"}),
            })
            .ok();
        return Err("plugin signature invalid — refusing to run".into());
    }
    // Integrity: the manifest's own name MUST equal the requested dir name (gemini
    // HIGH: prevents a manifest declaring a granted plugin's identity from a
    // different dir). And require entrypoint_sha256 to bind the binary (codex HIGH).
    if manifest.name != name {
        return Err("manifest name does not match plugin dir — refusing".into());
    }
    if manifest.entrypoint_sha256.is_none() {
        return Err("manifest missing entrypoint_sha256 — refusing to run unbound binary".into());
    }
    // spec-022 US1 — respetar el estado enable/disable (disco SoT). Un plugin
    // desactivado por el usuario NO se ejecuta.
    if !plugins_svc::is_enabled(&state.db, &name).map_err(|e| e.to_string())? {
        return Err("plugin disabled — habilitalo en Plugins para ejecutarlo".into());
    }
    // Ask-on-first-use: require user consent for this exact version (default-deny).
    if !plugin_host::is_granted(&manifest.name, &manifest.version) {
        return Err(format!(
            "NEEDS_GRANT:{}:{}",
            manifest.name, manifest.version
        ));
    }
    // spec-003 — inject ONLY the secrets the manifest declares AND the user granted.
    // Values read from the OS Keychain here; never persisted or logged (names only).
    let (secrets, missing) = plugin_host::resolve_granted_secrets(
        &manifest.name,
        &manifest.permissions.secrets,
        crate::services::keychain::load,
    );
    if !missing.is_empty() {
        state
            .audit
            .write(EventInput {
                kind: "plugin.secret.missing",
                actor: &crate::services::identity::current_actor(),
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"name": &name, "missing": &missing}),
            })
            .ok();
    }
    let res = plugin_host::run_tool(
        &dir,
        &manifest.entrypoint,
        manifest.entrypoint_sha256.as_deref(),
        &tool,
        &args_json,
        &manifest.permissions,
        &secrets,
        30_000,
    )
    .await
    .map_err(|e| e.to_string())?;
    state.audit.write(EventInput {
        kind: "plugin.invoke",
        actor: &crate::services::identity::current_actor(), pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"name": &name, "tool": &tool, "exit_ok": res.exit_ok, "net_sandboxed": res.sandboxed_net_deny}),
    }).ok();
    Ok(res)
}

fn plugins_svc_is_safe(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Read a plugin's manifest (signature-verified) so the UI can show the exact
/// permissions before asking for consent. Returns the parsed SignedManifest.
#[tauri::command]
pub fn plugin_manifest(name: String) -> Result<plugin_host::SignedManifest, String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let home = dirs::home_dir().ok_or("no home")?;
    let text = std::fs::read_to_string(
        home.join(".furx")
            .join("plugins")
            .join(&name)
            .join("manifest.json"),
    )
    .map_err(|e| format!("manifest: {e}"))?;
    let m: plugin_host::SignedManifest =
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    if !m.verify() {
        return Err("signature invalid".into());
    }
    Ok(m)
}

/// Record user consent (ask-on-first-use). Codex audit: the version is NOT taken
/// from the caller — we load + verify the on-disk manifest and record exactly its
/// `manifest.version`, so a frontend caller can't pre-grant an arbitrary version
/// and bypass the prompt for the real (newer) code.
#[tauri::command]
pub fn plugin_grant(state: State<'_, AppState>, name: String) -> Result<String, String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let home = dirs::home_dir().ok_or("no home")?;
    let text = std::fs::read_to_string(
        home.join(".furx")
            .join("plugins")
            .join(&name)
            .join("manifest.json"),
    )
    .map_err(|e| format!("manifest: {e}"))?;
    let m: plugin_host::SignedManifest =
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    if !m.verify() {
        return Err("signature invalid — cannot grant".into());
    }
    if m.name != name {
        return Err("manifest name mismatch".into());
    }
    // Auto-harden the plugin dir read-only on consent (codex/gemini MED: closes the
    // TOCTOU window in the real flow instead of relying on a manual harden call).
    let dir = home.join(".furx").join("plugins").join(&name);
    let _ = plugin_host::harden_readonly(&dir); // best-effort (already-readonly is fine)
    plugin_host::grant(&m.name, &m.version).map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "plugin.grant",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"name": &m.name, "version": &m.version}),
        })
        .ok();
    Ok(m.version)
}

/// Revoke consent (kill switch).
#[tauri::command]
pub fn plugin_revoke(state: State<'_, AppState>, name: String) -> Result<(), String> {
    plugin_host::revoke(&name).map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "plugin.revoke",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"name": &name}),
        })
        .ok();
    Ok(())
}

/// Whether the user has consented to a plugin at a version.
#[tauri::command]
pub fn plugin_is_granted(name: String, version: String) -> bool {
    plugin_host::is_granted(&name, &version)
}

/// spec-002 — install a plugin from the shipped bundle: copy the bundled dir into
/// ~/.furx/plugins/<name>, verify its signature against the pinned key, harden it
/// read-only. Fail-closed (invalid signature → nothing installed). Returns version.
#[tauri::command]
pub fn plugin_install_bundled(app: AppHandle, name: String) -> Result<String, String> {
    use tauri::Manager;
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let res_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let src = res_dir.join("plugins-bundle").join(&name);
    if !src.join("manifest.json").is_file() {
        return Err(format!("bundled plugin '{name}' not found in resources"));
    }
    plugin_host::install_bundled(&src, &name).map_err(|e| e.to_string())
}

/// spec-013 (T041) — list the shipped, signed bundle plugins as a marketplace catalog
/// with tier + category metadata (spec-002 installer). Reads the manifests from the
/// app's resource bundle; each entry reports whether it currently verifies against the
/// pinned key (fail-closed: a tampered entry shows verified=false). Read-only — does
/// not install anything.
#[tauri::command]
pub fn plugin_list_bundled(
    app: AppHandle,
) -> Result<Vec<crate::services::mcp_inject::BundlePluginInfo>, String> {
    use tauri::Manager;
    let res_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
    let bundle = res_dir.join("plugins-bundle");
    Ok(crate::services::mcp_inject::bundle_catalog(&bundle))
}

/// spec-003 — grant a plugin access to a named secret, backed by a Keychain entry.
/// Rejected unless the plugin's (signed) manifest DECLARES that secret name. The
/// value is never sent here — only the Keychain reference (service+account).
#[tauri::command]
pub fn plugin_grant_secret(
    state: State<'_, AppState>,
    name: String,
    secret_name: String,
    kc_service: String,
    kc_account: String,
) -> Result<(), String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let home = dirs::home_dir().ok_or("no home")?;
    let text = std::fs::read_to_string(
        home.join(".furx")
            .join("plugins")
            .join(&name)
            .join("manifest.json"),
    )
    .map_err(|e| format!("manifest: {e}"))?;
    let m: plugin_host::SignedManifest =
        serde_json::from_str(&text).map_err(|e| format!("parse: {e}"))?;
    if !m.verify() {
        return Err("signature invalid".into());
    }
    if !m.permissions.secrets.iter().any(|s| s == &secret_name) {
        return Err(format!(
            "plugin does not declare secret '{secret_name}' — cannot grant"
        ));
    }
    plugin_host::grant_secret(
        &name,
        &secret_name,
        plugin_host::KeychainRef {
            service: kc_service,
            account: kc_account,
        },
    )
    .map_err(|e| e.to_string())?;
    state
        .audit
        .write(EventInput {
            kind: "plugin.secret.grant",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"name": &name, "secret": &secret_name}), // NAME only, never value
        })
        .ok();
    Ok(())
}

/// Revoke a single secret grant.
#[tauri::command]
pub fn plugin_revoke_secret(name: String, secret_name: String) -> Result<(), String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    plugin_host::revoke_secret(&name, &secret_name).map_err(|e| e.to_string())
}

/// The secret grants for a plugin (secret_name → keychain ref). NO values.
#[tauri::command]
pub fn plugin_secret_refs(
    name: String,
) -> Result<std::collections::HashMap<String, plugin_host::KeychainRef>, String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    Ok(plugin_host::granted_secret_refs(&name))
}

/// Harden an installed plugin dir to read-only (closes the entrypoint-swap TOCTOU).
/// Called after install; idempotent.
#[tauri::command]
pub fn plugin_harden(name: String) -> Result<(), String> {
    if !plugins_svc_is_safe(&name) {
        return Err("unsafe plugin name".into());
    }
    let home = dirs::home_dir().ok_or("no home")?;
    plugin_host::harden_readonly(&home.join(".furx").join("plugins").join(&name))
        .map_err(|e| e.to_string())
}

// B5
use crate::services::{plugins as plugins_svc, sync_state as sync_svc};

#[tauri::command]
pub fn plugins_scan() -> Result<Vec<plugins_svc::PluginManifest>, String> {
    plugins_svc::scan_dir().map_err(|e| e.to_string())
}
#[tauri::command]
pub fn plugins_install(
    state: State<'_, AppState>,
    manifest: plugins_svc::PluginManifest,
) -> Result<String, String> {
    plugins_svc::install(&state.db, &manifest).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn plugins_list(state: State<'_, AppState>) -> Result<Vec<plugins_svc::Plugin>, String> {
    plugins_svc::list(&state.db).map_err(|e| e.to_string())
}
/// spec-022 US1 — persiste enable/disable. `id` = nombre del plugin en disco
/// (la identidad estable; disco = SoT). Upsert keyed por nombre.
#[tauri::command]
pub fn plugins_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    plugins_svc::set_enabled(&state.db, &id, enabled).map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// spec-043 · Ola 4 — Skills híbrido con verificación (F5: UI estado de verificación)
// ──────────────────────────────────────────────────────────────────────────
use crate::services::{
    skill_discovery as skill_disc_svc, skill_import as skill_import_svc,
    skill_manifest as skill_manifest_svc, skill_registry as skill_reg_svc,
};

/// A skill row for the UI: its disk metadata + trust state + badge token + the
/// revocation-file parse warning flag (banner). Built from the registry + disk.
#[derive(serde::Serialize)]
pub struct SkillTrustRow {
    pub name: String,
    pub trust_level: Option<String>, // "verified"|"promoted"|"sandboxed"|"rejected"
    pub badge: String,               // same set; UI maps to verde/amarillo/rojo
    pub inert: bool,
    pub status_message: Option<String>,
    pub may_execute: bool,
}

/// spec-043 F5 — list installed skills with their trust state (verde/amarillo/rojo) +
/// whether the revocation file had malformed entries (UI banner). Reads the registry
/// trust columns for every disk-present plugin name.
#[tauri::command]
pub fn skills_trust_list(
    state: State<'_, AppState>,
) -> Result<(Vec<SkillTrustRow>, bool), String> {
    let plugins = plugins_svc::list(&state.db).map_err(|e| e.to_string())?;
    let conn = state.db.lock();
    let mut rows = Vec::new();
    for p in plugins {
        let st = skill_reg_svc::get_state(&conn, &p.name).map_err(|e| e.to_string())?;
        let (trust_level, badge, inert, status, may_exec) = match st.and_then(|s| {
            s.trust_level.map(|l| (l, s.inert, s.status_message))
        }) {
            Some((l, inert, status)) => (
                Some(l.badge().to_string()),
                l.badge().to_string(),
                inert,
                status,
                l.may_execute(),
            ),
            // ⟨audit MED⟩ No skill trust row → legacy plugin (entrypoint-based
            // SignedManifest, NOT a 043 skill). Use a DISTINCT `legacy` badge so the UI
            // never claims skill-level tree_hash verification for it, and never offers
            // "Promover scripts" (which only applies to a Sandboxed *skill*). Carry the
            // Ed25519 verified flag in the trust_level field for display nuance.
            None => {
                let lvl = if p.verified { "legacy-verified" } else { "legacy-unsigned" };
                (Some(lvl.to_string()), "legacy".to_string(), !p.verified, None, p.verified)
            }
        };
        rows.push(SkillTrustRow {
            name: p.name,
            trust_level,
            badge,
            inert,
            status_message: status,
            may_execute: may_exec,
        });
    }
    drop(conn);
    // Revocation-file parse warning (banner).
    let revoked_path = dirs::home_dir()
        .ok_or("no home")?
        .join(".furx")
        .join("revoked_keys.txt");
    // ⟨audit HIGH⟩ Surface the warning on ANY revocation problem (parse warnings OR a
    // real read error) — never silently report "no warning" when we couldn't read it.
    let has_warn = match skill_manifest_svc::load_revoked_keys(&revoked_path) {
        Ok(rk) => rk.has_parse_warnings,
        Err(_) => true, // unreadable revocation file → flag the banner, don't hide it
    };
    Ok((rows, has_warn))
}

/// spec-043 F5/F4 — discover importable skills from the user's local sources
/// (`~/.furx/sources.user.toml`, type=local, paths inside $HOME). Read-only listing.
#[tauri::command]
pub fn skills_discover_local() -> Result<Vec<skill_disc_svc::DiscoveredSkill>, String> {
    let home = dirs::home_dir().ok_or("no home")?;
    let toml = home.join(".furx").join("sources.user.toml");
    skill_disc_svc::discover_local(&toml, &home).map_err(|e| e.to_string())
}

/// spec-043 F5/F3 — import a skill from a LOCAL directory path through the gate
/// (flock + staging + verify + install-only). Returns the resolved trust badge.
#[tauri::command]
pub fn skill_import_local(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("no home")?;
    let furx = home.join(".furx");
    let plugins_base = furx.join("plugins");
    let trusted: Vec<String> = skill_manifest_svc::pinned_trusted_keys();
    // ⟨audit HIGH⟩ FAIL-CLOSED on revocation: a missing file is Ok (empty set, handled
    // inside load_revoked_keys), but a real IO/read error MUST abort the import — never
    // proceed with an empty revoked set when we couldn't read the revocation list (that
    // would silently accept a revoked signing key).
    let revoked = skill_manifest_svc::load_revoked_keys(&furx.join("revoked_keys.txt"))
        .map_err(|e| format!("cannot read revocation list (refusing import): {e}"))?
        .keys;
    let conn = state.db.lock();
    let out = skill_import_svc::import_skill(
        &conn,
        &furx,
        &plugins_base,
        skill_import_svc::ImportSource::Local(std::path::PathBuf::from(path)),
        &trusted,
        &revoked,
    )
    .map_err(|e| e.to_string())?;
    Ok(out.level.badge().to_string())
}

/// spec-043 F5/F2 — promote a Sandboxed skill (user explicitly trusts the source).
/// Makes its scripts executable. Only valid from Sandboxed (errors otherwise).
#[tauri::command]
pub fn skill_promote(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let conn = state.db.lock();
    skill_reg_svc::promote(&conn, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sync_run(
    state: State<'_, AppState>,
    remote: Option<String>,
) -> Result<sync_svc::SyncReport, String> {
    sync_svc::snapshot_and_commit(&state.db, remote.as_deref()).map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// BLOQUE 1 · Wizard "Furx Connect" — providers + license
// ──────────────────────────────────────────────────────────────────────────

use crate::services::license as license_svc;
use crate::services::providers as providers_svc;

/// List all provider credentials.
#[tauri::command]
pub fn provider_list(
    state: State<'_, AppState>,
) -> Result<Vec<providers_svc::ProviderCredential>, String> {
    providers_svc::list_all(&state.db).map_err(|e| e.to_string())
}

/// Get a single provider by alias.
#[tauri::command]
pub fn provider_get(
    state: State<'_, AppState>,
    alias: String,
) -> Result<Option<providers_svc::ProviderCredential>, String> {
    providers_svc::get(&state.db, &alias).map_err(|e| e.to_string())
}

/// Persist provider credential. Saves key to Keychain if applicable.
#[tauri::command]
pub fn provider_persist(
    state: State<'_, AppState>,
    app: AppHandle,
    req: providers_svc::PersistRequest,
) -> Result<providers_svc::ProviderCredential, String> {
    let cred = providers_svc::persist(&state.db, req).map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit("provider:changed", &cred.alias);
    let _ = state.audit.write(EventInput {
        kind: "provider.persist",
        actor: "system",
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"alias": cred.alias, "provider": cred.provider}),
    });
    Ok(cred)
}

/// Delete provider credential. Removes Keychain entry + row.
#[tauri::command]
pub fn provider_delete(
    state: State<'_, AppState>,
    app: AppHandle,
    alias: String,
) -> Result<bool, String> {
    let removed = providers_svc::delete(&state.db, &alias).map_err(|e| e.to_string())?;
    if removed {
        use tauri::Emitter;
        let _ = app.emit("provider:changed", &alias);
        let _ = state.audit.write(EventInput {
            kind: "provider.delete",
            actor: "system",
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"alias": alias}),
        });
    }
    Ok(removed)
}

/// Test ping a provider — sends a 1-token chat completion, measures latency.
#[tauri::command]
pub async fn provider_test(
    state: State<'_, AppState>,
    app: AppHandle,
    alias: String,
) -> Result<providers_svc::PingResult, String> {
    let db = state.db.clone();
    let result = providers_svc::test_ping(&db, &alias)
        .await
        .map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit("provider:changed", &alias);
    let _ = state.audit.write(EventInput {
        kind: "provider.test",
        actor: "system",
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "alias": alias,
            "ok": result.ok,
            "latency_ms": result.latency_ms,
            "model": result.model,
        }),
    });
    Ok(result)
}

/// License check + trial status. Reads endpoint from settings.endpoints.license
/// (fallback to settings.endpoints.aie if absent — they live on the same the dev server host).
#[tauri::command]
pub async fn license_check(
    state: State<'_, AppState>,
) -> Result<license_svc::LicenseState, String> {
    let endpoint = {
        let conn = state.db.lock();
        crate::settings::get(&conn, "endpoints.license")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                crate::settings::get(&conn, "endpoints.aie")
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_str().map(String::from))
            })
            .unwrap_or_else(|| "https://aie.example.test".to_string())
    };
    license_svc::check(&state.db, &endpoint)
        .await
        .map_err(|e| e.to_string())
}

/// Returns install_id (UUID v4) — generates on first call.
#[tauri::command]
pub fn license_install_id(state: State<'_, AppState>) -> Result<String, String> {
    license_svc::install_id(&state.db).map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// BLOQUE 2 · Local provider scan + Council Mode multi-provider dispatch
// BLOQUE 3 · Resilience snapshot + preset overrides
// ──────────────────────────────────────────────────────────────────────────

use crate::services::council_multi as council_multi_svc;
use crate::services::resilience as resilience_svc;

#[tauri::command]
pub fn resilience_snapshot(
    state: State<'_, AppState>,
) -> Result<Vec<resilience_svc::ProviderHealthSnapshot>, String> {
    resilience_svc::snapshot(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preset_override_set(
    state: State<'_, AppState>,
    preset: String,
    provider_alias: String,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.db.lock();
    conn.execute(
        "INSERT INTO preset_overrides (preset, provider_alias, enabled, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(preset, provider_alias) DO UPDATE SET
            enabled = excluded.enabled,
            updated_at = excluded.updated_at",
        params![
            preset,
            provider_alias,
            enabled as i64,
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct PresetOverride {
    pub preset: String,
    pub provider_alias: String,
    pub enabled: bool,
    pub updated_at: String,
}

#[tauri::command]
pub fn preset_overrides_list(state: State<'_, AppState>) -> Result<Vec<PresetOverride>, String> {
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT preset, provider_alias, enabled, updated_at FROM preset_overrides")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PresetOverride {
                preset: r.get(0)?,
                provider_alias: r.get(1)?,
                enabled: r.get::<_, i64>(2)? != 0,
                updated_at: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

// ──────────────────────────────────────────────────────────────────────────
// B9 · Claude Accounts (multi-Max management)
// ──────────────────────────────────────────────────────────────────────────

// ── 006 agent-profiles commands ──────────────────────────────────────
use crate::services::agent_profiles as agent_profiles_svc;

#[tauri::command]
pub fn agent_profile_list(
    state: State<'_, AppState>,
) -> Result<Vec<agent_profiles_svc::AgentProfile>, String> {
    agent_profiles_svc::list_all(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn agent_profile_create(
    state: State<'_, AppState>,
    profile: agent_profiles_svc::AgentProfile,
) -> Result<agent_profiles_svc::AgentProfile, String> {
    let p = agent_profiles_svc::create(&state.db, profile).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "agent_profile.created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"id": p.id, "name": p.name, "cli_kind": p.cli_kind}),
    });
    Ok(p)
}

#[tauri::command]
pub fn agent_profile_update(
    state: State<'_, AppState>,
    profile: agent_profiles_svc::AgentProfile,
) -> Result<agent_profiles_svc::AgentProfile, String> {
    let p = agent_profiles_svc::update(&state.db, profile).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "agent_profile.updated",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"id": p.id, "name": p.name}),
    });
    Ok(p)
}

#[tauri::command]
pub fn agent_profile_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let removed = agent_profiles_svc::delete(&state.db, &id).map_err(|e| e.to_string())?;
    if removed {
        let _ = state.audit.write(EventInput {
            kind: "agent_profile.deleted",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"id": id}),
        });
    }
    Ok(removed)
}

/// Export sanitizado (sin secrets/slug/cwd/id) — FR-011.
#[tauri::command]
pub fn agent_profile_export(
    state: State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let p = agent_profiles_svc::get(&state.db, &id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("agente no encontrado: {}", id))?;
    Ok(agent_profiles_svc::export_sanitized(&p))
}

/// Import desde agent.json sanitizado → crea un perfil local (account_slug en None: el
/// user asocia su cuenta) — FR-012.
#[tauri::command]
pub fn agent_profile_import(
    state: State<'_, AppState>,
    json: serde_json::Value,
) -> Result<agent_profiles_svc::AgentProfile, String> {
    let parsed = agent_profiles_svc::import_from_json(&json).map_err(|e| e.to_string())?;
    let p = agent_profiles_svc::create(&state.db, parsed).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "agent_profile.imported",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"id": p.id, "name": p.name}),
    });
    Ok(p)
}

// ── 008 orchestration commands ──────────────────────────────────────
use crate::services::orchestration as orch_svc;
use crate::services::signals as signals_svc;

/// 010-furx-signals — emite el SignalEvent correspondiente a una transición de tarea.
/// Mapea estado → tipo de evento + severity. NO falla la operación si el emit falla.
fn emit_task_signal(state: &AppState, task: &orch_svc::OrchTask, new_state: &str) {
    let (event_type, severity) = match new_state {
        "done" => ("task.done", "info"),
        "failed" => ("task.failed", "critical"),
        "awaiting_review" => ("task.awaiting_review", "warning"),
        _ => return, // running/pending/canceled no notifican (canceled = acción del user)
    };
    let title = format!("{} · {}", task.title, new_state);
    let body = task.result_summary.clone().unwrap_or_default();
    let _ = signals_svc::emit_task_event(
        &state.db, event_type, &task.id, None, &title, &body, severity,
    );
}

#[tauri::command]
pub fn orchestration_create_batch(
    state: State<'_, AppState>,
    title: String,
    repo_path: String,
    base_branch: Option<String>,
    base_commit: Option<String>,
    tasks: Vec<orch_svc::TaskSpec>,
) -> Result<serde_json::Value, String> {
    let (batch_id, tasks) = orch_svc::create_batch(
        &state.db,
        &title,
        &repo_path,
        base_branch.as_deref(),
        base_commit.as_deref(),
        &tasks,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "orch.batch_created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"batch_id": batch_id, "tasks": tasks.len()}),
    });
    Ok(serde_json::json!({"batch_id": batch_id, "tasks": tasks}))
}

#[tauri::command]
pub fn orchestration_list(
    state: State<'_, AppState>,
    batch_id: Option<String>,
) -> Result<Vec<orch_svc::OrchTask>, String> {
    orch_svc::list_tasks(&state.db, batch_id.as_deref()).map_err(|e| e.to_string())
}

/// Prepara una tarea para lanzarse: CLAIM atómico (pending→running, evita doble-spawn) +
/// crea su worktree (reusa worktree::ensure). NO spawnea: devuelve los datos para que el
/// front monte un pane (Terminal) que spawnea el agente con sessionOverride único por tarea
/// (N tareas del mismo agente NO comparten sesión tmux). El front entrega el `objective` al
/// agente vía pty_write tras montar.
#[tauri::command]
pub fn orchestration_prepare_task(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    // Claim atómico: sólo un caller pasa pending→running (audit codex+deepseek HIGH).
    if !orch_svc::claim_for_launch(&state.db, &task_id).map_err(|e| e.to_string())? {
        return Err(format!("la tarea no está pending (estado: {})", task.state));
    }
    let pane_id = format!("orch-{}", task_id);
    let repo = std::path::Path::new(&task.repo_path);
    // 014 FR-005 — serializar la fase `git worktree add` por repo (no es concurrente-safe en el
    // index del repo padre). Lanzar N variantes del mismo repo en paralelo no debe colisionar.
    let repo_lock = orch_svc::repo_worktree_lock(&task.repo_path);
    let _wt_guard = repo_lock.lock();
    let wt = match worktree::ensure(repo, &task.branch) {
        Ok(w) => w,
        Err(e) => {
            // revertir el claim: running→failed (no dejar la tarea colgada en running).
            let _ = orch_svc::set_state(&state.db, &task_id, "failed", None);
            // 010-furx-signals — notificar el fallo.
            if let Ok(Some(t)) = orch_svc::get_task(&state.db, &task_id) {
                emit_task_signal(&state, &t, "failed");
            }
            return Err(format!("no se pudo crear el worktree: {}", e));
        }
    };
    let wt_cwd = wt.worktree_path.clone();
    orch_svc::mark_running(&state.db, &task_id, &wt_cwd, Some(&pane_id))
        .map_err(|e| e.to_string())?;
    // 019 F0 · T005 — CHECKPOINT-POR-ATTEMPT (R4): registrar el punto de partida del attempt al
    // empezar (HEAD del worktree + si lo creamos). Un kill posterior (orchestration_cancel) usa este
    // checkpoint para abortar transaccionalmente: descartar el worktree creado, o restaurarlo al base
    // — nunca dejar el repo a medio escribir. Best-effort en el registro (no romper el launch si el
    // rev-parse falla); el kill degrada a noop-de-worktree si no hay checkpoint.
    {
        use crate::services::attempt_checkpoint;
        let base_commit = std::process::Command::new("git")
            .args(["-C", &wt_cwd, "rev-parse", "HEAD"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if !base_commit.is_empty() {
            let _ = attempt_checkpoint::register(
                &state.db,
                &task_id,
                task.group_id.as_deref(),
                &wt_cwd,
                &base_commit,
                wt.created,
            );
        }
    }
    // 012-pty-done-detection — cachear el cli_kind (claude/codex/aider/gemini) para que el
    // poller elija la tabla de patrones correcta sin re-resolver el agent profile en cada tick.
    // 019 F1 (T010): el dispatch agent-neutral (profile→cli_kind, sino prefijo del mode legacy)
    // pasa AHORA por `agents::resolve_task_kind` — UN solo punto de derivación, comportamiento
    // idéntico al inline anterior. El `AgentKind` tipado queda disponible para el flujo best-of-N.
    {
        let (cli_kind, _agent_kind) = crate::services::agents::resolve_task_kind(
            task.agent_profile_id.as_deref(),
            task.mode.as_deref(),
            |pid| {
                crate::services::agent_profiles::get(&state.db, pid)
                    .ok()
                    .flatten()
            },
        );
        if let Some(ck) = cli_kind {
            let conn = state.db.lock();
            let _ = conn.execute(
                "UPDATE orchestration_tasks SET cli_kind = ?2 WHERE id = ?1",
                rusqlite::params![task_id, ck],
            );
        }
    }
    let _ = state.audit.write(EventInput {
        kind: "orch.task_prepared",
        actor: &crate::services::identity::current_actor(),
        pane_id: Some(&pane_id),
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"task_id": task_id, "branch": task.branch}),
    });
    Ok(serde_json::json!({
        "pane_id": pane_id,
        "worktree_path": wt_cwd,
        "mode": task.mode.clone().unwrap_or_else(|| "zsh".to_string()),
        "agent_profile_id": task.agent_profile_id,
        "objective": task.objective,
        "session": format!("orch_{}", task_id),
    }))
}

/// Recolecta el diff del worktree y pasa la tarea a awaiting_review (acción humana explícita
/// — council: NO auto-completion por polling/timeout).
#[tauri::command]
pub fn orchestration_mark_ready(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    task_id: String,
) -> Result<orch_svc::OrchTask, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    if let Some(wt) = task.worktree_path.as_deref() {
        let summary = orch_svc::collect_diff(wt);
        orch_svc::set_result_summary(&state.db, &task_id, &summary).map_err(|e| e.to_string())?;
    }
    // 014 FR-003 — snapshot del buffer al cerrar (último estado visible del agente).
    if let Some(pane_id) = task.pane_id.as_deref() {
        let lines = pty.snapshot(pane_id);
        let _ = orch_svc::append_log_history(&state.db, &task_id, "mark_ready", &lines);
    }
    orch_svc::set_state(&state.db, &task_id, "awaiting_review", None).map_err(|e| e.to_string())?;
    let updated = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "tarea desapareció".to_string())?;
    // 010-furx-signals — notificar awaiting_review.
    emit_task_signal(&state, &updated, "awaiting_review");
    Ok(updated)
}

/// 010-furx-signals — transición de estado de tarea genérica que EMITE el signal apropiado
/// (task.done / task.failed / task.awaiting_review). Hook de los productores (008). Valida
/// la transición vía orchestration::set_state. canceled NO notifica (acción del user).
#[tauri::command]
pub fn orchestration_set_state(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    new_state: String,
    exit_code: Option<i64>,
) -> Result<orch_svc::OrchTask, String> {
    orch_svc::set_state(&state.db, &task_id, &new_state, exit_code).map_err(|e| e.to_string())?;
    let updated = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "tarea desapareció".to_string())?;
    emit_task_signal(&state, &updated, &new_state);
    // 015 US3 — DEMOSTRACIÓN del event bus SSOT: en paralelo a las señales 010 (notificaciones),
    // publicamos la mutación de estado crítico por el bus tipado para que TODAS las ventanas
    // rehidraten. Best-effort, no altera el comportamiento existente.
    crate::services::event_bus::emit_event(
        &app,
        crate::services::event_bus::AppEvent::TaskChanged {
            id: updated.id.clone(),
            state: new_state.clone(),
        },
    );
    // 038 F1.3 — HOOK DE AVANCE del DAG, POST-return de `set_state` (su lock YA se soltó). Sólo para
    // tareas de pipeline que llegan a un estado TERMINAL: `done` desbloquea los dependientes listos;
    // `failed`/`canceled` cascadea skip a los descendientes `pending`. `on_task_settled` abre su PROPIO
    // scope de lock (sin re-entrancy → sin self-deadlock sobre el Mutex no-reentrante). Best-effort: un
    // fallo del avance NO revierte la transición ya aplicada (se loguea). NO toca el foco del micrófono.
    if matches!(new_state.as_str(), "done" | "failed" | "canceled")
        && updated.pipeline_run_id.is_some()
    {
        if let Err(e) =
            crate::services::pipeline_scheduler::on_task_settled(&state.db, &task_id, &new_state)
        {
            tracing::warn!("pipeline advance hook (task {task_id} → {new_state}) falló: {e}");
        }
    }
    Ok(updated)
}

// ── 015 US5 process manager commands ────────────────────────────────
// Registro CENTRAL de procesos/jobs (services/process_manager.rs). El proceso es
// PROPIEDAD del backend y SOBREVIVE a unmount/reload/cierre de ventana de la UI:
// la UI es un viewport que observa/controla. Estos comandos exponen list/cancel/attach.
use crate::services::process_manager as pm_svc;

/// Lista los procesos vivos del registro (lo que un viewport ve al reattach).
#[tauri::command]
pub fn process_list(state: State<'_, AppState>) -> Result<Vec<pm_svc::ProcessInfo>, String> {
    pm_svc::list(&state.db, true).map_err(|e| e.to_string())
}

/// 015 T014 (US5) — helper ÚNICO de cancelación registry-routed. Toda cancelación real de
/// un proceso (palette/UI → `process_cancel`, low-level → `pty_kill`, orquestación →
/// `orchestration_cancel`) pasa por acá para que el registro de procesos sea la SSOT y NO
/// diverja de la realidad (riesgo HIGH que levantó el council: los kills directos dejaban la
/// fila `running`). Marca `canceled` en el registry, mata el recurso real best-effort y, sólo
/// si ESTA llamada transicionó running→canceled, audita + emite `TaskChanged` por el bus.
///
/// El kill se hace SÓLO cuando ESTA llamada transicionó running→canceled (`newly_canceled`).
/// Una fila ya terminal vía `finish` significa que el PTY YA salió (el wait-thread sólo
/// finaliza tras observar el exit) y vía `cancel` que YA fue matado — re-matar sería un kill
/// del recurso bajo `external_ref`, que para un pane_id REUSADO sería OTRO proceso vivo
/// (audit 015 T014 HIGH: no matar el proceso equivocado).
///
/// `notify`: si `true`, audita + emite `TaskChanged{canceled}` por el bus (cancelación de cara
/// al usuario: palette/UI/Telegram). Si `false`, es un REAP interno (el respawn de un pane que
/// cambió de mode/cwd) → silencioso, igual que un primer spawn: no ensucia el audit ni emite un
/// `canceled` espurio que la UI vería como un flicker (audit re-ronda, gemini/deepseek/AIE).
/// Toma primitivas (no `&AppState`) para que también lo use el inbound de Telegram (otro state).
/// Devuelve la fila resultante.
pub(crate) fn cancel_reap_emit(
    db: &Arc<parking_lot::Mutex<rusqlite::Connection>>,
    pty: &Arc<PtyManager>,
    app: &tauri::AppHandle,
    audit: &crate::bases::audit::AuditWriter,
    process_id: &str,
    notify: bool,
) -> Result<pm_svc::ProcessInfo, String> {
    let outcome = pm_svc::cancel(db, process_id).map_err(|e| e.to_string())?;
    if outcome.newly_canceled {
        if outcome.info.kind == pm_svc::ProcessKind::Pty.as_str() {
            if let Some(pane_id) = outcome.info.external_ref.as_deref() {
                // 023 F1 — CIERRE ABRUPTO de cara al usuario (notify=true): antes de matar el pane
                // (lo que purga su SessionBuffer), resguardar el buffer en `incomplete_sessions`
                // (TTL 5 min) para reprocesarlo al próximo idle/capture en vez de perderlo. Gateado
                // por autocapture=on + CLI de agente; el buffer se SCRUBEA (incl. secretos partidos)
                // antes de tocar la DB. En un REAP interno (respawn de mode/cwd, notify=false) NO
                // se resguarda: no es un fin de sesión real, sólo el reemplazo del run del pane.
                if notify {
                    save_abrupt_close_buffer(db, pty, pane_id);
                }
                let _ = pty.kill(pane_id); // best-effort; puede ya estar muerto
            }
        }
        if notify {
            let _ = audit.write(EventInput {
                kind: "process.canceled",
                actor: &crate::services::identity::current_actor(),
                pane_id: outcome.info.external_ref.as_deref(),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"process_id": process_id, "kind": outcome.info.kind}),
            });
            crate::services::event_bus::emit_event(
                app,
                crate::services::event_bus::AppEvent::TaskChanged {
                    id: process_id.to_string(),
                    state: "canceled".into(),
                },
            );
        }
    }
    Ok(outcome.info)
}

/// 023 F1 — resguarda el SessionBuffer de un pane en `incomplete_sessions` ANTES de un cierre
/// abrupto (kill/cancelación), para reprocesarlo al próximo idle/capture y no perder la sesión.
/// Best-effort y gateado: sin autocapture on, o si el pane no es un CLI de agente / no tiene
/// buffer, es no-op. El buffer se SCRUBEA (incl. secretos partidos entre líneas) dentro de
/// `save_incomplete_session` antes de tocar la DB.
fn save_abrupt_close_buffer(
    db: &Arc<parking_lot::Mutex<rusqlite::Connection>>,
    pty: &Arc<PtyManager>,
    pane_id: &str,
) {
    // Tomar (y remover) el buffer del pane. Sin buffer → este pane no era de captura.
    let Some((cap_ctx, lines, had_output)) = pty.take_session_buffer(pane_id) else {
        return;
    };
    if !crate::services::memory_autocapture::is_agent_cli(&cap_ctx.cli_kind) {
        return;
    }
    // autocapture default-OFF: con off, cero resguardo (igual que cero captura).
    let enabled = {
        let conn = db.lock();
        crate::settings::get(&conn, "memory.autocapture")
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    if !enabled {
        return;
    }
    let ctx = crate::services::memory_autocapture::SessionCtx {
        pane_id: pane_id.to_string(),
        cli_kind: cap_ctx.cli_kind,
        project_key: cap_ctx.project_key,
        session_id: cap_ctx.session_id,
    };
    let _ = crate::services::memory_autocapture::save_incomplete_session(db, &ctx, &lines, had_output);
}

/// Cancelación de cara al usuario (audita + emite). Wrapper de `cancel_reap_emit(notify=true)`
/// para los call-sites que tienen `&AppState` (process_cancel/pty_kill/orchestration_cancel).
pub(crate) fn cancel_and_reap(
    state: &AppState,
    pty: &Arc<PtyManager>,
    app: &tauri::AppHandle,
    process_id: &str,
) -> Result<pm_svc::ProcessInfo, String> {
    cancel_reap_emit(&state.db, pty, app, &state.audit, process_id, true)
}

/// CANCELLATION EXPLÍCITA de un proceso. Marca `canceled` en el registro y mata el recurso
/// real (para `kind=pty`, vía el PtyManager por el `external_ref` = pane_id). Idempotente
/// (re-cancelar un terminal no es error). Emite AppEvent::TaskChanged por el bus para que
/// TODAS las ventanas rehidraten. Rutea por `cancel_and_reap` (SSOT).
#[tauri::command]
pub fn process_cancel(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    app: tauri::AppHandle,
    process_id: String,
) -> Result<pm_svc::ProcessInfo, String> {
    cancel_and_reap(state.inner(), pty.inner(), &app, &process_id)
}

/// ATTACH/reattach: una UI que se re-suscribe (re-mount tras reload) pide el estado
/// vigente del proceso, que siguió vivo en el backend mientras la vista no existía.
#[tauri::command]
pub fn process_attach(
    state: State<'_, AppState>,
    process_id: String,
) -> Result<pm_svc::ProcessInfo, String> {
    pm_svc::attach(&state.db, &process_id).map_err(|e| e.to_string())
}

/// Recolecta el diff stat de una tarea sin cambiar su estado (preview).
#[tauri::command]
pub fn orchestration_collect(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<String, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    match task.worktree_path.as_deref() {
        Some(wt) => {
            let summary = orch_svc::collect_diff(wt);
            let _ = orch_svc::set_result_summary(&state.db, &task_id, &summary);
            Ok(summary)
        }
        None => Ok("(la tarea aún no se lanzó)".to_string()),
    }
}

/// Cancela una tarea: mata su proceso (pty_kill) + estado canceled. NO toca el worktree
/// (limpieza con confirmación aparte). No tumba el batch.
#[tauri::command]
pub fn orchestration_cancel(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    app: tauri::AppHandle,
    task_id: String,
) -> Result<(), String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    // 015 T014 (US5): cancelar la tarea rutea el kill de su PTY por el registro de procesos
    // (SSOT) en vez de matar directo — así la fila del proceso transiciona a `canceled` y no
    // diverge. Si la tarea aún no tenía PTY registrado, `cancel_and_reap` falla suave y el
    // kill best-effort cubre el caso (el estado de la TAREA se setea igual abajo).
    if let Some(pid) = task.pane_id.as_deref() {
        if cancel_and_reap(state.inner(), pty.inner(), &app, pid).is_err() {
            let _ = pty.kill(pid); // best-effort; puede ya estar muerto o sin fila
        }
    }
    orch_svc::set_state(&state.db, &task_id, "canceled", None).map_err(|e| e.to_string())?;
    // 019 F0 · T005 — KILL-SWITCH TRANSACCIONAL (R4): tras matar el PTY (ruteado por el
    // process-registry — NUNCA mata procesos ajenos), abortar el WORKTREE del attempt usando su
    // checkpoint: descartar el worktree creado o restaurarlo al base. Idempotente (un 2º kill →
    // AlreadyKilled). Nunca deja el repo a medio escribir. No-checkpoint → noop (attempt sin worktree).
    let kill_outcome = crate::services::attempt_checkpoint::kill_attempt(&state.db, &task_id)
        .map_err(|e| format!("kill del worktree del attempt falló: {e}"))?;
    // 019 F0 · T001 — auditar el kill con vínculo audit↔change-set (group) (R2/FR-005).
    {
        use crate::services::review_audit::{
            self, ReviewAction, ReviewAuditEntry, ReviewTargetLink,
        };
        let outcome = format!("{kill_outcome:?}");
        let _ = review_audit::record(
            &state.db,
            &state.audit,
            ReviewAuditEntry {
                action: ReviewAction::Kill,
                actor: &crate::services::identity::current_actor(),
                target: &task_id,
                rationale: &outcome,
                link: ReviewTargetLink {
                    group_id: task.group_id.clone(),
                    hunk_id: None,
                    approval_id: None,
                    revision: None,
                },
            },
        );
    }
    Ok(())
}

/// 038 F1.4 — `pipeline_cancel(run_id)`: cancela un run de pipeline ENTERO. (1) `pipeline_runs.status
/// = 'canceled'` PRIMERO → el scheduler deja de promover (el hook de avance ve el run no-`running` y
/// es noop). (2) Cada tarea `pending`/bloqueada → `canceled` directo. (3) Cada tarea `running` →
/// la ruta de kill transaccional EXISTENTE (`cancel_and_reap` + `attempt_checkpoint::kill_attempt`,
/// la MISMA que `orchestration_cancel`/best_of_n: mata el PTY ruteado por el process-registry y
/// restaura el worktree sin huérfanos). NO roba ni re-otorga foco. `awaiting_review`/terminal se dejan
/// como están (el trabajo ya terminó). Idempotente: re-cancelar un run terminal no es error.
#[tauri::command]
pub fn pipeline_cancel(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    app: tauri::AppHandle,
    run_id: String,
) -> Result<serde_json::Value, String> {
    // (1) Marcar el run canceled PRIMERO: el scheduler deja de promover. Err si el run no existe.
    crate::services::pipeline_scheduler::mark_run_canceled(&state.db, &run_id)
        .map_err(|e| e.to_string())?;

    let tasks = orch_svc::list_run_tasks(&state.db, &run_id).map_err(|e| e.to_string())?;
    let mut canceled_pending = 0usize;
    let mut killed_running = 0usize;
    // Recolectar fallos POR TAREA en vez de abortar a mitad (cancelar las demás tareas igual importa).
    // Pero NO reportar éxito si algún paso CRÍTICO falló (audit codex): un `set_state`/`kill_attempt`
    // fallido puede dejar una tarea inconsistente o un worktree sin restaurar (huérfano). Se devuelve
    // Err agregado tras intentar TODAS las tareas, igual que `orchestration_cancel` propaga su kill.
    let mut errors: Vec<String> = Vec::new();
    for task in &tasks {
        match task.state.as_str() {
            // (2) pending/bloqueada → canceled directo (transición válida pending→canceled).
            "pending" => match orch_svc::set_state(&state.db, &task.id, "canceled", None) {
                Ok(()) => canceled_pending += 1,
                Err(e) => errors.push(format!("cancelar pending {}: {e}", task.id)),
            },
            // (3) running → ruta de kill EXISTENTE (idéntica a orchestration_cancel).
            "running" => {
                if let Some(pid) = task.pane_id.as_deref() {
                    if cancel_and_reap(state.inner(), pty.inner(), &app, pid).is_err() {
                        let _ = pty.kill(pid); // best-effort (puede ya estar muerto o sin fila)
                    }
                }
                // Propagar el error de la transición de estado (no contar como kill si falló).
                if let Err(e) = orch_svc::set_state(&state.db, &task.id, "canceled", None) {
                    errors.push(format!("transición canceled {}: {e}", task.id));
                    continue;
                }
                // KILL-SWITCH TRANSACCIONAL del worktree del attempt (restaura/descarta sin huérfanos).
                // Si FALLA, el worktree puede quedar a medio restaurar → reportar (no swallow, audit codex).
                if let Err(e) = crate::services::attempt_checkpoint::kill_attempt(&state.db, &task.id) {
                    errors.push(format!("kill del worktree del attempt {}: {e}", task.id));
                    continue;
                }
                killed_running += 1;
            }
            // awaiting_review / done / failed / canceled → se dejan (trabajo ya terminado o en review).
            _ => {}
        }
    }

    let _ = state.audit.write(EventInput {
        kind: "pipeline.canceled",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "run_id": run_id, "canceled_pending": canceled_pending,
            "killed_running": killed_running, "errors": errors.len()
        }),
    });
    if !errors.is_empty() {
        // El run YA quedó `canceled` (no se promueve más) y se intentó cancelar TODO, pero algún paso
        // crítico falló → NO reportamos éxito limpio (puede haber un worktree sin restaurar).
        return Err(format!(
            "pipeline cancelado con {} error(es) (run marcado canceled, {} pending cancelados, {} running matados): {}",
            errors.len(), canceled_pending, killed_running, errors.join("; ")
        ));
    }
    Ok(serde_json::json!({
        "run_id": run_id,
        "canceled_pending": canceled_pending,
        "killed_running": killed_running,
    }))
}

/// 038 F1.5 (FR-009) — runs `waiting_on_human`: por cada run `running` SIN tarea corriendo pero CON
/// algo en review, devuelve `{run_id, waiting_minutes}`. El board lo usa para el advisory "esperando
/// review hace Nm" (timeline 035) — hace VISIBLE que el pipeline NO está colgado, está esperando al
/// humano. Lectura derivada (stateless), no muta nada.
#[tauri::command]
pub fn pipeline_waiting_runs(
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let waiting = crate::services::pipeline_scheduler::waiting_on_human(&state.db)
        .map_err(|e| e.to_string())?;
    Ok(waiting
        .into_iter()
        .map(|(run_id, mins)| serde_json::json!({"run_id": run_id, "waiting_minutes": mins}))
        .collect())
}

// ── 038 Goose-C P1 — ejecución del DAG de pipelines ─────────────────
use crate::services::pipeline as pipeline_svc;

/// 038 F1.2 — `pipeline_run_yaml`: toma un pipeline declarativo YAML (029), lo valida + topo-sortea,
/// RESUELVE cada slug de agente PORTABLE a un `agent_profile_id` LOCAL (**fail-closed**: un slug que no
/// resuelve a un perfil → `Err` ANTES de crear nada, nunca un agente fantasma), y crea el run completo
/// (batch + tareas en orden topo + `pipeline_runs` + `pipeline_edges`) en UNA transacción. NO lanza
/// ninguna tarea: las raíces quedan `pending`/`dag_blocked=0` (lanzables por el humano); las que tienen
/// deps quedan `dag_blocked=1`. El avance posterior lo conduce el scheduler (F1.3) al `done` humano.
#[tauri::command]
pub fn pipeline_run_yaml(
    state: State<'_, AppState>,
    yaml: String,
    repo_path: String,
    base_branch: Option<String>,
    base_commit: Option<String>,
) -> Result<serde_json::Value, String> {
    // Defensa en capas (audit deepseek): tope de tamaño EN EL LÍMITE del comando, antes de cualquier
    // trabajo. `parse_yaml` ya rechaza > 256 KB, pero acotar acá hace explícito el bound en el borde
    // de IPC (no cargar/parsear un YAML gigante). 256 KB entra holgado un pipeline de ≤64 tasks.
    const MAX_YAML_BYTES: usize = 256 * 1024;
    if yaml.len() > MAX_YAML_BYTES {
        return Err(format!(
            "YAML de pipeline demasiado grande ({} bytes, máx {MAX_YAML_BYTES})",
            yaml.len()
        ));
    }
    // 1) parse + validate + topo (029, puro). Cualquier inconsistencia → Err claro, sin tocar la DB.
    let spec = pipeline_svc::parse_yaml(&yaml).map_err(|e| e.to_string())?;
    pipeline_svc::validate(&spec).map_err(|e| e.to_string())?;
    let topo = pipeline_svc::topo_order(&spec).map_err(|e| e.to_string())?;

    // 2) Resolver slugs de agente → agent_profile_id LOCAL (fail-closed). El slug `agent` del YAML es
    //    PORTABLE (no un id de DB); se resuelve por NOMBRE del perfil (UNIQUE). Un slug que no matchea
    //    NINGÚN perfil local aborta ANTES de crear el batch (nunca un agente fantasma).
    let profiles = agent_profiles_svc::list_all(&state.db).map_err(|e| e.to_string())?;
    let resolve = |slug: &str| -> Option<String> {
        profiles
            .iter()
            .find(|p| p.name == slug)
            .map(|p| p.id.clone())
    };
    let mut resolved_tasks = Vec::with_capacity(spec.tasks.len());
    for t in &spec.tasks {
        let agent_profile_id = match &t.agent {
            Some(slug) => match resolve(slug) {
                Some(id) => Some(id),
                None => {
                    return Err(format!(
                        "agente '{}' (task '{}') no resuelve a ningún perfil local — fail-closed (no se crea nada)",
                        slug, t.id
                    ));
                }
            },
            None => None, // sin agente: el orquestador resuelve por mode/default (paridad single-task).
        };
        resolved_tasks.push(orch_svc::ResolvedPipelineTask {
            yaml_id: t.id.clone(),
            title: t.title.clone(),
            objective: t.objective.clone(),
            agent_profile_id,
            mode: t.mode.clone(),
        });
    }
    // 3) Aristas en espacio YAML (se traducen a uuids dentro de la txn).
    let mut edges = Vec::new();
    for t in &spec.tasks {
        for dep in &t.depends_on {
            edges.push(orch_svc::YamlEdge {
                task_yaml_id: t.id.clone(),
                depends_on_yaml_id: dep.clone(),
                on_error: None, // v1: default bloqueante; el override por-arista llega en v2.
            });
        }
    }
    // 4) Crear el run completo en UNA transacción.
    let repo = if repo_path.trim().is_empty() {
        spec.repo.clone().unwrap_or_default()
    } else {
        repo_path
    };
    let (run_id, batch_id, tasks) = orch_svc::create_pipeline_run(
        &state.db,
        &spec.name,
        &repo,
        base_branch.as_deref().or(spec.base_branch.as_deref()),
        base_commit.as_deref(),
        &resolved_tasks,
        &edges,
        &topo,
        &yaml,
    )
    .map_err(|e| e.to_string())?;

    let _ = state.audit.write(EventInput {
        kind: "pipeline.run_created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "run_id": run_id, "batch_id": batch_id, "name": spec.name, "n_tasks": tasks.len()
        }),
    });
    Ok(serde_json::json!({
        "run_id": run_id,
        "batch_id": batch_id,
        "tasks": tasks,
    }))
}

// ── 012-pty-done-detection — auto-confirm toggle ────────────────────
use crate::services::done_detection as dd_svc;

/// Toggle del auto-confirm OPT-IN por tarea (default OFF). Constitución VI: no destructivo
/// automático — esto sólo presiona Enter ante trust prompts CONOCIDOS, con tope/min.
#[tauri::command]
pub fn orchestration_set_auto_confirm(
    state: State<'_, AppState>,
    task_id: String,
    enabled: bool,
) -> Result<(), String> {
    dd_svc::set_auto_confirm(&state.db, &task_id, enabled).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "orch.auto_confirm_toggled",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"task_id": task_id, "enabled": enabled}),
    });
    Ok(())
}

// ── 014-orchestration-ux commands ───────────────────────────────────

/// FR-001 best-of-N — lanzar un objetivo como N variantes (≤4), cada una en su worktree/branch.
/// Crea el batch + grupo + N tareas pending (reusa el modelo de 008). El launch real (worktree +
/// spawn) lo hace el front por variante con `orchestration_prepare_task`, igual que una tarea
/// normal — esto sólo arma el grupo. `agents` = lista de agent_profile_id (None = mode legacy/zsh).
#[tauri::command]
pub fn orchestration_create_best_of_n(
    state: State<'_, AppState>,
    title: String,
    repo_path: String,
    base_branch: Option<String>,
    base_commit: Option<String>,
    objective: String,
    agents: Vec<Option<String>>,
) -> Result<serde_json::Value, String> {
    let (batch_id, group, tasks) = orch_svc::create_best_of_n(
        &state.db,
        &title,
        &repo_path,
        base_branch.as_deref(),
        base_commit.as_deref(),
        &objective,
        &agents,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "orch.best_of_n_created",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"batch_id": batch_id, "group_id": group.id, "n": group.n}),
    });
    Ok(serde_json::json!({"batch_id": batch_id, "group": group, "tasks": tasks}))
}

/// FR-001 — las N variantes de un grupo (para la vista de comparación N-way).
#[tauri::command]
pub fn orchestration_group_tasks(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<orch_svc::OrchTask>, String> {
    orch_svc::list_group_tasks(&state.db, &group_id).map_err(|e| e.to_string())
}

/// FR-001 — el grupo best-of-N (incluye chosen_task_id).
#[tauri::command]
pub fn orchestration_get_group(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<orch_svc::TaskGroup>, String> {
    orch_svc::get_group(&state.db, &group_id).map_err(|e| e.to_string())
}

/// FR-001 — comparación N-way: el diff stat (vs base) de cada variante de un grupo, lado a lado.
/// Reusa worktree_merge_review por variante. Read-only (no mergea). Devuelve por variante:
/// {task_id, variant_index, title, branch, state, diff_stat, risky_paths}.
#[tauri::command]
pub fn orchestration_compare_group(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let tasks = orch_svc::list_group_tasks(&state.db, &group_id).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(tasks.len());
    for t in &tasks {
        // diff stat: preferimos el del worktree (working tree de la variante); si no hay worktree
        // (no lanzada) caemos al diff de la branch vs base.
        let (diff_stat, risky) = if let Some(wt) = t.worktree_path.as_deref() {
            (orch_svc::collect_diff(wt), Vec::<String>::new())
        } else {
            match worktree_merge_review(t.repo_path.clone(), t.branch.clone()) {
                Ok(r) => (r.diff_stat, r.risky_paths),
                Err(_) => ("(la variante aún no se lanzó)".to_string(), vec![]),
            }
        };
        out.push(serde_json::json!({
            "task_id": t.id,
            "variant_index": t.variant_index,
            "title": t.title,
            "repo_path": t.repo_path,
            "branch": t.branch,
            "state": t.state,
            "diff_stat": diff_stat,
            "risky_paths": risky,
        }));
    }
    Ok(out)
}

// ── 024-quality-gate F1 — evidencia objetiva por variante (linters/typecheck) ──
use crate::services::quality_gate as qg;

/// 024 F1 — corre el quality-gate sobre las variantes de un grupo best-of-N: por cada variante
/// con worktree, autodetecta los linters del repo (clippy/eslint+tsc/ruff+mypy, allow-list
/// configurable) y los corre EN su worktree (sandbox + timeout + argv-only, council v2 §3),
/// devolviendo el conteo estructurado `VariantEvidence`. ADVISORY/read-only (no toca la review).
///
/// GATE (FR-020, council v2 §3.1): opt-in `qualitygate.enabled` (default OFF). Con OFF NO se
/// ejecuta NADA y se devuelve error explícito. Fail-safe: un linter ausente/falla → "no
/// disponible", NUNCA un `0` falso.
#[tauri::command]
pub async fn quality_gate_run(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<qg::VariantEvidence>, String> {
    // 1) Gate por setting + resolver linters habilitados — bajo el lock de la DB, SIN await
    //    (no se mantiene el guard a través de un await; se suelta antes de spawnear).
    let enabled_linters: Vec<String> = {
        let conn = state.db.lock();
        let on = crate::settings::get(&conn, "qualitygate.enabled")
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !on {
            return Err(
                "El quality-gate está desactivado. Activá «qualitygate.enabled» en Ajustes para correr los linters de tu repo sobre las variantes.".into(),
            );
        }
        let linters_val = crate::settings::get(&conn, "qualitygate.linters").ok().flatten();
        qg::enabled_linters_from_setting(linters_val.as_ref())
    };

    // Recolectar (task_id, worktree) de las variantes lanzadas. `list_group_tasks` toma `&Db`
    // (el Arc<Mutex<..>>) y maneja su propio lock internamente.
    let variants: Vec<(String, String)> = orch_svc::list_group_tasks(&state.db, &group_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|t| t.worktree_path.map(|wt| (t.id, wt)))
        .collect();

    // 2) Correr SYNC por variante (council v2 §2; el paralelo es F2). El motor es fail-safe.
    //    FAIL-CLOSED (audit 024 MED): antes de correr cualquier toolchain, revalidar que el
    //    `worktree_path` persistido sea un worktree git válido BAJO el root gestionado por Furx
    //    (`~/.furx/worktrees`). Si la DB/path quedó stale o manipulado, se RECHAZA la variante
    //    (no se corren linters sobre un path arbitrario).
    let mut out = Vec::with_capacity(variants.len());
    for (task_id, wt) in &variants {
        let path = std::path::Path::new(wt);
        match qg::validate_managed_worktree(path) {
            Ok(canon) => {
                let specs = qg::detect_linters(&canon, &enabled_linters);
                let ev =
                    qg::run_linters_for_variant(task_id, &canon, &specs, qg::DEFAULT_TIMEOUT).await;
                out.push(ev);
            }
            Err(reason) => {
                out.push(qg::rejected_variant(task_id, format!("worktree rechazado: {reason}")));
            }
        }
    }

    // 3) Persistir en memoria (F2 lo lleva a SQLite) para que `quality_gate_get` lo lea.
    state.quality_gate.lock().insert(group_id.clone(), out.clone());

    // 4) Audit liviano del hecho de correr (gobierno; sin volcar diffs). FR-022.
    let tools_run: Vec<&str> = enabled_linters.iter().map(|s| s.as_str()).collect();
    let total_unavailable: usize = out.iter().map(|e| e.unavailable_tools.len()).sum();
    let _ = state.audit.write(EventInput {
        kind: "quality_gate.run",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "group_id": group_id,
            "variants": out.len(),
            "linters": tools_run,
            "unavailable_count": total_unavailable,
        }),
    });

    Ok(out)
}

/// 024 F1 — devuelve la última evidencia calculada para un grupo (de la cache en memoria) SIN
/// re-ejecutar. Read-only/advisory. Lista vacía si nunca se corrió `quality_gate_run`.
#[tauri::command]
pub fn quality_gate_get(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<qg::VariantEvidence>, String> {
    Ok(state
        .quality_gate
        .lock()
        .get(&group_id)
        .cloned()
        .unwrap_or_default())
}

/// FR-001 — elegir la variante a mergear (el merge sigue el flujo de 008 con confirmación).
#[tauri::command]
pub fn orchestration_choose_variant(
    state: State<'_, AppState>,
    group_id: String,
    task_id: String,
) -> Result<(), String> {
    orch_svc::choose_variant(&state.db, &group_id, &task_id).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "orch.variant_chosen",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"group_id": group_id, "task_id": task_id}),
    });
    Ok(())
}

/// FR-001 — descartar UNA variante no-elegida (la cancela; NO toca su worktree). El front YA pidió
/// confirmación al humano (constitución VI: no destructivo silencioso). Devuelve true si la descartó.
#[tauri::command]
pub fn orchestration_discard_variant(
    state: State<'_, AppState>,
    group_id: String,
    task_id: String,
) -> Result<bool, String> {
    let discarded =
        orch_svc::discard_variant(&state.db, &group_id, &task_id).map_err(|e| e.to_string())?;
    if discarded {
        let _ = state.audit.write(EventInput {
            kind: "orch.variant_discarded",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"group_id": group_id, "task_id": task_id}),
        });
        // liberar cualquier lock de recurso que tuviera (best-effort).
        let _ = orch_svc::release_all_locks(&state.db, &task_id);
    }
    Ok(discarded)
}

// ── 019 F1 — review hunk-level UNIFICADA (capa sobre orchestration) ─────────────────────────────

/// Abre/refresca la review hunk-level unificada de un grupo best-of-N: por cada variante con
/// worktree, colecta su UNIFIED diff vs el base del grupo (base OBLIGATORIO y verificado — NO hay
/// fallback a HEAD, que omitiría commits del worktree) y lo proyecta (init_group, first-write-wins
/// por variante — no pisa decisiones, snapshotea el cuerpo). Devuelve la GroupReview cargada.
/// Read-mostly: sólo inicializa la proyección de review; NO toca worktrees/procesos.
#[tauri::command]
pub fn review_open(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<crate::services::review::GroupReview, String> {
    use crate::services::review;
    // base OBLIGATORIO (audit codex #4/B2): sin base, el diff vs HEAD omitiría commits ya hechos en el
    // worktree → el usuario revisaría (y aprobaría) algo INCOMPLETO. Mejor fallar y exigir base estable.
    let base = orch_svc::group_base(&state.db, &group_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            "el grupo no tiene base_commit/branch — no se puede abrir una review confiable"
                .to_string()
        })?;
    // verificar que el base RESUELVE (sino el diff caería a HEAD silenciosamente → incompleto).
    let tasks = orch_svc::list_group_tasks(&state.db, &group_id).map_err(|e| e.to_string())?;
    let repo_for_base = tasks.iter().find_map(|t| {
        t.worktree_path
            .clone()
            .or_else(|| Some(t.repo_path.clone()))
    });
    if let Some(rp) = &repo_for_base {
        let ok = std::process::Command::new("git")
            .args([
                "-C",
                rp,
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{base}^{{commit}}"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return Err(format!("el base '{base}' no resuelve a un commit en el repo — review abortada (no degradamos a HEAD)"));
        }
    }
    let variants: Vec<(String, String)> = tasks
        .iter()
        .filter_map(|t| {
            t.worktree_path.as_deref().map(|wt| {
                (
                    t.id.clone(),
                    orch_svc::collect_unified_diff(wt, Some(&base)),
                )
            })
        })
        .collect();
    review::init_group(&state.db, &group_id, &variants).map_err(|e| e.to_string())?;
    let loaded = review::load_group(&state.db, &group_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "review no encontrada tras init".to_string())?;
    // 019 F0 · T001 — auditar el COMPARE (apertura de la review unificada del best-of-N) con
    // vínculo audit↔change-set (group). Append-only; el rastro deja quién comparó qué (R2/FR-005).
    {
        use crate::services::review_audit::{
            self, ReviewAction, ReviewAuditEntry, ReviewTargetLink,
        };
        let target = format!("{group_id} ({} variants)", loaded.variants.len());
        let _ = review_audit::record(
            &state.db,
            &state.audit,
            ReviewAuditEntry {
                action: ReviewAction::Compare,
                actor: &crate::services::identity::current_actor(),
                target: &target,
                rationale: "open unified diff/review for best-of-N",
                link: ReviewTargetLink {
                    group_id: Some(group_id.clone()),
                    hunk_id: None,
                    approval_id: None,
                    revision: Some(loaded.revision),
                },
            },
        );
    }
    Ok(loaded)
}

/// Carga la review de un grupo (read-only). `None` si no se abrió todavía.
#[tauri::command]
pub fn review_get(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<crate::services::review::GroupReview>, String> {
    crate::services::review::load_group(&state.db, &group_id).map_err(|e| e.to_string())
}

/// Decide un hunk (approve/reject/pending). VERSIONADO (rechaza revisión stale, FR-004). Audita la
/// acción (append-only; revertir = nueva acción). Devuelve la nueva revisión.
#[tauri::command]
pub fn review_hunk_decide(
    state: State<'_, AppState>,
    group_id: String,
    hunk_id: String,
    decision: String,
    expected_revision: u64,
    rationale: Option<String>,
) -> Result<u64, String> {
    use crate::services::review;
    use crate::services::review_audit::{self, ReviewAction, ReviewAuditEntry, ReviewTargetLink};
    let new_state = review::HunkState::from_str(&decision).map_err(|e| e.to_string())?;
    let saved = review::save_decision(&state.db, &group_id, &hunk_id, new_state, expected_revision)
        .map_err(|e| e.to_string())?;
    let rev = saved.revision;
    // 019 F0 · T001 — audit append-only ESTRUCTURADO con vínculo audit↔change-set/hunk (R2).
    // approve/reject/revert son acciones distintas; la transición se deriva de (previous, new)
    // (audit-3 L3): un hunk que YA estaba Pending y se re-decide a Pending NO es un "revert" (no
    // venía de una decisión previa) → no se audita como Revert espurio. Revertir-a-pending SÓLO
    // cuenta como `revert` si venía de Approved/Rejected. Cada acción es un evento nuevo (el
    // histórico append-only no se muta).
    let action = match (saved.previous, new_state) {
        (_, review::HunkState::Approved) => Some(ReviewAction::Approve),
        (_, review::HunkState::Rejected) => Some(ReviewAction::Reject),
        // → Pending: sólo es Revert si DESHACE una decisión previa (Approved/Rejected → Pending).
        (review::HunkState::Approved, review::HunkState::Pending)
        | (review::HunkState::Rejected, review::HunkState::Pending) => Some(ReviewAction::Revert),
        // Pending → Pending: no-op semántico (no había decisión que revertir) → no se audita.
        (review::HunkState::Pending, review::HunkState::Pending) => None,
    };
    let rationale = rationale.unwrap_or_default();
    if let Some(action) = action {
        let _ = review_audit::record(
            &state.db,
            &state.audit,
            ReviewAuditEntry {
                action,
                actor: &crate::services::identity::current_actor(),
                target: &hunk_id,
                rationale: &rationale,
                link: ReviewTargetLink {
                    group_id: Some(group_id.clone()),
                    hunk_id: Some(hunk_id.clone()),
                    approval_id: None,
                    revision: Some(rev),
                },
            },
        );
    }
    Ok(rev)
}

/// Conflictos actuales entre hunks aprobados de variantes distintas (R3). Read-only; el front los
/// muestra y exige resolución manual ANTES de habilitar apply.
#[tauri::command]
pub fn review_conflicts(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<crate::services::review::Conflict>, String> {
    use crate::services::review;
    match review::load_group(&state.db, &group_id).map_err(|e| e.to_string())? {
        Some(g) => Ok(g.detect_conflicts()),
        None => Ok(vec![]),
    }
}

/// Aplica los hunks APROBADOS al working copy del repo principal. DESTRUCTIVE → pasa por el gate
/// universal (aprobación). Salvaguardas (audit-3 codex/deepseek/AIE):
///   - `expected_revision`: rechaza si la review cambió desde que el usuario la vio (stale apply,
///     multi-ventana). Aplica EXACTAMENTE el set que confirmó.
///   - Patch construido desde el SNAPSHOT persistido del cuerpo (`load_approved_with_bodies`), NUNCA
///     re-derivado del worktree vivo (que pudo cambiar el cuerpo del mismo hunk).
///   - R3: rechaza si hay conflictos sin resolver (NUNCA auto-merge).
///   - Precondición de estado: working copy LIMPIO y `HEAD == base` del grupo (sino el patch caería
///     sobre contexto/rama equivocada). `git apply` all-or-nothing (sin estado parcial). NUNCA mata procesos.
#[tauri::command]
pub fn review_apply(
    state: State<'_, AppState>,
    group_id: String,
    expected_revision: u64,
) -> Result<serde_json::Value, String> {
    use crate::services::review;
    use std::io::Write;
    use std::process::{Command, Stdio};

    let g = review::load_group(&state.db, &group_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "review no abierta para este grupo".to_string())?;
    // STALE APPLY (codex #2/B3): la revisión que el usuario confirmó debe seguir vigente.
    if g.revision != expected_revision {
        return Err(format!(
            "la review cambió (revisión {} ≠ {} que confirmaste) — recargá y revisá de nuevo antes de aplicar",
            g.revision, expected_revision
        ));
    }
    // R3: conflictos sin resolver → NO aplicar (resolución manual obligatoria).
    let conflicts = g.detect_conflicts();
    if !conflicts.is_empty() {
        return Err(format!(
            "hay {} conflicto(s) entre hunks aprobados — resolvé manualmente antes de aplicar",
            conflicts.len()
        ));
    }
    // SNAPSHOT (codex #1): el patch sale de los cuerpos PERSISTIDOS al abrir la review, no del worktree
    // vivo → se aplica exactamente lo aprobado aunque la variante haya cambiado después.
    let approved =
        review::load_approved_with_bodies(&state.db, &group_id).map_err(|e| e.to_string())?;
    if approved.is_empty() {
        return Ok(serde_json::json!({"applied": false, "reason": "no hay hunks aprobados"}));
    }
    let patch = review::build_patch_from_hunks(&approved);
    if patch.trim().is_empty() {
        return Ok(serde_json::json!({"applied": false, "reason": "patch vacío"}));
    }

    let base = orch_svc::group_base(&state.db, &group_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "el grupo no tiene base — no se puede aplicar con seguridad".to_string())?;
    let tasks = orch_svc::list_group_tasks(&state.db, &group_id).map_err(|e| e.to_string())?;
    let repo_path = tasks
        .iter()
        .map(|t| t.repo_path.clone())
        .next()
        .ok_or_else(|| "grupo sin tareas/repo".to_string())?;

    let git = |args: &[&str]| -> Result<std::process::Output, String> {
        Command::new("git")
            .args(["-C", &repo_path])
            .args(args)
            .output()
            .map_err(|e| e.to_string())
    };
    // Precondición 1 (codex #3/B1): working copy LIMPIO (sino podríamos mezclar cambios del usuario).
    let status = git(&["status", "--porcelain=v1"])?;
    if !status.status.success() {
        return Err("no se pudo leer git status del repo".to_string());
    }
    if !String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return Err("el working copy del repo tiene cambios sin commitear — commiteá o stasheá antes de aplicar".to_string());
    }
    // Precondición 2: HEAD == base del grupo (el patch fue derivado contra ese base).
    let head = git(&["rev-parse", "HEAD"])?;
    let base_commit = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{base}^{{commit}}"),
    ])?;
    if !head.status.success() || !base_commit.status.success() {
        return Err(format!("no se pudo resolver HEAD o el base '{base}'"));
    }
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    let base_commit = String::from_utf8_lossy(&base_commit.stdout)
        .trim()
        .to_string();
    if head != base_commit {
        return Err(format!(
            "el repo no está en el base de la review (HEAD {} ≠ base {}) — hacé checkout del base antes de aplicar",
            &head[..head.len().min(8)], &base_commit[..base_commit.len().min(8)]
        ));
    }

    // git apply al working copy (all-or-nothing; sin --reject → no aplicación parcial).
    let mut child = Command::new("git")
        .args(["-C", &repo_path, "apply", "--whitespace=nowarn", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn git apply: {e}"))?;
    child
        .stdin
        .as_mut()
        .ok_or("git apply sin stdin")?
        .write_all(patch.as_bytes())
        .map_err(|e| format!("escribir patch: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git apply: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git apply falló: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // 019 F0 · T001 — audit append-only estructurado con vínculo audit↔change-set (R2).
    {
        use crate::services::review_audit::{
            self, ReviewAction, ReviewAuditEntry, ReviewTargetLink,
        };
        let target = format!("{group_id} ({} hunks)", approved.len());
        let _ = review_audit::record(
            &state.db,
            &state.audit,
            ReviewAuditEntry {
                action: ReviewAction::Apply,
                actor: &crate::services::identity::current_actor(),
                target: &target,
                rationale: "apply approved hunks to working copy",
                link: ReviewTargetLink {
                    group_id: Some(group_id.clone()),
                    hunk_id: None,
                    approval_id: None,
                    revision: Some(g.revision),
                },
            },
        );
    }
    // 026 F0 (US1) — captura de la señal de preferencia. INOCUO: deriva del estado FINAL de la review
    // (ya auditado) un PreferenceRecord append-only + actualiza el prior local. Gateado por
    // `preference.record_enabled` (default ON). Best-effort: un fallo NUNCA rompe el apply (que ya
    // tuvo éxito) — sólo se loguea. CERO código crudo de diffs en el record (FR-005/SC-008).
    capture_preference_signal(&state, &group_id, &base, &tasks);
    Ok(
        serde_json::json!({"applied": true, "approved_hunks": approved.len(), "patch_bytes": patch.len(), "revision": g.revision}),
    )
}

/// 026 F0 (US1) — deriva y persiste el `PreferenceRecord` del estado final de una review de
/// best-of-N. Best-effort + gateado por `preference.record_enabled`: si está OFF o falla, no hace
/// nada (NUNCA propaga error al caller, que ya completó su acción). Construye los `VariantInput` a
/// partir de las tareas del grupo: diff unificado vs el base (para diff-stat/risky-paths — el texto
/// NO se persiste) + la evidencia del quality-gate cacheada (si se corrió; ausente = no medido).
fn capture_preference_signal(
    state: &State<'_, AppState>,
    group_id: &str,
    base: &str,
    tasks: &[crate::services::orchestration::OrchTask],
) {
    use crate::services::preference_signal as pref;
    use crate::services::variant_features::QualityGateInput;
    if !pref::record_enabled(&state.db) {
        return; // registro OFF (opt-out).
    }
    // repo del grupo (para el repo_key scrubbeado — hash, no ruta absoluta).
    let Some(repo_path) = tasks.first().map(|t| t.repo_path.clone()) else {
        return;
    };
    // quality-gate cacheado del grupo (si se corrió). Map task_id → (errors, warnings, any_measured).
    let qg_by_task: std::collections::HashMap<String, QualityGateInput> = state
        .quality_gate
        .lock()
        .get(group_id)
        .map(|evs| {
            evs.iter()
                .map(|e| {
                    (
                        e.task_id.clone(),
                        QualityGateInput {
                            errors: e.total_errors,
                            warnings: e.total_warnings,
                            any_measured: e.any_measured,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Construir los inputs de variante. Diff unificado vs base (mismo origen que `review_open`); el
    // texto del diff SÓLO alimenta el cómputo de features y NUNCA se persiste.
    let inputs: Vec<pref::VariantInput> = tasks
        .iter()
        .filter_map(|t| {
            t.worktree_path.as_deref().map(|wt| pref::VariantInput {
                task_id: t.id.clone(),
                agent_profile_id: t.agent_profile_id.clone(),
                diff: orch_svc::collect_unified_diff(wt, Some(base)),
                quality_gate: qg_by_task.get(&t.id).copied(),
            })
        })
        .collect();
    if inputs.len() < 2 {
        // best-of-1 / sin variantes lanzadas: no hay preferencia comparativa que aprender (edge case
        // de la spec) → no registramos un record sin señal de ranking.
        return;
    }
    // task_type: bucket de contexto. Sin clasificador local determinista todavía → "unknown" (válido
    // como contexto; el prior agrupa por (repo_key, task_type)). La clasificación AIE es opt-in y
    // agregaría red a un path que debe quedar inocuo y rápido.
    let task_type = "unknown";
    let risky = pref::risky_paths_override(&state.db);
    match pref::capture_from_review(
        &state.db,
        group_id,
        &repo_path,
        task_type,
        &inputs,
        risky.as_deref(),
    ) {
        Ok(_rec) => {
            let _ = state.audit.write(EventInput {
                kind: "preference.recorded",
                actor: &crate::services::identity::current_actor(),
                pane_id: None,
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({
                    "group_id": group_id,
                    "variants": inputs.len(),
                    "task_type": task_type,
                }),
            });
        }
        Err(e) => {
            tracing::debug!("preference signal capture skipped: {e}");
        }
    }
}

// ── 019 T024 — retención + exportabilidad del audit del flujo review (FR-005) ──
//
// EXPORT-THEN-ROTATE, SIN DELETE físico (council 3-frontera, Opción B). El audit (`events` +
// `review_audit_links`) es APPEND-ONLY e INMUTABLE; estos comandos exportan/sellan a archivo y NUNCA
// borran filas. F-I/BYOK: los payloads son ids/estados/rationale — NUNCA secretos.

/// Política de retención por defecto. Conservadora: 365 días por edad, sin tope por cantidad. El
/// audit NO se purga in-place; este corte sólo define qué cae "fuera de ventana" (candidato a
/// export+rotación a archivo). Cuando exista UI de configuración, esto se leerá de settings.
fn default_retention_policy() -> crate::services::audit_retention::RetentionPolicy {
    crate::services::audit_retention::RetentionPolicy {
        max_age_days: Some(365),
        max_events: None,
    }
}

/// HIGH-2: directorio CONTROLADO y ÚNICO donde viven TODAS las salidas de audit. Confina los
/// archivos a `~/.furx/audit-exports/`. Se crea si no existe. Es la base del allowlist contra
/// path traversal / symlink escape — ningún comando debe escribir fuera de acá.
fn audit_exports_dir() -> Result<std::path::PathBuf, String> {
    let dir = dirs::home_dir()
        .ok_or("no home dir")?
        .join(".furx")
        .join("audit-exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear audit-exports dir: {e}"))?;
    Ok(dir)
}

/// HIGH-2: confina un sub-path RELATIVO provisto por el caller DENTRO de `audit_exports_dir()`.
/// Reglas (fail-closed):
///   - rechaza paths ABSOLUTOS, componentes `..` (ParentDir), prefijos (drive/UNC en Windows) y
///     RootDir → cualquier intento de escapar la base.
///   - cada componente debe ser un nombre normal NO vacío, sin separadores de path (los separadores
///     ya los descompone `Path::components`).
///   - canonicaliza la BASE (resuelve symlinks de la base ya existente) y, para el destino,
///     canonicaliza el PRIMER ANCESTRO EXISTENTE de la ruta final (subiendo por la cadena de
///     ancestros, incluido el propio `candidate` si ya existe como symlink) verificando que ese
///     ancestro canónico quede DENTRO de la base canónica → un symlink que apunte afuera (sea el
///     subdir mismo o un ancestro intermedio) es rechazado. El re-chequeo post-create (tras
///     `create_dir_all`) lo hace el caller (`confirm_within_base`).
/// Devuelve la ruta final (base ⊕ subpath sanitizado), garantizada dentro del allowlist.
fn confined_subpath(base: &std::path::Path, rel: &str) -> Result<std::path::PathBuf, String> {
    use std::path::Component;
    let rel_path = std::path::Path::new(rel);
    let mut sanitized = std::path::PathBuf::new();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_str().ok_or("componente de path no-UTF8")?;
                if s.is_empty() {
                    return Err("componente de path vacío".into());
                }
                sanitized.push(s);
            }
            Component::CurDir => { /* '.' inofensivo, ignorar */ }
            Component::ParentDir => {
                return Err("path traversal rechazado: '..' no permitido".into());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("path absoluto rechazado: debe ser relativo a audit-exports".into());
            }
        }
    }
    if sanitized.as_os_str().is_empty() {
        return Err("subpath vacío".into());
    }
    // Canonicalizar la base (existe siempre — la crea audit_exports_dir). Resuelve symlinks de la base.
    let base_canon = base
        .canonicalize()
        .map_err(|e| format!("no se pudo canonicalizar audit-exports dir: {e}"))?;
    let candidate = base_canon.join(&sanitized);

    // HIGH-2: subir por la cadena de ancestros del destino (incluido el propio `candidate`) hasta el
    // PRIMER ancestro que YA exista en disco; canonicalizarlo (resuelve cualquier symlink, sea el
    // subdir final o un ancestro intermedio) y verificar que cae dentro de la base canónica. Cubre
    // los dos escapes: (1) `candidate` mismo es un symlink afuera; (2) `link/nuevo` donde `link` es
    // symlink afuera y `link/nuevo` aún no existe. `base_canon` siempre existe y es su propio
    // canónico → el bucle termina como muy tarde ahí.
    let mut existing = candidate.as_path();
    loop {
        // `symlink_metadata` NO sigue el symlink: detecta el propio `candidate`/ancestro symlink.
        if existing.symlink_metadata().is_ok() {
            let canon = existing
                .canonicalize()
                .map_err(|e| format!("no se pudo canonicalizar ancestro del destino: {e}"))?;
            if !canon.starts_with(&base_canon) {
                return Err("destino fuera del directorio permitido (symlink escape)".into());
            }
            break;
        }
        match existing.parent() {
            Some(p) => existing = p,
            None => break,
        }
    }
    Ok(candidate)
}

/// HIGH-2 (defensa post-create): tras `create_dir_all` del directorio destino re-canonicaliza el
/// directorio FINAL y reconfirma que sigue dentro de `base` canónica. Cierra la ventana TOCTOU entre
/// la validación de `confined_subpath` y el `create_dir_all`: si un atacante plantó un symlink en esa
/// ventana, el `dir` ya existente resuelve afuera y se rechaza ANTES de escribir el archivo.
fn confirm_within_base(base: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    let base_canon = base
        .canonicalize()
        .map_err(|e| format!("no se pudo canonicalizar audit-exports dir: {e}"))?;
    let dir_canon = dir
        .canonicalize()
        .map_err(|e| format!("no se pudo canonicalizar el directorio de rotación: {e}"))?;
    if !dir_canon.starts_with(&base_canon) {
        return Err("destino fuera del directorio permitido (symlink plantado post-create)".into());
    }
    Ok(())
}

#[cfg(test)]
mod confined_subpath_tests {
    use super::{confined_subpath, confirm_within_base};
    use std::path::{Path, PathBuf};

    /// Tempdir único por proceso (evita colisión bajo `cargo test` concurrente — ver gotcha keychain).
    fn fresh_base(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "furx-confined-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Crea un symlink; si el entorno no lo permite (sandbox), devuelve false para SKIPEAR (no fallar).
    #[cfg(unix)]
    fn try_symlink(src: &Path, dst: &Path) -> bool {
        std::os::unix::fs::symlink(src, dst).is_ok()
    }
    /// Windows: los symlinks requieren privilegio elevado; este test de traversal degrada a skip.
    #[cfg(not(unix))]
    fn try_symlink(_src: &Path, _dst: &Path) -> bool {
        false
    }

    #[test]
    fn rejects_parent_traversal_and_absolute() {
        let base = fresh_base("trav");
        assert!(confined_subpath(&base, "../escape").is_err());
        assert!(confined_subpath(&base, "/etc/passwd").is_err());
        assert!(confined_subpath(&base, "a/../../b").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn accepts_legit_nested_new_subdir() {
        let base = fresh_base("legit");
        // (c) subdir legítimo anidado nuevo → aceptado y queda dentro de la base.
        let out = confined_subpath(&base, "nivel1/nivel2").expect("subdir anidado legítimo");
        let base_canon = base.canonicalize().unwrap();
        assert!(out.starts_with(&base_canon), "{out:?} debe estar bajo {base_canon:?}");
        // create_dir_all + confirm_within_base deben pasar.
        std::fs::create_dir_all(&out).unwrap();
        confirm_within_base(&base, &out).expect("dir legítimo confirmado dentro de la base");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_subdir_that_is_symlink_outside() {
        let base = fresh_base("symdir");
        let outside = std::env::temp_dir().join(format!(
            "furx-confined-outside-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        // (a) `link` es un symlink afuera de la base.
        let link = base.join("link");
        if !try_symlink(&outside, &link) {
            eprintln!("SKIP rejects_subdir_that_is_symlink_outside: el entorno no permite symlinks");
            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        let res = confined_subpath(&base, "link");
        assert!(res.is_err(), "un subdir que es symlink afuera debe rechazarse, dio {res:?}");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn rejects_intermediate_symlink_ancestor() {
        let base = fresh_base("syminter");
        let outside = std::env::temp_dir().join(format!(
            "furx-confined-outside2-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        // (b) `link` symlink afuera, `link/sub` aún NO existe → el ancestro existente (link) resuelve
        // fuera de la base → rechazado.
        let link = base.join("link");
        if !try_symlink(&outside, &link) {
            eprintln!("SKIP rejects_intermediate_symlink_ancestor: el entorno no permite symlinks");
            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        let res = confined_subpath(&base, "link/sub");
        assert!(res.is_err(), "ancestro intermedio symlink afuera debe rechazarse, dio {res:?}");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn confirm_within_base_rejects_post_create_symlink() {
        let base = fresh_base("postcreate");
        let outside = std::env::temp_dir().join(format!(
            "furx-confined-post-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        // Simula el symlink plantado en la ventana TOCTOU: el `dir` ya resuelto apunta afuera.
        let link = base.join("planted");
        if !try_symlink(&outside, &link) {
            eprintln!("SKIP confirm_within_base_rejects_post_create_symlink: sin symlinks");
            let _ = std::fs::remove_dir_all(&base);
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }
        assert!(
            confirm_within_base(&base, &link).is_err(),
            "confirm_within_base debe rechazar un dir que resuelve fuera de la base"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&outside);
    }
}

/// EXPORT del audit del flujo review a un archivo verificable. TOCA DISCO (escribe el export) → pasa
/// por el gate universal (Risk::Destructive en el registry). `format` = "ndjson"|"csv"; `scope` =
/// "all" (todo) | "out_of_window" (sólo el segmento más viejo que el corte de la política activa).
/// El archivo se escribe bajo `~/.furx/audit-exports/` con nombre con timestamp. Devuelve el manifest
/// SELLADO + el flag `verified` (round-trip: re-leer archivo → contar + re-hashear == manifest).
#[tauri::command]
pub fn audit_export(
    state: State<'_, AppState>,
    format: String,
    scope: String,
) -> Result<serde_json::Value, String> {
    use crate::services::audit_retention::{self as ar, ExportFormat, ExportScope};
    let fmt = ExportFormat::parse(&format).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().naive_utc();
    let policy = default_retention_policy();
    let export_scope = match scope.as_str() {
        "all" => ExportScope::All,
        "out_of_window" => {
            let cutoff = policy.cutoff_ts(now).ok_or_else(|| {
                "la política activa no define max_age_days — no hay segmento fuera de ventana"
                    .to_string()
            })?;
            ExportScope::OutOfWindow { cutoff_ts: cutoff }
        }
        other => return Err(format!("scope desconocido: {other} (esperado all|out_of_window)")),
    };

    let dir = audit_exports_dir()?;
    // HIGH-1: nombre ÚNICO (timestamp + uuid) → dos exports en el mismo segundo NUNCA colisionan ni
    // se pisan (export_audit usa create_new como red de seguridad). El uuid lo hace no-adivinable.
    let fname = format!(
        "audit-export-{}-{}.{}",
        now.format("%Y%m%dT%H%M%S"),
        uuid::Uuid::new_v4().simple(),
        fmt.as_str()
    );
    let out_path = dir.join(fname);

    // HIGH-2/T024: `dir` (= audit_exports_dir) es la base confinada; export_audit revalida el parent
    // real PEGADO a la escritura (canonicalize + starts_with) antes de abrir el archivo.
    let manifest =
        ar::export_audit(&state.db, export_scope, fmt, &out_path, &dir).map_err(|e| e.to_string())?;
    let verified = ar::verify_export(&manifest, &out_path).map_err(|e| e.to_string())?;
    serde_json::to_value(serde_json::json!({ "manifest": manifest, "verified": verified }))
        .map_err(|e| e.to_string())
}

/// Estado de retención (read-only): política activa + nº total de filas de audit + nº fuera de
/// ventana + último manifest sellado. Risk::Safe (no toca disco ni muta nada).
#[tauri::command]
pub fn audit_retention_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    use crate::services::audit_retention as ar;
    let now = chrono::Utc::now().naive_utc();
    let policy = default_retention_policy();
    let status = ar::retention_status(&state.db, &policy, now).map_err(|e| e.to_string())?;
    serde_json::to_value(status).map_err(|e| e.to_string())
}

/// ROTACIÓN del segmento fuera-de-ventana: exporta+sella las filas fuera de ventana de la política
/// activa (edad Y/O cantidad) y registra un evento NUEVO + manifest kind=rotation. NO borra filas
/// (export-then-rotate). TOCA DISCO → gate universal (Risk::Destructive). Devuelve el receipt.
///
/// HIGH-2: la salida SIEMPRE queda confinada a `~/.furx/audit-exports/`. `subdir` es OPCIONAL y, si
/// se provee, se sanitiza como sub-componente RELATIVO (rechaza `..`, paths absolutos, symlink
/// escape) vía `confined_subpath`. NUNCA se acepta un path crudo del invoke como destino.
#[tauri::command]
pub fn audit_rotate(
    state: State<'_, AppState>,
    subdir: Option<String>,
) -> Result<serde_json::Value, String> {
    use crate::services::audit_retention as ar;
    let now = chrono::Utc::now().naive_utc();
    let policy = default_retention_policy();
    let base = audit_exports_dir()?;
    let dir = match subdir {
        Some(s) if !s.trim().is_empty() => confined_subpath(&base, &s)?,
        _ => base.clone(),
    };
    // Asegurar que el dir destino existe (confined ya verificó que está dentro del allowlist).
    std::fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear el dir de rotación: {e}"))?;
    // HIGH-2: re-canonicalizar el dir final tras crearlo y reconfirmar que sigue dentro de la base —
    // defensa contra un symlink plantado en la ventana entre confined_subpath y create_dir_all.
    confirm_within_base(&base, &dir)?;
    // HIGH-2/T024: `base` es la base confinada permitida; rotate_segment la propaga a export_audit
    // para revalidar el parent real PEGADO a la escritura (defensa en capas + cierre del TOCTOU residual).
    let receipt = ar::rotate_segment(&state.db, &state.audit, &policy, now, &dir, &base)
        .map_err(|e| e.to_string())?;
    serde_json::to_value(receipt).map_err(|e| e.to_string())
}

// ── 020-aie-meta-orchestrator — US2/US3 advisory commands (opt-in OFF por default) ──
//
// El AIE free ($0, Tailscale) refina meta-decisiones del orquestador. AMBOS comandos son
// ADVISORY: NUNCA mutan estado de tarea ni eligen variante; sólo devuelven una sugerencia.
// Gate por el setting `orchestration.use_aie_for_meta` (default OFF, reusa `dd_svc::aie_meta_enabled`):
// con OFF devuelven `Ok(None)` sin tocar la red → comportamiento actual intacto (SC-001). El engine
// (`meta_decision`) sanitiza los diffs/objetivo CRUDOS internamente, cachea, audita y NUNCA propaga
// `Err` (todo fallo ⇒ `None`, FR-003). Los `build_meta_engine`/gate viven en done_detection.rs (US1).

/// US2 (P2) — ranking best-of-N (advisory). Dado un `group_id`, junta las N variantes + sus diffs
/// (working tree de cada variante; fallback a la review de su branch vs base para las no lanzadas —
/// mismo origen de diff que `orchestration_compare_group`) y pide al AIE un orden sugerido de mejor
/// a peor. Devuelve `Some(Vec<usize>)` con índices en el ORDEN DE LAS VARIANTES (variant_index ASC,
/// igual que `list_group_tasks`), o `None` si el feature está OFF / no hay diffs / el AIE falla.
/// Es SUGERENCIA: el picker manual (`orchestration_choose_variant`) sigue mandando.
#[tauri::command]
pub async fn meta_suggest_variant_ranking(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<Vec<usize>>, String> {
    // Gate: ningún motor meta ON (default) → sin red, comportamiento actual (SC-001). 036: habilita
    // si el motor LOCAL o el AIE está ON (`select_meta_engine_kind`); `build_meta_engine` elige cuál.
    if dd_svc::select_meta_engine_kind(&state.db) == dd_svc::MetaEngineKind::Heuristic {
        return Ok(None);
    }
    // Infalibilidad advisory (audit finding #5): CUALQUIER error de infraestructura previo al
    // engine (DB / git) degrada a `Ok(None)` — NUNCA propaga `Err` al frontend (este comando es
    // sólo una sugerencia opcional; un fallo de lectura no debe romper la UI). Sólo el engine,
    // que ya devuelve `None` solo, decide la sugerencia final.
    // Variantes en orden estable (variant_index ASC) — el ranking indexa contra ESTE orden.
    let Ok(tasks) = orch_svc::list_group_tasks(&state.db, &group_id) else {
        return Ok(None);
    };
    if tasks.is_empty() {
        return Ok(None);
    }
    // Objetivo común del grupo (cae al objective de la 1ª variante si el grupo desapareció). Un
    // error de DB en get_group degrada a None (advisory).
    let objective = match orch_svc::get_group(&state.db, &group_id) {
        Ok(g) => g
            .map(|g| g.objective)
            .or_else(|| tasks.first().map(|t| t.objective.clone()))
            .unwrap_or_default(),
        Err(_) => return Ok(None),
    };
    // Diff CRUDO por variante (el engine sanitiza internamente — NO re-sanitizar acá). Mismo origen
    // que orchestration_compare_group: working tree si está lanzada, sino la review de la branch.
    // `collect_diff` usa `GIT_OPTIONAL_LOCKS=0` (read-only, no toma index.lock — finding #4); un
    // Err de worktree_merge_review degrada a placeholder (advisory, finding #5).
    let diffs: Vec<String> = tasks
        .iter()
        .map(|t| match t.worktree_path.as_deref() {
            Some(wt) => orch_svc::collect_diff(wt),
            None => match worktree_merge_review(t.repo_path.clone(), t.branch.clone()) {
                Ok(r) => r.diff_stat,
                Err(_) => "(la variante aún no se lanzó)".to_string(),
            },
        })
        .collect();
    // Finding #1: NO consultar el AIE sin contenido útil. Si el objetivo (trim) está vacío O
    // NINGUNA variante tiene un diff REAL (los placeholders "(sin cambios)" / "(la variante aún no
    // se lanzó)" / vacío NO cuentan), no hay nada que rankear → `Ok(None)` SIN tocar el engine/red.
    if objective.trim().is_empty() || !diffs.iter().any(|d| diff_has_real_content(d)) {
        return Ok(None);
    }
    let engine = dd_svc::build_meta_engine(&state.db, &state.audit);
    // El engine ya audita la consulta (append-only, sin contenido) y nunca propaga Err.
    Ok(engine.rank_variants(&objective, &diffs).await)
}

/// ¿Tiene este diff contenido REAL (vs un placeholder)? Los strings que `collect_diff` /
/// `worktree_merge_review` devuelven cuando no hay cambios o la variante no se lanzó NO cuentan
/// como diff (finding #1: no llamar al AIE sin contenido útil).
fn diff_has_real_content(diff: &str) -> bool {
    let t = diff.trim();
    !t.is_empty() && t != "(sin cambios)" && t != "(la variante aún no se lanzó)"
}

/// 026 F1 (US2) — ranking best-of-N ENRIQUECIDO con el prior local explicable (advisory). Combina
/// el ranking del AIE de 020 (`meta_suggest_variant_ranking`) con el prior aprendido del contexto
/// `(repo_key, task_type)` como un feature más (0.85*AIE + 0.15*prior), y devuelve además la
/// EXPLICACIÓN legible de cada sugerencia (FR-023). SIEMPRE advisory (FR-024): NUNCA muta estado ni
/// auto-elige; el picker manual sigue mandando. Degradación:
///   - `preference.inject` OFF (default) ⇒ devuelve el ranking de 020 sin tocar (cero regresión,
///     SC-002) con `inject_disabled = true`.
///   - prior en cold-start (<15 muestras) ⇒ no inyecta, `still_learning = true`.
///   - AIE caído + prior frío ⇒ `Ok(None)` (picker manual, invariante de 020 preservado, SC-005).
#[tauri::command]
pub async fn meta_suggest_variant_ranking_explained(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<crate::services::preference_signal::RankingExplanation>, String> {
    use crate::services::preference_signal as pref;
    use crate::services::variant_features::{self as vfeat, QualityGateInput};

    // 1) base_order: el ranking advisory de 020 (Some sólo si el feature AIE-meta está ON y produjo
    //    una permutación válida). NUNCA propaga Err (advisory). Se computa primero porque reusa el
    //    mismo gating/data-gathering ya endurecido.
    let base_order = meta_suggest_variant_ranking(state.clone(), group_id.clone())
        .await
        .unwrap_or(None);

    // 2) Variantes en orden estable (variant_index ASC) — el ranking indexa contra ESTE orden.
    let Ok(tasks) = orch_svc::list_group_tasks(&state.db, &group_id) else {
        return Ok(None);
    };
    if tasks.is_empty() {
        return Ok(None);
    }
    let Some(base) = orch_svc::group_base(&state.db, &group_id).ok().flatten() else {
        // sin base no podemos colectar el unified diff confiable → sin features → degradamos al
        // base_order de 020 si existe (advisory), sino None.
        return Ok(base_order.map(|order| simple_ranking_explanation(&order, tasks.len())));
    };
    let Some(repo_path) = tasks.first().map(|t| t.repo_path.clone()) else {
        return Ok(None);
    };

    // 3) quality-gate cacheado (ausente = no medido, FR-012).
    let qg_by_task: std::collections::HashMap<String, QualityGateInput> = state
        .quality_gate
        .lock()
        .get(&group_id)
        .map(|evs| {
            evs.iter()
                .map(|e| {
                    (
                        e.task_id.clone(),
                        QualityGateInput {
                            errors: e.total_errors,
                            warnings: e.total_warnings,
                            any_measured: e.any_measured,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // 4) features por variante (puras sobre el diff; el texto NO sale del proceso).
    let risky = pref::risky_paths_override(&state.db);
    let variants: Vec<vfeat::VariantFeatures> = tasks
        .iter()
        .map(|t| {
            let diff = t
                .worktree_path
                .as_deref()
                .map(|wt| orch_svc::collect_unified_diff(wt, Some(&base)))
                .unwrap_or_default();
            vfeat::compute_features(
                &t.id,
                t.agent_profile_id.clone(),
                &diff,
                qg_by_task.get(&t.id).copied(),
                risky.as_deref(),
            )
        })
        .collect();

    // 5) prior del contexto + setting de inyección (opt-in, default OFF).
    let task_type = "unknown";
    let repo_key = pref::repo_key_for(&repo_path);
    let prior = pref::load_prior(&state.db, &repo_key, task_type).map_err(|e| e.to_string())?;
    let inject = pref::inject_enabled(&state.db);

    // 6) combinar (PURO, advisory). `None` ⇒ picker manual (sin base de 020 ni prior caliente).
    Ok(pref::rank_with_prior(
        base_order.as_deref(),
        &variants,
        &prior,
        inject,
    ))
}

/// Construye una `RankingExplanation` "passthrough" desde un orden de 020 cuando no podemos computar
/// features (p.ej. sin base): refleja exactamente el ranking del AIE, sin inyección de prior.
fn simple_ranking_explanation(
    order: &[usize],
    n: usize,
) -> crate::services::preference_signal::RankingExplanation {
    use crate::services::preference_signal::{RankingExplanation, VariantExplanation};
    let mut base_score = vec![0.5f64; n];
    for (rank, &idx) in order.iter().enumerate() {
        if idx < n {
            base_score[idx] = if n > 1 {
                1.0 - (rank as f64) / ((n - 1) as f64)
            } else {
                1.0
            };
        }
    }
    let variants = (0..n)
        .map(|i| VariantExplanation {
            task_id: String::new(),
            combined_score: base_score[i],
            base_score: base_score[i],
            prior_score: 0.5,
            factors: Vec::new(),
        })
        .collect();
    RankingExplanation {
        order: order.to_vec(),
        variants,
        still_learning: true,
        inject_disabled: true,
    }
}

/// US3 (P3) — sugerencia de agente / clasificación de tarea (advisory). Dado un `objective` (texto),
/// pide al AIE que clasifique la tarea (bugfix/feature/refactor/docs/test/chore) y devuelve esa
/// categoría como `Some(String)`, o `None` si el feature está OFF / el AIE falla. Es OPCIONAL y
/// descartable: el user sigue eligiendo el `agent_profile_id` a mano. NO hay un registry de perfiles
/// por categoría todavía, así que devolvemos la categoría cruda (el front la mapea a un perfil sugerido).
#[tauri::command]
pub async fn meta_suggest_agent(
    state: State<'_, AppState>,
    objective: String,
) -> Result<Option<String>, String> {
    // 036: habilita si el motor LOCAL o el AIE está ON; con ambos OFF (default) → Ok(None) sin red.
    if dd_svc::select_meta_engine_kind(&state.db) == dd_svc::MetaEngineKind::Heuristic {
        return Ok(None);
    }
    // Finding #1: no consultar el motor sin contenido útil — objetivo vacío ⇒ Ok(None) sin red.
    if objective.trim().is_empty() {
        return Ok(None);
    }
    let engine = dd_svc::build_meta_engine(&state.db, &state.audit);
    // El engine sanitiza el objetivo crudo internamente, audita la consulta y nunca propaga Err.
    Ok(engine.classify_task(&objective).await)
}

// ── 026 F1/US3 — gobierno del prior: inspección + reset + records ──────────────────────────────
//
// Transparencia obligatoria (FR-030): el usuario ve QUÉ aprendió el prior (features, peso, dirección,
// muestras) y puede RESETEARLO (FR-031, auditado). El prior es 100% local/determinista; estos
// comandos son read-only (inspect/list) o un reset auditado.

/// Vista inspeccionable de UN feature del prior (FR-030): peso aprendido ∈ [-1,1], dirección legible,
/// y la evidencia Beta que lo respalda (alpha/beta/distinct_obs). Explicable, NUNCA opaco.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PriorFeatureView {
    pub feature_key: String,
    /// Peso aprendido ∈ [-1,1]: signo = dirección, magnitud = fuerza.
    pub weight: f64,
    /// "menos es mejor" | "más es mejor" | "neutro".
    pub direction: String,
    pub alpha: f64,
    pub beta: f64,
    pub distinct_obs: i64,
}

/// La vista inspeccionable del prior de un contexto (FR-030).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PriorView {
    pub repo_key: String,
    pub task_type: String,
    pub sample_count: i64,
    /// ¿Superó el cold-start (≥15 muestras + diversidad)? Si no, no se inyecta (still learning).
    pub is_warm: bool,
    pub features: Vec<PriorFeatureView>,
}

/// 026 F1/US3 (FR-030) — inspecciona el prior aprendido del contexto `(repo_path, task_type)`:
/// construido desde `context_priors` (ya alimentado por `list_records` históricos). Read-only. Muestra
/// qué features preferís, su dirección/peso, cuántas muestras lo respaldan, y si está caliente.
#[tauri::command]
pub fn preference_prior_inspect(
    state: State<'_, AppState>,
    repo_path: String,
    task_type: Option<String>,
) -> Result<PriorView, String> {
    use crate::services::preference_signal as pref;
    let task_type = task_type.unwrap_or_else(|| "unknown".to_string());
    let repo_key = pref::repo_key_for(&repo_path);
    let prior = pref::load_prior(&state.db, &repo_key, &task_type).map_err(|e| e.to_string())?;
    let is_warm = prior.is_warm();
    let features = prior
        .features
        .iter()
        .map(|(key, fb)| {
            let weight = fb.weight();
            let direction = if weight < -1e-6 {
                "menos es mejor"
            } else if weight > 1e-6 {
                "más es mejor"
            } else {
                "neutro"
            };
            PriorFeatureView {
                feature_key: key.clone(),
                weight,
                direction: direction.to_string(),
                alpha: fb.alpha,
                beta: fb.beta,
                distinct_obs: fb.distinct_obs,
            }
        })
        .collect();
    Ok(PriorView {
        repo_key,
        task_type,
        sample_count: prior.sample_count,
        is_warm,
        features,
    })
}

/// 026 F1/US3 (FR-031) — resetea el prior a cold-start. `repo_path` None ⇒ TODOS los contextos;
/// `task_type` None ⇒ todos los task_types de ese repo. NO toca la señal append-only (los records
/// quedan). AUDITADO (append-only, FR-033). Devuelve cuántas filas de prior se borraron.
#[tauri::command]
pub fn preference_prior_reset(
    state: State<'_, AppState>,
    repo_path: Option<String>,
    task_type: Option<String>,
) -> Result<usize, String> {
    use crate::services::preference_signal as pref;
    let repo_key = repo_path.as_deref().map(pref::repo_key_for);
    let n = pref::reset_prior(&state.db, repo_key.as_deref(), task_type.as_deref())
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "preference.prior_reset",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "repo_key": repo_key,
            "task_type": task_type,
            "rows_cleared": n,
        }),
    });
    Ok(n)
}

/// 026 F0/US1 (FR-030) — lista los registros de preferencia más recientes (read-only, sin código
/// crudo). Para inspección/exportación de "qué elegís". `limit` por defecto 50, cap 500.
#[tauri::command]
pub fn preference_records_list(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<crate::services::preference_signal::PreferenceRecord>, String> {
    let limit = limit.unwrap_or(50).clamp(1, 500);
    crate::services::preference_signal::list_records(&state.db, limit).map_err(|e| e.to_string())
}

/// FR-002 pairing-sync — traer el branch de una tarea `awaiting_review` a la WORKING COPY del repo
/// principal manteniendo el git state (checkout del branch), con guardas anti-destructivo:
/// constitución VI — si la working copy está SUCIA, NUNCA pisamos: hacemos `git stash push -u` con
/// un mensaje rastreable y recién ahí cambiamos de branch. `confirm` debe venir en true (el front ya
/// confirmó). Devuelve un reporte de lo que pasó (stashed? branch previo, branch nuevo).
#[tauri::command]
pub fn orchestration_pairing_sync(
    state: State<'_, AppState>,
    task_id: String,
    confirm: bool,
) -> Result<serde_json::Value, String> {
    if !confirm {
        return Err("pairing-sync requiere confirmación explícita".into());
    }
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    if !worktree::is_safe_branch_for_api(&task.branch) {
        return Err(format!("branch no segura: {}", task.branch));
    }
    let report = crate::services::pairing::sync_branch_to_local(&task.repo_path, &task.branch)
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "orch.pairing_sync",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "task_id": task_id, "branch": task.branch,
            "stashed": report.stashed, "prev_branch": report.prev_branch,
        }),
    });
    Ok(serde_json::json!({
        "branch": report.branch,
        "prev_branch": report.prev_branch,
        "stashed": report.stashed,
        "stash_ref": report.stash_ref,
        "was_dirty": report.was_dirty,
        "message": report.message,
    }))
}

/// FR-003 log-history — el historial persistido de buffer-snapshots de una tarea (más reciente
/// primero). El poller (012) lo va capturando; mark-ready también. Para el detalle de la card.
#[tauri::command]
pub fn orchestration_log_history(
    state: State<'_, AppState>,
    task_id: String,
    limit: Option<i64>,
) -> Result<Vec<orch_svc::LogHistoryEntry>, String> {
    orch_svc::get_log_history(&state.db, &task_id, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// FR-003 — captura manual: persiste el snapshot ACTUAL del buffer-tail de la pane de la tarea
/// (cuando el usuario abre el detalle, para no depender sólo del tick del poller). Best-effort.
#[tauri::command]
pub fn orchestration_capture_log(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    task_id: String,
) -> Result<bool, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    let Some(pane_id) = task.pane_id.as_deref() else {
        return Ok(false);
    };
    let lines = pty.snapshot(pane_id);
    orch_svc::append_log_history(&state.db, &task_id, "manual", &lines).map_err(|e| e.to_string())
}

// ── 019 F3 (T030) — pause / resume + ETA + live-logs (tail) ─────────────────

/// T030 PAUSE — pausa un attempt corriendo: SIGSTOP del PTY (sin matar) + flag `paused_at`.
///
/// 019 F3 (audit HIGH-1) — SIGSTOP-FIRST: se manda el SIGSTOP al proceso ANTES de persistir el
/// flag, y el flag se persiste SÓLO si el SIGSTOP tuvo éxito sobre un proceso vivo. Así eliminamos
/// la race del orden anterior (flag→señal): si el proceso moría entre el flag y el SIGSTOP quedaba
/// un attempt marcado "pausado" que en realidad ya había terminado. Casos:
///   - `Err` del SIGSTOP → no se persiste flag, se reporta el error.
///   - `Ok(false)` (no hay proceso vivo: ya salió / sin pane) → no se persiste flag; la tarea no
///     está corriendo, así que no hay nada que pausar (devolvemos `false`, igual que idempotente).
///   - `Ok(true)` (proceso detenido) → recién ahí se persiste `paused_at`. Si la persistencia
///     fallara tras un SIGSTOP exitoso, REANUDAMOS el proceso para no dejarlo congelado sin flag.
/// Idempotente: si ya estaba pausado, `pause_task` devuelve `false` y no re-señalizamos.
/// DESTRUCTIVE/confirm en el registry → pasa por el gate universal + se audita (mutación, R2/FR-007).
#[tauri::command]
pub fn orchestration_pause_task(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    task_id: String,
) -> Result<bool, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    // Pre-chequeo de idempotencia: si ya está pausado, no re-señalizamos ni re-pisamos el flag.
    if task.paused_at.is_some() {
        return Ok(false);
    }
    // SIGSTOP PRIMERO. Si no hay pane no hay proceso vivo → nada que pausar.
    let Some(pane_id) = task.pane_id.as_deref() else {
        return Ok(false);
    };
    let stopped = pty
        .pause(pane_id)
        .map_err(|e| format!("no se pudo pausar el proceso: {e}"))?;
    if !stopped {
        // El proceso ya no existe (salió antes/durante el pause). No persistimos un flag
        // inconsistente: no hay nada congelado que "reanudar" después.
        return Ok(false);
    }
    // SIGSTOP OK sobre un proceso vivo → recién ahora persistimos el flag.
    match orch_svc::pause_task(&state.db, &task_id) {
        Ok(true) => {
            let _ = state.audit.write(EventInput {
                kind: "orch.task_paused",
                actor: &crate::services::identity::current_actor(),
                pane_id: Some(pane_id),
                card_id: None,
                correlation_id: None,
                payload: serde_json::json!({"task_id": task_id}),
            });
            Ok(true)
        }
        // Carrera: otro pause ganó el flag entre nuestro pre-chequeo y el UPDATE. El proceso ya
        // quedó detenido (estado deseado); no es error, simplemente no fuimos nosotros.
        Ok(false) => Ok(false),
        // El flag no se pudo persistir tras un SIGSTOP exitoso → REANUDAR para no dejar el proceso
        // congelado sin flag que lo represente (evita el zombie congelado de la otra dirección).
        Err(e) => {
            let _ = pty.resume(pane_id);
            Err(format!(
                "SIGSTOP ok pero no se pudo persistir paused_at: {e}"
            ))
        }
    }
}

/// 047 FR-007 — "Detener agentes": pausa (SIGSTOP, NO mata) TODAS las tareas de orquestación que
/// están corriendo en un pane y no están ya pausadas. Reusa la MISMA lógica per-task de
/// `orchestration_pause_task` (SIGSTOP-first → persistir `paused_at` sólo si el SIGSTOP tuvo éxito
/// sobre un proceso vivo → audit). Es una acción HUMANA explícita: el front la dispara detrás de una
/// confirmación; el backend nunca la auto-invoca. Devuelve cuántas tareas quedaron pausadas.
///
/// Idempotente y best-effort: una tarea cuyo proceso ya salió (Ok(false)) o que ya estaba pausada se
/// salta sin contar; un error de SIGSTOP en una tarea NO aborta el resto (se acumula y se reporta).
/// El estado "pausado" se REANUDA por tarea con `orchestration_resume_task` (no hay des-pausa masiva
/// para no reanudar a ciegas algo que el humano detuvo a propósito).
/// DESTRUCTIVE/confirm en el registry → gate universal + audit.
#[tauri::command]
pub fn stop_all_agents(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
) -> Result<usize, String> {
    // `list_tasks` devuelve un Vec OWNED y libera su lock de DB AL RETORNAR (el MutexGuard es local y
    // se dropea al final de la función). Así, cuando el loop llama `pause_task` (que re-lockea la DB),
    // NO hay un lock de `list_tasks` vivo → sin deadlock/re-entrancy. Lo cubre el test
    // `stop_all_agents_selects_only_running_with_pane_unpaused` (list_tasks→pause_task en loop, pasa).
    let tasks = orch_svc::list_tasks(&state.db, None).map_err(|e| e.to_string())?;
    let mut paused = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for task in tasks {
        // Sólo tareas corriendo, con pane vivo, no ya pausadas.
        if task.state != "running" || task.paused_at.is_some() {
            continue;
        }
        let Some(pane_id) = task.pane_id.as_deref() else {
            continue;
        };
        // SIGSTOP-FIRST (igual que orchestration_pause_task).
        match pty.pause(pane_id) {
            Ok(true) => {
                // Proceso detenido → recién ahora persistimos el flag.
                match orch_svc::pause_task(&state.db, &task.id) {
                    Ok(true) => {
                        paused += 1;
                        let _ = state.audit.write(EventInput {
                            kind: "orch.task_paused",
                            actor: &crate::services::identity::current_actor(),
                            pane_id: Some(pane_id),
                            card_id: None,
                            correlation_id: None,
                            payload: serde_json::json!({"task_id": task.id, "via": "stop_all_agents"}),
                        });
                    }
                    // Carrera: otro pause ganó el flag. El proceso ya quedó detenido (estado deseado).
                    Ok(false) => {}
                    // No se pudo persistir el flag tras un SIGSTOP OK → REANUDAR para no dejarlo
                    // congelado sin flag (simétrico a orchestration_pause_task).
                    Err(e) => {
                        let _ = pty.resume(pane_id);
                        errors.push(format!("{}: persist paused_at falló: {e}", task.id));
                    }
                }
            }
            // El proceso ya no existe (salió antes/durante) → nada que pausar, no es error.
            Ok(false) => {}
            Err(e) => errors.push(format!("{}: SIGSTOP falló: {e}", task.id)),
        }
    }
    if !errors.is_empty() {
        // Reportamos el agregado pero NO perdemos lo ya pausado: el front muestra el conteo + el error.
        return Err(format!(
            "pausadas {paused}, con {} error(es): {}",
            errors.len(),
            errors.join("; ")
        ));
    }
    Ok(paused)
}

/// T030 RESUME — reanuda un attempt pausado: SIGCONT del PTY + limpia `paused_at`. Idempotente.
///
/// 019 F3 (audit HIGH-1, simétrico a pause) — SIGCONT-FIRST: se reanuda el proceso ANTES de limpiar
/// el flag. Si el SIGCONT real falla (no "proceso ya muerto", sino un error de señal) NO limpiamos
/// el flag — el estado persistido sigue reflejando "pausado". Si el proceso YA murió mientras estaba
/// pausado (`Ok(false)`), igual limpiamos el flag: no hay nada congelado, "reanudado" se cumple.
/// Tras un SIGCONT exitoso se limpia `paused_at`. Idempotente: si no estaba pausado, no-op.
/// DESTRUCTIVE/confirm en el registry (es la contraparte de pause, misma clase de mutación).
#[tauri::command]
pub fn orchestration_resume_task(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    task_id: String,
) -> Result<bool, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    // Idempotente: si no estaba pausado, no hay nada que reanudar.
    if task.paused_at.is_none() {
        return Ok(false);
    }
    // SIGCONT PRIMERO. Un error real de la señal (no "ya muerto") → no limpiamos el flag.
    if let Some(pane_id) = task.pane_id.as_deref() {
        pty.resume(pane_id)
            .map_err(|e| format!("no se pudo reanudar el proceso: {e}"))?;
    }
    // SIGCONT ok (o proceso ya muerto) → recién ahora limpiamos el flag.
    let resumed = orch_svc::resume_task(&state.db, &task_id).map_err(|e| e.to_string())?;
    if resumed {
        let _ = state.audit.write(EventInput {
            kind: "orch.task_resumed",
            actor: &crate::services::identity::current_actor(),
            pane_id: task.pane_id.as_deref(),
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"task_id": task_id}),
        });
    }
    Ok(resumed)
}

/// T030 ETA — estimación de tiempo restante de un BATCH (todas sus tareas). Cálculo PURO sobre la
/// duración de los attempts ya terminados (sin LLM/red). Devuelve None si no hay base (ningún
/// attempt terminado) o nada corriendo. Read-only (no muta, no gateado).
#[tauri::command]
pub fn orchestration_eta(
    state: State<'_, AppState>,
    batch_id: String,
) -> Result<Option<orch_svc::EtaEstimate>, String> {
    let tasks = orch_svc::list_tasks(&state.db, Some(&batch_id)).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let timings: Vec<orch_svc::AttemptTiming> = tasks
        .iter()
        .filter_map(|t| orch_svc::task_timing(t, now))
        .collect();
    Ok(orch_svc::estimate_eta(&timings))
}

/// T030 ETA de un GRUPO best-of-N (las N variantes). Mismo cálculo puro, scope = grupo.
#[tauri::command]
pub fn orchestration_group_eta(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<orch_svc::EtaEstimate>, String> {
    let tasks = orch_svc::list_group_tasks(&state.db, &group_id).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now();
    let timings: Vec<orch_svc::AttemptTiming> = tasks
        .iter()
        .filter_map(|t| orch_svc::task_timing(t, now))
        .collect();
    Ok(orch_svc::estimate_eta(&timings))
}

/// T030 LIVE-LOGS — tail VIVO del scrollback de la pane de una tarea (no persistido). A diferencia
/// de `orchestration_log_history` (snapshots persistidos, redactados), esto es el buffer ACTUAL
/// (ANSI-stripped) para que la UI lo poll-ee en vivo mientras el agente trabaja. Read-only.
/// Se redacta igual (F-I BYOK: el tail vivo también puede mostrar un secret en pantalla).
#[tauri::command]
pub fn orchestration_tail_log(
    state: State<'_, AppState>,
    pty: State<'_, Arc<PtyManager>>,
    task_id: String,
) -> Result<Vec<String>, String> {
    let task = orch_svc::get_task(&state.db, &task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("tarea no encontrada: {}", task_id))?;
    let Some(pane_id) = task.pane_id.as_deref() else {
        return Ok(Vec::new());
    };
    let lines = pty.snapshot(pane_id);
    // redactar cada línea (el snapshot ya viene ANSI-stripped).
    let redacted = lines
        .iter()
        .map(|l| crate::services::tts::redact_secrets(l))
        .collect();
    Ok(redacted)
}

/// FR-005 file-locks — intenta adquirir un lock de recurso compartido (puerto/DB dev) para una
/// tarea. Devuelve None si lo consiguió, o el task_id dueño actual si está tomado. Advisory.
#[tauri::command]
pub fn orchestration_acquire_lock(
    state: State<'_, AppState>,
    resource_key: String,
    task_id: String,
    ttl_secs: Option<i64>,
) -> Result<Option<String>, String> {
    orch_svc::try_acquire_lock(&state.db, &resource_key, &task_id, ttl_secs)
        .map_err(|e| e.to_string())
}

/// FR-005 — libera un lock que tiene la tarea (no roba ajenos).
#[tauri::command]
pub fn orchestration_release_lock(
    state: State<'_, AppState>,
    resource_key: String,
    task_id: String,
) -> Result<bool, String> {
    orch_svc::release_lock(&state.db, &resource_key, &task_id).map_err(|e| e.to_string())
}

/// FR-004 board↔workspace — GC de worktrees orphan/expired + prune. Respeta el escape-hatch
/// `DISABLE_WORKTREE_CLEANUP` (env var O setting): si está activo, NO limpia nada (debug). Sólo
/// limpia worktrees de tareas en estado TERMINAL (done/failed/canceled) — nunca de una running.
#[tauri::command]
pub fn orchestration_cleanup_worktrees(
    state: State<'_, AppState>,
    repo_path: String,
    confirm: bool,
) -> Result<serde_json::Value, String> {
    if !confirm {
        return Err("el cleanup de worktrees requiere confirmación".into());
    }
    // Escape-hatch (constitución VI): env var O setting desactiva el cleanup para debug.
    if crate::services::workspace::cleanup_disabled(&state.db) {
        return Ok(serde_json::json!({
            "disabled": true,
            "reason": "DISABLE_WORKTREE_CLEANUP activo (escape-hatch) — no se limpió nada",
            "removed": [],
        }));
    }
    let removed = crate::services::workspace::cleanup_terminal_worktrees(&state.db, &repo_path)
        .map_err(|e| e.to_string())?;
    // prune de orphans registrados en git pero ya borrados del FS.
    let _ = orch_svc::prune_worktrees(&repo_path);
    let _ = state.audit.write(EventInput {
        kind: "orch.worktrees_cleaned",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"repo_path": repo_path, "removed": removed.len()}),
    });
    Ok(serde_json::json!({"disabled": false, "removed": removed}))
}

// ── 010-furx-signals — config (BYOK) + control remoto ───────────────
use crate::services::remote_control as rc_svc;

/// Lista de deliveries recientes (para verificación e2e / UI feed).
#[tauri::command]
pub fn signals_recent_deliveries(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    let lim = limit.unwrap_or(50).clamp(1, 500);
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT d.event_id, d.channel, d.status, d.attempts, d.last_error, e.type, e.severity
             FROM signal_deliveries d JOIN signal_events e ON e.id = d.event_id
             ORDER BY d.updated_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![lim], |r| {
            Ok(serde_json::json!({
                "event_id": r.get::<_, String>(0)?,
                "channel": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "attempts": r.get::<_, i64>(3)?,
                "last_error": r.get::<_, Option<String>>(4)?,
                "type": r.get::<_, String>(5)?,
                "severity": r.get::<_, String>(6)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// BYOK — guarda el bot token / HMAC secret de Telegram en el Keychain (NUNCA en DB/backend).
#[tauri::command]
pub fn signals_set_telegram_secret(secret: String) -> Result<(), String> {
    if secret.trim().is_empty() {
        return Err("secret vacío".into());
    }
    // 041 FR-006 — the Keychain account is the validated current user, NOT the hard-coded "hernan".
    // A stranger installs and saves under THEIR account; `telegram::read_secret` reads the same
    // account (with a documented legacy `hernan` read-fallback for el autor's existing entries).
    let account = crate::services::identity::keychain_account();
    crate::services::keychain::save("furx-telegram-hmac-keyring", &account, &secret)
        .map_err(|e| e.to_string())?;
    // Compat: telegram::read_secret usa `security` CLI service `furx-telegram-hmac`.
    crate::services::keychain::save("furx-telegram-hmac", &account, &secret)
        .map_err(|e| e.to_string())
}

/// BYOK — guarda el HMAC secret del webhook genérico en el Keychain.
#[tauri::command]
pub fn signals_set_webhook_secret(secret: String) -> Result<(), String> {
    crate::services::keychain::save("furx-signals", "webhook-secret", &secret)
        .map_err(|e| e.to_string())
}

/// Set/clear de un filtro de canal (signal_subscriptions).
#[tauri::command]
pub fn signals_set_subscription(
    state: State<'_, AppState>,
    event_type: String,
    channel: String,
    enabled: bool,
    min_severity: String,
) -> Result<(), String> {
    signals_svc::set_subscription(&state.db, &event_type, &channel, enabled, &min_severity)
        .map_err(|e| e.to_string())
}

/// Genera un código de pairing de un uso (lo muestra Settings; se manda por /pair <code>).
#[tauri::command]
pub fn signals_create_pair_code(state: State<'_, AppState>) -> Result<String, String> {
    rc_svc::create_pair_code(&state.db).map_err(|e| e.to_string())
}

/// Lista los chat_ids en la allowlist de control remoto.
#[tauri::command]
pub fn signals_list_allowlist(
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT chat_id, label, paired_via, created_at FROM signal_remote_allowlist ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(serde_json::json!({
                "chat_id": r.get::<_, String>(0)?,
                "label": r.get::<_, Option<String>>(1)?,
                "paired_via": r.get::<_, String>(2)?,
                "created_at": r.get::<_, String>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Agrega manualmente un chat_id a la allowlist (Settings).
#[tauri::command]
pub fn signals_add_allowlist(
    state: State<'_, AppState>,
    chat_id: String,
    label: Option<String>,
) -> Result<(), String> {
    rc_svc::add_to_allowlist(&state.db, &chat_id, label.as_deref(), "manual")
        .map_err(|e| e.to_string())
}

/// Quita un chat_id de la allowlist.
#[tauri::command]
pub fn signals_remove_allowlist(
    state: State<'_, AppState>,
    chat_id: String,
) -> Result<bool, String> {
    rc_svc::remove_from_allowlist(&state.db, &chat_id).map_err(|e| e.to_string())
}

use crate::services::claude_accounts as claude_accounts_svc;

// Backward-compatible names (claude_accounts_*) — kept so the frontend doesn't break.
// New universal commands have signature (cli_kind, slug).

#[tauri::command]
pub fn claude_accounts_list(
    state: State<'_, AppState>,
) -> Result<Vec<claude_accounts_svc::ClaudeAccount>, String> {
    claude_accounts_svc::list_all(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn claude_account_add(
    state: State<'_, AppState>,
    app: AppHandle,
    req: claude_accounts_svc::AddRequest,
) -> Result<claude_accounts_svc::ClaudeAccount, String> {
    let acct = claude_accounts_svc::add(&state.db, req).map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit("cli-accounts:changed", &acct.slug);
    let _ = app.emit("claude-accounts:changed", &acct.slug); // legacy
    let _ = state.audit.write(EventInput {
        kind: "cli_account.add",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "cli_kind": acct.cli_kind,
            "slug": acct.slug,
            "status": acct.status
        }),
    });
    Ok(acct)
}

#[tauri::command]
pub fn claude_account_delete(
    state: State<'_, AppState>,
    app: AppHandle,
    cli_kind: Option<String>,
    slug: String,
) -> Result<bool, String> {
    let kind = cli_kind.as_deref().unwrap_or("claude");
    let removed = claude_accounts_svc::delete(&state.db, kind, &slug).map_err(|e| e.to_string())?;
    if removed {
        use tauri::Emitter;
        let _ = app.emit("cli-accounts:changed", &slug);
        let _ = app.emit("claude-accounts:changed", &slug);
        let _ = state.audit.write(EventInput {
            kind: "cli_account.delete",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: serde_json::json!({"cli_kind": kind, "slug": slug}),
        });
    }
    Ok(removed)
}

#[tauri::command]
pub fn claude_account_verify(
    state: State<'_, AppState>,
    app: AppHandle,
    cli_kind: Option<String>,
    slug: String,
) -> Result<claude_accounts_svc::VerifyResult, String> {
    let kind = cli_kind.as_deref().unwrap_or("claude");
    let res = claude_accounts_svc::verify(&state.db, kind, &slug).map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit("cli-accounts:changed", &res.slug);
    let _ = app.emit("claude-accounts:changed", &res.slug);
    Ok(res)
}

/// Open setup-account.sh in a new Terminal window for a given (cli_kind, slug).
/// Backward-compatible: defaults to cli_kind=claude → setup-max-account.sh.
#[tauri::command]
pub fn claude_account_run_setup(cli_kind: Option<String>, slug: String) -> Result<String, String> {
    if slug.is_empty() || slug.len() > 32 {
        return Err("invalid slug (1-32 chars)".into());
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("invalid slug characters".into());
    }
    let kind = cli_kind.as_deref().unwrap_or("claude");
    // Validate kind against enum
    if claude_accounts_svc::CliKind::parse(kind).is_none() {
        return Err(format!("invalid cli_kind: {}", kind));
    }
    // Pick the right shell script — claude uses the legacy name for back-compat.
    let script_path = if kind == "claude" {
        "~/bin/setup-max-account.sh".to_string()
    } else {
        "~/bin/setup-account.sh".to_string()
    };
    let cmd_str = if kind == "claude" {
        format!("{} {}", script_path, slug)
    } else {
        format!("{} {} --cli {}", script_path, slug, kind)
    };
    let script = format!(
        "tell application \"Terminal\"\n  activate\n  do script \"{}\"\nend tell",
        cmd_str
    );
    let output = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("osascript failed: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Terminal launch failed: {}", err));
    }
    Ok(slug)
}

/// Scan default local LLM endpoints (Ollama 11434, LM Studio 1234, llama.cpp 8080).
#[tauri::command]
pub async fn provider_local_scan() -> Result<providers_svc::LocalScan, String> {
    Ok(providers_svc::local_scan().await)
}

/// Run a Council Mode round across all healthy providers (or those matching the preset).
/// Returns one VoiceResult per provider/model. Graceful degrade — fail-tolerant.
#[tauri::command]
pub async fn council_run_multi(
    state: State<'_, AppState>,
    req: council_multi_svc::CouncilRequest,
) -> Result<council_multi_svc::CouncilResult, String> {
    let db = state.db.clone();
    let result = council_multi_svc::run(db, req)
        .await
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "council.run",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "preset": result.preset,
            "voices_attempted": result.voices_attempted,
            "voices_succeeded": result.voices_succeeded,
            "elapsed_ms": result.elapsed_ms,
        }),
    });
    Ok(result)
}

/// List all Council Templates (built-in + user-defined) for the UI selector.
#[tauri::command]
pub fn council_templates_list(
    state: State<'_, AppState>,
) -> Result<Vec<council_multi_svc::CouncilTemplate>, String> {
    Ok(council_multi_svc::list_templates(&state.db))
}

// ── 019 F3 (T031) — council history + custom-voices (F-II: free para TODOS los tiers) ──

/// T031 HISTORY — lista los councils corridos (más reciente primero). Read-only (no gateado).
/// El council es free para todos los tiers (constitución F-II): el history NO es un feature pago.
#[tauri::command]
pub fn council_history_list(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<council_multi_svc::CouncilRunRecord>, String> {
    council_multi_svc::list_runs(&state.db, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// T031 HISTORY — vacía el history del council (acción del user sobre SU dato). DESTRUCTIVE/confirm
/// en el registry → pasa por el gate universal + se audita.
#[tauri::command]
pub fn council_history_clear(state: State<'_, AppState>) -> Result<usize, String> {
    let n = council_multi_svc::clear_runs(&state.db).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "council.history_cleared",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"removed": n}),
    });
    Ok(n)
}

/// T031 CUSTOM-VOICES — lista las voces custom configuradas por el user (config, no tier-gate).
/// Read-only. NUNCA expone keys (sólo el alias de la credencial conectada).
#[tauri::command]
pub fn council_custom_voices_list(
    state: State<'_, AppState>,
) -> Result<Vec<council_multi_svc::CustomVoice>, String> {
    council_multi_svc::list_custom_voices(&state.db).map_err(|e| e.to_string())
}

/// T031 CUSTOM-VOICES — agrega/re-activa una voz custom (provider conectado + model opcional).
/// F-II: NUNCA se gatea por tier — es configuración del council, no un paywall. Pero es una
/// MUTACIÓN de config → DESTRUCTIVE/confirm en el registry → gate universal + audit.
#[tauri::command]
pub fn council_custom_voice_add(
    state: State<'_, AppState>,
    provider_alias: String,
    model: Option<String>,
) -> Result<String, String> {
    let id = council_multi_svc::add_custom_voice(&state.db, &provider_alias, model.as_deref())
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "council.custom_voice_added",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        // alias + model son NO-secretos (la key vive en Keychain, jamás acá).
        payload: serde_json::json!({"id": id, "provider_alias": provider_alias, "model": model}),
    });
    Ok(id)
}

/// T031 CUSTOM-VOICES — habilita/deshabilita una voz custom sin borrarla. Mutación → gate + audit.
#[tauri::command]
pub fn council_custom_voice_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<bool, String> {
    let changed = council_multi_svc::set_custom_voice_enabled(&state.db, &id, enabled)
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "council.custom_voice_toggled",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"id": id, "enabled": enabled}),
    });
    Ok(changed)
}

/// T031 CUSTOM-VOICES — borra una voz custom. Mutación destructiva → gate + audit.
#[tauri::command]
pub fn council_custom_voice_remove(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    let removed =
        council_multi_svc::remove_custom_voice(&state.db, &id).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "council.custom_voice_removed",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"id": id}),
    });
    Ok(removed)
}

/// Returns true if Pro features should be unlocked right now (Valid or Trial).
/// HIGH-2 fix (Codex): fail-CLOSED on errors. No silent fallback to recompute-trial-from-local-clock.
/// The trial state is computed inside license_svc::check via offline_state(), which respects
/// the server-time anchor + offline grace cap. If even that fails, return false.
#[tauri::command]
pub async fn license_is_pro(state: State<'_, AppState>) -> Result<bool, String> {
    let endpoint = {
        let conn = state.db.lock();
        crate::settings::get(&conn, "endpoints.license")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| {
                crate::settings::get(&conn, "endpoints.aie")
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_str().map(String::from))
            })
            .unwrap_or_else(|| "https://aie.example.test".to_string())
    };
    match license_svc::check(&state.db, &endpoint).await {
        Ok(s) => Ok(license_svc::is_pro_active(&s)),
        Err(e) => {
            tracing::warn!("license_is_pro check failed (fail-closed): {}", e);
            Ok(false)
        }
    }
}

// --- BLOQUE D · F20 — spec-kit `spec` alias installer ---
// Idempotent: writes `~/bin/spec` (mode 0755) that execs `specify "$@"`.
// Lets the user type `spec build whatever` in ANY zsh pane without us having
// to monkey-patch xterm input (which would be brittle and surprise the user).

#[tauri::command]
pub fn spec_kit_install_alias() -> Result<bool, String> {
    use std::io::Write;
    let home = dirs::home_dir().ok_or("no home")?;
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let target = bin.join("spec");
    // If the file already exists and contains our marker, assume it's ours and skip.
    if let Ok(existing) = std::fs::read_to_string(&target) {
        if existing.contains("# furx-spec-alias") {
            return Ok(false); // already installed, nothing changed
        }
        // File exists but isn't ours — refuse to overwrite (preserve user's shim).
        return Err(format!(
            "~/bin/spec already exists and isn't a furx-managed alias: {}",
            target.display()
        ));
    }
    let script = "#!/usr/bin/env zsh\n# furx-spec-alias\nexec specify \"$@\"\n";
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&target)
        .map_err(|e| e.to_string())?;
    f.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(true)
}

#[tauri::command]
pub fn spec_kit_alias_status() -> Result<bool, String> {
    let home = dirs::home_dir().ok_or("no home")?;
    let target = home.join("bin").join("spec");
    if let Ok(contents) = std::fs::read_to_string(&target) {
        return Ok(contents.contains("# furx-spec-alias"));
    }
    Ok(false)
}

// --- BLOQUE D · F12 — auto-poll smart-paste gate ---
// Returns the classification only if the clipboard contents are worth
// surfacing. Frontend ticks this every ~1s. Returning None keeps the toast
// pipeline quiet; the user can still open the modal manually.
#[tauri::command]
pub fn smartpaste_offer(text: String) -> Result<Option<smartpaste::PasteClassification>, String> {
    let cls = smartpaste::classify(&text);
    if smartpaste::should_offer_paste(&cls) {
        Ok(Some(cls))
    } else {
        Ok(None)
    }
}

// --- F7 — inter-pane send audit event ---
// Frontend tomó los últimos N lines del buffer del pane source y los escribió
// al pane target via pty_write. Acá solo registramos el audit con metadata
// (NO el contenido, mismo principio que broadcast.sent).

#[derive(Debug, serde::Deserialize)]
pub struct InterPaneSendInput {
    pub source_pane_id: String,
    pub target_pane_id: String,
    pub length: u32,
    pub lines: u32,
}

#[tauri::command]
pub fn interpane_send_audit(
    state: State<'_, AppState>,
    payload: InterPaneSendInput,
) -> Result<(), String> {
    // Defensive caps + sanitisation
    let sanitize = |s: &str| -> String {
        s.chars()
            .take(64)
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
            .collect()
    };
    let src = sanitize(&payload.source_pane_id);
    let dst = sanitize(&payload.target_pane_id);
    if src.is_empty() || dst.is_empty() {
        return Err("invalid pane id".into());
    }
    if src == dst {
        return Err("source and target must differ".into());
    }
    let length = payload.length.min(64 * 1024);
    let lines = payload.lines.min(2048);
    state.audit.write(EventInput {
        kind: "interpane.sent",
        actor: &crate::services::identity::current_actor(),
        pane_id: Some(&src),
        card_id: None, correlation_id: None,
        payload: serde_json::json!({"source": src, "target": dst, "length": length, "lines": lines}),
    }).map_err(|e| e.to_string())?;
    Ok(())
}

// --- F4 — broadcast.sent audit event ---
// Codex-A must-fix: emitted only AFTER all selected pty_writes succeed in the
// frontend. Payload deliberately omits raw message content (security-privacy
// must-fix); we keep count + length + opt-in message hash for dedup.
#[derive(Debug, serde::Deserialize)]
pub struct BroadcastAuditInput {
    pub count: u32,
    pub length: u32,
    #[serde(default)]
    pub message_hash: Option<String>,
    #[serde(default)]
    pub success_count: Option<u32>,
    #[serde(default)]
    pub failure_count: Option<u32>,
}

#[tauri::command]
pub fn broadcast_audit_sent(
    state: State<'_, AppState>,
    payload: BroadcastAuditInput,
) -> Result<(), String> {
    // Defensive bounds — frontend sends count/length but we don't trust them.
    let count = payload.count.min(64);
    let length = payload.length.min(1024 * 64);
    let mut body = serde_json::json!({"count": count, "length": length});
    if let Some(s) = payload.success_count {
        body["success_count"] = serde_json::Value::from(s.min(64));
    }
    if let Some(f) = payload.failure_count {
        body["failure_count"] = serde_json::Value::from(f.min(64));
    }
    if let Some(h) = payload.message_hash.as_ref() {
        // Codex audit LOW #5: require EXACTLY 64 ascii-hex chars (SHA-256 hex).
        // Shorter hex strings would let an attacker collide trivially and a
        // longer one is not a SHA-256 by definition; reject silently.
        if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            body["message_hash"] = serde_json::Value::from(h.clone());
        }
    }
    state
        .audit
        .write(EventInput {
            kind: "broadcast.sent",
            actor: &crate::services::identity::current_actor(),
            pane_id: None,
            card_id: None,
            correlation_id: None,
            payload: body,
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

// --- FASE 1 — Skills Registry commands ---
use crate::services::skills as skills_svc;
use tauri::Emitter;

#[tauri::command]
pub fn skill_refresh(state: State<'_, AppState>) -> Result<usize, String> {
    skills_svc::refresh_from_disk(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_list(state: State<'_, AppState>) -> Result<Vec<skills_svc::SkillSummary>, String> {
    skills_svc::list_skills(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_get(
    state: State<'_, AppState>,
    name: String,
) -> Result<skills_svc::SkillDefinition, String> {
    skills_svc::get_skill(&state.db, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_set_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    skills_svc::set_enabled(&state.db, &name, enabled).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_delete(state: State<'_, AppState>, name: String) -> Result<(), String> {
    skills_svc::delete_skill(&state.db, &name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_history(
    state: State<'_, AppState>,
    name: String,
    limit: Option<usize>,
) -> Result<Vec<skills_svc::SkillRunHistory>, String> {
    skills_svc::get_run_history(&state.db, &name, limit.unwrap_or(20)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn skill_run(
    state: State<'_, AppState>,
    app: AppHandle,
    name: String,
    input: String,
) -> Result<String, String> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let db = state.db.clone();
    let skill_name = name.clone();

    // Spawn skill execution in background, emit progress events
    tokio::spawn(async move {
        let result = skills_svc::run_skill(&db, &skill_name, &input, tx).await;
        match result {
            Ok(r) => {
                let _ = app.emit("skill-run-complete", &r);
            }
            Err(e) => {
                let _ = app.emit(
                    "skill-run-error",
                    serde_json::json!({"skill": skill_name, "error": e.to_string()}),
                );
            }
        }
    });

    // Return immediately with the run being tracked via events
    Ok("started".to_string())
}

/// 050 Ola 8 P2 (FR-005) — CRL con señalización activa: revoca una signing-key de skill. (1) la
/// persiste en `revoked_keys.txt` (bloquea cargas futuras — Ola 4); (2) SEÑALIZA a todo span vivo
/// firmado por esa key para que aborte (un skill firmado por ella que esté corriendo se corta, no
/// solo se bloquean los futuros). FAIL-CLOSED: si no se puede escribir el archivo → Err. `key_hex`
/// = SHA-256 hex (64 chars) del pubkey Ed25519 (el `key_id[..64]` del manifest / línea de
/// revoked_keys.txt). DESTRUCTIVO → el gate de confirmación lo maneja la UI. Queda en el audit log.
#[tauri::command]
pub fn crl_revoke_key(
    state: State<'_, AppState>,
    key_hex: String,
) -> Result<crate::services::crl::RevokeResult, String> {
    let res = crate::services::crl::revoke_key(&key_hex).map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "skill.key_revoked",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({
            "key_hex": key_hex,
            "signaled_spans": res.signaled_spans,
        }),
    });
    Ok(res)
}

// --- F3.2 — LaunchAgent commands ---

#[tauri::command]
pub fn memory_launchagent_install() -> Result<String, String> {
    // Find our own binary path
    let self_path = std::env::current_exe().map_err(|e| e.to_string())?;
    mem_svc::install_launchagent(
        self_path
            .to_str()
            .unwrap_or("/Applications/Furx.app/Contents/MacOS/furx"),
    )
    .map_err(|e| e.to_string())?;
    Ok("installed".to_string())
}

#[tauri::command]
pub fn memory_launchagent_uninstall() -> Result<String, String> {
    mem_svc::uninstall_launchagent().map_err(|e| e.to_string())?;
    Ok("uninstalled".to_string())
}

#[tauri::command]
pub fn memory_launchagent_status() -> Result<bool, String> {
    mem_svc::launchagent_status().map_err(|e| e.to_string())
}

// --- F3.3 — CLI Hooks generator ---

#[tauri::command]
pub fn memory_generate_cli_hooks() -> Result<serde_json::Value, String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    let claude_cmds = mem_svc::generate_claude_commands(&home).map_err(|e| e.to_string())?;
    let claude_hooks = mem_svc::generate_claude_hooks(&home).map_err(|e| e.to_string())?;
    let codex_cmds = mem_svc::generate_codex_commands(&home).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "claude_commands": claude_cmds.len(),
        "claude_hooks": claude_hooks.to_string_lossy().to_string(),
        "codex_commands": codex_cmds.len(),
    }))
}

// --- F3.4 — Knowledge Graph commands ---

#[tauri::command]
pub fn memory_graph_entities(
    state: State<'_, AppState>,
) -> Result<Vec<mem_svc::GraphEntity>, String> {
    let conn = state.db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, entity_type, metadata, created_at FROM memory_entities ORDER BY name LIMIT 100"
    ).map_err(|e| e.to_string())?;
    let entities = stmt
        .query_map([], |r| {
            Ok(mem_svc::GraphEntity {
                id: r.get(0)?,
                name: r.get(1)?,
                entity_type: r.get(2)?,
                metadata: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(entities)
}

// --- FASE 2 — Memory Hub commands (original) ---
use crate::services::memory_daemon as mem_svc;

#[tauri::command]
pub fn memory_store(
    state: State<'_, AppState>,
    content: String,
    source: Option<String>,
    tags: Option<Vec<String>>,
) -> Result<String, String> {
    let source = source.unwrap_or_else(|| "furx".to_string());
    let tags: Vec<&str> = tags
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .collect();
    mem_svc::store_memory(&state.db, &source, &content, &tags).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_recall(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<mem_svc::MemoryEntry>, String> {
    mem_svc::recall_memories(&state.db, &query, limit.unwrap_or(10)).map_err(|e| e.to_string())
}

/// 045 FR-003 — recall con re-rank vectorial OPT-IN + circuit-breaker del embedder. Devuelve el
/// `RecallResult` con `backend` ("fts" | "vector") para que la UI muestre el indicador de calidad.
/// Con el embedder caído cae a FTS sin colgar (breaker abre al 3er timeout, reintenta a los 60s).
#[tauri::command]
pub async fn memory_recall_ranked(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
    rerank: Option<bool>,
) -> Result<mem_svc::RecallResult, String> {
    mem_svc::recall_memories_ranked(
        &state.db,
        &query,
        limit.unwrap_or(10),
        rerank.unwrap_or(false),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_stats(state: State<'_, AppState>) -> Result<mem_svc::MemoryStats, String> {
    mem_svc::memory_stats(&state.db).map_err(|e| e.to_string())
}

// --- 023 F0/F1 — Memory Hub no-opaco + bandeja de propuestas ---

/// Una propuesta de memoria (bandeja de revisión humana). Espejo de `memory_proposals`.
#[derive(Debug, serde::Serialize)]
pub struct MemoryProposal {
    pub id: String,
    pub project_key: String,
    pub source: String,
    pub source_id: Option<String>,
    pub cli_kind: Option<String>,
    pub session_id: Option<String>,
    pub content: String,
    pub kind: Option<String>,
    pub confidence_score: Option<f64>,
    pub status: String,
    pub rationale: Option<String>,
    pub created_at: String,
    pub decided_at: Option<String>,
}

/// 023 F1 — lista las propuestas de memoria pendientes (status='proposed'), más recientes primero.
#[tauri::command]
pub fn memory_proposals_list(
    state: State<'_, AppState>,
) -> Result<Vec<MemoryProposal>, String> {
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, project_key, source, source_id, cli_kind, session_id, content, kind,
                    confidence_score, status, rationale, created_at, decided_at
             FROM memory_proposals WHERE status = 'proposed' ORDER BY created_at DESC LIMIT 200",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(MemoryProposal {
                id: r.get(0)?,
                project_key: r.get(1)?,
                source: r.get(2)?,
                source_id: r.get(3)?,
                cli_kind: r.get(4)?,
                session_id: r.get(5)?,
                content: r.get(6)?,
                kind: r.get(7)?,
                confidence_score: r.get(8)?,
                status: r.get(9)?,
                rationale: r.get(10)?,
                created_at: r.get(11)?,
                decided_at: r.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// 023 F1 — decide sobre una propuesta. `action` ∈ {accept, reject, edit}.
///   - accept: crea la entry en el Hub (con procedencia/rationale/kind), marca la propuesta
///     `accepted` (INMUTABLE: no se modifica más). `content`/`kind` opcionales sobreescriben.
///   - edit:   igual que accept pero con `content` editado por el usuario (status 'edited').
///   - reject: marca `rejected` (no entra al Hub).
/// El content se RE-SCRUBEA idempotente antes de entrar al Hub (defensa en capas).
///
/// CLAIM ATÓMICO (audit MED — TOCTOU): para accept/edit se RECLAMA la propuesta con un UPDATE
/// condicional `proposed → accepting` ANTES de crear el `memory_entry`. Como `store_memory_full`
/// toma el lock de la DB por su cuenta, no se puede sostener el lock cruzando el insert; el claim
/// hace que SÓLO un caller gane la transición `proposed→accepting`. Dos accepts concurrentes del
/// mismo id producen EXACTAMENTE 1 entry: el perdedor ve rowcount==0 y aborta idempotente (no
/// crea entry). Si `store_memory_full` falla tras reclamar, se revierte `accepting → proposed`
/// para que la propuesta sea reintentable.
#[tauri::command]
pub fn memory_proposal_decide(
    state: State<'_, AppState>,
    id: String,
    action: String,
    content: Option<String>,
    kind: Option<String>,
) -> Result<Option<String>, String> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    match action.as_str() {
        "reject" => {
            // Reject es idempotente (no crea entry); el guard `status='proposed'` basta. Dos
            // rejects concurrentes son inocuos (el segundo no afecta filas).
            let conn = state.db.lock();
            let n = conn
                .execute(
                    "UPDATE memory_proposals SET status='rejected', decided_at=? WHERE id=? AND status='proposed'",
                    rusqlite::params![now, id],
                )
                .map_err(|e| e.to_string())?;
            drop(conn);
            if n == 0 {
                return Err("propuesta no encontrada o ya decidida".to_string());
            }
            let _ = state.audit.write(EventInput {
                kind: "memory.proposal.rejected",
                actor: &crate::services::identity::current_actor(),
                pane_id: None, card_id: None, correlation_id: None,
                payload: serde_json::json!({"proposal_id": id}),
            });
            Ok(None)
        }
        "accept" | "edit" => {
            let edited = action == "edit";
            let _ = now; // `decided_at` lo fija finalize_claim por su cuenta.

            use crate::services::memory_autocapture::{
                claim_proposal_for_accept, finalize_claim, revert_claim, ClaimResult,
            };

            // (1) CLAIM ATÓMICO `proposed → accepting` + lectura de datos en la misma transacción.
            // Sólo un caller gana; el resto recibe AlreadyTaken y NO crea entry (TOCTOU resuelto).
            let claim = claim_proposal_for_accept(&state.db, &id).map_err(|e| e.to_string())?;
            let ClaimResult::Claimed {
                project_key,
                content: orig_content,
                source_id,
                cli_kind,
                session_id,
                rationale,
                kind: orig_kind,
            } = claim
            else {
                return Err("propuesta no encontrada o ya decidida".to_string());
            };

            // (2) Construir y persistir el entry (fuera del lock; store_memory_full lockea solo).
            let final_content = content.unwrap_or(orig_content);
            // Re-scrub idempotente: nunca un secreto entra al Hub aunque el usuario lo haya tipeado.
            let (scrubbed, _r) = crate::services::cloud_sanitizer::sanitize(&final_content);
            let final_kind = kind.or(orig_kind);
            let prov = crate::services::memory_daemon::MemoryProvenance {
                project_key,
                source: cli_kind.clone().unwrap_or_else(|| "autocapture".to_string()),
                source_id,
                cli_kind,
                session_id,
                rationale,
                kind: final_kind,
            };
            let entry_id = match crate::services::memory_daemon::store_memory_full(&state.db, &prov, &scrubbed) {
                Ok(eid) => eid,
                Err(e) => {
                    // Revertir el claim para que la propuesta sea reintentable.
                    let _ = revert_claim(&state.db, &id);
                    return Err(e.to_string());
                }
            };

            // (3) Transición final `accepting → accepted|edited`.
            finalize_claim(&state.db, &id, edited).map_err(|e| e.to_string())?;

            let _ = state.audit.write(EventInput {
                kind: "memory.proposal.accepted",
                actor: &crate::services::identity::current_actor(),
                pane_id: None, card_id: None, correlation_id: None,
                payload: serde_json::json!({"proposal_id": id, "entry_id": entry_id, "edited": edited}),
            });
            Ok(Some(entry_id))
        }
        other => Err(format!("acción inválida: {other} (esperaba accept|reject|edit)")),
    }
}

/// 023 F0 — forget por PROYECTO: borra TODAS las entries del proyecto del Hub. Acción
/// DESTRUCTIVA (gate de confirmación lo maneja la UI). `__shared__` sólo con `include_shared`.
#[tauri::command]
pub fn memory_forget_project(
    state: State<'_, AppState>,
    project_key: String,
    include_shared: Option<bool>,
) -> Result<usize, String> {
    let include_shared = include_shared.unwrap_or(false);
    let deleted = crate::services::memory_daemon::forget_project(
        &state.db,
        &project_key,
        include_shared,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "memory.forget_project",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"project_key": project_key, "include_shared": include_shared, "deleted": deleted}),
    });
    Ok(deleted)
}

/// 023 F1 — estado actual de los settings de auto-captura (para la UI de la bandeja/ajustes).
#[derive(Debug, serde::Serialize)]
pub struct AutocaptureSettings {
    pub autocapture: bool,
    pub auto_accept: bool,
    pub inject: bool,
    pub max_candidates: u32,
}

/// 023 F1 — lee los settings de memoria (default-OFF). La escritura va por `settings_set_validated`
/// (registry tipado), este comando es sólo lectura agregada para la bandeja.
#[tauri::command]
pub fn memory_autocapture_settings(
    state: State<'_, AppState>,
) -> Result<AutocaptureSettings, String> {
    let conn = state.db.lock();
    let read_bool = |k: &str| {
        crate::settings::get(&conn, k)
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let max = crate::settings::get(&conn, "memory.autocapture_max_candidates")
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .map(|n| n as u32)
        .filter(|n| *n >= 1)
        .unwrap_or(5);
    Ok(AutocaptureSettings {
        autocapture: read_bool("memory.autocapture"),
        auto_accept: read_bool("memory.autocapture_auto_accept"),
        inject: read_bool("memory.inject"),
        max_candidates: max,
    })
}

/// 025 F1 — una lección procedural ACTIVA para listar/inyectar (dry-run de la sub-vista).
#[derive(Debug, serde::Serialize)]
pub struct ActiveLessonDto {
    pub entry_id: String,
    pub project_key: String,
    pub content: String,
    pub created_at: String,
    pub active: bool,
    // 050 FR-002 — feedback de utilidad (advisory; NO afecta `active`). Por defecto en cero / sin voto.
    pub useful_count: i64,
    pub not_useful_count: i64,
    /// "useful" | "not_useful" | "" (sin voto aún).
    pub last_vote: String,
}

/// 025 F1 — resultado de `lessons_active_list`: la lista + el TEXTO LITERAL del bloque que se
/// inyectaría (preview byte a byte, council v2 §5). NO inyecta nada.
#[derive(Debug, serde::Serialize)]
pub struct LessonsActiveView {
    pub lessons: Vec<ActiveLessonDto>,
    /// Bloque literal que se inyectaría con los settings actuales (None si no hay activas).
    pub injected_block: Option<String>,
    pub token_budget: u32,
}

/// Lee el presupuesto de tokens de inyección (default 1200).
fn procedural_token_budget(conn: &rusqlite::Connection) -> usize {
    crate::settings::get(conn, "memory.procedural_inject_max")
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .map(|n| n as usize)
        .filter(|n| *n >= 100)
        .unwrap_or(crate::services::procedural_gotchas::DEFAULT_INJECT_TOKEN_BUDGET)
}

/// 025 F1 (FR-015 / SC-005) — DRY-RUN: lista las lecciones procedurales aprobadas del proyecto con
/// su estado de activación + el TEXTO LITERAL del bloque que se inyectaría. NO inyecta nada.
#[tauri::command]
pub fn lessons_active_list(
    state: State<'_, AppState>,
    project_key: Option<String>,
) -> Result<LessonsActiveView, String> {
    use crate::services::procedural_gotchas as pg;
    let pk = project_key.unwrap_or_else(|| "__global__".to_string());
    let lessons = pg::list_active_lessons(&state.db, &pk).map_err(|e| e.to_string())?;
    let budget = {
        let conn = state.db.lock();
        procedural_token_budget(&conn)
    };
    let injected_block = pg::build_lessons_block(&lessons, budget);
    // 050 FR-002 — feedback de utilidad por lección (map entry_id → conteos). Best-effort: si la tabla
    // falla, el map queda vacío y los conteos caen a 0 (la lista de lecciones NUNCA se rompe por esto).
    let feedback = pg::load_lesson_feedback(&state.db, &pk);
    let dtos = lessons
        .into_iter()
        .map(|l| {
            let fb = feedback.get(&l.entry_id).cloned().unwrap_or_default();
            ActiveLessonDto {
                entry_id: l.entry_id,
                project_key: l.project_key,
                content: l.content,
                created_at: l.created_at,
                active: l.active,
                useful_count: fb.useful_count,
                not_useful_count: fb.not_useful_count,
                last_vote: fb.last_vote,
            }
        })
        .collect();
    Ok(LessonsActiveView {
        lessons: dtos,
        injected_block,
        token_budget: budget as u32,
    })
}

/// 025 F1 (LOW del audit 3-frontera) — resuelve el project_key del directorio dado (típicamente el
/// cwd del spawn actual), reusando la MISMA lógica que la inyección (`resolve_project_key_for_cwd`:
/// el `projects.path` más largo que sea prefijo del cwd, o el cwd canónico). Así la sub-vista de
/// Lecciones puede defaultear al proyecto que de verdad se va a inyectar, en vez de pedir un
/// project_key a mano. Si la UI no pasa cwd, cae al cwd del proceso Furx; si tampoco, `__global__`.
#[tauri::command]
pub fn lessons_current_project_key(
    state: State<'_, AppState>,
    cwd: Option<String>,
) -> Result<String, String> {
    // Preferir el cwd que pase la UI (el del pane activo). Si no hay, caer al cwd del proceso Furx
    // (mejor que pedir un project_key a mano). Si tampoco, `__global__`.
    let resolved_cwd = cwd
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()));
    match resolved_cwd {
        Some(c) => Ok(resolve_project_key_for_cwd(&state.db, &c)),
        None => Ok("__global__".to_string()),
    }
}

/// 025 F1 (FR-010) — activa/desactiva una lección para la inyección (reversible, sin borrarla).
#[tauri::command]
pub fn lesson_set_active(
    state: State<'_, AppState>,
    entry_id: String,
    project_key: String,
    active: bool,
) -> Result<(), String> {
    crate::services::procedural_gotchas::set_lesson_active(&state.db, &entry_id, &project_key, active)
        .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "memory.lesson.set_active",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"entry_id": entry_id, "project_key": project_key, "active": active}),
    });
    Ok(())
}

/// 025 F1 (FR-018 / SC-006) — borra una lección del Hub (DESTRUCTIVO; el gate universal de
/// confirmación lo maneja la UI) + limpia su fila de activación. Queda en el audit log.
#[tauri::command]
pub fn lesson_delete(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<bool, String> {
    let deleted = crate::services::memory_daemon::forget_entry(&state.db, &entry_id)
        .map_err(|e| e.to_string())?;
    {
        let conn = state.db.lock();
        let _ = conn.execute(
            "DELETE FROM lesson_activation WHERE entry_id = ?",
            rusqlite::params![entry_id],
        );
    }
    let _ = state.audit.write(EventInput {
        kind: "memory.lesson.deleted",
        actor: &crate::services::identity::current_actor(),
        pane_id: None, card_id: None, correlation_id: None,
        payload: serde_json::json!({"entry_id": entry_id, "deleted": deleted}),
    });
    Ok(deleted)
}

/// 050 Ola 8 P2 (FR-002) — registra feedback de utilidad sobre una lección procedural ("¿fue útil?").
/// ADVISORY (foco humano): suma un voto útil/no-útil pero NUNCA auto-desactiva ni borra la lección —
/// la activación sigue siendo decisión humana vía `lesson_set_active`. Queda en el audit log. Devuelve
/// el conteo actualizado para que la UI refleje sin re-fetch.
#[tauri::command]
pub fn lesson_record_feedback(
    state: State<'_, AppState>,
    entry_id: String,
    project_key: String,
    useful: bool,
) -> Result<crate::services::procedural_gotchas::LessonFeedback, String> {
    let fb = crate::services::procedural_gotchas::record_lesson_feedback(
        &state.db,
        &entry_id,
        &project_key,
        useful,
    )
    .map_err(|e| e.to_string())?;
    let _ = state.audit.write(EventInput {
        kind: "memory.lesson.feedback",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"entry_id": entry_id, "project_key": project_key, "useful": useful}),
    });
    Ok(fb)
}

// ── 048 Cost-Router Fase 1 (Savings Meter) — comandos read-only del dashboard ───────────────────
//
// MIDE el ahorro del routing que Furx YA hace (local/free/premium). NO desvía. Todos read-only: el
// frontend NUNCA escribe filas (tamper-proof, las trazas las emite el backend). Gating:
//   - kill-switch env `FURX_COST_ROUTER` OFF ⇒ status `off`.
//   - setting local `cost_router.tier_meter_on` OFF (default) ⇒ Free tier ⇒ status `off`.
// El copy del dashboard muestra SOLO lo medido (nunca proyecta) — esa garantía vive en el front.

/// ¿El meter está habilitado para el tier del user? Setting local `cost_router.tier_meter_on`
/// (default `false` = Free/off). La app lo prende para Pro/Team.
fn savings_tier_meter_on(state: &State<'_, AppState>) -> bool {
    let conn = state.db.lock();
    crate::settings::get(&conn, "cost_router.tier_meter_on")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Estado del meter (para gatear la UI). No expone montos. Removido del handler+registry (053):
/// duplicado exacto de savings_summary. Se conserva la fn como referencia.
#[allow(dead_code)]
#[tauri::command]
pub fn savings_status(
    state: State<'_, AppState>,
) -> Result<crate::services::savings_meter::SavingsSummary, String> {
    // El status se deriva del summary (mismo gating); el front usa `status`/`eta_days`/`days_observed`.
    let on = savings_tier_meter_on(&state);
    let conn = state.db.lock();
    Ok(crate::services::savings_meter::compute_summary(&conn, on, 30))
}

/// Resumen agregado del ahorro (SOLO lo medido). Free/off ⇒ montos en 0 + status `off`.
#[tauri::command]
pub fn savings_summary(
    state: State<'_, AppState>,
) -> Result<crate::services::savings_meter::SavingsSummary, String> {
    let on = savings_tier_meter_on(&state);
    let conn = state.db.lock();
    Ok(crate::services::savings_meter::compute_summary(&conn, on, 30))
}

/// Serie temporal del ahorro (bucket "day"|"week"). Free/off ⇒ vacío.
#[tauri::command]
pub fn savings_series(
    state: State<'_, AppState>,
    bucket: String,
) -> Result<Vec<crate::services::savings_meter::SavingsBucket>, String> {
    // El gating (Free/off + kill-switch) lo aplica `compute_series` internamente, simétrico con
    // `compute_summary` (audit: consistencia de gating entre los 3 comandos).
    let on = savings_tier_meter_on(&state);
    let conn = state.db.lock();
    Ok(crate::services::savings_meter::compute_series(
        &conn, on, &bucket, 30,
    ))
}

// ── 049 Cost-Router Fase 2 (Router ACTIVO) — comandos del router que DESVÍA ──────────────────────
//
// El router está APAGADO detrás de `FURX_COST_ROUTER_MODE` (default `off` ⇒ no-op). Estos comandos
// SOLO exponen el ESTADO (read-only) y la recarga de policy (autenticada). No desvían nada. El
// dashboard reusa el summary de ahorro de Fase 1 (solo lo medido, nunca proyecta).

/// Estado del router para el dashboard Team/Enterprise: modo (off/shadow/active), gate de KPI,
/// policy cargada, y el resumen de ahorro medido de Fase 1. Read-only.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CostRouterStatus {
    pub mode: crate::services::cost_router::RouterMode,
    pub gate_passed: bool,
    pub gate: crate::services::cost_router::KpiGate,
    pub policy_loaded: bool,
    pub savings: crate::services::savings_meter::SavingsSummary,
    /// 052 — estado del clasificador v2 (read-only). El router v2 NO desvía en esta fase (OFF por flag).
    pub v2: CostRouterV2Status,
}

/// 052 — estado read-only del clasificador v2 (bandit-ready). Informativo: el desvío real está OFF.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CostRouterV2Status {
    /// La config default v1 valida al boot (pesos suman 1.0, thresholds bien-formados).
    pub config_valid: bool,
    pub classifier_version: u32,
    /// Fase derivada del modo + gate de KPI (off/log_only/active). `canary` no se deriva del flag.
    pub phase: crate::services::cost_router_v2::Phase,
    /// Gate de salida del canary (72h + 50 outcomes locales + ambiguous<15%). Sin datos ⇒ no pasa.
    pub canary_gate_passed: bool,
}

/// Estado del router (read-only). NO desvía. El `gate` se deriva de las trazas de Fase 1; sin datos
/// productivos ⇒ no pasa (warming_up) ⇒ el router activo es no-op. El `savings` muestra SOLO lo
/// medido.
#[tauri::command]
pub fn cost_router_status(state: State<'_, AppState>) -> Result<CostRouterStatus, String> {
    let mode = crate::services::cost_router::installed_mode();
    // La policy default embebida siempre carga (fail-closed solo si el archivo externo —no usado por
    // default— fallara). En esta fase reportamos `true` porque el default v1 está embebido.
    let policy_loaded = true;
    let on = savings_tier_meter_on(&state);
    let conn = state.db.lock();
    let gate = crate::services::cost_router::KpiGate::evaluate(&conn);
    let savings = crate::services::savings_meter::compute_summary(&conn, on, 30);
    // 052 — estado del clasificador v2 (read-only). La config default v1 se valida acá; la fase se
    // deriva del modo + gate; el canary gate agrega las trazas de v2 (sin datos ⇒ no pasa).
    let v2_cfg = crate::services::cost_router_v2::RouterConfig::default_v1();
    let v2 = CostRouterV2Status {
        config_valid: v2_cfg.validate().is_ok(),
        classifier_version: v2_cfg.version,
        phase: crate::services::cost_router_v2::Phase::from_mode(mode, gate.passed),
        canary_gate_passed: crate::services::cost_router_v2::CanaryGate::evaluate(&conn).passed,
    };
    Ok(CostRouterStatus {
        mode,
        gate_passed: gate.passed,
        gate,
        policy_loaded,
        savings,
        v2,
    })
}

/// Resultado de recargar la policy del router.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyReloadResult {
    pub ok: bool,
    pub loaded_from: &'static str,
    pub version: u32,
    pub error: Option<String>,
}

/// Recarga AUTENTICADA de la policy del router (council §P3: NUNCA file-watch). En esta fase recarga
/// el default embebido (la policy v1). El gate universal de confirmación de la UI (Risk::Credential +
/// requires_confirmation en el registry) cubre la autorización. Fail-closed: nunca rompe el router.
#[tauri::command]
pub fn cost_router_policy_reload(state: State<'_, AppState>) -> Result<PolicyReloadResult, String> {
    // Reconstruir el default v1 valida que la policy es bien-formada (las regex compilan).
    let policy = crate::services::cost_router::RouterPolicy::default_v1();
    let _ = state.audit.write(EventInput {
        kind: "cost_router.policy.reloaded",
        actor: &crate::services::identity::current_actor(),
        pane_id: None,
        card_id: None,
        correlation_id: None,
        payload: serde_json::json!({"version": policy.version, "loaded_from": "embedded_default"}),
    });
    Ok(PolicyReloadResult {
        ok: true,
        loaded_from: "embedded_default",
        version: policy.version,
        error: None,
    })
}
// ── 050 Ola 8 P2 (FR-003) — Reliability board ─────────────────────────────────────────────────
//
// Dashboard read-only de CALIDAD (distinto del savings de cost-router): tasa de éxito / latencia /
// costo por agente y por modelo, agregando `reliability_events` (055). Opt-in: si el board está OFF
// (default), devuelve `enabled=false` + filas vacías → cero regresión. Solo-medido, sin proyección.

/// Resumen del reliability board en la ventana indicada (`window_days`, default 30). OFF ⇒ vacío.
#[tauri::command]
pub fn reliability_summary(
    state: State<'_, AppState>,
    window_days: Option<i64>,
) -> Result<crate::services::reliability::ReliabilitySummary, String> {
    let days = window_days.filter(|d| *d > 0).unwrap_or(30);
    let conn = state.db.lock();
    Ok(crate::services::reliability::compute_summary(&conn, days))
}

#[cfg(test)]
mod tests {
    use super::validate_snooze_until;
    use super::{diff_has_real_content, simple_ranking_explanation};

    // ── 026 F1 — passthrough de la explicación cuando no hay features (sin base) ──
    #[test]
    fn simple_ranking_explanation_mirrors_base_order() {
        // Reflejar el orden de 020 sin inyección de prior: rank 0 = mejor (base_score 1.0).
        let order = vec![2usize, 0usize, 1usize];
        let r = simple_ranking_explanation(&order, 3);
        assert_eq!(r.order, vec![2, 0, 1]);
        assert!(r.inject_disabled, "sin features ⇒ inyección no aplicada");
        assert!(r.still_learning);
        // la variante en rank 0 (idx 2) tiene el base_score más alto; rank 2 (idx 1) el más bajo.
        assert!(r.variants[2].base_score > r.variants[1].base_score);
        // sin prior ⇒ ningún factor (no opaco, simplemente vacío de prior).
        assert!(r.variants.iter().all(|v| v.factors.is_empty()));
        assert!(r.variants.iter().all(|v| (v.prior_score - 0.5).abs() < 1e-9));
    }

    #[test]
    fn simple_ranking_explanation_single_variant() {
        let r = simple_ranking_explanation(&[0], 1);
        assert_eq!(r.order, vec![0]);
        assert_eq!(r.variants[0].base_score, 1.0);
    }

    #[test]
    fn diff_has_real_content_rejects_placeholders() {
        assert!(diff_has_real_content("diff --git a/x b/x\n+real\n"));
        assert!(!diff_has_real_content(""));
        assert!(!diff_has_real_content("(sin cambios)"));
        assert!(!diff_has_real_content("(la variante aún no se lanzó)"));
    }

    #[test]
    fn accepts_front_sqlite_utc_format() {
        // El formato que manda computeSnoozeUntil: `YYYY-MM-DD HH:MM:SS` UTC, sin zona.
        assert_eq!(
            validate_snooze_until("2026-06-01 13:00:00").unwrap(),
            "2026-06-01 13:00:00"
        );
    }

    #[test]
    fn rfc3339_with_zone_is_converted_to_utc() {
        // Con `Z` (ya UTC) y con offset (se convierte) → canónico `YYYY-MM-DD HH:MM:SS` en UTC.
        assert_eq!(
            validate_snooze_until("2026-06-01T13:00:00Z").unwrap(),
            "2026-06-01 13:00:00"
        );
        // +02:00 → 11:00 UTC.
        assert_eq!(
            validate_snooze_until("2026-06-01T13:00:00+02:00").unwrap(),
            "2026-06-01 11:00:00"
        );
    }

    #[test]
    fn rejects_impossible_dates() {
        // Mes 13 / día 32 / hora 25: antes pasaban (sólo se chequeaba forma); ahora se rechazan.
        assert!(validate_snooze_until("2026-13-01 00:00:00").is_err());
        assert!(validate_snooze_until("2026-02-30 00:00:00").is_err());
        assert!(validate_snooze_until("2026-06-01 25:00:00").is_err());
    }

    #[test]
    fn rejects_garbage_and_empty() {
        assert!(validate_snooze_until("").is_err());
        assert!(validate_snooze_until("not-a-date").is_err());
        assert!(validate_snooze_until("2026-06-01").is_err()); // sin hora
    }

    #[test]
    fn output_is_lexicographically_comparable() {
        // El canónico debe ordenar igual que el tiempo real (mismo formato que datetime('now')).
        let a = validate_snooze_until("2026-06-01T09:00:00Z").unwrap();
        let b = validate_snooze_until("2026-06-01 10:00:00").unwrap();
        assert!(a < b, "{} < {}", a, b);
    }
}
