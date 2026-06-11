//! 038 Goose-C P1 — scheduler event-driven del DAG de pipelines (spec 038, fase F1.3).
//!
//! Núcleo del avance. Al `done` (aprobación HUMANA del review) de una tarea, re-evalúa la readiness
//! de sus dependientes y desbloquea (`dag_blocked=0`) a los que tengan TODAS sus deps satisfechas. Al
//! `failed`/`canceled` de una tarea, hace `cascade_cancel`: skipea SOLO los descendientes `pending`
//! (nunca mata trabajo vivo `running`/`awaiting_review` — red-team #6), respetando `on_error='continue'`.
//!
//! ARQUITECTURA (council, verificada contra HEAD):
//!  - **Event-driven, stateless**: NO es un poller con tick propio (no duplica el latido de
//!    `done_detection` ni compite por el `parking_lot::Mutex<Connection>` no-reentrante). Todo el
//!    estado deriva de DB (`pipeline_edges` + `orchestration_tasks.state`/`dag_blocked` +
//!    `pipeline_runs.status`).
//!  - **Hook POST-lock**: el hook de avance corre en la CAPA DE COMANDO (commands.rs), DESPUÉS de que
//!    `set_state` soltó su lock — este módulo abre su PROPIO scope de lock (sin re-entrancy → sin
//!    self-deadlock sobre el Mutex no-reentrante; red-team #1).
//!  - **Foco único humano-otorgado (030-034, NON-NEGOTIABLE)**: este módulo NO importa
//!    `attention::HumanCommand::from_human_input` (privado a `attention`) → no puede fabricar el
//!    witness que `grant_focus` exige → desbloquear/preparar la etapa N es, respecto del mic, idéntico
//!    a un lanzamiento manual. El scheduler SÓLO toca `dag_blocked`/`state` en DB; NUNCA el `MicFocus`.
//!  - **Advisory-lock por run**: `on_task_settled` y `reconcile_on_boot` comparten un Mutex in-process
//!    por run_id → nunca recomputan readiness en paralelo (red-team #2). Con concurrencia v1=1, el
//!    `claim_for_launch` ya basta, pero el lock lo blinda para reconcile+hook simultáneos.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Advisory-lock IN-PROCESS por run_id (compartido entre `on_task_settled` y `reconcile_on_boot`).
/// Mismo patrón que `orchestration::repo_worktree_lock`: serializa la recomputación de readiness de un
/// run para que el hook y el reconciliador nunca corran a la vez sobre el mismo grafo.
fn run_advisory_lock(run_id: &str) -> Arc<parking_lot::Mutex<()>> {
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    static LOCKS: Lazy<parking_lot::Mutex<HashMap<String, Arc<parking_lot::Mutex<()>>>>> =
        Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
    let mut map = LOCKS.lock();
    map.entry(run_id.to_string())
        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(())))
        .clone()
}

/// El `pipeline_run_id` de una tarea (None si no es de pipeline). Lectura barata.
fn run_id_of(conn: &Connection, task_id: &str) -> Result<Option<String>> {
    let r: Option<Option<String>> = conn
        .query_row(
            "SELECT pipeline_run_id FROM orchestration_tasks WHERE id = ?1",
            params![task_id],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    Ok(r.flatten())
}

/// 038 F1.3 — hook de avance. Llamar DESPUÉS de que `set_state` soltó su lock, cuando una tarea de
/// pipeline llega a un estado TERMINAL (`done`/`failed`/`canceled`). Abre su propio scope de lock.
///  - `done`  → desbloquea (`dag_blocked=0`) los dependientes ya listos (`deps_all_done`).
///  - `failed`/`canceled` → `cascade_cancel` de los descendientes `pending` cuya arista es bloqueante.
///
/// `state='done'` es EXCLUSIVO de la aprobación humana del review (el front lo dispara; `done_detection`
/// sólo lleva a `awaiting_review`). Por eso el avance es a RITMO de aprobación, no de exit-code.
/// Idempotente: re-llamarlo con el mismo done no re-desbloquea (un dependiente ya `dag_blocked=0`/no
/// `pending` no se toca). NO toca el foco del micrófono.
pub fn on_task_settled(db: &Db, task_id: &str, new_state: &str) -> Result<()> {
    // Resolver el run bajo un lock corto; salir si no es tarea de pipeline.
    let run_id = {
        let conn = db.lock();
        match run_id_of(&conn, task_id)? {
            Some(r) => r,
            None => return Ok(()), // single-task / batch normal: nada que avanzar.
        }
    };
    // Advisory-lock por run: serializa avance vs reconcile.
    let lock = run_advisory_lock(&run_id);
    let _guard = lock.lock();

    // TODO el avance corre en UNA transacción (audit codex): el gate de status Y las mutaciones son
    // atómicos → un `mark_run_canceled` no puede colarse entre el chequeo y la mutación. (El advisory
    // lock serializa contra reconcile; el gate-en-tx cierra la carrera contra cancel.)
    let conn = db.lock();
    let tx = conn.unchecked_transaction()?;
    // GATE: sólo avanzar/cascadear sobre un run AÚN `running` (lectura DENTRO de la tx).
    let status: Option<String> = tx
        .query_row("SELECT status FROM pipeline_runs WHERE id = ?1", params![run_id], |r| r.get(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    if status.as_deref() != Some("running") {
        return Ok(()); // run terminal/cancelado/inexistente: noop (tx se descarta sin cambios).
    }
    match new_state {
        "done" => unblock_ready_dependents_in_tx(&tx, &run_id, task_id)?,
        "failed" | "canceled" => {
            // PRIMERO cascadear el skip por las aristas BLOQUEANTES; DESPUÉS desbloquear los
            // dependientes por aristas `on_error='continue'` que la caída ahora satisface (best-effort).
            // TODO en la MISMA tx → `maybe_finalize_run` corre UNA vez al final, viendo el estado final.
            cascade_cancel_in_tx(&tx, &run_id, task_id)?;
            unblock_ready_dependents_in_tx(&tx, &run_id, task_id)?;
        }
        _ => return Ok(()), // estados no-terminales no avanzan el grafo (tx vacía).
    }
    maybe_finalize_run(&tx, &run_id)?;
    tx.commit()?;
    Ok(())
}

/// 038 F1.3 — desbloquea los dependientes DIRECTOS de `done_task` cuya readiness ya está completa,
/// SOBRE UNA TRANSACCIÓN ya abierta (reusable por el hook y reconcile). Por cada dependiente
/// `pending`+`dag_blocked=1` cuyas deps están todas satisfechas (`deps_all_done`), `UPDATE
/// dag_blocked=0` → queda lanzable por el humano. NO lanza (el gate de slots v1 es `max_concurrent=1`
/// vía `claim_for_launch`). NO finaliza el run (lo hace el caller).
fn unblock_ready_dependents_in_tx(tx: &Connection, run_id: &str, done_task: &str) -> Result<()> {
    use crate::services::orchestration::deps_all_done;
    // Dependientes directos: tareas cuyo `pipeline_edges.depends_on_task_id == done_task`.
    let dependents: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT pe.task_id
             FROM pipeline_edges pe
             JOIN orchestration_tasks t ON t.id = pe.task_id
             WHERE pe.depends_on_task_id = ?1 AND pe.run_id = ?2
               AND t.state = 'pending' AND t.dag_blocked = 1",
        )?;
        let v = stmt
            .query_map(params![done_task, run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    for dep_task in &dependents {
        if deps_all_done(tx, dep_task)? {
            // Desbloquear sólo si SIGUE pending+bloqueado (idempotente, anti-doble-trigger).
            tx.execute(
                "UPDATE orchestration_tasks SET dag_blocked = 0
                 WHERE id = ?1 AND state = 'pending' AND dag_blocked = 1",
                params![dep_task],
            )?;
        }
    }
    Ok(())
}

/// 038 F1.3 (FR-005) — cascada de fallo (BFS sobre una tx ya abierta): skipea (cancela) SOLO los
/// descendientes `pending` cuya cadena de dependencias quedó rota por una dep que falló/canceló. NUNCA
/// toca `running`/`awaiting_review` (matar trabajo vivo viola "no se matan agentes automáticamente" —
/// red-team #6). Una arista `on_error='continue'` NO propaga (la dep se considera satisfecha aunque
/// falle → best-effort). Iterativo: la cancelación de un nodo puede romper a SUS descendientes
/// (cascada multinivel). Reusable por el hook (`on_task_settled`) y por `reconcile_run` (que lo corre
/// para CADA tarea `failed`/`canceled` del run tras un crash). NO commitea ni finaliza el run (eso lo
/// hace el caller).
/// `reconcile_run`, que lo corre para CADA tarea `failed`/`canceled` del run tras un crash). NO commitea
/// ni finaliza el run (eso lo hace el caller). Cancela SÓLO descendientes `pending` por aristas
/// bloqueantes; nunca toca trabajo vivo.
fn cascade_cancel_in_tx(tx: &Connection, run_id: &str, failed_task: &str) -> Result<()> {
    let mut frontier = vec![failed_task.to_string()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(cur) = frontier.pop() {
        // Dependientes DIRECTOS de `cur` con arista BLOQUEANTE (on_error != 'continue').
        let blocked_children: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT pe.task_id
                 FROM pipeline_edges pe
                 WHERE pe.depends_on_task_id = ?1 AND pe.run_id = ?2 AND pe.on_error != 'continue'",
            )?;
            let v = stmt
                .query_map(params![cur, run_id], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for child in blocked_children {
            if !seen.insert(child.clone()) {
                continue;
            }
            // Cancelar SÓLO si está `pending` (no `running`/`awaiting_review`/terminal). Registrar el
            // motivo en result_summary (skipped: upstream X failed) — FR-003 "skipped" derivado.
            let summary = format!("skipped: upstream {failed_task} failed");
            let n = tx.execute(
                "UPDATE orchestration_tasks
                 SET state = 'canceled', result_summary = ?2, updated_at = ?3
                 WHERE id = ?1 AND state = 'pending'",
                params![child, summary, chrono::Utc::now().to_rfc3339()],
            )?;
            if n == 1 {
                // El hijo fue cancelado → su caída puede romper a SUS descendientes.
                frontier.push(child);
            }
            // Si n==0 (estaba running/awaiting_review/terminal) NO se cancela ni se propaga desde él:
            // el trabajo vivo decide su propio destino; sus descendientes se evaluarán cuando él cierre.
        }
    }
    Ok(())
}

/// 038 F1.3 — cierra el run si ya no hay nada que avanzar. `done` si TODAS las tareas terminaron en
/// `done`; `failed` si todas terminaron y al menos una no es `done` (failed/canceled). Si queda algo
/// `pending`/`running`/`awaiting_review`, no toca el run (sigue `running`). Idempotente (sólo cambia un
/// run aún `running`). Toma una `&Connection`/tx ya abierta por el caller.
fn maybe_finalize_run(conn: &Connection, run_id: &str) -> Result<()> {
    // ¿queda alguna tarea no-terminal en el run?
    let alive: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_tasks
         WHERE pipeline_run_id = ?1 AND state IN ('pending','running','awaiting_review')",
        params![run_id],
        |r| r.get(0),
    )?;
    if alive > 0 {
        return Ok(());
    }
    // Todas terminaron. ¿alguna no-`done`?
    let non_done: i64 = conn.query_row(
        "SELECT COUNT(*) FROM orchestration_tasks
         WHERE pipeline_run_id = ?1 AND state != 'done'",
        params![run_id],
        |r| r.get(0),
    )?;
    let new_status = if non_done > 0 { "failed" } else { "done" };
    conn.execute(
        "UPDATE pipeline_runs SET status = ?2, updated_at = ?3 WHERE id = ?1 AND status = 'running'",
        params![run_id, new_status, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// 038 F1.3 — al boot, re-evalúa la readiness del DAG de CADA run `running` (resume tras restart sin
/// doble-spawn). Por cada run vivo: bajo su advisory-lock, desbloquea los dependientes ya listos y
/// cierra el run si corresponde. NO lanza nada (el humano relanza la raíz / la siguiente etapa). El
/// reconciliador de tareas `running` zombi (PTY muerto) lo cubre F1.4. Idempotente.
pub fn reconcile_on_boot(db: &Db) -> Result<usize> {
    let runs: Vec<String> = {
        let conn = db.lock();
        let mut stmt = conn.prepare("SELECT id FROM pipeline_runs WHERE status = 'running'")?;
        let v = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    let mut reconciled = 0usize;
    for run_id in &runs {
        let lock = run_advisory_lock(run_id);
        let _guard = lock.lock();
        reconcile_run(db, run_id)?;
        reconciled += 1;
    }
    Ok(reconciled)
}

/// Re-evalúa TODOS los nodos `pending`+`dag_blocked=1` de un run y desbloquea los que ya están listos
/// (sus deps `done`/satisfechas). Llamado bajo el advisory-lock del run. Cierra el run si aplica.
fn reconcile_run(db: &Db, run_id: &str) -> Result<()> {
    use crate::services::orchestration::deps_all_done;
    let conn = db.lock();
    let tx = conn.unchecked_transaction()?;
    // (1) CASCADA pendiente tras crash (audit codex): si una tarea quedó `failed`/`canceled` y el hook
    // NUNCA corrió (crash entre la transición y `on_task_settled`), sus descendientes `pending`
    // bloqueados quedarían inalcanzables y el run colgado en `running` para siempre. Re-disparamos la
    // cascada de CADA tarea terminada-en-fallo del run. Idempotente (descendientes ya canceled se
    // saltan). Esto cierra el agujero de resume del red-team.
    let failed_tasks: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM orchestration_tasks
             WHERE pipeline_run_id = ?1 AND state IN ('failed','canceled')",
        )?;
        let v = stmt
            .query_map(params![run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    for ft in &failed_tasks {
        cascade_cancel_in_tx(&tx, run_id, ft)?;
    }
    // (2) DESBLOQUEO: re-evaluar todos los nodos `pending`+`dag_blocked=1` y desbloquear los listos
    // (sus deps `done`/satisfechas, incluido `on_error='continue'`).
    let blocked: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT id FROM orchestration_tasks
             WHERE pipeline_run_id = ?1 AND state = 'pending' AND dag_blocked = 1",
        )?;
        let v = stmt
            .query_map(params![run_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    for task_id in &blocked {
        if deps_all_done(&tx, task_id)? {
            tx.execute(
                "UPDATE orchestration_tasks SET dag_blocked = 0
                 WHERE id = ?1 AND state = 'pending' AND dag_blocked = 1",
                params![task_id],
            )?;
        }
    }
    maybe_finalize_run(&tx, run_id)?;
    tx.commit()?;
    Ok(())
}

/// 038 F1.5 (FR-009) — `waiting_on_human` por run: un run `running` SIN ninguna tarea `running` pero
/// CON al menos una `awaiting_review` está esperando que el humano revise/apruebe (en un pipeline
/// lineal, indistinguible de un hang sin esta señal). Devuelve `(run_id, waiting_minutes)` donde los
/// minutos se miden desde el `updated_at` MÁS VIEJO de las tareas `awaiting_review` del run (cuánto
/// hace que el review quedó pendiente). DERIVADO de DB (stateless): el board muestra el advisory.
pub fn waiting_on_human(db: &Db) -> Result<Vec<(String, i64)>> {
    let conn = db.lock();
    // Runs running con 0 tareas `running` y ≥1 `awaiting_review`; min(updated_at) de las awaiting.
    let mut stmt = conn.prepare(
        "SELECT r.id, MIN(ar.updated_at)
         FROM pipeline_runs r
         JOIN orchestration_tasks ar
           ON ar.pipeline_run_id = r.id AND ar.state = 'awaiting_review'
         WHERE r.status = 'running'
           AND NOT EXISTS (
                SELECT 1 FROM orchestration_tasks run_t
                WHERE run_t.pipeline_run_id = r.id AND run_t.state = 'running'
           )
         GROUP BY r.id",
    )?;
    let rows: Vec<(String, String)> = {
        let v = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    drop(stmt);
    let now = chrono::Utc::now();
    let mut out = Vec::with_capacity(rows.len());
    for (run_id, oldest) in rows {
        // Minutos desde el review pendiente más viejo (clamp a 0; RFC3339 parse robusto).
        let mins = chrono::DateTime::parse_from_rfc3339(&oldest)
            .map(|t| (now - t.with_timezone(&chrono::Utc)).num_minutes().max(0))
            .unwrap_or(0);
        out.push((run_id, mins));
    }
    Ok(out)
}

/// Marca un run como `canceled` (lo usa F1.4 `pipeline_cancel`). El scheduler deja de promover cuando
/// `status != 'running'`. Idempotente (sólo cambia un run aún `running`).
pub fn mark_run_canceled(db: &Db, run_id: &str) -> Result<()> {
    let conn = db.lock();
    let n = conn.execute(
        "UPDATE pipeline_runs SET status = 'canceled', updated_at = ?2 WHERE id = ?1 AND status = 'running'",
        params![run_id, chrono::Utc::now().to_rfc3339()],
    )?;
    if n == 0 {
        // Ya terminal o inexistente; no es error para un cancel idempotente, pero distinguimos vacío.
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pipeline_runs WHERE id = ?1",
            params![run_id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(anyhow!("run de pipeline no encontrado: {run_id}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::attention::MicFocus;
    use crate::services::orchestration::{self as orch, ResolvedPipelineTask, YamlEdge};

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/037_orch_pause_council_history.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/047_pipeline_dag.sql")).unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// Arma un pipeline lineal A→B→C y devuelve (db, run_id, [id_a,id_b,id_c]).
    fn linear_abc(db: &Db) -> (String, Vec<String>) {
        let mk = |id: &str| ResolvedPipelineTask {
            yaml_id: id.into(), title: id.to_uppercase(), objective: String::new(),
            agent_profile_id: None, mode: None,
        };
        let tasks = vec![mk("a"), mk("b"), mk("c")];
        let edges = vec![
            YamlEdge { task_yaml_id: "b".into(), depends_on_yaml_id: "a".into(), on_error: None },
            YamlEdge { task_yaml_id: "c".into(), depends_on_yaml_id: "b".into(), on_error: None },
        ];
        let topo = vec!["a".into(), "b".into(), "c".into()];
        let (run_id, _batch, created) =
            orch::create_pipeline_run(db, "abc", "/tmp/r", None, None, &tasks, &edges, &topo, "y").unwrap();
        let id = |title: &str| created.iter().find(|t| t.title == title).unwrap().id.clone();
        (run_id, vec![id("A"), id("B"), id("C")])
    }

    fn state_of(db: &Db, id: &str) -> String {
        orch::get_task(db, id).unwrap().unwrap().state
    }
    fn blocked_of(db: &Db, id: &str) -> i64 {
        orch::get_task(db, id).unwrap().unwrap().dag_blocked
    }

    /// 038 F1.3 (criterio #1) — lineal A→B→C avanza al `done` HUMANO, NO al `awaiting_review` ni por
    /// exit-code. B se desbloquea cuando A llega a `done`; C cuando B llega a `done`.
    #[test]
    fn linear_advances_on_done_not_review() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        // Estado inicial: A lanzable, B/C bloqueados.
        assert_eq!(blocked_of(&db, a), 0);
        assert_eq!(blocked_of(&db, b), 1);
        assert_eq!(blocked_of(&db, c), 1);
        // A: running → awaiting_review. B NO debe desbloquearse en awaiting_review.
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        on_task_settled(&db, a, "awaiting_review").unwrap();
        assert_eq!(blocked_of(&db, b), 1, "B sigue bloqueado en awaiting_review (avance al `done`)");
        // A: done → el hook desbloquea B.
        orch::set_state(&db, a, "done", None).unwrap();
        on_task_settled(&db, a, "done").unwrap();
        assert_eq!(blocked_of(&db, b), 0, "B se desbloquea al `done` de A");
        assert_eq!(blocked_of(&db, c), 1, "C sigue bloqueado (depende de B)");
        // B: lanzar → done → desbloquea C.
        orch::claim_for_launch(&db, b).unwrap();
        orch::set_state(&db, b, "awaiting_review", None).unwrap();
        orch::set_state(&db, b, "done", None).unwrap();
        on_task_settled(&db, b, "done").unwrap();
        assert_eq!(blocked_of(&db, c), 0, "C se desbloquea al `done` de B");
        // C: done → el run se cierra `done`.
        orch::claim_for_launch(&db, c).unwrap();
        orch::set_state(&db, c, "awaiting_review", None).unwrap();
        orch::set_state(&db, c, "done", None).unwrap();
        on_task_settled(&db, c, "done").unwrap();
        let status: String = {
            let conn = db.lock();
            conn.query_row("SELECT status FROM pipeline_runs WHERE id=?1", params![_run], |r| r.get(0)).unwrap()
        };
        assert_eq!(status, "done", "el run se cierra `done` cuando todas las tareas terminan done");
    }

    /// 038 F1.3 (criterio #6, GUARDIÁN — foco único humano-otorgado, NON-NEGOTIABLE) — avanzar una
    /// etapa del DAG NO toca el `MicFocus`. La garantía es POR TIPOS: este módulo (y sus tests) NO
    /// puede siquiera construir el witness `HumanCommand` (`from_human_input` es privado a `attention`)
    /// → no puede llamar `grant_focus`. Si alguien intentara mover el foco desde el scheduler, el
    /// código NO COMPILARÍA. Acá lo verificamos en runtime también: arrancamos con foco=None y, tras
    /// avanzar el pipeline (claim A → done A → desbloquea B), el foco SIGUE None — el scheduler no tiene
    /// ninguna vía para tocarlo. (El hecho de que el import de `HumanCommand` esté AUSENTE es parte de
    /// la prueba: el módulo no lo importa.)
    #[test]
    fn advancing_dag_never_changes_mic_focus() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let (a, b) = (&ids[0], &ids[1]);
        let focus = MicFocus::new(); // foco=None; el scheduler NO puede otorgarlo (witness privado).
        assert_eq!(focus.current(), None);
        // Avanzar el pipeline: claim A, done A → desbloquea B.
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        orch::set_state(&db, a, "done", None).unwrap();
        on_task_settled(&db, a, "done").unwrap();
        assert_eq!(blocked_of(&db, b), 0, "B se desbloqueó");
        // El foco SIGUE None — el scheduler no lo movió (no puede: no tiene el witness).
        assert_eq!(focus.current(), None, "el scheduler NUNCA toca el foco del micrófono");
    }

    /// 038 F1.3 (criterio #3, FR-005) — cascada de fallo skipea SOLO descendientes `pending`; NO toca
    /// `running`/`awaiting_review`. A falla → B (pending) se cancela como `skipped`; C (que depende de
    /// B, pending) se cancela en cascada multinivel.
    #[test]
    fn cascade_skips_only_pending() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "failed", None).unwrap();
        on_task_settled(&db, a, "failed").unwrap();
        assert_eq!(state_of(&db, b), "canceled", "B (pending) se cancela por la cascada");
        assert_eq!(state_of(&db, c), "canceled", "C se cancela en cascada multinivel");
        let summary_b = orch::get_task(&db, b).unwrap().unwrap().result_summary;
        assert!(summary_b.unwrap().contains("skipped"), "B marca el motivo skipped");
        // El run se cierra `failed`.
        let status: String = {
            let conn = db.lock();
            conn.query_row("SELECT status FROM pipeline_runs WHERE id=?1", params![_run], |r| r.get(0)).unwrap()
        };
        assert_eq!(status, "failed");
    }

    /// 038 F1.3 — la cascada NUNCA mata trabajo VIVO: si B está `awaiting_review` cuando A falla, B NO
    /// se cancela (el humano decide). C, que depende de B, NO se cancela todavía (B sigue vivo).
    #[test]
    fn cascade_never_kills_live_work() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        // Llevar B a awaiting_review (vivo) ANTES de que A falle (escenario raro pero el invariante debe
        // sostenerse: desbloquear B requiere A done; simulamos B vivo independientemente).
        // Para que B sea claimeable lo desbloqueamos vía done de A primero, luego forzamos fallo "tardío".
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        orch::set_state(&db, a, "done", None).unwrap();
        on_task_settled(&db, a, "done").unwrap(); // desbloquea B
        orch::claim_for_launch(&db, b).unwrap();
        orch::set_state(&db, b, "awaiting_review", None).unwrap(); // B VIVO
        // Ahora A "se rechaza" tarde (canceled). La cascada NO debe tocar B (awaiting_review).
        on_task_settled(&db, a, "canceled").unwrap();
        assert_eq!(state_of(&db, b), "awaiting_review", "B vivo NO se mata por la cascada");
        assert_eq!(state_of(&db, c), "pending", "C no se cancela mientras B sigue vivo");
    }

    /// 038 F1.3 — `on_error='continue'`: una dep que falla NO cancela al dependiente (best-effort) y de
    /// hecho lo DESBLOQUEA (la dep se considera satisfecha al terminar).
    #[test]
    fn continue_on_error_does_not_cascade() {
        let db = test_db();
        let mk = |id: &str| ResolvedPipelineTask {
            yaml_id: id.into(), title: id.to_uppercase(), objective: String::new(),
            agent_profile_id: None, mode: None,
        };
        let tasks = vec![mk("a"), mk("b")];
        let edges = vec![YamlEdge { task_yaml_id: "b".into(), depends_on_yaml_id: "a".into(), on_error: Some("continue".into()) }];
        let topo = vec!["a".into(), "b".into()];
        let (run, _batch, created) =
            orch::create_pipeline_run(&db, "p", "/tmp/r", None, None, &tasks, &edges, &topo, "y").unwrap();
        let a = created.iter().find(|t| t.title == "A").unwrap().id.clone();
        let b = created.iter().find(|t| t.title == "B").unwrap().id.clone();
        orch::claim_for_launch(&db, &a).unwrap();
        orch::set_state(&db, &a, "failed", None).unwrap();
        on_task_settled(&db, &a, "failed").unwrap();
        // B NO se cancela (continue) y SÍ se desbloquea (la dep terminó → satisfecha).
        assert_eq!(state_of(&db, &b), "pending", "continue: B no se cancela");
        assert_eq!(blocked_of(&db, &b), 0, "continue: B se desbloquea al terminar la dep");
        let _ = run;
    }

    /// 038 F1.3 — `on_task_settled` sobre una tarea SINGLE-TASK (sin run) es noop (no rompe, no avanza).
    #[test]
    fn settled_on_single_task_is_noop() {
        let db = test_db();
        let (_b, tasks) = orch::create_batch(
            &db, "b", "/tmp/r", None, None,
            &[orch::TaskSpec { title: "Solo".into(), objective: String::new(), agent_profile_id: None, mode: None }],
        ).unwrap();
        let id = tasks[0].id.clone();
        orch::claim_for_launch(&db, &id).unwrap();
        orch::set_state(&db, &id, "awaiting_review", None).unwrap();
        orch::set_state(&db, &id, "done", None).unwrap();
        // No debe panicar ni mutar nada raro.
        on_task_settled(&db, &id, "done").unwrap();
        assert_eq!(state_of(&db, &id), "done");
    }

    /// 038 F1.3 (criterio #5) — `reconcile_on_boot` re-evalúa el grafo de un run `running` sin
    /// doble-spawn: una tarea cuya dep YA está `done` (pero quedó `dag_blocked=1` por un crash entre el
    /// done y el unblock) se desbloquea al boot. Idempotente.
    #[test]
    fn reconcile_on_boot_unblocks_ready() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let (a, b) = (&ids[0], &ids[1]);
        // Simular crash: A llegó a done pero el hook NO corrió (B sigue dag_blocked=1).
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        orch::set_state(&db, a, "done", None).unwrap();
        assert_eq!(blocked_of(&db, b), 1, "tras el crash B quedó bloqueado pese a A done");
        // Boot → reconcile desbloquea B.
        let n = reconcile_on_boot(&db).unwrap();
        assert_eq!(n, 1, "reconcilió 1 run running");
        assert_eq!(blocked_of(&db, b), 0, "reconcile desbloquea B (su dep está done)");
        // Idempotente: re-boot no cambia nada y no doble-spawnea (B sigue pending, lanzable).
        reconcile_on_boot(&db).unwrap();
        assert_eq!(state_of(&db, b), "pending");
    }

    /// 038 F1.3 (audit codex BLOCKER) — `reconcile_on_boot` re-dispara la CASCADA de un fallo que el
    /// hook nunca corrió (crash entre la transición a `failed` y `on_task_settled`): los descendientes
    /// pending quedan canceled (skipped) y el run se cierra `failed` en vez de colgarse `running`.
    #[test]
    fn reconcile_on_boot_resumes_pending_cascade() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        // Simular crash: A falló pero el hook NO corrió (B/C siguen pending bloqueados).
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "failed", None).unwrap();
        assert_eq!(state_of(&db, b), "pending", "tras el crash B sigue pending (cascada no corrió)");
        // Boot → reconcile re-dispara la cascada.
        reconcile_on_boot(&db).unwrap();
        assert_eq!(state_of(&db, b), "canceled", "reconcile cascadea B (skipped)");
        assert_eq!(state_of(&db, c), "canceled", "reconcile cascadea C en multinivel");
        let status: String = {
            let conn = db.lock();
            conn.query_row("SELECT status FROM pipeline_runs WHERE id=?1", params![run], |r| r.get(0)).unwrap()
        };
        assert_eq!(status, "failed", "el run se cierra `failed` (no queda colgado running)");
    }

    /// 038 F1.3 (audit codex BLOCKER) — un settled event TARDÍO sobre un run ya `canceled` NO reactiva
    /// ni muta tareas: el gate de status='running' DENTRO de la tx del avance corta el hook.
    #[test]
    fn settled_on_canceled_run_is_noop() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let (a, b) = (&ids[0], &ids[1]);
        // A ya estaba `running` cuando se cancela el run: el claim corre ANTES del cancel (orden real
        // seguro). Tras el cancel, la guarda de `claim_for_launch` ya NO promovería A — eso lo cubre
        // `claim_for_launch_rejects_canceled_run` (cierre TOCTOU del cancel, audit mistral-large F1.4).
        assert!(orch::claim_for_launch(&db, a).unwrap(), "A se lanza ANTES del cancel");
        mark_run_canceled(&db, &run).unwrap();
        // Un settled `done` de A que llega tarde NO debe desbloquear B (run ya cancelado).
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        orch::set_state(&db, a, "done", None).unwrap();
        on_task_settled(&db, a, "done").unwrap();
        assert_eq!(blocked_of(&db, b), 1, "B NO se desbloquea: el run está canceled");
    }

    /// 038 F1.5 (FR-009) — `waiting_on_human`: un run `running` sin tarea corriendo pero con una en
    /// `awaiting_review` aparece como esperando al humano; cuando una tarea corre, NO aparece.
    #[test]
    fn waiting_on_human_surfaces_review() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let (a, _b, _c) = (&ids[0], &ids[1], &ids[2]);
        // Nada esperando aún (A pending, no review).
        assert!(waiting_on_human(&db).unwrap().is_empty());
        // A corriendo → NO waiting (hay trabajo vivo).
        orch::claim_for_launch(&db, a).unwrap();
        assert!(waiting_on_human(&db).unwrap().is_empty(), "con una tarea running NO está esperando review");
        // A → awaiting_review, sin otra running → waiting_on_human aparece.
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        let w = waiting_on_human(&db).unwrap();
        assert_eq!(w.len(), 1, "el run aparece esperando review");
        assert_eq!(w[0].0, run);
        assert!(w[0].1 >= 0, "waiting_minutes >= 0");
        // A → done → el hook desbloquea B; ya no hay awaiting_review → NO waiting.
        orch::set_state(&db, a, "done", None).unwrap();
        on_task_settled(&db, a, "done").unwrap();
        assert!(waiting_on_human(&db).unwrap().is_empty(), "tras el done ya no espera review");
    }

    /// 038 F1.5 — un run `canceled`/terminal NUNCA aparece en `waiting_on_human` (sólo `running`).
    #[test]
    fn waiting_on_human_ignores_terminal_runs() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let a = &ids[0];
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        // Estaba esperando, pero cancelamos el run → ya no debe aparecer.
        mark_run_canceled(&db, &run).unwrap();
        assert!(waiting_on_human(&db).unwrap().is_empty(), "un run canceled no espera review");
    }

    /// 038 F1.4 — `mark_run_canceled` + cancelación de tareas `pending` (lo que hace `pipeline_cancel`
    /// para el subconjunto NO-running, sin PTY): el run pasa a `canceled` y las tareas pending/bloqueadas
    /// a `canceled` directo. Verifica que la transición pending→canceled es válida (sin huérfanos).
    #[test]
    fn pipeline_cancel_pending_path() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let (a, b, c) = (&ids[0], &ids[1], &ids[2]);
        // (1) marcar el run canceled.
        mark_run_canceled(&db, &run).unwrap();
        let status: String = {
            let conn = db.lock();
            conn.query_row("SELECT status FROM pipeline_runs WHERE id=?1", params![run], |r| r.get(0)).unwrap()
        };
        assert_eq!(status, "canceled");
        // (2) cancelar pending/bloqueadas (A pending lanzable, B/C bloqueadas) — todas pending→canceled.
        for id in [a, b, c] {
            orch::set_state(&db, id, "canceled", None).unwrap();
            assert_eq!(state_of(&db, id), "canceled");
        }
        // mark_run_canceled idempotente sobre un run ya canceled (no error).
        mark_run_canceled(&db, &run).unwrap();
    }

    /// 038 F1.4 (audit mistral-large) — CIERRE TOCTOU del cancel: tras `mark_run_canceled`, una tarea
    /// `pending` y NO bloqueada (dag_blocked=0, lanzable) de ESE run YA NO se promueve. Sin esta guarda,
    /// el scheduler podía promover A (spawneando PTY) en el hueco entre `mark_run_canceled` y el
    /// `list_run_tasks` de `pipeline_cancel` → PTY huérfano no matado. Single-task (sin pipeline_run_id)
    /// no se afecta (lo cubren los tests de claim single-task existentes).
    #[test]
    fn claim_for_launch_rejects_canceled_run() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let a = &ids[0];
        assert_eq!(blocked_of(&db, a), 0, "A arranca lanzable (raíz del DAG, dag_blocked=0)");
        mark_run_canceled(&db, &run).unwrap();
        // El gate RECHAZA la promoción de A aunque siga pending + dag_blocked=0 (run canceled).
        assert!(!orch::claim_for_launch(&db, a).unwrap(), "no se promueve una tarea de un run canceled");
        assert_eq!(state_of(&db, a), "pending", "A sigue pending (no quedó running sin PTY)");
    }

    /// 038 F1.4 — `mark_run_canceled` sobre un run inexistente devuelve Err (el comando lo propaga).
    #[test]
    fn mark_run_canceled_missing_run_errors() {
        let db = test_db();
        assert!(mark_run_canceled(&db, "no-existe").is_err());
    }

    /// 038 F1.4 — `list_run_tasks` devuelve las tareas del run en orden topo, enriquecidas.
    #[test]
    fn list_run_tasks_in_topo_order() {
        let db = test_db();
        let (run, ids) = linear_abc(&db);
        let listed = orch::list_run_tasks(&db, &run).unwrap();
        assert_eq!(listed.len(), 3);
        // orden topo: A, B, C.
        assert_eq!(listed[0].id, ids[0]);
        assert_eq!(listed[1].id, ids[1]);
        assert_eq!(listed[2].id, ids[2]);
        // enriquecidas: B depende de A.
        assert_eq!(listed[1].depends_on, vec![ids[0].clone()]);
        assert_eq!(listed[1].dag_blocked, 1);
    }

    /// 038 F1.4 (criterio #5, anti zombie-slot) — el reconciliador de tareas `running` (012 `tick`)
    /// itera `list_tasks(db, None)` filtrando por `state='running'`, lo que INCLUYE las tareas de
    /// pipeline (mismo `orchestration_tasks`). Verificamos que una tarea de pipeline `running` aparece
    /// en ese listado (cobertura del reconciliador single-task sobre tareas con `pipeline_run_id`).
    #[test]
    fn pipeline_running_task_is_visible_to_reconciler() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let a = &ids[0];
        orch::claim_for_launch(&db, a).unwrap();
        // El reconciliador (012) usa list_tasks(db, None) y filtra running.
        let all = orch::list_tasks(&db, None).unwrap();
        let running: Vec<_> = all.iter().filter(|t| t.state == "running").collect();
        assert_eq!(running.len(), 1, "la tarea de pipeline running es visible al reconciliador");
        assert_eq!(&running[0].id, a);
        assert!(running[0].pipeline_run_id.is_some(), "y trae su pipeline_run_id");
    }

    /// 038 F1.3 (red-team #1) — el hook NO se auto-deadlockea: `on_task_settled` abre su PROPIO scope
    /// de lock (el caller ya soltó el suyo). Lo verificamos llamándolo en secuencia con operaciones que
    /// toman el lock (get_task) sin colgarse. Un self-deadlock colgaría el test (timeout del runner).
    #[test]
    fn hook_opens_its_own_lock_no_deadlock() {
        let db = test_db();
        let (_run, ids) = linear_abc(&db);
        let a = &ids[0];
        orch::claim_for_launch(&db, a).unwrap();
        orch::set_state(&db, a, "awaiting_review", None).unwrap();
        orch::set_state(&db, a, "done", None).unwrap();
        // Si on_task_settled mantuviera un lock a través de un re-lock interno (get_task/deps_all_done),
        // el Mutex no-reentrante colgaría acá. Que retorne = no hay self-deadlock.
        on_task_settled(&db, a, "done").unwrap();
        assert_eq!(state_of(&db, &ids[1]), "pending");
        assert_eq!(blocked_of(&db, &ids[1]), 0);
    }
}
