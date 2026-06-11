// services/best_of_n.rs — 019 F1 (T011): orquestación dedicada del flujo best-of-N (FR-001/002).
//
// HOY `orchestration::create_best_of_n` crea el batch + grupo + N variantes (pending), y el LANZAMIENTO
// real de cada variante (worktree + spawn PTY) lo dispara el front variante-por-variante con
// `orchestration_prepare_task`. Este módulo ENVUELVE/ORQUESTA esos N attempts en una sola operación
// transaccional-por-attempt, con tres garantías que la spec pide:
//
//   1. Aislamiento: cada attempt corre en su worktree/branch propio (reusa `worktree::ensure` + el
//      `repo_worktree_lock` por-repo que serializa la fase `git worktree add`).
//   2. Progreso vivo: emite un evento por attempt a medida que arranca (vía event_bus, reusando
//      `AppEvent::TaskChanged` — la misma semántica que el resto del flujo, sin variante nueva).
//   3. Falla parcial sin huérfanos: si UN attempt no puede lanzarse (worktree falla, claim perdido),
//      se marca `failed` y se sigue con los demás — NUNCA tira los otros ni deja el repo a medio
//      escribir (el checkpoint-por-attempt de F0 permite el kill transaccional después).
//
// NO reimplementa nada: reusa `create_best_of_n` (modelo), `claim_for_launch`/`mark_running`
// (lifecycle), `worktree::ensure` (aislamiento), `attempt_checkpoint::register` (kill transaccional
// F0) y `agents::*` (dispatch). El SPAWN real del PTY se INYECTA como closure (`spawn`) para que el
// caller (commands.rs, con el PtyManager) lo materialice y los tests no necesiten un PTY real.

use crate::services::agents;
use crate::services::attempt_checkpoint;
use crate::services::orchestration as orch;
use crate::services::worktree;
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Estado terminal-de-launch de UNA variante. Distingue tres outcomes ortogonales (audit MED 3):
/// - `Launched`: worktree + plan listos, attempt en `running`.
/// - `Failed`: no se pudo lanzar (worktree/mark_running/spawn/plan inválido) — el attempt quedó
///   `failed` (es NUESTRO, lo marcamos nosotros). El resto del grupo sigue.
/// - `Skipped`: el claim lo ganó OTRO launcher (`claim_for_launch == Ok(false)`); NO tocamos el
///   estado del attempt (pertenece al ganador legítimo) — no es un fallo nuestro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptStatus {
    Launched,
    Failed,
    Skipped,
}

/// Resultado de lanzar UNA variante. Distingue ok (worktree + plan listos), fallo (con motivo) y
/// skip (claim perdido), sin abortar el resto del grupo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptOutcome {
    pub task_id: String,
    pub variant_index: i64,
    pub branch: String,
    pub status: AttemptStatus,
    /// `Some(worktree_path)` si el attempt arrancó aislado; `None` si falló/se saltó al lanzarse.
    pub worktree_path: Option<String>,
    /// El plan de spawn (mode/cwd/objective) si arrancó — el caller lo materializa con el PtyManager.
    pub plan: Option<agents::SpawnPlan>,
    /// `Some(motivo)` si el attempt falló o se saltó al lanzarse — el resto siguió.
    pub error: Option<String>,
}

impl AttemptOutcome {
    pub fn launched(&self) -> bool {
        self.status == AttemptStatus::Launched
    }
    /// El claim lo ganó otro launcher: ni lanzado ni fallado por nosotros (no tocamos su estado).
    pub fn skipped(&self) -> bool {
        self.status == AttemptStatus::Skipped
    }
}

/// Resultado de lanzar el grupo best-of-N completo.
#[derive(Debug, Clone)]
pub struct BestOfNLaunch {
    pub batch_id: String,
    pub group_id: String,
    pub attempts: Vec<AttemptOutcome>,
}

impl BestOfNLaunch {
    /// Cuántos attempts arrancaron aislados (≥1 esperado; 0 = todo el grupo falló al lanzarse).
    pub fn launched_count(&self) -> usize {
        self.attempts.iter().filter(|a| a.launched()).count()
    }
}

/// Callback de progreso vivo: se invoca por attempt a medida que cambia de estado (launching/running/
/// failed). El caller (commands.rs) lo cablea a `event_bus::emit_event(AppEvent::TaskChanged{..})`.
/// En tests se inyecta un recolector. Tomar `(task_id, state)` mantiene la firma agnóstica del bus.
pub type ProgressFn<'a> = dyn Fn(&str, &str) + 'a;

/// Orquesta el lanzamiento de un objetivo como N attempts best-of-N aislados.
///
/// Crea el grupo (reusa `create_best_of_n`), y POR CADA variante: claim atómico → worktree aislado
/// (serializado por repo) → checkpoint F0 → mark_running → progreso vivo → `SpawnPlan` para el caller.
/// Una variante que falle al lanzarse se marca `failed` y NO interrumpe las demás.
///
/// - `agent_kinds`: lista de `(Option<agent_profile_id>, Option<AgentKind>, Option<account_slug>)`
///   por variante. `len` = N. Permite mezclar agentes (claude+codex+gemini) o repetir uno. El
///   `agent_profile_id` se persiste en la tarea (para que el done-detection/UI lo conozca); el
///   `AgentKind`+slug arman el `SpawnPlan`.
/// - `spawn`: closure que MATERIALIZA el spawn real del PTY a partir del `SpawnPlan` (lo provee
///   commands.rs con el PtyManager). En tests es un no-op. Si el spawn falla, el attempt se marca
///   failed (sin tirar los otros).
/// - `progress`: callback de progreso vivo por attempt (event_bus en prod).
#[allow(clippy::too_many_arguments)]
pub fn launch_best_of_n(
    db: &Db,
    title: &str,
    repo_path: &str,
    base_branch: Option<&str>,
    base_commit: Option<&str>,
    objective: &str,
    agent_kinds: &[(Option<String>, Option<agents::AgentKind>, Option<String>)],
    spawn: &dyn Fn(&agents::SpawnPlan, &orch::OrchTask) -> Result<()>,
    progress: &ProgressFn<'_>,
) -> Result<BestOfNLaunch> {
    if agent_kinds.is_empty() {
        return Err(anyhow!("best-of-N necesita al menos 1 variante"));
    }
    // Reusa el modelo: crea batch + grupo + N variantes pending, cada una con su branch única. El
    // agent_profile_id por variante se persiste acá (create_best_of_n lo guarda en la tarea).
    let agents_profiles: Vec<Option<String>> =
        agent_kinds.iter().map(|(pid, _, _)| pid.clone()).collect();
    let (batch_id, group, tasks) = orch::create_best_of_n(
        db,
        title,
        repo_path,
        base_branch,
        base_commit,
        objective,
        &agents_profiles,
    )?;

    let repo = Path::new(repo_path);
    let mut attempts = Vec::with_capacity(tasks.len());

    for task in &tasks {
        let vi = task.variant_index.unwrap_or(0);
        // El (agent_profile_id, AgentKind, slug) de ESTA variante (por variant_index, que
        // create_best_of_n asignó 0..n-1 en el mismo orden que `agents_profiles`). El profile_id se
        // usa para cachear el cli_kind (MED 4); el AgentKind+slug arman el SpawnPlan.
        let (agent_profile_id, agent_kind, account_slug) = agent_kinds
            .get(vi as usize)
            .map(|(p, k, s)| (p.clone(), *k, s.clone()))
            .unwrap_or((None, None, None));

        // helper para empujar un outcome failed (attempt NUESTRO: lo marcamos failed) + progreso.
        let push_failed = |attempts: &mut Vec<AttemptOutcome>,
                           wt_path: Option<String>,
                           plan: Option<agents::SpawnPlan>,
                           reason: String| {
            let _ = orch::set_state(db, &task.id, "failed", None);
            progress(&task.id, "failed");
            attempts.push(AttemptOutcome {
                task_id: task.id.clone(),
                variant_index: vi,
                branch: task.branch.clone(),
                status: AttemptStatus::Failed,
                worktree_path: wt_path,
                plan,
                error: Some(reason),
            });
        };

        // 1) Claim atómico pending→running. Si lo perdemos (otro launcher ganó), SALTAMOS esta
        //    variante SIN tocar su estado (es del ganador legítimo) → outcome `Skipped`, NO `Failed`
        //    (audit MED 3: marcarlo failed pisaría al ganador). Un error real de claim sí es Failed.
        match orch::claim_for_launch(db, &task.id) {
            Ok(true) => {}
            Ok(false) => {
                // Claim perdido: no es un fallo nuestro. NO tocamos el estado del attempt. Emitimos
                // progreso `skipped` para visibilidad/log.
                progress(&task.id, "skipped");
                attempts.push(AttemptOutcome {
                    task_id: task.id.clone(),
                    variant_index: vi,
                    branch: task.branch.clone(),
                    status: AttemptStatus::Skipped,
                    worktree_path: None,
                    plan: None,
                    error: Some("variante ya no estaba pending (claim perdido)".into()),
                });
                continue;
            }
            Err(e) => {
                // Un error de claim SÍ es nuestro fallo. Pero el claim no tocó el estado, así que
                // intentamos marcarlo failed best-effort (puede ser no-op si seguía pending).
                push_failed(&mut attempts, None, None, format!("claim falló: {e}"));
                continue;
            }
        }

        // 2) Resolver el SpawnPlan ANTES de crear el worktree (audit HIGH 1: fail-safe). Si se pidió
        //    un AgentKind específico que no se puede resolver (claude sin cuenta / slug inválido),
        //    `spawn_in_worktree` devuelve Err → marcamos el attempt failed y seguimos. NUNCA lanzamos
        //    un shell en lugar del agente pedido. Hacerlo acá (antes del worktree) evita además crear
        //    un worktree para un attempt que jamás podrá lanzarse.
        let plan = match agents::spawn_in_worktree(
            agent_kind,
            account_slug.as_deref(),
            "", // worktree_path real se setea abajo, una vez creado.
            &task.objective,
            task.mode.as_deref().unwrap_or("zsh"),
        ) {
            Ok(p) => p,
            Err(e) => {
                push_failed(
                    &mut attempts,
                    None,
                    None,
                    format!("agente no lanzable (sin fallback a shell): {e}"),
                );
                continue;
            }
        };

        // 3) Worktree aislado. Serializa la fase `git worktree add` por repo (FR-005): N variantes
        //    del mismo repo no deben colisionar en el index del padre.
        let wt = {
            let repo_lock = orch::repo_worktree_lock(repo_path);
            let _guard = repo_lock.lock();
            worktree::ensure(repo, &task.branch)
        };
        let wt = match wt {
            Ok(w) => w,
            Err(e) => {
                // revertir el claim: running→failed (no dejar la variante colgada en running) y
                // notificar progreso. NO aborta el grupo. (No hay worktree → no hay huérfano.)
                push_failed(&mut attempts, None, None, format!("worktree falló: {e}"));
                continue;
            }
        };
        let wt_path = wt.worktree_path.clone();
        // completar el plan con el worktree real recién creado.
        let plan = agents::SpawnPlan {
            worktree_path: wt_path.clone(),
            ..plan
        };

        // 4) CHECKPOINT-POR-ATTEMPT (F0/T005) — registrar INMEDIATAMENTE tras crear el worktree y
        //    ANTES de mark_running/spawn (audit HIGH 2). Así CUALQUIER fallo posterior deja el
        //    checkpoint persistido → el kill-switch de F0 puede limpiar el worktree (no queda
        //    huérfano). Best-effort: si el rev-parse falla, el kill degrada a noop-de-worktree.
        register_checkpoint(db, &task.id, task.group_id.as_deref(), &wt_path, wt.created);

        // 5) mark_running + registrar worktree/pane. Si falla, el checkpoint YA está registrado
        //    (paso 4) → el worktree creado NO queda huérfano. Como defensa adicional, si nosotros
        //    creamos el worktree lo limpiamos explícitamente acá mismo.
        let pane_id = format!("orch-{}", task.id);
        if let Err(e) = orch::mark_running(db, &task.id, &wt_path, Some(&pane_id)) {
            // Defensa adicional al checkpoint del paso 4: si NOSOTROS creamos este worktree, lo
            // limpiamos explícitamente acá (el checkpoint queda igual, idempotente con el kill).
            if wt.created {
                remove_worktree_best_effort(repo, &wt_path);
            }
            push_failed(
                &mut attempts,
                Some(wt_path),
                None,
                format!("mark_running falló: {e}"),
            );
            continue;
        }

        // 6) Cachear el `cli_kind` por attempt (audit MED 4) — IGUAL que `orchestration_prepare_task`.
        //    `launch_best_of_n` bypassa prepare_task, y `create_best_of_n` deja `mode=NULL` para las
        //    variants con agent_profile_id → sin esto el done-detection (020) cae a `Generic`. Lo
        //    derivamos del AgentKind/profile vía la SSOT `agents::resolve_task_kind`.
        cache_cli_kind(
            db,
            &task.id,
            agent_profile_id.as_deref(),
            task.mode.as_deref(),
        );

        // 7) Spawn real vía el closure inyectado.
        if let Err(e) = spawn(&plan, task) {
            // El spawn real falló → failed transaccional (el checkpoint del paso 4 permite el kill).
            push_failed(
                &mut attempts,
                Some(wt_path),
                Some(plan),
                format!("spawn falló: {e}"),
            );
            continue;
        }

        // 8) Progreso vivo: la variante arrancó.
        progress(&task.id, "running");
        attempts.push(AttemptOutcome {
            task_id: task.id.clone(),
            variant_index: vi,
            branch: task.branch.clone(),
            status: AttemptStatus::Launched,
            worktree_path: Some(wt_path),
            plan: Some(plan),
            error: None,
        });
    }

    Ok(BestOfNLaunch {
        batch_id,
        group_id: group.id,
        attempts,
    })
}

/// Registra el checkpoint-por-attempt (F0) — reusa exactamente la lógica de `orchestration_prepare_task`:
/// el HEAD del worktree al arrancar es la base a la que el kill restaura. Best-effort.
fn register_checkpoint(
    db: &Db,
    task_id: &str,
    group_id: Option<&str>,
    worktree_path: &str,
    created_worktree: bool,
) {
    let base_commit = std::process::Command::new("git")
        .args(["-C", worktree_path, "rev-parse", "HEAD"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if !base_commit.is_empty() {
        let _ = attempt_checkpoint::register(
            db,
            task_id,
            group_id,
            worktree_path,
            &base_commit,
            created_worktree,
        );
    }
}

/// Cachea el `cli_kind` de la tarea en `orchestration_tasks.cli_kind` (audit MED 4) — exactamente lo
/// que hace `orchestration_prepare_task`, que `launch_best_of_n` bypassa. Sin esto, las variants con
/// `agent_profile_id` (que `create_best_of_n` deja con `mode=NULL`) no tienen `cli_kind` y el
/// done-detection (020) cae a `Generic`, perdiendo la detección específica por agente. Deriva el
/// `cli_kind` vía la SSOT `agents::resolve_task_kind` (profile→cli_kind, sino prefijo del mode).
/// Best-effort: un fallo de DB no debe abortar el launch (igual que en prepare_task).
fn cache_cli_kind(db: &Db, task_id: &str, agent_profile_id: Option<&str>, mode: Option<&str>) {
    let (cli_kind, _agent_kind) = agents::resolve_task_kind(agent_profile_id, mode, |pid| {
        crate::services::agent_profiles::get(db, pid).ok().flatten()
    });
    if let Some(ck) = cli_kind {
        let conn = db.lock();
        let _ = conn.execute(
            "UPDATE orchestration_tasks SET cli_kind = ?2 WHERE id = ?1",
            rusqlite::params![task_id, ck],
        );
    }
}

/// Borra un worktree best-effort (defensa adicional al checkpoint si NOSOTROS lo creamos y un paso
/// posterior falla). Mismo `git worktree remove --force` que usa el kill-switch de F0. No propaga
/// error: el checkpoint ya garantiza la limpieza vía el kill; esto sólo acelera el caso obvio.
fn remove_worktree_best_effort(repo: &Path, worktree_path: &str) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "remove", "--force", worktree_path])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_profiles::AgentProfile;
    use std::cell::RefCell;
    use uuid::Uuid;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // 006 — agent_profiles (+ agent_profile_plugins) y sus columnas engine_kind/category, para
        // que `cache_cli_kind` pueda resolver el cli_kind vía el profile (audit MED 4).
        conn.execute_batch(include_str!("../../migrations/019_agent_profiles.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/020_agent_engine_presets.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql"))
            .unwrap();
        // 019 F0 — attempt_checkpoints (para el checkpoint-por-attempt del launch).
        conn.execute_batch(include_str!("../../migrations/035_attempt_checkpoint.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/036_attempt_checkpoint_kill_token.sql"
        ))
        .unwrap();
        // 019 F3 — columna paused_at en orchestration_tasks (pause/resume). El SELECT de
        // row_to_task (orchestration) ya la pide, así que el fixture debe quedar sincronizado
        // con la migración real.
        conn.execute_batch(include_str!(
            "../../migrations/037_orch_pause_council_history.sql"
        ))
        .unwrap();
        // 038 F1.0 — pipeline_runs/pipeline_edges + columna dag_blocked (la guarda de
        // `claim_for_launch` la referencia: `AND dag_blocked=0`). Sin esto el claim rompe en el fixture.
        conn.execute_batch(include_str!("../../migrations/047_pipeline_dag.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn git(dir: &Path, args: &[&str]) {
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

    /// Crea un repo git real bajo $HOME (requisito de worktree::ensure). Devuelve (root, repo).
    fn make_repo() -> (std::path::PathBuf, std::path::PathBuf) {
        let home = dirs::home_dir().unwrap();
        let root = home.join(".furx").join("e2e-tests");
        std::fs::create_dir_all(&root).unwrap();
        let repo = root.join(format!("bon-{}", &Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@t.io"]);
        git(&repo, &["config", "user.name", "t"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        (root, repo)
    }

    fn cleanup(repo: &Path) {
        if let Ok(wts) = worktree::list_for_repo(repo) {
            for wt in wts {
                let _ = std::process::Command::new("git")
                    .current_dir(repo)
                    .args(["worktree", "remove", "--force", &wt.worktree_path])
                    .output();
            }
        }
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn launch_requires_at_least_one_variant() {
        let db = test_db();
        let noop_spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());
        let noop_prog = |_: &str, _: &str| {};
        let r = launch_best_of_n(
            &db,
            "X",
            "/tmp/r",
            None,
            None,
            "o",
            &[],
            &noop_spawn,
            &noop_prog,
        );
        assert!(r.is_err(), "0 variantes debe fallar");
    }

    #[test]
    fn lifecycle_n_attempts_start_isolated() {
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        // progreso recolectado.
        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        // spawn no-op (el PTY real no se ejercita en el test).
        let spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());

        let kinds = vec![
            (None, None, None), // variante shell/legacy
            (None, Some(agents::AgentKind::Codex), None),
            (None, Some(agents::AgentKind::Gemini), None),
        ];
        let res = launch_best_of_n(
            &db,
            "Implementá X",
            &repo_str,
            Some("main"),
            None,
            "objetivo común",
            &kinds,
            &spawn,
            &progress,
        )
        .unwrap();

        assert_eq!(res.attempts.len(), 3);
        assert_eq!(res.launched_count(), 3, "los 3 attempts arrancaron");
        // 3 worktrees REALES y DISTINTOS (aislamiento).
        let wts: std::collections::HashSet<_> = res
            .attempts
            .iter()
            .filter_map(|a| a.worktree_path.clone())
            .collect();
        assert_eq!(wts.len(), 3, "3 worktrees aislados");
        for wt in &wts {
            assert!(Path::new(wt).exists(), "worktree existe: {}", wt);
        }
        // cada tarea quedó running + con su worktree registrado.
        for a in &res.attempts {
            let t = orch::get_task(&db, &a.task_id).unwrap().unwrap();
            assert_eq!(t.state, "running");
            assert_eq!(t.worktree_path.as_deref(), a.worktree_path.as_deref());
            // checkpoint registrado (kill transaccional listo).
            assert!(
                attempt_checkpoint::get(&db, &a.task_id).unwrap().is_some(),
                "checkpoint-por-attempt registrado"
            );
        }
        // el dispatch enrutó el mode correcto por variante.
        let by_idx: std::collections::HashMap<i64, &AttemptOutcome> =
            res.attempts.iter().map(|a| (a.variant_index, a)).collect();
        assert_eq!(by_idx[&0].plan.as_ref().unwrap().mode, "zsh");
        assert_eq!(by_idx[&1].plan.as_ref().unwrap().mode, "codex");
        assert_eq!(by_idx[&2].plan.as_ref().unwrap().mode, "gemini");
        // progreso vivo: un "running" por attempt.
        let running = events
            .borrow()
            .iter()
            .filter(|(_, s)| s == "running")
            .count();
        assert_eq!(running, 3, "3 eventos de progreso 'running' emitidos");

        cleanup(&repo);
    }

    #[test]
    fn partial_failure_does_not_take_down_others() {
        // Un attempt cuyo SPAWN falla se marca failed; los otros siguen vivos (sin huérfanos).
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        // spawn que FALLA SOLO para la variante de índice 1.
        let spawn = |plan: &agents::SpawnPlan, task: &orch::OrchTask| -> Result<()> {
            let _ = plan;
            if task.variant_index == Some(1) {
                Err(anyhow!("spawn simulado falla para v1"))
            } else {
                Ok(())
            }
        };

        let kinds = vec![(None, None, None), (None, None, None), (None, None, None)];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        assert_eq!(res.attempts.len(), 3);
        // v1 falló; v0 y v2 arrancaron.
        assert_eq!(res.launched_count(), 2, "2 de 3 arrancaron");
        let v1 = res.attempts.iter().find(|a| a.variant_index == 1).unwrap();
        assert!(!v1.launched());
        assert!(v1.error.is_some());
        assert_eq!(
            orch::get_task(&db, &v1.task_id).unwrap().unwrap().state,
            "failed",
            "la variante que falló quedó failed (no colgada en running)"
        );
        // HIGH 2: aunque el spawn falló DESPUÉS de crear el worktree, el checkpoint quedó registrado
        // (paso 4, antes de mark_running/spawn) → el kill-switch F0 puede limpiar ese worktree.
        assert!(
            attempt_checkpoint::get(&db, &v1.task_id).unwrap().is_some(),
            "el attempt con worktree creado tiene checkpoint (no huérfano)"
        );
        // los otros dos siguen running (no se tiraron).
        for a in res.attempts.iter().filter(|a| a.variant_index != 1) {
            assert_eq!(
                orch::get_task(&db, &a.task_id).unwrap().unwrap().state,
                "running"
            );
        }
        // progreso: 2 running + 1 failed.
        let ev = events.borrow();
        assert_eq!(ev.iter().filter(|(_, s)| s == "running").count(), 2);
        assert_eq!(ev.iter().filter(|(_, s)| s == "failed").count(), 1);

        cleanup(&repo);
    }

    #[test]
    fn progress_is_emitted_per_attempt() {
        // Verifica que cada attempt produce exactamente un evento de progreso terminal-de-launch
        // (running|failed), y que se referencia al task_id correcto.
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();
        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        let spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());

        let kinds = vec![(None, None, None), (None, None, None)];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        let ev = events.borrow();
        // un evento por attempt.
        assert_eq!(ev.len(), res.attempts.len());
        // cada task_id de un attempt lanzado aparece en el progreso.
        for a in &res.attempts {
            assert!(
                ev.iter().any(|(id, _)| id == &a.task_id),
                "progreso referencia el task_id {}",
                a.task_id
            );
        }
        cleanup(&repo);
    }

    #[test]
    fn spawn_fail_safe_claude_without_account_marks_failed_no_zsh() {
        // HIGH 1: una variante Claude SIN cuenta no se puede resolver a un mode → se marca failed (NO
        // se lanza un shell zsh en su lugar). Las otras variantes siguen vivas.
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        // spawn que registra qué modes se materializaron (para probar que NUNCA se lanza un zsh
        // por la variante Claude rota).
        let spawned_modes: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let spawn = |plan: &agents::SpawnPlan, _: &orch::OrchTask| -> Result<()> {
            spawned_modes.borrow_mut().push(plan.mode.clone());
            Ok(())
        };

        // v0: codex legacy (ok). v1: CLAUDE sin account_slug (no lanzable). v2: gemini (ok).
        let kinds = vec![
            (None, Some(agents::AgentKind::Codex), None),
            (None, Some(agents::AgentKind::ClaudeCode), None),
            (None, Some(agents::AgentKind::Gemini), None),
        ];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        assert_eq!(res.attempts.len(), 3);
        assert_eq!(
            res.launched_count(),
            2,
            "v0 y v2 arrancaron; v1 (claude) no"
        );
        let v1 = res.attempts.iter().find(|a| a.variant_index == 1).unwrap();
        assert!(!v1.launched());
        assert_eq!(v1.status, AttemptStatus::Failed);
        assert!(
            v1.worktree_path.is_none(),
            "v1 falló ANTES de crear worktree (no huérfano)"
        );
        assert_eq!(
            orch::get_task(&db, &v1.task_id).unwrap().unwrap().state,
            "failed"
        );
        // CRÍTICO: ningún spawn fue con mode "zsh" (no se lanzó un shell por la variante Claude rota).
        assert!(
            !spawned_modes.borrow().iter().any(|m| m == "zsh"),
            "NUNCA se lanza zsh por un agente que no resuelve: {:?}",
            spawned_modes.borrow()
        );
        // los modes que SÍ se lanzaron son los de los agentes válidos.
        let modes = spawned_modes.borrow();
        assert!(modes.contains(&"codex".to_string()));
        assert!(modes.contains(&"gemini".to_string()));

        cleanup(&repo);
    }

    #[test]
    fn orphan_prevented_when_mark_running_fails() {
        // HIGH 2: si mark_running falla DESPUÉS de crear el worktree, el checkpoint YA está registrado
        // → un kill posterior limpia el worktree (no queda huérfano sin checkpoint).
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        // Forzar el fallo de mark_running: un trigger que aborta el 2º UPDATE (el que setea
        // worktree_path). claim_for_launch ya dejó state='running', así que el set_state interno es
        // idempotente y NO dispara este trigger; sólo lo dispara el UPDATE de worktree_path.
        {
            let conn = db.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_set_worktree BEFORE UPDATE OF worktree_path \
                 ON orchestration_tasks BEGIN \
                 SELECT RAISE(ABORT, 'mark_running boom'); END;",
            )
            .unwrap();
        }

        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        let spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());

        let kinds = vec![(None, None, None)];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        let a = &res.attempts[0];
        assert_eq!(a.status, AttemptStatus::Failed);
        assert!(a.error.as_ref().unwrap().contains("mark_running"));
        // El checkpoint quedó registrado (paso 4, antes de mark_running) → kill puede limpiar.
        let ckpt = attempt_checkpoint::get(&db, &a.task_id).unwrap();
        assert!(
            ckpt.is_some(),
            "checkpoint registrado pese a fallar mark_running (no huérfano)"
        );
        let ckpt = ckpt.unwrap();
        let wt_path = ckpt.worktree_path.clone();
        // Quitar el trigger para que el kill pueda operar sobre la fila.
        {
            let conn = db.lock();
            conn.execute_batch("DROP TRIGGER fail_set_worktree;")
                .unwrap();
        }
        // Simular el kill-switch de F0: limpia el worktree creado vía el checkpoint.
        let outcome = attempt_checkpoint::kill_attempt(&db, &a.task_id).unwrap();
        let _ = outcome;
        assert!(
            !Path::new(&wt_path).exists(),
            "el kill limpió el worktree creado ({}): no queda colgado",
            wt_path
        );

        cleanup(&repo);
    }

    #[test]
    fn lost_claim_is_skipped_not_failed() {
        // MED 3: si OTRO launcher ya ganó el claim (la variante no está pending), el outcome es
        // `Skipped` y NO tocamos el estado del attempt (es del ganador legítimo).
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        // Pre-crear el grupo y robar el claim de la variante 0 ANTES de launch... pero launch crea el
        // grupo. En su lugar, simulamos el claim perdido marcando la variante running con un trigger:
        // hacemos que claim_for_launch falle (0 filas) pre-seteando state. Para eso interceptamos vía
        // un trigger que cambia el state de pending→running en el INSERT inicial de la tarea.
        {
            let conn = db.lock();
            conn.execute_batch(
                "CREATE TRIGGER steal_claim AFTER INSERT ON orchestration_tasks \
                 WHEN NEW.variant_index = 0 BEGIN \
                 UPDATE orchestration_tasks SET state='running' WHERE id = NEW.id; END;",
            )
            .unwrap();
        }

        let events: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let progress = |id: &str, st: &str| events.borrow_mut().push((id.into(), st.into()));
        let spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());

        let kinds = vec![(None, None, None), (None, None, None)];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        let v0 = res.attempts.iter().find(|a| a.variant_index == 0).unwrap();
        assert_eq!(v0.status, AttemptStatus::Skipped, "claim perdido → skipped");
        assert!(v0.skipped());
        assert!(!v0.launched());
        // NO lo marcamos failed: su state lo dejó el ganador (running), intacto.
        assert_eq!(
            orch::get_task(&db, &v0.task_id).unwrap().unwrap().state,
            "running",
            "el estado del attempt NO fue pisado por el launcher que perdió el claim"
        );
        // sin checkpoint nuestro (no creamos worktree para una variante que no clamamos).
        assert!(attempt_checkpoint::get(&db, &v0.task_id).unwrap().is_none());
        // progreso: un evento 'skipped' (no 'failed') para v0.
        let ev = events.borrow();
        assert!(ev.iter().any(|(id, s)| id == &v0.task_id && s == "skipped"));
        assert!(!ev.iter().any(|(id, s)| id == &v0.task_id && s == "failed"));
        // v1 sí arrancó normal.
        let v1 = res.attempts.iter().find(|a| a.variant_index == 1).unwrap();
        assert!(v1.launched());

        cleanup(&repo);
    }

    #[test]
    fn cli_kind_persisted_per_agent() {
        // MED 4: launch cachea el cli_kind por attempt (derivado del agent_profile) para que el
        // done-detection (020) no caiga a Generic. create_best_of_n deja mode=NULL para variants con
        // agent_profile_id → sin el cache, cli_kind quedaría NULL.
        let (_root, repo) = make_repo();
        let db = test_db();
        let repo_str = repo.to_str().unwrap().to_string();

        // profiles reales en DB (uno codex, uno gemini).
        let codex_pid =
            crate::services::agent_profiles::create(&db, profile_named("codex-prof", "codex"))
                .unwrap()
                .id;
        let gemini_pid =
            crate::services::agent_profiles::create(&db, profile_named("gemini-prof", "gemini"))
                .unwrap()
                .id;

        let progress = |_: &str, _: &str| {};
        let spawn = |_: &agents::SpawnPlan, _: &orch::OrchTask| Ok(());

        let kinds = vec![
            (
                Some(codex_pid.clone()),
                Some(agents::AgentKind::Codex),
                None,
            ),
            (
                Some(gemini_pid.clone()),
                Some(agents::AgentKind::Gemini),
                None,
            ),
        ];
        let res = launch_best_of_n(
            &db, "X", &repo_str, None, None, "o", &kinds, &spawn, &progress,
        )
        .unwrap();

        let v0 = res.attempts.iter().find(|a| a.variant_index == 0).unwrap();
        let v1 = res.attempts.iter().find(|a| a.variant_index == 1).unwrap();
        let t0 = orch::get_task(&db, &v0.task_id).unwrap().unwrap();
        let t1 = orch::get_task(&db, &v1.task_id).unwrap().unwrap();
        assert_eq!(
            t0.cli_kind.as_deref(),
            Some("codex"),
            "cli_kind cacheado por agente (codex)"
        );
        assert_eq!(
            t1.cli_kind.as_deref(),
            Some("gemini"),
            "cli_kind cacheado por agente (gemini)"
        );

        cleanup(&repo);
    }

    fn profile_named(name: &str, cli_kind: &str) -> AgentProfile {
        AgentProfile {
            id: String::new(),
            name: name.into(),
            description: String::new(),
            cli_kind: cli_kind.into(),
            account_slug: None,
            model: None,
            system_prompt: String::new(),
            default_cwd: None,
            council_enabled: false,
            council_preset: None,
            shell_enabled: false,
            icon: None,
            color: None,
            is_builtin: false,
            engine_kind: "cli".into(),
            category: None,
            plugins: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}
