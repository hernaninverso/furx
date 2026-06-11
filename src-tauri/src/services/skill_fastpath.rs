// spec-kit 046 · Ola 7 (Skills P1) F2 — fast-path cache de re-verificación.
//
// La Ola 4 RE-HASHEA SIEMPRE las scripts vivas vs el `tree_hash` registrado antes de cada
// spawn (`skill_registry::reverify_or_inert`). Es correcto y barato (<10ms para scripts
// chicos), pero re-leer y SHA-256-ear todo el árbol en cada ejecución es desperdicio cuando
// nada cambió. Esta ola agrega el fast-path que el consejo describió:
//
//   snapshot POR-ARCHIVO = Vec<(rel_path_nfc, inode, mtime_ns, size)>  (orden lexicográfico
//   por rel_path NFC) → JSON canónico → SHA-256 = `snapshot_hash`.
//
// Guardamos `snapshot_hash` en `plugins.scripts_cache_snapshot`. En el re-verify:
//   1. Recomputar el snapshot del árbol vivo (solo `stat` por archivo — sin leer contenido).
//   2. Si el `snapshot_hash` recomputado == el guardado → FAST-PATH: el árbol no cambió →
//      saltar el rehash de contenido. Devolver OK sin re-SHA-256-ear los bytes.
//   3. Si difiere (mtime/size/inode/rel_path cambió, o no hay snapshot guardado, o un
//      archivo nuevo/borrado) → SLOW-PATH: rehash completo del contenido (fail-safe) y, si
//      el contenido sigue concordando con el `tree_hash` firmado, refrescar el snapshot.
//
// POR QUÉ POR-ARCHIVO Y NO `stat` DEL DIRECTORIO (council ⟨v7⟩): el mtime de un directorio
// NO captura un reemplazo atómico de un archivo interno vía `rename(2)` en todos los FS →
// un `stat` del dir es FRÁGIL (un swap de archivo no movería el mtime del dir). El snapshot
// por-archivo (inode+mtime+size de CADA archivo) sí lo detecta. NO leemos contenido en el
// snapshot — eso es justamente lo que el fast-path evita; el contenido solo se re-SHA en el
// slow-path.
//
// GARANTÍA FAIL-SAFE (NON-NEGOTIABLE): ante CUALQUIER duda → slow-path (rehash). Un
// snapshot ausente, un error de `stat`, un symlink/hardlink, un archivo que apareció/
// desapareció → todo cae al slow-path. El fast-path NUNCA puede hacer ejecutar un árbol que
// el slow-path rechazaría: solo se toma cuando el snapshot por-archivo es BIT-IDÉNTICO, y
// aun así el slow-path (que SÍ lee contenido) es el árbitro final de la confianza. El
// snapshot es un acelerador, no una autoridad.
//
// NFC ya aplicado por la Ola 4 en `tree_hash`; reusamos la MISMA normalización para los
// rel_path del snapshot (consistencia con la identidad firmada del árbol).
//
// THREAT-MODEL BOUNDARIES (documentadas, alineadas con la Ola 4 — el fast-path es un
// ACELERADOR opt-in, NO una autoridad de confianza):
//   - SAME-UID mtime-forgery: un atacante con el MISMO UID puede sobrescribir un archivo
//     in-place con contenido del MISMO tamaño y RESTAURAR su mtime → el snapshot
//     (rel_path,inode,mtime,size) no cambia → el fast-path no lo detecta. Esta es la MISMA
//     frontera que la Ola 4 ya documenta (un same-UID que ya tiene los privilegios del
//     usuario y puede chmod+swap el dir read-only): el único cierre real es inmutabilidad
//     del SO (`UF_IMMUTABLE`). El fast-path NO empeora la postura: un caller que requiera
//     la garantía estricta llama directo a `reverify_or_inert` (rehash siempre); el
//     fast-path es para el caso común donde nada cambió. La PRIMERA verificación de cada
//     skill SIEMPRE slow-pathea (no hay snapshot aún) y siembra el snapshot recién tras un
//     rehash que pasó.
//   - El snapshot se computa en una traversal SEPARADA de la que `reverify_or_inert`
//     hasheó (ventana sub-ms entre ambas). Bajo el flock + app de un solo proceso no hay un
//     segundo escritor; el residual es la MISMA ventana same-UID de arriba. No se siembra
//     snapshot si `compute_snapshot_hash` no puede computarlo limpio (→ se limpia, próxima
//     llamada slow-pathea). NON-UNIX: fast-path DESACTIVADO (siempre slow-path).
//
// Dead-code-first: probado en aislamiento; el wiring al hot-path del spawn es aparte (un
// `reverify_or_inert_fast` que consulta el snapshot antes de delegar al rehash de la Ola 4).

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

use super::skill_registry::{reverify_or_inert, with_durable_immediate};

const MAX_SNAPSHOT_FILES: usize = 4096;

/// One per-file snapshot entry. `inode`/`mtime_ns`/`size` are the cheap `stat` fields that
/// change on ANY content replacement (a `rename(2)` swap gives a new inode; an in-place
/// write bumps mtime+size). `rel_path` is NFC-normalized + forward-slashed (same as the
/// tree_hash identity), so the snapshot is OS-separator independent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
struct SnapEntry {
    rel_path: String,
    inode: u64,
    mtime_ns: i128,
    size: u64,
}

/// Compute the per-file snapshot HASH of `scripts_dir`. `None` if the snapshot cannot be
/// computed cleanly (missing dir, symlink/hardlink, too many files, stat error, NFC
/// collision) — the caller MUST treat `None` as "no fast-path → rehash" (fail-safe).
///
/// The hash is SHA-256 over the JSON-canonical serialization of the sorted entry vec.
///
/// ⟨audit codex/deepseek HIGH⟩ NON-UNIX: the fast-path is DISABLED entirely (returns
/// `None`) because file identity (inode) + nanosecond mtime are not reliably available, so
/// a same-size content swap could evade a metadata-only snapshot. Off-unix we always
/// slow-path (full content rehash) — strictly fail-safe. The project targets macOS.
pub fn compute_snapshot_hash(scripts_dir: &Path) -> Option<String> {
    #[cfg(not(unix))]
    {
        let _ = scripts_dir;
        return None;
    }
    #[cfg(unix)]
    {
        compute_snapshot_hash_unix(scripts_dir)
    }
}

#[cfg(unix)]
fn compute_snapshot_hash_unix(scripts_dir: &Path) -> Option<String> {
    let entries = collect_snapshot(scripts_dir).ok()?;
    // Canonical JSON (sorted keys via the JCS helper) so the hash is deterministic across
    // serde versions / map orderings.
    let v = serde_json::to_value(&entries).ok()?;
    let canon = super::skill_manifest::canonical_json_for(&v);
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(canon.as_bytes());
    Some(hex::encode(h.finalize()))
}

/// Walk `scripts_dir` collecting per-file stat snapshots. Rejects (Err) the same hostile
/// shapes the tree_hash rejects (symlinks, hardlinked regular files, non-regular files) so
/// the snapshot can never paper over something the slow-path would refuse.
///
/// ⟨audit codex/deepseek HIGH⟩ The ROOT is checked with `symlink_metadata` (NOT `is_dir()`,
/// which follows a root symlink + turns "missing"/"not a dir" into a silent empty tree).
/// A missing root, a symlinked root, or a non-directory root → Err → caller slow-paths
/// (NEVER a fast-path on an empty/bogus snapshot). An empty *real* directory → empty vec
/// (a legitimate empty tree, same as tree_hash's sha256("")).
#[cfg(unix)]
fn collect_snapshot(scripts_dir: &Path) -> Result<Vec<SnapEntry>> {
    match std::fs::symlink_metadata(scripts_dir) {
        Ok(md) if md.file_type().is_symlink() => {
            return Err(anyhow!("snapshot root is a symlink — refusing"));
        }
        Ok(md) if !md.is_dir() => {
            return Err(anyhow!("snapshot root is not a directory"));
        }
        Ok(_) => {}
        // Missing root is NOT a clean empty snapshot — force slow-path (which itself handles
        // a missing dir as sha256("") under the recorded hash, the authoritative check).
        Err(e) => return Err(anyhow!("snapshot root stat failed: {e}")),
    }
    let mut out = Vec::new();
    walk(scripts_dir, scripts_dir, &mut out)?;
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    // NFC-collision guard (same as tree_hash): adjacent equal rel_paths = ambiguous.
    for w in out.windows(2) {
        if w[0].rel_path == w[1].rel_path {
            return Err(anyhow!("snapshot NFC rel_path collision: {}", w[0].rel_path));
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn walk(root: &Path, dir: &Path, out: &mut Vec<SnapEntry>) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    use unicode_normalization::UnicodeNormalization;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let md = std::fs::symlink_metadata(&path)?;
        let ft = md.file_type();
        if ft.is_symlink() {
            return Err(anyhow!("snapshot refuses a symlink: {}", path.display()));
        }
        if ft.is_dir() {
            walk(root, &path, out)?;
            continue;
        }
        if !ft.is_file() {
            return Err(anyhow!("snapshot refuses a non-regular file: {}", path.display()));
        }
        if md.nlink() > 1 {
            return Err(anyhow!("snapshot refuses a hard-linked file: {}", path.display()));
        }
        // ⟨audit codex MED⟩ Enforce the file cap DURING insertion (not after the full walk)
        // so a hostile huge/deep tree is bounded early.
        if out.len() >= MAX_SNAPSHOT_FILES {
            return Err(anyhow!("snapshot exceeds {MAX_SNAPSHOT_FILES} files"));
        }
        // mtime in nanoseconds (i128 to hold negative pre-epoch + ns without overflow).
        let mtime_ns = (md.mtime() as i128) * 1_000_000_000 + (md.mtime_nsec() as i128);
        let rel = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("path not under root"))?;
        let rel_str: String = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().nfc().collect::<String>())
            .collect::<Vec<_>>()
            .join("/");
        out.push(SnapEntry {
            rel_path: rel_str,
            inode: md.ino(),
            mtime_ns,
            size: md.len(),
        });
    }
    Ok(())
}

/// Read the stored snapshot hash for `name` (`plugins.scripts_cache_snapshot`).
pub fn stored_snapshot(conn: &Connection, name: &str) -> Result<Option<String>> {
    let v: Option<Option<String>> = conn
        .query_row(
            "SELECT scripts_cache_snapshot FROM plugins WHERE name=?",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.flatten())
}

/// Persist a fresh snapshot hash for `name` (durable). Called after a slow-path re-verify
/// confirms the live content still matches the signed tree_hash.
pub fn store_snapshot(conn: &Connection, name: &str, snapshot_hash: &str) -> Result<()> {
    with_durable_immediate(conn, |c| {
        c.execute(
            "UPDATE plugins SET scripts_cache_snapshot=? WHERE name=?",
            params![snapshot_hash, name],
        )?;
        Ok(())
    })
}

/// Clear the cached snapshot for `name` (e.g. on any state change that invalidates it).
pub fn clear_snapshot(conn: &Connection, name: &str) -> Result<()> {
    with_durable_immediate(conn, |c| {
        c.execute(
            "UPDATE plugins SET scripts_cache_snapshot=NULL WHERE name=?",
            params![name],
        )?;
        Ok(())
    })
}

/// Outcome of a fast-path re-verify: did we take the fast-path, and is the skill OK to run?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastVerify {
    /// `true` = the per-file snapshot matched the stored one → content rehash was skipped.
    pub fast_path_taken: bool,
    /// `true` = the skill may execute (matches `reverify_or_inert`'s contract).
    pub ok: bool,
}

/// FR-002 — fast-path re-verification. The hot-path entry the spawn flow can call instead
/// of `reverify_or_inert`:
///
///   1. Compute the live per-file snapshot hash. If it equals the stored snapshot AND there
///      is a stored snapshot → FAST-PATH: trust that the content is unchanged (the snapshot
///      captures any inode/mtime/size delta) and return ok=true WITHOUT re-hashing content.
///   2. Otherwise SLOW-PATH: delegate to the Ola 4 `reverify_or_inert` (full content rehash
///      vs the signed tree_hash). If it returns ok, refresh the stored snapshot so the next
///      call can fast-path. If it fails, CLEAR the snapshot (the skill is now inert).
///
/// CRITICAL fail-safe invariant: the fast-path is ONLY taken when the live snapshot is
/// bit-identical to a stored one that was itself written right after a passing slow-path.
/// A missing snapshot, an un-stat-able tree, or ANY delta forces the slow-path. The
/// fast-path can never grant execution that the slow-path would deny — it only skips the
/// re-read when nothing observably changed.
///
/// NOTE: this does NOT bypass the trust LEVEL check — it first confirms (via the slow-path
/// on the first call, then via the snapshot) that the recorded tree_hash holds. A skill
/// that is inert/sandboxed/rejected still returns ok=false (the slow-path enforces that and
/// no snapshot is ever stored for a non-executable skill).
pub fn reverify_fast(
    conn: &Connection,
    name: &str,
    scripts_dir: &Path,
) -> Result<FastVerify> {
    // The skill must currently be at an executable trust level for the fast-path to even be
    // considered — otherwise we must NOT short-circuit to ok=true. We reuse the slow-path's
    // own level gate by only fast-pathing when there IS a stored snapshot (which is only
    // ever written for a skill that passed a slow-path at an executable level), AND we
    // additionally re-check the level here to be safe against a level downgrade since the
    // snapshot was written.
    if super::skill_registry::is_executable_level(conn, name)? {
        if let Some(stored) = stored_snapshot(conn, name)? {
            if let Some(live) = compute_snapshot_hash(scripts_dir) {
                if live == stored {
                    // ⟨audit mistral⟩ Re-check the executable level AFTER the snapshot
                    // comparison: if the skill was downgraded/inerted between the first gate
                    // and here (another op under the same flock, or a fresh read), do NOT
                    // grant the fast-path — fall through to the slow-path which re-enforces
                    // the level. Cheap (one indexed read) and closes the level-flip window.
                    if super::skill_registry::is_executable_level(conn, name)? {
                        return Ok(FastVerify { fast_path_taken: true, ok: true });
                    }
                }
            }
        }
    }
    // Slow-path: the Ola 4 full content rehash vs the signed tree_hash + level gate.
    let ok = reverify_or_inert(conn, name, scripts_dir)?;
    if ok {
        // Refresh the snapshot from the (now-verified) live tree so the next call can be
        // fast. A snapshot that can't be computed cleanly is simply cleared (next call
        // slow-paths again — fail-safe, never a stale fast-path).
        match compute_snapshot_hash(scripts_dir) {
            Some(h) => store_snapshot(conn, name, &h)?,
            None => clear_snapshot(conn, name)?,
        }
    } else {
        // The skill is now inert/rejected → drop any stale snapshot so it can never grant a
        // future fast-path.
        clear_snapshot(conn, name)?;
    }
    Ok(FastVerify { fast_path_taken: false, ok })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::skill_manifest::{tree_hash, TrustLevel};
    use super::super::skill_registry::{begin_install, finalize_install};
    use rusqlite::Connection;
    use std::path::{Path, PathBuf};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(include_str!("../../migrations/010_b5.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/049_skill_trust.sql")).unwrap();
        conn.execute_batch(include_str!("../../migrations/052_skill_versions.sql")).unwrap();
        conn
    }

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("furx-fp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_scripts(dir: &Path, body: &[u8]) -> String {
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("run.sh"), body).unwrap();
        tree_hash(&s).unwrap()
    }

    /// Install a Verified skill row at `name` with the given tree_hash so the slow-path
    /// (reverify_or_inert) treats it as executable.
    fn install_verified(conn: &Connection, name: &str, th: &str) {
        begin_install(conn, name, ".t", th).unwrap();
        finalize_install(conn, name, TrustLevel::Verified).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_hash_is_stable_and_changes_with_content() {
        let dir = tmp();
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("a.sh"), b"hello").unwrap();
        let h1 = compute_snapshot_hash(&s).unwrap();
        let h1b = compute_snapshot_hash(&s).unwrap();
        assert_eq!(h1, h1b, "stable across calls when nothing changed");
        // Rewrite with different size → size delta → different snapshot.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(s.join("a.sh"), b"hello-world-bigger").unwrap();
        let h2 = compute_snapshot_hash(&s).unwrap();
        assert_ne!(h1, h2, "content/size change → snapshot changes");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_detects_new_and_removed_files() {
        let dir = tmp();
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("a.sh"), b"x").unwrap();
        let h1 = compute_snapshot_hash(&s).unwrap();
        std::fs::write(s.join("b.sh"), b"y").unwrap();
        let h2 = compute_snapshot_hash(&s).unwrap();
        assert_ne!(h1, h2, "added file → snapshot changes");
        std::fs::remove_file(s.join("b.sh")).unwrap();
        let h3 = compute_snapshot_hash(&s).unwrap();
        assert_eq!(h1, h3, "removing the added file returns to the original snapshot");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_rejects_symlink_and_hardlink() {
        use std::os::unix::fs::symlink;
        let dir = tmp();
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        std::fs::write(s.join("real"), b"x").unwrap();
        symlink(s.join("real"), s.join("link")).unwrap();
        assert!(compute_snapshot_hash(&s).is_none(), "symlink → None (fail-safe)");
        std::fs::remove_file(s.join("link")).unwrap();
        std::fs::hard_link(s.join("real"), s.join("hard")).unwrap();
        assert!(compute_snapshot_hash(&s).is_none(), "hardlink → None (fail-safe)");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── ⟨audit codex/deepseek HIGH⟩ a symlinked / missing / non-dir root → None ──
    #[cfg(unix)]
    #[test]
    fn snapshot_root_must_be_a_real_directory() {
        use std::os::unix::fs::symlink;
        // Missing root → None (fail-safe, NOT an empty snapshot).
        let missing = tmp().join("nope").join("scripts");
        assert!(compute_snapshot_hash(&missing).is_none(), "missing root → None");
        // Symlinked root → None.
        let dir = tmp();
        let real = dir.join("real_scripts");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("a.sh"), b"x").unwrap();
        let link = dir.join("scripts");
        symlink(&real, &link).unwrap();
        assert!(compute_snapshot_hash(&link).is_none(), "symlinked root → None");
        // A file (non-dir) as root → None.
        let f = dir.join("afile");
        std::fs::write(&f, b"x").unwrap();
        assert!(compute_snapshot_hash(&f).is_none(), "non-dir root → None");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── empty REAL directory still produces a (stable) snapshot ──────────────────
    #[cfg(unix)]
    #[test]
    fn empty_real_dir_has_a_snapshot() {
        let dir = tmp();
        let s = dir.join("scripts");
        std::fs::create_dir_all(&s).unwrap();
        assert!(compute_snapshot_hash(&s).is_some(), "empty real dir → Some (empty tree)");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── SC-002: re-verify with NO change uses the snapshot (fast-path) ────────────
    #[cfg(unix)]
    #[test]
    fn fast_path_taken_when_unchanged() {
        let conn = test_conn();
        let dir = tmp();
        let th = write_scripts(&dir, b"#!/bin/sh\necho hi\n");
        let scripts = dir.join("scripts");
        install_verified(&conn, "council", &th);
        // First call: no stored snapshot → slow-path, stores snapshot.
        let v1 = reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(!v1.fast_path_taken, "first call slow-paths");
        assert!(v1.ok);
        assert!(stored_snapshot(&conn, "council").unwrap().is_some(), "snapshot stored");
        // Second call: nothing changed → fast-path.
        let v2 = reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(v2.fast_path_taken, "unchanged tree → fast-path");
        assert!(v2.ok);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── SC-002: touching a script invalidates → slow-path rehash ─────────────────
    #[cfg(unix)]
    #[test]
    fn touching_a_script_invalidates_fast_path() {
        let conn = test_conn();
        let dir = tmp();
        let th = write_scripts(&dir, b"orig\n");
        let scripts = dir.join("scripts");
        install_verified(&conn, "council", &th);
        // Prime the snapshot.
        reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(reverify_fast(&conn, "council", &scripts).unwrap().fast_path_taken);
        // Swap the script content (same size to prove mtime/inode catches it too, then a
        // real content change to make the SLOW path actually fail).
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(scripts.join("run.sh"), b"EVIL\n").unwrap();
        // Snapshot now differs → slow-path; content no longer matches tree_hash → NOT ok.
        let v = reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(!v.fast_path_taken, "changed tree → slow-path (no fast-path)");
        assert!(!v.ok, "tampered content fails the slow-path rehash");
        // Snapshot cleared because the skill is now inert.
        assert!(stored_snapshot(&conn, "council").unwrap().is_none(), "snapshot cleared on failure");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── same-size in-place rewrite is still caught (mtime/inode delta) ────────────
    #[cfg(unix)]
    #[test]
    fn same_size_rewrite_breaks_fast_path() {
        let conn = test_conn();
        let dir = tmp();
        let th = write_scripts(&dir, b"AAAA\n");
        let scripts = dir.join("scripts");
        install_verified(&conn, "council", &th);
        reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(reverify_fast(&conn, "council", &scripts).unwrap().fast_path_taken);
        // Rewrite same SIZE → dir mtime would NOT necessarily change, but the FILE mtime
        // (and possibly inode) does → per-file snapshot catches it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(scripts.join("run.sh"), b"BBBB\n").unwrap();
        let v = reverify_fast(&conn, "council", &scripts).unwrap();
        assert!(!v.fast_path_taken, "same-size in-place rewrite must NOT fast-path");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── a non-executable skill never fast-paths to ok=true ───────────────────────
    #[cfg(unix)]
    #[test]
    fn sandboxed_skill_never_fast_paths_ok() {
        let conn = test_conn();
        let dir = tmp();
        let th = write_scripts(&dir, b"x\n");
        let scripts = dir.join("scripts");
        begin_install(&conn, "wild", ".t", &th).unwrap();
        finalize_install(&conn, "wild", TrustLevel::Sandboxed).unwrap();
        // Even if we manually plant a snapshot, the level gate must keep it from fast-ok.
        let h = compute_snapshot_hash(&scripts).unwrap();
        store_snapshot(&conn, "wild", &h).unwrap();
        let v = reverify_fast(&conn, "wild", &scripts).unwrap();
        assert!(!v.fast_path_taken, "sandboxed must not fast-path");
        assert!(!v.ok, "sandboxed scripts never execute");
        std::fs::remove_dir_all(&dir).ok();
    }
}
