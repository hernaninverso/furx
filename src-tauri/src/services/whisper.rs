// F19 — Whisper voice (partial). This sprint ships *detection* only: we
// check whether `whisper-cli` (and a model file) are installed locally and
// the user has `sox` for capture. If both are present the UI offers a
// "Transcribe last 5s" button; if not it shows an install hint.
//
// Audio capture + actual transcription wiring is opt-in and intentionally
// out of the sprint scope (CoreAudio + native bindings require their own
// hardening pass).

use crate::services::voice::VoiceModel;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct WhisperCheck {
    pub whisper_cli: Option<String>,
    pub model_path: Option<String>,
    pub sox: Option<String>,
    pub ready: bool,
    pub install_hint: String,
    /// 021-voice-es — true cuando existe el `ggml-tiny.en.bin` viejo (monolingüe inglés)
    /// pero NO el modelo multilingüe configurado. El front lo usa para explicar la migración.
    pub needs_migration: bool,
}

/// 021-voice-es — `check` ahora valida el modelo CONFIGURADO (`base`|`small`), no el
/// tiny.en hardcodeado. Si falta el configurado pero está el tiny.en viejo, reporta
/// `ready:false` + `needs_migration:true` con un hint para bajar el correcto. El tiny.en
/// NUNCA se borra automáticamente (convivencia).
pub fn check(model: VoiceModel) -> WhisperCheck {
    let whisper = which("whisper-cli").or_else(|| which("whisper"));
    let sox = which("sox");
    let configured = model_file_path(model.filename());
    let model_str = configured
        .as_ref()
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string());
    // ¿Está el tiny.en viejo pero falta el configurado? → migración pendiente.
    let legacy_tiny_present = model_file_path("ggml-tiny.en.bin")
        .map(|p| p.exists())
        .unwrap_or(false);
    let needs_migration = model_str.is_none() && legacy_tiny_present;
    let ready = whisper.is_some() && sox.is_some() && model_str.is_some();
    let install_hint = if ready {
        "ready".to_string()
    } else {
        let mut steps = Vec::new();
        if whisper.is_none() {
            steps.push("brew install whisper-cpp".to_string());
        }
        if sox.is_none() {
            steps.push("brew install sox".to_string());
        }
        if model_str.is_none() {
            if needs_migration {
                steps.push(format!(
                    "Migración: el modelo inglés viejo (ggml-tiny.en.bin) no entiende español. \
                     Descargá el modelo multilingüe configurado ({}) — el viejo podés dejarlo o borrarlo a mano.",
                    model.filename()
                ));
            }
            steps.push(format!(
                "mkdir -p ~/.furx/whisper && curl -L -o ~/.furx/whisper/{} {}",
                model.filename(),
                model.url()
            ));
        }
        if steps.is_empty() {
            "ready".to_string()
        } else {
            steps.join("\n")
        }
    };
    WhisperCheck {
        whisper_cli: whisper,
        model_path: model_str,
        sox,
        ready,
        install_hint,
        needs_migration,
    }
}

fn model_file_path(filename: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".furx").join("whisper").join(filename))
}

fn which(cmd: &str) -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let p = std::path::Path::new(dir).join(cmd);
            if p.exists() {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }
    let out = Command::new("/usr/bin/which").arg(cmd).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
