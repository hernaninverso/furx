// services/orchestration.rs — 008-parallel-orchestration.
//
// Orquestación de N tareas autónomas, cada una en su git worktree aislado con su agente
// (006). Council 2026-05-29: completion = PTY exit + "mark ready" explícito (NO polling ni
// timeout); branch única por tarea; cada tarea corre detached (tmux), el pane es una vista
// on-demand. Este módulo es el modelo + ciclo de vida + recolección (diff stat) + cleanup;
// el spawn real del PTY vive en commands.rs (que tiene el PtyManager).

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchTask {
    pub id: String,
    pub batch_id: String,
    pub title: String,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    pub repo_path: String,
    pub branch: String,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub pane_id: Option<String>,
    pub state: String,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub result_summary: Option<String>,
    /// 012-pty-done-detection — flag (0/1) sub-estado: el agente pide confirmación. NO es un
    /// estado de la state machine de 008, es un atributo del estado `running`.
    #[serde(default)]
    pub needs_input: i64,
    /// 012 — auto-confirm opt-in POR TAREA (0/1, default 0). El global vive en settings.
    #[serde(default)]
    pub auto_confirm: i64,
    /// 012 — cli_kind cacheado (claude/codex/aider/gemini) para la tabla de patrones del classifier.
    #[serde(default)]
    pub cli_kind: Option<String>,
    /// 014 — best-of-N: grupo de variantes al que pertenece (NULL = tarea normal).
    #[serde(default)]
    pub group_id: Option<String>,
    /// 014 — best-of-N: índice de la variante dentro del grupo (0..n-1).
    #[serde(default)]
    pub variant_index: Option<i64>,
    /// 019 F3 (T030) — pausa: timestamp ISO de cuándo se pausó el attempt (SIGSTOP). NULL = corriendo.
    #[serde(default)]
    pub paused_at: Option<String>,
    /// 038 F1.1 — campo DERIVADO (no columna): los `id` de las tareas de las que ésta depende
    /// (sus `depends_on_task_id` en `pipeline_edges`). Poblado por `list_tasks`/`get_task` vía un
    /// LEFT JOIN secundario; `row_to_task` NO lo toca. Default `vec![]` (`#[serde(default)]`) → una
    /// tarea single-task (sin aristas) serializa EXACTAMENTE como hoy. Hace VISIBLE en la UI por qué
    /// un nodo está bloqueado, sin meter el grafo en la state-machine.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// 038 F1.1 — run de pipeline al que pertenece (NULL = single-task/batch normal). Derivado de la
    /// columna `pipeline_run_id`. El front lo usa para mostrar el contexto del pipeline.
    #[serde(default)]
    pub pipeline_run_id: Option<String>,
    /// 038 F1.1 — gate de lanzamiento: 1 = esperando deps (la guarda de `claim_for_launch` lo rechaza),
    /// 0 = lanzable. Default 0 → single-task idéntico.
    #[serde(default)]
    pub dag_blocked: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// Input para crear una tarea de un batch.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskSpec {
    pub title: String,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

pub const STATES: &[&str] = &[
    "pending",
    "running",
    "awaiting_review",
    "done",
    "failed",
    "canceled",
];

/// Transiciones válidas del ciclo de vida (council 008).
pub fn can_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("pending", "running")
            | ("pending", "canceled")
            | ("running", "awaiting_review")
            | ("running", "failed")
            | ("running", "canceled")
            | ("awaiting_review", "done")
            | ("awaiting_review", "failed")
            | ("awaiting_review", "canceled")
    )
}

/// Slug seguro para nombre de branch (sin espacios ni metachars de ref git).
fn slugify(s: &str) -> String {
    let mut out: String = s
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "task".to_string()
    } else {
        out.chars().take(40).collect()
    }
}

/// Branch única y determinística por tarea — evita el "branch ya checked-out" entre worktrees.
/// Cap total ≤ ~56 chars: "furx/orch/"(10) + batch8 + "/" + slug(≤28) + "-" + task8 = ≤56,
/// dentro del límite de 64 de worktree::ensure (audit codex HIGH).
pub fn branch_name(batch_id: &str, task_id: &str, title: &str) -> String {
    let b: String = batch_id.chars().take(8).collect();
    let t: String = task_id.chars().take(8).collect();
    let slug: String = slugify(title).chars().take(28).collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "task" } else { slug };
    format!("furx/orch/{}/{}-{}", b, slug, t)
}

fn valid_repo_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 1024 && !p.chars().any(|c| c.is_control())
}

/// Crea un batch + sus tareas (todas pending, con branch asignada). NO crea worktrees ni
/// spawnea (eso lo hace el launch en commands.rs, serializando git por repo).
pub fn create_batch(
    db: &Db,
    title: &str,
    repo_path: &str,
    base_branch: Option<&str>,
    base_commit: Option<&str>,
    tasks: &[TaskSpec],
) -> Result<(String, Vec<OrchTask>)> {
    if !valid_repo_path(repo_path) {
        return Err(anyhow!("repo_path inválido"));
    }
    if tasks.is_empty() {
        return Err(anyhow!("un batch necesita al menos 1 tarea"));
    }
    // Validar TODAS las specs ANTES de tocar la DB (audit codex+gemini+deepseek: no dejar
    // batch parcial si una tarea falla a mitad del loop).
    for spec in tasks {
        if spec.title.trim().is_empty() {
            return Err(anyhow!("cada tarea necesita un título"));
        }
    }
    let batch_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    {
        let conn = db.lock();
        // Transacción: batch + tareas, todo o nada.
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO orchestration_batches (id, title, repo_path, base_branch, base_commit, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![batch_id, title, repo_path, base_branch, base_commit, now],
        )?;
        for spec in tasks {
            let task_id = Uuid::new_v4().to_string();
            let branch = branch_name(&batch_id, &task_id, &spec.title);
            tx.execute(
                "INSERT INTO orchestration_tasks
                    (id, batch_id, title, objective, agent_profile_id, mode, repo_path, branch, state, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9,?9)",
                params![task_id, batch_id, spec.title.trim(), spec.objective, spec.agent_profile_id, spec.mode, repo_path, branch, now],
            )?;
        }
        tx.commit()?;
    }
    let tasks = list_tasks(db, Some(&batch_id))?;
    Ok((batch_id, tasks))
}

const TASK_COLS: &str = "id, batch_id, title, objective, agent_profile_id, mode, repo_path,
    branch, worktree_path, pane_id, state, exit_code, result_summary, needs_input, auto_confirm,
    cli_kind, group_id, variant_index, paused_at, created_at, updated_at";

fn row_to_task(r: &rusqlite::Row) -> rusqlite::Result<OrchTask> {
    Ok(OrchTask {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        title: r.get(2)?,
        objective: r.get(3)?,
        agent_profile_id: r.get(4)?,
        mode: r.get(5)?,
        repo_path: r.get(6)?,
        branch: r.get(7)?,
        worktree_path: r.get(8)?,
        pane_id: r.get(9)?,
        state: r.get(10)?,
        exit_code: r.get(11)?,
        result_summary: r.get(12)?,
        needs_input: r.get(13)?,
        auto_confirm: r.get(14)?,
        cli_kind: r.get(15)?,
        group_id: r.get(16)?,
        variant_index: r.get(17)?,
        paused_at: r.get(18)?,
        // 038 F1.1 — campos DERIVADOS del DAG: NO se leen de TASK_COLS (que no cambia). Arrancan en
        // su default y los completa `enrich_dag_fields` en un paso posterior bajo el mismo lock. Una
        // tarea single-task queda con estos defaults → serialización idéntica a hoy.
        depends_on: Vec::new(),
        pipeline_run_id: None,
        dag_blocked: 0,
        created_at: r.get(19)?,
        updated_at: r.get(20)?,
    })
}

/// 038 F1.1 — completa los campos DERIVADOS del DAG (`pipeline_run_id`, `dag_blocked`, `depends_on`)
/// sobre tareas ya construidas por `row_to_task`. SEPARADO de `row_to_task` (que no cambia) y del
/// `TASK_COLS` original. Toma el `conn` ya bloqueado del caller. Una sola query por las columnas del
/// DAG + una por las aristas; ambas indexadas. Si una tarea no tiene fila de DAG (single-task),
/// queda con sus defaults (`dag_blocked=0`, `pipeline_run_id=None`, `depends_on=[]`).
fn enrich_dag_fields(conn: &Connection, tasks: &mut [OrchTask]) -> rusqlite::Result<()> {
    use std::collections::HashMap;
    if tasks.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let id_set: std::collections::HashSet<&str> = ids.iter().copied().collect();
    // Si el esquema del DAG (047) no está presente (p.ej. un test que sólo aplica 022), degradar a
    // single-task: dejar los defaults (`depends_on=[]`, `pipeline_run_id=None`, `dag_blocked=0`). En
    // producción 047 siempre existe; esto sólo blinda a `list_tasks`/`get_task` de crashear cuando la
    // feature opcional aún no migró. La presencia se chequea por sqlite_master, no por catch de error.
    let dag_present: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='pipeline_edges'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !dag_present {
        return Ok(());
    }
    // 1) Columnas del DAG sobre orchestration_tasks (pipeline_run_id/dag_blocked).
    let mut dag_cols: HashMap<String, (Option<String>, i64)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, pipeline_run_id, dag_blocked FROM orchestration_tasks WHERE pipeline_run_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, run, blocked) = row?;
            if id_set.contains(id.as_str()) {
                dag_cols.insert(id, (run, blocked));
            }
        }
    }
    // 2) Aristas: depends_on_task_id agrupado por task_id. Filtramos por el `id_set` en Rust (no por
    //    `pipeline_run_id IS NOT NULL` — audit deepseek: una arista cuyo task_id existe es relevante
    //    aunque el JOIN se complique; la fuente de verdad de "esta tarea tiene deps" es la arista). Sólo
    //    consultamos si hay tareas con run (las single-task no tienen aristas).
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    if !dag_cols.is_empty() {
        let mut stmt =
            conn.prepare("SELECT task_id, depends_on_task_id FROM pipeline_edges")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (tid, dep) = row?;
            if id_set.contains(tid.as_str()) {
                deps.entry(tid).or_default().push(dep);
            }
        }
    }
    for t in tasks.iter_mut() {
        if let Some((run, blocked)) = dag_cols.get(&t.id) {
            t.pipeline_run_id = run.clone();
            t.dag_blocked = *blocked;
        }
        if let Some(d) = deps.get(&t.id) {
            t.depends_on = d.clone();
        }
    }
    Ok(())
}

pub fn list_tasks(db: &Db, batch_id: Option<&str>) -> Result<Vec<OrchTask>> {
    let conn = db.lock();
    let (sql, want_batch) = match batch_id {
        Some(_) => (format!("SELECT {TASK_COLS} FROM orchestration_tasks WHERE batch_id = ?1 ORDER BY created_at ASC"), true),
        None => (format!("SELECT {TASK_COLS} FROM orchestration_tasks ORDER BY created_at DESC"), false),
    };
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = if want_batch {
        stmt.query_map(params![batch_id.unwrap()], row_to_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([], row_to_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    drop(stmt);
    enrich_dag_fields(&conn, &mut rows)?;
    Ok(rows)
}

/// 038 F1.4 — todas las tareas de un run de pipeline (por `pipeline_run_id`), en orden topo.
/// Enriquecidas (depends_on/dag_blocked). Lo usa `pipeline_cancel` para recorrer el run.
pub fn list_run_tasks(db: &Db, run_id: &str) -> Result<Vec<OrchTask>> {
    let conn = db.lock();
    let sql = format!(
        "SELECT {TASK_COLS} FROM orchestration_tasks WHERE pipeline_run_id = ?1
         ORDER BY topo_index ASC, created_at ASC"
    );
    let mut rows = {
        let mut stmt = conn.prepare(&sql)?;
        let v = stmt
            .query_map(params![run_id], row_to_task)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        v
    };
    enrich_dag_fields(&conn, &mut rows)?;
    Ok(rows)
}

pub fn get_task(db: &Db, id: &str) -> Result<Option<OrchTask>> {
    let conn = db.lock();
    let sql = format!("SELECT {TASK_COLS} FROM orchestration_tasks WHERE id = ?1");
    let task = {
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_task)?;
        match rows.next() {
            Some(r) => Some(r?),
            None => None,
        }
    };
    match task {
        Some(t) => {
            let mut one = [t];
            enrich_dag_fields(&conn, &mut one)?;
            let [t] = one;
            Ok(Some(t))
        }
        None => Ok(None),
    }
}

/// Transición de estado validada. exit_code se setea junto al estado terminal cuando aplica.
pub fn set_state(db: &Db, id: &str, to: &str, exit_code: Option<i64>) -> Result<()> {
    if !STATES.contains(&to) {
        return Err(anyhow!("estado inválido: {}", to));
    }
    let cur = get_task(db, id)?.ok_or_else(|| anyhow!("tarea no encontrada: {}", id))?;
    if cur.state == to {
        return Ok(()); // idempotente
    }
    if !can_transition(&cur.state, to) {
        return Err(anyhow!("transición inválida {} → {}", cur.state, to));
    }
    // 038 F1.1 (audit codex) — GATE del DAG a nivel de state-machine: NINGUNA ruta entra a `running`
    // saltándose el gate. `claim_for_launch` ya filtra con `AND dag_blocked=0`, pero `set_state` acepta
    // `pending→running` para cualquier caller; sin esto una 3ª ruta vía `set_state("running")`/
    // `mark_running` lanzaría una tarea bloqueada. El read de `cur.dag_blocked` da un error claro, pero
    // la GARANTÍA real es el UPDATE CONDICIONAL atómico de abajo (`AND dag_blocked=0`), que cierra el
    // TOCTOU: si otra ruta bloquea la tarea entre el read y el write, el UPDATE afecta 0 filas y
    // devolvemos Err en vez de dejar una tarea bloqueada en `running`.
    let now = Utc::now().to_rfc3339();
    if to == "running" {
        // UPDATE atómico: el gate (`dag_blocked=0`) Y el from-state (`state='pending'`, ya verificado
        // por `can_transition` arriba) van EN el WHERE → cierra el TOCTOU completo (audit deepseek): si
        // otra ruta bloquea la tarea O la mueve de `pending` entre el read y el write, afecta 0 filas.
        let n = {
            let conn = db.lock();
            // `state` se BINDEA por parámetro (`?2`), no como literal — así el guard de "3ª ruta de
            // spawn" (que escanea `SET state='running'` literal) no cuenta a `set_state` como una ruta
            // de bypass: ESTE UPDATE va POR el gate (`AND dag_blocked=0 AND state='pending'`).
            conn.execute(
                "UPDATE orchestration_tasks SET state = ?2, exit_code = COALESCE(?3, exit_code), updated_at = ?4
                 WHERE id = ?1 AND dag_blocked = 0 AND state = 'pending'",
                params![id, to, exit_code, now],
            )?
        }; // lock liberado antes de cualquier re-lectura (Mutex no-reentrante).
        if n != 1 {
            // 0 filas = o la tarea quedó bloqueada (dag_blocked=1) o ya no estaba `pending` (otra ruta
            // ganó). Re-leer para distinguir el mensaje; el resultado es el mismo: no se lanzó.
            let st = get_task(db, id)?.map(|t| (t.state, t.dag_blocked));
            return match st {
                Some((_, blocked)) if blocked != 0 => Err(anyhow!(
                    "no se puede lanzar la tarea {}: bloqueada por dependencias del DAG (dag_blocked=1)",
                    id
                )),
                Some((s, _)) => Err(anyhow!("transición inválida {} → running (estado cambió)", s)),
                None => Err(anyhow!("tarea no encontrada: {}", id)),
            };
        }
        return Ok(());
    }
    let conn = db.lock();
    conn.execute(
        "UPDATE orchestration_tasks SET state = ?2, exit_code = COALESCE(?3, exit_code), updated_at = ?4 WHERE id = ?1",
        params![id, to, exit_code, now],
    )?;
    Ok(())
}

/// Claim ATÓMICO para lanzar: pending → running en un solo UPDATE condicional. Devuelve
/// true si ESTE caller ganó el claim. Evita el doble-spawn de dos launches concurrentes
/// sobre la misma tarea (audit codex+deepseek HIGH).
pub fn claim_for_launch(db: &Db, id: &str) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // 038 F1.1 — GATE del DAG: una tarea con `dag_blocked=1` (esperando sus deps) NO se lanza, ni por
    // `orchestration_prepare_task` ni por best_of_n (ambas rutas pasan por acá). UNA guarda SQL cubre
    // las dos. Single-task / batch normal tiene `dag_blocked=0` (default) → comportamiento idéntico.
    // 038 F1.4 (audit mistral-large) — CIERRE TOCTOU del cancel: una tarea de un run `canceled` NO se
    // promueve. `pipeline_cancel` hace `mark_run_canceled` y LUEGO lista las tareas; sin esta guarda,
    // el scheduler podría promover una `pending` (dag_blocked=0) en el hueco entre el mark y el list →
    // PTY huérfano no matado. `mark_run_canceled` y `claim_for_launch` toman el MISMO `db.lock()` →
    // serializados: o el claim corre antes (la tarea queda `running` y `pipeline_cancel` la mata) o
    // después (esta guarda lo rechaza). Single-task (`pipeline_run_id IS NULL`) pasa siempre.
    let n = conn.execute(
        "UPDATE orchestration_tasks SET state='running', updated_at=?2 \
         WHERE id=?1 AND state='pending' AND dag_blocked=0 \
         AND (pipeline_run_id IS NULL \
              OR pipeline_run_id NOT IN (SELECT id FROM pipeline_runs WHERE status='canceled'))",
        params![id, now],
    )?;
    Ok(n == 1)
}

/// 038 F1.1 — una arista del DAG ya RESUELTA a uuids de tareas locales (lo que se persiste en
/// `pipeline_edges`). `on_error` None = default bloqueante; Some("continue") = best-effort.
#[derive(Debug, Clone)]
pub struct DagEdge {
    pub task_id: String,
    pub depends_on_task_id: String,
    pub on_error: Option<String>,
}

/// 038 F1.1 — puebla `pipeline_edges` y marca como `dag_blocked=1` toda tarea del run que tenga ≥1
/// dependencia (las raíces quedan en 0 = lanzables). Setea `pipeline_run_id`/`topo_index` por tarea.
/// PENSADO para correr DENTRO de la transacción de `pipeline_run_yaml` (F1.2): recibe la conexión/tx
/// del caller, no toma el lock él mismo. `topo_order` = uuids en orden topológico (índice = posición).
/// Idempotente sobre las aristas (`INSERT OR IGNORE` por la PK compuesta).
pub fn set_task_dependencies(
    conn: &Connection,
    run_id: &str,
    topo_order: &[String],
    edges: &[DagEdge],
) -> Result<()> {
    use std::collections::HashSet;
    // Asignar pipeline_run_id + topo_index a cada tarea del run.
    for (idx, task_id) in topo_order.iter().enumerate() {
        conn.execute(
            "UPDATE orchestration_tasks SET pipeline_run_id=?2, topo_index=?3 WHERE id=?1",
            params![task_id, run_id, idx as i64],
        )?;
    }
    // Insertar las aristas.
    let mut blocked: HashSet<&str> = HashSet::new();
    for e in edges {
        conn.execute(
            "INSERT OR IGNORE INTO pipeline_edges (run_id, task_id, depends_on_task_id, on_error)
             VALUES (?1,?2,?3,COALESCE(?4,'block_downstream'))",
            params![run_id, e.task_id, e.depends_on_task_id, e.on_error],
        )?;
        blocked.insert(e.task_id.as_str());
    }
    // Toda tarea con ≥1 dependencia arranca bloqueada; las raíces quedan lanzables (dag_blocked=0).
    for task_id in &blocked {
        conn.execute(
            "UPDATE orchestration_tasks SET dag_blocked=1 WHERE id=?1",
            params![task_id],
        )?;
    }
    Ok(())
}

/// 038 F1.1 — ¿todas las dependencias bloqueantes de `task_id` están en `done`? Una arista con
/// `on_error='continue'` cuya dep terminó en `failed`/`canceled` se considera SATISFECHA (best-effort).
/// Una arista bloqueante (default) exige que su dep esté `done`. Sin aristas → true (raíz / single-task).
/// Es la base del readiness del scheduler (F1.3). NO muta nada (cálculo puro sobre DB).
pub fn deps_all_done(conn: &Connection, task_id: &str) -> Result<bool> {
    // Cuenta las aristas NO satisfechas: la dep no está `done`, salvo que `on_error='continue'` y la
    // dep haya TERMINADO (done/failed/canceled). Una dep aún `pending`/`running`/`awaiting_review`
    // NUNCA satisface (ni con 'continue': el upstream sigue vivo).
    let unsatisfied: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM pipeline_edges pe
         JOIN orchestration_tasks dep ON dep.id = pe.depends_on_task_id
         WHERE pe.task_id = ?1
           AND NOT (
                dep.state = 'done'
                OR (pe.on_error = 'continue' AND dep.state IN ('failed','canceled'))
           )",
        params![task_id],
        |r| r.get(0),
    )?;
    Ok(unsatisfied == 0)
}

/// 038 F1.2 — una tarea de pipeline ya RESUELTA contra perfiles locales (el slug portable `agent`
/// del YAML ya se mapeó a un `agent_profile_id` local, o None si el YAML no especificó agente). El
/// `yaml_id` es el id declarativo del pipeline (referenciado por las aristas); se traduce a un uuid
/// de tarea local al crear el batch.
#[derive(Debug, Clone)]
pub struct ResolvedPipelineTask {
    pub yaml_id: String,
    pub title: String,
    pub objective: String,
    pub agent_profile_id: Option<String>,
    pub mode: Option<String>,
}

/// 038 F1.2 — una arista del DAG en ESPACIO YAML (ids declarativos del pipeline). Se traduce a uuids
/// locales dentro de la transacción de `create_pipeline_run`.
#[derive(Debug, Clone)]
pub struct YamlEdge {
    pub task_yaml_id: String,
    pub depends_on_yaml_id: String,
    pub on_error: Option<String>,
}

/// 038 F1.2 — crea un run de pipeline COMPLETO en UNA transacción: batch + tareas (en orden topo,
/// con `topo_index`) + fila `pipeline_runs` (yaml_sha256/topo_json/spec_yaml) + aristas
/// `pipeline_edges` traducidas yaml_id→uuid, marcando `dag_blocked=1` a las tareas con deps. Todo o
/// nada (un fallo a mitad NO deja un batch parcial — invariante de `create_batch` extendida al DAG).
///
/// El llamador (command F1.2) YA resolvió los slugs a `agent_profile_id` locales (fail-closed) ANTES
/// de invocar esto — acá no hay resolución ni red, sólo escritura atómica. `topo_yaml_order` = ids
/// YAML en orden topológico (029); `spec_yaml` = el YAML original (resume/audit). Devuelve
/// `(run_id, batch_id, tasks)` con las tareas ya enriquecidas (depends_on/dag_blocked).
#[allow(clippy::too_many_arguments)]
pub fn create_pipeline_run(
    db: &Db,
    name: &str,
    repo_path: &str,
    base_branch: Option<&str>,
    base_commit: Option<&str>,
    tasks: &[ResolvedPipelineTask],
    edges: &[YamlEdge],
    topo_yaml_order: &[String],
    spec_yaml: &str,
) -> Result<(String, String, Vec<OrchTask>)> {
    use std::collections::HashMap;
    if !valid_repo_path(repo_path) {
        return Err(anyhow!("repo_path inválido"));
    }
    if tasks.is_empty() {
        return Err(anyhow!("un pipeline necesita al menos 1 tarea"));
    }
    // Validar las tareas (paridad con create_batch: no dejar nada a medias si una falla).
    for t in tasks {
        if t.title.trim().is_empty() {
            return Err(anyhow!("cada tarea necesita un título"));
        }
    }
    // El orden topo debe cubrir EXACTAMENTE los yaml_id de las tareas (defensa: el caller pasa ambos).
    {
        let task_ids: std::collections::HashSet<&str> = tasks.iter().map(|t| t.yaml_id.as_str()).collect();
        if topo_yaml_order.len() != tasks.len()
            || !topo_yaml_order.iter().all(|y| task_ids.contains(y.as_str()))
        {
            return Err(anyhow!("topo_yaml_order no corresponde 1:1 con las tareas del pipeline"));
        }
    }
    let batch_id = Uuid::new_v4().to_string();
    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let yaml_sha256 = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(spec_yaml.as_bytes());
        hex::encode(h.finalize())
    };
    // Mapa yaml_id → uuid local.
    let mut id_map: HashMap<String, String> = HashMap::new();
    for t in tasks {
        id_map.insert(t.yaml_id.clone(), Uuid::new_v4().to_string());
    }
    // topo_json congelado: uuids locales en orden topo.
    let topo_uuids: Vec<String> = topo_yaml_order
        .iter()
        .map(|y| id_map.get(y).cloned().ok_or_else(|| anyhow!("yaml_id sin uuid: {y}")))
        .collect::<Result<Vec<_>>>()?;
    let topo_json = serde_json::to_string(&topo_uuids)?;

    {
        let conn = db.lock();
        let tx = conn.unchecked_transaction()?;
        // batch
        tx.execute(
            "INSERT INTO orchestration_batches (id, title, repo_path, base_branch, base_commit, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![batch_id, name, repo_path, base_branch, base_commit, now],
        )?;
        // run (antes de las aristas: pipeline_edges.run_id FK → pipeline_runs.id)
        tx.execute(
            "INSERT INTO pipeline_runs (id, batch_id, name, yaml_sha256, topo_json, spec_yaml, status, max_concurrent, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,'running',1,?7,?7)",
            params![run_id, batch_id, name, yaml_sha256, topo_json, spec_yaml, now],
        )?;
        // tareas EN ORDEN TOPO, con topo_index + pipeline_run_id.
        for (idx, yaml_id) in topo_yaml_order.iter().enumerate() {
            let task = tasks.iter().find(|t| &t.yaml_id == yaml_id).expect("topo cubre tasks");
            let task_id = &id_map[yaml_id];
            let branch = branch_name(&batch_id, task_id, &task.title);
            tx.execute(
                "INSERT INTO orchestration_tasks
                    (id, batch_id, title, objective, agent_profile_id, mode, repo_path, branch, state,
                     pipeline_run_id, topo_index, dag_blocked, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9,?10,0,?11,?11)",
                params![
                    task_id, batch_id, task.title.trim(), task.objective, task.agent_profile_id,
                    task.mode, repo_path, branch, run_id, idx as i64, now
                ],
            )?;
        }
        // aristas traducidas + marcar dag_blocked=1 a las tareas con deps.
        let mut blocked: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for e in edges {
            let tid = id_map.get(&e.task_yaml_id).ok_or_else(|| {
                anyhow!("arista refiere task_yaml_id inexistente: {}", e.task_yaml_id)
            })?;
            let dep = id_map.get(&e.depends_on_yaml_id).ok_or_else(|| {
                anyhow!("arista refiere depends_on inexistente: {}", e.depends_on_yaml_id)
            })?;
            tx.execute(
                "INSERT OR IGNORE INTO pipeline_edges (run_id, task_id, depends_on_task_id, on_error)
                 VALUES (?1,?2,?3,COALESCE(?4,'block_downstream'))",
                params![run_id, tid, dep, e.on_error],
            )?;
            blocked.insert(tid.as_str());
        }
        for tid in &blocked {
            tx.execute(
                "UPDATE orchestration_tasks SET dag_blocked=1 WHERE id=?1",
                params![tid],
            )?;
        }
        tx.commit()?;
    }
    let tasks = list_tasks(db, Some(&batch_id))?;
    Ok((run_id, batch_id, tasks))
}

/// Marca una tarea como running + registra su worktree + pane (al lanzarla).
pub fn mark_running(db: &Db, id: &str, worktree_path: &str, pane_id: Option<&str>) -> Result<()> {
    set_state(db, id, "running", None)?;
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE orchestration_tasks SET worktree_path = ?2, pane_id = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, worktree_path, pane_id, now],
    )?;
    Ok(())
}

/// Guarda el result_summary (diff stat) recolectado.
pub fn set_result_summary(db: &Db, id: &str, summary: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE orchestration_tasks SET result_summary = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, summary, now],
    )?;
    Ok(())
}

// ── 019 F3 (T030) — pause / resume transaccional de un attempt ───────────────
// El PTY se detiene con SIGSTOP (en pty.rs); ESTE flag es el SSOT de "está pausado": persistido,
// reversible, idempotente. El poller (012) y el auto-confirm respetan `paused_at` (no auto-presionan
// Enter sobre un proceso congelado). Pausar/reanudar NO cambia el estado de la state-machine
// (running sigue running) — es un sub-estado, como `needs_input`/`auto_confirm`. NO mata nada.

/// Marca un attempt como PAUSADO (setea `paused_at`). Sólo válido sobre una tarea `running`
/// (no se pausa algo que no corre). Idempotente: si ya estaba pausada, no re-pisa el timestamp y
/// devuelve `false` ("ya estaba pausada"); `true` si esta llamada la pausó. El SIGSTOP real lo hace
/// el caller (command) sobre el PTY — este flag y la señal van juntos transaccionalmente.
pub fn pause_task(db: &Db, id: &str) -> Result<bool> {
    let task = get_task(db, id)?.ok_or_else(|| anyhow!("tarea no encontrada: {}", id))?;
    if task.state != "running" {
        return Err(anyhow!(
            "sólo se pausa una tarea corriendo (estado actual: {})",
            task.state
        ));
    }
    if task.paused_at.is_some() {
        return Ok(false); // ya pausada — idempotente
    }
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // Condición `paused_at IS NULL` → atómico contra un pause concurrente (sólo uno gana).
    let n = conn.execute(
        "UPDATE orchestration_tasks SET paused_at = ?2, updated_at = ?2
         WHERE id = ?1 AND state = 'running' AND paused_at IS NULL",
        params![id, now],
    )?;
    Ok(n == 1)
}

/// REANUDA un attempt pausado (limpia `paused_at`). Idempotente: si no estaba pausada devuelve
/// `false`; `true` si esta llamada la reanudó. El SIGCONT real lo hace el caller sobre el PTY.
pub fn resume_task(db: &Db, id: &str) -> Result<bool> {
    let task = get_task(db, id)?.ok_or_else(|| anyhow!("tarea no encontrada: {}", id))?;
    if task.paused_at.is_none() {
        return Ok(false); // no estaba pausada — idempotente
    }
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    let n = conn.execute(
        "UPDATE orchestration_tasks SET paused_at = NULL, updated_at = ?2
         WHERE id = ?1 AND paused_at IS NOT NULL",
        params![id, now],
    )?;
    Ok(n == 1)
}

// ── 019 F3 (T030) — ETA: estimación de tiempo restante (cálculo PURO testeable) ──
// Estima cuánto le falta a un batch/grupo de attempts en base a la DURACIÓN de los attempts ya
// terminados (la mejor señal local, sin LLM ni red). Tres entradas por attempt: estado + segundos
// transcurridos (running) o segundos de duración total (terminado). Devuelve None si no hay base
// (ningún attempt terminado todavía → no inventamos un número).

/// Una observación de timing de un attempt para el cálculo de ETA. `elapsed_secs` = cuánto lleva
/// corriendo (si `running`) o cuánto duró (si terminó), en segundos.
#[derive(Debug, Clone, Copy)]
pub struct AttemptTiming {
    pub running: bool,
    pub terminal: bool,
    pub elapsed_secs: f64,
}

/// Estimación de ETA de un conjunto de attempts (un batch o un grupo best-of-N).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EtaEstimate {
    /// Duración promedio (segundos) de los attempts terminados — la base de la proyección.
    pub avg_terminal_secs: f64,
    /// Cuántos attempts siguen corriendo.
    pub running: usize,
    /// Cuántos attempts ya terminaron.
    pub finished: usize,
    /// Segundos estimados hasta que el ÚLTIMO attempt corriendo termine (max sobre los running de
    /// `avg - elapsed`, nunca < 0). Los attempts corren en paralelo, así que el ETA del conjunto es
    /// el del que más le falta, no la suma.
    pub eta_secs: f64,
}

/// Calcula el ETA de un conjunto de attempts. Devuelve `None` si:
///   - no hay NINGÚN attempt terminado (sin base de duración → no proyectamos), o
///   - no hay attempts corriendo (nada pendiente → ETA = 0 implícito, el caller no lo muestra).
/// El promedio se toma SÓLO de los terminados (la verdad observada). Para cada running, lo que le
/// falta = max(avg - su_elapsed, 0); el ETA del conjunto = el máximo (corren en paralelo).
pub fn estimate_eta(timings: &[AttemptTiming]) -> Option<EtaEstimate> {
    let terminal: Vec<f64> = timings
        .iter()
        .filter(|t| t.terminal && !t.running)
        .map(|t| t.elapsed_secs.max(0.0))
        .collect();
    if terminal.is_empty() {
        return None; // sin base observada
    }
    let running: Vec<f64> = timings
        .iter()
        .filter(|t| t.running && !t.terminal)
        .map(|t| t.elapsed_secs.max(0.0))
        .collect();
    if running.is_empty() {
        return None; // nada corriendo → no hay ETA que mostrar
    }
    let avg = terminal.iter().sum::<f64>() / terminal.len() as f64;
    // Cada running: cuánto le falta para alcanzar el promedio (si ya lo superó → 0, no negativo).
    let eta = running
        .iter()
        .map(|elapsed| (avg - elapsed).max(0.0))
        .fold(0.0_f64, f64::max);
    Some(EtaEstimate {
        avg_terminal_secs: avg,
        running: running.len(),
        finished: terminal.len(),
        eta_secs: eta,
    })
}

/// Helper: convierte el OrchTask en una observación de timing para `estimate_eta`. Usa
/// `created_at`→`updated_at` para los terminados (duración) y `created_at`→`now` para los running.
/// Acepta `now` inyectable para tests deterministas.
pub fn task_timing(task: &OrchTask, now: chrono::DateTime<Utc>) -> Option<AttemptTiming> {
    let start = chrono::DateTime::parse_from_rfc3339(&task.created_at)
        .ok()?
        .with_timezone(&Utc);
    let terminal = matches!(task.state.as_str(), "done" | "failed" | "canceled");
    let running = task.state == "running";
    if !terminal && !running {
        return None; // pending → todavía no aporta señal
    }
    let end = if terminal {
        chrono::DateTime::parse_from_rfc3339(&task.updated_at)
            .ok()?
            .with_timezone(&Utc)
    } else {
        now
    };
    let elapsed = (end - start).num_milliseconds() as f64 / 1000.0;
    Some(AttemptTiming {
        running,
        terminal,
        elapsed_secs: elapsed.max(0.0),
    })
}

/// `git -C <worktree> diff --stat` (working tree) con fallback a `status --short`.
///
/// Read-only para el repo PERO `git diff`/`status` REFRESCAN el índice y toman el lock
/// `index.lock` por default. Para un comando ADVISORY (US2 ranking) que sólo lee, eso puede
/// interferir con un git interactivo del usuario en el mismo worktree. Por eso seteamos
/// `GIT_OPTIONAL_LOCKS=0` (audit 3-frontera finding #4): git omite el refresh del índice y NO toma
/// el lock — la salida puede quedar marginalmente stale (un stat-info sin actualizar), aceptable
/// para una sugerencia. Idéntico al `GIT_OPTIONAL_LOCKS=0` que ya usan `git_overview` y
/// `worktree_merge_review`.
pub fn collect_diff(worktree_path: &str) -> String {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["-C", worktree_path, "--no-pager"])
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    run(&["diff", "--stat"])
        .or_else(|| run(&["diff", "--stat", "HEAD"]))
        .or_else(|| run(&["status", "--short"]))
        .unwrap_or_else(|| "(sin cambios)".to_string())
}

/// UNIFIED diff COMPLETO de lo que produjo una variante (headers `@@`/`+++`/`---`), para la review
/// hunk-level (019 F0; `review::parse_unified_diff`). Distinto de `collect_diff` (que devuelve
/// `--stat`, resumen). `--no-ext-diff` evita diffs externos del usuario.
///
/// `base` = el `base_commit`/`base_branch` de la variante (lo que orchestration ya guarda). Captura
/// TODO lo que difiere del base: committed EN el worktree (el agente puede commitear) + staged +
/// unstaged → `git diff <base>` (working tree vs base). Audit codex 019: si NO se pasara base y se
/// usara `git diff` (sin ref), con cambios staged Y unstaged a la vez se omitirían los staged → la
/// review aprobaría algo incompleto. Por eso `HEAD` (staged+unstaged vs HEAD) es el fallback primario
/// cuando no hay base, y `git diff` crudo el último recurso. Vacío → "" (0 hunks, nada que revisar).
pub fn collect_unified_diff(worktree_path: &str, base: Option<&str>) -> String {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["-C", worktree_path, "--no-pager"])
            .args(args)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).to_string();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    };
    // Con base conocido: working tree vs base (committed + staged + unstaged desde el base).
    if let Some(b) = base {
        if let Some(d) = run(&["diff", "--no-ext-diff", "--unified=3", b]) {
            return d;
        }
    }
    // Sin base: HEAD captura staged+unstaged (NO `git diff` solo, que omitiría los staged).
    run(&["diff", "--no-ext-diff", "--unified=3", "HEAD"])
        .or_else(|| run(&["diff", "--no-ext-diff", "--unified=3"]))
        .unwrap_or_default()
}

/// Limpia worktrees huérfanos del repo (git worktree prune). Best-effort, al boot/cleanup.
pub fn prune_worktrees(repo_path: &str) -> Result<()> {
    let out = std::process::Command::new("git")
        .args(["-C", repo_path, "worktree", "prune"])
        .output()
        .map_err(|e| anyhow!("git worktree prune: {}", e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git worktree prune falló: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

// ── 014-orchestration-ux — best-of-N grouping (FR-001) ───────────────────────
// Una tarea-objetivo lanzada como N variantes (≤4), cada una en su worktree/branch (reusa el
// mismo modelo de 008 + el launch). El grupo relaciona las variantes; el humano elige UNA para
// mergear y descarta el resto (con confirmación — constitución VI, no destructivo silencioso).

/// Tope de variantes por grupo (spec FR-001 / edge cases: ≤4).
pub const MAX_VARIANTS: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGroup {
    pub id: String,
    pub batch_id: String,
    pub objective: String,
    pub n: i64,
    pub chosen_task_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Crea un batch best-of-N: 1 grupo + N variantes (mismas `objective`/`title`, distinto agente o
/// el mismo) en `pending`, cada una con su branch única (reusa branch_name de 008). El launch real
/// (worktree + spawn) lo hace el caller por variante, igual que una tarea normal. `agents` es la
/// lista de agent_profile_id por variante (len == n; None = mode legacy). Devuelve (batch, group, tasks).
#[allow(clippy::type_complexity)]
pub fn create_best_of_n(
    db: &Db,
    title: &str,
    repo_path: &str,
    base_branch: Option<&str>,
    base_commit: Option<&str>,
    objective: &str,
    agents: &[Option<String>],
) -> Result<(String, TaskGroup, Vec<OrchTask>)> {
    if !valid_repo_path(repo_path) {
        return Err(anyhow!("repo_path inválido"));
    }
    let n = agents.len();
    if n == 0 {
        return Err(anyhow!("best-of-N necesita al menos 1 variante"));
    }
    if n > MAX_VARIANTS {
        return Err(anyhow!(
            "best-of-N admite hasta {} variantes (pediste {})",
            MAX_VARIANTS,
            n
        ));
    }
    if title.trim().is_empty() {
        return Err(anyhow!("el objetivo necesita un título"));
    }
    let batch_id = Uuid::new_v4().to_string();
    let group_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    {
        let conn = db.lock();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO orchestration_batches (id, title, repo_path, base_branch, base_commit, created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![batch_id, title, repo_path, base_branch, base_commit, now],
        )?;
        tx.execute(
            "INSERT INTO orch_task_groups (id, batch_id, objective, n, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?5)",
            params![group_id, batch_id, objective, n as i64, now],
        )?;
        for (i, agent) in agents.iter().enumerate() {
            let task_id = Uuid::new_v4().to_string();
            // título de variante distinguible en la card (objetivo común + nº de variante).
            let variant_title = format!("{} · v{}", title.trim(), i + 1);
            let branch = branch_name(&batch_id, &task_id, &variant_title);
            tx.execute(
                "INSERT INTO orchestration_tasks
                    (id, batch_id, title, objective, agent_profile_id, mode, repo_path, branch,
                     state, group_id, variant_index, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9,?10,?11,?11)",
                params![
                    task_id,
                    batch_id,
                    variant_title,
                    objective,
                    agent,
                    if agent.is_some() {
                        None
                    } else {
                        Some("zsh".to_string())
                    },
                    repo_path,
                    branch,
                    group_id,
                    i as i64,
                    now
                ],
            )?;
        }
        tx.commit()?;
    }
    let group = get_group(db, &group_id)?.ok_or_else(|| anyhow!("grupo desapareció"))?;
    let tasks = list_tasks(db, Some(&batch_id))?;
    Ok((batch_id, group, tasks))
}

const GROUP_COLS: &str = "id, batch_id, objective, n, chosen_task_id, created_at, updated_at";

fn row_to_group(r: &rusqlite::Row) -> rusqlite::Result<TaskGroup> {
    Ok(TaskGroup {
        id: r.get(0)?,
        batch_id: r.get(1)?,
        objective: r.get(2)?,
        n: r.get(3)?,
        chosen_task_id: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

pub fn get_group(db: &Db, group_id: &str) -> Result<Option<TaskGroup>> {
    let conn = db.lock();
    let sql = format!("SELECT {GROUP_COLS} FROM orch_task_groups WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![group_id], row_to_group)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// Base contra el cual diffear las variantes de un grupo (join group→batch). Devuelve
/// `base_commit` si existe, sino `base_branch` (codex 019: preferir base_commit). `None` si el
/// grupo/batch no existe o el batch no tiene base. NOTA: la review (review_open/apply) EXIGE base
/// (`None` → error, sin degradar a HEAD); otros callers de `collect_unified_diff` sí toleran HEAD.
pub fn group_base(db: &Db, group_id: &str) -> Result<Option<String>> {
    let conn = db.lock();
    let row: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT b.base_commit, b.base_branch FROM orch_task_groups g \
             JOIN orchestration_batches b ON b.id = g.batch_id WHERE g.id = ?1",
            params![group_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(commit, branch)| commit.or(branch)))
}

/// Todas las variantes de un grupo (ordenadas por variant_index).
pub fn list_group_tasks(db: &Db, group_id: &str) -> Result<Vec<OrchTask>> {
    let conn = db.lock();
    let sql = format!(
        "SELECT {TASK_COLS} FROM orchestration_tasks WHERE group_id = ?1 ORDER BY variant_index ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params![group_id], row_to_task)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(stmt);
    // 038 F1.1 (audit codex): consistencia con list_tasks/get_task — enriquecer los campos derivados
    // del DAG. Para variantes best_of_n (no nodos DAG en v1) quedan en defaults (depends_on=[]).
    enrich_dag_fields(&conn, &mut rows)?;
    Ok(rows)
}

/// Marca la variante elegida del grupo. NO mergea ni descarta — sólo registra la elección
/// (el merge sigue el flujo de 008 con confirmación; el descarte es una acción aparte explícita).
pub fn choose_variant(db: &Db, group_id: &str, task_id: &str) -> Result<()> {
    // validar que la tarea pertenece al grupo (no marcar una ajena).
    let belongs = {
        let conn = db.lock();
        conn.query_row(
            "SELECT 1 FROM orchestration_tasks WHERE id = ?1 AND group_id = ?2",
            params![task_id, group_id],
            |_| Ok(()),
        )
        .is_ok()
    };
    if !belongs {
        return Err(anyhow!(
            "la tarea {} no es una variante del grupo {}",
            task_id,
            group_id
        ));
    }
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE orch_task_groups SET chosen_task_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![group_id, task_id, now],
    )?;
    Ok(())
}

/// Cancela (descarta) UNA variante no-elegida del grupo: estado canceled. NO toca su worktree
/// (la limpieza física es una acción de cleanup aparte con su propio escape-hatch). El caller
/// (command) ya pidió confirmación al humano antes de llamar — constitución VI. Idempotente:
/// si la variante ya es terminal o es la elegida, no hace nada y devuelve false.
pub fn discard_variant(db: &Db, group_id: &str, task_id: &str) -> Result<bool> {
    // Audit fix codex+deepseek 014: hacer el descarte ATÓMICO contra una elección
    // concurrente. La guarda "no es la elegida" + "pertenece al grupo" + "cancelable" va
    // como condición SQL en UN solo UPDATE bajo `db.lock()`, así un `choose_variant` que
    // corra entre el check y el cancel NO puede hacer que cancelemos la variante elegida.
    // (No podemos sostener el lock y llamar `set_state` — el parking_lot Mutex no es
    // reentrante; canceled es terminal y la condición `state IN (...)` cubre la transición.)
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    // guard explícito: si pedís descartar la elegida → error claro (no silenciosamente noop).
    let chosen: Option<String> = conn
        .query_row(
            "SELECT chosen_task_id FROM orch_task_groups WHERE id = ?1",
            params![group_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    if chosen.as_deref() == Some(task_id) {
        return Err(anyhow!("no se descarta la variante elegida"));
    }
    let n = conn.execute(
        "UPDATE orchestration_tasks SET state = 'canceled', updated_at = ?3
         WHERE id = ?1 AND group_id = ?2
           AND state IN ('pending','running','awaiting_review')
           AND id <> COALESCE((SELECT chosen_task_id FROM orch_task_groups WHERE id = ?2), '')",
        params![task_id, group_id, now],
    )?;
    Ok(n == 1)
}

// ── 014 — log-history por tarea (FR-003) ─────────────────────────────────────
// Persistir el scrollback PTY (ANSI-stripped) por tarea, capturado por el poller (012) + en
// mark-ready. Append-only con cap/rotación por tarea (edge-case spec: buffer grande → rotación).

/// Cuántos snapshots de log-history retenemos por tarea (rotación FIFO).
pub const LOG_HISTORY_CAP: usize = 200;

/// Persiste un snapshot del buffer-tail de una tarea. `lines` ya viene ANSI-stripped (snapshot()).
/// De-dup ligero: si el contenido es idéntico al último snapshot de la tarea, no inserta (el poller
/// corre cada 2s sobre un buffer que cambia poco). Rota a LOG_HISTORY_CAP por tarea.
pub fn append_log_history(db: &Db, task_id: &str, source: &str, lines: &[String]) -> Result<bool> {
    // F-I BYOK (audit codex+deepseek 014): the PTY scrollback can contain secrets the user
    // or a command printed (tokens, env dumps, an echoed key). We persist log-history to
    // SQLite, so REDACT before it lands at rest in the DB — reuse the same redactor the
    // TTS / LLM-assist paths use. The history's purpose is the command/output *shape*, not
    // secret values.
    let content = crate::services::tts::redact_secrets(&lines.join("\n"));
    if content.trim().is_empty() {
        return Ok(false);
    }
    let conn = db.lock();
    // de-dup contra el último snapshot.
    let last: Option<String> = conn
        .query_row(
            "SELECT content FROM orch_log_history WHERE task_id = ?1 ORDER BY captured_at DESC, rowid DESC LIMIT 1",
            params![task_id],
            |r| r.get(0),
        )
        .ok();
    if last.as_deref() == Some(content.as_str()) {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO orch_log_history (id, task_id, source, content) VALUES (?1,?2,?3,?4)",
        params![Uuid::new_v4().to_string(), task_id, source, content],
    )?;
    // rotación FIFO: borrar los más viejos por encima del cap.
    conn.execute(
        "DELETE FROM orch_log_history WHERE task_id = ?1 AND id NOT IN (
            SELECT id FROM orch_log_history WHERE task_id = ?1 ORDER BY captured_at DESC, rowid DESC LIMIT ?2
        )",
        params![task_id, LOG_HISTORY_CAP as i64],
    )?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogHistoryEntry {
    pub id: String,
    pub task_id: String,
    pub captured_at: String,
    pub source: String,
    pub content: String,
}

/// Devuelve el log-history de una tarea (más reciente primero, hasta `limit`).
pub fn get_log_history(db: &Db, task_id: &str, limit: i64) -> Result<Vec<LogHistoryEntry>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, task_id, captured_at, source, content FROM orch_log_history
         WHERE task_id = ?1 ORDER BY captured_at DESC, rowid DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(
            params![task_id, limit.clamp(1, LOG_HISTORY_CAP as i64)],
            |r| {
                Ok(LogHistoryEntry {
                    id: r.get(0)?,
                    task_id: r.get(1)?,
                    captured_at: r.get(2)?,
                    source: r.get(3)?,
                    content: r.get(4)?,
                })
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── 014 — lock registry por archivo/recurso (FR-005) ─────────────────────────
// Coordinación más allá del aislamiento por worktree: dos tareas del mismo repo que comparten un
// puerto/DB de dev, o la fase `git worktree add` (que no es 100% concurrente-safe en el index del
// repo padre). Lock advisory, no bloqueante: try_acquire devuelve quién es el dueño actual.

/// Intenta adquirir el lock de `resource_key` para `task_id`. Devuelve `Ok(None)` si lo consiguió
/// (o ya lo tenía — reentrante), `Ok(Some(owner))` si lo tiene OTRA tarea. Limpia locks vencidos
/// (expires_at < now) antes de evaluar. `ttl_secs` None = sin TTL.
pub fn try_acquire_lock(
    db: &Db,
    resource_key: &str,
    task_id: &str,
    ttl_secs: Option<i64>,
) -> Result<Option<String>> {
    let conn = db.lock();
    // GC de locks vencidos (cualquier recurso) — barato y mantiene el registry sano.
    conn.execute(
        "DELETE FROM orch_resource_locks WHERE expires_at IS NOT NULL AND expires_at < datetime('now')",
        [],
    )?;
    let expires = ttl_secs.map(|s| format!("+{} seconds", s));
    // INSERT-or-noop: si la fila no existe la creamos con este dueño. Atómico bajo el mutex.
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO orch_resource_locks (resource_key, owner_task_id, acquired_at, expires_at)
         VALUES (?1, ?2, datetime('now'),
                 CASE WHEN ?3 IS NULL THEN NULL ELSE datetime('now', ?3) END)",
        params![resource_key, task_id, expires],
    )?;
    if inserted == 1 {
        return Ok(None); // lo adquirimos
    }
    // ya existe — ¿de quién es?
    let owner: Option<String> = conn
        .query_row(
            "SELECT owner_task_id FROM orch_resource_locks WHERE resource_key = ?1",
            params![resource_key],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    match owner {
        Some(o) if o == task_id => {
            // reentrante: ya lo teníamos. Audit fix codex 014: RENOVAR expires_at — si el
            // holder sigue trabajando y re-adquiere, su TTL no debe vencer y dejar que el GC
            // libere un lock activo (heartbeat-via-reacquire).
            conn.execute(
                "UPDATE orch_resource_locks SET expires_at =
                    CASE WHEN ?3 IS NULL THEN NULL ELSE datetime('now', ?3) END
                 WHERE resource_key = ?1 AND owner_task_id = ?2",
                params![resource_key, task_id, expires],
            )?;
            Ok(None)
        }
        Some(o) => Ok(Some(o)), // lo tiene otra tarea
        None => {
            // fila huérfana sin dueño (releaseada) — re-clamarla.
            conn.execute(
                "UPDATE orch_resource_locks SET owner_task_id = ?2, acquired_at = datetime('now'),
                    expires_at = CASE WHEN ?3 IS NULL THEN NULL ELSE datetime('now', ?3) END
                 WHERE resource_key = ?1 AND owner_task_id IS NULL",
                params![resource_key, task_id, expires],
            )?;
            Ok(None)
        }
    }
}

/// Libera el lock de `resource_key` SI lo tiene `task_id` (no roba locks ajenos). Devuelve true
/// si lo liberó. Idempotente.
pub fn release_lock(db: &Db, resource_key: &str, task_id: &str) -> Result<bool> {
    let conn = db.lock();
    let n = conn.execute(
        "DELETE FROM orch_resource_locks WHERE resource_key = ?1 AND owner_task_id = ?2",
        params![resource_key, task_id],
    )?;
    Ok(n == 1)
}

/// Libera TODOS los locks de una tarea (al terminar/cancelar). Best-effort.
pub fn release_all_locks(db: &Db, task_id: &str) -> Result<usize> {
    let conn = db.lock();
    let n = conn.execute(
        "DELETE FROM orch_resource_locks WHERE owner_task_id = ?1",
        params![task_id],
    )?;
    Ok(n)
}

/// FR-005 — serialización IN-PROCESS de la fase `git worktree add` POR REPO. Dos launches
/// concurrentes sobre el MISMO repo pueden chocar en el index del repo padre (`git worktree add`
/// no es 100% concurrente-safe). Esto da un Mutex por repo_path; el caller lo sostiene mientras
/// crea el worktree. Distinto del lock registry (DB, advisory, recursos como puertos): esto es un
/// guard de proceso para una fase corta de git. Devuelve el guard (drop = libera).
pub fn repo_worktree_lock(repo_path: &str) -> std::sync::Arc<parking_lot::Mutex<()>> {
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    static LOCKS: Lazy<
        parking_lot::Mutex<HashMap<String, std::sync::Arc<parking_lot::Mutex<()>>>>,
    > = Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
    // Audit fix codex 014: canonicalizar el key — el mismo repo accedido por symlink, ruta
    // relativa o trailing-slash distinto produciría keys distintos → el lock no serializaría.
    // Canonicalizamos (cae al string crudo si la ruta no existe, p.ej. en tests).
    let key = std::fs::canonicalize(repo_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| repo_path.to_string());
    let mut map = LOCKS.lock();
    map.entry(key)
        .or_insert_with(|| std::sync::Arc::new(parking_lot::Mutex::new(())))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql"))
            .unwrap();
        // 012: las columnas needs_input/auto_confirm/cli_kind que row_to_task ahora selecciona.
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql"))
            .unwrap();
        // 014: group_id/variant_index + grupos + log-history + lock registry.
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql"))
            .unwrap();
        // 019 F3: columna paused_at (pause/resume) — el SELECT de row_to_task ya la pide.
        conn.execute_batch(include_str!(
            "../../migrations/037_orch_pause_council_history.sql"
        ))
        .unwrap();
        // 038 F1.0: pipeline_runs + pipeline_edges + ALTER (pipeline_run_id/dag_blocked/topo_index)
        // — `enrich_dag_fields`/`claim_for_launch` los referencian; sin esto los tests rompen.
        conn.execute_batch(include_str!("../../migrations/047_pipeline_dag.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    #[test]
    fn slugify_and_branch_name() {
        assert_eq!(slugify("Fix the Login Bug!"), "fix-the-login-bug");
        assert_eq!(slugify("   "), "task");
        let b = branch_name("batch1234abcd", "task5678efgh", "Add feature X");
        assert!(b.starts_with("furx/orch/batch123/add-feature-x-task5678"));
        assert!(!b.contains(' '));
    }

    #[test]
    fn state_machine_transitions() {
        assert!(can_transition("pending", "running"));
        assert!(can_transition("running", "awaiting_review"));
        assert!(can_transition("awaiting_review", "done"));
        assert!(can_transition("running", "canceled"));
        // inválidas
        assert!(!can_transition("pending", "done")); // no se salta running
        assert!(!can_transition("done", "running")); // terminal
        assert!(!can_transition("awaiting_review", "running"));
        assert!(!can_transition("canceled", "done"));
    }

    #[test]
    fn claim_for_launch_is_atomic() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/r",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        assert!(claim_for_launch(&db, id).unwrap()); // primer claim gana
        assert!(!claim_for_launch(&db, id).unwrap()); // segundo pierde (ya no pending)
        assert_eq!(get_task(&db, id).unwrap().unwrap().state, "running");
    }

    #[test]
    fn branch_name_within_limit() {
        let b = branch_name(
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
            "Un título larguísimo con muchas palabras que excede a lo loco el límite",
        );
        assert!(b.len() <= 64, "branch demasiado larga: {} ({})", b, b.len());
    }

    #[test]
    fn batch_crud_and_lifecycle() {
        let db = test_db();
        let (batch_id, tasks) = create_batch(
            &db,
            "mi batch",
            "/tmp/repo",
            Some("main"),
            Some("abc123"),
            &[
                TaskSpec {
                    title: "Tarea A".into(),
                    objective: "hacé A".into(),
                    agent_profile_id: Some("ag1".into()),
                    mode: None,
                },
                TaskSpec {
                    title: "Tarea B".into(),
                    objective: String::new(),
                    agent_profile_id: None,
                    mode: Some("claude-A".into()),
                },
            ],
        )
        .unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.state == "pending"));
        assert!(tasks[0].branch.starts_with("furx/orch/"));
        assert_ne!(tasks[0].branch, tasks[1].branch); // branch única por tarea

        let t = &tasks[0];
        // lifecycle: pending → running → awaiting_review → done
        mark_running(&db, &t.id, "/tmp/repo/.wt/a", Some("pane-x")).unwrap();
        assert_eq!(get_task(&db, &t.id).unwrap().unwrap().state, "running");
        set_result_summary(&db, &t.id, " file.rs | 3 +++").unwrap();
        set_state(&db, &t.id, "awaiting_review", None).unwrap();
        set_state(&db, &t.id, "done", Some(0)).unwrap();
        let done = get_task(&db, &t.id).unwrap().unwrap();
        assert_eq!(done.state, "done");
        assert_eq!(done.exit_code, Some(0));
        assert_eq!(done.worktree_path.as_deref(), Some("/tmp/repo/.wt/a"));
        assert!(done.result_summary.is_some());

        // transición inválida rechazada
        assert!(set_state(&db, &tasks[1].id, "done", None).is_err()); // pending→done no

        // list filtra por batch
        assert_eq!(list_tasks(&db, Some(&batch_id)).unwrap().len(), 2);
    }

    // ── 014 best-of-N ────────────────────────────────────────────────────────

    #[test]
    fn best_of_n_creates_group_and_variants() {
        let db = test_db();
        let (batch_id, group, tasks) = create_best_of_n(
            &db,
            "Implementá X",
            "/tmp/repo",
            Some("main"),
            None,
            "objetivo común",
            &[None, Some("ag1".into()), Some("ag2".into())],
        )
        .unwrap();
        assert_eq!(group.n, 3);
        assert_eq!(group.batch_id, batch_id);
        assert_eq!(tasks.len(), 3);
        // todas pending, mismo objetivo, branches únicas, variant_index 0..2.
        assert!(tasks.iter().all(|t| t.state == "pending"));
        assert!(tasks.iter().all(|t| t.objective == "objetivo común"));
        assert!(tasks
            .iter()
            .all(|t| t.group_id.as_deref() == Some(group.id.as_str())));
        let mut idxs: Vec<i64> = tasks.iter().filter_map(|t| t.variant_index).collect();
        idxs.sort();
        assert_eq!(idxs, vec![0, 1, 2]);
        let branches: std::collections::HashSet<_> = tasks.iter().map(|t| &t.branch).collect();
        assert_eq!(branches.len(), 3, "branch única por variante");
        // list_group_tasks ordena por variant_index.
        let g = list_group_tasks(&db, &group.id).unwrap();
        assert_eq!(
            g.iter().filter_map(|t| t.variant_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn best_of_n_rejects_too_many_variants() {
        let db = test_db();
        let r = create_best_of_n(
            &db,
            "X",
            "/tmp/r",
            None,
            None,
            "o",
            &[None, None, None, None, None],
        );
        assert!(r.is_err(), "5 variantes > MAX_VARIANTS debe fallar");
        assert!(
            create_best_of_n(&db, "X", "/tmp/r", None, None, "o", &[]).is_err(),
            "0 variantes falla"
        );
    }

    #[test]
    fn choose_and_discard_variants() {
        let db = test_db();
        let (_b, group, tasks) =
            create_best_of_n(&db, "X", "/tmp/repo", None, None, "o", &[None, None, None]).unwrap();
        let chosen = &tasks[0].id;
        choose_variant(&db, &group.id, chosen).unwrap();
        assert_eq!(
            get_group(&db, &group.id)
                .unwrap()
                .unwrap()
                .chosen_task_id
                .as_deref(),
            Some(chosen.as_str())
        );
        // no se puede descartar la elegida.
        assert!(discard_variant(&db, &group.id, chosen).is_err());
        // las otras dos se descartan (canceled).
        assert!(discard_variant(&db, &group.id, &tasks[1].id).unwrap());
        assert!(discard_variant(&db, &group.id, &tasks[2].id).unwrap());
        assert_eq!(
            get_task(&db, &tasks[1].id).unwrap().unwrap().state,
            "canceled"
        );
        assert_eq!(
            get_task(&db, &tasks[2].id).unwrap().unwrap().state,
            "canceled"
        );
        // descartar una ya-terminal es no-op (false), no error.
        assert!(!discard_variant(&db, &group.id, &tasks[1].id).unwrap());
    }

    #[test]
    fn log_history_redacts_secrets_before_persisting() {
        // Audit F-I BYOK 014: el scrollback puede traer secrets; NO deben quedar at-rest en SQLite.
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/r",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        append_log_history(
            &db,
            id,
            "poller",
            &[
                "running build".into(),
                "export OPENAI_API_KEY=sk-proj-ABCDEFGHIJKLMNOPQRSTUV".into(),
                "token=ghp_abcdefghijklmnopqrstuvwxyz0123456789".into(),
            ],
        )
        .unwrap();
        let hist = get_log_history(&db, id, 10).unwrap();
        let joined = hist
            .iter()
            .map(|e| e.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !joined.contains("sk-proj-ABCDEFGHIJKLMNOPQRSTUV"),
            "no debe persistir la API key"
        );
        assert!(
            !joined.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "no debe persistir el token"
        );
        assert!(
            joined.contains("running build"),
            "el resto del output sí se conserva"
        );
    }

    #[test]
    fn choose_variant_rejects_foreign_task() {
        let db = test_db();
        let (_b, group, _t) =
            create_best_of_n(&db, "X", "/tmp/repo", None, None, "o", &[None]).unwrap();
        let (_b2, other) = create_batch(
            &db,
            "b2",
            "/tmp/repo2",
            None,
            None,
            &[TaskSpec {
                title: "Y".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        assert!(
            choose_variant(&db, &group.id, &other[0].id).is_err(),
            "tarea ajena al grupo rechazada"
        );
    }

    // ── 014 log-history ──────────────────────────────────────────────────────

    #[test]
    fn log_history_append_dedup_and_rotate() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/repo",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        // primer append OK.
        assert!(
            append_log_history(&db, id, "poller", &["línea 1".into(), "línea 2".into()]).unwrap()
        );
        // mismo contenido → de-dup (no inserta).
        assert!(
            !append_log_history(&db, id, "poller", &["línea 1".into(), "línea 2".into()]).unwrap()
        );
        // contenido distinto → inserta.
        assert!(append_log_history(&db, id, "mark_ready", &["línea 3".into()]).unwrap());
        // vacío → no inserta.
        assert!(!append_log_history(&db, id, "poller", &["  ".into()]).unwrap());
        let hist = get_log_history(&db, id, 50).unwrap();
        assert_eq!(hist.len(), 2);
        // más reciente primero.
        assert_eq!(hist[0].source, "mark_ready");
        assert_eq!(hist[0].content, "línea 3");
    }

    #[test]
    fn log_history_rotation_caps_per_task() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/repo",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        for i in 0..(LOG_HISTORY_CAP + 20) {
            append_log_history(&db, id, "poller", &[format!("snapshot {i}")]).unwrap();
        }
        let conn = db.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM orch_log_history WHERE task_id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            n as usize <= LOG_HISTORY_CAP,
            "rotación FIFO mantiene ≤ cap (n={})",
            n
        );
    }

    // ── 014 lock registry ────────────────────────────────────────────────────

    #[test]
    fn lock_registry_acquire_contend_release() {
        let db = test_db();
        // task A adquiere port:3000.
        assert_eq!(
            try_acquire_lock(&db, "port:3000", "taskA", None).unwrap(),
            None
        );
        // reentrante: A lo vuelve a pedir → ok (None).
        assert_eq!(
            try_acquire_lock(&db, "port:3000", "taskA", None).unwrap(),
            None
        );
        // task B contiende → ve a A como dueño.
        assert_eq!(
            try_acquire_lock(&db, "port:3000", "taskB", None).unwrap(),
            Some("taskA".to_string())
        );
        // A libera; B lo puede tomar.
        assert!(release_lock(&db, "port:3000", "taskA").unwrap());
        assert_eq!(
            try_acquire_lock(&db, "port:3000", "taskB", None).unwrap(),
            None
        );
        // B no puede liberar un lock ajeno (ya es suyo, pero A intenta liberar → false).
        assert!(!release_lock(&db, "port:3000", "taskA").unwrap());
    }

    #[test]
    fn lock_release_all_for_task() {
        let db = test_db();
        try_acquire_lock(&db, "port:3000", "taskA", None).unwrap();
        try_acquire_lock(&db, "devdb:furx", "taskA", None).unwrap();
        try_acquire_lock(&db, "port:4000", "taskB", None).unwrap();
        assert_eq!(release_all_locks(&db, "taskA").unwrap(), 2);
        // el de B sigue.
        assert_eq!(
            try_acquire_lock(&db, "port:4000", "taskA", None).unwrap(),
            Some("taskB".to_string())
        );
    }

    // ── 019 F3 (T030) pause/resume ───────────────────────────────────────────

    #[test]
    fn pause_resume_is_transactional_and_idempotent() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/repo",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        // no se puede pausar una tarea pending (no corre).
        assert!(pause_task(&db, id).is_err());
        // resume de algo no-pausado = no-op (false), no error.
        assert!(!resume_task(&db, id).unwrap());
        // pasar a running → pausable.
        mark_running(&db, id, "/tmp/repo/.wt/a", Some("pane-x")).unwrap();
        assert!(pause_task(&db, id).unwrap(), "primer pause gana");
        assert!(get_task(&db, id).unwrap().unwrap().paused_at.is_some());
        // segundo pause = idempotente (false, no re-pisa).
        assert!(!pause_task(&db, id).unwrap());
        // resume limpia el flag.
        assert!(resume_task(&db, id).unwrap());
        assert!(get_task(&db, id).unwrap().unwrap().paused_at.is_none());
        // segundo resume = idempotente.
        assert!(!resume_task(&db, id).unwrap());
        // sigue en running (pause NO cambió la state-machine).
        assert_eq!(get_task(&db, id).unwrap().unwrap().state, "running");
    }

    // ── 019 F3 (T030) ETA (cálculo puro) ─────────────────────────────────────

    #[test]
    fn eta_none_without_terminal_base() {
        // sin ningún attempt terminado → no hay base → None (no inventamos un número).
        let t = [AttemptTiming {
            running: true,
            terminal: false,
            elapsed_secs: 10.0,
        }];
        assert!(estimate_eta(&t).is_none());
    }

    #[test]
    fn eta_none_without_running() {
        // todos terminados, nada corriendo → None (no hay ETA que mostrar).
        let t = [AttemptTiming {
            running: false,
            terminal: true,
            elapsed_secs: 30.0,
        }];
        assert!(estimate_eta(&t).is_none());
    }

    #[test]
    fn eta_projects_from_terminal_average() {
        // 2 terminados (20s y 40s → avg 30) + 1 running con 10s elapsed → le faltan 20s.
        let t = [
            AttemptTiming {
                running: false,
                terminal: true,
                elapsed_secs: 20.0,
            },
            AttemptTiming {
                running: false,
                terminal: true,
                elapsed_secs: 40.0,
            },
            AttemptTiming {
                running: true,
                terminal: false,
                elapsed_secs: 10.0,
            },
        ];
        let e = estimate_eta(&t).unwrap();
        assert_eq!(e.avg_terminal_secs, 30.0);
        assert_eq!(e.finished, 2);
        assert_eq!(e.running, 1);
        assert_eq!(e.eta_secs, 20.0);
    }

    #[test]
    fn eta_running_over_average_clamps_to_zero_and_takes_max() {
        // avg = 30. Dos running: uno con 50s (ya pasó el avg → 0) y otro con 5s (le faltan 25).
        // El ETA del conjunto = el máximo (corren en paralelo) = 25, nunca negativo.
        let t = [
            AttemptTiming {
                running: false,
                terminal: true,
                elapsed_secs: 30.0,
            },
            AttemptTiming {
                running: true,
                terminal: false,
                elapsed_secs: 50.0,
            },
            AttemptTiming {
                running: true,
                terminal: false,
                elapsed_secs: 5.0,
            },
        ];
        let e = estimate_eta(&t).unwrap();
        assert_eq!(e.eta_secs, 25.0);
        assert_eq!(e.running, 2);
    }

    #[test]
    fn task_timing_maps_states() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/repo",
            None,
            None,
            &[TaskSpec {
                title: "T".into(),
                objective: String::new(),
                agent_profile_id: None,
                mode: None,
            }],
        )
        .unwrap();
        let id = &tasks[0].id;
        let now = Utc::now();
        // pending → no aporta señal.
        assert!(task_timing(&get_task(&db, id).unwrap().unwrap(), now).is_none());
        // running → aporta como running.
        mark_running(&db, id, "/tmp/repo/.wt/a", None).unwrap();
        let tm = task_timing(
            &get_task(&db, id).unwrap().unwrap(),
            now + chrono::Duration::seconds(5),
        )
        .unwrap();
        assert!(tm.running && !tm.terminal);
        assert!(tm.elapsed_secs >= 4.0 && tm.elapsed_secs <= 6.0);
    }

    // ── SC-001 e2e best-of-N sobre un repo git REAL ──────────────────────────
    // 3 variantes → 3 worktrees (worktree::ensure real) → 3 diffs distintos → elegir 1, descartar 2.
    // El repo se crea bajo $HOME (worktree::ensure exige $HOME). Se limpia al final.

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn e2e_best_of_n_three_variants_choose_one_discard_two() {
        // repo de prueba bajo $HOME (requisito de worktree::ensure).
        let home = dirs::home_dir().unwrap();
        let root = home.join(".furx").join("e2e-tests");
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join(format!("bestofn-{}", &Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t.io"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        // 3 variantes del mismo objetivo.
        let (batch_id, group, tasks) = create_best_of_n(
            &db,
            "Implementá feature X",
            &repo_str,
            Some("main"),
            None,
            "implementá la feature X de 3 maneras",
            &[None, None, None],
        )
        .unwrap();
        assert_eq!(tasks.len(), 3, "3 variantes creadas");
        assert_eq!(group.n, 3);

        // Lanzar cada variante: worktree REAL + un cambio DISTINTO por variante (simula al agente).
        let mut wt_paths = Vec::new();
        for (i, t) in tasks.iter().enumerate() {
            assert!(claim_for_launch(&db, &t.id).unwrap());
            let wt = crate::services::worktree::ensure(&repo, &t.branch).unwrap();
            mark_running(
                &db,
                &t.id,
                &wt.worktree_path,
                Some(&format!("orch-{}", t.id)),
            )
            .unwrap();
            // cambio distinto por variante.
            std::fs::write(
                std::path::Path::new(&wt.worktree_path).join(format!("variant_{}.txt", i)),
                format!("approach {}\n", i),
            )
            .unwrap();
            git(std::path::Path::new(&wt.worktree_path), &["add", "."]);
            wt_paths.push(wt.worktree_path);
        }

        // 3 worktrees REALES, distintos.
        let uniq: std::collections::HashSet<_> = wt_paths.iter().collect();
        assert_eq!(uniq.len(), 3, "3 worktrees aislados");

        // Comparar: cada variante tiene un diff DISTINTO (staged → diff --stat HEAD).
        let diffs: Vec<String> = tasks
            .iter()
            .map(|t| {
                let wt = get_task(&db, &t.id)
                    .unwrap()
                    .unwrap()
                    .worktree_path
                    .unwrap();
                collect_diff(&wt)
            })
            .collect();
        for (i, d) in diffs.iter().enumerate() {
            assert!(
                d.contains(&format!("variant_{}.txt", i)),
                "diff de v{} muestra su archivo: {}",
                i,
                d
            );
        }
        // los 3 diffs son distintos entre sí.
        assert_ne!(diffs[0], diffs[1]);
        assert_ne!(diffs[1], diffs[2]);

        // Elegir la variante 1 (índice 1).
        let chosen = &tasks[1].id;
        choose_variant(&db, &group.id, chosen).unwrap();
        assert_eq!(
            get_group(&db, &group.id)
                .unwrap()
                .unwrap()
                .chosen_task_id
                .as_deref(),
            Some(chosen.as_str())
        );

        // Descartar las otras 2 (constitución VI: el caller confirma; acá llamamos directo).
        let mut discarded = 0;
        for t in &tasks {
            if &t.id != chosen
                && discard_variant(&db, &group.id, &t.id).unwrap() {
                    discarded += 1;
                }
        }
        assert_eq!(discarded, 2, "2 variantes descartadas");
        // la elegida NO se puede descartar.
        assert!(discard_variant(&db, &group.id, chosen).is_err());
        // estados finales: chosen running (lista para merge), las otras canceled.
        assert_eq!(get_task(&db, chosen).unwrap().unwrap().state, "running");
        for t in &tasks {
            if &t.id != chosen {
                assert_eq!(get_task(&db, &t.id).unwrap().unwrap().state, "canceled");
            }
        }

        // cleanup: remover worktrees + borrar el repo de prueba.
        for wt in &wt_paths {
            let _ = std::process::Command::new("git")
                .current_dir(&repo)
                .args(["worktree", "remove", "--force", wt])
                .output();
        }
        let _ = std::fs::remove_dir_all(&repo);
        let _ = batch_id; // silenciar unused
    }

    // ── 038 F1.1 — guarda de claim + depends_on derivado ────────────────────────

    /// Helper: arma un batch de 2 tareas, las cablea como un edge a→b en un run de pipeline.
    /// Devuelve (db, run_id, id_a, id_b). `b` queda `dag_blocked=1` (depende de `a`).
    fn dag_two_tasks() -> (Db, String, String, String) {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "p",
            "/tmp/r",
            None,
            None,
            &[
                TaskSpec { title: "A".into(), objective: String::new(), agent_profile_id: None, mode: None },
                TaskSpec { title: "B".into(), objective: String::new(), agent_profile_id: None, mode: None },
            ],
        )
        .unwrap();
        let id_a = tasks[0].id.clone();
        let id_b = tasks[1].id.clone();
        let run_id = Uuid::new_v4().to_string();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO pipeline_runs (id, batch_id) VALUES (?1, ?2)",
                params![run_id, _b],
            )
            .unwrap();
            set_task_dependencies(
                &conn,
                &run_id,
                &[id_a.clone(), id_b.clone()],
                &[DagEdge {
                    task_id: id_b.clone(),
                    depends_on_task_id: id_a.clone(),
                    on_error: None,
                }],
            )
            .unwrap();
        }
        (db, run_id, id_a, id_b)
    }

    /// 038 F1.1 — `claim_for_launch` RECHAZA una tarea `dag_blocked=1` (esperando deps): no la pasa a
    /// running. La raíz (`dag_blocked=0`) SÍ se claimea.
    #[test]
    fn claim_rejects_dag_blocked_task() {
        let (db, _run, id_a, id_b) = dag_two_tasks();
        // b está bloqueada → claim falla, sigue pending.
        assert!(!claim_for_launch(&db, &id_b).unwrap(), "una tarea bloqueada no se claimea");
        assert_eq!(get_task(&db, &id_b).unwrap().unwrap().state, "pending");
        // a (raíz) sí se claimea.
        assert!(claim_for_launch(&db, &id_a).unwrap(), "la raíz sí se claimea");
        assert_eq!(get_task(&db, &id_a).unwrap().unwrap().state, "running");
    }

    /// 038 F1.1 — una tarea SINGLE-TASK (sin run/aristas) serializa `depends_on=[]`, `pipeline_run_id=None`,
    /// `dag_blocked=0` — idéntico a antes de 038. (Cero regresión, criterio de aceptación #2.)
    #[test]
    fn single_task_has_empty_depends_on() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db, "b", "/tmp/r", None, None,
            &[TaskSpec { title: "Solo".into(), objective: String::new(), agent_profile_id: None, mode: None }],
        )
        .unwrap();
        let t = get_task(&db, &tasks[0].id).unwrap().unwrap();
        assert!(t.depends_on.is_empty(), "single-task no tiene deps");
        assert!(t.pipeline_run_id.is_none());
        assert_eq!(t.dag_blocked, 0);
        // Y `list_tasks` lo serializa igual.
        let listed = list_tasks(&db, Some(&_b)).unwrap();
        assert!(listed[0].depends_on.is_empty());
        // Serde: el JSON de una single-task tiene `depends_on: []` y `pipeline_run_id: null`.
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["depends_on"], serde_json::json!([]));
        assert!(json["pipeline_run_id"].is_null());
    }

    /// 038 F1.1 — `list_tasks`/`get_task` POBLAN `depends_on` con los ids de las deps de una tarea del
    /// DAG, y marcan `dag_blocked`/`pipeline_run_id`.
    #[test]
    fn dag_task_exposes_depends_on() {
        let (db, run, id_a, id_b) = dag_two_tasks();
        let b = get_task(&db, &id_b).unwrap().unwrap();
        assert_eq!(b.depends_on, vec![id_a.clone()]);
        assert_eq!(b.dag_blocked, 1);
        assert_eq!(b.pipeline_run_id.as_deref(), Some(run.as_str()));
        // La raíz no tiene deps y está lanzable.
        let a = get_task(&db, &id_a).unwrap().unwrap();
        assert!(a.depends_on.is_empty());
        assert_eq!(a.dag_blocked, 0);
    }

    /// 038 F1.1 — `deps_all_done`: false mientras la dep no esté `done`; true cuando cierra. Una arista
    /// `on_error='continue'` se satisface si la dep TERMINA (incluso failed), pero NO mientras corre.
    #[test]
    fn deps_all_done_semantics() {
        let (db, _run, id_a, id_b) = dag_two_tasks();
        {
            let conn = db.lock();
            // a aún pending → b no está listo.
            assert!(!deps_all_done(&conn, &id_b).unwrap());
        }
        // a → running → awaiting_review → done.
        claim_for_launch(&db, &id_a).unwrap();
        set_state(&db, &id_a, "awaiting_review", None).unwrap();
        {
            let conn = db.lock();
            assert!(!deps_all_done(&conn, &id_b).unwrap(), "awaiting_review NO satisface (espera done)");
        }
        set_state(&db, &id_a, "done", None).unwrap();
        {
            let conn = db.lock();
            assert!(deps_all_done(&conn, &id_b).unwrap(), "dep done → satisfecho");
        }
        // Sin aristas (raíz) → true.
        {
            let conn = db.lock();
            assert!(deps_all_done(&conn, &id_a).unwrap());
        }
    }

    /// 038 F1.1 — `on_error='continue'`: una dep que FALLA satisface al dependiente (best-effort), pero
    /// una dep que aún corre NO.
    #[test]
    fn deps_all_done_continue_on_error() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db, "p", "/tmp/r", None, None,
            &[
                TaskSpec { title: "A".into(), objective: String::new(), agent_profile_id: None, mode: None },
                TaskSpec { title: "B".into(), objective: String::new(), agent_profile_id: None, mode: None },
            ],
        )
        .unwrap();
        let (id_a, id_b) = (tasks[0].id.clone(), tasks[1].id.clone());
        let run_id = Uuid::new_v4().to_string();
        {
            let conn = db.lock();
            conn.execute("INSERT INTO pipeline_runs (id, batch_id) VALUES (?1,?2)", params![run_id, _b]).unwrap();
            set_task_dependencies(
                &conn, &run_id, &[id_a.clone(), id_b.clone()],
                &[DagEdge { task_id: id_b.clone(), depends_on_task_id: id_a.clone(), on_error: Some("continue".into()) }],
            ).unwrap();
            // a pending → no satisface (upstream vivo, ni con continue).
            assert!(!deps_all_done(&conn, &id_b).unwrap());
        }
        claim_for_launch(&db, &id_a).unwrap();
        set_state(&db, &id_a, "failed", None).unwrap();
        {
            let conn = db.lock();
            assert!(deps_all_done(&conn, &id_b).unwrap(), "continue: dep failed satisface al dependiente");
        }
    }

    /// 038 F1.1 (red-team #8) — GUARDA anti tercera ruta de spawn: el único `UPDATE ... state='running'`
    /// directo en `src/` debe ser `claim_for_launch` (prod, con la guarda `dag_blocked=0`) y el trigger
    /// de test `steal_claim` (best_of_n.rs). Si aparece una 3ª ruta directa (que evadiría el gate del
    /// DAG), este test FALLA y obliga a routearla por `claim_for_launch`. Escanea el fuente al compilar.
    #[test]
    fn no_third_direct_running_spawn_route() {
        // Escanea TODO el árbol `src-tauri/src/` (no sólo 4 archivos — audit deepseek): cualquier
        // `UPDATE ... SET state='running'` directo evade el gate `dag_blocked=0`. El barrido recursivo
        // garantiza que un archivo nuevo no introduzca una ruta sin que el guard la vea.
        fn collapse_ws(s: &str) -> String {
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        }
        // `SET state='running'` (la MUTACIÓN), NO un `WHERE state='running'` (lectura, p.ej. el guard de
        // pause). El `SET ` exige que sea una asignación.
        let needles = ["SET state='running'", "SET state = 'running'"]; // GUARD_IGNORE
        // Recorre src/ recursivamente y junta los .rs.
        fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        collect_rs(&p, out);
                    } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                        out.push(p);
                    }
                }
            }
        }
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs(&src_root, &mut files);
        assert!(files.len() > 10, "el barrido de src/ debe ver muchos archivos, vio {}", files.len());

        let mut hits: Vec<String> = Vec::new();
        for path in &files {
            let src = std::fs::read_to_string(path).unwrap_or_default();
            let name = path
                .strip_prefix(&src_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            for (lineno, line) in src.lines().enumerate() {
                if line.contains("GUARD_IGNORE") {
                    continue;
                }
                let code = match line.find("//") {
                    Some(i) => &line[..i],
                    None => line,
                };
                let norm = collapse_ws(code);
                if needles.iter().any(|n| norm.contains(n)) {
                    hits.push(format!("{name}:{}", lineno + 1));
                }
            }
        }
        // Permitidas: claim_for_launch (orchestration.rs, con `AND dag_blocked=0`) + steal_claim
        // trigger (best_of_n.rs, test). Exactamente 2 en TODO src/. Cualquier otra = ruta que evade
        // el gate y debe routearse por `claim_for_launch`.
        assert_eq!(
            hits.len(),
            2,
            "rutas directas de spawn detectadas en src/ (esperadas 2: claim_for_launch + steal_claim): {hits:?}. \
             Una nueva ruta a 'running' debe pasar por claim_for_launch (gate dag_blocked=0)."
        );
        assert!(hits.iter().any(|h| h.contains("orchestration.rs")), "falta claim_for_launch");
        assert!(hits.iter().any(|h| h.contains("best_of_n.rs")), "falta steal_claim (test)");
    }

    // ── 038 F1.2 — create_pipeline_run (mapeo DAG→tasks atómico) ────────────────

    /// 038 F1.2 — el grafo persistido en DB == el DAG del pipeline: tareas en orden topo, aristas
    /// traducidas yaml_id→uuid, raíz lanzable y dependiente bloqueado.
    #[test]
    fn create_pipeline_run_maps_graph() {
        let db = test_db();
        let tasks = vec![
            ResolvedPipelineTask {
                yaml_id: "impl".into(), title: "Impl".into(), objective: "x".into(),
                agent_profile_id: None, mode: None,
            },
            ResolvedPipelineTask {
                yaml_id: "test".into(), title: "Test".into(), objective: "y".into(),
                agent_profile_id: None, mode: None,
            },
        ];
        let edges = vec![YamlEdge {
            task_yaml_id: "test".into(), depends_on_yaml_id: "impl".into(), on_error: None,
        }];
        let topo = vec!["impl".to_string(), "test".to_string()];
        let (run_id, batch_id, created) =
            create_pipeline_run(&db, "p", "/tmp/r", Some("main"), None, &tasks, &edges, &topo, "name: p\n...").unwrap();

        assert_eq!(created.len(), 2);
        // Orden topo: impl (topo_index 0) antes que test (1).
        let impl_t = created.iter().find(|t| t.title == "Impl").unwrap();
        let test_t = created.iter().find(|t| t.title == "Test").unwrap();
        // impl es raíz: lanzable; test depende → bloqueado.
        assert_eq!(impl_t.dag_blocked, 0, "la raíz es lanzable");
        assert_eq!(test_t.dag_blocked, 1, "el dependiente arranca bloqueado");
        assert_eq!(test_t.depends_on, vec![impl_t.id.clone()], "la arista apunta al uuid de impl");
        assert_eq!(impl_t.pipeline_run_id.as_deref(), Some(run_id.as_str()));
        // topo_index correcto en DB.
        {
            let conn = db.lock();
            let idx_impl: i64 = conn.query_row("SELECT topo_index FROM orchestration_tasks WHERE id=?1", params![impl_t.id], |r| r.get(0)).unwrap();
            let idx_test: i64 = conn.query_row("SELECT topo_index FROM orchestration_tasks WHERE id=?1", params![test_t.id], |r| r.get(0)).unwrap();
            assert!(idx_impl < idx_test, "impl debe tener topo_index menor que test");
            // pipeline_runs persistió yaml_sha256 + topo_json + 1:1 batch.
            let (sha, topo_json, name): (String, String, String) = conn.query_row(
                "SELECT yaml_sha256, topo_json, name FROM pipeline_runs WHERE id=?1", params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            assert_eq!(sha.len(), 64, "sha256 hex = 64 chars");
            assert_eq!(name, "p");
            let order: Vec<String> = serde_json::from_str(&topo_json).unwrap();
            assert_eq!(order, vec![impl_t.id.clone(), test_t.id.clone()]);
            let _ = batch_id;
        }
    }

    /// 038 F1.2 — la creación es ATÓMICA: si el orden topo no corresponde a las tareas, falla ANTES de
    /// tocar la DB (no deja batch parcial).
    #[test]
    fn create_pipeline_run_is_atomic_on_bad_input() {
        let db = test_db();
        let tasks = vec![ResolvedPipelineTask {
            yaml_id: "a".into(), title: "A".into(), objective: String::new(),
            agent_profile_id: None, mode: None,
        }];
        // topo no cubre la tarea → Err, sin crear nada.
        let r = create_pipeline_run(&db, "p", "/tmp/r", None, None, &tasks, &[], &["b".to_string()], "y");
        assert!(r.is_err());
        let n: i64 = {
            let conn = db.lock();
            conn.query_row("SELECT COUNT(*) FROM orchestration_batches", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(n, 0, "input inválido NO debe crear ningún batch");
    }

    /// 038 F1.2 — un diamante (a→b, a→c, b→d, c→d) persiste 4 tareas, a lanzable, d con 2 deps.
    #[test]
    fn create_pipeline_run_diamond() {
        let db = test_db();
        let mk = |id: &str| ResolvedPipelineTask {
            yaml_id: id.into(), title: id.to_uppercase(), objective: String::new(),
            agent_profile_id: None, mode: None,
        };
        let tasks = vec![mk("a"), mk("b"), mk("c"), mk("d")];
        let edges = vec![
            YamlEdge { task_yaml_id: "b".into(), depends_on_yaml_id: "a".into(), on_error: None },
            YamlEdge { task_yaml_id: "c".into(), depends_on_yaml_id: "a".into(), on_error: None },
            YamlEdge { task_yaml_id: "d".into(), depends_on_yaml_id: "b".into(), on_error: None },
            YamlEdge { task_yaml_id: "d".into(), depends_on_yaml_id: "c".into(), on_error: None },
        ];
        let topo = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let (_run, _batch, created) =
            create_pipeline_run(&db, "diamante", "/tmp/r", None, None, &tasks, &edges, &topo, "y").unwrap();
        let a = created.iter().find(|t| t.title == "A").unwrap();
        let d = created.iter().find(|t| t.title == "D").unwrap();
        assert_eq!(a.dag_blocked, 0);
        assert_eq!(d.dag_blocked, 1);
        assert_eq!(d.depends_on.len(), 2, "d depende de b y c");
    }

    /// 038 F1.1 (audit codex BLOCKER) — el gate del DAG NO es evadible por `set_state`: una tarea
    /// `dag_blocked=1` NO puede pasar `pending→running` ni siquiera por la ruta dinámica
    /// `set_state(id,"running")` (que `mark_running` usa). Cierra la 3ª ruta que el guard de literales
    /// SQL no ve (porque `set_state` emite `SET state = ?2`).
    #[test]
    fn set_state_rejects_running_when_dag_blocked() {
        let (db, _run, _id_a, id_b) = dag_two_tasks();
        // b está dag_blocked=1.
        let err = set_state(&db, &id_b, "running", None).unwrap_err();
        assert!(
            err.to_string().contains("dag_blocked") || err.to_string().contains("dependencias"),
            "set_state debe rechazar running sobre una tarea bloqueada: {err}"
        );
        assert_eq!(get_task(&db, &id_b).unwrap().unwrap().state, "pending");
        // mark_running (que llama set_state(running)) también rechaza una tarea bloqueada.
        assert!(mark_running(&db, &id_b, "/tmp/wt", None).is_err());
    }

    // 047 FR-007 — predicado de selección de `stop_all_agents`: pausa (vía `pause_task`) SÓLO las
    // tareas corriendo, con pane, no ya pausadas. Acá replicamos el predicado del comando sobre
    // `list_tasks` (el SIGSTOP real del PTY se omite — `pause_task` es la persistencia del flag).
    #[test]
    fn stop_all_agents_selects_only_running_with_pane_unpaused() {
        let db = test_db();
        let (_b, tasks) = create_batch(
            &db,
            "b",
            "/tmp/r",
            None,
            None,
            &[
                TaskSpec { title: "running con pane".into(), objective: String::new(), agent_profile_id: None, mode: None },
                TaskSpec { title: "pending".into(), objective: String::new(), agent_profile_id: None, mode: None },
                TaskSpec { title: "running ya pausada".into(), objective: String::new(), agent_profile_id: None, mode: None },
            ],
        )
        .unwrap();
        let (a, b, c) = (&tasks[0].id, &tasks[1].id, &tasks[2].id);
        // a: running con pane → elegible.
        mark_running(&db, a, "/tmp/r/.wt/a", Some("pane-a")).unwrap();
        // b: queda pending → NO elegible.
        // c: running con pane PERO ya pausada → NO elegible (idempotente).
        mark_running(&db, c, "/tmp/r/.wt/c", Some("pane-c")).unwrap();
        assert!(pause_task(&db, c).unwrap()); // pre-pausada

        // Replica del predicado del comando + el efecto (pause_task) sobre cada elegible.
        let mut paused = 0usize;
        for t in list_tasks(&db, None).unwrap() {
            if t.state != "running" || t.paused_at.is_some() { continue; }
            if t.pane_id.is_none() { continue; }
            if pause_task(&db, &t.id).unwrap() { paused += 1; }
        }
        // Sólo `a` se pausa en esta corrida (b pending, c ya pausada).
        assert_eq!(paused, 1, "sólo la tarea running+pane+no-pausada debe pausarse");
        assert!(get_task(&db, a).unwrap().unwrap().paused_at.is_some());
        assert!(get_task(&db, b).unwrap().unwrap().paused_at.is_none());
        assert!(get_task(&db, c).unwrap().unwrap().paused_at.is_some()); // seguía pausada

        // Idempotencia: re-correr no pausa nada nuevo (todas las elegibles ya están pausadas).
        let mut again = 0usize;
        for t in list_tasks(&db, None).unwrap() {
            if t.state != "running" || t.paused_at.is_some() || t.pane_id.is_none() { continue; }
            if pause_task(&db, &t.id).unwrap() { again += 1; }
        }
        assert_eq!(again, 0, "una 2ª corrida no debe pausar nada (idempotente)");
    }
}
