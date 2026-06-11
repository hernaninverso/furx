// services/aie_repl.rs — 009-aie-engine.
//
// Motor 'aie': un agente cuyo "CLI" es un REPL de chat liviano (Python stdlib, cero deps)
// que hace loop de inferencia HTTP contra el AIE (o un endpoint HTTP compatible). Se
// spawnea en un pane como cualquier proceso PTY. BYOK: el bearer se lee del Keychain en
// Rust y se pasa por env al subprocess (mismo patrón que los wrappers `*-as-<slug>`),
// NUNCA a SQLite, al audit, ni al backend Furx (F-I).

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::services::agent_profiles::AgentProfile;

type Db = Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// REPL client embebido. Pure stdlib (`urllib`) → corre con el `python3` del sistema sin pip.
/// Lee líneas de stdin, postea al endpoint y imprime la respuesta. Resiliente a errores
/// (timeout / !2xx / no-JSON / sin bearer): mensaje claro y sigue vivo.
pub const REPL_SCRIPT: &str = r#"#!/usr/bin/env python3
# Furx AIE chat REPL (009-aie-engine). Auto-generado — no editar a mano.
import os, sys, json, urllib.request, urllib.error

URL     = os.environ.get("FURX_AIE_URL", "http://localhost:8250/v1/infer")
BEARER  = os.environ.get("FURX_AIE_BEARER", "")
MODEL   = os.environ.get("FURX_MODEL", "").strip()
PROFILE = os.environ.get("FURX_AIE_PROFILE", "frontier_free").strip()
SYSTEM  = os.environ.get("FURX_SYSTEM_PROMPT", "").strip()

def banner():
    tag = MODEL or PROFILE
    sys.stdout.write("\033[2;36m[Furx · motor AIE · %s · %s]\033[0m\n" % (tag, URL))
    if not BEARER:
        sys.stdout.write("\033[31m⚠ Falta la credencial (FURX_AIE_BEARER): configurá el bearer en el Keychain del OS.\033[0m\n")
    sys.stdout.write("Escribí tu mensaje y Enter. Ctrl-D o 'salir' para terminar.\n")
    sys.stdout.flush()

def infer(prompt):
    body = {"prompt": prompt, "max_tokens": 2048}
    if MODEL:
        body["model"] = MODEL
    else:
        body["profile"] = PROFILE
    if SYSTEM:
        body["system"] = SYSTEM
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    if BEARER:
        headers["Authorization"] = "Bearer " + BEARER
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST", headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            payload = json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        detail = ""
        try:
            detail = e.read().decode()[:300]
        except Exception:
            pass
        return "\033[31m[HTTP %s] %s\033[0m" % (e.code, detail)
    except Exception as e:
        return "\033[31m[error de red/parseo] %s\033[0m" % e
    # AIE → {"text": ...}; OpenAI-compat → choices[0].message.content
    text = payload.get("text")
    if not text:
        try:
            text = payload["choices"][0]["message"]["content"]
        except Exception:
            text = json.dumps(payload)[:2000]
    return text

def main():
    banner()
    while True:
        try:
            sys.stdout.write("\033[36m› \033[0m")
            sys.stdout.flush()
            line = sys.stdin.readline()
            if not line:  # EOF (Ctrl-D)
                break
            line = line.strip()
            if not line:
                continue
            if line in ("salir", "exit", "quit"):
                break
            sys.stdout.write(infer(line) + "\n")
            sys.stdout.flush()
        except KeyboardInterrupt:
            sys.stdout.write("\n")
            continue
    sys.stdout.write("\033[2m[fin de la sesión AIE]\033[0m\n")
    sys.stdout.flush()

if __name__ == "__main__":
    main()
"#;

pub fn repl_script_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home dir"))?;
    Ok(home.join(".furx").join("furx-chat.py"))
}

/// Escribe el REPL a `~/.furx/furx-chat.py` (idempotente — re-escribe para mantenerlo al día
/// con la versión embebida). 0644. Devuelve el path.
pub fn ensure_repl_script() -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let p = repl_script_path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Escritura atómica (tmp único + rename) — boot y un spawn concurrente podrían escribir a
    // la vez; el tmp por-llamada evita corrupción y el rename atómico deja el contenido íntegro
    // (audit deepseek LOW).
    let tmp = p.with_extension(format!(
        "py.tmp.{}",
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, REPL_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
    }
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

/// Arma el env del REPL desde el agente + settings. El bearer sale del Keychain (service =
/// `account_slug` del agente, o `aie-internal-bearer` por default) y se pasa por env — nunca
/// se persiste. El endpoint sale del setting `aie_url` (default AIE local).
pub fn aie_env(agent: &AgentProfile, db: &Db) -> HashMap<String, String> {
    let mut env = HashMap::new();

    // Usar el helper canónico (lee `endpoints.aie`, default DEFAULT_AIE_URL) — NO un setting
    // ad-hoc, para respetar el backend que el user configuró (audit codex MED).
    let base = crate::services::aie_endpoint::resolve_url_arc(db);
    // SSRF guard: sólo http/https. Si el setting quedó con un scheme raro (file://, gopher…),
    // caer al default seguro en vez de inyectar el bearer a un destino arbitrario (audit MED).
    let base = if base.starts_with("http://") || base.starts_with("https://") {
        base
    } else {
        crate::services::aie_endpoint::DEFAULT_AIE_URL.to_string()
    };
    env.insert(
        "FURX_AIE_URL".to_string(),
        format!("{}/v1/infer", base.trim_end_matches('/')),
    );

    let svc = agent
        .account_slug
        .clone()
        .filter(|s| !s.is_empty())
        // 039 — reference the single canonical literal instead of re-spelling it (SC-004).
        .unwrap_or_else(|| crate::services::keychain_bearer::AIE_BEARER_SERVICE.to_string());
    let user = std::env::var("USER").unwrap_or_else(|_| "hernan".to_string());
    if let Some(bearer) = crate::services::keychain::load(&svc, &user) {
        env.insert("FURX_AIE_BEARER".to_string(), bearer);
    }

    match agent.model.as_deref().filter(|s| !s.is_empty()) {
        Some(m) => {
            env.insert("FURX_MODEL".to_string(), m.to_string());
        }
        None => {
            // sin model → usar un profile (council_preset si está, sino frontier_free)
            let profile = agent
                .council_preset
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "frontier_free".to_string());
            env.insert("FURX_AIE_PROFILE".to_string(), profile);
        }
    }
    if !agent.system_prompt.trim().is_empty() {
        env.insert(
            "FURX_SYSTEM_PROMPT".to_string(),
            agent.system_prompt.clone(),
        );
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(engine: &str, model: Option<&str>, slug: Option<&str>) -> AgentProfile {
        AgentProfile {
            id: "x".into(),
            name: "n".into(),
            description: String::new(),
            cli_kind: "claude".into(),
            account_slug: slug.map(String::from),
            model: model.map(String::from),
            system_prompt: "sé conciso".into(),
            default_cwd: None,
            council_enabled: false,
            council_preset: None,
            shell_enabled: false,
            icon: None,
            color: None,
            is_builtin: false,
            engine_kind: engine.into(),
            category: None,
            plugins: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn script_is_valid_python_shape() {
        assert!(REPL_SCRIPT.contains("urllib.request"));
        assert!(REPL_SCRIPT.contains("FURX_AIE_BEARER"));
        assert!(REPL_SCRIPT.contains("def main()"));
    }

    #[test]
    fn env_uses_profile_when_no_model_and_carries_system() {
        let db: Db = Arc::new(parking_lot::Mutex::new(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        // sin tabla settings → get devuelve Err → default URL
        let env = aie_env(&mk("aie", None, None), &db);
        // sin endpoints.aie en settings → DEFAULT_AIE_URL del helper canónico + /v1/infer
        assert_eq!(
            env.get("FURX_AIE_URL").unwrap(),
            &format!(
                "{}/v1/infer",
                crate::services::aie_endpoint::DEFAULT_AIE_URL
            )
        );
        assert_eq!(env.get("FURX_AIE_PROFILE").unwrap(), "frontier_free");
        assert!(!env.contains_key("FURX_MODEL"));
        assert_eq!(env.get("FURX_SYSTEM_PROMPT").unwrap(), "sé conciso");
    }

    #[test]
    fn env_uses_model_when_set() {
        let db: Db = Arc::new(parking_lot::Mutex::new(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let env = aie_env(&mk("aie", Some("gpt-oss-120b"), None), &db);
        assert_eq!(env.get("FURX_MODEL").unwrap(), "gpt-oss-120b");
        assert!(!env.contains_key("FURX_AIE_PROFILE"));
    }
}
