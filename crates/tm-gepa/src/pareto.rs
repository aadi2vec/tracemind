use serde::{Serialize, Deserialize};
use crate::policy::PolicyCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParetoAxis {
    F1,
    Latency,           // lower is better — stored as 1/(1+latency_ms/1000)
    ContradictionRate, // lower is better — stored as 1 - rate
    MultiHopF1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParetoScore {
    pub axis: ParetoAxis,
    pub value: f32,  // always higher = better (invert latency/contradiction)
}

/// The Pareto archive: keeps non-dominated candidates.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParetoArchive {
    pub candidates: Vec<PolicyCandidate>,
    pub axes: Vec<ParetoAxis>,
}

impl ParetoArchive {
    pub fn new(axes: Vec<ParetoAxis>) -> Self {
        Self { candidates: vec![], axes }
    }

    pub fn default_axes() -> Vec<ParetoAxis> {
        vec![
            ParetoAxis::F1,
            ParetoAxis::Latency,
            ParetoAxis::ContradictionRate,
            ParetoAxis::MultiHopF1,
        ]
    }

    /// Try to add a candidate. Returns true if it was added (non-dominated or archive empty).
    pub fn try_add(&mut self, mut candidate: PolicyCandidate) -> bool {
        if candidate.pareto_scores.is_empty() {
            // No scores yet — accept provisionally
            candidate.accepted = true;
            self.candidates.push(candidate);
            return true;
        }

        // Check if dominated by any existing candidate
        let dominated = self.candidates.iter().any(|existing| {
            existing.accepted && dominates(&existing.pareto_scores, &candidate.pareto_scores)
        });

        if dominated {
            candidate.accepted = false;
            self.candidates.push(candidate);
            false
        } else {
            // Remove candidates now dominated by the new one
            for c in &mut self.candidates {
                if c.accepted && dominates(&candidate.pareto_scores, &c.pareto_scores) {
                    c.accepted = false;
                }
            }
            candidate.accepted = true;
            self.candidates.push(candidate);
            true
        }
    }

    pub fn best_by_axis(&self, axis: ParetoAxis) -> Option<&PolicyCandidate> {
        let axis_idx = self.axes.iter().position(|a| *a == axis)?;
        self.candidates
            .iter()
            .filter(|c| c.accepted && c.pareto_scores.len() > axis_idx)
            .max_by(|a, b| {
                a.pareto_scores[axis_idx]
                    .partial_cmp(&b.pareto_scores[axis_idx])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    pub fn accepted_count(&self) -> usize {
        self.candidates.iter().filter(|c| c.accepted).count()
    }
}

fn dominates(a: &[f32], b: &[f32]) -> bool {
    if a.len() != b.len() { return false; }
    // a dominates b if a is >= b on all axes and > b on at least one
    let all_ge = a.iter().zip(b).all(|(ai, bi)| ai >= bi);
    let any_gt = a.iter().zip(b).any(|(ai, bi)| ai > bi);
    all_ge && any_gt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyCandidate;

    fn make_candidate(scores: Vec<f32>) -> PolicyCandidate {
        let mut c = PolicyCandidate::seed(5);
        c.pareto_scores = scores;
        c
    }

    #[test]
    fn dominates_basic() {
        assert!(dominates(&[0.8, 0.7], &[0.6, 0.6]));
        assert!(!dominates(&[0.6, 0.6], &[0.8, 0.7]));
        assert!(!dominates(&[0.8, 0.6], &[0.6, 0.8])); // neither dominates
        assert!(!dominates(&[0.8, 0.8], &[0.8, 0.8])); // equal, not dominating
    }

    #[test]
    fn dominates_mismatched_lengths() {
        assert!(!dominates(&[0.8, 0.7], &[0.6]));
    }

    #[test]
    fn pareto_archive_accepts_non_dominated() {
        let mut archive = ParetoArchive::new(ParetoArchive::default_axes());
        // Two candidates that don't dominate each other
        let c1 = make_candidate(vec![0.9, 0.5, 0.5, 0.5]);
        let c2 = make_candidate(vec![0.5, 0.9, 0.5, 0.5]);
        assert!(archive.try_add(c1));
        assert!(archive.try_add(c2));
        assert_eq!(archive.accepted_count(), 2);
    }

    #[test]
    fn pareto_archive_rejects_dominated() {
        let mut archive = ParetoArchive::new(ParetoArchive::default_axes());
        let dominant = make_candidate(vec![0.9, 0.9, 0.9, 0.9]);
        let weak = make_candidate(vec![0.5, 0.5, 0.5, 0.5]);
        assert!(archive.try_add(dominant));
        assert!(!archive.try_add(weak));
        assert_eq!(archive.accepted_count(), 1);
    }

    #[test]
    fn pareto_archive_evicts_when_dominated() {
        let mut archive = ParetoArchive::new(ParetoArchive::default_axes());
        let weak = make_candidate(vec![0.5, 0.5, 0.5, 0.5]);
        let dominant = make_candidate(vec![0.9, 0.9, 0.9, 0.9]);
        assert!(archive.try_add(weak));
        assert_eq!(archive.accepted_count(), 1);
        assert!(archive.try_add(dominant));
        // weak should now be evicted
        assert_eq!(archive.accepted_count(), 1);
    }

    #[test]
    fn pareto_archive_accepts_no_scores_provisionally() {
        let mut archive = ParetoArchive::new(ParetoArchive::default_axes());
        let c = PolicyCandidate::seed(5); // pareto_scores is empty
        assert!(archive.try_add(c));
        assert_eq!(archive.accepted_count(), 1);
    }

    #[test]
    fn best_by_axis_f1() {
        let mut archive = ParetoArchive::new(ParetoArchive::default_axes());
        let c1 = make_candidate(vec![0.9, 0.5, 0.5, 0.5]);
        let c2 = make_candidate(vec![0.5, 0.9, 0.5, 0.5]);
        archive.try_add(c1.clone());
        archive.try_add(c2);
        let best = archive.best_by_axis(ParetoAxis::F1).unwrap();
        assert_eq!(best.pareto_scores[0], 0.9);
    }

    #[test]
    fn best_by_axis_returns_none_for_empty_archive() {
        let archive = ParetoArchive::new(ParetoArchive::default_axes());
        assert!(archive.best_by_axis(ParetoAxis::F1).is_none());
    }

    #[test]
    fn pareto_score_serializes() {
        let s = ParetoScore { axis: ParetoAxis::MultiHopF1, value: 0.75 };
        let json = serde_json::to_string(&s).unwrap();
        let s2: ParetoScore = serde_json::from_str(&json).unwrap();
        assert_eq!(s.axis, s2.axis);
        assert!((s.value - s2.value).abs() < 1e-6);
    }
}
