//! Space trait and built-in Space implementations for ComposedIndex.
//!
//! Each Space scores how relevant a stored memory is to a query in one
//! retrieval dimension (semantic similarity, recency, confidence, etc.).
//! ComposedIndex combines multiple Spaces with per-verb weight vectors.

use chrono::{DateTime, Utc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// A single retrieval space in a ComposedIndex.
pub trait Space: Send + Sync {
    fn name(&self) -> &str;
    /// Score how relevant `memory_id` is to `query_text` in this space.
    /// Returns a score in [0.0, 1.0]. Higher = more relevant.
    fn score(&self, query_text: &str, memory_id: Uuid, meta: &MemoryMeta) -> f32;
    /// Dimension of this space (for documentation / diagnostics).
    fn dim(&self) -> usize {
        0
    }
}

// ---------------------------------------------------------------------------
// MemoryMeta
// ---------------------------------------------------------------------------

/// Lightweight metadata about a memory entry for space scoring.
#[derive(Debug, Clone)]
pub struct MemoryMeta {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub entity_kind: Option<String>,
    pub source: Option<String>,
    pub session_id: Option<Uuid>,
    pub host_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Built-in Space implementations
// ---------------------------------------------------------------------------

/// Text space: dense semantic similarity (stub — real embedding comparison
/// happens in VectorStore; this space wraps the cosine score already computed).
pub struct TextSpace;

impl Space for TextSpace {
    fn name(&self) -> &str {
        "text"
    }

    fn score(&self, _query_text: &str, _id: Uuid, _meta: &MemoryMeta) -> f32 {
        // Note: TextSpace.score() is intentionally a pass-through — the real
        // cosine score comes from VectorStore.search(). ComposedIndex uses
        // TextSpace to scale the pre-computed cosine result.
        0.0
    }
}

/// Lexical space: BM25 sparse relevance.
///
/// Like [`TextSpace`], this is a declaration rather than a scorer — BM25
/// needs the document body and corpus statistics, neither of which is
/// present in [`MemoryMeta`]. Callers compute the score with
/// [`crate::lexical::Bm25Index`] and fuse it via
/// [`crate::composed_index::ComposedIndex::score_precomputed`]. Registering
/// it here keeps `space_names()` an honest description of the index.
pub struct LexicalSpace;

impl Space for LexicalSpace {
    fn name(&self) -> &str {
        "lexical"
    }

    fn score(&self, _query_text: &str, _id: Uuid, _meta: &MemoryMeta) -> f32 {
        0.0
    }
}

/// Recency space: time-decay scoring.
pub struct RecencySpace {
    pub half_life_days: f32,
}

impl Default for RecencySpace {
    fn default() -> Self {
        Self {
            half_life_days: 7.0,
        }
    }
}

impl Space for RecencySpace {
    fn name(&self) -> &str {
        "recency"
    }

    fn score(&self, _query_text: &str, _id: Uuid, meta: &MemoryMeta) -> f32 {
        let t = meta.created_at;
        let now = Utc::now();
        let age_days = (now - t).num_seconds() as f32 / 86400.0;
        // Exponential decay: score = 2^(-age / half_life)
        2f32.powf(-age_days / self.half_life_days)
    }
}

/// Confidence space: governance-score pass-through.
pub struct ConfidenceSpace;

impl Space for ConfidenceSpace {
    fn name(&self) -> &str {
        "confidence"
    }

    fn score(&self, _query: &str, _id: Uuid, meta: &MemoryMeta) -> f32 {
        meta.confidence.clamp(0.0, 1.0)
    }
}

/// Entity-type space: boosts matches that share entity kind with the query hint.
pub struct EntityTypeSpace {
    pub query_kind_hint: Option<String>,
}

impl Space for EntityTypeSpace {
    fn name(&self) -> &str {
        "entity_type"
    }

    fn score(&self, _query: &str, _id: Uuid, meta: &MemoryMeta) -> f32 {
        if let (Some(hint), Some(kind)) = (&self.query_kind_hint, &meta.entity_kind) {
            if hint.eq_ignore_ascii_case(kind) {
                1.0
            } else {
                0.0
            }
        } else {
            0.5
        }
    }
}

/// Host/session scoping space: 1.0 if memory matches active session/host, else 0.0.
pub struct HostSessionSpace {
    pub active_session_id: Option<Uuid>,
    pub active_host_id: Option<String>,
}

impl Space for HostSessionSpace {
    fn name(&self) -> &str {
        "host_session"
    }

    fn score(&self, _query: &str, _id: Uuid, meta: &MemoryMeta) -> f32 {
        let session_match = match (&self.active_session_id, &meta.session_id) {
            (Some(a), Some(b)) => {
                if a == b {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.5,
        };
        let host_match = match (&self.active_host_id, &meta.host_id) {
            (Some(a), Some(b)) => {
                if a.eq_ignore_ascii_case(b) {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.5,
        };
        (session_match + host_match) / 2.0
    }
}
