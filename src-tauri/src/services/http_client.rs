// 2.27 — Built-in HTTP client (Postman-lite).
// Council: allowlist-aware → solo permitir hosts internos por default
// (toggle via setting allow_external_http).

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use uuid::Uuid;

/// Max response body we buffer (defence against an attacker-controlled multi-GB body).
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// True for loopback / private / link-local / ULA / unspecified addresses — the
/// ranges an SSRF would target (incl. cloud metadata 169.254.169.254). Blocked even
/// in `allow_external` mode: that mode is for PUBLIC hosts.
fn ip_is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_is_internal(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80 // ULA fc00::/7, link-local fe80::/10
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub allow_external: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
    pub elapsed_ms: u64,
    pub bytes: usize,
}

pub async fn send(db: &Mutex<Connection>, req: HttpRequest) -> Result<HttpResponse> {
    if req.url.len() > 2048 {
        return Err(anyhow!("URL too long"));
    }
    let parsed = url::Url::parse(&req.url).map_err(|e| anyhow!("bad URL: {}", e))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(anyhow!("scheme not allowed: {}", scheme));
    }
    let allow_ext = req.allow_external.unwrap_or(false);
    if !allow_ext && !crate::bases::allowlist::url_allowed(&req.url) {
        return Err(anyhow!(
            "URL not in internal allowlist; pass allow_external=true to call public hosts"
        ));
    }
    // SSRF guard: in external mode (allowlist bypassed) the target must be a PUBLIC
    // address. Resolve the host, refuse if any resolved IP is internal/private, and PIN
    // the first verified-public IP into the client so reqwest cannot re-resolve to a
    // private address at connect time (closes the DNS-rebinding window).
    let mut pinned: Option<(String, std::net::SocketAddr)> = None;
    if allow_ext {
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow!("URL has no host"))?
            .to_string();
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| anyhow!("DNS resolution failed: {}", e))?
            .collect();
        if addrs.is_empty() {
            return Err(anyhow!("host did not resolve"));
        }
        for sa in &addrs {
            if ip_is_internal(sa.ip()) {
                return Err(anyhow!(
                    "refusing to call internal/private address {} (SSRF guard)",
                    sa.ip()
                ));
            }
        }
        pinned = Some((host, addrs[0]));
    }
    let mut client_builder = reqwest::Client::builder().timeout(Duration::from_secs(20));
    if let Some((host, addr)) = &pinned {
        client_builder = client_builder.resolve(host, *addr);
    }
    let client = client_builder.build()?;
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|e| anyhow!("bad method: {}", e))?;
    let mut builder = client.request(method, &req.url);
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("host") {
            continue;
        }
        builder = builder.header(k, v);
    }
    if let Some(b) = &req.body {
        builder = builder.body(b.clone());
    }
    let started = std::time::Instant::now();
    let mut resp = builder.send().await?;
    let status = resp.status().as_u16();
    let headers: HashMap<String, String> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Bounded read: never buffer more than MAX_BODY_BYTES regardless of Content-Length
    // (an attacker-controlled endpoint could stream gigabytes). Truncates past the cap.
    let mut body_bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = MAX_BODY_BYTES - body_bytes.len();
        if chunk.len() >= room {
            body_bytes.extend_from_slice(&chunk[..room]);
            break;
        }
        body_bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let bytes = body_bytes.len();
    // Persist history (without body).
    let id = Uuid::new_v4().to_string();
    db.lock().execute(
        "INSERT INTO http_history (id, method, url, status, elapsed_ms, bytes) VALUES (?, ?, ?, ?, ?, ?)",
        params![id, req.method, req.url, status as i64, elapsed_ms as i64, bytes as i64],
    ).ok();
    Ok(HttpResponse {
        status,
        headers,
        body,
        elapsed_ms,
        bytes,
    })
}
