//! Furx Memory — binding seguro a `corpus-engine` (spec 066).
//!
//! corpus-engine es un motor local-first (Python, en un venv) que procesó el corpus de sesiones
//! de Claude Code del usuario y ya saneó todo secreto (firewall fail-closed). Furx lo consume por
//! subprocess (NO rusqlite directo: desacople + el saneo ya está hecho upstream).
//!
//! Council deepseek+codex (APPROVE con guardrails, aplicados acá): envelope de errores tipados,
//! byte-cap REAL de stdout, timeouts por comando, retry en DB-locked (ingesta), resolución de
//! binario con override + canonicalize, env sanitizado, parseo estricto forward-compatible.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024; // tope duro: rechaza un blob gigante
const MAX_QUERY_LEN: usize = 512;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;
const MIN_SCHEMA_VERSION: u64 = 2; // corpus-engine emite schema_version=2 hoy
const LOCKED_BACKOFF_MS: u64 = 600;

/// Argumentos interpolados (project/kind) deben matchear esto (argv-only igual; defensa extra).
fn is_safe_arg(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'/' | b'@' | b'-'))
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CorpusError {
    NotInstalled,
    Timeout,
    Locked,
    InvalidJson,
    IncompatibleVersion,
    OutputTooLarge,
    BadInput,
}

/// Envelope común (council): la UI distingue "no hay datos" de "el motor falló/indexando".
#[derive(Debug, Clone, Serialize)]
pub struct CorpusResult<T: Serialize> {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<CorpusError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl<T: Serialize> CorpusResult<T> {
    fn ok(data: T) -> Self {
        Self { available: true, data: Some(data), error_code: None, error_message: None }
    }
    fn err(code: CorpusError, msg: impl Into<String>) -> Self {
        // NotInstalled = no disponible; el resto = disponible-pero-falló (la UI lo refleja distinto).
        Self {
            available: code != CorpusError::NotInstalled,
            data: None,
            error_code: Some(code),
            error_message: Some(msg.into()),
        }
    }
}

// ── structs serde estrictos: SOLO los campos que se muestran (sin cwd/git_branch crudos).
// Sin deny_unknown_fields → ignora campos nuevos = forward-compatible.

/// corpus-engine emite algunos enteros como STRING ("2") y otros como número. Aceptar ambos
/// (verificado contra el corpus real: `schema_version` viene como `"2"`).
fn de_u64_flexible<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| Error::custom("not u64")),
        serde_json::Value::String(s) => s.trim().parse().map_err(|_| Error::custom("bad u64 string")),
        _ => Err(Error::custom("expected u64 or string")),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusStatus {
    #[serde(deserialize_with = "de_u64_flexible")]
    pub schema_version: u64,
    #[serde(deserialize_with = "de_u64_flexible")]
    pub sessions: u64,
    #[serde(deserialize_with = "de_u64_flexible")]
    pub messages: u64,
    #[serde(default, deserialize_with = "de_u64_flexible")]
    pub tool_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub uuid: String,
    pub session: String,
    pub project: String,
    pub timestamp: String,
    pub human: bool,
    pub snippet: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub n: u64,
    pub results: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deadend {
    pub signature: String,
    pub count: u64,
    #[serde(default)]
    pub example: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deadends {
    pub distinct_errors: u64,
    pub recurrent: Vec<Deadend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    #[serde(rename = "type")]
    pub kind: String,
    pub timestamp: String,
    pub project: String,
    pub session: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ledger {
    pub n: u64,
    pub decisions: Vec<Decision>,
}

/// El venv de Python pone el binario en `bin/` (Unix) o `Scripts/...exe` (Windows).
#[cfg(windows)]
const VENV_REL: &str = "corpus-engine/.venv/Scripts/corpus-engine.exe";
#[cfg(not(windows))]
const VENV_REL: &str = "corpus-engine/.venv/bin/corpus-engine";

/// Orden de resolución del binario: env override → venv canónico → PATH.
/// Usa los helpers cross-platform de `platform::` (audit: NO duplicar which/is_executable — el
/// `where`/`which` con ruta absoluta y el chequeo de extensión viven en un solo lugar).
fn resolve_bin() -> Option<PathBuf> {
    use crate::services::platform::{is_executable, which};
    if let Ok(p) = std::env::var("FURX_CORPUS_ENGINE_BIN") {
        let pb = PathBuf::from(p);
        if is_executable(&pb) {
            return std::fs::canonicalize(&pb).ok();
        }
    }
    if let Some(home) = dirs::home_dir() {
        let venv = home.join(VENV_REL);
        if is_executable(&venv) {
            return std::fs::canonicalize(&venv).ok();
        }
    }
    // PATH fallback (which/where con ruta absoluta) — sólo si está instalado globalmente.
    if let Some(pb) = which("corpus-engine") {
        if is_executable(&pb) {
            return std::fs::canonicalize(&pb).ok();
        }
    }
    None
}

/// Lee `r` hasta EOF para no bloquear al proceso (un pipe lleno lo cuelga), pero guarda a lo sumo
/// `cap` bytes en memoria. Devuelve (bytes_guardados, overflow) — overflow = hubo más de `cap`.
async fn drain_capped<R>(mut r: R, cap: usize) -> (Vec<u8>, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut kept = Vec::with_capacity(8192.min(cap));
    let mut chunk = [0u8; 16384];
    let mut overflow = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break, // EOF o error → terminamos de drenar
            Ok(k) => {
                if kept.len() < cap {
                    let room = cap - kept.len();
                    kept.extend_from_slice(&chunk[..k.min(room)]);
                    if k > room {
                        overflow = true;
                    }
                } else {
                    overflow = true;
                }
                // seguimos leyendo (descartando) aunque ya no guardemos → el proceso no se bloquea.
            }
        }
    }
    (kept, overflow)
}

const LOCK_NEEDLE: &[u8] = b"database is locked";

/// Drena stderr ENTERO (no bloquear el proceso) buscando el patrón de SQLite-locked en CUALQUIER
/// posición —incluso a caballo entre chunks— en vez de sólo sobre los primeros KB (audit codex r3:
/// el mensaje podría llegar pasado el cap). No guarda el stderr (sólo nos importa el flag).
async fn stderr_locked<R>(mut r: R) -> bool
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 16384];
    let mut tail: Vec<u8> = Vec::new(); // cola del chunk previo para match cross-boundary
    let mut seen = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                if !seen {
                    let mut window = std::mem::take(&mut tail);
                    window.extend_from_slice(&chunk[..k]);
                    window.make_ascii_lowercase();
                    if window
                        .windows(LOCK_NEEDLE.len())
                        .any(|w| w == LOCK_NEEDLE)
                    {
                        seen = true;
                    }
                    // conservar los últimos (needle-1) bytes para el próximo cruce.
                    let keep = LOCK_NEEDLE.len().saturating_sub(1);
                    let start = window.len().saturating_sub(keep);
                    tail = window[start..].to_vec();
                }
                // si ya lo vimos, seguimos leyendo igual (drenar hasta EOF → no bloquear).
            }
        }
    }
    seen
}

/// Corre `corpus-engine <args>` con byte-cap real, timeout, env sanitizado y kill_on_drop.
/// Devuelve el stdout (string) o un CorpusError tipado. NO hace retry (lo hace `run`).
async fn run_once(bin: &std::path::Path, args: &[&str], timeout: Duration) -> Result<String, CorpusError> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // env sanitizado: que un venv ajeno no secuestre el intérprete.
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONSTARTUP");

    let mut child = cmd.spawn().map_err(|_| CorpusError::NotInstalled)?;
    let stdout = child.stdout.take().ok_or(CorpusError::InvalidJson)?;
    let stderr = child.stderr.take().ok_or(CorpusError::InvalidJson)?;

    let work = async {
        // Drenar AMBOS pipes hasta EOF, CONCURRENTEMENTE, guardando sólo `cap` bytes en memoria.
        // Clave (audit codex): drenar hasta EOF —no `take()` que para al cap— para que el proceso
        // NUNCA se bloquee escribiendo a un pipe lleno; así `child.wait()` retorna en vez de colgar
        // (y un stdout gigante sale como OutputTooLarge, no Timeout). El stderr se drena entero pero
        // sólo guardamos 64KB (suficiente para detectar "database is locked", que va al principio).
        let (out_res, locked) = tokio::join!(
            drain_capped(stdout, MAX_STDOUT_BYTES),
            stderr_locked(stderr),
        );
        let (buf, out_overflow) = out_res;
        let status = child.wait().await.map_err(|_| CorpusError::InvalidJson)?;
        Ok::<_, CorpusError>((buf, out_overflow, locked, status))
    };

    let (buf, out_overflow, locked, status) = match tokio::time::timeout(timeout, work).await {
        Ok(r) => r?,
        Err(_) => return Err(CorpusError::Timeout),
    };

    if out_overflow {
        return Err(CorpusError::OutputTooLarge);
    }
    if !status.success() {
        // lock detectado en CUALQUIER posición del stderr (streaming, cross-chunk).
        if locked {
            return Err(CorpusError::Locked);
        }
        return Err(CorpusError::InvalidJson);
    }
    String::from_utf8(buf).map_err(|_| CorpusError::InvalidJson)
}

/// run con 1 retry+backoff ante DB-locked (la ingesta puede tener el lock).
async fn run(args: &[&str], timeout: Duration) -> Result<String, CorpusError> {
    let bin = resolve_bin().ok_or(CorpusError::NotInstalled)?;
    match run_once(&bin, args, timeout).await {
        Err(CorpusError::Locked) => {
            tokio::time::sleep(Duration::from_millis(LOCKED_BACKOFF_MS)).await;
            run_once(&bin, args, timeout).await
        }
        other => other,
    }
}

fn parse<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, CorpusError> {
    serde_json::from_str(s).map_err(|_| CorpusError::InvalidJson)
}

fn clamp_limit(limit: Option<u32>) -> String {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT).to_string()
}

// ── API pública (la consumen los comandos Tauri) ──

pub async fn status() -> CorpusResult<CorpusStatus> {
    match run(&["status"], Duration::from_secs(8)).await {
        Ok(out) => match parse::<CorpusStatus>(&out) {
            Ok(st) if st.schema_version >= MIN_SCHEMA_VERSION => CorpusResult::ok(st),
            Ok(st) => CorpusResult::err(
                CorpusError::IncompatibleVersion,
                format!("corpus-engine schema {} < {}", st.schema_version, MIN_SCHEMA_VERSION),
            ),
            Err(e) => CorpusResult::err(e, "status: JSON inválido"),
        },
        Err(e) => CorpusResult::err(e, "corpus-engine status falló"),
    }
}

pub async fn search(
    query: &str,
    project: Option<&str>,
    human_only: bool,
    limit: Option<u32>,
) -> CorpusResult<SearchResults> {
    let q = query.trim();
    if q.is_empty() || q.len() > MAX_QUERY_LEN {
        return CorpusResult::err(CorpusError::BadInput, "query vacía o demasiado larga");
    }
    let lim = clamp_limit(limit);
    let mut args: Vec<&str> = vec!["search", "--limit", &lim];
    if let Some(p) = project {
        if !is_safe_arg(p) {
            return CorpusResult::err(CorpusError::BadInput, "project inválido");
        }
        args.push("--project");
        args.push(p);
    }
    if human_only {
        args.push("--human-only");
    }
    // `--` separa opciones de positionals: una query que empieza con `-` (ej "--project") NO la
    // interpreta argparse como flag (audit opus MED, arg-injection).
    args.push("--");
    args.push(q);
    match run(&args, Duration::from_secs(20)).await {
        Ok(out) => match parse::<SearchResults>(&out) {
            Ok(r) => CorpusResult::ok(r),
            Err(e) => CorpusResult::err(e, "search: JSON inválido"),
        },
        Err(e) => CorpusResult::err(e, "corpus-engine search falló"),
    }
}

pub async fn deadends(top: Option<u32>) -> CorpusResult<Deadends> {
    let t = top.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT).to_string();
    match run(&["deadends", "--top", &t], Duration::from_secs(15)).await {
        Ok(out) => match parse::<Deadends>(&out) {
            Ok(d) => CorpusResult::ok(d),
            Err(e) => CorpusResult::err(e, "deadends: JSON inválido"),
        },
        Err(e) => CorpusResult::err(e, "corpus-engine deadends falló"),
    }
}

pub async fn ledger(project: Option<&str>, kind: Option<&str>) -> CorpusResult<Ledger> {
    let mut args: Vec<&str> = vec!["ledger"];
    if let Some(p) = project {
        if !is_safe_arg(p) {
            return CorpusResult::err(CorpusError::BadInput, "project inválido");
        }
        args.push("--project");
        args.push(p);
    }
    if let Some(k) = kind {
        if !matches!(k, "approval" | "correction" | "kill") {
            return CorpusResult::err(CorpusError::BadInput, "kind inválido");
        }
        args.push("--type");
        args.push(k);
    }
    match run(&args, Duration::from_secs(20)).await {
        Ok(out) => match parse::<Ledger>(&out) {
            Ok(l) => CorpusResult::ok(l),
            Err(e) => CorpusResult::err(e, "ledger: JSON inválido"),
        },
        Err(e) => CorpusResult::err(e, "corpus-engine ledger falló"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_arg_rejects_shell_and_spaces() {
        assert!(is_safe_arg("agent-a676393a0078aae1e"));
        assert!(is_safe_arg("dinero"));
        assert!(!is_safe_arg("a; rm -rf /"));
        assert!(!is_safe_arg("has space"));
        assert!(!is_safe_arg(""));
        assert!(!is_safe_arg(&"x".repeat(200)));
    }

    #[tokio::test]
    async fn stderr_locked_detects_across_chunks() {
        // patrón al final de un stream largo (>16KB), partido entre chunks → debe detectarse.
        let mut data = vec![b'x'; 20000];
        data.extend_from_slice(b"sqlite3.OperationalError: database is locked
");
        assert!(stderr_locked(&data[..]).await);
        // sin el patrón → false; "unlocked" no matchea "database is locked".
        assert!(!stderr_locked(&b"file is unlocked, permission denied"[..]).await);
    }

    #[test]
    fn clamp_limit_bounds() {
        assert_eq!(clamp_limit(None), "20");
        assert_eq!(clamp_limit(Some(0)), "1");
        assert_eq!(clamp_limit(Some(5000)), "100");
        assert_eq!(clamp_limit(Some(50)), "50");
    }

    #[test]
    fn parse_status_real_shape() {
        let s = r#"{"schema_version":"2","sessions":205,"messages":341187,"tool_events":178003,"extra_new_field":"ignored"}"#;
        let st: CorpusStatus = parse(s).expect("parsea + ignora campos nuevos");
        assert_eq!(st.sessions, 205);
        assert_eq!(st.schema_version, 2);
    }

    #[test]
    fn parse_deadends_real_shape() {
        let s = r#"{"distinct_errors":1527,"recurrent":[{"signature":"file not read","count":883,"example":"x"}]}"#;
        let d: Deadends = parse(s).expect("parsea");
        assert_eq!(d.recurrent[0].count, 883);
    }

    #[test]
    fn parse_ledger_real_shape() {
        let s = r#"{"counts":{"kill":4},"n":1,"decisions":[{"type":"kill","timestamp":"t","project":"dinero","session":"s","text":"x"}]}"#;
        let l: Ledger = parse(s).expect("parsea (ignora counts)");
        assert_eq!(l.decisions[0].kind, "kill");
    }

    #[test]
    fn bad_query_rejected() {
        let r = tokio_test_block(search("", None, false, None));
        assert_eq!(r.error_code, Some(CorpusError::BadInput));
        let long = "x".repeat(600);
        let r2 = tokio_test_block(search(&long, None, false, None));
        assert_eq!(r2.error_code, Some(CorpusError::BadInput));
    }

    #[test]
    fn missing_binary_is_not_installed() {
        // Forzar un override a un path inexistente → NotInstalled, available=false.
        std::env::set_var("FURX_CORPUS_ENGINE_BIN", "/nonexistent/corpus-engine-xyz");
        let r = tokio_test_block(status());
        std::env::remove_var("FURX_CORPUS_ENGINE_BIN");
        // puede caer a venv/PATH si existen; si no, NotInstalled. Aceptamos ambos pero si falló por
        // binario, debe ser NotInstalled (available=false), nunca un panic.
        if r.error_code == Some(CorpusError::NotInstalled) {
            assert!(!r.available);
        }
    }

    // mini block-on sin agregar dependencias de test: usa el runtime de tokio actual si existe,
    // si no crea uno efímero.
    fn tokio_test_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
            .block_on(f)
    }

    /// Integración real contra ~/.corpus-engine/corpus.db. Opt-in: FURX_CORPUS_E2E=1 (no flakea CI).
    #[test]
    fn e2e_against_real_corpus() {
        if std::env::var("FURX_CORPUS_E2E").as_deref() != Ok("1") {
            return; // skip salvo opt-in
        }
        let st = tokio_test_block(status());
        assert!(st.available, "corpus-engine disponible");
        let data = st.data.expect("status data");
        assert!(data.sessions > 0, "sessions > 0");
        let d = tokio_test_block(deadends(Some(3)));
        assert!(d.data.map(|x| x.distinct_errors > 0).unwrap_or(false));
        let s = tokio_test_block(search("keychain", None, false, Some(2)));
        assert!(s.available);
        // arg-injection: una query que empieza con dash NO debe romper (va tras --).
        let dash = tokio_test_block(search("--project", None, false, Some(1)));
        assert!(dash.available, "query con dash no rompe (separador --)");
    }
}
