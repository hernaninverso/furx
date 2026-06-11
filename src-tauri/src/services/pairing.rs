// services/pairing.rs — 014-orchestration-ux FR-002 (pairing sync-to-local).
//
// Traer el branch de una tarea de orquestación a la WORKING COPY del repo principal manteniendo
// el git state (checkout del branch, no copiar files a ciegas), para seguir el trabajo en el IDE.
//
// Constitución VI (parar ante destructivo): si la working copy está SUCIA, NUNCA pisamos cambios
// locales. Hacemos `git stash push --include-untracked` con un mensaje rastreable ANTES de cambiar
// de branch, así el usuario puede recuperar su WIP con `git stash pop`. argv-only (sin shell).
//
// Diseño propio (FR-006): inspirado en el patrón "sync-to-local" de sculptor, NO portado.

use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

/// Reporte de lo que hizo el sync (para auditoría + UI).
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// Branch al que quedó la working copy.
    pub branch: String,
    /// Branch en el que estaba ANTES del sync.
    pub prev_branch: Option<String>,
    /// ¿La working copy estaba sucia al empezar?
    pub was_dirty: bool,
    /// ¿Se hizo stash de cambios locales?
    pub stashed: bool,
    /// Referencia del stash creado (ej "stash@{0}") para que el user lo recupere.
    pub stash_ref: Option<String>,
    /// Mensaje humano de lo que pasó.
    pub message: String,
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| anyhow!("git {}: {}", args.join(" "), e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} -> {} | {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// ¿Está sucia la working copy? (cambios staged/unstaged/untracked).
fn is_dirty(cwd: &Path) -> Result<bool> {
    let status = run_git(cwd, &["status", "--porcelain"])?;
    Ok(status.lines().any(|l| !l.trim().is_empty()))
}

/// Cuántas entradas hay en el stash (para detectar si un `stash push` realmente guardó algo,
/// sin un pre-check `is_dirty` que abriría un TOCTOU). 0 si no hay stash.
fn stash_count(cwd: &Path) -> usize {
    run_git(cwd, &["stash", "list"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Trae `branch` a la working copy de `repo_path` manteniendo git state. Stash-guard anti-destructivo.
///
/// Pasos:
///   1. Validar repo. NO actuar si `repo_path` es un worktree separado (debe ser el repo principal).
///   2. Si la working copy está sucia → `git stash push -u -m "furx-pairing-sync …"` (guarda WIP).
///   3. `git checkout <branch>` (el branch ya existe — lo creó el worktree de la tarea).
///   4. Reportar qué pasó (stash_ref para recuperar).
///
/// NUNCA descarta cambios locales: si el stash o el checkout fallan, devuelve Err sin haber pisado
/// nada irrecuperable (el stash, si se hizo, queda intacto y recuperable con `git stash pop`).
pub fn sync_branch_to_local(repo_path: &str, branch: &str) -> Result<SyncReport> {
    let cwd = Path::new(repo_path);
    if !cwd.is_dir() || !cwd.join(".git").exists() {
        return Err(anyhow!("no es un repo git: {}", repo_path));
    }
    // El branch debe existir (lo creó el worktree de la tarea). Si no existe, abortamos antes de
    // tocar nada (no creamos branches acá — pairing trae trabajo existente).
    if run_git(
        cwd,
        &["rev-parse", "--verify", &format!("refs/heads/{}", branch)],
    )
    .is_err()
    {
        return Err(anyhow!("el branch '{}' no existe en {}", branch, repo_path));
    }

    let prev_branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();

    // Si ya estamos en ese branch, no hay nada que hacer (idempotente).
    if prev_branch.as_deref() == Some(branch) {
        return Ok(SyncReport {
            branch: branch.to_string(),
            prev_branch: prev_branch.clone(),
            was_dirty: is_dirty(cwd).unwrap_or(false),
            stashed: false,
            stash_ref: None,
            message: format!("Ya estabas en '{}', nada que sincronizar.", branch),
        });
    }

    // Importante: si el branch está checked-out en OTRO worktree (el de la tarea), git no deja
    // hacer checkout en el repo principal. Detectarlo y dar un error claro en vez del críptico de git.
    if branch_checked_out_elsewhere(cwd, branch)? {
        return Err(anyhow!(
            "'{}' está checked-out en el worktree de la tarea; cerrá/remové ese worktree antes de \
             traerlo a local (pairing trae el branch al repo principal).",
            branch
        ));
    }

    let was_dirty = is_dirty(cwd)?;
    let mut stashed = false;
    let mut stash_ref: Option<String> = None;

    // Audit fix codex+deepseek 014: serializar stash+checkout bajo el MISMO lock repo-wide que
    // `git worktree add` (FR-005), para que ninguna otra operación git de Furx sobre el repo
    // principal corra en paralelo (cierra la race status→stash→checkout). El guard se sostiene
    // hasta el final de la mutación.
    let repo_lock = crate::services::orchestration::repo_worktree_lock(repo_path);
    let _repo_guard = repo_lock.lock();

    // Constitución VI: stash ANTES de cambiar de branch, nunca pisar. Stash INCONDICIONAL para
    // eliminar el TOCTOU entre `is_dirty` y el stash — si no hay cambios, git no crea entrada y
    // lo reporta; detectamos si stasheó por el conteo de la stash-list (no por el pre-check).
    let stash_before = stash_count(cwd);
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let msg = format!("furx-pairing-sync {} (auto-stash {})", branch, stamp);
    // `--include-untracked` cubre tracked+untracked; los IGNORADOS (.env, etc.) NO los toca el
    // checkout de git, así que no hace falta `--all` (evita stashear node_modules/build dirs).
    run_git(cwd, &["stash", "push", "--include-untracked", "-m", &msg])?;
    if stash_count(cwd) > stash_before {
        stashed = true;
        stash_ref = Some("stash@{0}".to_string());
    }

    // Checkout del branch — mantiene el git state (es el branch real, no una copia de files).
    if let Err(e) = run_git(cwd, &["checkout", branch]) {
        // Si el checkout falla DESPUÉS de stashear, el WIP del usuario está a salvo en el stash.
        // Devolvemos un error que incluye cómo recuperarlo (no perdemos nada).
        if stashed {
            return Err(anyhow!(
                "no se pudo cambiar a '{}': {}. Tu trabajo local quedó guardado en {} \
                 (recuperalo con `git stash pop`).",
                branch,
                e,
                stash_ref.as_deref().unwrap_or("stash@{0}")
            ));
        }
        return Err(anyhow!("no se pudo cambiar a '{}': {}", branch, e));
    }

    let message = if stashed {
        format!(
            "Traje '{}' a la working copy. Tus cambios locales quedaron en {} \
             (recuperalos con `git stash pop` cuando vuelvas).",
            branch,
            stash_ref.as_deref().unwrap_or("stash@{0}")
        )
    } else {
        format!(
            "Traje '{}' a la working copy (no había cambios locales que guardar).",
            branch
        )
    };

    Ok(SyncReport {
        branch: branch.to_string(),
        prev_branch,
        was_dirty,
        stashed,
        stash_ref,
        message,
    })
}

/// ¿`branch` está checked-out en algún OTRO worktree de este repo? (git no deja checkout doble).
fn branch_checked_out_elsewhere(cwd: &Path, branch: &str) -> Result<bool> {
    let listing = run_git(cwd, &["worktree", "list", "--porcelain"])?;
    // El repo principal es el primer "worktree " del listado; cualquier otro con este branch es "elsewhere".
    let main_path = run_git(cwd, &["rev-parse", "--show-toplevel"]).ok();
    let mut current_path: Option<String> = None;
    for line in listing.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current_path = Some(p.trim().to_string());
        } else if let Some(b) = line.strip_prefix("branch ") {
            let b_short = b.trim().trim_start_matches("refs/heads/");
            if b_short == branch {
                // ¿es un worktree DISTINTO al principal?
                if let (Some(cp), Some(mp)) = (current_path.as_deref(), main_path.as_deref()) {
                    if cp != mp {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
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

    fn init_repo() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        let p = td.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.email", "t@t.io"]);
        git(p, &["config", "user.name", "t"]);
        std::fs::write(p.join("a.txt"), "hello\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "-q", "-m", "init"]);
        // crear el branch de la "tarea" (como haría un worktree, pero acá lo dejamos en el mismo repo).
        git(p, &["branch", "furx-task"]);
        td
    }

    #[test]
    fn sync_clean_working_copy_no_stash() {
        let td = init_repo();
        let repo = td.path().to_str().unwrap();
        let r = sync_branch_to_local(repo, "furx-task").unwrap();
        assert_eq!(r.branch, "furx-task");
        assert!(!r.was_dirty);
        assert!(!r.stashed);
        // HEAD quedó en furx-task.
        let head = run_git(td.path(), &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(head, "furx-task");
    }

    #[test]
    fn sync_dirty_working_copy_stashes_never_loses() {
        let td = init_repo();
        let repo = td.path().to_str().unwrap();
        // ensuciar la working copy (cambio NO commiteado).
        std::fs::write(td.path().join("a.txt"), "local WIP — no perder\n").unwrap();
        std::fs::write(td.path().join("untracked.txt"), "untracked WIP\n").unwrap();
        let r = sync_branch_to_local(repo, "furx-task").unwrap();
        assert!(r.was_dirty);
        assert!(r.stashed, "working copy sucia → stash");
        // el stash existe y contiene el WIP (no se perdió).
        let stash_list = run_git(td.path(), &["stash", "list"]).unwrap();
        assert!(
            stash_list.contains("furx-pairing-sync"),
            "stash rastreable creado"
        );
        // recuperar y verificar que el contenido local volvió.
        git(td.path(), &["checkout", "main"]);
        git(td.path(), &["stash", "pop"]);
        let content = std::fs::read_to_string(td.path().join("a.txt")).unwrap();
        assert_eq!(
            content, "local WIP — no perder\n",
            "el WIP local se recuperó intacto"
        );
        assert!(
            td.path().join("untracked.txt").exists(),
            "el untracked se recuperó"
        );
    }

    #[test]
    fn sync_idempotent_when_already_on_branch() {
        let td = init_repo();
        let repo = td.path().to_str().unwrap();
        git(td.path(), &["checkout", "furx-task"]);
        let r = sync_branch_to_local(repo, "furx-task").unwrap();
        assert!(!r.stashed);
        assert!(r.message.contains("Ya estabas"));
    }

    #[test]
    fn sync_rejects_nonexistent_branch() {
        let td = init_repo();
        let repo = td.path().to_str().unwrap();
        assert!(sync_branch_to_local(repo, "no-existe").is_err());
    }
}
