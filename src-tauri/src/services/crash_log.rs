// C2 — Crash + error capture (Sentry-free, $0).
//
// Captures Rust panics, JS unhandled errors, and JS unhandledrejection events
// into rotated log files under the app data dir. Panic hook is written so it
// CANNOT deadlock or recursively fail under panic — no audit/DB/locks reached.
//
// Storage layout: ~/Library/Application Support/furx/crashes/{iso_ts}-{uuid8}.log
// Rotation: keep max MAX_FILES, total ≤ MAX_TOTAL_BYTES, oldest-first FIFO.
//
// Audit-1 (5-voice council + Codex, 2026-05-27):
// - MED: scrub_pii now redacts Bearer tokens / API keys / sk-/cfut_/cf_/AKIA*/etc.
// - LOW: regexes compiled once via Lazy static (perf B007).

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_FILES: usize = 50;
pub const MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 64 * 1024; // per-event hard cap
const RATE_LIMIT_PER_MINUTE: u64 = 30;

/// File mutex shared across panic hook + JS-side writes. Locking under panic
/// is OK because writes are short and we never call user code holding the lock.
static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Rate-limit bucket: bounded count + window-start timestamp.
static RATE_BUCKET: OnceLock<Mutex<(u64, u64)>> = OnceLock::new(); // (count, window_start_secs)

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CrashSource {
    RustPanic,
    JsError,
    JsUnhandledRejection,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashEntry {
    pub iso_ts: String,
    pub version: String,
    pub os: String,
    pub source: CrashSource,
    pub location: Option<String>,
    pub message: String,
    pub backtrace: Option<String>,
}

impl CrashEntry {
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("furx crash report\n");
        out.push_str(&format!("ts: {}\n", self.iso_ts));
        out.push_str(&format!("version: {}\n", self.version));
        out.push_str(&format!("os: {}\n", self.os));
        out.push_str(&format!("source: {:?}\n", self.source));
        if let Some(loc) = &self.location {
            out.push_str(&format!("location: {}\n", scrub_pii(loc)));
        }
        out.push_str("message:\n");
        out.push_str(&scrub_pii(&self.message));
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if let Some(bt) = &self.backtrace {
            out.push_str("backtrace:\n");
            out.push_str(&scrub_pii(bt));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

/// Initialise the crash directory and install the panic hook. Idempotent.
pub fn init() {
    if CRASH_DIR.get().is_some() {
        return;
    }
    let Some(dir) = crash_dir_path() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let _ = CRASH_DIR.set(dir);
    WRITE_LOCK.get_or_init(|| Mutex::new(()));
    RATE_BUCKET.get_or_init(|| Mutex::new((0, current_secs())));
    install_panic_hook();
}

fn crash_dir_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("furx").join("crashes"))
}

pub fn dir() -> Option<PathBuf> {
    CRASH_DIR.get().cloned().or_else(crash_dir_path)
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Codex HIGH v1: keep the panic path minimal. Use try_lock (NO deadlock if
        // a write was already in flight), no rotation, no regex, no dir creation,
        // no chrono formatting, no allocator-heavy work.
        panic_write_minimal(info);
        prev(info);
    }));
}

fn panic_write_minimal(info: &std::panic::PanicHookInfo<'_>) {
    use std::io::Write;
    let Some(dir) = CRASH_DIR.get() else {
        return;
    };
    // try_lock: if write is already in flight, skip the panic capture rather than
    // risk deadlock. The previous hook (default) still prints to stderr.
    let lock = WRITE_LOCK.get();
    let guard = match lock {
        Some(m) => match m.try_lock() {
            Some(g) => Some(g),
            None => return,
        },
        None => None,
    };

    let location_owned = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    let location = location_owned.as_deref().unwrap_or("<unknown>");
    let message = match info.payload().downcast_ref::<&'static str>() {
        Some(s) => *s,
        None => match info.payload().downcast_ref::<String>() {
            Some(s) => s.as_str(),
            None => "<non-string panic payload>",
        },
    };

    // Filename built directly with epoch secs + atomic seed; no chrono on the panic path.
    let secs = current_secs();
    let id = short_uuid();
    let filename = format!("panic-{}-{}.log", secs, id);
    let path = dir.join(filename);

    let body = format!(
        "furx crash report\nts: epoch-{}\nversion: {}\nos: {} {}\nsource: RustPanic\nlocation: {}\nmessage:\n{}\n",
        secs,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        location,
        message,
    );
    // Codex HIGH v2: slice on UTF-8 boundary to avoid panic-in-panic.
    let cap = utf8_truncate_len(&body, MAX_PAYLOAD_BYTES);
    let body = &body[..cap];

    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
    {
        let _ = f.write_all(body.as_bytes());
    }
    drop(guard);
}

/// Write a crash entry. Returns the path of the new file on success.
pub fn write_entry(entry: &CrashEntry) -> std::io::Result<PathBuf> {
    let dir =
        dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no crash dir"))?;
    // Rate limit (per-process, per-minute window).
    if !rate_limit_ok() {
        return Err(std::io::Error::other("rate-limited"));
    }
    fs::create_dir_all(&dir)?;
    let lock_handle = WRITE_LOCK.get().map(|m| m.lock());
    let _guard = lock_handle; // hold until end of scope

    let id = short_uuid();
    let filename = format!("{}-{}.log", entry.iso_ts.replace(':', ""), id);
    let path = dir.join(filename);
    let mut body = entry.render();
    if body.len() > MAX_PAYLOAD_BYTES {
        // Codex HIGH v2: truncate on UTF-8 boundary; never byte-slice a String
        // because slicing inside a multibyte char would panic.
        let cap = utf8_truncate_len(&body, MAX_PAYLOAD_BYTES.saturating_sub(16));
        body.truncate(cap);
        body.push_str("\n[truncated]\n");
    }
    fs::write(&path, body)?;
    rotate(&dir);
    Ok(path)
}

/// Largest valid UTF-8 boundary ≤ `max_bytes`.
pub fn utf8_truncate_len(s: &str, max_bytes: usize) -> usize {
    if s.len() <= max_bytes {
        return s.len();
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn rate_limit_ok() -> bool {
    let Some(bucket) = RATE_BUCKET.get() else {
        return true;
    };
    let mut g = bucket.lock();
    let now = current_secs();
    if now.saturating_sub(g.1) >= 60 {
        g.0 = 0;
        g.1 = now;
    }
    if g.0 >= RATE_LIMIT_PER_MINUTE {
        return false;
    }
    g.0 += 1;
    true
}

fn rotate(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(PathBuf, u64, SystemTime)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            if p.extension().and_then(|s| s.to_str()) != Some("log") {
                return None;
            }
            Some((p, m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .collect();
    // Oldest first.
    entries.sort_by_key(|(_, _, t)| *t);
    let mut total: u64 = entries.iter().map(|(_, sz, _)| *sz).sum();
    let mut count = entries.len();
    let mut i = 0;
    while (count > MAX_FILES || total > MAX_TOTAL_BYTES) && i < entries.len() {
        let (p, sz, _) = &entries[i];
        if fs::remove_file(p).is_ok() {
            total = total.saturating_sub(*sz);
            count = count.saturating_sub(1);
        }
        i += 1;
    }
}

/// Audit-1 LOW B007: compile each pattern once.
static USER_PATH_RE: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"/Users/[^/\s]+").expect("USER_PATH_RE"));

/// Audit-1 MED: secret patterns — best-effort redaction of bearer tokens,
/// vendor-prefixed keys, generic API keys and password=… literals. False
/// negatives are acceptable (crash dumps are local-only); false positives
/// just over-redact and never crash the panic path.
static SECRET_PATTERNS: Lazy<Vec<(regex::Regex, &'static str)>> = Lazy::new(|| {
    let raw: &[(&str, &str)] = &[
        // Authorization: Bearer <token>
        (
            r"(?i)(authorization\s*:\s*bearer\s+)[A-Za-z0-9._\-]{6,}",
            "$1<redacted>",
        ),
        // Bare bearer token in URL or arg
        (r"(?i)\bbearer\s+[A-Za-z0-9._\-]{16,}", "Bearer <redacted>"),
        // Common provider keys (Anthropic, OpenAI, Slack, GitHub PAT, Stripe, AWS, Cloudflare tunnel).
        (r"\bsk-[A-Za-z0-9_\-]{16,}", "<redacted-sk>"),
        (r"\bsk-ant-[A-Za-z0-9_\-]{16,}", "<redacted-sk-ant>"),
        (r"\bxox[baprs]-[A-Za-z0-9_\-]{8,}", "<redacted-slack>"),
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}", "<redacted-gh>"),
        (r"\bAKIA[0-9A-Z]{12,}", "<redacted-aws>"),
        (r"\bcfut_[A-Za-z0-9._\-]{16,}", "<redacted-cf-tun>"),
        // Generic api_key/password/secret=… literals (single, double quoted, or bare).
        (
            r#"(?i)(api[_-]?key|password|secret|access[_-]?token)\s*[=:]\s*['"]?[A-Za-z0-9._\-/+=]{8,}['"]?"#,
            "$1=<redacted>",
        ),
    ];
    raw.iter()
        .filter_map(|(p, r)| regex::Regex::new(p).ok().map(|re| (re, *r)))
        .collect()
});

/// Best-effort scrubber: home paths + bearer/api-key/password patterns.
/// Defence-in-depth — never the only line of defence against secret leaks.
pub fn scrub_pii(s: &str) -> String {
    let mut out = s.to_string();
    if let Some(home) = dirs::home_dir().and_then(|p| p.to_str().map(String::from)) {
        out = out.replace(&home, "/Users/<user>");
    }
    out = USER_PATH_RE.replace_all(&out, "/Users/<user>").into_owned();
    for (re, replacement) in SECRET_PATTERNS.iter() {
        out = re.replace_all(&out, *replacement).into_owned();
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashSummary {
    pub filename: String,
    pub iso_ts: String,
    pub bytes: u64,
}

pub fn list_files() -> Vec<CrashSummary> {
    let Some(dir) = dir() else {
        return vec![];
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut entries: Vec<CrashSummary> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let m = e.metadata().ok()?;
            if !m.is_file() {
                return None;
            }
            if p.extension().and_then(|s| s.to_str()) != Some("log") {
                return None;
            }
            let filename = p.file_name()?.to_string_lossy().to_string();
            // iso_ts is the prefix before "-<uuid>.log".
            let iso_ts = filename.rsplit_once('-').map(|x| x.0).unwrap_or("").to_string();
            Some(CrashSummary {
                filename,
                iso_ts,
                bytes: m.len(),
            })
        })
        .collect();
    entries.sort_by(|a, b| b.iso_ts.cmp(&a.iso_ts));
    entries
}

/// Filename pattern: either `<isoTs-without-colons>-<8hex>.log` (JS path) or
/// `panic-<epochSecs>-<8hex>.log` (Rust panic path).
fn valid_filename(name: &str) -> bool {
    if !name.ends_with(".log") {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    if name.len() > 128 {
        return false;
    }
    if name.starts_with("panic-") {
        let core = &name[6..name.len() - 4]; // strip "panic-" + ".log"
        let mut parts = core.rsplitn(2, '-');
        let id = parts.next().unwrap_or("");
        let ts = parts.next().unwrap_or("");
        return id.len() == 8
            && id.chars().all(|c| c.is_ascii_hexdigit())
            && !ts.is_empty()
            && ts.chars().all(|c| c.is_ascii_digit());
    }
    let core = &name[..name.len() - 4];
    let mut parts = core.rsplitn(2, '-');
    let id = parts.next().unwrap_or("");
    let ts = parts.next().unwrap_or("");
    if id.len() != 8 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    // iso_ts (with colons stripped) chars: digits + [TZ-]
    !ts.is_empty()
        && ts
            .chars()
            .all(|c| c.is_ascii_digit() || c == 'T' || c == 'Z' || c == '-')
}

fn resolve_safe_path(filename: &str) -> std::io::Result<PathBuf> {
    if !valid_filename(filename) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bad filename",
        ));
    }
    let dir =
        dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no crash dir"))?;
    let path = dir.join(filename);
    // Codex MED v1: reject symlinks so a malicious link doesn't pivot to /etc/passwd etc.
    let meta = fs::symlink_metadata(&path)?;
    if meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "symlink",
        ));
    }
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(path)
}

pub fn read_file(filename: &str) -> std::io::Result<String> {
    let path = resolve_safe_path(filename)?;
    fs::read_to_string(path)
}

pub fn delete_file(filename: &str) -> std::io::Result<()> {
    let path = resolve_safe_path(filename)?;
    fs::remove_file(path)
}

pub fn clear_all() -> std::io::Result<usize> {
    let dir =
        dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no crash dir"))?;
    let mut n = 0;
    for entry in fs::read_dir(&dir)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("log")
            && fs::remove_file(&p).is_ok() {
                n += 1;
            }
    }
    Ok(n)
}

fn current_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn short_uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0);
    if SEED.load(Ordering::Relaxed) == 0 {
        SEED.store(current_secs() ^ 0x9E3779B97F4A7C15, Ordering::Relaxed);
    }
    let x = SEED.fetch_add(1, Ordering::Relaxed);
    let mut s = String::with_capacity(8);
    let chars = b"0123456789abcdef";
    let mut v = x;
    for _ in 0..8 {
        s.push(chars[(v & 0xf) as usize] as char);
        v >>= 4;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_replaces_home() {
        let home = dirs::home_dir().unwrap();
        let s = format!("error at {}/foo/bar.rs", home.display());
        let scrubbed = scrub_pii(&s);
        assert!(scrubbed.contains("/Users/<user>"));
        assert!(!scrubbed.contains(home.to_str().unwrap()));
    }

    #[test]
    fn read_file_rejects_traversal() {
        assert!(read_file("../etc/passwd").is_err());
        assert!(read_file("a/b.log").is_err());
        assert!(read_file("ok.txt").is_err());
    }

    #[test]
    fn scrub_redacts_bearer_token() {
        let s = "Authorization: Bearer abc123_def456-XYZ.789";
        let out = scrub_pii(s);
        assert!(out.contains("<redacted>"), "out: {}", out);
        assert!(!out.contains("abc123_def456"), "out: {}", out);
    }

    #[test]
    fn scrub_redacts_sk_and_gh_keys() {
        let s =
            "OPENAI_KEY=sk-1234567890abcdef and gh_pat=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let out = scrub_pii(s);
        assert!(
            out.contains("<redacted-sk>") || out.contains("<redacted>"),
            "out: {}",
            out
        );
        assert!(
            out.contains("<redacted-gh>") || out.contains("<redacted>"),
            "out: {}",
            out
        );
    }

    #[test]
    fn scrub_redacts_password_literal() {
        let s = r#"password="hunter2_aaaaaaaa""#;
        let out = scrub_pii(s);
        assert!(out.contains("<redacted>"), "out: {}", out);
        assert!(!out.contains("hunter2"), "out: {}", out);
    }

    #[test]
    fn render_includes_required_fields() {
        let entry = CrashEntry {
            iso_ts: "2026-05-27T00:00:00Z".into(),
            version: "0.2.0".into(),
            os: "darwin aarch64".into(),
            source: CrashSource::Manual,
            location: Some("file.rs:1:1".into()),
            message: "boom".into(),
            backtrace: None,
        };
        let s = entry.render();
        assert!(s.contains("ts: 2026-05-27T00:00:00Z"));
        assert!(s.contains("source: Manual"));
        assert!(s.contains("boom"));
    }
}
