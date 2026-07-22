use chrono::{DateTime, Utc};
use uuid::Uuid;
use serde::{Serialize, Deserialize};

/// What kind of mutation was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    PromptParaphrase,
    WeightPerturb,
    SpaceAdd,
    SpaceRemove,
}

/// Provenance of a policy mutation (for the graph edge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyProvenance {
    pub parent_id: Uuid,
    pub mutation_kind: MutationKind,
    pub evidence: Vec<Uuid>,  // trace_ids that motivated this
    pub generated_at: DateTime<Utc>,
}

/// Arm weight vector (one weight per bandit arm, 0..1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyWeights {
    pub arm_weights: Vec<f32>,  // length = NUM_ARMS (currently 6)
}

impl PolicyWeights {
    pub fn uniform(num_arms: usize) -> Self {
        let w = 1.0 / num_arms as f32;
        Self { arm_weights: vec![w; num_arms] }
    }

    pub fn perturb(&self, arm_idx: usize, delta: f32) -> Self {
        let mut weights = self.arm_weights.clone();
        if let Some(w) = weights.get_mut(arm_idx) {
            *w = (*w + delta).clamp(0.01, 1.0);
        }
        // Renormalize
        let total: f32 = weights.iter().sum();
        if total > 0.0 {
            for w in &mut weights { *w /= total; }
        }
        Self { arm_weights: weights }
    }
}

/// A retrieval policy prompt string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPrompt {
    pub text: String,
}

impl PolicyPrompt {
    pub fn default_policy() -> Self {
        Self {
            text: "Retrieve memories most relevant to the query. \
                   Prefer recent, high-confidence facts over stale low-confidence ones. \
                   Use graph traversal when the query implies relationships.".to_string(),
        }
    }

    pub fn paraphrase(&self) -> Self {
        // Simple deterministic paraphrase: reorder the sentences
        let sentences: Vec<&str> = self.text.split(". ").collect();
        let reordered = if sentences.len() >= 2 {
            let mut s = sentences[1..].to_vec();
            s.push(sentences[0]);
            s.join(". ")
        } else {
            format!("{} [revised]", self.text)
        };
        Self { text: reordered }
    }
}

/// One candidate in the GEPA Pareto archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyCandidate {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub mutation_kind: Option<MutationKind>,
    pub weights: PolicyWeights,
    pub prompt: PolicyPrompt,
    pub pareto_scores: Vec<f32>,  // one per ParetoAxis, same order
    pub accepted: bool,
    pub provenance: Option<PolicyProvenance>,
    pub created_at: DateTime<Utc>,
}

impl PolicyCandidate {
    pub fn seed(num_arms: usize) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent_id: None,
            mutation_kind: None,
            weights: PolicyWeights::uniform(num_arms),
            prompt: PolicyPrompt::default_policy(),
            pareto_scores: vec![],
            accepted: true,
            provenance: None,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_creates_uniform_weights() {
        let c = PolicyCandidate::seed(5);
        assert_eq!(c.weights.arm_weights.len(), 5);
        let sum: f32 = c.weights.arm_weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights should sum to 1.0, got {sum}");
        for &w in &c.weights.arm_weights {
            assert!((w - 0.2).abs() < 1e-5, "each weight should be 0.2, got {w}");
        }
    }

    #[test]
    fn perturb_renormalizes() {
        let w = PolicyWeights::uniform(5);
        let perturbed = w.perturb(0, 0.2);
        let sum: f32 = perturbed.arm_weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "perturbed weights should sum to 1.0, got {sum}");
        // The perturbed arm should be different from the others
        assert!(perturbed.arm_weights[0] > perturbed.arm_weights[1]);
    }

    #[test]
    fn perturb_clamps_to_min() {
        let w = PolicyWeights::uniform(3);
        // Subtracting more than the weight — should clamp to 0.01
        let perturbed = w.perturb(0, -10.0);
        assert!(perturbed.arm_weights[0] >= 0.01 / 3.0 - 1e-5);
    }

    #[test]
    fn prompt_paraphrase_reorders_sentences() {
        let p = PolicyPrompt::default_policy();
        let paraphrased = p.paraphrase();
        // Should differ from original
        assert_ne!(p.text, paraphrased.text);
        // Should contain the same words (just reordered)
        assert!(paraphrased.text.contains("Retrieve memories"));
    }

    #[test]
    fn prompt_paraphrase_single_sentence() {
        let p = PolicyPrompt { text: "Just one sentence".to_string() };
        let paraphrased = p.paraphrase();
        assert!(paraphrased.text.contains("[revised]"));
    }

    #[test]
    fn mutation_kind_serializes() {
        let k = MutationKind::PromptParaphrase;
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, "\"prompt_paraphrase\"");
        let k2: MutationKind = serde_json::from_str(&s).unwrap();
        assert_eq!(k, k2);
    }

    #[test]
    fn policy_candidate_round_trips_json() {
        let c = PolicyCandidate::seed(5);
        let json = serde_json::to_string(&c).unwrap();
        let c2: PolicyCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(c.id, c2.id);
        assert_eq!(c.weights.arm_weights, c2.weights.arm_weights);
    }
}
