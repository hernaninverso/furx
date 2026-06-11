// services/window_byok.rs — 018-fase-2-multiwindow-workspace · Phase B0 (T064, council-required)
//
// Enforcement BYOK POR-VENTANA (constitución F-I: una 2ª webview NO es un 2º vault).
//
// Multi-window (US2) introduce N webviews. La invariante F-I es que NINGUNA webview
// —Main o detached— recibe material de credencial: el backend Rust SSOT resuelve los
// secretos en el momento de ejecutar (`capability::secret::resolve`, backend-only) y
// arma el request; la key muere en ese scope. Este módulo lo vuelve EJECUTABLE (FR-015
// de prosa → tests de boundary) y declara la política por-ventana:
//
//   1. WHITELIST de comandos SENSIBLES: los `Risk::Credential` del registry. Son los
//      únicos que pueden tocar el Keychain. `is_sensitive_command(id)` lo deriva del
//      registry (fuente única de verdad), no de una lista paralela que se desincronice.
//   2. Política por-ventana: un comando sensible se permite SÓLO desde ventanas con
//      capacidad de credenciales. Hoy: Main sí; las detached NO (un pane detachado es
//      un viewport de comparación, no debe iniciar flujos de credencial). El gate de
//      capability/approval (US4) sigue corriendo encima — esto es una capa ADICIONAL.
//   3. BOUNDARY assert: `payload_has_secret(json)` reusa el guardrail de secretos para
//      probar, en tests, que NINGÚN payload de comando NI de evento broadcast lleva un
//      secreto. (El secreto vive SÓLO transitorio en el invoke de un comando Credential,
//      nunca en un evento; los eventos van por `emit`/`emit_all` a todas las webviews.)
//
// Preferir `emit` a ventana específica vs `emit_all` para payloads sensibles: como
// ningún AppEvent lleva secretos (todos son ids/estados), el broadcast es seguro; la
// regla queda documentada para variantes futuras (ver `event_is_broadcast_safe`).

use crate::services::command_registry::{registry, Risk};

/// Etiqueta de la ventana principal (== `layout_config::MAIN_WINDOW_KEY` /
/// el label de la WebviewWindow "main" de Tauri).
pub const MAIN_WINDOW_LABEL: &str = "main";

/// ¿`command_id` es un comando SENSIBLE (toca credenciales)? Derivado del registry:
/// son los `Risk::Credential`. Fuente única — si un comando nuevo se marca Credential,
/// queda automáticamente bajo esta política sin tocar este archivo.
pub fn is_sensitive_command(command_id: &str) -> bool {
    registry()
        .iter()
        .any(|c| c.id == command_id && matches!(c.risk, Risk::Credential))
}

/// ¿La ventana `window_label` tiene capacidad de ejecutar comandos sensibles
/// (credenciales)? Política T064: SÓLO la ventana Main. Las detached son viewports de
/// comparación — el backend SSOT resuelve secretos para ellas igual (su PTY/agente
/// puede usar una key), pero el INICIO de un flujo de credencial (rotar secreto mobile,
/// persistir provider, etc.) queda confinado a Main para no multiplicar la superficie.
pub fn window_can_invoke_sensitive(window_label: &str) -> bool {
    window_label == MAIN_WINDOW_LABEL
}

/// OWNERSHIP de cierre/reattach de ventana (018 US2 audit). Una ventana detached SÓLO puede
/// cerrarse/reatarse a SÍ MISMA; sólo Main puede operar sobre un `target` ajeno (op administrativa).
/// `caller_label` lo deriva Tauri server-side (`window.label()`), NO se confía en el arg del front
/// → una webview detached no puede cerrar/desplazar los panes de OTRA ventana. PURA → testeable.
pub fn can_close_window(caller_label: &str, target_label: &str) -> bool {
    caller_label == MAIN_WINDOW_LABEL || caller_label == target_label
}

/// Decisión de enforcement por-ventana para un comando. `Allow` si no es sensible o si
/// la ventana tiene capacidad; `Deny(reason)` si una ventana sin capacidad intenta un
/// comando sensible. El gate US4 (approval) corre por separado; esto es defensa-en-capas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowGate {
    Allow,
    Deny(String),
}

/// Aplica la política por-ventana (T064). Fail-closed.
pub fn check_window_command(window_label: &str, command_id: &str) -> WindowGate {
    if !is_sensitive_command(command_id) {
        return WindowGate::Allow;
    }
    if window_can_invoke_sensitive(window_label) {
        WindowGate::Allow
    } else {
        WindowGate::Deny(format!(
            "comando sensible '{command_id}' bloqueado desde la ventana '{window_label}' (sólo Main inicia flujos de credencial — BYOK F-I)"
        ))
    }
}

/// BOUNDARY assert (T064): ¿este JSON contiene un secreto? Reusa el guardrail de
/// secretos. Usado en tests para probar 0 secretos en payloads de comando/evento.
pub fn payload_has_secret(json: &str) -> bool {
    !crate::bases::guardrail::scan(json).is_empty()
}

/// ¿Es seguro hacer broadcast (`emit_all`) de este evento a TODAS las webviews? Hoy
/// SÍ para todos los AppEvent (ninguno lleva secretos — son ids/estados). La función
/// existe para que una variante futura con payload sensible se marque `false` y se
/// enrute con `emit` a una ventana específica (regla del council).
pub fn event_is_broadcast_safe(event_json: &str) -> bool {
    !payload_has_secret(event_json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::event_bus::AppEvent;

    #[test]
    fn sensitive_commands_derive_from_registry() {
        // Los comandos de credencial conocidos (mobile_secret_*) son sensibles…
        assert!(is_sensitive_command("mobile_secret_get"));
        assert!(is_sensitive_command("mobile_secret_rotate"));
        // …incluido provider_persist (018 US2 audit): ingiere una API key → debe ser sensible
        // para que una ventana detached NO pueda persistir credenciales (BYOK).
        assert!(is_sensitive_command("provider_persist"));
        // …y los Safe no lo son.
        assert!(!is_sensitive_command("layout_config_get"));
        assert!(!is_sensitive_command("list_panes"));
        assert!(!is_sensitive_command("comando_inexistente"));
    }

    #[test]
    fn only_main_window_can_invoke_sensitive() {
        assert!(window_can_invoke_sensitive("main"));
        assert!(!window_can_invoke_sensitive("detached-1"));
        assert!(!window_can_invoke_sensitive(""));
    }

    #[test]
    fn close_window_ownership() {
        // 018 US2 audit (codex): Main puede cerrar/reatar cualquier ventana.
        assert!(can_close_window("main", "detached-1"));
        assert!(can_close_window("main", "main"));
        // Una detached puede cerrarse a SÍ MISMA…
        assert!(can_close_window("detached-1", "detached-1"));
        // …pero NO a otra detached (anti cross-window: rompería ownership/paneles ajenos).
        assert!(!can_close_window("detached-1", "detached-2"));
        assert!(!can_close_window("detached-1", "main"));
    }

    #[test]
    fn window_gate_allows_safe_everywhere_blocks_sensitive_off_main() {
        // Safe desde cualquier ventana → Allow.
        assert_eq!(
            check_window_command("detached-1", "layout_config_get"),
            WindowGate::Allow
        );
        assert_eq!(
            check_window_command("main", "layout_config_get"),
            WindowGate::Allow
        );
        // Sensible desde Main → Allow.
        assert_eq!(
            check_window_command("main", "mobile_secret_rotate"),
            WindowGate::Allow
        );
        // Sensible desde detached → Deny.
        match check_window_command("detached-1", "mobile_secret_rotate") {
            WindowGate::Deny(r) => assert!(r.contains("BYOK")),
            WindowGate::Allow => panic!("debió denegar comando sensible desde ventana detached"),
        }
    }

    #[test]
    fn every_credential_command_is_denied_off_main() {
        // HIGH-2 (audit) — el gate central (lib.rs) llama `check_window_command(label, id)`
        // para CADA invoke. Este test garantiza que TODO comando Risk::Credential del registry
        // es DENEGADO desde una ventana no-Main (y PERMITIDO desde Main). Si alguien agrega un
        // comando Credential nuevo, queda cubierto automáticamente (deriva del registry) — y si
        // la política se rompiera (un Credential dejara de denegarse off-Main), este test falla.
        let mut checked = 0usize;
        for c in registry().iter() {
            if matches!(c.risk, Risk::Credential) {
                checked += 1;
                // Desde una ventana detached → Deny.
                match check_window_command("detached-1", &c.id) {
                    WindowGate::Deny(_) => {}
                    WindowGate::Allow => {
                        panic!(
                            "comando Credential '{}' NO fue denegado desde ventana no-Main",
                            c.id
                        )
                    }
                }
                // Desde Main → Allow.
                assert_eq!(
                    check_window_command(MAIN_WINDOW_LABEL, &c.id),
                    WindowGate::Allow,
                    "comando Credential '{}' debió permitirse desde Main",
                    c.id
                );
            }
        }
        assert!(
            checked > 0,
            "debe haber al menos un comando Credential en el registry"
        );
    }

    #[test]
    fn guardrail_detects_real_secrets_in_payload() {
        // El boundary assert detecta claves reales (defensa: si por bug una key se colara).
        assert!(payload_has_secret(
            r#"{"k":"sk-ant-0123456789abcdef0123456789abcdef0123"}"#
        ));
        assert!(payload_has_secret(
            r#"{"token":"ghp_0123456789abcdefghijklmnopqrstuvwxyz12"}"#
        ));
        // Un payload sin secretos no dispara.
        assert!(!payload_has_secret(r#"{"id":"p1","state":"running"}"#));
    }

    #[test]
    fn no_app_event_carries_a_secret() {
        // BOUNDARY (FR-015 ejecutable): NINGÚN AppEvent broadcasteado lleva un secreto.
        // Construimos una de cada variante con valores realistas y verificamos el JSON.
        let events = vec![
            AppEvent::TaskChanged {
                id: "t1".into(),
                state: "running".into(),
            },
            AppEvent::AgentStateChanged {
                id: "a1".into(),
                state: "idle".into(),
            },
            AppEvent::LayoutChanged {
                window_id: "main".into(),
            },
            AppEvent::CommandExecuted {
                command_id: "layout_config_save".into(),
            },
            AppEvent::ApprovalRequested {
                request_id: "r1".into(),
                command_id: "reset_furx".into(),
            },
        ];
        for ev in &events {
            let json = serde_json::to_string(ev).unwrap();
            assert!(!payload_has_secret(&json), "evento con secreto: {json}");
            assert!(
                event_is_broadcast_safe(&json),
                "evento no-broadcast-safe: {json}"
            );
        }
    }

    #[test]
    fn review_flow_payloads_carry_zero_secrets() {
        // 019 F0 · T004 — AISLAMIENTO BYOK del flujo review (FR-008, F-I): los payloads de comando y
        // de evento del flujo (compare/approve/reject/kill/apply) NUNCA llevan material de key. Son
        // ids/estados/rationale — el SSOT de secretos es el backend (Keychain), resuelto al ejecutar.
        //
        // 1) Args de los comandos del flujo (lo que el front manda en el invoke): sólo ids + texto.
        let command_args = vec![
            // review_open / review_get / review_conflicts
            r#"{"group_id":"g1"}"#.to_string(),
            // review_hunk_decide (con rationale libre del usuario)
            r#"{"group_id":"g1","hunk_id":"t1:src/a.rs:10,5","decision":"approved","expected_revision":2,"rationale":"looks correct"}"#.to_string(),
            // review_apply
            r#"{"group_id":"g1","expected_revision":3}"#.to_string(),
            // orchestration_cancel (kill-switch)
            r#"{"task_id":"t1"}"#.to_string(),
        ];
        for a in &command_args {
            assert!(
                !payload_has_secret(a),
                "un arg de comando del flujo review llevó un secreto: {a}"
            );
        }

        // 2) Payload del evento de audit que el flujo emite (review.compare/approve/…): ids +
        //    rationale, ningún secreto. Reproducimos la forma exacta que escribe `review_audit::record`.
        let audit_event = serde_json::json!({
            "action": "approve",
            "target": "t1:src/a.rs:10,5",
            "rationale": "the key insight is correct",
            "group_id": "g1",
            "hunk_id": "t1:src/a.rs:10,5",
            "approval_id": serde_json::Value::Null,
            "revision": 2,
        })
        .to_string();
        assert!(
            !payload_has_secret(&audit_event),
            "el evento de audit del flujo review llevó un secreto: {audit_event}"
        );
        assert!(event_is_broadcast_safe(&audit_event));

        // 3) DEFENSA: si por un bug una key se colara en un rationale/arg, el boundary la CAZA
        //    (no es un test trivialmente verde — el guardrail discrimina).
        let leaked = r#"{"group_id":"g1","rationale":"set ANTHROPIC_API_KEY=sk-ant-0123456789abcdef0123456789abcdef0123"}"#;
        assert!(
            payload_has_secret(leaked),
            "el boundary DEBE cazar una key filtrada en un payload del flujo"
        );
    }

    #[test]
    fn review_flow_commands_are_not_credential_sensitive() {
        // 019 F0 · T004 — los comandos del flujo review NO son Risk::Credential: no inician flujos de
        // credencial (no tocan el Keychain), así que no necesitan confinarse a Main por la regla
        // BYOK por-ventana (su gobierno es el approval gate de T003, capa aparte). Verifica que el
        // wiring no los marcó sensibles por error (lo que rompería el flujo desde ventanas detached).
        for id in [
            "review_open",
            "review_get",
            "review_hunk_decide",
            "review_conflicts",
            "review_apply",
            "orchestration_cancel",
        ] {
            assert!(
                !is_sensitive_command(id),
                "el comando del flujo review '{id}' NO debe ser Credential-sensible (no toca keys)"
            );
        }
    }

    #[test]
    fn resolve_key_never_returns_to_a_webview_signature() {
        // Defensa de tipo: `secret::resolve` devuelve Err para un ref con forma de KEY
        // (no de nombre de entry) → una key jamás "viaja" como ref ni se filtra en el error.
        let err = crate::services::capability::secret::resolve(
            "sk-ant-0123456789abcdef0123456789abcdef0123",
        )
        .unwrap_err()
        .to_string();
        // El error NO contiene el valor del ref (no se filtra).
        assert!(!err.contains("sk-ant-"), "el error filtró el ref: {err}");
    }
}
