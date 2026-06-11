// 2.3 — Persistent agent memory per project via mnemo subprocess.
// Council V1: regex strict para project name; argv-only.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const SAFE_PROJECT_RE: &str = r"^[A-Za-z0-9_./@-]+$";

#[derive(Debug, Clone, Serialize)]
pub struct ProjectMemory {
    pub project: String,
    pub recalled: String,
    pub source: String, // "mnemo" | "memento" | "fallback"
}

pub async fn recall(project_or_path: &str) -> Result<ProjectMemory> {
    let project = derive_project_name(project_or_path)?;
    if !is_safe(&project) {
        return Err(anyhow!("unsafe project name: {}", project));
    }
    // Try `mnemo recall` first.
    if let Some(out) = try_cli("mnemo", &["recall", &project]).await {
        return Ok(ProjectMemory {
            project,
            recalled: out,
            source: "mnemo".into(),
        });
    }
    // Fall back to `memento ask`.
    if let Some(out) = try_cli("memento", &["ask", &format!("recent work on {}", project)]).await {
        return Ok(ProjectMemory {
            project,
            recalled: out,
            source: "memento".into(),
        });
    }
    Ok(ProjectMemory {
        project,
        recalled: String::new(),
        source: "fallback".into(),
    })
}

async fn try_cli(bin: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(bin);
    cmd.args(args).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .ok()?
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

fn derive_project_name(s: &str) -> Result<String> {
    let p = Path::new(s);
    if p.is_absolute() {
        Ok(p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(s)
            .to_string())
    } else {
        Ok(s.to_string())
    }
}

fn is_safe(s: &str) -> bool {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(SAFE_PROJECT_RE).unwrap());
    !s.is_empty() && s.len() < 128 && !s.contains("..") && RE.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_project() {
        assert!(!is_safe(""));
        assert!(!is_safe("; rm -rf /"));
        assert!(!is_safe("../escape"));
        assert!(!is_safe("$(whoami)"));
        assert!(is_safe("furx"));
        assert!(is_safe("eleion-portal"));
    }

    #[test]
    fn derives_basename_from_path() {
        assert_eq!(derive_project_name("/Users/dev/furx").unwrap(), "furx");
        assert_eq!(derive_project_name("furx").unwrap(), "furx");
    }
}
