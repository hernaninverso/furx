// F23 — Telegram bridge (outbound only).
// Posts critical cards to the user's configured relay (settings.endpoints.telegram_relay)
// with HMAC-SHA256 + nonce. NO local HTTP callback server in this sprint
// (council V1/V4 flagged worker-leak + bind 0.0.0.0 risk). Inbound is opt-in
// future work; this module only signs and POSTs.

use anyhow::{anyhow, Result};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize)]
pub struct TelegramSend {
    pub endpoint: String,
    pub status: u16,
    pub nonce: String,
    pub bytes: usize,
}

pub fn endpoint_allowed(url: &str) -> bool {
    crate::bases::allowlist::url_allowed(url)
}

/// Build the canonical signing payload — `nonce.timestamp.body`.
fn canonical(nonce: &str, ts: i64, body: &str) -> String {
    format!("{}.{}.{}", nonce, ts, body)
}

/// Sign a payload with the user's HMAC secret. Returns hex digest.
pub fn sign(secret: &str, nonce: &str, ts: i64, body: &str) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| anyhow!("invalid hmac key: {}", e))?;
    mac.update(canonical(nonce, ts, body).as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub async fn post_card(
    endpoint: &str,
    secret: &str,
    card_id: &str,
    title: &str,
    severity: &str,
) -> Result<TelegramSend> {
    if !endpoint_allowed(endpoint) {
        return Err(anyhow!("endpoint not in allowlist: {}", endpoint));
    }
    let nonce = uuid::Uuid::new_v4().to_string();
    let ts = chrono::Utc::now().timestamp();
    let body = serde_json::to_string(&serde_json::json!({
        "type": "furx.card",
        "card_id": card_id,
        "title": title,
        "severity": severity,
    }))?;
    let sig = sign(secret, &nonce, ts, &body)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header("X-Furx-Nonce", &nonce)
        .header("X-Furx-Ts", ts.to_string())
        .header("X-Furx-Sig", &sig)
        .body(body.clone())
        .send()
        .await?;
    Ok(TelegramSend {
        endpoint: endpoint.to_string(),
        status: resp.status().as_u16(),
        nonce,
        bytes: body.len(),
    })
}

/// Read HMAC secret from macOS Keychain entry `furx-telegram-hmac`.
///
/// 041 FR-006 — the account is the validated current user (`identity::keychain_account()`), matching
/// what `signals_set_telegram_secret` writes. If that account has no entry AND it isn't already the
/// legacy `hernan` account, we fall back to reading `hernan` ONCE so el autor's pre-existing entry
/// keeps working after this change (documented legacy fallback; logged, never the secret).
pub fn read_secret() -> Option<String> {
    let account = crate::services::identity::keychain_account();
    if let Some(s) = read_secret_for_account(&account) {
        return Some(s);
    }
    if account != crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT {
        if let Some(s) = read_secret_for_account(crate::services::identity::LEGACY_KEYCHAIN_ACCOUNT)
        {
            tracing::warn!(
                user = %account,
                "furx-telegram-hmac not found for USER account; using legacy 'hernan' account"
            );
            return Some(s);
        }
    }
    None
}

fn read_secret_for_account(account: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            account,
            "-s",
            "furx-telegram-hmac",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_blocks_external() {
        // 041 FR-005 — loopback allowed by default; a relay host the user configures is allowed once
        // registered, NOT baked in (no infrastructure relay host is a default anymore).
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        assert!(!endpoint_allowed("http://attacker.com"));
        assert!(endpoint_allowed("http://localhost:8400"));
        assert!(!endpoint_allowed("https://relay.example.io"));
        crate::bases::allowlist::add_runtime_origin("https://relay.example.io:443").unwrap();
        assert!(endpoint_allowed("https://relay.example.io"));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn signs_deterministically() {
        let a = sign("secret", "n1", 1000, r#"{"x":1}"#).unwrap();
        let b = sign("secret", "n1", 1000, r#"{"x":1}"#).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64); // sha256 hex
    }

    #[test]
    fn different_nonce_changes_sig() {
        let a = sign("secret", "n1", 1000, "body").unwrap();
        let b = sign("secret", "n2", 1000, "body").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn read_secret_for_missing_account_is_none_no_panic() {
        // 041 FR-006 — reading the HMAC under a deterministically-absent account must fail-closed
        // (None), never panic. This exercises the per-account read path the multi-user resolution
        // (current account → legacy `hernan`) is built on, without depending on a real entry.
        let absent = format!("furx-telegram-absent-{}", std::process::id());
        assert_eq!(read_secret_for_account(&absent), None);
    }
}
