// services/process_manager.rs — 015-frontend-reform-kernel · US5
// (Headless Process / Task Lifecycle Manager).
//
// REGISTRO CENTRAL de procesos/jobs que viven en el BACKEND y SOBREVIVEN a
// unmount/reload/cierre de ventana de la UI.
//
// Problema que resuelve: hoy un proceso PTY está, de facto, atado al ciclo de vida
// de un pane de React — si el pane se desmonta o la ventana se cierra, la vista del
// proceso desaparece y, sin una capa de ownership, es fácil terminar matándolo. US5
// DESACOPLA eso: el proceso es PROPIEDAD del backend; la UI es un VIEWPORT que lo
// observa/controla. El registro persiste { process_id, kind, owner_context, status,
// progress, started_at } en la tabla `process_registry` (migración 027).
//
// Invariantes (acceptance US5):
//   1. CANCELLATION EXPLÍCITA. La ÚNICA forma de terminar un proceso vía este módulo
//      es `cancel(process_id)`. NO existe "cancelar al cerrar la ventana": desmontar
//      un pane / cerrar una ventana es una operación de la UI que NO toca el registro.
//      `detach_viewport()` lo modela: deja la fila intacta en `running`.
//   2. REATTACH / RESUMABILITY. Una UI que se re-suscribe pide `attach(process_id)`
//      (o `list()`) y recibe el estado vigente desde el registro persistido — el
//      proceso siguió vivo en el backend mientras la vista no existía. La rehidratación
//      por evento la maneja el event_bus (AppEvent::TaskChanged/AgentStateChanged).
//   3. REUSA el PtyManager existente (src/pty.rs). Este módulo NO reimplementa PTY:
//      es la capa de OWNERSHIP/LIFECYCLE por encima. `cancel` de un proceso `kind=pty`
//      delega el kill real al PtyManager vía el `external_ref` (= pane_id). El kill del
//      recurso real lo hace el CALLER (commands.rs, que tiene el PtyManager) tras
//      `cancel` marcar el estado — así el servicio queda testeable sin Tauri/PtyManager.
//
// Coexiste con orchestration (008/014): aquellos son el nivel TAREA (worktree+branch+
// agente). Este es el nivel PROCESO. Un OrchTask puede registrar aquí su proceso PTY
// con `owner_context = task_id` para que el lifecycle de proceso sea uniforme.

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// 015 T014 (audit final) — contador monotónico de GENERACIONES de proceso. Cada spawn toma uno
/// y lo lleva la fila (`run_token`) + la `PtySession` + su wait-thread; `finish` se scopea por él
/// para que el wait-thread de un run viejo NO marque terminal la fila de un run nuevo que reusó el
/// mismo pane_id (race hijack, codex+gemini HIGH). Reinicia a 1 en cada arranque de la app: sólo
/// necesita ser único por (process_id) DENTRO de una sesión, y `finish` siempre filtra por process_id.
static RUN_SEQ: AtomicI64 = AtomicI64::new(1);

/// Reserva el próximo run_token (monotónico dentro del proceso). Lo llama `pty_spawn` ANTES de
/// `register`, y lo pasa también a `PtyManager::spawn` para que la sesión y su wait-thread lo lleven.
pub fn next_run_token() -> i64 {
    RUN_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// Estados terminales: un proceso en uno de estos NO se puede re-cancelar.
pub const TERMINAL: &[&str] = &["done", "failed", "canceled"];

/// Clase de proceso registrado. `pty` = sesión del PtyManager; `job` = job de
/// background; `agent` = agente autónomo (orchestration). String-backed para que el
/// espejo TS sea trivial y la tabla legible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum ProcessKind {
    #[default]
    Pty,
    Job,
    Agent,
}

impl ProcessKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProcessKind::Pty => "pty",
            ProcessKind::Job => "job",
            ProcessKind::Agent => "agent",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "pty" => Ok(ProcessKind::Pty),
            "job" => Ok(ProcessKind::Job),
            "agent" => Ok(ProcessKind::Agent),
            other => Err(anyhow!("kind de proceso inválido: {other}")),
        }
    }
}

/// Fila del registro = un proceso/job que vive en el backend. Espejo de
/// `web/src/lib/processManager.ts` (`ProcessInfo`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessInfo {
    pub process_id: String,
    /// "pty" | "job" | "agent".
    pub kind: String,
    /// Contexto de origen (window_id/pane_id/task_id). SÓLO informativo: su muerte
    /// (cerrar ventana) NO cancela el proceso.
    pub owner_context: Option<String>,
    /// Referencia al recurso real (pane_id del PtyManager / task_id / job_id) para
    /// reconciliar al cancelar/reatachar.
    pub external_ref: Option<String>,
    /// "running" | "done" | "failed" | "canceled".
    pub status: String,
    /// Progreso 0.0..1.0 (opcional).
    pub progress: Option<f64>,
    pub label: Option<String>,
    pub started_at: String,
    pub updated_at: String,
    /// 015 T014 (audit final) — generación vigente de la fila. El caller de `register` la compara
    /// con el token que reservó: si difiere, OTRO spawn concurrente ganó este process_id y el
    /// caller debe abortar (no instalar una 2da generación sobre una fila ajena).
    pub run_token: Option<i64>,
}

/// Datos para registrar un proceso nuevo. `process_id` se genera si no se provee.
#[derive(Debug, Clone, Default)]
pub struct RegisterSpec {
    /// Si `None`, se genera un UUID. Útil pasar uno determinístico (p.ej. el pane_id).
    pub process_id: Option<String>,
    pub kind: ProcessKind,
    pub owner_context: Option<String>,
    pub external_ref: Option<String>,
    pub label: Option<String>,
    pub progress: Option<f64>,
    /// 015 T014 (audit final) — generación de ESTE spawn (de `next_run_token`). La fila lo guarda
    /// para que `finish` se scopee por generación. `None` sólo en call-sites legacy/tests.
    pub run_token: Option<i64>,
}


const COLS: &str = "process_id, kind, owner_context, external_ref, status, progress, label, started_at, updated_at, run_token";

fn row_to_info(r: &rusqlite::Row) -> rusqlite::Result<ProcessInfo> {
    Ok(ProcessInfo {
        process_id: r.get(0)?,
        kind: r.get(1)?,
        owner_context: r.get(2)?,
        external_ref: r.get(3)?,
        status: r.get(4)?,
        progress: r.get(5)?,
        label: r.get(6)?,
        started_at: r.get(7)?,
        updated_at: r.get(8)?,
        run_token: r.get(9)?,
    })
}

/// Registra un proceso como `running` por `process_id` (UPSERT). Semántica:
///   - si NO existe → inserta `running` (started_at = now).
///   - si existe y está TERMINAL (done/failed/canceled) → es un RE-spawn del mismo id
///     (p.ej. el front remonta el mismo pane.id al cambiar mode/cwd): REINICIA la fila a
///     `running` con un `started_at` fresco y refresca la metadata. Sin esto, el nuevo PTY
///     correría bajo una fila terminal → desync SSOT (audit 015 T014, 4 voces HIGH).
///   - si existe y está `running` → NO-OP (idempotente: doble-invoke de un re-mount NO
///     reinicia un proceso vivo ni le pisa el started_at/progress).
/// El reap del PTY viejo (matar el child para no leakear el OS process) lo hace el CALLER
/// (`pty_spawn` vía `cancel_and_reap` ANTES de registrar). Devuelve el `ProcessInfo` persistido.
pub fn register(db: &Db, spec: RegisterSpec) -> Result<ProcessInfo> {
    let id = spec
        .process_id
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // UPSERT: inserta running, o si ya existe y está TERMINAL reinicia a running (nuevo run);
    // si está running, sólo refresca metadata sin tocar started_at/progress (no-op efectivo).
    conn.execute(
        "INSERT INTO process_registry \
         (process_id, kind, owner_context, external_ref, status, progress, label, started_at, updated_at, run_token) \
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7, ?7, ?8) \
         ON CONFLICT(process_id) DO UPDATE SET \
           kind = excluded.kind, \
           owner_context = excluded.owner_context, \
           external_ref = excluded.external_ref, \
           label = excluded.label, \
           status = 'running', \
           updated_at = excluded.updated_at, \
           started_at = CASE WHEN process_registry.status IN ('done','failed','canceled') \
                             THEN excluded.started_at ELSE process_registry.started_at END, \
           progress  = CASE WHEN process_registry.status IN ('done','failed','canceled') \
                             THEN excluded.progress  ELSE process_registry.progress  END, \
           run_token = CASE WHEN process_registry.status IN ('done','failed','canceled') \
                             THEN excluded.run_token ELSE process_registry.run_token END",
        params![
            id,
            spec.kind.as_str(),
            spec.owner_context,
            spec.external_ref,
            spec.progress,
            spec.label,
            now,
            spec.run_token,
        ],
    )?;
    let sql = format!("SELECT {COLS} FROM process_registry WHERE process_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let info = stmt.query_row(params![id], row_to_info)?;
    Ok(info)
}

/// Lista procesos. `only_running = true` → sólo los vivos (lo que la UI muestra al
/// reattach). `false` → todos (historial). Orden: vivos primero, luego por started_at desc.
pub fn list(db: &Db, only_running: bool) -> Result<Vec<ProcessInfo>> {
    let conn = db.lock();
    let sql = if only_running {
        format!(
            "SELECT {COLS} FROM process_registry WHERE status = 'running' ORDER BY started_at DESC"
        )
    } else {
        format!(
            "SELECT {COLS} FROM process_registry \
             ORDER BY (status = 'running') DESC, started_at DESC"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], row_to_info)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Devuelve un proceso por id (None si no existe). Es el "attach": la UI que se
/// re-suscribe pide el estado vigente del proceso (que siguió vivo en el backend).
pub fn get(db: &Db, process_id: &str) -> Result<Option<ProcessInfo>> {
    let conn = db.lock();
    let sql = format!("SELECT {COLS} FROM process_registry WHERE process_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![process_id], row_to_info)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// ATTACH (reattach/resumability): igual que `get` pero error si el proceso no existe.
/// La UI lo llama al re-montar el viewport para rehidratar el estado del proceso vivo.
pub fn attach(db: &Db, process_id: &str) -> Result<ProcessInfo> {
    get(db, process_id)?.ok_or_else(|| anyhow!("proceso no encontrado: {process_id}"))
}

/// Actualiza el progreso/estado-vivo de un proceso `running`. No-op si ya es terminal.
pub fn set_progress(db: &Db, process_id: &str, progress: f64) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE process_registry SET progress = ?2, updated_at = ?3 \
         WHERE process_id = ?1 AND status = 'running'",
        params![process_id, progress, now],
    )?;
    Ok(())
}

/// Marca un proceso como terminado por el propio backend (NO por el usuario): el PTY
/// salió, el job completó, etc. `status` ∈ {done, failed}. Idempotente: no re-pisa un
/// estado ya terminal (p.ej. un `canceled` previo gana).
///
/// `run_token` (015 T014 audit final): si `Some(t)`, el UPDATE se scopea `AND run_token = t`
/// → un wait-thread de un run VIEJO (token N) NO marca terminal la fila de un run NUEVO que
/// reusó el mismo pane_id (token N+1) en la ventana de la race. Si `None`, sólo filtra por
/// status (compat legacy / tests). El wait-thread SIEMPRE pasa su token.
pub fn finish(db: &Db, process_id: &str, status: &str, run_token: Option<i64>) -> Result<()> {
    if !matches!(status, "done" | "failed") {
        return Err(anyhow!(
            "finish status inválido: {status} (esperaba done|failed)"
        ));
    }
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE process_registry SET status = ?2, updated_at = ?3 \
         WHERE process_id = ?1 AND status = 'running' AND (?4 IS NULL OR run_token = ?4)",
        params![process_id, status, now, run_token],
    )?;
    Ok(())
}

/// Resultado de `cancel`: el estado tras la cancelación + el `external_ref`/`kind` para
/// que el CALLER (commands.rs) mate el recurso real (PtyManager.kill / job abort).
#[derive(Debug, Clone, PartialEq)]
pub struct CancelOutcome {
    pub info: ProcessInfo,
    /// True si ESTA llamada transicionó running→canceled (vs. ya estaba terminal).
    pub newly_canceled: bool,
}

/// CANCELLATION EXPLÍCITA (única forma de terminar vía este módulo). Marca el proceso
/// `canceled` si estaba `running`. NO mata el recurso real: devuelve `external_ref`+`kind`
/// en el `CancelOutcome` para que el caller delegue el kill al PtyManager (kind=pty) o al
/// job runner. Idempotente: cancelar uno ya terminal devuelve `newly_canceled=false` sin error.
pub fn cancel(db: &Db, process_id: &str) -> Result<CancelOutcome> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    let changed = conn.execute(
        "UPDATE process_registry SET status = 'canceled', updated_at = ?2 \
         WHERE process_id = ?1 AND status = 'running'",
        params![process_id, now],
    )?;
    let sql = format!("SELECT {COLS} FROM process_registry WHERE process_id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![process_id], row_to_info)?;
    let info = match rows.next() {
        Some(r) => r?,
        None => return Err(anyhow!("proceso no encontrado: {process_id}")),
    };
    Ok(CancelOutcome {
        info,
        newly_canceled: changed == 1,
    })
}

/// DETACH del viewport — modela "cerrar la ventana / desmontar el pane". Es un NO-OP
/// deliberado sobre el ciclo de vida del proceso: la vista se va, el proceso QUEDA.
/// Existe como función con nombre para documentar y testear la invariante US5: detach
/// NO cancela. (Si en el futuro la UI quiere registrar "qué viewport observa qué proceso"
/// para telemetry, este es el lugar — pero NUNCA debe tocar `status`.)
pub fn detach_viewport(_db: &Db, _process_id: &str, _viewport_id: &str) -> Result<()> {
    // Intencionalmente vacío: el proceso es propiedad del backend, no del viewport.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// DB en memoria con SÓLO la tabla 027 (aislado del set completo de migraciones).
    fn mem_db() -> Db {
        let conn = Connection::open_in_memory().expect("open mem db");
        conn.execute_batch(include_str!("../../migrations/027_process_registry.sql"))
            .expect("apply 027");
        // 015 T014 (audit final): la columna run_token (scope de generación de finish).
        conn.execute_batch(include_str!("../../migrations/029_process_run_token.sql"))
            .expect("apply 029");
        Arc::new(parking_lot::Mutex::new(conn))
    }

    #[test]
    fn register_list_and_get() {
        let db = mem_db();
        let info = register(
            &db,
            RegisterSpec {
                kind: ProcessKind::Pty,
                owner_context: Some("pane-7".into()),
                external_ref: Some("pane-7".into()),
                label: Some("claude".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(info.status, "running");
        assert_eq!(info.kind, "pty");
        assert!(!info.process_id.is_empty());

        let listed = list(&db, true).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].process_id, info.process_id);

        let got = get(&db, &info.process_id).unwrap().unwrap();
        assert_eq!(got, info);
        assert!(get(&db, "nope").unwrap().is_none());
    }

    #[test]
    fn register_is_idempotent_by_id() {
        let db = mem_db();
        let a = register(
            &db,
            RegisterSpec {
                process_id: Some("p1".into()),
                kind: ProcessKind::Job,
                ..Default::default()
            },
        )
        .unwrap();
        // re-registrar el MISMO id (p.ej. un re-mount) no crea un duplicado ni resetea.
        let b = register(
            &db,
            RegisterSpec {
                process_id: Some("p1".into()),
                kind: ProcessKind::Job,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.process_id, b.process_id);
        assert_eq!(list(&db, false).unwrap().len(), 1);
    }

    #[test]
    fn cancel_is_explicit_and_idempotent() {
        let db = mem_db();
        let info = register(
            &db,
            RegisterSpec {
                process_id: Some("p9".into()),
                kind: ProcessKind::Pty,
                external_ref: Some("pane-9".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(info.status, "running");

        let out = cancel(&db, "p9").unwrap();
        assert!(out.newly_canceled, "primera cancelación transiciona");
        assert_eq!(out.info.status, "canceled");
        // el caller usa external_ref para matar el PTY real:
        assert_eq!(out.info.external_ref.as_deref(), Some("pane-9"));

        // idempotente: re-cancelar uno terminal no es error ni re-transiciona.
        let out2 = cancel(&db, "p9").unwrap();
        assert!(!out2.newly_canceled);
        assert_eq!(out2.info.status, "canceled");

        // cancelar un id inexistente SÍ es error (el caller debe saberlo).
        assert!(cancel(&db, "ghost").is_err());
    }

    #[test]
    fn finish_does_not_override_canceled() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("p2".into()),
                ..Default::default()
            },
        )
        .unwrap();
        cancel(&db, "p2").unwrap();
        // un finish que llega DESPUÉS del cancel (race PTY exit) NO debe pisar canceled.
        finish(&db, "p2", "done", None).unwrap();
        assert_eq!(get(&db, "p2").unwrap().unwrap().status, "canceled");
    }

    /// ACCEPTANCE US5 — el corazón de la feature: "cerrar la ventana / desmontar un pane
    /// NO mata el proceso". Simulamos el viewport como un guard que, al dropearse (cerrar
    /// ventana), llama a `detach_viewport`. Tras el drop, el proceso DEBE seguir `running`
    /// en el registro y ser reatachable.
    #[test]
    fn closing_window_does_not_kill_process() {
        let db = mem_db();
        let proc = register(
            &db,
            RegisterSpec {
                process_id: Some("survivor".into()),
                kind: ProcessKind::Agent,
                owner_context: Some("window-A".into()),
                ..Default::default()
            },
        )
        .unwrap();

        // Un "viewport" que observa el proceso. Al cerrarse la ventana, se DROPEA. El Drop
        // modela el unmount/cierre: llama a detach_viewport (NO a cancel).
        struct Viewport<'a> {
            db: &'a Db,
            process_id: String,
            viewport_id: String,
        }
        impl<'a> Drop for Viewport<'a> {
            fn drop(&mut self) {
                // Cerrar la ventana ⇒ detach (no-op sobre lifecycle), NUNCA cancel.
                detach_viewport(self.db, &self.process_id, &self.viewport_id).unwrap();
            }
        }

        {
            let _vp = Viewport {
                db: &db,
                process_id: proc.process_id.clone(),
                viewport_id: "vp-1".into(),
            };
            // ... la UI observa el proceso ...
        } // <- aquí se "cierra la ventana": _vp se dropea.

        // INVARIANTE US5: el proceso sobrevivió al cierre del viewport.
        let after = get(&db, "survivor").unwrap().unwrap();
        assert_eq!(
            after.status, "running",
            "cerrar la ventana NO debe matar el proceso"
        );

        // ... y una NUEVA ventana puede reatacharlo y ver el estado vivo (resumability).
        let reattached = attach(&db, "survivor").unwrap();
        assert_eq!(reattached.status, "running");
        assert_eq!(reattached.owner_context.as_deref(), Some("window-A"));

        // Sólo una cancelación EXPLÍCITA lo termina.
        assert!(cancel(&db, "survivor").unwrap().newly_canceled);
        assert_eq!(get(&db, "survivor").unwrap().unwrap().status, "canceled");
    }

    #[test]
    fn progress_updates_only_while_running() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("p3".into()),
                ..Default::default()
            },
        )
        .unwrap();
        set_progress(&db, "p3", 0.5).unwrap();
        assert_eq!(get(&db, "p3").unwrap().unwrap().progress, Some(0.5));
        cancel(&db, "p3").unwrap();
        // tras cancelar, el progress no se mueve más.
        set_progress(&db, "p3", 0.9).unwrap();
        assert_eq!(get(&db, "p3").unwrap().unwrap().progress, Some(0.5));
    }

    #[test]
    fn kind_roundtrips() {
        for k in [ProcessKind::Pty, ProcessKind::Job, ProcessKind::Agent] {
            assert_eq!(ProcessKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(ProcessKind::parse("bogus").is_err());
    }

    // ── 015 T014 (US5) — invariantes del WIRING (pty_spawn ↔ wait-thread ↔ cancel) ──
    // Estos tests no ejercitan Tauri (el harness es cargo test --lib): blindan, a nivel
    // servicio, las invariantes EXACTAS de las que depende el cableado de T014.

    /// T014 — ciclo de vida del spawn: `pty_spawn` registra `running` ANTES de spawnear; el
    /// wait-thread de pty.rs llama `finish("done"|"failed")` cuando el child sale. La fila debe
    /// terminar en el estado correcto. Cubre tanto el exit normal como el spawn fallido.
    #[test]
    fn t014_spawn_lifecycle_register_then_finish() {
        let db = mem_db();
        // pty_spawn: registra antes de spawnear (process_id == external_ref == pane_id).
        register(
            &db,
            RegisterSpec {
                process_id: Some("pane-ok".into()),
                kind: ProcessKind::Pty,
                owner_context: Some("pane-ok".into()),
                external_ref: Some("pane-ok".into()),
                label: Some("claude".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(get(&db, "pane-ok").unwrap().unwrap().status, "running");
        // wait-thread: el child salió con code 0 → done.
        finish(&db, "pane-ok", "done", None).unwrap();
        assert_eq!(get(&db, "pane-ok").unwrap().unwrap().status, "done");

        // Spawn fallido tras registrar: pty_spawn llama finish("failed").
        register(
            &db,
            RegisterSpec {
                process_id: Some("pane-bad".into()),
                kind: ProcessKind::Pty,
                ..Default::default()
            },
        )
        .unwrap();
        finish(&db, "pane-bad", "failed", None).unwrap();
        assert_eq!(get(&db, "pane-bad").unwrap().unwrap().status, "failed");
    }

    /// T014 (audit) — RE-spawn del mismo pane_id: `register` debe REINICIAR una fila terminal
    /// a `running` (nuevo run) en vez de dejarla terminal (lo que desyncaría el SSOT: el nuevo
    /// PTY corriendo bajo una fila done/canceled). Modela el remount del front (cambia mode/cwd,
    /// mismo pane.id) tras el reap del run anterior.
    #[test]
    fn t014_register_resets_terminal_row_on_respawn() {
        let db = mem_db();
        let first = register(
            &db,
            RegisterSpec {
                process_id: Some("pane".into()),
                kind: ProcessKind::Pty,
                label: Some("claude".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // El run anterior terminó (o fue reapeado/canceled).
        cancel(&db, "pane").unwrap();
        assert_eq!(get(&db, "pane").unwrap().unwrap().status, "canceled");
        // Re-spawn del MISMO pane_id (mode nuevo) → register reinicia a running, fila única.
        let second = register(
            &db,
            RegisterSpec {
                process_id: Some("pane".into()),
                kind: ProcessKind::Pty,
                label: Some("codex".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            second.status, "running",
            "re-spawn debe reiniciar la fila a running"
        );
        assert_eq!(
            second.label.as_deref(),
            Some("codex"),
            "metadata refrescada al nuevo run"
        );
        assert!(
            second.started_at >= first.started_at,
            "started_at fresco en el nuevo run"
        );
        assert_eq!(
            list(&db, false).unwrap().len(),
            1,
            "sigue siendo UNA fila (no duplica)"
        );
        assert_eq!(list(&db, true).unwrap().len(), 1, "y está viva otra vez");
    }

    /// T014 (audit) — re-registrar un proceso AÚN running es un NO-OP: no reinicia started_at
    /// ni le pisa el progress (protege el doble-invoke de un re-mount sobre un proceso vivo).
    #[test]
    fn t014_register_running_reregister_is_noop() {
        let db = mem_db();
        let a = register(
            &db,
            RegisterSpec {
                process_id: Some("live".into()),
                kind: ProcessKind::Pty,
                ..Default::default()
            },
        )
        .unwrap();
        set_progress(&db, "live", 0.42).unwrap();
        // re-register mientras sigue running → no-op sobre started_at/progress.
        let b = register(
            &db,
            RegisterSpec {
                process_id: Some("live".into()),
                kind: ProcessKind::Pty,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            a.started_at, b.started_at,
            "no reinicia started_at de un proceso vivo"
        );
        assert_eq!(
            get(&db, "live").unwrap().unwrap().progress,
            Some(0.42),
            "no pisa el progress vivo"
        );
        assert_eq!(get(&db, "live").unwrap().unwrap().status, "running");
    }

    /// T014 — POR QUÉ se registra ANTES de spawnear (race fix, decisión B del council). Si
    /// `finish` corriera sobre un id aún NO registrado (porque el wait-thread observó el exit
    /// antes de que `register` insertara la fila), el UPDATE no afecta filas y, al insertarse
    /// luego, la fila quedaría `running` para siempre. Registrar primero ELIMINA ese orden.
    #[test]
    fn t014_register_must_precede_finish() {
        let db = mem_db();
        // Orden PATOLÓGICO (el que el impl evita): finish antes de register.
        finish(&db, "racy", "done", None).unwrap(); // UPDATE 0 filas → no-op, no error.
        register(
            &db,
            RegisterSpec {
                process_id: Some("racy".into()),
                kind: ProcessKind::Pty,
                ..Default::default()
            },
        )
        .unwrap();
        // La fila quedó `running` (terminal perdido) — esto es lo que NO queremos.
        assert_eq!(get(&db, "racy").unwrap().unwrap().status, "running");

        // Orden CORRECTO (el del impl: register en pty_spawn ANTES de pty.spawn): register → finish.
        register(
            &db,
            RegisterSpec {
                process_id: Some("ordered".into()),
                kind: ProcessKind::Pty,
                ..Default::default()
            },
        )
        .unwrap();
        finish(&db, "ordered", "done", None).unwrap();
        assert_eq!(get(&db, "ordered").unwrap().unwrap().status, "done");
    }

    /// T014 — `cancel_and_reap` (commands.rs) se apoya en que `cancel` marque el registry
    /// `canceled` y devuelva el `external_ref`+`kind` para reapear el recurso real, y en que un
    /// `finish` posterior (race: el PTY salió mientras se cancelaba) NO pise el `canceled`.
    /// Verifica el contrato exacto que consume el helper de cancelación registry-routed.
    #[test]
    fn t014_cancel_routes_through_registry_then_reaps() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("pane-x".into()),
                kind: ProcessKind::Pty,
                external_ref: Some("pane-x".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // cancel marca canceled y da lo necesario para matar el PTY real (external_ref + kind).
        let out = cancel(&db, "pane-x").unwrap();
        assert!(out.newly_canceled);
        assert_eq!(out.info.status, "canceled");
        assert_eq!(out.info.kind, ProcessKind::Pty.as_str());
        assert_eq!(out.info.external_ref.as_deref(), Some("pane-x"));
        // El wait-thread del PTY que recién murió llama finish: NO debe resucitar a done.
        finish(&db, "pane-x", "done", None).unwrap();
        assert_eq!(get(&db, "pane-x").unwrap().unwrap().status, "canceled");
    }

    /// T014 (audit final) — `finish` se scopea por `run_token`: un token que no coincide con el
    /// de la fila vigente es un NO-OP (no marca terminal una generación que no es la suya).
    /// T014 (audit final, codex HIGH) — dos `pty_spawn` CONCURRENTES sobre el mismo pane_id: el
    /// primer `register` instala su token; el segundo (re-register de una fila `running`) CONSERVA
    /// el token del primero y lo DEVUELVE → el segundo caller detecta que NO ganó (run_token !=
    /// el suyo) y aborta sin spawnear una 2da generación. `register` devuelve el token vigente.
    #[test]
    fn t014_concurrent_register_same_id_keeps_first_token_and_signals_loser() {
        let db = mem_db();
        let a = register(
            &db,
            RegisterSpec {
                process_id: Some("p".into()),
                kind: ProcessKind::Pty,
                run_token: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(a.run_token, Some(1), "el primero instala su token");
        // segundo spawn concurrente con token 2: la fila ya está running → no-op, conserva token 1.
        let b = register(
            &db,
            RegisterSpec {
                process_id: Some("p".into()),
                kind: ProcessKind::Pty,
                run_token: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            b.run_token,
            Some(1),
            "el segundo NO gana: register devuelve el token vigente (1), no el suyo (2)"
        );
        // → pty_spawn del segundo aborta (b.run_token != Some(2)); la fila sigue siendo del primero.
        assert_eq!(list(&db, false).unwrap().len(), 1);
    }

    #[test]
    fn t014_finish_is_scoped_by_run_token() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("p".into()),
                kind: ProcessKind::Pty,
                run_token: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        // token EQUIVOCADO → no-op (la fila sigue running).
        finish(&db, "p", "done", Some(2)).unwrap();
        assert_eq!(get(&db, "p").unwrap().unwrap().status, "running");
        // token correcto → done.
        finish(&db, "p", "done", Some(1)).unwrap();
        assert_eq!(get(&db, "p").unwrap().unwrap().status, "done");
    }

    /// T014 (audit final) — EL CORAZÓN de la race hijack (codex+gemini HIGH): la generación 1
    /// termina; un respawn del mismo pane_id (gen 2) resetea la fila a `running` con token 2; el
    /// wait-thread VIEJO (gen 1) llega tarde a `finish` — scopeado por token 1, NO toca la fila
    /// del run nuevo (token 2). Sin el token, marcaría `done` un run vivo → orphan + desync.
    #[test]
    fn t014_old_wait_thread_cannot_finish_new_generation() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("p".into()),
                kind: ProcessKind::Pty,
                run_token: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        cancel(&db, "p").unwrap(); // el reap del respawn marca la gen vieja canceled
                                   // respawn (gen 2) → UPSERT reinicia la fila a running con token 2.
        register(
            &db,
            RegisterSpec {
                process_id: Some("p".into()),
                kind: ProcessKind::Pty,
                run_token: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(get(&db, "p").unwrap().unwrap().status, "running");
        // wait-thread de la gen 1 finaliza TARDE (token 1) → no-op sobre la gen 2.
        finish(&db, "p", "done", Some(1)).unwrap();
        assert_eq!(
            get(&db, "p").unwrap().unwrap().status,
            "running",
            "el run nuevo (gen 2) NO debe quedar done por el wait-thread de la gen 1"
        );
        // el wait-thread de la gen 2 sí lo finaliza con SU token.
        finish(&db, "p", "done", Some(2)).unwrap();
        assert_eq!(get(&db, "p").unwrap().unwrap().status, "done");
    }

    #[test]
    fn list_running_only_filters_terminal() {
        let db = mem_db();
        register(
            &db,
            RegisterSpec {
                process_id: Some("alive".into()),
                ..Default::default()
            },
        )
        .unwrap();
        register(
            &db,
            RegisterSpec {
                process_id: Some("dead".into()),
                ..Default::default()
            },
        )
        .unwrap();
        cancel(&db, "dead").unwrap();
        let running = list(&db, true).unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].process_id, "alive");
        // todos => 2, vivos primero.
        let all = list(&db, false).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].process_id, "alive");
    }
}
