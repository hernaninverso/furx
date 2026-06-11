// spec-kit 046 · Ola 7 (Skills P1) F3 — discovery del MCP Registry oficial.
//
// La Ola 4 hace discovery LOCAL (`sources.user.toml` → dirs `~/.hermes`/`~/.openclaw`) +
// un índice firmado furx-core. Esta ola EXTIENDE el discovery con el MCP Registry oficial
// (`registry.modelcontextprotocol.io`, formato `server.json`, namespaces reverse-DNS
// verificados).
//
// POSTURA (idéntica al discovery local de la Ola 4): el registry solo LISTA SUGERENCIAS.
// NADA se instala ni se ejecuta desde acá. La firma builtin de furx-core sigue siendo la
// ÚNICA capa de confianza; un server del MCP Registry es metadata (`type=http`/`url`/
// nombre/descripción) que el usuario podría querer agregar a SU `~/.claude.json` a mano —
// pero este módulo no lo hace, no descarga binarios, no toca el filesystem de plugins. Es
// FAIL-CLOSED: un server del registry NUNCA pasa el gate de la Ola 4 por sí solo (no trae
// un manifest firmado por Furx), así que jamás se vuelve ejecutable por este camino.
//
// Solo aceptamos `type` de transporte REMOTO (`streamable-http`/`http`/`sse`) — un server
// con SOLO `packages` (npm/pypi/oci, que implicarían DESCARGAR+EJECUTAR código) se lista
// igual como sugerencia pero marcado `installable=false` y SIN URL ejecutable: nunca damos
// un comando de instalación. El reverse-DNS del `name` se valida (forma `ns.reverse/server`)
// para no mostrar entradas con nombres arbitrarios.
//
// La capa de RED (fetch reqwest) es una envoltura fina sobre la lógica PURA de parseo +
// validación, que está testeada en aislamiento (sin red). Offline / 4xx / 5xx / body no-
// JSON → degrada limpio a `Vec` vacío + un error tipado que la UI muestra como "registry
// no disponible" — el discovery local nunca se ve afectado.
//
// Dead-code-first: probado en aislamiento; el wiring del comando Tauri + UI es aparte.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Default base URL of the official MCP Registry.
pub const DEFAULT_REGISTRY_BASE: &str = "https://registry.modelcontextprotocol.io";

/// Cap on the registry response body we will parse (defensive — the registry can be large;
/// we page, but never buffer an unbounded body).
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Cap on suggestions returned per call (UI list bound).
const MAX_SUGGESTIONS: usize = 500;

/// A discovered MCP server SUGGESTION (NOT installed, NOT executable). Mirrors the shape of
/// `skill_discovery::DiscoveredSkill` so the UI can render both discovery sources uniformly.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredMcpServer {
    /// Reverse-DNS server name, e.g. `io.github.owner/my-server`.
    pub name: String,
    pub description: String,
    pub version: String,
    /// The source repository URL (informational).
    pub repository_url: Option<String>,
    /// The remote transport kind (`streamable-http`|`http`|`sse`) if this server exposes a
    /// remote endpoint, else `None` (package-only servers).
    pub remote_type: Option<String>,
    /// The remote endpoint URL (only for remote servers; `None` for package-only).
    pub remote_url: Option<String>,
    /// Registry status (`active`/`deprecated`) from the official `_meta`.
    pub status: Option<String>,
    /// The discovery source label (always `"mcp-registry"`).
    pub source: String,
    /// ALWAYS `false`: nothing here is installable via Furx's gate. The UI shows it as a
    /// suggestion; importing/enabling is a separate, gated, explicit user action.
    pub installable: bool,
}

// ── server.json deserialization (only the fields we surface) ──────────────────

#[derive(Debug, Clone, Deserialize)]
struct RegistryResponse {
    // ⟨audit codex LOW⟩ `servers` is REQUIRED (no serde default): a body like `{}` is NOT a
    // valid registry response → parse error (caller shows "unavailable"), whereas an
    // explicit `"servers": []` is a clean empty result.
    servers: Vec<ServerEnvelope>,
    #[serde(default)]
    metadata: Option<PageMeta>,
}

#[derive(Debug, Clone, Deserialize)]
struct PageMeta {
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerEnvelope {
    server: ServerJson,
    #[serde(default, rename = "_meta")]
    meta: Option<MetaWrapper>,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerJson {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    repository: Option<Repository>,
    #[serde(default)]
    remotes: Vec<Remote>,
    // `packages` exist but we deliberately do NOT surface install commands; we only note
    // whether a server is remote-capable. Keeping the field out keeps us from echoing a
    // package identifier that could read as an install instruction.
}

#[derive(Debug, Clone, Deserialize)]
struct Repository {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Remote {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MetaWrapper {
    #[serde(default, rename = "io.modelcontextprotocol.registry/official")]
    official: Option<OfficialMeta>,
}

#[derive(Debug, Clone, Deserialize)]
struct OfficialMeta {
    #[serde(default)]
    status: Option<String>,
}

/// Accept ONLY these remote transport kinds (the metadata-only, nothing-executes set). A
/// `stdio` package transport is intentionally NOT a remote we surface a URL for.
fn is_accepted_remote(kind: &str) -> bool {
    matches!(kind, "streamable-http" | "http" | "sse")
}

/// Validate the SYNTAX of a reverse-DNS MCP server name: `<reverse.dns.namespace>/<server-
/// name>`. The namespace must be a dotted reverse-DNS (≥2 ASCII labels) and the server-name
/// a safe ASCII slug (no slash/traversal). ⟨audit codex LOW⟩ This is a SYNTAX check, NOT an
/// ownership/authenticity check — it rejects malformed/traversal ids so the UI never lists
/// a server with a path-bearing or non-ASCII (homoglyph) id, but it does not prove the
/// namespace belongs to the publisher (the registry's own verification does that; and
/// regardless, nothing here is executable — `installable=false`).
pub fn is_valid_reverse_dns_name(name: &str) -> bool {
    let Some((ns, server)) = name.split_once('/') else {
        return false;
    };
    if ns.is_empty() || server.is_empty() || name.len() > 256 {
        return false;
    }
    // Namespace: dotted labels, each alnum/hyphen, ≥2 labels (reverse-DNS).
    let labels: Vec<&str> = ns.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if !labels.iter().all(|l| label_ok(l)) {
        return false;
    }
    // Server slug: alnum + `-`/`_`/`.` (a path segment, no slashes/traversal).
    !server.is_empty()
        && server.len() <= 128
        && server
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && server != "."
        && server != ".."
}

/// Validate a remote/repository URL: must be absolute HTTPS with a non-empty host and no
/// userinfo. ⟨audit codex/mistral MED⟩ Parsed with `url::Url` (not a `starts_with` string
/// check) so missing-host, credentials, backslashes, and control chars are all rejected —
/// a downstream URL parser can't reinterpret it. A bad/insecure URL → the remote is dropped
/// (the server may still list as a suggestion with no URL).
fn is_https_url(u: &str) -> bool {
    if u.len() > 2048 {
        return false;
    }
    match url::Url::parse(u) {
        Ok(parsed) => {
            parsed.scheme() == "https"
                && parsed.host_str().is_some_and(|h| !h.is_empty())
                && parsed.username().is_empty()
                && parsed.password().is_none()
        }
        Err(_) => false,
    }
}

/// PURE: parse a registry `/v0/servers` JSON body into suggestions + the next page cursor.
/// Fail-closed filtering:
///   - reject entries whose `name` is not a valid reverse-DNS server name,
///   - keep only the FIRST accepted remote (streamable-http/http/sse) with a valid HTTPS
///     URL; a package-only server is kept as a suggestion with `remote_type=None`,
///   - everything is `installable=false`.
///
/// Returns `(suggestions, next_cursor)`.
pub fn parse_registry_body(body: &str) -> Result<(Vec<DiscoveredMcpServer>, Option<String>)> {
    if body.len() > MAX_BODY_BYTES {
        return Err(anyhow!("registry body too large ({} bytes)", body.len()));
    }
    let resp: RegistryResponse =
        serde_json::from_str(body).map_err(|e| anyhow!("registry JSON parse: {e}"))?;
    let mut out = Vec::new();
    for env in resp.servers {
        if out.len() >= MAX_SUGGESTIONS {
            break;
        }
        let s = env.server;
        if !is_valid_reverse_dns_name(&s.name) {
            tracing::warn!("mcp-registry: skipping server with invalid name '{}'", s.name);
            continue;
        }
        // First accepted remote with a valid HTTPS URL.
        let remote = s
            .remotes
            .iter()
            .find(|r| is_accepted_remote(&r.kind) && r.url.as_deref().is_some_and(is_https_url));
        let (remote_type, remote_url) = match remote {
            Some(r) => (Some(r.kind.clone()), r.url.clone()),
            None => (None, None),
        };
        let repository_url = s
            .repository
            .and_then(|r| r.url)
            .filter(|u| is_https_url(u));
        let status = env.meta.and_then(|m| m.official).and_then(|o| o.status);
        out.push(DiscoveredMcpServer {
            name: s.name,
            description: s.description,
            version: s.version,
            repository_url,
            remote_type,
            remote_url,
            status,
            source: "mcp-registry".to_string(),
            installable: false,
        });
    }
    let next = resp.metadata.and_then(|m| m.next_cursor).filter(|c| !c.is_empty());
    Ok((out, next))
}

/// Hosts we will talk to for the registry. ⟨audit codex/mistral/deepseek HIGH⟩ SSRF guard:
/// `base` must resolve to one of these hosts over HTTPS (the official registry; a couple of
/// historical aliases). An arbitrary `base` (localhost, file://, http://, a private IP, a
/// host with userinfo) is REFUSED — we never fetch from an attacker-chosen origin.
const ALLOWED_REGISTRY_HOSTS: &[&str] = &[
    "registry.modelcontextprotocol.io",
    "registry.modelcontextprotocol.org",
];

/// Build + validate the `/v0/servers` URL from `base` (SSRF-guarded) + an optional opaque
/// `cursor` (percent-encoded via the query API so it can NEVER change host/path). Returns a
/// parsed `url::Url`. ⟨audit codex/mistral/deepseek⟩
fn build_servers_url(base: &str, cursor: Option<&str>) -> Result<url::Url> {
    let parsed = url::Url::parse(base.trim_end_matches('/'))
        .map_err(|e| anyhow!("invalid registry base: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(anyhow!("registry base must be https"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(anyhow!("registry base must not carry userinfo"));
    }
    match parsed.host_str() {
        Some(h) if ALLOWED_REGISTRY_HOSTS.contains(&h) => {}
        other => return Err(anyhow!("registry host not allowed: {other:?}")),
    }
    // Build the endpoint from the validated origin (drop any path/query the caller passed).
    let mut url = parsed
        .join("/v0/servers")
        .map_err(|e| anyhow!("join path: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.clear();
        q.append_pair("limit", "100");
        if let Some(c) = cursor {
            // `append_pair` percent-encodes the cursor → a hostile cursor (`/`, `@`, `..`,
            // control chars) stays inside the query value; it cannot alter host/path.
            q.append_pair("cursor", c);
        }
    }
    Ok(url)
}

/// NETWORK: fetch one page of registry suggestions. Thin wrapper over the pure parser. A
/// non-2xx status, a too-large/non-JSON body, or any transport error → `Err` (the caller
/// shows "registry unavailable" and the local discovery is untouched). `cursor` pages.
///
/// `base` defaults to `DEFAULT_REGISTRY_BASE`. The request is GET-only, no auth, no cookies,
/// redirects DISABLED (a redirect to http://localhost/file:// can't downgrade the origin),
/// and the body is STREAMED with a hard cap (never buffer an unbounded response).
pub async fn fetch_registry_page(
    base: &str,
    cursor: Option<&str>,
) -> Result<(Vec<DiscoveredMcpServer>, Option<String>)> {
    use futures_util::StreamExt;
    let url = build_servers_url(base, cursor)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none()) // ⟨audit⟩ no redirect → no origin downgrade
        .user_agent(concat!("furx/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| anyhow!("http client: {e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("registry request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("registry returned HTTP {}", resp.status()));
    }
    // ⟨audit codex/mistral HIGH⟩ STREAM the body enforcing MAX_BODY_BYTES as we go — never
    // buffer an unbounded response (a hostile/broken endpoint could send GBs).
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("registry body read: {e}"))?;
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(anyhow!("registry body exceeds cap ({MAX_BODY_BYTES} bytes)"));
        }
        buf.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(buf).map_err(|_| anyhow!("registry body is not UTF-8"))?;
    parse_registry_body(&body)
}

/// Hard cap on pages we will ever follow, regardless of the caller's `max_pages` (a runaway
/// or cyclic registry can't make us page forever). ⟨audit codex/mistral MED⟩
const MAX_PAGES_HARD_CAP: usize = 50;
/// Cap on an opaque cursor token length (defensive). ⟨audit codex MED⟩
const MAX_CURSOR_LEN: usize = 1024;

/// NETWORK: fetch up to `max_pages` of registry suggestions, concatenated. Stops at the
/// first page error (returning what was gathered so far ALONGSIDE the error would hide the
/// failure — instead we fail-closed: any page error → Err, the UI shows "unavailable").
/// Deduplicates by `name`, keeping the FIRST occurrence (registry names are unique; this is
/// only a guard against a misbehaving registry repeating a name across pages).
///
/// ⟨audit codex/mistral MED⟩ Bounded against a cyclic/runaway registry: at most
/// `min(max_pages, MAX_PAGES_HARD_CAP)` requests, a per-cursor length cap, and a STOP if a
/// cursor repeats (cycle detection).
pub async fn fetch_registry_suggestions(
    base: &str,
    max_pages: usize,
) -> Result<Vec<DiscoveredMcpServer>> {
    let mut all: Vec<DiscoveredMcpServer> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut seen_cursors = std::collections::HashSet::new();
    let mut cursor: Option<String> = None;
    let pages = max_pages.clamp(1, MAX_PAGES_HARD_CAP);
    for _ in 0..pages {
        if let Some(c) = &cursor {
            if c.len() > MAX_CURSOR_LEN {
                return Err(anyhow!("registry cursor too long"));
            }
            // Cycle detection: a repeated cursor means the registry is looping → stop.
            if !seen_cursors.insert(c.clone()) {
                break;
            }
        }
        let (page, next) = fetch_registry_page(base, cursor.as_deref()).await?;
        for s in page {
            if seen.insert(s.name.clone()) {
                all.push(s);
            }
            if all.len() >= MAX_SUGGESTIONS {
                return Ok(all);
            }
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── reverse-DNS name validation ──────────────────────────────────────────────
    #[test]
    fn reverse_dns_names() {
        assert!(is_valid_reverse_dns_name("io.github.owner/my-server"));
        assert!(is_valid_reverse_dns_name("com.example/server_1.2"));
        assert!(is_valid_reverse_dns_name("ai.inference.sh/run"));
        // invalid:
        assert!(!is_valid_reverse_dns_name("noslash"));
        assert!(!is_valid_reverse_dns_name("single/server")); // ns needs ≥2 labels
        assert!(!is_valid_reverse_dns_name("io.github.owner/")); // empty server
        assert!(!is_valid_reverse_dns_name("/server"));
        assert!(!is_valid_reverse_dns_name("io.github.owner/../etc")); // traversal slug rejected (slash)
        assert!(!is_valid_reverse_dns_name("io.github.owner/a/b")); // extra slash
        assert!(!is_valid_reverse_dns_name("-bad.ns/server")); // label starts with hyphen
        assert!(!is_valid_reverse_dns_name("io.github.owner/.."));
    }

    #[test]
    fn https_url_validation() {
        assert!(is_https_url("https://api.example.com/mcp"));
        assert!(!is_https_url("http://api.example.com")); // insecure
        assert!(!is_https_url("file:///etc/passwd"));
        assert!(!is_https_url("ftp://x"));
        assert!(!is_https_url("https://x\nHost: evil")); // header injection chars
    }

    // ── SC-003: parse the registry into suggestions; nothing installable ─────────
    #[test]
    fn parse_lists_remote_server_as_suggestion() {
        let body = r#"{
          "servers": [
            {
              "server": {
                "name": "io.github.acme/weather",
                "description": "Weather data over MCP",
                "version": "1.2.0",
                "repository": {"url": "https://github.com/acme/weather"},
                "remotes": [{"type": "streamable-http", "url": "https://mcp.acme.io/weather"}]
              },
              "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active"}}
            }
          ],
          "metadata": {"nextCursor": "abc123", "count": 1}
        }"#;
        let (sugg, next) = parse_registry_body(body).unwrap();
        assert_eq!(sugg.len(), 1);
        let s = &sugg[0];
        assert_eq!(s.name, "io.github.acme/weather");
        assert_eq!(s.version, "1.2.0");
        assert_eq!(s.remote_type.as_deref(), Some("streamable-http"));
        assert_eq!(s.remote_url.as_deref(), Some("https://mcp.acme.io/weather"));
        assert_eq!(s.repository_url.as_deref(), Some("https://github.com/acme/weather"));
        assert_eq!(s.status.as_deref(), Some("active"));
        assert_eq!(s.source, "mcp-registry");
        assert!(!s.installable, "NEVER installable from the registry");
        assert_eq!(next.as_deref(), Some("abc123"));
    }

    // ── package-only server: suggestion with NO remote URL, still not installable ─
    #[test]
    fn package_only_server_has_no_remote_url() {
        let body = r#"{
          "servers": [
            {
              "server": {
                "name": "io.github.acme/local-only",
                "description": "stdio server",
                "version": "0.1.0",
                "packages": [{"registryType": "npm", "identifier": "@acme/mcp", "version": "0.1.0", "transport": {"type": "stdio"}}],
                "remotes": []
              }
            }
          ]
        }"#;
        let (sugg, _) = parse_registry_body(body).unwrap();
        assert_eq!(sugg.len(), 1);
        assert!(sugg[0].remote_type.is_none(), "no remote → no URL surfaced");
        assert!(sugg[0].remote_url.is_none());
        assert!(!sugg[0].installable);
    }

    // ── fail-closed: invalid name + insecure remote URL are dropped/sanitized ─────
    #[test]
    fn invalid_name_skipped_and_insecure_remote_dropped() {
        let body = r#"{
          "servers": [
            {"server": {"name": "not-reverse-dns", "version": "1.0.0", "remotes": [{"type":"http","url":"https://ok.io"}]}},
            {"server": {"name": "io.github.acme/insecure", "version": "1.0.0",
              "remotes": [{"type": "http", "url": "http://insecure.io/mcp"}]}}
          ]
        }"#;
        let (sugg, _) = parse_registry_body(body).unwrap();
        // The bad-name entry is skipped entirely.
        assert_eq!(sugg.len(), 1);
        assert_eq!(sugg[0].name, "io.github.acme/insecure");
        // Its insecure http:// remote is dropped → no remote surfaced.
        assert!(sugg[0].remote_url.is_none(), "insecure remote dropped");
        assert!(sugg[0].remote_type.is_none());
    }

    // ── unknown remote transport types are ignored (only http/sse/streamable) ────
    #[test]
    fn unknown_transport_ignored() {
        let body = r#"{
          "servers": [
            {"server": {"name": "io.github.acme/weird", "version": "1.0.0",
              "remotes": [{"type": "ws", "url": "https://x.io"}, {"type": "sse", "url": "https://good.io/sse"}]}}
          ]
        }"#;
        let (sugg, _) = parse_registry_body(body).unwrap();
        assert_eq!(sugg.len(), 1);
        // `ws` is skipped; the `sse` remote is the accepted one.
        assert_eq!(sugg[0].remote_type.as_deref(), Some("sse"));
        assert_eq!(sugg[0].remote_url.as_deref(), Some("https://good.io/sse"));
    }

    // ── offline-shape: malformed JSON degrades to Err (caller shows "unavailable") ─
    #[test]
    fn malformed_json_is_error_not_silent_empty() {
        assert!(parse_registry_body("not json at all").is_err());
        assert!(parse_registry_body("{").is_err());
        // An empty servers array is a VALID empty result (clean degrade), not an error.
        let (sugg, next) = parse_registry_body(r#"{"servers": []}"#).unwrap();
        assert!(sugg.is_empty());
        assert!(next.is_none());
    }

    // ── oversized body rejected ──────────────────────────────────────────────────
    #[test]
    fn oversized_body_rejected() {
        let big = format!("{{\"servers\": []}}{}", " ".repeat(MAX_BODY_BYTES));
        assert!(parse_registry_body(&big).is_err());
    }

    // ── ⟨audit codex LOW⟩ `{}` (no servers field) is an error, not a silent empty ─
    #[test]
    fn empty_object_without_servers_field_is_error() {
        assert!(parse_registry_body("{}").is_err(), "{{}} is not a valid registry response");
        // But an explicit empty array is a clean empty result.
        assert!(parse_registry_body(r#"{"servers":[]}"#).unwrap().0.is_empty());
    }

    // ── ⟨audit HIGH/MED⟩ SSRF: base URL is validated (https + allowed host, no userinfo) ─
    #[test]
    fn build_url_ssrf_guard() {
        // Good base → builds the canonical endpoint with limit + encoded cursor.
        let u = build_servers_url("https://registry.modelcontextprotocol.io", Some("ab/c..@x")).unwrap();
        assert_eq!(u.host_str(), Some("registry.modelcontextprotocol.io"));
        assert_eq!(u.path(), "/v0/servers");
        // The hostile cursor is percent-encoded INSIDE the query value — host/path unchanged.
        let q = u.query().unwrap();
        assert!(q.contains("limit=100"));
        assert!(q.contains("cursor="));
        assert!(!u.as_str().contains("@x@"), "cursor cannot inject userinfo");
        // Rejected bases:
        assert!(build_servers_url("http://registry.modelcontextprotocol.io", None).is_err(), "http rejected");
        assert!(build_servers_url("https://evil.example.com", None).is_err(), "host not allowed");
        assert!(build_servers_url("https://user:pw@registry.modelcontextprotocol.io", None).is_err(), "userinfo rejected");
        assert!(build_servers_url("file:///etc/passwd", None).is_err());
        assert!(build_servers_url("https://localhost:9999", None).is_err());
        // A base with an attacker path/query is normalized to the canonical endpoint.
        let u2 = build_servers_url("https://registry.modelcontextprotocol.io/evil?x=1", None).unwrap();
        assert_eq!(u2.path(), "/v0/servers");
        assert_eq!(u2.query(), Some("limit=100"));
    }

    #[test]
    fn https_url_rejects_userinfo_and_no_host() {
        assert!(!is_https_url("https://user:pw@host.io/x"), "userinfo rejected");
        assert!(!is_https_url("https://user@host.io/x"), "username-only rejected");
        assert!(is_https_url("https://ok.io/path"));
        assert!(!is_https_url("http://ok.io/path"), "http rejected");
    }
}

