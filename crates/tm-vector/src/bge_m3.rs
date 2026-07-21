//! BGE-M3 dense embedder via direct ONNX loading.
//!
//! BGE-M3 is not in fastembed's official model list as of Jul 2026.
//! We load the ONNX model directly using the `ort` crate.
//!
//! Output: 768-dimensional L2-normalized dense vectors.
//!
//! Auto-download: if the model is not present at
//! `~/.tracemind/models/bge-m3/onnx/model.onnx`, we download it from
//! HuggingFace Hub using a simple HTTP GET.
//!
//! Tokenizer: uses the tokenizers crate with the BGE-M3 tokenizer JSON
//! from HuggingFace.
//!
//! # Status (Q3.9)
//!
//! This module is **scaffolded**. The struct, constants, and download helper
//! are all in place; `BgeM3DenseModel::load()` returns a descriptive error
//! until the `ort` and `tokenizers` dependencies are wired up in Cargo.toml.
//! See the commented-out dep block in `crates/tm-vector/Cargo.toml` and the
//! Q4 task that will fully enable this path.

use std::path::PathBuf;
use tm_types::{Result, TraceMindError};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Output dimension of BGE-M3 dense vectors.
pub const BGE_M3_DIM: usize = 768;

const HF_MODEL_URL: &str =
    "https://huggingface.co/BAAI/bge-m3/resolve/main/onnx/model.onnx";
const HF_TOKENIZER_URL: &str =
    "https://huggingface.co/BAAI/bge-m3/resolve/main/tokenizer.json";

// ---------------------------------------------------------------------------
// Model struct
// ---------------------------------------------------------------------------

/// BGE-M3 dense-only embedder loaded via direct ONNX (not fastembed).
///
/// Produces 768-dimensional L2-normalized dense vectors.
///
/// Use [`BgeM3DenseModel::load`] to construct. The struct owns the ONNX
/// session and tokenizer; both are Send so it can be wrapped in `Mutex<_>`.
#[derive(Debug)]
pub struct BgeM3DenseModel {
    /// Always 768 — kept as a field so callers can assert it.
    _dim: usize,
    // Future fields (Q4 wire-up):
    //   session: ort::Session,
    //   tokenizer: tokenizers::Tokenizer,
}

impl BgeM3DenseModel {
    /// Load (or auto-download) the BGE-M3 ONNX model.
    ///
    /// `model_dir` should be `~/.tracemind/models/bge-m3/`. The ONNX file is
    /// expected at `<model_dir>/onnx/model.onnx` and the tokenizer at
    /// `<model_dir>/tokenizer.json`.
    ///
    /// # Errors
    ///
    /// Returns [`TraceMindError::Embedding`] with a human-readable message
    /// explaining what is missing and how to obtain it. This will change to a
    /// real load once the `ort` + `tokenizers` deps are added (Q4).
    pub fn load(model_dir: &PathBuf) -> Result<Self> {
        let model_path = model_dir.join("onnx/model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        // Attempt downloads (no-ops if files already exist).
        download_if_missing(HF_MODEL_URL, &model_path)?;
        download_if_missing(HF_TOKENIZER_URL, &tokenizer_path)?;

        // TODO(Q4): replace the error below with real ort::Session + tokenizer
        // construction once `ort = "2"` and `tokenizers = "0.19"` are added to
        // Cargo.toml.
        //
        // let env = Arc::new(ort::Environment::builder().build()?);
        // let session = ort::SessionBuilder::new(&env)?
        //     .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
        //     .with_intra_threads(4)?
        //     .with_model_from_file(&model_path)?;
        // let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
        //     .map_err(|e| TraceMindError::Embedding(e.to_string()))?;
        // return Ok(Self { _dim: BGE_M3_DIM, session, tokenizer });

        Err(TraceMindError::Embedding(format!(
            "BGE-M3 ONNX backend is scaffolded (Q3.9) — \
             the `ort` and `tokenizers` crate dependencies must be added to \
             crates/tm-vector/Cargo.toml before this path is live. \
             Model files are expected at: {:?}",
            model_dir
        )))
    }

    /// Embed `texts` into 768-dimensional L2-normalized dense vectors.
    ///
    /// # Errors
    ///
    /// Returns an error if the ONNX session fails or tokenization fails.
    /// In Q3.9 this always returns an error because `load()` never succeeds.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let _ = texts; // suppress unused-variable lint
        Err(TraceMindError::Embedding(
            "BgeM3DenseModel::embed called on uninitialized model (Q3.9 scaffold)".to_string(),
        ))
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
// Download helper
// ---------------------------------------------------------------------------

/// Download `url` to `dest` if `dest` does not already exist.
///
/// Creates all parent directories. Returns an informative error if the
/// file is missing and we cannot fetch it (no HTTP client compiled in yet).
fn download_if_missing(url: &str, dest: &PathBuf) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }

    // Ensure the parent directory exists so a future download has somewhere
    // to write.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TraceMindError::Storage(format!(
                "failed to create model directory {:?}: {e}", parent
            )))?;
    }

    // TODO(Q4): swap the error below for a real HTTP fetch once an HTTP
    // client (ureq or reqwest) is added to Cargo.toml alongside `ort`.
    //
    // let bytes = ureq::get(url).call()
    //     .map_err(|e| TraceMindError::Embedding(e.to_string()))?
    //     .into_reader();
    // let mut file = std::fs::File::create(dest)
    //     .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    // std::io::copy(&mut bytes, &mut file)
    //     .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    Err(TraceMindError::Embedding(format!(
        "BGE-M3 model file not found at {:?}. \
         Auto-download requires the `ureq`/`reqwest` dep (added in Q4). \
         Manual download URL: {}",
        dest, url
    )))
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

    #[test]
    fn bge_m3_load_returns_descriptive_error() {
        let tmp = std::env::temp_dir().join("tm-test-bge-m3-scaffold");
        let result = BgeM3DenseModel::load(&tmp);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        // The error should mention the scaffold state or the missing model file
        // so operators know what to do next.
        assert!(
            msg.contains("scaffolded") || msg.contains("ort") || msg.contains("Q3.9")
                || msg.contains("not found") || msg.contains("BGE-M3"),
            "unexpected error message: {msg}"
        );
    }
}
