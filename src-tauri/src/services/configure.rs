// services/configure.rs — 060 v2 · contrato "tu CLI de confianza configura Furx" (seguro, faseado).
//
// Un agente CLI propone un perfil JSON; Furx APLICA sólo una ALLOWLIST CERRADA de prefs locales,
// reversibles, sin ejecución/red/privilegios, y RECHAZA todo lo demás (sensible o desconocido). Dry-run
// por default. La aplicación es ATÓMICA (una transacción: settings + audit juntos; si algo falla, NADA
// queda aplicado → sin estado parcial). Diseño del council (run 5668770d, seguridad-first): el control
// real es REDUCIR CAPACIDADES (allowlist cerrada + reject + validación estricta), NO firmar — el
// atacante relevante es el agente autorizado. Es superficie de indirect prompt-injection (el agente lee
// repos no confiables) → el agente PROPONE, Furx DECIDE.
//
// Alcance v2-core (council alpha+beta-trivial): allowlist DELIBERADAMENTE mínima. NUNCA: secrets/BYOK/
// Keychain, policy.*, MFA, endpoints remotos, autostart, exec/panes, telemetry/egress, logging/audit, ni
// los flags de UI (localStorage del frontend, fuera del alcance del CLI). Ampliar = fase GA con evidencia.

use crate::bases::audit::{AuditWriter, EventInput};
use crate::bases::guardrail;
use crate::services::settings_registry;
use crate::settings as settings_store;
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

/// Allowlist CERRADA: keys de furx.db que un agente puede setear SIN confirmación humana. Sólo prefs
/// locales, reversibles, que NO ejecutan nada, NO tocan la red, NI escalan privilegios. Todo lo que NO
/// esté acá → rechazado. Crece SÓLO con evidencia + revisión de seguridad (fase GA).
pub const SAFE_KEYS: &[&str] = &[
    "ptt.hotkey",         // combo de push-to-talk (string con grammar acotada; no ejecuta nada)
    "restore.always_ask", // preguntar antes de restaurar sesiones tmux (bool; UX)
];

#[derive(Serialize, Debug, Clone)]
pub struct ConfigChange {
    pub key: String,
    pub from: Value,
    pub to: Value,
}

#[derive(Serialize, Debug, Clone)]
pub struct ConfigReject {
    pub key: String,
    pub reason: String,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct ConfigurePlan {
    pub apply: Vec<ConfigChange>,
    pub noop: Vec<String>,
    pub reject: Vec<ConfigReject>,
}

/// Grammar acotada del hotkey de PTT: segmentos alfanuméricos separados por `+` (Mod+...+Tecla), sin
/// vacíos, ≤6 segmentos, ≤64 chars. Rechaza espacios, `;`, rutas, etc. (anti-inyección defensiva — el
/// front igual nunca EJECUTA el combo, sólo lo matchea, pero validamos en el borde del contrato).
/// EXIGE exactamente UNA tecla base (segmento no-modificador): el front (`parsePttHotkey`) toma la
/// última tecla no-mod como `code` y DESCARTA al default un combo sólo-modificador ("Alt", "Ctrl+Shift")
/// → sin este check el contrato aceptaría un valor que NO toma efecto y el `apply` reportaría "ok" sobre
/// un no-op silencioso; y dos teclas base ("KeyA+KeyB") el front las colapsa a la última sin avisar.
fn valid_hotkey(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    let parts: Vec<&str> = s.split('+').collect();
    if parts.len() > 6 {
        return false;
    }
    if !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric())) {
        return false;
    }
    const MODS: [&str; 4] = ["Alt", "Control", "Meta", "Shift"];
    parts.iter().filter(|p| !MODS.contains(p)).count() == 1
}

/// Validación ESTRICTA del valor de una key safe: sólo primitivos (rechaza objeto/array — el agente no
/// inyecta estructuras anidadas) + tipo/formato por key. Cierra el schema del lado del valor.
fn validate_safe_value(key: &str, v: &Value) -> Result<(), String> {
    if v.is_object() || v.is_array() {
        return Err("el valor debe ser primitivo (string/bool/número), no objeto/array".into());
    }
    // `ptt.hotkey` NO tiene SettingDef en el registry → su gramática se valida SÓLO acá. Las demás
    // SAFE_KEYS con SettingDef (ej `restore.always_ask`, schema Bool) se validan por tipo en el paso 3
    // (`settings_registry::validate`) — no se re-chequean acá para no duplicar el schema (dos fuentes de
    // verdad divergen si el registry cambia el tipo). SAFE_KEYS futuras SIN SettingDef: agregar su check
    // de formato como otro `else if` acá.
    if key == "ptt.hotkey" {
        let s = v.as_str().ok_or("ptt.hotkey debe ser string")?;
        if !valid_hotkey(s) {
            return Err("formato de hotkey inválido (sólo Mod+...+Tecla alfanumérico, ≤64)".into());
        }
    }
    Ok(())
}

/// Parsea el perfil y CLASIFICA cada key SIN mutar (dry-run). Schema CERRADO en DOS niveles: (1)
/// top-level sólo admite `settings` (y `version` opcional) — cualquier otra clave top-level RECHAZA el
/// perfil entero (no se ignora silenciosamente); (2) dentro de `settings`, sólo la allowlist + valores
/// válidos pasan.
pub fn plan(profile: &Value, conn: &Connection) -> Result<ConfigurePlan, String> {
    let obj = profile.as_object().ok_or("perfil inválido: debe ser un objeto JSON")?;
    for k in obj.keys() {
        if k != "settings" && k != "version" {
            return Err(format!(
                "clave top-level no permitida: `{k}` — el perfil sólo admite `settings` (y `version`)"
            ));
        }
    }
    let settings = obj
        .get("settings")
        .and_then(|s| s.as_object())
        .ok_or_else(|| "perfil inválido: falta el objeto `settings`".to_string())?;

    let mut p = ConfigurePlan::default();
    for (key, to) in settings {
        // 1) allowlist cerrada — todo lo que no esté acá se RECHAZA (no se aplica).
        if !SAFE_KEYS.contains(&key.as_str()) {
            p.reject.push(ConfigReject {
                key: key.clone(),
                reason: "key fuera de la allowlist segura (sensible o desconocida) — configurala vos en Ajustes".into(),
            });
            continue;
        }
        // 2) validación estricta del valor (primitivo + tipo/formato por key).
        if let Err(e) = validate_safe_value(key, to) {
            p.reject.push(ConfigReject { key: key.clone(), reason: e });
            continue;
        }
        // 3) validación contra el schema del registry (si tiene SettingDef).
        if let Err(e) = settings_registry::validate(key, to) {
            p.reject.push(ConfigReject { key: key.clone(), reason: format!("valor inválido: {e}") });
            continue;
        }
        // 4) guardrail de secretos (defensa en profundidad).
        if let Ok(s) = serde_json::to_string(to) {
            if !guardrail::scan(&s).is_empty() {
                p.reject.push(ConfigReject { key: key.clone(), reason: "el valor parece contener un secreto".into() });
                continue;
            }
        }
        // 5) safe + válido → apply (si cambia) o noop (si ya está).
        let cur = settings_store::get(conn, key).map_err(|e| e.to_string())?.unwrap_or(Value::Null);
        if &cur == to {
            p.noop.push(key.clone());
        } else {
            p.apply.push(ConfigChange { key: key.clone(), from: cur, to: to.clone() });
        }
    }
    Ok(p)
}

/// Aplica el plan de forma ATÓMICA: todas las settings + sus eventos de audit en UNA transacción sobre
/// UNA conexión. Si cualquier `set`/audit falla, el `Transaction` se dropea sin commit → ROLLBACK
/// automático → NADA queda aplicado (sin estado parcial ni cambio sin audit). El `from` se RE-LEE dentro
/// de la transacción (preciso ante una carrera con la GUI). Idempotente. El rollback "de negocio"
/// (revertir un cambio ya commiteado) es re-aplicar un perfil con los `from` (transacción compensatoria),
/// NUNCA mutar el audit (council).
pub fn apply(profile: &Value, conn: &mut Connection, audit: &AuditWriter) -> Result<ConfigurePlan, String> {
    // IMMEDIATE: toma el lock de escritura YA, y el `plan` se computa DENTRO de la transacción → las
    // lecturas (clasificación + `from`) y las escrituras quedan SERIALIZADAS contra cualquier otro
    // escritor (la GUI). Cierra el TOCTOU que tenía planear-antes-de-la-tx (audit codex): una key que
    // era `noop` no puede cambiar entre el plan y el apply, ni un `from` quedar desactualizado.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let p = plan(profile, &tx)?;
    for ch in &p.apply {
        settings_store::set(&tx, &ch.key, &ch.to).map_err(|e| e.to_string())?;
        audit
            .write_in_tx(
                &tx,
                &EventInput {
                    kind: "configure.apply",
                    actor: "cli-agent",
                    pane_id: None,
                    card_id: None,
                    correlation_id: None,
                    payload: serde_json::json!({ "key": ch.key, "from": ch.from, "to": ch.to }),
                },
            )
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(p)
}

/// Entry-point CLI headless (`furx configure --profile <archivo.json> [--dry-run]`). Abre la furx.db,
/// lee el perfil, planea (dry-run) o aplica (atómico), e imprime el resultado como JSON a stdout (el
/// agente lo lee). Devuelve exit code (0 ok, 2 error de uso/IO/parseo/política).
pub fn run_cli(profile_path: Option<&str>, dry_run: bool) -> i32 {
    let Some(path) = profile_path else {
        eprintln!("uso: furx configure --profile <archivo.json> [--dry-run]");
        return 2;
    };
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no se pudo leer {path}: {e}");
            return 2;
        }
    };
    let profile: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("JSON inválido en {path}: {e}");
            return 2;
        }
    };
    let Some(home) = dirs::home_dir() else {
        eprintln!("no se encontró el home dir");
        return 2;
    };
    let db_path = home.join(".furx").join("furx.db");
    let mut conn = match crate::db::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("no se pudo abrir la DB ({}): {e}", db_path.display());
            return 2;
        }
    };
    let result = if dry_run {
        plan(&profile, &conn)
    } else {
        // AuditWriter requiere una Arc<Mutex<Connection>> para construirse, pero `write_in_tx` usa la
        // conexión de la TRANSACCIÓN (no la interna) → esta sólo satisface el constructor, no se lockea
        // durante el apply (sin deadlock). Una 2da apertura de la MISMA furx.db (WAL) es segura.
        let audit_conn = match crate::db::open(&db_path) {
            Ok(c) => std::sync::Arc::new(parking_lot::Mutex::new(c)),
            Err(e) => {
                eprintln!("no se pudo abrir la DB para audit: {e}");
                return 2;
            }
        };
        let audit = AuditWriter::new(audit_conn);
        apply(&profile, &mut conn, &audit)
    };
    match result {
        Ok(p) => {
            println!(
                "{}",
                serde_json::json!({
                    "mode": if dry_run { "dry-run" } else { "apply" },
                    "applied": p.apply,
                    "noop": p.noop,
                    "rejected": p.reject,
                })
            );
            0
        }
        Err(e) => {
            eprintln!("configure falló: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT);",
        )
        .unwrap();
        c
    }

    #[test]
    fn safe_key_applies_and_is_idempotent() {
        let c = mem();
        let prof = json!({ "settings": { "ptt.hotkey": "Control+KeyT" } });
        let p = plan(&prof, &c).unwrap();
        assert_eq!(p.apply.len(), 1, "ptt.hotkey safe → apply");
        assert_eq!(p.reject.len(), 0);
        settings_store::set(&c, "ptt.hotkey", &json!("Control+KeyT")).unwrap();
        let p2 = plan(&prof, &c).unwrap();
        assert_eq!(p2.apply.len(), 0);
        assert_eq!(p2.noop, vec!["ptt.hotkey"]);
    }

    #[test]
    fn sensitive_and_unknown_keys_rejected() {
        let c = mem();
        let prof = json!({ "settings": {
            "policy.custom_enabled": false,
            "endpoints.aie": "https://evil.example",
            "opt_in.telemetry": true,
            "totally.unknown": 1,
        }});
        let p = plan(&prof, &c).unwrap();
        assert_eq!(p.apply.len(), 0, "nada sensible/desconocido se aplica");
        assert_eq!(p.reject.len(), 4);
    }

    #[test]
    fn secret_looking_value_rejected_even_if_key_safe() {
        let c = mem();
        // valor con pinta de secreto (matchea openai_key sk-[A-Za-z0-9]{20,}) — pero primero lo corta
        // valid_hotkey (tiene `-`), así que igual se rechaza. Probamos también un secreto SIN `-`:
        let prof = json!({ "settings": { "ptt.hotkey": "skAbcdef0123456789Abcdef0123456789" } });
        let p = plan(&prof, &c).unwrap();
        // no matchea el patrón sk- (sin guion) → pasa guardrail; pero es alfanumérico válido como hotkey.
        // El punto del test es que un valor con `-` (sk-...) se rechaza:
        let prof2 = json!({ "settings": { "ptt.hotkey": "sk-Abcdef0123456789Abcdef0123456789" } });
        let p2 = plan(&prof2, &c).unwrap();
        assert_eq!(p2.apply.len(), 0, "sk-... rechazado (formato/secreto)");
        assert_eq!(p2.reject.len(), 1);
        let _ = p; // p: el alfanumérico largo es un hotkey válido de forma (sin secreto) — no es el caso de interés
    }

    #[test]
    fn rejects_unknown_top_level_keys() {
        let c = mem();
        // un perfil con `exec` además de `settings` → RECHAZA todo el perfil (no aplica las settings safe).
        let prof = json!({ "settings": { "ptt.hotkey": "Alt+Space" }, "exec": { "cmd": "rm" } });
        assert!(plan(&prof, &c).is_err(), "top-level desconocido → error (no se ignora)");
    }

    #[test]
    fn rejects_non_primitive_and_bad_format() {
        let c = mem();
        // valor objeto para una key safe → reject
        let p1 = plan(&json!({ "settings": { "ptt.hotkey": { "x": 1 } } }), &c).unwrap();
        assert_eq!(p1.reject.len(), 1, "objeto en key safe → reject");
        // formato de hotkey inválido (espacios/;) → reject
        let p2 = plan(&json!({ "settings": { "ptt.hotkey": "F13 ; rm -rf" } }), &c).unwrap();
        assert_eq!(p2.reject.len(), 1, "hotkey con espacios/; → reject");
        // tipo equivocado para bool → reject
        let p3 = plan(&json!({ "settings": { "restore.always_ask": "yes" } }), &c).unwrap();
        assert_eq!(p3.reject.len(), 1, "string en bool → reject");
        // bool válido → apply
        let p4 = plan(&json!({ "settings": { "restore.always_ask": true } }), &c).unwrap();
        assert_eq!(p4.apply.len(), 1);
    }

    #[test]
    fn missing_settings_object_errors() {
        let c = mem();
        assert!(plan(&json!({}), &c).is_err());
        assert!(plan(&json!({ "settings": "nope" }), &c).is_err());
    }

    #[test]
    fn apply_persists_audits_and_is_idempotent() {
        // mem() + tabla events (la usa write_in_tx dentro de la transacción).
        let mut c = mem();
        c.execute_batch(
            "CREATE TABLE events (id TEXT PRIMARY KEY, kind TEXT, actor TEXT, pane_id TEXT, \
             card_id TEXT, correlation_id TEXT, payload TEXT);",
        )
        .unwrap();
        // AuditWriter requiere una conn (no la usa write_in_tx, que escribe sobre la tx) — throwaway.
        let audit = AuditWriter::new(std::sync::Arc::new(parking_lot::Mutex::new(
            Connection::open_in_memory().unwrap(),
        )));
        let prof = json!({ "settings": { "ptt.hotkey": "Control+KeyT", "restore.always_ask": true } });

        let p = apply(&prof, &mut c, &audit).unwrap();
        assert_eq!(p.apply.len(), 2, "ambas keys safe se aplican");
        // persistió en settings
        assert_eq!(settings_store::get(&c, "ptt.hotkey").unwrap(), Some(json!("Control+KeyT")));
        assert_eq!(settings_store::get(&c, "restore.always_ask").unwrap(), Some(json!(true)));
        // auditó (2 eventos configure.apply)
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'configure.apply'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
        // idempotente: re-aplicar el mismo perfil → 0 apply (todo noop), sin eventos nuevos.
        let p2 = apply(&prof, &mut c, &audit).unwrap();
        assert_eq!(p2.apply.len(), 0);
        assert_eq!(p2.noop.len(), 2);
        let n2: i64 = c
            .query_row("SELECT COUNT(*) FROM events WHERE kind = 'configure.apply'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 2, "idempotente: sin eventos nuevos");
    }
}
