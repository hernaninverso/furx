// services/mcp_inject.rs — spec-011 · codebase-memory as a signed MCP plugin.
//
// Wires a SIGNED plugin's MCP-server descriptor into the config the agent CLI reads,
// GATED by that agent's plugin allow-list (006). The security contract is unchanged:
//   - only plugins INSTALLED under ~/.furx/plugins with a manifest that VERIFIES
//     against TRUSTED_PUBKEYS are eligible (fail-closed: bad/absent signature → skip);
//   - default-deny: a plugin is injected ONLY if it appears in the agent's allow-list
//     AND its manifest declares an `mcp` server (FR-003);
//   - placeholders ($PROJECT_ROOT/$PROJECT_KEY/$FURX_DATA) are resolved HERE, by the
//     runtime — never by the plugin — and the per-project store stays inside the
//     declared fs_write grant (FR-002 / FR-004).
//
// The MCP server itself is launched by the AGENT CLI (claude/codex/gemini), which we
// point at a per-agent `.mcp.json` we write under ~/.furx/mcp/<agent-key>.mcp.json.
// We do NOT run it through plugin_host::run_tool (that's the per-tool fire-and-forget
// path); an MCP server is long-lived and owned by the agent process. The signed
// manifest pins the exact `command` (the indexer binary), so `shell:false` still
// holds — the plugin can't launch an arbitrary shell, only its pinned binary.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::plugin_host::{McpServerSpec, Permissions, SignedManifest};

/// One resolved MCP server, ready to serialize into the agent CLI's mcpServers map.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Where Furx keeps per-agent MCP config it generates (separate from the user's own
/// ~/.claude.json so we never clobber it).
pub fn mcp_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("mcp"))
}

/// Per-project codebase store base: $FURX_DATA/codebase-memory  (== ~/.furx/codebase-memory).
pub fn codebase_store_base() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("codebase-memory"))
}

/// Slugify a project_key (a repo path, may contain '/') into a single filesystem-safe
/// segment for the per-project store dir. Stable + collision-resistant: replace every
/// non-[A-Za-z0-9._-] char with '-', then append a short hash of the original so two
/// different paths that slug to the same string don't collide.
pub fn project_key_slug(project_key: &str) -> String {
    let mapped: String = project_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('-');
    let base = if trimmed.is_empty() { "root" } else { trimmed };
    // short stable suffix so distinct keys never collide after slugging.
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(project_key.as_bytes());
    let hash = hex::encode(h.finalize());
    let base = if base.len() > 96 {
        &base[base.len() - 96..]
    } else {
        base
    };
    format!("{}-{}", base, &hash[..12])
}

/// Resolve the spec-011 placeholders in a single string. `$PROJECT_ROOT` → repo cwd,
/// `$PROJECT_KEY` → the stable project key (raw), `$FURX_DATA` → ~/.furx. The
/// per-project store is `$FURX_DATA/codebase-memory/<slug(project_key)>`; we expose it
/// as the resolved value of `$FURX_DATA/codebase-memory/$PROJECT_KEY` so the manifest
/// can write the canonical spec path while we keep it filesystem-safe.
fn expand_placeholders(s: &str, project_root: &str, project_key: &str, furx_data: &str) -> String {
    // Order matters: replace the longest tokens first. `$PROJECT_ROOT` and
    // `$PROJECT_KEY` share the `$PROJECT_` prefix but are distinct full tokens.
    s.replace("$PROJECT_ROOT", project_root)
        .replace("$FURX_DATA", furx_data)
        // The combined store path uses the SLUG so it's a single safe segment.
        .replace(
            "codebase-memory/$PROJECT_KEY",
            &format!("codebase-memory/{}", project_key_slug(project_key)),
        )
        // A bare $PROJECT_KEY elsewhere stays the raw key (e.g. as an MCP arg/env value).
        .replace("$PROJECT_KEY", project_key)
}

fn expand_vec(v: &[String], root: &str, key: &str, data: &str) -> Vec<String> {
    v.iter()
        .map(|s| expand_placeholders(s, root, key, data))
        .collect()
}

fn expand_env(
    e: &BTreeMap<String, String>,
    root: &str,
    key: &str,
    data: &str,
) -> BTreeMap<String, String> {
    e.iter()
        .map(|(k, val)| (k.clone(), expand_placeholders(val, root, key, data)))
        .collect()
}

/// Resolve an MCP server spec to a concrete `ResolvedMcpServer` for a given project.
pub fn resolve_server(
    name: &str,
    spec: &McpServerSpec,
    project_root: &str,
    project_key: &str,
    furx_data: &str,
) -> ResolvedMcpServer {
    ResolvedMcpServer {
        name: name.to_string(),
        command: expand_placeholders(&spec.command, project_root, project_key, furx_data),
        args: expand_vec(&spec.args, project_root, project_key, furx_data),
        env: expand_env(&spec.env, project_root, project_key, furx_data),
    }
}

/// Read + verify a plugin's manifest from an installed plugins dir. Fail-closed: an
/// unreadable / unparseable / unsigned / untrusted manifest yields `None`.
pub fn load_verified_manifest(plugins_base: &Path, name: &str) -> Option<SignedManifest> {
    if !is_safe_name(name) {
        return None;
    }
    let mpath = plugins_base.join(name).join("manifest.json");
    let text = std::fs::read_to_string(mpath).ok()?;
    let m: SignedManifest = serde_json::from_str(&text).ok()?;
    if m.name != name {
        return None;
    }
    if !m.verify() {
        return None; // bad/absent/untrusted signature → never inject
    }
    Some(m)
}

fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// CORE GATING (FR-003, default-deny). Given an agent's plugin allow-list and the set
/// of installed+verified MCP plugins, return the servers to inject. A plugin is
/// included ONLY if (a) it's in the allow-list, (b) it's installed+verified, (c) its
/// manifest declares an `mcp` server. Anything else → not injected.
///
/// `lookup` resolves a plugin name → its verified manifest (or None). Pure so it's
/// unit-testable without touching disk.
pub fn servers_for_allowlist<F>(
    allow_list: &[String],
    project_root: &str,
    project_key: &str,
    furx_data: &str,
    lookup: F,
) -> Vec<ResolvedMcpServer>
where
    F: Fn(&str) -> Option<SignedManifest>,
{
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in allow_list {
        if name.trim().is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let Some(m) = lookup(name) else { continue }; // not installed/verified → skip
        let Some(spec) = m.mcp.as_ref() else { continue }; // not an MCP server → skip
        out.push(resolve_server(
            &m.name,
            spec,
            project_root,
            project_key,
            furx_data,
        ));
    }
    out
}

/// Resolve the plugin's signed entrypoint to an ABSOLUTE path and verify its on-disk
/// bytes match the manifest's `entrypoint_sha256`. This is the SAME content binding the
/// Plugin Host uses for per-tool exec (plugin_host::file_sha256), reused for the MCP /
/// indexer launch path. Fail-closed: missing `entrypoint_sha256`, an unreadable file, or
/// a hash mismatch → Err (never inject / never exec).
pub fn verified_entrypoint_path(plugins_base: &Path, m: &SignedManifest) -> Result<PathBuf> {
    let expected = m.entrypoint_sha256.as_deref().ok_or_else(|| {
        anyhow!(
            "plugin '{}' has no entrypoint_sha256 → cannot bind command",
            m.name
        )
    })?;
    let abs = plugins_base.join(&m.name).join(&m.entrypoint);
    let got = super::plugin_host::file_sha256(&abs).map_err(|e| {
        anyhow!(
            "plugin '{}' entrypoint unreadable ({}): {e}",
            m.name,
            abs.display()
        )
    })?;
    if got != expected {
        return Err(anyhow!(
            "plugin '{}' entrypoint hash mismatch (on-disk {}… != signed {}…) → refusing",
            m.name,
            &got[..got.len().min(16)],
            &expected[..expected.len().min(16)]
        ));
    }
    Ok(abs)
}

/// Disk-backed, HASH-VERIFIED resolution of one MCP server for a project. Unlike the pure
/// `resolve_server`, this binds `mcp.command` to the signed entrypoint inside the
/// installed plugin dir and verifies its content hash (FR: the launched command must be
/// exactly the bytes the signature covers — a relative `run.sh` could otherwise be
/// PATH-hijacked or point at unsigned content). For auto-indexing code plugins it also
/// re-asserts the FR-002 default-deny permission contract. Fail-closed.
pub fn resolve_verified_server(
    plugins_base: &Path,
    m: &SignedManifest,
    project_root: &str,
    project_key: &str,
    furx_data: &str,
) -> Result<ResolvedMcpServer> {
    let spec = m
        .mcp
        .as_ref()
        .ok_or_else(|| anyhow!("plugin '{}' declares no mcp server", m.name))?;
    // The MCP command MUST be the signed entrypoint (nothing else is hash-covered).
    if spec.command.trim() != m.entrypoint {
        return Err(anyhow!(
            "plugin '{}' mcp.command ({:?}) must be the signed entrypoint ({:?})",
            m.name,
            spec.command,
            m.entrypoint
        ));
    }
    // Auto-indexing code plugins (index_command present) must satisfy FR-002 default-deny.
    if spec.index_command.is_some() {
        assert_codebase_permissions(&m.permissions)?;
    }
    let abs = verified_entrypoint_path(plugins_base, m)?;
    let mut r = resolve_server(&m.name, spec, project_root, project_key, furx_data);
    r.command = abs.to_string_lossy().into_owned();
    Ok(r)
}

/// Disk-backed, default-deny variant of `servers_for_allowlist`: loads + verifies each
/// allow-listed plugin's signed manifest from `plugins_base`, then HASH-VERIFIES the
/// entrypoint before resolving. A plugin that fails any check is silently skipped
/// (fail-closed) — never injected. Used by the productive spawn path.
pub fn servers_for_allowlist_verified(
    plugins_base: &Path,
    allow_list: &[String],
    project_root: &str,
    project_key: &str,
    furx_data: &str,
) -> Vec<ResolvedMcpServer> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in allow_list {
        if name.trim().is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let Some(m) = load_verified_manifest(plugins_base, name) else {
            continue;
        };
        if m.mcp.is_none() {
            continue; // classic (non-MCP) plugin → not injected here
        }
        match resolve_verified_server(plugins_base, &m, project_root, project_key, furx_data) {
            Ok(r) => out.push(r),
            Err(e) => tracing::warn!("mcp_inject: skipping plugin '{}': {}", name, e),
        }
    }
    out
}

/// Serialize the resolved servers into the `{"mcpServers": {...}}` JSON the agent CLI
/// (Claude Code / Codex / Gemini) reads. Empty list ⇒ `{"mcpServers":{}}` (no servers,
/// proves the default-deny path is verifiable — SC-003).
pub fn build_mcp_config(servers: &[ResolvedMcpServer]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for s in servers {
        let mut entry = serde_json::Map::new();
        entry.insert(
            "command".into(),
            serde_json::Value::String(s.command.clone()),
        );
        entry.insert("args".into(), serde_json::json!(s.args));
        if !s.env.is_empty() {
            entry.insert("env".into(), serde_json::json!(s.env));
        }
        map.insert(s.name.clone(), serde_json::Value::Object(entry));
    }
    serde_json::json!({ "mcpServers": serde_json::Value::Object(map) })
}

/// Write the per-agent MCP config to ~/.furx/mcp/<agent-key>.mcp.json (0600) and return
/// its path. The agent CLI is pointed at it via an env var / flag (see commands.rs).
/// Always writes (even an empty server map) so the config is deterministic per spawn.
pub fn write_agent_mcp_config(agent_key: &str, servers: &[ResolvedMcpServer]) -> Result<PathBuf> {
    let dir = mcp_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    // Harden the config dir itself to 0700 — it holds per-agent MCP configs (project
    // store paths). With the dir private, the temp+rename below can't be symlink-raced.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(md) = std::fs::metadata(&dir) {
            let mut perm = md.permissions();
            perm.set_mode(0o700);
            let _ = std::fs::set_permissions(&dir, perm);
        }
    }
    let safe: String = agent_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let path = dir.join(format!("{}.mcp.json", safe));
    let body = serde_json::to_string_pretty(&build_mcp_config(servers))?;
    // Write to a UNIQUE temp file created EXCLUSIVELY (create_new) with mode 0600 from
    // the start (no write-then-chmod window where the config is world-readable, and no
    // truncate of a pre-existing/symlinked predictable name), then atomically rename.
    let uniq = {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(".{}.{}.{}.mcp.json.tmp", safe, pid, nanos)
    };
    let tmp = dir.join(uniq);
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.flush()?;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(path)
}

/// Ensure the per-project store dir exists for a project_key and return it. This is the
/// `fs_write` target the manifest declares (FR-002): $FURX_DATA/codebase-memory/<slug>.
pub fn ensure_project_store(project_key: &str) -> Result<PathBuf> {
    let base = codebase_store_base()?;
    let dir = base.join(project_key_slug(project_key));
    // Defensive containment: project_key_slug already emits a single hashed segment (no
    // path separators, never bare `.`/`..`), but assert the result stays inside the store
    // base so any future change to the slug can never escape it (path-traversal guard).
    if !dir.starts_with(&base) {
        return Err(anyhow!(
            "refusing store path outside base: {}",
            dir.display()
        ));
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Sanity gate for what the manifest declared vs what the runtime guarantees: the
/// declared fs_write MUST include the per-project store, and net/shell/secrets MUST be
/// empty/false for an MCP code-graph plugin (FR-002 default-deny). Returns Err with a
/// human reason on violation (used at load time + in tests).
pub fn assert_codebase_permissions(perms: &Permissions) -> Result<()> {
    if !perms.net.is_empty() {
        return Err(anyhow!(
            "codebase-memory MCP must declare net:[] (got {:?})",
            perms.net
        ));
    }
    if perms.shell {
        return Err(anyhow!("codebase-memory MCP must declare shell:false"));
    }
    if !perms.secrets.is_empty() {
        return Err(anyhow!(
            "codebase-memory MCP must declare secrets:[] (BYOK; got {:?})",
            perms.secrets
        ));
    }
    let writes_store = perms.fs_write.iter().any(|p| p.contains("codebase-memory"));
    if !writes_store {
        return Err(anyhow!(
            "codebase-memory MCP must declare fs_write under $FURX_DATA/codebase-memory"
        ));
    }
    let reads_root = perms
        .fs_read
        .iter()
        .any(|p| p == "$PROJECT_ROOT" || p == ".");
    if !reads_root {
        return Err(anyhow!(
            "codebase-memory MCP must declare fs_read:[$PROJECT_ROOT]"
        ));
    }
    Ok(())
}

// ── spec-013 T041 — bundle catalog (tiers/categories) ────────────────────────
// The marketplace/installer (spec-002) lists the shipped, signed bundle plugins with
// a tier + category so the UI can group them. Tier/category are CURATED metadata that
// live here (not in the signed manifest — they're presentation, not a security
// contract, so keeping them out of the manifest avoids re-signing on a re-tag). A
// plugin not in this table is shown as tier "other".

/// One installable bundle plugin, as surfaced to the marketplace UI. `verified`
/// reflects whether its on-disk manifest verifies against the pinned key RIGHT NOW
/// (so a tampered/un-signed bundle entry shows as not-installable, fail-closed).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BundlePluginInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub tier: String,
    pub category: String,
    pub is_mcp: bool,
    pub verified: bool,
    /// Declared net hosts (for the UI to show "reaches: api.github.com" etc.). Empty = offline.
    pub net: Vec<String>,
    /// Declared secret names the plugin needs granted (BYOK). Empty = none.
    pub secrets: Vec<String>,
}

/// Curated (tier, category) for a bundle plugin name. Tiers follow spec-013:
/// "tier-1" (highest-impact coding MCPs), "tier-2" (LSP/docs/git), "first-party"
/// (codebase-memory — ours), "tool" (the classic per-tool bundle plugins).
fn tier_category_for(name: &str) -> (&'static str, &'static str) {
    match name {
        // spec-013 Tier 1
        "serena" => ("tier-1", "code-intelligence"),
        "github-mcp" => ("tier-1", "vcs-remote"),
        "codanna" => ("tier-1", "code-intelligence"),
        "test-coverage" => ("tier-1", "testing"),
        // spec-013 Tier 2
        "mcp-language-server" => ("tier-2", "code-intelligence"),
        "git-mcp" => ("tier-2", "vcs-local"),
        "context7" => ("tier-2", "docs"),
        // spec-011 code-graph MCP
        "codebase-memory" => ("first-party", "code-intelligence"),
        // classic per-tool plugins (spec-001/002)
        _ => ("tool", "utility"),
    }
}

/// Enumerate the shipped bundle plugins under `bundle_dir` with tier/category metadata.
/// Verification is best-effort per entry (a bad manifest is listed with verified=false
/// rather than hidden, so the UI can warn). Sorted by (tier, name) for stable display.
pub fn bundle_catalog(bundle_dir: &Path) -> Vec<BundlePluginInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(bundle_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) if is_safe_name(n) => n.to_string(),
            _ => continue,
        };
        let mpath = p.join("manifest.json");
        let Ok(text) = std::fs::read_to_string(&mpath) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<SignedManifest>(&text) else {
            continue;
        };
        if m.name != name {
            continue;
        }
        let (tier, category) = tier_category_for(&name);
        out.push(BundlePluginInfo {
            name: m.name.clone(),
            version: m.version.clone(),
            description: m.description.clone(),
            tier: tier.to_string(),
            category: category.to_string(),
            is_mcp: m.mcp.is_some(),
            verified: m.verify(),
            net: m.permissions.net.clone(),
            secrets: m.permissions.secrets.clone(),
        });
    }
    // tier order: tier-1, tier-2, first-party, tool, then by name.
    fn tier_rank(t: &str) -> u8 {
        match t {
            "tier-1" => 0,
            "tier-2" => 1,
            "first-party" => 2,
            "tool" => 3,
            _ => 4,
        }
    }
    out.sort_by(|a, b| {
        tier_rank(&a.tier)
            .cmp(&tier_rank(&b.tier))
            .then(a.name.cmp(&b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed_with_real_key(
        name: &str,
        mcp: Option<McpServerSpec>,
        perms: Permissions,
    ) -> SignedManifest {
        // Sign with a throwaway key, but tests that need REAL trust use load_verified
        // against the bundle. Here we exercise the *gating logic* with manifests whose
        // verify() we bypass by injecting them directly via the `lookup` closure.
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let eng = base64::engine::general_purpose::STANDARD;
        let mut m = SignedManifest {
            name: name.into(),
            version: "1.0.0".into(),
            description: None,
            entrypoint: "run.sh".into(),
            entrypoint_sha256: None,
            permissions: perms,
            signature: None,
            pubkey: Some(eng.encode(sk.verifying_key().to_bytes())),
            mcp,
        };
        let bytes = m.signing_bytes().unwrap();
        m.signature = Some(eng.encode(sk.sign(&bytes).to_bytes()));
        m
    }

    fn cm_spec() -> McpServerSpec {
        McpServerSpec {
            command: "/bin/codebase-memory-mcp".into(),
            args: vec![],
            env: BTreeMap::new(),
            index_command: None,
        }
    }

    #[test]
    fn placeholder_expansion() {
        let s = expand_placeholders("$PROJECT_ROOT/x", "/repo", "/repo", "/h/.furx");
        assert_eq!(s, "/repo/x");
        let store = expand_placeholders(
            "$FURX_DATA/codebase-memory/$PROJECT_KEY",
            "/repo",
            "/Users/h/My Repo",
            "/h/.furx",
        );
        assert!(store.starts_with("/h/.furx/codebase-memory/"));
        assert!(
            !store.contains(' '),
            "store path must be a single safe segment: {}",
            store
        );
        // a bare $PROJECT_KEY (not the store combo) stays the raw key.
        assert_eq!(
            expand_placeholders("key=$PROJECT_KEY", "/r", "/a/b", "/d"),
            "key=/a/b"
        );
    }

    #[test]
    fn slug_is_stable_and_distinct() {
        assert_eq!(project_key_slug("/a/b"), project_key_slug("/a/b"));
        assert_ne!(project_key_slug("/a/b"), project_key_slug("/a/c"));
        // collision-prone inputs (slug to same base) stay distinct via hash suffix.
        assert_ne!(project_key_slug("/a/b"), project_key_slug("/a-b"));
    }

    #[test]
    fn gating_includes_only_allowlisted_mcp_plugins() {
        // installed+verified set: an MCP plugin + a classic (no mcp) plugin.
        let cm = signed_with_real_key(
            "codebase-memory",
            Some(cm_spec()),
            Permissions {
                fs_read: vec!["$PROJECT_ROOT".into()],
                fs_write: vec!["$FURX_DATA/codebase-memory/$PROJECT_KEY".into()],
                ..Default::default()
            },
        );
        let classic = signed_with_real_key("filesystem-ls", None, Permissions::default());
        let lookup = |name: &str| -> Option<SignedManifest> {
            match name {
                "codebase-memory" => Some(cm.clone()),
                "filesystem-ls" => Some(classic.clone()),
                _ => None, // not installed
            }
        };

        // allow-list HAS codebase-memory → injected.
        let got = servers_for_allowlist(
            &["codebase-memory".into()],
            "/repo",
            "/repo",
            "/h/.furx",
            lookup,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "codebase-memory");

        // allow-list has a classic (non-MCP) plugin only → NOT injected.
        let got = servers_for_allowlist(
            &["filesystem-ls".into()],
            "/repo",
            "/repo",
            "/h/.furx",
            lookup,
        );
        assert!(got.is_empty(), "non-MCP plugin must not inject a server");

        // allow-list references an un-installed plugin → NOT injected (default-deny).
        let got = servers_for_allowlist(&["ghost".into()], "/repo", "/repo", "/h/.furx", lookup);
        assert!(got.is_empty());

        // EMPTY allow-list → no servers (SC-003).
        let got = servers_for_allowlist(&[], "/repo", "/repo", "/h/.furx", lookup);
        assert!(got.is_empty());
    }

    #[test]
    fn build_config_shape() {
        let servers = vec![ResolvedMcpServer {
            name: "codebase-memory".into(),
            command: "/bin/cm".into(),
            args: vec!["--stdio".into()],
            env: {
                let mut e = BTreeMap::new();
                e.insert("X".into(), "y".into());
                e
            },
        }];
        let cfg = build_mcp_config(&servers);
        let cm = &cfg["mcpServers"]["codebase-memory"];
        assert_eq!(cm["command"], "/bin/cm");
        assert_eq!(cm["args"][0], "--stdio");
        assert_eq!(cm["env"]["X"], "y");
        // empty list → empty map (verifiable default-deny).
        assert_eq!(build_mcp_config(&[]), serde_json::json!({"mcpServers": {}}));
    }

    #[test]
    fn permissions_contract_enforced() {
        let ok = Permissions {
            fs_read: vec!["$PROJECT_ROOT".into()],
            fs_write: vec!["$FURX_DATA/codebase-memory/$PROJECT_KEY".into()],
            ..Default::default()
        };
        assert!(assert_codebase_permissions(&ok).is_ok());
        // net → reject
        let mut bad = ok.clone();
        bad.net = vec!["*".into()];
        assert!(assert_codebase_permissions(&bad).is_err());
        // shell → reject
        let mut bad = ok.clone();
        bad.shell = true;
        assert!(assert_codebase_permissions(&bad).is_err());
        // secret → reject
        let mut bad = ok.clone();
        bad.secrets = vec!["OPENAI_API_KEY".into()];
        assert!(assert_codebase_permissions(&bad).is_err());
        // missing store write → reject
        let mut bad = ok.clone();
        bad.fs_write = vec![];
        assert!(assert_codebase_permissions(&bad).is_err());
    }

    #[test]
    fn deduplicates_allowlist() {
        let cm = signed_with_real_key(
            "codebase-memory",
            Some(cm_spec()),
            Permissions {
                fs_read: vec!["$PROJECT_ROOT".into()],
                fs_write: vec!["$FURX_DATA/codebase-memory/$PROJECT_KEY".into()],
                ..Default::default()
            },
        );
        let lookup = |name: &str| {
            if name == "codebase-memory" {
                Some(cm.clone())
            } else {
                None
            }
        };
        let got = servers_for_allowlist(
            &["codebase-memory".into(), "codebase-memory".into()],
            "/repo",
            "/repo",
            "/h/.furx",
            lookup,
        );
        assert_eq!(
            got.len(),
            1,
            "duplicate allow-list entries must not duplicate the server"
        );
    }

    // REAL trust: the shipped bundle codebase-memory manifest verifies against the
    // pinned TRUSTED_PUBKEYS and declares an MCP server with FR-002 permissions.
    #[test]
    fn bundle_codebase_memory_verifies_and_is_mcp() {
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("plugins")
            .join("bundle");
        let m = load_verified_manifest(&bundle, "codebase-memory");
        let Some(m) = m else {
            // bundle not present in this checkout (e.g. shallow CI) → skip.
            return;
        };
        assert!(
            m.mcp.is_some(),
            "bundle codebase-memory must declare an MCP server"
        );
        assert!(
            assert_codebase_permissions(&m.permissions).is_ok(),
            "bundle codebase-memory must satisfy FR-002 default-deny perms"
        );
        let spec = m.mcp.unwrap();
        // resolves cleanly with placeholders.
        let r = resolve_server(&m.name, &spec, "/repo", "/repo", "/h/.furx");
        assert_eq!(r.name, "codebase-memory");
        assert!(!r.command.is_empty());
    }

    // ── spec-013 — Tier 1/2 bundle plugins ───────────────────────────
    // Each shipped MCP plugin verifies against the pinned key, declares an MCP server,
    // and its mcp.command is its signed entrypoint (so resolve_verified_server binds it).
    fn assert_bundle_mcp(name: &str) -> SignedManifest {
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("plugins")
            .join("bundle");
        let m = load_verified_manifest(&bundle, name).unwrap_or_else(|| {
            panic!("bundle plugin '{name}' must load + verify against pinned key")
        });
        assert!(m.mcp.is_some(), "{name} must declare an MCP server");
        let spec = m.mcp.as_ref().unwrap();
        assert_eq!(
            spec.command.trim(),
            m.entrypoint,
            "{name} mcp.command must be the signed entrypoint"
        );
        assert!(
            m.entrypoint_sha256.is_some(),
            "{name} must bind entrypoint_sha256"
        );
        // hash-verified resolution must succeed (entrypoint bytes match the manifest).
        let r = resolve_verified_server(&bundle, &m, "/repo", "/repo", "/h/.furx")
            .unwrap_or_else(|e| panic!("{name} entrypoint must hash-verify: {e}"));
        assert!(
            std::path::Path::new(&r.command).is_absolute(),
            "{name} command must resolve absolute"
        );
        m
    }

    #[test]
    fn bundle_tier1_offline_plugins_default_deny() {
        // Serena / codanna / test-coverage: offline (net:[]), no
        // secrets, read the project root. (FR-002 default-deny.)
        for name in ["serena", "codanna", "test-coverage"] {
            let m = assert_bundle_mcp(name);
            let p = &m.permissions;
            assert!(p.net.is_empty(), "{name} must declare net:[] (offline)");
            assert!(p.secrets.is_empty(), "{name} must declare secrets:[]");
            assert!(!p.shell, "{name} must declare shell:false");
            assert!(
                p.fs_read.iter().any(|x| x == "$PROJECT_ROOT"),
                "{name} must read $PROJECT_ROOT"
            );
        }
    }

    #[test]
    fn bundle_github_mcp_is_byok_and_host_scoped() {
        // FR-002: github-mcp → net:["api.github.com"] (default-deny: ONLY that host),
        // secret GITHUB_PERSONAL_ACCESS_TOKEN (BYOK), no fs grant.
        let m = assert_bundle_mcp("github-mcp");
        let p = &m.permissions;
        assert_eq!(
            p.net,
            vec!["api.github.com".to_string()],
            "github-mcp net must be exactly api.github.com"
        );
        assert!(
            !p.net.iter().any(|h| h == "*"),
            "github-mcp must NEVER grant net:[\"*\"]"
        );
        assert_eq!(p.secrets, vec!["GITHUB_PERSONAL_ACCESS_TOKEN".to_string()]);
        assert!(
            p.fs_read.is_empty() && p.fs_write.is_empty(),
            "github-mcp must not grant fs"
        );
        assert!(!p.shell);
    }

    #[test]
    fn bundle_tier2_plugins_present_and_scoped() {
        // mcp-language-server: offline LSP, no secrets, reads project.
        let m = assert_bundle_mcp("mcp-language-server");
        assert!(
            m.permissions.net.is_empty(),
            "mcp-language-server must be offline"
        );
        assert!(m.permissions.secrets.is_empty());
        // git-mcp: local git, offline, no env secrets.
        let m = assert_bundle_mcp("git-mcp");
        assert!(m.permissions.net.is_empty(), "git-mcp must be local-only");
        assert!(m.permissions.secrets.is_empty());
        // context7: OPT-IN hosted → net allowlist (its hosts only, never "*"), BYOK key.
        let m = assert_bundle_mcp("context7");
        assert!(
            !m.permissions.net.is_empty(),
            "context7 needs a net allowlist"
        );
        assert!(
            !m.permissions.net.iter().any(|h| h == "*"),
            "context7 must NEVER grant net:[\"*\"]"
        );
        assert!(
            m.permissions
                .net
                .iter()
                .all(|h| h.ends_with("context7.com")),
            "context7 net must be its own hosts only"
        );
        assert_eq!(m.permissions.secrets, vec!["CONTEXT7_API_KEY".to_string()]);
    }

    #[test]
    fn bundle_catalog_lists_tiers_and_verifies() {
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("plugins")
            .join("bundle");
        if !bundle.is_dir() {
            return;
        }
        let cat = bundle_catalog(&bundle);
        assert!(!cat.is_empty(), "catalog must list bundle plugins");
        // every shipped entry must verify (we ship only signed plugins).
        for info in &cat {
            assert!(
                info.verified,
                "bundle '{}' must verify against pinned key",
                info.name
            );
        }
        // tier-1 plugins are present and tagged.
        let serena = cat
            .iter()
            .find(|p| p.name == "serena")
            .expect("serena in catalog");
        assert_eq!(serena.tier, "tier-1");
        assert!(serena.is_mcp);
        // github-mcp surfaces its net host + BYOK secret for the UI.
        let gh = cat
            .iter()
            .find(|p| p.name == "github-mcp")
            .expect("github-mcp in catalog");
        assert_eq!(gh.net, vec!["api.github.com".to_string()]);
        assert_eq!(gh.secrets, vec!["GITHUB_PERSONAL_ACCESS_TOKEN".to_string()]);
        // first-party tagging.
        assert_eq!(
            cat.iter().find(|p| p.name == "codebase-memory").unwrap().tier,
            "first-party"
        );
        // sorted tier-1 first.
        assert_eq!(cat.first().unwrap().tier, "tier-1");
    }

    // HASH BINDING (audit HIGH fix): the disk-backed resolver must bind the command to
    // the signed entrypoint, verify its on-disk hash, and use the ABSOLUTE path. A
    // tampered entrypoint (hash mismatch) must be refused.
    #[test]
    fn verified_resolver_binds_and_hashes_entrypoint() {
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("plugins")
            .join("bundle");
        let Some(m) = load_verified_manifest(&bundle, "codebase-memory") else {
            return;
        };
        // happy path: resolves to the absolute, hash-verified entrypoint.
        let r = resolve_verified_server(&bundle, &m, "/repo", "/repo", "/h/.furx")
            .expect("bundle entrypoint must verify");
        let abs = bundle.join("codebase-memory").join(&m.entrypoint);
        assert_eq!(r.command, abs.to_string_lossy());
        assert!(std::path::Path::new(&r.command).is_absolute());

        // tamper: a manifest claiming a wrong entrypoint_sha256 must be rejected.
        let mut bad = m.clone();
        bad.entrypoint_sha256 = Some("0".repeat(64));
        assert!(
            resolve_verified_server(&bundle, &bad, "/repo", "/repo", "/h/.furx").is_err(),
            "hash mismatch must refuse to resolve the command"
        );
        // a manifest with NO entrypoint_sha256 must also be rejected (fail-closed).
        let mut nohash = m.clone();
        nohash.entrypoint_sha256 = None;
        assert!(resolve_verified_server(&bundle, &nohash, "/repo", "/repo", "/h/.furx").is_err());
    }

    // 040 AJ-001 (P3) — the per-agent MCP config MUST land on disk as 0600 (u+rw only):
    // it points at per-project store paths and is read by the agent CLI. The test owns its
    // cleanup via an inline RAII Guard so a `metadata().unwrap()` panic (file missing, perms)
    // can never leave garbage behind, and it scrubs leftovers from a prior run up front.
    // No external deps (NO tempfile): Rust runs Drop on unwind, which is the guarantee we need.
    #[test]
    #[cfg(unix)]
    fn write_agent_mcp_config_is_mode_0600() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        struct Guard(PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }

        // Unique per process so two concurrent `cargo test` runs never clash on the file.
        let agent_key = format!("test-perm-{}", std::process::id());
        let path = mcp_config_dir()
            .expect("mcp_config_dir")
            .join(format!("{}.mcp.json", agent_key));
        let _ = fs::remove_file(&path); // scrub leftovers from a previous run
        let _guard = Guard(path.clone()); // cleanup even if metadata() below panics

        let servers = vec![ResolvedMcpServer {
            name: "test-server".into(),
            command: "/bin/true".into(),
            args: vec![],
            env: BTreeMap::new(),
        }];
        write_agent_mcp_config(&agent_key, &servers).expect("write ok");
        let mode = fs::metadata(&path)
            .expect("metadata must exist post-write")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "config must be u+rw only (0600)");
        // _guard runs Drop() on scope exit, even if the assert above panics.
    }
}
