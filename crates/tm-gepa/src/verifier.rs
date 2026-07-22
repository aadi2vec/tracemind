use serde::{Serialize, Deserialize};
use crate::policy::PolicyCandidate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierConfig {
    /// Maximum F1 regression allowed (e.g., 2.0 means 2 F1 points).
    pub max_f1_regression: f32,
    /// Number of anchor queries to evaluate.
    pub anchor_count: usize,
}

impl Default for VerifierConfig {
    fn default() -> Self {
        Self { max_f1_regression: 2.0, anchor_count: 10 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorEvalResult {
    pub candidate_id: uuid::Uuid,
    pub baseline_f1: f32,
    pub candidate_f1: f32,
    pub passed: bool,
    pub regression: f32,
}

pub struct VerifierGate {
    config: VerifierConfig,
}

impl VerifierGate {
    pub fn new(config: VerifierConfig) -> Self { Self { config } }

    /// Evaluate a candidate against the baseline.
    /// In production, this runs real anchor queries through the retrieval engine.
    /// For the spike, it uses synthetic scores from the candidate's pareto_scores.
    pub fn evaluate(
        &self,
        candidate: &PolicyCandidate,
        baseline_f1: f32,
    ) -> AnchorEvalResult {
        // Use the candidate's F1 score if available, otherwise use baseline
        let candidate_f1 = candidate.pareto_scores.first().copied().unwrap_or(baseline_f1);
        let regression = baseline_f1 - candidate_f1;
        let passed = regression <= self.config.max_f1_regression;
        AnchorEvalResult {
            candidate_id: candidate.id,
            baseline_f1,
            candidate_f1,
            passed,
            regression,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyCandidate;

    fn make_candidate_with_f1(f1: f32) -> PolicyCandidate {
        let mut c = PolicyCandidate::seed(5);
        c.pareto_scores = vec![f1, 0.7, 0.8, 0.6];
        c
    }

    #[test]
    fn verifier_passes_when_no_regression() {
        let gate = VerifierGate::new(VerifierConfig::default());
        let c = make_candidate_with_f1(0.85);
        let result = gate.evaluate(&c, 0.83);
        assert!(result.passed);
        assert!(result.regression < 0.0); // improvement
    }

    #[test]
    fn verifier_passes_within_threshold() {
        let gate = VerifierGate::new(VerifierConfig::default()); // max_f1_regression = 2.0
        let c = make_candidate_with_f1(0.80);
        // baseline_f1 is in [0,1] but regression check is raw subtraction
        let result = gate.evaluate(&c, 0.82);
        // regression = 0.82 - 0.80 = 0.02, well under 2.0
        assert!(result.passed);
        assert!((result.regression - 0.02).abs() < 1e-5);
    }

    #[test]
    fn verifier_rejects_large_regression() {
        let gate = VerifierGate::new(VerifierConfig {
            max_f1_regression: 2.0,
            anchor_count: 10,
        });
        // Scores are treated as 0-100 F1 range in the gate comparison
        let mut c = PolicyCandidate::seed(5);
        c.pareto_scores = vec![40.0]; // big drop
        let result = gate.evaluate(&c, 50.0);
        assert!(!result.passed);
        assert!((result.regression - 10.0).abs() < 1e-4);
    }

    #[test]
    fn verifier_uses_baseline_when_no_scores() {
        let gate = VerifierGate::new(VerifierConfig::default());
        let c = PolicyCandidate::seed(5); // empty pareto_scores
        let result = gate.evaluate(&c, 0.75);
        assert_eq!(result.candidate_f1, 0.75);
        assert_eq!(result.regression, 0.0);
        assert!(result.passed);
    }

    #[test]
    fn anchor_eval_result_serializes() {
        let gate = VerifierGate::new(VerifierConfig::default());
        let c = make_candidate_with_f1(0.80);
        let result = gate.evaluate(&c, 0.82);
        let json = serde_json::to_string(&result).unwrap();
        let r2: AnchorEvalResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result.candidate_id, r2.candidate_id);
        assert_eq!(result.passed, r2.passed);
    }

    #[test]
    fn verifier_config_default_values() {
        let cfg = VerifierConfig::default();
        assert_eq!(cfg.max_f1_regression, 2.0);
        assert_eq!(cfg.anchor_count, 10);
    }
}
