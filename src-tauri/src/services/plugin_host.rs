// spec-kit 001 · US2 — Plugin Host (MCP).
//
// Adds the security + runtime layer on top of the existing `plugins.rs` registry
// (which owns install/list/enabled in SQLite). Council rules enforced here:
//   - manifest is Ed25519-SIGNED; invalid/absent signature → never load (FR-014)
//   - permissions default-DENY; net/fs/shell/secrets only by explicit grant (FR-012)
//   - secrets/API keys NEVER passed to a plugin without a grant (FR-013, F-I BYOK)
//   - plugins run OUTSIDE the main process (subprocess; WASM lands in Fase 2) (FR-011)
//   - net-deny is FAIL-CLOSED: a plugin without a `net` grant is wrapped in an OS
//     sandbox that blocks network (sandbox-exec on macOS, `unshare -n`/firejail on
//     Linux); if no sandbox tool is available we REFUSE to run it (never silently
//     allow network). True memory/syscall isolation is the WASM/seccomp path (Fase 2/3).
//   - every load/invoke/access is audited by the caller (FR-015).

use anyhow::{anyhow, Result};
use base64::Engine;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Declared, default-deny permission set. Empty == no access to anything.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Permissions {
    /// Allowed outbound network hosts (exact host match). Empty = no network.
    #[serde(default)]
    pub net: Vec<String>,
    /// Filesystem paths the plugin may read.
    #[serde(default)]
    pub fs_read: Vec<String>,
    /// Filesystem paths the plugin may write.
    #[serde(default)]
    pub fs_write: Vec<String>,
    /// May the plugin run shell/exec beyond its own entrypoint?
    #[serde(default)]
    pub shell: bool,
    /// Names of secrets (e.g. provider API keys) the plugin is granted. BYOK gate:
    /// a secret is passed to the plugin ONLY if its name appears here.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// spec-013 (T030) — Roots/readonly model (from the official filesystem MCP). A
    /// dynamic allowlist of filesystem ROOTS the plugin may access, each with an
    /// explicit `readonly` flag. This is the structured superset of `fs_read`/`fs_write`:
    /// a root with `readonly:true` is read-only (like an entry in `fs_read`), a root with
    /// `readonly:false` is read-write (like `fs_write`). It lets a plugin declare, e.g.,
    /// "the repo is writable but my config dir is read-only" without overloading two flat
    /// lists.
    ///
    /// BACK-COMPAT (critical): this field is `skip_serializing_if = Vec::is_empty`, so a
    /// manifest that doesn't use it serializes byte-identically to before → existing
    /// Ed25519 signatures stay valid (the signature covers the canonical JSON, and an
    /// absent/empty `fs_roots` is simply not emitted). Manifests keep verifying.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fs_roots: Vec<FsRoot>,
}

/// spec-013 (T030) — one filesystem root in the Roots/readonly model. `path` may carry
/// the same placeholders as `fs_read`/`fs_write` ($PROJECT_ROOT/$PROJECT_KEY/$FURX_DATA),
/// resolved by the runtime — never the plugin. `readonly` is the access mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FsRoot {
    pub path: String,
    /// When true the root is read-only (no writes). Defaults to read-only (fail-safe):
    /// an under-specified manifest grants the LEAST access.
    #[serde(default = "default_true")]
    pub readonly: bool,
}

fn default_true() -> bool {
    true
}

impl Permissions {
    /// Whether to grant FULL network (skip the net-deny sandbox). Codex audit: the
    /// OS net sandbox is all-or-nothing, so per-host allowlists can't be enforced
    /// yet. Only `net:["*"]` grants full network; a specific-host list is NOT
    /// honored as an allowlist (no proxy) → such a plugin still runs sandboxed
    /// (net-deny, fail-safe) rather than getting un-enforced full network.
    pub fn grants_net(&self) -> bool {
        self.net.iter().any(|h| h == "*")
    }
    /// Specific allowed hosts (spec-004): non-"*" entries. Empty ⇒ no per-host grant
    /// (either full "*" or net-deny). These run through the egress proxy + sandbox.
    pub fn net_hosts(&self) -> Vec<String> {
        if self.grants_net() {
            return vec![];
        }
        self.net.iter().filter(|h| !h.is_empty()).cloned().collect()
    }
    pub fn grants_secret(&self, name: &str) -> bool {
        self.secrets.iter().any(|s| s == name)
    }

    /// spec-013 (T030) — the effective set of READABLE roots: the flat `fs_read` list
    /// PLUS the flat `fs_write` list (a writable path is also readable) PLUS every
    /// `fs_roots` entry (readonly OR read-write). De-duplicated, order-stable (fs_read,
    /// fs_write, then fs_roots). Single source of truth a future fs-sandbox consults.
    pub fn readable_roots(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |p: &str| {
            if !p.is_empty() && !out.iter().any(|e| e == p) {
                out.push(p.to_string());
            }
        };
        for p in &self.fs_read {
            push(p);
        }
        for p in &self.fs_write {
            push(p);
        }
        for r in &self.fs_roots {
            push(&r.path);
        }
        out
    }

    /// spec-013 (T030) — the effective set of WRITABLE roots: the flat `fs_write` list
    /// PLUS every `fs_roots` entry whose `readonly` is false. De-duplicated, order-stable.
    /// A `readonly:true` root is NEVER writable (Roots/readonly contract).
    pub fn writable_roots(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |p: &str| {
            if !p.is_empty() && !out.iter().any(|e| e == p) {
                out.push(p.to_string());
            }
        };
        for p in &self.fs_write {
            push(p);
        }
        for r in &self.fs_roots {
            if !r.readonly {
                push(&r.path);
            }
        }
        out
    }

    /// spec-013 (T030) — whether a given (already-resolved) path is WRITABLE under this
    /// permission set: it must be inside some writable root, AND must NOT be inside a
    /// root that is declared read-only (readonly wins — a path covered by both a
    /// writable and a readonly root is treated as read-only, fail-safe). Prefix match on
    /// path segments (so `/a/b` does not match `/a/bc`).
    pub fn allows_write(&self, resolved_path: &str) -> bool {
        // readonly roots veto writes to anything under them.
        for r in &self.fs_roots {
            if r.readonly && path_within(resolved_path, &r.path) {
                return false;
            }
        }
        self.writable_roots()
            .iter()
            .any(|root| path_within(resolved_path, root))
    }

    /// spec-013 (T030) — whether a given (already-resolved) path is READABLE under this
    /// permission set: inside any readable root (flat or structured). Prefix match on
    /// path segments.
    pub fn allows_read(&self, resolved_path: &str) -> bool {
        self.readable_roots()
            .iter()
            .any(|root| path_within(resolved_path, root))
    }
}

/// Segment-aware prefix containment: is `child` the same as, or nested under, `root`?
/// Compares path components so `/a/b` contains `/a/b/c` but NOT `/a/bc`. A bare `.`
/// root matches everything (used by some manifests to mean "the project cwd").
///
/// SECURITY CONTRACT (audit codex+deepseek 013): this is a LEXICAL predicate. It is
/// fail-closed against `..` traversal — any `ParentDir` component in `child` or `root`
/// makes it return `false`, because a `..` means the path is NOT resolved and could
/// escape the root (`/repo/../etc`). SYMLINK resolution is NOT done here (it needs I/O
/// and the path may not exist yet); the ENFORCEMENT layer that wires `allows_read`/
/// `allows_write` to real filesystem access MUST pass an already-`canonicalize`d path
/// (the param is named `resolved_path`). Today these helpers are a declared model + API
/// (not yet syscall-enforced), so this contract guards the future enforcement caller.
fn path_within(child: &str, root: &str) -> bool {
    use std::path::Component;
    // Reject unresolved paths: a `..` component means containment can't be proven
    // lexically (it could escape the root). Fail-closed.
    let has_parent = |p: &str| {
        std::path::Path::new(p)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    };
    if has_parent(child) || has_parent(root) {
        return false;
    }
    if root == "." || root == child {
        return true;
    }
    let c = std::path::Path::new(child);
    let r = std::path::Path::new(root);
    c.starts_with(r)
}

/// Public keys the app TRUSTS to sign plugins, pinned out-of-band (baked into the
/// binary). Codex audit: trusting the `pubkey` embedded in the manifest is a
/// signature bypass — an attacker swaps in their own keypair. The trust decision
/// uses ONLY this pinned set; `manifest.pubkey` must be a member to be accepted.
/// Production keys are injected via the `FURX_TRUSTED_PLUGIN_KEYS` build/env list;
/// the array below is the compiled-in default (empty until a real signing key is
/// provisioned — fail-closed: with no trusted key, NOTHING verifies).
pub const TRUSTED_PUBKEYS: &[&str] = &[
    // Furx project signing key (Ed25519). Private key in Keychain
    // `furx-plugin-signing-key`. Bundle plugins are signed with this; rotate by
    // appending the new pubkey here and re-signing.
    "bgkQbOB0kcIRVzmmv7FVf8Bm2Cx/UPsDbT0VNcBrOp8=",
];

/// A signed plugin manifest. The `signature` is base64(Ed25519 over the canonical
/// JSON of every field EXCEPT `signature`). `pubkey` is base64 of the 32-byte key
/// and MUST be in the pinned trusted set. `entrypoint_sha256` binds the manifest
/// to the exact executable content (closes TOCTOU + "signature doesn't cover the
/// binary").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Program to run for this plugin (resolved relative to the plugin dir).
    pub entrypoint: String,
    /// hex SHA-256 of the entrypoint file. Verified before exec.
    #[serde(default)]
    pub entrypoint_sha256: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// base64 Ed25519 signature over the manifest sans `signature`.
    #[serde(default)]
    pub signature: Option<String>,
    /// base64 of the 32-byte Ed25519 public key — MUST be in TRUSTED_PUBKEYS.
    #[serde(default)]
    pub pubkey: Option<String>,
    /// spec-011 — optional MCP-server descriptor. When present, this plugin is an
    /// MCP server (long-lived stdio process the AGENT CLI launches), not a
    /// fire-and-forget per-tool subprocess. Furx injects it into the agent's MCP
    /// config ONLY when the plugin is in that agent's allow-list (006). The
    /// signature still covers this field (it's part of the manifest), and the
    /// declared `permissions` (FR-002) remain the security contract. `None` for
    /// the classic per-tool bundle plugins (filesystem-ls, http-get, …) so they
    /// keep verifying unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpServerSpec>,
}

/// spec-011 — how to launch this plugin as an MCP server inside the agent's CLI.
/// `command`/`args`/`env` accept the placeholders `$PROJECT_ROOT`, `$PROJECT_KEY`,
/// `$FURX_DATA` (resolved at spawn against the pane cwd + per-project store), which
/// MUST stay inside the declared `fs_read`/`fs_write` grants — the runtime resolves
/// them, never the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpServerSpec {
    /// Program to launch (absolute path or PATH name). May contain placeholders.
    pub command: String,
    /// argv for the MCP server. May contain placeholders.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra env for the server process (placeholder-expanded). Used e.g. to point
    /// the indexer's store at `$FURX_DATA/codebase-memory/$PROJECT_KEY`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Optional indexer invocation: a `cli`-style subcommand the runtime runs in the
    /// background (FR-004) to (re)index a project. Same placeholder rules. When set,
    /// `project.opened` enqueues a background index job. `None` ⇒ no auto-index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_command: Option<McpIndexSpec>,
}

/// spec-011 — background indexer invocation (FR-004). `args` typically encodes the
/// tool call, e.g. `["cli", "index_repository", "{\"path\":\"$PROJECT_ROOT\"}"]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct McpIndexSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl SignedManifest {
    /// Canonical bytes signed: the manifest serialized with `signature` cleared.
    /// Deterministic via BTreeMap ordering so signer and verifier agree.
    /// Public so the `furx_sign` example produces byte-identical input.
    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        let mut m = self.clone();
        m.signature = None;
        // Re-serialize through a sorted map for determinism.
        let v = serde_json::to_value(&m)?;
        let canonical = canonicalize(&v);
        Ok(canonical.into_bytes())
    }

    /// Verify against the pinned trusted key set. Fail-closed.
    pub fn verify(&self) -> bool {
        let trusted: Vec<String> = TRUSTED_PUBKEYS.iter().map(|s| s.to_string()).collect();
        self.verify_with_trusted(&trusted)
    }

    /// Core verification: (1) sig+pubkey present, (2) the claimed pubkey is in the
    /// trusted set (out-of-band pin — NOT trusting the manifest's own key blindly),
    /// (3) the Ed25519 signature is valid over the canonical bytes. Fail-closed.
    pub fn verify_with_trusted(&self, trusted: &[String]) -> bool {
        let (Some(sig_b64), Some(pk_b64)) = (&self.signature, &self.pubkey) else {
            return false;
        };
        if !trusted.iter().any(|t| t == pk_b64) {
            return false; // unknown/attacker key → reject (closes the bypass)
        }
        verify_signature(&self.signing_bytes().unwrap_or_default(), sig_b64, pk_b64)
    }
}

// ── Ask-on-first-use consent store (Fase 2, T029) ───────────────────────────
// The manifest DECLARES permissions; the user must CONSENT before a plugin runs.
// Consent is recorded per (name, version): bumping a plugin's version invalidates
// the grant (re-prompt), since new code may request more. Stored as JSON at
// ~/.furx/plugin-grants.json. Default-deny: no entry → not granted.

// Process-wide lock for all grant-file read-modify-write (consent + secrets).
// Codex audit: concurrent grant/revoke without a lock can lose updates or
// resurrect a revoked grant. Serialize them.
static GRANT_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// Write `contents` to `path` with private (0600) perms on unix (frontier audit:
/// the grant JSON holds Keychain refs; don't leave it world-readable).
fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

fn grants_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("plugin-grants.json"))
}

type GrantStore = std::collections::HashMap<String, String>; // name → granted version

fn load_grants() -> GrantStore {
    grants_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Has the user consented to this plugin at this exact version?
pub fn is_granted(name: &str, version: &str) -> bool {
    load_grants()
        .get(name)
        .map(|v| v == version)
        .unwrap_or(false)
}

/// Record user consent for (name, version). Persists to ~/.furx/plugin-grants.json.
pub fn grant(name: &str, version: &str) -> Result<()> {
    let _g = GRANT_LOCK.lock();
    let path = grants_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut store = load_grants();
    store.insert(name.to_string(), version.to_string());
    write_private(&path, &serde_json::to_string_pretty(&store)?)?;
    Ok(())
}

/// Revoke consent (kill switch / user revocation). Also revokes the plugin's
/// secret-grants (spec-003): killing a plugin must drop its secret access.
pub fn revoke(name: &str) -> Result<()> {
    {
        let _g = GRANT_LOCK.lock(); // scoped: released before revoke_all_secrets re-locks
        let path = grants_path()?;
        let mut store = load_grants();
        if store.remove(name).is_some() {
            write_private(&path, &serde_json::to_string_pretty(&store)?)?;
        }
    }
    let _ = revoke_all_secrets(name);
    Ok(())
}

// ── BYOK secret grants (spec-003, T) ────────────────────────────────────────
// Maps (plugin, secret_name) → a Keychain reference {service, account}. The VALUE
// is read from the OS Keychain at invoke time and never persisted in the store.
// A secret can only be granted if the plugin's manifest declares it.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct KeychainRef {
    pub service: String,
    pub account: String,
}
type SecretStore =
    std::collections::HashMap<String, std::collections::HashMap<String, KeychainRef>>;

fn secret_grants_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("plugin-secret-grants.json"))
}
fn load_secret_grants() -> SecretStore {
    secret_grants_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
fn save_secret_grants(store: &SecretStore) -> Result<()> {
    let path = secret_grants_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_private(&path, &serde_json::to_string_pretty(store)?)
}

/// Grant `plugin` access to `secret_name`, backed by a Keychain entry. Caller MUST
/// have verified that `secret_name` is declared in the plugin's manifest.
pub fn grant_secret(plugin: &str, secret_name: &str, kc: KeychainRef) -> Result<()> {
    let _g = GRANT_LOCK.lock();
    let mut store = load_secret_grants();
    store
        .entry(plugin.to_string())
        .or_default()
        .insert(secret_name.to_string(), kc);
    save_secret_grants(&store)
}

/// Revoke one secret grant.
pub fn revoke_secret(plugin: &str, secret_name: &str) -> Result<()> {
    let _g = GRANT_LOCK.lock();
    let mut store = load_secret_grants();
    if let Some(m) = store.get_mut(plugin) {
        m.remove(secret_name);
        if m.is_empty() {
            store.remove(plugin);
        }
        save_secret_grants(&store)?;
    }
    Ok(())
}

/// Revoke all secret grants for a plugin (called on plugin revoke / re-install).
pub fn revoke_all_secrets(plugin: &str) -> Result<()> {
    let _g = GRANT_LOCK.lock();
    let mut store = load_secret_grants();
    if store.remove(plugin).is_some() {
        save_secret_grants(&store)?;
    }
    Ok(())
}

/// The Keychain refs the user granted this plugin (secret_name → ref). NO values.
pub fn granted_secret_refs(plugin: &str) -> std::collections::HashMap<String, KeychainRef> {
    load_secret_grants()
        .get(plugin)
        .cloned()
        .unwrap_or_default()
}

/// Build the actual secret env map to inject: for each secret the manifest DECLARES
/// AND the user GRANTED, read the value from the Keychain. Returns name→value.
/// `loader` reads (service, account) → Option<value> (the keychain service).
pub fn resolve_granted_secrets<F>(
    plugin: &str,
    declared: &[String],
    loader: F,
) -> (BTreeMap<String, String>, Vec<String>)
where
    F: Fn(&str, &str) -> Option<String>,
{
    let refs = granted_secret_refs(plugin);
    let mut out = BTreeMap::new();
    let mut missing = Vec::new();
    for name in declared {
        if let Some(kc) = refs.get(name) {
            match loader(&kc.service, &kc.account) {
                Some(val) => {
                    out.insert(name.clone(), val);
                }
                None => missing.push(name.clone()),
            }
        }
    }
    (out, missing)
}

/// T036 — strip write bits from a plugin dir, recursively, so a writer cannot swap
/// the entrypoint (or imports/aux files in subdirs) between the hash check and exec.
/// Closes the TOCTOU window run_tool only narrows. (Codex audit applied: mask
/// `& !0o222` instead of forcing 555 → no permission widening / world-read; errors
/// propagated, not flattened; recursive into subdirs; symlink-safe — symlinks are
/// rejected, never followed.) Unix-only; no-op elsewhere.
pub fn harden_readonly(dir: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn strip_write(p: &std::path::Path) -> Result<()> {
            use std::os::unix::fs::PermissionsExt;
            let md = std::fs::symlink_metadata(p)?; // do NOT follow symlinks
            if md.file_type().is_symlink() {
                return Err(anyhow!(
                    "refusing to harden symlink in plugin dir: {}",
                    p.display()
                ));
            }
            let mut perm = md.permissions();
            perm.set_mode(perm.mode() & !0o222); // remove all write bits, preserve the rest
            std::fs::set_permissions(p, perm)?;
            Ok(())
        }
        // Depth-first: harden entries (recursing into real subdirs) then the dir.
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            let md = std::fs::symlink_metadata(&p)?;
            if md.file_type().is_symlink() {
                return Err(anyhow!("refusing to harden symlink: {}", p.display()));
            }
            if md.is_dir() {
                harden_readonly(&p)?;
            } else {
                strip_write(&p)?;
            }
        }
        // Lock the dir itself last (prevents creating/renaming/deleting entries).
        let mut dperm = std::fs::symlink_metadata(dir)?.permissions();
        dperm.set_mode(dperm.mode() & !0o222);
        std::fs::set_permissions(dir, dperm)?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// spec-002 · install a bundled plugin: copy `src` → ~/.furx/plugins/<name>, verify
/// the signature against TRUSTED_PUBKEYS, then harden read-only. Idempotent (relaxes
/// perms + replaces if already installed). Fail-closed: an invalid signature leaves
/// NOTHING installed (the freshly-copied dir is removed). Returns the manifest version.
pub fn install_bundled(src: &std::path::Path, name: &str) -> Result<String> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    install_bundled_to(src, name, &home.join(".furx").join("plugins"))
}

/// Core install with an explicit plugins base dir (testable without touching $HOME).
///
/// Panel audit (codex+gemini+frontier): the previous "install into the live path
/// then verify+harden there" had TOCTOU + residue windows. This version STAGES in a
/// private temp dir under `plugins_base`, copies+verifies+hardens THERE, then
/// publishes with an atomic rename. Any failure removes the staging dir and leaves
/// the live path untouched. Symlinks are rejected for `src`, staging entries, and
/// an existing `dest` (no chmod/traverse outside the base).
pub fn install_bundled_to(
    src: &std::path::Path,
    name: &str,
    plugins_base: &std::path::Path,
) -> Result<String> {
    if !is_safe_plugin_name(name) {
        return Err(anyhow!("unsafe plugin name"));
    }
    // Reject a symlinked source (would let copy_dir pull in attacker-controlled tree).
    let src_md = std::fs::symlink_metadata(src).map_err(|e| anyhow!("src: {e}"))?;
    if src_md.file_type().is_symlink() {
        return Err(anyhow!("refusing to install from a symlinked source"));
    }
    if !src.join("manifest.json").is_file() {
        return Err(anyhow!("source plugin has no manifest.json"));
    }
    std::fs::create_dir_all(plugins_base)?;
    let staging = plugins_base.join(format!(".staging-{}", uuid::Uuid::new_v4()));

    // Everything happens in staging; on ANY error we clean it up and the live path
    // is never touched until the final atomic rename.
    let result = (|| -> Result<String> {
        copy_dir(src, &staging)?; // create + recursive copy (rejects internal symlinks)
        let text = std::fs::read_to_string(staging.join("manifest.json"))?;
        let m: SignedManifest = serde_json::from_str(&text)?;
        if m.name != name {
            return Err(anyhow!("manifest name != requested name"));
        }
        if !m.verify() {
            return Err(anyhow!("signature invalid"));
        }
        match &m.entrypoint_sha256 {
            Some(want) => {
                let got = file_sha256(&staging.join(&m.entrypoint))?;
                if !got.eq_ignore_ascii_case(want) {
                    return Err(anyhow!("entrypoint hash mismatch"));
                }
            }
            None => return Err(anyhow!("manifest missing entrypoint_sha256")),
        }
        Ok(m.version)
    })();

    let version = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = relax_writable(&staging);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(anyhow!("install rejected: {e}"));
        }
    };

    // Publish: remove any prior install (reject if it's a symlink), then atomic rename.
    let dest = plugins_base.join(name);
    if let Ok(md) = std::fs::symlink_metadata(&dest) {
        if md.file_type().is_symlink() {
            let _ = relax_writable(&staging);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(anyhow!(
                "existing plugin path is a symlink — refusing to replace"
            ));
        }
        let _ = relax_writable(&dest);
        let _ = std::fs::remove_dir_all(&dest);
    }
    if let Err(e) = std::fs::rename(&staging, &dest) {
        let _ = relax_writable(&staging);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(anyhow!("publish failed: {e}"));
    }
    // Re-install replaces the code, so RESET prior consent + secret grants (panel:
    // a same-name reinstall must NOT inherit grants → forces explicit re-consent).
    let _ = revoke(name);
    // Harden the published tree read-only (best-effort: a chmod hiccup must not fail
    // an already-verified install; run_tool re-checks the entrypoint hash at exec).
    // The verify ran on the private staging dir, so it saw no concurrent tamper.
    let _ = harden_readonly(&dest);
    Ok(version)
}

fn is_safe_plugin_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Public wrapper over `relax_writable` (spec-043 F3 reuses the harden/relax pair from
/// its own staging cleanup). Undoes `harden_readonly` so a staging dir can be removed.
pub fn relax_writable_pub(dir: &std::path::Path) -> Result<()> {
    relax_writable(dir)
}

/// Recursively make a dir tree writable again (undo harden_readonly) so it can be
/// replaced/removed. Unix-only; no-op elsewhere.
fn relax_writable(dir: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut dperm = std::fs::symlink_metadata(dir)?.permissions();
        dperm.set_mode(dperm.mode() | 0o700);
        std::fs::set_permissions(dir, dperm)?;
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            let md = std::fs::symlink_metadata(&p)?;
            if md.file_type().is_symlink() {
                continue;
            }
            if md.is_dir() {
                relax_writable(&p)?;
            } else {
                let mut perm = md.permissions();
                perm.set_mode(perm.mode() | 0o600);
                std::fs::set_permissions(&p, perm)?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Recursive copy, rejecting symlinks (no escape via a symlinked source entry).
fn copy_dir(src: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let md = std::fs::symlink_metadata(&from)?;
        if md.file_type().is_symlink() {
            return Err(anyhow!("refusing to copy symlink: {}", from.display()));
        }
        let to = dest.join(entry.file_name());
        if md.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// hex SHA-256 of a file's bytes (for entrypoint content binding).
pub fn file_sha256(path: &std::path::Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(hex::encode(h.finalize()))
}

/// Deterministic JSON canonicalization (sorted keys, no whitespace).
fn canonicalize(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted: BTreeMap<&String, &serde_json::Value> = BTreeMap::new();
            for (k, val) in map {
                sorted.insert(k, val);
            }
            let inner: Vec<String> = sorted
                .iter()
                .map(|(k, val)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonicalize(val)
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonicalize).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Verify a base64 Ed25519 signature over `msg` with a base64 32-byte pubkey.
pub fn verify_signature(msg: &[u8], sig_b64: &str, pubkey_b64: &str) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    let eng = base64::engine::general_purpose::STANDARD;
    let Ok(pk_bytes) = eng.decode(pubkey_b64) else {
        return false;
    };
    let Ok(sig_bytes) = eng.decode(sig_b64) else {
        return false;
    };
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.try_into() else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.try_into() else {
        return false;
    };
    let Ok(vk) = VerifyingKey::from_bytes(&pk_arr) else {
        return false;
    };
    let sig = Signature::from_bytes(&sig_arr);
    vk.verify_strict(msg, &sig).is_ok()
}

/// An OS-level network-deny sandbox wrapper, if one is available on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetSandbox {
    /// macOS `sandbox-exec -p '(version 1)(allow default)(deny network*)'`
    MacSandboxExec,
    /// Linux `firejail --net=none`
    Firejail,
    /// Linux `unshare -n` (needs user namespaces / privileges)
    UnshareNet,
    /// Nothing available — caller must fail closed for net-deny plugins.
    None,
}

impl NetSandbox {
    /// Absolute path of the sandbox binary. Codex audit: resolving by bare name via
    /// $PATH lets an attacker-influenced PATH select a fake sandbox (net-deny
    /// fail-OPEN). We only ever exec from known system locations.
    fn abs_path(&self) -> Option<&'static str> {
        match self {
            NetSandbox::MacSandboxExec => Some("/usr/bin/sandbox-exec"),
            NetSandbox::Firejail => {
                for p in ["/usr/bin/firejail", "/usr/local/bin/firejail"] {
                    if std::path::Path::new(p).is_file() {
                        return Some(p);
                    }
                }
                None
            }
            NetSandbox::UnshareNet => Some("/usr/bin/unshare"),
            NetSandbox::None => None,
        }
    }
}

pub fn detect_net_sandbox() -> NetSandbox {
    if cfg!(target_os = "macos") {
        if std::path::Path::new("/usr/bin/sandbox-exec").is_file() {
            return NetSandbox::MacSandboxExec;
        }
    } else if cfg!(target_os = "linux") {
        if std::path::Path::new("/usr/bin/firejail").is_file()
            || std::path::Path::new("/usr/local/bin/firejail").is_file()
        {
            return NetSandbox::Firejail;
        }
        if std::path::Path::new("/usr/bin/unshare").is_file() {
            return NetSandbox::UnshareNet;
        }
    }
    NetSandbox::None
}

/// Result of invoking a plugin tool.
#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub stdout: String,
    pub exit_ok: bool,
    pub sandboxed_net_deny: bool,
}

/// Run a plugin tool as a subprocess. Honors the permission set:
///   - ENTRYPOINT BINDING: `expected_sha256` (from the signed manifest) is checked
///     against the file content immediately before exec — closes the TOCTOU window
///     and ensures the signature covers exactly the binary we run.
///   - secrets: only granted ones are injected into the child env (BYOK gate)
///   - net: if NOT granted, wrap in an OS net-deny sandbox resolved by ABSOLUTE
///     path; if no sandbox tool, return an error (fail closed)
///   - cwd is pinned to `plugin_dir`
///
/// NOTE (honest scope): `fs_read`/`fs_write`/`shell` permissions are DECLARED and
/// audited but NOT fully enforced by the subprocess runtime — a subprocess can
/// touch the user's filesystem and fork. True fs/syscall isolation is the WASM
/// (wasmtime) / OS-profile path landing in Fase 2/3. Only net-deny (via OS
/// sandbox) and the secret/BYOK gate are enforced here.
pub async fn run_tool(
    plugin_dir: &std::path::Path,
    entrypoint: &str,
    expected_sha256: Option<&str>,
    tool: &str,
    args_json: &str,
    perms: &Permissions,
    available_secrets: &BTreeMap<String, String>,
    timeout_ms: u64,
) -> Result<ToolResult> {
    // Resolve entrypoint inside the plugin dir; reject path escapes.
    let entry = plugin_dir.join(entrypoint);
    let entry_canon = entry
        .canonicalize()
        .map_err(|e| anyhow!("entrypoint not found: {e}"))?;
    let dir_canon = plugin_dir
        .canonicalize()
        .map_err(|e| anyhow!("plugin dir not found: {e}"))?;
    if !entry_canon.starts_with(&dir_canon) {
        return Err(anyhow!("entrypoint escapes plugin dir"));
    }

    // Content binding: verify the entrypoint hash as the LAST step before exec.
    // This binds the signed manifest to the binary and NARROWS the TOCTOU window to
    // the hash→spawn gap (does not fully close it: a writer with access to the
    // plugin dir could swap the file in that window). Full close needs post-install
    // immutability (read-only plugin dir) — Fase 3 hardening (T036).
    if let Some(want) = expected_sha256 {
        let got = file_sha256(&entry_canon)?;
        if !got.eq_ignore_ascii_case(want) {
            return Err(anyhow!(
                "entrypoint sha256 mismatch — refusing to run (manifest does not bind this binary)"
            ));
        }
    }

    // Three network policies (spec-004):
    //   net:["*"]    → full network, no sandbox
    //   net:[hosts]  → per-host: egress proxy + sandbox that allows ONLY loopback:port
    //   net:[]       → net-deny sandbox (fail-closed)
    let mut sandboxed = false;
    let entry_str = entry_canon.to_string_lossy().into_owned();
    let net_hosts = perms.net_hosts();
    // Proxy lifecycle: the handle (kept alive until after the child exits) cancels
    // the accept loop AND active tunnels on drop.
    let mut _proxy_guard: Option<crate::services::net_proxy::ProxyHandle> = None;
    let mut proxy_env: Option<String> = None;

    let (prog, mut argv): (String, Vec<String>) = if perms.grants_net() {
        (entry_str, vec![])
    } else if !net_hosts.is_empty() {
        // Per-host allowlist: only macOS sandbox-exec can express "allow loopback:port,
        // deny the rest" portably in v1. Otherwise FAIL-CLOSED (no un-enforced net).
        let sb = detect_net_sandbox();
        if sb != NetSandbox::MacSandboxExec {
            return Err(anyhow!(
                "per-host net allowlist for '{}' needs macOS sandbox-exec in v1 \
                 (other platforms: grant net:[\"*\"] or none) — refusing (fail-closed)",
                entrypoint
            ));
        }
        let abs = sb
            .abs_path()
            .ok_or_else(|| anyhow!("sandbox-exec missing"))?;
        let proxy = crate::services::net_proxy::spawn(net_hosts.clone()).await?;
        let port = proxy.addr.port();
        proxy_env = Some(proxy.url()); // includes the per-proxy auth token
        _proxy_guard = Some(proxy);
        sandboxed = true;
        // Allow only outbound to the loopback proxy; deny everything else. The plugin
        // can't bypass the proxy (kernel blocks direct egress); the proxy enforces the
        // signed host allowlist + blocks internal IPs.
        let profile = format!(
            "(version 1)(allow default)(deny network*)(allow network-outbound (remote ip \"localhost:{port}\"))"
        );
        (abs.into(), vec!["-p".into(), profile, entry_str])
    } else {
        let sb = detect_net_sandbox();
        let abs = sb.abs_path().ok_or_else(|| {
            anyhow!(
                "net-deny plugin '{}' cannot run: no OS network sandbox available \
             (install firejail/sandbox-exec) — refusing to run un-sandboxed (fail-closed)",
                entrypoint
            )
        })?;
        sandboxed = true;
        match sb {
            NetSandbox::MacSandboxExec => (
                abs.into(),
                vec![
                    "-p".into(),
                    "(version 1)(allow default)(deny network*)".into(),
                    entry_str,
                ],
            ),
            NetSandbox::Firejail => (
                abs.into(),
                vec!["--quiet".into(), "--net=none".into(), entry_str],
            ),
            NetSandbox::UnshareNet => (abs.into(), vec!["-n".into(), entry_str]),
            NetSandbox::None => unreachable!("abs_path None handled above"),
        }
    };

    // tool + args as the final argv entries (the plugin reads them).
    argv.push(tool.to_string());
    argv.push(args_json.to_string());

    let mut cmd = Command::new(&prog);
    cmd.args(&argv)
        .current_dir(&dir_canon)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        // Start from a CLEAN env: nothing from the host leaks in.
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("FURX_PLUGIN", "1");
    // Point HTTP clients at the egress proxy (per-host case). Direct egress is
    // blocked by the sandbox, so this is the only route out.
    if let Some(url) = &proxy_env {
        cmd.env("HTTP_PROXY", url)
            .env("HTTPS_PROXY", url)
            .env("ALL_PROXY", url)
            .env("http_proxy", url)
            .env("https_proxy", url)
            .env("all_proxy", url);
    }

    // BYOK gate: inject ONLY granted secrets.
    for name in &perms.secrets {
        if let Some(val) = available_secrets.get(name) {
            cmd.env(name, val);
        }
    }

    let child = cmd.spawn().map_err(|e| anyhow!("spawn {prog}: {e}"))?;
    let out = tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("plugin tool timed out after {timeout_ms}ms"))?
        .map_err(|e| anyhow!("plugin wait: {e}"))?;

    Ok(ToolResult {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        exit_ok: out.status.success(),
        sandboxed_net_deny: sandboxed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    // Test signing key [7u8;32] → its pubkey is the "trusted" set for these tests.
    fn test_trusted() -> Vec<String> {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let eng = base64::engine::general_purpose::STANDARD;
        vec![eng.encode(sk.verifying_key().to_bytes())]
    }

    fn make_signed(name: &str, perms: Permissions) -> SignedManifest {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        let eng = base64::engine::general_purpose::STANDARD;
        let mut m = SignedManifest {
            name: name.into(),
            version: "1.0.0".into(),
            description: None,
            entrypoint: "run.sh".into(),
            entrypoint_sha256: None,
            permissions: perms,
            signature: None,
            pubkey: Some(eng.encode(vk.to_bytes())),
            mcp: None,
        };
        let bytes = m.signing_bytes().unwrap();
        let sig = sk.sign(&bytes);
        m.signature = Some(eng.encode(sig.to_bytes()));
        m
    }

    #[test]
    fn valid_signature_verifies() {
        let m = make_signed("git", Permissions::default());
        assert!(m.verify_with_trusted(&test_trusted()));
    }

    #[test]
    fn untrusted_pubkey_is_rejected() {
        // A perfectly valid self-signature with a key NOT in the trusted set must
        // be rejected (closes the embedded-pubkey bypass).
        let m = make_signed("git", Permissions::default());
        assert!(
            m.verify_with_trusted(&test_trusted()),
            "sanity: trusted key verifies"
        );
        assert!(
            !m.verify_with_trusted(&[]),
            "untrusted/empty set must reject"
        );
        assert!(
            !m.verify_with_trusted(&["AAAA".into()]),
            "wrong key must reject"
        );
    }

    #[test]
    fn unsigned_manifest_is_rejected() {
        let mut m = make_signed("git", Permissions::default());
        m.signature = None;
        assert!(!m.verify_with_trusted(&test_trusted()));
        m.pubkey = None;
        assert!(!m.verify_with_trusted(&test_trusted()));
    }

    #[test]
    fn tampered_manifest_fails_verification() {
        let mut m = make_signed("git", Permissions::default());
        // Flip a field after signing → signature no longer matches.
        m.version = "9.9.9".into();
        assert!(!m.verify_with_trusted(&test_trusted()));
    }

    #[test]
    fn permissions_default_deny() {
        let p = Permissions::default();
        assert!(!p.grants_net());
        assert!(!p.grants_secret("OPENAI_API_KEY"));
    }

    #[test]
    fn secret_grant_is_explicit() {
        let p = Permissions {
            secrets: vec!["OPENAI_API_KEY".into()],
            ..Default::default()
        };
        assert!(p.grants_secret("OPENAI_API_KEY"));
        assert!(!p.grants_secret("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn net_grant_only_wildcard_grants_full_net() {
        // Only "*" grants full network (skips sandbox). A specific host is NOT an
        // enforceable allowlist yet → does NOT skip the net-deny sandbox.
        assert!(Permissions {
            net: vec!["*".into()],
            ..Default::default()
        }
        .grants_net());
        assert!(!Permissions {
            net: vec!["api.github.com".into()],
            ..Default::default()
        }
        .grants_net());
        assert!(!Permissions::default().grants_net());
    }

    // E2E (constitution III): a real signed plugin on disk, invoked out-of-process.
    #[cfg(unix)]
    #[test]
    fn run_tool_executes_signed_plugin_and_gates_secrets() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("furx-plugintest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        // entrypoint: echo the argv, and whether a granted/denied secret reached env.
        let entry = tmp.join("run.sh");
        let mut f = std::fs::File::create(&entry).unwrap();
        writeln!(f, "#!/bin/sh\necho \"tool=$1 args=$2 granted=${{GRANTED_KEY:-none}} denied=${{DENIED_KEY:-none}}\"").unwrap();
        let mut perm = std::fs::metadata(&entry).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&entry, perm).unwrap();

        // net granted (so the test doesn't depend on a sandbox tool being installed);
        // GRANTED_KEY granted, DENIED_KEY NOT granted (BYOK gate).
        let perms = Permissions {
            net: vec!["*".into()], // full net → skip sandbox; this test targets the secret gate
            secrets: vec!["GRANTED_KEY".into()],
            ..Default::default()
        };
        let mut secrets = BTreeMap::new();
        secrets.insert("GRANTED_KEY".to_string(), "yes".to_string());
        secrets.insert("DENIED_KEY".to_string(), "leak".to_string());

        let want_hash = file_sha256(&entry).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt
            .block_on(run_tool(
                &tmp,
                "run.sh",
                Some(&want_hash),
                "list_files",
                "{\"path\":\".\"}",
                &perms,
                &secrets,
                10_000,
            ))
            .unwrap();

        assert!(res.exit_ok);
        assert!(
            res.stdout.contains("tool=list_files"),
            "stdout: {}",
            res.stdout
        );
        // BYOK gate: granted secret crossed, denied secret did NOT.
        assert!(
            res.stdout.contains("granted=yes"),
            "granted secret should be injected"
        );
        assert!(
            res.stdout.contains("denied=none"),
            "ungranted secret must NOT leak: {}",
            res.stdout
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // E2E with the REAL pinned key: the shipped bundle plugin verifies against
    // TRUSTED_PUBKEYS and runs out-of-process. Proves signing CLI ↔ verify ↔ runtime.
    // Every shipped bundle plugin verifies against the REAL pinned key.
    #[test]
    fn all_bundle_plugins_verify_against_pinned_key() {
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("plugins")
            .join("bundle");
        if !bundle.is_dir() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(&bundle).unwrap().flatten() {
            let mpath = entry.path().join("manifest.json");
            if !mpath.is_file() {
                continue;
            }
            let m: SignedManifest =
                serde_json::from_str(&std::fs::read_to_string(&mpath).unwrap()).unwrap();
            assert!(
                m.verify(),
                "bundle plugin {} must verify against pinned TRUSTED_PUBKEYS",
                m.name
            );
            assert!(
                m.entrypoint_sha256.is_some(),
                "{} missing entrypoint hash",
                m.name
            );
            checked += 1;
        }
        assert!(checked >= 2, "expected bundle plugins, found {checked}");
    }

    #[cfg(unix)]
    #[test]
    fn bundled_filesystem_ls_verifies_and_runs() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let dir = repo.join("plugins").join("bundle").join("filesystem-ls");
        if !dir.join("manifest.json").is_file() {
            return; // bundle not present in this checkout
        }
        let text = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        let m: SignedManifest = serde_json::from_str(&text).unwrap();
        // REAL trusted-key verification (no test key).
        assert!(
            m.verify(),
            "bundle plugin must verify against pinned TRUSTED_PUBKEYS"
        );

        // net=[] → runs under the OS net-deny sandbox (mac sandbox-exec present in CI/dev).
        let rt = tokio::runtime::Runtime::new().unwrap();
        match rt.block_on(run_tool(
            &dir,
            &m.entrypoint,
            m.entrypoint_sha256.as_deref(),
            "list",
            "{}",
            &m.permissions,
            &BTreeMap::new(),
            10_000,
        )) {
            Ok(res) => {
                assert!(
                    res.stdout.contains("\"tool\":\"list\""),
                    "stdout: {}",
                    res.stdout
                );
                assert!(res.sandboxed_net_deny, "net=[] plugin must run sandboxed");
            }
            Err(e) => {
                // Only acceptable failure: no OS net sandbox on this host (fail-closed).
                assert!(
                    e.to_string().contains("no OS network sandbox"),
                    "unexpected error: {e}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_bundled_verifies_hardens_and_rejects_tampered() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let src = repo.join("plugins").join("bundle").join("filesystem-ls");
        if !src.join("manifest.json").is_file() {
            return;
        }
        let base = std::env::temp_dir().join(format!("furx-install-{}", uuid::Uuid::new_v4()));

        // Happy path: real signed bundle plugin installs + verifies + hardens.
        let ver = install_bundled_to(&src, "filesystem-ls", &base).unwrap();
        assert_eq!(ver, "1.0.0");
        let dest = base.join("filesystem-ls");
        assert!(dest.join("manifest.json").is_file());
        #[cfg(unix)] // permiso POSIX read-only; en Windows el hardening usa otro modelo (fase 2)
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dest.join("run.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o222,
                0,
                "installed entrypoint must be read-only"
            );
        }

        // Idempotent re-install (dir is hardened → must relax + replace).
        assert!(install_bundled_to(&src, "filesystem-ls", &base).is_ok());

        // Tampered source corrupts the signature. Into a FRESH base (no prior
        // install) it must reject and leave NOTHING.
        let tsrc = std::env::temp_dir().join(format!("furx-tamper-{}", uuid::Uuid::new_v4()));
        copy_dir(&src, &tsrc).unwrap();
        let mt = tsrc.join("manifest.json");
        let mut m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&mt).unwrap()).unwrap();
        m["version"] = serde_json::json!("9.9.9"); // breaks signature
        std::fs::write(&mt, serde_json::to_string(&m).unwrap()).unwrap();
        let base2 = std::env::temp_dir().join(format!("furx-install2-{}", uuid::Uuid::new_v4()));
        let r = install_bundled_to(&tsrc, "filesystem-ls", &base2);
        assert!(r.is_err(), "tampered plugin must be rejected");
        assert!(
            !base2.join("filesystem-ls").exists(),
            "rejected fresh install must leave nothing"
        );
        // No leftover staging dirs either.
        let leftovers: Vec<_> = std::fs::read_dir(&base2)
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "no staging residue after rejection");
        // And a rejected RE-install must preserve the prior valid install.
        let r2 = install_bundled_to(&tsrc, "filesystem-ls", &base);
        assert!(r2.is_err());
        assert!(
            base.join("filesystem-ls").join("manifest.json").is_file(),
            "rejected re-install must keep the prior valid one"
        );

        let _ = relax_writable(&base);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&base2);
        let _ = std::fs::remove_dir_all(&tsrc);
    }

    // spec-013 e2e (T040): every Tier-1/2 + first-party MCP bundle plugin installs
    // through the REAL install path (copy → verify signature → check entrypoint hash →
    // harden read-only) and is recognized as an MCP server. Proves the signing CLI ↔
    // verify ↔ install pipeline for the new plugins, not just filesystem-ls.
    #[cfg(unix)]
    #[test]
    fn spec013_mcp_bundle_plugins_install_and_verify() {
        use std::os::unix::fs::PermissionsExt;
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let bundle = repo.join("plugins").join("bundle");
        if !bundle.is_dir() {
            return;
        }
        let names = [
            "serena",
            "github-mcp",
            "codanna",
            "test-coverage",
            "mcp-language-server",
            "git-mcp",
            "context7",
        ];
        let base = std::env::temp_dir().join(format!("furx-013-install-{}", uuid::Uuid::new_v4()));
        for name in names {
            let src = bundle.join(name);
            if !src.join("manifest.json").is_file() {
                continue;
            }
            // Real install path: rejects bad signature / hash, hardens read-only.
            let ver = install_bundled_to(&src, name, &base)
                .unwrap_or_else(|e| panic!("{name} must install: {e}"));
            assert_eq!(ver, "1.0.0", "{name} version");
            let dest = base.join(name);
            // entrypoint hardened read-only.
            assert_eq!(
                std::fs::metadata(dest.join("run.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o222,
                0,
                "{name} run.sh must be read-only after install"
            );
            // installed manifest re-verifies + declares an MCP server.
            let m: SignedManifest =
                serde_json::from_str(&std::fs::read_to_string(dest.join("manifest.json")).unwrap())
                    .unwrap();
            assert!(m.verify(), "{name} installed manifest must verify");
            assert!(m.mcp.is_some(), "{name} must declare an MCP server");
            // default-deny invariant: no plugin grants net:["*"].
            assert!(
                !m.permissions.grants_net(),
                "{name} must NEVER grant net:[\"*\"]"
            );
        }
        let _ = relax_writable(&base);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn harden_readonly_strips_write_bits() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("furx-ro-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("run.sh"), "#!/bin/sh\necho hi").unwrap();
        harden_readonly(&tmp).unwrap();
        let mode = std::fs::metadata(tmp.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o222, 0, "no write bits after harden");
        // cleanup needs write back
        let mut p = std::fs::metadata(&tmp).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&tmp, p).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn secret_grants_resolve_only_declared_and_granted() {
        let plugin = format!("sec-test-{}", uuid::Uuid::new_v4());
        let declared = vec!["API_KEY".to_string(), "OTHER".to_string()];
        // No grants yet → nothing resolves (default-deny).
        let (m0, _) = resolve_granted_secrets(&plugin, &declared, |_, _| Some("v".into()));
        assert!(m0.is_empty(), "default-deny: no secret without grant");
        // Grant API_KEY → only it resolves; OTHER (declared, not granted) does not.
        grant_secret(
            &plugin,
            "API_KEY",
            KeychainRef {
                service: "svc".into(),
                account: "acc".into(),
            },
        )
        .unwrap();
        let (m1, _) = resolve_granted_secrets(&plugin, &declared, |svc, acc| {
            if svc == "svc" && acc == "acc" {
                Some("secret-value".into())
            } else {
                None
            }
        });
        assert_eq!(m1.get("API_KEY").map(|s| s.as_str()), Some("secret-value"));
        assert!(
            !m1.contains_key("OTHER"),
            "ungranted declared secret must not resolve"
        );
        // A granted secret whose keychain entry is missing → reported, not injected.
        let (m2, missing) = resolve_granted_secrets(&plugin, &declared, |_, _| None);
        assert!(m2.is_empty());
        assert_eq!(missing, vec!["API_KEY".to_string()]);
        // Revoke → gone.
        revoke_secret(&plugin, "API_KEY").unwrap();
        let (m3, _) = resolve_granted_secrets(&plugin, &declared, |_, _| Some("v".into()));
        assert!(m3.is_empty(), "revoked secret must not resolve");
        let _ = revoke_all_secrets(&plugin);
    }

    #[test]
    fn revoke_plugin_drops_its_secret_grants() {
        let plugin = format!("sec-rev-{}", uuid::Uuid::new_v4());
        grant_secret(
            &plugin,
            "K",
            KeychainRef {
                service: "s".into(),
                account: "a".into(),
            },
        )
        .unwrap();
        assert!(!granted_secret_refs(&plugin).is_empty());
        revoke(&plugin).unwrap(); // revoking consent must also drop secret grants
        assert!(
            granted_secret_refs(&plugin).is_empty(),
            "plugin revoke must clear secret grants"
        );
    }

    #[test]
    fn grant_flow_default_deny_then_consent() {
        // Use a unique plugin name so we don't collide with real grants on disk.
        let name = format!("test-grant-{}", uuid::Uuid::new_v4());
        assert!(!is_granted(&name, "1.0.0"), "default-deny: no consent yet");
        grant(&name, "1.0.0").unwrap();
        assert!(is_granted(&name, "1.0.0"), "consent recorded");
        // Version bump invalidates the grant (re-prompt).
        assert!(!is_granted(&name, "1.1.0"), "new version must re-prompt");
        revoke(&name).unwrap();
        assert!(!is_granted(&name, "1.0.0"), "revoked");
    }

    // ── spec-013 T030 — Roots/readonly model ─────────────────────────────────
    #[test]
    fn fs_roots_readonly_defaults_to_readonly() {
        // An FsRoot deserialized WITHOUT `readonly` defaults to read-only (fail-safe).
        let r: FsRoot = serde_json::from_str(r#"{"path":"/repo"}"#).unwrap();
        assert!(r.readonly, "under-specified root must default read-only");
        let rw: FsRoot = serde_json::from_str(r#"{"path":"/repo","readonly":false}"#).unwrap();
        assert!(!rw.readonly);
    }

    #[test]
    fn empty_fs_roots_does_not_serialize_preserving_signatures() {
        // CRITICAL back-compat: a manifest with no fs_roots must serialize WITHOUT the
        // field, so existing Ed25519 signatures (which cover the canonical JSON) stay
        // valid. We prove the signed bytes are byte-identical with vs without the field
        // present-but-empty.
        let p = Permissions {
            fs_read: vec!["$PROJECT_ROOT".into()],
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert!(
            v.get("fs_roots").is_none(),
            "empty fs_roots must be omitted from serialization"
        );
        // And a real signed manifest with empty fs_roots verifies (round-trips).
        let m = make_signed(
            "roots",
            Permissions {
                fs_read: vec!["$PROJECT_ROOT".into()],
                ..Default::default()
            },
        );
        assert!(m.verify_with_trusted(&test_trusted()));
    }

    #[test]
    fn readable_and_writable_roots_merge_flat_and_structured() {
        let p = Permissions {
            fs_read: vec!["/flat-ro".into()],
            fs_write: vec!["/flat-rw".into()],
            fs_roots: vec![
                FsRoot {
                    path: "/struct-ro".into(),
                    readonly: true,
                },
                FsRoot {
                    path: "/struct-rw".into(),
                    readonly: false,
                },
            ],
            ..Default::default()
        };
        let readable = p.readable_roots();
        // every declared path is readable (writable roots are also readable).
        for want in ["/flat-ro", "/flat-rw", "/struct-ro", "/struct-rw"] {
            assert!(
                readable.contains(&want.to_string()),
                "missing readable {want}: {readable:?}"
            );
        }
        let writable = p.writable_roots();
        assert!(writable.contains(&"/flat-rw".to_string()));
        assert!(writable.contains(&"/struct-rw".to_string()));
        assert!(
            !writable.contains(&"/struct-ro".to_string()),
            "readonly root must NOT be writable"
        );
        assert!(
            !writable.contains(&"/flat-ro".to_string()),
            "fs_read entry must NOT be writable"
        );
    }

    #[test]
    fn readonly_root_vetoes_overlapping_write() {
        // A path covered by BOTH a writable root and a readonly root is read-only
        // (readonly wins — fail-safe). Segment-aware so /a/b != /a/bc.
        let p = Permissions {
            fs_roots: vec![
                FsRoot {
                    path: "/repo".into(),
                    readonly: false,
                },
                FsRoot {
                    path: "/repo/vendor".into(),
                    readonly: true,
                },
            ],
            ..Default::default()
        };
        assert!(
            p.allows_write("/repo/src/main.rs"),
            "writable root grants write"
        );
        assert!(
            !p.allows_write("/repo/vendor/lib.rs"),
            "readonly subtree vetoes write"
        );
        assert!(p.allows_read("/repo/vendor/lib.rs"), "still readable");
        assert!(
            !p.allows_write("/repobad/x"),
            "segment boundary: /repobad is not under /repo"
        );
        assert!(
            !p.allows_read("/elsewhere"),
            "outside any root → not readable"
        );
    }

    #[test]
    fn dot_root_matches_everything() {
        let p = Permissions {
            fs_read: vec![".".into()],
            ..Default::default()
        };
        assert!(p.allows_read("/anywhere/at/all"));
    }

    #[test]
    fn parent_dir_traversal_is_fail_closed() {
        // audit codex+deepseek 013: a `..` in the candidate path means it isn't resolved
        // and could escape the root → containment must DENY (never lexically "contained").
        let p = Permissions {
            fs_roots: vec![FsRoot {
                path: "/repo".into(),
                readonly: false,
            }],
            fs_read: vec!["/repo".into()],
            ..Default::default()
        };
        assert!(
            !p.allows_write("/repo/../etc/passwd"),
            "..-traversal must not be writable"
        );
        assert!(
            !p.allows_read("/repo/../etc/passwd"),
            "..-traversal must not be readable"
        );
        assert!(
            !p.allows_read("/repo/sub/../../etc"),
            "deep ..-traversal denied"
        );
        // a clean path under the root is still fine.
        assert!(p.allows_read("/repo/src/main.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn entrypoint_escape_is_rejected() {
        let tmp = std::env::temp_dir().join(format!("furx-escape-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let perms = Permissions {
            net: vec!["x".into()],
            ..Default::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        // ../../bin/sh escapes the plugin dir → must error.
        let res = rt.block_on(run_tool(
            &tmp,
            "../../../../bin/sh",
            None,
            "t",
            "{}",
            &perms,
            &BTreeMap::new(),
            5_000,
        ));
        assert!(res.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
