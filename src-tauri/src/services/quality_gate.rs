// 024-quality-gate F0 — motor de evidencia objetiva por variante (linters/typecheck).
//
// Corre los linters/typecheckers del PROPIO repo del usuario (clippy / eslint+tsc /
// ruff+mypy — autodetectados por los manifiestos) sobre CADA variante de un best-of-N
// (en su worktree aislado, reusando el aislamiento de 019), parsea la salida a un conteo
// estructurado `{errors, warnings, by_tool, by_severity}` y la devuelve como evidencia
// ADVISORY para la UI de comparación.
//
// CONTRATO FAIL-SAFE (invariante de diseño, NO opcional): un linter ausente / que falla /
// timeout / sandbox-falla / salida no parseable se marca `unavailable | timeout | unparsable`
// — NUNCA un `0` falso (un `0` falso diría "limpio" cuando en realidad NO se midió).
//
// LOCAL-FIRST / BYOK: todo ocurre en la Mac, en el worktree de la variante. Nada sale a la
// nube; no se usan API keys ni el AIE (la señal es determinista).
//
// SEGURIDAD (council v2, HIGH — el diferencial de gobierno): defensa en capas. El subproceso
// del linter corre con (1) opt-in default OFF [gate en el comando], (2) allow-list de linters
// conocidos, (3) timeout + kill_on_drop, (4) cwd = worktree, (5) argv-only sin shell, (6)
// `sandbox-exec` de macOS (Seatbelt): deny-default + sin red + FS-confine al worktree. Si el
// sandbox falla/no está → fail-safe: el linter se marca `unavailable (sandbox)` y NO se corre
// sin aislamiento por default. Modelo de amenaza en `specs/024-quality-gate/clarify.md`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ── Tipos compartidos (motor ↔ UI; espejo TS en web/src/types.ts) ──────────────

/// Estado del resultado de correr UNA herramienta sobre UNA variante. Distingue el
/// éxito (`ok`, con conteo real) de las distintas formas de "no se midió" — que NUNCA
/// deben confundirse con `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinterStatus {
    /// Corrió y la salida se parseó: `errors`/`warnings` son reales.
    Ok,
    /// Binario ausente / no encontrado, o sandbox no disponible. NO es `0`.
    Unavailable,
    /// Excedió el timeout. NO es `0`.
    Timeout,
    /// Corrió pero la salida no se pudo interpretar (formato inesperado). NO es `0`.
    Unparsable,
}

/// Una issue individual del detalle clickeable (cap configurable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub file: String,
    pub line: u32,
    /// Regla/código del linter (p.ej. `clippy::needless_return`, `no-unused-vars`, `F401`).
    pub rule: String,
    pub message: String,
    /// `"error"` | `"warning"`.
    pub severity: String,
}

/// Outcome de correr UNA herramienta sobre UNA variante.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinterResult {
    /// id del linter (`clippy`, `eslint`, `tsc`, `ruff`, `mypy`, `cargo_check`).
    pub tool: String,
    pub status: LinterStatus,
    pub errors: u32,
    pub warnings: u32,
    /// Primeras N issues (cap). Vacío si no-ok o si el linter no las reporta.
    #[serde(default)]
    pub issues: Vec<Issue>,
    /// Motivo legible cuando `status != ok` (ej "binario no encontrado", "timeout",
    /// "sandbox no disponible", "salida no interpretable").
    #[serde(default)]
    pub reason: Option<String>,
    /// Extracto crudo de stdout/stderr cuando no-ok (diagnóstico). Capado.
    #[serde(default)]
    pub raw_excerpt: Option<String>,
    pub elapsed_ms: u64,
}

impl LinterResult {
    /// Constructor del caso fail-safe: NUNCA emite un conteo (`errors=warnings=0` pero
    /// con `status != ok`, así la UI muestra "no disponible", no "0 issues").
    fn unavailable(tool: &str, reason: impl Into<String>, raw: Option<String>, elapsed_ms: u64) -> Self {
        Self {
            tool: tool.to_string(),
            status: LinterStatus::Unavailable,
            errors: 0,
            warnings: 0,
            issues: vec![],
            reason: Some(reason.into()),
            raw_excerpt: raw.map(|r| cap_excerpt(&r)),
            elapsed_ms,
        }
    }
}

/// Agregación por variante: lo que consume la UI (y, opt-in en F2, el ranking advisory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantEvidence {
    /// task_id de la variante.
    pub task_id: String,
    pub total_errors: u32,
    pub total_warnings: u32,
    /// Un `LinterResult` por herramienta aplicable (incluye los unavailable/timeout).
    pub by_tool: Vec<LinterResult>,
    /// Lista de herramientas que quedaron "no disponible" (transparencia: qué NO se midió).
    pub unavailable_tools: Vec<String>,
    /// `true` si NINGUNA herramienta produjo una medición real (`status == ok`). La UI lo usa
    /// para distinguir "0 errores medidos" de "nada se pudo medir".
    pub any_measured: bool,
}

/// Especificación de un linter detectable (sin estado). Resuelve a un comando argv-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinterSpec {
    /// id estable (`clippy`, `eslint`, ...).
    pub id: String,
    /// Binario a ejecutar (debe estar en la allow-list).
    pub bin: String,
    /// Args argv-only (sin shell).
    pub args: Vec<String>,
    /// Manifiesto que disparó la detección (informativo).
    pub manifest: String,
    /// Formato de salida que el parser espera.
    pub format: LinterFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinterFormat {
    ClippyJson,
    EslintJson,
    RuffJson,
    TscText,
    MypyText,
}

// ── Allow-list + defaults (council v2 §1, §5) ─────────────────────────────────

/// Linters canónicos detectados por default (council v2 §1). `cargo_check` NO está acá
/// (es opt-in por costo, §5) aunque sí es un binario válido de la allow-list.
pub const DEFAULT_LINTERS: &[&str] = &["clippy", "eslint", "tsc", "ruff", "mypy"];

/// Allow-list de binarios que el motor tiene permitido invocar. Nada fuera de esto se ejecuta.
pub const ALLOWED_BINS: &[&str] = &["cargo", "eslint", "npx", "tsc", "ruff", "mypy"];

fn bin_is_allowed(bin: &str) -> bool {
    ALLOWED_BINS.contains(&bin)
}

/// Timeout por herramienta (council v2 §3.3). Holgado porque clippy recompila.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// Cap de issues guardadas en el detalle por (variante, herramienta) — evita saturar UI/memoria.
pub const ISSUE_CAP: usize = 50;

/// Cap del extracto crudo conservado en un resultado no-ok.
const RAW_CAP: usize = 2000;

fn cap_excerpt(s: &str) -> String {
    if s.len() <= RAW_CAP {
        s.to_string()
    } else {
        let mut out = s.chars().take(RAW_CAP).collect::<String>();
        out.push_str("…[truncado]");
        out
    }
}

// ── F0.1 — Detección (pura, NO ejecuta nada; sólo lee manifiestos) ────────────

/// Autodetecta qué linters aplican a un worktree por sus manifiestos. Determinista,
/// read-only (FR-001). `enabled` = los ids habilitados por el usuario (allow-list
/// configurable, `qualitygate.linters`); por default = `DEFAULT_LINTERS`.
pub fn detect_linters(worktree: &Path, enabled: &[String]) -> Vec<LinterSpec> {
    let mut out = Vec::new();
    let allow = |id: &str| enabled.iter().any(|e| e == id);

    // Rust: Cargo.toml → clippy (default) ; cargo_check (opt-in, §5).
    if worktree.join("Cargo.toml").is_file() {
        if allow("clippy") {
            out.push(LinterSpec {
                id: "clippy".into(),
                bin: "cargo".into(),
                args: vec![
                    "clippy".into(),
                    "--message-format=json".into(),
                    "--quiet".into(),
                ],
                manifest: "Cargo.toml".into(),
                format: LinterFormat::ClippyJson,
            });
        }
        if allow("cargo_check") {
            out.push(LinterSpec {
                id: "cargo_check".into(),
                bin: "cargo".into(),
                args: vec!["check".into(), "--message-format=json".into(), "--quiet".into()],
                manifest: "Cargo.toml".into(),
                format: LinterFormat::ClippyJson, // mismo formato de diagnóstico cargo.
            });
        }
    }

    // JS/TS: package.json → eslint + tsc.
    if worktree.join("package.json").is_file() {
        if allow("eslint") && has_eslint_config(worktree) {
            out.push(LinterSpec {
                id: "eslint".into(),
                bin: "eslint".into(),
                args: vec!["-f".into(), "json".into(), ".".into()],
                manifest: "package.json".into(),
                format: LinterFormat::EslintJson,
            });
        }
        if allow("tsc") && worktree.join("tsconfig.json").is_file() {
            out.push(LinterSpec {
                id: "tsc".into(),
                bin: "tsc".into(),
                args: vec!["--noEmit".into(), "--pretty".into(), "false".into()],
                manifest: "tsconfig.json".into(),
                format: LinterFormat::TscText,
            });
        }
    }

    // Python: pyproject.toml / ruff.toml / setup.cfg → ruff + mypy.
    let has_py = worktree.join("pyproject.toml").is_file()
        || worktree.join("ruff.toml").is_file()
        || worktree.join("setup.cfg").is_file();
    if has_py {
        let manifest = if worktree.join("pyproject.toml").is_file() {
            "pyproject.toml"
        } else if worktree.join("ruff.toml").is_file() {
            "ruff.toml"
        } else {
            "setup.cfg"
        };
        if allow("ruff") {
            out.push(LinterSpec {
                id: "ruff".into(),
                bin: "ruff".into(),
                args: vec![
                    "check".into(),
                    "--output-format=json".into(),
                    ".".into(),
                ],
                manifest: manifest.into(),
                format: LinterFormat::RuffJson,
            });
        }
        if allow("mypy") && worktree.join("pyproject.toml").is_file() {
            out.push(LinterSpec {
                id: "mypy".into(),
                bin: "mypy".into(),
                args: vec!["--no-color-output".into(), "--no-error-summary".into(), ".".into()],
                manifest: "pyproject.toml".into(),
                format: LinterFormat::MypyText,
            });
        }
    }

    out
}

/// Heurística read-only para "este repo usa eslint": archivos de config comunes.
fn has_eslint_config(worktree: &Path) -> bool {
    const CANDIDATES: &[&str] = &[
        ".eslintrc",
        ".eslintrc.js",
        ".eslintrc.cjs",
        ".eslintrc.json",
        ".eslintrc.yml",
        ".eslintrc.yaml",
        "eslint.config.js",
        "eslint.config.mjs",
        "eslint.config.cjs",
        "eslint.config.ts",
    ];
    CANDIDATES.iter().any(|c| worktree.join(c).is_file())
}

// ── F0.2 — Perfil sandbox-exec (Seatbelt). Puro/testeable. ────────────────────

/// Subdir confinado para los temporales del toolchain, BAJO el worktree. El runner exporta
/// `TMPDIR`/`TMP`/`TEMP` apuntando acá para que rustc/cargo/node escriban su tmp DENTRO del
/// worktree (no en el `/private/tmp` global ni `/private/var/folders`).
pub const QG_TMP_SUBDIR: &str = ".qg-tmp";

/// Construye el perfil Seatbelt (`sandbox-exec -p <profile>`). `deny default` + SIN red
/// + lectura MÍNIMA del toolchain + lectura/escritura SÓLO bajo el worktree (council v2 §3.6).
///
/// PURO (no toca el FS más allá de canonicalizar el path de entrada). El llamador pasa el
/// worktree (canonicalizado) y los roots del toolchain (home-derivados). NO se agrega
/// `(allow network*)` — la red queda denegada por el deny-default.
///
/// SEGURIDAD (audit 3-frontera 024, HIGH 1 + HIGH/MED 3):
/// - `file-write*` SÓLO bajo el worktree (que incluye `<worktree>/.qg-tmp`). NO se permite
///   `/private/tmp` ni `/private/var/folders` (un build.rs/proc-macro/postinstall malicioso
///   ya no puede escribir fuera del worktree). Los temporales del toolchain van al subdir
///   confinado vía `TMPDIR` que setea el runner.
/// - `file-read*` MÍNIMO: sólo las rutas del toolchain estrictamente necesarias. NUNCA `$HOME`
///   a secas ni dirs de credenciales (`~/.ssh`, `~/.aws`, `~/.config`, `~/.furx` con BYOK).
pub fn build_sandbox_profile(worktree: &Path, home: &Path) -> String {
    let wt = sb_path(worktree);
    let home_s = home.to_string_lossy();

    // Roots de lectura del toolchain (subpath = el dir y todo lo de abajo). MÍNIMO: sólo lo
    // que cargo/rustc/node/eslint/ruff/mypy necesitan. Bajo $HOME se listan SÓLO subdirs del
    // toolchain (cargo/rustup/npm), NUNCA `$HOME` a secas ni dirs de credenciales.
    let read_roots = vec![
        format!("{}/.cargo", home_s),
        format!("{}/.rustup", home_s),
        format!("{}/.npm", home_s),
        "/usr".to_string(),
        "/bin".to_string(),
        "/sbin".to_string(),
        "/System".to_string(),
        // Toolchain de Apple (Xcode / Command Line Tools). NO `/Library` a secas (puede tener
        // datos sensibles bajo `/Library/Keychains`, etc.).
        "/Library/Developer".to_string(),
        "/opt/homebrew".to_string(),
        "/usr/local".to_string(),
        "/private/etc".to_string(),
        "/Applications/Xcode.app".to_string(),
    ];

    let mut p = String::new();
    p.push_str("(version 1)\n");
    p.push_str("(deny default)\n");
    // Procesos hijos del toolchain (clippy → rustc, eslint → node).
    p.push_str("(allow process-fork)\n");
    p.push_str("(allow process-exec)\n");
    p.push_str("(allow signal (target same-sandbox))\n");
    p.push_str("(allow sysctl-read)\n");
    // FOLLOW-UP (audit 024, NO bloqueante): `mach-lookup` genérico amplía la superficie IPC.
    // Acotarlo con `(global-name ...)` a los servicios que cargo/rustc/node realmente piden
    // (p.ej. `com.apple.system.notification_center`, `com.apple.CoreServices.coreservicesd`,
    // dyld/`com.apple.dyld`) requiere enumerar el set exacto por toolchain — frágil y puede
    // romper builds. Se deja genérico por ahora; el deny-default + sin-red + FS-confine ya
    // contiene la exfiltración. Ticket de hardening: acotar global-name cuando se tenga el set.
    p.push_str("(allow mach-lookup)\n");
    p.push_str("(allow file-read-metadata)\n");
    // SIN red: NO se agrega (allow network*). deny-default la bloquea.
    // Lectura del toolchain (mínima).
    for r in &read_roots {
        p.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", sb_escape(r)));
    }
    // Lectura+escritura SÓLO bajo el worktree (FS-confine). El subdir `.qg-tmp` queda dentro,
    // así que los temporales del toolchain (con TMPDIR apuntando ahí) caen bajo este allow.
    // NO se permite `/private/tmp` ni `/private/var/folders`: nada se escribe fuera del worktree.
    p.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", wt));
    p.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", wt));
    p
}

/// Normaliza un path para el perfil Seatbelt: absoluto, con prefijo `/private` si macOS lo
/// canonicaliza (los perfiles Seatbelt comparan rutas reales). Devuelve el string escapado.
fn sb_path(p: &Path) -> String {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    sb_escape(&canon.to_string_lossy())
}

/// Escapa comillas/backslashes para un literal de string del perfil Seatbelt.
fn sb_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// ¿Está `sandbox-exec` disponible? Fail-safe: si no, los linters se marcan `unavailable`.
fn sandbox_exec_available() -> bool {
    Path::new("/usr/bin/sandbox-exec").exists()
}

// ── F0.4 — Parsers deterministas por formato máquina ──────────────────────────
//
// Cada parser devuelve `Some((errors, warnings, issues))` si el formato esperado aparece,
// o `None` → el runner lo mapea a `Unparsable` (NUNCA infiere un conteo).

type Counts = (u32, u32, Vec<Issue>);

/// `cargo clippy --message-format=json` / `cargo check --message-format=json`:
/// una línea JSON por mensaje. Contamos los `reason == "compiler-message"` con
/// `message.level` en {error, warning}. Otras líneas (artifacts) se ignoran.
pub fn parse_clippy_json(stdout: &str) -> Option<Counts> {
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut issues = Vec::new();
    let mut saw_any_json = false;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || !line.starts_with('{') {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        saw_any_json = true;
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let msg = match v.get("message") {
            Some(m) => m,
            None => continue,
        };
        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("");
        let (sev, is_err) = match level {
            "error" | "error: internal compiler error" => ("error", true),
            "warning" => ("warning", false),
            _ => continue, // note/help/ICE-aux → no se cuentan como issue.
        };
        if is_err {
            errors += 1;
        } else {
            warnings += 1;
        }
        if issues.len() < ISSUE_CAP {
            let text = msg.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let rule = msg
                .get("code")
                .and_then(|c| c.get("code"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let (file, line) = msg
                .get("spans")
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.iter().find(|s| s.get("is_primary").and_then(|p| p.as_bool()).unwrap_or(false)).or_else(|| arr.first()))
                .map(|s| {
                    (
                        s.get("file_name").and_then(|f| f.as_str()).unwrap_or("").to_string(),
                        s.get("line_start").and_then(|l| l.as_u64()).unwrap_or(0) as u32,
                    )
                })
                .unwrap_or_default();
            issues.push(Issue { file, line, rule, message: text, severity: sev.into() });
        }
    }
    if saw_any_json {
        Some((errors, warnings, issues))
    } else {
        None // no era JSON de cargo → unparsable.
    }
}

/// `eslint -f json`: array de `{filePath, messages:[{severity:1|2, ruleId, message, line}]}`.
/// severity 2 = error, 1 = warning.
pub fn parse_eslint_json(stdout: &str) -> Option<Counts> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let files = v.as_array()?;
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut issues = Vec::new();
    for f in files {
        let path = f.get("filePath").and_then(|p| p.as_str()).unwrap_or("").to_string();
        let msgs = match f.get("messages").and_then(|m| m.as_array()) {
            Some(m) => m,
            None => continue,
        };
        for m in msgs {
            let sev = m.get("severity").and_then(|s| s.as_u64()).unwrap_or(0);
            let (severity, is_err) = match sev {
                2 => ("error", true),
                1 => ("warning", false),
                _ => continue,
            };
            if is_err {
                errors += 1;
            } else {
                warnings += 1;
            }
            if issues.len() < ISSUE_CAP {
                issues.push(Issue {
                    file: path.clone(),
                    line: m.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32,
                    rule: m.get("ruleId").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                    message: m.get("message").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    severity: severity.into(),
                });
            }
        }
    }
    Some((errors, warnings, issues))
}

/// `ruff check --output-format=json`: array de `{code, filename, location:{row}, message}`.
/// Ruff reporta lints (sin severidad explícita estable) → los contamos como warnings
/// (ruff es lint, no typecheck; un error de sintaxis aparece como código `E999`/`syntax-error`
/// → ese sí lo escalamos a error).
pub fn parse_ruff_json(stdout: &str) -> Option<Counts> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let arr = v.as_array()?;
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut issues = Vec::new();
    for d in arr {
        let code = d.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string();
        // E999 / syntax-error de ruff = error duro; el resto = warning (lint advisory).
        let is_err = code == "E999" || code.eq_ignore_ascii_case("syntax-error");
        if is_err {
            errors += 1;
        } else {
            warnings += 1;
        }
        if issues.len() < ISSUE_CAP {
            issues.push(Issue {
                file: d.get("filename").and_then(|f| f.as_str()).unwrap_or("").to_string(),
                line: d
                    .get("location")
                    .and_then(|l| l.get("row"))
                    .and_then(|r| r.as_u64())
                    .unwrap_or(0) as u32,
                rule: code,
                message: d.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                severity: if is_err { "error" } else { "warning" }.into(),
            });
        }
    }
    Some((errors, warnings, issues))
}

/// `tsc --noEmit --pretty false`: líneas `file(line,col): error TSxxxx: message`.
/// tsc sólo emite errores (no warnings) en este modo. Si no aparece ninguna línea con el
/// patrón y la salida no está vacía-limpia, igual devolvemos 0 (tsc limpio = stdout vacío).
pub fn parse_tsc_text(stdout: &str, stderr: &str) -> Option<Counts> {
    let combined = format!("{stdout}\n{stderr}");
    let mut errors = 0u32;
    let mut issues = Vec::new();
    let mut saw_unexpected = false;
    for line in combined.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // Patrón: "path(12,5): error TS2304: ..."
        if let Some((loc, rest)) = line.split_once("): ") {
            if let Some(open) = loc.rfind('(') {
                let file = loc[..open].to_string();
                let nums = &loc[open + 1..];
                let line_no = nums.split(',').next().and_then(|n| n.trim().parse::<u32>().ok());
                if let Some(line_no) = line_no {
                    if let Some(rest2) = rest.strip_prefix("error ") {
                        errors += 1;
                        let (rule, message) = rest2.split_once(": ").unwrap_or(("", rest2));
                        if issues.len() < ISSUE_CAP {
                            issues.push(Issue {
                                file,
                                line: line_no,
                                rule: rule.to_string(),
                                message: message.to_string(),
                                severity: "error".into(),
                            });
                        }
                        continue;
                    }
                }
            }
        }
        // Línea "Found N errors." es esperada; cualquier otra cosa rara → marca para unparsable.
        if !line.starts_with("Found ") && !line.contains("error TS") {
            saw_unexpected = true;
        }
    }
    // Si tsc no produjo NINGÚN error y la salida tiene ruido inesperado (p.ej. crash de tsc),
    // preferimos unparsable a un 0 falso.
    if errors == 0 && saw_unexpected && !combined.trim().is_empty() {
        return None;
    }
    Some((errors, 0, issues))
}

/// `mypy --no-color-output --no-error-summary`: líneas `file:line: error: message  [code]`
/// y `file:line: note: ...`. note no se cuenta.
pub fn parse_mypy_text(stdout: &str, stderr: &str) -> Option<Counts> {
    let combined = format!("{stdout}\n{stderr}");
    let mut errors = 0u32;
    let mut warnings = 0u32;
    let mut issues = Vec::new();
    // Igual que `parse_tsc_text`: si la salida no-vacía NO matchea el formato esperado y no es
    // claramente "sin issues", preferimos `None` (Unparsable) a un `0` inventado — un crash o
    // config-error de mypy escribe a stderr/stdout pero NO produce líneas `file:line: sev:`.
    let mut saw_unexpected = false;
    for line in combined.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // file:line: error: message  [code]
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        let parsed = parts.len() == 4
            && parts[1].trim().parse::<u32>().is_ok()
            && matches!(parts[2].trim(), "error" | "warning" | "note");
        if !parsed {
            // Líneas de resumen/éxito esperadas de mypy; cualquier otra cosa → unparsable.
            let l = line.trim();
            let is_summary = l.starts_with("Found ")
                || l.starts_with("Success: no issues found")
                || l.starts_with("Success:");
            if !is_summary {
                saw_unexpected = true;
            }
            continue;
        }
        let file = parts[0].to_string();
        let line_no = parts[1].trim().parse::<u32>().expect("checked above");
        let sev_raw = parts[2].trim();
        let rest = parts[3].trim();
        let (is_err, sev) = match sev_raw {
            "error" => (true, "error"),
            "warning" => (false, "warning"),
            _ => continue, // note/info → no cuenta (pero es formato esperado).
        };
        if is_err {
            errors += 1;
        } else {
            warnings += 1;
        }
        if issues.len() < ISSUE_CAP {
            let (message, rule) = match rest.rsplit_once("  [") {
                Some((m, code)) => (m.to_string(), code.trim_end_matches(']').to_string()),
                None => (rest.to_string(), String::new()),
            };
            issues.push(Issue { file, line: line_no, rule, message, severity: sev.into() });
        }
    }
    // Sin issues medidas + ruido inesperado (crash/config-error de mypy) → unparsable, no `0`.
    if errors == 0 && warnings == 0 && saw_unexpected && !combined.trim().is_empty() {
        return None;
    }
    Some((errors, warnings, issues))
}

/// Despacha el parser por formato. `None` ⇒ el runner marca `Unparsable`.
fn parse_by_format(fmt: LinterFormat, stdout: &str, stderr: &str) -> Option<Counts> {
    match fmt {
        LinterFormat::ClippyJson => parse_clippy_json(stdout),
        LinterFormat::EslintJson => parse_eslint_json(stdout),
        LinterFormat::RuffJson => parse_ruff_json(stdout),
        LinterFormat::TscText => parse_tsc_text(stdout, stderr),
        LinterFormat::MypyText => parse_mypy_text(stdout, stderr),
    }
}

// ── F0.6 — Cache key (pura; F2 la persiste) ───────────────────────────────────

/// Identidad de una variante para el cache: `(HEAD del worktree + dirtiness + linter)`
/// (council v2 §6). Determinista. F2 la usa como clave del cache SQLite.
pub fn variant_cache_key(head: &str, dirty: bool, tool: &str) -> String {
    format!("{}:{}:{}", head, if dirty { "dirty" } else { "clean" }, tool)
}

// ── F0.3 — Runner acotado (async, timeout + kill_on_drop + sandbox + argv-only) ─

/// PATH controlado para el subproceso del linter. NO el PATH del usuario (puede traer dirs
/// raros / shims que filtren). Cubre Homebrew (cargo/node/eslint/ruff/mypy típicos en Mac),
/// los binarios del sistema y `/usr/local`. cargo/rustc resuelven sus propios subcomandos vía
/// CARGO_HOME/RUSTUP_HOME, no vía PATH.
const LINTER_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Construye el environment MÍNIMO del subproceso del linter (council v2 §3.6, audit 024
/// blocker FINAL: env-inheritance secret exfiltration).
///
/// PURA: dado el `worktree` (su `.qg-tmp` confinado) y el `home`, devuelve EXACTAMENTE las
/// env vars que el subproceso debe ver. El llamador hace `cmd.env_clear()` y luego setea
/// SÓLO estas — así NINGÚN secreto del proceso padre Furx (`AWS_*`, `OPENAI_API_KEY`,
/// `ANTHROPIC_API_KEY`, `GITHUB_TOKEN`, `*_TOKEN`, `*_KEY`, `*_SECRET`, `SSH_AUTH_SOCK`,
/// tokens BYOK, etc.) llega al linter, que puede escribir bajo el worktree y exfiltrarlos.
///
/// Se incluye SÓLO lo necesario para que el toolchain funcione:
/// - `PATH` controlado (NO el del usuario).
/// - `HOME` real: el sandbox file-read sólo permite `~/.cargo`/`~/.rustup`/`~/.npm`, así que
///   apuntar HOME al home real deja al toolchain encontrar esos dirs SIN exponer secretos
///   (el FS-confine del perfil Seatbelt bloquea el resto de `$HOME`).
/// - `CARGO_HOME`/`RUSTUP_HOME`/`NPM_CONFIG_CACHE` → los roots permitidos (idempotente con HOME).
/// - `TMPDIR`/`TMP`/`TEMP` → `<worktree>/.qg-tmp` (confinado, como ya estaba).
/// - `LANG`/`LC_ALL` UTF-8 (algunos linters lo necesitan para no romper con no-ASCII).
pub fn build_linter_env(qg_tmp: &Path, home: &Path) -> Vec<(String, String)> {
    let home_s = home.to_string_lossy().into_owned();
    let tmp_s = qg_tmp.to_string_lossy().into_owned();
    vec![
        ("PATH".into(), LINTER_PATH.into()),
        ("HOME".into(), home_s.clone()),
        ("CARGO_HOME".into(), format!("{home_s}/.cargo")),
        ("RUSTUP_HOME".into(), format!("{home_s}/.rustup")),
        ("NPM_CONFIG_CACHE".into(), format!("{home_s}/.npm")),
        ("TMPDIR".into(), tmp_s.clone()),
        ("TMP".into(), tmp_s.clone()),
        ("TEMP".into(), tmp_s),
        ("LANG".into(), "en_US.UTF-8".into()),
        ("LC_ALL".into(), "en_US.UTF-8".into()),
    ]
}

/// Corre UNA herramienta sobre UN worktree, acotada. Fail-safe en TODOS los caminos.
///
/// SEGURIDAD: envuelve el binario en `sandbox-exec -p <perfil>` (deny-default, sin red,
/// FS-confine al worktree). Si sandbox-exec no está → `unavailable (sandbox)`, NO se corre
/// sin aislamiento (council v2 §3.6 fail-safe).
pub async fn run_one_linter(worktree: &Path, spec: &LinterSpec, timeout: Duration) -> LinterResult {
    let started = std::time::Instant::now();
    let elapsed = |start: std::time::Instant| start.elapsed().as_millis() as u64;

    // Defensa: el binario debe estar en la allow-list (no comando arbitrario).
    if !bin_is_allowed(&spec.bin) {
        return LinterResult::unavailable(&spec.id, "binario fuera de la allow-list", None, elapsed(started));
    }
    if !worktree.is_dir() {
        return LinterResult::unavailable(&spec.id, "worktree inexistente", None, elapsed(started));
    }
    // Fail-safe sandbox: sin sandbox-exec NO se corre (no caer a sin-aislamiento silencioso).
    if !sandbox_exec_available() {
        return LinterResult::unavailable(
            &spec.id,
            "sandbox no disponible (sandbox-exec ausente) — no se corre sin aislamiento",
            None,
            elapsed(started),
        );
    }
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return LinterResult::unavailable(&spec.id, "no home dir", None, elapsed(started)),
    };
    let profile = build_sandbox_profile(worktree, &home);

    // Subdir de tmp CONFINADO al worktree. Se crea ANTES de correr y se exporta vía TMPDIR/TMP/
    // TEMP para que rustc/cargo/node/eslint escriban su tmp DENTRO del worktree (no en el tmp
    // global, que el perfil ya no permite). Canonicalizado para que matchee el path del perfil.
    let qg_tmp = match std::fs::canonicalize(worktree) {
        Ok(c) => c.join(QG_TMP_SUBDIR),
        Err(_) => worktree.join(QG_TMP_SUBDIR),
    };
    if let Err(e) = std::fs::create_dir_all(&qg_tmp) {
        return LinterResult::unavailable(
            &spec.id,
            format!("no se pudo crear el tmp confinado del worktree: {e}"),
            None,
            elapsed(started),
        );
    }

    // sandbox-exec -p <profile> <bin> <args...>   (argv-only; el inner cmd NO pasa por shell).
    let mut cmd = tokio::process::Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-p").arg(&profile).arg(&spec.bin).args(&spec.args);
    cmd.current_dir(worktree);
    // SEGURIDAD (audit 024 blocker FINAL): NO heredar el environment del proceso Furx. Un
    // build.rs/proc-macro/eslint-plugin/mypy-plugin podría leer secretos del padre (AWS_*,
    // OPENAI_API_KEY, GITHUB_TOKEN, ANTHROPIC_API_KEY, SSH_AUTH_SOCK, tokens BYOK…) y
    // escribirlos bajo el worktree (file-write permitido) → exfiltración. Limpiamos TODO el
    // env y reconstruimos uno MÍNIMO y controlado con `build_linter_env`. TMPDIR/TMP/TEMP
    // siguen confinados al `<worktree>/.qg-tmp`.
    cmd.env_clear();
    for (k, v) in build_linter_env(&qg_tmp, &home) {
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true);
    // No heredar stdin; capturar stdout/stderr.
    cmd.stdin(std::process::Stdio::null());

    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => {
            return LinterResult {
                tool: spec.id.clone(),
                status: LinterStatus::Timeout,
                errors: 0,
                warnings: 0,
                issues: vec![],
                reason: Some(format!("timeout (>{}s)", timeout.as_secs())),
                raw_excerpt: None,
                elapsed_ms: elapsed(started),
            };
        }
        Ok(Err(e)) => {
            // spawn falló (binario no encontrado, etc.) → unavailable (NUNCA 0).
            return LinterResult::unavailable(
                &spec.id,
                format!("no se pudo ejecutar: {e}"),
                None,
                elapsed(started),
            );
        }
        Ok(Ok(o)) => o,
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Detección de "sandbox/permiso denegado por el perfil": stderr de sandbox-exec.
    if !out.status.success()
        && (stderr.contains("sandbox-exec: ") || stderr.contains("Operation not permitted"))
        && stdout.trim().is_empty()
    {
        return LinterResult::unavailable(
            &spec.id,
            "el toolchain falló bajo el sandbox",
            Some(stderr.to_string()),
            elapsed(started),
        );
    }

    match parse_by_format(spec.format, &stdout, &stderr) {
        Some((errors, warnings, issues)) => LinterResult {
            tool: spec.id.clone(),
            status: LinterStatus::Ok,
            errors,
            warnings,
            issues,
            reason: None,
            raw_excerpt: None,
            elapsed_ms: elapsed(started),
        },
        None => LinterResult {
            tool: spec.id.clone(),
            status: LinterStatus::Unparsable,
            errors: 0,
            warnings: 0,
            issues: vec![],
            reason: Some("salida no interpretable".into()),
            raw_excerpt: Some(cap_excerpt(&format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}"))),
            elapsed_ms: elapsed(started),
        },
    }
}

/// Corre TODOS los linters aplicables sobre UN worktree (SYNC por variante, council v2 §2) y
/// agrega a `VariantEvidence`. Fail-safe: una herramienta caída no tumba al conjunto.
pub async fn run_linters_for_variant(
    task_id: &str,
    worktree: &Path,
    specs: &[LinterSpec],
    timeout: Duration,
) -> VariantEvidence {
    let mut by_tool = Vec::with_capacity(specs.len());
    for spec in specs {
        by_tool.push(run_one_linter(worktree, spec, timeout).await);
    }
    aggregate(task_id, by_tool)
}

/// Agregación pura de `LinterResult[]` → `VariantEvidence` (totales sólo de los `ok`).
pub fn aggregate(task_id: &str, by_tool: Vec<LinterResult>) -> VariantEvidence {
    let mut total_errors = 0u32;
    let mut total_warnings = 0u32;
    let mut unavailable_tools = Vec::new();
    let mut any_measured = false;
    for r in &by_tool {
        match r.status {
            LinterStatus::Ok => {
                any_measured = true;
                total_errors += r.errors;
                total_warnings += r.warnings;
            }
            _ => unavailable_tools.push(r.tool.clone()),
        }
    }
    VariantEvidence {
        task_id: task_id.to_string(),
        total_errors,
        total_warnings,
        by_tool,
        unavailable_tools,
        any_measured,
    }
}

/// Root de los worktrees gestionados por Furx: `~/.furx/worktrees`. Canonicalizado si existe.
/// `None` si no hay home dir.
pub fn managed_worktrees_root() -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".furx").join("worktrees");
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

/// Fail-closed (audit 024 MED): valida que `worktree` sea un worktree git válido BAJO el root
/// gestionado por Furx (`~/.furx/worktrees`). Devuelve `Ok(canonical_path)` o `Err(motivo)`.
/// Defiende contra un `worktree_path` stale/manipulado en la DB que apunte a un path arbitrario.
pub fn validate_managed_worktree(worktree: &Path) -> Result<PathBuf, String> {
    let root = managed_worktrees_root().ok_or_else(|| "no home dir".to_string())?;
    // Canonicalizar resuelve symlinks y `..` → evita escapes tipo `~/.furx/worktrees/../../etc`.
    let canon = std::fs::canonicalize(worktree)
        .map_err(|e| format!("worktree inaccesible: {e}"))?;
    if !canon.starts_with(&root) {
        return Err(format!(
            "worktree fuera del root gestionado ({}): {}",
            root.display(),
            canon.display()
        ));
    }
    if !canon.is_dir() {
        return Err("el worktree no es un directorio".to_string());
    }
    // Un worktree git tiene `.git` (archivo apuntando al gitdir, o dir). Si falta → no es git.
    if !canon.join(".git").exists() {
        return Err("no es un worktree git (falta .git)".to_string());
    }
    Ok(canon)
}

/// Evidencia "rechazada": el worktree no pasó la validación fail-closed. NO se corrió ningún
/// linter (no se ejecutan toolchains sobre un path arbitrario). `any_measured=false` y se
/// expone el motivo como herramienta `worktree` unavailable (transparencia para la UI).
pub fn rejected_variant(task_id: &str, reason: impl Into<String>) -> VariantEvidence {
    let r = LinterResult::unavailable("worktree", reason, None, 0);
    aggregate(task_id, vec![r])
}

/// ids resolubles por el motor (DEFAULT_LINTERS + el opt-in cargo_check).
fn is_known_linter_id(id: &str) -> bool {
    DEFAULT_LINTERS.contains(&id) || id == "cargo_check"
}

fn filter_known(ids: impl Iterator<Item = String>) -> Vec<String> {
    let filtered: Vec<String> = ids.filter(|s| is_known_linter_id(s)).collect();
    if filtered.is_empty() {
        DEFAULT_LINTERS.iter().map(|s| s.to_string()).collect()
    } else {
        filtered
    }
}

/// Resuelve la lista de linters habilitados desde el setting `qualitygate.linters` con fallback
/// a `DEFAULT_LINTERS`. Acepta tanto un **array JSON** de ids como un **string separado por comas**
/// (el setting se persiste como string CSV). Cualquier id desconocido se descarta (no se permite
/// inyectar binarios arbitrarios). PURO (recibe el value ya leído).
pub fn enabled_linters_from_setting(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            filter_known(arr.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()))
        }
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => filter_known(
            s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()),
        ),
        _ => DEFAULT_LINTERS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Path helper expuesto para tests del perfil sandbox (no usado fuera).
#[allow(dead_code)]
pub(crate) fn _sb_path_for_test(p: &Path) -> PathBuf {
    PathBuf::from(sb_path(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_repo(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join("furx-qg-tests").join(name);
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn defaults() -> Vec<String> {
        DEFAULT_LINTERS.iter().map(|s| s.to_string()).collect()
    }

    // ── Detección ──────────────────────────────────────────────────────────────

    #[test]
    fn detect_rust_only_clippy_by_default_not_cargo_check() {
        let d = tmp_repo("rust");
        fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let specs = detect_linters(&d, &defaults());
        let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"clippy"), "clippy detectado");
        assert!(!ids.contains(&"cargo_check"), "cargo_check NO es default (§5)");
        // No mete linters de otros ecosistemas ausentes.
        assert!(!ids.contains(&"eslint"));
        assert!(!ids.contains(&"ruff"));
        // clippy es argv-only con formato máquina.
        let clippy = specs.iter().find(|s| s.id == "clippy").unwrap();
        assert_eq!(clippy.bin, "cargo");
        assert!(clippy.args.iter().any(|a| a == "--message-format=json"));
    }

    #[test]
    fn detect_cargo_check_is_opt_in() {
        let d = tmp_repo("rust-check");
        fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let enabled = vec!["clippy".into(), "cargo_check".into()];
        let ids: Vec<String> = detect_linters(&d, &enabled).into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"cargo_check".to_string()), "opt-in vía qualitygate.linters");
    }

    #[test]
    fn detect_js_eslint_and_tsc() {
        let d = tmp_repo("js");
        fs::write(d.join("package.json"), "{\"name\":\"x\"}").unwrap();
        fs::write(d.join(".eslintrc.json"), "{}").unwrap();
        fs::write(d.join("tsconfig.json"), "{}").unwrap();
        let ids: Vec<String> = detect_linters(&d, &defaults()).into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"eslint".to_string()));
        assert!(ids.contains(&"tsc".to_string()));
    }

    #[test]
    fn detect_js_no_eslint_config_skips_eslint() {
        let d = tmp_repo("js-noeslint");
        fs::write(d.join("package.json"), "{\"name\":\"x\"}").unwrap();
        let ids: Vec<String> = detect_linters(&d, &defaults()).into_iter().map(|s| s.id).collect();
        assert!(!ids.contains(&"eslint".to_string()), "sin config eslint → no se detecta");
    }

    #[test]
    fn detect_python_ruff_and_mypy() {
        let d = tmp_repo("py");
        fs::write(d.join("pyproject.toml"), "[tool.ruff]\n").unwrap();
        let ids: Vec<String> = detect_linters(&d, &defaults()).into_iter().map(|s| s.id).collect();
        assert!(ids.contains(&"ruff".to_string()));
        assert!(ids.contains(&"mypy".to_string()));
    }

    #[test]
    fn detect_empty_repo_is_empty() {
        let d = tmp_repo("empty");
        assert!(detect_linters(&d, &defaults()).is_empty());
    }

    #[test]
    fn enabled_linters_setting_fallback_and_filter() {
        // None → defaults.
        assert_eq!(enabled_linters_from_setting(None), defaults());
        // Array JSON: filtra ids desconocidos (no permite inyectar binarios arbitrarios).
        let v = serde_json::json!(["clippy", "evil-linter", "ruff"]);
        assert_eq!(
            enabled_linters_from_setting(Some(&v)),
            vec!["clippy".to_string(), "ruff".to_string()]
        );
        // String CSV (forma persistida del setting): mismo filtrado.
        let s = serde_json::json!("clippy, cargo_check , rm -rf /, mypy");
        assert_eq!(
            enabled_linters_from_setting(Some(&s)),
            vec!["clippy".to_string(), "cargo_check".to_string(), "mypy".to_string()]
        );
        // String vacío / sólo basura → defaults (nunca lista vacía → nunca "0 linters" silencioso).
        let empty = serde_json::json!("   ");
        assert_eq!(enabled_linters_from_setting(Some(&empty)), defaults());
        let garbage = serde_json::json!("foo,bar");
        assert_eq!(enabled_linters_from_setting(Some(&garbage)), defaults());
    }

    // ── Fail-safe (el footgun central: "no disponible" ≠ 0) ──────────────────────

    #[tokio::test]
    async fn missing_linter_is_unavailable_not_zero() {
        // bin allowed (cargo) pero corremos sobre un worktree real; el punto del test es que
        // un id resoluble a un binario AUSENTE nunca produzca un 0 falso. Forzamos un bin de la
        // allow-list que no existe usando 'npx' contra un worktree vacío con un spec inválido.
        let d = tmp_repo("missing");
        // spec con bin allowed pero subcomando que falla a spawnear binario inexistente.
        let spec = LinterSpec {
            id: "ghost".into(),
            bin: "eslint".into(), // allowed, pero típicamente ausente en el entorno de test.
            args: vec!["-f".into(), "json".into(), ".".into()],
            manifest: "package.json".into(),
            format: LinterFormat::EslintJson,
        };
        let r = run_one_linter(&d, &spec, Duration::from_secs(10)).await;
        // En el entorno de test eslint casi seguro no está → unavailable; si por casualidad
        // estuviera, el contrato igual exige status != Ok con un 0 sólo si midió. Lo que NUNCA
        // debe pasar: status Ok con un 0 inventado sin haber corrido. Aceptamos cualquier
        // status != Ok (unavailable/unparsable/timeout) — todos respetan "no 0 falso".
        assert_ne!(
            r.status,
            LinterStatus::Ok,
            "un linter ausente/no medible NUNCA debe reportar Ok (0 falso). status={:?}",
            r.status
        );
        // Y aunque errors==0, el status comunica que NO se midió.
        assert!(matches!(
            r.status,
            LinterStatus::Unavailable | LinterStatus::Unparsable | LinterStatus::Timeout
        ));
    }

    #[tokio::test]
    async fn bin_not_in_allowlist_is_unavailable() {
        let d = tmp_repo("denybin");
        let spec = LinterSpec {
            id: "evil".into(),
            bin: "rm".into(), // NO está en la allow-list.
            args: vec!["-rf".into(), "/".into()],
            manifest: "x".into(),
            format: LinterFormat::EslintJson,
        };
        let r = run_one_linter(&d, &spec, Duration::from_secs(5)).await;
        assert_eq!(r.status, LinterStatus::Unavailable);
        assert!(r.reason.as_deref().unwrap_or("").contains("allow-list"));
        assert_eq!(r.errors, 0);
        assert_eq!(r.warnings, 0);
    }

    #[tokio::test]
    async fn nonexistent_worktree_is_unavailable() {
        let spec = LinterSpec {
            id: "clippy".into(),
            bin: "cargo".into(),
            args: vec!["clippy".into()],
            manifest: "Cargo.toml".into(),
            format: LinterFormat::ClippyJson,
        };
        let r = run_one_linter(Path::new("/nope/does/not/exist/xyz"), &spec, Duration::from_secs(5)).await;
        assert_eq!(r.status, LinterStatus::Unavailable);
    }

    // ── Agregación ──────────────────────────────────────────────────────────────

    #[test]
    fn aggregate_only_counts_ok_and_lists_unavailable() {
        let by_tool = vec![
            LinterResult {
                tool: "clippy".into(),
                status: LinterStatus::Ok,
                errors: 2,
                warnings: 3,
                issues: vec![],
                reason: None,
                raw_excerpt: None,
                elapsed_ms: 1,
            },
            LinterResult::unavailable("eslint", "no instalado", None, 1),
        ];
        let ev = aggregate("t1", by_tool);
        assert_eq!(ev.total_errors, 2);
        assert_eq!(ev.total_warnings, 3);
        assert_eq!(ev.unavailable_tools, vec!["eslint".to_string()]);
        assert!(ev.any_measured);
    }

    #[test]
    fn aggregate_all_unavailable_any_measured_false() {
        let by_tool = vec![
            LinterResult::unavailable("clippy", "x", None, 1),
            LinterResult::unavailable("ruff", "y", None, 1),
        ];
        let ev = aggregate("t1", by_tool);
        assert_eq!(ev.total_errors, 0);
        assert!(!ev.any_measured, "nada medido → any_measured=false (la UI NO muestra '0 limpio')");
        assert_eq!(ev.unavailable_tools.len(), 2);
    }

    // ── Parsers ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_clippy_counts_errors_and_warnings() {
        let stdout = r#"{"reason":"compiler-artifact","x":1}
{"reason":"compiler-message","message":{"level":"warning","message":"unused var","code":{"code":"unused_variables"},"spans":[{"is_primary":true,"file_name":"src/a.rs","line_start":4}]}}
{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":null,"spans":[{"is_primary":true,"file_name":"src/b.rs","line_start":9}]}}
{"reason":"compiler-message","message":{"level":"note","message":"note here","spans":[]}}"#;
        let (e, w, issues) = parse_clippy_json(stdout).unwrap();
        assert_eq!(e, 1);
        assert_eq!(w, 1);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].file, "src/a.rs");
        assert_eq!(issues[0].line, 4);
        assert_eq!(issues[0].severity, "warning");
    }

    #[test]
    fn parse_clippy_non_json_is_none() {
        assert!(parse_clippy_json("error: could not compile\nblah\n").is_none());
        assert!(parse_clippy_json("").is_none());
    }

    #[test]
    fn parse_eslint_severity_mapping() {
        let stdout = r#"[{"filePath":"/x/a.ts","messages":[{"severity":2,"ruleId":"no-undef","message":"x is not defined","line":3},{"severity":1,"ruleId":"no-unused-vars","message":"y unused","line":7}]}]"#;
        let (e, w, issues) = parse_eslint_json(stdout).unwrap();
        assert_eq!(e, 1);
        assert_eq!(w, 1);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].rule, "no-undef");
    }

    #[test]
    fn parse_eslint_clean_is_zero_not_none() {
        // eslint sobre repo limpio devuelve [] o files con messages vacíos → 0 (medido, ok).
        let (e, w, _) = parse_eslint_json("[]").unwrap();
        assert_eq!(e, 0);
        assert_eq!(w, 0);
    }

    #[test]
    fn parse_eslint_garbage_is_none() {
        assert!(parse_eslint_json("not json at all").is_none());
    }

    #[test]
    fn parse_ruff_lint_is_warning_syntax_is_error() {
        let stdout = r#"[{"code":"F401","filename":"a.py","location":{"row":2},"message":"imported but unused"},{"code":"E999","filename":"b.py","location":{"row":1},"message":"SyntaxError"}]"#;
        let (e, w, _) = parse_ruff_json(stdout).unwrap();
        assert_eq!(e, 1);
        assert_eq!(w, 1);
    }

    #[test]
    fn parse_ruff_clean_is_zero() {
        let (e, w, _) = parse_ruff_json("[]").unwrap();
        assert_eq!(e, 0);
        assert_eq!(w, 0);
    }

    #[test]
    fn parse_tsc_errors() {
        let stdout = "src/x.ts(12,5): error TS2304: Cannot find name 'foo'.\nFound 1 error.\n";
        let (e, w, issues) = parse_tsc_text(stdout, "").unwrap();
        assert_eq!(e, 1);
        assert_eq!(w, 0);
        assert_eq!(issues[0].file, "src/x.ts");
        assert_eq!(issues[0].line, 12);
        assert_eq!(issues[0].rule, "TS2304");
    }

    #[test]
    fn parse_tsc_clean_is_zero() {
        let (e, _, _) = parse_tsc_text("", "").unwrap();
        assert_eq!(e, 0);
    }

    #[test]
    fn parse_mypy_errors_and_codes() {
        let stdout = "a.py:10: error: Incompatible return value type  [return-value]\nb.py:3: note: see here\n";
        let (e, w, issues) = parse_mypy_text(stdout, "").unwrap();
        assert_eq!(e, 1);
        assert_eq!(w, 0);
        assert_eq!(issues[0].rule, "return-value");
        assert_eq!(issues[0].line, 10);
    }

    #[test]
    fn parse_mypy_crash_is_unparsable_not_zero() {
        // audit 024 HIGH 2: una salida no-vacía y NO parseable (crash/config-error de mypy) NO
        // debe devolver (0,0,[]) — eso inventaría "0 issues". Debe ser None → Unparsable.
        let crash = "mypy: error: Cannot find config file 'mypy.ini'\nTraceback (most recent call last):\n  File \"mypy/main.py\", line 1, in <module>\nValueError: boom\n";
        assert!(
            parse_mypy_text("", crash).is_none(),
            "crash de mypy → Unparsable, NUNCA 0 inventado"
        );
        // Un crash en stdout también.
        assert!(parse_mypy_text("internal error: something exploded\n", "").is_none());
    }

    #[test]
    fn parse_mypy_clean_success_is_zero_not_none() {
        // El éxito explícito de mypy NO es unparsable: es 0 medido.
        let (e, w, _) = parse_mypy_text("Success: no issues found in 3 source files\n", "").unwrap();
        assert_eq!((e, w), (0, 0));
        // Y la salida totalmente vacía (clean con --no-error-summary) también es 0 medido.
        let (e2, w2, _) = parse_mypy_text("", "").unwrap();
        assert_eq!((e2, w2), (0, 0));
        // "Found N errors" como resumen junto a una línea real NO marca unparsable.
        let mixed = "a.py:1: error: boom  [misc]\nFound 1 error in 1 file (checked 1 source file)\n";
        let (e3, _, _) = parse_mypy_text(mixed, "").unwrap();
        assert_eq!(e3, 1);
    }

    // ── Worktree fail-closed (audit 024 MED) ─────────────────────────────────────

    #[test]
    fn validate_rejects_path_outside_managed_root() {
        // Un path REAL fuera de ~/.furx/worktrees debe rechazarse (no se corre toolchain ahí).
        let outside = tmp_repo("not-managed");
        let res = validate_managed_worktree(&outside);
        assert!(res.is_err(), "path fuera del root gestionado debe rechazarse");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("fuera del root gestionado") || msg.contains("no home dir"),
            "motivo claro: {msg}"
        );
    }

    #[test]
    fn rejected_variant_measures_nothing() {
        let ev = rejected_variant("t1", "worktree rechazado: prueba");
        assert!(!ev.any_measured, "una variante rechazada no mide nada");
        assert_eq!(ev.total_errors, 0);
        assert_eq!(ev.total_warnings, 0);
        assert!(ev.unavailable_tools.contains(&"worktree".to_string()));
    }

    // ── Sandbox profile ─────────────────────────────────────────────────────────

    #[test]
    fn sandbox_profile_denies_default_and_network_and_confines() {
        let d = tmp_repo("sb");
        let home = std::env::temp_dir().join("fake-home");
        let p = build_sandbox_profile(&d, &home);
        assert!(p.contains("(deny default)"), "deny-default presente");
        assert!(!p.contains("(allow network"), "NO debe permitir red");
        // write SÓLO bajo el worktree (canonicalizado).
        let wt = std::fs::canonicalize(&d).unwrap();
        let wts = wt.to_string_lossy();
        assert!(
            p.contains(&format!("(allow file-write* (subpath \"{}\"))", wts)),
            "write confinado al worktree"
        );
        // lectura del toolchain.
        assert!(p.contains(".cargo"));
        assert!(p.contains("/opt/homebrew"));
    }

    #[test]
    fn sandbox_profile_no_global_tmp_writes() {
        // audit 024 HIGH 1: el perfil NO permite escribir en el tmp global — sólo el worktree
        // (que incluye su `.qg-tmp`). Un build.rs/proc-macro malicioso no puede escribir fuera.
        let d = tmp_repo("sb-notmp");
        let home = std::env::temp_dir().join("fake-home");
        let p = build_sandbox_profile(&d, &home);
        // El único write permitido es bajo el worktree; el `.qg-tmp` queda contenido en él.
        let wt = std::fs::canonicalize(&d).unwrap();
        let wts = wt.to_string_lossy();
        assert!(
            p.contains(&format!("(allow file-write* (subpath \"{}\"))", wts)),
            "write confinado al worktree (que contiene .qg-tmp)"
        );
        // 058 (ultrareview audit fix) — robusto al entorno Y a un perfil futuro más amplio. Antes:
        // lista de roots HARDCODEADA (se le escapaba el intermedio del TMPDIR por-usuario, ej.
        // /private/var/folders/zr/xxx). Ahora DERIVAMOS los ancestros REALES: cada ancestro del worktree
        // canónico Y del TMPDIR del sistema (+ /tmp y /private/tmp clásicos). NINGUNO puede tener un
        // write-allow pelado — cubriría el tmp global. El worktree EXACTO (subpath más profundo) sí está
        // permitido y se excluye del chequeo.
        let mut forbidden: Vec<std::path::PathBuf> =
            wt.ancestors().skip(1).map(|a| a.to_path_buf()).collect();
        if let Ok(td) = std::fs::canonicalize(std::env::temp_dir()) {
            forbidden.extend(td.ancestors().map(|a| a.to_path_buf()));
        }
        forbidden.push(std::path::PathBuf::from("/private/tmp"));
        forbidden.push(std::path::PathBuf::from("/tmp"));
        for anc in forbidden {
            if anc == wt {
                continue; // el worktree exacto SÍ está permitido
            }
            assert!(
                !p.contains(&format!("(allow file-write* (subpath \"{}\"))", anc.to_string_lossy())),
                "NO debe permitir write al ancestro/global '{}' (cubriría el tmp global; el worktree es un subpath más profundo, OK)",
                anc.to_string_lossy()
            );
        }
        // El subdir confinado debe caer DENTRO del subpath del worktree.
        let tmp_under_wt = wt.join(QG_TMP_SUBDIR);
        assert!(
            tmp_under_wt.starts_with(&wt),
            "el .qg-tmp queda bajo el worktree → cubierto por el allow del worktree"
        );
    }

    #[test]
    fn sandbox_profile_does_not_read_home_secrets() {
        // audit 024 HIGH/MED 3: el file-read* es mínimo — NO expone $HOME a secas ni dirs de
        // credenciales (~/.ssh, ~/.aws, ~/.furx con BYOK), ni /Library a secas (Keychains).
        let home = Path::new("/Users/x");
        let d = tmp_repo("sb-secrets");
        let p = build_sandbox_profile(&d, home);
        assert!(
            !p.contains("(allow file-read* (subpath \"/Users/x\"))"),
            "NO debe permitir leer $HOME a secas"
        );
        assert!(!p.contains("/Users/x/.ssh"), "NO ~/.ssh");
        assert!(!p.contains("/Users/x/.aws"), "NO ~/.aws");
        assert!(!p.contains("/Users/x/.furx"), "NO ~/.furx (BYOK)");
        assert!(!p.contains("/Users/x/.config"), "NO ~/.config");
        // /Library a secas no debe estar (sólo /Library/Developer para el toolchain).
        assert!(
            !p.contains("(allow file-read* (subpath \"/Library\"))"),
            "NO /Library a secas (Keychains)"
        );
        assert!(p.contains("/Library/Developer"), "SÍ /Library/Developer (toolchain)");
        // Roots del toolchain bajo home: SÓLO cargo/rustup/npm.
        assert!(p.contains("/Users/x/.cargo"));
        assert!(p.contains("/Users/x/.rustup"));
    }

    #[test]
    fn sandbox_profile_escapes_paths() {
        let weird = Path::new("/tmp/a\"b");
        let home = Path::new("/Users/x");
        let p = build_sandbox_profile(weird, home);
        // No debe romper el formato: las comillas internas quedan escapadas.
        assert!(!p.contains("\"/tmp/a\"b\""), "comilla sin escapar rompería el perfil");
    }

    // ── Env del subproceso (audit 024 blocker FINAL: secret exfiltration) ─────────

    #[test]
    fn linter_env_is_minimal_and_excludes_secrets() {
        // audit 024 blocker FINAL: el subproceso del linter NO debe heredar el env del padre.
        // `build_linter_env` devuelve EXACTAMENTE las vars que se setean tras `env_clear()`.
        let qg_tmp = Path::new("/Users/x/wt/.qg-tmp");
        let home = Path::new("/Users/x");
        let env = build_linter_env(qg_tmp, home);
        let keys: std::collections::HashSet<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        let get = |k: &str| env.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());

        // (b) SÍ contiene lo mínimo del toolchain.
        assert_eq!(get("PATH"), Some(LINTER_PATH), "PATH controlado (NO el del usuario)");
        // PATH controlado: no arrastra dirs raros del usuario.
        assert!(
            !LINTER_PATH.contains(".cargo/bin") && !LINTER_PATH.contains("node_modules"),
            "PATH acotado a dirs del sistema/toolchain"
        );
        assert_eq!(get("HOME"), Some("/Users/x"));
        assert_eq!(get("CARGO_HOME"), Some("/Users/x/.cargo"));
        assert_eq!(get("RUSTUP_HOME"), Some("/Users/x/.rustup"));
        // TMPDIR/TMP/TEMP confinados al .qg-tmp del worktree.
        for k in ["TMPDIR", "TMP", "TEMP"] {
            assert_eq!(get(k), Some("/Users/x/wt/.qg-tmp"), "{k} confinado al worktree");
        }
        assert_eq!(get("LANG"), Some("en_US.UTF-8"));
        assert_eq!(get("LC_ALL"), Some("en_US.UTF-8"));

        // (a) NO contiene NINGÚN secreto típico del proceso padre Furx.
        for secret in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "GITHUB_TOKEN",
            "SSH_AUTH_SOCK",
        ] {
            assert!(!keys.contains(secret), "el env del linter NO debe traer {secret}");
        }
        // Tampoco patrones de credenciales: nada que termine en _TOKEN/_KEY/_SECRET salvo los
        // benignos del toolchain explícitamente whitelisteados (CARGO_HOME/RUSTUP_HOME no aplican).
        for (k, _) in &env {
            let bad = (k.ends_with("_TOKEN") || k.ends_with("_SECRET"))
                || (k.ends_with("_KEY") && k != "NPM_CONFIG_CACHE");
            assert!(!bad, "var sospechosa de credencial en el env del linter: {k}");
        }
    }

    #[test]
    fn linter_env_does_not_inherit_injected_parent_secret() {
        // Verifica el CONTRATO: aunque el proceso padre tenga un secreto inyectado en SU env,
        // `build_linter_env` (pura, no lee el env del padre) NO lo propaga. El runner real hace
        // `cmd.env_clear()` + setea SÓLO estas → el secreto del padre nunca llega al subproceso.
        std::env::set_var("OPENAI_API_KEY", "sk-INJECTED-SHOULD-NOT-LEAK");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "INJECTED-AWS-SECRET");
        let env = build_linter_env(Path::new("/wt/.qg-tmp"), Path::new("/home/u"));
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("AWS_SECRET_ACCESS_KEY");
        for (k, v) in &env {
            assert_ne!(k, "OPENAI_API_KEY", "no debe incluir el secreto inyectado del padre");
            assert_ne!(k, "AWS_SECRET_ACCESS_KEY");
            assert!(!v.contains("INJECTED"), "ningún valor debe arrastrar el secreto del padre");
        }
    }

    // ── Cache key ───────────────────────────────────────────────────────────────

    #[test]
    fn cache_key_is_deterministic_and_sensitive() {
        assert_eq!(
            variant_cache_key("abc123", false, "clippy"),
            variant_cache_key("abc123", false, "clippy")
        );
        assert_ne!(
            variant_cache_key("abc123", false, "clippy"),
            variant_cache_key("abc123", true, "clippy"),
            "dirtiness cambia la clave"
        );
        assert_ne!(
            variant_cache_key("abc123", false, "clippy"),
            variant_cache_key("def456", false, "clippy"),
            "HEAD cambia la clave"
        );
    }
}
