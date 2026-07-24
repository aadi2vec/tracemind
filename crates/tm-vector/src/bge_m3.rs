//! BGE-M3 dense embedder via direct ONNX loading.
//!
//! BGE-M3 is not in fastembed's official model list as of Jul 2026.
//! We load the ONNX model directly using the `ort` crate (v2.0.0-rc.10),
//! which is already a transitive dependency through fastembed.
//!
//! Output: 768-dimensional L2-normalized dense vectors.
//!
//! Auto-download: if the model is not present at
//! `~/.tracemind/models/bge-m3/onnx/model.onnx`, we download it from
//! HuggingFace Hub using `hf-hub` (also already transitive).
//!
//! Tokenizer: uses the `tokenizers` crate with the BGE-M3 tokenizer JSON
//! from HuggingFace.

use std::path::PathBuf;

use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;
use tm_types::{Result, TraceMindError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Output dimension of BGE-M3 dense vectors.
pub const BGE_M3_DIM: usize = 768;

const MODEL_REPO: &str = "BAAI/bge-m3";
/// ONNX sub-path inside the HF repo.
const MODEL_HF_PATH: &str = "onnx/model.onnx";
/// Tokenizer sub-path inside the HF repo.
const TOKENIZER_HF_PATH: &str = "tokenizer.json";

/// Maximum tokens to pass to the model (BGE-M3 supports up to 8192, but
/// for TraceMind's short-to-medium passages 512 is sufficient and much faster).
const MAX_SEQ_LEN: usize = 512;

// ---------------------------------------------------------------------------
// Model struct
// ---------------------------------------------------------------------------

/// BGE-M3 dense-only embedder loaded via direct ONNX (not fastembed).
///
/// Produces 768-dimensional L2-normalized dense vectors.
///
/// Use [`BgeM3DenseModel::load`] to construct. The struct owns the ONNX
/// session and tokenizer; both are `Send` so it can be wrapped in `Mutex<_>`.
pub struct BgeM3DenseModel {
    session: Session,
    tokenizer: tokenizers::Tokenizer,
}

// SAFETY: ort::Session is Send+Sync, tokenizers::Tokenizer is Send.
// We expose this via Mutex<BgeM3DenseModel> so concurrent access is safe.
unsafe impl Send for BgeM3DenseModel {}

impl std::fmt::Debug for BgeM3DenseModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BgeM3DenseModel")
            .field("dim", &BGE_M3_DIM)
            .finish()
    }
}

impl BgeM3DenseModel {
    /// Load (or auto-download) the BGE-M3 ONNX model.
    ///
    /// `model_dir` should be `~/.tracemind/models/bge-m3/`. The ONNX file is
    /// expected at `<model_dir>/onnx/model.onnx` and the tokenizer at
    /// `<model_dir>/tokenizer.json`.
    ///
    /// On first use, both files are downloaded from HuggingFace Hub via
    /// `hf-hub` (cached in `~/.cache/huggingface/`).
    ///
    /// # Errors
    ///
    /// Returns [`TraceMindError::Embedding`] if the model cannot be loaded or
    /// downloaded, with a human-readable message.
    pub fn load(model_dir: &PathBuf) -> Result<Self> {
        let model_path = model_dir.join("onnx/model.onnx");
        let tok_path = model_dir.join("tokenizer.json");

        // Ensure model files exist, downloading if necessary.
        download_if_missing(&model_path, MODEL_REPO, MODEL_HF_PATH)?;
        download_if_missing(&tok_path, MODEL_REPO, TOKENIZER_HF_PATH)?;

        // Load tokenizer.
        let tokenizer = tokenizers::Tokenizer::from_file(&tok_path)
            .map_err(|e| TraceMindError::Embedding(format!("tokenizer load: {e}")))?;

        // Build ONNX session.
        let session = Session::builder()
            .map_err(|e| TraceMindError::Embedding(format!("ort builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| TraceMindError::Embedding(format!("ort opt level: {e}")))?
            .commit_from_file(&model_path)
            .map_err(|e| TraceMindError::Embedding(format!("ort load model: {e}")))?;

        Ok(Self { session, tokenizer })
    }

    /// Embed `texts` into 768-dimensional L2-normalized dense vectors.
    ///
    /// # Errors
    ///
    /// Returns an error if the ONNX session fails or tokenization fails.
    pub fn embed(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|text| Self::embed_one(&mut self.session, &self.tokenizer, text)).collect()
    }

    fn embed_one(session: &mut Session, tokenizer: &tokenizers::Tokenizer, text: &str) -> Result<Vec<f32>> {
        // Truncate to avoid hitting OOM on pathologically long inputs.
        // We truncate on char boundary at MAX_SEQ_LEN * 4 bytes which
        // comfortably exceeds the token budget for typical text.
        let truncated = truncate_chars(text, MAX_SEQ_LEN * 4);

        let encoding = tokenizer
            .encode(truncated, /* add_special_tokens */ true)
            .map_err(|e| TraceMindError::Embedding(format!("tokenize: {e}")))?;

        // Clamp to model's max sequence length.
        let ids_raw = encoding.get_ids();
        let mask_raw = encoding.get_attention_mask();
        let type_raw = encoding.get_type_ids();

        let seq_len = ids_raw.len().min(MAX_SEQ_LEN);

        let ids: Vec<i64> = ids_raw[..seq_len].iter().map(|&x| x as i64).collect();
        let mask: Vec<i64> = mask_raw[..seq_len].iter().map(|&x| x as i64).collect();
        let type_ids: Vec<i64> = type_raw[..seq_len].iter().map(|&x| x as i64).collect();

        let shape = vec![1i64, seq_len as i64];

        // Build typed i64 tensors from (shape, data) tuples.
        let ids_tensor = Tensor::<i64>::from_array((shape.clone(), ids))
            .map_err(|e| TraceMindError::Embedding(format!("ids tensor: {e}")))?;
        let mask_tensor = Tensor::<i64>::from_array((shape.clone(), mask))
            .map_err(|e| TraceMindError::Embedding(format!("mask tensor: {e}")))?;
        let type_tensor = Tensor::<i64>::from_array((shape, type_ids))
            .map_err(|e| TraceMindError::Embedding(format!("type_ids tensor: {e}")))?;

        // Run inference using named inputs (BGE-M3 ONNX standard names).
        let outputs = session
            .run(ort::inputs![
                "input_ids"      => ids_tensor,
                "attention_mask" => mask_tensor,
                "token_type_ids" => type_tensor
            ])
            .map_err(|e| TraceMindError::Embedding(format!("inference: {e}")))?;

        // BGE-M3's exported ONNX typically has two outputs:
        //   0: last_hidden_state  [1, seq_len, 768]
        //   1: sentence_embedding  [1, 768]   (mean-pooled, the dense embedding)
        //
        // We prefer output index 1 (sentence_embedding) if it's 2-D; fall back
        // to mean-pooling output 0 ourselves if only one output is present.
        let embedding = if outputs.len() >= 2 {
            extract_2d_embedding(&outputs[1])?
        } else {
            extract_mean_pool_embedding(&outputs[0])?
        };

        Ok(l2_normalize(embedding))
    }

    /// The output dimension of this model: always 768.
    pub fn dim(&self) -> usize {
        BGE_M3_DIM
    }

    /// Static dimension accessor (used by `Embedder::dim()` without a live model).
    pub fn dim_static() -> usize {
        BGE_M3_DIM
    }
}

// ---------------------------------------------------------------------------
// Tensor extraction helpers
// ---------------------------------------------------------------------------

/// Extract a flat f32 vec from a 2-D output tensor with shape `[1, hidden]`.
fn extract_2d_embedding(value: &ort::value::DynValue) -> Result<Vec<f32>> {
    let array = value
        .try_extract_array::<f32>()
        .map_err(|e| TraceMindError::Embedding(format!("extract sentence_embedding: {e}")))?;
    let shape = array.shape().to_vec();
    if shape.len() == 2 && shape[0] == 1 {
        Ok(array.as_slice().unwrap_or(&[]).to_vec())
    } else if shape.len() == 3 && shape[0] == 1 {
        // Unexpected 3-D here; mean-pool it.
        mean_pool_3d(array.as_slice().unwrap_or(&[]), shape[1], shape[2])
    } else {
        Err(TraceMindError::Embedding(format!(
            "unexpected sentence_embedding shape: {shape:?}"
        )))
    }
}

/// Extract a flat f32 vec from a 3-D `last_hidden_state` tensor `[1, seq, hidden]`
/// by mean-pooling across the sequence dimension.
fn extract_mean_pool_embedding(value: &ort::value::DynValue) -> Result<Vec<f32>> {
    let array = value
        .try_extract_array::<f32>()
        .map_err(|e| TraceMindError::Embedding(format!("extract last_hidden_state: {e}")))?;
    let shape = array.shape().to_vec();
    match shape.as_slice() {
        [1, seq_len, hidden_size] => {
            mean_pool_3d(array.as_slice().unwrap_or(&[]), *seq_len, *hidden_size)
        }
        [1, hidden_size] => Ok(array.as_slice().unwrap_or(&[])[..*hidden_size].to_vec()),
        _ => Err(TraceMindError::Embedding(format!(
            "unexpected last_hidden_state shape: {shape:?}"
        ))),
    }
}

/// Mean-pool a flat `[seq_len × hidden_size]` slice.
fn mean_pool_3d(data: &[f32], seq_len: usize, hidden_size: usize) -> Result<Vec<f32>> {
    if data.len() < seq_len * hidden_size {
        return Err(TraceMindError::Embedding(format!(
            "mean_pool_3d: data length {} < seq_len * hidden_size ({} * {})",
            data.len(),
            seq_len,
            hidden_size
        )));
    }
    let mut pooled = vec![0.0f32; hidden_size];
    for i in 0..seq_len {
        let row = &data[i * hidden_size..(i + 1) * hidden_size];
        for (p, &v) in pooled.iter_mut().zip(row.iter()) {
            *p += v;
        }
    }
    let n = seq_len as f32;
    pooled.iter_mut().for_each(|v| *v /= n);
    Ok(pooled)
}

// ---------------------------------------------------------------------------
// L2 normalization
// ---------------------------------------------------------------------------

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-8 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max_chars` Unicode scalar values.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

// ---------------------------------------------------------------------------
// Auto-download via hf-hub
// ---------------------------------------------------------------------------

/// Download `hf_filename` from `hf_repo` to `dest` if `dest` does not exist.
///
/// Uses `hf-hub`'s sync API which caches downloads in `~/.cache/huggingface/`.
/// The cached file is then hard-linked / copied to `dest`.
fn download_if_missing(dest: &PathBuf, hf_repo: &str, hf_filename: &str) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }

    // Ensure parent directory exists.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            TraceMindError::Storage(format!(
                "failed to create model directory {:?}: {e}",
                parent
            ))
        })?;
    }

    tracing::info!(
        "[bge-m3] downloading {} from {} ...",
        hf_filename,
        hf_repo
    );

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| TraceMindError::Embedding(format!("hf-hub api init: {e}")))?;
    let repo = api.model(hf_repo.to_string());
    let cached = repo.get(hf_filename).map_err(|e| {
        TraceMindError::Embedding(format!(
            "hf-hub download {hf_filename} from {hf_repo}: {e}"
        ))
    })?;

    // Copy from hf-hub's internal cache to our well-known path.
    std::fs::copy(&cached, dest).map_err(|e| {
        TraceMindError::Storage(format!(
            "copy model file {:?} -> {:?}: {e}",
            cached, dest
        ))
    })?;

    tracing::info!("[bge-m3] saved {} to {:?}", hf_filename, dest);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bge_m3_dim_constant() {
        assert_eq!(BGE_M3_DIM, 768);
    }

    #[test]
    fn bge_m3_dim_static_matches_constant() {
        assert_eq!(BgeM3DenseModel::dim_static(), BGE_M3_DIM);
    }

    /// When the model is not present *and* no network is available we should get
    /// a clear error — not a panic or a hang.
    #[test]
    fn bge_m3_load_returns_error_without_model() {
        // Use a deliberately missing path so we exercise the download path;
        // in CI there is no network so hf-hub will fail, which is expected.
        let tmp = std::env::temp_dir().join("tm-test-bge-m3-nonexistent-12345");
        // Remove the dir if it somehow exists from a prior run.
        let _ = std::fs::remove_dir_all(&tmp);

        let result = BgeM3DenseModel::load(&tmp);
        // Either Ok (model was already in hf-hub cache) or a descriptive Err.
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                msg.contains("BGE-M3")
                    || msg.contains("hf-hub")
                    || msg.contains("download")
                    || msg.contains("model")
                    || msg.contains("ort"),
                "unexpected error message: {msg}"
            );
        }
        // Clean up.
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn truncate_chars_ascii() {
        assert_eq!(truncate_chars("hello world", 5), "hello");
        assert_eq!(truncate_chars("hi", 100), "hi");
    }

    #[test]
    fn truncate_chars_unicode() {
        let s = "日本語テスト";
        assert_eq!(truncate_chars(s, 3), "日本語");
    }

    #[test]
    fn l2_normalize_unit_vector() {
        let v = vec![3.0f32, 4.0];
        let n = l2_normalize(v);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm={norm}");
    }

    #[test]
    fn l2_normalize_zero_vector() {
        let v = vec![0.0f32; 4];
        let n = l2_normalize(v.clone());
        assert_eq!(n, v); // should not panic; remains zero
    }

    #[test]
    fn mean_pool_3d_basic() {
        // Two rows: [1,2,3] and [3,4,5] → mean [2,3,4]
        let data = vec![1.0f32, 2.0, 3.0, 3.0, 4.0, 5.0];
        let pooled = mean_pool_3d(&data, 2, 3).unwrap();
        assert!((pooled[0] - 2.0).abs() < 1e-6);
        assert!((pooled[1] - 3.0).abs() < 1e-6);
        assert!((pooled[2] - 4.0).abs() < 1e-6);
    }
}
