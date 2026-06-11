//! Cloud sanitizer — first-pass redactor that runs in the Tauri client BEFORE
//! the trace payload is uploaded to api.furx.cloud.
//!
//! Mirror of `cloud/src/lib/sanitizer.ts` patterns. The Worker runs a second pass;
//! both passes must agree for the BYOK invariant (F-I) to hold.
//!
//! Patterns covered: sk-*, Bearer tokens, RFC5322 emails, IBAN, AWS ARN, UUID v4.
//! Redaction format: `[REDACTED:<pattern>]`.
//!
//! NFR-2: < 1% false-negative rate. Unit tests in `tests::*` mirror the Worker's
//! sanitizer.test.ts inputs to guarantee both implementations agree.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SanitizerReport {
    pub total_hits: u32,
    pub by_pattern: BTreeMap<String, u32>,
}

static REGEXES: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "sk",
            Regex::new(r"\bsk-(?:[a-zA-Z]+-)?[A-Za-z0-9_-]{16,}\b").unwrap(),
        ),
        (
            "bearer",
            Regex::new(r"\bBearer\s+[A-Za-z0-9._~+/=-]{20,}\b").unwrap(),
        ),
        (
            "email",
            Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").unwrap(),
        ),
        (
            "iban",
            Regex::new(r"\b[A-Z]{2}\d{2}[A-Z0-9]{10,30}\b").unwrap(),
        ),
        (
            "arn",
            Regex::new(r"\barn:aws:[a-z0-9-]+:[a-z0-9-]*:[a-z0-9-]*:[A-Za-z0-9_/.:-]+\b").unwrap(),
        ),
        (
            "guid",
            Regex::new(
                r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b",
            )
            .unwrap(),
        ),
    ]
});

/// Apply all redaction patterns. Returns (sanitized_text, report).
pub fn sanitize(input: &str) -> (String, SanitizerReport) {
    let mut out = input.to_string();
    let mut report = SanitizerReport::default();
    for (name, re) in REGEXES.iter() {
        let mut local_hits = 0u32;
        out = re
            .replace_all(&out, |_caps: &regex::Captures| {
                local_hits += 1;
                format!("[REDACTED:{}]", name)
            })
            .to_string();
        if local_hits > 0 {
            report.by_pattern.insert((*name).to_string(), local_hits);
            report.total_hits += local_hits;
        }
    }
    (out, report)
}

/// Boundary-relaxed variants of the secret patterns, used ONLY by `secret_match_ranges`
/// (NOT by `sanitize`). The leading `\b` is dropped so a secret whose start abuts a
/// word char of a *previous line* — after newlines are removed to reconstruct a
/// split secret — is still located. Safe for DETECTION: callers (`scrub_buffer`)
/// redact conservatively (whole lines), so a slightly looser left edge cannot leak.
/// The trailing `\b` (and the rest of each pattern) is unchanged.
static REGEXES_NOLEAD: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        (
            "sk",
            Regex::new(r"sk-(?:[a-zA-Z]+-)?[A-Za-z0-9_-]{16,}\b").unwrap(),
        ),
        (
            "bearer",
            Regex::new(r"Bearer\s+[A-Za-z0-9._~+/=-]{20,}\b").unwrap(),
        ),
        (
            "email",
            Regex::new(r"(?i)[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").unwrap(),
        ),
        (
            "iban",
            Regex::new(r"[A-Z]{2}\d{2}[A-Z0-9]{10,30}\b").unwrap(),
        ),
        (
            "arn",
            Regex::new(r"arn:aws:[a-z0-9-]+:[a-z0-9-]*:[a-z0-9-]*:[A-Za-z0-9_/.:-]+\b").unwrap(),
        ),
        (
            "guid",
            Regex::new(
                r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b",
            )
            .unwrap(),
        ),
    ]
});

/// A secret match found by one of the redaction patterns: which pattern, and its
/// byte range in the input. Used by callers that need to map matches back onto a
/// different representation of the text (e.g. de-newlined views — see
/// `memory_autocapture::scrub_buffer`, which catches secrets split across lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub pattern: &'static str,
    pub start: usize,
    pub end: usize,
}

/// Return the byte ranges of every secret match in `input`, across all patterns,
/// using the boundary-relaxed pattern set. Does NOT redact — just locates. Ranges
/// are over `input`'s own bytes (each pattern scanned independently against the
/// original, so ranges may overlap between patterns; callers redact conservatively
/// and tolerate overlap).
pub fn secret_match_ranges(input: &str) -> Vec<SecretMatch> {
    let mut out = Vec::new();
    for (name, re) in REGEXES_NOLEAD.iter() {
        for m in re.find_iter(input) {
            out.push(SecretMatch {
                pattern: name,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_sk() {
        let (out, r) = sanitize("my key is sk-proj-Abc123_def-XYZ987_more thanks");
        assert!(out.contains("[REDACTED:sk]"));
        assert!(!out.contains("sk-proj-Abc123"));
        assert_eq!(r.by_pattern.get("sk"), Some(&1));
    }

    #[test]
    fn redacts_anthropic_sk() {
        let (out, _) = sanitize("sk-ant-api03-aAaAaAaAaAaAaAaAaAaAaAaAaA-foo");
        assert!(out.contains("[REDACTED:sk]"));
    }

    #[test]
    fn redacts_bearer() {
        let (out, r) = sanitize("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc");
        assert!(out.contains("[REDACTED:bearer]"));
        assert_eq!(r.by_pattern.get("bearer"), Some(&1));
    }

    #[test]
    fn redacts_two_emails() {
        let (out, r) = sanitize("contact john.doe@example.com or jane_smith@sub.example.org");
        assert_eq!(out.matches("[REDACTED:email]").count(), 2);
        assert_eq!(r.by_pattern.get("email"), Some(&2));
    }

    #[test]
    fn redacts_aws_arn() {
        let (out, r) = sanitize(
            "arn:aws:s3:::123456789012:bucket/my-bucket and arn:aws:iam:us-east-1:000000000000:role/Admin",
        );
        assert_eq!(out.matches("[REDACTED:arn]").count(), 2);
        assert_eq!(r.by_pattern.get("arn"), Some(&2));
    }

    #[test]
    fn redacts_iban() {
        let (out, r) = sanitize("Send to DE89370400440532013000 or FR1420041010050500013M02606");
        assert_eq!(out.matches("[REDACTED:iban]").count(), 2);
        assert_eq!(r.by_pattern.get("iban"), Some(&2));
    }

    #[test]
    fn redacts_uuid_v4() {
        let (out, _) = sanitize("user 550e8400-e29b-41d4-a716-446655440000 logged in");
        assert!(out.contains("[REDACTED:guid]"));
    }

    #[test]
    fn no_pii_no_changes() {
        let txt = "function add(a, b) { return a + b; } // simple math";
        let (out, r) = sanitize(txt);
        assert_eq!(out, txt);
        assert_eq!(r.total_hits, 0);
    }

    #[test]
    fn total_hits_sum() {
        let (_, r) = sanitize("sk-test-abcdefghijklmnop and john@example.com");
        assert_eq!(r.total_hits, 2);
    }

    #[test]
    fn empty_input() {
        let (out, r) = sanitize("");
        assert_eq!(out, "");
        assert_eq!(r.total_hits, 0);
    }
}
