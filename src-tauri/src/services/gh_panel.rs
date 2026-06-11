// 2.24 — GitHub issue/PR side panel via `gh` CLI subprocess.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct GhItem {
    pub number: i64,
    pub title: String,
    pub state: String,
    pub author: Option<String>,
    pub updated_at: Option<String>,
    pub url: Option<String>,
    pub kind: String, // "pr" | "issue"
}

pub async fn list_prs(repo: &Path) -> Result<Vec<GhItem>> {
    list(repo, "pr").await
}
pub async fn list_issues(repo: &Path) -> Result<Vec<GhItem>> {
    list(repo, "issue").await
}

async fn list(repo: &Path, kind: &str) -> Result<Vec<GhItem>> {
    if !repo.join(".git").exists() {
        return Err(anyhow!("not a git repo"));
    }
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("gh")
            .current_dir(repo)
            .args([
                kind,
                "list",
                "--json",
                "number,title,state,author,updatedAt,url",
                "--limit",
                "20",
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("gh timed out"))?
    .map_err(|e| anyhow!("gh spawn: {}", e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let items = v.as_array().cloned().unwrap_or_default();
    Ok(items
        .into_iter()
        .map(|it| GhItem {
            number: it.get("number").and_then(|x| x.as_i64()).unwrap_or(0),
            title: it
                .get("title")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string(),
            state: it
                .get("state")
                .and_then(|x| x.as_str())
                .unwrap_or("?")
                .to_string(),
            author: it
                .get("author")
                .and_then(|a| a.get("login"))
                .and_then(|x| x.as_str())
                .map(String::from),
            updated_at: it
                .get("updatedAt")
                .and_then(|x| x.as_str())
                .map(String::from),
            url: it.get("url").and_then(|x| x.as_str()).map(String::from),
            kind: kind.to_string(),
        })
        .collect())
}
