// spec-kit 001 · US1 — TTS "Read aloud".
//
// Reads a pane's *finished* output aloud using the LOCAL OS speech engine
// (macOS `say`, Windows SAPI via PowerShell, Linux `spd-say`/`espeak`). No
// network, ever (constitution F-I BYOK / F-IV privacy). Council rules:
//   - opt-in per pane, OFF by default (enforced in the UI layer)
//   - read at END of a block, never token-by-token (caller decides when)
//   - prefer a summary; never read code/diffs/logs (see `summarize`)
//   - ONE speaking pane at a time (mutex on the active pane)
//   - voice-interrupt: STT start calls `stop()`; global Stop also calls `stop()`
//
// argv-only (no shell string interpolation), kill_on_drop, timeouts — same
// hardening posture as voice.rs/whisper.rs.

use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::process::Stdio;
use tokio::process::Command;

/// Which OS speech backend to drive. Resolved once at first use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsEngine {
    /// macOS `say`
    MacSay,
    /// Linux `spd-say` (speech-dispatcher) — preferred on Linux
    SpdSay,
    /// Linux `espeak` / `espeak-ng` fallback
    Espeak,
    /// Windows SAPI via PowerShell System.Speech
    WinSapi,
    /// No engine available on this host
    None,
}

impl TtsEngine {
    /// Probe the host for an available local TTS engine. Pure lookup of binaries
    /// on PATH per-OS; returns `None` when nothing is installed (caller degrades
    /// gracefully — never an error that breaks the app).
    pub fn detect() -> TtsEngine {
        if cfg!(target_os = "macos") {
            if which("say") {
                return TtsEngine::MacSay;
            }
        } else if cfg!(target_os = "windows") {
            // PowerShell ships System.Speech on all supported Windows.
            return TtsEngine::WinSapi;
        } else {
            if which("spd-say") {
                return TtsEngine::SpdSay;
            }
            if which("espeak-ng") || which("espeak") {
                return TtsEngine::Espeak;
            }
        }
        TtsEngine::None
    }

    /// Build the (program, args) for speaking `text`. Returns None for `None`
    /// engine. argv-only: `text` is passed as a single arg, never shell-interpolated.
    ///
    /// 033 U2 — config fina de audio: `voice` (nombre de voz, sólo si pasa la validación del caller) y
    /// `rate` (multiplicador ya clampeado 0.5..=2.0). Sólo macOS `say` los honra (`-v <voz> -r <wpm>`,
    /// wpm = 175*rate); los demás engines los IGNORAN. `voice`/`rate` en default (`None`/1.0) ⇒ comando
    /// idéntico al previo (cero regresión).
    fn command(self, text: &str, voice: Option<&str>, rate: f64) -> Option<(String, Vec<String>)> {
        match self {
            TtsEngine::MacSay => {
                let mut args: Vec<String> = Vec::new();
                if let Some(v) = voice.filter(|v| !v.trim().is_empty()) {
                    args.push("-v".into());
                    args.push(v.to_string());
                }
                // rate=1.0 ⇒ no pasar -r (mantiene el default del sistema, cero regresión).
                if (rate - 1.0).abs() > f64::EPSILON {
                    let wpm = (175.0 * rate).round().clamp(90.0, 360.0) as i64;
                    args.push("-r".into());
                    args.push(wpm.to_string());
                }
                args.push(text.to_string());
                Some(("say".into(), args))
            }
            TtsEngine::SpdSay => Some((
                "spd-say".into(),
                // -w: wait until done (so child exit == speech done)
                vec!["-w".into(), text.to_string()],
            )),
            TtsEngine::Espeak => {
                let bin = if which("espeak-ng") {
                    "espeak-ng"
                } else {
                    "espeak"
                };
                Some((bin.into(), vec![text.to_string()]))
            }
            TtsEngine::WinSapi => {
                // Drive System.Speech; text is a single quoted arg to -Command's here-string
                // would be shell-ish, so instead pass via a -EncodedCommand-free approach:
                // we hand PowerShell the text as a separate arg bound to $args[0].
                let script = "Add-Type -AssemblyName System.Speech; \
                              (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak($args[0])";
                Some((
                    "powershell".into(),
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        script.into(),
                        text.to_string(),
                    ],
                ))
            }
            TtsEngine::None => None,
        }
    }
}

fn which(bin: &str) -> bool {
    // Cross-platform PATH probe without spawning a shell.
    if let Ok(path) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for dir in path.split(sep) {
            let cand = std::path::Path::new(dir).join(bin);
            if cand.is_file() {
                return true;
            }
            if cfg!(windows) {
                let cand_exe = std::path::Path::new(dir).join(format!("{bin}.exe"));
                if cand_exe.is_file() {
                    return true;
                }
            }
        }
    }
    false
}

/// Global single-speaker state. Only ONE pane may speak at a time (council rule).
/// `generation` increments per speak so a watcher task only clears the slot it
/// owns. `kill` cancels the current speaker (preempt/stop). The child itself is
/// owned by the watcher task, which clears the slot when speech ends NATURALLY
/// (Codex audit: the slot was never cleared on natural exit → auto-read with
/// preempt:false got dropped forever after the first read).
struct SpeakingState {
    pane_id: Option<String>,
    generation: u64,
    kill: Option<tokio::sync::oneshot::Sender<()>>,
}

static SPEAKING: Lazy<Mutex<SpeakingState>> = Lazy::new(|| {
    Mutex::new(SpeakingState {
        pane_id: None,
        generation: 0,
        kill: None,
    })
});

/// Policy for what happens when a pane requests TTS while another is speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenBusy {
    /// Cancel the current speaker and take over (default for an explicit user action)
    Preempt,
    /// Drop the new request, let the current speaker finish
    Drop,
}

/// Speak `text` for `pane_id` con la voz/rate por defecto. Wrapper de `speak_with` (cero regresión:
/// callers existentes no cambian).
pub async fn speak(pane_id: &str, text: &str, when_busy: WhenBusy) -> Result<bool> {
    speak_with(pane_id, text, when_busy, None, 1.0).await
}

/// 033 U2 — como `speak` pero con `voice`/`rate` (config fina de audio). Enforces the single-speaker
/// rule per `when_busy`. Returns `Ok(true)` if speech started, `Ok(false)` if dropped (busy+Drop) or
/// no engine / empty text. Never returns an error for "no engine" — degrades. `rate` se clampa a
/// 0.5..=2.0; `voice` vacía/no-válida la ignora `command`.
pub async fn speak_with(
    pane_id: &str,
    text: &str,
    when_busy: WhenBusy,
    voice: Option<&str>,
    rate: f64,
) -> Result<bool> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(false);
    }
    let rate = if rate.is_finite() {
        rate.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let engine = TtsEngine::detect();
    let (prog, args) = match engine.command(text, voice, rate) {
        Some(c) => c,
        None => return Ok(false), // no local engine → silently unavailable
    };

    // Reserve the single-speaker slot + spawn atomically under the lock. Command::
    // spawn is synchronous (returns Child immediately, no await), so holding the
    // parking_lot guard across it is sound and closes the check-then-spawn race.
    let (gen, mut child, kill_rx) = {
        let mut st = SPEAKING.lock();
        if st.pane_id.is_some() {
            match when_busy {
                WhenBusy::Drop => return Ok(false),
                WhenBusy::Preempt => {
                    if let Some(k) = st.kill.take() {
                        let _ = k.send(()); // cancel current speaker
                    }
                }
            }
        }
        let child = Command::new(&prog)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow!("tts spawn {prog} failed: {e}"))?;
        st.generation += 1;
        let gen = st.generation;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        st.pane_id = Some(pane_id.to_string());
        st.kill = Some(tx);
        (gen, child, rx)
    };

    // Watcher: owns the child; clears the slot on natural exit OR kill, but only
    // if it still owns the current generation (a newer speak won the slot).
    tokio::spawn(async move {
        tokio::select! {
            _ = child.wait() => {}
            _ = kill_rx => { let _ = child.start_kill(); let _ = child.wait().await; }
        }
        let mut st = SPEAKING.lock();
        if st.generation == gen {
            st.pane_id = None;
            st.kill = None;
        }
    });
    Ok(true)
}

/// Stop any current speech immediately (global Stop, or voice-interrupt when the
/// STT detects the user starting to talk). Idempotent. The watcher task clears
/// the slot once the child is killed.
pub fn stop() {
    let mut st = SPEAKING.lock();
    if let Some(k) = st.kill.take() {
        let _ = k.send(());
    }
    st.pane_id = None;
}

/// The pane currently speaking, if any.
pub fn speaking_pane() -> Option<String> {
    SPEAKING.lock().pane_id.clone()
}

/// Whether a usable local TTS engine exists on this host.
pub fn available() -> bool {
    TtsEngine::detect() != TtsEngine::None
}

/// Redact secrets/PII before anything is spoken. Council T032 (gemini+codex,
/// unanimous): a key/token read aloud in a public space is a physical privacy
/// leak. Replaces common secret shapes with "[redacted]". Conservative — favors
/// over-redaction. Applied inside `summarize`, so every TTS path is covered.
pub fn redact_secrets(s: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        [
            r"sk-[A-Za-z0-9_-]{16,}",            // OpenAI-style
            r"gh[posru]_[A-Za-z0-9]{20,}",       // GitHub tokens
            r"github_pat_[A-Za-z0-9_]{20,}",     // GitHub fine-grained PAT
            r"xox[baprs]-[A-Za-z0-9-]{10,}",     // Slack
            r"AKIA[0-9A-Z]{16}",                 // AWS access key id
            r"(?i)bearer\s+[A-Za-z0-9._-]{16,}", // Bearer tokens
            r"(?i)(password|passwd|secret|token|api[_-]?key)\s*[:=]\s*[^\r\n]+", // key=value → redact rest of line (gemini: \S+ leaks values with spaces)
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",    // JWT
            r"(?s)-----BEGIN[A-Z ]+PRIVATE KEY-----.*?-----END[A-Z ]+PRIVATE KEY-----", // full PEM block
            r"\b[A-Fa-f0-9]{40,}\b", // long hex blobs (hashes/keys)
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    });
    let mut out = s.to_string();
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, "[redacted]").into_owned();
    }
    out
}

/// Reduce a finished output block to something worth *hearing*: redact secrets,
/// drop fenced code, diffs and obvious log noise, keep prose, cap length.
/// Heuristic (council T032 default — privacy-safe, no network). The opt-in
/// BYOK-LLM summary path is a future increment. Never reads code/logs/tokens.
pub fn summarize(block: &str, max_chars: usize) -> String {
    let block = &redact_secrets(block);
    let mut out: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in block.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Drop diff/log-ish lines.
        if t.starts_with('+')
            || t.starts_with('-')
            || t.starts_with('@')
            || t.starts_with("diff ")
            || t.starts_with("commit ")
            || looks_like_log(t)
        {
            continue;
        }
        if t.is_empty() {
            continue;
        }
        out.push(line.trim());
    }
    // Prefer the tail (decisions/conclusions usually land at the end).
    let joined = out.join(". ");
    let cleaned = joined.trim();
    if cleaned.chars().count() <= max_chars {
        return cleaned.to_string();
    }
    // Keep the last `max_chars` worth, on a char boundary.
    let start = cleaned.chars().count().saturating_sub(max_chars);
    let tail: String = cleaned.chars().skip(start).collect();
    format!("…{}", tail.trim_start())
}

fn looks_like_log(t: &str) -> bool {
    // crude: timestamps / level prefixes / shell prompts
    t.starts_with('$')
        || t.starts_with("[20")
        || t.starts_with("INFO ")
        || t.starts_with("DEBUG ")
        || t.starts_with("WARN ")
        || t.starts_with("ERROR ")
        || t.starts_with("TRACE ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_does_not_speak() {
        // tokio runtime not needed: empty short-circuits before spawn.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let started = rt.block_on(speak("p1", "   ", WhenBusy::Preempt)).unwrap();
        assert!(!started);
        assert_eq!(speaking_pane(), None);
    }

    #[test]
    fn engine_command_is_argv_only() {
        // text with shell metacharacters must remain a single arg, never split.
        let danger = "hi; rm -rf / `whoami`";
        let (_prog, args) = TtsEngine::MacSay.command(danger, None, 1.0).unwrap();
        assert!(args.contains(&danger.to_string()));
        let (_p2, a2) = TtsEngine::SpdSay.command(danger, None, 1.0).unwrap();
        assert!(a2.contains(&danger.to_string()));
    }

    #[test]
    fn none_engine_yields_no_command() {
        assert!(TtsEngine::None.command("hello", None, 1.0).is_none());
    }

    // 033 U2 — voz/rate: default (None, 1.0) NO agrega flags (cero regresión); con valores, agrega
    // `-v <voz>` y `-r <wpm>` y el texto sigue siendo un único arg (argv-only, sin inyección).
    #[test]
    fn mac_say_voice_and_rate() {
        // default → sin -v ni -r, sólo el texto.
        let (_p, a) = TtsEngine::MacSay.command("hola", None, 1.0).unwrap();
        assert_eq!(a, vec!["hola".to_string()]);
        // con voz + rate 2.0 → -v Mónica -r 350, texto al final como un solo arg.
        let (_p2, a2) = TtsEngine::MacSay.command("hola mundo", Some("Monica"), 2.0).unwrap();
        assert_eq!(a2[0], "-v");
        assert_eq!(a2[1], "Monica");
        assert_eq!(a2[2], "-r");
        assert_eq!(a2[3], "350"); // 175*2.0
        assert_eq!(a2[4], "hola mundo");
        // voz vacía → se ignora (no -v).
        let (_p3, a3) = TtsEngine::MacSay.command("x", Some("  "), 1.0).unwrap();
        assert_eq!(a3, vec!["x".to_string()]);
    }

    #[test]
    fn summarize_drops_code_fences_and_logs() {
        let block = "Here is the plan.\n```rust\nfn main() {}\n```\n$ cargo build\n+added line\nDone: 3 files changed.";
        let s = summarize(block, 500);
        assert!(s.contains("Here is the plan"));
        assert!(s.contains("Done: 3 files changed"));
        assert!(!s.contains("fn main"));
        assert!(!s.contains("cargo build"));
        assert!(!s.contains("added line"));
    }

    #[test]
    fn redact_strips_secrets_before_speaking() {
        let s = "ok done. token=ghp_abcdefghijklmnopqrstuvwxyz0123 and sk-proj-ABCDEFGHIJKLMNOPQRST done";
        let r = redact_secrets(s);
        assert!(!r.contains("ghp_abcdefghijklmnopqrstuvwxyz0123"));
        assert!(!r.contains("sk-proj-ABCDEFGHIJKLMNOPQRST"));
        assert!(r.contains("[redacted]"));
        // summarize must also redact (covers every TTS path)
        let sum = summarize(
            "Bearer eyJhbGciOiJIUzI1Niand more.password = hunter2hunter2",
            500,
        );
        assert!(!sum.contains("hunter2hunter2"));
    }

    #[test]
    fn summarize_caps_length() {
        let long = "word ".repeat(500);
        let s = summarize(&long, 100);
        assert!(s.chars().count() <= 101); // +1 for the leading ellipsis
    }
}
