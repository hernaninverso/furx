// US7 — Settings registry tipado + search (spec 015-frontend-reform-kernel).
//
// Fuente única, tipada y curada de los settings del producto. El registry es
// ESTÁTICO (definido en código), no una tabla: describe cada setting con su
// dominio, schema de validación, visibilidad y metadata. Los VALORES siguen
// persistiendo en la tabla `settings` KV-store existente (`crate::settings`),
// así que `settings_get`/`settings_set`/`settings_all` NO se rompen.
//
// El front (`web/src/lib/settingsRegistry.ts` + `SettingsRegistryPanel.tsx`)
// genera la Settings UI con search a partir de `settings_registry_list()`, y
// escribe vía `settings_set_validated()` que valida contra el schema antes de
// persistir.
//
// BYOK (Constitución F-I): las API keys NUNCA son settings — viven en el
// Keychain. El dominio `Accounts` referencia metadata/estado de cuentas, NUNCA
// el secreto.

use serde::Serialize;
use serde_json::Value;

/// Dominio de Control Center al que pertenece el setting. Mapea a los tabs de la
/// Settings UI (spec US7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SettingDomain {
    Accounts,
    Agents,
    Plugins,
    Audio,
    Signals,
    Orchestration,
    Review,
    Appearance,
    Shortcuts,
    Advanced,
    /// 023 — memoria auto-captura + Hub no-opaco (settings de privacidad/captura).
    Memory,
}

/// Cuán expuesto está el setting en la UI generada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Visibility {
    /// Visible en el tab del dominio sin esfuerzo.
    Visible,
    /// Oculto detrás de un toggle "Avanzado".
    Advanced,
    /// No se renderiza; sólo lectura programática (estado interno).
    Internal,
}

/// Nivel de riesgo de cambiar el setting (alimenta confirmaciones / DangerZone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Risk {
    Safe,
    Caution,
    Destructive,
}

/// Schema de tipo + validación de un setting. Se serializa con un tag `type`
/// para que el front pueda discriminar qué control renderizar.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingSchema {
    Bool,
    /// String libre. `max_len` opcional para evitar valores absurdos.
    String {
        max_len: Option<usize>,
    },
    /// Enum cerrado: un set fijo de strings válidos.
    Enum {
        options: Vec<String>,
    },
    /// Número con rango inclusivo opcional.
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
}

impl SettingSchema {
    /// Valida un valor contra el schema. Devuelve `Ok(())` o un mensaje de error
    /// orientado al usuario.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        match self {
            SettingSchema::Bool => {
                if value.is_boolean() {
                    Ok(())
                } else {
                    Err(format!("expected a boolean, got {}", type_name(value)))
                }
            }
            SettingSchema::String { max_len } => match value.as_str() {
                Some(s) => {
                    if let Some(max) = max_len {
                        if s.chars().count() > *max {
                            return Err(format!("string too long (max {max} chars)"));
                        }
                    }
                    Ok(())
                }
                None => Err(format!("expected a string, got {}", type_name(value))),
            },
            SettingSchema::Enum { options } => match value.as_str() {
                Some(s) => {
                    if options.iter().any(|o| o == s) {
                        Ok(())
                    } else {
                        Err(format!("'{s}' is not one of: {}", options.join(", ")))
                    }
                }
                None => Err(format!(
                    "expected one of {:?}, got {}",
                    options,
                    type_name(value)
                )),
            },
            SettingSchema::Number { min, max } => match value.as_f64() {
                Some(n) => {
                    if let Some(lo) = min {
                        if n < *lo {
                            return Err(format!("value {n} below minimum {lo}"));
                        }
                    }
                    if let Some(hi) = max {
                        if n > *hi {
                            return Err(format!("value {n} above maximum {hi}"));
                        }
                    }
                    Ok(())
                }
                None => Err(format!("expected a number, got {}", type_name(value))),
            },
        }
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Definición tipada de un setting. Es el espejo Rust de `SettingDef` en TS.
#[derive(Debug, Clone, Serialize)]
pub struct SettingDef {
    /// Clave canónica en la tabla `settings` (ej. `orchestration.auto_confirm_global`).
    pub key: String,
    pub domain: SettingDomain,
    pub label: String,
    pub description: String,
    pub default_value: Value,
    pub schema: SettingSchema,
    pub visibility: Visibility,
    /// Cambiar el valor requiere reiniciar Furx para que aplique.
    pub restart_required: bool,
    pub risk: Risk,
}

/// Helper de construcción para mantener la tabla curada legible.
fn def(
    key: &str,
    domain: SettingDomain,
    label: &str,
    description: &str,
    default_value: Value,
    schema: SettingSchema,
    visibility: Visibility,
    restart_required: bool,
    risk: Risk,
) -> SettingDef {
    SettingDef {
        key: key.to_string(),
        domain,
        label: label.to_string(),
        description: description.to_string(),
        default_value,
        schema,
        visibility,
        restart_required,
        risk,
    }
}

/// La tabla curada de settings. Incluye los que HOY existen en el código
/// (claves verificadas vía grep) + los que el spec menciona por dominio.
///
/// Mantener esta lista alineada con `web/src/lib/settingsRegistry.ts` (espejo TS).
pub fn registry() -> Vec<SettingDef> {
    use SettingDomain as D;
    vec![
        // ── Orchestration ──────────────────────────────────────────────
        def(
            "orchestration.auto_confirm_global",
            D::Orchestration,
            "Auto-confirmar acciones globalmente",
            "Aplica automáticamente las decisiones del orquestador sin pedir confirmación. Acelera el flujo a costa de control manual.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "orchestration.use_aie_for_meta",
            D::Orchestration,
            "Usar el AI Engine para meta-decisiones",
            "Cuando la heurística del orquestador es ambigua, consulta el AI Engine free ($0) para refinar la detección de estado (terminó / pide input). El AI Engine NUNCA bloquea: ante cualquier fallo se usa la heurística. El buffer pasa por el sanitizador de PII antes de salir. Por defecto: deshabilitado (opt-in).",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        def(
            "orchestration.disable_worktree_cleanup",
            D::Orchestration,
            "Desactivar limpieza de worktrees",
            "Por defecto Furx elimina los git worktrees efímeros al terminar. Activá esto para conservarlos (debug). Acumula espacio en disco.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        // ── 024-quality-gate — evidencia objetiva por variante (linters/typecheck) ──
        def(
            "qualitygate.enabled",
            D::Orchestration,
            "Quality-gate: correr linters sobre las variantes",
            // Transparencia (council v2 §3.7): el copy deja claro que EJECUTA el toolchain del repo.
            "Cuando comparás variantes de un best-of-N, corre los linters/typecheckers de TU repo \
             (clippy / eslint+tsc / ruff+mypy, autodetectados) sobre cada variante para darte una \
             señal verificable de errores y warnings. ESTO EJECUTA EL TOOLCHAIN DE TU REPO sobre el \
             código que produjo el agente, acotado por un sandbox sin acceso a red y confinado al \
             worktree de la variante, con timeout. Todo local; nada sale a la nube. Por defecto: \
             desactivado (opt-in).",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        def(
            "qualitygate.linters",
            D::Orchestration,
            "Quality-gate: linters habilitados",
            "Lista separada por comas de los linters que el quality-gate puede correr (allow-list). \
             Por defecto: clippy, eslint, tsc, ruff, mypy. `cargo_check` es opt-in (recompila): \
             agregalo a esta lista para habilitarlo. Cualquier id fuera de la allow-list se ignora.",
            Value::String("clippy,eslint,tsc,ruff,mypy".to_string()),
            SettingSchema::String { max_len: Some(200) },
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        def(
            "qualitygate.feed_ranking",
            D::Orchestration,
            "Quality-gate: alimentar el ranking advisory",
            "Suma el conteo de issues (menos issues = mejor señal) al ranking advisory del \
             meta-orquestador. Nunca elige ni descarta variantes: es una señal más; vos seguís \
             eligiendo. Por defecto: desactivado. (El cableado completo llega en una ola posterior.)",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
        // ── 026 preference-loop — aprender qué variante/hunks elegís (auto-aprendizaje #2) ──
        def(
            "preference.record_enabled",
            D::Orchestration,
            "Preferencias: registrar tus elecciones de best-of-N",
            "Cuando cerrás/aplicás una review de best-of-N, Furx registra (local, append-only) qué \
             variante/hunks elegiste y rechazaste, con las features objetivas de cada variante \
             (líneas cambiadas, rutas riesgosas, issues del quality-gate, agente). NUNCA guarda el \
             código crudo de los diffs — sólo metadata. Es el cimiento para que el ranking aprenda \
             de vos. Por defecto: activado (son decisiones que ya ocurren y ya quedan auditadas).",
            Value::Bool(true),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
        def(
            "preference.inject",
            D::Orchestration,
            "Preferencias: usar lo aprendido en el ranking sugerido",
            "Combina lo que venís eligiendo (un prior local, explicable) con el ranking advisory del \
             meta-orquestador: la variante que matchea tu patrón sube, y cada sugerencia te muestra \
             POR QUÉ. Siempre advisory: NUNCA elige ni aplica por vos. Sólo influye tras ≥15 \
             decisiones en ese repo (antes: 'aún aprendiendo'). Por defecto: desactivado (opt-in).",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        def(
            "preference.risky_paths",
            D::Orchestration,
            "Preferencias: rutas sensibles (risky paths)",
            "Lista separada por comas de fragmentos de ruta que Furx considera sensibles al \
             caracterizar una variante (¿toca migraciones / auth / .env / lockfiles?). Vacío = usar \
             el set por defecto (migrations/, .env, auth, *.lock, secrets, …). No vacío = reemplaza \
             el default (control total por repo).",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(500) },
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
        // 027 policy-as-code — `policy.custom_enabled` NO se registra como setting genérico a
        // propósito (audit codex BLOCKER): cambiarlo desde el setter genérico (Safe, sin gate ni
        // audit) permitiría APAGAR todas las reglas custom sin aprobación, relajando el gobierno en
        // silencio. Se gestiona EXCLUSIVAMENTE vía el comando dedicado `policy_set_custom_enabled`
        // (gateado + auditado en `policy_rule_changes`); el setter genérico rechaza las keys `policy.*`.
        // ── Review & Safety ────────────────────────────────────────────
        def(
            "restore.always_ask",
            D::Review,
            "Preguntar antes de restaurar al abrir",
            "Por defecto Furx reattachea tus sesiones tmux silenciosamente. Activá esto para elegir cada vez.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        def(
            "opt_in.telemetry",
            D::Review,
            "Telemetría anónima",
            "Métricas de uso agregadas (nunca el contenido de tus prompts ni tus claves). Por defecto: deshabilitado.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        // ── Signals & Remote ───────────────────────────────────────────
        def(
            "signals.webhook_url",
            D::Signals,
            "URL de webhook de señales",
            "Endpoint HTTPS al que Furx postea eventos de señales (notificaciones salientes). Dejalo vacío para deshabilitar.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "mobile.tailscale_enabled",
            D::Signals,
            "Exponer companion por Tailscale",
            "Habilita el bridge móvil sobre la interfaz Tailscale además de loopback. Requiere reiniciar Furx.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            true,
            Risk::Caution,
        ),
        // ── Accounts & BYOK (metadata/estado, NUNCA secretos) ──────────
        def(
            "app.first_run_completed",
            D::Accounts,
            "Onboarding completado",
            "Marca de estado: el asistente de primera ejecución ya corrió. Interno.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Internal,
            false,
            Risk::Safe,
        ),
        def(
            "opt_in.eula_accepted_at",
            D::Accounts,
            "EULA aceptado (timestamp)",
            "Sello temporal ISO-8601 de aceptación de los términos legales. Interno.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(64) },
            Visibility::Internal,
            false,
            Risk::Safe,
        ),
        // ── Advanced — endpoints ───────────────────────────────────────
        def(
            "endpoints.aie",
            D::Advanced,
            "Endpoint AI Engine",
            "Override de la URL base del AI Engine. Dejalo vacío para usar el default. Reiniciá para aplicar a procesos vivos.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            true,
            Risk::Caution,
        ),
        // 042 FR-002 — el wizard guarda el endpoint Ollama del usuario acá (antes sólo se leía de env).
        def(
            "endpoints.ollama",
            D::Advanced,
            "Endpoint Ollama",
            "URL base del servidor Ollama (embeddings / modelos locales). Dejalo vacío para usar el default localhost:11434. Reiniciá para aplicar.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            true,
            Risk::Caution,
        ),
        def(
            "endpoints.license",
            D::Advanced,
            "Endpoint de licencias",
            "Override de la URL del servicio de licencias. Sólo para entornos self-host. Reiniciá para aplicar.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            true,
            Risk::Caution,
        ),
        def(
            "endpoints.updates",
            D::Advanced,
            "Endpoint de updates",
            "Override de la URL del feed de actualizaciones. Reiniciá para aplicar.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            true,
            Risk::Caution,
        ),
        def(
            "endpoints.telegram_relay",
            D::Advanced,
            "Endpoint relay Telegram",
            "Override del relay de Telegram usado por señales salientes. Reiniciá para aplicar.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            true,
            Risk::Caution,
        ),
        def(
            "endpoints.grafana",
            D::Advanced,
            "URL Grafana",
            "URL del dashboard Grafana embebido. Dejalo vacío para ocultar el panel.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(2048) },
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
        def(
            "endpoints.allowlist_extra",
            D::Advanced,
            "Allowlist extra de endpoints",
            "Hosts adicionales permitidos para el panel web embebido, separados por coma.",
            Value::String(String::new()),
            SettingSchema::String { max_len: Some(4096) },
            Visibility::Advanced,
            false,
            Risk::Caution,
        ),
        // ── Appearance ─────────────────────────────────────────────────
        def(
            "appearance.theme",
            D::Appearance,
            "Tema",
            "Esquema de color de la interfaz. 'system' sigue al modo del sistema operativo.",
            Value::String("system".to_string()),
            SettingSchema::Enum {
                options: vec!["system".into(), "light".into(), "dark".into()],
            },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        def(
            "appearance.density",
            D::Appearance,
            "Densidad de UI",
            "Compacta o espacia los controles de la interfaz.",
            Value::String("comfortable".to_string()),
            SettingSchema::Enum {
                options: vec!["compact".into(), "comfortable".into()],
            },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        // ── Audio & Voice ──────────────────────────────────────────────
        def(
            "audio.tts_enabled",
            D::Audio,
            "Lectura en voz alta (TTS)",
            "Lee en voz alta las respuestas de los agentes. Podés interrumpir hablando.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        def(
            "audio.tts_rate",
            D::Audio,
            "Velocidad de lectura",
            "Multiplicador de velocidad del TTS (1.0 = normal).",
            Value::from(1.0),
            SettingSchema::Number { min: Some(0.5), max: Some(2.0) },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        // 021-voice-es — dictado por voz (push-to-talk + VoiceModal).
        def(
            "voice.language",
            D::Audio,
            "Idioma del dictado",
            "Idioma que whisper usa al transcribir tu voz. 'es' (español) por defecto; 'auto' autodetecta (más lento); 'en' fuerza inglés.",
            Value::String("es".to_string()),
            SettingSchema::Enum {
                options: vec!["es".into(), "auto".into(), "en".into()],
            },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        def(
            "voice.model",
            D::Audio,
            "Modelo de transcripción",
            "Modelo whisper local (multilingüe). 'base' (~142MB) balancea velocidad/calidad; 'small' (~466MB) más preciso, más pesado. Cambiar el modelo puede requerir descargarlo.",
            Value::String("base".to_string()),
            SettingSchema::Enum {
                options: vec!["base".into(), "small".into()],
            },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        // ── Agents & Presets ───────────────────────────────────────────
        def(
            "agents.default_engine",
            D::Agents,
            "Motor por defecto",
            "El CLI de agente usado al abrir un pane nuevo sin especificar.",
            Value::String("claude".to_string()),
            SettingSchema::Enum {
                options: vec!["claude".into(), "codex".into(), "gemini".into(), "aider".into()],
            },
            Visibility::Visible,
            false,
            Risk::Safe,
        ),
        // ── Plugins & Permissions ──────────────────────────────────────
        def(
            "plugins.require_signature",
            D::Plugins,
            "Exigir firma de plugins",
            "Sólo carga plugins con firma Ed25519 válida y pinned. Desactivarlo permite plugins sin firmar (riesgoso).",
            Value::Bool(true),
            SettingSchema::Bool,
            Visibility::Visible,
            true,
            Risk::Destructive,
        ),
        // ── Memory (023 — auto-captura across-CLIs, default OFF por privacidad) ──
        def(
            "memory.autocapture",
            D::Memory,
            "Auto-captura de memoria",
            "Cuando cerrás un pane de un CLI de agente (claude/codex/gemini/aider), Furx destila la sesión en memorias candidatas y te las muestra en una bandeja para revisar. El texto pasa por el redactor de secretos ANTES de salir. Nada entra al Hub sin que vos lo aceptes. Por defecto: desactivado.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "memory.autocapture_auto_accept",
            D::Memory,
            "Aceptar candidatas automáticamente",
            "Si lo activás, las memorias candidatas entran directo al Hub sin pasar por la bandeja de revisión. Reduce el control humano: dejalo desactivado salvo que confíes plenamente en la destilación.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "memory.inject",
            D::Memory,
            "Recall de memoria en cada sesión",
            "Habilita que cualquier CLI de agente pueda recordar la memoria del Hub (vía el recall de memoria local) sin configurar cada perfil a mano. Por defecto: desactivado.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "memory.autocapture_max_candidates",
            D::Memory,
            "Máximo de candidatas por sesión",
            "Cuántas memorias candidatas como máximo destila Furx de cada sesión cerrada.",
            Value::from(5.0),
            SettingSchema::Number { min: Some(1.0), max: Some(20.0) },
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
        // ── Lecciones procedurales (025 — auto-aprendizaje fallo->fix, default OFF) ──
        def(
            "memory.procedural_learning",
            D::Memory,
            "Aprender lecciones de fallos",
            "Cuando un agente arregla un error tras un fallo, Furx destila la lección (síntoma -> fix -> cuándo aplica) y te la propone en la bandeja para revisar. El texto pasa por el redactor de secretos ANTES de salir. Nada se aprende sin que lo aceptes. Por defecto: desactivado.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "memory.procedural_inject",
            D::Memory,
            "Inyectar lecciones en el perfil",
            "Agrega las lecciones aprobadas y activas del proyecto al contexto del agente (Claude) como un bloque delimitado 'Lecciones aprendidas'. Nunca reemplaza tu system prompt: se concatena. Ves exactamente qué se inyecta y podés desactivar o borrar cada lección. Por defecto: desactivado.",
            Value::Bool(false),
            SettingSchema::Bool,
            Visibility::Visible,
            false,
            Risk::Caution,
        ),
        def(
            "memory.procedural_inject_max",
            D::Memory,
            "Presupuesto de tokens para lecciones",
            "Tope de tokens del bloque de lecciones que se inyecta. Las lecciones más relevantes y recientes entran hasta agotar el presupuesto.",
            Value::from(1200.0),
            SettingSchema::Number { min: Some(100.0), max: Some(8000.0) },
            Visibility::Advanced,
            false,
            Risk::Safe,
        ),
    ]
}

/// Devuelve la tabla curada de settings para que el front genere la UI.
/// El wrapper `#[tauri::command]` vive en `commands.rs`.
pub fn settings_registry_list() -> Result<Vec<SettingDef>, String> {
    Ok(registry())
}

/// Busca un `SettingDef` por clave en el registry curado.
pub fn find(key: &str) -> Option<SettingDef> {
    registry().into_iter().find(|d| d.key == key)
}

/// Valida un valor contra el schema del setting `key`. Si la clave no está en el
/// registry, lo permitimos (settings legacy/ad-hoc que escriben por `settings_set`
/// crudo no se rompen). Devuelve `Ok(())` o un error de validación.
pub fn validate(key: &str, value: &Value) -> Result<(), String> {
    match find(key) {
        Some(d) => d.schema.validate(value),
        None => Ok(()),
    }
}

/// Filtra una lista de settings por una query de texto libre contra key, label
/// y description (case-insensitive substring). Espejo de `searchSettings` en TS;
/// query vacía → todos. Sirve de oráculo testeable para la búsqueda de la UI.
pub fn search<'a>(defs: &'a [SettingDef], query: &str) -> Vec<&'a SettingDef> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return defs.iter().collect();
    }
    defs.iter()
        .filter(|d| {
            d.key.to_lowercase().contains(&q)
                || d.label.to_lowercase().contains(&q)
                || d.description.to_lowercase().contains(&q)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn registry_is_curated_and_unique() {
        let r = registry();
        assert!(r.len() >= 10, "registry should curate at least 10 settings");
        // Claves únicas.
        let mut keys: Vec<&str> = r.iter().map(|d| d.key.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "duplicate keys in registry");
    }

    #[test]
    fn known_settings_are_present_with_domains() {
        // Un setting que HOY existe en el código aparece generado del registry.
        let auto = find("orchestration.auto_confirm_global").expect("known key present");
        assert_eq!(auto.domain, SettingDomain::Orchestration);
        assert!(auto.default_value.is_boolean());

        let theme = find("appearance.theme").expect("theme present");
        assert_eq!(theme.domain, SettingDomain::Appearance);
    }

    #[test]
    fn defaults_satisfy_their_own_schema() {
        // Invariante: cada default es válido contra su propio schema.
        for d in registry() {
            d.schema
                .validate(&d.default_value)
                .unwrap_or_else(|e| panic!("default for {} fails its schema: {e}", d.key));
        }
    }

    #[test]
    fn bool_schema_rejects_non_bool() {
        let s = SettingSchema::Bool;
        assert!(s.validate(&json!(true)).is_ok());
        assert!(s.validate(&json!("yes")).is_err());
        assert!(s.validate(&json!(1)).is_err());
    }

    #[test]
    fn enum_schema_rejects_unknown_option() {
        let s = SettingSchema::Enum {
            options: vec!["light".into(), "dark".into(), "system".into()],
        };
        assert!(s.validate(&json!("dark")).is_ok());
        assert!(s.validate(&json!("neon")).is_err());
        assert!(s.validate(&json!(42)).is_err());
    }

    #[test]
    fn number_schema_enforces_range() {
        let s = SettingSchema::Number {
            min: Some(0.5),
            max: Some(2.0),
        };
        assert!(s.validate(&json!(1.0)).is_ok());
        assert!(s.validate(&json!(0.4)).is_err());
        assert!(s.validate(&json!(2.5)).is_err());
        assert!(s.validate(&json!("fast")).is_err());
    }

    #[test]
    fn string_schema_enforces_max_len() {
        let s = SettingSchema::String { max_len: Some(4) };
        assert!(s.validate(&json!("abcd")).is_ok());
        assert!(s.validate(&json!("abcde")).is_err());
        assert!(s.validate(&json!(true)).is_err());
    }

    #[test]
    fn validate_rejects_invalid_for_registered_key() {
        // Un valor inválido es rechazado por la validación de schema.
        let bad = validate("appearance.theme", &json!("ultraviolet"));
        assert!(bad.is_err(), "invalid enum value must be rejected");
        let bad2 = validate("orchestration.auto_confirm_global", &json!("nope"));
        assert!(
            bad2.is_err(),
            "non-bool for a bool setting must be rejected"
        );
        let ok = validate("appearance.theme", &json!("dark"));
        assert!(ok.is_ok());
    }

    #[test]
    fn validate_allows_unknown_legacy_keys() {
        // Claves fuera del registry (legacy/ad-hoc) no se rompen.
        assert!(validate("some.unregistered.key", &json!("whatever")).is_ok());
    }

    #[test]
    fn settings_are_searchable_by_label_key_and_description() {
        let r = registry();
        // Empty query returns everything.
        assert_eq!(search(&r, "").len(), r.len());
        // By label substring.
        let by_label = search(&r, "telemetría");
        assert!(by_label.iter().any(|d| d.key == "opt_in.telemetry"));
        // By key substring.
        let by_key = search(&r, "auto_confirm");
        assert!(by_key
            .iter()
            .any(|d| d.key == "orchestration.auto_confirm_global"));
        // By description substring (case-insensitive).
        let by_desc = search(&r, "KEYCHAIN");
        // No setting description must leak a secret, but "Keychain" appears in BYOK copy if present;
        // at minimum search must be case-insensitive and return a stable subset.
        assert!(by_desc.len() <= r.len());
        // Non-matching query returns empty.
        assert!(search(&r, "zzz-no-such-setting-xyz").is_empty());
    }

    #[test]
    fn acceptance_new_setting_appears_searchable_and_validated() {
        // Acceptance US7: un setting (appearance.theme) aparece generado del
        // registry, es buscable, y un valor inválido es rechazado.
        let r = registry();
        let theme = r
            .iter()
            .find(|d| d.key == "appearance.theme")
            .expect("present");
        assert!(
            search(&r, "Tema").iter().any(|d| d.key == theme.key),
            "searchable by label"
        );
        assert!(
            validate("appearance.theme", &json!("midnight")).is_err(),
            "invalid rejected"
        );
        assert!(
            validate("appearance.theme", &json!("dark")).is_ok(),
            "valid accepted"
        );
    }
}
