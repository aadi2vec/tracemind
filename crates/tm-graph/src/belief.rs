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
use tm_tms::{BeliefStatus, Contradiction, ContradictionResolution, TmsEngine};
use uuid::Uuid;

/// User-facing resolution choice from the brief drawer. Maps onto
/// [`tm_tms::ContradictionResolution`] but in *triple-id space* and
/// adds a `KeepBoth` semantic the JTMS doesn't natively have ("these
/// aren't actually contradictory — likely about different times").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResolveChoice {
    /// Keep `triple_a`, retract `triple_b` (mark `Out` in JTMS).
    KeepA,
    /// Keep `triple_b`, retract `triple_a` (mark `Out` in JTMS).
    KeepB,
    /// Both stay `In`. The contradiction is marked resolved (so the
    /// brief stops surfacing it) but neither triple is retracted.
    /// Used when the human reviewer judges the two triples as
    /// referring to different time periods or contexts.
    KeepBoth,
}

/// Project the engine's resolution back into triple-space. Returns
/// `None` for unresolved contradictions and for `RetractBoth`, which
/// `BeliefStore` never emits via its public API (so we treat it as
/// "unmapped" rather than inventing a new variant).
fn project_resolution(r: Option<ContradictionResolution>) -> Option<ResolveChoice> {
    match r? {
        ContradictionResolution::RetractA => Some(ResolveChoice::KeepB),
        ContradictionResolution::RetractB => Some(ResolveChoice::KeepA),
        // The only path that reaches `UserOverride` from BeliefStore
        // is `KeepBoth` (we use it as a sentinel and then force the
        // other belief back to `In`). Anything else came from a
        // direct caller of the engine — treat as KeepBoth too.
        ContradictionResolution::UserOverride(_) => Some(ResolveChoice::KeepBoth),
        ContradictionResolution::RetractBoth => None,
    }
}

/// A contradiction reported back to callers in *triple-id* space.
/// Mirrors `tm_tms::Contradiction` but replaces `belief_a/b` with the
/// originating triple UUIDs so the brief / UI never needs to know
/// belief ids exist.
///
/// `resolution` is `None` while the contradiction is still
/// outstanding (the brief surfaces it as a row to act on). After the
/// user resolves it via the drawer, the sidecar persists the choice
/// so the next process start can replay the resolution and the brief
/// stops showing the row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionView {
    pub id: Uuid,
    pub triple_a: Uuid,
    pub triple_b: Uuid,
    pub detected_at: DateTime<Utc>,
    pub cosine_similarity: f32,
    /// `None` while outstanding; set to a `ResolveChoice` once the
    /// human acted. Persisted so resolutions survive restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ResolveChoice>,
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
            resolution: project_resolution(raw.resolution),
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
                    resolution: project_resolution(c.resolution),
                })
            })
            .collect()
    }

    /// Total number of beliefs the engine is tracking. Used by
    /// rebuild diagnostics.
    pub fn belief_count(&self) -> usize {
        self.triple_to_belief.borrow().len()
    }

    /// Resolve an outstanding contradiction by triple-id. Returns the
    /// affected triple uuids `(retracted, kept)` so the caller can
    /// invalidate caches / persist the new statuses. Returns `None`
    /// if no matching contradiction exists.
    ///
    /// Mapping to the underlying JTMS:
    /// - `KeepA`     → `RetractB`
    /// - `KeepB`     → `RetractA`
    /// - `KeepBoth`  → no JTMS retraction; we manually mark both
    ///                 beliefs back to `In`, then mark the
    ///                 contradiction as resolved with a sentinel
    ///                 `UserOverride(belief_a)` (engine's resolution
    ///                 type can't express "neither retracted", but
    ///                 setting it to anything moves the row out of
    ///                 the unresolved set the brief reads from).
    pub fn resolve_contradiction(
        &self,
        contradiction_id: Uuid,
        choice: ResolveChoice,
    ) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
        // Look up triples + belief ids without holding mutable
        // borrows while we call into the engine.
        let (triple_a, triple_b, belief_a, belief_b) = {
            let engine = self.engine.borrow();
            let raw = engine.contradictions().iter().find(|c| c.id == contradiction_id)?.clone();
            let b2t = self.belief_to_triple.borrow();
            let ta = *b2t.get(&raw.belief_a)?;
            let tb = *b2t.get(&raw.belief_b)?;
            (ta, tb, raw.belief_a, raw.belief_b)
        };

        let (retracted, kept) = match choice {
            ResolveChoice::KeepA => {
                let _ = self
                    .engine
                    .borrow_mut()
                    .resolve_contradiction(contradiction_id, ContradictionResolution::RetractB)
                    .ok()?;
                (vec![triple_b], vec![triple_a])
            }
            ResolveChoice::KeepB => {
                let _ = self
                    .engine
                    .borrow_mut()
                    .resolve_contradiction(contradiction_id, ContradictionResolution::RetractA)
                    .ok()?;
                (vec![triple_a], vec![triple_b])
            }
            ResolveChoice::KeepBoth => {
                // Pin a sentinel resolution so the brief filter sees
                // this row as resolved, then explicitly restore both
                // beliefs to In (UserOverride only sets one to In).
                let mut engine = self.engine.borrow_mut();
                let _ = engine
                    .resolve_contradiction(
                        contradiction_id,
                        ContradictionResolution::UserOverride(belief_a),
                    )
                    .ok()?;
                // The UserOverride branch of resolve_contradiction
                // sets `keep_id` to In and `retract_id` to Out — so
                // belief_b is now Out. Force it back to In since the
                // user said "keep both."
                engine.force_status(belief_b, BeliefStatus::In);
                (Vec::new(), vec![triple_a, triple_b])
            }
        };

        Some((retracted, kept))
    }

    /// Resolve by the stable `(triple_a, triple_b)` pair instead of
    /// the engine-assigned contradiction UUID. Necessary for clients
    /// (Tauri / MCP / CLI) that receive a contradiction id from one
    /// `GraphStore::open` and act on it from a fresh open — IDs
    /// regenerate on every replay but the triple pair doesn't.
    /// Order is direction-insensitive: (a, b) and (b, a) match the
    /// same row, but the resulting `KeepA` / `KeepB` semantics are
    /// applied as if the caller's `triple_a` is the "A" side.
    pub fn resolve_by_triples(
        &self,
        triple_a: Uuid,
        triple_b: Uuid,
        choice: ResolveChoice,
    ) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
        let (id, swapped) = {
            let engine = self.engine.borrow();
            let b2t = self.belief_to_triple.borrow();
            let row = engine.contradictions().iter().find_map(|c| {
                let ta = *b2t.get(&c.belief_a)?;
                let tb = *b2t.get(&c.belief_b)?;
                if ta == triple_a && tb == triple_b {
                    Some((c.id, false))
                } else if ta == triple_b && tb == triple_a {
                    Some((c.id, true))
                } else {
                    None
                }
            })?;
            row
        };
        // If the triples were stored in the opposite order from what
        // the caller passed, flip the choice so KeepA still keeps the
        // caller's `triple_a`.
        let effective = if swapped {
            match choice {
                ResolveChoice::KeepA => ResolveChoice::KeepB,
                ResolveChoice::KeepB => ResolveChoice::KeepA,
                ResolveChoice::KeepBoth => ResolveChoice::KeepBoth,
            }
        } else {
            choice
        };
        self.resolve_contradiction(id, effective)
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
            // Re-detect rebuilds the contradiction in the engine with
            // a fresh id. If the persisted row had a resolution, we
            // re-apply it against that fresh id so the engine ends up
            // in the same `(Contradicted | resolved)` state as before
            // the restart — otherwise resolved rows would resurrect
            // unresolved on every process start.
            if let Some(view) =
                self.detect_contradiction(r.triple_a, r.triple_b, r.cosine_similarity)
            {
                if let Some(choice) = r.resolution {
                    let _ = self.resolve_contradiction(view.id, choice);
                }
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
    fn resolve_keep_a_retracts_b() {
        let (store, _, _, t1, t2) = fresh();
        let c = store.detect_contradiction(t1, t2, -0.95).unwrap();
        let (retracted, kept) = store
            .resolve_contradiction(c.id, ResolveChoice::KeepA)
            .unwrap();
        assert_eq!(retracted, vec![t2]);
        assert_eq!(kept, vec![t1]);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::In));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::Out));
        // The brief filter looks at .resolution.is_none() — verify the
        // engine now reports the contradiction as resolved.
        let listed = store.contradictions();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].resolution, Some(ResolveChoice::KeepA));
    }

    #[test]
    fn resolve_keep_b_retracts_a() {
        let (store, _, _, t1, t2) = fresh();
        let c = store.detect_contradiction(t1, t2, -0.95).unwrap();
        let (retracted, kept) = store
            .resolve_contradiction(c.id, ResolveChoice::KeepB)
            .unwrap();
        assert_eq!(retracted, vec![t1]);
        assert_eq!(kept, vec![t2]);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::Out));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::In));
    }

    #[test]
    fn resolve_by_triples_swaps_choice_when_pair_is_reversed() {
        let (store, _, _, t1, t2) = fresh();
        let _ = store.detect_contradiction(t1, t2, -0.95).unwrap();
        // Caller passes (t2, t1) — engine stored (t1, t2). KeepA from
        // the caller's perspective should keep t2.
        let (retracted, kept) = store
            .resolve_by_triples(t2, t1, ResolveChoice::KeepA)
            .unwrap();
        assert_eq!(retracted, vec![t1]);
        assert_eq!(kept, vec![t2]);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::Out));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::In));
    }

    #[test]
    fn resolve_keep_both_leaves_both_in() {
        let (store, _, _, t1, t2) = fresh();
        let c = store.detect_contradiction(t1, t2, -0.95).unwrap();
        let (retracted, kept) = store
            .resolve_contradiction(c.id, ResolveChoice::KeepBoth)
            .unwrap();
        assert!(retracted.is_empty());
        assert_eq!(kept.len(), 2);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::In));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::In));
        let listed = store.contradictions();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].resolution, Some(ResolveChoice::KeepBoth));
    }

    #[test]
    fn resolution_round_trips_through_save_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contradictions.json");

        // First store: detect, resolve, save.
        let s = Uuid::new_v4();
        let o = Uuid::new_v4();
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        {
            let store = BeliefStore::new();
            store.assert_for_triple(t1, s, "likes", o, 0.9);
            store.assert_for_triple(t2, s, "dislikes", o, 0.85);
            let c = store.detect_contradiction(t1, t2, -0.95).unwrap();
            store
                .resolve_contradiction(c.id, ResolveChoice::KeepA)
                .unwrap();
            store.save_contradictions(&path).unwrap();
        }

        // Second store: same triples, replay sidecar — resolution
        // should re-apply so the contradiction is still resolved.
        let store = BeliefStore::new();
        store.assert_for_triple(t1, s, "likes", o, 0.9);
        store.assert_for_triple(t2, s, "dislikes", o, 0.85);
        let replayed = store.replay_contradictions(&path).unwrap();
        assert_eq!(replayed, 1);
        assert_eq!(store.status_for(t1), Some(BeliefStatus::In));
        assert_eq!(store.status_for(t2), Some(BeliefStatus::Out));
        let listed = store.contradictions();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].resolution, Some(ResolveChoice::KeepA));
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
