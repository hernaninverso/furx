#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// A bundled `.app` launched from Finder/Dock inherits launchd's minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`) — Homebrew's `/opt/homebrew/bin` is absent, so
/// `which("sox")` / `which("whisper-cli")` return None and Voice/push-to-talk dies
/// silently (no recording, no indicator). Prepend the common Homebrew prefixes so the
/// bundled app resolves CLI deps the same way a terminal-launched dev build does.
#[cfg(target_os = "macos")]
fn augment_path_for_bundled_app() {
    let cur = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = cur
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let mut dirs: Vec<String> = vec!["/usr/local/bin".into(), "/opt/homebrew/bin".into()];
    // 062: el grok CLI de xAI se instala en ~/.grok/bin (fuera de los prefijos estándar) → agregarlo o
    // el `.app` empaquetado no lo encuentra (mismo fallo que sox/whisper-cli con el PATH de launchd).
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".grok").join("bin").to_string_lossy().into_owned());
    }
    for dir in dirs {
        if !parts.contains(&dir) {
            parts.insert(0, dir);
        }
    }
    std::env::set_var("PATH", parts.join(":"));
}

fn main() {
    #[cfg(target_os = "macos")]
    augment_path_for_bundled_app();

    // F3.2 — --memory-daemon: run headless memory hub only (for LaunchAgent).
    if std::env::args().any(|a| a == "--memory-daemon") {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        rt.block_on(async {
            let home = dirs::home_dir().expect("home");
            let db_path = home.join(".furx").join("memory.db");
            std::fs::create_dir_all(home.join(".furx")).ok();
            let conn = furx_lib::db::open(&db_path).expect("db");
            let db = std::sync::Arc::new(parking_lot::Mutex::new(conn));
            furx_lib::services::memory_daemon::start(db)
                .await
                .expect("memory daemon");
        });
        return;
    }

    // 060 v2 — `furx configure --profile <archivo.json> [--dry-run]`: un agente CLI propone un perfil;
    // Furx aplica SÓLO la allowlist segura + rechaza lo demás (headless, sin GUI). Ver services/configure.rs.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(|s| s.as_str()) == Some("configure") {
            let profile = args.windows(2).find(|w| w[0] == "--profile").map(|w| w[1].clone());
            let dry_run = args.iter().any(|a| a == "--dry-run");
            std::process::exit(furx_lib::services::configure::run_cli(profile.as_deref(), dry_run));
        }
        if args.get(1).map(|s| s.as_str()) == Some("council") {
            use std::io::Read;
            let use_stdin = args.iter().any(|a| a == "--stdin");
            let prompt_arg = args
                .windows(2)
                .find(|w| w[0] == "--prompt")
                .map(|w| w[1].clone());
            let prompt = if use_stdin {
                let mut input = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut input) {
                    eprintln!("no se pudo leer stdin: {e}");
                    std::process::exit(2);
                }
                input
            } else {
                prompt_arg.unwrap_or_default()
            };
            let preset = args
                .windows(2)
                .find(|w| w[0] == "--preset")
                .map(|w| w[1].clone());
            let template = args
                .windows(2)
                .find(|w| w[0] == "--template")
                .map(|w| w[1].clone());
            let max_voices = args
                .windows(2)
                .find(|w| w[0] == "--max-voices")
                .and_then(|w| w[1].parse::<usize>().ok());
            std::process::exit(furx_lib::services::council_multi::run_cli(
                prompt,
                preset,
                template,
                max_voices,
            ));
        }
    }

    // BLOQUE 4 · Linux NVIDIA + Wayland workaround.
    // Council EDGE_1 B4: WebKitGTK 4.1 crashea al lanzar Tauri en sistemas NVIDIA + Wayland.
    // Workaround conocido: deshabilitar el DMA-BUF renderer + composiciónal mode antes
    // de inicializar WebView. Solo aplica en Linux y solo si los envvars no están seteados
    // (deja al usuario override-ar).
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("WEBKIT_DISABLE_COMPOSITING_MODE").is_none() {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        }
    }

    furx_lib::run();
}
