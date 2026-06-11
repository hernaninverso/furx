//! 027 · Policy-as-code en el gate de permisos — F0: motor + reglas default (cero regresión).
//!
//! Hoy el gobierno está hardcodeado en `capability::check()`:
//!   `requires_approval = Risk::Destructive | Risk::Credential || requires_confirmation`
//! y comando desconocido ⇒ fail-closed (requires_approval=true).
//!
//! Este módulo expone esa MISMA decisión como **política declarativa, determinista, local y
//! enumerable** (exportable/auditable). F0 NO introduce un motor externo (Cedar/DSL): las reglas
//! por defecto se DERIVAN del Command Registry y reproducen exactamente el gate actual. El motor
//! para reglas custom *autoradas por el usuario* (Cedar vs DSL) es decisión del council → F2.
//!
//! Invariantes (spec 027):
//!  - **Cero-regresión**: `evaluate_default(ctx)` ≡ `capability::check()` para todo comando.
//!  - **Fail-closed**: comando desconocido / contexto inválido ⇒ `RequireApproval` (lo más
//!    restrictivo), NUNCA `Allow` silencioso (FR-004).
//!  - **Local**: nada sale de la Mac. Sin red, sin estado global mutable.

use crate::services::command_registry::{self, Risk};
use serde::{Deserialize, Serialize};

/// Decisión tipada del motor de política (FR-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "n")]
pub enum Decision {
    /// Se ejecuta directo, sin aprobación humana.
    Allow,
    /// Se bloquea siempre (NO existe en el gate hardcodeado de hoy; sólo reglas custom F2).
    Deny,
    /// Requiere una aprobación humana (pending_approval → modal → re-invoke).
    RequireApproval,
    /// Requiere `n` aprobaciones independientes (F2; el kernel puede no soportarlo aún).
    RequireNApprovals(u8),
}

impl Decision {
    /// Proyección al booleano que consume hoy `CapabilityCheck.requires_approval`.
    /// Allow ⇒ false; todo lo demás (Deny/RequireApproval/RequireN) ⇒ true (más restrictivo).
    pub fn requires_approval(&self) -> bool {
        !matches!(self, Decision::Allow)
    }

    /// Nivel de restricción TOTALMENTE ORDENADO (027 F2). Mayor = más restrictivo.
    /// `Allow(0) < RequireApproval(1) == RequireNApprovals(≤1) < RequireNApprovals(n≥2) < Deny(255)`.
    /// Lo usa el combinador hardening-only: la decisión efectiva es la MÁS restrictiva entre el
    /// default y las reglas custom que matchean (una regla custom NUNCA puede bajar el nivel).
    ///
    /// Audit codex+deepseek F2: `RequireNApprovals(0|1)` se NORMALIZA al nivel de `RequireApproval`
    /// (1) — "1 aprobación" ≡ "una aprobación", y "0 aprobaciones" no debe quedar por encima de
    /// RequireApproval. (Además `is_valid_hardening` rechaza `RequireNApprovals(0)`.)
    pub fn restriction_level(&self) -> u16 {
        match self {
            Decision::Allow => 0,
            Decision::RequireApproval => 1,
            // n≤1 ≡ RequireApproval (nivel 1); n≥2 sube; saturar para no chocar con Deny(255).
            Decision::RequireNApprovals(n) => {
                if *n <= 1 {
                    1
                } else {
                    1u16.saturating_add(*n as u16).min(254)
                }
            }
            Decision::Deny => 255,
        }
    }

    /// ¿`self` es estrictamente MÁS restrictivo que `other`?
    pub fn is_more_restrictive_than(&self, other: &Decision) -> bool {
        self.restriction_level() > other.restriction_level()
    }
}

/// De dónde salió la regla que decidió (para audit, FR-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    /// Derivada del Command Registry (gate por defecto = comportamiento de hoy).
    Default,
    /// Autorada por el usuario/admin (F2).
    Custom,
}

/// La regla que aplicó a una evaluación. Se persiste en el audit (FR-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedRule {
    /// Id estable de la regla (p.ej. `default:credential`, `default:unknown-command`).
    pub id: String,
    pub source: RuleSource,
    /// Razón legible (para el audit y la UI de inspección).
    pub rationale: String,
}

/// Contexto de un request a evaluar (FR-001). `risk`/`requires_confirmation` son el input base
/// (del registry); el resto habilita reglas custom (F2) sin cambiar el default (cero-regresión).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestContext {
    pub command: String,
    /// Perfil de agente activo (F2 — reglas por perfil). `None` = sin perfil.
    #[serde(default)]
    pub agent_profile: Option<String>,
    /// Paths tocados por la acción (F2 — globs de sensibles: `migrations/`, auth, `.env`...).
    #[serde(default)]
    pub touched_paths: Vec<String>,
    /// Plugin que origina el request (F2 — reglas por plugin). `None` = núcleo.
    #[serde(default)]
    pub plugin: Option<String>,
}

impl RequestContext {
    /// Atajo: contexto mínimo de un comando (lo único que mira el gate default).
    pub fn for_command(command: &str) -> Self {
        RequestContext {
            command: command.to_string(),
            agent_profile: None,
            touched_paths: Vec::new(),
            plugin: None,
        }
    }
}

/// Resultado de una evaluación: la decisión + la regla que la produjo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluation {
    pub decision: Decision,
    pub applied_rule: AppliedRule,
}

/// Una regla default enumerable (para export/inspección/audit — FR-007). Se deriva del registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultRule {
    pub command: String,
    pub decision: Decision,
    pub applied_rule: AppliedRule,
}

/// Clasifica el `Risk` + `requires_confirmation` de un comando a una decisión + regla.
/// ESTA función ES el gate por defecto: reproduce 1:1 `capability::check()` (cero-regresión).
fn classify(risk: Risk, requires_confirmation: bool) -> (Decision, AppliedRule) {
    // Orden = el de `check()`: Destructive|Credential primero, luego requires_confirmation.
    match risk {
        Risk::Destructive => (
            Decision::RequireApproval,
            AppliedRule {
                id: "default:destructive".into(),
                source: RuleSource::Default,
                rationale: "Risk::Destructive ⇒ aprobación humana".into(),
            },
        ),
        Risk::Credential => (
            Decision::RequireApproval,
            AppliedRule {
                id: "default:credential".into(),
                source: RuleSource::Default,
                rationale: "Risk::Credential (BYOK) ⇒ aprobación humana".into(),
            },
        ),
        // Safe / External: sólo confirma si requires_confirmation está seteado (igual que hoy).
        Risk::Safe | Risk::External if requires_confirmation => (
            Decision::RequireApproval,
            AppliedRule {
                id: "default:requires-confirmation".into(),
                source: RuleSource::Default,
                rationale: "requires_confirmation=true ⇒ aprobación humana".into(),
            },
        ),
        Risk::Safe | Risk::External => (
            Decision::Allow,
            AppliedRule {
                id: "default:allow".into(),
                source: RuleSource::Default,
                rationale: "Safe/External sin requires_confirmation ⇒ directo".into(),
            },
        ),
    }
}

/// Regla fail-closed para comando desconocido (no está en el registry tipado).
fn unknown_command_rule() -> (Decision, AppliedRule) {
    (
        Decision::RequireApproval,
        AppliedRule {
            id: "default:unknown-command".into(),
            source: RuleSource::Default,
            rationale: "comando ausente del registry ⇒ fail-closed (aprobación)".into(),
        },
    )
}

/// Evalúa SÓLO con las reglas por defecto (gate de hoy). F0/MVP.
///
/// Determinista, sin red, sin estado. Comando desconocido ⇒ `RequireApproval` (fail-closed).
pub fn evaluate_default(ctx: &RequestContext) -> Evaluation {
    let (decision, applied_rule) = match command_registry::registry()
        .into_iter()
        .find(|c| c.id == ctx.command)
    {
        Some(def) => classify(def.risk, def.requires_confirmation),
        None => unknown_command_rule(),
    };
    Evaluation {
        decision,
        applied_rule,
    }
}

/// Enumera TODAS las reglas por defecto (una por comando del registry) — para export/inspección
/// (FR-007/FR-009). El orden sigue el del registry (estable).
pub fn default_rules() -> Vec<DefaultRule> {
    command_registry::registry()
        .into_iter()
        .map(|def| {
            let (decision, applied_rule) = classify(def.risk, def.requires_confirmation);
            DefaultRule {
                command: def.id,
                decision,
                applied_rule,
            }
        })
        .collect()
}

// ── 027 F2: reglas custom (hardening-only) ───────────────────────────────────
//
// Council v2 (clarify §3): en MVP una regla custom SÓLO PUEDE ENDURECER (subir el nivel de
// restricción), NUNCA relajar — permitir `Destructive require_approval → allow` bajaría la
// seguridad respecto del default. Relajar = flag avanzado separado → diferido. Por eso `decision`
// de una `CustomRule` se restringe a `RequireApproval | Deny` (validado), y el combinador toma el
// MÁXIMO nivel de restricción entre el default y las custom que matchean.
//
// Matching en MVP: sólo sobre contexto CONOCIDO en el dispatch (command/risk/agent_profile/plugin).
// Las reglas por `touched_paths` se difieren (council: "los paths pueden llegar tarde" si el comando
// los determina post-ejecución; requieren plumbing de contexto de paths pre-gate).

/// Una regla custom autorada por el usuario/admin. Campos de match `None` = comodín (matchea todo).
/// Una regla matchea un `RequestContext` si TODOS sus campos seteados coinciden (AND).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRule {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// Match exacto por command id. `None` = cualquiera.
    #[serde(default)]
    pub match_command: Option<String>,
    /// Match por clase de riesgo del comando. `None` = cualquiera.
    #[serde(default)]
    pub match_risk: Option<Risk>,
    /// Match por perfil de agente activo. `None` = cualquiera.
    #[serde(default)]
    pub match_agent_profile: Option<String>,
    /// Match por plugin originante. `None` = cualquiera.
    #[serde(default)]
    pub match_plugin: Option<String>,
    /// Decisión a imponer si matchea. SÓLO endurece: `RequireApproval | RequireNApprovals(n≥1) |
    /// Deny` (validado por `is_valid_hardening`; `Allow` y `RequireNApprovals(0)` se rechazan).
    pub decision: Decision,
}

impl CustomRule {
    /// ¿Esta regla es válida para hardening-only? La `decision` debe ser endurecedora
    /// (`RequireApproval | Deny | RequireNApprovals(n≥2)`), NUNCA `Allow` (eso relajaría) ni
    /// `RequireNApprovals(0)` (audit codex: "0 aprobaciones" no tiene sentido y sería peligroso si
    /// se cablea literalmente). Además al menos un criterio de match debe estar seteado (una regla
    /// sin matchers aplicaría a TODO y es casi seguro un error → la rechazamos para no romper el
    /// flujo entero por accidente).
    pub fn is_valid_hardening(&self) -> bool {
        match self.decision {
            Decision::Allow => return false,
            Decision::RequireNApprovals(0) => return false,
            _ => {}
        }
        self.match_command.is_some()
            || self.match_risk.is_some()
            || self.match_agent_profile.is_some()
            || self.match_plugin.is_some()
    }

    /// ¿Matchea este contexto? (AND de los campos seteados.) `cmd_risk` es el riesgo del comando
    /// ya resuelto del registry (cacheado por el caller para no re-escanear el registry por regla —
    /// audit deepseek F2); `None` si el comando no está en el registry (un `match_risk` no matchea
    /// un comando desconocido → queda fail-closed por el default, que para desconocidos es
    /// `RequireApproval`).
    fn matches(&self, ctx: &RequestContext, cmd_risk: Option<Risk>) -> bool {
        if let Some(cmd) = &self.match_command {
            if cmd != &ctx.command {
                return false;
            }
        }
        if let Some(risk) = &self.match_risk {
            if Some(*risk) != cmd_risk {
                return false;
            }
        }
        if let Some(profile) = &self.match_agent_profile {
            if Some(profile) != ctx.agent_profile.as_ref() {
                return false;
            }
        }
        if let Some(plugin) = &self.match_plugin {
            if Some(plugin) != ctx.plugin.as_ref() {
                return false;
            }
        }
        true
    }
}

/// Evalúa el contexto contra el default + reglas custom, HARDENING-ONLY (027 F2).
///
/// - `custom_enabled=false` (default OFF) ⇒ idéntico a `evaluate_default` (cero overhead/cambio).
/// - `custom_enabled=true` ⇒ parte del default y por cada regla custom VÁLIDA que matchee, sube al
///   nivel de restricción MÁXIMO. Una regla custom NUNCA baja la restricción (hardening-only): si su
///   decisión NO es ESTRICTAMENTE más restrictiva que la vigente, se ignora. Reglas inválidas
///   (decision=Allow / RequireNApprovals(0) / sin matchers) se IGNORAN (fail-safe: el default sigue
///   aplicando — council §6).
///
/// Atribución determinista: gana la PRIMERA regla (en orden del slice) que alcanza el nivel máximo
/// estrictamente por encima del default. La unicidad de `id` la garantiza el storage (F2-wiring,
/// constraint UNIQUE), no este motor puro.
pub fn evaluate(ctx: &RequestContext, custom_enabled: bool, custom_rules: &[CustomRule]) -> Evaluation {
    let base = evaluate_default(ctx);
    if !custom_enabled {
        return base;
    }
    // Riesgo del comando resuelto UNA vez (audit deepseek F2: evita re-escanear el registry por
    // cada regla con `match_risk`).
    let cmd_risk = command_registry::registry()
        .into_iter()
        .find(|c| c.id == ctx.command)
        .map(|c| c.risk);
    // La regla custom que más endurece (estrictamente por encima del base).
    let mut best: Option<(&CustomRule, u16)> = None;
    for rule in custom_rules {
        if !rule.is_valid_hardening() || !rule.matches(ctx, cmd_risk) {
            continue;
        }
        // Hardening-only: sólo cuenta si supera el nivel actual (base o la mejor custom hasta ahora).
        let level = rule.decision.restriction_level();
        let current = best.map(|(_, l)| l).unwrap_or(base.decision.restriction_level());
        if level > current {
            best = Some((rule, level));
        }
    }
    match best {
        Some((rule, _)) => Evaluation {
            decision: rule.decision.clone(),
            applied_rule: AppliedRule {
                id: format!("custom:{}", rule.id),
                source: RuleSource::Custom,
                rationale: if rule.description.is_empty() {
                    format!("regla custom '{}' (hardening)", rule.id)
                } else {
                    rule.description.clone()
                },
            },
        },
        // Ninguna custom superó el default ⇒ queda el default (cero relajación).
        None => base,
    }
}

// ── 027 F2-wiring: persistencia local de reglas custom + audit de cambios ─────
//
// Storage de las reglas custom (tabla `policy_rules`, migración 044) + audit append-only de los
// CAMBIOS de política (`policy_rule_changes`). El motor (`evaluate`) es puro; estas funciones SÓLO
// cargan/persisten. La unicidad de `id` la garantiza el PK de la tabla (council).
pub mod store {
    use super::*;
    use anyhow::{anyhow, Result};

    type Db = std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>;

    /// ¿Aplicar reglas custom? Setting `policy.custom_enabled`. Default **OFF** (opt-in, council §6).
    pub fn custom_enabled(db: &Db) -> bool {
        let conn = db.lock();
        crate::settings::get(&conn, "policy.custom_enabled")
            .ok()
            .flatten()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Cambia `policy.custom_enabled` + AUDITA el cambio (transacción atómica). Audit codex BLOCKER:
    /// es el ÚNICO escritor de ese setting (el setter genérico rechaza `policy.*`). El comando Tauri
    /// que lo expone está gateado (requires_confirmation) → APAGAR las reglas custom (relajar el
    /// gobierno) requiere aprobación humana y queda en el audit append-only `policy_rule_changes`.
    pub fn set_custom_enabled(db: &Db, enabled: bool) -> Result<()> {
        let mut conn = db.lock();
        let tx = conn.transaction()?;
        crate::settings::set(&tx, "policy.custom_enabled", &serde_json::Value::Bool(enabled))?;
        tx.execute(
            "INSERT INTO policy_rule_changes (id, rule_id, action, snapshot) VALUES (?1, '*', ?2, ?3)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                if enabled { "enable" } else { "disable" },
                serde_json::json!({ "policy.custom_enabled": enabled }).to_string(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Serializa una `Decision` a la representación de texto de la columna `decision`.
    fn decision_to_str(d: &Decision) -> String {
        match d {
            Decision::Allow => "allow".into(), // nunca persistido (CHECK lo prohíbe), por completitud
            Decision::Deny => "deny".into(),
            Decision::RequireApproval => "require_approval".into(),
            Decision::RequireNApprovals(n) => format!("require_n_approvals:{n}"),
        }
    }

    /// Parsea la representación de texto de `decision`. Fail-safe: texto inválido → None (la regla se
    /// descarta al cargar, NO se asume Allow).
    fn decision_from_str(s: &str) -> Option<Decision> {
        match s {
            "deny" => Some(Decision::Deny),
            "require_approval" => Some(Decision::RequireApproval),
            other => other
                .strip_prefix("require_n_approvals:")
                .and_then(|n| n.parse::<u8>().ok())
                .map(Decision::RequireNApprovals),
            // 'allow' u otro → None (no se relaja vía storage corrupto).
        }
    }

    fn risk_to_str(r: &Risk) -> &'static str {
        match r {
            Risk::Safe => "safe",
            Risk::Destructive => "destructive",
            Risk::Credential => "credential",
            Risk::External => "external",
        }
    }

    fn risk_from_str(s: &str) -> Option<Risk> {
        match s {
            "safe" => Some(Risk::Safe),
            "destructive" => Some(Risk::Destructive),
            "credential" => Some(Risk::Credential),
            "external" => Some(Risk::External),
            _ => None,
        }
    }

    /// Carga las reglas custom HABILITADAS y VÁLIDAS (hardening-only) de la DB. Las inválidas o con
    /// `decision`/`risk` no-parseable se DESCARTAN (fail-safe: el default sigue aplicando), nunca se
    /// asume Allow.
    pub fn list_enabled_valid(db: &Db) -> Result<Vec<CustomRule>> {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, description, match_command, match_risk, match_agent_profile, match_plugin, decision
             FROM policy_rules WHERE enabled = 1 ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            let decision_s: String = row.get(6)?;
            let risk_s: Option<String> = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                risk_s,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                decision_s,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, description, mc, mr, mp_prof, mp_plug, dec_s) = r?;
            let decision = match decision_from_str(&dec_s) {
                Some(d) => d,
                None => continue, // texto corrupto → descartar (no relajar)
            };
            let match_risk = match mr {
                Some(s) => match risk_from_str(&s) {
                    Some(rk) => Some(rk),
                    None => continue, // risk corrupto → descartar
                },
                None => None,
            };
            let rule = CustomRule {
                id,
                description,
                match_command: mc,
                match_risk,
                match_agent_profile: mp_prof,
                match_plugin: mp_plug,
                decision,
            };
            // Defensa: sólo reglas hardening válidas.
            if rule.is_valid_hardening() {
                out.push(rule);
            }
        }
        Ok(out)
    }

    /// TODAS las reglas (habilitadas o no) para la UI de inspección.
    pub fn list_all(db: &Db) -> Result<Vec<StoredRule>> {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, description, match_command, match_risk, match_agent_profile, match_plugin, decision, enabled
             FROM policy_rules ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredRule {
                id: row.get(0)?,
                description: row.get(1)?,
                match_command: row.get(2)?,
                match_risk: row.get(3)?,
                match_agent_profile: row.get(4)?,
                match_plugin: row.get(5)?,
                decision: row.get(6)?,
                enabled: row.get::<_, i64>(7)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Crea o reemplaza una regla custom (upsert por `id`) + registra el cambio en el audit.
    /// Valida hardening-only ANTES de tocar la DB (fail-closed: una regla relajante se rechaza).
    ///
    /// Audit codex F2-wiring (BLOCKER atomicidad): el cambio de la regla y su registro en el audit
    /// van en UNA transacción — si el insert del audit falla, el cambio de la regla se revierte
    /// (nunca queda una política vigente cambiada sin registro append-only).
    pub fn upsert(db: &Db, rule: &CustomRule) -> Result<()> {
        if !rule.is_valid_hardening() {
            return Err(anyhow!(
                "regla '{}' inválida (debe endurecer y tener al menos un matcher)",
                rule.id
            ));
        }
        let mut conn = db.lock();
        let existed: bool = conn
            .query_row(
                "SELECT 1 FROM policy_rules WHERE id = ?1",
                [&rule.id],
                |_| Ok(()),
            )
            .is_ok();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO policy_rules (id, description, match_command, match_risk, match_agent_profile, match_plugin, decision, enabled, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
               description=excluded.description, match_command=excluded.match_command,
               match_risk=excluded.match_risk, match_agent_profile=excluded.match_agent_profile,
               match_plugin=excluded.match_plugin, decision=excluded.decision,
               enabled=1, updated_at=datetime('now')",
            rusqlite::params![
                rule.id,
                rule.description,
                rule.match_command,
                rule.match_risk.as_ref().map(risk_to_str),
                rule.match_agent_profile,
                rule.match_plugin,
                decision_to_str(&rule.decision),
            ],
        )?;
        log_change(&tx, &rule.id, if existed { "update" } else { "create" }, rule)?;
        tx.commit()?;
        Ok(())
    }

    /// Borra una regla custom (si existe) + registra el cambio en el audit (transacción atómica:
    /// el borrado y el audit confirman juntos o se revierten juntos — audit codex BLOCKER).
    /// El snapshot del audit conserva la regla COMPLETA (no sólo id/decision/enabled).
    pub fn remove(db: &Db, id: &str) -> Result<()> {
        let mut conn = db.lock();
        // Snapshot COMPLETO previo para el audit (si existe).
        let snap: Option<String> = conn
            .query_row(
                "SELECT json_object('id',id,'description',description,'match_command',match_command,\
                 'match_risk',match_risk,'match_agent_profile',match_agent_profile,\
                 'match_plugin',match_plugin,'decision',decision,'enabled',enabled) \
                 FROM policy_rules WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .ok();
        let tx = conn.transaction()?;
        let n = tx.execute("DELETE FROM policy_rules WHERE id = ?1", [id])?;
        if n > 0 {
            let snapshot = snap.unwrap_or_else(|| format!("{{\"id\":\"{id}\"}}"));
            tx.execute(
                "INSERT INTO policy_rule_changes (id, rule_id, action, snapshot) VALUES (?1, ?2, 'delete', ?3)",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), id, snapshot],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Registra un cambio de política en el audit append-only.
    fn log_change(
        conn: &rusqlite::Connection,
        rule_id: &str,
        action: &str,
        rule: &CustomRule,
    ) -> Result<()> {
        // Audit deepseek/AIE F2-wiring: propagar el error de serialización en vez de silenciarlo con
        // "{}" — un snapshot vacío perdería trazabilidad del cambio de política.
        let snapshot = serde_json::to_string(rule)
            .map_err(|e| anyhow!("no se pudo serializar la regla '{rule_id}' para el audit: {e}"))?;
        conn.execute(
            "INSERT INTO policy_rule_changes (id, rule_id, action, snapshot) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), rule_id, action, snapshot],
        )?;
        Ok(())
    }

    /// Vista de fila para la UI (incluye `enabled` y los strings crudos de la DB).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StoredRule {
        pub id: String,
        pub description: String,
        pub match_command: Option<String>,
        pub match_risk: Option<String>,
        pub match_agent_profile: Option<String>,
        pub match_plugin: Option<String>,
        pub decision: String,
        pub enabled: bool,
    }
}

// ── 027 F2-wiring: comandos Tauri (gestión de reglas custom desde el front) ───

/// Resultado de `policy_preview`: qué decidiría el gate para un comando con/ sin las reglas custom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPreview {
    /// Decisión del default (sin reglas custom).
    pub default_decision: Decision,
    /// Decisión efectiva (default + custom si está habilitado).
    pub effective_decision: Decision,
    /// La regla que decidió la efectiva (default o custom).
    pub applied_rule: AppliedRule,
    /// `true` si una regla custom endureció la decisión respecto del default.
    pub hardened_by_custom: bool,
    /// Si las reglas custom están habilitadas (`policy.custom_enabled`).
    pub custom_enabled: bool,
}

/// 027 — lista TODAS las reglas custom (habilitadas o no) para la UI de inspección. Safe (read).
#[tauri::command]
pub fn policy_list_rules(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<store::StoredRule>, String> {
    store::list_all(&state.db).map_err(|e| e.to_string())
}

/// 027 — crea o reemplaza una regla custom (upsert). Valida hardening-only (rechaza relajantes).
/// Cada cambio queda auditado (append-only). Gateado (requires_confirmation): cambiar la política de
/// gobierno es un acto deliberado.
#[tauri::command]
pub fn policy_set_rule(
    state: tauri::State<'_, crate::AppState>,
    rule: CustomRule,
) -> Result<(), String> {
    store::upsert(&state.db, &rule).map_err(|e| e.to_string())
}

/// 027 — borra una regla custom. RELAJA el gobierno → gateado (requires_confirmation) + auditado.
#[tauri::command]
pub fn policy_remove_rule(
    state: tauri::State<'_, crate::AppState>,
    id: String,
) -> Result<(), String> {
    store::remove(&state.db, &id).map_err(|e| e.to_string())
}

/// 027 — habilita/deshabilita las reglas custom (`policy.custom_enabled`). ÚNICO path para cambiar
/// ese setting (el setter genérico rechaza `policy.*`). Gateado (requires_confirmation) + auditado:
/// apagarlo RELAJA el gobierno → requiere aprobación humana. Audit codex BLOCKER.
#[tauri::command]
pub fn policy_set_custom_enabled(
    state: tauri::State<'_, crate::AppState>,
    enabled: bool,
) -> Result<(), String> {
    store::set_custom_enabled(&state.db, enabled).map_err(|e| e.to_string())
}

/// 027 — previsualiza qué decidiría el gate para un comando (default vs efectivo con custom).
/// Safe (read, no muta). Útil para la UI: "esta regla endurecería X de Allow a RequireApproval".
#[tauri::command]
pub fn policy_preview(
    state: tauri::State<'_, crate::AppState>,
    command_id: String,
    agent_profile: Option<String>,
    plugin: Option<String>,
) -> Result<PolicyPreview, String> {
    let ctx = RequestContext {
        command: command_id,
        agent_profile,
        touched_paths: Vec::new(),
        plugin,
    };
    let default = evaluate_default(&ctx);
    let enabled = store::custom_enabled(&state.db);
    let rules = store::list_enabled_valid(&state.db).map_err(|e| e.to_string())?;
    let eff = evaluate(&ctx, enabled, &rules);
    Ok(PolicyPreview {
        hardened_by_custom: eff.decision.is_more_restrictive_than(&default.decision),
        default_decision: default.decision,
        effective_decision: eff.decision,
        applied_rule: eff.applied_rule,
        custom_enabled: enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::capability;

    /// SC-001 — CERO-REGRESIÓN: para TODO comando del registry, la decisión del motor default
    /// proyecta exactamente a la FÓRMULA LEGACY EXPLÍCITA del gate
    /// (`Risk::Destructive | Risk::Credential || requires_confirmation`).
    ///
    /// Audit codex F1: NO se compara contra `capability::check()` — eso quedó tautológico porque
    /// `check()` ahora DELEGA en `evaluate_default` (se compararía contra sí mismo). El ancla de
    /// cero-regresión es la fórmula inline original, replicada acá a propósito como oráculo
    /// independiente del refactor. Si alguien cambia el motor, este test (y no la delegación) lo caza.
    #[test]
    fn default_engine_matches_legacy_formula_for_every_command() {
        for def in command_registry::registry() {
            let legacy_requires_approval =
                matches!(def.risk, Risk::Destructive | Risk::Credential) || def.requires_confirmation;
            let eval = evaluate_default(&RequestContext::for_command(&def.id));
            assert_eq!(
                eval.decision.requires_approval(),
                legacy_requires_approval,
                "comando {} divergió de la fórmula legacy: motor={:?} legacy={}",
                def.id,
                eval.decision,
                legacy_requires_approval
            );
            assert_eq!(eval.applied_rule.source, RuleSource::Default);
        }
    }

    /// SC-002 (fail-closed) — comando que NO está en el registry ⇒ RequireApproval.
    #[test]
    fn unknown_command_fails_closed() {
        let ctx = RequestContext::for_command("comando_inexistente_xyz_123");
        let eval = evaluate_default(&ctx);
        assert_eq!(eval.decision, Decision::RequireApproval);
        assert!(eval.decision.requires_approval());
        assert_eq!(eval.applied_rule.id, "default:unknown-command");
        // Coincide con el fail-closed del gate legacy.
        assert!(capability::check("comando_inexistente_xyz_123").requires_approval);
    }

    /// Las clases de riesgo mapean a la decisión esperada (independiente del registry).
    #[test]
    fn risk_classes_map_to_expected_decisions() {
        assert_eq!(classify(Risk::Destructive, false).0, Decision::RequireApproval);
        assert_eq!(classify(Risk::Credential, false).0, Decision::RequireApproval);
        assert_eq!(classify(Risk::Safe, false).0, Decision::Allow);
        assert_eq!(classify(Risk::External, false).0, Decision::Allow);
        // requires_confirmation fuerza aprobación aun en Safe/External.
        assert_eq!(classify(Risk::Safe, true).0, Decision::RequireApproval);
        assert_eq!(classify(Risk::External, true).0, Decision::RequireApproval);
    }

    /// `default_rules()` enumera exactamente un rule por comando del registry, sin perder ninguno.
    #[test]
    fn default_rules_cover_every_command() {
        let rules = default_rules();
        let registry_ids: Vec<String> =
            command_registry::registry().into_iter().map(|c| c.id).collect();
        assert_eq!(rules.len(), registry_ids.len());
        for id in &registry_ids {
            assert!(
                rules.iter().any(|r| &r.command == id),
                "default_rules() no cubre {id}"
            );
        }
    }

    /// `Decision::requires_approval()` proyecta bien cada variante.
    #[test]
    fn decision_projection_is_restrictive() {
        assert!(!Decision::Allow.requires_approval());
        assert!(Decision::Deny.requires_approval());
        assert!(Decision::RequireApproval.requires_approval());
        assert!(Decision::RequireNApprovals(2).requires_approval());
    }

    /// Audit codex F0: el campo extra del contexto (agent_profile/touched_paths/plugin) NO cambia
    /// la decisión del motor default (custom OFF en F0 = cero-regresión sin importar el contexto).
    #[test]
    fn extra_context_does_not_affect_default_evaluation() {
        // Tomamos un comando real cualquiera y comparamos contexto mínimo vs contexto enriquecido.
        let any = command_registry::registry()
            .into_iter()
            .next()
            .expect("registry no vacío");
        let bare = evaluate_default(&RequestContext::for_command(&any.id));
        let enriched = evaluate_default(&RequestContext {
            command: any.id.clone(),
            agent_profile: Some("perfil-cualquiera".into()),
            touched_paths: vec!["migrations/0001.sql".into(), ".env".into()],
            plugin: Some("plugin-x".into()),
        });
        assert_eq!(
            bare, enriched,
            "el contexto extra no debe alterar la decisión default (custom OFF)"
        );
    }

    /// Audit codex F0: `default_rules()` no repite comandos (unicidad de `command`).
    #[test]
    fn default_rules_have_unique_commands() {
        let rules = default_rules();
        let mut seen = std::collections::HashSet::new();
        for r in &rules {
            assert!(
                seen.insert(r.command.clone()),
                "default_rules() repite el comando {}",
                r.command
            );
        }
    }

    // ── 027 F2: motor de hardening-only ──────────────────────────────────────

    /// Helper: encuentra un comando del registry con una decisión default dada.
    fn cmd_with_default(target: Decision) -> Option<String> {
        command_registry::registry().into_iter().find_map(|c| {
            let d = evaluate_default(&RequestContext::for_command(&c.id)).decision;
            if d == target {
                Some(c.id)
            } else {
                None
            }
        })
    }

    /// Niveles de restricción totalmente ordenados (con la normalización n≤1 ≡ RequireApproval).
    #[test]
    fn restriction_levels_are_totally_ordered() {
        assert!(Decision::Allow.restriction_level() < Decision::RequireApproval.restriction_level());
        // n≤1 ≡ RequireApproval (audit codex+deepseek).
        assert_eq!(
            Decision::RequireApproval.restriction_level(),
            Decision::RequireNApprovals(1).restriction_level()
        );
        // n≥2 sí supera a RequireApproval.
        assert!(
            Decision::RequireApproval.restriction_level()
                < Decision::RequireNApprovals(2).restriction_level()
        );
        assert!(
            Decision::RequireNApprovals(2).restriction_level()
                < Decision::RequireNApprovals(3).restriction_level()
        );
        assert!(
            Decision::RequireNApprovals(250).restriction_level() < Decision::Deny.restriction_level()
        );
        assert!(Decision::Deny.is_more_restrictive_than(&Decision::RequireApproval));
        assert!(!Decision::Allow.is_more_restrictive_than(&Decision::RequireApproval));
    }

    /// custom_enabled=false ⇒ idéntico al default sin importar las reglas (cero cambio).
    #[test]
    fn custom_disabled_is_identical_to_default() {
        let rule = CustomRule {
            id: "x".into(),
            description: String::new(),
            match_command: None,
            match_risk: Some(Risk::Safe),
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        for c in command_registry::registry() {
            let ctx = RequestContext::for_command(&c.id);
            assert_eq!(evaluate(&ctx, false, std::slice::from_ref(&rule)), evaluate_default(&ctx));
        }
    }

    /// HARDENING: una regla custom sube un comando Allow → RequireApproval (SC-003).
    #[test]
    fn custom_rule_hardens_allow_to_require_approval() {
        let cmd = cmd_with_default(Decision::Allow).expect("hay algún comando Allow en el registry");
        let ctx = RequestContext::for_command(&cmd);
        assert_eq!(evaluate_default(&ctx).decision, Decision::Allow);
        let rule = CustomRule {
            id: "harden".into(),
            description: "endurecer".into(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::RequireApproval,
        };
        let ev = evaluate(&ctx, true, &[rule]);
        assert_eq!(ev.decision, Decision::RequireApproval);
        assert_eq!(ev.applied_rule.source, RuleSource::Custom);
        assert_eq!(ev.applied_rule.id, "custom:harden");
    }

    /// HARDENING-ONLY: una regla custom NO puede RELAJAR (decision=Allow es inválida → ignorada;
    /// y aunque matchee, jamás baja la restricción del default).
    #[test]
    fn custom_rule_cannot_relax() {
        // Un comando que el default gatea (RequireApproval).
        let cmd =
            cmd_with_default(Decision::RequireApproval).expect("hay algún comando gateado");
        let ctx = RequestContext::for_command(&cmd);
        // Regla custom que intenta poner Allow (relajar): inválida → ignorada → queda el default.
        let relax = CustomRule {
            id: "relax".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Allow,
        };
        assert!(!relax.is_valid_hardening());
        assert_eq!(evaluate(&ctx, true, &[relax]).decision, Decision::RequireApproval);
    }

    /// La regla MÁS restrictiva gana entre varias que matchean (Deny > RequireApproval).
    #[test]
    fn most_restrictive_matching_rule_wins() {
        let cmd = cmd_with_default(Decision::Allow).expect("hay algún comando Allow");
        let ctx = RequestContext::for_command(&cmd);
        let r1 = CustomRule {
            id: "soft".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::RequireApproval,
        };
        let r2 = CustomRule {
            id: "hard".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        // En cualquier orden gana Deny.
        assert_eq!(evaluate(&ctx, true, &[r1.clone(), r2.clone()]).decision, Decision::Deny);
        assert_eq!(evaluate(&ctx, true, &[r2, r1]).decision, Decision::Deny);
    }

    /// Una regla SIN matchers (aplicaría a todo) es inválida → ignorada (no rompe el flujo entero).
    #[test]
    fn rule_without_matchers_is_ignored() {
        let cmd = cmd_with_default(Decision::Allow).expect("hay algún comando Allow");
        let ctx = RequestContext::for_command(&cmd);
        let no_match = CustomRule {
            id: "catch-all".into(),
            description: String::new(),
            match_command: None,
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        assert!(!no_match.is_valid_hardening());
        assert_eq!(evaluate(&ctx, true, &[no_match]).decision, Decision::Allow);
    }

    /// Match por perfil de agente (sólo endurece cuando el perfil coincide).
    #[test]
    fn custom_rule_matches_by_agent_profile() {
        let cmd = cmd_with_default(Decision::Allow).expect("hay algún comando Allow");
        let rule = CustomRule {
            id: "by-profile".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: Some("riesgoso".into()),
            match_plugin: None,
            decision: Decision::RequireApproval,
        };
        // Con el perfil que matchea → endurece.
        let ctx_match = RequestContext {
            command: cmd.clone(),
            agent_profile: Some("riesgoso".into()),
            touched_paths: vec![],
            plugin: None,
        };
        assert_eq!(evaluate(&ctx_match, true, std::slice::from_ref(&rule)).decision, Decision::RequireApproval);
        // Con otro perfil → NO matchea → queda default (Allow).
        let ctx_other = RequestContext {
            command: cmd.clone(),
            agent_profile: Some("otro".into()),
            touched_paths: vec![],
            plugin: None,
        };
        assert_eq!(evaluate(&ctx_other, true, &[rule]).decision, Decision::Allow);
    }

    /// Audit codex F2: `RequireNApprovals(0)` es inválido (no endurece) → ignorado; `RequireNApprovals(1)`
    /// ≡ RequireApproval en nivel; `RequireNApprovals(n≥2)` supera a RequireApproval.
    #[test]
    fn require_n_approvals_levels_and_validity() {
        assert_eq!(
            Decision::RequireNApprovals(1).restriction_level(),
            Decision::RequireApproval.restriction_level()
        );
        assert!(
            Decision::RequireNApprovals(2).restriction_level()
                > Decision::RequireApproval.restriction_level()
        );
        // RequireNApprovals(0) como decisión de una regla custom es inválida.
        let r0 = CustomRule {
            id: "n0".into(),
            description: String::new(),
            match_command: Some("x".into()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::RequireNApprovals(0),
        };
        assert!(!r0.is_valid_hardening());
        // n≥2 sí es válida.
        let r2 = CustomRule {
            decision: Decision::RequireNApprovals(2),
            ..r0.clone()
        };
        assert!(r2.is_valid_hardening());
    }

    /// Audit codex F2: una regla custom endurece un comando ya gateado RequireApproval → Deny.
    #[test]
    fn custom_rule_hardens_require_approval_to_deny() {
        let cmd = cmd_with_default(Decision::RequireApproval).expect("hay comando gateado");
        let ctx = RequestContext::for_command(&cmd);
        let rule = CustomRule {
            id: "to-deny".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        assert_eq!(evaluate(&ctx, true, &[rule]).decision, Decision::Deny);
    }

    /// Audit codex F2: AND de múltiples matchers — todos deben coincidir para aplicar.
    #[test]
    fn multi_matcher_rule_is_and() {
        let cmd = cmd_with_default(Decision::Allow).expect("hay comando Allow");
        let rule = CustomRule {
            id: "and".into(),
            description: String::new(),
            match_command: Some(cmd.clone()),
            match_risk: None,
            match_agent_profile: Some("p".into()),
            match_plugin: Some("plug".into()),
            decision: Decision::Deny,
        };
        // Todos coinciden → aplica.
        let full = RequestContext {
            command: cmd.clone(),
            agent_profile: Some("p".into()),
            touched_paths: vec![],
            plugin: Some("plug".into()),
        };
        assert_eq!(evaluate(&full, true, std::slice::from_ref(&rule)).decision, Decision::Deny);
        // Falta el plugin → NO aplica → default.
        let partial = RequestContext {
            command: cmd.clone(),
            agent_profile: Some("p".into()),
            touched_paths: vec![],
            plugin: None,
        };
        assert_eq!(evaluate(&partial, true, &[rule]).decision, Decision::Allow);
    }

    /// Audit codex F2: una regla por `match_command` sobre un comando DESCONOCIDO endurece de
    /// RequireApproval (fail-closed default) a Deny — válido.
    #[test]
    fn unknown_command_can_be_hardened_by_command_match() {
        let ctx = RequestContext::for_command("comando_desconocido_zzz");
        assert_eq!(evaluate_default(&ctx).decision, Decision::RequireApproval);
        let rule = CustomRule {
            id: "deny-unknown".into(),
            description: String::new(),
            match_command: Some("comando_desconocido_zzz".into()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        assert_eq!(evaluate(&ctx, true, &[rule]).decision, Decision::Deny);
        // Pero un `match_risk` NO matchea un comando desconocido (su risk es None) → queda el default.
        let by_risk = CustomRule {
            id: "by-risk".into(),
            description: String::new(),
            match_command: None,
            match_risk: Some(Risk::Safe),
            match_agent_profile: None,
            match_plugin: None,
            decision: Decision::Deny,
        };
        assert_eq!(evaluate(&ctx, true, &[by_risk]).decision, Decision::RequireApproval);
    }

    // ── 027 F2-wiring: storage ───────────────────────────────────────────────

    type Db = std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>;

    fn store_db() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/002_settings.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/044_policy_custom_rules.sql"))
            .unwrap();
        std::sync::Arc::new(parking_lot::Mutex::new(conn))
    }

    fn sample_rule(id: &str, decision: Decision) -> CustomRule {
        CustomRule {
            id: id.into(),
            description: "regla de prueba".into(),
            match_command: Some("algun_comando".into()),
            match_risk: None,
            match_agent_profile: None,
            match_plugin: None,
            decision,
        }
    }

    /// custom_enabled default OFF; roundtrip de upsert→list_enabled_valid.
    #[test]
    fn store_roundtrip_and_default_off() {
        let db = store_db();
        assert!(!store::custom_enabled(&db), "default debe ser OFF");
        store::upsert(&db, &sample_rule("r1", Decision::Deny)).unwrap();
        let loaded = store::list_enabled_valid(&db).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "r1");
        assert_eq!(loaded[0].decision, Decision::Deny);
        // upsert del mismo id = update (no duplica).
        store::upsert(&db, &sample_rule("r1", Decision::RequireApproval)).unwrap();
        let loaded = store::list_enabled_valid(&db).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].decision, Decision::RequireApproval);
    }

    /// El storage RECHAZA una regla relajante (decision=Allow) antes de tocar la DB (fail-closed).
    #[test]
    fn store_rejects_relaxing_rule() {
        let db = store_db();
        let relax = sample_rule("bad", Decision::Allow);
        assert!(store::upsert(&db, &relax).is_err());
        assert!(store::list_enabled_valid(&db).unwrap().is_empty());
    }

    /// El audit de cambios es append-only: UPDATE/DELETE sobre `policy_rule_changes` fallan.
    #[test]
    fn store_change_audit_is_append_only() {
        let db = store_db();
        store::upsert(&db, &sample_rule("r1", Decision::Deny)).unwrap();
        store::remove(&db, "r1").unwrap();
        let conn = db.lock();
        // create + delete = 2 eventos.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM policy_rule_changes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // UPDATE y DELETE prohibidos por trigger.
        assert!(conn
            .execute("UPDATE policy_rule_changes SET action='x'", [])
            .is_err());
        assert!(conn
            .execute("DELETE FROM policy_rule_changes", [])
            .is_err());
    }

    /// Una fila con `decision` corrupto se DESCARTA al cargar (no se asume Allow).
    #[test]
    fn store_discards_corrupt_decision() {
        let db = store_db();
        {
            let conn = db.lock();
            // Insertar directo una fila con decision inválida (saltea la validación del backend).
            conn.execute(
                "INSERT INTO policy_rules (id, match_command, decision) VALUES ('corrupt', 'x', 'require_approval')",
                [],
            )
            .unwrap();
            // Forzar un valor corrupto (el CHECK sólo prohíbe 'allow', no texto arbitrario).
            conn.execute(
                "UPDATE policy_rules SET decision='basura_no_parseable' WHERE id='corrupt'",
                [],
            )
            .unwrap();
        }
        // Al cargar, la fila corrupta se descarta (no rompe, no relaja).
        assert!(store::list_enabled_valid(&db).unwrap().is_empty());
    }

    /// `set_custom_enabled` cambia el flag Y audita el cambio (enable/disable) en append-only.
    #[test]
    fn store_set_custom_enabled_audits() {
        let db = store_db();
        assert!(!store::custom_enabled(&db));
        store::set_custom_enabled(&db, true).unwrap();
        assert!(store::custom_enabled(&db));
        store::set_custom_enabled(&db, false).unwrap();
        assert!(!store::custom_enabled(&db));
        let conn = db.lock();
        // 2 eventos de cambio de política (enable + disable), inmutables.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM policy_rule_changes WHERE rule_id='*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
        let actions: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT action FROM policy_rule_changes WHERE rule_id='*' ORDER BY changed_at, action")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert!(actions.contains(&"enable".to_string()));
        assert!(actions.contains(&"disable".to_string()));
    }

    /// Las reglas DESHABILITADAS no se cargan como activas.
    #[test]
    fn store_disabled_rules_not_loaded() {
        let db = store_db();
        store::upsert(&db, &sample_rule("r1", Decision::Deny)).unwrap();
        {
            let conn = db.lock();
            conn.execute("UPDATE policy_rules SET enabled=0 WHERE id='r1'", [])
                .unwrap();
        }
        assert!(store::list_enabled_valid(&db).unwrap().is_empty());
        // list_all sí la muestra (para la UI).
        let all = store::list_all(&db).unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].enabled);
    }
}
