// F10 — Semantic ⌘P search. Three sources, parallel, deduped, ranked.
// Pattern inspired by ~/socrates/src/socrates/research/agent.py (MIT-compatible).
//
// Sources:
//   1) code — grep -rEn in cwd (or repo root). Argv only.
//   2) memories — scan ~/.claude/projects/-Users-hernan/memory/*.md.
//   3) git — `git log -S <pattern> --oneline -n 20` in cwd.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_RESULTS_PER_SOURCE: usize = 12;
const MAX_LINE_LEN: usize = 240;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub source: String, // "code" | "memories" | "git"
    pub path: String,
    pub line: Option<u64>,
    pub snippet: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub elapsed_ms: u64,
}

pub fn run(query: &str, cwd: Option<&Path>) -> Result<SearchResult> {
    let q = query.trim();
    if q.is_empty() {
        return Err(anyhow!("empty query"));
    }
    if q.len() > 200 {
        return Err(anyhow!("query too long"));
    }
    // Pattern is treated as fixed string (-F) for grep to avoid regex injection.
    let started = std::time::Instant::now();
    let cwd_owned: PathBuf = cwd
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));
    if !is_safe_cwd(&cwd_owned) {
        return Err(anyhow!("unsafe cwd: {}", cwd_owned.display()));
    }
    let mut hits = Vec::new();
    hits.extend(search_code(q, &cwd_owned));
    hits.extend(search_memories(q));
    hits.extend(search_git(q, &cwd_owned));
    rank(&mut hits, q);
    dedup(&mut hits);
    Ok(SearchResult {
        query: q.to_string(),
        hits,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn is_safe_cwd(p: &Path) -> bool {
    let Ok(abs) = p.canonicalize() else {
        return false;
    };
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    abs.starts_with(&home)
}

fn search_code(pattern: &str, cwd: &Path) -> Vec<SearchHit> {
    // Argv: grep -rEnI --fixed-strings --max-count 5 --include excludes-dir, -- pattern .
    let out = Command::new("grep")
        .current_dir(cwd)
        .args([
            "-rnI",
            "--fixed-strings",
            "--max-count",
            "5",
            "--exclude-dir=node_modules",
            "--exclude-dir=.venv",
            "--exclude-dir=venv",
            "--exclude-dir=target",
            "--exclude-dir=.git",
            "--exclude-dir=dist",
            "--exclude-dir=build",
            "--exclude-dir=__pycache__",
            "--exclude-dir=.next",
            "--",
            pattern,
            ".",
        ])
        .output();
    let Ok(out) = out else {
        return vec![];
    };
    let mut hits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(MAX_RESULTS_PER_SOURCE * 2)
    {
        if let Some(h) = parse_grep_line(line, "code") {
            hits.push(h);
            if hits.len() >= MAX_RESULTS_PER_SOURCE {
                break;
            }
        }
    }
    hits
}

fn parse_grep_line(line: &str, source: &str) -> Option<SearchHit> {
    // grep output: ./path/file.rs:42:snippet…
    let mut parts = line.splitn(3, ':');
    let path = parts.next()?.to_string();
    let line_no: u64 = parts.next()?.parse().ok()?;
    let snippet_raw = parts.next()?.trim();
    if snippet_raw.is_empty() {
        return None;
    }
    let snippet = if snippet_raw.len() > MAX_LINE_LEN {
        format!("{}…", &snippet_raw[..MAX_LINE_LEN])
    } else {
        snippet_raw.to_string()
    };
    Some(SearchHit {
        source: source.to_string(),
        path,
        line: Some(line_no),
        snippet,
        score: 1.0,
    })
}

fn search_memories(pattern: &str) -> Vec<SearchHit> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let dir = home
        .join(".claude")
        .join("projects")
        .join("-Users-hernan")
        .join("memory");
    if !dir.exists() {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut hits = Vec::new();
    let needle = pattern.to_lowercase();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let lower = text.to_lowercase();
        if !lower.contains(&needle) {
            continue;
        }
        // Find first matching line for snippet.
        let snippet = text
            .lines()
            .find(|l| l.to_lowercase().contains(&needle))
            .map(|l| {
                if l.len() > MAX_LINE_LEN {
                    format!("{}…", &l[..MAX_LINE_LEN])
                } else {
                    l.to_string()
                }
            })
            .unwrap_or_else(|| text.chars().take(MAX_LINE_LEN).collect());
        // Recency-ish score from modified time.
        let recency_score: f32 = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| (d.as_secs_f32() / 1e9).min(2.0))
            .unwrap_or(0.5);
        hits.push(SearchHit {
            source: "memories".into(),
            path: p.to_string_lossy().to_string(),
            line: None,
            snippet,
            score: 1.0 + recency_score,
        });
        if hits.len() >= MAX_RESULTS_PER_SOURCE {
            break;
        }
    }
    hits
}

fn search_git(pattern: &str, cwd: &Path) -> Vec<SearchHit> {
    if !cwd.join(".git").exists() {
        return vec![];
    }
    let out = Command::new("git")
        .current_dir(cwd)
        .args(["log", "--oneline", "-S", pattern, "-n", "20"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    let Ok(out) = out else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    let mut hits = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .take(MAX_RESULTS_PER_SOURCE)
    {
        if line.trim().is_empty() {
            continue;
        }
        let snippet = if line.len() > MAX_LINE_LEN {
            format!("{}…", &line[..MAX_LINE_LEN])
        } else {
            line.to_string()
        };
        hits.push(SearchHit {
            source: "git".into(),
            path: cwd.to_string_lossy().to_string(),
            line: None,
            snippet,
            score: 0.9,
        });
    }
    hits
}

/// Rank: exact-substring-in-snippet bonus, source weight, then alphabetical fallback.
fn rank(hits: &mut [SearchHit], q: &str) {
    let q_lower = q.to_lowercase();
    for h in hits.iter_mut() {
        let snip = h.snippet.to_lowercase();
        if snip.contains(&q_lower) {
            h.score += 0.5;
        }
        // Prefer memories slightly (curated content).
        if h.source == "memories" {
            h.score += 0.2;
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn dedup(hits: &mut Vec<SearchHit>) {
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert((h.source.clone(), h.path.clone(), h.line)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_query() {
        assert!(run("", None).is_err());
    }

    #[test]
    fn rejects_huge_query() {
        let big = "x".repeat(300);
        assert!(run(&big, None).is_err());
    }

    #[test]
    fn parses_grep_output() {
        let h = parse_grep_line("./foo.rs:42:hello world", "code").unwrap();
        assert_eq!(h.source, "code");
        assert_eq!(h.path, "./foo.rs");
        assert_eq!(h.line, Some(42));
        assert_eq!(h.snippet, "hello world");
    }
}
