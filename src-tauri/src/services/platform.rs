//! Helpers cross-platform (spec 067). Centraliza lo no-portable para que el resto del código no
//! tenga `std::os::unix` disperso (que rompe la compilación de Windows).
//!
//! Regla del council: `cfg(unix)` = Linux + macOS (POSIX real: permisos, modes). `cfg(target_os
//! = "macos")` = SOLO macOS (LaunchAgent, Keychain, codesign). NO confundir: Linux ES unix.
//!
//! NOTA (spec 067): estas funciones quedan compilables en Windows POR CONSTRUCCIÓN (cfg-guards);
//! NO se compilaron en Windows todavía (cross-check falla en deps C desde la Mac) — pendiente CI/VM.

use std::path::Path;

/// Error tipado único de "feature no soportada en esta plataforma" (council: no strings/panic).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlatformUnsupported {
    pub feature: &'static str,
    pub platform: &'static str,
    pub remediation: Option<&'static str>,
}

impl PlatformUnsupported {
    pub fn here(feature: &'static str, remediation: Option<&'static str>) -> Self {
        Self { feature, platform: std::env::consts::OS, remediation }
    }
}

impl std::fmt::Display for PlatformUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} no está soportado en {}", self.feature, self.platform)?;
        if let Some(r) = self.remediation {
            write!(f, " ({r})")?;
        }
        Ok(())
    }
}
impl std::error::Error for PlatformUnsupported {}

/// Aplica un modo POSIX a un archivo. Unix (Linux+macOS): set_permissions con el mode. Windows:
/// no-op (no hay bit de permisos POSIX; el ACL es otro modelo). Devuelve Ok aunque sea no-op.
pub fn set_file_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        // Windows no tiene el modelo POSIX. Si el mode pretende RESTRINGIR (denegar lectura a
        // group/other — el patrón de un secret 0o600), NO podemos garantizarlo sin un ACL, así que
        // devolvemos un error DISTINGUIBLE en vez de un Ok() silencioso que haría creer al call-site
        // que el archivo quedó privado (audit MEDIUM: secrets quedarían readable sin señal alguna).
        // Modes no-restrictivos (group/other con algún permiso) → no-op Ok (no son control de seguridad).
        if mode & 0o077 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "restrictive POSIX mode not enforceable on Windows without an ACL — caller must \
                 apply a Windows ACL or not assume confidentiality",
            ));
        }
        Ok(())
    }
}

/// Lee el modo POSIX de un archivo. Unix: Some(mode). Windows: None (no aplica).
pub fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// ¿El archivo tiene bit de ejecución? Unix: mode & 0o111. Windows: la ejecutabilidad la da la
/// EXTENSIÓN (.exe/.bat/.cmd/.com), no un permiso — chequearla (audit: un archivo de texto suelto
/// que `where` matchee literalmente NO debe pasar como ejecutable).
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        path.is_file() && file_mode(path).map(|m| m & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "exe" || e == "bat" || e == "cmd" || e == "com"
                })
                .unwrap_or(false)
    }
}

/// Resuelve un binario en el PATH. Unix: `/usr/bin/which`. Windows: `where.exe` con RUTA ABSOLUTA
/// (`%SystemRoot%\System32\where.exe`).
///
/// SECURITY (audit MEDIUM-HIGH): NUNCA spawnear `where` pelado — en Windows la resolución de
/// `CreateProcess` incluye el directorio de la app y a veces el CWD, así que un `where.exe`
/// plantado se ejecutaría, y su salida después se spawnea como el binario buscado (→ RCE). El
/// path absoluto lo evita, igual que el `/usr/bin/which` hardcodeado en Unix.
pub fn which(bin: &str) -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    let prog = std::path::PathBuf::from("/usr/bin/which");
    // SECURITY (audit MEDIUM round-2): NO derivar de la env var `SystemRoot` — el parent process
    // que lanza Furx la controla SIN elevación, y podría apuntarla a un `where.exe` plantado. Usamos
    // la ubicación canónica fija de System32. Si Windows está en otra unidad (raro), `is_file()`
    // falla → None (caemos al env override / venv, nunca a un `where` inseguro).
    #[cfg(windows)]
    let prog = std::path::PathBuf::from("C:\\Windows\\System32\\where.exe");
    #[cfg(not(any(unix, windows)))]
    {
        let _ = bin;
        return None;
    }
    #[cfg(any(unix, windows))]
    {
        if !prog.is_file() {
            return None;
        }
        let out = std::process::Command::new(&prog).arg(bin).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines()
            .next()
            .map(|l| std::path::PathBuf::from(l.trim()))
            .filter(|p| !p.as_os_str().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_display() {
        let u = PlatformUnsupported::here("LaunchAgent", Some("usá un servicio del SO"));
        assert!(u.to_string().contains("LaunchAgent"));
    }

    #[test]
    fn set_and_read_mode_roundtrip() {
        let dir = std::env::temp_dir().join(format!("furx-plat-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("x");
        std::fs::write(&f, b"x").unwrap();
        #[cfg(unix)]
        {
            set_file_mode(&f, 0o600).unwrap();
            assert_eq!(file_mode(&f).map(|m| m & 0o777), Some(0o600));
        }
        #[cfg(not(unix))]
        {
            // mode restrictivo (0o600) → Err DISTINGUIBLE, no Ok() silencioso
            assert!(set_file_mode(&f, 0o600).is_err());
            // mode no-restrictivo (0o644) → Ok no-op
            assert!(set_file_mode(&f, 0o644).is_ok());
            assert_eq!(file_mode(&f), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
