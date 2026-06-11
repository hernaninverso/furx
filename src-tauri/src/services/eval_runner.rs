// 2.6 — Eval harness UI: wrapper sobre Inspect AI + promptfoo en ~/eval/.
// Council V4: timeout 60s, captura stderr, isolated subprocess.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct EvalTask {
    pub name: String,
    pub kind: String, // "inspect" | "promptfoo" | "custom"
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalRun {
    pub task: String,
    pub status: String, // "ok" | "fail" | "timeout"
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u64,
}

pub fn list_tasks() -> Result<Vec<EvalTask>> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let root = home.join("eval");
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            if p.is_file() {
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                let kind = match ext {
                    "yaml" | "yml" => "promptfoo",
                    "py" => "inspect",
                    _ => continue,
                };
                out.push(EvalTask {
                    name,
                    kind: kind.into(),
                    path: p.to_string_lossy().to_string(),
                });
            }
        }
    }
    Ok(out)
}

pub async fn run(task: &EvalTask) -> Result<EvalRun> {
    if !is_safe_name(&task.name) {
        return Err(anyhow!("unsafe task name"));
    }
    // SECURITY: task.path comes from the frontend; confine it to ~/eval/ so a forged
    // path can't run an arbitrary file as a promptfoo/inspect config (arbitrary-file →
    // RCE). Canonicalize (resolves `..`/symlinks) and verify containment.
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let root = home
        .join("eval")
        .canonicalize()
        .map_err(|e| anyhow!("eval root unavailable: {}", e))?;
    let resolved = std::path::Path::new(&task.path)
        .canonicalize()
        .map_err(|e| anyhow!("eval path: {}", e))?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(anyhow!("eval path must be an existing file under ~/eval/"));
    }
    let safe_path = resolved.to_string_lossy().to_string();
    let started = std::time::Instant::now();
    let (cmd, args): (&str, Vec<&str>) = match task.kind.as_str() {
        "promptfoo" => ("promptfoo", vec!["eval", "-c", safe_path.as_str()]),
        "inspect" => ("inspect", vec!["eval", safe_path.as_str()]),
        _ => return Err(anyhow!("unknown kind: {}", task.kind)),
    };
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(cmd).args(&args).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| anyhow!("eval timed out"))?
    .map_err(|e| anyhow!("eval spawn: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    Ok(EvalRun {
        task: task.name.clone(),
        status: if out.status.success() { "ok" } else { "fail" }.into(),
        stdout,
        stderr,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 128
        && !s.contains("..")
        && !s.contains('/')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_task_names() {
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("../escape"));
        assert!(!is_safe_name("path/to"));
        assert!(is_safe_name("twin_voice.py"));
        assert!(is_safe_name("config.yaml"));
    }
}
