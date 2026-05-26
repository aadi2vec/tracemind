//! Aggregated report types + JSON serialization + regression gate.
//!
//! Shape is intentionally close to `tm-bench-locomo`'s report so the
//! same CI machinery (diff vs. baseline, fail on big drops) can run
//! against both. The top-level metric here is `persistence_score` —
//! the seed-gate target is ≥ 90.0.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::dataset::{Category, PersistenceDataset};
use crate::scoring::{best_exact_match, best_f1, Score};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairOutcome {
    pub pair_id: String,
    pub category: Category,
    pub query: String,
    pub prediction: String,
    pub references: Vec<String>,
    pub f1: f32,
    pub exact_match: f32,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryBreakdown {
    pub count: usize,
    pub f1: f32,
    pub exact_match: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub runner: String,
    pub timestamp: String,
    pub total_pairs: usize,
    /// Top-line metric. Micro-averaged F1 × 100. Seed-gate target ≥ 90.0.
    pub persistence_score: f32,
    pub overall_exact_match: f32,
    pub by_category: BTreeMap<String, CategoryBreakdown>,
    pub wall_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<PairOutcome>,
}

impl BenchmarkReport {
    pub fn from_outcomes(
        runner: impl Into<String>,
        _dataset: &PersistenceDataset,
        outcomes: Vec<PairOutcome>,
        wall_seconds: f64,
    ) -> Self {
        let total_pairs = outcomes.len();
        let (sum_f1, sum_em) = outcomes.iter().fold((0.0f32, 0.0f32), |(f, e), o| {
            (f + o.f1, e + o.exact_match)
        });
        let persistence_score = if total_pairs == 0 {
            0.0
        } else {
            sum_f1 / total_pairs as f32 * 100.0
        };
        let overall_em = if total_pairs == 0 {
            0.0
        } else {
            sum_em / total_pairs as f32 * 100.0
        };

        let mut acc: BTreeMap<String, (usize, f32, f32)> = BTreeMap::new();
        for o in &outcomes {
            let entry = acc
                .entry(o.category.as_str().to_string())
                .or_default();
            entry.0 += 1;
            entry.1 += o.f1;
            entry.2 += o.exact_match;
        }
        let by_category: BTreeMap<String, CategoryBreakdown> = acc
            .into_iter()
            .map(|(k, (count, sf, sem))| {
                (
                    k,
                    CategoryBreakdown {
                        count,
                        f1: if count == 0 { 0.0 } else { sf / count as f32 * 100.0 },
                        exact_match: if count == 0 {
                            0.0
                        } else {
                            sem / count as f32 * 100.0
                        },
                    },
                )
            })
            .collect();

        Self {
            runner: runner.into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            total_pairs,
            persistence_score,
            overall_exact_match: overall_em,
            by_category,
            wall_seconds,
            outcomes,
        }
    }

    pub fn score_pair(prediction: &str, references: &[String]) -> (Score, Score) {
        (
            best_f1(prediction, references),
            best_exact_match(prediction, references),
        )
    }

    pub fn summarized(mut self) -> Self {
        self.outcomes.clear();
        self
    }

    pub fn one_line(&self) -> String {
        format!(
            "{}  persistence={:.2}  EM={:.2}  n={}  t={:.1}s",
            self.runner,
            self.persistence_score,
            self.overall_exact_match,
            self.total_pairs,
            self.wall_seconds
        )
    }
}

/// CI regression gate. Returns Err with a human-readable diff when the
/// new score is more than `tolerance` points below the baseline.
pub fn regression_gate(
    previous: &BenchmarkReport,
    current: &BenchmarkReport,
    tolerance: f32,
) -> Result<(), String> {
    let delta = current.persistence_score - previous.persistence_score;
    if delta < -tolerance {
        return Err(format!(
            "persistence regressed {:.2} points (prev {:.2} → now {:.2}, tolerance {:.2})",
            -delta, previous.persistence_score, current.persistence_score, tolerance
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(cat: Category, f1: f32, em: f32) -> PairOutcome {
        PairOutcome {
            pair_id: "p".into(),
            category: cat,
            query: "?".into(),
            prediction: "p".into(),
            references: vec!["r".into()],
            f1,
            exact_match: em,
            latency_ms: 0,
            error: None,
        }
    }

    #[test]
    fn aggregates_by_category() {
        let ds = PersistenceDataset::default();
        let r = BenchmarkReport::from_outcomes(
            "t",
            &ds,
            vec![
                outcome(Category::FactualRecall, 1.0, 1.0),
                outcome(Category::FactualRecall, 0.5, 0.0),
                outcome(Category::TemporalPinning, 0.0, 0.0),
            ],
            1.0,
        );
        assert_eq!(r.total_pairs, 3);
        assert!((r.persistence_score - 50.0).abs() < 1e-4);
        let fc = r.by_category.get("factual_recall").unwrap();
        assert_eq!(fc.count, 2);
        assert!((fc.f1 - 75.0).abs() < 1e-4);
    }

    #[test]
    fn empty_report_zero_score() {
        let ds = PersistenceDataset::default();
        let r = BenchmarkReport::from_outcomes("t", &ds, vec![], 0.0);
        assert_eq!(r.persistence_score, 0.0);
    }

    #[test]
    fn gate_passes_on_improvement() {
        let ds = PersistenceDataset::default();
        let prev = BenchmarkReport::from_outcomes(
            "old",
            &ds,
            vec![outcome(Category::FactualRecall, 0.8, 0.8)],
            0.0,
        );
        let curr = BenchmarkReport::from_outcomes(
            "new",
            &ds,
            vec![outcome(Category::FactualRecall, 0.9, 0.9)],
            0.0,
        );
        assert!(regression_gate(&prev, &curr, 0.5).is_ok());
    }

    #[test]
    fn gate_fails_on_big_drop() {
        let ds = PersistenceDataset::default();
        let prev = BenchmarkReport::from_outcomes(
            "old",
            &ds,
            vec![outcome(Category::FactualRecall, 0.9, 0.9)],
            0.0,
        );
        let curr = BenchmarkReport::from_outcomes(
            "new",
            &ds,
            vec![outcome(Category::FactualRecall, 0.5, 0.5)],
            0.0,
        );
        assert!(regression_gate(&prev, &curr, 0.5).is_err());
    }
}
