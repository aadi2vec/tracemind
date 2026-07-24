//! Report types + aggregation + JSON serialization.
//!
//! The report shape is stable so CI can diff runs and gate merges on the
//! top-level `overall_f1`. Per-category breakdowns help spot regressions
//! localized to e.g. temporal questions while overall holds.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::dataset::{Category, LocomoDataset};
use crate::scoring::{best_exact_match, best_f1, Score};

/// Per-question outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOutcome {
    pub sample_id: String,
    pub question_id: String,
    pub category: Category,
    pub question: String,
    pub prediction: String,
    pub references: Vec<String>,
    pub f1: f32,
    pub exact_match: f32,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Per-category aggregate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryBreakdown {
    pub count: usize,
    pub f1: f32,
    pub exact_match: f32,
}

/// Top-level report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    /// Runner name (e.g. "tracemind-v0.1").
    pub runner: String,
    /// ISO-8601 UTC timestamp of run completion.
    pub timestamp: String,
    /// Total questions evaluated.
    pub total_questions: usize,
    /// Total samples (conversations) evaluated.
    pub total_samples: usize,
    /// Overall micro-averaged F1 × 100 (e.g. 87.3).
    pub overall_f1: f32,
    /// Overall micro-averaged exact-match × 100.
    pub overall_exact_match: f32,
    /// Per-category breakdown. Keyed by category name for stable JSON diffs.
    pub by_category: BTreeMap<String, CategoryBreakdown>,
    /// Wall-clock seconds for the full run.
    pub wall_seconds: f64,
    /// Per-question outcomes. Omitted in summary reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<QuestionOutcome>,
}

impl BenchmarkReport {
    /// Build a report from a set of outcomes. `wall_seconds` is the total
    /// wall-clock time the caller measured.
    pub fn from_outcomes(
        runner: impl Into<String>,
        dataset: &LocomoDataset,
        outcomes: Vec<QuestionOutcome>,
        wall_seconds: f64,
    ) -> Self {
        let total_questions = outcomes.len();
        let total_samples = dataset.samples.len();

        let (sum_f1, sum_em) = outcomes.iter().fold((0.0f32, 0.0f32), |(f, e), o| {
            (f + o.f1, e + o.exact_match)
        });
        let overall_f1 = if total_questions == 0 {
            0.0
        } else {
            sum_f1 / total_questions as f32 * 100.0
        };
        let overall_em = if total_questions == 0 {
            0.0
        } else {
            sum_em / total_questions as f32 * 100.0
        };

        let mut by_category: BTreeMap<String, (usize, f32, f32)> = BTreeMap::new();
        for o in &outcomes {
            let entry = by_category
                .entry(o.category.as_str().to_string())
                .or_default();
            entry.0 += 1;
            entry.1 += o.f1;
            entry.2 += o.exact_match;
        }
        let by_category: BTreeMap<String, CategoryBreakdown> = by_category
            .into_iter()
            .map(|(k, (count, sf1, sem))| {
                (
                    k,
                    CategoryBreakdown {
                        count,
                        f1: if count == 0 { 0.0 } else { sf1 / count as f32 * 100.0 },
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
            total_questions,
            total_samples,
            overall_f1,
            overall_exact_match: overall_em,
            by_category,
            wall_seconds,
            outcomes,
        }
    }

    /// Score a single (prediction, references) pair. Utility used by the
    /// CLI / integration tests.
    pub fn score_pair(prediction: &str, references: &[String]) -> (Score, Score) {
        (best_f1(prediction, references), best_exact_match(prediction, references))
    }

    /// Drop per-question detail — useful when printing just the summary.
    pub fn summarized(mut self) -> Self {
        self.outcomes.clear();
        self
    }

    /// One-line human summary for CI logs.
    pub fn one_line(&self) -> String {
        format!(
            "{}  F1={:.2}  EM={:.2}  n={}  t={:.1}s",
            self.runner, self.overall_f1, self.overall_exact_match, self.total_questions, self.wall_seconds
        )
    }
}

/// Compare two reports and decide whether the new one would trigger the
/// CI regression gate. Returns `Ok(())` on pass, `Err(msg)` on regression.
pub fn regression_gate(
    previous: &BenchmarkReport,
    current: &BenchmarkReport,
    tolerance: f32,
) -> Result<(), String> {
    let delta = current.overall_f1 - previous.overall_f1;
    if delta < -tolerance {
        return Err(format!(
            "LoCoMo F1 regressed {:.2} points (prev {:.2} → now {:.2}, tolerance {:.2})",
            -delta, previous.overall_f1, current.overall_f1, tolerance
        ));
    }
    Ok(())
}

/// Fail when a semantic embedder scores no better than the hash embedder.
///
/// This is the check that would have caught the defect described in
/// `docs/H2-AUDIT-2026-07.md` §2.2: for four consecutive releases BGE and
/// `--hash-embed` produced *byte-identical* scores, because the retrieval
/// engine was returning nothing and the harness's own keyword fallback was
/// producing every answer. A one-sided "did F1 drop" gate cannot see that —
/// the number was stable, it just wasn't measuring the system.
///
/// A trained 384-dim encoder must beat a hash by a clear margin. If it does
/// not, retrieval is disconnected from the score regardless of how good the
/// score looks.
pub fn embedder_separation_gate(
    hash_report: &BenchmarkReport,
    real_report: &BenchmarkReport,
    min_margin: f32,
) -> Result<(), String> {
    let margin = real_report.overall_f1 - hash_report.overall_f1;
    if margin < min_margin {
        return Err(format!(
            "embedder separation {:.2} below minimum {:.2} (hash {:.2} vs real {:.2}) — \
             retrieval is probably not contributing to the score",
            margin, min_margin, hash_report.overall_f1, real_report.overall_f1
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_with_f1(f1: f32) -> BenchmarkReport {
        BenchmarkReport {
            runner: "t".into(),
            timestamp: "2026-07-22T00:00:00Z".into(),
            total_questions: 20,
            total_samples: 3,
            overall_f1: f1,
            overall_exact_match: 0.0,
            by_category: BTreeMap::new(),
            wall_seconds: 0.0,
            outcomes: Vec::new(),
        }
    }

    #[test]
    fn separation_gate_passes_when_real_embeddings_win() {
        assert!(embedder_separation_gate(&report_with_f1(48.3), &report_with_f1(70.5), 5.0).is_ok());
    }

    /// The exact historical failure: identical scores from a trained encoder
    /// and a hash must be treated as a broken benchmark, not a stable one.
    #[test]
    fn separation_gate_rejects_identical_scores() {
        let err = embedder_separation_gate(&report_with_f1(49.27), &report_with_f1(49.27), 5.0)
            .unwrap_err();
        assert!(err.contains("separation"), "got {err}");
    }

    #[test]
    fn separation_gate_rejects_a_thin_margin() {
        assert!(embedder_separation_gate(&report_with_f1(49.0), &report_with_f1(51.0), 5.0).is_err());
    }

    fn outcome(cat: Category, f1: f32, em: f32) -> QuestionOutcome {
        QuestionOutcome {
            sample_id: "s1".into(),
            question_id: "q".into(),
            category: cat,
            question: "?".into(),
            prediction: "p".into(),
            references: vec!["r".into()],
            f1,
            exact_match: em,
            latency_ms: 0,
            error: None,
        }
    }

    #[test]
    fn aggregates_overall_and_by_category() {
        let ds = LocomoDataset::default();
        let outcomes = vec![
            outcome(Category::SingleHop, 1.0, 1.0),
            outcome(Category::SingleHop, 0.5, 0.0),
            outcome(Category::Temporal, 0.0, 0.0),
        ];
        let r = BenchmarkReport::from_outcomes("test", &ds, outcomes, 1.0);
        assert_eq!(r.total_questions, 3);
        assert!((r.overall_f1 - 50.0).abs() < 1e-4);
        assert!((r.overall_exact_match - (1.0 / 3.0 * 100.0)).abs() < 1e-4);
        let sh = r.by_category.get("single_hop").unwrap();
        assert_eq!(sh.count, 2);
        assert!((sh.f1 - 75.0).abs() < 1e-4);
        let tp = r.by_category.get("temporal").unwrap();
        assert_eq!(tp.count, 1);
        assert_eq!(tp.f1, 0.0);
    }

    #[test]
    fn empty_report_has_zero_scores() {
        let ds = LocomoDataset::default();
        let r = BenchmarkReport::from_outcomes("test", &ds, vec![], 0.0);
        assert_eq!(r.total_questions, 0);
        assert_eq!(r.overall_f1, 0.0);
    }

    #[test]
    fn summarized_drops_outcomes() {
        let ds = LocomoDataset::default();
        let r = BenchmarkReport::from_outcomes(
            "test",
            &ds,
            vec![outcome(Category::SingleHop, 1.0, 1.0)],
            0.0,
        );
        assert_eq!(r.outcomes.len(), 1);
        let r = r.summarized();
        assert!(r.outcomes.is_empty());
    }

    #[test]
    fn regression_gate_passes_on_improvement() {
        let ds = LocomoDataset::default();
        let prev = BenchmarkReport::from_outcomes(
            "old",
            &ds,
            vec![outcome(Category::SingleHop, 0.8, 0.8)],
            0.0,
        );
        let curr = BenchmarkReport::from_outcomes(
            "new",
            &ds,
            vec![outcome(Category::SingleHop, 0.9, 0.9)],
            0.0,
        );
        assert!(regression_gate(&prev, &curr, 0.5).is_ok());
    }

    #[test]
    fn regression_gate_fails_on_big_drop() {
        let ds = LocomoDataset::default();
        let prev = BenchmarkReport::from_outcomes(
            "old",
            &ds,
            vec![outcome(Category::SingleHop, 0.9, 0.9)],
            0.0,
        );
        let curr = BenchmarkReport::from_outcomes(
            "new",
            &ds,
            vec![outcome(Category::SingleHop, 0.5, 0.5)],
            0.0,
        );
        assert!(regression_gate(&prev, &curr, 0.5).is_err());
    }

    #[test]
    fn regression_gate_respects_tolerance() {
        let ds = LocomoDataset::default();
        let prev = BenchmarkReport::from_outcomes(
            "old",
            &ds,
            vec![outcome(Category::SingleHop, 0.90, 0.9)],
            0.0,
        );
        // Drop 0.3 points — within 0.5 tolerance, should pass.
        let curr = BenchmarkReport::from_outcomes(
            "new",
            &ds,
            vec![outcome(Category::SingleHop, 0.897, 0.9)],
            0.0,
        );
        assert!(regression_gate(&prev, &curr, 0.5).is_ok());
    }
}
