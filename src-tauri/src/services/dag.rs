// 2.4 — Plan→Tasks DAG visual.
// Parse .specify/*.md (spec-kit) y extrae tasks + dependencies → nodos+edges.
// Council V1: serde_yaml::IgnoredAny y cycle detection (Tarjan).

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DagNode {
    pub id: String,
    pub title: String,
    pub status: String,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dag {
    pub nodes: Vec<DagNode>,
    pub source: String,
    pub cycle: Option<Vec<String>>,
}

pub fn parse_repo(repo: &Path) -> Result<Vec<Dag>> {
    let specify_dir = repo.join(".specify");
    if !specify_dir.is_dir() {
        return Err(anyhow!(".specify not found in {}", repo.display()));
    }
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&specify_dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if ext != "md" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let source = p
            .strip_prefix(repo)
            .unwrap_or(&p)
            .to_string_lossy()
            .to_string();
        let mut nodes = parse_md_tasks(&text);
        let cycle = detect_cycle(&nodes);
        // Normalize node order alphabetically by id.
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        out.push(Dag {
            nodes,
            source,
            cycle,
        });
    }
    Ok(out)
}

/// Heuristic: parse markdown looking for `- [ ] T<NUM>: title (deps: T1, T2)` or
/// numbered tasks `1.1 title`, etc. Status from checkbox state.
fn parse_md_tasks(md: &str) -> Vec<DagNode> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE_CHECK: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^\s*-\s*\[([ x])\]\s*(T\d+(?:\.\d+)*|[\d.]+)\s*[:.]?\s*(.+?)(?:\s*\(deps?:\s*([^)]+)\))?\s*$").unwrap()
    });
    let mut out = Vec::new();
    for line in md.lines() {
        if let Some(caps) = RE_CHECK.captures(line) {
            let done = &caps[1] == "x";
            let id = caps[2].to_string();
            let title = caps[3].trim().to_string();
            let deps: Vec<String> = caps
                .get(4)
                .map(|m| {
                    m.as_str()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            out.push(DagNode {
                id,
                title,
                status: if done { "done" } else { "pending" }.into(),
                deps,
            });
        }
    }
    out
}

fn detect_cycle(nodes: &[DagNode]) -> Option<Vec<String>> {
    use std::collections::{HashMap, HashSet};
    let by_id: HashMap<&str, &DagNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut on_stack: HashSet<&str> = HashSet::new();
    let mut path: Vec<String> = Vec::new();
    fn dfs<'a>(
        id: &'a str,
        by_id: &HashMap<&'a str, &'a DagNode>,
        visited: &mut HashSet<&'a str>,
        on_stack: &mut HashSet<&'a str>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        if on_stack.contains(id) {
            return Some(path.clone());
        }
        if visited.contains(id) {
            return None;
        }
        visited.insert(id);
        on_stack.insert(id);
        path.push(id.to_string());
        if let Some(n) = by_id.get(id) {
            for dep in &n.deps {
                if let Some(cycle) = dfs(dep.as_str(), by_id, visited, on_stack, path) {
                    return Some(cycle);
                }
            }
        }
        on_stack.remove(id);
        path.pop();
        None
    }
    for n in nodes {
        if let Some(cycle) = dfs(
            n.id.as_str(),
            &by_id,
            &mut visited,
            &mut on_stack,
            &mut path,
        ) {
            return Some(cycle);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_checked_list() {
        let md = "- [x] T1: Setup\n- [ ] T2: Build (deps: T1)\n";
        let nodes = parse_md_tasks(md);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].status, "done");
        assert_eq!(nodes[1].deps, vec!["T1"]);
    }

    #[test]
    fn detects_cycle() {
        let nodes = vec![
            DagNode {
                id: "A".into(),
                title: "a".into(),
                status: "pending".into(),
                deps: vec!["B".into()],
            },
            DagNode {
                id: "B".into(),
                title: "b".into(),
                status: "pending".into(),
                deps: vec!["A".into()],
            },
        ];
        let c = detect_cycle(&nodes);
        assert!(c.is_some());
    }

    #[test]
    fn no_cycle_simple_chain() {
        let nodes = vec![
            DagNode {
                id: "A".into(),
                title: "a".into(),
                status: "pending".into(),
                deps: vec![],
            },
            DagNode {
                id: "B".into(),
                title: "b".into(),
                status: "pending".into(),
                deps: vec!["A".into()],
            },
        ];
        assert!(detect_cycle(&nodes).is_none());
    }
}
