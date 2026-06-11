//! 028 F0 · ACP Agent Registry — CRUD + resolución de definiciones de agentes ACP.
//!
//! Furx ya es cliente ACP (`services/acp.rs`) con `AgentKind::Acp` hardcodeado a
//! `agents::ACP_DEFAULT_BIN`. Este registro permite definir MÚLTIPLES agentes ACP nombrados
//! (Zed/JetBrains style): agregar uno = datos, no código (diferencial agent-neutral).
//!
//! INVARIANTES:
//!  - **argv-only**: `bin`/`args` se materializan como argv (NUNCA por shell). La validación rechaza
//!    metacaracteres de shell en `bin` (defensa en profundidad; el spawn ya es argv).
//!  - **BYOK (F-I)**: una definición NUNCA lleva un secret. `env_extra` pasa el guardrail de secretos.
//!  - **Cero-regresión / fail-safe**: `resolve(None|inexistente)` cae a la const default
//!    (`agents::ACP_DEFAULT_BIN`); borrar todas las definiciones NO rompe el spawn.

use crate::services::agents::ACP_DEFAULT_BIN;
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

type Db = Arc<parking_lot::Mutex<Connection>>;

/// Una definición declarativa de agente ACP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpAgentDef {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Binario ACP a spawnear (argv[0]). Resoluble en PATH o ruta; argv-only.
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_extra: HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub is_default: bool,
}

fn default_true() -> bool {
    true
}

/// Resultado de resolver qué agente ACP usar para un spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAcp {
    pub bin: String,
    pub args: Vec<String>,
    pub env_extra: HashMap<String, String>,
}

// ── Validación (argv-only, sin secretos) ──────────────────────────────────────

/// `id` (slug): `[A-Za-z0-9_-]{1,48}`.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `name`: legible, sin control chars ni comillas (defensa anti-inyección en logs/SQL shell).
fn valid_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && t.len() <= 80 && t.chars().all(|c| !c.is_control() && c != '\'' && c != '"')
}

/// Intérpretes/launchers genéricos PROHIBIDOS como `bin`.
///
/// MODELO DE SEGURIDAD (audit codex): el control PRIMARIO de "qué binario spawnea Furx" es el
/// **gating** (`acp_agents_upsert` tiene `requires_confirmation=true` → cada registro pasa por
/// aprobación humana) MÁS el **PATH-only** del charset. Este denylist es **defensa en profundidad
/// NO-EXHAUSTIVA**: bloquea los intérpretes/launchers obvios que con `args=["-c"/"run"/"-e", …]`
/// evadirían el PATH-only al cablear el spawn (F2). NO pretende ser un sandbox: un binario que evada
/// el denylist igual requiere la aprobación explícita del usuario. Un agente ACP legítimo es un
/// servidor dedicado, no un intérprete/launcher genérico.
const INTERPRETER_BINS: &[&str] = &[
    // shells
    "sh", "bash", "zsh", "dash", "fish", "ksh", "csh", "tcsh", "ash", "pwsh", "powershell", "cmd",
    "command",
    // launchers de proceso
    "env", "xargs", "eval", "exec", "nohup", "nice", "timeout", "time", "watch", "setsid", "stdbuf",
    // intérpretes / runtimes
    "perl", "ruby", "node", "nodejs", "deno", "bun", "php", "lua", "tclsh", "osascript", "groovy",
    "java", "javaw", "jshell", "dotnet", "r", "rscript", "julia", "swift", "scala", "clojure", "bb",
    "go", "py", "pyw", "pypy", "pypy3", // launchers Python (Windows `py`, PyPy)
    "wscript", "cscript", "mshta", // Windows Script Host / launchers de script
    // launchers de paquetes / shims (ejecutan código arbitrario vía args)
    "npx", "npm", "yarn", "yarnpkg", "pnpm", "pnpx", "bunx", "corepack", "uv", "uvx", "pipx", "pip",
    "pip3", "gem", "bundle", "cargo", "rake", "make", "gradle", "mvn", "lein",
];

/// Sufijos de ejecutable a normalizar ANTES de matchear el denylist (audit codex): como el charset
/// permite `.`, un shim con extensión (`python.exe`, `npx.cmd`, `powershell.ps1`) evadiría el match
/// exacto. Se les quita el sufijo conocido antes de comparar.
const EXE_SUFFIXES: &[&str] = &[".exe", ".cmd", ".bat", ".com", ".ps1"];

/// `bin`: **PATH-only** (council ALTA): un NOMBRE de comando resoluble en PATH — `[A-Za-z0-9._-]`,
/// sin separadores de ruta, sin espacios, sin metacaracteres de shell. Esto impide apuntar a una
/// ruta absoluta arbitraria (`/tmp/evil`) o inyectar shell. Además rechaza intérpretes/launchers
/// genéricos (ver `INTERPRETER_BINS` — defensa en profundidad, NO sandbox; el gating es el control
/// primario). (Rutas validadas → F2 si hay demanda.)
fn valid_bin(bin: &str) -> bool {
    if bin.is_empty()
        || bin.len() > 128
        || bin.starts_with('-') // no confundir con un flag
        || !bin
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return false;
    }
    let mut base = bin.to_ascii_lowercase();
    // Normalizar sufijo de ejecutable (audit codex: `python.exe`/`npx.cmd` evadían el match exacto).
    for suf in EXE_SUFFIXES {
        if let Some(stripped) = base.strip_suffix(suf) {
            base = stripped.to_string();
            break;
        }
    }
    // Familia python: bloquear cualquier `python*` (python, python3, python3.11, python3.11m,
    // pythonw, …) por PREFIJO (audit codex: el version-strip dejaba pasar `pythonw`/`python3.11m`).
    // Acepto el raro falso positivo de un binario legítimo llamado `python-*` — es más seguro y un
    // agente ACP no se llama así (a diferencia de `node-*`, común en tooling JS, que sí se permite).
    if base.starts_with("python") {
        return false;
    }
    // Resto: base EXACTA o version-stripped (trailing `[0-9.]`) — sin partir por `-`, para no
    // bloquear binarios legítimos tipo `node-acp-wrapper`. Así `node18`→`node` (bloqueado), `node-acp` (ok).
    let unversioned = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    !INTERPRETER_BINS.contains(&base.as_str()) && !INTERPRETER_BINS.contains(&unversioned)
}

/// Valida una definición completa para add/update.
/// Council ALTA: bin PATH-only. Council ALTA: `env_extra` DIFERIDO al MVP (guardrail frágil) → se
/// rechaza si no está vacío.
fn validate(def: &AcpAgentDef) -> Result<()> {
    if !valid_id(&def.id) {
        return Err(anyhow!("id inválido (permitido: [A-Za-z0-9_-]{{1,48}})"));
    }
    if !valid_name(&def.name) {
        return Err(anyhow!("name inválido (sin control/comillas, ≤80)"));
    }
    if !valid_bin(&def.bin) {
        return Err(anyhow!(
            "bin inválido: debe ser un NOMBRE de comando resoluble en PATH ([A-Za-z0-9._-], sin rutas, espacios ni metacaracteres de shell)"
        ));
    }
    if def.args.len() > 64 {
        return Err(anyhow!("demasiados args (máx 64)"));
    }
    for a in &def.args {
        if a.contains('\0') {
            return Err(anyhow!("arg con NUL byte"));
        }
    }
    // Council ALTA: `env_extra` está DIFERIDO a F2 (el guardrail de secretos es frágil). En el MVP
    // una definición NUNCA lleva env propio — los secrets viven en el Keychain (BYOK).
    if !def.env_extra.is_empty() {
        return Err(anyhow!(
            "env_extra está diferido a una versión futura (council): por ahora una definición ACP no lleva env propio"
        ));
    }
    Ok(())
}

// ── Seed lazy de la default (cero-regresión) ──────────────────────────────────

/// La definición SEMILLA por defecto, derivada de la const `ACP_DEFAULT_BIN`. Inserta lazy (no
/// migración destructiva): la primera vez que se lista/usa el registro, si no hay una default, se crea.
fn ensure_default_seeded(conn: &Connection) -> Result<()> {
    let exists: bool = conn
        .query_row("SELECT 1 FROM acp_agents WHERE is_default = 1", [], |_| Ok(()))
        .is_ok();
    if !exists {
        conn.execute(
            "INSERT OR IGNORE INTO acp_agents (id, name, bin, args, env_extra, enabled, is_default)
             VALUES ('default', 'Claude Code (ACP)', ?1, '[]', '{}', 1, 1)",
            [ACP_DEFAULT_BIN],
        )?;
    }
    Ok(())
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

fn row_to_def(
    id: String,
    name: String,
    bin: String,
    args_json: String,
    env_json: String,
    enabled: bool,
    is_default: bool,
) -> AcpAgentDef {
    AcpAgentDef {
        id,
        name,
        bin,
        args: serde_json::from_str(&args_json).unwrap_or_default(),
        env_extra: serde_json::from_str(&env_json).unwrap_or_default(),
        enabled,
        is_default,
    }
}

/// Lista todas las definiciones (sembrando la default si falta).
pub fn list_all(db: &Db) -> Result<Vec<AcpAgentDef>> {
    let conn = db.lock();
    ensure_default_seeded(&conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, name, bin, args, env_extra, enabled, is_default FROM acp_agents ORDER BY is_default DESC, created_at ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(row_to_def(
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get::<_, i64>(5)? != 0,
            r.get::<_, i64>(6)? != 0,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Crea o reemplaza (upsert por id) una definición. Valida argv-only + sin secretos. NO se puede
/// marcar `is_default` desde acá (la default la gestiona el seed).
pub fn upsert(db: &Db, mut def: AcpAgentDef) -> Result<AcpAgentDef> {
    def.is_default = false; // sólo el seed marca la default
    validate(&def)?;
    // BLOCKER codex: NO se puede modificar la definición default vía upsert. El id `default` está
    // reservado al seed; y aunque la default tuviera otro id, el `ON CONFLICT DO UPDATE` conservaría
    // `is_default=1` y dejaría la default con un `bin` arbitrario → rompe la cero-regresión. Por eso
    // se rechaza tanto el id reservado como cualquier fila `is_default=1`.
    if def.id == "default" {
        return Err(anyhow!("'default' es un id reservado para la definición ACP por defecto"));
    }
    let conn = db.lock();
    let target_is_default: bool = conn
        .query_row(
            "SELECT is_default FROM acp_agents WHERE id=?1",
            [&def.id],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false);
    if target_is_default {
        return Err(anyhow!(
            "no se puede modificar la definición ACP por defecto vía upsert (es seed-managed)"
        ));
    }
    let args_json = serde_json::to_string(&def.args)?;
    let env_json = serde_json::to_string(&def.env_extra)?;
    conn.execute(
        "INSERT INTO acp_agents (id, name, bin, args, env_extra, enabled, is_default, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
           name=excluded.name, bin=excluded.bin, args=excluded.args, env_extra=excluded.env_extra,
           enabled=excluded.enabled, updated_at=datetime('now')",
        rusqlite::params![
            def.id,
            def.name,
            def.bin,
            args_json,
            env_json,
            def.enabled as i64,
        ],
    )?;
    Ok(def)
}

/// Borra una definición. NO se puede borrar la default (protege la cero-regresión). Devuelve `true`
/// si borró algo.
pub fn delete(db: &Db, id: &str) -> Result<bool> {
    let conn = db.lock();
    let is_default: bool = conn
        .query_row("SELECT is_default FROM acp_agents WHERE id=?1", [id], |r| {
            r.get::<_, i64>(0)
        })
        .map(|v| v != 0)
        .unwrap_or(false);
    if is_default {
        return Err(anyhow!("no se puede borrar la definición ACP por defecto"));
    }
    let n = conn.execute("DELETE FROM acp_agents WHERE id=?1", [id])?;
    Ok(n > 0)
}

// ── Resolución (fail-safe → const default) ────────────────────────────────────

/// Resuelve qué agente ACP usar. `id` = la definición seleccionada (de un perfil/SpawnPlan):
///   - `Some(id)` que existe y está habilitada → su `(bin, args, env_extra)`.
///   - `None`, id inexistente, o deshabilitada → la const default (`ACP_DEFAULT_BIN`) — cero-regresión.
///
/// NUNCA falla por una definición ausente: el spawn ACP siempre tiene un binario.
pub fn resolve(db: &Db, id: Option<&str>) -> ResolvedAcp {
    if let Some(id) = id {
        let conn = db.lock();
        let found = conn.query_row(
            "SELECT bin, args, env_extra FROM acp_agents WHERE id=?1 AND enabled=1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        );
        if let Ok((bin, args_json, env_json)) = found {
            return ResolvedAcp {
                bin,
                args: serde_json::from_str(&args_json).unwrap_or_default(),
                env_extra: serde_json::from_str(&env_json).unwrap_or_default(),
            };
        }
        // Council MEDIA: fallback NO silencioso — el perfil pidió un agente ACP explícito que no
        // existe o está deshabilitado. Avisamos (no rompemos): cae a la default.
        tracing::warn!(
            acp_agent_id = id,
            "agente ACP solicitado no encontrado o deshabilitado; usando el agente ACP por defecto"
        );
    }
    // Fallback: const default (cero-regresión).
    ResolvedAcp {
        bin: ACP_DEFAULT_BIN.to_string(),
        args: Vec::new(),
        env_extra: HashMap::new(),
    }
}

// ── 028 F1: comandos Tauri ─────────────────────────────────────────────────

/// 028 — lista las definiciones de agentes ACP (siembra la default). Safe (read).
#[tauri::command]
pub fn acp_agents_list(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<AcpAgentDef>, String> {
    list_all(&state.db).map_err(|e| e.to_string())
}

/// 028 — crea o actualiza (upsert) una definición de agente ACP. Valida PATH-only + sin env_extra
/// (council). GATEADO (requires_confirmation): registrar un agente ACP = "Furx va a spawnear este
/// binario", acto deliberado que pasa por aprobación humana (council ALTA: code-exec).
#[tauri::command]
pub fn acp_agents_upsert(
    state: tauri::State<'_, crate::AppState>,
    def: AcpAgentDef,
) -> Result<AcpAgentDef, String> {
    upsert(&state.db, def).map_err(|e| e.to_string())
}

/// 028 — borra una definición (no la default). Safe-ish (reversible re-agregando; no puede borrar la
/// default que protege la cero-regresión).
#[tauri::command]
pub fn acp_agents_delete(
    state: tauri::State<'_, crate::AppState>,
    id: String,
) -> Result<bool, String> {
    delete(&state.db, &id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/045_acp_agents.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    fn def(id: &str, bin: &str) -> AcpAgentDef {
        AcpAgentDef {
            id: id.into(),
            name: format!("Agente {id}"),
            bin: bin.into(),
            args: vec![],
            env_extra: HashMap::new(),
            enabled: true,
            is_default: false,
        }
    }

    /// El seed lazy crea la default; list la muestra.
    #[test]
    fn default_is_seeded_lazily() {
        let d = db();
        let all = list_all(&d).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].is_default);
        assert_eq!(all[0].bin, ACP_DEFAULT_BIN);
    }

    /// SC-005 / cero-regresión: resolve(None) y resolve(inexistente) caen a la const default.
    #[test]
    fn resolve_falls_back_to_default_const() {
        let d = db();
        let r_none = resolve(&d, None);
        assert_eq!(r_none.bin, ACP_DEFAULT_BIN);
        assert!(r_none.args.is_empty() && r_none.env_extra.is_empty());
        let r_missing = resolve(&d, Some("no_existe"));
        assert_eq!(r_missing.bin, ACP_DEFAULT_BIN);
    }

    /// SC-002: registrar una 2ª def y resolverla usa su bin/args.
    #[test]
    fn resolve_uses_registered_def() {
        let d = db();
        let mut g = def("gemini-acp", "gemini-acp-server");
        g.args = vec!["--flag".into()];
        upsert(&d, g).unwrap();
        let r = resolve(&d, Some("gemini-acp"));
        assert_eq!(r.bin, "gemini-acp-server");
        assert_eq!(r.args, vec!["--flag".to_string()]);
    }

    /// CRUD roundtrip.
    #[test]
    fn crud_roundtrip() {
        let d = db();
        upsert(&d, def("a", "bin-a")).unwrap();
        assert!(list_all(&d).unwrap().iter().any(|x| x.id == "a"));
        // upsert mismo id = update.
        upsert(&d, def("a", "bin-a2")).unwrap();
        let got = resolve(&d, Some("a"));
        assert_eq!(got.bin, "bin-a2");
        assert!(delete(&d, "a").unwrap());
        assert!(!list_all(&d).unwrap().iter().any(|x| x.id == "a"));
    }

    /// SC-003 (council ALTA): env_extra está diferido al MVP → cualquier env_extra no vacío se rechaza
    /// (los secrets nunca van en la definición; viven en el Keychain).
    #[test]
    fn rejects_nonempty_env_extra_in_mvp() {
        let d = db();
        let mut g = def("leaky", "some-bin");
        g.env_extra
            .insert("TOKEN".into(), "sk-ant-api03-REALLOOKINGSECRETVALUE000000".into());
        assert!(upsert(&d, g).is_err());
        // incluso un env no-secreto se rechaza en el MVP (diferido).
        let mut g2 = def("envy", "some-bin");
        g2.env_extra.insert("FOO".into(), "bar".into());
        assert!(upsert(&d, g2).is_err());
    }

    /// SC-004 (council ALTA): bin PATH-only — rechaza metacaracteres de shell, espacios Y rutas
    /// (absolutas o relativas con `/`).
    #[test]
    fn rejects_non_path_only_bin() {
        let d = db();
        for bad in [
            "bin; rm -rf /",
            "bin | cat",
            "bin$(whoami)",
            "bin`id`",
            "bin with space",
            "/usr/bin/evil",        // ruta absoluta
            "./relative",           // ruta relativa
            "sub/dir/bin",          // separador de ruta
            "-rf",                  // parece flag
        ] {
            assert!(upsert(&d, def("x", bad)).is_err(), "debió rechazar bin {bad:?}");
        }
        // Un nombre de comando legítimo SÍ pasa.
        assert!(upsert(&d, def("ok", "gemini-acp.server_v2")).is_ok());
    }

    /// No se puede borrar la default (protege cero-regresión).
    #[test]
    fn cannot_delete_default() {
        let d = db();
        list_all(&d).unwrap(); // siembra la default
        assert!(delete(&d, "default").is_err());
        assert_eq!(resolve(&d, None).bin, ACP_DEFAULT_BIN);
    }

    /// BLOCKER codex: upsert NO puede sobrescribir la default (ni por id reservado ni por fila
    /// is_default) — la cero-regresión queda protegida.
    #[test]
    fn upsert_cannot_overwrite_default() {
        let d = db();
        list_all(&d).unwrap(); // siembra la default (id='default', bin=ACP_DEFAULT_BIN)
        // Intento directo de pisar la default con un bin malicioso.
        let evil = def("default", "evilacp");
        assert!(upsert(&d, evil).is_err(), "upsert con id='default' debe fallar");
        // La default sigue intacta.
        assert_eq!(resolve(&d, None).bin, ACP_DEFAULT_BIN);
        assert_eq!(resolve(&d, Some("default")).bin, ACP_DEFAULT_BIN);
    }

    /// Audit codex: intérpretes genéricos (sh/bash/python/node/…) se rechazan como bin (cerrarían el
    /// PATH-only vía args=["-c", ...] al cablear el spawn).
    #[test]
    fn rejects_interpreter_bins() {
        let d = db();
        for bad in [
            // shells / launchers / intérpretes
            "sh", "bash", "zsh", "node", "node18", "ruby", "env", "deno", "nohup", "timeout",
            // familia python completa (audit codex: pythonw / python3.11m evadían)
            "python", "python3", "python3.11", "pythonw", "python3.11m",
            // launchers de paquetes (audit codex)
            "npx", "npm", "yarn", "pnpm", "uv", "uvx", "pipx", "pip3", "cargo", "make",
            // runtimes + launchers ronda 2 (audit codex)
            "java", "javaw", "jshell", "dotnet", "go", "julia", "scala", "clojure", "bunx", "pnpx",
            "corepack", "yarnpkg", "rscript", "py", "pyw", "pypy", "pypy3", "wscript", "cscript", "mshta",
            // bypass por sufijo ejecutable (audit codex): python.exe / npx.cmd / powershell.ps1
            "python.exe", "npx.cmd", "powershell.ps1", "bash.exe", "node.cmd", "python3.11.exe",
        ] {
            assert!(upsert(&d, def("x", bad)).is_err(), "debió rechazar intérprete/launcher {bad:?}");
        }
        // Un binario legítimo cuyo nombre CONTIENE un intérprete (con `-`) NO se bloquea.
        assert!(upsert(&d, def("ok1", "node-acp-wrapper")).is_ok());
        assert!(upsert(&d, def("ok2", "bashir-acp")).is_ok());
        assert!(upsert(&d, def("ok3", "gemini-acp.server_v2")).is_ok());
    }

    /// Una def deshabilitada cae al default en resolve.
    #[test]
    fn disabled_def_resolves_to_default() {
        let d = db();
        let mut g = def("off", "off-bin");
        g.enabled = false;
        upsert(&d, g).unwrap();
        assert_eq!(resolve(&d, Some("off")).bin, ACP_DEFAULT_BIN);
    }
}
