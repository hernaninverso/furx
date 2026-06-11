// services/variant_features.rs — 026 F0 (US1) · features objetivas por variante.
//
// PURO y testeable (sin red/DB/Tauri): caracteriza cada variante de un best-of-N con un vector de
// features objetivas y DETERMINISTAS a partir del unified diff YA colectado (no re-deriva del
// worktree vivo) + la evidencia del quality-gate de 024 + el agente que la generó.
//
// CONTRATO fail-safe heredado de 024 (FR-012): una feature "no medida" (quality-gate
// unavailable/timeout) se registra como AUSENTE (`measured = false`), NUNCA como `0`. El prior NO
// debe confundir "no se midió" con "0 issues / limpio".
//
// Set de features (clarify §4 + FR-010). `feature_key` estable + `FEATURE_SCHEMA_VERSION` versionado
// (FR-011) para que registros viejos sigan siendo interpretables si el set evoluciona.

use serde::{Deserialize, Serialize};

/// Versión del set de features. Subir cuando se agregue/quite/cambie la semántica de un feature
/// (FR-011). Los registros viejos guardan su versión → interpretables aunque el set evolucione.
pub const FEATURE_SCHEMA_VERSION: i64 = 1;

// ── feature keys estables (no cambiar sin subir FEATURE_SCHEMA_VERSION) ──────────
pub const F_DIFF_ADDED: &str = "diff_added"; // líneas agregadas
pub const F_DIFF_REMOVED: &str = "diff_removed"; // líneas eliminadas
pub const F_DIFF_TOTAL: &str = "diff_total"; // added + removed (tamaño del cambio)
pub const F_FILES_TOUCHED: &str = "files_touched"; // nº de archivos tocados
pub const F_RISKY_PATHS: &str = "risky_paths"; // nº de archivos en rutas sensibles
pub const F_QG_ERRORS: &str = "qg_errors"; // errores del quality-gate (024)
pub const F_QG_WARNINGS: &str = "qg_warnings"; // warnings del quality-gate (024)

/// Lista canónica de risky-paths por defecto (clarify §4). Substring match case-insensitive sobre el
/// path RELATIVO. Override por repo vía setting `preference.risky_paths` (lista separada por comas).
pub const DEFAULT_RISKY_PATHS: &[&str] = &[
    "migrations/",
    "migration/",
    ".env",
    "secret",
    "auth",
    "cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    ".lock",
    "gemfile.lock",
    "go.sum",
    "credentials",
    ".pem",
];

/// Un valor de feature con su estado MEDIDO|AUSENTE. `measured = false` ⇒ no se midió (≠ 0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FeatureValue {
    pub value: f64,
    pub measured: bool,
}

impl FeatureValue {
    pub fn measured(value: f64) -> Self {
        Self {
            value,
            measured: true,
        }
    }
    pub fn absent() -> Self {
        Self {
            value: 0.0,
            measured: false,
        }
    }
}

/// El vector de features de UNA variante. `chosen` lo setea el derivador de la señal (no se computa
/// acá). Cada feature es un `(key, FeatureValue)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantFeatures {
    pub task_id: String,
    pub agent_profile_id: Option<String>,
    /// `(feature_key, value)` — orden estable (insertion order) para reproducibilidad.
    pub features: Vec<(String, FeatureValue)>,
}

impl VariantFeatures {
    /// Busca un feature por key (lineal — pocas features).
    pub fn get(&self, key: &str) -> Option<FeatureValue> {
        self.features
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
    }
}

/// Diff-stat de un unified diff: líneas agregadas/eliminadas + archivos tocados. PURO.
///
/// Cuenta una línea como AGREGADA si empieza con `+` (y no es la cabecera `+++ `), ELIMINADA si
/// empieza con `-` (y no es `--- `). Los archivos tocados se cuentan por cabeceras `diff --git` /
/// `+++ ` distintas (set para deduplicar). Robusto a diffs vacíos (todo 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStat {
    pub added: u32,
    pub removed: u32,
    pub files: u32,
}

pub fn diff_stat(diff: &str) -> DiffStat {
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/path b/path` → tomamos el path nuevo (segundo token), o el primero.
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(t) = toks.last() {
                files.insert(strip_ab(t));
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let p = rest.trim();
            if p != "/dev/null" {
                files.insert(strip_ab(p));
            }
        } else if line.starts_with("+++ ") || line.starts_with("--- ") {
            // cabeceras de archivo: NO son cambios de contenido.
        } else if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    DiffStat {
        added,
        removed,
        files: files.len() as u32,
    }
}

fn strip_ab(p: &str) -> String {
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .trim()
        .to_string()
}

/// Parsea la lista de risky-paths de un setting (lista separada por comas). Vacío ⇒ `None`
/// (usar el default). No vacío ⇒ `Some(lista)` que REEMPLAZA el default (clarify §4).
pub fn parse_risky_paths_setting(raw: &str) -> Option<Vec<String>> {
    let items: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// Cuenta cuántos archivos del diff tocan rutas riesgosas (substring match case-insensitive sobre el
/// path relativo). `patterns = None` ⇒ usar `DEFAULT_RISKY_PATHS`. PURO.
pub fn risky_path_count(diff: &str, patterns: Option<&[String]>) -> u32 {
    let default_owned: Vec<String>;
    let pats: &[String] = match patterns {
        Some(p) => p,
        None => {
            default_owned = DEFAULT_RISKY_PATHS.iter().map(|s| s.to_string()).collect();
            &default_owned
        }
    };
    let mut files: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(t) = rest.split_whitespace().last() {
                files.insert(strip_ab(t).to_lowercase());
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let p = rest.trim();
            if p != "/dev/null" {
                files.insert(strip_ab(p).to_lowercase());
            }
        }
    }
    files
        .iter()
        .filter(|f| pats.iter().any(|p| f.contains(p.as_str())))
        .count() as u32
}

/// Evidencia del quality-gate de 024, adaptada al contrato measured|ausente SIN acoplar a su tipo.
/// El caller pasa los conteos + si hubo alguna medición real. Si `any_measured == false` ⇒ las
/// features qg quedan AUSENTES (no `0`) — contrato fail-safe (FR-012).
#[derive(Debug, Clone, Copy)]
pub struct QualityGateInput {
    pub errors: u32,
    pub warnings: u32,
    pub any_measured: bool,
}

/// Computa el `VariantFeatures` completo de una variante a partir de su diff + quality-gate + agente.
/// `risky_patterns = None` ⇒ default. PURO.
pub fn compute_features(
    task_id: &str,
    agent_profile_id: Option<String>,
    diff: &str,
    quality_gate: Option<QualityGateInput>,
    risky_patterns: Option<&[String]>,
) -> VariantFeatures {
    let stat = diff_stat(diff);
    let risky = risky_path_count(diff, risky_patterns);
    let mut features: Vec<(String, FeatureValue)> = vec![
        (F_DIFF_ADDED.into(), FeatureValue::measured(stat.added as f64)),
        (
            F_DIFF_REMOVED.into(),
            FeatureValue::measured(stat.removed as f64),
        ),
        (
            F_DIFF_TOTAL.into(),
            FeatureValue::measured((stat.added + stat.removed) as f64),
        ),
        (
            F_FILES_TOUCHED.into(),
            FeatureValue::measured(stat.files as f64),
        ),
        (F_RISKY_PATHS.into(), FeatureValue::measured(risky as f64)),
    ];
    // Quality-gate: AUSENTE si no se midió (≠ 0). FR-012.
    match quality_gate {
        Some(qg) if qg.any_measured => {
            features.push((F_QG_ERRORS.into(), FeatureValue::measured(qg.errors as f64)));
            features.push((
                F_QG_WARNINGS.into(),
                FeatureValue::measured(qg.warnings as f64),
            ));
        }
        _ => {
            features.push((F_QG_ERRORS.into(), FeatureValue::absent()));
            features.push((F_QG_WARNINGS.into(), FeatureValue::absent()));
        }
    }
    VariantFeatures {
        task_id: task_id.to_string(),
        agent_profile_id,
        features,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\n\
--- a/src/lib.rs\n\
+++ b/src/lib.rs\n\
@@ -1,3 +1,4 @@\n\
 fn main() {}\n\
+let x = 1;\n\
+let y = 2;\n\
-old_line();\n";

    #[test]
    fn diff_stat_counts_added_removed_files() {
        let s = diff_stat(SAMPLE_DIFF);
        assert_eq!(s.added, 2, "2 líneas agregadas (sin contar +++ header)");
        assert_eq!(s.removed, 1, "1 línea eliminada (sin contar --- header)");
        assert_eq!(s.files, 1);
    }

    #[test]
    fn diff_stat_empty_is_zero() {
        let s = diff_stat("");
        assert_eq!(s, DiffStat::default());
    }

    #[test]
    fn diff_stat_ignores_file_headers() {
        // `+++ b/x` y `--- a/x` no cuentan como added/removed.
        let d = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+content\n";
        let s = diff_stat(d);
        assert_eq!(s.added, 1);
        assert_eq!(s.removed, 0);
    }

    #[test]
    fn risky_path_detected_with_default() {
        let d = "diff --git a/src-tauri/migrations/043_x.sql b/src-tauri/migrations/043_x.sql\n+CREATE TABLE\n";
        assert_eq!(risky_path_count(d, None), 1, "migrations/ es risky por default");
        let safe = "diff --git a/src/util.rs b/src/util.rs\n+fn x(){}\n";
        assert_eq!(risky_path_count(safe, None), 0);
    }

    #[test]
    fn risky_path_override_replaces_default() {
        let d = "diff --git a/config/app.toml b/config/app.toml\n+x=1\n";
        // default NO incluye config/ → 0
        assert_eq!(risky_path_count(d, None), 0);
        // override con "config" → 1
        let pats = parse_risky_paths_setting("config").unwrap();
        assert_eq!(risky_path_count(d, Some(&pats)), 1);
    }

    #[test]
    fn parse_risky_setting_empty_is_none() {
        assert!(parse_risky_paths_setting("").is_none());
        assert!(parse_risky_paths_setting("  ,  ").is_none());
        let p = parse_risky_paths_setting("auth, .env , Secrets").unwrap();
        assert_eq!(p, vec!["auth", ".env", "secrets"]);
    }

    #[test]
    fn quality_gate_unavailable_is_absent_not_zero() {
        // any_measured = false ⇒ qg features AUSENTES (FR-012). NUNCA 0 medido.
        let vf = compute_features(
            "t1",
            None,
            SAMPLE_DIFF,
            Some(QualityGateInput {
                errors: 0,
                warnings: 0,
                any_measured: false,
            }),
            None,
        );
        let e = vf.get(F_QG_ERRORS).unwrap();
        assert!(!e.measured, "qg_errors debe ser AUSENTE cuando no se midió");
        let none_qg = compute_features("t2", None, SAMPLE_DIFF, None, None);
        assert!(
            !none_qg.get(F_QG_ERRORS).unwrap().measured,
            "sin quality-gate ⇒ ausente"
        );
    }

    #[test]
    fn quality_gate_measured_zero_is_distinct_from_absent() {
        let vf = compute_features(
            "t1",
            None,
            SAMPLE_DIFF,
            Some(QualityGateInput {
                errors: 0,
                warnings: 0,
                any_measured: true,
            }),
            None,
        );
        let e = vf.get(F_QG_ERRORS).unwrap();
        assert!(e.measured, "medido");
        assert_eq!(e.value, 0.0, "0 errores MEDIDOS ≠ ausente");
    }

    #[test]
    fn compute_features_carries_agent_and_schema_keys() {
        let vf = compute_features("t9", Some("planner".into()), SAMPLE_DIFF, None, None);
        assert_eq!(vf.task_id, "t9");
        assert_eq!(vf.agent_profile_id.as_deref(), Some("planner"));
        // todas las features esperadas presentes
        for k in [
            F_DIFF_ADDED,
            F_DIFF_REMOVED,
            F_DIFF_TOTAL,
            F_FILES_TOUCHED,
            F_RISKY_PATHS,
            F_QG_ERRORS,
            F_QG_WARNINGS,
        ] {
            assert!(vf.get(k).is_some(), "feature {k} presente");
        }
        assert_eq!(FEATURE_SCHEMA_VERSION, 1);
    }

    #[test]
    fn diff_total_is_added_plus_removed() {
        let vf = compute_features("t", None, SAMPLE_DIFF, None, None);
        let total = vf.get(F_DIFF_TOTAL).unwrap().value;
        let added = vf.get(F_DIFF_ADDED).unwrap().value;
        let removed = vf.get(F_DIFF_REMOVED).unwrap().value;
        assert_eq!(total, added + removed);
    }
}
