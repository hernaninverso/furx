// services/attempt_checkpoint.rs — 019 F0 · T005 — kill-switch transaccional con
// checkpoint-por-attempt (R4 / FR-006).
//
// Protocolo (council R4 + audit-3 codex/deepseek H1/H2/H3):
//   1. Al EMPEZAR un attempt (variante del best-of-N) se registra un checkpoint = el HEAD del
//      worktree del que parte + el worktree path + si el worktree fue CREADO por el attempt.
//   2. Un kill posterior usa ese checkpoint para abortar TRANSACCIONALMENTE:
//        - si el attempt creó el worktree → se DESCARTA entero (`git worktree remove --force`),
//          sin dejar archivos a medio escribir.
//        - si reusó un worktree existente → se RESTAURA al HEAD del checkpoint (`git reset --hard`
//          + `git clean -fdq`), descartando los cambios sin commitear del attempt.
//   3. NUNCA mata procesos ajenos (el kill del PTY lo rutea el process-registry desde
//      `orchestration_cancel`); este módulo se ocupa SÓLO del estado del worktree.
//   4. El kill es TRANSACCIONAL + IDEMPOTENTE + RE-INTENTABLE (audit-3 H3): el git corre PRIMERO
//      (estado intermedio `killing`, un solo ganador del claim); SÓLO si el worktree quedó limpio
//      se marca `killed`. Si el git falla, el checkpoint vuelve a `open` → un re-kill REINTENTA (no
//      queda zombie DB=killed/worktree-sucio). Re-killear un checkpoint ya `killed` es noop. Un
//      `killing` huérfano (kill que murió a mitad) es re-clamable tras `STALE_KILLING_SECS`.
//   4b. SERIALIZACIÓN DEL GIT (audit r3 HIGH): el DB-claim resuelve el ganador en estado estable, pero
//      el stale-reclaim (300s) asume que el killer original MURIÓ. Si seguía VIVO ejecutando su git
//      (lento/colgado), dos killers correrían `worktree remove`/`reset --hard` DESTRUCTIVOS a la vez
//      sobre el MISMO worktree → corrupción. Para cerrar esa ventana, el git del kill corre bajo un
//      ADVISORY FILE LOCK exclusivo de OS (`flock(LOCK_EX)`) ligado al worktree (archivo en
//      `~/.furx/locks/kill-<hash>.lock`, FUERA del worktree que se borra). Dos killers NUNCA corren
//      git concurrente sobre el mismo path: el 2º ve el lock tomado → aborta limpio (revierte su claim
//      → re-intentable). El OS libera el lock si el proceso tenedor muere → sin deadlock por zombie.
//   5. OWNERSHIP (audit-3 H1): antes de `worktree remove --force <path>` se valida que `<path>` es
//      EXACTAMENTE el worktree del checkpoint, es un worktree git válido (tiene `.git`) y está
//      REGISTRADO en su repo padre. Nunca se hace `remove --force` sobre un path arbitrario.
//   6. AISLAMIENTO (audit-3 H2): en best-of-N (spec 019) cada attempt corre en su PROPIO worktree
//      aislado — el path es `<repo>-<branch>` (ver `worktree::ensure`) y cada attempt usa una branch
//      distinta, así que el `git reset --hard` del restore SÓLO afecta el worktree de ESTE attempt,
//      nunca el de otro. El guard de ownership (mismo path del checkpoint + worktree registrado) lo
//      verifica antes de tocar el árbol. Ver `restore_worktree`.
//
// El git real corre acá (no es puro), pero la construcción de los argv y la lógica de decisión
// (descartar vs restaurar) están en helpers PUROS testeables (`KillPlan`).

use anyhow::{anyhow, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Un `killing` más viejo que esto se considera HUÉRFANO (el kill que lo reclamó murió a mitad) y
/// puede ser re-clamado por un nuevo kill. Generoso: el git de un kill tarda ms, no minutos.
const STALE_KILLING_SECS: i64 = 300;

/// Un checkpoint registrado de un attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub task_id: String,
    pub group_id: Option<String>,
    pub worktree_path: String,
    pub base_commit: String,
    pub created_worktree: bool,
    pub status: String,
}

/// Registra el checkpoint de partida de un attempt. Idempotente por `task_id`: si ya existe, NO
/// pisa el punto de partida (el attempt sólo registra su base UNA vez). `created_worktree` = true si
/// `worktree::ensure` CREÓ el worktree para este attempt (el kill podrá descartarlo).
/// `base_commit` = el HEAD del worktree al arrancar (la base a la que el kill restaura).
pub fn register(
    db: &Db,
    task_id: &str,
    group_id: Option<&str>,
    worktree_path: &str,
    base_commit: &str,
    created_worktree: bool,
) -> Result<()> {
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO attempt_checkpoints \
         (task_id, group_id, worktree_path, base_commit, created_worktree, status) \
         VALUES (?1,?2,?3,?4,?5,'open')",
        params![
            task_id,
            group_id,
            worktree_path,
            base_commit,
            created_worktree as i64,
        ],
    )?;
    Ok(())
}

/// Lee el checkpoint de un attempt (None si no se registró).
pub fn get(db: &Db, task_id: &str) -> Result<Option<Checkpoint>> {
    let conn = db.lock();
    let row = conn
        .query_row(
            "SELECT task_id, group_id, worktree_path, base_commit, created_worktree, status \
             FROM attempt_checkpoints WHERE task_id = ?1",
            params![task_id],
            |r| {
                Ok(Checkpoint {
                    task_id: r.get(0)?,
                    group_id: r.get(1)?,
                    worktree_path: r.get(2)?,
                    base_commit: r.get(3)?,
                    created_worktree: r.get::<_, i64>(4)? != 0,
                    status: r.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Resultado de un kill: qué hizo (para audit/UI). Distingue el caso ya-consumido (idempotencia).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillOutcome {
    /// Se descartó el worktree entero (lo había creado el attempt).
    WorktreeDiscarded { worktree_path: String },
    /// Se restauró el worktree al HEAD del checkpoint (reuso de worktree existente).
    WorktreeRestored {
        worktree_path: String,
        base_commit: String,
    },
    /// El checkpoint ya estaba `killed` → noop idempotente.
    AlreadyKilled,
    /// Otro kill ya está ejecutando el git (estado `killing` fresco) → este llamado no re-ejecuta.
    KillInFlight,
    /// No había checkpoint registrado para el attempt → nada que abortar a nivel worktree.
    NoCheckpoint,
}

/// Plan de kill PURO: dado el checkpoint, ¿descartar el worktree o restaurarlo? Sin efectos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillPlan {
    Discard,
    Restore { base_commit: String },
}

/// Decide el plan a partir del checkpoint (PURO): si el attempt creó el worktree → descartar; si lo
/// reusó → restaurar al base del checkpoint. La lógica de decisión está separada del git real para
/// testearla sin tocar el filesystem.
pub fn plan_for(ckpt: &Checkpoint) -> KillPlan {
    if ckpt.created_worktree {
        KillPlan::Discard
    } else {
        KillPlan::Restore {
            base_commit: ckpt.base_commit.clone(),
        }
    }
}

/// Resultado de intentar reclamar un kill (claim atómico).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Claim {
    /// ESTE llamado ganó el claim (open→killing): es el único autorizado a ejecutar el git. Lleva el
    /// FENCING TOKEN único de este claim (audit r2): `mark_killed`/`release_killing` deben presentarlo
    /// para no pisar un claim ajeno que reclamó tras el stale.
    Won { token: String },
    /// Ya hay un kill in-flight (otro `killing` fresco) → este llamado no ejecuta git.
    InFlight,
    /// Ya estaba `killed` → noop idempotente.
    AlreadyKilled,
}

/// Resultado de cerrar/revertir un claim (`mark_killed`/`release_killing`): distingue el caso en que
/// el fencing token YA NO es nuestro (otro killer reclamó tras el stale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FenceResult {
    /// El UPDATE afectó nuestra fila (el token coincide): la transición se aplicó.
    Applied,
    /// 0 filas afectadas: el claim ya no es nuestro (otro killer lo reclamó tras el stale). NO se debe
    /// reportar éxito ni asumir que la DB quedó en el estado pedido.
    LostClaim,
}

/// Genera un fencing token único para un claim del kill (audit r2). UUID v4 (la dep ya está en el
/// crate) → único entre killers concurrentes y entre reintentos del mismo killer.
fn new_kill_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Reclama el kill ATÓMICAMENTE: transiciona `open`→`killing` (o re-clama un `killing` HUÉRFANO,
/// más viejo que `STALE_KILLING_SECS`), grabando un FENCING TOKEN nuevo y único en la fila. Exactamente
/// UN llamado gana ante kills concurrentes (el UPDATE condicionado por status es atómico bajo el mutex
/// de la conexión). El stale-reclaim SOBRESCRIBE el token viejo → el killer huérfano, al intentar
/// cerrar/liberar con su token viejo, afectará 0 filas y NO pisará el claim del nuevo (audit r2). NO
/// marca `killed` acá: `killed` se setea SÓLO tras el git OK (audit-3 H3, kill re-intentable).
fn claim_killing(db: &Db, task_id: &str) -> Result<Claim> {
    let conn = db.lock();
    let token = new_kill_token();
    // Gana si está `open`, o si está `killing` pero huérfano (el kill previo murió a mitad). En ambos
    // casos graba el token NUEVO (sobrescribe el viejo en el stale-reclaim).
    let n = conn.execute(
        "UPDATE attempt_checkpoints \
         SET status = 'killing', killing_at = datetime('now'), kill_token = ?3 \
         WHERE task_id = ?1 AND ( \
             status = 'open' \
             OR (status = 'killing' \
                 AND (killing_at IS NULL \
                      OR killing_at <= datetime('now', ?2))) \
         )",
        params![task_id, format!("-{STALE_KILLING_SECS} seconds"), token],
    )?;
    if n == 1 {
        return Ok(Claim::Won { token });
    }
    // No ganamos: distinguir killed (noop) de killing-fresco (in-flight).
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM attempt_checkpoints WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("killed") => Ok(Claim::AlreadyKilled),
        _ => Ok(Claim::InFlight),
    }
}

/// Cierra el kill: `killing`→`killed` (el git terminó OK y el worktree quedó limpio). Sólo el ganador
/// del claim llama esto, PRESENTANDO su fencing token. Si el token ya no coincide (otro killer reclamó
/// tras el stale) el UPDATE afecta 0 filas → `LostClaim`: el caller NO debe reportar el kill como
/// completado (audit r2).
fn mark_killed(db: &Db, task_id: &str, token: &str) -> Result<FenceResult> {
    let conn = db.lock();
    let n = conn.execute(
        "UPDATE attempt_checkpoints SET status = 'killed', killed_at = datetime('now'), \
         killing_at = NULL WHERE task_id = ?1 AND status = 'killing' AND kill_token = ?2",
        params![task_id, token],
    )?;
    Ok(if n == 1 {
        FenceResult::Applied
    } else {
        FenceResult::LostClaim
    })
}

/// Revalida bajo DB que el claim del kill SIGUE siendo nuestro JUSTO ANTES de correr el git destructivo
/// (audit r4 HIGH). Cierra la ventana entre el DB-claim y la toma del flock: si ESTE killer ganó el
/// claim (token tA) y se PAUSÓ antes de tomar el lock, otro killer pudo hacer stale-reclaim (token tB),
/// tomar el lock, correr su git, marcar `killed` y SOLTAR el lock — todo mientras estábamos dormidos.
/// Al despertar tomaríamos el flock YA LIBRE y correríamos git destructivo aunque tA ya NO es el dueño.
/// El flock serializa (no hay git concurrente) pero NO impide que un killer que perdió la autoridad
/// corra git. Por eso, con el lock ya tomado, revalidamos: la fila debe seguir `status='killing' AND
/// kill_token = <nuestro token>`. Devuelve `Applied` si seguimos siendo el dueño (proceder con git),
/// `LostClaim` si no (otro killer reclamó/completó tras el stale → NO tocar git).
fn still_owner(db: &Db, task_id: &str, token: &str) -> Result<FenceResult> {
    let conn = db.lock();
    let current: Option<String> = conn
        .query_row(
            "SELECT kill_token FROM attempt_checkpoints \
             WHERE task_id = ?1 AND status = 'killing' AND kill_token = ?2",
            params![task_id, token],
            |r| r.get(0),
        )
        .optional()?;
    Ok(if current.is_some() {
        FenceResult::Applied
    } else {
        FenceResult::LostClaim
    })
}

/// Revierte el claim: `killing`→`open` (el git FALLÓ → el checkpoint queda RE-INTENTABLE, no zombie).
/// Sólo el ganador del claim llama esto, en el path de error del git, PRESENTANDO su fencing token. Si
/// el token ya no coincide (otro killer reclamó tras el stale) el UPDATE afecta 0 filas → `LostClaim`:
/// NO se libera el claim del nuevo dueño (audit r2).
fn release_killing(db: &Db, task_id: &str, token: &str) -> Result<FenceResult> {
    let conn = db.lock();
    let n = conn.execute(
        "UPDATE attempt_checkpoints SET status = 'open', killing_at = NULL, kill_token = NULL \
         WHERE task_id = ?1 AND status = 'killing' AND kill_token = ?2",
        params![task_id, token],
    )?;
    Ok(if n == 1 {
        FenceResult::Applied
    } else {
        FenceResult::LostClaim
    })
}

/// Guard RAII sobre el lock de OS que SERIALIZA el git destructivo del kill de un worktree
/// (audit r3 HIGH). Mantiene abierto el `File` con el `flock(LOCK_EX)` tomado; al dropearse libera
/// el lock. El OS además libera el lock automáticamente si el proceso TENEDOR muere → un killer
/// colgado/muerto NUNCA deja el lock tomado para siempre (no hay deadlock por killer zombie). El
/// archivo del lock vive FUERA del worktree (en `~/.furx/locks/`), así que `discard_worktree` puede
/// borrar el worktree entero sin tocar el lock.
struct WorktreeKillLock {
    // El `File` debe seguir vivo: el flock se libera al cerrar el descriptor. `_file` no se lee, sólo
    // se retiene. El lock se libera explícitamente en Drop (best-effort) y por el cierre del fd.
    _file: File,
}

impl Drop for WorktreeKillLock {
    fn drop(&mut self) {
        // Best-effort: liberar explícitamente. Aunque falle, cerrar el fd (al dropear `_file`)
        // libera el flock igual.
        let _ = FileExt::unlock(&self._file);
    }
}

/// Resultado de intentar tomar el lock de OS del worktree (TRY, no bloqueante).
enum LockTry {
    /// Tomamos el lock exclusivo: somos el único killer ejecutando git sobre este worktree.
    Acquired(WorktreeKillLock),
    /// Otro killer ya tiene el lock (su git destructivo está corriendo) → NO ejecutamos git
    /// concurrente sobre el mismo worktree. El caller debe abortar limpio (revertir su DB-claim).
    Held,
}

/// Deriva el PATH del archivo de lock para un worktree. CRÍTICO (audit r3): el lock NO puede vivir
/// dentro del worktree que `discard_worktree` borra — vive en `~/.furx/locks/`, un dir estable. El
/// nombre es un hash blake3 del path CANONICALIZADO del worktree (estable entre killers que apuntan
/// al mismo worktree, aunque lo pasen con `..`/symlinks distintos). Sin home (entorno raro) cae a un
/// dir temporal del sistema — sigue siendo un path estable derivado del mismo hash.
fn kill_lock_path(worktree_path: &str) -> PathBuf {
    let wt = Path::new(worktree_path);
    let canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
    let hash = blake3::hash(canon.to_string_lossy().as_bytes()).to_hex();
    let locks_dir = dirs::home_dir()
        .map(|h| h.join(".furx").join("locks"))
        .unwrap_or_else(std::env::temp_dir);
    locks_dir.join(format!("kill-{hash}.lock"))
}

/// Toma (TRY) el lock de OS exclusivo que serializa el git del kill sobre `worktree_path`. NO bloquea:
/// si otro killer lo tiene, devuelve `Held` para que el caller aborte limpio en vez de correr git
/// concurrente sobre el mismo worktree (audit r3 HIGH). El `flock` se libera al dropear el guard o al
/// morir el proceso.
fn try_acquire_kill_lock(worktree_path: &str) -> Result<LockTry> {
    let path = kill_lock_path(worktree_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow!(
                "no se pudo crear el dir del kill-lock {}: {e}",
                parent.display()
            )
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| anyhow!("no se pudo abrir el kill-lock {}: {e}", path.display()))?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(LockTry::Acquired(WorktreeKillLock { _file: file })),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(LockTry::Held),
        Err(e) => Err(anyhow!(
            "error tomando el kill-lock {}: {e}",
            path.display()
        )),
    }
}

/// Aborta TRANSACCIONALMENTE el worktree de un attempt usando su checkpoint. Idempotente,
/// concurrencia-safe y RE-INTENTABLE (audit-3 H3):
///   - `claim_killing` garantiza UN solo ganador (open→killing); los demás → `AlreadyKilled` o
///     `KillInFlight`.
///   - El git corre PRIMERO. SÓLO si el git termina OK se marca `killed`. Si el git FALLA, el
///     checkpoint vuelve a `open` (re-intentable) y se propaga el error — nunca queda
///     DB=killed/worktree-sucio.
/// NUNCA mata procesos ajenos (eso lo rutea el caller por el process-registry).
///
/// `created_worktree` → `git worktree remove --force` desde el repo padre (con guard de ownership,
/// audit-3 H1); reuso → `git reset --hard <base>` + `git clean -fdq` en el worktree aislado del
/// attempt (audit-3 H2).
pub fn kill_attempt(db: &Db, task_id: &str) -> Result<KillOutcome> {
    let Some(ckpt) = get(db, task_id)? else {
        return Ok(KillOutcome::NoCheckpoint);
    };
    // Claim atómico: exactamente un kill ejecuta el git; los demás → AlreadyKilled / KillInFlight. El
    // claim ganador lleva un FENCING TOKEN único (audit r2) que se presenta para cerrar/liberar.
    let token = match claim_killing(db, task_id)? {
        Claim::Won { token } => token,
        Claim::AlreadyKilled => return Ok(KillOutcome::AlreadyKilled),
        Claim::InFlight => return Ok(KillOutcome::KillInFlight),
    };
    // Lock de OS que SERIALIZA el git destructivo sobre el worktree (audit r3 HIGH). El DB-claim ya
    // garantiza un único ganador en estado estable, pero un `killing` HUÉRFANO se re-clama tras el
    // stale (300s) ASUMIENDO que el killer original murió. Si ese killer original sigue VIVO y todavía
    // ejecutando su `git worktree remove`/`reset --hard` (lento/colgado), el re-claim dispararía un 2º
    // git DESTRUCTIVO concurrente sobre el MISMO worktree → corrupción. El flock impide ese solape: el
    // killer original SIGUE teniendo el lock mientras corre su git, así que el re-clamador no puede
    // tomarlo y aborta limpio (revierte su claim → re-intentable). El lock se mantiene vivo (`_lock`)
    // durante TODO el git de ESTE killer y se libera al dropear el guard (o si este proceso muere).
    let _lock = match try_acquire_kill_lock(&ckpt.worktree_path)? {
        LockTry::Acquired(g) => g,
        LockTry::Held => {
            // Otro killer está corriendo git sobre este worktree AHORA MISMO. No corremos git
            // concurrente: revertimos nuestro DB-claim a `open` (re-intentable) y reportamos in-flight.
            // El otro killer (que tiene el lock) terminará y marcará el estado; si murió, el OS libera
            // el lock y un re-kill posterior lo tomará.
            match release_killing(db, task_id, &token) {
                Ok(FenceResult::Applied) | Ok(FenceResult::LostClaim) => {}
                Err(re) => {
                    return Err(anyhow!(
                        "no se pudo tomar el lock del worktree (otro kill corre su git) y además \
                         no se pudo revertir el claim a 'open' ({re})"
                    ));
                }
            }
            return Ok(KillOutcome::KillInFlight);
        }
    };
    // REVALIDACIÓN BAJO DB (audit r4 HIGH): con el flock YA tomado y ANTES de tocar git, confirmar que
    // el claim sigue siendo NUESTRO (status='killing' AND kill_token = nuestro token). Cierra la ventana
    // entre ganar el claim y tomar el lock: si nos pausamos ahí, otro killer pudo hacer stale-reclaim
    // (token nuevo), tomar el lock, correr su git, marcar `killed` y SOLTAR el lock — y al despertar
    // tomaríamos el flock libre y correríamos git destructivo sin ser ya el dueño. Si perdimos la
    // autoridad → NO tocamos git: soltamos el lock (RAII al return) y devolvemos KillInFlight (el nuevo
    // dueño ya hizo / hará el kill; es re-intentable por él).
    match still_owner(db, task_id, &token)? {
        FenceResult::Applied => {}
        FenceResult::LostClaim => {
            // Otro killer reclamó (y quizá ya completó) el kill mientras estábamos pausados entre el
            // claim y el lock. NO corremos git: el `_lock` se libera al retornar (RAII). No revertimos
            // nada — la fila pertenece ahora al nuevo dueño (su token), y release_killing con NUESTRO
            // token sería noop igual.
            return Ok(KillOutcome::KillInFlight);
        }
    }
    // Ejecutar el git PRIMERO. Si falla, revertir el claim a `open` (re-intentable) y propagar.
    let plan = plan_for(&ckpt);
    let git_result = match &plan {
        KillPlan::Discard => discard_worktree(&ckpt.worktree_path),
        KillPlan::Restore { base_commit } => restore_worktree(&ckpt.worktree_path, base_commit),
    };
    if let Err(e) = git_result {
        // El git falló → NO marcar killed; dejar el checkpoint re-intentable (si AÚN somos el dueño).
        match release_killing(db, task_id, &token) {
            Ok(FenceResult::Applied) => {}
            Ok(FenceResult::LostClaim) => {
                // Otro killer reclamó nuestro claim tras el stale (mientras corría nuestro git). NO
                // tocamos su claim; abortamos limpio — el nuevo dueño se encarga del kill.
                return Err(anyhow!(
                    "kill del worktree falló ({e}) y además perdimos el claim (otro kill lo reclamó \
                     tras el stale) — el nuevo dueño reintenta; abortamos sin revertir su claim"
                ));
            }
            Err(re) => {
                return Err(anyhow!(
                    "kill del worktree falló ({e}) y además no se pudo revertir el claim a 'open' ({re}) \
                     — el checkpoint puede quedar en 'killing' hasta el timeout"
                ));
            }
        }
        return Err(anyhow!("kill del worktree falló (re-intentable): {e}"));
    }
    // Git OK → cerrar el kill (killing→killed) presentando nuestro fencing token. Si perdimos el claim
    // (otro killer reclamó tras el stale durante nuestro git) → 0 filas → NO reportamos éxito: el nuevo
    // dueño es quien manda y reejecutará su propio kill (audit r2).
    if mark_killed(db, task_id, &token)? == FenceResult::LostClaim {
        return Err(anyhow!(
            "perdimos el claim del kill (otro kill lo reclamó tras el stale): el git de este killer \
             corrió pero el checkpoint NO se marca killed — el nuevo dueño es la autoridad"
        ));
    }
    Ok(match plan {
        KillPlan::Discard => KillOutcome::WorktreeDiscarded {
            worktree_path: ckpt.worktree_path,
        },
        KillPlan::Restore { base_commit } => KillOutcome::WorktreeRestored {
            worktree_path: ckpt.worktree_path,
            base_commit,
        },
    })
}

/// Valida que `<path>` es un worktree git VÁLIDO y REGISTRADO en su repo padre, y devuelve el
/// repo-root (padre del git-common-dir) para correr `git worktree remove` desde ahí. Guard de
/// ownership (audit-3 H1/H2): garantiza que NO operamos sobre un path arbitrario/no-worktree.
///   - `<path>/.git` debe existir (un worktree linkeado tiene un FILE `.git` con `gitdir: ...`; el
///     repo principal tiene un DIR `.git`). En ambos casos `.git` existe.
///   - `<path>` debe aparecer EXACTAMENTE en `git worktree list --porcelain` del repo (canonicalizado
///     a ambos lados para evitar mismatches por symlinks/`..`).
/// El path proviene del checkpoint de ESTE attempt (no de input de usuario), pero igual se valida
/// para no ejecutar `remove --force`/`reset --hard` sobre algo que no es el worktree esperado.
fn validate_owned_worktree(worktree_path: &str) -> Result<std::path::PathBuf> {
    let wt = Path::new(worktree_path);
    if !wt.join(".git").exists() {
        return Err(anyhow!(
            "no es un worktree git (falta `.git`): {worktree_path}"
        ));
    }
    let common_dir = git(wt, &["rev-parse", "--git-common-dir"])?;
    let common_dir = common_dir.trim();
    // git-common-dir puede ser relativo al worktree; resolverlo. Termina en `/.git` → el repo es su
    // padre.
    let common_abs = if Path::new(common_dir).is_absolute() {
        Path::new(common_dir).to_path_buf()
    } else {
        wt.join(common_dir)
    };
    let repo_root = common_abs
        .parent()
        .ok_or_else(|| anyhow!("no se pudo resolver el repo padre del worktree {worktree_path}"))?
        .to_path_buf();
    // Confirmar que el path está REGISTRADO como worktree de este repo (no un dir cualquiera con un
    // `.git` espurio). Canonicalizar ambos lados.
    let listing = git(&repo_root, &["worktree", "list", "--porcelain"])?;
    let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
    let registered = listing
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .any(|p| {
            let p = Path::new(p.trim());
            let p_canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            p_canon == wt_canon
        });
    if !registered {
        return Err(anyhow!(
            "el path no está registrado como worktree de su repo (stale/ajeno): {worktree_path}"
        ));
    }
    Ok(repo_root)
}

/// RESTAURA el worktree REUSADO al base del checkpoint: `git reset --hard <base>` + `git clean -fdq`.
///
/// SEGURIDAD (audit-3 H2 — no pisar trabajo ajeno): en best-of-N (spec 019) cada attempt corre en su
/// PROPIO worktree aislado. El path es `<repo>-<branch>` (ver `worktree::ensure`) y cada attempt usa
/// una branch distinta, de modo que ESTE worktree pertenece SÓLO a este attempt; el `reset --hard`
/// nunca toca el worktree de otro. El guard `validate_owned_worktree` confirma que el path es el
/// worktree git registrado de su repo (mismo path del checkpoint), no un dir compartido/ajeno, ANTES
/// de tocar el árbol. El reset es atómico desde git (mueve índice + working tree de una).
fn restore_worktree(worktree_path: &str, base_commit: &str) -> Result<()> {
    let wt = Path::new(worktree_path);
    if !wt.is_dir() {
        // el worktree ya no existe → nada que restaurar (no es un error de kill).
        return Ok(());
    }
    // Guard de ownership: sólo restauramos si es el worktree git registrado de su repo (audit-3 H2).
    validate_owned_worktree(worktree_path)?;
    git(wt, &["reset", "--hard", base_commit])?;
    git(wt, &["clean", "-fdq"])?;
    Ok(())
}

/// Descarta el worktree del attempt: `git worktree remove --force <path>` desde el repo principal.
/// `--force` porque puede tener cambios sin commitear (los del attempt que estamos abortando).
///
/// OWNERSHIP (audit-3 H1): NUNCA se ejecuta `remove --force` sobre un path arbitrario. Antes se valida
/// que `<path>` es un worktree git válido (`.git` existe) y está REGISTRADO en su repo padre
/// (`validate_owned_worktree`). Sólo entonces se hace el remove, desde el repo-root devuelto por el
/// guard (no re-derivado de forma laxa).
fn discard_worktree(worktree_path: &str) -> Result<()> {
    let wt = Path::new(worktree_path);
    if !wt.is_dir() {
        return Ok(()); // ya no existe → noop.
    }
    let repo_root = validate_owned_worktree(worktree_path)?;
    git(
        &repo_root,
        &["worktree", "remove", "--force", worktree_path],
    )?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| anyhow!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} -> {} | {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/035_attempt_checkpoint.sql"))
            .unwrap();
        // audit r2: la columna kill_token la agrega la migración 036 (ALTER idempotente).
        conn.execute_batch(include_str!(
            "../../migrations/036_attempt_checkpoint_kill_token.sql"
        ))
        .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// Helper de tests: lee el `kill_token` crudo de la fila (None si NULL).
    fn token_of(db: &Db, task_id: &str) -> Option<String> {
        let conn = db.lock();
        conn.query_row(
            "SELECT kill_token FROM attempt_checkpoints WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap()
        .flatten()
    }

    #[test]
    fn register_is_idempotent_and_keeps_first_base() {
        let db = test_db();
        register(&db, "t1", Some("g1"), "/wt/t1", "abc123", true).unwrap();
        // re-registrar con una base distinta NO pisa el punto de partida.
        register(&db, "t1", Some("g1"), "/wt/t1", "def456", true).unwrap();
        let c = get(&db, "t1").unwrap().expect("existe");
        assert_eq!(
            c.base_commit, "abc123",
            "first-write-wins del punto de partida"
        );
        assert_eq!(c.status, "open");
        assert!(c.created_worktree);
    }

    #[test]
    fn plan_discards_created_worktree_restores_reused() {
        let created = Checkpoint {
            task_id: "t1".into(),
            group_id: None,
            worktree_path: "/wt/t1".into(),
            base_commit: "abc".into(),
            created_worktree: true,
            status: "open".into(),
        };
        assert_eq!(plan_for(&created), KillPlan::Discard);
        let reused = Checkpoint {
            created_worktree: false,
            ..created
        };
        assert_eq!(
            plan_for(&reused),
            KillPlan::Restore {
                base_commit: "abc".into()
            }
        );
    }

    #[test]
    fn no_checkpoint_kill_is_noop() {
        let db = test_db();
        assert_eq!(
            kill_attempt(&db, "ghost").unwrap(),
            KillOutcome::NoCheckpoint
        );
    }

    #[test]
    fn claim_killing_is_single_winner_concurrent() {
        // Concurrencia: dos claims sobre el mismo checkpoint → exactamente UNO transiciona
        // open→killing; el 2º ve un killing fresco → InFlight (NO re-ejecuta git).
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let first = claim_killing(&db, "t1").unwrap();
        let second = claim_killing(&db, "t1").unwrap();
        let token = match first {
            Claim::Won { token } => token,
            other => panic!("el primer claim debe ganar, fue {other:?}"),
        };
        assert_eq!(
            second,
            Claim::InFlight,
            "el segundo ve el git in-flight, no re-ejecuta"
        );
        assert_eq!(get(&db, "t1").unwrap().unwrap().status, "killing");
        // Tras cerrar el kill (git OK) → killed → un claim posterior es AlreadyKilled.
        assert_eq!(
            mark_killed(&db, "t1", &token).unwrap(),
            FenceResult::Applied
        );
        assert_eq!(get(&db, "t1").unwrap().unwrap().status, "killed");
        assert_eq!(claim_killing(&db, "t1").unwrap(), Claim::AlreadyKilled);
    }

    #[test]
    fn release_killing_makes_checkpoint_retryable() {
        // El git falló (estado killing) → release vuelve a `open` → un nuevo claim PUEDE reintentar.
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let token = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("el primer claim debe ganar, fue {other:?}"),
        };
        assert_eq!(
            release_killing(&db, "t1", &token).unwrap(),
            FenceResult::Applied
        );
        assert_eq!(
            get(&db, "t1").unwrap().unwrap().status,
            "open",
            "el fallo del git deja el checkpoint re-intentable, no zombie"
        );
        assert!(
            token_of(&db, "t1").is_none(),
            "release limpia el fencing token al volver a open"
        );
        assert!(
            matches!(claim_killing(&db, "t1").unwrap(), Claim::Won { .. }),
            "el re-kill vuelve a ganar el claim"
        );
    }

    #[test]
    fn stale_killing_is_reclaimable_fresh_is_not() {
        // Un `killing` HUÉRFANO (kill que murió a mitad, killing_at viejo) es re-clamable; uno fresco
        // no.
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let token_a = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("el primer claim debe ganar, fue {other:?}"),
        };
        // killing FRESCO → otro claim NO gana.
        assert_eq!(claim_killing(&db, "t1").unwrap(), Claim::InFlight);
        // envejecer artificialmente el killing_at más allá del timeout.
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE attempt_checkpoints SET killing_at = datetime('now','-1 hour') \
                 WHERE task_id = 't1'",
                [],
            )
            .unwrap();
        }
        let token_b = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("un killing huérfano (>timeout) debe ser re-clamable, fue {other:?}"),
        };
        assert_ne!(
            token_a, token_b,
            "el stale-reclaim genera un fencing token NUEVO (sobrescribe el viejo)"
        );
    }

    /// Helper: arma un repo git temporal con un commit inicial dentro de un `TempDir`. El TempDir se
    /// borra solo al dropear (audit-3 L2: un assert que falla a mitad NO deja dirs colgados).
    fn git_repo() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_OPTIONAL_LOCKS", "0")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.email", "t@t.io"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(repo.join("f.txt"), "base\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "init"]);
        let base = g(&["rev-parse", "HEAD"]);
        (tmp, repo, base)
    }

    fn git_in(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn kill_real_worktree_discard_and_restore() {
        // E2E real con git: un worktree CREADO por el attempt → kill lo descarta; otro REUSADO +
        // cambios sin commitear → kill restaura al base. TempDir → cleanup automático (audit-3 L2).
        let (tmp, repo, base) = git_repo();
        let db = test_db();

        // ── Caso A: worktree CREADO por el attempt → discard.
        let wt_a = tmp.path().join("wt-a");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "attempt-a",
                wt_a.to_str().unwrap(),
            ],
        );
        std::fs::write(wt_a.join("new.txt"), "half-written\n").unwrap();
        register(&db, "tA", Some("g1"), wt_a.to_str().unwrap(), &base, true).unwrap();
        let out_a = kill_attempt(&db, "tA").unwrap();
        assert!(matches!(out_a, KillOutcome::WorktreeDiscarded { .. }));
        assert!(
            !wt_a.is_dir(),
            "el worktree creado se descartó por completo"
        );
        assert_eq!(get(&db, "tA").unwrap().unwrap().status, "killed");
        // idempotente: re-kill → AlreadyKilled, sin re-ejecutar git.
        assert_eq!(kill_attempt(&db, "tA").unwrap(), KillOutcome::AlreadyKilled);

        // ── Caso B: worktree REUSADO (no creado por el attempt) + cambios sin commitear → restore.
        let wt_b = tmp.path().join("wt-b");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "attempt-b",
                wt_b.to_str().unwrap(),
            ],
        );
        let base_b = git_in(&wt_b, &["rev-parse", "HEAD"]);
        // el attempt ensucia el worktree (modifica un tracked + agrega un untracked).
        std::fs::write(wt_b.join("f.txt"), "MUTATED by agent\n").unwrap();
        std::fs::write(wt_b.join("scratch.txt"), "junk\n").unwrap();
        register(
            &db,
            "tB",
            Some("g1"),
            wt_b.to_str().unwrap(),
            &base_b,
            false,
        )
        .unwrap();
        let out_b = kill_attempt(&db, "tB").unwrap();
        assert!(matches!(out_b, KillOutcome::WorktreeRestored { .. }));
        assert!(
            wt_b.is_dir(),
            "el worktree reusado NO se borra, se restaura"
        );
        assert_eq!(get(&db, "tB").unwrap().unwrap().status, "killed");
        // el tracked volvió a su contenido base; el untracked se limpió.
        assert_eq!(
            std::fs::read_to_string(wt_b.join("f.txt")).unwrap(),
            "base\n",
            "git reset --hard restauró el archivo tracked"
        );
        assert!(
            !wt_b.join("scratch.txt").exists(),
            "git clean -fdq quitó el untracked"
        );
    }

    #[test]
    fn kill_git_failure_leaves_checkpoint_retryable() {
        // audit-3 H3: si el git falla, el checkpoint NO queda `killed` (zombie) — vuelve a `open` y
        // un re-kill REINTENTA. Forzamos el fallo: discard de un path que existe (dir) pero NO es un
        // worktree git → `validate_owned_worktree` rechaza ANTES de tocar nada.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("not-a-worktree");
        std::fs::create_dir_all(&bogus).unwrap();
        let db = test_db();
        register(&db, "tX", None, bogus.to_str().unwrap(), "deadbeef", true).unwrap();

        let r1 = kill_attempt(&db, "tX");
        assert!(r1.is_err(), "el kill falla (path no es worktree git)");
        assert_eq!(
            get(&db, "tX").unwrap().unwrap().status,
            "open",
            "tras el fallo del git el checkpoint queda RE-INTENTABLE (no zombie)"
        );
        // un 2º intento PUEDE volver a intentar (no es noop AlreadyKilled).
        let r2 = kill_attempt(&db, "tX");
        assert!(
            r2.is_err(),
            "el re-kill reintenta (y vuelve a fallar igual)"
        );
        assert_eq!(get(&db, "tX").unwrap().unwrap().status, "open");
    }

    #[test]
    fn discard_rejects_unowned_path() {
        // audit-3 H1: `git worktree remove --force` NUNCA corre sobre un path arbitrario. Un dir que
        // NO es worktree git → rechazo del guard de ownership, SIN borrar nada.
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("precious-data");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("important.txt"), "no me borres\n").unwrap();

        let err = discard_worktree(victim.to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("no es un worktree git"),
            "rechaza por falta de `.git`: {err}"
        );
        assert!(
            victim.join("important.txt").exists(),
            "el guard NO ejecutó remove → los datos siguen intactos"
        );
    }

    #[test]
    fn restore_isolated_does_not_touch_other_worktree() {
        // audit-3 H2: el restore de UN attempt NO pisa el worktree de otro. En best-of-N cada attempt
        // tiene su worktree aislado (branch distinta → path distinto). Restaurar wt-b no toca wt-c.
        let (tmp, repo, _base) = git_repo();
        let db = test_db();

        // wt-b: reusado por el attempt B, lo ensucia.
        let wt_b = tmp.path().join("wt-b");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "attempt-b",
                wt_b.to_str().unwrap(),
            ],
        );
        let base_b = git_in(&wt_b, &["rev-parse", "HEAD"]);
        std::fs::write(wt_b.join("f.txt"), "B mutated\n").unwrap();

        // wt-c: OTRO attempt, su PROPIO worktree aislado, con su propio trabajo sin commitear.
        let wt_c = tmp.path().join("wt-c");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "attempt-c",
                wt_c.to_str().unwrap(),
            ],
        );
        std::fs::write(wt_c.join("f.txt"), "C work in progress\n").unwrap();
        std::fs::write(wt_c.join("c-scratch.txt"), "C untracked\n").unwrap();

        // Kill SÓLO el attempt B (restore al base de B).
        register(
            &db,
            "tB",
            Some("g1"),
            wt_b.to_str().unwrap(),
            &base_b,
            false,
        )
        .unwrap();
        assert!(matches!(
            kill_attempt(&db, "tB").unwrap(),
            KillOutcome::WorktreeRestored { .. }
        ));
        // B restaurado.
        assert_eq!(
            std::fs::read_to_string(wt_b.join("f.txt")).unwrap(),
            "base\n"
        );
        // C INTACTO: su trabajo sin commitear no se tocó (worktrees aislados).
        assert_eq!(
            std::fs::read_to_string(wt_c.join("f.txt")).unwrap(),
            "C work in progress\n",
            "el restore de B no pisó el tracked de C"
        );
        assert!(
            wt_c.join("c-scratch.txt").exists(),
            "el restore de B no limpió el untracked de C"
        );
    }

    #[test]
    fn stale_reclaim_fences_old_killer() {
        // audit r2: killer A reclama (token tA). A queda colgado. Tras el stale, killer B reclama
        // (token tB ≠ tA). A, al intentar `mark_killed` con tA, afecta 0 filas (LostClaim) y NO
        // cambia el estado; B cierra con tB → Applied. Sin el fencing token, A pisaría el claim de B.
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let token_a = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("A debe ganar el claim, fue {other:?}"),
        };
        // A se cuelga: envejecemos su killing_at más allá del timeout.
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE attempt_checkpoints SET killing_at = datetime('now','-1 hour') \
                 WHERE task_id = 't1'",
                [],
            )
            .unwrap();
        }
        // B re-clama el killing huérfano → token nuevo, sobrescribe el de A.
        let token_b = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("B debe re-clamar el killing huérfano, fue {other:?}"),
        };
        assert_ne!(token_a, token_b, "el reclaim genera un token nuevo");
        // A (killer viejo) intenta cerrar con SU token → 0 filas, no toca el claim de B.
        assert_eq!(
            mark_killed(&db, "t1", &token_a).unwrap(),
            FenceResult::LostClaim,
            "el killer viejo perdió el claim: su mark_killed es noop"
        );
        assert_eq!(
            get(&db, "t1").unwrap().unwrap().status,
            "killing",
            "el estado sigue siendo el claim de B (killing), no se marcó killed"
        );
        assert_eq!(
            token_of(&db, "t1").as_deref(),
            Some(token_b.as_str()),
            "el token de la fila sigue siendo el de B"
        );
        // B cierra con su token → Applied.
        assert_eq!(
            mark_killed(&db, "t1", &token_b).unwrap(),
            FenceResult::Applied,
            "el dueño actual (B) sí cierra el kill"
        );
        assert_eq!(get(&db, "t1").unwrap().unwrap().status, "killed");
    }

    #[test]
    fn release_killing_wrong_token_is_noop() {
        // audit r2: liberar con un token que NO es el del claim actual → 0 filas (LostClaim), no
        // revierte el claim del dueño real.
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let token = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("el claim debe ganar, fue {other:?}"),
        };
        // un token ajeno (de un killer viejo / inexistente) NO libera el claim.
        assert_eq!(
            release_killing(&db, "t1", "token-ajeno").unwrap(),
            FenceResult::LostClaim,
            "release con token equivocado es noop"
        );
        assert_eq!(
            get(&db, "t1").unwrap().unwrap().status,
            "killing",
            "el claim del dueño real sigue intacto"
        );
        assert_eq!(
            token_of(&db, "t1").as_deref(),
            Some(token.as_str()),
            "el token de la fila no cambió"
        );
        // el dueño real SÍ libera con su token.
        assert_eq!(
            release_killing(&db, "t1", &token).unwrap(),
            FenceResult::Applied
        );
        assert_eq!(get(&db, "t1").unwrap().unwrap().status, "open");
    }

    #[test]
    fn kill_lock_path_is_outside_worktree_and_stable() {
        // audit r3: el lock NO debe vivir dentro del worktree que se borra, y debe ser estable para
        // el mismo path (dos killers apuntando al mismo worktree comparten el lock).
        let (tmp, repo, _base) = git_repo();
        let wt = tmp.path().join("wt-lockpath");
        git_in(
            &repo,
            &["worktree", "add", "-q", "-b", "lp", wt.to_str().unwrap()],
        );
        let p = kill_lock_path(wt.to_str().unwrap());
        assert!(
            !p.starts_with(&wt),
            "el lock-file NO puede estar dentro del worktree que discard borra: {}",
            p.display()
        );
        // estable: mismo worktree → mismo lock-path (canonicalización idempotente).
        assert_eq!(p, kill_lock_path(wt.to_str().unwrap()));
        // y derivado del path → worktrees distintos dan locks distintos.
        let wt2 = tmp.path().join("wt-other");
        git_in(
            &repo,
            &["worktree", "add", "-q", "-b", "lp2", wt2.to_str().unwrap()],
        );
        assert_ne!(p, kill_lock_path(wt2.to_str().unwrap()));
    }

    #[test]
    fn kill_serializes_git_under_os_lock() {
        // audit r3 HIGH: si OTRO killer tiene el lock de OS del worktree (su git destructivo está
        // corriendo), un `kill_attempt` concurrente NO ejecuta git solapado — ve el lock tomado,
        // revierte su DB-claim a `open` (re-intentable) y devuelve KillInFlight. Simulamos al "otro
        // killer" tomando el lock manualmente y reteniéndolo durante el kill.
        let (tmp, repo, base) = git_repo();
        let db = test_db();
        let wt = tmp.path().join("wt-serial");
        git_in(
            &repo,
            &["worktree", "add", "-q", "-b", "ser", wt.to_str().unwrap()],
        );
        std::fs::write(wt.join("agent.txt"), "work in progress\n").unwrap();
        register(&db, "tS", Some("g1"), wt.to_str().unwrap(), &base, true).unwrap();

        // "Otro killer" toma el lock de OS y lo retiene (su git está corriendo).
        let held = match try_acquire_kill_lock(wt.to_str().unwrap()).unwrap() {
            LockTry::Acquired(g) => g,
            LockTry::Held => panic!("nadie más tiene el lock todavía"),
        };

        // Nuestro kill_attempt ve el lock tomado → NO corre git → KillInFlight, claim revertido.
        let out = kill_attempt(&db, "tS").unwrap();
        assert_eq!(
            out,
            KillOutcome::KillInFlight,
            "con el lock tomado por otro killer, NO se ejecuta git concurrente"
        );
        assert_eq!(
            get(&db, "tS").unwrap().unwrap().status,
            "open",
            "el claim se revierte a open (re-intentable), no queda colgado en killing"
        );
        // el worktree y su trabajo siguen INTACTOS (el git NO corrió bajo el lock ajeno).
        assert!(wt.is_dir(), "el worktree no se tocó");
        assert_eq!(
            std::fs::read_to_string(wt.join("agent.txt")).unwrap(),
            "work in progress\n",
            "ningún git destructivo corrió en paralelo al lock ajeno"
        );

        // Liberamos el lock del "otro killer"; ahora un re-kill SÍ puede tomar el lock y completar.
        drop(held);
        let out2 = kill_attempt(&db, "tS").unwrap();
        assert!(
            matches!(out2, KillOutcome::WorktreeDiscarded { .. }),
            "tras liberar el lock, el re-kill toma el lock y descarta el worktree: {out2:?}"
        );
        assert!(!wt.is_dir(), "el worktree se descartó");
        assert_eq!(get(&db, "tS").unwrap().unwrap().status, "killed");
    }

    #[test]
    fn kill_lock_released_after_successful_kill() {
        // audit r3: tras un kill EXITOSO el lock de OS queda libre — un intento posterior de tomarlo
        // (otro killer) lo consigue. Usamos un worktree REUSADO (restore) para que el path siga
        // existiendo tras el kill y poder re-tomar el lock sobre el mismo path.
        let (tmp, repo, _base) = git_repo();
        let db = test_db();
        let wt = tmp.path().join("wt-rel");
        git_in(
            &repo,
            &["worktree", "add", "-q", "-b", "rel", wt.to_str().unwrap()],
        );
        let base = git_in(&wt, &["rev-parse", "HEAD"]);
        std::fs::write(wt.join("f.txt"), "mutated\n").unwrap();
        register(&db, "tR", Some("g1"), wt.to_str().unwrap(), &base, false).unwrap();

        assert!(matches!(
            kill_attempt(&db, "tR").unwrap(),
            KillOutcome::WorktreeRestored { .. }
        ));
        // el lock se liberó al terminar el kill → se puede re-tomar.
        match try_acquire_kill_lock(wt.to_str().unwrap()).unwrap() {
            LockTry::Acquired(_g) => {}
            LockTry::Held => panic!("el lock debió quedar libre tras el kill exitoso"),
        }
    }

    #[test]
    fn stale_reclaim_then_old_killer_with_lock_does_not_run_git() {
        // audit r4 HIGH: ventana entre DB-claim y flock. A gana el claim (tA) y se PAUSA antes de
        // tomar el lock. Tras el stale, B re-clama (tB) y COMPLETA su kill (toma el lock, corre git,
        // marca killed, suelta el lock). A despierta y toma el flock YA LIBRE. La revalidación bajo DB
        // (`still_owner`) detecta que el token actual ya NO es tA → A NO corre git destructivo.
        //
        // Reproducimos la secuencia con un worktree REUSADO (restore) para poder observar el árbol tras
        // el kill de B y confirmar que el "git de A" no vuelve a tocarlo.
        let (tmp, repo, _base) = git_repo();
        let db = test_db();
        let wt = tmp.path().join("wt-r4");
        git_in(
            &repo,
            &["worktree", "add", "-q", "-b", "r4", wt.to_str().unwrap()],
        );
        let base = git_in(&wt, &["rev-parse", "HEAD"]);
        // el attempt ensucia el worktree.
        std::fs::write(wt.join("f.txt"), "agent mutation\n").unwrap();
        std::fs::write(wt.join("scratch.txt"), "junk\n").unwrap();
        register(&db, "t1", Some("g1"), wt.to_str().unwrap(), &base, false).unwrap();

        // ── A gana el claim (token tA) y se "pausa" ANTES de tomar el lock.
        let token_a = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("A debe ganar el claim, fue {other:?}"),
        };

        // ── A está pausado. Envejecemos su killing_at para que B pueda hacer stale-reclaim.
        {
            let conn = db.lock();
            conn.execute(
                "UPDATE attempt_checkpoints SET killing_at = datetime('now','-1 hour') \
                 WHERE task_id = 't1'",
                [],
            )
            .unwrap();
        }

        // ── B re-clama el killing huérfano (tB) y COMPLETA su kill: toma el lock, corre el git
        // (restore al base), marca killed, suelta el lock. Lo hacemos vía `kill_attempt` que es el
        // camino real de B (gana el stale-reclaim por sí solo).
        let out_b = kill_attempt(&db, "t1").unwrap();
        assert!(
            matches!(out_b, KillOutcome::WorktreeRestored { .. }),
            "B completa el kill (restore): {out_b:?}"
        );
        assert_eq!(get(&db, "t1").unwrap().unwrap().status, "killed");
        let token_b = token_of(&db, "t1"); // killed deja kill_token con el de B (no se limpia).
        assert_ne!(
            token_b.as_deref(),
            Some(token_a.as_str()),
            "el token de la fila tras el kill de B NO es el de A"
        );
        // El worktree quedó restaurado por B. Re-ensuciamos para detectar si el "git de A" corriera.
        std::fs::write(wt.join("f.txt"), "post-B sentinel\n").unwrap();
        std::fs::write(wt.join("sentinel.txt"), "must survive A\n").unwrap();

        // ── A despierta. Tiene tA. El lock está libre (B lo soltó). A reanuda su tail: toma el flock y
        // revalida bajo DB ANTES del git. Reproducimos exactamente ese tramo con las funciones internas.
        let _lock = match try_acquire_kill_lock(wt.to_str().unwrap()).unwrap() {
            LockTry::Acquired(g) => g,
            LockTry::Held => panic!("el lock debió quedar libre tras el kill de B"),
        };
        // La revalidación de A (token tA) detecta que ya NO es el dueño → NO corre git.
        assert_eq!(
            still_owner(&db, "t1", &token_a).unwrap(),
            FenceResult::LostClaim,
            "A perdió la autoridad: still_owner debe ser LostClaim antes de tocar git"
        );

        // Verificación de comportamiento: el árbol que dejó B (con nuestros sentinels) sigue INTACTO —
        // ningún git destructivo de A corrió.
        assert_eq!(
            std::fs::read_to_string(wt.join("f.txt")).unwrap(),
            "post-B sentinel\n",
            "el git de A NO corrió: el tracked sigue como lo dejamos tras B"
        );
        assert!(
            wt.join("sentinel.txt").exists(),
            "el git de A NO corrió: el untracked sentinel sobrevivió (no hubo clean -fdq)"
        );
        assert_eq!(
            get(&db, "t1").unwrap().unwrap().status,
            "killed",
            "el estado sigue siendo el kill de B; A no lo alteró"
        );
        assert_eq!(
            token_of(&db, "t1"),
            token_b,
            "el token de la fila sigue siendo el de B; A no lo pisó"
        );
    }

    #[test]
    fn mark_killed_zero_rows_does_not_report_success() {
        // audit r2: un `mark_killed` que afecta 0 filas (token equivocado) DEBE devolver LostClaim,
        // nunca Applied — el caller no debe reportar el kill como completado.
        let db = test_db();
        register(&db, "t1", None, "/wt/t1", "abc", true).unwrap();
        let _token = match claim_killing(&db, "t1").unwrap() {
            Claim::Won { token } => token,
            other => panic!("el claim debe ganar, fue {other:?}"),
        };
        // intentar cerrar con un token equivocado → LostClaim, sin marcar killed.
        assert_eq!(
            mark_killed(&db, "t1", "no-soy-el-dueño").unwrap(),
            FenceResult::LostClaim,
            "0 filas afectadas NO reporta éxito"
        );
        assert_eq!(
            get(&db, "t1").unwrap().unwrap().status,
            "killing",
            "el checkpoint NO quedó killed con un token equivocado"
        );
    }
}
