//! ComposedIndex: combines multiple retrieval Spaces with per-verb weight vectors.
//!
//! Implements the Superlinked-style ComposedIndex pattern from the H2 2026
//! charter §5 Pillar 2. Each registered Space contributes a score; the final
//! score is a weighted sum normalised by verb weight vectors.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;
use crate::space::{ConfidenceSpace, LexicalSpace, MemoryMeta, RecencySpace, Space, TextSpace};

// ---------------------------------------------------------------------------
// VerbWeights
// ---------------------------------------------------------------------------

/// Query-time weight vector for a verb. Weights are normalised to sum to 1.0
/// before scoring. Unspecified spaces get weight 0.0.
#[derive(Debug, Clone)]
pub struct VerbWeights {
    pub verb: String,
    pub weights: HashMap<String, f32>,
}

impl VerbWeights {
    pub fn new(verb: impl Into<String>, pairs: &[(&str, f32)]) -> Self {
        let weights = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        Self {
            verb: verb.into(),
            weights,
        }
    }

    /// Normalise so all weights sum to 1.0.
    pub fn normalised(&self) -> HashMap<String, f32> {
        let total: f32 = self.weights.values().sum();
        if total <= 0.0 {
            return self.weights.clone();
        }
        self.weights
            .iter()
            .map(|(k, v)| (k.clone(), v / total))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Default verb weight vectors
// ---------------------------------------------------------------------------

/// Default verb weight vectors (charter §5 Pillar 2).
///
/// `lexical` is the BM25 sparse space. It carries real weight on `recall`
/// because exact tokens (names, times, amounts) are what make a stored turn
/// *the* answer, and dense cosine over short conversational turns routinely
/// ranks a topically-similar turn above the one holding the literal answer.
/// The `recall` vector is the output of a GEPA run against the LoCoMo
/// anchor set (see `docs/REVIEW-2026-07.md`); the others are hand-set
/// defaults awaiting their own anchor sets.
pub fn default_verb_weights() -> Vec<VerbWeights> {
    vec![
        VerbWeights::new("recall",     &[("text", 0.478), ("lexical", 0.348), ("recency", 0.087), ("confidence", 0.087)]),
        VerbWeights::new("plan",       &[("text", 0.3), ("lexical", 0.2), ("recency", 0.35), ("confidence", 0.15)]),
        VerbWeights::new("contradict", &[("text", 0.35), ("lexical", 0.3), ("confidence", 0.2), ("recency", 0.15)]),
        VerbWeights::new("reflect",    &[("text", 0.25), ("lexical", 0.15), ("recency", 0.45), ("confidence", 0.15)]),
    ]
}

// ---------------------------------------------------------------------------
// ComposedIndex
// ---------------------------------------------------------------------------

/// Composed retrieval index: combines multiple Spaces with per-verb weights.
///
/// Each registered Space contributes a score; the final score is a weighted
/// sum. A pre-computed cosine similarity from `VectorStore` can be passed in
/// as the `text` space score.
///
/// Q4.6: pinned memory IDs always score 1.0 regardless of other spaces.
pub struct ComposedIndex {
    spaces: Vec<Box<dyn Space>>,
    verb_weights: Vec<VerbWeights>,
    /// Q4.6 — pinned memory IDs always return score 1.0.
    pinned_ids: HashSet<Uuid>,
}

impl ComposedIndex {
    pub fn new(spaces: Vec<Box<dyn Space>>, verb_weights: Vec<VerbWeights>) -> Self {
        Self { spaces, verb_weights, pinned_ids: HashSet::new() }
    }

    /// Build the default three-space index (text + recency + confidence).
    pub fn default_three_space() -> Self {
        Self::new(
            vec![
                Box::new(TextSpace),
                Box::new(RecencySpace::default()),
                Box::new(ConfidenceSpace),
            ],
            default_verb_weights(),
        )
    }

    /// Build the default hybrid index: dense `text` + sparse `lexical` +
    /// `recency` + `confidence`. This is what the live retrieval path uses.
    pub fn default_hybrid() -> Self {
        Self::new(
            vec![
                Box::new(TextSpace),
                Box::new(LexicalSpace),
                Box::new(RecencySpace::default()),
                Box::new(ConfidenceSpace),
            ],
            default_verb_weights(),
        )
    }

    /// Q4.6 — Pin a memory ID so it always scores 1.0.
    pub fn pin(&mut self, id: Uuid) {
        self.pinned_ids.insert(id);
    }

    /// Q4.6 — Unpin a memory ID.
    pub fn unpin(&mut self, id: &Uuid) {
        self.pinned_ids.remove(id);
    }

    /// Q4.6 — Replace the full pinned set (e.g., loaded from TierStore).
    pub fn set_pinned(&mut self, ids: HashSet<Uuid>) {
        self.pinned_ids = ids;
    }

    /// Whether a memory ID is currently pinned.
    pub fn is_pinned(&self, id: &Uuid) -> bool {
        self.pinned_ids.contains(id)
    }

    /// Score a memory entry given a pre-computed text cosine similarity.
    ///
    /// `text_cosine` — cosine similarity from `VectorStore.search()` (0..1).
    /// `verb` — active MCP verb (recall / plan / contradict / reflect).
    ///          Defaults to "recall" if unknown.
    /// Q4.6: pinned memories always return 1.0 regardless of other spaces.
    pub fn score(
        &self,
        query_text: &str,
        meta: &MemoryMeta,
        text_cosine: f32,
        verb: Option<&str>,
    ) -> f32 {
        // Q4.6 — pinned memories are always surfaced at maximum score.
        if self.pinned_ids.contains(&meta.id) {
            return 1.0;
        }

        let verb = verb.unwrap_or("recall");
        let weights = self.weights_for_verb(verb);

        let mut total = 0.0f32;
        for space in &self.spaces {
            let w = weights.get(space.name()).copied().unwrap_or(0.0);
            if w <= 0.0 {
                continue;
            }
            // For TextSpace: use the pre-computed cosine rather than re-embedding.
            let raw_score = if space.name() == "text" {
                text_cosine
            } else {
                space.score(query_text, meta.id, meta)
            };
            total += w * raw_score;
        }
        total.clamp(0.0, 1.0)
    }

    /// Fuse pre-computed per-space scores using this verb's weight vector.
    ///
    /// Some spaces cannot be scored from [`MemoryMeta`] alone — `text` needs
    /// the cosine from the vector store and `lexical` needs the document body
    /// and corpus statistics. Callers that already hold those scores fuse
    /// them here so the weight vector stays the single source of truth for
    /// how spaces combine (and stays GEPA-mutable in one place).
    ///
    /// Unknown space names are ignored. Weights are renormalised over the
    /// spaces actually supplied, so omitting a space degrades gracefully
    /// instead of silently shrinking every score.
    pub fn score_precomputed(&self, scores: &HashMap<String, f32>, verb: Option<&str>) -> f32 {
        let weights = self.weights_for_verb(verb.unwrap_or("recall"));
        let mut total = 0.0f32;
        let mut weight_sum = 0.0f32;
        for (space, score) in scores {
            let w = weights.get(space).copied().unwrap_or(0.0);
            if w <= 0.0 {
                continue;
            }
            total += w * score;
            weight_sum += w;
        }
        if weight_sum <= 0.0 {
            return 0.0;
        }
        (total / weight_sum).clamp(0.0, 1.0)
    }

    /// Replace this index's verb weight vectors (used by the GEPA loop to
    /// apply a mutated policy without rebuilding the spaces).
    pub fn set_verb_weights(&mut self, weights: Vec<VerbWeights>) {
        self.verb_weights = weights;
    }

    pub fn verb_weights(&self) -> &[VerbWeights] {
        &self.verb_weights
    }

    fn weights_for_verb(&self, verb: &str) -> HashMap<String, f32> {
        self.verb_weights
            .iter()
            .find(|vw| vw.verb == verb)
            .map(|vw| vw.normalised())
            .unwrap_or_else(|| {
                // Default: all weight on text
                let mut m = HashMap::new();
                m.insert("text".to_string(), 1.0);
                m
            })
    }

    pub fn space_names(&self) -> Vec<&str> {
        self.spaces.iter().map(|s| s.name()).collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::MemoryMeta;
    use chrono::Utc;
    use uuid::Uuid;

    fn meta_now() -> MemoryMeta {
        MemoryMeta {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            last_accessed_at: None,
            confidence: 0.9,
            entity_kind: None,
            source: None,
            session_id: None,
            host_id: None,
        }
    }

    #[test]
    fn default_three_space_scores_range() {
        let idx = ComposedIndex::default_three_space();
        let meta = meta_now();
        let s = idx.score("test query", &meta, 0.8, Some("recall"));
        assert!(s >= 0.0 && s <= 1.0, "score out of range: {s}");
    }

    #[test]
    fn verb_weights_normalise() {
        let vw = VerbWeights::new("test", &[("a", 2.0), ("b", 2.0)]);
        let n = vw.normalised();
        assert!((n["a"] - 0.5).abs() < 1e-6);
        assert!((n["b"] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn pinned_memory_scores_one() {
        let mut idx = ComposedIndex::default_three_space();
        let meta = meta_now();
        idx.pin(meta.id);
        // A pinned memory always scores 1.0 regardless of cosine
        assert_eq!(idx.score("anything", &meta, 0.1, Some("recall")), 1.0);
    }

    #[test]
    fn unpinned_memory_uses_space_scoring() {
        let mut idx = ComposedIndex::default_three_space();
        let meta = meta_now();
        let id = meta.id;
        idx.pin(id);
        idx.unpin(&id);
        let s = idx.score("test", &meta, 0.5, Some("recall"));
        // Should NOT be 1.0 after unpinning
        assert!(s < 1.0, "unpinned memory should use normal scoring: {s}");
        assert!(s >= 0.0);
    }

    #[test]
    fn is_pinned_reflects_state() {
        let mut idx = ComposedIndex::default_three_space();
        let id = Uuid::new_v4();
        assert!(!idx.is_pinned(&id));
        idx.pin(id);
        assert!(idx.is_pinned(&id));
        idx.unpin(&id);
        assert!(!idx.is_pinned(&id));
    }

    #[test]
    fn default_verb_weights_all_four_verbs() {
        let vws = default_verb_weights();
        let verbs: Vec<&str> = vws.iter().map(|v| v.verb.as_str()).collect();
        assert!(verbs.contains(&"recall"));
        assert!(verbs.contains(&"plan"));
        assert!(verbs.contains(&"contradict"));
        assert!(verbs.contains(&"reflect"));
    }

    #[test]
    fn space_names_default_three() {
        let idx = ComposedIndex::default_three_space();
        let names = idx.space_names();
        assert!(names.contains(&"text"));
        assert!(names.contains(&"recency"));
        assert!(names.contains(&"confidence"));
    }

    #[test]
    fn unknown_verb_falls_back_to_text_weight() {
        let idx = ComposedIndex::default_three_space();
        let meta = meta_now();
        // With unknown verb, all weight goes to text; score = text_cosine * 1.0
        let cosine = 0.75;
        let s = idx.score("anything", &meta, cosine, Some("nonexistent_verb"));
        assert!((s - cosine).abs() < 1e-6, "expected {cosine}, got {s}");
    }

    #[test]
    fn none_verb_defaults_to_recall() {
        let idx = ComposedIndex::default_three_space();
        let meta = meta_now();
        let s_recall = idx.score("q", &meta, 0.5, Some("recall"));
        let s_none = idx.score("q", &meta, 0.5, None);
        assert!((s_recall - s_none).abs() < 1e-6);
    }
}
