// F11 — Project registry: scan $HOME for `.git` directories, cache to `projects` table.
// Scan is bounded: max depth 4, excludes node_modules/.venv/target/dist/build, max 500 entries.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

const MAX_DEPTH: usize = 4;
const MAX_RESULTS: usize = 500;
const EXCLUDED: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    "__pycache__",
    ".idea",
    ".vscode",
    "Library",
    ".Trash",
    ".docker",
    ".cargo",
    ".rustup",
    ".npm",
    ".pyenv",
    ".pnpm-store",
];

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub path: String,
    pub name: String,
    pub branch: Option<String>,
    pub last_commit: Option<String>,
    pub last_commit_at: Option<String>,
    pub dirty: bool,
    pub scanned_at: String,
}

/// Walk $HOME shallow, collect `.git` parents, upsert to `projects` table.
/// Honors a hard cap on count and a depth cap. Returns updated count.
pub fn scan(db: Arc<Mutex<Connection>>) -> Result<usize> {
    let started = Instant::now();
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
    let mut found: Vec<PathBuf> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((home.clone(), 0));
    while let Some((dir, depth)) = queue.pop_front() {
        if found.len() >= MAX_RESULTS {
            break;
        }
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Symlinks → skip (avoid loops & escapes).
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with('.') && name != ".furx" && name != ".claude" {
                // skip hidden EXCEPT a few known infra dirs
                if !path.join(".git").exists() {
                    continue;
                }
            }
            if EXCLUDED.contains(&name) {
                continue;
            }
            if path.join(".git").exists() {
                found.push(path.clone());
                // Don't recurse into a repo's children (worktrees handled separately).
                continue;
            }
            queue.push_back((path, depth + 1));
        }
    }
    // Use rusqlite::Connection::transaction — RAII rollback on Drop if commit
    // is not called. Replaces manual BEGIN/COMMIT which could leave the DB
    // in an open-tx state on error (ultra-review V3 HIGH).
    let mut conn = db.lock();
    let tx = conn.transaction()?;
    let mut count = 0usize;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO projects (path, name, branch, last_commit, last_commit_at, dirty, scanned_at) \
             VALUES (?, ?, ?, ?, ?, ?, datetime('now')) \
             ON CONFLICT(path) DO UPDATE SET \
                name = excluded.name, branch = excluded.branch, \
                last_commit = excluded.last_commit, last_commit_at = excluded.last_commit_at, \
                dirty = excluded.dirty, scanned_at = excluded.scanned_at",
        )?;
        for repo in &found {
            let info = inspect_repo(repo);
            stmt.execute(params![
                info.path,
                info.name,
                info.branch,
                info.last_commit,
                info.last_commit_at,
                info.dirty as i64
            ])?;
            count += 1;
        }
    }
    tx.commit()?;
    tracing::info!(
        found = count,
        took_ms = started.elapsed().as_millis() as u64,
        "projects::scan"
    );
    Ok(count)
}

/// Query cached project list. Cheap; safe to call from UI.
pub fn list(db: &Mutex<Connection>) -> Result<Vec<Project>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT path, name, branch, last_commit, last_commit_at, dirty, scanned_at \
         FROM projects ORDER BY scanned_at DESC LIMIT 500",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Project {
                path: r.get(0)?,
                name: r.get(1)?,
                branch: r.get(2)?,
                last_commit: r.get(3)?,
                last_commit_at: r.get(4)?,
                dirty: r.get::<_, i64>(5)? != 0,
                scanned_at: r.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn inspect_repo(p: &Path) -> Project {
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string();
    let branch = run_git(p, &["symbolic-ref", "--short", "HEAD"]);
    let last_commit = run_git(p, &["log", "-1", "--format=%h %s"]);
    let last_commit_at = run_git(p, &["log", "-1", "--format=%cI"]);
    let dirty = run_git(p, &["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    Project {
        path: p.to_string_lossy().to_string(),
        name,
        branch,
        last_commit,
        last_commit_at,
        dirty,
        scanned_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(cwd)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Arc<Mutex<Connection>> {
        let mut conn = Connection::open_in_memory().unwrap();
        rusqlite_migration::Migrations::new(vec![
            rusqlite_migration::M::up(include_str!("../../migrations/001_init.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/002_settings.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/003_layout_default.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/004_sprint_tables.sql")),
            rusqlite_migration::M::up(include_str!("../../migrations/005_cards_context.sql")),
        ])
        .to_latest(&mut conn)
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn list_empty_returns_empty() {
        let db = fresh_db();
        let v = list(&db).unwrap();
        assert!(v.is_empty());
    }
}
