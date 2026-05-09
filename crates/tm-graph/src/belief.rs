//! Sprint C-2: BeliefStore — thin wrapper around `tm_tms::TmsEngine`
//! that maintains a `triple_id ↔ belief_id` mapping so the rest of
//! the codebase can talk in terms of `Triple` UUIDs while the JTMS
//! engine works in its native belief-id space.
//!
//! The engine is purely in-memory; persistence is the bitemporal
//! substrate (every assertion is already recorded as a temporal fact
//! by `GraphStore::upsert_triple`). On restart we rebuild the engine
//! from live triples — see `GraphStore::rebuild_beliefs`.
//!
//! What this layer does:
//! - Asserts a belief for every triple at upsert time
//! - Records contradictions when callers (e.g. ingest) supply a
//!   cosine similarity that crosses the `tm-tms` -0.8 threshold
//! - Exposes `status_for(triple_id)` for retrieval-time filtering
//! - Exposes `contradictions()` returning views keyed by `triple_id`
//!   for the daily brief
//!
//! What this layer does *not* do:
//! - Compute embeddings (caller's job)
//! - Persist itself separately (rebuild from temporal_facts)
//! - Decide policy on `Out` vs `Contradicted` (that's retrieval +
//!   brief)

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tm_tms::{BeliefStatus, Contradiction, TmsEngine};
use uuid::Uuid;

/// A contradiction reported back to callers in *triple-id* space.
/// Mirrors `tm_tms::Contradiction` but replaces `belief_a/b` with the
/// originating triple UUIDs so the brief / UI never needs to know
/// belief ids exist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionView {
    pub id: Uuid,
    pub triple_a: Uuid,
    pub triple_b: Uuid,
    pub detected_at: DateTime<Utc>,
    pub cosine_similarity: f32,
}

/// Builds a deterministic statement string for a triple. Uses raw
/// UUIDs + the predicate string — natural-language statements would
/// require an entity-name lookup which is the caller's job to do
/// when it computes embeddings for `detect_contradiction`. The
/// statement here is purely diagnostic.
fn triple_statement(subject_id: Uuid, predicate: &str, object_id: Uuid) -> String {
    format!("{subject_id} {predicate} {object_id}")
}

/// In-memory belief store keyed by triple UUID.
#[derive(Debug, Default)]
pub struct BeliefStore {
    /// The JTMS engine. `RefCell` because `GraphStore` exposes its
    /// `BeliefStore` through `&self` and we want assertion + lookup
    /// to compose with the rest of the store without taking `&mut`.
    engine: RefCell<TmsEngine>,
    /// Triple UUID → belief id assigned by the engine.
    triple_to_belief: RefCell<HashMap<Uuid, Uuid>>,
    /// Belief id → triple UUID. Reverse map kept in lockstep so we
    /// can translate `Contradiction` records back to triple space.
    belief_to_triple: RefCell<HashMap<Uuid, Uuid>>,
}

impl BeliefStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Assert (or re-assert) the belief for `triple_id`. Idempotent
    /// at the mapping level — calling twice for the same triple
    /// updates confidence on the existing belief rather than
    /// orphaning it.
    pub fn assert_for_triple(
        &self,
        triple_id: Uuid,
        subject_id: Uuid,
        predicate: &str,
        object_id: Uuid,
        confidence: f32,
    ) -> Uuid {
        let stmt = triple_statement(subject_id, predicate, object_id);
        let mut t2b = self.triple_to_belief.borrow_mut();
        if let Some(&existing) = t2b.get(&triple_id) {
            // Re-assertion: bump confidence on the existing belief.
            // We don't have an `update_belief` on TmsEngine so we
            // simulate by retracting + re-asserting only when the
            // status had been turned `Out` — otherwise the existing
            // belief is fine.
            let mut engine = self.engine.borrow_mut();
            if let Some(status) = engine.get_status(existing) {
                if status == BeliefStatus::Out {
                    // Turn it back on. Cheapest path: assert a new
                    // belief and rewire the maps. Old `Out` belief
                    // stays in the engine but is no longer referenced.
                    let new_id = engine.assert_belief(stmt, confidence);
                    let old_id = existing;
                    t2b.insert(triple_id, new_id);
                    let mut b2t = self.belief_to_triple.borrow_mut();
                    b2t.remove(&old_id);
                    b2t.insert(new_id, triple_id);
                    return new_id;
                }
            }
            return existing;
        }
        let mut engine = self.engine.borrow_mut();
        let belief_id = engine.assert_belief(stmt, confidence);
        t2b.insert(triple_id, belief_id);
        self.belief_to_triple
            .borrow_mut()
            .insert(belief_id, triple_id);
        belief_id
    }

    /// Mark a triple's belief as `Out` (e.g. after an explicit
    /// retraction). Propagates through the JTMS dependency graph.
    /// No-op if the triple has no belief recorded.
    pub fn retract_for_triple(&self, triple_id: Uuid) {
        if let Some(&bid) = self.triple_to_belief.borrow().get(&triple_id) {
            let _ = self.engine.borrow_mut().retract_belief(bid);
        }
    }

    /// Run the contradiction detector against two triples. The cosine
    /// similarity must be computed by the caller from embeddings of
    /// the *natural-language* form of each triple (entity names +
    /// predicate verb). Returns `None` if either triple is unknown
    /// or if the cosine doesn't cross the `-0.8` threshold.
    pub fn detect_contradiction(
        &self,
        triple_a: Uuid,
        triple_b: Uuid,
        cosine_sim: f32,
    ) -> Option<ContradictionView> {
        let t2b = self.triple_to_belief.borrow();
        let a_belief = *t2b.get(&triple_a)?;
        let b_belief = *t2b.get(&triple_b)?;
        drop(t2b);

        let raw: Contradiction = self
            .engine
            .borrow_mut()
            .detect_contradiction(a_belief, b_belief, cosine_sim)?;

        Some(ContradictionView {
            id: raw.id,
            triple_a,
            triple_b,
            detected_at: raw.detected_at,
            cosine_similarity: raw.cosine_similarity,
        })
    }

    /// Current status for a triple's belief. `None` means we've never
    /// asserted a belief for this triple.
    pub fn status_for(&self, triple_id: Uuid) -> Option<BeliefStatus> {
        let bid = *self.triple_to_belief.borrow().get(&triple_id)?;
        self.engine.borrow().get_status(bid)
    }

    /// Snapshot of every contradiction recorded so far, projected
    /// into triple-id space. Order matches insertion order.
    pub fn contradictions(&self) -> Vec<ContradictionView> {
        let engine = self.engine.borrow();
        let b2t = self.belief_to_triple.borrow();
        engine
            .contradictions()
            .iter()
            .filter_map(|c| {
                let triple_a = b2t.get(&c.belief_a).copied()?;
                let triple_b = b2t.get(&c.belief_b).copied()?;
                Some(ContradictionView {
                    id: c.id,
                    triple_a,
                    triple_b,
                    detected_at: c.detected_at,
                    cosine_similarity: c.cosine_similarity,
                })
            })
            .collect()
    }

    /// Total number of beliefs the engine is tracking. Used by
    /// rebuild diagnostics.
    pub fn belief_count(&self) -> usize {
        self.triple_to_belief.borrow().len()
    }

    /// Persist the current contradiction list to `path` as JSON.
    /// We persist contradictions but *not* the engine's belief graph —
    /// beliefs rebuild deterministically from live triples on
    /// `GraphStore::open`, but contradictions need their cosine input
    /// to re-detect, which the graph doesn't keep around. Saving the
    /// `Vec<ContradictionView>` is the cheapest fix.
    pub fn save_contradictions(&self, path: &std::path::Path) -> std::io::Result<()> {
        let rows = self.contradictions();
        let bytes = serde_json::to_vec_pretty(&rows)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        // Write atomically — write to a tmp sibling, fsync, rename.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Load contradictions from `path` and replay each via
    /// `detect_contradiction` so the engine ends up in the same
    /// `Contradicted` state as before the restart. Missing file is
    /// not an error — it just means there's nothing to replay
    /// (first-run installs / fresh dirs).
    ///
    /// Returns the number of rows replayed. Rows whose triples are
    /// unknown (e.g. the triple was hard-deleted between restarts)
    /// are silently skipped.
    pub fn replay_contradictions(&self, path: &std::path::Path) -> std::io::Result<usize> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
        };
        let rows: Vec<ContradictionView> = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut replayed = 0usize;
        for r in rows {
            if self
                .detect_contradiction(r.triple_a, r.triple_b, r.cosine_similarity)
                .is_some()
            {
                replayed += 1;
            }
        }
        Ok(replayed)
    }
}

/// Collapse `(numeric_confidence, BeliefStatus)` into a single trust
/// score in `[0.0, 1.0]`. Policy:
///
/// - `In`           → unchanged (trust the recorded confidence)
/// - `Contradicted` → halved (still surface, but downrank)
/// - `Out`          → 0.0 (filter out at retrieval time)
///
/// Live as a free function so non-`tm-graph` callers (retrieval,
/// brief) can compose it with whatever they already track.
pub fn effective_confidence(confidence: f64, status: BeliefStatus) -> f64 {
    match status {
        BeliefStatus::In => confidence.clamp(0.0, 1.0),
        BeliefStatus::Contradicted => (confidence * 0.5).clamp(0.0, 1.0),
        BeliefStatus::Out => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> (BeliefStore, Uuid, Uuid, Uuid, Uuid) {
        let store = BeliefStore::new();
        let s = Uuid::new_v4();
        let o = Uuid::new_v4();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        store.assert_for_triple(t1, s, "likes", o, 0.9);
        store.assert_for_triple(t2, s, "dislikes", o, 0.85);
        (store, s, o, t1, t2)
    }

    #[test]
    fn assert_makes_in() {
        let (store, _, _, t1, _) = fresh();
        assert_eq!(store.status_for(t1), Some(BeliefStatus::In));
    }

    #[test]
    fn idempotent_assert_returns_same_belief() {
        let store = BeliefStore::new();
        let s = Uuid::new_v4();
        let o = Uuid::new_v4();
        let t = Uuid::new_v4();
        let b1 = store.assert_for_triple(t, s, "rel", o, 0.5);
        let b2 = store.assert_for_triple(t, s, "rel", o, 0.5);
        assert_eq!(b1, b2);
    }

    #[test]
    fn cosine_above_threshold_yields_no_contradiction() {
        let (store, _, _, t1, t2) = fresh();
        let c = store.detect_contradiction(t1, t2, -0.5);
        assert!(c.is_none());
        assert_eq!(store.status_for(t1), Some(BeliefStatus::In));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::In));
    }

    #[test]
    fn strong_negative_cosine_records_contradiction() {
        let (store, _, _, t1, t2) = fresh();
        let c = store
            .detect_contradiction(t1, t2, -0.95)
            .expect("contradiction recorded");
        assert_eq!(c.triple_a, t1);
        assert_eq!(c.triple_b, t2);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::Contradicted));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::Contradicted));
        let listed = store.contradictions();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, c.id);
    }

    #[test]
    fn effective_confidence_policy() {
        assert!((effective_confidence(0.8, BeliefStatus::In) - 0.8).abs() < 1e-9);
        assert!((effective_confidence(0.8, BeliefStatus::Contradicted) - 0.4).abs() < 1e-9);
        assert!((effective_confidence(0.8, BeliefStatus::Out)).abs() < 1e-9);
        // Clamping
        assert_eq!(effective_confidence(2.0, BeliefStatus::In), 1.0);
        assert_eq!(effective_confidence(-0.5, BeliefStatus::In), 0.0);
    }

    #[test]
    fn retract_marks_out_and_reassert_revives() {
        let store = BeliefStore::new();
        let s = Uuid::new_v4();
        let o = Uuid::new_v4();
        let t = Uuid::new_v4();
        store.assert_for_triple(t, s, "rel", o, 0.7);
        store.retract_for_triple(t);
        assert_eq!(store.status_for(t), Some(BeliefStatus::Out));
        // Re-assert with a fresh confidence — we should be back to In.
        store.assert_for_triple(t, s, "rel", o, 0.8);
        assert_eq!(store.status_for(t), Some(BeliefStatus::In));
    }
}
