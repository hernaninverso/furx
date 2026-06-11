// F15 — AIE cascade state indicator.
// Fetches /v1/resilience/state with Keychain bearer. SSRF allowlist: only the
// user-configured AIE endpoint (validated against settings) plus loopback; no
// infrastructure hosts are baked into the defaults (see bases/allowlist.rs, 041 FR-005).

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct AieStateSummary {
    pub enabled: bool,
    pub shadow_mode: bool,
    pub healthy_providers: Vec<String>,
    pub blocked_providers: Vec<String>,
    pub total_providers: usize,
}

pub async fn fetch_state(endpoint: &str) -> Result<AieStateSummary> {
    if !endpoint_allowed(endpoint) {
        return Err(anyhow!("aie endpoint not in allowlist: {}", endpoint));
    }
    let bearer = crate::services::keychain_bearer::get_bearer()
        .ok_or_else(|| anyhow!("missing Keychain entry aie-internal-bearer"))?;
    let url = format!("{}/v1/resilience/state", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let resp = client
        .get(&url)
        .bearer_auth(bearer)
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        // 039 — bearer rotation: a 401 means the cached bearer is stale; drop it so the next call
        // re-reads the rotated value from the Keychain.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            crate::services::keychain_bearer::invalidate_bearer_cache();
        }
        return Err(anyhow!("aie status {}", status));
    }
    let v: serde_json::Value = resp.json().await?;
    let enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false);
    let shadow_mode = v
        .get("shadow_mode")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let rate_limits = v
        .get("rate_limits")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let mut healthy = Vec::new();
    let mut blocked = Vec::new();
    // 2026-05-28 fix: AIE sometimes returns `blocked_until` as an empty string ""
    // or a past timestamp (cooldown already passed). Treat both as healthy. Only
    // FUTURE timestamps count as "currently blocked" — otherwise the UI surfaces
    // a stale "N blocked" indicator that nags the user about nothing.
    let now = chrono::Utc::now();
    for rl in &rate_limits {
        let provider = rl
            .get("provider")
            .and_then(|s| s.as_str())
            .unwrap_or("?")
            .to_string();
        let is_currently_blocked = rl
            .get("blocked_until")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc) > now)
            .unwrap_or(false);
        if is_currently_blocked {
            blocked.push(provider);
        } else {
            healthy.push(provider);
        }
    }
    // Also consider open circuits (status == "open") as blocked — that's a
    // different dimension of "unavailable" the prior code missed entirely.
    if let Some(circuits) = v.get("circuits").and_then(|x| x.as_array()) {
        for c in circuits {
            let status = c.get("status").and_then(|s| s.as_str()).unwrap_or("");
            if status == "open" {
                if let Some(provider) = c.get("provider").and_then(|s| s.as_str()) {
                    blocked.push(provider.to_string());
                }
            }
        }
    }
    healthy.sort();
    healthy.dedup();
    blocked.sort();
    blocked.dedup();
    Ok(AieStateSummary {
        enabled,
        shadow_mode,
        total_providers: healthy.len() + blocked.len(),
        healthy_providers: healthy,
        blocked_providers: blocked,
    })
}

fn endpoint_allowed(endpoint: &str) -> bool {
    crate::bases::allowlist::url_allowed(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_blocks_external_hosts() {
        // 041 FR-005 — only loopback is allowed by default; a user-configured host (e.g. a Tailscale
        // AIE) is allowed once registered as a runtime origin, NOT baked in as a default.
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        assert!(!endpoint_allowed("http://evil.com:8250"));
        assert!(!endpoint_allowed("file:///etc/passwd"));
        assert!(endpoint_allowed("http://localhost:8250"));
        // el autor's old Tailscale default is NOT allowed by default anymore.
        assert!(!endpoint_allowed("http://100.64.0.10:8250"));
        // …but once the user configures it, it's allowed (no range-block on 100.x).
        crate::bases::allowlist::add_runtime_origin("http://100.64.0.10:8250").unwrap();
        assert!(endpoint_allowed("http://100.64.0.10:8250"));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn allowlist_blocks_subdomain_attack() {
        // Critical: the ultra-review HIGH finding (V1+V2). Realistic attack vectors are FQDNs where
        // the attacker controls the actual root (e.g. attacker.com), and a Furx-trusted token
        // appears as a prefix. A configured EXACT host must not leak to a look-alike subdomain.
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        crate::bases::allowlist::add_runtime_origin("http://100.64.0.10:8250").unwrap();
        assert!(!endpoint_allowed("https://example.test.attacker.com"));
        assert!(!endpoint_allowed("https://100.64.0.10.attacker.com"));
        assert!(!endpoint_allowed("https://example.internal.evil.host"));
        assert!(!endpoint_allowed("https://100.64.0.10.attacker.com:8250"));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }
}
