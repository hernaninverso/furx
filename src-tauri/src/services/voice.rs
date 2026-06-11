// D / F19 — Whisper end-to-end (download + capture + transcribe).
// argv-only (no shell), constant-time hash verify, kill_on_drop on child procs,
// timeouts everywhere.

use anyhow::{anyhow, Result};
use serde::Serialize;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

// 021-voice-es — Modelos whisper.cpp MULTILINGÜES (ggerganov/whisper.cpp, tag main, Hugging Face).
// El default cambió de `ggml-tiny.en.bin` (monolingüe INGLÉS — rompía el dictado en español)
// a `ggml-base.bin` (multilingüe). El idioma se pasa a whisper-cli con `-l <lang>` (default `es`).
// SHA-256 oficiales de whisper.cpp (verificar con: shasum -a 256 <archivo>).
//
// NOTA migración: el `ggml-tiny.en.bin` viejo NO se borra automáticamente — convive en
// `~/.furx/whisper/`. Se puede limpiar a mano (`rm ~/.furx/whisper/ggml-tiny.en.bin`) si sobra.
const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
const BASE_SHA256: &str = "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";
const SMALL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
const SMALL_SHA256: &str = "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b";

/// Variante de modelo whisper configurable vía el setting `voice.model`.
/// `Base` (142MB, default) balancea tamaño/calidad; `Small` (466MB) más calidad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceModel {
    Base,
    Small,
}

impl VoiceModel {
    /// Resuelve el setting `voice.model` (`base`|`small`) a la variante. Default `Base`
    /// ante valor ausente/desconocido (multilingüe seguro).
    pub fn from_setting(s: &str) -> VoiceModel {
        match s.trim().to_ascii_lowercase().as_str() {
            "small" => VoiceModel::Small,
            _ => VoiceModel::Base,
        }
    }
    pub fn filename(self) -> &'static str {
        match self {
            VoiceModel::Base => "ggml-base.bin",
            VoiceModel::Small => "ggml-small.bin",
        }
    }
    pub fn url(self) -> &'static str {
        match self {
            VoiceModel::Base => BASE_URL,
            VoiceModel::Small => SMALL_URL,
        }
    }
    pub fn sha256(self) -> &'static str {
        match self {
            VoiceModel::Base => BASE_SHA256,
            VoiceModel::Small => SMALL_SHA256,
        }
    }
}

/// Normaliza el setting `voice.language` a un valor que whisper-cli acepta tras `-l`.
/// Default `es` (el usuario principal dicta en español). `auto` deja que whisper autodetecte.
pub fn normalize_lang(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "auto" => "auto".to_string(),
        "en" => "en".to_string(),
        "es" => "es".to_string(),
        "" => "es".to_string(),
        // Cualquier otro código ISO razonable se pasa tal cual (lower-case), default `es` si vacío.
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadResult {
    pub path: String,
    pub bytes: u64,
    pub sha256_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureResult {
    pub path: String,
    pub bytes: u64,
    pub seconds: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscribeResult {
    pub text: String,
    pub elapsed_ms: u64,
}

fn model_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let d = home.join(".furx").join("whisper");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// Path del archivo de modelo para la variante configurada (`~/.furx/whisper/ggml-<variant>.bin`).
pub fn model_path(model: VoiceModel) -> Result<PathBuf> {
    Ok(model_dir()?.join(model.filename()))
}

/// Construye los argumentos de whisper-cli. PURO para ser testeable. Agrega `-l <lang>`
/// (el bug original: se invocaba SIN `-l`, así que whisper forzaba inglés y el español salía roto).
fn whisper_args(model: &str, audio: &str, lang: &str) -> Vec<String> {
    vec![
        "-m".into(),
        model.into(),
        "-f".into(),
        audio.into(),
        "-l".into(),
        lang.into(),
        "-nt".into(),
        "-np".into(),
        "-otxt".into(),
    ]
}

/// Progress callback invoked from the streaming download — gives the caller a
/// chance to forward progress to the UI (we use `app.emit("voice:download-progress", _)`).
pub type ProgressCb = Box<dyn FnMut(u64, Option<u64>) + Send>;

pub async fn download_model(model: VoiceModel) -> Result<DownloadResult> {
    download_model_streamed(model, None).await
}

/// BLOQUE H · D (PLAN_CLOSE F19): streaming download with optional per-chunk
/// progress callback. Replaces the previous "buffer 75 MiB then write" path so
/// the user actually sees progress and the OS doesn't stall on a single 75 MiB
/// allocation under memory pressure.
///
/// 021-voice-es: descarga el modelo CONFIGURADO (`base`|`small`) con su URL+SHA-256
/// oficiales. El gate de host-allowlist `huggingface.co` se mantiene intacto.
pub async fn download_model_streamed(
    model: VoiceModel,
    mut cb: Option<ProgressCb>,
) -> Result<DownloadResult> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let dest = model_path(model)?;
    let tmp = dest.with_extension("bin.tmp");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host = attempt.url().host_str().unwrap_or("").to_string();
            if host == "huggingface.co" || host.ends_with(".huggingface.co") {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let resp = client.get(model.url()).send().await?;
    let final_host = resp.url().host_str().unwrap_or("").to_string();
    if !(final_host == "huggingface.co" || final_host.ends_with(".huggingface.co")) {
        return Err(anyhow!(
            "model fetch ended at disallowed host: {}",
            final_host
        ));
    }
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status()));
    }
    let content_length = resp.content_length();
    let mut hasher = sha2::Sha256::new();
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut downloaded: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        hasher.update(&bytes);
        file.write_all(&bytes).await?;
        downloaded = downloaded.saturating_add(bytes.len() as u64);
        if let Some(ref mut f) = cb {
            f(downloaded, content_length);
        }
    }
    file.flush().await?;
    drop(file);
    let hex = hex::encode(hasher.finalize());
    let expected = model.sha256().to_ascii_lowercase();
    let sha256_ok = constant_time_eq(hex.as_bytes(), expected.as_bytes());
    if !sha256_ok {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!(
            "sha256 mismatch (got {}, expected {})",
            hex,
            expected
        ));
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(DownloadResult {
        path: dest.to_string_lossy().to_string(),
        bytes: downloaded,
        sha256_ok,
    })
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

// ── Push-to-talk: held capture (spec-005) ───────────────────────────────────
// sox records to a temp WAV until we SIGTERM it (kill -TERM flushes the header so
// the WAV is valid; tokio's kill()=SIGKILL would truncate it). A watchdog kills it
// after a hard max so a lost keyup can't record forever.
const PTT_MAX_SECS: u64 = 60;
// Registry holds the tokio Child (so we can reap it → no zombie) + the WAV path.
type HeldChild = tokio::process::Child;
static HELD: once_cell::sync::Lazy<
    parking_lot::Mutex<std::collections::HashMap<String, (HeldChild, PathBuf)>>,
> = once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

#[cfg(unix)]
fn sigterm(pid: u32) {
    // SIGTERM (not SIGKILL) so sox finalizes the WAV header.
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status();
}
#[cfg(not(unix))]
fn sigterm(_pid: u32) {}

/// SIGTERM the child + AWAIT its exit (reap → no zombie, WAV flushed). Bounded so a
/// stuck sox can't hang us.
async fn terminate_and_reap(mut child: HeldChild) {
    if let Some(pid) = child.id() {
        sigterm(pid);
    }
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
    // If it didn't exit on SIGTERM within 3s, hard-kill + reap.
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Start a held recording; returns a capture id. Records until ptt_stop/cancel or
/// the watchdog (PTT_MAX_SECS). MVP unix (sox).
pub async fn ptt_start() -> Result<String> {
    let sox = which("sox").ok_or_else(|| anyhow!("sox not in PATH — `brew install sox`"))?;
    let id = Uuid::new_v4().to_string();
    let out = std::env::temp_dir().join(format!("furx-voice-{id}.wav"));
    let child = Command::new(&sox)
        .args([
            "-d",
            "-r",
            "16000",
            "-c",
            "1",
            out.to_str().ok_or_else(|| anyhow!("bad temp path"))?,
        ])
        .kill_on_drop(false) // we reap explicitly via SIGTERM + wait
        .spawn()
        .map_err(|e| anyhow!("sox spawn: {e}"))?;
    HELD.lock().insert(id.clone(), (child, out.clone()));
    // Watchdog: a lost keyup must not record forever — terminate + reap + delete temp.
    let id_wd = id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(PTT_MAX_SECS)).await;
        let entry = HELD.lock().remove(&id_wd);
        if let Some((child, path)) = entry {
            terminate_and_reap(child).await;
            let _ = std::fs::remove_file(path); // watchdog discards (no one transcribes it)
        }
    });
    Ok(id)
}

/// Stop a held capture → WAV path (caller transcribes). SIGTERMs sox + AWAITS exit so
/// the WAV is fully flushed before we return.
pub async fn ptt_stop(id: &str) -> Result<CaptureResult> {
    let (child, path) = HELD
        .lock()
        .remove(id)
        .ok_or_else(|| anyhow!("no such capture"))?;
    terminate_and_reap(child).await;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(CaptureResult {
        path: path.to_string_lossy().to_string(),
        bytes,
        seconds: 0,
    })
}

/// Cancel a held capture: SIGTERM + reap sox + delete temp (no transcription).
pub async fn ptt_cancel(id: &str) {
    let entry = HELD.lock().remove(id);
    if let Some((child, path)) = entry {
        terminate_and_reap(child).await;
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod ptt_tests {
    use super::*;
    #[tokio::test]
    async fn cancel_unknown_id_is_noop() {
        ptt_cancel("does-not-exist").await; // must not panic
    }
    #[tokio::test]
    async fn stop_unknown_id_errs() {
        assert!(ptt_stop("does-not-exist").await.is_err());
    }
}

pub async fn capture(seconds: u16) -> Result<CaptureResult> {
    let secs = seconds.clamp(1, 30);
    let sox = which("sox").ok_or_else(|| anyhow!("sox not in PATH — `brew install sox`"))?;
    let out = std::env::temp_dir().join(format!("furx-voice-{}.wav", Uuid::new_v4()));
    let mut cmd = Command::new(&sox);
    cmd.args([
        "-d",
        "-r",
        "16000",
        "-c",
        "1",
        out.to_str().ok_or_else(|| anyhow!("bad temp path"))?,
        "trim",
        "0",
        &secs.to_string(),
    ])
    .kill_on_drop(true);
    // overall timeout = seconds + 5s slack
    let status = tokio::time::timeout(Duration::from_secs(secs as u64 + 5), cmd.status())
        .await
        .map_err(|_| anyhow!("sox capture timed out"))?
        .map_err(|e| anyhow!("sox spawn: {}", e))?;
    if !status.success() {
        return Err(anyhow!("sox failed: {}", status));
    }
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(CaptureResult {
        path: out.to_string_lossy().to_string(),
        bytes,
        seconds: secs,
    })
}

/// 021-voice-es: transcribe con el modelo (`base`|`small`) y el idioma (`es`|`auto`|`en`)
/// configurados. Pasa `-l <lang>` a whisper-cli — sin esto, whisper forzaba inglés y el
/// dictado en español salía roto (el bug que esta feature arregla).
pub async fn transcribe(
    audio_path: &Path,
    model_variant: VoiceModel,
    lang: &str,
) -> Result<TranscribeResult> {
    if !audio_path.is_file() {
        return Err(anyhow!("audio file not found: {}", audio_path.display()));
    }
    let model = model_path(model_variant)?;
    if !model.is_file() {
        return Err(anyhow!(
            "model not found ({}) — call voice_download_model first",
            model_variant.filename()
        ));
    }
    let whisper = which("whisper-cli")
        .or_else(|| which("whisper"))
        .ok_or_else(|| anyhow!("whisper-cli not in PATH — `brew install whisper-cpp`"))?;
    // Reject silent captures. When mic permission is denied (or the input is muted),
    // macOS CoreAudio hands sox a full-length but SILENT stream; whisper then
    // hallucinates "you" / "(whistling)" on it. Surface a typed `no_audio` error so the
    // UI guides the user to grant mic permission instead of typing garbage into the pane.
    // -46 dBFS. Pure silence (denied mic) reads 0.000000; real speech peaks far above
    // this. Kept low (vs 0.01) so an unusually quiet/low-gain mic isn't false-rejected.
    const SILENCE_PEAK: f32 = 0.005;
    if let Some(peak) = max_amplitude(audio_path).await {
        if peak < SILENCE_PEAK {
            return Err(anyhow!(
                "no_audio: captura silenciosa (pico {:.4} < {:.3}) — micrófono sin permiso o mudo",
                peak,
                SILENCE_PEAK
            ));
        }
    }
    let started = std::time::Instant::now();
    let lang = normalize_lang(lang);
    let args = whisper_args(
        model.to_str().ok_or_else(|| anyhow!("bad model path"))?,
        audio_path
            .to_str()
            .ok_or_else(|| anyhow!("bad audio path"))?,
        &lang,
    );
    let mut cmd = Command::new(&whisper);
    cmd.args(&args).kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(120), cmd.status())
        .await
        .map_err(|_| anyhow!("whisper-cli timed out"))?
        .map_err(|e| anyhow!("whisper-cli spawn: {}", e))?;
    if !status.success() {
        return Err(anyhow!("whisper-cli failed: {}", status));
    }
    let txt_path = audio_path.with_extension("wav.txt");
    let text = std::fs::read_to_string(&txt_path).unwrap_or_default();
    // Cleanup temp files.
    let _ = std::fs::remove_file(&txt_path);
    let _ = std::fs::remove_file(audio_path);
    Ok(TranscribeResult {
        text: text.trim().to_string(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

fn which(cmd: &str) -> Option<String> {
    if let Ok(p) = std::env::var("PATH") {
        for d in p.split(':') {
            let cand = std::path::Path::new(d).join(cmd);
            if cand.exists() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Parse `Maximum amplitude:` (0.0–1.0) out of `sox … -n stat` stderr. Pure so it's unit-testable.
fn parse_max_amplitude(stat_stderr: &str) -> Option<f32> {
    for line in stat_stderr.lines() {
        if let Some(rest) = line.trim().strip_prefix("Maximum amplitude:") {
            return rest.trim().parse::<f32>().ok();
        }
    }
    None
}

/// Peak amplitude of a WAV via `sox <path> -n stat` (stats go to stderr). Returns None
/// when sox is unavailable or can't analyze the file — in that case we don't block
/// transcription (fail-open on analysis, the guard only fires on a *confirmed* silent file).
async fn max_amplitude(path: &Path) -> Option<f32> {
    let sox = which("sox")?;
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(&sox)
            .args([path.to_str()?, "-n", "stat"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_max_amplitude(&String::from_utf8_lossy(&out.stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    // 021-voice-es — modelo multilingüe + idioma.
    #[test]
    fn model_variant_from_setting_defaults_to_base() {
        assert_eq!(VoiceModel::from_setting("base"), VoiceModel::Base);
        assert_eq!(VoiceModel::from_setting("small"), VoiceModel::Small);
        assert_eq!(VoiceModel::from_setting("SMALL"), VoiceModel::Small);
        // Desconocido/vacío → Base (multilingüe seguro).
        assert_eq!(VoiceModel::from_setting(""), VoiceModel::Base);
        assert_eq!(VoiceModel::from_setting("tiny.en"), VoiceModel::Base);
    }

    #[test]
    fn model_path_respects_variant() {
        // El path termina en el archivo de la variante configurada.
        let base = model_path(VoiceModel::Base).unwrap();
        assert!(base.to_string_lossy().ends_with("ggml-base.bin"), "{base:?}");
        let small = model_path(VoiceModel::Small).unwrap();
        assert!(
            small.to_string_lossy().ends_with("ggml-small.bin"),
            "{small:?}"
        );
    }

    #[test]
    fn model_url_and_sha_are_official_per_variant() {
        assert!(VoiceModel::Base.url().ends_with("ggml-base.bin"));
        assert!(VoiceModel::Small.url().ends_with("ggml-small.bin"));
        assert_eq!(
            VoiceModel::Base.sha256(),
            "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"
        );
        assert_eq!(
            VoiceModel::Small.sha256(),
            "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"
        );
    }

    #[test]
    fn whisper_args_include_lang_flag() {
        // El bug original: faltaba `-l`. Ahora SIEMPRE va `-l <lang>`.
        let args = whisper_args("/m/ggml-base.bin", "/tmp/a.wav", "es");
        // -l seguido de "es".
        let li = args.iter().position(|a| a == "-l").expect("-l presente");
        assert_eq!(args[li + 1], "es");
        // -m con el modelo configurado.
        let mi = args.iter().position(|a| a == "-m").expect("-m presente");
        assert_eq!(args[mi + 1], "/m/ggml-base.bin");
        // -f con el audio.
        let fi = args.iter().position(|a| a == "-f").expect("-f presente");
        assert_eq!(args[fi + 1], "/tmp/a.wav");
        // flags de plano-texto sin timestamps siguen.
        assert!(args.iter().any(|a| a == "-nt"));
        assert!(args.iter().any(|a| a == "-otxt"));
    }

    #[test]
    fn normalize_lang_maps_values() {
        assert_eq!(normalize_lang("es"), "es");
        assert_eq!(normalize_lang("ES"), "es");
        assert_eq!(normalize_lang("auto"), "auto");
        assert_eq!(normalize_lang("en"), "en");
        // Vacío → default español.
        assert_eq!(normalize_lang(""), "es");
        assert_eq!(normalize_lang("  "), "es");
    }

    #[test]
    fn parse_max_amplitude_works() {
        // Real `sox … -n stat` stderr shape.
        let silent = "Samples read:             48000\nMaximum amplitude:     0.000000\nRMS     amplitude:     0.000000\n";
        assert_eq!(parse_max_amplitude(silent), Some(0.0));
        let loud = "Maximum amplitude:     0.705003\nMean    norm:          0.448777\n";
        assert!((parse_max_amplitude(loud).unwrap() - 0.705003).abs() < 1e-4);
        // No stat line → None (fail-open: we don't block transcription on a parse miss).
        assert_eq!(parse_max_amplitude("could not analyze\n"), None);
    }
}
