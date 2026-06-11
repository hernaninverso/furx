// F6 — Council on demand (⌘J).
// Invokes the local ~/council/ CLI (already battle-tested with 4 voices +
// Codex local) rather than reimplementing the orchestration. This was the
// V3 pragmatic recommendation: use what works, don't duplicate.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct CouncilRun {
    pub query: String,
    pub markdown: String,
    pub elapsed_ms: u64,
}

pub async fn run(query: &str) -> Result<CouncilRun> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    if q.len() > 8000 {
        return Err(anyhow!("query > 8KB"));
    }
    let council_py = which("python3").unwrap_or_else(|| "/usr/bin/env".to_string());
    let council_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("no home"))?
        .join("council");
    if !council_dir.join("council.py").exists() {
        return Err(anyhow!("~/council/council.py not installed"));
    }
    let started = Instant::now();

    // Write query to a temp file (council.py expects --plan-file).
    // Gemini MED: RAII guard — the file is removed when `_guard` drops, even on
    // panic or early return.
    let tmp = std::env::temp_dir().join(format!("furx-council-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, q)?;
    struct TmpFileGuard(std::path::PathBuf);
    impl Drop for TmpFileGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _guard = TmpFileGuard(tmp.clone());

    let mut cmd = Command::new(council_py);
    cmd.current_dir(&council_dir)
        .args(["council.py", "review", "--plan-file"])
        .arg(tmp.to_str().unwrap())
        .kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| anyhow!("council CLI timed out"))?
        .map_err(|e| anyhow!("council CLI spawn: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(anyhow!(
            "council CLI exit {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(CouncilRun {
        query: q.to_string(),
        markdown: if stdout.trim().is_empty() {
            stderr
        } else {
            stdout
        },
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn which(cmd: &str) -> Option<String> {
    if let Ok(p) = std::env::var("PATH") {
        for d in p.split(':') {
            let cand = std::path::Path::new(d).join(cmd);
            if cand.exists() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}
