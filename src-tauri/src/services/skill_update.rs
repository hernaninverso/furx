// spec-kit 046 · Ola 7 (Skills P1) F1 — `furx skill update` versionado.
//
// La Ola 4 es INSTALL-ONLY: instala en `plugins/<name>/` con un `rename(2)` atómico, y
// REHÚSA re-instalar un name existente porque `rename(2)` sobre un directorio NO vacío NO
// es atómico en APFS (ENOTEMPTY / no RENAME_SWAP portable). Update queda "fuera de scope"
// del P0 por eso mismo.
//
// Esta ola agrega update con un LAYOUT VERSIONADO que evita el rename-over-dir:
//
//   plugins/<name>/
//   ├── versions/
//   │   ├── <tree_hash_v1>/   (SKILL.md, manifest.json, scripts/)
//   │   └── <tree_hash_v2>/
//   └── current  ->  versions/<tree_hash_vN>/   (symlink RELATIVO)
//
// El swap atómico es sobre el SYMLINK, no sobre el directorio: se crea un symlink temporal
// `current.tmp_<uuid>` apuntando a la versión nueva y se hace `rename(2)` de ESE symlink
// sobre `current`. `rename(2)` que reemplaza un symlink existente por otro SÍ es atómico
// (es un reemplazo de entrada de directorio, no un rename-over-non-empty-dir). Rollback =
// re-apuntar `current` a un hash previo. GC = borrar dirs `versions/<hash>/` viejos
// (manteniendo N, nunca la `current`).
//
// FAIL-CLOSED, igual que la Ola 4: cada versión nueva pasa por el MISMO gate (tree_hash +
// firma Ed25519) en un staging privado ANTES de moverse a `versions/<hash>/`; una versión
// `Rejected` NO se instala. La identidad de una versión es su `tree_hash` (el dir se llama
// como el hash), así que un swap de bytes cambia el nombre del dir → no puede colisionar.
//
// Dead-code-first: probado en aislamiento aquí; el wiring del comando Tauri + UI es aparte.
// Coexiste con el install-only de la Ola 4 sin tocarlo: un skill instalado en el layout
// PLANO (`plugins/<name>/SKILL.md`) sigue intacto; el update versionado se usa para skills
// gestionados con este layout nuevo. La detección del layout es por la presencia del dir
// `versions/`.
//
// THREAT-MODEL BOUNDARIES (heredadas de la Ola 4, documentadas — NO son bugs nuevos):
//   - El `tree_hash` cubre `scripts/` (el contenido EJECUTABLE firmado). `SKILL.md`/
//     `manifest.json` son metadata que NUNCA se ejecuta — el gate ya rechaza un name/version
//     que no concuerda entre ambos. Esto es idéntico al `skill_import` de la Ola 4.
//   - Existe una ventana sub-ms entre la re-verificación final (`tree_hash`) y el swap del
//     symlink en la que un atacante con el MISMO UID (ya tiene los privilegios del usuario)
//     podría chmod+swap. El cierre REAL es run-time: `skill_registry::reverify_or_inert`
//     re-hashea las scripts vivas vs el `tree_hash` registrado ANTES de CADA spawn → un
//     swap post-install se caza en ejecución y el skill queda inert. Cerrar la ventana de
//     instalación por completo necesita inmutabilidad del SO (`UF_IMMUTABLE`) — misma
//     postura que `plugin_host::run_tool`.
//   - DB↔disco: el disco es la fuente de verdad (el symlink `current`). El swap se hace
//     ANTES del write a la DB; si la DB falla tras el swap, el disco ya refleja la versión
//     nueva y `reverify_or_inert` (que lee el `tree_hash` registrado de la fila `plugins`)
//     más el sweep de recovery de la Ola 4 reconcilian. Bajo el flock hay un solo escritor.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

use super::plugin_host::{harden_readonly, relax_writable_pub};
use super::skill_import::{read_capped_nofollow, ImportSource};
use super::skill_manifest::{tree_hash, SkillManifest, TrustLevel};
use super::skill_registry::with_durable_immediate;

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// How many NON-current versions to keep on disk after a successful update (GC retention).
/// The `current` version is NEVER garbage-collected regardless of this bound.
pub const DEFAULT_KEEP_VERSIONS: usize = 3;

/// ⟨audit codex MED⟩ Validate a skill `name` as a SINGLE safe path component before it is
/// ever joined into a filesystem path (defense-in-depth against traversal even if an
/// upstream validation is bypassed). Mirrors `skill_import::is_safe_skill_name`.
fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// ⟨audit codex MED⟩ Validate a `tree_hash` as exactly 64 lowercase hex chars (the SHA-256
/// the version dir is named by) before it is joined into a path. A hash that doesn't match
/// this shape can never be a dir we wrote → reject (no traversal via a hostile hash).
fn is_safe_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Outcome of an update or rollback: which tree_hash is now `current` + its trust level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionOutcome {
    pub name: String,
    pub version: String,
    pub tree_hash: String,
    pub level: TrustLevel,
    /// Tree hashes that were garbage-collected from disk by this call.
    pub gc_removed: Vec<String>,
}

/// A recorded version row (DB view of `skill_versions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    pub name: String,
    pub tree_hash: String,
    pub version: String,
    pub trust_level: Option<TrustLevel>,
    pub is_current: bool,
    pub installed_at: String,
}

fn level_to_str(l: TrustLevel) -> &'static str {
    match l {
        TrustLevel::Verified => "verified",
        TrustLevel::SandboxedPromoted => "promoted",
        TrustLevel::Sandboxed => "sandboxed",
        TrustLevel::Rejected => "rejected",
    }
}

fn str_to_level(s: &str) -> Option<TrustLevel> {
    match s {
        "verified" => Some(TrustLevel::Verified),
        "promoted" => Some(TrustLevel::SandboxedPromoted),
        "sandboxed" => Some(TrustLevel::Sandboxed),
        "rejected" => Some(TrustLevel::Rejected),
        _ => None,
    }
}

/// The import flock path (`~/.furx/.import.lock`) — shared serialization point with the
/// Ola 4 install-only path so an update never races a concurrent add/update.
fn import_lock_path(furx_dir: &Path) -> PathBuf {
    furx_dir.join(".import.lock")
}

fn versions_dir(plugins_base: &Path, name: &str) -> PathBuf {
    plugins_base.join(name).join("versions")
}

fn current_link(plugins_base: &Path, name: &str) -> PathBuf {
    plugins_base.join(name).join("current")
}

/// `true` if `<name>` is managed with the versioned layout (has a `versions/` dir).
pub fn is_versioned(plugins_base: &Path, name: &str) -> bool {
    versions_dir(plugins_base, name).is_dir()
}

/// SKILL.md frontmatter name/version validation reused from import (light wrapper so the
/// caller can read name/version without pulling the whole import flow).
fn read_name_version_manifest(src_dir: &Path) -> Result<(String, String, Option<SkillManifest>)> {
    use super::skill_import::parse_skill_frontmatter;
    let skill_md_path = src_dir.join("SKILL.md");
    if !skill_md_path.exists() {
        return Err(anyhow!("source has no SKILL.md"));
    }
    let skill_md = read_capped_nofollow(&skill_md_path, MAX_MANIFEST_BYTES)
        .map_err(|e| anyhow!("SKILL.md: {e}"))?;
    let fm = parse_skill_frontmatter(&skill_md)?;
    let manifest_path = src_dir.join("manifest.json");
    let manifest: Option<SkillManifest> = if manifest_path.exists() {
        let text = read_capped_nofollow(&manifest_path, MAX_MANIFEST_BYTES)
            .map_err(|e| anyhow!("manifest.json: {e}"))?;
        Some(serde_json::from_str(&text).map_err(|e| anyhow!("manifest.json parse: {e}"))?)
    } else {
        None
    };
    if let Some(m) = &manifest {
        if m.payload.name != fm.name {
            return Err(anyhow!(
                "name mismatch: SKILL.md '{}' vs manifest '{}'",
                fm.name,
                m.payload.name
            ));
        }
        if m.payload.version != fm.version {
            return Err(anyhow!(
                "version mismatch: SKILL.md '{}' vs manifest '{}'",
                fm.version,
                m.payload.version
            ));
        }
    }
    Ok((fm.name, fm.version, manifest))
}

/// Copy `src` → `dest` rejecting symlinks (mirrors the import staging copy; kept local).
fn copy_dir_no_symlinks(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let md = std::fs::symlink_metadata(&from)?;
        let ft = md.file_type();
        if ft.is_symlink() {
            return Err(anyhow!("refusing to copy symlink: {}", from.display()));
        }
        let to = dest.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_no_symlinks(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        } else {
            return Err(anyhow!("refusing to copy non-regular file: {}", from.display()));
        }
    }
    Ok(())
}

/// Atomically point `current` → `versions/<tree_hash>` (a RELATIVE symlink). The swap is
/// done on a unix platform with `symlink` of a temp link + `rename(2)` over the existing
/// `current` (atomic replacement of a symlink — NOT a rename-over-dir). On non-unix we
/// fall back to remove+create (best-effort; the project targets macOS).
fn atomic_point_current(plugins_base: &Path, name: &str, tree_hash: &str) -> Result<()> {
    // ⟨audit codex MED⟩ Validate both components locally before building any path.
    if !is_safe_name(name) {
        return Err(anyhow!("unsafe skill name: {name}"));
    }
    if !is_safe_hash(tree_hash) {
        return Err(anyhow!("unsafe tree_hash: {tree_hash}"));
    }
    let link = current_link(plugins_base, name);
    // RELATIVE target so the link survives a move of the whole plugins tree.
    let target: PathBuf = Path::new("versions").join(tree_hash);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let parent = link
            .parent()
            .ok_or_else(|| anyhow!("current link has no parent"))?;
        let tmp = parent.join(format!("current.tmp_{}", uuid::Uuid::new_v4()));
        // Create the temp symlink, then rename it over `current` (atomic on the same dir).
        symlink(&target, &tmp).map_err(|e| anyhow!("create temp symlink: {e}"))?;
        match std::fs::rename(&tmp, &link) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(anyhow!("atomic symlink swap failed: {e}"))
            }
        }
    }
    #[cfg(not(unix))]
    {
        if link.exists() || std::fs::symlink_metadata(&link).is_ok() {
            let _ = std::fs::remove_file(&link);
        }
        std::os::windows::fs::symlink_dir(&target, &link)
            .map_err(|e| anyhow!("create symlink: {e}"))
    }
}

/// Read the tree_hash that `current` currently points at (the basename of its target).
/// `None` if there is no `current` symlink (e.g. a half-created layout).
pub fn current_tree_hash(plugins_base: &Path, name: &str) -> Option<String> {
    let link = current_link(plugins_base, name);
    let target = std::fs::read_link(&link).ok()?;
    // ⟨audit codex LOW⟩ Require the target to be EXACTLY `versions/<64-hex>`; an externally
    // modified link (e.g. pointing elsewhere or with a non-hash basename) must NOT be
    // mistaken for a valid current version → return None (treated as "no valid current").
    let mut comps = target.components();
    let first = comps.next()?;
    if first.as_os_str() != std::ffi::OsStr::new("versions") {
        return None;
    }
    let hash = comps.next()?.as_os_str().to_string_lossy().to_string();
    if comps.next().is_some() || !is_safe_hash(&hash) {
        return None;
    }
    Some(hash)
}

/// DB: list every recorded version of `name`, newest first by `installed_at`.
pub fn list_versions(conn: &Connection, name: &str) -> Result<Vec<VersionRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, tree_hash, version, trust_level, is_current, installed_at \
         FROM skill_versions WHERE name = ? ORDER BY installed_at DESC, tree_hash ASC",
    )?;
    let rows = stmt.query_map(params![name], |r| {
        Ok(VersionRow {
            name: r.get(0)?,
            tree_hash: r.get(1)?,
            version: r.get(2)?,
            trust_level: r.get::<_, Option<String>>(3)?.as_deref().and_then(str_to_level),
            is_current: r.get::<_, i64>(4)? != 0,
            installed_at: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// DB: record a version row + flip `is_current` to it (clearing the previous current),
/// all in one durable IMMEDIATE tx. Used after the symlink swap committed on disk so the
/// DB tracks what the symlink reflects.
fn db_set_current_version(
    conn: &Connection,
    name: &str,
    tree_hash: &str,
    version: &str,
    level: TrustLevel,
) -> Result<()> {
    with_durable_immediate(conn, |c| {
        // Upsert the (name, tree_hash) row.
        c.execute(
            "INSERT INTO skill_versions (name, tree_hash, version, trust_level, is_current) \
             VALUES (?, ?, ?, ?, 0) \
             ON CONFLICT(name, tree_hash) DO UPDATE SET version=excluded.version, \
                trust_level=excluded.trust_level",
            params![name, tree_hash, version, level_to_str(level)],
        )?;
        // Exactly-one-current: clear all, then set this one. The partial unique index
        // guarantees AT MOST one row holds is_current=1; we clear first then set.
        c.execute(
            "UPDATE skill_versions SET is_current=0 WHERE name=?",
            params![name],
        )?;
        // ⟨audit codex MED⟩ The partial unique index only enforces AT-MOST-one current; it
        // does NOT guarantee the set-current actually matched a row. Require EXACTLY one row
        // updated (the row we just upserted) — otherwise abort the tx (rolled back by
        // with_durable_immediate) so we never commit zero current rows for this skill.
        let set = c.execute(
            "UPDATE skill_versions SET is_current=1 WHERE name=? AND tree_hash=?",
            params![name, tree_hash],
        )?;
        if set != 1 {
            return Err(anyhow!(
                "db_set_current_version: expected to set exactly 1 current row for '{name}'/{tree_hash}, set {set}"
            ));
        }
        Ok(())
    })
}

/// DB: remove version rows for `hashes` (used after GC removes their dirs from disk).
fn db_remove_versions(conn: &Connection, name: &str, hashes: &[String]) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }
    with_durable_immediate(conn, |c| {
        for h in hashes {
            c.execute(
                "DELETE FROM skill_versions WHERE name=? AND tree_hash=? AND is_current=0",
                params![name, h],
            )?;
        }
        Ok(())
    })
}

/// Stage a source into a private `.tmp_<uuid>` under `versions/`, run the gate, and return
/// the resolved (level, tree_hash) WITHOUT publishing. The staging dir is hardened
/// read-only and re-hashed (same discipline as `skill_import::import_locked`).
fn stage_and_gate(
    versions: &Path,
    src_dir: &Path,
    manifest: &Option<SkillManifest>,
    trusted: &[String],
    revoked: &std::collections::HashSet<String>,
) -> Result<(TrustLevel, String, PathBuf)> {
    std::fs::create_dir_all(versions)?;
    let staging = versions.join(format!(".tmp_{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(TrustLevel, String)> {
        std::fs::create_dir(&staging)?;
        // Copy metadata + scripts (rejecting symlinks).
        let skill_md = read_capped_nofollow(&src_dir.join("SKILL.md"), MAX_MANIFEST_BYTES)?;
        std::fs::write(staging.join("SKILL.md"), skill_md.as_bytes())?;
        if let Some(m) = manifest {
            std::fs::write(staging.join("manifest.json"), serde_json::to_string_pretty(m)?)?;
        }
        let src_scripts = src_dir.join("scripts");
        let staging_scripts = staging.join("scripts");
        match std::fs::symlink_metadata(&src_scripts) {
            Ok(md) if md.file_type().is_symlink() => {
                return Err(anyhow!("scripts/ is a symlink — refusing"));
            }
            Ok(md) if md.is_dir() => copy_dir_no_symlinks(&src_scripts, &staging_scripts)?,
            Ok(_) => return Err(anyhow!("scripts exists but is not a directory")),
            Err(_) => { /* no scripts → empty tree */ }
        }
        let computed = tree_hash(&staging_scripts)?;
        let level = match manifest {
            Some(m) => m.gate(trusted, revoked, Some(&computed)).level,
            None => TrustLevel::Sandboxed,
        };
        if level == TrustLevel::Rejected {
            return Err(anyhow!("manifest rejected by trust gate — not installing"));
        }
        Ok((level, computed))
    })();
    match result {
        Ok((level, computed)) => Ok((level, computed, staging)),
        Err(e) => {
            let _ = relax_writable_pub(&staging);
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// FR-001 — update (or first versioned install) of `name` from `source`.
///
/// Flow (under the shared import flock):
///   1. PARSE: read name/version/manifest from source; reject name/version mismatch.
///   2. STAGE+GATE: copy into a private `.tmp_<uuid>` under `versions/`, gate it. Reject
///      a `Rejected` version (fail-closed).
///   3. PUBLISH the version dir: rename `.tmp_<uuid>` → `versions/<tree_hash>/` (atomic;
///      the dest never pre-exists because it's named by content hash → no rename-over-dir
///      collision; if the SAME tree_hash is already installed it's a no-op re-point).
///   4. SWAP `current` → `versions/<tree_hash>` atomically (symlink rename).
///   5. DB: record + flip is_current.
///   6. GC: keep the newest `keep` non-current versions; remove older dirs + rows.
///
/// `keep` = how many non-current versions to retain (`DEFAULT_KEEP_VERSIONS` if unsure).
// The explicit args mirror `skill_import::import_skill` (conn/furx_dir/plugins_base/source/
// trusted/revoked) plus `name_hint` + `keep`; bundling them into a struct would only move
// the same parameters elsewhere without clarifying the call.
#[allow(clippy::too_many_arguments)]
pub fn update_skill(
    conn: &Connection,
    furx_dir: &Path,
    plugins_base: &Path,
    name_hint: &str,
    source: ImportSource,
    trusted: &[String],
    revoked: &std::collections::HashSet<String>,
    keep: usize,
) -> Result<VersionOutcome> {
    use fs2::FileExt;
    let src_dir = match source {
        ImportSource::Local(p) => p,
        ImportSource::Url(_) => {
            return Err(anyhow!("URL import is not wired — use a local path"))
        }
    };
    std::fs::create_dir_all(furx_dir)?;
    std::fs::create_dir_all(plugins_base)?;

    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(import_lock_path(furx_dir))?;
    FileExt::lock_exclusive(&lock_file)?;
    let result = update_locked(conn, plugins_base, name_hint, &src_dir, trusted, revoked, keep);
    let _ = FileExt::unlock(&lock_file);
    result
}

fn update_locked(
    conn: &Connection,
    plugins_base: &Path,
    name_hint: &str,
    src_dir: &Path,
    trusted: &[String],
    revoked: &std::collections::HashSet<String>,
    keep: usize,
) -> Result<VersionOutcome> {
    // 1. LOCATE + PARSE. Reject symlinked source / self-import (same as import).
    let src_md = std::fs::symlink_metadata(src_dir).map_err(|e| anyhow!("source: {e}"))?;
    if src_md.file_type().is_symlink() {
        return Err(anyhow!("refusing to update from a symlinked source"));
    }
    if !src_md.is_dir() {
        return Err(anyhow!("update source must be a directory"));
    }
    let base_c = plugins_base
        .canonicalize()
        .map_err(|e| anyhow!("plugins_base canonicalize: {e}"))?;
    let src_c = src_dir
        .canonicalize()
        .map_err(|e| anyhow!("source canonicalize: {e}"))?;
    if src_c.starts_with(&base_c) {
        return Err(anyhow!("refusing to update from a path inside the plugins dir"));
    }
    let (name, version, manifest) = read_name_version_manifest(src_dir)?;
    // The source must match the skill the caller intends to update.
    if name != name_hint {
        return Err(anyhow!(
            "source skill '{name}' does not match target '{name_hint}'"
        ));
    }

    let versions = versions_dir(plugins_base, &name);

    // 2. STAGE + GATE in a private staging dir.
    let (level, computed_hash, staging) =
        stage_and_gate(&versions, src_dir, &manifest, trusted, revoked)?;

    // 3. PUBLISH the version dir. Dest = versions/<tree_hash>/ (named by content). If it
    // already exists (same content re-installed) → drop staging, treat as re-point.
    let dest = versions.join(&computed_hash);
    let published = (|| -> Result<()> {
        if dest.exists() {
            // ⟨audit codex HIGH⟩ A pre-existing `versions/<hash>/` dir must NOT be trusted
            // blindly — re-hash it and require it STILL equals the content hash it's named
            // by. A tampered/pre-created dir whose scripts no longer hash to <hash> is
            // rejected (fail-closed: we never re-point `current` at unverified bytes).
            let existing = tree_hash(&dest.join("scripts"))
                .map_err(|e| anyhow!("re-hash of existing version dir failed: {e}"))?;
            if !existing.eq_ignore_ascii_case(&computed_hash) {
                return Err(anyhow!(
                    "existing version dir '{computed_hash}' tree_hash mismatch (got {existing}) — refusing to re-point"
                ));
            }
            // Verified same content already on disk → discard staging, re-point + re-record.
            let _ = relax_writable_pub(&staging);
            let _ = std::fs::remove_dir_all(&staging);
            return Ok(());
        }
        std::fs::rename(&staging, &dest).map_err(|e| anyhow!("publish rename failed: {e}"))?;
        let _ = harden_readonly(&dest);
        // Re-hash the published dir; require it equals the gate-verified hash.
        let republished = tree_hash(&dest.join("scripts"))
            .map_err(|e| anyhow!("post-publish re-hash failed: {e}"))?;
        if !republished.eq_ignore_ascii_case(&computed_hash) {
            return Err(anyhow!(
                "post-publish tree_hash mismatch (verified {computed_hash}, published {republished})"
            ));
        }
        Ok(())
    })();
    if let Err(e) = published {
        let _ = relax_writable_pub(&staging);
        let _ = std::fs::remove_dir_all(&staging);
        if dest.exists() {
            let _ = relax_writable_pub(&dest);
            let _ = std::fs::remove_dir_all(&dest);
        }
        return Err(e);
    }

    // 4. ATOMIC symlink swap: current → versions/<tree_hash>.
    atomic_point_current(plugins_base, &name, &computed_hash)?;

    // 5. DB: record + flip current.
    db_set_current_version(conn, &name, &computed_hash, &version, level)?;

    // 6. GC older non-current versions (keep the newest `keep`).
    let gc_removed = gc_versions(conn, plugins_base, &name, keep)?;

    Ok(VersionOutcome {
        name,
        version,
        tree_hash: computed_hash,
        level,
        gc_removed,
    })
}

/// FR-001 — rollback `name`'s `current` to a previously-installed `target_hash`. The target
/// version dir must still exist on disk (not GC'd) and be recorded. Re-points the symlink
/// atomically + flips the DB current. Does NOT run GC (rollback should not delete the
/// version you rolled away from).
pub fn rollback_skill(
    conn: &Connection,
    furx_dir: &Path,
    plugins_base: &Path,
    name: &str,
    target_hash: &str,
) -> Result<VersionOutcome> {
    use fs2::FileExt;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(import_lock_path(furx_dir))?;
    FileExt::lock_exclusive(&lock_file)?;
    let result = (|| -> Result<VersionOutcome> {
        // ⟨audit codex MED⟩ Validate path components before any join.
        if !is_safe_name(name) {
            return Err(anyhow!("unsafe skill name: {name}"));
        }
        if !is_safe_hash(target_hash) {
            return Err(anyhow!("unsafe target tree_hash: {target_hash}"));
        }
        let dest = versions_dir(plugins_base, name).join(target_hash);
        if !dest.is_dir() {
            return Err(anyhow!(
                "cannot rollback '{name}': version '{target_hash}' is not on disk"
            ));
        }
        // The recorded row tells us the version string + trust level to restore.
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT version, trust_level FROM skill_versions WHERE name=? AND tree_hash=?",
                params![name, target_hash],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (version, level) = match row {
            Some((v, lvl)) => (v, lvl.as_deref().and_then(str_to_level)),
            None => {
                return Err(anyhow!(
                    "cannot rollback '{name}': version '{target_hash}' is not recorded"
                ))
            }
        };
        let level = level.ok_or_else(|| {
            anyhow!("cannot rollback '{name}': version '{target_hash}' has no trust level")
        })?;
        // ⟨audit codex HIGH⟩ Never re-point `current` at a `Rejected` version (the schema
        // can hold 'rejected' rows). Fail-closed.
        if level == TrustLevel::Rejected {
            return Err(anyhow!(
                "cannot rollback '{name}': version '{target_hash}' is Rejected"
            ));
        }
        // Re-verify the target tree on disk still matches its hash (fail-closed). A version
        // whose bytes were tampered post-install must NOT be re-pointed to. ⟨audit codex LOW⟩
        // Propagate the hash error with context (don't swallow it via unwrap_or_default).
        let actual = tree_hash(&dest.join("scripts"))
            .map_err(|e| anyhow!("cannot rollback '{name}': re-hash of '{target_hash}' failed: {e}"))?;
        if !actual.eq_ignore_ascii_case(target_hash) {
            return Err(anyhow!(
                "cannot rollback '{name}': version '{target_hash}' tree_hash mismatch (got {actual})"
            ));
        }
        atomic_point_current(plugins_base, name, target_hash)?;
        db_set_current_version(conn, name, target_hash, &version, level)?;
        Ok(VersionOutcome {
            name: name.to_string(),
            version,
            tree_hash: target_hash.to_string(),
            level,
            gc_removed: vec![],
        })
    })();
    let _ = FileExt::unlock(&lock_file);
    result
}

/// GC: keep the newest `keep` NON-current versions; remove older version dirs from disk +
/// their DB rows. NEVER removes the `current` version. Returns the removed tree hashes.
/// Best-effort on disk (a dir that fails to remove is left + logged), but the DB row is
/// only removed when the dir is gone (so the DB never claims a version exists that doesn't,
/// nor vice-versa for the removal set).
pub fn gc_versions(
    conn: &Connection,
    plugins_base: &Path,
    name: &str,
    keep: usize,
) -> Result<Vec<String>> {
    let rows = list_versions(conn, name)?; // newest first
    let mut non_current: Vec<&VersionRow> = rows.iter().filter(|r| !r.is_current).collect();
    // Already newest-first from the query; keep the first `keep`, GC the rest.
    if non_current.len() <= keep {
        return Ok(vec![]);
    }
    let to_remove: Vec<String> = non_current
        .split_off(keep)
        .into_iter()
        .map(|r| r.tree_hash.clone())
        .collect();
    let mut removed = Vec::new();
    let versions = versions_dir(plugins_base, name);
    let current = current_tree_hash(plugins_base, name);
    for h in &to_remove {
        // Defensive: never remove the dir the symlink points at, even if the DB disagrees.
        if current.as_deref() == Some(h.as_str()) {
            tracing::warn!("gc: skipping '{name}'/{h}: it is the current symlink target");
            continue;
        }
        let dir = versions.join(h);
        let _ = relax_writable_pub(&dir);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed.push(h.clone()),
            Err(e) if !dir.exists() => {
                // Already gone → still drop the row.
                tracing::warn!("gc: '{name}'/{h} dir already absent: {e}");
                removed.push(h.clone());
            }
            Err(e) => {
                tracing::warn!("gc: failed to remove '{name}'/{h}: {e} — keeping row");
            }
        }
    }
    db_remove_versions(conn, name, &removed)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::skill_import::make_test_payload;
    use super::super::skill_manifest::{pubkey_b64_sha256, SkillManifest, SkillPayload};
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/010_b5.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/049_skill_trust.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/052_skill_versions.sql")).unwrap();
        conn
    }

    fn tmp(prefix: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("furx-{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_skill_md(dir: &Path, name: &str, version: &str) {
        let md = format!("---\nname: {name}\nversion: {version}\ndescription: a test skill\n---\n# {name}\n");
        std::fs::write(dir.join("SKILL.md"), md).unwrap();
    }

    fn write_scripts(dir: &Path, body: &[u8]) -> String {
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("run.sh"), body).unwrap();
        tree_hash(&s).unwrap()
    }

    fn sign_manifest(seed: [u8; 32], mut payload: SkillPayload) -> (SkillManifest, String) {
        use base64::Engine as _;
        let sk = SigningKey::from_bytes(&seed);
        let pk_b64 = base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes());
        let key_hex = pubkey_b64_sha256(&pk_b64).unwrap();
        payload.key_id = format!("{key_hex}_1");
        let msg = payload.signed_message().unwrap();
        let sig = sk.sign(&msg);
        (
            SkillManifest { payload, signature: hex::encode(sig.to_bytes()) },
            pk_b64,
        )
    }

    /// Build a signed source dir with a given version + script body; returns (dir, tree_hash, pk).
    fn signed_source(name: &str, version: &str, body: &[u8]) -> (PathBuf, String, String) {
        let src = tmp("src");
        write_skill_md(&src, name, version);
        let th = write_scripts(&src, body);
        let (m, pk) = sign_manifest([7u8; 32], make_test_payload(name, version, &th));
        std::fs::write(src.join("manifest.json"), serde_json::to_string(&m).unwrap()).unwrap();
        (src, th, pk)
    }

    // ── SC-001: install v1 → update v2 → current points v2, v1 in versions/ ──────
    #[cfg(unix)]
    #[test]
    fn update_versioned_swaps_current_and_keeps_old() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();

        let (src1, th1, pk) = signed_source("council", "1.0.0", b"v1\n");
        let out1 = update_skill(&conn, &furx, &base, "council", ImportSource::Local(src1.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        assert_eq!(out1.level, TrustLevel::Verified);
        assert_eq!(out1.tree_hash, th1);
        // current → versions/<th1>
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(th1.as_str()));
        assert!(base.join("council").join("versions").join(&th1).join("scripts").join("run.sh").is_file());
        assert!(is_versioned(&base, "council"));

        let (src2, th2, _) = signed_source("council", "2.0.0", b"v2-content\n");
        assert_ne!(th1, th2);
        let out2 = update_skill(&conn, &furx, &base, "council", ImportSource::Local(src2.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        assert_eq!(out2.tree_hash, th2);
        // current now → v2; v1 still on disk (within keep).
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(th2.as_str()));
        assert!(base.join("council").join("versions").join(&th1).is_dir(), "v1 retained");
        assert!(base.join("council").join("versions").join(&th2).is_dir());
        // DB: exactly one current = v2.
        let vers = list_versions(&conn, "council").unwrap();
        assert_eq!(vers.len(), 2);
        let cur: Vec<_> = vers.iter().filter(|v| v.is_current).collect();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].tree_hash, th2);

        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src1).ok();
        std::fs::remove_dir_all(&src2).ok();
    }

    // ── SC-001: rollback re-points to v1 ─────────────────────────────────────────
    #[cfg(unix)]
    #[test]
    fn rollback_re_points_to_previous() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src1, th1, pk) = signed_source("council", "1.0.0", b"v1\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src1.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        let (src2, th2, _) = signed_source("council", "2.0.0", b"v2\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src2.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(th2.as_str()));
        // Rollback to v1.
        let out = rollback_skill(&conn, &furx, &base, "council", &th1).unwrap();
        assert_eq!(out.tree_hash, th1);
        assert_eq!(out.version, "1.0.0");
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(th1.as_str()));
        let cur: Vec<_> = list_versions(&conn, "council").unwrap().into_iter().filter(|v| v.is_current).collect();
        assert_eq!(cur.len(), 1);
        assert_eq!(cur[0].tree_hash, th1);
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src1).ok();
        std::fs::remove_dir_all(&src2).ok();
    }

    // ── rollback to a non-existent / unrecorded version is rejected ──────────────
    #[cfg(unix)]
    #[test]
    fn rollback_to_missing_version_rejected() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src1, _th1, pk) = signed_source("council", "1.0.0", b"v1\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src1.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        let r = rollback_skill(&conn, &furx, &base, "council", "deadbeefdeadbeef");
        assert!(r.is_err(), "rollback to absent version must fail");
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src1).ok();
    }

    // ── GC keeps N non-current versions ──────────────────────────────────────────
    #[cfg(unix)]
    #[test]
    fn gc_keeps_only_n_non_current_versions() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let mut srcs = vec![];
        let mut hashes = vec![];
        let (_, _, pk) = signed_source("council", "0.0.0", b"seed-unused\n");
        // keep=1 → after installing 3 versions, only current + 1 non-current remain (2 total).
        for i in 0..3u8 {
            let body = format!("version-body-{i}\n");
            let (src, th, _) = signed_source("council", &format!("{}.0.0", i + 1), body.as_bytes());
            // Tiny sleep so installed_at ordering is deterministic across versions.
            std::thread::sleep(std::time::Duration::from_millis(1100));
            update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
                std::slice::from_ref(&pk), &HashSet::new(), 1).unwrap();
            hashes.push(th);
            srcs.push(src);
        }
        // current = the last installed.
        let last = hashes.last().unwrap();
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(last.as_str()));
        // On disk: current + 1 retained non-current = 2 version dirs.
        let vdir = base.join("council").join("versions");
        let dirs: Vec<_> = std::fs::read_dir(&vdir).unwrap().flatten()
            .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with(".tmp_"))
            .collect();
        assert_eq!(dirs.len(), 2, "current + keep(1) non-current");
        // The OLDEST version's dir is gone.
        assert!(!vdir.join(&hashes[0]).is_dir(), "oldest GC'd");
        // DB rows match (2 rows).
        assert_eq!(list_versions(&conn, "council").unwrap().len(), 2);
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        for s in srcs { std::fs::remove_dir_all(&s).ok(); }
    }

    // ── fail-closed: a tampered (tree_hash-mismatching) source is rejected ────────
    #[cfg(unix)]
    #[test]
    fn update_rejects_tampered_tree() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let src = tmp("src");
        write_skill_md(&src, "council", "1.0.0");
        let th = write_scripts(&src, b"orig\n");
        let (m, pk) = sign_manifest([7u8; 32], make_test_payload("council", "1.0.0", &th));
        std::fs::write(src.join("manifest.json"), serde_json::to_string(&m).unwrap()).unwrap();
        // tamper AFTER signing.
        std::fs::write(src.join("scripts").join("run.sh"), b"EVIL\n").unwrap();
        let r = update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS);
        assert!(r.is_err(), "tampered tree must be rejected");
        // Nothing published, no leftover staging.
        let vdir = base.join("council").join("versions");
        let residue: Vec<_> = std::fs::read_dir(&vdir).map(|rd| rd.flatten().collect()).unwrap_or_default();
        assert!(residue.is_empty(), "no staging/version residue after rejection: {residue:?}");
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src).ok();
    }

    // ── name-hint mismatch is rejected ───────────────────────────────────────────
    #[cfg(unix)]
    #[test]
    fn update_rejects_name_mismatch_with_hint() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src, _, pk) = signed_source("council", "1.0.0", b"x\n");
        let r = update_skill(&conn, &furx, &base, "OTHER", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS);
        assert!(r.is_err(), "name hint mismatch must reject");
        assert!(r.unwrap_err().to_string().contains("does not match"));
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src).ok();
    }

    // ── ⟨audit codex HIGH⟩ a pre-existing version dir whose bytes were tampered must NOT
    //    be blindly re-pointed (re-hash + reject on mismatch) ──────────────────────
    #[cfg(unix)]
    #[test]
    fn reinstall_over_tampered_existing_dir_is_rejected() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src, th, pk) = signed_source("council", "1.0.0", b"genuine\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        // Tamper the published (read-only) version dir in place so it no longer hashes to <th>.
        let vdir = base.join("council").join("versions").join(&th);
        let _ = relax_writable_pub(&vdir);
        std::fs::write(vdir.join("scripts").join("run.sh"), b"EVIL\n").unwrap();
        // Re-installing the same source (same computed_hash <th>) finds dest existing →
        // re-hashes it → mismatch → reject (does not re-point to tampered bytes).
        let r = update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS);
        assert!(r.is_err(), "re-point over tampered existing dir must be rejected");
        assert!(r.unwrap_err().to_string().contains("mismatch"));
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src).ok();
    }

    // ── ⟨audit codex HIGH⟩ rollback to a Rejected version is refused ──────────────
    #[cfg(unix)]
    #[test]
    fn rollback_to_rejected_version_is_refused() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src1, th1, pk) = signed_source("council", "1.0.0", b"v1\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src1.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        let (src2, _th2, _) = signed_source("council", "2.0.0", b"v2\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src2.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        // Force v1's recorded trust_level to 'rejected' (simulate a degraded/revoked row).
        conn.execute("UPDATE skill_versions SET trust_level='rejected' WHERE name='council' AND tree_hash=?",
            params![th1]).unwrap();
        let r = rollback_skill(&conn, &furx, &base, "council", &th1);
        assert!(r.is_err(), "rollback to a Rejected version must be refused");
        assert!(r.unwrap_err().to_string().contains("Rejected"));
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src1).ok();
        std::fs::remove_dir_all(&src2).ok();
    }

    // ── ⟨audit codex MED⟩ hostile name / hash path components are rejected ────────
    #[test]
    fn hostile_path_components_rejected() {
        assert!(!is_safe_name("../escape"));
        assert!(!is_safe_name("a/b"));
        assert!(!is_safe_name(""));
        assert!(is_safe_name("council"));
        assert!(is_safe_name("a_b-1"));
        assert!(!is_safe_hash("../../etc"));
        assert!(!is_safe_hash("DEADBEEF")); // uppercase rejected (we emit lowercase hex)
        assert!(!is_safe_hash(&"a".repeat(63)));
        assert!(is_safe_hash(&"a".repeat(64)));
        assert!(is_safe_hash("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
    }

    // ── ⟨audit codex LOW⟩ current_tree_hash rejects a non-`versions/<hash>` symlink ─
    #[cfg(unix)]
    #[test]
    fn current_tree_hash_rejects_foreign_symlink() {
        use std::os::unix::fs::symlink;
        let base = tmp("base");
        std::fs::create_dir_all(base.join("council")).unwrap();
        // A symlink pointing somewhere that isn't versions/<hash>.
        symlink("/etc/passwd", base.join("council").join("current")).unwrap();
        assert_eq!(current_tree_hash(&base, "council"), None, "foreign target → None");
        // A versions/<non-hash> target is also rejected.
        std::fs::remove_file(base.join("council").join("current")).unwrap();
        symlink("versions/not-a-hash", base.join("council").join("current")).unwrap();
        assert_eq!(current_tree_hash(&base, "council"), None);
        std::fs::remove_dir_all(&base).ok();
    }

    // ── re-installing the SAME content is an idempotent re-point (no dup dir) ─────
    #[cfg(unix)]
    #[test]
    fn reinstall_same_content_is_idempotent() {
        let conn = test_conn();
        let furx = tmp("furx");
        let base = furx.join("plugins");
        std::fs::create_dir_all(&base).unwrap();
        let (src, th, pk) = signed_source("council", "1.0.0", b"same\n");
        update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        // Same source again → same tree_hash → re-point, not a new dir.
        let out = update_skill(&conn, &furx, &base, "council", ImportSource::Local(src.clone()),
            std::slice::from_ref(&pk), &HashSet::new(), DEFAULT_KEEP_VERSIONS).unwrap();
        assert_eq!(out.tree_hash, th);
        assert_eq!(list_versions(&conn, "council").unwrap().len(), 1, "no duplicate version row");
        assert_eq!(current_tree_hash(&base, "council").as_deref(), Some(th.as_str()));
        let _ = relax_writable_pub(&base);
        std::fs::remove_dir_all(&furx).ok();
        std::fs::remove_dir_all(&src).ok();
    }
}
