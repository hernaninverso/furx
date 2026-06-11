// F3 — Git worktree per Claude pane.
// Creates ~/.furx/worktrees/<repo>-<branch>/, runs `git worktree add` if missing.
// Sanitizes repo/branch names; argv-only (no shell).

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Worktree {
    pub repo_path: String,
    pub branch: String,
    pub worktree_path: String,
    pub created: bool,
}

const SAFE_NAME: &str = r"^[A-Za-z0-9_.-][A-Za-z0-9_./-]{0,62}[A-Za-z0-9_-]$|^[A-Za-z0-9_-]$";

/// Create (or reuse) a worktree for `repo_path` at `branch`.
/// branch may already exist or be a new name; if new, `git worktree add -b` is used.
pub fn ensure(repo_path: &Path, branch: &str) -> Result<Worktree> {
    if !is_safe_name(branch) {
        return Err(anyhow!("unsafe branch name: {}", branch));
    }
    if !repo_path.is_dir() || !repo_path.join(".git").exists() {
        return Err(anyhow!("not a git repo: {}", repo_path.display()));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let abs_repo = repo_path.canonicalize()?;
    if !abs_repo.starts_with(&home) {
        return Err(anyhow!("repo outside $HOME: {}", abs_repo.display()));
    }
    let repo_name = abs_repo
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("repo has no name"))?
        .to_string();
    if !is_safe_name(&repo_name) {
        return Err(anyhow!("unsafe repo name: {}", repo_name));
    }
    let worktrees_root = home.join(".furx").join("worktrees");
    std::fs::create_dir_all(&worktrees_root)?;
    // Single-segment path under worktrees_root — sanitized above.
    let wt_path: PathBuf =
        worktrees_root.join(format!("{}-{}", repo_name, branch.replace('/', "_")));

    if wt_path.exists() {
        // BLOQUE B · Codex must-fix: differentiate "same worktree (idempotent)"
        // from "stale path that belongs to another repo or another branch".
        // `git worktree list --porcelain` from THIS repo is the source of truth.
        // Codex audit B LOW: don't swallow git errors as "no entries" — that
        // would misreport collision when the real cause is a broken repo.
        let listing = run_git(&abs_repo, &["worktree", "list", "--porcelain"])
            .map_err(|e| anyhow!("git worktree list failed for {}: {}", abs_repo.display(), e))?;
        let wt_str = wt_path.to_string_lossy();
        let mut owned_by_repo = false;
        let mut matches_branch = false;
        let mut current_path: Option<String> = None;
        for line in listing.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                current_path = Some(p.trim().to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                if let Some(p) = current_path.as_deref() {
                    let b_short = b.trim().trim_start_matches("refs/heads/");
                    if p == wt_str {
                        owned_by_repo = true;
                        matches_branch = b_short == branch;
                    }
                }
            }
        }
        if !owned_by_repo {
            return Err(anyhow!(
                "worktree path exists but is not registered in this repo (stale or another repo): {}",
                wt_str
            ));
        }
        if !matches_branch {
            return Err(anyhow!(
                "worktree {} already checked out to a different branch (requested: {})",
                wt_str,
                branch
            ));
        }
        return Ok(Worktree {
            repo_path: abs_repo.to_string_lossy().to_string(),
            branch: branch.to_string(),
            worktree_path: wt_str.to_string(),
            created: false,
        });
    }
    // Resolve whether the branch exists.
    let branch_exists = run_git(&abs_repo, &["rev-parse", "--verify", branch]).is_ok();
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
    if !branch_exists {
        args.push("-b".into());
        args.push(branch.to_string());
        args.push(wt_path.to_string_lossy().to_string());
    } else {
        args.push(wt_path.to_string_lossy().to_string());
        args.push(branch.to_string());
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_git(&abs_repo, &args_ref).map_err(|e| anyhow!("git worktree add failed: {}", e))?;
    Ok(Worktree {
        repo_path: abs_repo.to_string_lossy().to_string(),
        branch: branch.to_string(),
        worktree_path: wt_path.to_string_lossy().to_string(),
        created: true,
    })
}

pub fn list_for_repo(repo_path: &Path) -> Result<Vec<Worktree>> {
    if !repo_path.join(".git").exists() {
        return Ok(vec![]);
    }
    let abs_repo = repo_path.canonicalize()?;
    let out = run_git(&abs_repo, &["worktree", "list", "--porcelain"])?;
    let mut current: Option<(String, String)> = None;
    let mut all = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some((path, branch)) = current.take() {
                all.push(Worktree {
                    repo_path: abs_repo.to_string_lossy().to_string(),
                    branch,
                    worktree_path: path,
                    created: false,
                });
            }
            current = Some((p.trim().to_string(), String::new()));
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some((_, ref mut br)) = current {
                *br = b.trim().trim_start_matches("refs/heads/").to_string();
            }
        }
    }
    if let Some((path, branch)) = current {
        all.push(Worktree {
            repo_path: abs_repo.to_string_lossy().to_string(),
            branch,
            worktree_path: path,
            created: false,
        });
    }
    Ok(all)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
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

/// Exposed for use from `commands::worktree_merge_review`.
pub fn is_safe_branch_for_api(s: &str) -> bool {
    is_safe_name(s)
}

fn is_safe_name(s: &str) -> bool {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(SAFE_NAME).unwrap());
    // Forbid path traversal segments.
    if s.contains("..") || s.contains("//") || s.starts_with('/') || s.ends_with('/') {
        return false;
    }
    RE.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_branch() {
        assert!(!is_safe_name("../etc"));
        assert!(!is_safe_name("a/../b"));
        assert!(!is_safe_name("/etc/passwd"));
    }

    #[test]
    fn allows_typical_branches() {
        assert!(is_safe_name("main"));
        assert!(is_safe_name("feature/X"));
        assert!(is_safe_name("hot-fix_1"));
        assert!(is_safe_name("v0.1.0-stable"));
    }
}
