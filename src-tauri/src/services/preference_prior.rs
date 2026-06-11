// services/preference_prior.rs — 026 F1 (US2/US3) · prior local explicable por contexto.
//
// Modelo de prior (clarify §1, council v2 HIGH): pesos lineales por feature + actualización
// bayesiana **Beta** por feature. Determinista, incremental, EXPLICABLE. NO LLM, NO gradiente, NO
// training. 100% local (F-I BYOK puro: no toca API keys).
//
// Por cada feature, un par Beta `(alpha, beta)` acumula evidencia:
//   - `alpha` += "el feature de la variante ELEGIDA fue favorable vs el de las rechazadas"
//   - `beta`  += lo inverso
// El peso aprendido = `2*(alpha/(alpha+beta) - 0.5)` ∈ [-1, 1] (dirección + magnitud).
//
// INVARIANTES:
//   - SIEMPRE advisory: este módulo SÓLO produce un score + explicación; NUNCA muta estado de
//     review ni auto-elige (el caller, `rank_with_prior`, preserva el contrato de 020).
//   - Cold-start conservador (clarify §2): bajo `sample_count < COLD_START_N` (15) + sin diversidad
//     ⇒ el prior NO se inyecta; flag `still_learning`.
//   - Decay exponencial (clarify §3): cada update encoge la evidencia previa hacia el prior neutro
//     (DECAY=0.98) → se adapta a cambios de gusto, evita feedback loops.
//   - Independencia por contexto (FR-021/SC-006): la clave es `(repo_key, task_type)`; un contexto
//     NUNCA altera otro.
//   - "No medido" ≠ 0 (FR-012): los features ausentes NO contribuyen al score ni al update.

use crate::services::variant_features::VariantFeatures;
use serde::{Deserialize, Serialize};

/// Umbral de cold-start: nº de decisiones registradas en un contexto antes de que el prior influya
/// (clarify §2). Por debajo: `still_learning`.
pub const COLD_START_N: i64 = 15;

/// Factor de decay exponencial por update (clarify §3). Encoge la evidencia previa hacia el prior
/// neutro Beta(1,1) antes de sumar la nueva → las muestras viejas pesan menos.
pub const DECAY: f64 = 0.98;

/// Nº mínimo de features que deben mostrar ≥2 valores distintos para superar la restricción de
/// diversidad (anti-prior-degenerado, clarify §2).
pub const DIVERSITY_MIN_FEATURES: i64 = 2;

/// Estado Beta de UN feature dentro de un contexto.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FeatureBeta {
    pub alpha: f64,
    pub beta: f64,
    /// Nº de valores distintos observados de este feature (diversidad, clarify §2).
    pub distinct_obs: i64,
}

impl Default for FeatureBeta {
    fn default() -> Self {
        // Beta(1,1) = uniforme = neutro (sin sesgo inicial).
        Self {
            alpha: 1.0,
            beta: 1.0,
            distinct_obs: 0,
        }
    }
}

impl FeatureBeta {
    /// Media Beta = alpha/(alpha+beta) ∈ (0,1). 0.5 = neutro.
    pub fn mean(&self) -> f64 {
        let denom = self.alpha + self.beta;
        if denom <= 0.0 {
            0.5
        } else {
            self.alpha / denom
        }
    }

    /// Peso aprendido ∈ [-1,1]: dirección (signo) + magnitud. Centrado en la media Beta.
    /// `+` ⇒ "valores ALTOS de este feature se asocian a tu elección"; `-` ⇒ valores BAJOS.
    pub fn weight(&self) -> f64 {
        2.0 * (self.mean() - 0.5)
    }
}

/// El prior aprendido de UN contexto `(repo_key, task_type)`: Beta por feature + sample_count.
/// MUTABLE (evoluciona). Explicable y reseteable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPrior {
    pub repo_key: String,
    pub task_type: String,
    /// `(feature_key, FeatureBeta)`.
    pub features: Vec<(String, FeatureBeta)>,
    pub sample_count: i64,
}

impl ContextPrior {
    pub fn empty(repo_key: &str, task_type: &str) -> Self {
        Self {
            repo_key: repo_key.to_string(),
            task_type: task_type.to_string(),
            features: Vec::new(),
            sample_count: 0,
        }
    }

    fn get_mut(&mut self, key: &str) -> &mut FeatureBeta {
        if let Some(pos) = self.features.iter().position(|(k, _)| k == key) {
            &mut self.features[pos].1
        } else {
            self.features.push((key.to_string(), FeatureBeta::default()));
            &mut self.features.last_mut().unwrap().1
        }
    }

    pub fn get(&self, key: &str) -> Option<FeatureBeta> {
        self.features
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, b)| *b)
    }

    /// ¿El prior superó el cold-start? (clarify §2): suficientes muestras Y diversidad mínima.
    pub fn is_warm(&self) -> bool {
        if self.sample_count < COLD_START_N {
            return false;
        }
        let diverse = self
            .features
            .iter()
            .filter(|(_, b)| b.distinct_obs >= 2)
            .count() as i64;
        diverse >= DIVERSITY_MIN_FEATURES
    }
}

/// Una observación normalizada para el update: por cada feature MEDIDO, si el valor de la variante
/// elegida fue favorable (más bajo, en nuestro dominio "menos es mejor" se aprende solo) vs la
/// referencia de las rechazadas. Ver `update_from_record`.
///
/// `chosen` distingue elegidas (parcial/total) de rechazadas. `weight` (1.0 normal, 0.5 para
/// elección mixta) atenúa la evidencia (clarify §7).
#[derive(Debug, Clone)]
pub struct PreferenceObservation {
    pub variants: Vec<VariantFeatures>,
    pub chosen: Vec<bool>,
    /// Atenuación de la evidencia (1.0 = single, 0.5 = mixed, 1.0 = none-negativa).
    pub weight: f64,
}

/// Actualiza el prior con UNA observación (un PreferenceRecord). Determinista + incremental.
///
/// Por cada feature MEDIDO en TODAS las variantes comparables:
///   - referencia = mediana del valor del feature entre las variantes RECHAZADAS.
///   - por cada variante ELEGIDA: si su valor < referencia ⇒ evidencia de que "valores bajos de este
///     feature se asocian a tu elección" → suma a `beta` (peso hacia "menos es mejor"); si > ⇒
///     suma a `alpha`. (alpha = "alto favorable", beta = "bajo favorable").
///   - el decay encoge la evidencia previa ANTES de sumar (clarify §3).
///
/// Caso `none` (todo rechazado): no hay elegida → no se mueve la dirección (no inventamos ganador);
/// sólo cuenta como muestra (sample_count++) y registra diversidad. Caso `mixed`: peso 0.5.
pub fn update_from_record(prior: &mut ContextPrior, obs: &PreferenceObservation) {
    // 1) decay de TODA la evidencia previa hacia el neutro (1,1) — clarify §3.
    for (_, b) in prior.features.iter_mut() {
        b.alpha = 1.0 + (b.alpha - 1.0) * DECAY;
        b.beta = 1.0 + (b.beta - 1.0) * DECAY;
    }

    prior.sample_count += 1;

    // 2) Reunir, por feature, los valores medidos de elegidas y rechazadas.
    use std::collections::BTreeMap;
    let mut chosen_vals: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut rejected_vals: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut all_vals: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for (i, vf) in obs.variants.iter().enumerate() {
        let chosen = *obs.chosen.get(i).unwrap_or(&false);
        for (k, fv) in &vf.features {
            if !fv.measured {
                continue; // AUSENTE no contribuye (FR-012).
            }
            all_vals.entry(k.clone()).or_default().push(fv.value);
            if chosen {
                chosen_vals.entry(k.clone()).or_default().push(fv.value);
            } else {
                rejected_vals.entry(k.clone()).or_default().push(fv.value);
            }
        }
    }

    // 3) Diversidad: por feature, cuántos valores distintos vimos en esta observación.
    for (k, vals) in &all_vals {
        let distinct = distinct_count(vals);
        let fb = prior.get_mut(k);
        // distinct_obs acumula (monótono): refleja que el feature ha variado a lo largo del tiempo.
        if distinct >= 2 {
            fb.distinct_obs = fb.distinct_obs.saturating_add(1);
        }
    }

    // 4) Evidencia direccional: sólo cuando HAY elegidas Y rechazadas (comparación válida).
    if chosen_vals.is_empty() || rejected_vals.is_empty() {
        return; // none o single-variante: muestra contada, sin dirección inventada.
    }
    for (k, ch) in &chosen_vals {
        let Some(rej) = rejected_vals.get(k) else {
            continue;
        };
        let ref_val = median(rej);
        let fb = prior.get_mut(k);
        for &cv in ch {
            // evidencia con atenuación (clarify §7 mixed=0.5).
            let w = obs.weight.max(0.0);
            if cv < ref_val {
                // valor BAJO favorable ⇒ refuerza "menos es mejor".
                fb.beta += w;
            } else if cv > ref_val {
                fb.alpha += w;
            }
            // cv == ref_val: empate, sin evidencia direccional.
        }
    }
}

/// Un factor de la explicación de una sugerencia (FR-023): qué feature, en qué dirección, cuánto
/// contribuyó. Legible, NUNCA opaco.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplanationFactor {
    pub feature_key: String,
    /// Dirección legible: "menos es mejor" / "más es mejor" / "neutro".
    pub direction: String,
    /// Contribución al score (signed) de ESTA variante por este feature.
    pub contribution: f64,
    /// Peso aprendido del feature ∈ [-1,1].
    pub weight: f64,
}

/// El resultado de scorear una variante contra el prior: score normalizado ∈ [0,1] + factores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriorScore {
    pub task_id: String,
    /// Score normalizado [0,1] (0.5 = neutro). Más alto = mejor según lo aprendido.
    pub score: f64,
    pub factors: Vec<ExplanationFactor>,
}

/// Scorea UNA variante contra el prior (PURO). Para cada feature MEDIDO con peso aprendido no
/// despreciable, computa `weight * value_norm` donde `value_norm` ∈ [0,1] es el valor del feature
/// normalizado por el MÁXIMO observado entre las variantes del set (`feature_max`). Devuelve un
/// score [0,1] + los factores legibles.
///
/// `feature_max`: el máximo de cada feature en el set actual (para normalizar). Si 0 ⇒ feature
/// constante, no contribuye.
pub fn score_variant(
    prior: &ContextPrior,
    vf: &VariantFeatures,
    feature_max: &std::collections::BTreeMap<String, f64>,
) -> PriorScore {
    let mut raw = 0.0f64;
    let mut wsum = 0.0f64;
    let mut factors = Vec::new();
    for (k, fb) in &prior.features {
        let Some(fval) = vf.get(k) else { continue };
        if !fval.measured {
            continue; // ausente no contribuye (FR-012).
        }
        let w = fb.weight();
        if w.abs() < 1e-6 {
            continue; // peso despreciable: feature aún neutro.
        }
        let max = feature_max.get(k).copied().unwrap_or(0.0);
        if max <= 0.0 {
            continue; // feature constante / cero en el set.
        }
        let value_norm = (fval.value / max).clamp(0.0, 1.0);
        // contribución: w>0 ⇒ "más es mejor" (alto value_norm suma); w<0 ⇒ "menos es mejor".
        let contribution = w * value_norm;
        raw += contribution;
        wsum += w.abs();
        let direction = if w < 0.0 {
            "menos es mejor"
        } else {
            "más es mejor"
        };
        factors.push(ExplanationFactor {
            feature_key: k.clone(),
            direction: direction.to_string(),
            contribution,
            weight: w,
        });
    }
    // Normalizar a [0,1]: raw ∈ [-wsum, wsum] → (raw/wsum + 1)/2. wsum=0 ⇒ neutro 0.5.
    let score = if wsum <= 0.0 {
        0.5
    } else {
        ((raw / wsum) + 1.0) / 2.0
    };
    PriorScore {
        task_id: vf.task_id.clone(),
        score: score.clamp(0.0, 1.0),
        factors,
    }
}

// ── helpers numéricos ───────────────────────────────────────────────────────────

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

fn distinct_count(v: &[f64]) -> usize {
    let mut s: Vec<i64> = v.iter().map(|x| (x * 1000.0).round() as i64).collect();
    s.sort_unstable();
    s.dedup();
    s.len()
}

/// Helper para construir el `feature_max` de un set de variantes (normalización del score).
pub fn feature_max_of(variants: &[VariantFeatures]) -> std::collections::BTreeMap<String, f64> {
    let mut m: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    for vf in variants {
        for (k, fv) in &vf.features {
            if fv.measured {
                let e = m.entry(k.clone()).or_insert(0.0);
                if fv.value > *e {
                    *e = fv.value;
                }
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::variant_features::*;

    fn vf(task: &str, added: f64, risky: f64, chosen_marker: bool) -> VariantFeatures {
        let _ = chosen_marker;
        VariantFeatures {
            task_id: task.to_string(),
            agent_profile_id: None,
            features: vec![
                (F_DIFF_ADDED.into(), FeatureValue::measured(added)),
                (F_RISKY_PATHS.into(), FeatureValue::measured(risky)),
            ],
        }
    }

    /// Una observación: variante 0 elegida con MENOS cambios (added bajo) vs rechazada con más.
    fn obs_prefers_less(weight: f64) -> PreferenceObservation {
        PreferenceObservation {
            variants: vec![vf("a", 5.0, 0.0, true), vf("b", 50.0, 2.0, false)],
            chosen: vec![true, false],
            weight,
        }
    }

    #[test]
    fn beta_default_is_neutral() {
        let fb = FeatureBeta::default();
        assert_eq!(fb.mean(), 0.5);
        assert_eq!(fb.weight(), 0.0);
    }

    #[test]
    fn update_learns_less_is_better_for_diff() {
        let mut p = ContextPrior::empty("repoX", "feature");
        for _ in 0..20 {
            update_from_record(&mut p, &obs_prefers_less(1.0));
        }
        let fb = p.get(F_DIFF_ADDED).expect("aprendió diff_added");
        // elegida tenía MENOS added ⇒ beta crece ⇒ weight negativo ("menos es mejor").
        assert!(
            fb.weight() < 0.0,
            "esperaba peso negativo (menos cambios mejor), got {}",
            fb.weight()
        );
        assert_eq!(p.sample_count, 20);
    }

    #[test]
    fn cold_start_blocks_until_threshold() {
        let mut p = ContextPrior::empty("r", "t");
        for _ in 0..(COLD_START_N - 1) {
            update_from_record(&mut p, &obs_prefers_less(1.0));
        }
        assert!(!p.is_warm(), "bajo el umbral ⇒ frío");
        update_from_record(&mut p, &obs_prefers_less(1.0));
        assert!(
            p.is_warm(),
            "alcanzado el umbral con diversidad ⇒ caliente (sample={})",
            p.sample_count
        );
    }

    #[test]
    fn no_diversity_stays_cold_even_above_n() {
        // todas las variantes idénticas en cada record ⇒ sin diversidad ⇒ frío aunque sample>=N.
        let mut p = ContextPrior::empty("r", "t");
        let flat = PreferenceObservation {
            variants: vec![vf("a", 10.0, 0.0, true), vf("b", 10.0, 0.0, false)],
            chosen: vec![true, false],
            weight: 1.0,
        };
        for _ in 0..(COLD_START_N + 5) {
            update_from_record(&mut p, &flat);
        }
        assert!(p.sample_count >= COLD_START_N);
        assert!(!p.is_warm(), "sin diversidad ⇒ debe seguir frío (anti-degenerado)");
    }

    #[test]
    fn score_favors_learned_pattern() {
        let mut p = ContextPrior::empty("r", "t");
        for _ in 0..20 {
            update_from_record(&mut p, &obs_prefers_less(1.0));
        }
        let small = vf("small", 5.0, 0.0, false);
        let big = vf("big", 50.0, 0.0, false);
        let set = vec![small.clone(), big.clone()];
        let fmax = feature_max_of(&set);
        let s_small = score_variant(&p, &small, &fmax);
        let s_big = score_variant(&p, &big, &fmax);
        assert!(
            s_small.score > s_big.score,
            "la variante con menos cambios debe scorear más alto ({} vs {})",
            s_small.score,
            s_big.score
        );
        // explicación no vacía (FR-023/SC-004).
        assert!(!s_small.factors.is_empty());
        assert!(s_small.factors.iter().any(|f| f.feature_key == F_DIFF_ADDED));
    }

    #[test]
    fn absent_feature_does_not_contribute() {
        let mut p = ContextPrior::empty("r", "t");
        for _ in 0..20 {
            update_from_record(&mut p, &obs_prefers_less(1.0));
        }
        // variante con qg_errors AUSENTE: no aporta factor de qg.
        let mut v = vf("x", 5.0, 0.0, false);
        v.features.push((F_QG_ERRORS.into(), FeatureValue::absent()));
        let set = vec![v.clone()];
        let fmax = feature_max_of(&set);
        let s = score_variant(&p, &v, &fmax);
        assert!(
            !s.factors.iter().any(|f| f.feature_key == F_QG_ERRORS),
            "feature ausente NO debe contribuir"
        );
    }

    #[test]
    fn deterministic_same_input_same_score() {
        let mut p1 = ContextPrior::empty("r", "t");
        let mut p2 = ContextPrior::empty("r", "t");
        for _ in 0..18 {
            update_from_record(&mut p1, &obs_prefers_less(1.0));
            update_from_record(&mut p2, &obs_prefers_less(1.0));
        }
        let v = vf("x", 7.0, 0.0, false);
        let set = vec![v.clone()];
        let fmax = feature_max_of(&set);
        assert_eq!(
            score_variant(&p1, &v, &fmax).score,
            score_variant(&p2, &v, &fmax).score,
            "mismo input ⇒ mismo score (determinista)"
        );
    }

    #[test]
    fn none_outcome_counts_sample_without_direction() {
        let mut p = ContextPrior::empty("r", "t");
        let none_obs = PreferenceObservation {
            variants: vec![vf("a", 5.0, 0.0, false), vf("b", 50.0, 0.0, false)],
            chosen: vec![false, false], // todo rechazado
            weight: 1.0,
        };
        update_from_record(&mut p, &none_obs);
        assert_eq!(p.sample_count, 1, "cuenta como muestra");
        // sin elegidas ⇒ no se movió la dirección (no inventó ganador).
        let fb = p.get(F_DIFF_ADDED).unwrap_or_default();
        assert_eq!(fb.weight(), 0.0, "sin dirección inventada en outcome=none");
    }

    #[test]
    fn decay_shrinks_old_evidence() {
        let mut p = ContextPrior::empty("r", "t");
        // mucha evidencia "menos es mejor"
        for _ in 0..40 {
            update_from_record(&mut p, &obs_prefers_less(1.0));
        }
        let w_before = p.get(F_DIFF_ADDED).unwrap().weight();
        // ahora evidencia OPUESTA (elegida con MÁS cambios)
        let opposite = PreferenceObservation {
            variants: vec![vf("a", 50.0, 0.0, true), vf("b", 5.0, 0.0, false)],
            chosen: vec![true, false],
            weight: 1.0,
        };
        for _ in 0..40 {
            update_from_record(&mut p, &opposite);
        }
        let w_after = p.get(F_DIFF_ADDED).unwrap().weight();
        assert!(
            w_after > w_before,
            "el decay debe permitir que el criterio nuevo (más es mejor) revierta el viejo ({} → {})",
            w_before,
            w_after
        );
    }
}
