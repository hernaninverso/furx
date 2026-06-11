// F21 — Auto context-bundle.
// Combina git log/diff + journalctl (si server) + log excerpt en un markdown,
// pasa por F32 Guardrail antes de devolverlo. Sanitiza paths.

use crate::bases::guardrail;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_DIFF_BYTES: usize = 4096;
const MAX_LOG_BYTES: usize = 2048;

#[derive(Debug, Clone)]
pub struct BundleInputs<'a> {
    pub project_dir: Option<&'a Path>,
    pub log_path: Option<&'a Path>,
    pub extra_note: Option<&'a str>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Bundle {
    pub markdown: String,
    pub redacted: bool,
    pub bytes: usize,
}

/// Build a context bundle. Never panics; missing pieces are skipped silently.
pub fn build(inputs: BundleInputs<'_>) -> Bundle {
    let mut out = String::new();
    out.push_str("# Context bundle\n\n");
    if let Some(dir) = inputs.project_dir {
        if is_safe_git_dir(dir) {
            out.push_str("## Git log (last 20)\n\n```\n");
            out.push_str(
                &run_git(dir, &["log", "--oneline", "--decorate", "-n", "20"]).unwrap_or_default(),
            );
            out.push_str("```\n\n");
            out.push_str("## Git diff (HEAD, truncated)\n\n```diff\n");
            let mut diff = run_git(dir, &["diff", "HEAD", "--stat"]).unwrap_or_default();
            if diff.len() > MAX_DIFF_BYTES {
                diff.truncate(MAX_DIFF_BYTES);
                diff.push_str("\n…(truncated)\n");
            }
            out.push_str(&diff);
            out.push_str("```\n\n");
        }
    }
    if let Some(lp) = inputs.log_path {
        if is_safe_log_path(lp) {
            if let Ok(text) = read_tail(lp, MAX_LOG_BYTES) {
                out.push_str(&format!("## Log excerpt — {}\n\n```\n", lp.display()));
                out.push_str(&text);
                out.push_str("\n```\n\n");
            }
        }
    }
    if let Some(note) = inputs.extra_note {
        out.push_str("## Notes\n\n");
        out.push_str(note);
        out.push('\n');
    }
    // Guardrail — redact, never block (we still want SOMETHING to show).
    let (redacted_text, hits) = guardrail::redact(&out);
    let redacted = !hits.is_empty();
    let mut final_text = redacted_text;
    if redacted {
        final_text.push_str(&format!(
            "\n\n> ⚠ {} secret pattern(s) redacted by F32 guardrail: {}\n",
            hits.len(),
            hits.join(", ")
        ));
    }
    let bytes = final_text.len();
    Bundle {
        markdown: final_text,
        redacted,
        bytes,
    }
}

/// F21 — Build the per-card context bundle: pulls the card row, looks up the
/// project dir from the cached `projects` registry, and feeds both into
/// `build()`. This is the standalone, reusable form that both
/// `card_open_in_claude` and other surfaces can call without duplicating logic.
pub fn card_context(
    db: &parking_lot::Mutex<rusqlite::Connection>,
    card_id: &str,
) -> Result<Bundle> {
    if card_id.is_empty() || card_id.len() > 64 {
        return Err(anyhow!("invalid card_id: {}", card_id));
    }
    // Pull card metadata in a short critical section, then drop the lock before
    // running git (which can take >100ms).
    let (project, source, title, cause, project_dir): (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
    ) = {
        let conn = db.lock();
        let (project, source, title, cause) = conn
            .query_row(
                "SELECT project, source, title, cause FROM cards WHERE id = ?",
                rusqlite::params![card_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .map_err(|e| anyhow!("card lookup failed: {}", e))?;
        let project_dir: Option<String> = conn
            .query_row(
                "SELECT path FROM projects WHERE name = ? LIMIT 1",
                rusqlite::params![project],
                |r| r.get::<_, String>(0),
            )
            .ok();
        (project, source, title, cause, project_dir)
    };
    let proj_path: Option<PathBuf> = project_dir.as_deref().map(PathBuf::from);
    let note = format!(
        "Triggered from card `{}`\n- project: {}\n- source: {}\n- title: {}\n- cause: {}",
        card_id,
        project,
        source,
        title,
        cause.as_deref().unwrap_or("—"),
    );
    let inputs = BundleInputs {
        project_dir: proj_path.as_deref(),
        log_path: None, // future: derive from card.context_log_path if added
        extra_note: Some(&note),
    };
    Ok(build(inputs))
}

/// Save a bundle to ~/.furx/contexts/card-<id>.md, returning the path.
pub fn save_for_card(card_id: &str, bundle: &Bundle) -> Result<PathBuf> {
    // card_id sanitization — UUID-ish only.
    if card_id.is_empty()
        || card_id.len() > 64
        || !card_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(anyhow!("invalid card_id: {}", card_id));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let dir = home.join(".furx").join("contexts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("card-{}.md", card_id));
    std::fs::write(&path, &bundle.markdown)?;
    Ok(path)
}

fn is_safe_git_dir(p: &Path) -> bool {
    if !p.is_dir() {
        return false;
    }
    let Ok(abs) = p.canonicalize() else {
        return false;
    };
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    if !abs.starts_with(&home) {
        return false;
    }
    abs.join(".git").exists()
}

fn is_safe_log_path(p: &Path) -> bool {
    let Ok(abs) = p.canonicalize() else {
        return false;
    };
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    // Allow logs under $HOME or /tmp only — never /etc, /var/log, /private, /dev.
    abs.starts_with(&home) || abs.starts_with(Path::new("/tmp"))
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    // We pass args directly to Command, NOT through a shell — argv array prevents injection.
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0") // don't wait for index lock
        .output()?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn read_tail(p: &Path, max_bytes: usize) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p)?;
    let len = f.metadata()?.len();
    let start = if (len as usize) > max_bytes {
        len - max_bytes as u64
    } else {
        0
    };
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(max_bytes);
    f.take(max_bytes as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bundle_still_renders() {
        let b = build(BundleInputs {
            project_dir: None,
            log_path: None,
            extra_note: None,
        });
        assert!(b.markdown.contains("# Context bundle"));
        assert!(!b.redacted);
    }

    #[test]
    fn redacts_secret_in_extra_note() {
        let note = "leaked: sk-ant-abcdefghijklmnopqrstuvwxyz0123456789ABCD".to_string();
        let b = build(BundleInputs {
            project_dir: None,
            log_path: None,
            extra_note: Some(&note),
        });
        assert!(b.redacted);
        assert!(!b
            .markdown
            .contains("sk-ant-abcdefghijklmnopqrstuvwxyz0123456789ABCD"));
    }

    #[test]
    fn rejects_unsafe_card_id() {
        let b = Bundle {
            markdown: "x".into(),
            redacted: false,
            bytes: 1,
        };
        assert!(save_for_card("../escape", &b).is_err());
        assert!(save_for_card("with/slash", &b).is_err());
    }
}
