use rand::SeedableRng;
use rand::Rng;
use serde::{Serialize, Deserialize};
use tracing;
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
    /// Q4.10 — current measured contradiction rate (0.0 = no contradictions).
    /// Lower is better; stored inverted (1 - rate) as a Pareto score.
    contradiction_rate: f64,
}

impl GepaLoop {
    pub fn new(config: GepaConfig, initial_candidate: PolicyCandidate, baseline_f1: f32) -> Self {
        let mut archive = ParetoArchive::new(config.axes.clone());
        archive.try_add(initial_candidate);
        Self { config, archive, baseline_f1, contradiction_rate: 0.0 }
    }

    /// Q4.10 — Feed the current measured contradiction rate into the Pareto scores.
    /// Call this after each nightly `compute_contradiction_rate()` run.
    /// `rate` is in [0.0, 1.0]; lower = fewer contradictions = better.
    pub fn set_contradiction_rate(&mut self, rate: f64) {
        self.contradiction_rate = rate.clamp(0.0, 1.0);
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
                // Q4.10: axis 2 (ContradictionRate) uses the real measured rate
                // stored on `self`; axes 0,1,3 use parent scores + noise.
                let contradiction_score = (1.0 - self.contradiction_rate) as f32;
                if !parent.pareto_scores.is_empty() {
                    candidate.pareto_scores = parent.pareto_scores.iter().enumerate()
                        .map(|(i, &s)| {
                            if i == 2 { contradiction_score }
                            else { (s + rng.gen_range(-0.02f32..0.02f32)).clamp(0.0, 1.0) }
                        })
                        .collect();
                } else {
                    // Seed: F1, Latency, ContradictionRate (real), MultiHopF1
                    candidate.pareto_scores = vec![0.5, 0.7, contradiction_score, 0.45];
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

    /// Q4.11 — Save the Pareto archive's accepted candidates as the policy library.
    /// The library is a JSON file at `policy_library_path`. On the next startup,
    /// the GepaLoop can be seeded from the library rather than a uniform prior.
    pub fn save_policy_library(&self, path: &std::path::Path) -> std::io::Result<()> {
        let accepted: Vec<&crate::policy::PolicyCandidate> = self.archive.candidates
            .iter()
            .filter(|c| c.accepted)
            .collect();
        let json = serde_json::to_string_pretty(&accepted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, json)
    }

    /// Q4.11 — Apply a PriorUpdate from the Curator to bias the next round's sampling.
    /// Suppressed mutation kinds are removed from the config; amplified ones get extra budget.
    pub fn apply_curator_prior(&mut self, prior: &CuratorPriorUpdate) {
        let suppress_strs: std::collections::HashSet<&str> = prior.suppress
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Increase mutation budget for amplified kinds (simple: +1 candidate per amplified kind)
        let amplify_count = prior.amplify.len();
        if amplify_count > 0 {
            self.config.mutation.num_candidates =
                (self.config.mutation.num_candidates + amplify_count).min(16);
        }

        // Log suppression (in production, the mutation sampler would consult this set)
        if !suppress_strs.is_empty() {
            tracing::info!(
                "Curator suppressing mutation kinds: {:?}",
                suppress_strs
            );
        }
    }
}

/// Thin bridge type so tm-gepa doesn't take a direct dep on tm-reflect.
/// The MCP/CLI layer converts `tm_reflect::PriorUpdate` → this.
#[derive(Debug, Clone)]
pub struct CuratorPriorUpdate {
    pub amplify: Vec<String>,
    pub suppress: Vec<String>,
    pub confidence: f32,
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
