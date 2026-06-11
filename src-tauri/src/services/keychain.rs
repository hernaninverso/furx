// services/keychain.rs — credential storage via the `keyring` crate.
// Uses native macOS Security API (no argv leak — Gemini HIGH fix B1).
//
// On macOS the underlying call is `SecItemAdd` / `SecItemCopyMatching`, which never
// puts the secret in argv. `security add-generic-password -w SECRET` was the previous
// implementation; the secret was visible to `ps -ef` for any local user.

use anyhow::{anyhow, Result};
use keyring::Entry;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

pub const SERVICE_PROVIDER: &str = "furx-provider";

/// Aliases for which we've already logged "key came from the env override". Keeps the
/// operator signal (audit MED) to ONE info line per alias instead of one per dispatch.
fn logged_env_overrides() -> &'static Mutex<HashSet<String>> {
    static S: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Save secret idempotently. Existing entries are overwritten silently.
pub fn save(service: &str, account: &str, secret: &str) -> Result<()> {
    let entry = Entry::new(service, account).map_err(|e| anyhow!("keychain entry: {}", e))?;
    entry
        .set_password(secret)
        .map_err(|e| anyhow!("keychain set: {}", e))?;
    Ok(())
}

/// Load secret. Returns None if entry doesn't exist or is empty.
pub fn load(service: &str, account: &str) -> Option<String> {
    let entry = Entry::new(service, account).ok()?;
    let secret = entry.get_password().ok()?;
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

/// Load a BYOK provider key. Resolution order: env override → OS keychain.
///
/// The env var `FURX_PROVIDER_KEY_<ALIAS>` (alias upper-cased, `-`→`_`) lets Furx run
/// **without touching the OS keychain** — useful for CI, headless boxes, sandboxes, and
/// scripted demos where no interactive keychain unlock is possible. In a normal install
/// the var is never set, so behaviour is byte-identical to `load(SERVICE_PROVIDER, alias)`.
///
/// The override only applies to aliases in Furx's canonical format `[a-z0-9-]+`
/// ("openrouter-main", "cerebras-main", …). Over that domain the env-name mapping
/// (`-`→`_`, upper-case) is INJECTIVE, so no two aliases can collide on one env var and a
/// key can never be misrouted to the wrong provider (audit: without this guard `open-ai`
/// and `open_ai` would both map to `FURX_PROVIDER_KEY_OPEN_AI`). Aliases with any other
/// character skip the env path entirely and read the keychain.
///
/// Security note: env vars are visible to other processes of the same user (`ps`,
/// `/proc/self/environ`), child processes, and crash dumps — the keychain stays the
/// default for interactive desktop use; the override is explicit opt-in.
pub fn load_provider_key(alias: &str) -> Option<String> {
    let canonical = !alias.is_empty()
        && alias
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if canonical {
        let env_name = format!(
            "FURX_PROVIDER_KEY_{}",
            alias.to_ascii_uppercase().replace('-', "_")
        );
        if let Ok(k) = std::env::var(&env_name) {
            // trim: una env seteada a whitespace (" ") devolvería una key inválida que falla
            // auth de forma silenciosa y difícil de debuggear (audit H2). Vacía/whitespace →
            // caer al keychain como si no estuviera seteada.
            let trimmed = k.trim();
            if !trimmed.is_empty() {
                // Operator signal (audit MED): la identidad de runtime de este alias viene de
                // un env override, no del keychain — observable una vez por alias, NUNCA el valor.
                // El lock se suelta ANTES del tracing::info! (audit codex r3): el guard se dropea
                // al final de la expresión, así el macro de log no corre con el mutex tomado.
                let first_time = logged_env_overrides()
                    .lock()
                    .map(|mut seen| seen.insert(alias.to_string()))
                    .unwrap_or(false);
                if first_time {
                    tracing::info!(
                        alias = %alias, env = %env_name,
                        "provider key sourced from env override, not the OS keychain"
                    );
                }
                return Some(trimmed.to_string());
            }
        }
    }
    load(SERVICE_PROVIDER, alias)
}

/// Delete entry. Returns true if removed, false if didn't exist.
pub fn delete(service: &str, account: &str) -> bool {
    let Ok(entry) = Entry::new(service, account) else {
        return false;
    };
    entry.delete_credential().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El env override `FURX_PROVIDER_KEY_<ALIAS>` gana sobre el keychain y permite correr
    /// sin acceso al keychain (CI/headless/demo). Alias único por PID → no colisiona con
    /// otros tests concurrentes ni toca un secreto real.
    #[test]
    fn load_provider_key_prefers_env_override() {
        let alias = format!("demo-or-{}", std::process::id());
        let env_name = format!("FURX_PROVIDER_KEY_{}", alias.to_ascii_uppercase().replace('-', "_"));
        std::env::set_var(&env_name, "sk-or-env-override");
        assert_eq!(load_provider_key(&alias).as_deref(), Some("sk-or-env-override"));
        // whitespace-only → tratado como ausente, cae al keychain (audit H2).
        std::env::set_var(&env_name, "   ");
        assert!(load_provider_key(&alias).is_none());
        std::env::remove_var(&env_name);
        // sin env y sin entrada en el keychain (alias inexistente) → None.
        assert!(load_provider_key(&alias).is_none());
    }

    /// Guarda anti-colisión (audit codex HIGH): un alias NO canónico (con `_` o mayúscula)
    /// NUNCA usa el env override — aunque exista un env var que matchee su mapeo — para que
    /// la key de un alias no se enrute a otro provider. Va directo al keychain.
    #[test]
    fn load_provider_key_non_canonical_alias_skips_env() {
        let pid = std::process::id();
        // este alias tiene `_`: su mapeo colisionaría con el de un alias `-`. Debe ignorar el env.
        let bad_alias = format!("demo_or_{pid}");
        let env_name = format!("FURX_PROVIDER_KEY_DEMO_OR_{pid}");
        std::env::set_var(&env_name, "sk-should-be-ignored");
        // no canónico → ignora env → keychain (sin entrada) → None.
        assert!(load_provider_key(&bad_alias).is_none());
        std::env::remove_var(&env_name);
    }

    /// Fallback a keychain (audit LOW): alias canónico SIN env → lee la entrada del keychain.
    /// svc/acct pid-unique → no colisiona con otros procesos de test (mismo cuidado que el
    /// roundtrip de abajo, recurso global compartido).
    #[test]
    fn load_provider_key_falls_back_to_keychain() {
        let alias = format!("demo-kc-{}", std::process::id());
        let env_name = format!("FURX_PROVIDER_KEY_{}", alias.to_ascii_uppercase().replace('-', "_"));
        std::env::remove_var(&env_name); // asegurar que el env NO está
        save(SERVICE_PROVIDER, &alias, "sk-from-keychain").expect("save");
        assert_eq!(load_provider_key(&alias).as_deref(), Some("sk-from-keychain"));
        delete(SERVICE_PROVIDER, &alias);
    }

    #[test]
    fn save_load_delete_roundtrip() {
        // Per-process-unique svc/acct: this test hits the REAL macOS Keychain, a global
        // shared resource. With a fixed key, concurrent `cargo test` runs (e.g. parallel
        // worktree builds) collide on the same entry and flake. The pid makes each test
        // process touch its own entry.
        let pid = std::process::id();
        let svc = format!("furx-test-svc-keyring-{pid}");
        let acct = format!("furx-test-roundtrip-keyring-{pid}");
        let (svc, acct) = (svc.as_str(), acct.as_str());
        let _ = delete(svc, acct);
        save(svc, acct, "hello-secret").unwrap();
        let got = load(svc, acct);
        assert_eq!(got, Some("hello-secret".to_string()));
        // idempotencia: re-save sobreescribe
        save(svc, acct, "new-secret-v2").unwrap();
        assert_eq!(load(svc, acct), Some("new-secret-v2".to_string()));
        assert!(delete(svc, acct));
        assert_eq!(load(svc, acct), None);
    }
}
