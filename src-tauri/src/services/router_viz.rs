// 2.10 / W4 — Cost-aware router visualizer. Wrapper sobre /v1/resilience/state
// que devuelve estructura de árbol cascade (tier → providers → models).

use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct CascadeProvider {
    pub provider: String,
    pub model: String,
    pub blocked_until: Option<String>,
    pub bucket_used: u64,
    pub bucket_limit: u64,
    pub dimension: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CascadeSnapshot {
    pub enabled: bool,
    pub shadow_mode: bool,
    pub fetched_at: String,
    pub providers: Vec<CascadeProvider>,
}

pub async fn fetch(endpoint: &str, bearer: &str) -> Result<CascadeSnapshot> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let url = format!("{}/v1/resilience/state", endpoint.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .bearer_auth(bearer)
        .header("Accept", "application/json")
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("AIE {}", resp.status()));
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
    let providers: Vec<CascadeProvider> = rate_limits
        .iter()
        .map(|rl| CascadeProvider {
            provider: rl
                .get("provider")
                .and_then(|s| s.as_str())
                .unwrap_or("?")
                .to_string(),
            model: rl
                .get("model")
                .and_then(|s| s.as_str())
                .unwrap_or("?")
                .to_string(),
            blocked_until: rl
                .get("blocked_until")
                .and_then(|s| s.as_str())
                .map(String::from),
            bucket_used: rl.get("bucket_used").and_then(|s| s.as_u64()).unwrap_or(0),
            bucket_limit: rl.get("bucket_limit").and_then(|s| s.as_u64()).unwrap_or(0),
            dimension: rl
                .get("dimension")
                .and_then(|s| s.as_str())
                .unwrap_or("?")
                .to_string(),
        })
        .collect();
    Ok(CascadeSnapshot {
        enabled,
        shadow_mode,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        providers,
    })
}
