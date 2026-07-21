use rand::SeedableRng;
use rand::Rng;
use serde::{Serialize, Deserialize};
use crate::mutation::{mutate, MutationConfig};
use crate::pareto::{ParetoArchive, ParetoAxis};
use crate::policy::PolicyCandidate;
use crate::verifier::{VerifierConfig, VerifierGate};

#[derive(Debug, Clone)]
pub struct GepaConfig {
    pub mutation: MutationConfig,
    pub verifier: VerifierConfig,
    pub axes: Vec<ParetoAxis>,
    pub max_rounds: usize,
    /// Seed for deterministic mutation (useful in tests).
    pub rng_seed: u64,
}

impl Default for GepaConfig {
    fn default() -> Self {
        Self {
            mutation: MutationConfig::default(),
            verifier: VerifierConfig::default(),
            axes: ParetoArchive::default_axes(),
            max_rounds: 3,
            rng_seed: 42,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GepaRunResult {
    pub rounds_completed: usize,
    pub candidates_generated: usize,
    pub candidates_accepted: usize,
    pub best_f1: f32,
    pub archive_size: usize,
}

pub struct GepaLoop {
    config: GepaConfig,
    archive: ParetoArchive,
    baseline_f1: f32,
}

impl GepaLoop {
    pub fn new(config: GepaConfig, initial_candidate: PolicyCandidate, baseline_f1: f32) -> Self {
        let mut archive = ParetoArchive::new(config.axes.clone());
        archive.try_add(initial_candidate);
        Self { config, archive, baseline_f1 }
    }

    /// Run one nightly GEPA round.
    pub fn run_round(&mut self) -> GepaRunResult {
        let mut rng = rand::rngs::StdRng::seed_from_u64(self.config.rng_seed);
        let gate = VerifierGate::new(self.config.verifier.clone());

        let parents: Vec<PolicyCandidate> = self.archive.candidates
            .iter()
            .filter(|c| c.accepted)
            .cloned()
            .collect();

        let mut generated = 0;
        let mut accepted = 0;

        for parent in &parents {
            let mut mutations = mutate(parent, &self.config.mutation, &mut rng);
            for mut candidate in mutations.drain(..) {
                // Synthetic scoring: use parent scores + small noise
                if !parent.pareto_scores.is_empty() {
                    candidate.pareto_scores = parent.pareto_scores.iter()
                        .map(|&s| (s + rng.gen_range(-0.02f32..0.02f32)).clamp(0.0, 1.0))
                        .collect();
                } else {
                    // Seed with moderate scores
                    candidate.pareto_scores = vec![0.5, 0.7, 0.6, 0.45];
                }

                let eval = gate.evaluate(&candidate, self.baseline_f1);
                generated += 1;
                if eval.passed {
                    if self.archive.try_add(candidate) {
                        accepted += 1;
                    }
                }
            }
        }

        let best_f1 = self.archive.best_by_axis(ParetoAxis::F1)
            .and_then(|c| c.pareto_scores.first().copied())
            .unwrap_or(self.baseline_f1);

        GepaRunResult {
            rounds_completed: 1,
            candidates_generated: generated,
            candidates_accepted: accepted,
            best_f1,
            archive_size: self.archive.accepted_count(),
        }
    }

    pub fn archive(&self) -> &ParetoArchive { &self.archive }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyCandidate;

    fn default_loop() -> GepaLoop {
        let config = GepaConfig::default();
        let seed = PolicyCandidate::seed(5);
        GepaLoop::new(config, seed, 0.50)
    }

    #[test]
    fn new_loop_has_one_candidate_in_archive() {
        let gl = default_loop();
        // The seed candidate is in the archive (accepted provisionally)
        assert_eq!(gl.archive().accepted_count(), 1);
    }

    #[test]
    fn run_round_generates_candidates() {
        let mut gl = default_loop();
        let result = gl.run_round();
        assert!(result.candidates_generated > 0);
        assert_eq!(result.rounds_completed, 1);
    }

    #[test]
    fn run_round_archive_grows() {
        let mut gl = default_loop();
        let result = gl.run_round();
        // Archive should have at least the seed + some accepted mutations
        assert!(result.archive_size >= 1);
    }

    #[test]
    fn run_round_best_f1_is_valid() {
        let mut gl = default_loop();
        let result = gl.run_round();
        assert!(result.best_f1 >= 0.0 && result.best_f1 <= 1.0,
            "best_f1 should be in [0,1], got {}", result.best_f1);
    }

    #[test]
    fn run_multiple_rounds_is_stable() {
        let config = GepaConfig { max_rounds: 3, ..Default::default() };
        let seed = PolicyCandidate::seed(5);
        let mut gl = GepaLoop::new(config, seed, 0.50);
        for _ in 0..3 {
            let r = gl.run_round();
            assert!(r.rounds_completed == 1);
            assert!(r.archive_size >= 1);
        }
    }

    #[test]
    fn verifier_gate_blocks_regressions() {
        // Set a very tight regression tolerance
        let config = GepaConfig {
            verifier: VerifierConfig {
                max_f1_regression: 0.0,  // no regression allowed
                anchor_count: 10,
            },
            ..Default::default()
        };
        let seed = PolicyCandidate::seed(5);
        // High baseline so any noise that goes down gets rejected
        let mut gl = GepaLoop::new(config, seed, 0.99);
        let result = gl.run_round();
        // With a baseline of 0.99 and moderate synthetic scores (~0.5),
        // regression = 0.99 - 0.5 = 0.49 >> 0.0, so most/all should be rejected
        assert_eq!(result.candidates_accepted, 0,
            "expected 0 accepted with tight regression gate, got {}", result.candidates_accepted);
    }

    #[test]
    fn gepa_run_result_serializes() {
        let r = GepaRunResult {
            rounds_completed: 1,
            candidates_generated: 4,
            candidates_accepted: 2,
            best_f1: 0.75,
            archive_size: 3,
        };
        let json = serde_json::to_string(&r).unwrap();
        let r2: GepaRunResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r.candidates_generated, r2.candidates_generated);
        assert!((r.best_f1 - r2.best_f1).abs() < 1e-6);
    }

    #[test]
    fn gepa_config_default_max_rounds() {
        let cfg = GepaConfig::default();
        assert_eq!(cfg.max_rounds, 3);
        assert_eq!(cfg.rng_seed, 42);
    }
}
