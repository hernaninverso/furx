// F32 Secret Guardrail — pre-flight check para outbound payloads.
// Si el payload contiene un patrón sospechoso de secreto, lo bloquea y emite audit event.
// Usado por telemetry (F45), export (F43), update-check (F44).

use once_cell::sync::Lazy;
use regex::Regex;

pub struct SecretFinding {
    pub pattern_id: &'static str,
    pub sample: String,
}

static PATTERNS: Lazy<Vec<(&'static str, Regex)>> = Lazy::new(|| {
    vec![
        ("aws_access_key", Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
        ("github_pat", Regex::new(r"ghp_[A-Za-z0-9]{36,}").unwrap()),
        ("github_token", Regex::new(r"gho_[A-Za-z0-9]{36,}").unwrap()),
        (
            "anthropic_key",
            Regex::new(r"sk-ant-[A-Za-z0-9_-]{32,}").unwrap(),
        ),
        ("openai_key", Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap()),
        // OpenAI project keys (sk-proj-…) need the hyphen; a dedicated pattern keeps it
        // from overlapping the more specific sk-ant-/sk-or- categories above (L5).
        (
            "openai_project_key",
            Regex::new(r"sk-proj-[A-Za-z0-9_-]{20,}").unwrap(),
        ),
        (
            "openrouter_key",
            Regex::new(r"sk-or-[A-Za-z0-9-]{20,}").unwrap(),
        ),
        (
            "slack_token",
            Regex::new(r"xox[abprs]-[A-Za-z0-9-]{10,}").unwrap(),
        ),
        (
            "private_key_pem",
            Regex::new(r"-----BEGIN (RSA |EC |OPENSSH |)PRIVATE KEY-----").unwrap(),
        ),
        (
            "jwt",
            Regex::new(r"eyJ[A-Za-z0-9_-]{20,}\.eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}")
                .unwrap(),
        ),
        (
            "password_literal",
            Regex::new(r#"(?i)password\s*[:=]\s*["'][^"']{6,}["']"#).unwrap(),
        ),
    ]
});

pub fn scan(payload: &str) -> Vec<SecretFinding> {
    let mut out = Vec::new();
    for (id, re) in PATTERNS.iter() {
        if let Some(m) = re.find(payload) {
            // Truncate sample to keep audit logs from leaking the secret itself.
            let s = m.as_str();
            let masked = if s.len() > 12 {
                format!("{}…{}", &s[..6], "*".repeat(s.len().saturating_sub(6)))
            } else {
                "*".repeat(s.len())
            };
            out.push(SecretFinding {
                pattern_id: id,
                sample: masked,
            });
        }
    }
    out
}

/// Redact every match of every pattern in-place. Used by F21 bundle save flow.
/// Returns (redacted_text, list_of_pattern_ids_that_matched).
pub fn redact(payload: &str) -> (String, Vec<&'static str>) {
    let mut out = payload.to_string();
    let mut matched = Vec::new();
    for (id, re) in PATTERNS.iter() {
        if re.is_match(&out) {
            matched.push(*id);
            out = re
                .replace_all(&out, format!("‹redacted:{}›", id).as_str())
                .into_owned();
        }
    }
    (out, matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_anthropic() {
        let findings = scan("Bearer sk-ant-abcdefghijklmnopqrstuvwxyz0123456789AB");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].pattern_id, "anthropic_key");
    }

    #[test]
    fn clean_payload_passes() {
        assert!(scan("hello world, no secrets here").is_empty());
    }
}
