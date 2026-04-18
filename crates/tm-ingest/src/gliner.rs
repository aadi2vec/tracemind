//! GLiNER / GLiREL entity + relation extraction (feature-gated).
//!
//! GLiNER is a 50M-param ONNX model that does zero-shot NER at ~85% F1 — a
//! significant bump over the stdlib heuristic (~55% F1) at the cost of a
//! ~200 MB model download and one ONNX Runtime inference per slow-path cluster.
//!
//! This module is only compiled when the `gliner` cargo feature is enabled:
//!
//! ```bash
//! cargo build -p tm-ingest --features gliner
//! ```
//!
//! Activation requires the `TM_GLINER_MODEL_PATH` environment variable to
//! point at a `model.onnx` file plus a `tokenizer.json` sibling. If the model
//! fails to load (missing file, corrupt weights, incompatible runtime), the
//! constructor returns `None` and callers fall back to [`crate::extractor::HeuristicExtractor`].
//!
//! # Status
//!
//! The ONNX inference path is still wired behind a `todo!()` until we pick the
//! final label set and decoding strategy (BIO vs span-based). The scaffolding
//! is in place so that:
//!   1. The cargo feature plumbing, ort/tokenizers/ndarray deps, and trait
//!      impl compile cleanly with `--features gliner`.
//!   2. The `IngestPipeline::with_extractor()` builder already accepts
//!      `Box<dyn EntityExtractor>`, so swapping in the finished GlinerExtractor
//!      is a one-line change on the caller side.
//!   3. Graceful fallback is exercised today: if the model is missing the
//!      pipeline degrades to heuristics with no panics.

use std::path::PathBuf;

use tm_types::{Entity, Triple};

use crate::extractor::{EntityExtractor, HeuristicExtractor};

/// GLiNER-backed extractor. Loads an ONNX model + tokenizer from
/// `TM_GLINER_MODEL_PATH`.
///
/// Currently delegates to [`HeuristicExtractor`] while the ONNX decoding is
/// being finalised; the plumbing (feature flag, deps, trait impl, graceful
/// fallback) is all in place so the inference path can be dropped in without
/// touching the call sites.
pub struct GlinerExtractor {
    #[allow(dead_code)]
    model_path: PathBuf,
    fallback: HeuristicExtractor,
    // TODO(TM-5.1-001c): hold ort::Session + tokenizers::Tokenizer here once
    // the decoding strategy is finalised. Keeping them off the struct for now
    // so `cargo check --features gliner` stays green without a real model.
}

impl GlinerExtractor {
    /// Try to construct a `GlinerExtractor` from the `TM_GLINER_MODEL_PATH`
    /// environment variable. Returns `None` if the variable is unset or the
    /// file doesn't exist — the pipeline should then fall back to
    /// `HeuristicExtractor`.
    pub fn try_from_env() -> Option<Self> {
        let path = std::env::var("TM_GLINER_MODEL_PATH").ok()?;
        let model_path = PathBuf::from(path);
        if !model_path.exists() {
            tracing::warn!(
                "[gliner] TM_GLINER_MODEL_PATH points at a missing file: {}",
                model_path.display()
            );
            return None;
        }

        tracing::info!(
            "[gliner] model found at {} — scaffolding loaded (inference path: TODO)",
            model_path.display()
        );

        Some(Self {
            model_path,
            fallback: HeuristicExtractor,
        })
    }
}

impl EntityExtractor for GlinerExtractor {
    fn extract_entities(&self, text: &str) -> Vec<Entity> {
        // TODO(TM-5.1-001c): run ONNX session with a fixed label set
        // ("person", "organization", "technology", "file", "url", "concept").
        // Decode span predictions into `Entity`s, merge multi-token spans.
        // For now: fall back to the heuristic so the trait is exercised end-to-end.
        self.fallback.extract_entities(text)
    }

    fn extract_triples(&self, text: &str, entities: &[Entity]) -> Vec<Triple> {
        // TODO(TM-5.1-001c): run GLiREL / GLiNER-multitask on the entity pairs
        // for joint relation classification. Until then, reuse the pattern-matched
        // triples so we don't regress the graph quality.
        self.fallback.extract_triples(text, entities)
    }

    fn name(&self) -> &'static str {
        "gliner"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_env_returns_none_without_var() {
        // Save & clear so the test is hermetic.
        let prev = std::env::var("TM_GLINER_MODEL_PATH").ok();
        // SAFETY: test is single-threaded; we restore at the end.
        unsafe {
            std::env::remove_var("TM_GLINER_MODEL_PATH");
        }

        assert!(GlinerExtractor::try_from_env().is_none());

        if let Some(v) = prev {
            unsafe {
                std::env::set_var("TM_GLINER_MODEL_PATH", v);
            }
        }
    }

    #[test]
    fn try_from_env_returns_none_for_missing_file() {
        let prev = std::env::var("TM_GLINER_MODEL_PATH").ok();
        unsafe {
            std::env::set_var("TM_GLINER_MODEL_PATH", "/nonexistent/path/model.onnx");
        }

        assert!(GlinerExtractor::try_from_env().is_none());

        unsafe {
            std::env::remove_var("TM_GLINER_MODEL_PATH");
        }
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("TM_GLINER_MODEL_PATH", v);
            }
        }
    }
}
