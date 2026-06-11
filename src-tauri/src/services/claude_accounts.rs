// services/claude_accounts.rs — Universal CLI account registry (renamed from claude-only).
//
// Storage: tabla `claude_accounts` (mantiene el nombre para no romper PK), pero ahora
// soporta cli_kind ∈ { claude, codex, gemini, aider, openai-api, custom } y env_var
// asociado. Los wrappers ~/bin/claude-as-<slug> / ~/bin/codex-as-<slug> etc. exportan
// el env_var correcto desde Keychain antes de exec.

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::services::keychain;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Claude,
    Codex,
    Gemini,
    Aider,
    #[serde(rename = "openai-api")]
    OpenaiApi,
    Custom,
}

impl CliKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Aider => "aider",
            Self::OpenaiApi => "openai-api",
            Self::Custom => "custom",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "gemini" => Self::Gemini,
            "aider" => Self::Aider,
            "openai-api" => Self::OpenaiApi,
            "custom" => Self::Custom,
            _ => return None,
        })
    }
    /// Default env var that the wrapper exports before exec.
    pub fn default_env_var(&self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_CODE_OAUTH_TOKEN",
            Self::Codex => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Aider => "ANTHROPIC_API_KEY", // Aider config-driven; this is the most common
            Self::OpenaiApi => "OPENAI_API_KEY",
            Self::Custom => "API_KEY",
        }
    }
    /// Default Keychain service prefix for a given kind.
    pub fn default_service_prefix(&self) -> &'static str {
        match self {
            Self::Claude => "claude-max-",
            Self::Codex => "codex-cli-",
            Self::Gemini => "gemini-cli-",
            Self::Aider => "aider-",
            Self::OpenaiApi => "openai-api-",
            Self::Custom => "custom-",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeAccount {
    pub cli_kind: String,
    pub slug: String,
    pub label: String,
    pub browser: Option<String>,
    pub status: String, // "verified" | "unverified" | "missing_token"
    pub env_var: Option<String>,
    pub keychain_service: Option<String>,
    pub last_verified_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 32
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn list_all(db: &Arc<parking_lot::Mutex<Connection>>) -> Result<Vec<ClaudeAccount>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT cli_kind, slug, label, browser, status, env_var, keychain_service,
                last_verified_at, last_used_at, created_at, updated_at
         FROM claude_accounts ORDER BY cli_kind, created_at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ClaudeAccount {
            cli_kind: r.get(0)?,
            slug: r.get(1)?,
            label: r.get(2)?,
            browser: r.get(3)?,
            status: r.get(4)?,
            env_var: r.get(5)?,
            keychain_service: r.get(6)?,
            last_verified_at: r.get(7)?,
            last_used_at: r.get(8)?,
            created_at: r.get(9)?,
            updated_at: r.get(10)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        let mut acc = r?;
        // Codex HIGH fix B9.1: strip "<kind>:" prefix from storage slug for non-claude rows.
        // The storage uses "<kind>:<slug>" to avoid PK collision across kinds (DB legacy PK
        // was plain slug); the UI/wrappers expect just "<slug>".
        let prefix = format!("{}:", acc.cli_kind);
        if let Some(plain) = acc.slug.strip_prefix(&prefix) {
            acc.slug = plain.to_string();
        }
        out.push(acc);
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddRequest {
    pub slug: String,
    pub label: String,
    pub cli_kind: String, // "claude" | "codex" | ...
    pub browser: Option<String>,
    pub env_var: Option<String>,          // optional override
    pub keychain_service: Option<String>, // optional override
}

// Gemini MED fix B9.1: validate env_var and keychain_service overrides before they
// reach DB / wrappers. Both are used in shell scripts; loose values = injection vector.
fn valid_env_var(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase() || c == '_')
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn valid_keychain_service(svc: &str) -> bool {
    !svc.is_empty()
        && svc.len() <= 128
        && svc
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn valid_label(label: &str) -> bool {
    let trimmed = label.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 80
        // Reject control chars + single quote (defends sqlite3 shell injection from setup-account.sh)
        && trimmed.chars().all(|c| !c.is_control() && c != '\'' && c != '"' && c != ';')
}

pub fn add(db: &Arc<parking_lot::Mutex<Connection>>, req: AddRequest) -> Result<ClaudeAccount> {
    if !valid_slug(&req.slug) {
        return Err(anyhow!("invalid slug (allowed: [A-Za-z0-9_-]{{1,32}})"));
    }
    if !valid_label(&req.label) {
        return Err(anyhow!("invalid label (no control/quote chars, max 80)"));
    }
    let kind = CliKind::parse(&req.cli_kind)
        .ok_or_else(|| anyhow!("unknown cli_kind: {}", req.cli_kind))?;

    // Validate overrides BEFORE they reach DB / wrappers (Gemini MED fix B9.1).
    if let Some(ref e) = req.env_var {
        if !valid_env_var(e) {
            return Err(anyhow!(
                "invalid env_var '{}' (must be [A-Z_][A-Z0-9_]*)",
                e
            ));
        }
    }
    if let Some(ref s) = req.keychain_service {
        if !valid_keychain_service(s) {
            return Err(anyhow!(
                "invalid keychain_service '{}' (alfanum + . - _)",
                s
            ));
        }
    }

    let env_var = req
        .env_var
        .clone()
        .unwrap_or_else(|| kind.default_env_var().to_string());
    let keychain_service = req
        .keychain_service
        .clone()
        .unwrap_or_else(|| format!("{}{}", kind.default_service_prefix(), req.slug));

    let now = Utc::now().to_rfc3339();
    let user = keychain_user();
    let status = if keychain::load(&keychain_service, &user).is_some() {
        "unverified"
    } else {
        "missing_token"
    };

    {
        let conn = db.lock();
        // PK in DB is just `slug` (legacy). We disambiguate by (cli_kind, slug) on read
        // but the actual write key is slug — we now use composite "kind:slug" as DB slug
        // when there's a conflict. For migration safety: if cli_kind=claude, use plain slug.
        let storage_slug = if matches!(kind, CliKind::Claude) {
            req.slug.clone()
        } else {
            format!("{}:{}", kind.as_str(), req.slug)
        };
        conn.execute(
            "INSERT INTO claude_accounts
                (slug, cli_kind, label, browser, status, env_var, keychain_service,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT(slug) DO UPDATE SET
                cli_kind = excluded.cli_kind,
                label = excluded.label,
                browser = excluded.browser,
                status = excluded.status,
                env_var = excluded.env_var,
                keychain_service = excluded.keychain_service,
                updated_at = excluded.updated_at",
            params![
                storage_slug,
                kind.as_str(),
                req.label.trim(),
                req.browser,
                status,
                env_var,
                keychain_service,
                now,
            ],
        )?;
    }

    get(db, &req.cli_kind, &req.slug)?.ok_or_else(|| anyhow!("add round-trip read failed"))
}

pub fn get(
    db: &Arc<parking_lot::Mutex<Connection>>,
    cli_kind: &str,
    slug: &str,
) -> Result<Option<ClaudeAccount>> {
    let conn = db.lock();
    let storage_slug = if cli_kind == "claude" {
        slug.to_string()
    } else {
        format!("{}:{}", cli_kind, slug)
    };
    let mut stmt = conn.prepare(
        "SELECT cli_kind, slug, label, browser, status, env_var, keychain_service,
                last_verified_at, last_used_at, created_at, updated_at
         FROM claude_accounts WHERE slug = ?1 AND cli_kind = ?2",
    )?;
    let mut rows = stmt.query_map(params![storage_slug, cli_kind], |r| {
        Ok(ClaudeAccount {
            cli_kind: r.get(0)?,
            slug: r.get(1)?,
            label: r.get(2)?,
            browser: r.get(3)?,
            status: r.get(4)?,
            env_var: r.get(5)?,
            keychain_service: r.get(6)?,
            last_verified_at: r.get(7)?,
            last_used_at: r.get(8)?,
            created_at: r.get(9)?,
            updated_at: r.get(10)?,
        })
    })?;
    match rows.next() {
        Some(r) => {
            let mut acc = r?;
            // Strip the "kind:" prefix if present so the UI shows plain slug.
            if let Some(plain) = acc.slug.strip_prefix(&format!("{}:", cli_kind)) {
                acc.slug = plain.to_string();
            }
            Ok(Some(acc))
        }
        None => Ok(None),
    }
}

pub fn delete(
    db: &Arc<parking_lot::Mutex<Connection>>,
    cli_kind: &str,
    slug: &str,
) -> Result<bool> {
    if !valid_slug(slug) {
        return Err(anyhow!("invalid slug"));
    }
    let storage_slug = if cli_kind == "claude" {
        slug.to_string()
    } else {
        format!("{}:{}", cli_kind, slug)
    };
    let (svc, _kind) = {
        let conn = db.lock();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT keychain_service, cli_kind FROM claude_accounts WHERE slug = ?1",
                params![storage_slug],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                        r.get(1)?,
                    ))
                },
            )
            .ok();
        match row {
            Some((s, k)) => (s, k),
            None => return Ok(false),
        }
    };
    let n = {
        let conn = db.lock();
        conn.execute(
            "DELETE FROM claude_accounts WHERE slug = ?1",
            params![storage_slug],
        )?
    };
    if n > 0 && !svc.is_empty() {
        keychain::delete(&svc, &keychain_user());
    }
    Ok(n > 0)
}

pub fn mark_used(
    db: &Arc<parking_lot::Mutex<Connection>>,
    cli_kind: &str,
    slug: &str,
) -> Result<()> {
    let storage_slug = if cli_kind == "claude" {
        slug.to_string()
    } else {
        format!("{}:{}", cli_kind, slug)
    };
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    conn.execute(
        "UPDATE claude_accounts SET last_used_at = ?1, updated_at = ?1 WHERE slug = ?2",
        params![now, storage_slug],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyResult {
    pub slug: String,
    pub cli_kind: String,
    pub ok: bool,
    pub status: String,
    pub message: String,
}

pub fn verify(
    db: &Arc<parking_lot::Mutex<Connection>>,
    cli_kind: &str,
    slug: &str,
) -> Result<VerifyResult> {
    if !valid_slug(slug) {
        return Err(anyhow!("invalid slug"));
    }
    let kind = CliKind::parse(cli_kind).ok_or_else(|| anyhow!("unknown cli_kind: {}", cli_kind))?;
    let svc = match get(db, cli_kind, slug)? {
        Some(a) => a
            .keychain_service
            .unwrap_or_else(|| format!("{}{}", kind.default_service_prefix(), slug)),
        None => return Err(anyhow!("account not found: {}:{}", cli_kind, slug)),
    };
    let token_opt = keychain::load(&svc, &keychain_user());
    let (ok, status, message) = match token_opt {
        Some(t) if t.len() >= 16 => (
            true,
            "verified".to_string(),
            format!("Token en Keychain ({} chars)", t.len()),
        ),
        Some(_) => (
            false,
            "missing_token".to_string(),
            "Token muy corto — re-run setup".to_string(),
        ),
        None => (
            false,
            "missing_token".to_string(),
            format!("Sin Keychain entry '{}'", svc),
        ),
    };
    let now = Utc::now().to_rfc3339();
    let storage_slug = if cli_kind == "claude" {
        slug.to_string()
    } else {
        format!("{}:{}", cli_kind, slug)
    };
    let conn = db.lock();
    let verified_at = if ok { Some(&now) } else { None };
    conn.execute(
        "UPDATE claude_accounts SET status = ?1,
                last_verified_at = COALESCE(?2, last_verified_at),
                updated_at = ?3 WHERE slug = ?4",
        params![status, verified_at, now, storage_slug],
    )?;
    Ok(VerifyResult {
        slug: slug.to_string(),
        cli_kind: cli_kind.to_string(),
        ok,
        status,
        message,
    })
}

fn keychain_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "hernan".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(valid_slug("A"));
        assert!(valid_slug("work-personal"));
        assert!(!valid_slug(""));
        assert!(!valid_slug("has space"));
    }

    #[test]
    fn cli_kind_roundtrip() {
        for k in [
            CliKind::Claude,
            CliKind::Codex,
            CliKind::Gemini,
            CliKind::Aider,
            CliKind::OpenaiApi,
            CliKind::Custom,
        ] {
            assert_eq!(CliKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn cli_kind_env_vars() {
        assert_eq!(CliKind::Claude.default_env_var(), "CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(CliKind::Codex.default_env_var(), "OPENAI_API_KEY");
        assert_eq!(CliKind::Gemini.default_env_var(), "GEMINI_API_KEY");
    }
}
