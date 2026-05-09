//! Working memory — short-term in-RAM context window.
//!
//! Holds the last N "turns" (a turn = one user query + the entities
//! the retriever returned). The brain analogue is the ~7±2 short-term
//! buffer. Two jobs:
//!
//! 1. **Conversational continuity.** Follow-up questions like
//!    "and what about X?" can inherit the topic centroid + entity set
//!    from the previous turn instead of cold-starting BGE every time.
//! 2. **Trigger-context fingerprint.** [`Anticipation::trigger`] in
//!    `tm-intent` carries a `working_memory_hash` + `topic_centroid`;
//!    this struct is the canonical source of those values so the
//!    world model trains on the same fingerprints it predicts against.
//!
//! Lives in RAM only. We don't persist working memory across CLI
//! invocations on purpose — the whole point is that it represents
//! "right now". Cross-session continuity is the job of the trace log
//! and the world model, not WM.
//!
//! See `docs/INTENT_SYSTEM.md` §6 (predictive layer L1) and
//! `docs/PHASE4_DELIGHT.md` §4.5.

use std::collections::VecDeque;
use std::hash::Hasher;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One conversational turn in working memory.
///
/// Compact on purpose — WM should not become a full retrieval log.
/// The `topic_centroid` is the BGE-small embedding of the query (or
/// the retriever's chosen pivot); we keep it as `Vec<f32>` rather
/// than baking a dimension constant in so the buffer stays usable
/// when the embedder is swapped (hash embedder for tests, alternate
/// dims later).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingTurn {
    /// Raw user query for this turn.
    pub query: String,
    /// Embedding centroid summarizing the turn's focus. Empty when
    /// the caller has no embedding handy (still a valid turn — we
    /// just won't contribute to the WM centroid).
    pub topic_centroid: Vec<f32>,
    /// Entity IDs the retriever surfaced. Order is significant — it
    /// reflects rank — but duplicates within a turn are *not*
    /// deduplicated here; that's the caller's call.
    pub entity_ids: Vec<Uuid>,
    /// Wall-clock time the turn was recorded.
    pub timestamp: DateTime<Utc>,
}

impl WorkingTurn {
    /// Build a turn at "now" with the supplied query / centroid /
    /// entity-ids.
    pub fn new(
        query: impl Into<String>,
        topic_centroid: Vec<f32>,
        entity_ids: Vec<Uuid>,
    ) -> Self {
        Self {
            query: query.into(),
            topic_centroid,
            entity_ids,
            timestamp: Utc::now(),
        }
    }
}

/// In-RAM ring buffer of recent [`WorkingTurn`]s. Bounded to
/// [`WorkingMemory::DEFAULT_CAPACITY`] by default.
///
/// Cloning is cheap relative to a vector copy and explicit on
/// purpose: handing a `&WorkingMemory` to the world model and a
/// `WorkingMemory` clone to the bandit is a typical pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemory {
    capacity: usize,
    turns: VecDeque<WorkingTurn>,
}

impl WorkingMemory {
    /// Default capacity. Brain analogue: ~7±2. We pick 8 — the lower
    /// edge of the 7±2 range, plus one. Empirically this is enough
    /// for "and what about X?" follow-ups without dragging stale
    /// topic shifts forward.
    pub const DEFAULT_CAPACITY: usize = 8;

    /// New WM with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    /// New WM with a custom capacity. Capacity 0 is allowed — the WM
    /// then accepts pushes but never retains anything (useful for
    /// tests that want the API surface without the state).
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            turns: VecDeque::with_capacity(capacity),
        }
    }

    /// Append a turn. If the buffer is full, the oldest turn is
    /// evicted FIFO. Capacity-0 buffers drop the turn on the floor.
    pub fn push(&mut self, turn: WorkingTurn) {
        if self.capacity == 0 {
            return;
        }
        if self.turns.len() == self.capacity {
            self.turns.pop_front();
        }
        self.turns.push_back(turn);
    }

    /// Iterate turns oldest-first. Most recent is `last()`.
    pub fn turns(&self) -> impl Iterator<Item = &WorkingTurn> {
        self.turns.iter()
    }

    /// Most recent turn, if any.
    pub fn last(&self) -> Option<&WorkingTurn> {
        self.turns.back()
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Clear all turns. Useful for "new session" boundaries.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Stable, deterministic fingerprint of the WM contents — the
    /// value `Anticipation::trigger.working_memory_hash` should carry.
    ///
    /// Hashes the query strings + entity IDs in turn order. Topic
    /// centroids are deliberately *excluded*: float comparisons are
    /// fragile and the centroid is recomputed from the same inputs
    /// elsewhere anyway. Same content → same hash, regardless of
    /// process or platform.
    ///
    /// Uses `seahash` (fast, non-crypto). The hash is not a security
    /// surface; it's a deduplication key.
    pub fn fingerprint(&self) -> String {
        let mut h = seahash::SeaHasher::new();
        for turn in &self.turns {
            h.write(turn.query.as_bytes());
            h.write_u8(0); // separator
            for id in &turn.entity_ids {
                h.write(id.as_bytes());
            }
            h.write_u8(1); // turn boundary
        }
        format!("wm-{:016x}", h.finish())
    }

    /// Mean topic centroid over turns that contributed an embedding.
    ///
    /// Returns an empty `Vec` when no turn has a centroid (or when the
    /// dimensions disagree across turns, in which case mismatched
    /// turns are skipped — defensive against embedder swaps).
    pub fn topic_centroid(&self) -> Vec<f32> {
        let dim = self
            .turns
            .iter()
            .find(|t| !t.topic_centroid.is_empty())
            .map(|t| t.topic_centroid.len())
            .unwrap_or(0);
        if dim == 0 {
            return Vec::new();
        }

        let mut sum = vec![0.0_f32; dim];
        let mut n = 0usize;
        for t in &self.turns {
            if t.topic_centroid.len() != dim {
                continue;
            }
            for (i, v) in t.topic_centroid.iter().enumerate() {
                sum[i] += *v;
            }
            n += 1;
        }
        if n == 0 {
            return Vec::new();
        }
        let scale = 1.0 / n as f32;
        for v in &mut sum {
            *v *= scale;
        }
        sum
    }

    /// Union of entity IDs across all turns, oldest-first, with
    /// duplicates suppressed — handy when seeding a follow-up
    /// retrieval from the WM context.
    pub fn entity_set(&self) -> Vec<Uuid> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for t in &self.turns {
            for id in &t.entity_ids {
                if seen.insert(*id) {
                    out.push(*id);
                }
            }
        }
        out
    }
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(q: &str, c: Vec<f32>, ids: Vec<Uuid>) -> WorkingTurn {
        WorkingTurn::new(q, c, ids)
    }

    #[test]
    fn ring_evicts_oldest_when_full() {
        let mut wm = WorkingMemory::with_capacity(3);
        for i in 0..5 {
            wm.push(turn(&format!("q{i}"), vec![], vec![]));
        }
        let queries: Vec<&str> = wm.turns().map(|t| t.query.as_str()).collect();
        assert_eq!(queries, vec!["q2", "q3", "q4"]);
        assert_eq!(wm.len(), 3);
        assert_eq!(wm.last().unwrap().query, "q4");
    }

    #[test]
    fn capacity_zero_drops_pushes() {
        let mut wm = WorkingMemory::with_capacity(0);
        wm.push(turn("q", vec![1.0], vec![Uuid::nil()]));
        assert!(wm.is_empty());
        assert_eq!(wm.capacity(), 0);
        assert!(wm.fingerprint().starts_with("wm-"));
    }

    #[test]
    fn fingerprint_is_stable_for_same_content() {
        let id = Uuid::new_v4();
        let mut a = WorkingMemory::new();
        a.push(turn("hello", vec![0.1, 0.2], vec![id]));
        a.push(turn("world", vec![0.3, 0.4], vec![]));

        let mut b = WorkingMemory::new();
        b.push(turn("hello", vec![9.9, 9.9], vec![id])); // centroid ignored
        b.push(turn("world", vec![], vec![]));

        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_query_order() {
        let id = Uuid::new_v4();
        let mut a = WorkingMemory::new();
        a.push(turn("hello", vec![], vec![id]));
        a.push(turn("world", vec![], vec![]));

        let mut b = WorkingMemory::new();
        b.push(turn("world", vec![], vec![]));
        b.push(turn("hello", vec![], vec![id]));

        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_entity_set() {
        let mut a = WorkingMemory::new();
        a.push(turn("q", vec![], vec![Uuid::from_u128(1)]));

        let mut b = WorkingMemory::new();
        b.push(turn("q", vec![], vec![Uuid::from_u128(2)]));

        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn topic_centroid_averages_only_embedded_turns() {
        let mut wm = WorkingMemory::new();
        wm.push(turn("a", vec![1.0, 0.0], vec![]));
        wm.push(turn("b", vec![], vec![])); // skipped
        wm.push(turn("c", vec![3.0, 4.0], vec![]));
        let c = wm.topic_centroid();
        assert_eq!(c.len(), 2);
        assert!((c[0] - 2.0).abs() < 1e-6);
        assert!((c[1] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn topic_centroid_empty_when_no_embeddings() {
        let mut wm = WorkingMemory::new();
        wm.push(turn("a", vec![], vec![]));
        wm.push(turn("b", vec![], vec![]));
        assert!(wm.topic_centroid().is_empty());
    }

    #[test]
    fn topic_centroid_skips_dim_mismatch() {
        let mut wm = WorkingMemory::new();
        wm.push(turn("a", vec![1.0, 1.0], vec![]));
        wm.push(turn("b", vec![5.0, 5.0, 5.0], vec![])); // skipped
        wm.push(turn("c", vec![3.0, 3.0], vec![]));
        let c = wm.topic_centroid();
        assert_eq!(c, vec![2.0, 2.0]);
    }

    #[test]
    fn entity_set_dedups_oldest_first() {
        let id1 = Uuid::from_u128(1);
        let id2 = Uuid::from_u128(2);
        let id3 = Uuid::from_u128(3);

        let mut wm = WorkingMemory::new();
        wm.push(turn("a", vec![], vec![id1, id2]));
        wm.push(turn("b", vec![], vec![id2, id3, id1]));

        assert_eq!(wm.entity_set(), vec![id1, id2, id3]);
    }

    #[test]
    fn clear_resets_buffer_but_keeps_capacity() {
        let mut wm = WorkingMemory::with_capacity(5);
        wm.push(turn("a", vec![], vec![]));
        wm.push(turn("b", vec![], vec![]));
        wm.clear();
        assert!(wm.is_empty());
        assert_eq!(wm.capacity(), 5);
    }

    #[test]
    fn empty_wm_has_stable_fingerprint() {
        let a = WorkingMemory::new().fingerprint();
        let b = WorkingMemory::new().fingerprint();
        assert_eq!(a, b);
        assert!(a.starts_with("wm-"));
    }

    #[test]
    fn round_trips_through_serde_json() {
        let mut wm = WorkingMemory::with_capacity(4);
        wm.push(turn("hello", vec![0.5, -0.5], vec![Uuid::from_u128(7)]));
        let s = serde_json::to_string(&wm).unwrap();
        let back: WorkingMemory = serde_json::from_str(&s).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.fingerprint(), wm.fingerprint());
        assert_eq!(back.capacity(), 4);
    }
}
