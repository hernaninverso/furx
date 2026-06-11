// VPN support — Tailscale + WireGuard status (and optional bring-up).
// Most observability features (Grafana iframe, the dev server monitors, SSH connect)
// depend on the Tailscale tunnel being up. If it drops, things fail silently.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::process::Command;
use std::time::Duration;
use tokio::process::Command as TokioCommand;

#[derive(Debug, Clone, Serialize)]
pub struct TailscalePeer {
    pub hostname: String,
    pub tailscale_ip: Option<String>,
    pub online: bool,
    pub os: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TailscaleStatus {
    pub installed: bool,
    pub running: bool,
    pub backend_state: Option<String>, // "Running" | "NeedsLogin" | "Stopped" | ...
    pub self_ip: Option<String>,
    pub self_hostname: Option<String>,
    pub peers: Vec<TailscalePeer>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WireguardInterface {
    pub name: String,
    pub public_key: Option<String>,
    pub peers_count: usize,
    pub up: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VpnStatus {
    pub tailscale: TailscaleStatus,
    pub wireguard: Vec<WireguardInterface>,
}

pub async fn status() -> VpnStatus {
    VpnStatus {
        tailscale: tailscale_status().await,
        wireguard: wireguard_status().await,
    }
}

async fn tailscale_status() -> TailscaleStatus {
    let bin = which("tailscale");
    let installed = bin.is_some();
    if !installed {
        return TailscaleStatus {
            installed: false,
            running: false,
            backend_state: None,
            self_ip: None,
            self_hostname: None,
            peers: vec![],
        };
    }
    // `tailscale status --json` is stable since v1.20.
    let out = match tokio::time::timeout(
        Duration::from_secs(3),
        TokioCommand::new(bin.as_ref().unwrap())
            .args(["status", "--json"])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        _ => {
            return TailscaleStatus {
                installed: true,
                running: false,
                backend_state: Some("unreachable".into()),
                self_ip: None,
                self_hostname: None,
                peers: vec![],
            }
        }
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return TailscaleStatus {
            installed: true,
            running: false,
            backend_state: Some("bad-json".into()),
            self_ip: None,
            self_hostname: None,
            peers: vec![],
        };
    };
    let backend_state = v
        .get("BackendState")
        .and_then(|x| x.as_str())
        .map(String::from);
    let running = backend_state.as_deref() == Some("Running");
    let self_info = v.get("Self");
    let self_ip = self_info
        .and_then(|s| s.get("TailscaleIPs"))
        .and_then(|i| i.as_array())
        .and_then(|a| a.first())
        .and_then(|i| i.as_str())
        .map(String::from);
    let self_hostname = self_info
        .and_then(|s| s.get("HostName"))
        .and_then(|h| h.as_str())
        .map(String::from);
    let mut peers = Vec::new();
    if let Some(peer_map) = v.get("Peer").and_then(|p| p.as_object()) {
        for (_id, peer) in peer_map {
            let hostname = peer
                .get("HostName")
                .and_then(|h| h.as_str())
                .unwrap_or("?")
                .to_string();
            let tailscale_ip = peer
                .get("TailscaleIPs")
                .and_then(|i| i.as_array())
                .and_then(|a| a.first())
                .and_then(|i| i.as_str())
                .map(String::from);
            let online = peer
                .get("Online")
                .and_then(|o| o.as_bool())
                .unwrap_or(false);
            let os = peer.get("OS").and_then(|o| o.as_str()).map(String::from);
            let last_seen = peer
                .get("LastSeen")
                .and_then(|s| s.as_str())
                .map(String::from);
            peers.push(TailscalePeer {
                hostname,
                tailscale_ip,
                online,
                os,
                last_seen,
            });
        }
    }
    peers.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| a.hostname.cmp(&b.hostname))
    });
    TailscaleStatus {
        installed: true,
        running,
        backend_state,
        self_ip,
        self_hostname,
        peers,
    }
}

async fn wireguard_status() -> Vec<WireguardInterface> {
    let bin = which("wg");
    let Some(bin) = bin else {
        return vec![];
    };
    let out = match tokio::time::timeout(
        Duration::from_secs(3),
        TokioCommand::new(bin)
            .args(["show", "all", "dump"])
            .output(),
    )
    .await
    {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return vec![],
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut by_iface: std::collections::BTreeMap<String, (Option<String>, usize)> =
        std::collections::BTreeMap::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.is_empty() {
            continue;
        }
        let iface = cols[0].to_string();
        let entry = by_iface.entry(iface).or_insert((None, 0));
        if cols.len() == 5 {
            // interface line: NAME PRIVATE PUBLIC PORT FWMARK
            entry.0 = Some(cols[2].to_string());
        } else {
            // peer line
            entry.1 += 1;
        }
    }
    by_iface
        .into_iter()
        .map(|(name, (pk, peers_count))| WireguardInterface {
            name,
            public_key: pk,
            peers_count,
            up: true,
        })
        .collect()
}

/// `tailscale up` — only allowed for the tailscale daemon (no arbitrary names).
pub async fn bring_up(name: &str) -> Result<String> {
    match name {
        "tailscale" => {
            let bin = which("tailscale").ok_or_else(|| anyhow!("tailscale not installed"))?;
            let out = tokio::time::timeout(
                Duration::from_secs(15),
                TokioCommand::new(bin).args(["up"]).output(),
            )
            .await
            .map_err(|_| anyhow!("tailscale up timed out"))?
            .map_err(|e| anyhow!("tailscale up spawn: {}", e))?;
            if !out.status.success() {
                return Err(anyhow!(
                    "tailscale up failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
        _ => Err(anyhow!("vpn name not in allowlist: {}", name)),
    }
}

fn which(cmd: &str) -> Option<String> {
    if let Ok(p) = std::env::var("PATH") {
        for d in p.split(':') {
            let cand = std::path::Path::new(d).join(cmd);
            if cand.exists() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    Command::new("/usr/bin/which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bring_up_rejects_unknown_name() {
        assert!(bring_up("evil").await.is_err());
        assert!(bring_up("rm -rf /").await.is_err());
    }
}
