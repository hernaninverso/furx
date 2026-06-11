// F12 — Smart paste detection. Stateless classifier over a clipboard string.
// Detects: stack traces, URLs, unified diffs, JSON, error blobs.
// Used by the UI to suggest "Send to focused Claude as bug report".
//
// BLOQUE D · 2026-05-27: classifier surface unchanged; the auto-poll loop
// lives in the frontend (useEffect over clipboard_read) so we don't bake a
// per-user toggle into Rust state. `should_offer_paste()` is the shared
// "is this worth interrupting the user?" gate.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PasteKind {
    StackTrace,
    Url,
    Diff,
    Json,
    Error,
    Code,
    Plain,
}

#[derive(Debug, Clone, Serialize)]
pub struct PasteClassification {
    pub kind: PasteKind,
    pub bytes: usize,
    pub lines: usize,
    pub preview: String,
    pub action_hint: String,
}

static RE_STACK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?m)^(\s*at\s+[\w$.<>]+\(|Traceback \(most recent|thread\s+['\x22].+['\x22]\s+panicked|panicked at|File\s+['\x22][^'\x22]+['\x22],\s+line\s+\d+)").unwrap()
});
static RE_URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^https?://\S+$").unwrap());
static RE_DIFF: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^(---\s+|\+\+\+\s+|@@\s+-)").unwrap());
static RE_ERROR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?im)^\s*(error[: ]|fatal:|panic:)").unwrap());

pub fn classify(text: &str) -> PasteClassification {
    let s = text.trim();
    let lines = s.lines().count();
    let bytes = s.len();
    let preview = s.chars().take(200).collect::<String>();
    let kind = decide(s);
    let action_hint = hint_for(&kind);
    PasteClassification {
        kind,
        bytes,
        lines,
        preview,
        action_hint,
    }
}

/// BLOQUE D · F12 auto-poll gate: returns `true` if a clipboard tick is worth
/// surfacing as a toast. Avoids noise from short selection/copies and from
/// plain text (most clipboard usage). Frontend additionally tracks "last
/// offered" hashes so the same payload doesn't re-toast.
pub fn should_offer_paste(c: &PasteClassification) -> bool {
    if c.bytes < 50 || c.bytes > 32 * 1024 {
        return false;
    }
    !matches!(c.kind, PasteKind::Plain)
}

fn decide(s: &str) -> PasteKind {
    if RE_URL.is_match(s.trim()) {
        return PasteKind::Url;
    }
    if RE_STACK.is_match(s) {
        return PasteKind::StackTrace;
    }
    if RE_DIFF.is_match(s) {
        return PasteKind::Diff;
    }
    let trimmed = s.trim();
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return PasteKind::Json;
        }
    if RE_ERROR.is_match(s) {
        return PasteKind::Error;
    }
    // Heuristic for "looks like code": ≥2 lines with leading whitespace and any of {;, =>, ->, def , fn , function, class, import, use }.
    let code_markers = [
        "fn ",
        "def ",
        "function ",
        "class ",
        "import ",
        "use ",
        " => ",
        "->",
        "<-",
        "return ",
        "raise ",
        "throw ",
    ];
    let lower = s.to_lowercase();
    let mut markers_hit = 0;
    for m in code_markers {
        if lower.contains(m) {
            markers_hit += 1;
        }
    }
    if markers_hit >= 2 && s.lines().count() >= 2 {
        return PasteKind::Code;
    }
    PasteKind::Plain
}

fn hint_for(kind: &PasteKind) -> String {
    match kind {
        PasteKind::StackTrace => "Send to focused Claude as crash report".into(),
        PasteKind::Url => "Open in browser / send to Claude as context".into(),
        PasteKind::Diff => "Inspect / paste into Codex review".into(),
        PasteKind::Json => "Pretty-print / send to Claude with schema ask".into(),
        PasteKind::Error => "Send to focused Claude as bug report".into(),
        PasteKind::Code => "Send to focused Claude with 'review this' framing".into(),
        PasteKind::Plain => "Send to focused pane".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_python_traceback() {
        let s = "Traceback (most recent call last):\n  File \"x.py\", line 3, in <module>\n    raise ValueError";
        assert_eq!(classify(s).kind, PasteKind::StackTrace);
    }

    #[test]
    fn detects_diff() {
        let s = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1,3 +1,4 @@\n hello";
        assert_eq!(classify(s).kind, PasteKind::Diff);
    }

    #[test]
    fn detects_json() {
        let s = r#"{"hello": "world", "n": 42}"#;
        assert_eq!(classify(s).kind, PasteKind::Json);
    }

    #[test]
    fn detects_url() {
        assert_eq!(classify("https://github.com/eleata").kind, PasteKind::Url);
    }

    #[test]
    fn plain_text_is_plain() {
        assert_eq!(
            classify("hello world from a normal sentence").kind,
            PasteKind::Plain
        );
    }

    #[test]
    fn should_offer_paste_skips_plain_and_short() {
        let plain = classify("hello world");
        assert!(!should_offer_paste(&plain));
        // short stack trace still wouldn't qualify on size
        let short_trace = classify("Traceback");
        assert!(!should_offer_paste(&short_trace));
        // long enough stack trace → yes
        let long_trace = classify("Traceback (most recent call last):\n  File \"x.py\", line 3, in <module>\n    raise ValueError\n  File \"y.py\", line 4\n  File \"z.py\", line 5");
        assert!(should_offer_paste(&long_trace));
    }
}
