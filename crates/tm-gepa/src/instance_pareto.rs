//! Per-instance Pareto frontier — the mechanism that makes GEPA GEPA.
//!
//! Aggregate-only selection (keep the candidate with the best mean F1)
//! collapses the search into hill-climbing and discards the candidate that
//! is the *only* one solving some hard instance. GEPA's contribution is to
//! treat each evaluation instance as its own objective: a candidate stays
//! in the archive if it is the best-so-far on **any** instance, even when
//! its aggregate is mediocre. That preserved diversity is what lets later
//! mutations recombine partial wins instead of getting stuck.
//!
//! This module tracks, for every instance, the best score achieved by any
//! candidate, and selects parents in proportion to how many instances a
//! candidate currently "owns".

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::retrieval_policy::RetrievalPolicy;
use crate::scorer::ScoreReport;

/// One archived candidate with its full per-instance score vector.
#[derive(Debug, Clone)]
pub struct ArchivedCandidate {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub policy: RetrievalPolicy,
    /// instance id -> f1
    pub scores: HashMap<String, f32>,
    pub mean_f1: f32,
    pub mean_em: f32,
    pub latency_ms: u64,
    /// Instances on which this candidate is (tied for) best in the archive.
    pub owned_instances: usize,
}

/// Archive over the per-instance frontier.
#[derive(Debug, Default)]
pub struct InstanceParetoArchive {
    pub candidates: Vec<ArchivedCandidate>,
    /// instance id -> best score seen from any candidate.
    best_per_instance: HashMap<String, f32>,
}

impl InstanceParetoArchive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a candidate. Returns true when it advances the frontier on at
    /// least one instance (or the archive was empty).
    pub fn try_add(
        &mut self,
        id: Uuid,
        parent_id: Option<Uuid>,
        policy: RetrievalPolicy,
        report: &ScoreReport,
    ) -> bool {
        let scores: HashMap<String, f32> = report
            .instances
            .iter()
            .map(|i| (i.id.clone(), i.f1))
            .collect();

        let first = self.candidates.is_empty();
        let mut advances = first;
        for (inst, score) in &scores {
            match self.best_per_instance.get(inst) {
                Some(best) if *score <= *best + 1e-6 => {}
                _ => advances = true,
            }
        }

        // A candidate that sets no new per-instance record can still be the
        // best *aggregate* — it recombines wins that earlier candidates
        // achieved separately. That is precisely the candidate that ships,
        // so admitting only per-instance record-setters would discard the
        // winner. Keep both: the frontier for diversity, the aggregate
        // champion for deployment.
        let beats_best_mean = self
            .candidates
            .iter()
            .map(|c| c.mean_f1)
            .fold(f32::NEG_INFINITY, f32::max)
            < report.mean_f1 - 1e-6;

        if !advances && !beats_best_mean {
            return false;
        }

        for (inst, score) in &scores {
            let e = self.best_per_instance.entry(inst.clone()).or_insert(*score);
            if *score > *e {
                *e = *score;
            }
        }

        self.candidates.push(ArchivedCandidate {
            id,
            parent_id,
            policy,
            scores,
            mean_f1: report.mean_f1,
            mean_em: report.mean_em,
            latency_ms: report.latency_ms,
            owned_instances: 0,
        });
        self.recompute_ownership();
        true
    }

    /// Recompute, for each candidate, how many instances it is best on.
    fn recompute_ownership(&mut self) {
        let best = self.best_per_instance.clone();
        for c in &mut self.candidates {
            c.owned_instances = c
                .scores
                .iter()
                .filter(|(inst, score)| {
                    best.get(*inst)
                        .map(|b| **score >= *b - 1e-6)
                        .unwrap_or(false)
                })
                .count();
        }
    }

    /// Candidates eligible to be mutated, best-owning first.
    ///
    /// Selecting by instance ownership rather than by mean is the point:
    /// a candidate that solves one otherwise-unsolved question is a more
    /// valuable parent than one that is marginally better on average.
    pub fn parents(&self, limit: usize) -> Vec<&ArchivedCandidate> {
        let mut by_ownership: Vec<&ArchivedCandidate> = self.candidates.iter().collect();
        by_ownership.sort_by(|a, b| {
            b.owned_instances.cmp(&a.owned_instances).then_with(|| {
                b.mean_f1
                    .partial_cmp(&a.mean_f1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        by_ownership.into_iter().take(limit).collect()
    }

    /// The candidate with the highest aggregate F1 — what actually ships.
    pub fn best_aggregate(&self) -> Option<&ArchivedCandidate> {
        self.candidates.iter().max_by(|a, b| {
            a.mean_f1
                .partial_cmp(&b.mean_f1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Theoretical ceiling: mean of the best score achieved on each
    /// instance by *any* candidate. The gap between this and
    /// `best_aggregate` is how much a recombination could still win.
    pub fn frontier_ceiling(&self) -> f32 {
        if self.best_per_instance.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.best_per_instance.values().sum();
        sum / self.best_per_instance.len() as f32 * 100.0
    }

    /// Instances no candidate has ever solved. These are what reflection
    /// should target next.
    pub fn unsolved(&self, threshold: f32) -> HashSet<String> {
        self.best_per_instance
            .iter()
            .filter(|(_, v)| **v < threshold)
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::InstanceScore;

    fn report(scores: &[(&str, f32)]) -> ScoreReport {
        ScoreReport::from_instances(
            scores
                .iter()
                .map(|(id, f)| InstanceScore {
                    id: id.to_string(),
                    f1: *f,
                    exact_match: if *f >= 1.0 { 1.0 } else { 0.0 },
                    feedback: String::new(),
                })
                .collect(),
            10,
        )
    }

    #[test]
    fn first_candidate_is_always_added() {
        let mut a = InstanceParetoArchive::new();
        assert!(a.try_add(
            Uuid::new_v4(),
            None,
            RetrievalPolicy::default(),
            &report(&[("q1", 0.5)])
        ));
    }

    #[test]
    fn strictly_worse_candidate_is_rejected() {
        let mut a = InstanceParetoArchive::new();
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 0.9), ("q2", 0.9)]));
        let added = a.try_add(
            Uuid::new_v4(),
            None,
            RetrievalPolicy::default(),
            &report(&[("q1", 0.5), ("q2", 0.5)]),
        );
        assert!(!added, "dominated candidate must not enter the archive");
    }

    /// The defining GEPA behaviour: a candidate with a *worse average* is
    /// kept when it is the only one solving some instance.
    #[test]
    fn worse_average_but_unique_win_is_kept() {
        let mut a = InstanceParetoArchive::new();
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 0.8)]));
        // mean 0.55 < 0.9, but it is the only candidate scoring on q2's peak.
        let added = a.try_add(
            Uuid::new_v4(),
            None,
            RetrievalPolicy::default(),
            &report(&[("q1", 0.1), ("q2", 1.0)]),
        );
        assert!(added, "unique per-instance win must be preserved");
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn ownership_counts_best_instances() {
        let mut a = InstanceParetoArchive::new();
        let id1 = Uuid::new_v4();
        a.try_add(id1, None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 0.2)]));
        let id2 = Uuid::new_v4();
        a.try_add(id2, None, RetrievalPolicy::default(), &report(&[("q1", 0.3), ("q2", 1.0)]));
        let c1 = a.candidates.iter().find(|c| c.id == id1).unwrap();
        let c2 = a.candidates.iter().find(|c| c.id == id2).unwrap();
        assert_eq!(c1.owned_instances, 1);
        assert_eq!(c2.owned_instances, 1);
    }

    #[test]
    fn frontier_ceiling_exceeds_best_single_candidate() {
        let mut a = InstanceParetoArchive::new();
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 0.0)]));
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 0.0), ("q2", 1.0)]));
        // Each candidate averages 50; the per-instance frontier is 100.
        assert!((a.frontier_ceiling() - 100.0).abs() < 1e-3, "{}", a.frontier_ceiling());
        assert!((a.best_aggregate().unwrap().mean_f1 - 50.0).abs() < 1e-3);
    }

    #[test]
    fn unsolved_reports_instances_below_threshold() {
        let mut a = InstanceParetoArchive::new();
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 0.1)]));
        let u = a.unsolved(0.5);
        assert!(u.contains("q2"));
        assert!(!u.contains("q1"));
    }

    /// Regression: a candidate that recombines existing per-instance wins
    /// into the best aggregate sets no new record, but is the one that
    /// ships. It must not be discarded as "dominated".
    #[test]
    fn best_aggregate_is_kept_even_without_a_new_instance_record() {
        let mut a = InstanceParetoArchive::new();
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 0.0)]));
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 0.0), ("q2", 1.0)]));
        // Ties both per-instance maxima on neither, but averages higher
        // than either parent (0.9 vs 0.5).
        let combined = Uuid::new_v4();
        let added = a.try_add(
            combined,
            None,
            RetrievalPolicy::default(),
            &report(&[("q1", 0.9), ("q2", 0.9)]),
        );
        assert!(added, "best-aggregate candidate must be archived");
        assert_eq!(a.best_aggregate().unwrap().id, combined);
    }

    #[test]
    fn parents_are_ordered_by_ownership() {
        let mut a = InstanceParetoArchive::new();
        let winner = Uuid::new_v4();
        a.try_add(winner, None, RetrievalPolicy::default(), &report(&[("q1", 1.0), ("q2", 1.0)]));
        a.try_add(Uuid::new_v4(), None, RetrievalPolicy::default(), &report(&[("q1", 0.1), ("q2", 0.1), ("q3", 0.4)]));
        assert_eq!(a.parents(1)[0].id, winner);
    }
}
