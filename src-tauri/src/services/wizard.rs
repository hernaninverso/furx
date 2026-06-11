// 042 FR-002 — onboarding wizard backend: validar + guardar los endpoints del usuario y un
// health-check para el botón "Probar".
//
// Diseño (council /tmp/council-gtm-result.md, G-wizard):
//   - `save_endpoints` es SYNC: manipula `state.db.lock()` (Mutex sync) + `add_runtime_origin`
//     (escribe en parking_lot::RwLock). No hay red → no necesita async.
//   - LOCK ORDERING DB→allowlist: retenemos el `db.lock()` hasta DESPUÉS de actualizar la allowlist
//     runtime, así el daemon de monitor no puede leer un endpoint nuevo de la DB antes de que esté en
//     la allowlist (sino lo rechazaría). Nunca al revés.
//   - El health-check (`health_check`) SÍ es async (hace red). Timeout DURO 1500ms, SIN seguir
//     redirects (`redirect::Policy::none()`): sólo nos importa el status de ESE endpoint, no adónde
//     redirige (defensa SSRF + no colgar el wizard).
//   - El usuario PUEDE dejar un campo vacío (saltear) → ese endpoint NO se toca (cae al default
//     localhost del resolver). Foco humano: el wizard NO auto-configura, sólo persiste lo que el
//     usuario tipeó y confirmó.

use std::time::Duration;

use serde::Serialize;

use crate::settings as settings_store;

/// Un origin normalizado validado (esquema http(s), host no vacío, puerto explícito).
#[derive(Debug, Clone)]
pub struct ValidEndpoint {
    /// `scheme://host:port` — la forma canónica que va a la allowlist runtime.
    pub origin: String,
}

/// Valida una URL de endpoint candidata con `url::Url` (NO sólo un prefix-check: `http://` sin host
/// pasaría un check ingenuo). Acepta sólo http/https, host no vacío, sin credenciales embebidas.
/// Devuelve el origin canónico `scheme://host:port` (puerto explícito) para la allowlist.
pub fn validate_candidate_endpoint(url_str: &str) -> Result<ValidEndpoint, String> {
    let url = url::Url::parse(url_str).map_err(|e| format!("URL inválida: {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        s => return Err(format!("esquema '{s}' no permitido (usá http o https)")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs con credenciales embebidas no permitidas".to_string());
    }
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "URL sin host".to_string())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "no se pudo determinar el puerto".to_string())?;
    // Bracketear IPv6 para el origin canónico (un `::1` crudo no formaría un origin parseable).
    let host_for_origin = match url.host() {
        Some(url::Host::Ipv6(v6)) => format!("[{v6}]"),
        _ => host.to_string(),
    };
    let origin = format!("{}://{}:{}", url.scheme(), host_for_origin, port);
    Ok(ValidEndpoint { origin })
}

/// Resultado del health-check de UN endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResult {
    /// El endpoint respondió con un status 2xx.
    pub reachable: bool,
    /// Latencia de la respuesta (ms) cuando `reachable`.
    pub latency_ms: Option<u64>,
    /// Razón del fallo cuando `!reachable` (timeout / HTTP <status> / mensaje de transporte).
    pub error: Option<String>,
}

/// Par de resultados para AIE + Ollama (lo que el botón "Probar" del wizard pinta).
#[derive(Debug, Clone, Serialize)]
pub struct HealthPair {
    pub aie: HealthResult,
    pub ollama: HealthResult,
}

/// Health-check de los dos endpoints del wizard. Pinguea `{aie}/health` y `{ollama}/api/tags`.
/// Cliente con timeout DURO 1500ms y `redirect::Policy::none()` (sólo status, no seguir redirects).
/// Una URL vacía se reporta como "no configurado" (reachable=false) sin hacer red.
pub async fn health_check(aie_url: &str, ollama_url: &str) -> Result<HealthPair, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(1500))
        .build()
        .map_err(|e| e.to_string())?;

    let aie = ping_endpoint(&client, aie_url, "/health").await;
    let ollama = ping_endpoint(&client, ollama_url, "/api/tags").await;
    Ok(HealthPair { aie, ollama })
}

/// Pinguea un endpoint base + path. Valida la URL ANTES de salir a red (no pinguear un esquema raro).
async fn ping_endpoint(client: &reqwest::Client, base: &str, path: &str) -> HealthResult {
    let base = base.trim();
    if base.is_empty() {
        return HealthResult {
            reachable: false,
            latency_ms: None,
            error: Some("no configurado".to_string()),
        };
    }
    if let Err(e) = validate_candidate_endpoint(base) {
        return HealthResult {
            reachable: false,
            latency_ms: None,
            error: Some(e),
        };
    }
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let start = std::time::Instant::now();
    match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => HealthResult {
            reachable: true,
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Ok(r) => HealthResult {
            reachable: false,
            latency_ms: None,
            error: Some(format!("HTTP {}", r.status().as_u16())),
        },
        Err(e) if e.is_timeout() => HealthResult {
            reachable: false,
            latency_ms: None,
            error: Some("timeout".to_string()),
        },
        Err(e) => HealthResult {
            reachable: false,
            latency_ms: None,
            error: Some(e.to_string()),
        },
    }
}

/// Guarda los endpoints del wizard en `settings` y agrega sus hosts a la allowlist runtime.
/// SYNC (sin red). Lock-ordering DB→allowlist: retiene el `conn` hasta DESPUÉS de
/// `add_runtime_origin`. Un campo vacío NO se toca (cae al default localhost del resolver).
///
/// `conn`: ya bloqueado por el caller (el comando Tauri tiene el `state.db.lock()`).
pub fn save_endpoints(
    conn: &rusqlite::Connection,
    aie_url: &str,
    ollama_url: &str,
) -> Result<(), String> {
    let aie_url = aie_url.trim();
    let ollama_url = ollama_url.trim();

    // Validar PRIMERO ambos campos no vacíos (falla atómica: no persistimos nada si uno es inválido).
    let aie_valid = if aie_url.is_empty() {
        None
    } else {
        Some(validate_candidate_endpoint(aie_url).map_err(|e| format!("AIE: {e}"))?)
    };
    let ollama_valid = if ollama_url.is_empty() {
        None
    } else {
        Some(validate_candidate_endpoint(ollama_url).map_err(|e| format!("Ollama: {e}"))?)
    };

    // Calcular el nuevo array de `network.extra_origins` (MERGE con lo previo: no pisar otros
    // endpoints del usuario). Dedup exacto + CAP (audit codex/mistral): si el usuario cambia su host
    // repetidas veces, los origins viejos se acumularían; capeamos al mismo límite que el bootstrap
    // (MAX_RUNTIME_ORIGINS=256) descartando los MÁS VIEJOS, así un origin nuevo nunca queda fuera.
    const MAX_RUNTIME_ORIGINS: usize = 256;
    let mut origins: Vec<String> = match settings_store::get(conn, "network.extra_origins") {
        Ok(Some(serde_json::Value::Array(items))) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    };
    for v in [&aie_valid, &ollama_valid].into_iter().flatten() {
        if !origins.contains(&v.origin) {
            origins.push(v.origin.clone());
        }
    }
    if origins.len() > MAX_RUNTIME_ORIGINS {
        let overflow = origins.len() - MAX_RUNTIME_ORIGINS;
        origins.drain(0..overflow); // descarta los más viejos (frente del array)
    }

    // Persistir las TRES claves en UNA transacción (audit codex MED): si una falla, ninguna queda.
    // `settings::set` (autocommit) por sí solo dejaría un estado parcial ante un error a mitad.
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    if aie_valid.is_some() {
        // Guardamos el string TIPEADO por el usuario (el resolver ya strippea trailing slash al leer).
        settings_store::set(
            &tx,
            "endpoints.aie",
            &serde_json::Value::String(aie_url.to_string()),
        )
        .map_err(|e| e.to_string())?;
    }
    if ollama_valid.is_some() {
        settings_store::set(
            &tx,
            "endpoints.ollama",
            &serde_json::Value::String(ollama_url.to_string()),
        )
        .map_err(|e| e.to_string())?;
    }
    settings_store::set(
        &tx,
        "network.extra_origins",
        &serde_json::Value::Array(origins.into_iter().map(serde_json::Value::String).collect()),
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;

    // Actualizar la allowlist runtime MIENTRAS retenemos el `conn` (lock-ordering DB→allowlist). Esto
    // va DESPUÉS del commit pero ANTES de soltar el lock (el comando Tauri retiene `state.db.lock()`):
    // el daemon de monitor no puede leer la DB hasta que soltemos, así que verá settings+allowlist
    // consistentes. Si esto fallara, el origin ya está commiteado y el bootstrap lo recargará igual.
    for v in [&aie_valid, &ollama_valid].into_iter().flatten() {
        crate::bases::allowlist::add_runtime_origin(&v.origin)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_http_and_https_with_port() {
        assert_eq!(
            validate_candidate_endpoint("http://localhost:8250")
                .unwrap()
                .origin,
            "http://localhost:8250"
        );
        assert_eq!(
            validate_candidate_endpoint("https://aie.example.io")
                .unwrap()
                .origin,
            "https://aie.example.io:443"
        );
        // un 100.x Tailscale del usuario es válido (NO se bloquea ningún rango).
        assert_eq!(
            validate_candidate_endpoint("http://100.99.1.2:8250")
                .unwrap()
                .origin,
            "http://100.99.1.2:8250"
        );
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_candidate_endpoint("http://").is_err()); // sin host
        assert!(validate_candidate_endpoint("not a url").is_err());
        assert!(validate_candidate_endpoint("ftp://host:21").is_err()); // esquema
        assert!(validate_candidate_endpoint("file:///etc/passwd").is_err());
        assert!(validate_candidate_endpoint("http://user:pass@host:80").is_err()); // credenciales
    }

    #[test]
    fn validate_brackets_ipv6_origin() {
        let v = validate_candidate_endpoint("http://[::1]:8250").unwrap();
        assert_eq!(v.origin, "http://[::1]:8250");
    }

    #[test]
    fn save_endpoints_persists_and_allowlists() {
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        // Antes: el host del usuario NO está permitido.
        assert!(!crate::bases::allowlist::url_allowed("http://my-aie.example.io:8250/health"));

        save_endpoints(&conn, "http://my-aie.example.io:8250", "http://localhost:11434").unwrap();

        // settings guardados con el string del usuario.
        let aie = settings_store::get(&conn, "endpoints.aie").unwrap().unwrap();
        assert_eq!(aie.as_str().unwrap(), "http://my-aie.example.io:8250");
        let ollama = settings_store::get(&conn, "endpoints.ollama").unwrap().unwrap();
        assert_eq!(ollama.as_str().unwrap(), "http://localhost:11434");
        // extra_origins es un JSON array con los origins canónicos.
        let origins = settings_store::get(&conn, "network.extra_origins")
            .unwrap()
            .unwrap();
        let arr = origins.as_array().unwrap();
        assert!(arr.iter().any(|v| v.as_str() == Some("http://my-aie.example.io:8250")));
        // allowlist runtime: el host del usuario AHORA está permitido.
        assert!(crate::bases::allowlist::url_allowed("http://my-aie.example.io:8250/health"));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn save_endpoints_skips_empty_field() {
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        // Sólo AIE; Ollama vacío → NO se persiste endpoints.ollama.
        save_endpoints(&conn, "http://aie.example.io:8250", "").unwrap();
        assert!(settings_store::get(&conn, "endpoints.aie").unwrap().is_some());
        assert!(settings_store::get(&conn, "endpoints.ollama").unwrap().is_none());
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn save_endpoints_rejects_invalid_without_persisting() {
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        // AIE válido pero Ollama inválido → falla ANTES de persistir nada (atómico).
        let err = save_endpoints(&conn, "http://aie.example.io:8250", "ftp://bad").unwrap_err();
        assert!(err.contains("Ollama"));
        assert!(settings_store::get(&conn, "endpoints.aie").unwrap().is_none());
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn save_endpoints_merges_existing_origins() {
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));
             INSERT INTO settings (key,value) VALUES ('network.extra_origins', json('[\"https://existing.example.io:443\"]'));",
        )
        .unwrap();
        save_endpoints(&conn, "http://aie.example.io:8250", "").unwrap();
        let origins = settings_store::get(&conn, "network.extra_origins")
            .unwrap()
            .unwrap();
        let arr = origins.as_array().unwrap();
        // No pisa el origin previo; agrega el nuevo.
        assert!(arr.iter().any(|v| v.as_str() == Some("https://existing.example.io:443")));
        assert!(arr.iter().any(|v| v.as_str() == Some("http://aie.example.io:8250")));
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[test]
    fn save_endpoints_caps_origins_keeping_the_new_one() {
        crate::bases::allowlist::reset_runtime_hosts_for_test();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Pre-cargar 256 origins viejos (el cap del bootstrap). Al agregar uno nuevo, el más viejo
        // se descarta y el nuevo SIEMPRE queda (sino el bootstrap lo ignoraría silenciosamente).
        let pre: Vec<serde_json::Value> = (0..256)
            .map(|i| serde_json::Value::String(format!("https://old{i}.example.io:443")))
            .collect();
        let pre_json = serde_json::to_string(&serde_json::Value::Array(pre)).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT, updated_at TEXT DEFAULT (datetime('now')));",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key,value) VALUES ('network.extra_origins', ?1)",
            rusqlite::params![pre_json],
        )
        .unwrap();

        save_endpoints(&conn, "http://brand-new.example.io:8250", "").unwrap();
        let origins = settings_store::get(&conn, "network.extra_origins")
            .unwrap()
            .unwrap();
        let arr = origins.as_array().unwrap();
        assert_eq!(arr.len(), 256, "array capeado al máximo");
        assert!(
            arr.iter().any(|v| v.as_str() == Some("http://brand-new.example.io:8250")),
            "el origin nuevo nunca se descarta"
        );
        assert!(
            !arr.iter().any(|v| v.as_str() == Some("https://old0.example.io:443")),
            "el más viejo se descartó"
        );
        crate::bases::allowlist::reset_runtime_hosts_for_test();
    }

    #[tokio::test]
    async fn health_check_empty_urls_report_not_configured() {
        let pair = health_check("", "").await.unwrap();
        assert!(!pair.aie.reachable);
        assert_eq!(pair.aie.error.as_deref(), Some("no configurado"));
        assert!(!pair.ollama.reachable);
    }

    #[tokio::test]
    async fn health_check_invalid_url_reports_error_not_panic() {
        let pair = health_check("ftp://bad", "not a url").await.unwrap();
        assert!(!pair.aie.reachable);
        assert!(pair.aie.error.is_some());
        assert!(!pair.ollama.reachable);
        assert!(pair.ollama.error.is_some());
    }
}
