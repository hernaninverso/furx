// F1 — Compile a per-pane bootstrap.md so `claude-as-A/B` can pass it via
// `--append-system-prompt @path`. Combines: MEMORY.md index head + recent cards +
// project git log (if cwd is a repo).

use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_MEMORY_BYTES: usize = 4096;
const MAX_GIT_LOG_LINES: usize = 10;
// BLOQUE B · F1: spec says last 5 cards; previous impl used 10. Aligned.
const RECENT_CARDS_LIMIT: usize = 5;
// BLOQUE B · F1: cap optional ~/.furx/contexts/furx_logo_decision.md so a
// malformed/huge file can't blow up the bootstrap markdown.
const MAX_LOGO_DECISION_BYTES: usize = 4096;

pub fn compile_for_pane(
    pane_id: &str,
    project_dir: Option<&Path>,
    db: &Connection,
) -> Result<PathBuf> {
    if pane_id.is_empty()
        || pane_id.len() > 64
        || pane_id == "."
        || pane_id == ".."
        || pane_id.contains("..")
        || pane_id.starts_with('.')
        || !pane_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(anyhow::anyhow!("invalid pane_id: {}", pane_id));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
    let contexts = home.join(".furx").join("contexts");
    std::fs::create_dir_all(&contexts)?;

    let mut md = String::new();
    md.push_str("# (Furx) bootstrap\n\n");
    md.push_str(&format!(
        "Pane: `{}`. Generated: {}\n\n",
        pane_id,
        chrono::Utc::now().to_rfc3339()
    ));

    // Section 1 — MEMORY.md index (truncated head).
    let memory_path = home
        .join(".claude")
        .join("projects")
        .join("-Users-hernan")
        .join("memory")
        .join("MEMORY.md");
    if let Ok(content) = std::fs::read_to_string(&memory_path) {
        let head: String = content.chars().take(MAX_MEMORY_BYTES).collect();
        md.push_str("## Memory index (head)\n\n");
        md.push_str(&head);
        if content.len() > MAX_MEMORY_BYTES {
            md.push_str("\n…(truncated; see MEMORY.md for full index)\n");
        }
        md.push_str("\n\n");
    }

    // Section 2 — recent cards (last 5 per spec, F1).
    if let Ok(rows) = recent_cards(db) {
        if !rows.is_empty() {
            md.push_str(&format!(
                "## Recent cards (last {})\n\n",
                RECENT_CARDS_LIMIT
            ));
            for (project, title, severity, status) in rows {
                md.push_str(&format!(
                    "- [{}/{}] **{}** — {}\n",
                    project, severity, title, status
                ));
            }
            md.push('\n');
        }
    }

    // Section 2b — optional brand/visual decision context (F1 edge-case).
    // Skip silently if missing; cap size; never render as HTML — code-block it.
    let logo_decision_path = home
        .join(".furx")
        .join("contexts")
        .join("furx_logo_decision.md");
    if let Ok(text) = std::fs::read_to_string(&logo_decision_path) {
        let trimmed: String = text.chars().take(MAX_LOGO_DECISION_BYTES).collect();
        md.push_str("## Brand / visual decision\n\n```text\n");
        md.push_str(&trimmed);
        if text.len() > MAX_LOGO_DECISION_BYTES {
            md.push_str("\n…(truncated)\n");
        }
        md.push_str("\n```\n\n");
    }

    // Section 3 — project git log (only if cwd is a real git repo under $HOME).
    if let Some(dir) = project_dir {
        if dir.starts_with(&home) && dir.join(".git").exists() {
            if let Some(log) = run_git(
                dir,
                &["log", "--oneline", "-n", &MAX_GIT_LOG_LINES.to_string()],
            ) {
                md.push_str(&format!(
                    "## Recent commits in `{}`\n\n```\n",
                    dir.display()
                ));
                md.push_str(&log);
                md.push_str("```\n\n");
            }
            if let Some(branch) = run_git(dir, &["symbolic-ref", "--short", "HEAD"]) {
                md.push_str(&format!("Current branch: `{}`\n\n", branch.trim()));
            }
        }
    }

    let path = contexts.join(format!("{}-bootstrap.md", pane_id));
    std::fs::write(&path, &md)?;
    Ok(path)
}

fn recent_cards(db: &Connection) -> Result<Vec<(String, String, String, String)>> {
    let stmt_sql = format!(
        "SELECT project, title, severity, status FROM cards \
         ORDER BY created_at DESC LIMIT {}",
        RECENT_CARDS_LIMIT
    );
    let mut stmt = db.prepare(&stmt_sql)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn rejects_unsafe_pane_id() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(compile_for_pane("..", None, &conn).is_err());
        assert!(compile_for_pane("a/b", None, &conn).is_err());
        assert!(compile_for_pane("", None, &conn).is_err());
    }
}
