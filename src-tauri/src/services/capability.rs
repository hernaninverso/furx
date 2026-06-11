//! 015 US4 — Capability / Permission + Approval gate + Secret provider.
//!
//! Puerta de decisión (una función, no atada a una superficie) que decide, dado un
//! `command_id`, si el comando puede ejecutarse directo o debe pasar por una
//! aprobación humana. Diseñada para aplicar venga de la command palette (US2), de un
//! botón, del companion móvil, de un plugin o de un deep-link (US9).
//!
//! ALCANCE HONESTO (audit codex US4): este módulo provee el MECANISMO (la decisión +
//! la persistencia del approval + el secret provider + el hardening BYOK). HOY las
//! superficies OPTAN por llamarlo (la palette US2 ya lo hace para confirmar
//! destructivos). El ENFORCEMENT UNIVERSAL backend-side — que TODO comando ruteado por
//! `generate_handler!` pase obligatoriamente por el gate, incluso plugins/móvil/deeplinks
//! que no opten — es una ola de integración explícita (ver tasks.md "enforcement
//! integration"): requiere un executor único que intercepte cada invoke. Hasta entonces,
//! NO afirmar que "todo comando está gateado": está gateado lo que opta por el gate.
//!
//! Cómo decide (consume el `risk` del Command Registry, US1):
//!   - Busca el `CommandDef` del `command_id` en `command_registry::registry()`.
//!   - `Risk::Destructive` / `Risk::Credential`, o `requires_confirmation == true`
//!     → requiere aprobación. Crea un `approval` *pending* (estado de primera
//!     clase, tabla `approvals`, migración 028) y emite
//!     `AppEvent::ApprovalRequested { request_id, command_id }` por el event bus
//!     (US3). El comando NO se ejecuta hasta `approval_resolve(id, true)`.
//!   - `Risk::Safe` (sin requires_confirmation) → pasa directo.
//!   - command_id desconocido → fail-closed: se trata como que requiere aprobación
//!     (no auto-ejecutamos algo que no está en el registry tipado).
//!
//! Secret/Context provider (constitución F-I BYOK) — ver módulo `secret` abajo:
//! el front NUNCA recibe la key. Despacha comandos con un *credential ref* (un id
//! string = nombre del entry del Keychain); el backend resuelve la key SÓLO al
//! ejecutar, contra el Keychain nativo. La key nunca toca el front, ni SQLite, ni
//! los logs, ni `args_json` de un approval.

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

use crate::services::command_registry::{self, Risk};
use crate::services::policy; // 027 F1 — motor de política = fuente única de la decisión del gate.

type Db = Arc<parking_lot::Mutex<Connection>>;

/// 015 T015 — TTL de un approval APROBADO-sin-consumir. Pasado este lapso desde `created_at`,
/// el approval ya no es consumible (no se puede ejecutar el comando con una aprobación vieja).
/// El estado pending puede mostrarse más tiempo en la UI; lo que caduca es la AUTORIZACIÓN.
pub const APPROVAL_TTL_SECS: i64 = 300; // 5 min

/// 015 T015 — hash CANÓNICO de los args de un comando (sha256 hex). El approval queda atado a
/// (command_id, args_hash): no se puede aprobar con unos args y ejecutar con otros. Canónico =
/// claves de objeto ordenadas recursivamente, así dos serializaciones equivalentes ({a:1,b:2} y
/// {b:2,a:1}) producen el MISMO hash. args_json vacío/blanco se trata como `{}`.
pub fn canonical_args_hash(args_json: &str) -> String {
    let trimmed = args_json.trim();
    // FAIL-CLOSED (audit T015, AIE/deepseek MED): un input que NO parsea como JSON se hashea como
    // `raw:<bytes>` (prefijo distintivo), NUNCA se colapsa a `{}`. Colapsar a `{}` permitiría que un
    // payload malformado matchee un approval de `{}` y se ejecute SIN los args reales (fail-open).
    let canon_str = if trimmed.is_empty() {
        "{}".to_string()
    } else {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(v) => serde_json::to_string(&canonicalize(&v))
                .unwrap_or_else(|_| format!("raw:{trimmed}")),
            Err(_) => format!("raw:{trimmed}"),
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(canon_str.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 015 T015 (audit codex HIGH, F-I BYOK) — REDACTA todo valor hoja (string/number/bool/null) de un
/// JSON, conservando claves y estructura. Para comandos `Credential` cuyos args podrían llevar un
/// secret crudo (ej `signals_set_telegram_secret {secret:...}`): NUNCA persistimos el valor en
/// `approvals.args_json`; sólo las CLAVES (para que la UI muestre la forma) + el `args_hash` (sha256
/// del valor real, irreversible). El secret real vive sólo transitorio en el payload del invoke.
/// Si el input no parsea, devuelve `{}` (no filtra nada). El binding del approval usa el hash del
/// valor REAL (no el redactado), así que el consumo sigue matcheando.
fn redact_values(args_json: &str) -> String {
    fn redact(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(k, val)| (k.clone(), redact(val)))
                    .collect(),
            ),
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(redact).collect())
            }
            _ => serde_json::Value::String("\u{2039}redacted\u{203a}".to_string()),
        }
    }
    match serde_json::from_str::<serde_json::Value>(args_json.trim()) {
        Ok(v) => serde_json::to_string(&redact(&v)).unwrap_or_else(|_| "{}".to_string()),
        Err(_) => "{}".to_string(),
    }
}

/// ¿el comando es `Risk::Credential` en el registry? (sus args pueden llevar secrets → redactar).
fn is_credential_command(command_id: &str) -> bool {
    command_registry::registry()
        .into_iter()
        .any(|c| c.id == command_id && matches!(c.risk, Risk::Credential))
}

/// Reconstruye un `Value` con las claves de cada objeto ORDENADAS (recursivo) para que el hash
/// no dependa del orden de inserción (independiente del feature `preserve_order` de serde_json).
fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonicalize(&map[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

// ── Decisión del gate ────────────────────────────────────────────────────────

/// Resultado de consultar el gate para un comando, SIN ejecutar nada.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityCheck {
    /// Si el comando debe pasar por aprobación humana antes de ejecutarse.
    pub requires_approval: bool,
    /// El `risk` del comando según el registry (`"safe"|"destructive"|"credential"|"external"`),
    /// o `"unknown"` si el command_id no está en el registry.
    pub risk: String,
    /// `true` si el command_id no está en el registry (fail-closed → requires_approval).
    pub unknown: bool,
}

/// Núcleo testeable y puro: ¿este comando requiere aprobación? Fail-closed para comandos
/// desconocidos.
///
/// 027 F1 — la decisión ya NO se computa inline: delega en el motor de política
/// (`policy::evaluate_default`), que es la FUENTE ÚNICA de verdad del gate. Con reglas custom OFF
/// (F0/F1), `evaluate_default` reproduce EXACTAMENTE la fórmula legacy
/// (`Destructive|Credential || requires_confirmation`, desconocido ⇒ aprobación) — verificado por
/// el test de cero-regresión de `policy`. El `risk`/`unknown` del CapabilityCheck siguen saliendo
/// del registry (metadata para UI), pero el booleano `requires_approval` lo decide el motor.
pub fn check(command_id: &str) -> CapabilityCheck {
    let requires_approval = policy::evaluate_default(&policy::RequestContext::for_command(
        command_id,
    ))
    .decision
    .requires_approval();
    match command_registry::registry()
        .into_iter()
        .find(|c| c.id == command_id)
    {
        Some(def) => CapabilityCheck {
            requires_approval,
            risk: risk_str(def.risk).to_string(),
            unknown: false,
        },
        // Fail-closed: un comando que no está en el registry tipado NO se auto-ejecuta
        // (el motor ya devolvió requires_approval=true para el desconocido).
        None => CapabilityCheck {
            requires_approval,
            risk: "unknown".to_string(),
            unknown: true,
        },
    }
}

fn risk_str(r: Risk) -> &'static str {
    match r {
        Risk::Safe => "safe",
        Risk::Destructive => "destructive",
        Risk::Credential => "credential",
        Risk::External => "external",
    }
}

// ── Estado pending_approval (primera clase, tabla `approvals` / migración 028) ─

/// Estado de una solicitud de aprobación.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

impl ApprovalStatus {
    fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Rejected => "rejected",
        }
    }
    fn parse(s: &str) -> ApprovalStatus {
        match s {
            "approved" => ApprovalStatus::Approved,
            "rejected" => ApprovalStatus::Rejected,
            _ => ApprovalStatus::Pending,
        }
    }
}

/// Fila persistida de un approval. NUNCA contiene secrets (BYOK): `args_json` lleva
/// args NO sensibles (incl. un *credential ref*, jamás la key).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Approval {
    pub id: String,
    pub command_id: String,
    pub args_json: String,
    pub status: ApprovalStatus,
    pub created_at: String,
    pub resolved_at: Option<String>,
    /// 015 T015 — ISO-8601 de cuándo este approval (aprobado) fue CONSUMIDO por una ejecución
    /// real. NON-NULL = ya usado (single-use, sin replay). `None` = aún consumible o no aprobado.
    pub consumed_at: Option<String>,
}

/// Resultado de pasar un comando por el gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GateOutcome {
    /// El comando es seguro: la superficie puede ejecutarlo ya.
    Allowed,
    /// El comando quedó pendiente de aprobación: se creó un `approval` pending y se
    /// emitió `ApprovalRequested`. La superficie NO debe ejecutar hasta la aprobación.
    PendingApproval { request_id: String },
}

/// PUERTA CENTRAL. Toda superficie (palette/botón/móvil/plugin/deeplink) llama acá
/// ANTES de ejecutar `command_id`. Consume el `risk` del registry; si requiere
/// aprobación crea el `approval` pending y deja al caller emitir `ApprovalRequested`
/// por el event bus (se devuelve el `request_id` para eso). Si es seguro, `Allowed`.
///
/// El backend NO ejecuta el comando acá — sólo decide. La superficie que recibe
/// `Allowed` procede; la que recibe `PendingApproval` espera la resolución humana.
pub fn gate(db: &Db, command_id: &str, args_json: &str) -> Result<GateOutcome> {
    let decision = check(command_id);
    if !decision.requires_approval {
        return Ok(GateOutcome::Allowed);
    }
    let request_id = create_pending(db, command_id, args_json)?;
    Ok(GateOutcome::PendingApproval { request_id })
}

/// Inserta un approval `pending` y devuelve su id (uuid). Estado de primera clase.
pub fn create_pending(db: &Db, command_id: &str, args_json: &str) -> Result<String> {
    // Audit codex US4 (MED): no crear un approval para un comando que NO existe en el
    // registry — un pending/approved de un command_id fantasma no es ejecutable y sólo
    // ensucia la tabla. (check() es fail-closed para riesgo; acá validamos existencia.)
    if !crate::services::command_registry::registry()
        .iter()
        .any(|c| c.id == command_id)
    {
        return Err(anyhow!(
            "comando desconocido '{command_id}': no se crea approval"
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    // 015 T015 — el binding del approval (consumo post-aprobación) usa el hash del valor REAL de los
    // args, computado ANTES de validar/redactar. Así el re-invoke (con los mismos args reales)
    // matchea, aunque lo que PERSISTIMOS sea redactado.
    let args_hash = canonical_args_hash(args_json);
    // Qué `args_json` PERSISTIMOS (display + audit). NUNCA el secret crudo:
    let args = if args_json.trim().is_empty() {
        "{}".to_string()
    } else {
        // Debe ser JSON (defensa: nunca persistir basura).
        serde_json::from_str::<serde_json::Value>(args_json)
            .map_err(|e| anyhow!("args_json inválido (debe ser JSON): {e}"))?;
        if is_credential_command(command_id) {
            // Audit codex T015 (HIGH, F-I BYOK): un comando Credential puede llevar un secret
            // CRUDO en los args (ej `signals_set_telegram_secret {secret:...}`) que el guardrail
            // (sólo patrones conocidos) NO caza → leak at-rest, o si lo caza queda INAPROBABLE.
            // Solución: REDACTAR los valores → en SQLite quedan sólo las claves. El secret real
            // nunca toca la tabla; el hash (irreversible) basta para el binding.
            redact_values(args_json)
        } else {
            // No-Credential (Destructive/requires_confirmation): los args son operacionales
            // (paths/ids) y se muestran en el confirm. Igual corremos el guardrail por si se cuela
            // un secret conocido (defensa; los secrets van por comandos Credential, no acá).
            let findings = crate::bases::guardrail::scan(args_json);
            if !findings.is_empty() {
                let kinds: Vec<&str> = findings.iter().map(|f| f.pattern_id).collect();
                return Err(anyhow!(
                    "args_json rechazado por el guardrail de secretos (BYOK): {kinds:?} — los secrets viven SÓLO en el Keychain, nunca en un approval"
                ));
            }
            args_json.to_string()
        }
    };
    let conn = db.lock();
    conn.execute(
        "INSERT INTO approvals (id, command_id, args_json, args_hash, status, created_at, resolved_at, consumed_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL, NULL)",
        rusqlite::params![id, command_id, args, args_hash, now],
    )?;
    Ok(id)
}

/// Lista los approvals (pendientes primero, luego por fecha desc).
pub fn list(db: &Db) -> Result<Vec<Approval>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, command_id, args_json, status, created_at, resolved_at, consumed_at
         FROM approvals
         ORDER BY (status = 'pending') DESC, created_at DESC",
    )?;
    let rows = stmt
        .query_map([], row_to_approval)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Mapeo fila→Approval (compartido por list/get). Orden de columnas:
/// id, command_id, args_json, status, created_at, resolved_at, consumed_at.
fn row_to_approval(r: &rusqlite::Row) -> rusqlite::Result<Approval> {
    Ok(Approval {
        id: r.get(0)?,
        command_id: r.get(1)?,
        args_json: r.get(2)?,
        status: ApprovalStatus::parse(&r.get::<_, String>(3)?),
        created_at: r.get(4)?,
        resolved_at: r.get(5)?,
        consumed_at: r.get(6)?,
    })
}

/// Resuelve un approval pending: `approved=true` → status `approved`; else `rejected`.
/// Sólo mueve approvals que están en `pending` (idempotente: re-resolver no hace nada).
/// Devuelve el approval actualizado (para que el caller sepa command_id/args si ya
/// puede ejecutar). Un approval `approved` es la SEÑAL para ejecutar el comando real.
pub fn resolve(db: &Db, id: &str, approved: bool) -> Result<Approval> {
    let now = chrono::Utc::now().to_rfc3339();
    let new_status = if approved {
        ApprovalStatus::Approved
    } else {
        ApprovalStatus::Rejected
    };
    {
        let conn = db.lock();
        let changed = conn.execute(
            "UPDATE approvals SET status = ?1, resolved_at = ?2
             WHERE id = ?3 AND status = 'pending'",
            rusqlite::params![new_status.as_str(), now, id],
        )?;
        if changed == 0 {
            // O no existe, o ya estaba resuelto. Diferenciamos para un error claro.
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM approvals WHERE id = ?1",
                    rusqlite::params![id],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            return Err(if exists {
                anyhow!("approval {id} ya estaba resuelto")
            } else {
                anyhow!("approval {id} no existe")
            });
        }
    }
    get(db, id)
}

/// Lee un approval por id.
pub fn get(db: &Db, id: &str) -> Result<Approval> {
    let conn = db.lock();
    conn.query_row(
        "SELECT id, command_id, args_json, status, created_at, resolved_at, consumed_at
         FROM approvals WHERE id = ?1",
        rusqlite::params![id],
        row_to_approval,
    )
    .map_err(|e| anyhow!("approval {id}: {e}"))
}

/// 015 T015 — CONSUMO del approval (paso final de approve→execute). Busca un approval CONSUMIBLE
/// para `(command_id, args_hash(args_json))` — `status='approved'`, `consumed_at IS NULL`, dentro
/// del TTL — y lo marca consumido ATÓMICAMENTE. Devuelve `Ok(Some(approval))` si se consumió uno
/// (el caller delega al comando real), `Ok(None)` si no hay ninguno consumible (el caller crea un
/// pending nuevo y rechaza). Single-use: el `UPDATE ... WHERE consumed_at IS NULL` garantiza que
/// dos invokes en carrera consuman a lo sumo UNO (el segundo ve 0 filas → None).
pub fn consume_approved(db: &Db, command_id: &str, args_json: &str) -> Result<Option<Approval>> {
    let args_hash = canonical_args_hash(args_json);
    let now = chrono::Utc::now();
    let now_rfc = now.to_rfc3339();
    // TTL anclado en `resolved_at` (el momento de la APROBACIÓN), NO en `created_at` (audit T015,
    // gemini MED): el usuario tiene APPROVAL_TTL_SECS para ejecutar DESPUÉS de aprobar. Anclar en
    // created_at causaría un deadlock si el humano tarda >TTL en leer el DangerZone y aprobar (el
    // approval nacería ya vencido → loop aprobar→re-invoke→vencido→nuevo pending).
    let min_resolved = (now - chrono::Duration::seconds(APPROVAL_TTL_SECS)).to_rfc3339();
    let conn = db.lock();
    // Elegimos el candidato más reciente y válido (dentro del TTL desde la aprobación), y lo
    // marcamos consumido en un único UPDATE atómico. Selección + UPDATE bajo el MISMO lock de
    // conexión → consumo serializado dentro del proceso (no hay dos ganadores).
    let candidate: Option<String> = conn
        .query_row(
            "SELECT id FROM approvals
             WHERE command_id = ?1 AND args_hash = ?2 AND status = 'approved'
               AND consumed_at IS NULL AND resolved_at >= ?3
             ORDER BY resolved_at DESC LIMIT 1",
            rusqlite::params![command_id, args_hash, min_resolved],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let Some(cand_id) = candidate else {
        return Ok(None);
    };
    let changed = conn.execute(
        "UPDATE approvals SET consumed_at = ?2 WHERE id = ?1 AND consumed_at IS NULL",
        rusqlite::params![cand_id, now_rfc],
    )?;
    if changed == 1 {
        let approval = conn.query_row(
            "SELECT id, command_id, args_json, status, created_at, resolved_at, consumed_at
             FROM approvals WHERE id = ?1",
            rusqlite::params![cand_id],
            row_to_approval,
        )?;
        Ok(Some(approval))
    } else {
        // Otro invoke lo consumió entre el SELECT y el UPDATE (carrera) → este no gana.
        Ok(None)
    }
}

// ── Enforcement UNIVERSAL del dispatch (015 T015) ─────────────────────────────
//
// El interceptor del `invoke_handler` (lib.rs) llama `dispatch_gate` para CADA invoke ANTES de
// delegar al comando real. Así el gate aplica venga de donde venga (palette/botón/plugin/móvil/
// deeplink), no sólo de las superficies que opten. El núcleo de la decisión vive acá (testeable
// sin Tauri); la glue (leer command/payload del Invoke, rechazar, emitir el evento) vive en lib.rs.

/// Conjunto de command ids GATEADOS (Destructive/Credential/requires_confirmation), derivado del
/// Command Registry (US1) UNA vez. Lookup O(1) en el hot-path del dispatch; los Safe/External ni
/// se consultan (costo ~0). Se reconstruye sólo al reiniciar (la lista de comandos es estática).
static GATED: Lazy<HashSet<String>> = Lazy::new(|| {
    // 027 F1 — derivado del motor de política (fuente única), NO de la fórmula inline. Con custom
    // OFF reproduce el mismo set que antes (cero-regresión); centralizar la regla en `policy` evita
    // que `check()`, `GATED` y las reglas custom (F2) diverjan.
    command_registry::registry()
        .into_iter()
        .filter(|c| {
            policy::evaluate_default(&policy::RequestContext::for_command(&c.id))
                .decision
                .requires_approval()
        })
        .map(|c| c.id.to_string())
        .collect()
});

/// Conjunto de TODOS los command ids del registry, O(1) lookup (audit deepseek F1: evita el O(n)
/// por dispatch en el path de comandos desconocidos → sin DoS leve por spam de comandos fantasma).
/// Estático: la lista de comandos es compile-time.
static KNOWN: Lazy<HashSet<String>> = Lazy::new(|| {
    command_registry::registry()
        .into_iter()
        .map(|c| c.id.to_string())
        .collect()
});

/// ¿el `command_id` existe en el Command Registry tipado? O(1).
fn is_known_command(command_id: &str) -> bool {
    KNOWN.contains(command_id)
}

/// Comandos que DEBEN saltear el gate aunque el registry los marque gateados — sino, deadlock:
/// `approval_resolve` lleva `requires_confirmation=true` (entra en GATED), pero APROBAR no puede
/// requerir aprobación. `approval_list`/`capability_check`/`health` son Safe (ya no gateados),
/// pero los listamos explícito como defensa: el flujo de aprobación SIEMPRE debe ser alcanzable.
///
/// 027 F1 (audit codex BLOCKER): `command_registry_list` es el ÚNICO handler real de
/// `generate_handler!` que está DELIBERADAMENTE ausente del registry (es infra del propio registry;
/// el test `registry_covers_all_handler_commands` lo auto-excluye). Sin esta entrada, el fail-closed
/// de F1 para "desconocido" lo trataría como gateado → `create_pending` lo rechaza → rompería
/// Command Palette / Help / GlobalApprovalModal, que lo invocan. Es Safe (read-only, lista comandos)
/// y DEBE ser alcanzable → excepción explícita y testeada (`f1_command_registry_list_passes_gate`).
pub fn is_bypass(command_id: &str) -> bool {
    matches!(
        command_id,
        "approval_resolve"
            | "approval_list"
            | "capability_check"
            | "health"
            | "command_registry_list"
    )
}

/// ¿este comando pasa por el gate del dispatch? (gateado por política y NO en la bypass-list).
///
/// 027 F1 (audit codex, fail-open cerrado): un comando DESCONOCIDO (ausente del registry) también
/// se considera gateado → llega a `dispatch_gate` → `create_pending` lo rechaza (comando
/// desconocido) → el invoke se rechaza fail-closed, en vez de fast-pathear a `generated()` sin
/// aprobación. Antes, "no está en GATED" trataba a lo desconocido como no-gateado (fail-OPEN).
/// El test 1:1 registry↔handlers hace esto inalcanzable hoy, pero la defensa en capas cierra el
/// hueco estructural ante un registry stale.
pub fn is_gated_for_dispatch(command_id: &str) -> bool {
    if is_bypass(command_id) {
        return false;
    }
    GATED.contains(command_id) || !is_known_command(command_id)
}

/// Decisión del interceptor del dispatch para un invoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Delegar al handler real: comando no gateado/bypass, o un approval que se acaba de CONSUMIR
    /// (ejecución autorizada).
    Pass,
    /// Cortar: se creó un `approval` pending. El caller rechaza el invoke con este payload y emite
    /// `AppEvent::ApprovalRequested`. El comando NO se ejecuta hasta que el humano aprueba y el
    /// front re-invoca (consumo).
    Pending {
        request_id: String,
        command_id: String,
        risk: String,
    },
    /// 027 F2-wiring: una regla custom de política DENEGÓ el comando (hardening `deny`). El comando
    /// NO se ejecuta y NO hay aprobación posible — el caller rechaza el invoke. Es terminal.
    Denied {
        command_id: String,
        rule_id: String,
    },
}

/// 027 F2-wiring — decisión EFECTIVA del gate para un comando (default + reglas custom si están
/// habilitadas). Con `policy.custom_enabled` OFF (default) replica EXACTAMENTE el gate estático
/// (cero overhead extra de DB salvo la lectura del flag); con ON evalúa las reglas custom
/// hardening-only sobre el default. Fail-safe: si la carga de reglas falla, se usa sólo el default
/// (restrictivo), nunca se relaja.
fn effective_decision(db: &Db, command_id: &str) -> (policy::Decision, Option<String>) {
    if !policy::store::custom_enabled(db) {
        // Path estático (= comportamiento de hoy): gateado ⇒ RequireApproval, sino Allow.
        let d = if is_gated_for_dispatch(command_id) {
            policy::Decision::RequireApproval
        } else {
            policy::Decision::Allow
        };
        return (d, None);
    }
    // Fail-safe (audit deepseek/AIE F2-wiring): si la carga de reglas falla, se usa SÓLO el default
    // (restrictivo) — nunca se relaja — pero se LOGUEA para no esconder un problema de storage.
    let rules = policy::store::list_enabled_valid(db).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "policy: no se pudieron cargar las reglas custom; usando sólo el default");
        Vec::new()
    });
    let ev = policy::evaluate(
        &policy::RequestContext::for_command(command_id),
        true,
        &rules,
    );
    let rule_id = if ev.applied_rule.source == policy::RuleSource::Custom {
        // El id de la AppliedRule viene como "custom:<id>"; extraer el id crudo.
        Some(
            ev.applied_rule
                .id
                .strip_prefix("custom:")
                .unwrap_or(&ev.applied_rule.id)
                .to_string(),
        )
    } else {
        None
    };
    (ev.decision, rule_id)
}

/// NÚCLEO del enforcement universal (T015 + 027 F2-wiring). Para `(command_id, args_json)`:
///   - bypass → `Pass` (el flujo de aprobación SIEMPRE alcanzable; las reglas custom NO tocan bypass).
///   - decisión efectiva `Allow` → `Pass`.
///   - decisión efectiva `Deny` (sólo con custom ON) → `Denied` (terminal, no ejecuta, sin aprobación).
///   - decisión efectiva `RequireApproval`/`RequireNApprovals`:
///       - hay un approval CONSUMIBLE (mismo command_id+args_hash, dentro del TTL) → consume → `Pass`.
///       - no hay → crea `pending` → `Pending`. Fail-closed: si `create_pending` rechaza (comando
///         desconocido, args no-JSON, secret en args), el error se propaga y el caller NO ejecuta.
pub fn dispatch_gate(db: &Db, command_id: &str, args_json: &str) -> Result<GateDecision> {
    // El flujo de aprobación (approval_resolve, etc.) NUNCA se gatea (deadlock) — ni por custom.
    if is_bypass(command_id) {
        return Ok(GateDecision::Pass);
    }
    let (decision, rule_id) = effective_decision(db, command_id);
    match decision {
        policy::Decision::Allow => Ok(GateDecision::Pass),
        policy::Decision::Deny => Ok(GateDecision::Denied {
            command_id: command_id.to_string(),
            rule_id: rule_id.unwrap_or_default(),
        }),
        policy::Decision::RequireApproval | policy::Decision::RequireNApprovals(_) => {
            if consume_approved(db, command_id, args_json)?.is_some() {
                return Ok(GateDecision::Pass);
            }
            let request_id = create_pending(db, command_id, args_json)?;
            Ok(GateDecision::Pending {
                request_id,
                command_id: command_id.to_string(),
                risk: check(command_id).risk,
            })
        }
    }
}

// ── Secret / Context provider (constitución F-I BYOK) ─────────────────────────
//
// Contrato:
//   - El FRONT nunca ve una key. Cuando un comando External/Credential necesita una
//     credencial, despacha un *credential ref*: un `String` id = nombre del entry del
//     Keychain (en la práctica, `provider_credentials.key_ref` / el alias del provider).
//   - El BACKEND resuelve la key SÓLO al ejecutar, contra el Keychain nativo
//     (`keychain::load`), y la usa in-process (ej. como header `Authorization`). La key
//     NO se devuelve a Tauri/IPC, NO se persiste en SQLite, NO se loguea, NO entra en
//     `args_json` de un approval.
//
// Ejemplo (comando External que necesita un token):
//   1. front: invoke("some_external_cmd", { credentialRef: "anthropic-personal", ... })
//   2. gate: External no requiere aprobación per se, pero si el comando es Credential o
//      lleva requires_confirmation, pasa por el approval primero.
//   3. backend, al ejecutar: `let key = secret::resolve("anthropic-personal")?;`
//      → arma `Authorization: Bearer {key}` y hace el request. `key` muere con el scope.
pub mod secret {
    use super::*;
    use crate::services::keychain;

    /// Servicio del Keychain bajo el que viven las credenciales BYOK del provider.
    /// Reusamos el mismo namespace que `providers.rs` (entry: service=furx-provider,
    /// account=credential ref/alias).
    pub const SERVICE: &str = keychain::SERVICE_PROVIDER;

    /// Resuelve la key a partir de un *credential ref* (nombre del entry del Keychain).
    /// SÓLO el backend llama esto, en el momento de ejecutar. NUNCA se expone al front.
    ///
    /// Devuelve `Err` si no hay key para ese ref (fail-closed: mejor fallar que ejecutar
    /// un comando que necesitaba auth sin auth).
    pub fn resolve(credential_ref: &str) -> Result<String> {
        let r = credential_ref.trim();
        if r.is_empty() {
            return Err(anyhow!("credential ref vacío"));
        }
        // Audit codex US4 (MED): el ref es un NOMBRE de entry del Keychain, no una key.
        // Validamos forma segura (alfanumérico + -_./@ acotado) para (a) defendernos si por
        // bug llegara una key como ref, (b) NUNCA echar el ref crudo en el error/logs (si
        // fuese una key, se filtraría). Errores genéricos sin el valor del ref.
        let safe = r.len() <= 128
            && r.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@'));
        if !safe {
            return Err(anyhow!(
                "credential ref con forma inválida (¿se mandó una key en vez de un nombre?)"
            ));
        }
        keychain::load(SERVICE, r)
            .ok_or_else(|| anyhow!("sin credencial en Keychain para el ref pedido"))
    }

    /// ¿Existe una credencial para este ref? Útil para el gate/UI sin tocar la key.
    /// Devuelve sólo un bool — la key jamás sale de acá.
    pub fn has(credential_ref: &str) -> bool {
        !credential_ref.trim().is_empty() && keychain::load(SERVICE, credential_ref).is_some()
    }
}

// ── Comandos Tauri ────────────────────────────────────────────────────────────
//
// Estos 3 comandos están registrados en command_registry.rs y en generate_handler!
// (lib.rs). Clasificación:
//   - capability_check: Safe / Internal (consulta pura, no muta).
//   - approval_list:    Safe / Palette  (lectura, ofrecible en la palette).
//   - approval_resolve: Safe / Palette pero requires_confirmation=true — ES la decisión
//     humana del gate; confirmarla es el punto, no auto-disparable.

/// US4 — ¿este comando requiere aprobación? Consulta pura (no crea approvals).
#[tauri::command]
pub fn capability_check(command_id: String) -> Result<CapabilityCheck, String> {
    Ok(check(&command_id))
}

/// US4 — lista los approvals (pendientes primero).
#[tauri::command]
pub fn approval_list(state: tauri::State<'_, crate::AppState>) -> Result<Vec<Approval>, String> {
    list(&state.db).map_err(|e| e.to_string())
}

/// US4 — resuelve un approval (la decisión humana). Aprobar = señal para ejecutar.
/// Emite `ApprovalRequested`? No: ese evento es de creación. La resolución la consume
/// el caller que esperaba (re-fetch via approval_list). Devuelve el approval actualizado.
#[tauri::command]
pub fn approval_resolve(
    state: tauri::State<'_, crate::AppState>,
    id: String,
    approved: bool,
) -> Result<Approval, String> {
    resolve(&state.db, &id, approved).map_err(|e| e.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::keychain;

    fn test_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/028_approvals.sql"))
            .unwrap();
        // 015 T015: args_hash + consumed_at (protocolo approve→execute).
        conn.execute_batch(include_str!("../../migrations/030_approval_consume.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// 027 F2-wiring: test_db + settings (002) + policy_rules (044) para los tests del gate con
    /// reglas custom.
    fn test_db_policy() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../migrations/028_approvals.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/030_approval_consume.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/002_settings.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/044_policy_custom_rules.sql"))
            .unwrap();
        Arc::new(parking_lot::Mutex::new(conn))
    }

    /// Encuentra un comando Safe (no gateado por el default) para los tests de hardening custom.
    fn a_safe_command() -> String {
        command_registry::registry()
            .into_iter()
            .find(|c| !is_gated_for_dispatch(&c.id) && !is_bypass(&c.id))
            .map(|c| c.id)
            .expect("hay algún comando Safe no gateado")
    }

    /// 027 F2-wiring: con custom OFF (default), el gate se comporta EXACTO que antes — un comando
    /// Safe pasa, sin tocar reglas custom.
    #[test]
    fn f2_custom_off_safe_command_passes() {
        let db = test_db_policy();
        let safe = a_safe_command();
        assert!(!policy::store::custom_enabled(&db));
        assert_eq!(dispatch_gate(&db, &safe, "{}").unwrap(), GateDecision::Pass);
    }

    /// 027 F2-wiring: con custom ON + una regla `deny` que matchea un comando Safe → `Denied`.
    #[test]
    fn f2_custom_rule_denies_safe_command() {
        let db = test_db_policy();
        let safe = a_safe_command();
        {
            let conn = db.lock();
            crate::settings::set(&conn, "policy.custom_enabled", &serde_json::Value::Bool(true))
                .unwrap();
        }
        policy::store::upsert(
            &db,
            &policy::CustomRule {
                id: "deny-safe".into(),
                description: "denegar".into(),
                match_command: Some(safe.clone()),
                match_risk: None,
                match_agent_profile: None,
                match_plugin: None,
                decision: policy::Decision::Deny,
            },
        )
        .unwrap();
        match dispatch_gate(&db, &safe, "{}").unwrap() {
            GateDecision::Denied { command_id, rule_id } => {
                assert_eq!(command_id, safe);
                assert_eq!(rule_id, "deny-safe");
            }
            other => panic!("esperaba Denied, obtuve {other:?}"),
        }
        // No creó approvals (denegar es terminal).
        assert!(list(&db).unwrap().is_empty());
    }

    /// 027 F2-wiring: con custom ON + una regla `require_approval` que matchea un comando Safe →
    /// `Pending` (lo endurece de pasar-directo a pedir aprobación).
    #[test]
    fn f2_custom_rule_hardens_safe_to_pending() {
        let db = test_db_policy();
        let safe = a_safe_command();
        {
            let conn = db.lock();
            crate::settings::set(&conn, "policy.custom_enabled", &serde_json::Value::Bool(true))
                .unwrap();
        }
        policy::store::upsert(
            &db,
            &policy::CustomRule {
                id: "harden-safe".into(),
                description: "endurecer".into(),
                match_command: Some(safe.clone()),
                match_risk: None,
                match_agent_profile: None,
                match_plugin: None,
                decision: policy::Decision::RequireApproval,
            },
        )
        .unwrap();
        assert!(matches!(
            dispatch_gate(&db, &safe, "{}").unwrap(),
            GateDecision::Pending { .. }
        ));
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    /// 027 F2-wiring: con custom ON pero NINGUNA regla matchea, un comando Safe sigue pasando
    /// (cero falsos positivos — sólo endurece lo que el usuario pidió).
    #[test]
    fn f2_custom_on_no_match_safe_passes() {
        let db = test_db_policy();
        let safe = a_safe_command();
        {
            let conn = db.lock();
            crate::settings::set(&conn, "policy.custom_enabled", &serde_json::Value::Bool(true))
                .unwrap();
        }
        policy::store::upsert(
            &db,
            &policy::CustomRule {
                id: "otro-comando".into(),
                description: String::new(),
                match_command: Some("comando_que_no_es_el_safe".into()),
                match_risk: None,
                match_agent_profile: None,
                match_plugin: None,
                decision: policy::Decision::Deny,
            },
        )
        .unwrap();
        assert_eq!(dispatch_gate(&db, &safe, "{}").unwrap(), GateDecision::Pass);
    }

    // ── Gate: consume el risk del registry ──

    #[test]
    fn safe_command_passes_gate() {
        // `list_panes` es Safe/sin confirmación en el registry.
        let c = check("list_panes");
        assert!(!c.requires_approval, "Safe no debería requerir aprobación");
        assert_eq!(c.risk, "safe");
        assert!(!c.unknown);
    }

    #[test]
    fn destructive_command_requires_approval() {
        // `reset_furx` es Destructive en el registry.
        let c = check("reset_furx");
        assert!(c.requires_approval, "Destructive DEBE requerir aprobación");
        assert_eq!(c.risk, "destructive");
    }

    #[test]
    fn credential_command_requires_approval() {
        // `mobile_secret_get` es Credential en el registry.
        let c = check("mobile_secret_get");
        assert!(c.requires_approval, "Credential DEBE requerir aprobación");
        assert_eq!(c.risk, "credential");
    }

    #[test]
    fn review_flow_actions_are_gated_by_exception() {
        // 019 F0 · T003 — boundary del gate por excepción cableado al flujo review:
        //   - review_hunk_decide (approve/reject) → gateado por requires_confirmation=true
        //     (pending + ApprovalRequested → consumo single-use), aunque su Risk sea Safe.
        //   - review_apply → Destructive → gateado (modifica el working copy).
        //   - orchestration_cancel (el kill-switch del attempt) → Destructive → gateado.
        // Las lecturas del flujo NO se gatean (no son acciones de riesgo).
        assert!(
            check("review_hunk_decide").requires_approval,
            "approve/reject debe pasar por aprobación por excepción (FR-007)"
        );
        assert!(
            check("review_apply").requires_approval,
            "apply (Destructive) debe pasar por el gate"
        );
        assert!(
            check("orchestration_cancel").requires_approval,
            "el kill-switch (Destructive) debe pasar por el gate"
        );
        // Lecturas del flujo: sin aprobación.
        assert!(!check("review_get").requires_approval);
        assert!(!check("review_conflicts").requires_approval);
    }

    #[test]
    fn unknown_command_fails_closed() {
        let c = check("does_not_exist_xyz");
        assert!(c.requires_approval, "comando desconocido → fail-closed");
        assert!(c.unknown);
        assert_eq!(c.risk, "unknown");
    }

    #[test]
    fn create_pending_rejects_unknown_command() {
        // Audit codex US4: no se crea un approval para un comando fantasma.
        let db = test_db();
        assert!(create_pending(&db, "does_not_exist_xyz", "{}").is_err());
        // un comando real Credential sí crea pending.
        assert!(create_pending(&db, "approval_resolve", "{}").is_ok());
    }

    #[test]
    fn create_pending_guardrail_rejects_secret_in_args() {
        // Audit codex US4 (F-I BYOK): args_json con un secret detectable se rechaza,
        // NUNCA queda at-rest en SQLite.
        let db = test_db();
        let r = create_pending(
            &db,
            "approval_resolve",
            r#"{"api_key":"sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"}"#,
        );
        assert!(
            r.is_err(),
            "el guardrail debe rechazar un secret en args_json"
        );
    }

    #[test]
    fn credential_ref_with_key_shape_is_rejected_and_not_echoed() {
        // Audit codex US4: un ref con forma de key (caracteres fuera del set seguro) se
        // rechaza; el error NO contiene el valor del ref (no leak a logs/IPC).
        let bad = "sk-proj-ABC+DEF/GHI=secret";
        let err = secret::resolve(bad).unwrap_err().to_string();
        assert!(!err.contains(bad), "el error NO debe echar el ref/key");
    }

    #[test]
    fn requires_confirmation_safe_still_gates() {
        // approval_resolve está clasificado Safe pero requires_confirmation=true:
        // debe requerir aprobación aunque su risk sea safe.
        let c = check("approval_resolve");
        assert_eq!(c.risk, "safe");
        assert!(
            c.requires_approval,
            "requires_confirmation=true debe forzar aprobación aun siendo Safe"
        );
    }

    // ── Gate end-to-end: bloquea Destructive/Credential sin aprobación ──

    #[test]
    fn gate_blocks_destructive_until_approved() {
        let db = test_db();
        // Destructive → PendingApproval (NO Allowed → NO se ejecuta).
        let outcome = gate(&db, "reset_furx", "{}").unwrap();
        let request_id = match outcome {
            GateOutcome::PendingApproval { request_id } => request_id,
            GateOutcome::Allowed => panic!("Destructive NO debe pasar directo"),
        };
        // Estado pending de primera clase persistido.
        let pending = list(&db).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, ApprovalStatus::Pending);
        assert_eq!(pending[0].command_id, "reset_furx");
        // Aprobación humana → approved.
        let updated = resolve(&db, &request_id, true).unwrap();
        assert_eq!(updated.status, ApprovalStatus::Approved);
        assert!(updated.resolved_at.is_some());
    }

    #[test]
    fn gate_allows_safe_directly() {
        let db = test_db();
        let outcome = gate(&db, "list_panes", "{}").unwrap();
        assert_eq!(outcome, GateOutcome::Allowed);
        // No se creó ningún approval para un Safe.
        assert!(list(&db).unwrap().is_empty());
    }

    #[test]
    fn rejecting_an_approval_marks_rejected() {
        let db = test_db();
        let id = create_pending(&db, "crash_log_clear", "{}").unwrap();
        let updated = resolve(&db, &id, false).unwrap();
        assert_eq!(updated.status, ApprovalStatus::Rejected);
        assert!(updated.resolved_at.is_some());
    }

    #[test]
    fn resolve_is_idempotent_per_pending() {
        let db = test_db();
        let id = create_pending(&db, "reset_furx", "{}").unwrap();
        resolve(&db, &id, true).unwrap();
        // Re-resolver un approval ya resuelto falla (no re-muta).
        assert!(resolve(&db, &id, false).is_err());
    }

    #[test]
    fn create_pending_rejects_non_json_args() {
        let db = test_db();
        // Defensa: args_json debe ser JSON; nunca persistir un secret crudo como args.
        assert!(create_pending(&db, "reset_furx", "sk-live-SECRET-not-json").is_err());
    }

    // ── Secret provider: la key SÓLO la ve el backend, jamás el ref/front ──

    #[test]
    fn secret_provider_keeps_key_out_of_ref() {
        // PID-unique para no colisionar con el Keychain real compartido entre tests.
        let pid = std::process::id();
        let credential_ref = format!("furx-test-cap-ref-{pid}");
        let secret_value = "sk-super-secret-token-value";

        // Backend guarda la key en el Keychain bajo el ref.
        let _ = keychain::delete(secret::SERVICE, &credential_ref);
        keychain::save(secret::SERVICE, &credential_ref, secret_value).unwrap();

        // 1) El ref que viajaría al/desde el front NO contiene la key.
        assert!(
            !credential_ref.contains(secret_value),
            "el credential ref jamás debe contener la key"
        );

        // 2) `has` informa existencia con un bool — sin exponer la key.
        assert!(secret::has(&credential_ref));

        // 3) SÓLO el backend, al ejecutar, resuelve la key real contra el Keychain.
        let resolved = secret::resolve(&credential_ref).unwrap();
        assert_eq!(resolved, secret_value);

        // 4) Un approval persistido por un comando Credential NUNCA lleva la key (T015: ni el ref
        //    crudo — los VALORES se redactan; sólo quedan las claves). El secret no toca SQLite.
        let db = test_db();
        let args = format!("{{\"credential_ref\":\"{credential_ref}\"}}");
        let approval_id = create_pending(&db, "mobile_secret_get", &args).unwrap();
        let row = get(&db, &approval_id).unwrap();
        assert!(
            !row.args_json.contains(secret_value),
            "args_json de un approval NUNCA debe contener la key"
        );
        // T015 (audit codex HIGH): los VALORES de un comando Credential se redactan — ni el ref
        // crudo queda at-rest. La CLAVE sí (para la UI). El binding usa el args_hash (irreversible).
        assert!(
            !row.args_json.contains(&credential_ref),
            "el valor (ref) debe estar redactado"
        );
        assert!(
            row.args_json.contains("credential_ref"),
            "la clave sí se conserva (display)"
        );
        assert!(row.args_json.contains("redacted"));

        let _ = keychain::delete(secret::SERVICE, &credential_ref);
    }

    #[test]
    fn secret_resolve_fails_closed_when_missing() {
        let pid = std::process::id();
        let missing_ref = format!("furx-test-cap-absent-{pid}");
        let _ = keychain::delete(secret::SERVICE, &missing_ref);
        assert!(secret::resolve(&missing_ref).is_err());
        assert!(!secret::has(&missing_ref));
        assert!(secret::resolve("").is_err());
    }

    // ── 015 T015 — enforcement universal del dispatch + approve→execute ──

    #[test]
    fn t015_canonical_hash_is_order_independent_and_args_sensitive() {
        // Mismo contenido, distinto orden de claves → MISMO hash.
        assert_eq!(
            canonical_args_hash(r#"{"a":1,"b":2}"#),
            canonical_args_hash(r#"{"b":2,"a":1}"#)
        );
        // Anidado también canónico.
        assert_eq!(
            canonical_args_hash(r#"{"x":{"a":1,"b":2}}"#),
            canonical_args_hash(r#"{"x":{"b":2,"a":1}}"#)
        );
        // Args distintos → hash distinto (binding real).
        assert_ne!(
            canonical_args_hash(r#"{"path":"/safe"}"#),
            canonical_args_hash(r#"{"path":"/etc/passwd"}"#)
        );
        // Vacío == {}.
        assert_eq!(canonical_args_hash(""), canonical_args_hash("{}"));
    }

    #[test]
    fn t015_dispatch_gate_passes_safe_and_bypass() {
        let db = test_db();
        // Safe → Pass, sin crear pending.
        assert_eq!(
            dispatch_gate(&db, "list_panes", "{}").unwrap(),
            GateDecision::Pass
        );
        // approval_resolve es requires_confirmation=true (entra en GATED) pero está en la
        // bypass-list → Pass (sino, deadlock: aprobar requeriría aprobar).
        assert!(is_bypass("approval_resolve"));
        assert!(GATED.contains("approval_resolve"));
        assert_eq!(
            dispatch_gate(&db, "approval_resolve", "{}").unwrap(),
            GateDecision::Pass
        );
        // Nada de esto creó approvals.
        assert!(list(&db).unwrap().is_empty());
    }

    /// 027 F1 (audit codex, fail-open cerrado): un comando DESCONOCIDO nunca fast-pathea a Pass.
    #[test]
    fn f1_dispatch_gate_unknown_command_never_passes() {
        let db = test_db();
        let unknown = "comando_fantasma_que_no_existe_999";
        // Se considera gateado (defensa en capas), NO bypass.
        assert!(is_gated_for_dispatch(unknown));
        assert!(!is_bypass(unknown));
        // En el dispatch, el desconocido NO produce Pass: `create_pending` lo rechaza → Err
        // (fail-closed), nunca `GateDecision::Pass`.
        let dec = dispatch_gate(&db, unknown, "{}");
        assert!(
            dec.is_err(),
            "comando desconocido debe fallar el gate (fail-closed), no pasar"
        );
        // Y no ensució la tabla de approvals.
        assert!(list(&db).unwrap().is_empty());
    }

    /// 027 F1 (audit codex BLOCKER, regresión cazada): `command_registry_list` es un handler real
    /// ausente del registry a propósito (infra). El fail-closed de F1 NO debe romperlo: está en la
    /// bypass-list → dispatch Pass (lo necesitan palette/help/approval-modal).
    #[test]
    fn f1_command_registry_list_passes_gate() {
        let db = test_db();
        // No está en el registry tipado...
        assert!(!is_known_command("command_registry_list"));
        // ...pero está en la bypass-list (excepción explícita) → NO se gatea.
        assert!(is_bypass("command_registry_list"));
        assert!(!is_gated_for_dispatch("command_registry_list"));
        // Y el dispatch lo deja pasar (no lo rechaza fail-closed como a un fantasma).
        assert_eq!(
            dispatch_gate(&db, "command_registry_list", "{}").unwrap(),
            GateDecision::Pass
        );
        assert!(list(&db).unwrap().is_empty());
    }

    /// 027 F1: el GATED derivado del motor coincide con la fórmula legacy para todo el registry
    /// (cero-regresión del set de gateados).
    #[test]
    fn f1_gated_set_matches_legacy_formula() {
        for c in command_registry::registry() {
            let legacy = matches!(c.risk, Risk::Destructive | Risk::Credential)
                || c.requires_confirmation;
            assert_eq!(
                GATED.contains(&c.id),
                legacy,
                "GATED divergió para {} (motor vs fórmula legacy)",
                c.id
            );
        }
    }

    #[test]
    fn t015_dispatch_gate_blocks_destructive_then_consume_then_no_replay() {
        let db = test_db();
        // 1er invoke de un Destructive → Pending (crea approval, NO ejecuta).
        let dec = dispatch_gate(&db, "reset_furx", "{}").unwrap();
        let request_id = match dec {
            GateDecision::Pending { request_id, .. } => request_id,
            other => panic!("Destructive sin approval debe ser Pending, no {other:?}"),
        };
        assert_eq!(list(&db).unwrap().len(), 1);
        // Humano aprueba.
        resolve(&db, &request_id, true).unwrap();
        // Re-invoke con los MISMOS args → consume el approval → Pass (ejecución autorizada).
        assert_eq!(
            dispatch_gate(&db, "reset_furx", "{}").unwrap(),
            GateDecision::Pass
        );
        // El approval quedó consumido (single-use).
        assert!(get(&db, &request_id).unwrap().consumed_at.is_some());
        // 2do re-invoke (replay) → ya no hay approval consumible → Pending NUEVO (no se re-ejecuta
        // con una aprobación vieja). El request_id es DISTINTO.
        let dec2 = dispatch_gate(&db, "reset_furx", "{}").unwrap();
        match dec2 {
            GateDecision::Pending {
                request_id: rid2, ..
            } => assert_ne!(rid2, request_id),
            other => panic!("replay NO debe autorizar una 2da ejecución, fue {other:?}"),
        }
    }

    #[test]
    fn t015_consume_requires_matching_args() {
        let db = test_db();
        // pending+approved para unos args.
        let id = create_pending(&db, "reset_furx", r#"{"scope":"cache"}"#).unwrap();
        resolve(&db, &id, true).unwrap();
        // Intentar ejecutar con OTROS args → no consume (anti bait-and-switch) → Pending nuevo.
        let dec = dispatch_gate(&db, "reset_furx", r#"{"scope":"all"}"#).unwrap();
        assert!(matches!(dec, GateDecision::Pending { .. }));
        // El approval original sigue sin consumir.
        assert!(get(&db, &id).unwrap().consumed_at.is_none());
        // Con los args correctos sí consume.
        assert_eq!(
            dispatch_gate(&db, "reset_furx", r#"{"scope":"cache"}"#).unwrap(),
            GateDecision::Pass
        );
        assert!(get(&db, &id).unwrap().consumed_at.is_some());
    }

    #[test]
    fn t015_consume_respects_ttl() {
        let db = test_db();
        let id = create_pending(&db, "reset_furx", "{}").unwrap();
        resolve(&db, &id, true).unwrap();
        // Envejecemos la APROBACIÓN (resolved_at) más allá del TTL — el TTL se ancla ahí (no en
        // created_at), para no deadlockear si el humano tarda en aprobar.
        let old =
            (chrono::Utc::now() - chrono::Duration::seconds(APPROVAL_TTL_SECS + 60)).to_rfc3339();
        db.lock()
            .execute(
                "UPDATE approvals SET resolved_at = ?1 WHERE id = ?2",
                rusqlite::params![old, id],
            )
            .unwrap();
        // Aprobado pero VENCIDO → no consumible.
        assert!(consume_approved(&db, "reset_furx", "{}").unwrap().is_none());
        let dec = dispatch_gate(&db, "reset_furx", "{}").unwrap();
        assert!(
            matches!(dec, GateDecision::Pending { .. }),
            "approval vencido no autoriza"
        );
    }

    #[test]
    fn t015_consume_is_single_winner_under_repeat() {
        let db = test_db();
        let id = create_pending(&db, "reset_furx", "{}").unwrap();
        resolve(&db, &id, true).unwrap();
        // Dos consumos seguidos del MISMO approval: el 1ro gana, el 2do ve None (sin replay).
        assert!(consume_approved(&db, "reset_furx", "{}").unwrap().is_some());
        assert!(consume_approved(&db, "reset_furx", "{}").unwrap().is_none());
    }

    #[test]
    fn t015_gated_set_is_built_from_registry() {
        // Destructive/Credential/requires_confirmation entran; Safe (sin confirm) no.
        assert!(GATED.contains("reset_furx")); // Destructive
        assert!(GATED.contains("mobile_secret_get")); // Credential (devuelve un secret)
        assert!(!GATED.contains("list_panes")); // Safe
                                                // Audit codex MED: un READ de metadata (sin secrets) que el front llama on-load NO debe
                                                // estar gateado (sino pediría aprobación en cada carga). Reclasificado a Safe.
        assert!(!GATED.contains("claude_accounts_list"));
    }

    // ── 015 T015 — fixes de auditoría (BYOK redaction, fail-closed, TTL-on-resolved) ──

    /// Audit codex HIGH (BYOK): los VALORES de un comando Credential se redactan al persistir (el
    /// secret nunca toca SQLite), PERO el binding usa el hash del valor REAL → el consumo con los
    /// args reales sigue matcheando. `mobile_secret_get` es Credential.
    #[test]
    fn t015_credential_args_redacted_but_consume_still_matches() {
        let db = test_db();
        let real_args = r#"{"credential_ref":"super-secret-token-value"}"#;
        let id = create_pending(&db, "mobile_secret_get", real_args).unwrap();
        // Persistido SIN el valor (redactado).
        let stored = get(&db, &id).unwrap().args_json;
        assert!(
            !stored.contains("super-secret-token-value"),
            "el valor no debe quedar at-rest"
        );
        assert!(stored.contains("credential_ref"), "la clave sí (display)");
        // Pero el consumo con los args REALES matchea (hash del valor real).
        resolve(&db, &id, true).unwrap();
        assert!(consume_approved(&db, "mobile_secret_get", real_args)
            .unwrap()
            .is_some());
    }

    /// Audit AIE/deepseek MED (fail-closed): un input que NO parsea como JSON produce un hash
    /// DISTINTO al de `{}` — así no puede matchear un approval de `{}` y ejecutar sin args reales.
    #[test]
    fn t015_unparseable_args_hash_is_fail_closed() {
        assert_ne!(
            canonical_args_hash("no soy json"),
            canonical_args_hash("{}")
        );
        assert_ne!(canonical_args_hash("{roto"), canonical_args_hash(""));
        // dos inputs malformados DISTINTOS → hashes distintos (no colapsan al mismo).
        assert_ne!(
            canonical_args_hash("malformado-A"),
            canonical_args_hash("malformado-B")
        );
    }

    /// Audit gemini MED (TTL deadlock): el TTL se ancla en `resolved_at` (aprobación), NO en
    /// `created_at`. Un approval CREADO hace rato pero APROBADO recién es consumible; uno aprobado
    /// hace rato (más allá del TTL) no.
    #[test]
    fn t015_ttl_is_anchored_on_resolved_at() {
        let db = test_db();
        // Caso A: creado hace mucho, aprobado RECIÉN → consumible (no deadlock por deliberación).
        let id = create_pending(&db, "reset_furx", "{}").unwrap();
        let old_created =
            (chrono::Utc::now() - chrono::Duration::seconds(APPROVAL_TTL_SECS + 600)).to_rfc3339();
        db.lock()
            .execute(
                "UPDATE approvals SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![old_created, id],
            )
            .unwrap();
        resolve(&db, &id, true).unwrap(); // resolved_at = ahora
        assert!(
            consume_approved(&db, "reset_furx", "{}").unwrap().is_some(),
            "aprobado recién → consumible aunque se creó hace rato"
        );

        // Caso B: aprobado hace mucho (resolved_at viejo) → NO consumible.
        let id2 = create_pending(&db, "reset_furx", "{}").unwrap();
        resolve(&db, &id2, true).unwrap();
        let old_resolved =
            (chrono::Utc::now() - chrono::Duration::seconds(APPROVAL_TTL_SECS + 60)).to_rfc3339();
        db.lock()
            .execute(
                "UPDATE approvals SET resolved_at = ?1 WHERE id = ?2",
                rusqlite::params![old_resolved, id2],
            )
            .unwrap();
        assert!(
            consume_approved(&db, "reset_furx", "{}").unwrap().is_none(),
            "aprobado hace rato → vencido"
        );
    }
}
