// F16 — MCP server health: parse ~/.claude/.mcp.json (and Claude Code's
// mcp_servers settings); for stdio servers we run `<cmd> --help` with a
// short timeout; for http(s) servers we do a HEAD with sandbox allowlist.
// Pattern adapted from ~/eleion-workspace/src-tauri/src/health.rs:
// concurrent cap + circuit breaker. Simplified: 1-shot timeout, no breaker
// (each poll is independent).

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const PING_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
const STDIO_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize)]
pub struct McpServerHealth {
    pub name: String,
    pub transport: String, // "stdio" | "http" | "sse" | "unknown"
    pub healthy: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
    /// BLOQUE F · F16 — count of tools exposed by `tools/list`. None when the
    /// server doesn't respond to the MCP handshake in time, isn't reachable,
    /// or its response is malformed. UI surfaces this in the tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools_count: Option<u32>,
    /// First N tool names (truncated) for the tooltip, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools_sample: Option<Vec<String>>,
    /// 045 FR-002 — estado efectivo del toggle del usuario (DB override sobre ~/.claude.json).
    /// `true` = el server está activo para Furx; `false` = el usuario lo deshabilitó (la fila de
    /// override existe con enabled=0). Default `true` (sin override en DB). NO toca el JSON.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpHealthReport {
    pub config_path: Option<String>,
    pub servers: Vec<McpServerHealth>,
    pub elapsed_ms: u64,
}

/// 045 FR-002 — nombres canónicos de los MCP servers declarados en ~/.claude.json
/// (lista de verdad contra la que `mcp_set_enabled` valida). Incluye los de proyecto con el
/// prefijo `project:` (igual que `parse_servers`). Vacío si no hay config / no se puede leer.
pub fn list_server_names() -> Vec<String> {
    let (_, servers) = read_mcp_config();
    servers.into_iter().map(|(name, _)| name).collect()
}

pub async fn check_all() -> McpHealthReport {
    let started = Instant::now();
    let (config_path, servers) = read_mcp_config();
    let mut out = Vec::new();
    for (name, spec) in servers {
        out.push(check_one(&name, &spec).await);
    }
    McpHealthReport {
        config_path: config_path.map(|p| p.to_string_lossy().to_string()),
        servers: out,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

#[derive(Debug, Clone)]
enum ServerSpec {
    // `args` is informational only — we never spawn the server (stateful), just
    // confirm the binary exists in PATH. Kept for future probing.
    Stdio {
        command: String,
        #[allow(dead_code)]
        args: Vec<String>,
    },
    Http {
        url: String,
    },
    Sse {
        url: String,
    },
    Unknown,
}

fn read_mcp_config() -> (Option<PathBuf>, Vec<(String, ServerSpec)>) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return (None, vec![]),
    };
    // Try both ~/.claude.json (claude-cli) and ~/.claude/.mcp.json (alt).
    let candidates = [
        home.join(".claude.json"),
        home.join(".claude").join(".mcp.json"),
    ];
    for p in &candidates {
        if let Ok(text) = std::fs::read_to_string(p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                return (Some(p.clone()), parse_servers(&v));
            }
        }
    }
    (None, vec![])
}

fn parse_servers(v: &serde_json::Value) -> Vec<(String, ServerSpec)> {
    // Claude Code stores under `mcpServers` (top-level or per-project).
    let mut out = Vec::new();
    if let Some(servers) = v.get("mcpServers").and_then(|x| x.as_object()) {
        for (name, spec) in servers {
            out.push((name.clone(), spec_from(spec)));
        }
    }
    if let Some(projects) = v.get("projects").and_then(|x| x.as_object()) {
        for (_proj, body) in projects {
            if let Some(servers) = body.get("mcpServers").and_then(|x| x.as_object()) {
                for (name, spec) in servers {
                    out.push((format!("project:{}", name), spec_from(spec)));
                }
            }
        }
    }
    out
}

fn spec_from(spec: &serde_json::Value) -> ServerSpec {
    if let Some(url) = spec.get("url").and_then(|x| x.as_str()) {
        if spec.get("type").and_then(|x| x.as_str()) == Some("sse") {
            return ServerSpec::Sse {
                url: url.to_string(),
            };
        }
        return ServerSpec::Http {
            url: url.to_string(),
        };
    }
    if let Some(command) = spec.get("command").and_then(|x| x.as_str()) {
        let args = spec
            .get("args")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        return ServerSpec::Stdio {
            command: command.to_string(),
            args,
        };
    }
    ServerSpec::Unknown
}

async fn check_one(name: &str, spec: &ServerSpec) -> McpServerHealth {
    let started = Instant::now();
    match spec {
        ServerSpec::Stdio { command, args } => {
            // BLOQUE F · F16: best-effort MCP handshake via stdio. If `command`
            // is not in PATH we early-return as before; otherwise spawn briefly
            // (3s timeout), send initialize + tools/list, parse response. Any
            // failure leaves tools_count=None but doesn't mark the server
            // unhealthy — `which` success is still the canonical health signal.
            let path_ok = which_in_path(command);
            if !path_ok {
                return McpServerHealth {
                    name: name.to_string(),
                    transport: "stdio".into(),
                    healthy: false,
                    latency_ms: Some(started.elapsed().as_millis() as u64),
                    error: Some(format!("not in PATH: {}", command)),
                    tools_count: None,
                    tools_sample: None,
                    enabled: true,
                };
            }
            let (tools_count, tools_sample) = stdio_tools_probe(command, args).await;
            McpServerHealth {
                name: name.to_string(),
                transport: "stdio".into(),
                healthy: true,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                error: None,
                tools_count,
                tools_sample,
                enabled: true,
            }
        }
        ServerSpec::Http { url } | ServerSpec::Sse { url } => {
            let transport = if matches!(spec, ServerSpec::Sse { .. }) {
                "sse"
            } else {
                "http"
            };
            if !http_allowed(url) {
                return McpServerHealth {
                    name: name.to_string(),
                    transport: transport.into(),
                    healthy: false,
                    latency_ms: None,
                    error: Some("url not in allowlist".into()),
                    tools_count: None,
                    tools_sample: None,
                    enabled: true,
                };
            }
            let client = match reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
                Ok(c) => c,
                Err(e) => return err(name, transport, e.to_string()),
            };
            match client.head(url).send().await {
                Ok(resp) => {
                    let healthy = resp.status().as_u16() < 500;
                    // Best-effort tools/list probe — only if HEAD succeeded.
                    let (tools_count, tools_sample) = if healthy {
                        http_tools_probe(&client, url).await
                    } else {
                        (None, None)
                    };
                    McpServerHealth {
                        name: name.to_string(),
                        transport: transport.into(),
                        healthy,
                        latency_ms: Some(started.elapsed().as_millis() as u64),
                        error: if resp.status().as_u16() >= 500 {
                            Some(format!("status {}", resp.status()))
                        } else {
                            None
                        },
                        tools_count,
                        tools_sample,
                        enabled: true,
                    }
                }
                Err(e) => err(name, transport, e.to_string()),
            }
        }
        ServerSpec::Unknown => McpServerHealth {
            name: name.to_string(),
            transport: "unknown".into(),
            healthy: false,
            latency_ms: None,
            error: Some("could not parse server spec".into()),
            tools_count: None,
            tools_sample: None,
            enabled: true,
        },
    }
}

/// MCP JSON-RPC HTTP transport: POST `{jsonrpc:"2.0", method:"tools/list"}` to
/// the server URL; the spec says the response is `{result:{tools:[...]}}`.
async fn http_tools_probe(
    client: &reqwest::Client,
    url: &str,
) -> (Option<u32>, Option<Vec<String>>) {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});
    let resp = match client.post(url).json(&body).send().await {
        Ok(r) => r,
        Err(_) => return (None, None),
    };
    let val: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let tools = val
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array());
    let Some(arr) = tools else {
        return (None, None);
    };
    let count = arr.len() as u32;
    let sample: Vec<String> = arr
        .iter()
        .take(8)
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();
    (
        Some(count),
        if sample.is_empty() {
            None
        } else {
            Some(sample)
        },
    )
}

/// MCP JSON-RPC stdio transport: spawn the binary, send `initialize` then
/// `tools/list` on stdin (newline-delimited JSON), read stdout for the matching
/// response IDs. Bounded by STDIO_HANDSHAKE_TIMEOUT. Returns (None, None) on
/// any failure — we never block a health check on this probe.
async fn stdio_tools_probe(command: &str, args: &[String]) -> (Option<u32>, Option<Vec<String>>) {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let mut cmd = TokioCommand::new(command);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = child.start_kill();
            return (None, None);
        }
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.start_kill();
            return (None, None);
        }
    };
    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "furx-health", "version": env!("CARGO_PKG_VERSION")}}
    });
    let tools =
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let probe = async move {
        // Best-effort writes; ignore EOF — server may close immediately.
        let _ = stdin.write_all((init.to_string() + "\n").as_bytes()).await;
        let _ = stdin.write_all((tools.to_string() + "\n").as_bytes()).await;
        let _ = stdin.flush().await;
        drop(stdin);
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        // Read up to ~6 lines (initialize ack + tools/list response + maybe noise).
        for _ in 0..6 {
            line.clear();
            // Err (incl. EOF/transporte) → 0 == el Default de usize → tratado como fin (break abajo).
            let n = reader.read_line(&mut line).await.unwrap_or_default();
            if n == 0 {
                break;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v.get("id").and_then(|i| i.as_u64()) == Some(2) {
                    let arr = v
                        .get("result")
                        .and_then(|r| r.get("tools"))
                        .and_then(|t| t.as_array());
                    if let Some(a) = arr {
                        let count = a.len() as u32;
                        let sample: Vec<String> = a
                            .iter()
                            .take(8)
                            .filter_map(|t| {
                                t.get("name").and_then(|n| n.as_str()).map(String::from)
                            })
                            .collect();
                        return (
                            Some(count),
                            if sample.is_empty() {
                                None
                            } else {
                                Some(sample)
                            },
                        );
                    }
                }
            }
        }
        (None, None)
    };
    // timeout → (None, None) en Err es el Default del tuple; `.unwrap_or_default()` es equivalente.
    let result = tokio::time::timeout(STDIO_HANDSHAKE_TIMEOUT, probe).await.unwrap_or_default();
    // Kill regardless of outcome.
    let _ = child.start_kill();
    result
}

fn err(name: &str, transport: &str, msg: String) -> McpServerHealth {
    McpServerHealth {
        name: name.to_string(),
        transport: transport.into(),
        healthy: false,
        latency_ms: None,
        error: Some(msg),
        tools_count: None,
        tools_sample: None,
        enabled: true,
    }
}

fn http_allowed(url: &str) -> bool {
    crate::bases::allowlist::url_allowed(url)
}

fn which_in_path(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if std::path::Path::new(dir).join(cmd).exists() {
                return true;
            }
        }
    }
    Command::new("/usr/bin/which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// PING_TIMEOUT kept for future stdio probing (unused right now).
const _: Duration = PING_TIMEOUT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stdio_spec() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"mcpServers":{"mnemo":{"command":"mnemo","args":["serve"]}}}"#,
        )
        .unwrap();
        let servers = parse_servers(&v);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "mnemo");
        match &servers[0].1 {
            ServerSpec::Stdio { command, args } => {
                assert_eq!(command, "mnemo");
                assert_eq!(args, &vec!["serve".to_string()]);
            }
            _ => panic!("expected Stdio"),
        }
    }

    #[test]
    fn parses_http_spec() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"mcpServers":{"ai":{"url":"http://localhost:8250","type":"http"}}}"#,
        )
        .unwrap();
        let servers = parse_servers(&v);
        assert_eq!(servers.len(), 1);
        match &servers[0].1 {
            ServerSpec::Http { url } => assert_eq!(url, "http://localhost:8250"),
            _ => panic!("expected Http"),
        }
    }

    #[test]
    fn http_allowlist_blocks_external() {
        // 041 FR-005 — loopback allowed by default; a configured host is allowed once registered.
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        assert!(!http_allowed("http://evil.com"));
        assert!(http_allowed("http://localhost:8250"));
        assert!(!http_allowed("https://aie.example.io"));
        crate::bases::allowlist::add_runtime_origin("https://aie.example.io:443").unwrap();
        assert!(http_allowed("https://aie.example.io"));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    // 040 P3 — verifiable parity between the two orthogonal MCP pipelines: the config the
    // INJECTOR writes (`mcp_inject::build_mcp_config`) round-trips through the HEALTH parser
    // (`parse_servers`). Same `{"mcpServers": {...}}` schema on both ends; no disk, no spawn.
    #[test]
    fn inject_output_is_parseable_by_health() {
        use crate::services::mcp_inject::{build_mcp_config, ResolvedMcpServer};
        use std::collections::BTreeMap;

        let injected = vec![ResolvedMcpServer {
            name: "codebase-memory".into(),
            command: "/abs/codebase-memory".into(),
            args: vec!["--stdio".into()],
            env: {
                let mut e = BTreeMap::new();
                e.insert("FURX_PROJECT_KEY".into(), "k".into());
                e
            },
        }];
        let cfg = build_mcp_config(&injected);
        let parsed = parse_servers(&cfg);
        assert_eq!(parsed.len(), 1, "health must see the injected server");
        assert_eq!(parsed[0].0, "codebase-memory");
        match &parsed[0].1 {
            ServerSpec::Stdio { command, args } => {
                assert_eq!(command, "/abs/codebase-memory");
                assert_eq!(args, &vec!["--stdio".to_string()]);
            }
            other => panic!("expected Stdio, got {other:?}"),
        }
        // empty injection (default-deny) → health sees zero servers, no crash.
        assert!(parse_servers(&build_mcp_config(&[])).is_empty());
    }
}
