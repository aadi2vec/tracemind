//! The evaluation interface the GEPA loop optimises against.
//!
//! GEPA's mechanism depends on two things this interface is built to
//! supply:
//!
//! 1. **Per-instance scores**, not just an aggregate. The Pareto archive
//!    keeps candidates that win on *any* instance, which is what preserves
//!    diversity and stops the search collapsing into a local optimum. An
//!    aggregate F1 cannot express "this candidate is the only one that gets
//!    question 14 right".
//! 2. **Textual feedback**, not just a float. The reflection step needs to
//!    read *why* an instance failed in order to propose a better artifact.
//!    A scalar carries no such signal.
//!
//! Implementations live outside this crate — `tm-bench-locomo` provides one
//! backed by the real ingest + retrieval + answer pipeline.

use crate::retrieval_policy::RetrievalPolicy;

/// Outcome for a single evaluation instance.
#[derive(Debug, Clone)]
pub struct InstanceScore {
    /// Stable identifier for the instance (question id).
    pub id: String,
    /// Primary metric in [0,1].
    pub f1: f32,
    /// Exact match in {0,1}.
    pub exact_match: f32,
    /// Human/LLM-readable description of what went wrong. Empty when the
    /// instance passed. This is what the reflection step consumes.
    pub feedback: String,
}

/// Result of evaluating one policy over the whole anchor set.
#[derive(Debug, Clone)]
pub struct ScoreReport {
    pub instances: Vec<InstanceScore>,
    /// Mean F1 over instances, expressed on the 0–100 scale used by the
    /// LoCoMo harness and the charter's success metrics.
    pub mean_f1: f32,
    pub mean_em: f32,
    /// Wall time for the evaluation, a Pareto axis in its own right.
    pub latency_ms: u64,
}

impl ScoreReport {
    pub fn from_instances(instances: Vec<InstanceScore>, latency_ms: u64) -> Self {
        let n = instances.len().max(1) as f32;
        let mean_f1 = instances.iter().map(|i| i.f1).sum::<f32>() / n * 100.0;
        let mean_em = instances.iter().map(|i| i.exact_match).sum::<f32>() / n * 100.0;
        Self {
            instances,
            mean_f1,
            mean_em,
            latency_ms,
        }
    }

    /// Instances that scored below `threshold`, worst first. These are the
    /// examples the reflection step should read.
    pub fn failures(&self, threshold: f32) -> Vec<&InstanceScore> {
        let mut out: Vec<&InstanceScore> =
            self.instances.iter().filter(|i| i.f1 < threshold).collect();
        out.sort_by(|a, b| a.f1.partial_cmp(&b.f1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// Executes a candidate policy against a fixed anchor set.
///
/// `&mut self` because real implementations own an ingest+retrieval stack
/// and mutate it to apply the policy under test.
pub trait PolicyScorer {
    fn score(&mut self, policy: &RetrievalPolicy) -> ScoreReport;
    /// Number of instances in the anchor set.
    fn anchor_count(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inst(id: &str, f1: f32) -> InstanceScore {
        InstanceScore {
            id: id.to_string(),
            f1,
            exact_match: if f1 >= 1.0 { 1.0 } else { 0.0 },
            feedback: if f1 < 1.0 { "missed".into() } else { String::new() },
        }
    }

    #[test]
    fn mean_f1_is_on_0_100_scale() {
        let r = ScoreReport::from_instances(vec![inst("a", 1.0), inst("b", 0.0)], 5);
        assert!((r.mean_f1 - 50.0).abs() < 1e-3, "got {}", r.mean_f1);
    }

    #[test]
    fn mean_em_counts_only_exact() {
        let r = ScoreReport::from_instances(vec![inst("a", 1.0), inst("b", 0.5)], 5);
        assert!((r.mean_em - 50.0).abs() < 1e-3, "got {}", r.mean_em);
    }

    #[test]
    fn failures_are_worst_first() {
        let r = ScoreReport::from_instances(
            vec![inst("a", 0.9), inst("b", 0.1), inst("c", 0.5)],
            1,
        );
        let f = r.failures(1.0);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0].id, "b");
        assert_eq!(f[2].id, "a");
    }

    #[test]
    fn empty_report_does_not_divide_by_zero() {
        let r = ScoreReport::from_instances(vec![], 0);
        assert_eq!(r.mean_f1, 0.0);
    }
}
