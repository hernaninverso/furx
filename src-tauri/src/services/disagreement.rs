// W1 / 1.10 — Cross-LLM disagreement detector.
// Stateless analyzer: tomar N responses (1 por pane LLM) y devolver
// similarity scores + outlier flags.
//
// Council V1: no exec, no network — solo análisis de strings.
// V4: edge cases — responses vacías, all idénticas, all distintas.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmResponse {
    pub pane_id: String,
    pub mode: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisagreementReport {
    pub consensus_score: f32, // 0.0 = todos discrepan, 1.0 = todos iguales
    pub pairwise: Vec<PairwiseSim>,
    pub outliers: Vec<String>, // pane_ids semantically apart
    pub by_pane_summary: Vec<PaneSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairwiseSim {
    pub a: String,
    pub b: String,
    pub jaccard: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaneSummary {
    pub pane_id: String,
    pub mode: String,
    pub avg_sim: f32,
    pub chars: usize,
    pub first_line: String,
}

pub fn analyze(responses: &[LlmResponse]) -> DisagreementReport {
    if responses.len() < 2 {
        return DisagreementReport {
            consensus_score: 1.0,
            pairwise: vec![],
            outliers: vec![],
            by_pane_summary: responses
                .iter()
                .map(|r| PaneSummary {
                    pane_id: r.pane_id.clone(),
                    mode: r.mode.clone(),
                    avg_sim: 1.0,
                    chars: r.text.len(),
                    first_line: r.text.lines().next().unwrap_or("").to_string(),
                })
                .collect(),
        };
    }
    let tokens: Vec<std::collections::HashSet<String>> =
        responses.iter().map(|r| tokenize(&r.text)).collect();
    let mut pairwise = Vec::new();
    let mut per_pane_sims: HashMap<String, Vec<f32>> = HashMap::new();
    for i in 0..responses.len() {
        for j in (i + 1)..responses.len() {
            let sim = jaccard(&tokens[i], &tokens[j]);
            pairwise.push(PairwiseSim {
                a: responses[i].pane_id.clone(),
                b: responses[j].pane_id.clone(),
                jaccard: sim,
            });
            per_pane_sims
                .entry(responses[i].pane_id.clone())
                .or_default()
                .push(sim);
            per_pane_sims
                .entry(responses[j].pane_id.clone())
                .or_default()
                .push(sim);
        }
    }
    let consensus_score = if pairwise.is_empty() {
        1.0
    } else {
        pairwise.iter().map(|p| p.jaccard).sum::<f32>() / pairwise.len() as f32
    };
    let mut summaries: Vec<PaneSummary> = responses
        .iter()
        .map(|r| {
            let sims = per_pane_sims.get(&r.pane_id).cloned().unwrap_or_default();
            let avg = if sims.is_empty() {
                1.0
            } else {
                sims.iter().sum::<f32>() / sims.len() as f32
            };
            PaneSummary {
                pane_id: r.pane_id.clone(),
                mode: r.mode.clone(),
                avg_sim: avg,
                chars: r.text.len(),
                first_line: r
                    .text
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect(),
            }
        })
        .collect();
    summaries.sort_by(|a, b| {
        a.avg_sim
            .partial_cmp(&b.avg_sim)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Outlier: avg_sim < 0.3 AND below consensus by 0.2.
    let outliers: Vec<String> = summaries
        .iter()
        .filter(|s| s.avg_sim < 0.3 && (consensus_score - s.avg_sim) > 0.2)
        .map(|s| s.pane_id.clone())
        .collect();
    DisagreementReport {
        consensus_score,
        pairwise,
        outliers,
        by_pane_summary: summaries,
    }
}

fn tokenize(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 3)
        .map(String::from)
        .collect()
}

fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_responses_full_consensus() {
        let r = vec![
            LlmResponse {
                pane_id: "p1".into(),
                mode: "claude-A".into(),
                text: "fix the null pointer".into(),
            },
            LlmResponse {
                pane_id: "p2".into(),
                mode: "codex".into(),
                text: "fix the null pointer".into(),
            },
        ];
        let report = analyze(&r);
        assert!(report.consensus_score > 0.95);
        assert!(report.outliers.is_empty());
    }

    #[test]
    fn divergent_responses_low_consensus() {
        let r = vec![
            LlmResponse {
                pane_id: "p1".into(),
                mode: "claude-A".into(),
                text: "delete the file completely".into(),
            },
            LlmResponse {
                pane_id: "p2".into(),
                mode: "codex".into(),
                text: "preserve all data forever".into(),
            },
            LlmResponse {
                pane_id: "p3".into(),
                mode: "gemini".into(),
                text: "preserve all data forever".into(),
            },
        ];
        let report = analyze(&r);
        assert!(report.consensus_score < 0.5);
        // p1 should be flagged outlier.
        assert!(report.outliers.iter().any(|p| p == "p1"));
    }

    #[test]
    fn empty_responses_no_panic() {
        let r = vec![
            LlmResponse {
                pane_id: "p1".into(),
                mode: "claude-A".into(),
                text: "".into(),
            },
            LlmResponse {
                pane_id: "p2".into(),
                mode: "codex".into(),
                text: "".into(),
            },
        ];
        let report = analyze(&r);
        assert!(report.consensus_score.is_finite());
    }
}
