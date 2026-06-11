// spec-kit 043 · Ola 4 F4 — discovery híbrido + SKILL.md.
//
// Two discovery sources (council §6):
//   - furx-core: a SIGNED index baked into the binary (the curated registry, the moat).
//     P0 carries the format + a verify path; the live URL fetch is P1.
//   - local scan: user-editable `~/.furx/sources.user.toml` lists `type="local"` paths
//     (e.g. `~/.hermes/skills`, `~/.openclaw/.../skills`). Each is canonicalized and
//     REJECTED if it resolves OUTSIDE `$HOME` (no scanning arbitrary system dirs).
//
// Discovery surfaces SKILL.md frontmatter (name/version/description) as the primary,
// interoperable metadata (Hermes/OpenClaw/Agent-Skills standard). It NEVER executes or
// imports anything — it just lists what's available + whether the source carries a Furx
// signature. Importing is F3 (`skill_import`), gated.
//
// Dead-code-first: tested in isolation; F5 wires the Tauri command + UI list.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::skill_import::{parse_skill_frontmatter, read_capped_nofollow, SkillFrontmatter};

/// A discovered skill (NOT installed): where it is + its SKILL.md metadata + whether the
/// source directory carries a `manifest.json` (signed candidate) at all.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredSkill {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Absolute path of the skill directory (the import source).
    pub path: String,
    /// The discovery source name (e.g. "hermes-local", "furx-core").
    pub source: String,
    /// `true` if a `manifest.json` is present (a SIGNED candidate — the gate decides the
    /// trust level at import). `false` → unsigned (will import as Sandboxed).
    pub has_manifest: bool,
}

/// One entry in `sources.user.toml`. P0 accepts ONLY `type="local"`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UserSource {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct UserSourcesFile {
    #[serde(default, rename = "sources")]
    sources: Vec<UserSource>,
}

const MAX_SKILLS_PER_SOURCE: usize = 512;
const MAX_FRONTMATTER_FILE_BYTES: u64 = 256 * 1024;
/// Size cap for `sources.user.toml` (it lists a handful of local source dirs).
const MAX_SOURCES_TOML_BYTES: u64 = 256 * 1024;

/// Resolve `~` and env at the START of a path against `home`. Only a LEADING `~` (or
/// `~/`) and a leading `$HOME` are expanded — we do NOT do general shell expansion.
fn expand_home(p: &str, home: &Path) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        return home.join(rest);
    }
    if p == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = p.strip_prefix("$HOME/") {
        return home.join(rest);
    }
    PathBuf::from(p)
}

/// Parse + validate `sources.user.toml`. Each accepted source MUST be `type="local"`
/// with a `path` that canonicalizes to somewhere INSIDE `$HOME`. Anything else is
/// dropped (with a warning) — fail-safe: an attacker can't point the scanner at `/etc`.
pub fn load_user_sources(path: &Path, home: &Path) -> Result<Vec<(UserSource, PathBuf)>> {
    // ⟨audit MED⟩ Read the config from a SINGLE no-follow fd with the size cap enforced
    // from the fstat'd handle (no metadata/read-by-path TOCTOU, no OOM, no symlink swap).
    // A missing file → empty (fresh install); a symlinked/oversized config → error.
    if !path.exists() {
        return Ok(vec![]);
    }
    let text = read_capped_nofollow(path, MAX_SOURCES_TOML_BYTES)
        .map_err(|e| anyhow!("sources.user.toml: {e}"))?;
    let parsed: UserSourcesFile =
        toml::from_str(&text).map_err(|e| anyhow!("sources.user.toml parse: {e}"))?;
    let home_canon = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut out = Vec::new();
    for s in parsed.sources {
        if s.kind != "local" {
            tracing::warn!(
                "sources.user.toml: source '{}' type='{}' ignored (only 'local' allowed in P0)",
                s.name,
                s.kind
            );
            continue;
        }
        let Some(raw) = s.path.clone() else {
            tracing::warn!("sources.user.toml: local source '{}' has no path", s.name);
            continue;
        };
        let resolved = expand_home(&raw, home);
        // Canonicalize + must be inside $HOME. A non-existent path is dropped (can't scan).
        let canon = match resolved.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                tracing::warn!(
                    "sources.user.toml: local source '{}' path '{}' does not exist — skipping",
                    s.name,
                    resolved.display()
                );
                continue;
            }
        };
        if !canon.starts_with(&home_canon) {
            tracing::warn!(
                "sources.user.toml: local source '{}' path '{}' is OUTSIDE $HOME — rejected",
                s.name,
                canon.display()
            );
            continue;
        }
        out.push((s, canon));
    }
    Ok(out)
}

/// Scan one local source directory for skills. A "skill" is a first-level subdir that
/// contains a `SKILL.md` with valid frontmatter. Bounded by `MAX_SKILLS_PER_SOURCE`.
/// Symlinked subdirs and oversized SKILL.md files are skipped (logged). Never executes.
pub fn scan_local_source(source_name: &str, dir: &Path) -> Vec<DiscoveredSkill> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for (iterated, entry) in rd.flatten().enumerate() {
        if out.len() >= MAX_SKILLS_PER_SOURCE || iterated >= MAX_SKILLS_PER_SOURCE * 4 {
            tracing::warn!("discovery: '{source_name}' scan truncated at cap");
            break;
        }
        let p = entry.path();
        // First-level dirs only; reject symlinked subdirs (no escape).
        let Ok(md) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if md.file_type().is_symlink() || !md.is_dir() {
            continue;
        }
        let skill_md = p.join("SKILL.md");
        // ⟨audit MED⟩ Read SKILL.md via O_NOFOLLOW (check==read on one fd, no symlink-swap
        // TOCTOU) with the size cap enforced from the fstat'd handle.
        let text = match read_capped_nofollow(&skill_md, MAX_FRONTMATTER_FILE_BYTES) {
            Ok(t) => t,
            Err(_) => continue, // missing / symlink / oversized / unreadable → skip
        };
        let fm: SkillFrontmatter = match parse_skill_frontmatter(&text) {
            Ok(fm) => fm,
            Err(e) => {
                tracing::warn!("discovery: '{}' bad SKILL.md: {e}", p.display());
                continue;
            }
        };
        // ⟨audit MED⟩ Don't use symlink-following is_file() for manifest.json — a
        // symlinked manifest.json shouldn't count as "has signed candidate".
        let has_manifest = matches!(
            std::fs::symlink_metadata(p.join("manifest.json")),
            Ok(md) if md.is_file()
        );
        out.push(DiscoveredSkill {
            name: fm.name,
            version: fm.version,
            description: fm.description,
            path: p.to_string_lossy().into_owned(),
            source: source_name.to_string(),
            has_manifest,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Full local discovery: read `sources.user.toml`, scan every accepted local source.
/// Returns the flattened, de-duplicated-by-(source,name) list.
pub fn discover_local(sources_toml: &Path, home: &Path) -> Result<Vec<DiscoveredSkill>> {
    let sources = load_user_sources(sources_toml, home)?;
    let mut out = Vec::new();
    for (src, dir) in sources {
        out.extend(scan_local_source(&src.name, &dir));
    }
    Ok(out)
}

// ── furx-core signed index (the curated registry / moat) ──────────────────────

/// One entry of the furx-core registry index. The index file as a whole is Ed25519-
/// signed (payload/signature split, same as a skill manifest). P0 defines the format +
/// a verify path; the live HTTPS fetch + cache is P1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoreIndexEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    /// HTTPS URL of the signed skill bundle (imported through the F3 gate).
    pub url: String,
}

/// The furx-core registry index payload (the SIGNED object). `schema_version` + the
/// entries; the signature lives OUTSIDE this (in `CoreIndex.signature`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreIndexPayload {
    pub schema_version: u32,
    pub entries: Vec<CoreIndexEntry>,
}

/// A signed furx-core index: payload + detached hex Ed25519 signature over JCS(payload).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreIndex {
    pub payload: CoreIndexPayload,
    pub signature: String,
}

impl CoreIndexPayload {
    /// JCS-canonical bytes + the domain-separated signed message (same discipline as
    /// `SkillPayload`: the signature is structurally outside the hashed object).
    fn signed_message(&self) -> Result<Vec<u8>> {
        use sha2::{Digest, Sha256};
        let v = serde_json::to_value(self)?;
        let jcs = super::skill_manifest::canonical_json_for(&v);
        let mut h = Sha256::new();
        h.update(jcs.as_bytes());
        let jcs_hash = hex::encode(h.finalize());
        Ok(format!(
            "FURX_CORE_INDEX_V1\nschema_version={}\ncanonical_json_sha256={jcs_hash}\n",
            self.schema_version
        )
        .into_bytes())
    }
}

impl CoreIndex {
    /// Verify the index signature against the pinned `trusted` pubkeys. Fail-closed: an
    /// unsigned/invalid/untrusted index is rejected entirely (the curated registry must
    /// be signed by Furx — that's the whole point of the moat).
    pub fn verify(&self, trusted: &[String]) -> bool {
        use base64::Engine as _;
        let Ok(sig_bytes) = hex::decode(self.signature.trim()) else {
            return false;
        };
        if sig_bytes.len() != 64 {
            return false;
        }
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig_bytes);
        let Ok(msg) = self.payload.signed_message() else {
            return false;
        };
        trusted
            .iter()
            .any(|pk| super::plugin_host::verify_signature(&msg, &sig_b64, pk))
    }

    /// Verify against the compiled-in `TRUSTED_PUBKEYS`.
    pub fn verify_pinned(&self) -> bool {
        let trusted: Vec<String> = super::plugin_host::TRUSTED_PUBKEYS
            .iter()
            .map(|s| s.to_string())
            .collect();
        self.verify(&trusted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn home() -> PathBuf {
        let p = std::env::temp_dir().join(format!("furx-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_skill(dir: &Path, name: &str, version: &str, with_manifest: bool) {
        let sk = dir.join(name);
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(
            sk.join("SKILL.md"),
            format!("---\nname: {name}\nversion: {version}\ndescription: d for {name}\n---\n# {name}\n"),
        )
        .unwrap();
        if with_manifest {
            std::fs::write(sk.join("manifest.json"), "{}").unwrap();
        }
    }

    #[test]
    fn scan_finds_skills_with_valid_frontmatter() {
        let h = home();
        let skills = h.join(".hermes").join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(&skills, "dogfood", "1.0.0", false);
        write_skill(&skills, "council", "2.1.0", true);
        // A dir without SKILL.md is ignored.
        std::fs::create_dir_all(skills.join("not-a-skill")).unwrap();

        let found = scan_local_source("hermes-local", &skills);
        assert_eq!(found.len(), 2);
        let council = found.iter().find(|s| s.name == "council").unwrap();
        assert_eq!(council.version, "2.1.0");
        assert!(council.has_manifest, "manifest.json present");
        let dogfood = found.iter().find(|s| s.name == "dogfood").unwrap();
        assert!(!dogfood.has_manifest);
        std::fs::remove_dir_all(&h).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scan_rejects_symlinked_subdir_and_skill_md() {
        use std::os::unix::fs::symlink;
        let h = home();
        let skills = h.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        // a real skill elsewhere, symlinked in → must be ignored.
        let real = home();
        write_skill(&real, "evil", "1.0.0", false);
        symlink(real.join("evil"), skills.join("evil")).unwrap();
        // a dir whose SKILL.md is a symlink → ignored.
        let d = skills.join("sneaky");
        std::fs::create_dir_all(&d).unwrap();
        symlink(real.join("evil").join("SKILL.md"), d.join("SKILL.md")).unwrap();
        let found = scan_local_source("x", &skills);
        assert!(found.is_empty(), "symlinked subdir + symlinked SKILL.md ignored: {found:?}");
        std::fs::remove_dir_all(&h).ok();
        std::fs::remove_dir_all(&real).ok();
    }

    #[test]
    fn user_sources_accepts_local_inside_home_rejects_outside() {
        let h = home();
        // a local source inside $HOME (exists).
        let inside = h.join(".hermes").join("skills");
        std::fs::create_dir_all(&inside).unwrap();
        let toml = h.join("sources.user.toml");
        std::fs::write(
            &toml,
            "[[sources]]\nname = \"hermes\"\ntype = \"local\"\npath = \"~/.hermes/skills\"\n\
             [[sources]]\nname = \"etc\"\ntype = \"local\"\npath = \"/etc\"\n\
             [[sources]]\nname = \"remote\"\ntype = \"signed-registry\"\npath = \"https://x\"\n",
        )
        .unwrap();
        let sources = load_user_sources(&toml, &h).unwrap();
        // Only the in-$HOME local source survives.
        assert_eq!(sources.len(), 1, "got {sources:?}");
        assert_eq!(sources[0].0.name, "hermes");
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn user_sources_oversized_file_rejected() {
        // ⟨audit MED⟩ a huge sources.user.toml is rejected before parsing (no OOM).
        let h = home();
        let toml = h.join("sources.user.toml");
        let big = "x".repeat((MAX_SOURCES_TOML_BYTES as usize) + 1024);
        std::fs::write(&toml, big).unwrap();
        assert!(load_user_sources(&toml, &h).is_err(), "oversized config rejected");
        std::fs::remove_dir_all(&h).ok();
    }

    #[cfg(unix)]
    #[test]
    fn scan_ignores_symlinked_manifest_json() {
        // ⟨audit MED⟩ a symlinked manifest.json must NOT count as a signed candidate.
        use std::os::unix::fs::symlink;
        let h = home();
        let skills = h.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(&skills, "council", "1.0.0", false);
        let real_manifest = h.join("real.json");
        std::fs::write(&real_manifest, "{}").unwrap();
        symlink(&real_manifest, skills.join("council").join("manifest.json")).unwrap();
        let found = scan_local_source("x", &skills);
        assert_eq!(found.len(), 1);
        assert!(!found[0].has_manifest, "symlinked manifest.json is not a signed candidate");
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn user_sources_missing_file_is_empty() {
        let h = home();
        let toml = h.join("nope.toml");
        assert!(load_user_sources(&toml, &h).unwrap().is_empty());
        std::fs::remove_dir_all(&h).ok();
    }

    #[test]
    fn discover_local_end_to_end() {
        let h = home();
        let skills = h.join(".openclaw").join("workspace").join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        write_skill(&skills, "release-notes-gen", "1.0.0", false);
        let toml = h.join("sources.user.toml");
        std::fs::write(
            &toml,
            "[[sources]]\nname = \"openclaw\"\ntype = \"local\"\npath = \"~/.openclaw/workspace/skills\"\n",
        )
        .unwrap();
        let found = discover_local(&toml, &h).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "release-notes-gen");
        assert_eq!(found[0].source, "openclaw");
        std::fs::remove_dir_all(&h).ok();
    }

    // ── furx-core signed index ───────────────────────────────────────────────
    fn sign_index(seed: [u8; 32], payload: CoreIndexPayload) -> (CoreIndex, String) {
        use base64::Engine as _;
        let sk = SigningKey::from_bytes(&seed);
        let pk = base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes());
        let msg = payload.signed_message().unwrap();
        let sig = sk.sign(&msg);
        (
            CoreIndex {
                payload,
                signature: hex::encode(sig.to_bytes()),
            },
            pk,
        )
    }

    #[test]
    fn core_index_verifies_with_trusted_key_and_rejects_tamper() {
        let payload = CoreIndexPayload {
            schema_version: 1,
            entries: vec![CoreIndexEntry {
                name: "council".into(),
                version: "1.0.0".into(),
                description: "frontier council".into(),
                url: "https://registry.furx.dev/council-1.0.0.tar".into(),
            }],
        };
        let (idx, pk) = sign_index([5u8; 32], payload);
        assert!(idx.verify(std::slice::from_ref(&pk)), "signed index verifies");
        assert!(!idx.verify(&[]), "no trusted key → reject");
        // Tamper an entry → signature breaks.
        let mut bad = idx.clone();
        bad.payload.entries[0].url = "https://evil/x.tar".into();
        assert!(!bad.verify(&[pk]), "tampered index must be rejected");
    }

    #[test]
    fn core_index_unsigned_is_rejected() {
        let idx = CoreIndex {
            payload: CoreIndexPayload {
                schema_version: 1,
                entries: vec![],
            },
            signature: String::new(),
        };
        assert!(!idx.verify_pinned(), "empty signature → reject (fail-closed)");
    }

    #[test]
    fn core_index_deny_unknown_fields() {
        let bad = r#"{"payload":{"schema_version":1,"entries":[],"evil":1},"signature":""}"#;
        assert!(serde_json::from_str::<CoreIndex>(bad).is_err());
    }
}
