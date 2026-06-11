// 2.5 — Live diff preview pre-accept. Heurística sobre buffer PTY:
// detecta bloque diff (++/--/@@) y devuelve span + parsed hunks.
// V4: ignora bloques dentro de heredocs ("<<EOF" ... "EOF").

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiffBlock {
    pub start_line: usize,
    pub end_line: usize,
    pub raw: String,
    pub added: usize,
    pub removed: usize,
}

pub fn detect_blocks(buffer: &str) -> Vec<DiffBlock> {
    let lines: Vec<&str> = buffer.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    let mut heredoc: Option<String> = None;
    while i < lines.len() {
        let l = lines[i];
        // Heredoc tracking
        if heredoc.is_none() {
            if let Some(tag) = find_heredoc_open(l) {
                heredoc = Some(tag);
                i += 1;
                continue;
            }
        } else if let Some(tag) = &heredoc {
            if l.trim() == tag.as_str() {
                heredoc = None;
            }
            i += 1;
            continue;
        }
        // Look for unified diff start: "--- " then "+++ " on next line, or "@@ -X +Y @@"
        if is_diff_header(l, lines.get(i + 1).copied()) || l.starts_with("@@ ") {
            let start = i;
            let mut end = i;
            let mut added = 0usize;
            let mut removed = 0usize;
            while end < lines.len() {
                let m = lines[end];
                if end > start
                    && (m.is_empty() || m.starts_with("$ ") || m.starts_with("> "))
                    && !m.starts_with("+")
                    && !m.starts_with("-")
                {
                    break;
                }
                if m.starts_with('+') && !m.starts_with("+++") {
                    added += 1;
                }
                if m.starts_with('-') && !m.starts_with("---") {
                    removed += 1;
                }
                end += 1;
            }
            if added + removed >= 2 {
                blocks.push(DiffBlock {
                    start_line: start,
                    end_line: end - 1,
                    raw: lines[start..end].join("\n"),
                    added,
                    removed,
                });
            }
            i = end;
            continue;
        }
        i += 1;
    }
    blocks
}

fn is_diff_header(line: &str, next: Option<&str>) -> bool {
    line.starts_with("--- ") && next.map(|n| n.starts_with("+++ ")).unwrap_or(false)
}

fn find_heredoc_open(line: &str) -> Option<String> {
    // Match "<<EOF", "<<-EOF", "<< 'EOF'", etc.
    let trimmed = line.trim_end();
    let idx = trimmed.find("<<")?;
    let after = &trimmed[idx + 2..];
    let after = after.trim_start_matches('-').trim_start();
    let after = after.trim_start_matches('\'').trim_start_matches('"');
    let tag: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if tag.is_empty() {
        None
    } else {
        Some(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_basic_unified_diff() {
        let buf = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,2 +1,3 @@\n hello\n-old\n+new\n+new2\n";
        let blocks = detect_blocks(buf);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].added, 2);
        assert_eq!(blocks[0].removed, 1);
    }

    #[test]
    fn ignores_heredoc_diff_like_content() {
        let buf = "cat <<EOF\n--- not a diff\n+++ not a diff\nbody\nEOF\nreal line";
        let blocks = detect_blocks(buf);
        assert_eq!(blocks.len(), 0);
    }
}
