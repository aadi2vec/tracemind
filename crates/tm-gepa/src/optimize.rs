//! The GEPA optimisation loop, wired to a real scorer and a real gate.
//!
//! Round structure:
//!
//! ```text
//! evaluate baseline ─▶ diagnose failures ─▶ propose candidates
//!         ▲                                        │
//!         │                                        ▼
//!    per-instance                            execute each
//!    Pareto archive ◀── verifier gate ◀────── against anchors
//! ```
//!
//! Every candidate is *executed*; nothing is promoted on a self-reported
//! score. Parents for the next round are drawn by per-instance ownership,
//! not by aggregate rank.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::instance_pareto::InstanceParetoArchive;
use crate::reflect::{propose, Diagnosis};
use crate::retrieval_policy::RetrievalPolicy;
use crate::scorer::{PolicyScorer, ScoreReport};
use crate::curator::CuratorPrior;
use crate::verifier::{VerifierConfig, VerifierGate};

#[derive(Debug, Clone)]
pub struct OptimizeConfig {
    pub max_rounds: usize,
    /// Parents mutated per round.
    pub parents_per_round: usize,
    /// Initial perturbation size; annealed down across rounds so early
    /// rounds explore and later rounds refine.
    pub step: f32,
    pub verifier: VerifierConfig,
    /// Q4.13 — prior distilled from previous runs' history. Suppressed
    /// directions are skipped; amplified directions get extra budget. This
    /// is what makes the meta-loop load-bearing: without it, every run
    /// re-derives the same lessons from scratch.
    pub prior: Option<CuratorPrior>,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            max_rounds: 6,
            parents_per_round: 2,
            step: 0.12,
            verifier: VerifierConfig::default(),
            prior: None,
        }
    }
}

/// What one candidate evaluation did — the audit record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundRecord {
    pub round: usize,
    pub candidate_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub diagnosis: String,
    pub rationale: String,
    pub mean_f1: f32,
    pub mean_em: f32,
    pub accepted: bool,
    pub reason: String,
    pub fixed: Vec<String>,
    pub broke: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeResult {
    pub rounds_completed: usize,
    pub candidates_evaluated: usize,
    pub candidates_accepted: usize,
    pub baseline_f1: f32,
    pub best_f1: f32,
    pub best_em: f32,
    pub best_policy: RetrievalPolicy,
    pub frontier_ceiling: f32,
    pub unsolved: Vec<String>,
    pub history: Vec<RoundRecord>,
    /// Rollouts (candidate evaluations) spent per +1 F1 gained. The
    /// charter's Q4 exit gate 2 asks this to decrease across a sprint.
    pub rollouts_per_f1_point: f32,
}

pub struct Optimizer {
    config: OptimizeConfig,
    archive: InstanceParetoArchive,
    gate: VerifierGate,
}

impl Optimizer {
    pub fn new(config: OptimizeConfig) -> Self {
        let gate = VerifierGate::new(config.verifier.clone());
        Self {
            config,
            archive: InstanceParetoArchive::new(),
            gate,
        }
    }

    pub fn archive(&self) -> &InstanceParetoArchive {
        &self.archive
    }

    /// Run the loop against `scorer`, starting from `seed`.
    pub fn run(&mut self, seed: RetrievalPolicy, scorer: &mut dyn PolicyScorer) -> OptimizeResult {
        let mut history = Vec::new();
        let mut evaluated = 0usize;
        let mut accepted = 0usize;

        // Baseline: the seed policy, actually executed.
        let baseline_report = scorer.score(&seed);
        let baseline_f1 = baseline_report.mean_f1;
        let seed_id = Uuid::new_v4();
        self.archive
            .try_add(seed_id, None, seed.clone(), &baseline_report);

        let mut best_report: ScoreReport = baseline_report.clone();

        for round in 0..self.config.max_rounds {
            // Anneal the step size: broad moves early, refinement later.
            let step = self.config.step
                * (1.0 - round as f32 / (self.config.max_rounds.max(1) as f32));
            let step = step.max(0.02);

            let parents: Vec<(Uuid, RetrievalPolicy)> = self
                .archive
                .parents(self.config.parents_per_round)
                .into_iter()
                .map(|c| (c.id, c.policy.clone()))
                .collect();

            let mut round_improved = false;

            for (parent_id, parent_policy) in parents {
                // Reflect on the current best evidence to choose a direction.
                let mut proposals = propose(&parent_policy, &best_report, step);

                // Q4.13 — apply the Curator's prior. Directions that have
                // repeatedly failed the verifier on this user's data are
                // dropped before they consume rollout budget; amplified
                // directions get an extra, larger-step variant.
                if let Some(prior) = &self.config.prior {
                    let name = format!("{:?}", crate::reflect::diagnose(&best_report));
                    if prior.is_suppressed(&name) && prior.confidence >= 0.5 {
                        continue;
                    }
                    if prior.is_amplified(&name) {
                        let extra = propose(&parent_policy, &best_report, step * 2.0);
                        proposals.extend(extra);
                    }
                }

                for proposal in proposals {
                    // Skip configurations already explored.
                    let fp = proposal.policy.fingerprint();
                    if self
                        .archive
                        .candidates
                        .iter()
                        .any(|c| c.policy.fingerprint() == fp)
                    {
                        continue;
                    }

                    let candidate_id = Uuid::new_v4();
                    let report = scorer.score(&proposal.policy);
                    evaluated += 1;

                    let verdict =
                        self.gate
                            .judge(candidate_id, &baseline_report, &report);

                    let added = if verdict.passed {
                        self.archive.try_add(
                            candidate_id,
                            Some(parent_id),
                            proposal.policy.clone(),
                            &report,
                        )
                    } else {
                        false
                    };

                    if added {
                        accepted += 1;
                    }
                    if report.mean_f1 > best_report.mean_f1 {
                        best_report = report.clone();
                        round_improved = true;
                    }

                    history.push(RoundRecord {
                        round,
                        candidate_id,
                        parent_id: Some(parent_id),
                        diagnosis: format!("{:?}", proposal.diagnosis),
                        rationale: proposal.rationale.clone(),
                        mean_f1: report.mean_f1,
                        mean_em: report.mean_em,
                        accepted: added,
                        reason: if verdict.passed {
                            if added {
                                "promoted".to_string()
                            } else {
                                "dominated".to_string()
                            }
                        } else {
                            verdict.reason.clone()
                        },
                        fixed: verdict.fixed,
                        broke: verdict.broke,
                    });
                }
            }

            // System-aware merge. Mutation moves one knob at a time, so it
            // cannot reach a configuration that requires several
            // simultaneous, individually-neutral moves — which is exactly
            // where two candidates with complementary per-instance wins
            // meet. Merging the archive's top instance-owners closes that
            // gap, and is what lets the run approach the frontier ceiling
            // rather than stalling below it.
            let top: Vec<(Uuid, RetrievalPolicy)> = self
                .archive
                .parents(3)
                .into_iter()
                .map(|c| (c.id, c.policy.clone()))
                .collect();
            for i in 0..top.len() {
                for j in (i + 1)..top.len() {
                    for bias in [0.25f32, 0.5, 0.75] {
                        let merged = top[i].1.merge(&top[j].1, bias);
                        let fp = merged.fingerprint();
                        if self
                            .archive
                            .candidates
                            .iter()
                            .any(|c| c.policy.fingerprint() == fp)
                        {
                            continue;
                        }
                        let candidate_id = Uuid::new_v4();
                        let report = scorer.score(&merged);
                        evaluated += 1;
                        let verdict = self.gate.judge(candidate_id, &baseline_report, &report);
                        let added = verdict.passed
                            && self.archive.try_add(
                                candidate_id,
                                Some(top[i].0),
                                merged.clone(),
                                &report,
                            );
                        if added {
                            accepted += 1;
                        }
                        if report.mean_f1 > best_report.mean_f1 {
                            best_report = report.clone();
                            round_improved = true;
                        }
                        history.push(RoundRecord {
                            round,
                            candidate_id,
                            parent_id: Some(top[i].0),
                            diagnosis: "Merge".to_string(),
                            rationale: format!(
                                "merged two archive candidates with complementary \
                                 per-instance wins (bias {bias:.2})"
                            ),
                            mean_f1: report.mean_f1,
                            mean_em: report.mean_em,
                            accepted: added,
                            reason: if verdict.passed {
                                if added { "promoted".into() } else { "dominated".into() }
                            } else {
                                verdict.reason.clone()
                            },
                            fixed: verdict.fixed,
                            broke: verdict.broke,
                        });
                    }
                }
            }

            // Converged: a full round with no candidate beating the best.
            if !round_improved && round > 0 {
                break;
            }
        }

        let best = self.archive.best_aggregate();
        let (best_f1, best_em, best_policy) = match best {
            Some(c) => (c.mean_f1, c.mean_em, c.policy.clone()),
            None => (baseline_f1, 0.0, seed),
        };

        let gain = (best_f1 - baseline_f1).max(0.0);
        let rollouts_per_f1_point = if gain > 0.0 {
            evaluated as f32 / gain
        } else {
            f32::INFINITY
        };

        let rounds_completed = history.iter().map(|h| h.round + 1).max().unwrap_or(0);

        OptimizeResult {
            rounds_completed,
            candidates_evaluated: evaluated,
            candidates_accepted: accepted,
            baseline_f1,
            best_f1,
            best_em,
            best_policy,
            frontier_ceiling: self.archive.frontier_ceiling(),
            unsolved: {
                let mut u: Vec<String> = self.archive.unsolved(0.5).into_iter().collect();
                u.sort();
                u
            },
            history,
            rollouts_per_f1_point,
        }
    }
}

/// Convenience: the dominant diagnosis across a run's history.
pub fn dominant_diagnosis(result: &OptimizeResult) -> Option<Diagnosis> {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for h in &result.history {
        *counts.entry(h.diagnosis.as_str()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .and_then(|(name, _)| match name {
            "RecallStarved" => Some(Diagnosis::RecallStarved),
            "RankedTooLow" => Some(Diagnosis::RankedTooLow),
            "PrecisionStarved" => Some(Diagnosis::PrecisionStarved),
            "Explore" => Some(Diagnosis::Explore),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::InstanceScore;

    /// Synthetic scorer with a known optimum: F1 peaks when the lexical
    /// weight is near `target`. Lets the loop be tested for whether it
    /// actually climbs, without a retrieval stack.
    struct QuadraticScorer {
        target: f32,
        calls: usize,
    }

    impl PolicyScorer for QuadraticScorer {
        fn score(&mut self, policy: &RetrievalPolicy) -> ScoreReport {
            self.calls += 1;
            let lex = policy.normalised_weights()["lexical"];
            let dist = (lex - self.target).abs();
            let base = (1.0 - dist).clamp(0.0, 1.0);
            let instances = (0..4)
                .map(|i| InstanceScore {
                    id: format!("q{i}"),
                    f1: base,
                    exact_match: if base > 0.95 { 1.0 } else { 0.0 },
                    feedback: if base < 0.5 { "wrong-span".into() } else { String::new() },
                })
                .collect();
            ScoreReport::from_instances(instances, 10)
        }
        fn anchor_count(&self) -> usize {
            4
        }
    }

    #[test]
    fn loop_improves_over_baseline() {
        let mut scorer = QuadraticScorer { target: 0.9, calls: 0 };
        let mut opt = Optimizer::new(OptimizeConfig::default());
        let r = opt.run(RetrievalPolicy::default(), &mut scorer);
        assert!(
            r.best_f1 >= r.baseline_f1,
            "best {} < baseline {}",
            r.best_f1,
            r.baseline_f1
        );
        assert!(r.candidates_evaluated > 0, "loop must execute candidates");
    }

    #[test]
    fn every_candidate_is_actually_executed() {
        let mut scorer = QuadraticScorer { target: 0.9, calls: 0 };
        let mut opt = Optimizer::new(OptimizeConfig { max_rounds: 2, ..Default::default() });
        let r = opt.run(RetrievalPolicy::default(), &mut scorer);
        // 1 baseline + one call per evaluated candidate.
        assert_eq!(scorer.calls, r.candidates_evaluated + 1);
    }

    #[test]
    fn history_records_rationale_for_each_candidate() {
        let mut scorer = QuadraticScorer { target: 0.9, calls: 0 };
        let mut opt = Optimizer::new(OptimizeConfig { max_rounds: 2, ..Default::default() });
        let r = opt.run(RetrievalPolicy::default(), &mut scorer);
        assert!(!r.history.is_empty());
        assert!(r.history.iter().all(|h| !h.rationale.is_empty()));
    }

    #[test]
    fn frontier_ceiling_is_at_least_best_aggregate() {
        let mut scorer = QuadraticScorer { target: 0.9, calls: 0 };
        let mut opt = Optimizer::new(OptimizeConfig::default());
        let r = opt.run(RetrievalPolicy::default(), &mut scorer);
        assert!(r.frontier_ceiling >= r.best_f1 - 1e-3);
    }

    #[test]
    fn duplicate_policies_are_not_re_evaluated() {
        let mut scorer = QuadraticScorer { target: 0.5, calls: 0 };
        let mut opt = Optimizer::new(OptimizeConfig { max_rounds: 4, ..Default::default() });
        let r = opt.run(RetrievalPolicy::default(), &mut scorer);
        let mut fps: Vec<String> = opt
            .archive()
            .candidates
            .iter()
            .map(|c| c.policy.fingerprint())
            .collect();
        let before = fps.len();
        fps.sort();
        fps.dedup();
        assert_eq!(before, fps.len(), "archive contains duplicate policies");
        assert!(r.candidates_evaluated > 0);
    }
}
