// services/agent_profiles.rs — 006-agent-profiles.
//
// El agente como entidad de primera clase: un perfil ejecutable guardable y conmutable
// por pane. Config NO-secreta sólo (SQLite); el secret/token vive en Keychain y lo
// referencia indirectamente vía `account_slug` (NUNCA el token) — F-I BYOK.
//
// La RESOLUCIÓN a (cmd,args,env) vive en commands.rs (necesita resolve_mode + flags
// por-CLI). Acá: data model + CRUD + seed de built-ins + export/import sanitizado +
// synth_mode (la pieza pura que mapea (cli_kind, account_slug) → mode string legacy).

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

type Db = Arc<parking_lot::Mutex<Connection>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub cli_kind: String, // zsh|claude|codex|gemini|aider|openai-api|custom
    #[serde(default)]
    pub account_slug: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub default_cwd: Option<String>,
    #[serde(default)]
    pub council_enabled: bool,
    #[serde(default)]
    pub council_preset: Option<String>,
    #[serde(default)]
    pub shell_enabled: bool,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub is_builtin: bool,
    /// Motor: 'cli' (corre un CLI en un pane PTY) | 'aie' (REPL HTTP contra AIE — DIFERIDO
    /// a spec aparte). MVP: siempre 'cli'. El council 2026-05-29 separó el motor del cli_kind.
    #[serde(default = "default_engine_kind")]
    pub engine_kind: String,
    /// Categoría para agrupar presets/roles en la UI ('soporte' | 'ventas' | 'qa' | ...).
    #[serde(default)]
    pub category: Option<String>,
    /// Allow-list de plugin ids habilitados. NO altera permisos del plugin (los gobierna
    /// el manifest + consent); sólo decide qué plugins se cargan para este agente.
    #[serde(default)]
    pub plugins: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn default_engine_kind() -> String {
    "cli".to_string()
}

const VALID_KINDS: &[&str] = &[
    "zsh",
    "claude",
    "codex",
    "gemini",
    "aider",
    "grok",
    "openai-api",
    "custom",
];
const VALID_ENGINES: &[&str] = &["cli", "aie"];

fn valid_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty()
        && t.len() <= 64
        && t.chars()
            .all(|c| !c.is_control() && c != '\'' && c != '"' && c != ';')
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 32
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Mapea (cli_kind, account_slug) al `mode` string que entiende `resolve_mode()`.
/// Pura → unit-testable. Devuelve Err si la combinación es inválida (ej. claude sin slug).
pub fn synth_mode(cli_kind: &str, account_slug: Option<&str>) -> Result<String> {
    let slug = account_slug.filter(|s| !s.is_empty());
    if let Some(s) = slug {
        if !valid_slug(s) {
            return Err(anyhow!("invalid account_slug '{}'", s));
        }
    }
    Ok(match cli_kind {
        "zsh" => "zsh".to_string(),
        // Claude SIEMPRE necesita una cuenta (no hay mode legacy "claude").
        "claude" => match slug {
            Some(s) => format!("claude-{}", s),
            None => {
                return Err(anyhow!(
                    "un agente Claude requiere una cuenta (account_slug)"
                ))
            }
        },
        // Estos tienen mode legacy sin slug (usan la config/env default del CLI).
        "codex" | "gemini" | "aider" => match slug {
            Some(s) => format!("{}-{}", cli_kind, s),
            None => cli_kind.to_string(),
        },
        // 062: grok NO es account-managed (auth por su propio `grok login`/OAuth) → SIEMPRE "grok",
        // ignora cualquier slug. Sin esto un perfil con account_slug daría "grok-<slug>", que
        // resolve_mode no reconoce → caería a zsh (audit codex). No hay cuentas grok, así que un slug
        // acá es espurio: lo descartamos y lanzamos el grok legacy.
        "grok" => "grok".to_string(),
        // Estos sólo existen como "<kind>-<slug>" en resolve_mode.
        "openai-api" => match slug {
            Some(s) => format!("openai-api-{}", s),
            None => {
                return Err(anyhow!(
                    "un agente openai-api requiere una cuenta (account_slug)"
                ))
            }
        },
        "custom" => match slug {
            Some(s) => format!("custom-{}", s),
            None => {
                return Err(anyhow!(
                    "un agente custom requiere una cuenta (account_slug)"
                ))
            }
        },
        other => return Err(anyhow!("cli_kind desconocido: {}", other)),
    })
}

fn validate(p: &AgentProfile) -> Result<()> {
    if !valid_name(&p.name) {
        return Err(anyhow!("nombre inválido (1-64, sin control/comillas/;)"));
    }
    if !VALID_KINDS.contains(&p.cli_kind.as_str()) {
        return Err(anyhow!("cli_kind inválido: {}", p.cli_kind));
    }
    if !VALID_ENGINES.contains(&p.engine_kind.as_str()) {
        return Err(anyhow!("engine_kind inválido: {} (cli|aie)", p.engine_kind));
    }
    // El slug, si está presente, debe ser válido. La REQUISITORIA de cuenta (claude/
    // openai-api/custom necesitan una) se DIFIERE al spawn (synth_mode) — así se puede
    // crear/importar un perfil "borrador" sin cuenta y asignarla después (FR-012: un
    // agent.json importado llega con account_slug=None y se asocia localmente).
    if let Some(s) = p.account_slug.as_deref().filter(|s| !s.is_empty()) {
        if !valid_slug(s) {
            return Err(anyhow!("invalid account_slug '{}'", s));
        }
    }
    Ok(())
}

fn load_plugins(conn: &Connection, agent_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT plugin_id FROM agent_profile_plugins WHERE agent_id = ?1 AND enabled = 1 ORDER BY plugin_id",
    )?;
    let rows = stmt.query_map(params![agent_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

fn write_plugins(conn: &Connection, agent_id: &str, plugins: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM agent_profile_plugins WHERE agent_id = ?1",
        params![agent_id],
    )?;
    for pid in plugins {
        if pid.trim().is_empty() {
            continue;
        }
        conn.execute(
            "INSERT OR IGNORE INTO agent_profile_plugins (agent_id, plugin_id, enabled) VALUES (?1, ?2, 1)",
            params![agent_id, pid],
        )?;
    }
    Ok(())
}

fn row_to_profile(conn: &Connection, r: &rusqlite::Row) -> rusqlite::Result<AgentProfile> {
    let id: String = r.get(0)?;
    Ok(AgentProfile {
        id: id.clone(),
        name: r.get(1)?,
        description: r.get(2)?,
        cli_kind: r.get(3)?,
        account_slug: r.get(4)?,
        model: r.get(5)?,
        system_prompt: r.get(6)?,
        default_cwd: r.get(7)?,
        council_enabled: r.get::<_, i64>(8)? != 0,
        council_preset: r.get(9)?,
        shell_enabled: r.get::<_, i64>(10)? != 0,
        icon: r.get(11)?,
        color: r.get(12)?,
        is_builtin: r.get::<_, i64>(13)? != 0,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
        engine_kind: r.get(16)?,
        category: r.get(17)?,
        plugins: load_plugins(conn, &id).unwrap_or_default(),
    })
}

const SELECT_COLS: &str = "id, name, description, cli_kind, account_slug, model, system_prompt,
    default_cwd, council_enabled, council_preset, shell_enabled, icon, color, is_builtin,
    created_at, updated_at, engine_kind, category";

pub fn list_all(db: &Db) -> Result<Vec<AgentProfile>> {
    let conn = db.lock();
    let sql =
        format!("SELECT {SELECT_COLS} FROM agent_profiles ORDER BY is_builtin DESC, name ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| row_to_profile(&conn, r))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn get(db: &Db, id: &str) -> Result<Option<AgentProfile>> {
    let conn = db.lock();
    let sql = format!("SELECT {SELECT_COLS} FROM agent_profiles WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![id], |r| row_to_profile(&conn, r))?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

pub fn create(db: &Db, mut p: AgentProfile) -> Result<AgentProfile> {
    validate(&p)?;
    p.id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    {
        let conn = db.lock();
        conn.execute(
            "INSERT INTO agent_profiles
                (id, name, description, cli_kind, account_slug, model, system_prompt,
                 default_cwd, council_enabled, council_preset, shell_enabled, icon, color,
                 is_builtin, engine_kind, category, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?17)",
            params![
                p.id,
                p.name.trim(),
                p.description,
                p.cli_kind,
                p.account_slug,
                p.model,
                p.system_prompt,
                p.default_cwd,
                p.council_enabled as i64,
                p.council_preset,
                p.shell_enabled as i64,
                p.icon,
                p.color,
                0i64,
                p.engine_kind,
                p.category,
                now,
            ],
        )?;
        write_plugins(&conn, &p.id, &p.plugins)?;
    }
    get(db, &p.id)?.ok_or_else(|| anyhow!("create round-trip read failed"))
}

pub fn update(db: &Db, p: AgentProfile) -> Result<AgentProfile> {
    if p.id.is_empty() {
        return Err(anyhow!("update requiere id"));
    }
    validate(&p)?;
    let now = Utc::now().to_rfc3339();
    {
        let conn = db.lock();
        let n = conn.execute(
            "UPDATE agent_profiles SET
                name=?2, description=?3, cli_kind=?4, account_slug=?5, model=?6, system_prompt=?7,
                default_cwd=?8, council_enabled=?9, council_preset=?10, shell_enabled=?11,
                icon=?12, color=?13, engine_kind=?14, category=?15, updated_at=?16
             WHERE id=?1",
            params![
                p.id,
                p.name.trim(),
                p.description,
                p.cli_kind,
                p.account_slug,
                p.model,
                p.system_prompt,
                p.default_cwd,
                p.council_enabled as i64,
                p.council_preset,
                p.shell_enabled as i64,
                p.icon,
                p.color,
                p.engine_kind,
                p.category,
                now,
            ],
        )?;
        if n == 0 {
            return Err(anyhow!("no existe agente con id {}", p.id));
        }
        write_plugins(&conn, &p.id, &p.plugins)?;
    }
    get(db, &p.id)?.ok_or_else(|| anyhow!("update round-trip read failed"))
}

pub fn delete(db: &Db, id: &str) -> Result<bool> {
    let conn = db.lock();
    let is_builtin: Option<i64> = conn
        .query_row(
            "SELECT is_builtin FROM agent_profiles WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .ok();
    match is_builtin {
        None => Ok(false),
        Some(1) => Err(anyhow!("no se puede borrar un agente built-in")),
        Some(_) => {
            // ON DELETE CASCADE limpia agent_profile_plugins.
            let n = conn.execute("DELETE FROM agent_profiles WHERE id = ?1", params![id])?;
            Ok(n > 0)
        }
    }
}

/// Siembra agentes built-in (idempotente, id determinístico) desde los modes legacy
/// + una cuenta Claude por slug. Re-correr no duplica (INSERT OR IGNORE por id+name).
pub fn seed_builtins(db: &Db, claude_slugs: &[String]) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = db.lock();
    let seed = |bid: &str, name: &str, cli_kind: &str, slug: Option<&str>| -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO agent_profiles
                (id, name, description, cli_kind, account_slug, system_prompt,
                 council_enabled, shell_enabled, is_builtin, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,'',0,0,1,?6,?6)",
            params![
                bid,
                name,
                "Built-in (migrado del modo legacy)",
                cli_kind,
                slug,
                now
            ],
        )?;
        Ok(())
    };
    seed("builtin:zsh", "Shell (zsh)", "zsh", None)?;
    seed("builtin:codex", "Codex", "codex", None)?;
    seed("builtin:gemini", "Gemini", "gemini", None)?;
    seed("builtin:aider", "Aider", "aider", None)?;
    seed("builtin:grok", "Grok", "grok", None)?;
    for slug in claude_slugs {
        if !valid_slug(slug) {
            continue;
        }
        seed(
            &format!("builtin:claude:{}", slug),
            &format!("Claude · {}", slug),
            "claude",
            Some(slug),
        )?;
    }

    // Role PRESETS (council 2026-05-29): plantillas built-in con system_prompt curado +
    // category. cli_kind=claude, SIN cuenta (borrador) — el user clona y asigna la suya.
    // engine_kind='cli'. Idempotentes por id. NOTA: `name` es UNIQUE — en install fresco
    // siembran OK; si el user ya tiene un agente con ese nombre exacto, OR IGNORE saltea ese
    // preset (el user ya es dueño de ese nombre) — degradación aceptable, sin crash.
    let seed_preset = |bid: &str, name: &str, category: &str, prompt: &str| -> Result<()> {
        conn.execute(
            "INSERT OR IGNORE INTO agent_profiles
                (id, name, description, cli_kind, account_slug, system_prompt,
                 council_enabled, shell_enabled, is_builtin, engine_kind, category,
                 icon, created_at, updated_at)
             VALUES (?1,?2,?3,'claude',NULL,?4,0,0,1,'cli',?5,?6,?7,?7)",
            params![
                bid,
                name,
                "Preset de rol (cloná y asigná tu cuenta)",
                prompt,
                category,
                "◆",
                now
            ],
        )?;
        Ok(())
    };
    seed_preset(
        "preset:soporte-l1", "Soporte L1", "soporte",
        "Sos un agente de Soporte Nivel 1. Atendé consultas iniciales con empatía y claridad: \
identificá el problema, respondé dudas frecuentes, seguí los runbooks conocidos y resolvé lo \
de primer nivel. Si excede tu alcance (bug real, acceso a datos sensibles, cambios de cuenta), \
escalá a Soporte L2 con un resumen del caso y los pasos ya intentados. No inventes; si no sabés, decilo.",
    )?;
    seed_preset(
        "preset:soporte-l2", "Soporte L2", "soporte",
        "Sos un agente de Soporte Nivel 2 (técnico). Tomás casos escalados de L1: reproducís el \
problema, leés logs y código, diagnosticás la causa raíz y proponés un fix o workaround concreto. \
Documentá la causa y la solución. Si es un bug de producto, redactá un reporte accionable para ingeniería.",
    )?;
    seed_preset(
        "preset:ventas", "Ventas", "ventas",
        "Sos un agente de Ventas. Entendé la necesidad del prospecto, conectá el producto con su \
caso de uso, respondé objeciones con honestidad y proponé el siguiente paso (demo, trial, propuesta). \
No prometas features inexistentes; ante una duda técnica, derivá a quien corresponda.",
    )?;
    seed_preset(
        "preset:qa", "QA", "qa",
        "Sos un agente de QA. Diseñá y ejecutá pruebas: casos felices, bordes y de error. Verificá \
contra la spec/criterios de aceptación, reproducí bugs con pasos mínimos y reportá con evidencia \
(qué se esperaba vs qué pasó). Preferí tests automatizados cuando sea posible. No marques algo como \
ok sin verificarlo end-to-end.",
    )?;
    Ok(())
}

/// Export sanitizado: sólo config no-secreta. NUNCA exporta tokens, service names del
/// Keychain, ids/timestamps, ni el account_slug real (se reemplaza por placeholder para
/// que el importador asocie SU cuenta local). FR-011.
pub fn export_sanitized(p: &AgentProfile) -> serde_json::Value {
    serde_json::json!({
        "schema": "furx.agent-profile.v1",
        "name": p.name,
        "description": p.description,
        "cli_kind": p.cli_kind,
        "model": p.model,
        "system_prompt": p.system_prompt,
        "council_enabled": p.council_enabled,
        "council_preset": p.council_preset,
        "plugins": p.plugins,
        "icon": p.icon,
        "color": p.color,
        "engine_kind": p.engine_kind,
        "category": p.category,
        // El agente requiere una cuenta del kind; el importador elige la suya local.
        "required_account": if p.account_slug.is_some() { serde_json::json!({"cli_kind": p.cli_kind}) } else { serde_json::Value::Null },
    })
}

/// Parsea un agent.json sanitizado a un AgentProfile de creación. `account_slug` queda en
/// None (el user lo asocia localmente) y default_cwd vacío. FR-012.
pub fn import_from_json(v: &serde_json::Value) -> Result<AgentProfile> {
    if v.get("schema").and_then(|s| s.as_str()) != Some("furx.agent-profile.v1") {
        return Err(anyhow!(
            "schema desconocido (esperado furx.agent-profile.v1)"
        ));
    }
    let name = v
        .get("name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let cli_kind = v
        .get("cli_kind")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let plugins = v
        .get("plugins")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let p = AgentProfile {
        id: String::new(),
        name,
        description: v
            .get("description")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        cli_kind,
        account_slug: None, // el importador asocia su cuenta local
        model: v.get("model").and_then(|s| s.as_str()).map(String::from),
        system_prompt: v
            .get("system_prompt")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        default_cwd: None,
        council_enabled: v
            .get("council_enabled")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        council_preset: v
            .get("council_preset")
            .and_then(|s| s.as_str())
            .map(String::from),
        shell_enabled: false,
        icon: v.get("icon").and_then(|s| s.as_str()).map(String::from),
        color: v.get("color").and_then(|s| s.as_str()).map(String::from),
        is_builtin: false,
        engine_kind: v
            .get("engine_kind")
            .and_then(|s| s.as_str())
            .unwrap_or("cli")
            .to_string(),
        category: v.get("category").and_then(|s| s.as_str()).map(String::from),
        plugins,
        created_at: String::new(),
        updated_at: String::new(),
    };
    if !valid_name(&p.name) {
        return Err(anyhow!("agent.json: nombre inválido"));
    }
    if !VALID_KINDS.contains(&p.cli_kind.as_str()) {
        return Err(anyhow!("agent.json: cli_kind inválido"));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_mode_mapping() {
        assert_eq!(synth_mode("zsh", None).unwrap(), "zsh");
        assert_eq!(synth_mode("claude", Some("A")).unwrap(), "claude-A");
        assert_eq!(synth_mode("codex", None).unwrap(), "codex");
        assert_eq!(synth_mode("codex", Some("work")).unwrap(), "codex-work");
        assert_eq!(synth_mode("openai-api", Some("x")).unwrap(), "openai-api-x");
        // Claude sin cuenta → error; openai-api/custom sin cuenta → error.
        assert!(synth_mode("claude", None).is_err());
        assert!(synth_mode("openai-api", None).is_err());
        assert!(synth_mode("custom", None).is_err());
        // kind desconocido y slug inválido → error.
        assert!(synth_mode("bogus", None).is_err());
        assert!(synth_mode("claude", Some("bad slug")).is_err());
    }

    #[test]
    fn name_validation() {
        assert!(valid_name("Rust reviewer"));
        assert!(!valid_name(""));
        assert!(!valid_name("has'quote"));
        assert!(!valid_name(&"x".repeat(65)));
    }

    #[test]
    fn export_has_no_secrets_or_slug() {
        let p = AgentProfile {
            id: "abc".into(),
            name: "Secret reviewer".into(),
            description: String::new(),
            cli_kind: "claude".into(),
            account_slug: Some("my-private-slug".into()),
            model: Some("sonnet".into()),
            system_prompt: "instrucciones".into(),
            default_cwd: Some("/Users/dev/private".into()),
            council_enabled: true,
            council_preset: None,
            shell_enabled: false,
            icon: None,
            color: None,
            is_builtin: false,
            engine_kind: "cli".into(),
            category: Some("dev".into()),
            plugins: vec!["browser-tools".into()],
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let v = export_sanitized(&p);
        let s = serde_json::to_string(&v).unwrap();
        // No filtra el slug real, ni el cwd absoluto, ni id/timestamps.
        assert!(!s.contains("my-private-slug"));
        assert!(!s.contains("/Users/dev/private"));
        assert!(!s.contains("\"id\""));
        assert!(!s.contains("\"abc\""));
        // Sí conserva lo compartible.
        assert!(s.contains("Secret reviewer"));
        assert!(s.contains("sonnet"));
        assert!(s.contains("furx.agent-profile.v1"));
    }

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        // Aplica la migración 019 real (caza typos de SQL / desajuste de columnas).
        conn.execute_batch(include_str!("../../migrations/019_agent_profiles.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../../migrations/020_agent_engine_presets.sql"
        ))
        .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    #[test]
    fn crud_roundtrip_against_sqlite() {
        let db = test_db();
        // create
        let p = create(
            &db,
            AgentProfile {
                id: String::new(),
                name: "Rust reviewer".into(),
                description: "d".into(),
                cli_kind: "claude".into(),
                account_slug: Some("A".into()),
                model: Some("sonnet".into()),
                system_prompt: "sp".into(),
                default_cwd: Some("/tmp".into()),
                council_enabled: true,
                council_preset: None,
                shell_enabled: false,
                icon: None,
                color: None,
                is_builtin: false,
                engine_kind: "cli".into(),
                category: Some("dev".into()),
                plugins: vec!["browser-tools".into(), "git".into()],
                created_at: String::new(),
                updated_at: String::new(),
            },
        )
        .unwrap();
        assert!(!p.id.is_empty());
        assert_eq!(p.plugins.len(), 2);
        assert!(p.council_enabled);
        assert_eq!(p.engine_kind, "cli");
        assert_eq!(p.category.as_deref(), Some("dev"));
        // list
        assert_eq!(list_all(&db).unwrap().len(), 1);
        // update (cambia model + reduce plugins)
        let mut up = p.clone();
        up.model = Some("opus".into());
        up.plugins = vec!["git".into()];
        let up = update(&db, up).unwrap();
        assert_eq!(up.model.as_deref(), Some("opus"));
        assert_eq!(up.plugins, vec!["git".to_string()]);
        // seed idempotente
        seed_builtins(&db, &["A".into()]).unwrap();
        seed_builtins(&db, &["A".into()]).unwrap(); // 2da vez no duplica
        let all = list_all(&db).unwrap();
        let builtins = all.iter().filter(|a| a.is_builtin).count();
        assert_eq!(builtins, 10); // zsh, codex, gemini, aider, grok, claude:A + 4 presets (L1/L2/Ventas/QA)
                                 // los presets de rol existen con su categoría
        let l1 = all.iter().find(|a| a.id == "preset:soporte-l1").unwrap();
        assert_eq!(l1.category.as_deref(), Some("soporte"));
        assert!(l1.account_slug.is_none() && !l1.system_prompt.is_empty()); // borrador con prompt curado
                                                                            // no se puede borrar built-in
        let zsh = all.iter().find(|a| a.id == "builtin:zsh").unwrap();
        assert!(delete(&db, &zsh.id).is_err());
        // sí se borra el custom + cascade de plugins
        assert!(delete(&db, &up.id).unwrap());
        assert!(get(&db, &up.id).unwrap().is_none());
    }

    #[test]
    fn create_account_requiring_kind_without_account_is_draft() {
        // Regresión: importar/crear un agente claude (que exige cuenta) NO debe fallar al
        // crear — el account_slug se asigna después. La cuenta se exige recién en el spawn.
        let db = test_db();
        let p = create(
            &db,
            AgentProfile {
                id: String::new(),
                name: "Claude draft".into(),
                description: String::new(),
                cli_kind: "claude".into(),
                account_slug: None,
                model: None,
                system_prompt: String::new(),
                default_cwd: None,
                council_enabled: false,
                council_preset: None,
                shell_enabled: false,
                icon: None,
                color: None,
                is_builtin: false,
                engine_kind: "cli".into(),
                category: None,
                plugins: vec![],
                created_at: String::new(),
                updated_at: String::new(),
            },
        )
        .unwrap();
        assert!(!p.id.is_empty());
        assert!(p.account_slug.is_none());
        // Pero correrlo sin cuenta sí falla (la requisitoria vive en synth_mode/spawn).
        assert!(synth_mode("claude", None).is_err());
    }

    #[test]
    fn import_roundtrip_drops_account() {
        let v = serde_json::json!({
            "schema": "furx.agent-profile.v1",
            "name": "Imported", "cli_kind": "codex", "model": "gpt-5",
            "system_prompt": "hi", "plugins": ["p1"], "council_enabled": true
        });
        let p = import_from_json(&v).unwrap();
        assert_eq!(p.name, "Imported");
        assert_eq!(p.cli_kind, "codex");
        assert!(p.account_slug.is_none()); // el user asocia su cuenta local
        assert_eq!(p.plugins, vec!["p1".to_string()]);
        // schema inválido → error
        assert!(import_from_json(&serde_json::json!({"name":"x"})).is_err());
    }
}
