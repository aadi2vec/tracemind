//! ACE Curator — the recursive meta-loop (charter Q4.11 + Q4.13).
//!
//! Pillar 7's claim is that the improvement loop itself improves. That only
//! means something if evidence from *past* optimisation runs changes how
//! the *next* run searches. The Curator reads the accumulated
//! [`RoundRecord`] history and emits a [`CuratorPrior`]: which reflection
//! directions have historically paid off on this user's data, and which
//! have repeatedly been rejected by the verifier.
//!
//! The spike's version bumped a candidate count and logged a string; the
//! prior was never consulted by the sampler. Here the prior is applied by
//! [`crate::optimize::Optimizer`] through `OptimizeConfig`, so amplified
//! directions genuinely receive more of the rollout budget.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::optimize::RoundRecord;

/// Evidence-weighted guidance for the next optimisation run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CuratorPrior {
    /// Diagnosis names whose proposals have historically been accepted.
    pub amplify: Vec<String>,
    /// Diagnosis names whose proposals have historically been rejected.
    pub suppress: Vec<String>,
    /// Share of evaluated candidates that were accepted, across history.
    pub acceptance_rate: f32,
    /// How much evidence backs this prior: 0 with no history, → 1 with lots.
    pub confidence: f32,
}

impl CuratorPrior {
    /// Whether a diagnosis should get extra rollout budget.
    pub fn is_amplified(&self, diagnosis: &str) -> bool {
        self.amplify.iter().any(|d| d == diagnosis)
    }

    /// Whether a diagnosis should be starved of budget.
    pub fn is_suppressed(&self, diagnosis: &str) -> bool {
        self.suppress.iter().any(|d| d == diagnosis)
    }
}

/// Per-diagnosis tally used to build the prior.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    evaluated: usize,
    accepted: usize,
    net_f1: f32,
}

/// Build a prior from optimisation history.
///
/// `min_samples` guards against concluding anything from one lucky round —
/// a direction needs repeated evidence before it is amplified or
/// suppressed.
pub fn curate(history: &[RoundRecord], min_samples: usize) -> CuratorPrior {
    if history.is_empty() {
        return CuratorPrior::default();
    }

    let mut tallies: HashMap<String, Tally> = HashMap::new();
    for h in history {
        let t = tallies.entry(h.diagnosis.clone()).or_default();
        t.evaluated += 1;
        if h.accepted {
            t.accepted += 1;
        }
        t.net_f1 += h.fixed.len() as f32 - h.broke.len() as f32;
    }

    let mut amplify = Vec::new();
    let mut suppress = Vec::new();
    for (name, t) in &tallies {
        if t.evaluated < min_samples {
            continue;
        }
        let rate = t.accepted as f32 / t.evaluated as f32;
        // A direction earns amplification by being accepted often *and*
        // fixing more instances than it breaks. Acceptance alone can be
        // satisfied by no-op mutations that neither help nor hurt.
        if rate >= 0.5 && t.net_f1 > 0.0 {
            amplify.push(name.clone());
        } else if rate == 0.0 || t.net_f1 < 0.0 {
            suppress.push(name.clone());
        }
    }
    amplify.sort();
    suppress.sort();

    let evaluated: usize = tallies.values().map(|t| t.evaluated).sum();
    let accepted: usize = tallies.values().map(|t| t.accepted).sum();
    let acceptance_rate = if evaluated == 0 {
        0.0
    } else {
        accepted as f32 / evaluated as f32
    };

    // Confidence saturates as history accumulates; 30 evaluations is
    // treated as "enough to trust the shape of the prior".
    let confidence = (evaluated as f32 / 30.0).min(1.0);

    CuratorPrior {
        amplify,
        suppress,
        acceptance_rate,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn rec(diagnosis: &str, accepted: bool, fixed: usize, broke: usize) -> RoundRecord {
        RoundRecord {
            round: 0,
            candidate_id: Uuid::new_v4(),
            parent_id: None,
            diagnosis: diagnosis.to_string(),
            rationale: "r".into(),
            mean_f1: 50.0,
            mean_em: 25.0,
            accepted,
            reason: String::new(),
            fixed: (0..fixed).map(|i| format!("f{i}")).collect(),
            broke: (0..broke).map(|i| format!("b{i}")).collect(),
        }
    }

    #[test]
    fn empty_history_yields_empty_prior() {
        let p = curate(&[], 2);
        assert!(p.amplify.is_empty() && p.suppress.is_empty());
        assert_eq!(p.confidence, 0.0);
    }

    #[test]
    fn consistently_accepted_direction_is_amplified() {
        let h = vec![
            rec("RankedTooLow", true, 2, 0),
            rec("RankedTooLow", true, 1, 0),
        ];
        let p = curate(&h, 2);
        assert!(p.is_amplified("RankedTooLow"), "{p:?}");
    }

    #[test]
    fn never_accepted_direction_is_suppressed() {
        let h = vec![
            rec("PrecisionStarved", false, 0, 1),
            rec("PrecisionStarved", false, 0, 2),
        ];
        let p = curate(&h, 2);
        assert!(p.is_suppressed("PrecisionStarved"), "{p:?}");
    }

    /// A direction that is accepted but breaks more than it fixes must not
    /// be amplified — acceptance alone is satisfiable by no-op mutations.
    #[test]
    fn accepted_but_net_negative_is_not_amplified() {
        let h = vec![
            rec("Explore", true, 0, 2),
            rec("Explore", true, 1, 3),
        ];
        let p = curate(&h, 2);
        assert!(!p.is_amplified("Explore"), "{p:?}");
        assert!(p.is_suppressed("Explore"), "{p:?}");
    }

    #[test]
    fn thin_evidence_is_ignored() {
        let h = vec![rec("RankedTooLow", true, 5, 0)];
        let p = curate(&h, 3);
        assert!(p.amplify.is_empty(), "one sample must not drive a prior");
    }

    #[test]
    fn confidence_grows_with_history() {
        let few: Vec<RoundRecord> = (0..3).map(|_| rec("Explore", true, 1, 0)).collect();
        let many: Vec<RoundRecord> = (0..40).map(|_| rec("Explore", true, 1, 0)).collect();
        assert!(curate(&few, 2).confidence < curate(&many, 2).confidence);
        assert_eq!(curate(&many, 2).confidence, 1.0);
    }

    #[test]
    fn acceptance_rate_is_reported() {
        let h = vec![
            rec("A", true, 1, 0),
            rec("A", false, 0, 0),
            rec("B", true, 1, 0),
            rec("B", false, 0, 0),
        ];
        assert!((curate(&h, 2).acceptance_rate - 0.5).abs() < 1e-5);
    }
}
