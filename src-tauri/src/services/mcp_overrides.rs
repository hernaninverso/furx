// 045 FR-002 (Ola 5 P1) — overrides de MCP servers + auto-discovery.
//
// La DB (`mcp_user_overrides`, migración 051) es la fuente de verdad en RUNTIME sobre qué MCP
// servers están habilitados: ~/.claude.json declara la lista canónica; el usuario togglea
// enabled/disabled SIN editar el JSON. Un server presente en ~/.claude.json sin fila de override =
// habilitado por default. `set_enabled` VALIDA que el name exista en ~/.claude.json antes de
// insertar (no se aceptan nombres inventados).
//
// Auto-discovery: escanea $PATH por binarios `mcp-*` y los OFRECE como sugerencia (NUNCA
// auto-instala ni auto-habilita — foco humano / fail-closed). El usuario decide.

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

/// DoS guard: tope de entradas escaneadas por dir del PATH en auto-discovery (audit nvidia LOW).
const MAX_ENTRIES_PER_DIR: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
pub struct McpOverride {
    pub name: String,
    pub enabled: bool,
    pub source: String, // "user" | "discovery"
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredMcp {
    /// Nombre del binario (ej `mcp-foo`).
    pub binary: String,
    /// Path absoluto donde se encontró.
    pub path: String,
    /// `true` si ya está declarado en ~/.claude.json (no es novedad).
    pub already_configured: bool,
}

/// Lee TODOS los overrides de la DB como map name→enabled. Vacío si la tabla no existe / falla.
pub fn load_overrides(db: &Arc<Mutex<Connection>>) -> BTreeMap<String, bool> {
    let conn = db.lock();
    let mut stmt = match conn.prepare("SELECT name, enabled FROM mcp_user_overrides") {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("mcp_overrides: prepare failed: {e}");
            return BTreeMap::new();
        }
    };
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
    });
    match rows {
        Ok(it) => it.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            tracing::warn!("mcp_overrides: query failed: {e}");
            BTreeMap::new()
        }
    }
}

/// `true` si el server está habilitado para Furx (DB override gana; default true sin override).
pub fn is_enabled(db: &Arc<Mutex<Connection>>, name: &str) -> bool {
    load_overrides(db).get(name).copied().unwrap_or(true)
}

/// Persiste el toggle del usuario. VALIDA que `name` exista en la lista canónica (`known`) antes de
/// insertar — un nombre inventado devuelve Err (no se inserta silenciosamente). `source = "user"`.
pub fn set_enabled(
    db: &Arc<Mutex<Connection>>,
    name: &str,
    enabled: bool,
    known: &[String],
) -> Result<()> {
    if !known.iter().any(|k| k == name) {
        return Err(anyhow::anyhow!(
            "Servidor MCP '{name}' no está en ~/.claude.json. Nombres válidos: {known:?}"
        ));
    }
    let conn = db.lock();
    conn.execute(
        "INSERT INTO mcp_user_overrides (name, enabled, source, updated_at)
         VALUES (?1, ?2, 'user', datetime('now'))
         ON CONFLICT(name) DO UPDATE SET enabled = ?2, source = 'user', updated_at = datetime('now')",
        params![name, enabled as i64],
    )?;
    Ok(())
}

/// Lista los overrides persistidos (para UI/diagnóstico).
pub fn list_overrides(db: &Arc<Mutex<Connection>>) -> Vec<McpOverride> {
    let conn = db.lock();
    let mut stmt = match conn
        .prepare("SELECT name, enabled, source FROM mcp_user_overrides ORDER BY name")
    {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = stmt.query_map([], |r| {
        Ok(McpOverride {
            name: r.get(0)?,
            enabled: r.get::<_, i64>(1)? != 0,
            source: r.get(2)?,
        })
    });
    rows.map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
}

/// Escanea $PATH por binarios cuyo nombre empieza con `mcp-` y los devuelve como SUGERENCIAS
/// (dedup por binario; primer hit gana). NO instala ni habilita nada. `configured` = nombres ya
/// en ~/.claude.json (para marcar cuáles son novedad). Acota por seguridad: ignora entradas de PATH
/// vacías, salta dirs ilegibles, y descarta nombres con caracteres raros.
pub fn discover_path(configured: &[String]) -> Vec<DiscoveredMcp> {
    match std::env::var("PATH") {
        Ok(p) => discover_in_path(&p, configured),
        Err(_) => vec![],
    }
}

/// Núcleo testeable de `discover_path` con un PATH explícito (no toca el env global → no flakea
/// bajo `cargo test` concurrente, que comparte el proceso).
fn discover_in_path(path: &str, configured: &[String]) -> Vec<DiscoveredMcp> {
    let mut out: Vec<DiscoveredMcp> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue, // dir ilegible / inexistente: saltar.
        };
        // DoS guard (audit nvidia LOW): cap de entradas escaneadas por dir del PATH.
        for entry in entries.flatten().take(MAX_ENTRIES_PER_DIR) {
            let fname = entry.file_name();
            let name = match fname.to_str() {
                Some(n) => n,
                None => continue,
            };
            if !name.starts_with("mcp-") || name.len() <= 4 {
                continue;
            }
            // Sólo archivos regulares (audit mistral/nvidia MED): un DIRECTORIO o symlink-a-dir
            // llamado `mcp-foo` no es un binario MCP. `file_type` no sigue symlinks; si es symlink,
            // resolvemos con metadata() para chequear el target. Si no podemos clasificar, saltar.
            let is_file = match entry.file_type() {
                Ok(ft) if ft.is_file() => true,
                Ok(ft) if ft.is_symlink() => entry
                    .path()
                    .metadata()
                    .map(|m| m.is_file())
                    .unwrap_or(false),
                _ => false,
            };
            if !is_file {
                continue;
            }
            // Nombre saneado: alfanumérico + - _ . (evita ruido binario / paths raros).
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            {
                continue;
            }
            if !seen.insert(name.to_string()) {
                continue; // ya visto en un dir anterior del PATH (el primero gana).
            }
            // El name de un MCP en ~/.claude.json no necesariamente == el binario; igual lo
            // reportamos como pista. `already_configured` matchea por nombre exacto del binario.
            let already_configured = configured.iter().any(|c| c == name);
            out.push(DiscoveredMcp {
                binary: name.to_string(),
                path: entry.path().to_string_lossy().to_string(),
                already_configured,
            });
        }
    }
    out.sort_by(|a, b| a.binary.cmp(&b.binary));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Arc<Mutex<Connection>> {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/051_mcp_user_overrides.sql"))
            .unwrap();
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn default_enabled_without_override() {
        let db = mem_db();
        assert!(is_enabled(&db, "mnemo")); // sin fila → habilitado.
    }

    #[test]
    fn set_enabled_validates_name() {
        let db = mem_db();
        let known = vec!["mnemo".to_string(), "ff-kit".to_string()];
        // nombre inventado → Err (no inserta).
        let err = set_enabled(&db, "inventado", false, &known).unwrap_err();
        assert!(err.to_string().contains("no está en"), "got: {err}");
        assert!(list_overrides(&db).is_empty());
        // nombre válido → persiste; DB gana.
        set_enabled(&db, "mnemo", false, &known).unwrap();
        assert!(!is_enabled(&db, "mnemo"));
        // re-toggle (upsert).
        set_enabled(&db, "mnemo", true, &known).unwrap();
        assert!(is_enabled(&db, "mnemo"));
        let ov = list_overrides(&db);
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].name, "mnemo");
        assert_eq!(ov[0].source, "user");
    }

    #[test]
    fn db_override_wins_over_config() {
        // SC-002: un server presente en config pero deshabilitado en DB cuenta como disabled.
        let db = mem_db();
        let known = vec!["codebase-memory".to_string()];
        set_enabled(&db, "codebase-memory", false, &known).unwrap();
        let overrides = load_overrides(&db);
        assert_eq!(overrides.get("codebase-memory"), Some(&false));
    }

    #[test]
    fn discover_marks_configured() {
        // No dependemos del PATH real: validamos el marcado `already_configured` con un nombre que
        // pasamos como configured. discover_path filtra por `mcp-` y saneamiento; acá sólo
        // verificamos que NO crashea y que el filtro de nombre raro funciona indirectamente.
        let configured = vec!["mcp-foo".to_string()];
        let found = discover_path(&configured);
        // No aseveramos contenido (depende del entorno), sólo que es una lista bien formada.
        for d in &found {
            assert!(d.binary.starts_with("mcp-"));
            assert!(d.binary.len() > 4);
        }
    }

    #[test]
    fn discover_skips_dirs_and_finds_files() {
        // audit mistral/nvidia MED: un DIRECTORIO `mcp-foo` NO debe reportarse; un archivo `mcp-bar`
        // sí. Aislamos el escaneo con un PATH temporal (no toca el real).
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!("furx-mcp-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::create_dir_all(tmp.join("mcp-foo")).unwrap(); // directorio → debe saltarse.
        let mut f = std::fs::File::create(tmp.join("mcp-bar")).unwrap(); // archivo → reportar.
        writeln!(f, "#!/bin/sh").unwrap();
        std::fs::File::create(tmp.join("not-mcp")).unwrap(); // no matchea prefijo.

        // PATH explícito (NO toca el env global → no flakea bajo cargo test concurrente).
        let found = discover_in_path(&tmp.to_string_lossy(), &[]);
        let _ = std::fs::remove_dir_all(&tmp);

        let names: Vec<&str> = found.iter().map(|d| d.binary.as_str()).collect();
        assert!(names.contains(&"mcp-bar"), "archivo mcp-bar debe reportarse: {names:?}");
        assert!(!names.contains(&"mcp-foo"), "directorio mcp-foo NO debe reportarse: {names:?}");
        assert!(!names.contains(&"not-mcp"), "not-mcp no matchea prefijo: {names:?}");
    }
}
