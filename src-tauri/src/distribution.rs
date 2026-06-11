// F44 update check (read-only ping) · F45 telemetry helpers · F47 compat matrix · F48 reset.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct CompatReport {
    pub macos_ok: bool,
    pub macos_version: String,
    pub arch_ok: bool,
    pub arch: String,
    pub claude_cli: Option<String>,
    pub codex_cli: Option<String>,
    pub gemini_cli: Option<String>,
    pub aider_cli: Option<String>,
    pub grok_cli: Option<String>, // 062: xAI Grok CLI (~/.grok/bin/grok)
    pub tmux: Option<String>,
    pub git: Option<String>,
    pub all_ok: bool,
}

pub fn compat_check() -> CompatReport {
    let arch = std::env::consts::ARCH.to_string();
    let arch_ok = arch == "aarch64";

    let macos_version = sw_vers().unwrap_or_else(|| "unknown".into());
    let macos_ok = macos_version
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .map(|major| major >= 11)
        .unwrap_or(false);

    // Las 6 detecciones corren en PARALELO con timeout por probe. Antes eran SECUENCIALES y cada
    // `which_version` spawnea `<cli> --version`; los CLIs Node/Python (claude/codex/gemini/aider) tardan
    // ~1s en arrancar → la suma estancaba el Settings varios segundos. Concurrente = ~max (no la suma);
    // el timeout evita que un CLI colgado (el comentario de `which_version` ya nota que el wrapper de
    // claude puede colgarse) bloquee la pantalla. (Perf-fix Settings 2026-06-03.)
    let probe = |cmd: &'static str| -> std::sync::mpsc::Receiver<Option<String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(which_version(cmd));
        });
        rx
    };
    // DEADLINE GLOBAL (audit codex): recoger todos los receivers contra UN deadline compartido (no un
    // timeout de 3s POR recv secuencial — eso daría 6×3s=18s en el peor caso). Así el wall-time total
    // queda capeado a ~3s aunque varias probes tarden. (Cada probe además se auto-mata a los 3s en
    // `run_version_with_timeout`, así no quedan subprocesos vivos.)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let recv = |rx: std::sync::mpsc::Receiver<Option<String>>| {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        rx.recv_timeout(remaining).unwrap_or(None)
    };
    // Lanzar TODAS primero (concurrentes), recoger después (cada recv ya casi resuelta).
    let (r_claude, r_codex, r_gemini, r_aider, r_grok, r_tmux, r_git) = (
        probe("claude"),
        probe("codex"),
        probe("gemini"),
        probe("aider"),
        probe("grok"),
        probe("tmux"),
        probe("git"),
    );
    let claude_cli = recv(r_claude);
    let codex_cli = recv(r_codex);
    let gemini_cli = recv(r_gemini);
    let aider_cli = recv(r_aider);
    let grok_cli = recv(r_grok);
    let tmux = recv(r_tmux);
    let git = recv(r_git);

    let all_ok = macos_ok && arch_ok && tmux.is_some() && git.is_some();

    CompatReport {
        macos_ok,
        macos_version,
        arch_ok,
        arch,
        claude_cli,
        codex_cli,
        gemini_cli,
        aider_cli,
        grok_cli,
        tmux,
        git,
        all_ok,
    }
}

fn sw_vers() -> Option<String> {
    let out = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn which_version(cmd: &str) -> Option<String> {
    let path = Command::new("/usr/bin/which").arg(cmd).output().ok()?;
    if !path.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&path.stdout).trim().to_string();
    // Try `cmd --version`. Some tools (claude wrapper) hang on no-args, so always pass --version.
    // Audit codex (perf-fix 2026-06-03): un `--version` COLGADO (el wrapper de claude puede colgarse)
    // dejaría el subproceso vivo y el thread bloqueado. Lo corremos con timeout + KILL del hijo.
    let raw = run_version_with_timeout(&p, std::time::Duration::from_secs(3))?;
    if raw.is_empty() {
        Some(format!("present @ {}", p))
    } else {
        // First line only to keep it compact.
        Some(raw.lines().next().unwrap_or(&raw).to_string())
    }
}

/// Corre `<prog> --version` con timeout. Si excede, MATA el proceso hijo (no deja zombies) y devuelve
/// `None`. Output (stdout) leído tras la salida del hijo (`--version` es chico → el pipe no se llena).
fn run_version_with_timeout(prog: &str, timeout: std::time::Duration) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new(prog)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            // try_wait() con `Ok(Some(_))` YA REAPEA el hijo en Unix (waitpid lo recolecta) → NO queda
            // zombie; el stdout bufferizado se lee abajo (sin `wait()` previo → sin pérdida de output).
            Ok(Some(_)) => break, // terminó solo (ya reapeado)
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait(); // reap (no zombie)
                    return None; // colgado/timeout → tratar como no-disponible
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut s = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut s);
    }
    Some(s.trim().to_string())
}

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub url: Option<String>,
    pub error: Option<String>,
}

pub async fn check_updates(endpoint: &str, current: &str) -> UpdateInfo {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent(format!("furx/{}", current))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return UpdateInfo {
                current: current.into(),
                latest: None,
                url: None,
                error: Some(e.to_string()),
            }
        }
    };
    let resp = match client.get(endpoint).send().await {
        Ok(r) => r,
        Err(e) => {
            return UpdateInfo {
                current: current.into(),
                latest: None,
                url: None,
                error: Some(e.to_string()),
            }
        }
    };
    if !resp.status().is_success() {
        return UpdateInfo {
            current: current.into(),
            latest: None,
            url: None,
            error: Some(format!("HTTP {}", resp.status())),
        };
    }
    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return UpdateInfo {
                current: current.into(),
                latest: None,
                url: None,
                error: Some(e.to_string()),
            }
        }
    };
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string());
    let url = body
        .get("html_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    UpdateInfo {
        current: current.into(),
        latest: tag,
        url,
        error: None,
    }
}

#[derive(Debug, Serialize)]
pub struct ResetReport {
    pub level: String,
    pub removed: Vec<String>,
}

/// F48 Uninstaller / reset.
/// Levels: "soft" (data only), "hard" (data + settings), "full" (data + settings + Keychain hints).
pub fn reset(level: &str) -> Result<ResetReport> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    let furx_dir = home.join(".furx");
    let mut removed = Vec::new();
    match level {
        "soft" => {
            for f in ["furx.db-wal", "furx.db-shm"] {
                let p = furx_dir.join(f);
                if p.exists() {
                    std::fs::remove_file(&p)?;
                    removed.push(p.display().to_string());
                }
            }
        }
        "hard" => {
            if furx_dir.exists() {
                std::fs::remove_dir_all(&furx_dir)?;
                removed.push(furx_dir.display().to_string());
            }
        }
        "full" => {
            if furx_dir.exists() {
                std::fs::remove_dir_all(&furx_dir)?;
                removed.push(furx_dir.display().to_string());
            }
            // Note: Keychain entries are kept by default — `security delete-generic-password`
            // requires elevated trust we don't claim. We list them so the user can clean manually.
            removed.push("Keychain entries to remove manually: claude-max-A, claude-max-B".into());
        }
        _ => return Err(anyhow!("unknown reset level: {}", level)),
    }
    Ok(ResetReport {
        level: level.into(),
        removed,
    })
}
