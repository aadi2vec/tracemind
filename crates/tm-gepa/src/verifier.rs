//! Anchor-eval verifier gate (charter Q4.3).
//!
//! A candidate policy is promoted only if *executing* it against the anchor
//! set does not regress the primary metric beyond `max_f1_regression`.
//!
//! The previous implementation read `candidate.pareto_scores.first()` — the
//! score the candidate asserted about itself — and compared it to the
//! baseline without running anything. With `max_f1_regression = 2.0` against
//! F1 values in [0,1] that predicate was true for every possible input, so
//! the gate reported success unconditionally. A gate that cannot fail is
//! worse than no gate, because it certifies that self-improvement is
//! working while nothing is being checked. The gate now takes a
//! [`PolicyScorer`] and runs it.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::retrieval_policy::RetrievalPolicy;
use crate::scorer::{PolicyScorer, ScoreReport};

#[derive(Debug, Clone)]
pub struct VerifierConfig {
    /// Maximum tolerated drop in mean F1 (0–100 scale) versus baseline.
    pub max_f1_regression: f32,
    /// Maximum tolerated increase in evaluation latency, as a ratio of
    /// baseline (1.5 = may be up to 50% slower).
    pub max_latency_ratio: f32,
}

impl Default for VerifierConfig {
    fn default() -> Self {
        Self {
            max_f1_regression: 0.5,
            max_latency_ratio: 1.5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEvalResult {
    pub candidate_id: Uuid,
    pub baseline_f1: f32,
    pub candidate_f1: f32,
    pub candidate_em: f32,
    /// Positive = the candidate got worse.
    pub regression: f32,
    pub passed: bool,
    /// Why the gate rejected the candidate, when it did.
    pub reason: String,
    /// Instance ids the candidate newly fixed relative to baseline.
    pub fixed: Vec<String>,
    /// Instance ids the candidate newly broke relative to baseline.
    pub broke: Vec<String>,
}

pub struct VerifierGate {
    config: VerifierConfig,
}

impl VerifierGate {
    pub fn new(config: VerifierConfig) -> Self {
        Self { config }
    }

    /// Execute `policy` against the anchor set and decide whether it may be
    /// promoted, given a `baseline` report to compare against.
    pub fn evaluate(
        &self,
        candidate_id: Uuid,
        policy: &RetrievalPolicy,
        baseline: &ScoreReport,
        scorer: &mut dyn PolicyScorer,
    ) -> AnchorEvalResult {
        let report = scorer.score(policy);
        self.judge(candidate_id, baseline, &report)
    }

    /// Compare an already-computed candidate report against baseline.
    /// Split out so the loop can reuse a report it has already paid for.
    pub fn judge(
        &self,
        candidate_id: Uuid,
        baseline: &ScoreReport,
        candidate: &ScoreReport,
    ) -> AnchorEvalResult {
        let regression = baseline.mean_f1 - candidate.mean_f1;

        let latency_ratio = if baseline.latency_ms == 0 {
            1.0
        } else {
            candidate.latency_ms as f32 / baseline.latency_ms as f32
        };

        let mut passed = true;
        let mut reason = String::new();
        if regression > self.config.max_f1_regression {
            passed = false;
            reason = format!(
                "F1 regressed {:.2} (limit {:.2}): {:.2} -> {:.2}",
                regression, self.config.max_f1_regression, baseline.mean_f1, candidate.mean_f1
            );
        } else if latency_ratio > self.config.max_latency_ratio {
            passed = false;
            reason = format!(
                "latency ratio {:.2}x exceeds limit {:.2}x",
                latency_ratio, self.config.max_latency_ratio
            );
        }

        // Per-instance deltas — what the reflection step reads to learn
        // which change helped which kind of question.
        let mut fixed = Vec::new();
        let mut broke = Vec::new();
        for c in &candidate.instances {
            if let Some(b) = baseline.instances.iter().find(|b| b.id == c.id) {
                if c.f1 > b.f1 + 1e-6 {
                    fixed.push(c.id.clone());
                } else if c.f1 + 1e-6 < b.f1 {
                    broke.push(c.id.clone());
                }
            }
        }

        AnchorEvalResult {
            candidate_id,
            baseline_f1: baseline.mean_f1,
            candidate_f1: candidate.mean_f1,
            candidate_em: candidate.mean_em,
            regression,
            passed,
            reason,
            fixed,
            broke,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::InstanceScore;

    fn inst(id: &str, f1: f32) -> InstanceScore {
        InstanceScore {
            id: id.to_string(),
            f1,
            exact_match: if f1 >= 1.0 { 1.0 } else { 0.0 },
            feedback: String::new(),
        }
    }

    fn report(scores: &[(&str, f32)], latency_ms: u64) -> ScoreReport {
        ScoreReport::from_instances(
            scores.iter().map(|(id, f)| inst(id, *f)).collect(),
            latency_ms,
        )
    }

    /// A scorer that returns a fixed report — lets the gate be tested
    /// without a retrieval stack.
    struct FixedScorer(ScoreReport);
    impl PolicyScorer for FixedScorer {
        fn score(&mut self, _p: &RetrievalPolicy) -> ScoreReport {
            self.0.clone()
        }
        fn anchor_count(&self) -> usize {
            self.0.instances.len()
        }
    }

    #[test]
    fn improvement_passes() {
        let base = report(&[("a", 0.5), ("b", 0.5)], 100);
        let cand = report(&[("a", 0.9), ("b", 0.9)], 100);
        let gate = VerifierGate::new(VerifierConfig::default());
        let r = gate.judge(Uuid::new_v4(), &base, &cand);
        assert!(r.passed);
        assert!(r.regression < 0.0);
        assert_eq!(r.fixed.len(), 2);
    }

    /// The regression test the old gate could not express: a candidate that
    /// is genuinely worse must be rejected.
    #[test]
    fn large_regression_is_rejected() {
        let base = report(&[("a", 1.0), ("b", 1.0)], 100);
        let cand = report(&[("a", 0.0), ("b", 0.0)], 100);
        let gate = VerifierGate::new(VerifierConfig::default());
        let r = gate.judge(Uuid::new_v4(), &base, &cand);
        assert!(!r.passed, "gate must reject a 100-point regression");
        assert_eq!(r.broke.len(), 2);
        assert!(r.reason.contains("F1 regressed"), "reason = {}", r.reason);
    }

    #[test]
    fn small_regression_within_tolerance_passes() {
        let base = report(&[("a", 1.0), ("b", 1.0)], 100);
        // 0.2 F1 point drop on the 0-100 scale, under the 0.5 limit.
        let cand = report(&[("a", 0.998), ("b", 1.0)], 100);
        let gate = VerifierGate::new(VerifierConfig::default());
        let r = gate.judge(Uuid::new_v4(), &base, &cand);
        assert!(r.passed, "reason = {}", r.reason);
    }

    #[test]
    fn latency_blowup_is_rejected() {
        let base = report(&[("a", 1.0)], 100);
        let cand = report(&[("a", 1.0)], 1000);
        let gate = VerifierGate::new(VerifierConfig::default());
        let r = gate.judge(Uuid::new_v4(), &base, &cand);
        assert!(!r.passed);
        assert!(r.reason.contains("latency"), "reason = {}", r.reason);
    }

    #[test]
    fn evaluate_runs_the_scorer() {
        let base = report(&[("a", 1.0)], 100);
        let mut scorer = FixedScorer(report(&[("a", 0.0)], 100));
        let gate = VerifierGate::new(VerifierConfig::default());
        let r = gate.evaluate(
            Uuid::new_v4(),
            &RetrievalPolicy::default(),
            &base,
            &mut scorer,
        );
        // The verdict must come from the scorer's output, not the candidate.
        assert!(!r.passed);
        assert_eq!(r.candidate_f1, 0.0);
    }
}
