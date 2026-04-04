//! Text embedder for TraceMind (Phase 2).
//!
//! Two backends:
//! * **Model** – `all-MiniLM-L6-v2` via fastembed (~80 MB, cached).  [`Embedder::new`]
//! * **Hash** – deterministic seahash-seeded; for unit tests.  [`Embedder::new_hash`]

use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tm_types::{Result, TraceMindError};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

enum EmbedBackend {
    Model(Mutex<TextEmbedding>),
    Hash,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// A text embedder producing 384-dimensional unit vectors.
pub struct Embedder {
    backend: EmbedBackend,
    dim: usize,
}

impl Embedder {
    /// Load the real `all-MiniLM-L6-v2` ONNX model via fastembed.
    ///
    /// Downloads ~80 MB on first call; subsequent calls use the local cache.
    pub fn new() -> Result<Self> {
        info!("[embed] loading all-MiniLM-L6-v2 model via fastembed...");
        let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::AllMiniLML6V2))
            .map_err(|e| {
                warn!("[embed] failed to load model: {e}");
                TraceMindError::Embedding(e.to_string())
            })?;
        info!("[embed] model loaded successfully (384-dim)");
        Ok(Self {
            backend: EmbedBackend::Model(Mutex::new(model)),
            dim: 384,
        })
    }

    /// Deterministic hash-based embedder — no model download. For tests.
    pub fn new_hash() -> Self {
        debug!("[embed] using hash-based embedder (test mode)");
        Self {
            backend: EmbedBackend::Hash,
            dim: 384,
        }
    }

    /// Embed `text` into a 384-d unit vector.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        match &self.backend {
            EmbedBackend::Model(model) => {
                debug!("[embed] encoding {:?} with ONNX model", &text[..text.len().min(60)]);
                let mut guard = model.lock().expect("embedder mutex poisoned");
                guard
                    .embed(vec![text.to_string()], None)
                    .map(|mut v| {
                        debug!("[embed] got {}-dim vector from model", v[0].len());
                        v.remove(0)
                    })
                    .unwrap_or_else(|e| {
                        warn!("[embed] model inference failed ({e}), falling back to hash");
                        hash_embed(text, self.dim)
                    })
            }
            EmbedBackend::Hash => hash_embed(text, self.dim),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// Hash-based fallback
// ---------------------------------------------------------------------------

pub(crate) fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let h = seahash::hash(text.as_bytes()) as u64;

    let mut v: Vec<f32> = (0..dim)
        .map(|i| {
            let mixed = h
                .wrapping_mul(i as u64 + 1)
                .wrapping_add((i as u64).wrapping_mul(6_364_136_223_846_793_005));
            (mixed as f32) / (u64::MAX as f32) * 2.0 - 1.0
        })
        .collect();

    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_hello_has_correct_length_and_unit_norm() {
        let embedder = Embedder::new_hash();
        let v = embedder.embed("hello");
        assert_eq!(v.len(), 384);
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
    }

    #[test]
    fn embed_is_deterministic() {
        let embedder = Embedder::new_hash();
        assert_eq!(embedder.embed("hello"), embedder.embed("hello"));
    }

    #[test]
    fn embed_different_texts_differ() {
        let embedder = Embedder::new_hash();
        assert_ne!(embedder.embed("hello"), embedder.embed("world"));
    }
}
