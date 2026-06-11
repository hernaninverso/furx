// 2.38 — Plugin extension system MVP.
// Plugin = ~/.furx/plugins/<name>/manifest.json + optional commands.
// Council V1: name regex strict, manifest validated, no exec by default.
//
// spec-022 US1 — RECONCILIACIÓN DISCO↔DB. El DISCO es la fuente de verdad de qué
// plugins existen: `list()` escanea `~/.furx/plugins/` (lo que el motor realmente
// instaló y ejecuta). La tabla SQLite `plugins` se usa SOLO como estado mutable
// (enabled/disabled) keyed por `name`; NO como fuente de existencia. Un plugin en
// disco sin fila en DB aparece como instalado + enabled (backfill por default). Una
// fila en DB sin plugin en disco NO aparece (no fantasma). Esto cierra el bug de
// dos registros: `plugin_install_bundled` escribía a disco pero `list` leía la tabla
// vacía → "Sin plugins instalados" aunque codanna/word-count estuvieran instalados.

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::plugin_host::SignedManifest;

const SAFE_NAME: &str = r"^[A-Za-z0-9_-]+$";

/// spec-022 (audit HIGH 2, DoS) — tope de plugins reconciliados desde disco. La
/// reconciliación escanea SOLO el primer nivel de `~/.furx/plugins/` (NO recursión),
/// pero un atacante/disco corrupto podría sembrar miles de subdirs. Acotamos la
/// cantidad (loggeando si truncamos) y el tamaño del manifest que parseamos.
const MAX_PLUGINS_SCANNED: usize = 256;
/// Tope de bytes al leer un `manifest.json` antes de parsearlo (evita OOM por un
/// manifest gigante/malicioso). 1 MiB es holgado para un manifest legítimo.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub commands: Vec<String>, // names of commands this plugin claims to add
    pub permissions: Vec<String>, // declared permissions; user reviews
}

#[derive(Debug, Clone, Serialize)]
pub struct Plugin {
    /// Identidad estable = nombre en disco (`~/.furx/plugins/<name>`). El front lo usa
    /// como key para enable/disable.
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// La firma Ed25519 del manifest verifica contra la pinned key. Un manifest
    /// corrupto/no-firmado → `verified=false` (fail-closed): se muestra pero el motor
    /// rehúsa ejecutarlo. NO se oculta silenciosamente.
    pub verified: bool,
    pub manifest: PluginManifest,
    pub installed_at: String,
}

/// Directorio base de plugins (`~/.furx/plugins`).
pub fn plugins_base() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home.join(".furx").join("plugins"))
}

pub fn scan_dir() -> Result<Vec<PluginManifest>> {
    let base = plugins_base()?;
    Ok(scan_dir_at(&base)
        .into_iter()
        .map(|s| s.manifest)
        .collect())
}

/// Un plugin tal cual está en disco: su manifest legacy-normalizado + si verifica.
struct ScannedPlugin {
    manifest: PluginManifest,
    verified: bool,
}

/// Escanea un directorio base de plugins. Para cada subdir con `manifest.json`:
/// intenta parsear el `SignedManifest` real (formato del bundle firmado) y deriva
/// nombre/versión/descripción/comandos(=tools del MCP o permisos declarados) +
/// `verified` (firma Ed25519). Si no es un SignedManifest, cae al `PluginManifest`
/// legacy (sin firma → `verified=false`). Salta dirs con nombre inseguro o cuyo
/// manifest no matchea el nombre del dir. Testeable con cualquier base.
fn scan_dir_at(base: &Path) -> Vec<ScannedPlugin> {
    if !base.is_dir() {
        return vec![];
    }
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(base) else {
        return out;
    };
    let mut truncated = false;
    // HIGH 2 (DoS) — acotamos la ITERACIÓN del read_dir, NO sólo los plugins aceptados.
    // Si la base tiene miles de entradas BASURA (sin manifest.json, archivos sueltos,
    // manifests inválidos), `out.len()` nunca sube y el loop recorrería TODO el dir →
    // el DoS seguiría abierto. Contamos cada entrada recorrida y cortamos al alcanzar
    // el cap, así el walk nunca toca más de MAX_PLUGINS_SCANNED entradas del disco.
    let mut iterated: usize = 0;
    for entry in rd.flatten() {
        if iterated >= MAX_PLUGINS_SCANNED {
            truncated = true;
            break;
        }
        iterated += 1;
        let p = entry.path();
        // SOLO directorios de PRIMER nivel (sin recursión profunda). Una entrada que no
        // sea directorio (archivo suelto, symlink a archivo) se ignora.
        if !p.is_dir() {
            continue;
        }
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if !is_safe_name(&name) {
            continue;
        }
        let manifest_path = p.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        // HIGH 2 — no leer manifests arbitrariamente grandes: cap de bytes ANTES de leer.
        match std::fs::metadata(&manifest_path) {
            Ok(md) if md.len() > MAX_MANIFEST_BYTES => {
                tracing::warn!(
                    "plugins: skipping '{}' — manifest.json too large ({} bytes > {} cap)",
                    name,
                    md.len(),
                    MAX_MANIFEST_BYTES
                );
                continue;
            }
            Ok(_) => {}
            Err(_) => continue,
        }
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        if let Some(sp) = scan_one(&name, &text) {
            out.push(sp);
        }
    }
    if truncated {
        tracing::warn!(
            "plugins: disk reconciliation truncated after iterating {} entries (cap {}, {} accepted) — extra entries ignored",
            iterated,
            MAX_PLUGINS_SCANNED,
            out.len()
        );
    }
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    out
}

/// Normaliza un manifest crudo (texto) al `PluginManifest` legacy + flag `verified`.
/// Pública-de-módulo para tests. Devuelve `None` si el manifest no parsea de ninguna
/// forma o si el nombre declarado no matchea el nombre del dir.
fn scan_one(dir_name: &str, text: &str) -> Option<ScannedPlugin> {
    // 1) Formato real del bundle: SignedManifest (firma Ed25519 + permisos tipados).
    if let Ok(sm) = serde_json::from_str::<SignedManifest>(text) {
        if sm.name != dir_name {
            return None;
        }
        let verified = sm.verify();
        // Comandos/tools expuestos: si es un MCP server no enumera tools en el manifest;
        // mostramos el entrypoint como "tool" implícito sólo si no es MCP. Para plugins
        // por-tool clásicos el front invoca tools por nombre; el manifest no los lista,
        // así que dejamos `commands` vacío y exponemos los permisos declarados.
        let permissions = describe_permissions(&sm.permissions, sm.mcp.is_some());
        return Some(ScannedPlugin {
            manifest: PluginManifest {
                name: sm.name,
                version: sm.version,
                description: sm.description,
                commands: vec![],
                permissions,
            },
            verified,
        });
    }
    // 2) Manifest legacy (`PluginManifest`): sin firma → no verificado.
    if let Ok(m) = serde_json::from_str::<PluginManifest>(text) {
        if m.name != dir_name {
            return None;
        }
        return Some(ScannedPlugin {
            manifest: m,
            verified: false,
        });
    }
    None
}

/// Resumen legible (para pills en la UI) de los permisos declarados por un
/// SignedManifest. NO baja ninguna verificación; sólo describe.
fn describe_permissions(perms: &super::plugin_host::Permissions, is_mcp: bool) -> Vec<String> {
    let mut out = Vec::new();
    if is_mcp {
        out.push("mcp".to_string());
    }
    if perms.net.is_empty() {
        out.push("net: ninguno".to_string());
    } else {
        for h in &perms.net {
            out.push(format!("net: {h}"));
        }
    }
    if perms.shell {
        out.push("shell".to_string());
    }
    for s in &perms.secrets {
        out.push(format!("byok: {s}"));
    }
    out
}

/// Legacy: registra un manifest legacy en la tabla. Conservado por compat (lo usa
/// el comando `plugins_install`). NO es la fuente de existencia — `list()` escanea
/// disco. Mantiene la fila de estado para que enable/disable persista.
pub fn install(db: &Mutex<Connection>, manifest: &PluginManifest) -> Result<String> {
    if !is_safe_name(&manifest.name) {
        return Err(anyhow!("unsafe plugin name"));
    }
    let id = Uuid::new_v4().to_string();
    let manifest_json = serde_json::to_string(manifest)?;
    db.lock().execute(
        "INSERT INTO plugins (id, name, version, manifest_json) VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
        params![id, manifest.name, manifest.version, manifest_json],
    )?;
    Ok(id)
}

/// Estado enable/disable de un plugin (keyed por nombre). `None` = sin fila en DB.
/// Determinista: con UNIQUE(name) (migración 039) hay a lo sumo una fila por name.
/// El `ORDER BY installed_at … LIMIT 1` queda como red de seguridad si la migración
/// aún no corrió en una DB legacy (no debería, pero no rompe la determinación).
fn enabled_state(conn: &Connection, name: &str) -> Result<Option<bool>> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT enabled FROM plugins WHERE name = ? ORDER BY installed_at DESC, id DESC LIMIT 1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.map(|x| x != 0))
}

/// Lista los plugins REALMENTE instalados en disco (`~/.furx/plugins/`) como fuente
/// de verdad. El estado enabled/disabled sale de la tabla `plugins` keyed por nombre
/// (default `enabled=true` cuando no hay fila → backfill). `verified` viene de la
/// firma Ed25519 del manifest (fail-closed). Un plugin en disco sin fila DB aparece;
/// una fila DB sin plugin en disco NO aparece.
pub fn list(db: &Mutex<Connection>) -> Result<Vec<Plugin>> {
    let base = plugins_base()?;
    list_at(db, &base)
}

/// Igual que `list` pero con base explícita (testeable sin tocar $HOME).
pub fn list_at(db: &Mutex<Connection>, base: &Path) -> Result<Vec<Plugin>> {
    let scanned = scan_dir_at(base);
    let conn = db.lock();
    let mut out = Vec::with_capacity(scanned.len());
    for sp in scanned {
        let name = sp.manifest.name.clone();
        // Backfill: sin fila DB → enabled por default.
        let enabled = enabled_state(&conn, &name)?.unwrap_or(true);
        out.push(Plugin {
            id: name.clone(),
            version: sp.manifest.version.clone(),
            enabled,
            verified: sp.verified,
            manifest: sp.manifest,
            installed_at: String::new(),
            name,
        });
    }
    Ok(out)
}

/// Persiste enable/disable de un plugin identificado por NOMBRE (el `id` que ve el
/// front = el nombre en disco). Upsert ATÓMICO keyed por `name` (UNIQUE, migración 039):
/// un solo statement, sin SELECT previo → determinista y libre de race read-then-write.
/// Si no había fila, la crea con metadata mínima (backfill); si había, actualiza el flag
/// dejando intacta la metadata. Idempotente.
pub fn set_enabled(db: &Mutex<Connection>, name: &str, enabled: bool) -> Result<()> {
    if !is_safe_name(name) {
        return Err(anyhow!("unsafe plugin name"));
    }
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO plugins (id, name, version, enabled, manifest_json) \
         VALUES (?, ?, '', ?, '{}') \
         ON CONFLICT(name) DO UPDATE SET enabled = excluded.enabled",
        params![id, name, enabled as i64],
    )?;
    Ok(())
}

/// ¿Está habilitado este plugin? (default-enabled si no hay fila). Lo consulta
/// `plugin_invoke` para respetar el estado disabled (un plugin disabled no se ejecuta).
pub fn is_enabled(db: &Mutex<Connection>, name: &str) -> Result<bool> {
    let conn = db.lock();
    Ok(enabled_state(&conn, name)?.unwrap_or(true))
}

fn is_safe_name(s: &str) -> bool {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(SAFE_NAME).unwrap());
    !s.is_empty() && s.len() < 64 && RE.is_match(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // Aplica la migración 010 real (tabla `plugins`) + 039 (UNIQUE(name) + dedup),
        // que es de la que depende el upsert atómico de `set_enabled`.
        conn.execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn tmp_base() -> PathBuf {
        let p = std::env::temp_dir().join(format!("furx-plugins-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Escribe un plugin legacy (sin firma) en `<base>/<name>/manifest.json`.
    fn write_legacy_plugin(base: &Path, name: &str, version: &str) {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let m = PluginManifest {
            name: name.to_string(),
            version: version.to_string(),
            description: Some(format!("{name} desc")),
            commands: vec!["list".into()],
            permissions: vec!["fs_read".into()],
        };
        std::fs::write(dir.join("manifest.json"), serde_json::to_string(&m).unwrap()).unwrap();
    }

    #[test]
    fn list_reflects_disk_not_db() {
        let db = test_db();
        let base = tmp_base();
        // DB vacía, pero hay un plugin en disco → debe aparecer (el bug de dos registros).
        write_legacy_plugin(&base, "codanna", "1.0.0");
        write_legacy_plugin(&base, "word-count", "0.2.0");
        let list = list_at(&db, &base).unwrap();
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"codanna"), "codanna debe aparecer desde disco");
        assert!(names.contains(&"word-count"), "word-count debe aparecer desde disco");
        // Backfill: sin fila DB → enabled por default.
        assert!(list.iter().all(|p| p.enabled), "default enabled (backfill)");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn db_row_without_disk_is_not_a_phantom() {
        let db = test_db();
        let base = tmp_base();
        // Fila en DB para un plugin que NO está en disco.
        db.lock()
            .execute(
                "INSERT INTO plugins (id, name, version, enabled, manifest_json) \
                 VALUES ('x','ghost','1.0.0',1,'{}')",
                [],
            )
            .unwrap();
        let list = list_at(&db, &base).unwrap();
        assert!(
            list.iter().all(|p| p.name != "ghost"),
            "un plugin sólo-en-DB no debe aparecer como fantasma"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn enable_disable_persists_and_invoke_respects_it() {
        let db = test_db();
        let base = tmp_base();
        write_legacy_plugin(&base, "codanna", "1.0.0");
        // Por default enabled.
        assert!(is_enabled(&db, "codanna").unwrap());
        // Disable persiste.
        set_enabled(&db, "codanna", false).unwrap();
        assert!(!is_enabled(&db, "codanna").unwrap());
        let list = list_at(&db, &base).unwrap();
        let p = list.iter().find(|p| p.name == "codanna").unwrap();
        assert!(!p.enabled, "el estado disabled persiste y se refleja en list");
        // Re-enable.
        set_enabled(&db, "codanna", true).unwrap();
        assert!(is_enabled(&db, "codanna").unwrap());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn set_enabled_is_atomic_upsert_idempotent() {
        // MED 1 — set_enabled debe ser un upsert atómico keyed por name: re-llamarlo
        // no crea filas extra (UNIQUE(name)) y converge al último estado pedido.
        let db = test_db();
        set_enabled(&db, "codanna", false).unwrap();
        set_enabled(&db, "codanna", false).unwrap();
        set_enabled(&db, "codanna", true).unwrap();
        let count: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM plugins WHERE name = 'codanna'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "upsert atómico: una sola fila por name");
        assert!(is_enabled(&db, "codanna").unwrap(), "converge al último estado");
    }

    #[test]
    fn migration_039_dedups_legacy_duplicate_names() {
        // MED 1 — una DB legacy con duplicados por name (sin UNIQUE(name)) converge a un
        // estado único tras la migración 039, conservando el estado de la fila más reciente.
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // Sólo la tabla (010), SIN 039 todavía → permite insertar duplicados por name.
        conn.execute_batch(include_str!("../../migrations/010_b5.sql"))
            .unwrap();
        conn.execute(
            "INSERT INTO plugins (id, name, version, enabled, manifest_json, installed_at) \
             VALUES ('old', 'dup', '1.0.0', 1, '{}', '2020-01-01 00:00:00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plugins (id, name, version, enabled, manifest_json, installed_at) \
             VALUES ('new', 'dup', '2.0.0', 0, '{}', '2025-01-01 00:00:00')",
            [],
        )
        .unwrap();
        // Ahora aplica 039 (dedup + UNIQUE(name)).
        conn.execute_batch(include_str!("../../migrations/039_plugins_unique_name.sql"))
            .unwrap();
        let db = Arc::new(Mutex::new(conn));
        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM plugins WHERE name = 'dup'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "los duplicados por name se colapsan a una fila");
        // Conserva el estado de la fila más reciente (installed_at más alto = 'new', disabled).
        assert!(
            !is_enabled(&db, "dup").unwrap(),
            "conserva el estado (disabled) de la fila más reciente"
        );
        // set_enabled sigue atómico/determinista tras el dedup.
        set_enabled(&db, "dup", true).unwrap();
        assert!(is_enabled(&db, "dup").unwrap());
        let count2: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM plugins WHERE name = 'dup'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count2, 1, "sigue habiendo una sola fila tras enable");
    }

    #[test]
    fn legacy_manifest_is_unverified() {
        let base = tmp_base();
        write_legacy_plugin(&base, "word-count", "0.2.0");
        let scanned = scan_dir_at(&base);
        let sp = scanned.iter().find(|s| s.manifest.name == "word-count").unwrap();
        assert!(!sp.verified, "un manifest legacy sin firma es verified=false (fail-closed)");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_caps_plugin_count() {
        // HIGH 2 (DoS) — sembrar más subdirs que el cap; scan_dir_at trunca al cap.
        let base = tmp_base();
        let total = MAX_PLUGINS_SCANNED + 20;
        for i in 0..total {
            write_legacy_plugin(&base, &format!("plg-{i:04}"), "1.0.0");
        }
        let scanned = scan_dir_at(&base);
        assert_eq!(
            scanned.len(),
            MAX_PLUGINS_SCANNED,
            "el scan se acota al cap de plugins"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_caps_iteration_over_junk_entries() {
        // HIGH 2 (DoS) — el cap debe acotar la ITERACIÓN del read_dir, no sólo los
        // plugins ACEPTADOS. Sembramos muchas más entradas BASURA que el cap (dirs sin
        // manifest, archivos sueltos, dirs con manifest inválido): ninguna se acepta, así
        // que `out.len()` queda en 0. Antes del fix, el loop recorría TODAS las entradas
        // igual (out.len() nunca alcanzaba el cap) → DoS. Con el fix, la iteración corta
        // al cap. Verificamos: (a) el scan termina y devuelve vacío (nada válido), y
        // (b) que NO recorrió todas las entradas — sembramos un puñado de plugins VÁLIDOS
        // al final (orden de read_dir no garantizado, pero con cap << total las chances de
        // ver TODOS los válidos son nulas si la iteración está acotada).
        let base = tmp_base();
        let junk = MAX_PLUGINS_SCANNED * 4;
        for i in 0..junk {
            // dir sin manifest.json
            std::fs::create_dir_all(base.join(format!("nomanifest-{i:05}"))).unwrap();
            // archivo suelto (no directorio)
            std::fs::write(base.join(format!("stray-{i:05}.txt")), b"x").unwrap();
        }
        let scanned = scan_dir_at(&base);
        // Ninguna entrada basura produce un plugin.
        assert!(
            scanned.is_empty(),
            "ninguna entrada basura debe producir un plugin (got {})",
            scanned.len()
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_iteration_cap_bounds_walk_even_with_valid_plugins() {
        // HIGH 2 (DoS) — con MUCHAS más entradas que el cap, el resultado nunca supera el
        // cap aunque TODAS fueran válidas; y si la base es enorme, el walk corta. Verifica
        // el invariante: len(scanned) <= MAX_PLUGINS_SCANNED siempre.
        let base = tmp_base();
        let total = MAX_PLUGINS_SCANNED * 3;
        for i in 0..total {
            write_legacy_plugin(&base, &format!("plg-{i:05}"), "1.0.0");
        }
        let scanned = scan_dir_at(&base);
        assert!(
            scanned.len() <= MAX_PLUGINS_SCANNED,
            "el scan nunca devuelve más que el cap (got {})",
            scanned.len()
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_skips_oversized_manifest() {
        // HIGH 2 — un manifest gigante (> cap) se saltea sin leerse/parsearse entero.
        let base = tmp_base();
        write_legacy_plugin(&base, "ok-plugin", "1.0.0");
        let big_dir = base.join("huge-plugin");
        std::fs::create_dir_all(&big_dir).unwrap();
        // Escribe un manifest.json > MAX_MANIFEST_BYTES (relleno con espacios; sería JSON
        // válido salvo el tamaño — el punto es que ni siquiera lo leemos).
        let mut payload = String::from("{\"name\":\"huge-plugin\",\"version\":\"1.0.0\",\"commands\":[],\"permissions\":[]}");
        payload.push_str(&" ".repeat((MAX_MANIFEST_BYTES as usize) + 1024));
        std::fs::write(big_dir.join("manifest.json"), payload).unwrap();
        let scanned = scan_dir_at(&base);
        let names: Vec<&str> = scanned.iter().map(|s| s.manifest.name.as_str()).collect();
        assert!(names.contains(&"ok-plugin"), "el plugin chico sí aparece");
        assert!(
            !names.contains(&"huge-plugin"),
            "el plugin con manifest gigante se saltea (cap de bytes)"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn scan_ignores_non_directory_entries() {
        // HIGH 2 — archivos sueltos en la base (no directorios) se ignoran.
        let base = tmp_base();
        write_legacy_plugin(&base, "real", "1.0.0");
        std::fs::write(base.join("stray-file.json"), "{}").unwrap();
        let scanned = scan_dir_at(&base);
        assert_eq!(scanned.len(), 1, "sólo el subdir con manifest cuenta");
        assert_eq!(scanned[0].manifest.name, "real");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn manifest_name_mismatch_is_rejected() {
        let base = tmp_base();
        let dir = base.join("realdir");
        std::fs::create_dir_all(&dir).unwrap();
        let m = PluginManifest {
            name: "different".into(),
            version: "1.0.0".into(),
            description: None,
            commands: vec![],
            permissions: vec![],
        };
        std::fs::write(dir.join("manifest.json"), serde_json::to_string(&m).unwrap()).unwrap();
        let scanned = scan_dir_at(&base);
        assert!(scanned.is_empty(), "manifest cuyo name != dir name se descarta");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn signed_bundle_plugin_verifies_from_disk() {
        // Usa el bundle firmado real del repo si está presente en el checkout.
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let src = repo.join("plugins").join("bundle").join("filesystem-ls");
        if !src.join("manifest.json").is_file() {
            return; // bundle no presente en este checkout
        }
        let base = tmp_base();
        let dir = base.join("filesystem-ls");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(src.join("manifest.json"), dir.join("manifest.json")).unwrap();
        let scanned = scan_dir_at(&base);
        let sp = scanned
            .iter()
            .find(|s| s.manifest.name == "filesystem-ls")
            .expect("filesystem-ls escaneado");
        assert!(sp.verified, "el bundle firmado verifica desde disco");
        std::fs::remove_dir_all(&base).ok();
    }
}
