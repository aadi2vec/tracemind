//! Text embedder for TraceMind.
//!
//! Three backends:
//! * **Model** – ONNX model via fastembed (cached on first download). [`Embedder::new`]
//! * **Hash** – deterministic seahash-seeded; for unit tests. [`Embedder::new_hash`]
//!
//! Supported models (all Apache 2.0, all 384-dim):
//!
//! | Model | Variant | Size | Quality | Speed |
//! |-------|---------|------|---------|-------|
//! | BGE-small-en-v1.5 | `Bge` | ~130 MB | Best | Fast |
//! | BGE-small-en-v1.5-Q | `BgeQ` | ~33 MB | Good | Fastest |
//! | all-MiniLM-L6-v2 | `MiniLM` | ~80 MB | Good | Fast |
//! | all-MiniLM-L6-v2-Q | `MiniLMQ` | ~22 MB | Decent | Fastest |
//! | snowflake-arctic-embed-xs | `Arctic` | ~90 MB | Good | Fast |
//! | snowflake-arctic-embed-xs-Q | `ArcticQ` | ~23 MB | Decent | Fastest |

use std::fmt;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tm_types::{Result, TraceMindError};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Model selection
// ---------------------------------------------------------------------------

/// Supported embedding models. All produce 384-dimensional vectors.
/// All are Apache 2.0 licensed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedModel {
    /// BGE-small-en-v1.5 — fastembed's default, best quality/speed ratio.
    Bge,
    /// BGE-small-en-v1.5 quantized — smallest BGE variant.
    BgeQ,
    /// all-MiniLM-L6-v2 — well-established Sentence Transformers model.
    MiniLM,
    /// all-MiniLM-L6-v2 quantized — smallest MiniLM variant.
    MiniLMQ,
    /// snowflake-arctic-embed-xs — extra-small, good for constrained envs.
    Arctic,
    /// snowflake-arctic-embed-xs quantized — tiniest model available.
    ArcticQ,
}

impl EmbedModel {
    fn to_fastembed(self) -> EmbeddingModel {
        match self {
            EmbedModel::Bge => EmbeddingModel::BGESmallENV15,
            EmbedModel::BgeQ => EmbeddingModel::BGESmallENV15Q,
            EmbedModel::MiniLM => EmbeddingModel::AllMiniLML6V2,
            EmbedModel::MiniLMQ => EmbeddingModel::AllMiniLML6V2Q,
            EmbedModel::Arctic => EmbeddingModel::SnowflakeArcticEmbedXS,
            EmbedModel::ArcticQ => EmbeddingModel::SnowflakeArcticEmbedXSQ,
        }
    }

    /// Parse from string (CLI/env var). Case-insensitive.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "bge" | "bge-small" | "bge-small-en-v1.5" => Some(EmbedModel::Bge),
            "bge-q" | "bge-quantized" => Some(EmbedModel::BgeQ),
            "minilm" | "all-minilm-l6-v2" | "minilm-l6" => Some(EmbedModel::MiniLM),
            "minilm-q" | "minilm-quantized" => Some(EmbedModel::MiniLMQ),
            "arctic" | "snowflake-arctic" | "arctic-xs" => Some(EmbedModel::Arctic),
            "arctic-q" | "arctic-quantized" => Some(EmbedModel::ArcticQ),
            _ => None,
        }
    }

    /// All available models for benchmarking.
    pub fn all() -> &'static [EmbedModel] {
        &[
            EmbedModel::Bge,
            EmbedModel::BgeQ,
            EmbedModel::MiniLM,
            EmbedModel::MiniLMQ,
            EmbedModel::Arctic,
            EmbedModel::ArcticQ,
        ]
    }
}

impl Default for EmbedModel {
    fn default() -> Self {
        EmbedModel::Bge
    }
}

impl fmt::Display for EmbedModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbedModel::Bge => write!(f, "BGE-small-en-v1.5"),
            EmbedModel::BgeQ => write!(f, "BGE-small-en-v1.5-Q"),
            EmbedModel::MiniLM => write!(f, "all-MiniLM-L6-v2"),
            EmbedModel::MiniLMQ => write!(f, "all-MiniLM-L6-v2-Q"),
            EmbedModel::Arctic => write!(f, "snowflake-arctic-embed-xs"),
            EmbedModel::ArcticQ => write!(f, "snowflake-arctic-embed-xs-Q"),
        }
    }
}

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
    model_name: String,
}

impl Embedder {
    /// Load the default ONNX model (BGE-small-en-v1.5) via fastembed.
    ///
    /// Downloads on first call; subsequent calls use the local cache.
    pub fn new() -> Result<Self> {
        Self::with_model(EmbedModel::default())
    }

    /// Load a specific ONNX model via fastembed.
    pub fn with_model(model: EmbedModel) -> Result<Self> {
        let model_name = model.to_string();
        info!("[embed] loading {model_name} model via fastembed...");
        let te = TextEmbedding::try_new(InitOptions::new(model.to_fastembed()))
            .map_err(|e| {
                warn!("[embed] failed to load {model_name}: {e}");
                TraceMindError::Embedding(e.to_string())
            })?;
        info!("[embed] {model_name} loaded successfully (384-dim)");
        Ok(Self {
            backend: EmbedBackend::Model(Mutex::new(te)),
            dim: 384,
            model_name,
        })
    }

    /// Deterministic hash-based embedder — no model download. For tests.
    pub fn new_hash() -> Self {
        debug!("[embed] using hash-based embedder (test mode)");
        Self {
            backend: EmbedBackend::Hash,
            dim: 384,
            model_name: "hash".to_string(),
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

    /// Embed a batch of texts (more efficient than repeated single calls).
    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        match &self.backend {
            EmbedBackend::Model(model) => {
                let owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
                let mut guard = model.lock().expect("embedder mutex poisoned");
                guard
                    .embed(owned, None)
                    .unwrap_or_else(|e| {
                        warn!("[embed] batch inference failed ({e}), falling back to hash");
                        texts.iter().map(|t| hash_embed(t, self.dim)).collect()
                    })
            }
            EmbedBackend::Hash => texts.iter().map(|t| hash_embed(t, self.dim)).collect(),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Name of the active model (for benchmarking / display).
    pub fn model_name(&self) -> &str {
        &self.model_name
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

    #[test]
    fn embed_batch_matches_singles() {
        let embedder = Embedder::new_hash();
        let texts = &["hello", "world", "foo"];
        let batch = embedder.embed_batch(texts);
        for (i, text) in texts.iter().enumerate() {
            assert_eq!(batch[i], embedder.embed(text));
        }
    }

    #[test]
    fn embed_model_name_hash() {
        let embedder = Embedder::new_hash();
        assert_eq!(embedder.model_name(), "hash");
    }

    #[test]
    fn embed_model_from_str_loose() {
        assert_eq!(EmbedModel::from_str_loose("bge"), Some(EmbedModel::Bge));
        assert_eq!(EmbedModel::from_str_loose("BGE"), Some(EmbedModel::Bge));
        assert_eq!(EmbedModel::from_str_loose("minilm"), Some(EmbedModel::MiniLM));
        assert_eq!(EmbedModel::from_str_loose("arctic-q"), Some(EmbedModel::ArcticQ));
        assert_eq!(EmbedModel::from_str_loose("unknown"), None);
    }

    #[test]
    fn embed_model_default_is_bge() {
        assert_eq!(EmbedModel::default(), EmbedModel::Bge);
    }
}
