// services/preference_signal.rs — 026 F0/F1 · deriva, persiste y combina la señal de preferencia.
//
// US1 (F0): de cada review de best-of-N cerrada/aplicada deriva UN `PreferenceRecord` (variante(s)
// elegida(s)/rechazada(s) + features objetivas + contexto repo/tarea) a partir del estado FINAL de
// la review (`review::load_group`) — REUSA el audit/review existentes, NO los duplica. Persiste
// append-only (migración 043). CERO código crudo de diffs (FR-005/SC-008): `repo_key` = hash
// blake3 (no ruta absoluta), y NUNCA se guarda el texto del diff — sólo features numéricas.
//
// US2 (F1): combina el prior local (`preference_prior`) con el ranking advisory de 020
// (`meta_decision::rank_variants`) en `rank_with_prior` — SIEMPRE advisory (FR-024): re-ordena y
// explica, NUNCA muta estado ni auto-elige. Con `inject = false` ⇒ ranking idéntico a 020.

use crate::services::preference_prior::{
    self, ContextPrior, ExplanationFactor, FeatureBeta, PreferenceObservation,
};
use crate::services::review::{self, HunkState};
use crate::services::variant_features::{self, FeatureValue, QualityGateInput, VariantFeatures};
use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

type Db = std::sync::Arc<parking_lot::Mutex<rusqlite::Connection>>;

/// Tipo de resultado de la review (clarify §7 / FR-004): NUNCA inventa un ganador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// Una sola variante con hunks aprobados.
    Single,
    /// Hunks aprobados de ≥2 variantes (cherry-pick cross-variante).
    Mixed,
    /// Nada aprobado (todo rechazado / grupo matado).
    None,
}

impl OutcomeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutcomeKind::Single => "single",
            OutcomeKind::Mixed => "mixed",
            OutcomeKind::None => "none",
        }
    }
    pub fn from_str(s: &str) -> Option<OutcomeKind> {
        match s {
            "single" => Some(OutcomeKind::Single),
            "mixed" => Some(OutcomeKind::Mixed),
            "none" => Some(OutcomeKind::None),
            _ => None,
        }
    }
    /// Atenuación de la evidencia para el prior (clarify §7): mixed pesa 0.5.
    pub fn obs_weight(&self) -> f64 {
        match self {
            OutcomeKind::Mixed => 0.5,
            _ => 1.0,
        }
    }
}

/// El registro de preferencia (append-only). NO contiene código crudo (FR-005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreferenceRecord {
    pub id: String,
    pub group_id: String,
    /// hash/relativo del repo (no ruta absoluta) — scrubbeado.
    pub repo_key: String,
    pub task_type: String,
    pub outcome_kind: OutcomeKind,
    pub feature_schema_version: i64,
    pub revision: Option<i64>,
    /// Features por variante + flag `chosen`.
    pub variants: Vec<RecordedVariant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedVariant {
    pub features: VariantFeatures,
    pub chosen: bool,
}

/// `repo_key` scrubbeado: hash blake3 del path de repo (no ruta absoluta, no contenido). FR-005.
/// Determinista por path → el mismo repo agrupa siempre al mismo contexto.
pub fn repo_key_for(repo_path: &str) -> String {
    let h = blake3::hash(repo_path.trim().as_bytes());
    // prefijo legible corto + hash (sin filtrar el path).
    format!("repo:{}", &h.to_hex()[..16])
}

/// Insumo para derivar el record: por variante, su `task_id`, `agent_profile_id`, su `diff` (para
/// diff-stat — NO se persiste el texto) y su quality-gate opcional.
#[derive(Debug, Clone)]
pub struct VariantInput {
    pub task_id: String,
    pub agent_profile_id: Option<String>,
    pub diff: String,
    pub quality_gate: Option<QualityGateInput>,
}

/// Deriva el `PreferenceRecord` del ESTADO FINAL de la review (FR-001/FR-004). Una variante está
/// "elegida" si tiene ≥1 hunk APROBADO en el estado final (revert ya reflejado → un hunk revertido a
/// pending NO cuenta). PURO (recibe el `GroupReview` ya cargado + los inputs de variante).
pub fn derive_record(
    group: &review::GroupReview,
    repo_path: &str,
    task_type: &str,
    inputs: &[VariantInput],
    risky_patterns: Option<&[String]>,
) -> PreferenceRecord {
    // qué task_ids tienen ≥1 hunk aprobado en el estado FINAL.
    let chosen_ids: std::collections::BTreeSet<String> = group
        .variants
        .iter()
        .filter(|v| {
            v.change_set
                .hunks
                .iter()
                .any(|h| h.state == HunkState::Approved)
        })
        .map(|v| v.task_id.clone())
        .collect();

    let outcome = match chosen_ids.len() {
        0 => OutcomeKind::None,
        1 => OutcomeKind::Single,
        _ => OutcomeKind::Mixed,
    };

    let mut variants = Vec::new();
    for inp in inputs {
        let features = variant_features::compute_features(
            &inp.task_id,
            inp.agent_profile_id.clone(),
            &inp.diff,
            inp.quality_gate,
            risky_patterns,
        );
        variants.push(RecordedVariant {
            chosen: chosen_ids.contains(&inp.task_id),
            features,
        });
    }

    PreferenceRecord {
        id: uuid::Uuid::new_v4().to_string(),
        group_id: group.group_id.clone(),
        repo_key: repo_key_for(repo_path),
        task_type: task_type.to_string(),
        outcome_kind: outcome,
        feature_schema_version: variant_features::FEATURE_SCHEMA_VERSION,
        revision: Some(group.revision as i64),
        variants,
    }
}

/// Persiste el record (append-only, transaccional) + actualiza el prior del contexto. REUSA el
/// patrón de `review::init_group`. Idempotente por `group_id` NO se asume (cada apply = un record).
pub fn persist_record(db: &Db, rec: &PreferenceRecord) -> Result<()> {
    {
    let mut conn = db.lock();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO preference_records \
         (id, group_id, repo_key, task_type, outcome_kind, feature_schema_version, revision) \
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            rec.id,
            rec.group_id,
            rec.repo_key,
            rec.task_type,
            rec.outcome_kind.as_str(),
            rec.feature_schema_version,
            rec.revision,
        ],
    )?;
    for rv in &rec.variants {
        for (key, fv) in &rv.features.features {
            tx.execute(
                "INSERT INTO variant_features \
                 (record_id, task_id, chosen, agent_profile_id, feature_key, value, measured) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    rec.id,
                    rv.features.task_id,
                    rv.chosen as i64,
                    rv.features.agent_profile_id,
                    key,
                    fv.value,
                    fv.measured as i64,
                ],
            )?;
        }
    }
    tx.commit()?;
    } // DROP del lock ANTES de update_prior_from_record (parking_lot Mutex NO es reentrante — sin
      // este scope, re-lockear adentro de update_prior_from_record/load_prior DEADLOCKEA).
    // Actualizar el prior del contexto (mutable) — separado para que un fallo del prior NO aborte la
    // captura inmutable de la señal.
    update_prior_from_record(db, rec)?;
    Ok(())
}

/// Lista los records (read-only) más recientes — para inspección/exportación (US1). Sin código crudo.
pub fn list_records(db: &Db, limit: i64) -> Result<Vec<PreferenceRecord>> {
    let conn = db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, group_id, repo_key, task_type, outcome_kind, feature_schema_version, revision \
         FROM preference_records ORDER BY created_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<i64>>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, group_id, repo_key, task_type, outcome, schema, revision) = row?;
        // cargar features por variante.
        let mut vstmt = conn.prepare(
            "SELECT task_id, chosen, agent_profile_id, feature_key, value, measured \
             FROM variant_features WHERE record_id = ?1 ORDER BY task_id, feature_key",
        )?;
        let vrows = vstmt.query_map(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut by_variant: Vec<RecordedVariant> = Vec::new();
        for vr in vrows {
            let (task_id, chosen, agent, key, value, measured) = vr?;
            let fv = FeatureValue {
                value,
                measured: measured != 0,
            };
            match by_variant.iter_mut().find(|v| v.features.task_id == task_id) {
                Some(v) => v.features.features.push((key, fv)),
                None => by_variant.push(RecordedVariant {
                    chosen: chosen != 0,
                    features: VariantFeatures {
                        task_id,
                        agent_profile_id: agent,
                        features: vec![(key, fv)],
                    },
                }),
            }
        }
        out.push(PreferenceRecord {
            id,
            group_id,
            repo_key,
            task_type,
            outcome_kind: OutcomeKind::from_str(&outcome).unwrap_or(OutcomeKind::None),
            feature_schema_version: schema,
            revision,
            variants: by_variant,
        });
    }
    Ok(out)
}

// ── Persistencia del prior por contexto ──────────────────────────────────────────

/// Carga el `ContextPrior` de `(repo_key, task_type)` desde `context_priors` + `context_prior_meta`.
pub fn load_prior(db: &Db, repo_key: &str, task_type: &str) -> Result<ContextPrior> {
    let conn = db.lock();
    let sample_count: i64 = conn
        .query_row(
            "SELECT sample_count FROM context_prior_meta WHERE repo_key=?1 AND task_type=?2",
            params![repo_key, task_type],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let mut stmt = conn.prepare(
        "SELECT feature_key, alpha, beta, distinct_obs FROM context_priors \
         WHERE repo_key=?1 AND task_type=?2 ORDER BY feature_key",
    )?;
    let rows = stmt.query_map(params![repo_key, task_type], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, f64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut features = Vec::new();
    for row in rows {
        let (key, alpha, beta, distinct_obs) = row?;
        features.push((
            key,
            FeatureBeta {
                alpha,
                beta,
                distinct_obs,
            },
        ));
    }
    Ok(ContextPrior {
        repo_key: repo_key.to_string(),
        task_type: task_type.to_string(),
        features,
        sample_count,
    })
}

/// Persiste el `ContextPrior` (mutable) — upsert por feature + meta. Transaccional.
pub fn save_prior(db: &Db, prior: &ContextPrior) -> Result<()> {
    let mut conn = db.lock();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO context_prior_meta (repo_key, task_type, sample_count, updated_at) \
         VALUES (?1,?2,?3,datetime('now')) \
         ON CONFLICT(repo_key, task_type) DO UPDATE SET sample_count=excluded.sample_count, updated_at=datetime('now')",
        params![prior.repo_key, prior.task_type, prior.sample_count],
    )?;
    for (key, fb) in &prior.features {
        tx.execute(
            "INSERT INTO context_priors (repo_key, task_type, feature_key, alpha, beta, distinct_obs, updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,datetime('now')) \
             ON CONFLICT(repo_key, task_type, feature_key) DO UPDATE SET \
               alpha=excluded.alpha, beta=excluded.beta, distinct_obs=excluded.distinct_obs, updated_at=datetime('now')",
            params![prior.repo_key, prior.task_type, key, fb.alpha, fb.beta, fb.distinct_obs],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Carga el prior del contexto, lo actualiza con el record, y lo persiste (independencia por
/// contexto: SOLO toca `(repo_key, task_type)` del record — FR-021/SC-006).
pub fn update_prior_from_record(db: &Db, rec: &PreferenceRecord) -> Result<()> {
    let mut prior = load_prior(db, &rec.repo_key, &rec.task_type)?;
    let obs = PreferenceObservation {
        variants: rec.variants.iter().map(|v| v.features.clone()).collect(),
        chosen: rec.variants.iter().map(|v| v.chosen).collect(),
        weight: rec.outcome_kind.obs_weight(),
    };
    preference_prior::update_from_record(&mut prior, &obs);
    save_prior(db, &prior)?;
    Ok(())
}

/// Resetea el prior de un contexto (o TODOS si `repo_key`/`task_type` son None) a cold-start.
/// Devuelve cuántas filas de prior se borraron. NO toca la señal append-only (los records quedan).
pub fn reset_prior(db: &Db, repo_key: Option<&str>, task_type: Option<&str>) -> Result<usize> {
    let conn = db.lock();
    let n = match (repo_key, task_type) {
        (Some(r), Some(t)) => {
            let n = conn.execute(
                "DELETE FROM context_priors WHERE repo_key=?1 AND task_type=?2",
                params![r, t],
            )?;
            conn.execute(
                "DELETE FROM context_prior_meta WHERE repo_key=?1 AND task_type=?2",
                params![r, t],
            )?;
            n
        }
        (Some(r), None) => {
            let n = conn.execute("DELETE FROM context_priors WHERE repo_key=?1", params![r])?;
            conn.execute("DELETE FROM context_prior_meta WHERE repo_key=?1", params![r])?;
            n
        }
        _ => {
            let n = conn.execute("DELETE FROM context_priors", [])?;
            conn.execute("DELETE FROM context_prior_meta", [])?;
            n
        }
    };
    Ok(n)
}

// ── F1: combinación prior ↔ ranking advisory de 020 ──────────────────────────────

/// La explicación legible de UNA variante en el ranking enriquecido (FR-023). NUNCA opaco.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantExplanation {
    pub task_id: String,
    /// Score combinado final usado para ordenar.
    pub combined_score: f64,
    /// Score del ranking actual (AIE/heurística de 020), normalizado [0,1].
    pub base_score: f64,
    /// Contribución del prior [0,1] (0.5 neutro).
    pub prior_score: f64,
    pub factors: Vec<ExplanationFactor>,
}

/// El resultado del ranking enriquecido: el orden advisory + la explicación por variante + flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankingExplanation {
    /// Orden advisory (índices de `variants`, mejor→peor).
    pub order: Vec<usize>,
    pub variants: Vec<VariantExplanation>,
    /// El prior está en cold-start (aún aprendiendo) → no se inyectó.
    pub still_learning: bool,
    /// La inyección está desactivada por setting → no se inyectó.
    pub inject_disabled: bool,
}

/// Pesos de la combinación (clarify §3).
pub const BASE_WEIGHT: f64 = 0.85;
pub const PRIOR_WEIGHT: f64 = 0.15;

/// Combina el ranking advisory de 020 (`base_order`, índices mejor→peor) con el prior local,
/// produciendo un orden enriquecido + explicación (FR-022/FR-023). SIEMPRE advisory (FR-024):
/// NUNCA muta estado ni auto-elige.
///
/// - `base_order = None` ⇒ 020 no produjo ranking (AIE caído/parse-fail). Degradamos: si el prior
///   está caliente Y inject ON, producimos un orden advisory SOLO con el prior; sino devolvemos
///   `None` (picker manual — invariante de 020 preservado).
/// - `inject = false` ⇒ devolvemos exactamente el `base_order` (cero regresión, SC-002).
/// - cold-start ⇒ no inyectamos (flag `still_learning`).
///
/// PURO (recibe el prior ya cargado + las features). El caller decide leer settings y AIE.
pub fn rank_with_prior(
    base_order: Option<&[usize]>,
    variants: &[VariantFeatures],
    prior: &ContextPrior,
    inject: bool,
) -> Option<RankingExplanation> {
    if variants.is_empty() {
        return None;
    }
    let n = variants.len();
    let warm = prior.is_warm();
    let inject_disabled = !inject;

    // base_score por variante: posición normalizada del ranking de 020 (mejor=1.0, peor=0.0).
    // Si no hay base_order, base_score = neutro 0.5 (sin señal de 020).
    let mut base_score = vec![0.5f64; n];
    if let Some(order) = base_order {
        // order[rank] = índice de variante. rank 0 = mejor.
        for (rank, &idx) in order.iter().enumerate() {
            if idx < n {
                base_score[idx] = if n > 1 {
                    1.0 - (rank as f64) / ((n - 1) as f64)
                } else {
                    1.0
                };
            }
        }
    }

    let fmax = preference_prior::feature_max_of(variants);
    let use_prior = inject && warm;

    let mut explanations = Vec::with_capacity(n);
    let mut scored: Vec<(usize, f64)> = Vec::with_capacity(n);
    for (i, vf) in variants.iter().enumerate() {
        let ps = preference_prior::score_variant(prior, vf, &fmax);
        let combined = if use_prior {
            BASE_WEIGHT * base_score[i] + PRIOR_WEIGHT * ps.score
        } else {
            base_score[i]
        };
        scored.push((i, combined));
        explanations.push(VariantExplanation {
            task_id: vf.task_id.clone(),
            combined_score: combined,
            base_score: base_score[i],
            prior_score: ps.score,
            factors: if use_prior { ps.factors } else { Vec::new() },
        });
    }

    // Si no hay base_order Y no usamos prior ⇒ sin señal ⇒ None (picker manual, invariante 020).
    if base_order.is_none() && !use_prior {
        return None;
    }

    // ordenar por combined_score desc; tie-break estable por índice (determinismo).
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let order: Vec<usize> = scored.iter().map(|(i, _)| *i).collect();

    Some(RankingExplanation {
        order,
        variants: explanations,
        still_learning: !warm,
        inject_disabled,
    })
}

// ── lectura de settings (mismo patrón que done_detection::aie_meta_enabled) ──────

/// ¿Registrar la señal? Setting `preference.record_enabled`. Default **ON** (clarify §5).
pub fn record_enabled(db: &Db) -> bool {
    let conn = db.lock();
    crate::settings::get(&conn, "preference.record_enabled")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// ¿Inyectar el prior al ranking? Setting `preference.inject`. Default **OFF** (clarify §5, opt-in).
pub fn inject_enabled(db: &Db) -> bool {
    let conn = db.lock();
    crate::settings::get(&conn, "preference.inject")
        .ok()
        .flatten()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Override de risky-paths del setting `preference.risky_paths` (vacío ⇒ None ⇒ default).
pub fn risky_paths_override(db: &Db) -> Option<Vec<String>> {
    let conn = db.lock();
    let raw = crate::settings::get(&conn, "preference.risky_paths")
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();
    variant_features::parse_risky_paths_setting(&raw)
}

/// Carga la review + deriva el record desde el estado final. Helper para el comando de captura.
/// `Err` si la review no existe.
pub fn capture_from_review(
    db: &Db,
    group_id: &str,
    repo_path: &str,
    task_type: &str,
    inputs: &[VariantInput],
    risky_patterns: Option<&[String]>,
) -> Result<PreferenceRecord> {
    let group = review::load_group(db, group_id)?
        .ok_or_else(|| anyhow!("review no abierta para el grupo {group_id}"))?;
    let rec = derive_record(&group, repo_path, task_type, inputs, risky_patterns);
    persist_record(db, &rec)?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::review::{ChangeSet, GroupReview, Hunk, VariantReview};
    use crate::services::variant_features::F_DIFF_ADDED;

    fn db_in_memory() -> Db {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // aplicar SOLO la migración 043 + las tablas de review que necesita load_group en los tests
        // que la usan. Para los tests de persistencia basta 043.
        conn.execute_batch(include_str!("../../migrations/043_preference_loop.sql"))
            .unwrap();
        std::sync::Arc::new(parking_lot::Mutex::new(conn))
    }

    fn group_with(states: &[(&str, &[HunkState])]) -> GroupReview {
        let variants = states
            .iter()
            .map(|(task, hstates)| VariantReview {
                task_id: task.to_string(),
                change_set: ChangeSet {
                    hunks: hstates
                        .iter()
                        .enumerate()
                        .map(|(n, st)| Hunk {
                            id: format!("{task}:{n}"),
                            file: "f.rs".into(),
                            header: "@@ -1,1 +1,1 @@".into(),
                            state: *st,
                        })
                        .collect(),
                },
            })
            .collect();
        GroupReview {
            group_id: "g1".into(),
            revision: 3,
            variants,
        }
    }

    fn inputs() -> Vec<VariantInput> {
        vec![
            VariantInput {
                task_id: "a".into(),
                agent_profile_id: Some("planner".into()),
                diff: "diff --git a/f.rs b/f.rs\n+x\n+y\n".into(),
                quality_gate: None,
            },
            VariantInput {
                task_id: "b".into(),
                agent_profile_id: None,
                diff: "diff --git a/f.rs b/f.rs\n+1\n+2\n+3\n+4\n".into(),
                quality_gate: None,
            },
        ]
    }

    #[test]
    fn derive_single_winner() {
        let g = group_with(&[
            ("a", &[HunkState::Approved]),
            ("b", &[HunkState::Rejected]),
        ]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        assert_eq!(rec.outcome_kind, OutcomeKind::Single);
        assert!(rec.variants.iter().find(|v| v.features.task_id == "a").unwrap().chosen);
        assert!(!rec.variants.iter().find(|v| v.features.task_id == "b").unwrap().chosen);
        // repo_key scrubbeado: NO contiene la ruta.
        assert!(rec.repo_key.starts_with("repo:"));
        assert!(!rec.repo_key.contains("/tmp/repo"));
    }

    #[test]
    fn derive_mixed_cherry_pick() {
        let g = group_with(&[
            ("a", &[HunkState::Approved]),
            ("b", &[HunkState::Approved]),
        ]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        assert_eq!(rec.outcome_kind, OutcomeKind::Mixed);
        assert_eq!(rec.outcome_kind.obs_weight(), 0.5);
    }

    #[test]
    fn derive_none_all_rejected() {
        let g = group_with(&[
            ("a", &[HunkState::Rejected]),
            ("b", &[HunkState::Pending]),
        ]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        assert_eq!(rec.outcome_kind, OutcomeKind::None);
        assert!(rec.variants.iter().all(|v| !v.chosen), "ningún ganador inventado");
    }

    #[test]
    fn no_raw_code_in_record() {
        // SC-008: el record solo lleva features numéricas; NUNCA el texto del diff.
        let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(!json.contains("diff --git"), "no debe filtrar el diff crudo");
        assert!(!json.contains("+x"), "no debe filtrar líneas de código");
    }

    #[test]
    fn persist_and_list_roundtrip() {
        let db = db_in_memory();
        let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        persist_record(&db, &rec).unwrap();
        let listed = list_records(&db, 10).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].outcome_kind, OutcomeKind::Single);
        assert_eq!(listed[0].variants.len(), 2);
    }

    #[test]
    fn preference_records_append_only() {
        let db = db_in_memory();
        let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        persist_record(&db, &rec).unwrap();
        let conn = db.lock();
        let upd = conn.execute("UPDATE preference_records SET task_type='x' WHERE id=?1", params![rec.id]);
        assert!(upd.is_err(), "UPDATE debe ser rechazado (append-only)");
        let del = conn.execute("DELETE FROM preference_records WHERE id=?1", params![rec.id]);
        assert!(del.is_err(), "DELETE debe ser rechazado (append-only)");
    }

    #[test]
    fn variant_features_append_only() {
        let db = db_in_memory();
        let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        persist_record(&db, &rec).unwrap();
        let conn = db.lock();
        let upd = conn.execute("UPDATE variant_features SET value=99 WHERE record_id=?1", params![rec.id]);
        assert!(upd.is_err(), "variant_features UPDATE rechazado");
    }

    #[test]
    fn context_priors_mutable() {
        // el prior SÍ debe permitir update (evoluciona).
        let db = db_in_memory();
        let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
        let rec = derive_record(&g, "/tmp/repo", "feature", &inputs(), None);
        persist_record(&db, &rec).unwrap(); // esto ya hace update_prior (upsert)
        let prior = load_prior(&db, &rec.repo_key, "feature").unwrap();
        assert_eq!(prior.sample_count, 1, "el prior se actualizó (mutable)");
    }

    #[test]
    fn prior_independence_by_context() {
        // SC-006: dos contextos no se contaminan.
        let db = db_in_memory();
        for _ in 0..20 {
            let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
            let rec = derive_record(&g, "/repoA", "feature", &inputs(), None);
            persist_record(&db, &rec).unwrap();
        }
        let pa = load_prior(&db, &repo_key_for("/repoA"), "feature").unwrap();
        let pb = load_prior(&db, &repo_key_for("/repoB"), "feature").unwrap();
        assert_eq!(pa.sample_count, 20);
        assert_eq!(pb.sample_count, 0, "repoB intacto (independiente)");
    }

    #[test]
    fn reset_prior_returns_to_cold_start() {
        let db = db_in_memory();
        for _ in 0..20 {
            let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
            let rec = derive_record(&g, "/repoA", "feature", &inputs(), None);
            persist_record(&db, &rec).unwrap();
        }
        let rk = repo_key_for("/repoA");
        reset_prior(&db, Some(&rk), Some("feature")).unwrap();
        let p = load_prior(&db, &rk, "feature").unwrap();
        assert_eq!(p.sample_count, 0, "reset ⇒ cold-start");
        assert!(p.features.is_empty());
        // los records (señal) NO se tocan.
        assert!(!list_records(&db, 100).unwrap().is_empty(), "la señal append-only sobrevive al reset");
    }

    fn vf(task: &str, added: f64) -> VariantFeatures {
        VariantFeatures {
            task_id: task.into(),
            agent_profile_id: None,
            features: vec![(F_DIFF_ADDED.into(), FeatureValue::measured(added))],
        }
    }

    #[test]
    fn inject_off_returns_base_order() {
        // SC-002: inject=false ⇒ orden idéntico al de 020.
        let variants = vec![vf("a", 5.0), vf("b", 50.0)];
        let prior = ContextPrior::empty("r", "t");
        let base = vec![1usize, 0usize]; // 020 dice b mejor que a
        let r = rank_with_prior(Some(&base), &variants, &prior, false).unwrap();
        assert_eq!(r.order, vec![1, 0], "inject OFF ⇒ base_order intacto");
        assert!(r.inject_disabled);
    }

    #[test]
    fn inject_cold_start_does_not_change_order() {
        // SC-003 (negativo): prior frío ⇒ no inyecta aunque inject=ON.
        let variants = vec![vf("a", 5.0), vf("b", 50.0)];
        let prior = ContextPrior::empty("r", "t"); // sample_count 0
        let base = vec![1usize, 0usize];
        let r = rank_with_prior(Some(&base), &variants, &prior, true).unwrap();
        assert_eq!(r.order, vec![1, 0]);
        assert!(r.still_learning, "cold-start ⇒ aún aprendiendo");
    }

    #[test]
    fn inject_warm_prior_promotes_learned_pattern() {
        // SC-003: con prior caliente "menos cambios", la variante con menos added sube.
        let db = db_in_memory();
        for _ in 0..20 {
            let g = group_with(&[("a", &[HunkState::Approved]), ("b", &[HunkState::Rejected])]);
            // a tiene MENOS líneas (2) que b (4) en los inputs → aprende "menos es mejor".
            let rec = derive_record(&g, "/repoA", "feature", &inputs(), None);
            persist_record(&db, &rec).unwrap();
        }
        let prior = load_prior(&db, &repo_key_for("/repoA"), "feature").unwrap();
        assert!(prior.is_warm(), "20 muestras con diversidad ⇒ caliente");
        // ahora 020 dice que la grande (más cambios) es mejor; el prior debe contrarrestar.
        let variants = vec![vf("small", 5.0), vf("big", 50.0)];
        let base = vec![1usize, 0usize]; // 020: big(idx1) mejor
        let r = rank_with_prior(Some(&base), &variants, &prior, true).unwrap();
        // el prior empuja "small" arriba; con 15% no necesariamente lidera, pero su explicación
        // y prior_score deben favorecer a small.
        let small_expl = r.variants.iter().find(|v| v.task_id == "small").unwrap();
        let big_expl = r.variants.iter().find(|v| v.task_id == "big").unwrap();
        assert!(
            small_expl.prior_score > big_expl.prior_score,
            "el prior debe favorecer la variante con menos cambios ({} vs {})",
            small_expl.prior_score,
            big_expl.prior_score
        );
        // explicación NO vacía (SC-004).
        assert!(!small_expl.factors.is_empty(), "sugerencia con prior ⇒ razón visible");
    }

    #[test]
    fn no_base_no_warm_is_none() {
        // SC-005: 020 caído (None) + prior frío ⇒ None (picker manual). NUNCA auto-elige.
        let variants = vec![vf("a", 5.0), vf("b", 50.0)];
        let prior = ContextPrior::empty("r", "t");
        assert!(rank_with_prior(None, &variants, &prior, true).is_none());
    }

    #[test]
    fn empty_variants_is_none() {
        let prior = ContextPrior::empty("r", "t");
        assert!(rank_with_prior(Some(&[]), &[], &prior, true).is_none());
    }
}
