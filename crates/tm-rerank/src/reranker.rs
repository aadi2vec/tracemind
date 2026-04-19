//! ColBERT reranker implementation.
//!
//! Uses ONNX Runtime to run the `mxbai-edge-colbert-v0-17m` model. Always
//! available — no feature flag. Auto-downloads the model on first use via
//! [`ColbertReranker::auto_download`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Special token IDs for mxbai-edge-colbert-v0-17m
// ---------------------------------------------------------------------------

const CLS_ID: i64 = 50281;
const SEP_ID: i64 = 50282;
const PAD_ID: i64 = 50283;
const MASK_ID: i64 = 50284;
const QUERY_MARKER_ID: i64 = 50368;
const DOC_MARKER_ID: i64 = 50369;

/// Max sequence length for tokenization (matches ONNX export default).
const MAX_SEQ_LEN: usize = 128;

/// Embedding dimension of the ColBERT output.
pub const COLBERT_DIM: usize = 48;

/// Punctuation token IDs to filter from document embeddings during MaxSim.
const SKIPLIST: &[i64] = &[
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 27, 28, 29, 30, 31,
    32, 33, 60, 61, 62, 63, 64, 65, 92, 93, 94, 95,
];

/// HuggingFace repo for the model weights + tokenizer.
const HF_REPO: &str = "mixedbread-ai/mxbai-edge-colbert-v0-17m";
/// File name of the ONNX export inside the repo.
const HF_MODEL_FILE: &str = "onnx/model.onnx";
/// File name of the tokenizer inside the repo.
const HF_TOKENIZER_FILE: &str = "tokenizer.json";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A candidate document to be reranked.
#[derive(Debug, Clone)]
pub struct RerankCandidate {
    /// Unique identifier (e.g. entity UUID string).
    pub id: String,
    /// Original retrieval score (e.g. cosine similarity from vector search).
    pub initial_score: f32,
    /// The document text to rerank against the query.
    pub text: String,
}

/// Result of reranking a candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankResult {
    pub id: String,
    pub initial_score: f32,
    pub rerank_score: f32,
    /// Combined score: `alpha * rerank_score + (1 - alpha) * initial_score`
    pub combined_score: f32,
}

// ---------------------------------------------------------------------------
// ColBERT reranker
// ---------------------------------------------------------------------------

/// ColBERT late-interaction reranker.
///
/// Produces per-token embeddings and computes MaxSim relevance scores.
/// Pre-computed document embeddings can be cached for fast reranking.
pub struct ColbertReranker {
    session: Mutex<ort::session::Session>,
    tokenizer: tokenizers::Tokenizer,

    /// Weight for combining rerank score with initial score.
    /// `combined = alpha * rerank + (1 - alpha) * initial`
    pub alpha: f32,
}

impl ColbertReranker {
    /// Load the reranker from explicit model + tokenizer file paths.
    ///
    /// # Arguments
    /// * `model_path` - Path to the ColBERT ONNX model file
    /// * `tokenizer_path` - Path to the tokenizer.json file
    /// * `alpha` - Interpolation weight (0.0 = initial only, 1.0 = rerank only)
    pub fn new(model_path: &str, tokenizer_path: &str, alpha: f32) -> Result<Self> {
        info!("[rerank] loading ColBERT model from {model_path}");

        let session = ort::session::Session::builder()
            .map_err(|e| TraceMindError::Embedding(format!("ort session builder: {e}")))?
            .with_intra_threads(2)
            .map_err(|e| TraceMindError::Embedding(format!("ort threads: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| TraceMindError::Embedding(format!("ort load model: {e}")))?;

        let tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
            .map_err(|e| TraceMindError::Embedding(format!("load tokenizer: {e}")))?;

        info!("[rerank] ColBERT model loaded (dim={COLBERT_DIM})");
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            alpha: alpha.clamp(0.0, 1.0),
        })
    }

    /// Auto-download the ColBERT model + tokenizer from HuggingFace Hub and
    /// return a ready-to-use reranker.
    ///
    /// Files are cached on disk by `hf-hub` (default: `~/.cache/huggingface/`);
    /// subsequent calls are fast and work offline.
    ///
    /// Returns `Err` if the network is unreachable AND the cache is empty —
    /// callers should log and continue without reranking in that case.
    pub fn auto_download(alpha: f32) -> Result<Self> {
        info!("[rerank] resolving ColBERT model via hf-hub ({HF_REPO})");

        let api = hf_hub::api::sync::Api::new()
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub init: {e}")))?;
        let repo = api.model(HF_REPO.to_string());

        let model_path = repo
            .get(HF_MODEL_FILE)
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub fetch model: {e}")))?;
        let tokenizer_path = repo
            .get(HF_TOKENIZER_FILE)
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub fetch tokenizer: {e}")))?;

        info!(
            "[rerank] cached model at {} / tokenizer at {}",
            model_path.display(),
            tokenizer_path.display()
        );

        Self::new(
            model_path.to_string_lossy().as_ref(),
            tokenizer_path.to_string_lossy().as_ref(),
            alpha,
        )
    }

    /// Convenience: try [`Self::auto_download`] and, on failure, emit a
    /// warning and return `None` so callers can run without reranking.
    pub fn auto_download_or_none(alpha: f32) -> Option<Self> {
        match Self::auto_download(alpha) {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(
                    "[rerank] ColBERT unavailable — continuing without reranking: {}",
                    e
                );
                None
            }
        }
    }

    /// Best-effort hint at the on-disk cache directory for the model. Used for
    /// logging / diagnostics; not authoritative (hf-hub owns the real layout).
    pub fn default_cache_dir() -> Option<PathBuf> {
        dirs_cache_dir().map(|base| {
            base.join("huggingface/hub")
                .join(format!("models--{}", HF_REPO.replace('/', "--")))
        })
    }

    /// Encode a query into per-token embeddings.
    ///
    /// Query format: `[CLS] [Q] <tokens> [SEP] [MASK]...` (padded to MAX_SEQ_LEN)
    /// All positions (including MASK) have attention_mask=1.
    pub fn encode_query(&self, text: &str) -> Result<Vec<Vec<f32>>> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| TraceMindError::Embedding(format!("tokenize query: {e}")))?;

        let token_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();

        // Build: [CLS] [Q] <tokens> [SEP] [MASK]...
        let mut input_ids = Vec::with_capacity(MAX_SEQ_LEN);
        input_ids.push(CLS_ID);
        input_ids.push(QUERY_MARKER_ID);

        let max_tokens = MAX_SEQ_LEN - 3; // reserve CLS, Q, SEP
        let take = token_ids.len().min(max_tokens);
        input_ids.extend_from_slice(&token_ids[..take]);
        input_ids.push(SEP_ID);

        // Pad with MASK tokens (enables query augmentation)
        while input_ids.len() < MAX_SEQ_LEN {
            input_ids.push(MASK_ID);
        }

        // All positions active (including MASK)
        let attention_mask = vec![1_i64; MAX_SEQ_LEN];

        self.run_inference(&input_ids, &attention_mask, None)
    }

    /// Encode a document into per-token embeddings, filtering skiplist tokens.
    ///
    /// Document format: `[CLS] [D] <tokens> [SEP] [PAD]...`
    /// attention_mask=1 for real tokens, 0 for PAD.
    pub fn encode_document(&self, text: &str) -> Result<Vec<Vec<f32>>> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| TraceMindError::Embedding(format!("tokenize doc: {e}")))?;

        let token_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();

        // Build: [CLS] [D] <tokens> [SEP] [PAD]...
        let mut input_ids = Vec::with_capacity(MAX_SEQ_LEN);
        input_ids.push(CLS_ID);
        input_ids.push(DOC_MARKER_ID);

        let max_tokens = MAX_SEQ_LEN - 3;
        let take = token_ids.len().min(max_tokens);
        input_ids.extend_from_slice(&token_ids[..take]);
        input_ids.push(SEP_ID);

        let real_len = input_ids.len();

        while input_ids.len() < MAX_SEQ_LEN {
            input_ids.push(PAD_ID);
        }

        let mut attention_mask = vec![0_i64; MAX_SEQ_LEN];
        for v in attention_mask.iter_mut().take(real_len) {
            *v = 1;
        }

        // Filter: only keep embeddings for non-skiplist, non-pad tokens
        self.run_inference(&input_ids, &attention_mask, Some(&input_ids))
    }

    /// Run ONNX inference and return per-token embeddings.
    ///
    /// If `filter_ids` is provided, embeddings for skiplist token IDs are removed.
    fn run_inference(
        &self,
        input_ids: &[i64],
        attention_mask: &[i64],
        filter_ids: Option<&[i64]>,
    ) -> Result<Vec<Vec<f32>>> {
        use ndarray::Array2;
        use ort::value::Value;

        let seq_len = input_ids.len();
        let ids_array = Array2::from_shape_vec((1, seq_len), input_ids.to_vec())
            .map_err(|e| TraceMindError::Embedding(format!("shape ids: {e}")))?;
        let mask_array = Array2::from_shape_vec((1, seq_len), attention_mask.to_vec())
            .map_err(|e| TraceMindError::Embedding(format!("shape mask: {e}")))?;

        let ids_value = Value::from_array(ids_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort ids tensor: {e}")))?;
        let mask_value = Value::from_array(mask_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort mask tensor: {e}")))?;

        let session_inputs = ort::inputs![
            "input_ids" => ids_value,
            "attention_mask" => mask_value,
        ];

        let mut session = self.session.lock().expect("reranker session mutex poisoned");
        let outputs = session
            .run(session_inputs)
            .map_err(|e| TraceMindError::Embedding(format!("ort run: {e}")))?;

        // Output shape: [1, seq_len, COLBERT_DIM]
        // try_extract_tensor returns (shape, data_slice)
        let first_output = outputs.values().next()
            .ok_or_else(|| TraceMindError::Embedding("no output from model".into()))?;
        let (shape, data) = first_output
            .try_extract_tensor::<f32>()
            .map_err(|e| TraceMindError::Embedding(format!("extract output: {e}")))?;

        let dim = shape.last().copied().unwrap_or(0) as usize;
        let mut embeddings = Vec::new();

        for i in 0..seq_len {
            // Skip PAD tokens (attention_mask == 0)
            if attention_mask[i] == 0 {
                continue;
            }

            // Skip skiplist tokens for documents
            if let Some(ids) = filter_ids {
                if SKIPLIST.contains(&ids[i]) {
                    continue;
                }
            }

            let offset = i * dim;
            let mut emb: Vec<f32> = data[offset..offset + dim.min(COLBERT_DIM)].to_vec();
            // L2-normalize each token embedding
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                emb.iter_mut().for_each(|x| *x /= norm);
            }
            embeddings.push(emb);
        }

        Ok(embeddings)
    }

    /// Rerank candidates against a query using ColBERT MaxSim scoring.
    ///
    /// Returns candidates sorted by combined score (descending).
    pub fn rerank(
        &self,
        query: &str,
        candidates: Vec<RerankCandidate>,
    ) -> Result<Vec<RerankResult>> {
        let query_tokens = self.encode_query(query)?;
        debug!(
            "[rerank] encoded query into {} token embeddings",
            query_tokens.len()
        );

        let mut results: Vec<RerankResult> = Vec::with_capacity(candidates.len());

        for c in &candidates {
            let doc_tokens = self.encode_document(&c.text)?;
            let rerank_score = maxsim(&query_tokens, &doc_tokens);

            let combined =
                self.alpha * rerank_score + (1.0 - self.alpha) * c.initial_score;

            results.push(RerankResult {
                id: c.id.clone(),
                initial_score: c.initial_score,
                rerank_score,
                combined_score: combined,
            });
        }

        results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        info!("[rerank] reranked {} candidates", results.len());
        Ok(results)
    }
}

fn dirs_cache_dir() -> Option<PathBuf> {
    // Prefer XDG_CACHE_HOME, then HOME/.cache (unix), then a platform fallback.
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(Path::new(&home).join(".cache"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// MaxSim computation (pure math, no model dependency)
// ---------------------------------------------------------------------------

/// Compute the MaxSim score between query and document token embeddings.
///
/// For each query token, find the maximum cosine similarity against all
/// document tokens, then sum across query tokens.
pub fn maxsim(query_tokens: &[Vec<f32>], doc_tokens: &[Vec<f32>]) -> f32 {
    if query_tokens.is_empty() || doc_tokens.is_empty() {
        return 0.0;
    }

    let mut total = 0.0_f32;

    for qt in query_tokens {
        let mut max_sim = f32::NEG_INFINITY;
        for dt in doc_tokens {
            let sim = cosine_sim(qt, dt);
            if sim > max_sim {
                max_sim = sim;
            }
        }
        total += max_sim;
    }

    total
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0_f32;
    let mut norm_a = 0.0_f32;
    let mut norm_b = 0.0_f32;

    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a.sqrt() * norm_b.sqrt())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maxsim_identical_tokens() {
        let qt = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        let dt = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        let score = maxsim(&qt, &dt);
        assert!((score - 2.0).abs() < 1e-5, "score={score}");
    }

    #[test]
    fn test_maxsim_orthogonal_tokens() {
        let qt = vec![vec![1.0, 0.0]];
        let dt = vec![vec![0.0, 1.0]];
        let score = maxsim(&qt, &dt);
        assert!((score - 0.0).abs() < 1e-5, "score={score}");
    }

    #[test]
    fn test_maxsim_picks_best_match() {
        let qt = vec![vec![1.0, 0.0, 0.0]];
        let dt = vec![
            vec![0.0, 1.0, 0.0],
            vec![0.9, 0.1, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        let score = maxsim(&qt, &dt);
        let expected = cosine_sim(&[1.0, 0.0, 0.0], &[0.9, 0.1, 0.0]);
        assert!(
            (score - expected).abs() < 1e-5,
            "score={score}, expected={expected}"
        );
    }

    #[test]
    fn test_maxsim_empty_returns_zero() {
        assert_eq!(maxsim(&[], &[vec![1.0]]), 0.0);
        assert_eq!(maxsim(&[vec![1.0]], &[]), 0.0);
    }

    #[test]
    fn test_cosine_sim_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_sim(&a, &a);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_sim_zero_vector() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 0.0];
        assert_eq!(cosine_sim(&a, &b), 0.0);
    }

    /// When a model file is absent at the given path, `new()` must surface an
    /// error (not panic) so callers can fall back gracefully.
    #[test]
    fn test_new_errors_on_missing_model() {
        let result = ColbertReranker::new(
            "/nonexistent/model.onnx",
            "/nonexistent/tokenizer.json",
            0.7,
        );
        assert!(result.is_err(), "expected Err for missing model file");
    }

    /// `auto_download_or_none` must never panic, even on pathological env state.
    /// We don't assert Some/None because CI may or may not have network or a
    /// populated cache. The contract is "returns Option, no panic".
    #[test]
    fn test_auto_download_or_none_does_not_panic() {
        let _ = ColbertReranker::auto_download_or_none(0.7);
    }
}
