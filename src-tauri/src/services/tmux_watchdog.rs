// F24 — Tmux launchd watchdog.
// Installs/uninstalls a LaunchAgent plist that keeps the tmux server alive
// (`tmux start-server` doesn't survive logout, but a KeepAlive=true plist does).
// Also exposes a "list FURX_* sessions" probe for the boot-restore modal.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

const PLIST_LABEL: &str = "cloud.furx.desktop.tmux";

#[derive(Debug, Clone, Serialize)]
pub struct WatchdogStatus {
    pub plist_path: String,
    pub installed: bool,
    pub loaded: bool,
    pub tmux_bin: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FurxSession {
    pub name: String,
    pub created: Option<String>,
}

pub fn plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("no home"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", PLIST_LABEL)))
}

pub fn status() -> Result<WatchdogStatus> {
    let p = plist_path()?;
    let installed = p.exists();
    let loaded = Command::new("/bin/launchctl")
        .args(["list", PLIST_LABEL])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let tmux_bin = Command::new("/usr/bin/which")
        .arg("tmux")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(WatchdogStatus {
        plist_path: p.to_string_lossy().to_string(),
        installed,
        loaded,
        tmux_bin,
    })
}

pub fn install() -> Result<WatchdogStatus> {
    let tmux = Command::new("/usr/bin/which")
        .arg("tmux")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("tmux not in PATH — `brew install tmux` first"))?;
    let p = plist_path()?;
    std::fs::create_dir_all(p.parent().ok_or_else(|| anyhow!("no parent"))?)?;
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".furx")
        .join("tmux-watchdog.log");
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{tmux}</string>
    <string>-L</string>
    <string>furx</string>
    <string>start-server</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key><false/>
  </dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        label = PLIST_LABEL,
        tmux = xml_escape(&tmux),
        log = xml_escape(&log_path.to_string_lossy()),
    );
    std::fs::write(&p, plist)?;
    // launchctl bootstrap / load — best-effort. New macOS prefers bootstrap; old prefers load.
    let _ = Command::new("/bin/launchctl")
        .args(["unload", p.to_string_lossy().as_ref()])
        .output();
    let load_out = Command::new("/bin/launchctl")
        .args(["load", p.to_string_lossy().as_ref()])
        .output()?;
    if !load_out.status.success() {
        return Err(anyhow!(
            "launchctl load failed: {}",
            String::from_utf8_lossy(&load_out.stderr).trim()
        ));
    }
    status()
}

pub fn uninstall() -> Result<()> {
    let p = plist_path()?;
    if p.exists() {
        let _ = Command::new("/bin/launchctl")
            .args(["unload", p.to_string_lossy().as_ref()])
            .output();
        std::fs::remove_file(&p)?;
    }
    Ok(())
}

/// Returns FURX_* tmux sessions, if any. Used by boot restore modal.
pub fn list_furx_sessions() -> Vec<FurxSession> {
    // -F '#{session_name}|#{session_created_string}' gives parseable output.
    // 058 — `-L furx`: lista las sesiones del socket DEDICADO de Furx (no las del server del usuario).
    let out = Command::new("tmux")
        .args([
            "-L",
            "furx",
            "list-sessions",
            "-F",
            "#{session_name}|#{session_created_string}",
        ])
        .output();
    let Ok(out) = out else {
        return vec![];
    };
    if !out.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '|');
            let name = parts.next()?.to_string();
            if !name.starts_with("FURX_") {
                return None;
            }
            let created = parts.next().map(String::from);
            Some(FurxSession { name, created })
        })
        .collect()
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_handles_special_chars() {
        assert_eq!(xml_escape("a&b<c>d"), "a&amp;b&lt;c&gt;d");
    }
}
