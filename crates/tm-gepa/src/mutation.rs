use rand::Rng;
use crate::policy::{MutationKind, PolicyCandidate, PolicyProvenance};
use uuid::Uuid;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct MutationConfig {
    pub weight_perturbation: f32,  // epsilon for weight mutation
    pub num_candidates: usize,     // mutations to generate per round
}

impl Default for MutationConfig {
    fn default() -> Self {
        Self { weight_perturbation: 0.1, num_candidates: 4 }
    }
}

/// Generate mutation candidates from a parent.
pub fn mutate(parent: &PolicyCandidate, config: &MutationConfig, rng: &mut impl Rng) -> Vec<PolicyCandidate> {
    let num_arms = parent.weights.arm_weights.len();
    let mut out = Vec::new();

    for i in 0..config.num_candidates {
        let (kind, weights, prompt) = if i % 2 == 0 {
            // Even: weight perturbation
            let arm = rng.gen_range(0..num_arms);
            let delta = if rng.gen_bool(0.5) { config.weight_perturbation } else { -config.weight_perturbation };
            let new_weights = parent.weights.perturb(arm, delta);
            (MutationKind::WeightPerturb, new_weights, parent.prompt.clone())
        } else {
            // Odd: prompt paraphrase
            let new_prompt = parent.prompt.paraphrase();
            (MutationKind::PromptParaphrase, parent.weights.clone(), new_prompt)
        };

        out.push(PolicyCandidate {
            id: Uuid::new_v4(),
            parent_id: Some(parent.id),
            mutation_kind: Some(kind.clone()),
            weights,
            prompt,
            pareto_scores: vec![],
            accepted: false,
            provenance: Some(PolicyProvenance {
                parent_id: parent.id,
                mutation_kind: kind,
                evidence: vec![],
                generated_at: Utc::now(),
            }),
            created_at: Utc::now(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn seeded_rng() -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(42)
    }

    #[test]
    fn mutate_produces_correct_count() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig::default();
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        assert_eq!(children.len(), config.num_candidates);
    }

    #[test]
    fn mutate_alternates_weight_and_prompt() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig { num_candidates: 4, ..Default::default() };
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);

        // Even indices → WeightPerturb, odd → PromptParaphrase
        assert_eq!(children[0].mutation_kind, Some(MutationKind::WeightPerturb));
        assert_eq!(children[1].mutation_kind, Some(MutationKind::PromptParaphrase));
        assert_eq!(children[2].mutation_kind, Some(MutationKind::WeightPerturb));
        assert_eq!(children[3].mutation_kind, Some(MutationKind::PromptParaphrase));
    }

    #[test]
    fn mutate_sets_parent_id() {
        let parent = PolicyCandidate::seed(5);
        let parent_id = parent.id;
        let config = MutationConfig::default();
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        for child in &children {
            assert_eq!(child.parent_id, Some(parent_id));
        }
    }

    #[test]
    fn mutate_children_not_accepted_initially() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig::default();
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        for child in &children {
            assert!(!child.accepted, "children should start un-accepted");
        }
    }

    #[test]
    fn mutate_children_have_provenance() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig::default();
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        for child in &children {
            assert!(child.provenance.is_some());
            let prov = child.provenance.as_ref().unwrap();
            assert_eq!(prov.parent_id, parent.id);
        }
    }

    #[test]
    fn mutate_weight_perturb_changes_weights() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig { num_candidates: 1, ..Default::default() };
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        assert_eq!(children[0].mutation_kind, Some(MutationKind::WeightPerturb));
        // Weights should differ from parent
        assert_ne!(children[0].weights.arm_weights, parent.weights.arm_weights);
    }

    #[test]
    fn mutate_prompt_paraphrase_changes_prompt() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig { num_candidates: 2, ..Default::default() };
        let mut rng = seeded_rng();
        let children = mutate(&parent, &config, &mut rng);
        let prompt_child = &children[1];
        assert_eq!(prompt_child.mutation_kind, Some(MutationKind::PromptParaphrase));
        assert_ne!(prompt_child.prompt.text, parent.prompt.text);
    }

    #[test]
    fn mutate_deterministic_with_same_seed() {
        let parent = PolicyCandidate::seed(5);
        let config = MutationConfig::default();
        let mut rng1 = seeded_rng();
        let mut rng2 = seeded_rng();
        let children1 = mutate(&parent, &config, &mut rng1);
        let children2 = mutate(&parent, &config, &mut rng2);
        // Weights should be identical (same seed)
        assert_eq!(children1[0].weights.arm_weights, children2[0].weights.arm_weights);
    }
}
