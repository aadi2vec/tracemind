//! Q4.13 — Recursive meta-loop: the ACE Curator.
//!
//! The Curator reads its own past PolicyMutations from TraceMind's memory,
//! selects which mutation patterns survive into the next mutation batch,
//! and writes provenance edges into the graph.
//!
//! This closes the L3 loop: reflections → policies → Curator selects policies
//! → next Curator's prior is shaped by what the current Curator observed.
//!
//! **TraceMind uses TraceMind to improve TraceMind.**
//!
//! The recursion is bounded: no self-modifying code, only self-modifying
//! prompts, weights, and space-configs. Every update writes a provenance
//! edge. Users can roll back any policy to any prior version via Tauri.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// PolicyMutation schema (charter §5 Pillar 7)
// ---------------------------------------------------------------------------

/// The unit of change that the GEPA loop produces and the Curator reads.
/// Stored as first-class memory with a provenance edge in the graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyMutation {
    pub id: Uuid,
    /// Which Pareto candidate was mutated.
    pub parent_id: Uuid,
    /// The kind of mutation applied.
    pub mutation_kind: MutationKind,
    /// Δ on {F1, latency, contradiction_rate, multi_hop} — positive = improvement.
    pub delta_scores: [f32; 4],
    /// Whether the mutation passed the anchor-eval verifier gate.
    pub accepted: bool,
    /// Trace IDs that motivated this mutation.
    pub evidence: Vec<Uuid>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationKind {
    PromptParaphrase,
    WeightPerturb,
    SpaceAdd,
    SpaceRemove,
}

impl MutationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MutationKind::PromptParaphrase => "prompt_paraphrase",
            MutationKind::WeightPerturb => "weight_perturb",
            MutationKind::SpaceAdd => "space_add",
            MutationKind::SpaceRemove => "space_remove",
        }
    }
}

// ---------------------------------------------------------------------------
// Curator output
// ---------------------------------------------------------------------------

/// The Curator's decision about which mutation families to amplify or suppress
/// in the next GEPA batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriorUpdate {
    /// Mutation kinds to generate more of in the next batch.
    pub amplify: Vec<MutationKind>,
    /// Mutation kinds to generate less of (or suppress entirely).
    pub suppress: Vec<MutationKind>,
    /// Confidence in this prior update (0.0-1.0).
    pub confidence: f32,
    pub generated_at: DateTime<Utc>,
    /// The Curator's own narrative reasoning (for the graph edge + rollback UI).
    pub narrative: String,
}

// ---------------------------------------------------------------------------
// Rollback record
// ---------------------------------------------------------------------------

/// Written when the user rolls back a policy via Tauri. Serves as a
/// negative signal for the Curator: repeated rollback of the same mutation
/// family disables that class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRollback {
    pub id: Uuid,
    pub mutation_id: Uuid,
    pub mutation_kind: MutationKind,
    pub rolled_back_at: DateTime<Utc>,
    pub user_note: Option<String>,
}

// ---------------------------------------------------------------------------
// Curator
// ---------------------------------------------------------------------------

/// The ACE Curator: reads past mutations + rollbacks, produces a PriorUpdate.
pub struct Curator {
    /// Maximum past mutations to consider.
    pub window: usize,
    /// Minimum delta_f1 to consider a mutation "effective".
    pub effectiveness_threshold: f32,
    /// Number of rollbacks of a kind before it's suppressed.
    pub rollback_suppress_threshold: usize,
}

impl Default for Curator {
    fn default() -> Self {
        Self {
            window: 50,
            effectiveness_threshold: 0.01,
            rollback_suppress_threshold: 3,
        }
    }
}

impl Curator {
    pub fn new(window: usize, effectiveness_threshold: f32, rollback_suppress_threshold: usize) -> Self {
        Self { window, effectiveness_threshold, rollback_suppress_threshold }
    }

    /// Run the Curator on recent mutation history.
    ///
    /// `mutations` — last N accepted+rejected mutations, newest last.
    /// `rollbacks` — user-initiated rollback events.
    pub fn run(
        &self,
        mutations: &[PolicyMutation],
        rollbacks: &[PolicyRollback],
    ) -> PriorUpdate {
        let recent = if mutations.len() > self.window {
            &mutations[mutations.len() - self.window..]
        } else {
            mutations
        };

        // Count effective vs ineffective mutations per kind.
        let mut kind_stats: std::collections::HashMap<&str, (usize, usize, f32)> =
            std::collections::HashMap::new(); // kind → (effective, total, avg_delta_f1)

        for m in recent {
            let kind = m.mutation_kind.as_str();
            let e = kind_stats.entry(kind).or_insert((0, 0, 0.0));
            e.1 += 1;
            e.2 += m.delta_scores[0]; // F1 delta
            if m.accepted && m.delta_scores[0] >= self.effectiveness_threshold {
                e.0 += 1;
            }
        }

        // Count rollbacks per kind.
        let mut rollback_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for r in rollbacks {
            *rollback_counts.entry(r.mutation_kind.as_str()).or_insert(0) += 1;
        }

        let mut amplify = Vec::new();
        let mut suppress = Vec::new();

        let all_kinds = [
            MutationKind::PromptParaphrase,
            MutationKind::WeightPerturb,
            MutationKind::SpaceAdd,
            MutationKind::SpaceRemove,
        ];

        for kind in &all_kinds {
            let kind_str = kind.as_str();

            // Suppress if too many rollbacks.
            let rollbacks_for_kind = rollback_counts.get(kind_str).copied().unwrap_or(0);
            if rollbacks_for_kind >= self.rollback_suppress_threshold {
                suppress.push(kind.clone());
                continue;
            }

            // Amplify if effective rate > 50%.
            if let Some(&(effective, total, avg_delta)) = kind_stats.get(kind_str) {
                if total > 0 {
                    let rate = effective as f64 / total as f64;
                    if rate > 0.5 && avg_delta > self.effectiveness_threshold * total as f32 {
                        amplify.push(kind.clone());
                    } else if rate < 0.2 {
                        suppress.push(kind.clone());
                    }
                }
            }
        }

        // Compute confidence: higher when we have more data.
        let data_coverage = (recent.len() as f32 / self.window as f32).min(1.0);
        let rollback_pressure = (rollbacks.len() as f32 / 10.0).min(1.0);
        let confidence = data_coverage * (1.0 - 0.3 * rollback_pressure);

        let narrative = format!(
            "Curator analyzed {} mutations ({} rollbacks). \
             Amplifying: {:?}. Suppressing: {:?}. Confidence: {:.2}.",
            recent.len(),
            rollbacks.len(),
            amplify.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            suppress.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            confidence,
        );

        PriorUpdate {
            amplify,
            suppress,
            confidence,
            generated_at: Utc::now(),
            narrative,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyMutation store (JSONL)
// ---------------------------------------------------------------------------

pub struct PolicyMutationStore {
    path: std::path::PathBuf,
}

impl PolicyMutationStore {
    pub fn open(path: impl Into<std::path::PathBuf>) -> tm_types::Result<Self> {
        let path = path.into();
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)
                .map_err(|e| tm_types::TraceMindError::Storage(e.to_string()))?;
        }
        Ok(Self { path })
    }

    pub fn append(&self, m: &PolicyMutation) -> tm_types::Result<()> {
        use std::io::Write;
        let mut line = serde_json::to_string(m)
            .map_err(|e| tm_types::TraceMindError::Serialization(e))?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .map_err(|e| tm_types::TraceMindError::Storage(e.to_string()))?;
        f.write_all(line.as_bytes())
            .map_err(|e| tm_types::TraceMindError::Storage(e.to_string()))
    }

    pub fn recent(&self, limit: usize) -> tm_types::Result<Vec<PolicyMutation>> {
        let raw = std::fs::read_to_string(&self.path).unwrap_or_default();
        let mut v: Vec<PolicyMutation> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let s = v.len().saturating_sub(limit);
        Ok(v.drain(s..).collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn mutation(kind: MutationKind, delta_f1: f32, accepted: bool) -> PolicyMutation {
        PolicyMutation {
            id: Uuid::new_v4(),
            parent_id: Uuid::new_v4(),
            mutation_kind: kind,
            delta_scores: [delta_f1, 0.0, 0.0, 0.0],
            accepted,
            evidence: vec![],
            generated_at: Utc::now(),
        }
    }

    fn rollback(kind: MutationKind) -> PolicyRollback {
        PolicyRollback {
            id: Uuid::new_v4(),
            mutation_id: Uuid::new_v4(),
            mutation_kind: kind,
            rolled_back_at: Utc::now(),
            user_note: None,
        }
    }

    #[test]
    fn amplifies_effective_mutations() {
        let curator = Curator::default();
        let mutations: Vec<_> = (0..6).map(|_| {
            mutation(MutationKind::WeightPerturb, 0.05, true)
        }).collect();
        let prior = curator.run(&mutations, &[]);
        assert!(prior.amplify.contains(&MutationKind::WeightPerturb),
            "WeightPerturb should be amplified: {:?}", prior);
    }

    #[test]
    fn suppresses_after_rollbacks() {
        let curator = Curator::default();
        let mutations: Vec<_> = (0..3).map(|_| {
            mutation(MutationKind::PromptParaphrase, 0.02, true)
        }).collect();
        let rollbacks: Vec<_> = (0..3).map(|_| {
            rollback(MutationKind::PromptParaphrase)
        }).collect();
        let prior = curator.run(&mutations, &rollbacks);
        assert!(prior.suppress.contains(&MutationKind::PromptParaphrase),
            "PromptParaphrase should be suppressed after 3 rollbacks: {:?}", prior);
    }

    #[test]
    fn empty_input_produces_neutral_prior() {
        let curator = Curator::default();
        let prior = curator.run(&[], &[]);
        assert!(prior.amplify.is_empty());
        assert!(prior.suppress.is_empty());
        assert!(prior.confidence >= 0.0 && prior.confidence <= 1.0);
    }

    #[test]
    fn policy_mutation_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = PolicyMutationStore::open(dir.path().join("mutations.jsonl")).unwrap();
        let m = mutation(MutationKind::WeightPerturb, 0.03, true);
        let id = m.id;
        store.append(&m).unwrap();
        let loaded = store.recent(10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, id);
    }
}
