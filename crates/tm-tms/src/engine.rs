//! The JTMS propagation engine (in-memory, no DB).

use std::collections::{HashMap, VecDeque};

use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;

use crate::types::{
    Belief, BeliefStatus, Contradiction, ContradictionResolution, Justification,
    JustificationType, PropagationResult,
};

// ── Errors ─────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum TmsError {
    #[error("belief not found: {0}")]
    BeliefNotFound(Uuid),
    #[error("contradiction not found: {0}")]
    ContradictionNotFound(Uuid),
    #[error("premise belief not found: {0}")]
    PremiseNotFound(Uuid),
}

// ── Engine ─────────────────────────────────────────────────────────

/// Justification-based Truth Maintenance System.
///
/// All state is kept in memory.  Serialise via `serde` if persistence
/// is required.
#[derive(Debug, Default)]
pub struct TmsEngine {
    beliefs: HashMap<Uuid, Belief>,
    justifications: HashMap<Uuid, Justification>,
    contradictions: Vec<Contradiction>,
}

impl TmsEngine {
    /// Create an empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    // ── Belief CRUD ────────────────────────────────────────────────

    /// Assert a new belief.  It starts as `In`.
    pub fn assert_belief(&mut self, statement: impl Into<String>, confidence: f32) -> Uuid {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let belief = Belief {
            id,
            statement: statement.into(),
            status: BeliefStatus::In,
            confidence,
            created_at: now,
            updated_at: now,
        };
        self.beliefs.insert(id, belief);
        id
    }

    /// Retract a belief (set to `Out`) and propagate to dependents.
    pub fn retract_belief(&mut self, id: Uuid) -> PropagationResult {
        if let Some(b) = self.beliefs.get_mut(&id) {
            b.status = BeliefStatus::Out;
            b.updated_at = Utc::now();
        }
        self.propagate(&[id])
    }

    pub fn get_belief(&self, id: Uuid) -> Option<&Belief> {
        self.beliefs.get(&id)
    }

    pub fn get_status(&self, id: Uuid) -> Option<BeliefStatus> {
        self.beliefs.get(&id).map(|b| b.status)
    }

    /// Direct status override. Used by callers that resolve a
    /// contradiction with semantics the four `ContradictionResolution`
    /// variants don't cover — e.g. "keep both, they're about
    /// different times". Bumps `updated_at` so consumers can tell the
    /// belief was touched.
    ///
    /// No propagation: dependencies of this belief keep their old
    /// status. Use sparingly — JTMS invariants only hold for beliefs
    /// whose status was set through `assert_belief` /
    /// `retract_belief` / `resolve_contradiction`.
    pub fn force_status(&mut self, id: Uuid, status: BeliefStatus) -> bool {
        if let Some(b) = self.beliefs.get_mut(&id) {
            b.status = status;
            b.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    /// All beliefs currently `In`.
    pub fn active_beliefs(&self) -> Vec<&Belief> {
        self.beliefs
            .values()
            .filter(|b| b.status == BeliefStatus::In)
            .collect()
    }

    // ── Justifications ─────────────────────────────────────────────

    /// Add a justification and propagate status changes.
    pub fn add_justification(
        &mut self,
        conclusion_id: Uuid,
        premise_ids: Vec<Uuid>,
        jtype: JustificationType,
    ) -> Result<PropagationResult, TmsError> {
        // Validate references.
        if !self.beliefs.contains_key(&conclusion_id) {
            return Err(TmsError::BeliefNotFound(conclusion_id));
        }
        for &pid in &premise_ids {
            if !self.beliefs.contains_key(&pid) {
                return Err(TmsError::PremiseNotFound(pid));
            }
        }

        let id = Uuid::new_v4();
        let j = Justification {
            id,
            conclusion: conclusion_id,
            premises: premise_ids,
            justification_type: jtype,
            created_at: Utc::now(),
        };
        self.justifications.insert(id, j);

        // Re-evaluate the conclusion itself, then propagate outward.
        let new_status = self.compute_status(conclusion_id);
        let mut result = PropagationResult::default();
        if let Some(b) = self.beliefs.get_mut(&conclusion_id) {
            if b.status != new_status {
                b.status = new_status;
                b.updated_at = Utc::now();
                result.changed.push((conclusion_id, new_status));
            }
        }

        let downstream = self.propagate(&[conclusion_id]);
        result.changed.extend(downstream.changed);
        result.contradictions.extend(downstream.contradictions);
        Ok(result)
    }

    /// All justifications whose conclusion is `id`.
    pub fn justifications_for(&self, id: Uuid) -> Vec<&Justification> {
        self.justifications
            .values()
            .filter(|j| j.conclusion == id)
            .collect()
    }

    /// All beliefs that appear as *conclusions* of justifications
    /// where `id` is a premise.
    pub fn dependents_of(&self, id: Uuid) -> Vec<Uuid> {
        self.justifications
            .values()
            .filter(|j| j.premises.contains(&id))
            .map(|j| j.conclusion)
            .collect()
    }

    // ── Contradictions ─────────────────────────────────────────────

    /// If `cosine_sim < -0.8`, record a contradiction between two
    /// beliefs and mark them as `Contradicted`.
    pub fn detect_contradiction(
        &mut self,
        a: Uuid,
        b: Uuid,
        cosine_sim: f32,
    ) -> Option<Contradiction> {
        if cosine_sim >= -0.8 {
            return None;
        }

        let c = Contradiction {
            id: Uuid::new_v4(),
            belief_a: a,
            belief_b: b,
            detected_at: Utc::now(),
            resolution: None,
            cosine_similarity: cosine_sim,
        };

        // Mark both beliefs as contradicted.
        if let Some(ba) = self.beliefs.get_mut(&a) {
            ba.status = BeliefStatus::Contradicted;
            ba.updated_at = Utc::now();
        }
        if let Some(bb) = self.beliefs.get_mut(&b) {
            bb.status = BeliefStatus::Contradicted;
            bb.updated_at = Utc::now();
        }

        self.contradictions.push(c.clone());
        Some(c)
    }

    /// Resolve a previously detected contradiction.
    pub fn resolve_contradiction(
        &mut self,
        contradiction_id: Uuid,
        resolution: ContradictionResolution,
    ) -> Result<PropagationResult, TmsError> {
        let idx = self
            .contradictions
            .iter()
            .position(|c| c.id == contradiction_id)
            .ok_or(TmsError::ContradictionNotFound(contradiction_id))?;

        self.contradictions[idx].resolution = Some(resolution);
        let belief_a = self.contradictions[idx].belief_a;
        let belief_b = self.contradictions[idx].belief_b;

        let mut changed_roots = Vec::new();

        match resolution {
            ContradictionResolution::RetractA => {
                if let Some(b) = self.beliefs.get_mut(&belief_a) {
                    b.status = BeliefStatus::Out;
                    b.updated_at = Utc::now();
                }
                // Restore B.
                if let Some(b) = self.beliefs.get_mut(&belief_b) {
                    b.status = BeliefStatus::In;
                    b.updated_at = Utc::now();
                }
                changed_roots.push(belief_a);
                changed_roots.push(belief_b);
            }
            ContradictionResolution::RetractB => {
                if let Some(b) = self.beliefs.get_mut(&belief_b) {
                    b.status = BeliefStatus::Out;
                    b.updated_at = Utc::now();
                }
                if let Some(b) = self.beliefs.get_mut(&belief_a) {
                    b.status = BeliefStatus::In;
                    b.updated_at = Utc::now();
                }
                changed_roots.push(belief_a);
                changed_roots.push(belief_b);
            }
            ContradictionResolution::RetractBoth => {
                for &id in &[belief_a, belief_b] {
                    if let Some(b) = self.beliefs.get_mut(&id) {
                        b.status = BeliefStatus::Out;
                        b.updated_at = Utc::now();
                    }
                }
                changed_roots.push(belief_a);
                changed_roots.push(belief_b);
            }
            ContradictionResolution::UserOverride(keep_id) => {
                let retract_id = if keep_id == belief_a {
                    belief_b
                } else {
                    belief_a
                };
                if let Some(b) = self.beliefs.get_mut(&keep_id) {
                    b.status = BeliefStatus::In;
                    b.updated_at = Utc::now();
                }
                if let Some(b) = self.beliefs.get_mut(&retract_id) {
                    b.status = BeliefStatus::Out;
                    b.updated_at = Utc::now();
                }
                changed_roots.push(keep_id);
                changed_roots.push(retract_id);
            }
        }

        Ok(self.propagate(&changed_roots))
    }

    /// All recorded contradictions.
    pub fn contradictions(&self) -> &[Contradiction] {
        &self.contradictions
    }

    // ── Propagation ────────────────────────────────────────────────

    /// BFS propagation from a set of changed belief IDs.
    ///
    /// For each belief whose status may have changed, re-evaluate all
    /// justifications that reference it as a premise.  A conclusion is
    /// `In` iff:
    ///
    /// 1. At least one `Support` justification has all premises `In`, AND
    /// 2. No `Defeat` justification has all premises `In`.
    ///
    /// If no justifications exist for a belief the status is left
    /// unchanged (it was asserted directly).
    fn propagate(&mut self, roots: &[Uuid]) -> PropagationResult {
        let mut result = PropagationResult::default();
        let mut queue: VecDeque<Uuid> = roots.iter().copied().collect();
        // Track beliefs we have already enqueued to avoid cycles.
        let mut visited: std::collections::HashSet<Uuid> = roots.iter().copied().collect();

        while let Some(changed_id) = queue.pop_front() {
            // Find all justifications where `changed_id` is a premise.
            let affected_conclusions: Vec<Uuid> = self
                .justifications
                .values()
                .filter(|j| j.premises.contains(&changed_id))
                .map(|j| j.conclusion)
                .collect();

            for conclusion_id in affected_conclusions {
                let new_status = self.compute_status(conclusion_id);

                let old_status = match self.beliefs.get(&conclusion_id) {
                    Some(b) => b.status,
                    None => continue,
                };

                if new_status != old_status {
                    if let Some(b) = self.beliefs.get_mut(&conclusion_id) {
                        b.status = new_status;
                        b.updated_at = Utc::now();
                    }
                    result.changed.push((conclusion_id, new_status));
                    if visited.insert(conclusion_id) {
                        queue.push_back(conclusion_id);
                    }
                }
            }
        }

        result
    }

    /// Determine the correct status for a belief based on its
    /// justifications.  If the belief has no justifications its
    /// current status is returned unchanged.
    fn compute_status(&self, id: Uuid) -> BeliefStatus {
        let justs: Vec<&Justification> = self
            .justifications
            .values()
            .filter(|j| j.conclusion == id)
            .collect();

        if justs.is_empty() {
            // Directly asserted — keep current status.
            return self
                .beliefs
                .get(&id)
                .map(|b| b.status)
                .unwrap_or(BeliefStatus::Out);
        }

        // Check for active defeat.
        let defeated = justs.iter().any(|j| {
            j.justification_type == JustificationType::Defeat
                && j.premises.iter().all(|p| {
                    self.beliefs
                        .get(p)
                        .map(|b| b.status == BeliefStatus::In)
                        .unwrap_or(false)
                })
        });

        if defeated {
            return BeliefStatus::Out;
        }

        // Check for active support.
        let supported = justs.iter().any(|j| {
            j.justification_type == JustificationType::Support
                && j.premises.iter().all(|p| {
                    self.beliefs
                        .get(p)
                        .map(|b| b.status == BeliefStatus::In)
                        .unwrap_or(false)
                })
        });

        if supported {
            BeliefStatus::In
        } else {
            BeliefStatus::Out
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BeliefStatus, ContradictionResolution, JustificationType};

    #[test]
    fn assert_belief_is_in() {
        let mut engine = TmsEngine::new();
        let id = engine.assert_belief("the sky is blue", 0.9);
        assert_eq!(engine.get_status(id), Some(BeliefStatus::In));
        assert_eq!(engine.get_belief(id).unwrap().statement, "the sky is blue");
    }

    #[test]
    fn retract_belief_sets_out() {
        let mut engine = TmsEngine::new();
        let id = engine.assert_belief("temporary fact", 0.5);
        engine.retract_belief(id);
        assert_eq!(engine.get_status(id), Some(BeliefStatus::Out));
    }

    #[test]
    fn justification_chain_propagates() {
        let mut engine = TmsEngine::new();

        // A is directly asserted.
        let a = engine.assert_belief("A", 0.9);
        // B depends on A.
        let b = engine.assert_belief("B", 0.8);
        engine
            .add_justification(b, vec![a], JustificationType::Support)
            .unwrap();
        // C depends on B.
        let c = engine.assert_belief("C", 0.7);
        engine
            .add_justification(c, vec![b], JustificationType::Support)
            .unwrap();

        assert_eq!(engine.get_status(a), Some(BeliefStatus::In));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::In));
        assert_eq!(engine.get_status(c), Some(BeliefStatus::In));

        // Retract A → B goes Out → C goes Out.
        let result = engine.retract_belief(a);
        assert_eq!(engine.get_status(a), Some(BeliefStatus::Out));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::Out));
        assert_eq!(engine.get_status(c), Some(BeliefStatus::Out));

        // Both B and C should appear in the changed set.
        let changed_ids: Vec<Uuid> = result.changed.iter().map(|(id, _)| *id).collect();
        assert!(changed_ids.contains(&b));
        assert!(changed_ids.contains(&c));
    }

    #[test]
    fn detect_contradiction_below_threshold() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("it will rain", 0.9);
        let b = engine.assert_belief("it will not rain", 0.9);

        // Cosine similarity above threshold — no contradiction.
        assert!(engine.detect_contradiction(a, b, -0.5).is_none());
        assert_eq!(engine.contradictions().len(), 0);

        // Below threshold — contradiction recorded.
        let c = engine.detect_contradiction(a, b, -0.85).unwrap();
        assert_eq!(c.belief_a, a);
        assert_eq!(c.belief_b, b);
        assert_eq!(engine.get_status(a), Some(BeliefStatus::Contradicted));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::Contradicted));
        assert_eq!(engine.contradictions().len(), 1);
    }

    #[test]
    fn resolve_contradiction_retract_a() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("belief A", 0.9);
        let b = engine.assert_belief("belief B", 0.9);
        let c = engine.detect_contradiction(a, b, -0.9).unwrap();

        engine
            .resolve_contradiction(c.id, ContradictionResolution::RetractA)
            .unwrap();

        assert_eq!(engine.get_status(a), Some(BeliefStatus::Out));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::In));
    }

    #[test]
    fn resolve_contradiction_retract_both() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("X", 0.8);
        let b = engine.assert_belief("not X", 0.8);
        let c = engine.detect_contradiction(a, b, -0.95).unwrap();

        engine
            .resolve_contradiction(c.id, ContradictionResolution::RetractBoth)
            .unwrap();

        assert_eq!(engine.get_status(a), Some(BeliefStatus::Out));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::Out));
    }

    #[test]
    fn active_beliefs_filters_correctly() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("active", 0.9);
        let b = engine.assert_belief("also active", 0.8);
        let c = engine.assert_belief("will retract", 0.7);

        engine.retract_belief(c);

        let active: Vec<Uuid> = engine.active_beliefs().iter().map(|b| b.id).collect();
        assert!(active.contains(&a));
        assert!(active.contains(&b));
        assert!(!active.contains(&c));
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn dependents_of_returns_conclusions() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("premise", 0.9);
        let b = engine.assert_belief("conclusion 1", 0.8);
        let c = engine.assert_belief("conclusion 2", 0.7);

        engine
            .add_justification(b, vec![a], JustificationType::Support)
            .unwrap();
        engine
            .add_justification(c, vec![a], JustificationType::Support)
            .unwrap();

        let deps = engine.dependents_of(a);
        assert!(deps.contains(&b));
        assert!(deps.contains(&c));
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn defeat_justification_overrides_support() {
        let mut engine = TmsEngine::new();
        let premise = engine.assert_belief("evidence", 0.9);
        let defeater = engine.assert_belief("counter-evidence", 0.95);
        let target = engine.assert_belief("hypothesis", 0.8);

        // Support from premise.
        engine
            .add_justification(target, vec![premise], JustificationType::Support)
            .unwrap();
        assert_eq!(engine.get_status(target), Some(BeliefStatus::In));

        // Defeat from defeater overrides the support.
        engine
            .add_justification(target, vec![defeater], JustificationType::Defeat)
            .unwrap();
        assert_eq!(engine.get_status(target), Some(BeliefStatus::Out));

        // Retract the defeater — support kicks back in.
        engine.retract_belief(defeater);
        assert_eq!(engine.get_status(target), Some(BeliefStatus::In));
    }

    #[test]
    fn add_justification_validates_ids() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("A", 0.9);
        let bogus = Uuid::new_v4();

        // Unknown conclusion.
        assert!(engine
            .add_justification(bogus, vec![a], JustificationType::Support)
            .is_err());

        // Unknown premise.
        assert!(engine
            .add_justification(a, vec![bogus], JustificationType::Support)
            .is_err());
    }

    #[test]
    fn user_override_resolution() {
        let mut engine = TmsEngine::new();
        let a = engine.assert_belief("A", 0.9);
        let b = engine.assert_belief("B", 0.8);
        let c = engine.detect_contradiction(a, b, -0.9).unwrap();

        // User chooses to keep B.
        engine
            .resolve_contradiction(c.id, ContradictionResolution::UserOverride(b))
            .unwrap();

        assert_eq!(engine.get_status(a), Some(BeliefStatus::Out));
        assert_eq!(engine.get_status(b), Some(BeliefStatus::In));
    }
}
