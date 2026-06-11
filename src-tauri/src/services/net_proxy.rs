// spec-kit 004 · per-host network allowlist for plugins.
//
// A tiny HTTP CONNECT proxy bound to 127.0.0.1:<random>. A plugin granted
// `net:["api.github.com"]` runs with HTTP_PROXY/HTTPS_PROXY pointing here AND a
// sandbox that blocks all direct egress except this loopback port (see plugin_host)
// — so the ONLY way out is through this proxy, which enforces the signed allowlist.
//
// Council (codex+gemini+3 frontier, unanimous): the host resolves DNS (no plugin
// DNS rebinding); internal/SSRF ranges are blocked; MVP is HTTPS CONNECT (:443).
// Exfil via a subdomain of an allowed host is an accepted residual (no DPI).

use anyhow::{anyhow, Result};
use base64::Engine;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Canonicalize a host for allowlist comparison: lowercase, strip a trailing dot,
/// strip a :port if present, strip surrounding brackets for IPv6 literals.
fn canon_host(h: &str) -> String {
    let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
    // strip :port only if not an unbracketed IPv6 (which contains many colons)
    let h = if h.starts_with('[') {
        // [::1]:443 → ::1
        h.trim_start_matches('[').split(']')
            .next()
            .unwrap_or(&h)
            .to_string()
    } else if h.matches(':').count() == 1 {
        h.split(':').next().unwrap_or(&h).to_string()
    } else {
        h
    };
    h
}

/// Is `host` in the allowlist? Exact canonical match (no wildcards in v1 beyond the
/// caller's "*" which is handled before reaching the proxy).
pub fn host_allowed(host: &str, allowlist: &[String]) -> bool {
    let h = canon_host(host);
    allowlist.iter().any(|a| canon_host(a) == h)
}

/// Block internal / SSRF-prone addresses regardless of allowlist (a permitted host
/// must not resolve into the internal network / cloud metadata).
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || *v4 == Ipv4Addr::new(169, 254, 169, 254) // cloud metadata
                || v4.octets()[0] == 0
                // CGNAT 100.64.0.0/10 (Tailscale etc.)
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || is_ipv6_ula(v6)         // fc00::/7 unique-local
                || is_ipv6_link_local(v6)  // fe80::/10
                || v6.to_ipv4_mapped().map(|m| is_internal_ip(&IpAddr::V4(m))).unwrap_or(false)
        }
    }
}

fn is_ipv6_ula(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xfe00) == 0xfc00
}
fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

/// Parse a `CONNECT host:port HTTP/1.1` request line. Returns (host, port).
pub fn parse_connect(line: &str) -> Option<(String, u16)> {
    let line = line.trim();
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    let authority = parts.next()?;
    // host:port (IPv6 in brackets: [::1]:443)
    if let Some(rest) = authority.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        let port = p.trim_start_matches(':').parse().ok()?;
        return Some((h.to_string(), port));
    }
    let (h, p) = authority.rsplit_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

/// Validate a CONNECT target against the allowlist + MVP policy + SSRF block, and
/// return the resolved external SocketAddr to dial. Errors describe the denial.
pub async fn validate_target(host: &str, port: u16, allowlist: &[String]) -> Result<SocketAddr> {
    if port != 443 {
        return Err(anyhow!(
            "only HTTPS CONNECT (:443) allowed in v1, got :{port}"
        ));
    }
    if !host_allowed(host, allowlist) {
        return Err(anyhow!("host '{host}' not in allowlist"));
    }
    // Host-side DNS resolution (plugin never resolves → no rebinding).
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| anyhow!("resolve {host}: {e}"))?;
    for addr in addrs {
        if !is_internal_ip(&addr.ip()) {
            return Ok(addr);
        }
    }
    Err(anyhow!(
        "host '{host}' resolves only to internal/blocked addresses (SSRF)"
    ))
}

/// A running egress proxy. Bound loopback address + the proxy-auth token + a guard
/// whose drop (or `shutdown()`) cancels the accept loop AND every active tunnel.
pub struct ProxyHandle {
    pub addr: SocketAddr,
    pub token: String,
    shutdown: tokio::sync::watch::Sender<bool>,
}
impl ProxyHandle {
    /// Proxy URL with embedded credentials for HTTP_PROXY (clients send the token
    /// as Proxy-Authorization). A local process without the token gets 407.
    pub fn url(&self) -> String {
        format!("http://furx:{}@127.0.0.1:{}", self.token, self.addr.port())
    }
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}
impl Drop for ProxyHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// Generate a 32-hex random proxy-auth token (loopback isolation vs other local procs).
fn gen_token() -> String {
    let id = uuid::Uuid::new_v4();
    let id2 = uuid::Uuid::new_v4();
    format!("{}{}", id.simple(), id2.simple())[..32].to_string()
}

/// Spawn the egress proxy for `allowlist`. Bound before returning (no readiness
/// race). Shutting down the returned handle cancels active tunnels too.
pub async fn spawn(allowlist: Vec<String>) -> Result<ProxyHandle> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let token = gen_token();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let token_task = token.clone();
    let rx_task = rx.clone();
    tokio::spawn(async move {
        let mut rx_loop = rx_task;
        loop {
            tokio::select! {
                _ = rx_loop.changed() => break, // shutdown signalled or sender dropped
                accepted = listener.accept() => {
                    let Ok((client, _)) = accepted else { continue };
                    let allow = allowlist.clone();
                    let tok = token_task.clone();
                    let conn_rx = rx_loop.clone();
                    tokio::spawn(async move { let _ = handle_conn(client, allow, tok, conn_rx).await; });
                }
            }
        }
    });
    Ok(ProxyHandle {
        addr,
        token,
        shutdown: tx,
    })
}

/// Extract the bearer/basic token from a `Proxy-Authorization: Basic <b64>` header,
/// returning the part after "furx:". None if absent/malformed.
fn proxy_auth_token(head: &str) -> Option<String> {
    for line in head.lines() {
        if let Some(v) = line
            .strip_prefix("Proxy-Authorization:")
            .or_else(|| line.strip_prefix("proxy-authorization:"))
        {
            let v = v.trim();
            if let Some(b64) = v
                .strip_prefix("Basic ")
                .or_else(|| v.strip_prefix("basic "))
            {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .ok()?;
                let s = String::from_utf8(decoded).ok()?;
                return s.split_once(':').map(|(_, t)| t.to_string());
            }
        }
    }
    None
}

async fn handle_conn(
    mut client: TcpStream,
    allowlist: Vec<String>,
    token: String,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let mut buf = [0u8; 8192];
    let n = client.read(&mut buf).await?;
    let head = String::from_utf8_lossy(&buf[..n]);
    // Proxy auth: a local process without the token can't use this proxy.
    if proxy_auth_token(&head).as_deref() != Some(token.as_str()) {
        let _ = client
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\n\r\n",
            )
            .await;
        return Ok(());
    }
    let line = head.lines().next().unwrap_or("");
    let Some((host, port)) = parse_connect(line) else {
        let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        return Ok(());
    };
    match validate_target(&host, port, &allowlist).await {
        Ok(target) => {
            // Connect to the EXACT validated IP (no second resolution → no rebinding).
            let mut upstream = match TcpStream::connect(target).await {
                Ok(s) => s,
                Err(_) => {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                    return Ok(());
                }
            };
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            // Tunnel — cancelled if the proxy shuts down (closes the lifecycle race:
            // a forked descendant can't keep egress after the tool ends).
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
                _ = shutdown.changed() => {}
            }
        }
        Err(_) => {
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_exact_canonical_match() {
        let allow = vec!["API.GitHub.com".to_string(), "example.org".into()];
        assert!(host_allowed("api.github.com", &allow));
        assert!(host_allowed("api.github.com.", &allow)); // trailing dot
        assert!(host_allowed("api.github.com:443", &allow)); // with port
        assert!(!host_allowed("evil.com", &allow));
        assert!(!host_allowed("sub.api.github.com", &allow)); // no implicit subdomain
    }

    #[test]
    fn internal_ips_blocked() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.10",
        ] {
            assert!(is_internal_ip(&s.parse().unwrap()), "{s} must be internal");
        }
        for s in ["8.8.8.8", "1.1.1.1", "140.82.112.3"] {
            assert!(!is_internal_ip(&s.parse().unwrap()), "{s} must be external");
        }
    }

    #[test]
    fn internal_ipv6_blocked() {
        for s in [
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_internal_ip(&s.parse().unwrap()), "{s} must be internal");
        }
        assert!(!is_internal_ip(&"2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn parse_connect_handles_host_and_ipv6() {
        assert_eq!(
            parse_connect("CONNECT api.github.com:443 HTTP/1.1"),
            Some(("api.github.com".into(), 443))
        );
        assert_eq!(
            parse_connect("CONNECT [::1]:443 HTTP/1.1"),
            Some(("::1".into(), 443))
        );
        assert_eq!(parse_connect("GET / HTTP/1.1"), None);
    }

    fn auth_header(token: &str) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("furx:{token}"));
        format!("Proxy-Authorization: Basic {b64}\r\n")
    }

    #[tokio::test]
    async fn proxy_requires_auth_token() {
        let h = spawn(vec!["example.org".into()]).await.unwrap();
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        // No Proxy-Authorization → 407.
        s.write_all(b"CONNECT example.org:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 80];
        let n = s.read(&mut buf).await.unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).contains("407"),
            "missing token must 407"
        );
    }

    #[tokio::test]
    async fn proxy_server_rejects_disallowed_host_with_403() {
        let h = spawn(vec!["example.org".into()]).await.unwrap();
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        let req = format!(
            "CONNECT evil.com:443 HTTP/1.1\r\n{}\r\n",
            auth_header(&h.token)
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(
            resp.contains("403"),
            "disallowed host (authed) must 403, got: {resp}"
        );
    }

    #[tokio::test]
    async fn proxy_server_rejects_non_connect() {
        let h = spawn(vec!["example.org".into()]).await.unwrap();
        let mut s = TcpStream::connect(h.addr).await.unwrap();
        let req = format!(
            "GET http://evil.com/ HTTP/1.1\r\n{}\r\n",
            auth_header(&h.token)
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("400"));
    }

    #[tokio::test]
    async fn validate_rejects_non443_and_disallowed_and_internal() {
        let allow = vec!["localhost".to_string()];
        assert!(validate_target("localhost", 80, &allow).await.is_err()); // non-443
        assert!(validate_target("evil.com", 443, &allow).await.is_err()); // not in allowlist
                                                                          // localhost is allowed by name but resolves to loopback → SSRF block.
        assert!(validate_target("localhost", 443, &allow).await.is_err());
    }
}
