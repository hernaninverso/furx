// F22 — Parse ~/.ssh/config Host blocks. Returns list with name + hostname/port.
// Plus TCP check :22 (or configured port) with timeout.

use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct SshHost {
    pub name: String,
    pub hostname: Option<String>,
    pub port: u16,
    pub user: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SshHostPing {
    pub host: SshHost,
    pub up: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

pub fn parse_config() -> Result<Vec<SshHost>> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
    let p = home.join(".ssh").join("config");
    if !p.exists() {
        return Ok(vec![]);
    }
    let text = std::fs::read_to_string(&p)?;
    Ok(parse_str(&text))
}

fn parse_str(text: &str) -> Vec<SshHost> {
    let mut out = Vec::new();
    let mut current: Option<SshHost> = None;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, |c: char| c.is_ascii_whitespace());
        let key = parts.next().unwrap_or("").to_ascii_lowercase();
        let value = parts.next().unwrap_or("").trim();
        match key.as_str() {
            "host" => {
                if let Some(h) = current.take() {
                    out.push(h);
                }
                // Take the first token only (Host can be a list).
                let name = value.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() && !name.contains('*') && !name.contains('?') {
                    current = Some(SshHost {
                        name,
                        hostname: None,
                        port: 22,
                        user: None,
                    });
                } else {
                    current = None;
                }
            }
            "hostname" => {
                if let Some(c) = current.as_mut() {
                    c.hostname = Some(value.to_string());
                }
            }
            "port" => {
                if let Some(c) = current.as_mut() {
                    if let Ok(p) = value.parse::<u16>() {
                        c.port = p;
                    }
                }
            }
            "user" => {
                if let Some(c) = current.as_mut() {
                    c.user = Some(value.to_string());
                }
            }
            _ => {}
        }
    }
    if let Some(h) = current.take() {
        out.push(h);
    }
    out
}

pub async fn ping(host: SshHost) -> SshHostPing {
    let addr = format!(
        "{}:{}",
        host.hostname.clone().unwrap_or_else(|| host.name.clone()),
        host.port
    );
    let started = std::time::Instant::now();
    let connect = tokio::net::TcpStream::connect(&addr);
    match tokio::time::timeout(Duration::from_secs(2), connect).await {
        Ok(Ok(_)) => SshHostPing {
            host,
            up: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(Err(e)) => SshHostPing {
            host,
            up: false,
            latency_ms: None,
            error: Some(e.to_string()),
        },
        Err(_) => SshHostPing {
            host,
            up: false,
            latency_ms: None,
            error: Some("timeout".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_config() {
        let s = "Host web\n  HostName 192.0.2.10\n  User alice\n  Port 22\n\nHost dr\n  HostName 192.0.2.20\n";
        let hosts = parse_str(s);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "web");
        assert_eq!(hosts[0].hostname.as_deref(), Some("192.0.2.10"));
        assert_eq!(hosts[0].user.as_deref(), Some("alice"));
        assert_eq!(hosts[1].name, "dr");
    }

    #[test]
    fn skips_wildcard_hosts() {
        let s = "Host *\n  ForwardAgent yes\n\nHost real\n  HostName r.example.com\n";
        let hosts = parse_str(s);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "real");
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let s = "# c1\nHost x\n  # inline comment\n  HostName y.example.com\n  Port 2222\n";
        let hosts = parse_str(s);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].port, 2222);
    }
}
