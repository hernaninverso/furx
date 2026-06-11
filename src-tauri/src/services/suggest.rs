// F9 — Suggested next action heuristic.
// Stateless: caller passes the recent PTY output, we return at most one suggestion
// based on simple regex over the last ~50 lines.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub kind: String, // "error" | "test-pass" | "merge-conflict" | "prompt" | "build-ok"
    pub label: String, // short button label
    pub hint: String, // sentence for tooltip
}

static RE_ERROR: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?im)(^|\s)(error[: ]|panic!|panicked\s+at|traceback|fatal:|\bFAIL\b|✗\s)")
        .unwrap()
});
static RE_TESTS_PASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?im)\b(\d+\s+(passed|tests?\s+ok|tests?\s+passed)|all tests passed)\b").unwrap()
});
static RE_MERGE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?im)\b(CONFLICT|merge conflict|<<<<<<< HEAD)\b").unwrap());
static RE_PROMPT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[y/n\]|proceed\?|continue\?|approve\?").unwrap());
static RE_BUILD_OK: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?im)\b(build succeeded|finished `release`|compiled successfully|Built in)\b")
        .unwrap()
});

pub fn suggest(text: &str) -> Option<Suggestion> {
    // Take the last 8KB only — heuristic should react to recent output, not history.
    // UTF-8 safe: slicing at an arbitrary byte index can split a multibyte char (e.g. ─ U+2500)
    // and panic. Find the next char boundary ≥ the target offset.
    let tail = if text.len() > 8192 {
        let mut idx = text.len() - 8192;
        while idx < text.len() && !text.is_char_boundary(idx) {
            idx += 1;
        }
        &text[idx..]
    } else {
        text
    };
    if RE_MERGE.is_match(tail) {
        return Some(Suggestion {
            kind: "merge-conflict".into(),
            label: "Resolve".into(),
            hint: "Conflict markers detected — open editor / abort".into(),
        });
    }
    if RE_PROMPT.is_match(tail) {
        return Some(Suggestion {
            kind: "prompt".into(),
            label: "Answer".into(),
            hint: "Pane is waiting for yes/no input".into(),
        });
    }
    if RE_ERROR.is_match(tail) {
        return Some(Suggestion {
            kind: "error".into(),
            label: "Investigate".into(),
            hint: "Error/panic/traceback in recent output".into(),
        });
    }
    if RE_TESTS_PASS.is_match(tail) {
        return Some(Suggestion {
            kind: "test-pass".into(),
            label: "Commit?".into(),
            hint: "Tests passed — consider a commit".into(),
        });
    }
    if RE_BUILD_OK.is_match(tail) {
        return Some(Suggestion {
            kind: "build-ok".into(),
            label: "Run".into(),
            hint: "Build succeeded — start the binary?".into(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_panic() {
        let s = suggest("compiling..\nthread 'main' panicked at 'oops'").unwrap();
        assert_eq!(s.kind, "error");
    }

    #[test]
    fn detects_tests_passed() {
        let s = suggest("running 17 tests\ntest result: ok. 17 passed; 0 failed").unwrap();
        assert_eq!(s.kind, "test-pass");
    }

    #[test]
    fn detects_merge_conflict() {
        let s = suggest("CONFLICT (content): Merge conflict in src/x.rs").unwrap();
        assert_eq!(s.kind, "merge-conflict");
    }

    #[test]
    fn clean_text_returns_none() {
        assert!(suggest("hello there, all good\nfinished some task").is_none());
    }

    #[test]
    fn prompt_outranks_error() {
        let s = suggest("error: blah\nproceed?").unwrap();
        // Merge > prompt > error precedence — verify here only prompt+error: prompt wins.
        assert_eq!(s.kind, "prompt");
    }
}
