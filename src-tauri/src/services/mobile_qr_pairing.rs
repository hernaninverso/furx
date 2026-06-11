// services/mobile_qr_pairing.rs — pairing por QR del companion (spec 065, council máximo v4).
//
// El secreto permanente de 64-hex NUNCA sale del Keychain en el QR. El QR codifica un TOKEN EFÍMERO
// de 32 bytes (OsRng), un solo uso, TTL 120s. El companion lo escanea, conecta al bridge por WS
// (pre-Hello, sin HMAC), manda `PairingRedeem{token}`, el bridge canjea el token por el secreto y lo
// entrega en `PairingGrant`. Idempotente ante WS-drops (ventana de 10s). Modelo de amenaza en
// specs/065-qr-pairing/council-design.md §12.
//
// Diseño de testeo: la LÓGICA vive en métodos de `PairingSessions` (puros sobre una instancia) y el
// estado global es un wrapper fino. Así los tests corren sobre instancias frescas, sin colisionar en el
// static bajo `cargo test` paralelo (mismo gotcha que el Keychain global). El Keychain IO (load_secret)
// se hace SIEMPRE fuera del lock.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Once;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PAIR_TTL_SECS: u64 = 120; // vida del token en el QR
const GRACE_SECS: u64 = 45; // gracia de expiración (> intervalo de cleanup 30s → sin race)
const RESEND_GRACE: Duration = Duration::from_secs(10); // reenvío del grant ante WS-drop
const INFLIGHT_GRACE: Duration = Duration::from_secs(30); // grant en vuelo preservado en rotación
const DONE_GRACE: Duration = Duration::from_secs(60); // grant completado purgado por cleanup
const MAX_SESSIONS: usize = 50;
const CLEANUP_INTERVAL_SECS: u64 = 30;
const SHORT_ALPHA: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ"; // sin 0/1/I/O ambiguos

pub struct PairingSession {
    pub token_hex: String,
    pub session_id: String,
    pub exp_epoch: u64, // epoch Unix — coordinado con el JS del companion
    pub used: bool,
    pub send_ts: Option<Instant>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub short_code: String,
}

#[derive(Debug)]
pub struct NewSession {
    pub token_hex: String,
    pub session_id: String,
    pub short_code: String,
    pub exp_epoch: u64,
}

/// Resultado de marcar un canje BAJO EL LOCK (sin tocar el Keychain todavía).
enum RedeemMark {
    /// Marcado used por primera vez (o reintento dentro de gracia) → el caller debe cargar el secreto.
    Proceed { session_id: String },
    AlreadyUsed,
    Expired,
    Invalid,
}

/// Resultado público del canje (ya con el secreto cargado fuera del lock).
pub enum RedeemResult {
    Grant { secret: String, session_id: String },
    AlreadyUsed,
    Expired,
    Invalid,
    SecretLoadFailed,
}

pub enum SessionStatus {
    Pending,
    Completed,
    Expired,
}

#[derive(Default)]
struct PairingSessions {
    by_token: HashMap<String, PairingSession>,
    by_short: HashMap<String, String>,   // short_code → token_hex
    by_session: HashMap<String, String>, // session_id → token_hex
}

impl PairingSessions {
    fn new() -> Self {
        Self::default()
    }

    /// Remueve un token de los TRES mapas (evita huérfanos en by_short/by_session — bug del §4).
    fn remove(&mut self, token: &str) {
        if let Some(s) = self.by_token.remove(token) {
            self.by_short.remove(&s.short_code);
            self.by_session.remove(&s.session_id);
        }
    }

    /// Purga sesiones expiradas (exp + gracia) de los tres mapas.
    fn prune_expired(&mut self, now_epoch: u64) {
        let expired: Vec<String> = self
            .by_token
            .iter()
            .filter(|(_, s)| now_epoch >= s.exp_epoch + GRACE_SECS)
            .map(|(k, _)| k.clone())
            .collect();
        for t in expired {
            self.remove(&t);
        }
    }

    /// Genera una sesión nueva. Soft-limit de 50 sesiones activas.
    fn generate(&mut self, now_epoch: u64) -> Result<NewSession, &'static str> {
        self.prune_expired(now_epoch);
        if self.by_token.len() >= MAX_SESSIONS {
            return Err("too_many_pending_sessions");
        }
        let exp_epoch = now_epoch + PAIR_TTL_SECS;
        for _ in 0..100 {
            let token_hex = gen_token_hex();
            let short = token_to_short(&token_hex);
            if self.by_short.contains_key(&short) {
                continue;
            }
            let session_id = uuid::Uuid::new_v4().to_string();
            self.by_short.insert(short.clone(), token_hex.clone());
            self.by_session.insert(session_id.clone(), token_hex.clone());
            self.by_token.insert(
                token_hex.clone(),
                PairingSession {
                    token_hex: token_hex.clone(),
                    session_id: session_id.clone(),
                    exp_epoch,
                    used: false,
                    send_ts: None,
                    device_id: None,
                    device_name: None,
                    short_code: short.clone(),
                },
            );
            return Ok(NewSession {
                token_hex,
                session_id,
                short_code: short,
                exp_epoch,
            });
        }
        Err("short_code_collision")
    }

    fn token_for_short(&self, short_code: &str, now_epoch: u64) -> Option<String> {
        let token = self.by_short.get(short_code)?.clone();
        let session = self.by_token.get(&token)?;
        if now_epoch >= session.exp_epoch + GRACE_SECS {
            return None;
        }
        Some(token)
    }

    /// Marca el canje BAJO EL LOCK. NO toca el Keychain. Exactamente un caller obtiene `Proceed` en
    /// primer canje; reintentos dentro de gracia también obtienen `Proceed` (idempotencia WS-drop).
    fn mark_redeem(
        &mut self,
        token: &str,
        device_id: &str,
        device_name: &str,
        now_epoch: u64,
        now_instant: Instant,
    ) -> RedeemMark {
        let session = match self.by_token.get_mut(token) {
            Some(s) => s,
            None => return RedeemMark::Invalid,
        };
        if now_epoch >= session.exp_epoch + GRACE_SECS {
            return RedeemMark::Expired;
        }
        if session.used {
            // El retry idempotente (WS-drop / send fallido) SOLO se concede al MISMO device que canjeó
            // primero (audit codex+deepseek BLOCKER): sin esto, dos redeem concurrentes con send_ts=None
            // obtenían AMBOS un Grant → un atacante en LAN sacaba el secreto sin que el móvil legítimo se
            // enterara. Con el bind: el 2º device distinto → AlreadyUsed (señal para rotar). El council
            // había descartado device_id binding, pero el doble-grant lo justifica.
            let same_device = session.device_id.as_deref() == Some(device_id);
            let in_grace = same_device
                && match session.send_ts {
                    None => true, // grant en vuelo o send fallido del MISMO device → reintento OK
                    Some(t) => now_instant.duration_since(t) < RESEND_GRACE,
                };
            if in_grace {
                RedeemMark::Proceed {
                    session_id: session.session_id.clone(),
                }
            } else {
                RedeemMark::AlreadyUsed
            }
        } else {
            session.used = true;
            session.device_id = Some(device_id.to_string());
            session.device_name = Some(device_name.to_string());
            RedeemMark::Proceed {
                session_id: session.session_id.clone(),
            }
        }
    }

    fn mark_grant_sent(&mut self, token: &str, now_instant: Instant) {
        if let Some(s) = self.by_token.get_mut(token) {
            s.send_ts = Some(now_instant);
        }
    }

    fn session_status(&self, session_id: &str) -> SessionStatus {
        let token = match self.by_session.get(session_id) {
            Some(t) => t,
            None => return SessionStatus::Expired,
        };
        match self.by_token.get(token) {
            None => SessionStatus::Expired,
            Some(s) if s.used && s.send_ts.is_some() => SessionStatus::Completed,
            Some(_) => SessionStatus::Pending,
        }
    }

    /// Conserva grants en vuelo (used + send_ts < 30s); purga el resto. Llamado por rotate_secret().
    fn clear_pending(&mut self, now_instant: Instant) {
        let to_remove: Vec<String> = self
            .by_token
            .iter()
            .filter(|(_, s)| {
                let in_flight = s.used
                    && s.send_ts
                        .is_none_or(|t| now_instant.duration_since(t) < INFLIGHT_GRACE);
                !in_flight
            })
            .map(|(k, _)| k.clone())
            .collect();
        for token in to_remove {
            self.remove(&token);
        }
    }

    /// Cleanup periódico: expirados (exp+gracia) o completados hace >60s.
    fn cleanup(&mut self, now_epoch: u64, now_instant: Instant) {
        let drop: Vec<String> = self
            .by_token
            .iter()
            .filter(|(_, s)| {
                let age_expired = now_epoch >= s.exp_epoch + GRACE_SECS;
                let done = s.used
                    && s.send_ts
                        .is_some_and(|t| now_instant.duration_since(t) > DONE_GRACE);
                age_expired || done
            })
            .map(|(k, _)| k.clone())
            .collect();
        for token in drop {
            self.remove(&token);
        }
    }
}

static PAIRING: Lazy<Mutex<PairingSessions>> = Lazy::new(|| Mutex::new(PairingSessions::new()));

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn gen_token_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 8 chars Base32-sin-ambiguos derivados de los primeros 5 bytes del token (fallback de tipeo manual).
fn token_to_short(token_hex: &str) -> String {
    let bytes = hex::decode(token_hex).unwrap_or_default();
    let n: u64 = bytes[..5.min(bytes.len())]
        .iter()
        .fold(0u64, |acc, &b| acc.wrapping_mul(256).wrapping_add(b as u64));
    let base = SHORT_ALPHA.len() as u64;
    let mut s = Vec::with_capacity(8);
    let mut v = n;
    for _ in 0..8 {
        s.push(SHORT_ALPHA[(v % base) as usize]);
        v /= base;
    }
    s.reverse();
    String::from_utf8(s).expect("SHORT_ALPHA is ASCII")
}

// ───────────────────────────── API global (wrapper fino) ─────────────────────────────

/// Genera una sesión de pairing (token efímero + short_code + session_id + exp).
pub fn generate_session() -> Result<NewSession, &'static str> {
    PAIRING.lock().generate(now_epoch())
}

pub fn token_for_short(short_code: &str) -> Option<String> {
    PAIRING.lock().token_for_short(short_code, now_epoch())
}

/// Canjea un token. Marca used BAJO EL LOCK, suelta el lock, y RECIÉN ahí toca el Keychain.
/// `mark_grant_sent` lo llama el caller tras un send exitoso.
pub fn redeem(token: &str, device_id: &str, device_name: &str) -> RedeemResult {
    let now_instant = Instant::now();
    let mark = {
        let mut sessions = PAIRING.lock();
        sessions.mark_redeem(token, device_id, device_name, now_epoch(), now_instant)
    }; // lock liberado ANTES del Keychain IO
    match mark {
        RedeemMark::Proceed { session_id } => match load_secret() {
            Ok(secret) => RedeemResult::Grant { secret, session_id },
            Err(()) => RedeemResult::SecretLoadFailed,
        },
        RedeemMark::AlreadyUsed => RedeemResult::AlreadyUsed,
        RedeemMark::Expired => RedeemResult::Expired,
        RedeemMark::Invalid => RedeemResult::Invalid,
    }
}

fn load_secret() -> Result<String, ()> {
    crate::services::keychain::load(
        crate::services::mobile_bridge::KEYCHAIN_SVC_MOBILE,
        crate::services::mobile_bridge::KEYCHAIN_ACCT_SECRET,
    )
    .filter(|s| !s.is_empty())
    .ok_or(())
}

pub fn mark_grant_sent(token: &str) {
    PAIRING.lock().mark_grant_sent(token, Instant::now());
}

pub fn session_status(session_id: &str) -> SessionStatus {
    PAIRING.lock().session_status(session_id)
}

/// Llamado por rotate_secret(): purga sesiones pendientes (conserva grants en vuelo).
pub fn clear_pending_sessions() {
    PAIRING.lock().clear_pending(Instant::now());
}

static CLEANUP_ONCE: Once = Once::new();

/// Arranca el task de limpieza periódica (idempotente: una sola instancia por proceso).
pub fn spawn_cleanup_task() {
    CLEANUP_ONCE.call_once(|| {
        tauri::async_runtime::spawn(async {
            let mut ticker =
                tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                PAIRING.lock().cleanup(now_epoch(), Instant::now());
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk() -> PairingSessions {
        PairingSessions::new()
    }
    const T0: u64 = 1_700_000_000;

    #[test]
    fn gen_token_is_64_hex() {
        let t = gen_token_hex();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn token_to_short_8_chars_no_ambiguous() {
        let s = token_to_short(&gen_token_hex());
        assert_eq!(s.len(), 8);
        assert!(s.chars().all(|c| !matches!(c, '0' | '1' | 'I' | 'O')));
        assert!(s.chars().all(|c| SHORT_ALPHA.contains(&(c as u8))));
    }

    #[test]
    fn pairing_token_single_use() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        // primer canje → Proceed
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", T0, i),
            RedeemMark::Proceed { .. }
        ));
        // marca enviado, pasa la gracia → AlreadyUsed
        s.mark_grant_sent(&ns.token_hex, i - Duration::from_secs(11));
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", T0, i),
            RedeemMark::AlreadyUsed
        ));
    }

    #[test]
    fn pairing_token_expired() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        // now > exp + 45
        let later = ns.exp_epoch + GRACE_SECS + 1;
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", later, Instant::now()),
            RedeemMark::Expired
        ));
    }

    #[test]
    fn pairing_token_invalid() {
        let mut s = mk();
        assert!(matches!(
            s.mark_redeem("deadbeef", "d", "n", T0, Instant::now()),
            RedeemMark::Invalid
        ));
    }

    #[test]
    fn pairing_idempotent_send_failed() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "d", "n", T0, i); // used=true, send_ts=None
        // sin mark_grant_sent → send_ts None → reintento Proceed
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", T0, i),
            RedeemMark::Proceed { .. }
        ));
    }

    #[test]
    fn pairing_idempotent_ws_drop() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "d", "n", T0, i);
        s.mark_grant_sent(&ns.token_hex, i); // send_ts ahora; elapsed < 10s
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", T0, i),
            RedeemMark::Proceed { .. }
        ));
    }

    #[test]
    fn pairing_rotate_keeps_inflight() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "d", "n", T0, i);
        s.mark_grant_sent(&ns.token_hex, i); // en vuelo (< 30s)
        s.clear_pending(i);
        assert!(s.by_token.contains_key(&ns.token_hex), "grant en vuelo se conserva");
    }

    #[test]
    fn pairing_rotate_clears_pending() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap(); // used=false
        s.clear_pending(Instant::now());
        assert!(!s.by_token.contains_key(&ns.token_hex), "pendiente sin usar se purga");
        // y sin huérfanos
        assert!(!s.by_short.contains_key(&ns.short_code));
        assert!(!s.by_session.contains_key(&ns.session_id));
    }

    #[test]
    fn pairing_cleanup_removes_expired() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        s.cleanup(ns.exp_epoch + GRACE_SECS + 1, Instant::now());
        assert!(s.by_token.is_empty());
        assert!(s.by_short.is_empty(), "sin huérfanos en by_short");
        assert!(s.by_session.is_empty(), "sin huérfanos en by_session");
    }

    #[test]
    fn pairing_cleanup_removes_done() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "d", "n", T0, i);
        s.mark_grant_sent(&ns.token_hex, i - Duration::from_secs(61)); // hace >60s
        s.cleanup(T0, i);
        assert!(s.by_token.is_empty(), "grant completado >60s se purga");
    }

    #[test]
    fn session_status_completed_and_expired() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        assert!(matches!(s.session_status(&ns.session_id), SessionStatus::Pending));
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "d", "n", T0, i);
        s.mark_grant_sent(&ns.token_hex, i);
        assert!(matches!(s.session_status(&ns.session_id), SessionStatus::Completed));
        assert!(matches!(s.session_status("desconocido"), SessionStatus::Expired));
    }

    #[test]
    fn grace_period_45s() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        // now < exp + 45 → aceptado (Proceed)
        let within = ns.exp_epoch + GRACE_SECS - 1;
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "d", "n", within, Instant::now()),
            RedeemMark::Proceed { .. }
        ));
    }

    #[test]
    fn soft_limit_50_sessions() {
        let mut s = mk();
        for _ in 0..MAX_SESSIONS {
            s.generate(T0).unwrap();
        }
        assert_eq!(s.generate(T0).unwrap_err(), "too_many_pending_sessions");
    }

    #[test]
    fn short_code_resolves_and_expires() {
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        assert_eq!(s.token_for_short(&ns.short_code, T0), Some(ns.token_hex.clone()));
        // expirado → None
        assert_eq!(
            s.token_for_short(&ns.short_code, ns.exp_epoch + GRACE_SECS + 1),
            None
        );
        assert_eq!(s.token_for_short("ZZZZZZZZ", T0), None);
    }

    #[test]
    fn concurrent_redeem_one_proceeds() {
        // Dos redeem concurrentes con device DISTINTO: el 1º marca used+device, el 2º → AlreadyUsed
        // AUNQUE send_ts siga None (dentro de gracia). Cierra el doble-grant (BLOCKER audit codex+deepseek).
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let a = s.mark_redeem(&ns.token_hex, "a", "A", T0, i);
        let b = s.mark_redeem(&ns.token_hex, "b", "B", T0, i); // device distinto, sin mark_grant_sent
        assert!(matches!(a, RedeemMark::Proceed { .. }));
        assert!(matches!(b, RedeemMark::AlreadyUsed), "un device distinto NO obtiene segundo grant");
    }

    #[test]
    fn retry_same_device_within_grace_proceeds() {
        // El MISMO device puede reintentar (WS-drop): used + send_ts fresco + mismo device → Proceed.
        let mut s = mk();
        let ns = s.generate(T0).unwrap();
        let i = Instant::now();
        let _ = s.mark_redeem(&ns.token_hex, "phone-A", "iPhone", T0, i);
        s.mark_grant_sent(&ns.token_hex, i);
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "phone-A", "iPhone", T0, i),
            RedeemMark::Proceed { .. }
        ));
        // pero otro device dentro de la misma gracia → AlreadyUsed
        assert!(matches!(
            s.mark_redeem(&ns.token_hex, "attacker", "X", T0, i),
            RedeemMark::AlreadyUsed
        ));
    }
}
