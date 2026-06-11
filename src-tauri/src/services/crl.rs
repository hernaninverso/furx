// spec-050 · Ola 8 P2 (FR-005) — CRL con señalización activa.
//
// La Ola 4 dejó la revocación de skills como GATE de cargas FUTURAS: una key en `revoked_keys.txt`
// hace que el próximo gate resuelva `Rejected` (scripts inert). LIMITACIÓN documentada de P0: NO
// señalizaba al span que YA estaba corriendo — terminaba normal.
//
// Esta Ola 8 P2 cierra esa brecha: un registro de SPANS VIVOS (ejecuciones de skill en curso) atado
// a la signing-key de cada span + una bandera de aborto por span. Al revocar una key, NO sólo se
// bloquean cargas futuras: TODO span vivo firmado por esa key recibe la señal de aborto (fail-closed:
// el span chequea la bandera en sus await-points y corta).
//
// FAIL-CLOSED: si no se puede leer/escribir `revoked_keys.txt`, la revocación devuelve Err (no se
// asume "todo OK"). El registro de spans es best-effort: registrar/desregistrar nunca rompe el run;
// si el registro fallara, el span simplemente no es abortable en vivo (degrada al gate de carga).
//
// El span_id es opaco (un UUID de run). La signing-key es el SHA-256 hex (64 chars) de los bytes del
// pubkey Ed25519 — el MISMO formato que `revoked_keys.txt` y que el `key_id[..64]` del manifest.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Handle de un span vivo: su signing-key (si conocida) + la bandera de aborto compartida.
#[derive(Clone)]
struct SpanHandle {
    /// SHA-256 hex (64 chars) del pubkey que firmó el skill, si se conoce. `None` = sin firma
    /// (skill local/unsigned) → una revocación de key NUNCA lo aborta (no matchea ninguna key).
    signing_key: Option<String>,
    /// Bandera de aborto: el span la chequea en sus await-points. `true` = abortar.
    abort: Arc<AtomicBool>,
}

/// Registro global de spans vivos (run_id → handle). RwLock: muchas lecturas (check), escrituras
/// rarísimas (register/unregister/revoke). HashMap basta (decenas de spans concurrentes como mucho).
static LIVE_SPANS: once_cell::sync::Lazy<RwLock<HashMap<String, SpanHandle>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(HashMap::new()));

/// Guard RAII de un span vivo: registra al crear, DESREGISTRA al dropear (incluso si el run paniquea o
/// retorna temprano). Expone `aborted()` para que el span chequee si debe cortar.
///
/// REENTRANCIA (audit mistral MED): `Drop` toma `LIVE_SPANS.write()`. `parking_lot::RwLock` NO es
/// reentrante → NO dropear un `SpanGuard` mientras se tiene el write-lock de `LIVE_SPANS` en el MISMO
/// hilo (deadlock). En la práctica esto no pasa: el guard se dropea al final de `run_skill` (fuera de
/// cualquier sección que tome el lock; `register_span`/`signal_revoked_key` lo sueltan antes de
/// retornar). El guard nunca viaja dentro de un closure ejecutado bajo el lock.
pub struct SpanGuard {
    span_id: String,
    abort: Arc<AtomicBool>,
}

impl SpanGuard {
    /// `true` si este span fue señalizado para abortar (su key se revocó mientras corría).
    pub fn aborted(&self) -> bool {
        self.abort.load(Ordering::Acquire)
    }

    /// El id del span (para logs/correlación).
    pub fn span_id(&self) -> &str {
        &self.span_id
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        // Desregistro best-effort: el span ya terminó (normal o abortado), sale del registro.
        LIVE_SPANS.write().remove(&self.span_id);
    }
}

/// Registra un span vivo y devuelve su guard. `signing_key` = SHA-256 hex del pubkey firmante (o
/// `None` si el skill no está firmado). El guard desregistra al dropearse.
///
/// CIERRE DE LA VENTANA TOCTOU (audit deepseek HIGH R1): el chequeo "¿la key ya está revocada?" y el
/// INSERT en `LIVE_SPANS` se hacen bajo el MISMO write-lock de `LIVE_SPANS` que usa la señalización de
/// `revoke`. Como `revoke` también toma ese write-lock para marcar-revocado-y-señalizar, las dos
/// operaciones quedan SERIALIZADAS: o `register` ve la key ya revocada (el span nace abortado), o
/// `revoke` corre después y encuentra el span recién insertado en el HashMap (lo aborta). No hay
/// estado intermedio donde un span arranque sin abortar y la señal ya haya pasado.
pub fn register_span(span_id: impl Into<String>, signing_key: Option<String>) -> SpanGuard {
    let span_id = span_id.into();
    let abort = Arc::new(AtomicBool::new(false));
    // Bajo el write-lock: decidir born-aborted leyendo el cache de revocados Y insertar — atómico
    // frente a `revoke` (que toma el mismo lock para señalizar).
    {
        let mut spans = LIVE_SPANS.write();
        let already_revoked = match &signing_key {
            Some(k) => is_key_revoked_normalized(&k.trim().to_ascii_lowercase()),
            None => false,
        };
        if already_revoked {
            abort.store(true, Ordering::Release);
        }
        spans.insert(
            span_id.clone(),
            SpanHandle {
                signing_key: signing_key.clone(),
                abort: abort.clone(),
            },
        );
    }
    SpanGuard { span_id, abort }
}

/// Señaliza a TODOS los spans vivos firmados por `key_hex` que aborten. Devuelve cuántos señalizó.
/// `key_hex` se normaliza a minúsculas (consistente con el loader de revoked_keys). Idempotente:
/// re-señalizar un span ya abortado es no-op. Toma el write-lock de `LIVE_SPANS` (no read): así se
/// SERIALIZA con `register_span` (que también escribe) → cierra la ventana TOCTOU register/revoke.
pub fn signal_revoked_key(key_hex: &str) -> usize {
    let key = key_hex.trim().to_ascii_lowercase();
    if key.is_empty() {
        return 0;
    }
    // write-lock (no read): serializa con `register_span` para cerrar la ventana TOCTOU. No mutamos el
    // HashMap, sólo las banderas atómicas — el write-lock es por exclusión con el insert de register.
    let spans = LIVE_SPANS.write();
    let mut n = 0;
    for handle in spans.values() {
        if handle.signing_key.as_deref() == Some(key.as_str()) {
            handle.abort.store(true, Ordering::Release);
            n += 1;
        }
    }
    n
}

// ── revoked_keys.txt: append + lookup en memoria ─────────────────────────────────────────────────
//
// El SET de keys revocadas en MEMORIA (cache del archivo) — para que `register_span` sepa si una key
// ya está revocada sin tocar disco en el hot-path. Se siembra desde el archivo al primer uso y se
// actualiza al revocar.

static REVOKED_CACHE: once_cell::sync::Lazy<RwLock<Option<std::collections::HashSet<String>>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(None));

/// Path de `~/.furx/revoked_keys.txt` (override por env `FURX_REVOKED_KEYS_PATH` para tests).
fn revoked_keys_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("FURX_REVOKED_KEYS_PATH") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".furx")
        .join("revoked_keys.txt")
}

/// Siembra el cache desde el archivo (idempotente; sólo la 1ª vez o tras `reset_cache_for_test`).
fn ensure_cache() {
    if REVOKED_CACHE.read().is_some() {
        return;
    }
    let path = revoked_keys_path();
    let set = match crate::services::skill_manifest::load_revoked_keys(&path) {
        Ok(rk) => rk.keys,
        Err(_) => std::collections::HashSet::new(), // fail-open SÓLO para el cache de lookup; la
                                                    // revocación real (escritura) sí es fail-closed.
    };
    *REVOKED_CACHE.write() = Some(set);
}

/// `true` si la key (SHA-256 hex) está revocada (consulta el cache; lo siembra si hace falta).
pub fn is_key_revoked(key_hex: &str) -> bool {
    is_key_revoked_normalized(&key_hex.trim().to_ascii_lowercase())
}

/// Variante con la key YA normalizada (lowercase). La usa `register_span` mientras tiene el write-lock
/// de `LIVE_SPANS` (sin re-normalizar). Lee el cache de revocados (lock propio de REVOKED_CACHE, que
/// NUNCA se toma a la vez que se escribe `LIVE_SPANS` desde `revoke` → sin inversión de locks).
fn is_key_revoked_normalized(key: &str) -> bool {
    if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return false; // key malformada → no la consideramos revocada (el gate de carga la maneja).
    }
    ensure_cache();
    REVOKED_CACHE
        .read()
        .as_ref()
        .map(|s| s.contains(key))
        .unwrap_or(false)
}

/// Resultado de una revocación activa.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct RevokeResult {
    /// `true` si la key se agregó al archivo (o ya estaba).
    pub persisted: bool,
    /// Cuántos spans vivos se señalizaron para abortar.
    pub signaled_spans: usize,
}

/// REVOCA una key de forma ACTIVA (FR-005): (1) la persiste en `revoked_keys.txt` (bloquea cargas
/// futuras — comportamiento Ola 4), (2) actualiza el cache en memoria, (3) SEÑALIZA a todos los spans
/// vivos firmados por ella para que aborten. FAIL-CLOSED: si no se puede escribir el archivo,
/// devuelve Err (no se asume revocado). Valida el formato de la key (64 hex).
pub fn revoke_key(key_hex: &str) -> anyhow::Result<RevokeResult> {
    let key = key_hex.trim().to_ascii_lowercase();
    if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("key inválida: se esperan 64 chars hex (SHA-256 del pubkey)");
    }
    let path = revoked_keys_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("no se pudo crear {}: {e}", parent.display()))?;
    }
    // ¿ya estaba? (evita duplicar líneas). Cargamos el set vigente del archivo.
    let existing = crate::services::skill_manifest::load_revoked_keys(&path)
        .map(|rk| rk.keys)
        .unwrap_or_default();
    if !existing.contains(&key) {
        // Append atómico de UNA línea (la key + newline). Fail-closed: error de IO → Err.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("no se pudo abrir {}: {e}", path.display()))?;
        writeln!(f, "{key}").map_err(|e| anyhow::anyhow!("no se pudo escribir la key: {e}"))?;
    }
    // Actualizar el cache en memoria (re-siembra desde el archivo recién escrito para consistencia).
    *REVOKED_CACHE.write() = None;
    ensure_cache();
    // Señalizar spans vivos.
    let signaled = signal_revoked_key(&key);
    Ok(RevokeResult {
        persisted: true,
        signaled_spans: signaled,
    })
}

#[cfg(test)]
pub fn reset_cache_for_test() {
    *REVOKED_CACHE.write() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    // El env var `FURX_REVOKED_KEYS_PATH` + el cache de revocados son GLOBALES del proceso. Los tests
    // que los tocan se SERIALIZAN con este mutex para no pisarse bajo `cargo test` concurrente (mismo
    // patrón que el gotcha del Keychain/recurso-global). Los tests de spans puros (sin archivo) NO lo
    // necesitan: usan signing-keys únicas por test, así que sus señalizaciones no se cruzan.
    static FILE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // Cada test usa un archivo de revoked_keys propio (por-PID + nonce) para no colisionar bajo
    // `cargo test` concurrente (el cache + el path son globales del proceso).
    fn isolated_path(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "furx-crl-{}-{}-{}.txt",
            std::process::id(),
            tag,
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn key(n: u8) -> String {
        // 64 hex chars determinista.
        std::iter::repeat_n(format!("{n:02x}"), 32).collect()
    }

    #[test]
    fn signal_aborts_only_matching_live_spans() {
        let k1 = key(0x11);
        let k2 = key(0x22);
        let g1 = register_span("run-1", Some(k1.clone()));
        let g2 = register_span("run-2", Some(k2.clone()));
        let g3 = register_span("run-3", None); // unsigned → nunca se aborta por key.
        assert!(!g1.aborted() && !g2.aborted() && !g3.aborted());

        let n = signal_revoked_key(&k1);
        assert_eq!(n, 1, "sólo el span firmado por k1 se señaliza");
        assert!(g1.aborted(), "el span de k1 fue abortado");
        assert!(!g2.aborted(), "el span de k2 NO");
        assert!(!g3.aborted(), "el span unsigned NO");
    }

    #[test]
    fn drop_deregisters_span() {
        let k = key(0x33);
        {
            let _g = register_span("run-drop", Some(k.clone()));
            // dentro del scope está registrado → señalizar lo encuentra.
            assert_eq!(signal_revoked_key(&k), 1);
        }
        // tras el drop, ya no está → señalizar no encuentra nada.
        assert_eq!(signal_revoked_key(&k), 0);
    }

    #[test]
    fn revoke_persists_and_signals_live_span() {
        let _lock = FILE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = isolated_path("rev");
        std::env::set_var("FURX_REVOKED_KEYS_PATH", &path);
        reset_cache_for_test();
        let k = key(0x44);
        let g = register_span("run-rev", Some(k.clone()));

        let r = revoke_key(&k).unwrap();
        assert!(r.persisted);
        assert_eq!(r.signaled_spans, 1, "el span vivo de k fue señalizado");
        assert!(g.aborted(), "el span se abortó al revocar la key");
        // persistido en el archivo.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(&k), "la key quedó en revoked_keys.txt");

        // limpieza.
        std::env::remove_var("FURX_REVOKED_KEYS_PATH");
        reset_cache_for_test();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn span_registered_after_revoke_is_born_aborted() {
        let _lock = FILE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Carrera: si la key ya está revocada cuando un span arranca, nace abortado (cierra la ventana).
        let path = isolated_path("born");
        std::env::set_var("FURX_REVOKED_KEYS_PATH", &path);
        reset_cache_for_test();
        let k = key(0x55);
        revoke_key(&k).unwrap(); // sin spans vivos todavía.
        let g = register_span("run-late", Some(k.clone()));
        assert!(g.aborted(), "un span que arranca con la key ya revocada nace abortado");

        std::env::remove_var("FURX_REVOKED_KEYS_PATH");
        reset_cache_for_test();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn revoke_rejects_malformed_key() {
        assert!(revoke_key("not-hex").is_err());
        assert!(revoke_key(&"a".repeat(63)).is_err()); // 63 chars
        assert!(signal_revoked_key("") == 0);
    }

    // 050 FR-005 (audit deepseek HIGH R1) — ventana TOCTOU register/revoke: un span que arranca en
    // paralelo con un revoke de SU key DEBE terminar abortado (o nace abortado, o lo señaliza el
    // revoke). Estresamos el cruce N veces; en todas el span queda abortado (nunca escapa la señal).
    #[test]
    fn concurrent_register_during_revoke_always_aborts() {
        let _lock = FILE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = isolated_path("toctou");
        std::env::set_var("FURX_REVOKED_KEYS_PATH", &path);
        for i in 0..50u32 {
            // Limpiar estado entre iteraciones.
            reset_cache_for_test();
            let _ = std::fs::remove_file(&path);
            let k = key((i % 200) as u8);
            // Nota: distintas iteraciones pueden reusar la misma key, pero limpiamos el archivo+cache,
            // así que cada iter arranca "no revocada". El span registra en el hilo y revoke en otro.
            let kk = k.clone();
            let span_thread = std::thread::spawn(move || register_span(format!("r-{i}"), Some(kk)));
            let kk2 = k.clone();
            let revoke_thread = std::thread::spawn(move || {
                let _ = revoke_key(&kk2);
            });
            let guard = span_thread.join().unwrap();
            revoke_thread.join().unwrap();
            // Tras el cruce (SIN señal extra), el span DEBE estar abortado: por la serialización del
            // write-lock de LIVE_SPANS entre register y signal + el orden "cache antes que signal" en
            // revoke, todo entrelazado deja el span abortado (nació abortado o lo señalizó el revoke).
            assert!(
                guard.aborted(),
                "iter {i}: el span debe quedar abortado tras el cruce register/revoke (TOCTOU cerrado)"
            );
        }
        std::env::remove_var("FURX_REVOKED_KEYS_PATH");
        reset_cache_for_test();
        let _ = std::fs::remove_file(&path);
    }
}
