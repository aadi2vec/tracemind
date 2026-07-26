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

/// Product Plan X7 — Image embedding space.
///
/// Scores how similar a query's image (or a caller-supplied image
/// signature) is to a stored memory's image. Real CLIP embeddings land
/// behind `feature = "clip-image-embed"`; the default build uses a
/// 64-bit perceptual hash (aHash-style) — coarse but honest, and
/// enough to make "find the whiteboard photo" work while a heavier
/// model is being staged.
///
/// The Space itself is metadata-only — the actual bit-signature lives
/// on the attachment row. Callers pass the query's signature in
/// through [`Self::with_query_signature`] and the score is `1 - hamming
/// / 64` (already normalised into [0, 1]).
pub struct ImageEmbedSpace {
    /// The image aHash of the current query, if any. When `None` the
    /// space is a neutral 0.5 — retrieval falls through to the other
    /// spaces.
    pub query_signature: Option<u64>,
}

impl Default for ImageEmbedSpace {
    fn default() -> Self {
        Self {
            query_signature: None,
        }
    }
}

impl ImageEmbedSpace {
    pub fn with_query_signature(sig: u64) -> Self {
        Self {
            query_signature: Some(sig),
        }
    }
}

impl Space for ImageEmbedSpace {
    fn name(&self) -> &str {
        "image_embed"
    }

    fn score(&self, _query: &str, _id: Uuid, meta: &MemoryMeta) -> f32 {
        let Some(q) = self.query_signature else {
            return 0.5;
        };
        // Convention: for image-modality memories, `source` is prefixed
        // by "img_sig:<64-bit hex>" — this Space is a metadata-driven
        // pass-through. Non-image memories return 0.
        let Some(src) = meta.source.as_ref() else {
            return 0.0;
        };
        let Some(hex) = src.strip_prefix("img_sig:") else {
            return 0.0;
        };
        let Ok(m) = u64::from_str_radix(hex.trim(), 16) else {
            return 0.0;
        };
        let hamming = (q ^ m).count_ones() as f32;
        1.0 - (hamming / 64.0)
    }

    fn dim(&self) -> usize {
        64
    }
}

/// Compute a very simple 64-bit perceptual-hash signature over raw
/// bytes — deterministic, fast, and dependency-free. Real image aHash
/// downsamples to 8×8 grayscale first; we approximate by folding the
/// byte stream so equivalent images with identical byte content share
/// signatures, and small edits produce small Hamming distances. Good
/// enough for a placeholder; the CLIP feature swaps in real vectors.
pub fn ahash64(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    // Split into 64 buckets, average each, threshold above the mean.
    let mut buckets = [0u64; 64];
    let mut counts = [0u64; 64];
    let n = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        let bucket = (i * 64) / n.max(1);
        let b_i = bucket.min(63);
        buckets[b_i] = buckets[b_i].wrapping_add(*b as u64);
        counts[b_i] = counts[b_i].saturating_add(1);
    }
    let mut avgs = [0f64; 64];
    let mut mean = 0f64;
    for i in 0..64 {
        avgs[i] = if counts[i] > 0 {
            buckets[i] as f64 / counts[i] as f64
        } else {
            0.0
        };
        mean += avgs[i];
    }
    mean /= 64.0;
    let mut sig = 0u64;
    for (i, a) in avgs.iter().enumerate() {
        if *a > mean {
            sig |= 1u64 << i;
        }
    }
    sig
}

#[cfg(test)]
mod image_embed_tests {
    use super::*;

    fn meta_with_source(src: Option<String>) -> MemoryMeta {
        MemoryMeta {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            last_accessed_at: None,
            confidence: 1.0,
            entity_kind: None,
            source: src,
            session_id: None,
            host_id: None,
        }
    }

    #[test]
    fn ahash64_deterministic() {
        assert_eq!(ahash64(b"hello world"), ahash64(b"hello world"));
        assert_eq!(ahash64(&[]), 0);
    }

    #[test]
    fn similar_bytes_have_small_hamming_distance() {
        let a = ahash64(b"the quick brown fox jumps over the lazy dog");
        let b = ahash64(b"the quick brown fox jumps over the lazy dog!");
        let hd = (a ^ b).count_ones();
        assert!(hd < 28, "similar strings should stay below random (32/64), got hd={hd}");
    }

    #[test]
    fn image_embed_missing_query_signature_returns_neutral() {
        let space = ImageEmbedSpace::default();
        let m = meta_with_source(Some("img_sig:0000000000000000".into()));
        assert!((space.score("q", Uuid::new_v4(), &m) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn image_embed_non_image_source_returns_zero() {
        let space = ImageEmbedSpace::with_query_signature(0xdeadbeef);
        let m = meta_with_source(Some("plaintext".into()));
        assert_eq!(space.score("q", Uuid::new_v4(), &m), 0.0);
    }

    #[test]
    fn image_embed_identical_signature_is_perfect() {
        let sig: u64 = 0xdeadbeefdeadbeef;
        let space = ImageEmbedSpace::with_query_signature(sig);
        let src = format!("img_sig:{:016x}", sig);
        let m = meta_with_source(Some(src));
        assert!((space.score("q", Uuid::new_v4(), &m) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn image_embed_opposite_signature_is_zero() {
        let sig: u64 = 0x0000000000000000;
        let space = ImageEmbedSpace::with_query_signature(sig);
        let m = meta_with_source(Some("img_sig:ffffffffffffffff".into()));
        assert_eq!(space.score("q", Uuid::new_v4(), &m), 0.0);
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
