// spec-kit 043 · Ola 4 — Skills híbrido con verificación (F1: trust gate consistente).
//
// This is the SKILL manifest trust layer. It is SEPARATE from the existing
// `plugin_host::SignedManifest` (entrypoint-bound bundle plugins), whose signatures
// are load-bearing and MUST NOT change. Skills use the Agent-Skills standard format
// (SKILL.md frontmatter for metadata) plus a `manifest.json` that carries the
// signature over a content `tree_hash`.
//
// NON-NEGOTIABLE gotchas the council caught (v7), all enforced here:
//   - the `signature` is NOT inside the payload that is signed (would be a cycle).
//     Structure: { payload: {schema_version,name,version,tree_hash,key_id,
//     permissions,external_imports}, signature: "<hex>" }. The signed message hashes
//     ONLY `payload` via JCS (RFC 8785, canonical_json_sha256).
//   - `Instant::saturating_sub` does NOT exist in Rust → `checked_duration_since`.
//   - NFC: normalize every rel_path to NFC before sort+hash (else tree_hash differs
//     NFC vs NFD cross-platform on APFS).
//   - hard-link check `nlink>1` ONLY on `is_file()` (dirs have nlink≥2 normally).
//   - revoked_keys.txt = SHA-256 hex (64 chars) of pubkey bytes; all-malformed →
//     WARN + UI flag, NOT silent degrade.
//
// Fail-closed posture: no signature trusted → SKILL.md as prompt/text, scripts INERT
// until explicit user promotion, all under the existing default-deny sandbox.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, Instant};

use super::plugin_host::{verify_signature, Permissions, TRUSTED_PUBKEYS};

/// Domain-separation prefix for the signed message. Versioned so a future format
/// change can never be confused with this one.
pub const SKILL_SIGNED_MSG_PREFIX: &str = "FURX_SKILL_MANIFEST_V1";

/// Lowest manifest schema version this build accepts.
pub const MIN_ACCEPTED_SCHEMA_VERSION: u32 = 1;

/// Re-verification TTL fast-path window (P1 only; P0 always rehashes). Kept here so
/// the clock-skew guard test and any future fast-path agree on the constant.
pub const REVERIFY_TTL: Duration = Duration::from_secs(300);

/// The trust level a skill resolves to after the gate runs. Mirrors the council's
/// 4-state model (§3 of the canonical doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLevel {
    /// Furx-signed (key in TRUSTED_PUBKEYS) + tree_hash matches → executes.
    Verified,
    /// Tree_hash signed locally by the user (LOCAL_USER_KEY) → executes (per-machine).
    SandboxedPromoted,
    /// No signature, or publisher not trusted → SKILL.md as prompt, scripts INERT.
    Sandboxed,
    /// Signature present but invalid, or tree_hash mismatch → fail-closed, INERT.
    Rejected,
}

impl TrustLevel {
    /// Whether a skill at this trust level may execute its scripts. Sandboxed/Rejected
    /// are INERT (scripts never run until explicit promotion). Fail-closed default.
    pub fn may_execute(&self) -> bool {
        matches!(self, TrustLevel::Verified | TrustLevel::SandboxedPromoted)
    }

    /// DB `inert` column value (1 = inert/no-exec).
    pub fn inert(&self) -> i64 {
        if self.may_execute() {
            0
        } else {
            1
        }
    }

    /// UI badge color token (frontend maps to verde/amarillo/rojo).
    pub fn badge(&self) -> &'static str {
        match self {
            TrustLevel::Verified => "verified",
            TrustLevel::SandboxedPromoted => "promoted",
            TrustLevel::Sandboxed => "sandboxed",
            TrustLevel::Rejected => "rejected",
        }
    }
}

/// The SEMANTIC payload of a skill manifest. The signed message hashes the JCS of
/// THIS object — never including the `signature`. Adding a field here changes the
/// signed bytes (intentional: the signature must cover all semantics).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillPayload {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    /// hex SHA-256 of the canonical (NFC) content tree of `scripts/`.
    pub tree_hash: String,
    /// `sha256(pubkey_bytes)_hex + "_" + unix_timestamp` (73+ chars). The first 64
    /// chars are the SHA-256 hex of the signing key — used for revocation matching.
    pub key_id: String,
    #[serde(default)]
    pub permissions: Permissions,
    /// Compatibility hints (MCP servers / tools the skill expects). NOT an integrity
    /// gate — checked at import for availability only.
    #[serde(default)]
    pub external_imports: Vec<String>,
}

/// A skill manifest on disk: the payload + a detached Ed25519 signature (hex). The
/// signature NEVER lives inside `payload` (would be a self-referential cycle).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillManifest {
    pub payload: SkillPayload,
    /// hex-encoded Ed25519 signature over `signed_message(payload)`.
    pub signature: String,
}

impl SkillPayload {
    /// RFC 8785 JCS canonical form of this payload (sorted keys, minimal whitespace,
    /// canonical number/string encoding). The `signature` is structurally absent
    /// (it's not a field of `SkillPayload`), so the cycle is impossible by design.
    pub fn jcs(&self) -> Result<String> {
        let v = serde_json::to_value(self)?;
        Ok(canonical_json(&v))
    }

    /// SHA-256 (hex) of the JCS bytes — the content the signature commits to.
    pub fn jcs_sha256(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(self.jcs()?.as_bytes());
        Ok(hex::encode(h.finalize()))
    }

    /// The exact byte string that gets signed/verified. Domain-separated + binds the
    /// schema version + the JCS hash of the payload.
    pub fn signed_message(&self) -> Result<Vec<u8>> {
        let jcs_hash = self.jcs_sha256()?;
        Ok(format!(
            "{SKILL_SIGNED_MSG_PREFIX}\nschema_version={}\ncanonical_json_sha256={jcs_hash}\n",
            self.schema_version
        )
        .into_bytes())
    }

    /// The 64-char SHA-256 hex of the signing pubkey extracted from `key_id`. This is
    /// what `revoked_keys.txt` lists. `None` if `key_id` is malformed (<64 chars).
    pub fn key_sha256(&self) -> Option<&str> {
        self.key_id.get(..64).filter(|s| {
            s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
        })
    }
}

/// Outcome of verifying a skill manifest in memory (the trust gate). Carries the
/// resolved level + a human reason for the UI/audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub level: TrustLevel,
    pub reason: String,
}

impl SkillManifest {
    /// Trust gate — pure, in-memory, fail-closed. Decides the trust level from:
    ///   1. schema_version acceptable,
    ///   2. the Ed25519 signature is valid over `signed_message(payload)` against a
    ///      key in `trusted` (the pinned set) and NOT revoked,
    ///   3. the supplied `actual_tree_hash` (computed from disk) equals the signed one.
    ///
    /// `actual_tree_hash` is the caller's freshly-computed content hash. A mismatch is
    /// `Rejected` even with a valid signature (the binary doesn't match what was signed).
    /// `None` means the caller has NOT computed it yet → the gate REFUSES to grant
    /// `Verified`/`SandboxedPromoted` execution rights and falls back to `Sandboxed`
    /// (scripts inert): a valid signature alone never authorizes execution without the
    /// content binding (audit codex BLOCKER + AIE HIGH — closes a "forgot to hash"
    /// bypass). Callers that want the executable verdict MUST pass the disk hash.
    pub fn gate(
        &self,
        trusted: &[String],
        revoked_key_sha256: &HashSet<String>,
        actual_tree_hash: Option<&str>,
    ) -> GateOutcome {
        let p = &self.payload;
        // (1) schema gate.
        if p.schema_version < MIN_ACCEPTED_SCHEMA_VERSION {
            return GateOutcome {
                level: TrustLevel::Rejected,
                reason: format!(
                    "schema_version {} < minimum {}",
                    p.schema_version, MIN_ACCEPTED_SCHEMA_VERSION
                ),
            };
        }
        // The pubkey hex from key_id; malformed key_id → cannot verify → Sandboxed
        // (no trusted signature, but not actively rejected: it's the unsigned-ecosystem
        // path). The signature is checked below; if it's empty we treat as unsigned.
        if self.signature.trim().is_empty() {
            return GateOutcome {
                level: TrustLevel::Sandboxed,
                reason: "no signature — trust-the-source, scripts inert".into(),
            };
        }
        let Some(key_hex) = p.key_sha256() else {
            return GateOutcome {
                level: TrustLevel::Rejected,
                reason: "malformed key_id (need 64 hex chars prefix)".into(),
            };
        };
        // (2a) revocation: a revoked key → Rejected even if otherwise valid.
        if revoked_key_sha256.contains(key_hex) {
            return GateOutcome {
                level: TrustLevel::Rejected,
                reason: format!("signing key {key_hex} is revoked"),
            };
        }
        // (2b) find the trusted pubkey whose sha256 matches key_hex.
        let signed_msg = match p.signed_message() {
            Ok(m) => m,
            Err(e) => {
                return GateOutcome {
                    level: TrustLevel::Rejected,
                    reason: format!("cannot build signed message: {e}"),
                }
            }
        };
        let sig_b64 = match hex_sig_to_b64(&self.signature) {
            Some(b) => b,
            None => {
                return GateOutcome {
                    level: TrustLevel::Rejected,
                    reason: "signature is not valid hex".into(),
                }
            }
        };
        let mut matched_trusted = false;
        for pk_b64 in trusted {
            if pubkey_b64_sha256(pk_b64).as_deref() != Some(key_hex) {
                continue;
            }
            matched_trusted = true;
            if verify_signature(&signed_msg, &sig_b64, pk_b64) {
                // (3) tree_hash binding — a valid signature NEVER grants execution
                // without the content hash. No hash supplied → Sandboxed (inert),
                // never Verified. Mismatch → Rejected.
                return match actual_tree_hash {
                    None => GateOutcome {
                        level: TrustLevel::Sandboxed,
                        reason: "signature valid but tree_hash not yet computed — \
                                 scripts inert until content is bound"
                            .into(),
                    },
                    Some(actual) if !actual.eq_ignore_ascii_case(&p.tree_hash) => GateOutcome {
                        level: TrustLevel::Rejected,
                        reason: format!(
                            "tree_hash mismatch: signed {} got {}",
                            p.tree_hash, actual
                        ),
                    },
                    Some(_) => GateOutcome {
                        level: TrustLevel::Verified,
                        reason: "Ed25519 signature valid against pinned key + tree_hash bound"
                            .into(),
                    },
                };
            }
        }
        if matched_trusted {
            GateOutcome {
                level: TrustLevel::Rejected,
                reason: "signature invalid for the claimed trusted key".into(),
            }
        } else {
            // Signature present but signed by a key NOT in the pinned set → publisher
            // is not Furx. Trust-the-source: Sandboxed (scripts inert), not Rejected.
            GateOutcome {
                level: TrustLevel::Sandboxed,
                reason: "signed by an untrusted publisher — scripts inert".into(),
            }
        }
    }

    /// Convenience: gate against the compiled-in `TRUSTED_PUBKEYS`.
    pub fn gate_pinned(
        &self,
        revoked_key_sha256: &HashSet<String>,
        actual_tree_hash: Option<&str>,
    ) -> GateOutcome {
        let trusted: Vec<String> = TRUSTED_PUBKEYS.iter().map(|s| s.to_string()).collect();
        self.gate(&trusted, revoked_key_sha256, actual_tree_hash)
    }
}

/// The compiled-in pinned trusted pubkeys as owned strings (convenience for callers
/// that pass `&[String]` to `gate`/`import_skill`).
pub fn pinned_trusted_keys() -> Vec<String> {
    TRUSTED_PUBKEYS.iter().map(|s| s.to_string()).collect()
}

/// hex Ed25519 signature → base64 (the `verify_signature` API takes base64). Returns
/// `None` if not valid hex or not 64 bytes.
fn hex_sig_to_b64(hex_sig: &str) -> Option<String> {
    let bytes = hex::decode(hex_sig.trim()).ok()?;
    if bytes.len() != 64 {
        return None;
    }
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

use base64::Engine as _;

/// SHA-256 hex of a base64-encoded pubkey's RAW bytes. Used to match a pinned pubkey
/// against the `key_id`'s 64-hex prefix. `None` if the base64 doesn't decode.
pub fn pubkey_b64_sha256(pubkey_b64: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let raw = base64::engine::general_purpose::STANDARD
        .decode(pubkey_b64.trim())
        .ok()?;
    let mut h = Sha256::new();
    h.update(&raw);
    Some(hex::encode(h.finalize()))
}

/// RFC 8785-flavored canonical JSON: object keys sorted lexicographically by their
/// UTF-16 code units (serde_json already emits canonical string/number forms for our
/// payload — strings are short ASCII, numbers are small integers). Minimal whitespace.
///
/// NOTE: full RFC 8785 number canonicalization (ECMAScript `Number::toString`) is not
/// re-implemented; our payload numbers are small non-negative integers (`schema_version`)
/// which serde_json renders identically to the JCS form. Strings are escaped by
/// serde_json's `to_string`, matching JCS for the BMP ASCII we use. Key ordering is the
/// load-bearing part and is enforced here.
/// Public re-export of the JCS canonicalizer (F4's furx-core index signs JCS(payload)
/// with the same discipline as a skill manifest).
pub fn canonical_json_for(v: &serde_json::Value) -> String {
    canonical_json(v)
}

fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            // JCS sorts by UTF-16 code units. For our ASCII keys this equals byte order.
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            let inner: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ── Canonical content tree hash (NFC, hard-link-safe) ─────────────────────────

/// One file collected for the tree hash: its NFC-normalized rel_path + content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    rel_path_nfc: String,
    content_sha256: String,
}

/// Compute the canonical content `tree_hash` of a directory of skill `scripts/`.
///
/// Algorithm (council §4, with all v7 gotchas):
///   1. enumerate files recursively (rel to `root`);
///   2. REJECT symlinks (no escape);
///   3. REJECT hard-linked regular files (`nlink>1` ONLY on `is_file()` — dirs always
///      have nlink≥2 on APFS, that's not a hard link);
///   4. enforce per-file (50 MiB) and total (100 MiB) size caps;
///   5. NFC-normalize each rel_path before sort+hash (cross-platform determinism);
///   6. sort lexicographically by NFC rel_path;
///   7. per file: `"{rel_path_nfc}\x00{sha256(content)}\n"`;
///   8. sha256 over the concatenation.
///
/// Empty `scripts/` (or a non-existent dir) → `sha256("")`.
///
/// TOCTOU note (audit codex MED): each file is stat'd (symlink/type/nlink/size) then
/// re-opened by path for content. A concurrent swap could defeat those checks. In the
/// LIVE import path (F3) this is closed the same way `plugin_host::install_bundled_to`
/// does it: the tree is materialized in a PRIVATE staging dir under an exclusive
/// `flock`, hardened read-only (`harden_readonly`), and only THEN hashed — no other
/// writer can race a dir we hold exclusively and have stripped write bits from. Callers
/// that hash a shared, writable directory get a best-effort hash, not a security gate.
pub fn tree_hash(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut total: u64 = 0;
    if root.is_dir() {
        collect_tree(root, root, &mut entries, &mut total)?;
    }
    // NFC sort (entries already carry NFC rel_paths).
    entries.sort_by(|a, b| a.rel_path_nfc.cmp(&b.rel_path_nfc));
    // ⟨audit codex MED⟩ Reject NFC-COLLISIONS: two byte-distinct filenames (one NFC,
    // one NFD spelling of the same string) normalize to the same rel_path → sort order
    // among equal keys would follow read_dir order → nondeterministic hash. After
    // sorting, equal NFC rel_paths are adjacent; any adjacent pair is a collision.
    for w in entries.windows(2) {
        if w[0].rel_path_nfc == w[1].rel_path_nfc {
            return Err(anyhow!(
                "NFC rel_path collision (two files normalize to '{}') — ambiguous tree",
                w[0].rel_path_nfc
            ));
        }
    }
    let mut h = Sha256::new();
    for e in &entries {
        h.update(e.rel_path_nfc.as_bytes());
        h.update([0u8]);
        h.update(e.content_sha256.as_bytes());
        h.update([b'\n']);
    }
    Ok(hex::encode(h.finalize()))
}

const MAX_SCRIPT_FILE_SIZE: u64 = 50 * 1024 * 1024;
const MAX_SCRIPT_BYTES_TOTAL: u64 = 100 * 1024 * 1024;

fn collect_tree(
    root: &Path,
    dir: &Path,
    out: &mut Vec<TreeEntry>,
    total: &mut u64,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use unicode_normalization::UnicodeNormalization;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let md = std::fs::symlink_metadata(&path)?;
        let ft = md.file_type();
        if ft.is_symlink() {
            return Err(anyhow!(
                "refusing to hash a symlink in skill tree: {}",
                path.display()
            ));
        }
        if ft.is_dir() {
            collect_tree(root, &path, out, total)?;
            continue;
        }
        if !ft.is_file() {
            // FIFOs, sockets, devices — not allowed in a content tree.
            return Err(anyhow!(
                "refusing to hash a non-regular file: {}",
                path.display()
            ));
        }
        // ⟨v7⟩ hard-link check ONLY on regular files (dirs have nlink≥2 normally).
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if md.nlink() > 1 {
                return Err(anyhow!(
                    "refusing to hash a hard-linked file (nlink={}): {}",
                    md.nlink(),
                    path.display()
                ));
            }
        }
        let size = md.len();
        if size > MAX_SCRIPT_FILE_SIZE {
            return Err(anyhow!(
                "file too large ({} > {} cap): {}",
                size,
                MAX_SCRIPT_FILE_SIZE,
                path.display()
            ));
        }
        *total = total.saturating_add(size);
        if *total > MAX_SCRIPT_BYTES_TOTAL {
            return Err(anyhow!(
                "skill tree exceeds total size cap ({} > {})",
                *total,
                MAX_SCRIPT_BYTES_TOTAL
            ));
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("path not under root"))?;
        // ⟨v7⟩ NFC-normalize the rel_path before sort+hash. Use forward slashes so the
        // hash is OS-separator independent.
        let rel_str: String = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(&path)?;
        let mut fh = Sha256::new();
        fh.update(&bytes);
        out.push(TreeEntry {
            rel_path_nfc: rel_str,
            content_sha256: hex::encode(fh.finalize()),
        });
    }
    Ok(())
}

// ── revoked_keys.txt loader (v7 silent-fail guard) ────────────────────────────

/// Result of loading `revoked_keys.txt`: the set of revoked key SHA-256 hexes plus a
/// flag that ALL non-empty lines were malformed (→ UI banner, NOT silent degrade).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevokedKeys {
    pub keys: HashSet<String>,
    /// `true` iff there were non-empty lines AND none parsed → show a UI warning.
    pub has_parse_warnings: bool,
}

/// Load `revoked_keys.txt`. Each line = SHA-256 hex (64 chars) of the revoked pubkey's
/// raw bytes. `#` comments + blank lines ignored. Missing file → empty (fresh install,
/// OK). Other IO error → fail-closed (Err). All-malformed → empty set + warning flag.
pub fn load_revoked_keys(path: &Path) -> Result<RevokedKeys> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(RevokedKeys::default()),
        Err(e) => return Err(anyhow!("revoked_keys.txt io: {e}")),
        Ok(_) => {}
    }
    let content = std::fs::read_to_string(path)?;
    let mut keys = HashSet::new();
    let mut total_non_empty = 0usize;
    let mut malformed = 0usize;
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        total_non_empty += 1;
        if line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()) {
            keys.insert(line.to_ascii_lowercase());
        } else {
            malformed += 1;
            tracing::warn!(
                "revoked_keys.txt line {}: malformed (expected 64 hex chars), skipping",
                i + 1
            );
        }
    }
    // ⟨audit F5 MED⟩ Flag the UI banner on ANY malformed line (the banner says "tiene
    // líneas malformadas"). This is a SUPERSET of the v7 all-malformed silent-fail guard
    // (all-malformed → no keys loaded → flagged) and never hides a partial-corruption.
    let has_parse_warnings = malformed > 0;
    if total_non_empty > 0 && keys.is_empty() {
        tracing::warn!(
            "revoked_keys.txt: all {} non-empty lines were malformed; no keys loaded",
            total_non_empty
        );
    }
    Ok(RevokedKeys {
        keys,
        has_parse_warnings,
    })
}

// ── Re-verification TTL (v7 checked_duration_since, NO saturating_sub) ─────────

/// Whether the fast-path cache is still warm: `true` if `last_verified_at` is within
/// `REVERIFY_TTL` of `now`. Uses `checked_duration_since` (NOT the non-existent
/// `Instant::saturating_sub`). Clock skew (`now < last_verified_at`) → `Duration::MAX`
/// → cache cold → re-verify (the safe choice) + a WARN.
///
/// In P0 the caller ALWAYS re-hashes (no fast-path); this helper is the building block
/// for the P1 fast-path and is exercised by the clock-skew test now (dead-code-first).
pub fn reverify_is_warm(now: Instant, last_verified_at: Instant) -> bool {
    if now < last_verified_at {
        tracing::warn!(
            "skill re-verify: clock skew detected (now < last_verified_at) — forcing re-verification"
        );
    }
    let elapsed = now
        .checked_duration_since(last_verified_at)
        .unwrap_or(Duration::MAX);
    elapsed < REVERIFY_TTL
}

// ── BEGIN IMMEDIATE retry backoff with jitter (v7) ────────────────────────────

/// Base delays (ms) for the `BEGIN IMMEDIATE` retry backoff. Council §3: exponential
/// `[100,200,400,800,1600]`, ±20ms jitter, max 5 attempts, 10s global cap.
pub const RETRY_BASE_DELAYS_MS: [u64; 5] = [100, 200, 400, 800, 1600];
pub const RETRY_JITTER_MS: i64 = 20;

/// The actual delay for retry attempt `i` (0-based): base ± up to 20ms jitter, clamped
/// to ≥0. Pure given an `rng` so it's testable. Anti thundering-herd.
pub fn retry_delay_ms<R: rand::Rng>(attempt: usize, rng: &mut R) -> u64 {
    let base = RETRY_BASE_DELAYS_MS
        .get(attempt)
        .copied()
        .unwrap_or(*RETRY_BASE_DELAYS_MS.last().unwrap());
    let jitter = rng.gen_range(-RETRY_JITTER_MS..=RETRY_JITTER_MS);
    (base as i64 + jitter).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Build a fully-signed SkillManifest with key bytes `seed`, given a payload.
    fn signed_with(seed: [u8; 32], mut payload: SkillPayload) -> (SkillManifest, String) {
        let sk = SigningKey::from_bytes(&seed);
        let vk = sk.verifying_key();
        let pk_b64 = b64(&vk.to_bytes());
        let key_hex = pubkey_b64_sha256(&pk_b64).unwrap();
        payload.key_id = format!("{key_hex}_1700000000");
        let msg = payload.signed_message().unwrap();
        let sig = sk.sign(&msg);
        let manifest = SkillManifest {
            payload,
            signature: hex::encode(sig.to_bytes()),
        };
        (manifest, pk_b64)
    }

    fn base_payload(tree_hash: &str) -> SkillPayload {
        SkillPayload {
            schema_version: 1,
            name: "council".into(),
            version: "1.0.0".into(),
            tree_hash: tree_hash.into(),
            key_id: String::new(), // filled by signed_with
            permissions: Permissions::default(),
            external_imports: vec![],
        }
    }

    // ── JCS / signed-message ─────────────────────────────────────────────────
    #[test]
    fn jcs_is_key_order_independent() {
        // Two JSON objects with the same content but different key insertion order
        // MUST canonicalize identically (RFC 8785 sorts keys).
        let a: serde_json::Value =
            serde_json::from_str(r#"{"b":1,"a":2,"c":[3,{"y":1,"x":2}]}"#).unwrap();
        let b: serde_json::Value =
            serde_json::from_str(r#"{"c":[3,{"x":2,"y":1}],"a":2,"b":1}"#).unwrap();
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(canonical_json(&a), r#"{"a":2,"b":1,"c":[3,{"x":2,"y":1}]}"#);
    }

    #[test]
    fn payload_jcs_sha256_stable_across_field_order() {
        // serde always emits the struct fields in declaration order, but JCS must sort.
        // Prove the JCS hash equals a hand-built differently-ordered JSON's hash.
        let p = base_payload("00");
        let from_struct = p.jcs_sha256().unwrap();
        // Hand-build the same logical object with a scrambled key order. Use the REAL
        // serialized form of the default Permissions (serde emits all its fields, not
        // `{}`), so we're testing JCS key-ordering, not a serde shape mismatch.
        let perms_val = serde_json::to_value(Permissions::default()).unwrap();
        let scrambled = serde_json::json!({
            "version": "1.0.0",
            "tree_hash": "00",
            "schema_version": 1,
            "permissions": perms_val,
            "name": "council",
            "external_imports": [],
            "key_id": "",
        });
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(canonical_json(&scrambled).as_bytes());
        let scrambled_hash = hex::encode(h.finalize());
        assert_eq!(from_struct, scrambled_hash);
    }

    // ── trust gate ───────────────────────────────────────────────────────────
    #[test]
    fn signed_by_pinned_key_with_matching_tree_hash_is_verified() {
        let th = "deadbeef";
        let (m, pk) = signed_with([7u8; 32], base_payload(th));
        let trusted = vec![pk];
        let out = m.gate(&trusted, &HashSet::new(), Some(th));
        assert_eq!(out.level, TrustLevel::Verified, "{}", out.reason);
        assert!(out.level.may_execute());
        assert_eq!(out.level.inert(), 0);
    }

    #[test]
    fn unsigned_is_sandboxed_inert() {
        let mut p = base_payload("00");
        p.key_id = "00".repeat(32) + "_1";
        let m = SkillManifest {
            payload: p,
            signature: String::new(),
        };
        let out = m.gate(&[], &HashSet::new(), None);
        assert_eq!(out.level, TrustLevel::Sandboxed, "{}", out.reason);
        assert!(!out.level.may_execute(), "scripts must be inert");
        assert_eq!(out.level.inert(), 1);
    }

    #[test]
    fn tree_hash_mismatch_is_rejected_even_with_valid_signature() {
        let (m, pk) = signed_with([7u8; 32], base_payload("aaaa"));
        let trusted = vec![pk];
        // Disk content hashes to something else → fail-closed Rejected.
        let out = m.gate(&trusted, &HashSet::new(), Some("bbbb"));
        assert_eq!(out.level, TrustLevel::Rejected, "{}", out.reason);
        assert!(out.reason.contains("tree_hash mismatch"));
    }

    #[test]
    fn invalid_signature_for_trusted_key_is_rejected() {
        let (mut m, pk) = signed_with([7u8; 32], base_payload("cc"));
        // Corrupt the signature (still valid hex, right length) → invalid for the key.
        let mut sig = hex::decode(&m.signature).unwrap();
        sig[0] ^= 0xff;
        m.signature = hex::encode(sig);
        let out = m.gate(&[pk], &HashSet::new(), Some("cc"));
        assert_eq!(out.level, TrustLevel::Rejected, "{}", out.reason);
    }

    #[test]
    fn untrusted_publisher_signature_is_sandboxed_not_rejected() {
        // A perfectly valid self-signature with a key NOT pinned → trust-the-source.
        let (m, _pk) = signed_with([9u8; 32], base_payload("dd"));
        let out = m.gate(&[], &HashSet::new(), Some("dd")); // empty trusted set
        assert_eq!(out.level, TrustLevel::Sandboxed, "{}", out.reason);
        assert!(!out.level.may_execute());
    }

    #[test]
    fn revoked_key_is_rejected() {
        let (m, pk) = signed_with([7u8; 32], base_payload("ee"));
        let key_hex = pubkey_b64_sha256(&pk).unwrap();
        let mut revoked = HashSet::new();
        revoked.insert(key_hex);
        let out = m.gate(&[pk], &revoked, Some("ee"));
        assert_eq!(out.level, TrustLevel::Rejected, "{}", out.reason);
        assert!(out.reason.contains("revoked"));
    }

    #[test]
    fn valid_signature_without_tree_hash_is_not_verified() {
        // audit codex BLOCKER + AIE HIGH: a valid signature with actual_tree_hash=None
        // must NOT grant execution. Falls back to Sandboxed (inert), never Verified.
        let (m, pk) = signed_with([7u8; 32], base_payload("abcd"));
        let out = m.gate(&[pk], &HashSet::new(), None);
        assert_eq!(out.level, TrustLevel::Sandboxed, "{}", out.reason);
        assert!(!out.level.may_execute(), "no tree_hash → scripts must stay inert");
    }

    #[test]
    fn deny_unknown_fields_blocks_smuggled_payload_fields() {
        // audit codex HIGH + AIE MED: a manifest with an extra (unsigned) semantic field
        // must FAIL to parse, not silently drop the field before JCS/verify.
        let extra = r#"{"payload":{"schema_version":1,"name":"x","version":"1.0.0","tree_hash":"00","key_id":"","permissions":{},"external_imports":[],"evil":"injected"},"signature":""}"#;
        let r: Result<SkillManifest, _> = serde_json::from_str(extra);
        assert!(r.is_err(), "unknown payload field must be rejected at parse");
        let extra_top = r#"{"payload":{"schema_version":1,"name":"x","version":"1.0.0","tree_hash":"00","key_id":"","permissions":{},"external_imports":[]},"signature":"","rogue":1}"#;
        let r2: Result<SkillManifest, _> = serde_json::from_str(extra_top);
        assert!(r2.is_err(), "unknown top-level field must be rejected");
    }

    #[cfg(unix)]
    #[test]
    fn tree_hash_rejects_nfc_collision() {
        // audit codex MED: two byte-distinct filenames that NFC-normalize to the same
        // string make the hash order-ambiguous → must be rejected, not silently hashed.
        let d = std::env::temp_dir().join(format!("furx-nfccol-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("caf\u{00e9}.sh"), b"a").unwrap(); // NFC
        std::fs::write(d.join("cafe\u{0301}.sh"), b"b").unwrap(); // NFD — same NFC form
        let r = tree_hash(&d);
        // If the FS coalesced the two names into one entry, there's no collision (only
        // one file exists); otherwise we must reject. Accept either: a collision error,
        // or a single-file hash (FS-dependent), but NEVER a silent two-entry hash that
        // depends on read_dir order. We assert: if two entries exist, it errors.
        let count = std::fs::read_dir(&d).unwrap().count();
        if count == 2 {
            assert!(r.is_err(), "two NFC-colliding files must be rejected");
            assert!(r.unwrap_err().to_string().contains("collision"));
        }
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn schema_below_minimum_is_rejected() {
        let mut p = base_payload("ff");
        p.schema_version = 0;
        let (m, pk) = signed_with([7u8; 32], p);
        let out = m.gate(&[pk], &HashSet::new(), Some("ff"));
        assert_eq!(out.level, TrustLevel::Rejected, "{}", out.reason);
    }

    #[test]
    fn signature_cannot_be_inside_payload_by_construction() {
        // The structure makes the cycle impossible: SkillPayload has no `signature`
        // field, so jcs() never includes it. Proven by: the JCS of the payload does NOT
        // contain the signature string, and changing the signature does NOT change the
        // signed message.
        let (m, _pk) = signed_with([7u8; 32], base_payload("0011"));
        let jcs = m.payload.jcs().unwrap();
        assert!(!jcs.contains(&m.signature), "JCS must not contain signature");
        let msg1 = m.payload.signed_message().unwrap();
        let mut m2 = m.clone();
        m2.signature = "deadbeef".repeat(16);
        let msg2 = m2.payload.signed_message().unwrap();
        assert_eq!(msg1, msg2, "signed message is independent of the signature");
    }

    // ── tree_hash NFC / hard-link / symlink ──────────────────────────────────
    #[test]
    fn tree_hash_empty_dir_is_sha256_of_empty() {
        use sha2::{Digest, Sha256};
        let tmp = std::env::temp_dir().join(format!("furx-th-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let got = tree_hash(&tmp).unwrap();
        let want = hex::encode(Sha256::digest(b""));
        assert_eq!(got, want);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn tree_hash_missing_dir_is_sha256_of_empty() {
        use sha2::{Digest, Sha256};
        let tmp = std::env::temp_dir().join(format!("furx-th-missing-{}", uuid::Uuid::new_v4()));
        let got = tree_hash(&tmp).unwrap();
        assert_eq!(got, hex::encode(Sha256::digest(b"")));
    }

    #[test]
    fn tree_hash_is_nfc_invariant() {
        // SC-004: same content with an NFC vs NFD filename → SAME tree_hash.
        // "café" — composed (NFC, é = U+00E9) vs decomposed (NFD, e + U+0301).
        let nfc_name = "caf\u{00e9}.sh";
        let nfd_name = "cafe\u{0301}.sh";
        assert_ne!(nfc_name, nfd_name, "byte-different filenames");
        let body = b"#!/bin/sh\necho hi\n";

        let d1 = std::env::temp_dir().join(format!("furx-nfc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::write(d1.join(nfc_name), body).unwrap();
        let h1 = tree_hash(&d1).unwrap();

        let d2 = std::env::temp_dir().join(format!("furx-nfd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d2).unwrap();
        std::fs::write(d2.join(nfd_name), body).unwrap();
        let h2 = tree_hash(&d2).unwrap();

        assert_eq!(h1, h2, "NFC and NFD filenames must hash identically");
        std::fs::remove_dir_all(&d1).ok();
        std::fs::remove_dir_all(&d2).ok();
    }

    #[test]
    fn tree_hash_changes_with_content() {
        let d = std::env::temp_dir().join(format!("furx-thc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.sh"), b"one").unwrap();
        let h1 = tree_hash(&d).unwrap();
        std::fs::write(d.join("a.sh"), b"two").unwrap();
        let h2 = tree_hash(&d).unwrap();
        assert_ne!(h1, h2);
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn tree_hash_rejects_symlink() {
        use std::os::unix::fs::symlink;
        let d = std::env::temp_dir().join(format!("furx-sym-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("real.sh"), b"x").unwrap();
        symlink(d.join("real.sh"), d.join("link.sh")).unwrap();
        let r = tree_hash(&d);
        assert!(r.is_err(), "symlink in tree must be rejected");
        std::fs::remove_dir_all(&d).ok();
    }

    #[cfg(unix)]
    #[test]
    fn tree_hash_rejects_hardlinked_file_but_not_subdir() {
        // ⟨v7⟩ A subdir (nlink≥2 on APFS) must NOT be falsely rejected; a hard-linked
        // REGULAR FILE (nlink>1) must be rejected.
        let d = std::env::temp_dir().join(format!("furx-hl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("subdir")).unwrap();
        std::fs::write(d.join("subdir").join("ok.sh"), b"x").unwrap();
        // First prove a plain dir+file hashes fine (subdir has nlink≥2 — not a false +).
        assert!(tree_hash(&d).is_ok(), "subdir must not trigger hard-link reject");
        // Now add a hard link to a regular file → must reject.
        std::fs::write(d.join("orig.sh"), b"y").unwrap();
        std::fs::hard_link(d.join("orig.sh"), d.join("dup.sh")).unwrap();
        let r = tree_hash(&d);
        assert!(r.is_err(), "hard-linked regular file must be rejected");
        assert!(r.unwrap_err().to_string().contains("hard-linked"));
        std::fs::remove_dir_all(&d).ok();
    }

    // ── revoked_keys.txt ─────────────────────────────────────────────────────
    #[test]
    fn revoked_keys_missing_file_is_empty_ok() {
        let p = std::env::temp_dir().join(format!("furx-rk-missing-{}", uuid::Uuid::new_v4()));
        let rk = load_revoked_keys(&p).unwrap();
        assert!(rk.keys.is_empty());
        assert!(!rk.has_parse_warnings);
    }

    #[test]
    fn revoked_keys_parses_valid_and_skips_comments() {
        let p = std::env::temp_dir().join(format!("furx-rk-{}", uuid::Uuid::new_v4()));
        let key = "ab".repeat(32); // 64 hex chars
        std::fs::write(&p, format!("# a comment\n\n{key}\n  {key}  \n")).unwrap();
        let rk = load_revoked_keys(&p).unwrap();
        assert_eq!(rk.keys.len(), 1, "deduped, valid");
        assert!(rk.keys.contains(&key));
        assert!(!rk.has_parse_warnings);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn revoked_keys_mixed_valid_and_malformed_flags_warning_and_loads_valid() {
        // ⟨audit F5 MED⟩ a file with SOME valid + SOME malformed lines loads the valid
        // keys AND flags the banner (don't suppress partial corruption).
        let p = std::env::temp_dir().join(format!("furx-rkmix-{}", uuid::Uuid::new_v4()));
        let good = "ab".repeat(32);
        std::fs::write(&p, format!("{good}\nnot-hex\n")).unwrap();
        let rk = load_revoked_keys(&p).unwrap();
        assert_eq!(rk.keys.len(), 1, "valid key still loaded");
        assert!(rk.keys.contains(&good));
        assert!(rk.has_parse_warnings, "any malformed line flags the banner");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn revoked_keys_all_malformed_sets_warning_not_silent() {
        // ⟨v7⟩ all non-empty lines malformed → empty set + has_parse_warnings (UI banner),
        // NOT a silent empty set. Includes the 73-char key_id form (too long → malformed).
        let p = std::env::temp_dir().join(format!("furx-rkbad-{}", uuid::Uuid::new_v4()));
        let key_id_form = format!("{}_1700000000", "cd".repeat(32)); // 73+ chars
        std::fs::write(&p, format!("not-hex\nZZZZ\n{key_id_form}\n")).unwrap();
        let rk = load_revoked_keys(&p).unwrap();
        assert!(rk.keys.is_empty());
        assert!(rk.has_parse_warnings, "all-malformed must flag a warning");
        std::fs::remove_file(&p).ok();
    }

    // ── re-verify TTL / clock skew ───────────────────────────────────────────
    #[test]
    fn reverify_warm_within_ttl_cold_after() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(10);
        assert!(reverify_is_warm(now, recent), "within TTL → warm");
        let old = now - Duration::from_secs(400);
        assert!(!reverify_is_warm(now, old), "beyond TTL → cold");
    }

    #[test]
    fn reverify_clock_skew_forces_cold() {
        // now < last_verified_at → checked_duration_since → None → Duration::MAX → cold.
        let last = Instant::now();
        let now = last - Duration::from_secs(5);
        assert!(!reverify_is_warm(now, last), "clock skew must force re-verify");
    }

    // ── retry jitter ─────────────────────────────────────────────────────────
    #[test]
    fn retry_delay_is_base_plus_minus_jitter_and_nonneg() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        // 058 (audit): iteramos el array COMPLETO (sin `take(5)` mágico) → cubre exactamente
        // RETRY_BASE_DELAYS_MS sea cual sea su largo; no hay under-iteración silenciosa si se reduce.
        for (attempt, &base_ms) in RETRY_BASE_DELAYS_MS.iter().enumerate() {
            let base = base_ms as i64;
            for _ in 0..50 {
                let d = retry_delay_ms(attempt, &mut rng) as i64;
                assert!(
                    (d - base).abs() <= RETRY_JITTER_MS,
                    "attempt {attempt}: delay {d} not within {RETRY_JITTER_MS}ms of {base}"
                );
                assert!(d >= 0);
            }
        }
        // Beyond the table → clamps to the last base.
        let d = retry_delay_ms(99, &mut rng);
        let last = *RETRY_BASE_DELAYS_MS.last().unwrap() as i64;
        assert!(((d as i64) - last).abs() <= RETRY_JITTER_MS);
    }
}
