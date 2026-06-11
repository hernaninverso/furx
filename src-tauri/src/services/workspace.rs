// services/workspace.rs — 014-orchestration-ux FR-004 (board↔workspace + cleanup escape-hatch).
//
// El board de 008 ya muestra branch/worktree/estado por card; esto agrega el GC de worktrees con
// un ESCAPE-HATCH para debug (constitución VI: el cleanup destructivo es opt-out vía
// `DISABLE_WORKTREE_CLEANUP`). Sólo limpia worktrees de tareas en estado TERMINAL
// (done/failed/canceled) — NUNCA de una running/awaiting_review (esos los necesita el humano).
//
// Diseño propio (FR-006): inspirado en el board↔workspace de vibe-kanban, NO portado.

use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Estados terminales cuyos worktrees son candidatos a cleanup.
const TERMINAL_STATES: &[&str] = &["done", "failed", "canceled"];

/// ¿Está activo el escape-hatch que DESACTIVA el cleanup de worktrees? (constitución VI — opt-out
/// para debug). Se chequea env var `DISABLE_WORKTREE_CLEANUP` (cualquier valor truthy) O el setting
/// `orchestration.disable_worktree_cleanup` en la DB. Default: cleanup HABILITADO.
pub fn cleanup_disabled(db: &Db) -> bool {
    if env_truthy("DISABLE_WORKTREE_CLEANUP") {
        return true;
    }
    let conn = db.lock();
    crate::settings::get(&conn, "orchestration.disable_worktree_cleanup")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim().to_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "no" && v != "off"
        }
        Err(_) => false,
    }
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

/// Remueve los worktrees de tareas TERMINALES (done/failed/canceled) de `repo_path`. NO toca
/// worktrees de tareas running/awaiting_review (los necesita el humano), ni el repo principal.
/// `git worktree remove --force` por cada worktree terminal registrado en la DB que siga en git.
/// El caller (command) YA verificó el escape-hatch + confirmación. Devuelve los paths removidos.
pub fn cleanup_terminal_worktrees(db: &Db, repo_path: &str) -> Result<Vec<String>> {
    let cwd = Path::new(repo_path);
    if !cwd.is_dir() || !cwd.join(".git").exists() {
        return Err(anyhow!("no es un repo git: {}", repo_path));
    }
    // Worktrees que git conoce para este repo (para no intentar remover lo que ya no existe).
    let known = crate::services::worktree::list_for_repo(cwd)
        .map(|v| {
            v.into_iter()
                .map(|w| w.worktree_path)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    // Worktrees de tareas terminales según la DB.
    let candidates: Vec<(String, String)> = {
        let conn = db.lock();
        let placeholders = TERMINAL_STATES
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, worktree_path FROM orchestration_tasks
             WHERE repo_path = ? AND worktree_path IS NOT NULL AND state IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&repo_path];
        for s in TERMINAL_STATES {
            params_vec.push(s);
        }
        let rows = stmt
            .query_map(params_vec.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };

    let mut removed = Vec::new();
    for (task_id, wt) in candidates {
        if !known.contains(&wt) {
            continue; // git ya no lo conoce (prune se encarga del registro)
        }
        // Defensa (audit 014): `known` ya acota `wt` a worktrees que GIT reporta para este
        // repo, pero rechazamos `..` por las dudas (path viene de la DB).
        if wt.contains("..") {
            continue;
        }
        // Audit fix codex+deepseek 014 (constitución VI — parar ante destructivo): usar
        // `git worktree remove` SIN `--force`. git REHÚSA si el worktree tiene cambios no
        // commiteados → un worktree sucio (p.ej. una variante best-of-N descartada con
        // trabajo útil sin commitear) NO se destruye: se SALTEA y queda para revisión
        // humana. Sólo se limpian worktrees limpios (su trabajo ya está en la branch).
        if run_git(cwd, &["worktree", "remove", &wt]).is_ok() {
            removed.push(wt.clone());
            // limpiar el puntero en la DB + soltar locks de esa tarea (best-effort).
            {
                let conn = db.lock();
                let _ = conn.execute(
                    "UPDATE orchestration_tasks SET worktree_path = NULL WHERE id = ?1",
                    rusqlite::params![task_id],
                );
            }
            let _ = crate::services::orchestration::release_all_locks(db, &task_id);
        }
        // si falló (worktree sucio) → lo dejamos intacto, NO forzamos.
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/022_orchestration.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/024_done_detection.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/025_orchestration_ux.sql"))
            .unwrap();
        // 019 F3: columna paused_at (pause/resume) — sincroniza el schema con la migración real.
        conn.execute_batch(include_str!(
            "../../migrations/037_orch_pause_council_history.sql"
        ))
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    #[test]
    fn escape_hatch_setting_disables_cleanup() {
        let db = test_db();
        assert!(!cleanup_disabled(&db), "default: cleanup habilitado");
        {
            let conn = db.lock();
            crate::settings::set(
                &conn,
                "orchestration.disable_worktree_cleanup",
                &serde_json::json!(true),
            )
            .unwrap();
        }
        assert!(cleanup_disabled(&db), "setting activa el escape-hatch");
    }

    #[test]
    fn env_truthy_parsing() {
        std::env::set_var("FURX_TEST_HATCH", "1");
        assert!(env_truthy("FURX_TEST_HATCH"));
        std::env::set_var("FURX_TEST_HATCH", "false");
        assert!(!env_truthy("FURX_TEST_HATCH"));
        std::env::set_var("FURX_TEST_HATCH", "");
        assert!(!env_truthy("FURX_TEST_HATCH"));
        std::env::remove_var("FURX_TEST_HATCH");
        assert!(!env_truthy("FURX_TEST_HATCH"));
    }

    #[test]
    fn cleanup_rejects_non_repo() {
        let db = test_db();
        let td = tempfile::tempdir().unwrap();
        // dir sin .git → error.
        assert!(cleanup_terminal_worktrees(&db, td.path().to_str().unwrap()).is_err());
    }
}
