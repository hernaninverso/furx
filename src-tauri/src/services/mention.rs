// 1.5 — @mention pane handoff.
// "@codex: foo" → route via PaneInputRouter al pane con mode=codex.
// Council V1: regex strict, no exec. V3: idempotent (correlation_id).
// V4: edge cases — empty body, mention sin destino, dest mode no presente.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

static MENTION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^@(claude-A|claude-B|codex|gemini|aider|zsh)\s*:\s*(.+)$").unwrap());

#[derive(Debug, Clone, Serialize)]
pub struct MentionRoute {
    pub target_mode: String,
    pub body: String,
}

pub fn parse(input: &str) -> Option<MentionRoute> {
    let line = input.trim();
    let caps = MENTION_RE.captures(line)?;
    Some(MentionRoute {
        target_mode: caps.get(1)?.as_str().to_string(),
        body: caps.get(2)?.as_str().trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_mention() {
        let r = parse("@codex: fix this bug").unwrap();
        assert_eq!(r.target_mode, "codex");
        assert_eq!(r.body, "fix this bug");
    }

    #[test]
    fn parses_claude_a_mention() {
        let r = parse("@claude-A: write tests").unwrap();
        assert_eq!(r.target_mode, "claude-A");
    }

    #[test]
    fn rejects_no_mention() {
        assert!(parse("just normal text").is_none());
        assert!(parse("email@gmail.com").is_none());
    }

    #[test]
    fn rejects_unknown_mode() {
        assert!(parse("@grok: hi").is_none());
        assert!(parse("@; rm -rf /: pwn").is_none());
    }

    #[test]
    fn handles_extra_whitespace() {
        let r = parse("@gemini :   hello world  ").unwrap();
        assert_eq!(r.target_mode, "gemini");
        assert_eq!(r.body, "hello world");
    }
}
