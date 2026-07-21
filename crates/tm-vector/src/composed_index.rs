//! ComposedIndex: combines multiple retrieval Spaces with per-verb weight vectors.
//!
//! Implements the Superlinked-style ComposedIndex pattern from the H2 2026
//! charter §5 Pillar 2. Each registered Space contributes a score; the final
//! score is a weighted sum normalised by verb weight vectors.

use std::collections::HashMap;

use crate::space::{ConfidenceSpace, MemoryMeta, RecencySpace, Space, TextSpace};

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
pub fn default_verb_weights() -> Vec<VerbWeights> {
    vec![
        VerbWeights::new("recall",     &[("text", 0.6), ("recency", 0.2), ("confidence", 0.2)]),
        VerbWeights::new("plan",       &[("text", 0.4), ("recency", 0.4), ("confidence", 0.2)]),
        VerbWeights::new("contradict", &[("text", 0.5), ("confidence", 0.3), ("recency", 0.2)]),
        VerbWeights::new("reflect",    &[("text", 0.3), ("recency", 0.5), ("confidence", 0.2)]),
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
pub struct ComposedIndex {
    spaces: Vec<Box<dyn Space>>,
    verb_weights: Vec<VerbWeights>,
}

impl ComposedIndex {
    pub fn new(spaces: Vec<Box<dyn Space>>, verb_weights: Vec<VerbWeights>) -> Self {
        Self {
            spaces,
            verb_weights,
        }
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

    /// Score a memory entry given a pre-computed text cosine similarity.
    ///
    /// `text_cosine` — cosine similarity from `VectorStore.search()` (0..1).
    /// `verb` — active MCP verb (recall / plan / contradict / reflect).
    ///          Defaults to "recall" if unknown.
    pub fn score(
        &self,
        query_text: &str,
        meta: &MemoryMeta,
        text_cosine: f32,
        verb: Option<&str>,
    ) -> f32 {
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
