//! Tier 1 — bundled local LLM (Qwen 2.5 1.5B Q4_K_M primary, Llama 3.2 1B fallback).
//!
//! **Status**: scaffolding. The actual `llama-cpp-2` integration lands in the
//! follow-up ticket. Today we expose the shape so downstream crates can depend
//! on the trait without waiting for the inference engine.
//!
//! Selection: llama.cpp via `llama-cpp-2` — Metal on Mac, CPU fallback
//! elsewhere, GGUF ecosystem, battle-tested. See `docs/PHASE3.md §2`.
//!
//! Lifecycle:
//! - weights live under `~/.tracemind/models/<name>-<hash>.gguf`
//! - lazy-loaded on first [`AnswerBackend::answer`] call
//! - mmap'd; unloaded after `idle_unload_secs` without traffic
//! - ~1.2 GB active RAM, ~600–800 MB idle (mmap only)

use async_trait::async_trait;
use std::path::PathBuf;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Result};

/// Approximate download size for the primary Qwen 2.5 1.5B Q4_K_M weights.
pub const QWEN_1_5B_Q4_APPROX_BYTES: u64 = 900 * 1024 * 1024;

/// Configuration for the local LLM backend.
#[derive(Debug, Clone)]
pub struct LocalLlmConfig {
    /// Absolute path to the GGUF weights file.
    pub model_path: PathBuf,
    /// Unload the model from RAM after this many seconds of inactivity.
    pub idle_unload_secs: u64,
    /// Hard cap on concurrent inferences (1 is correct for a single-user Mac).
    pub max_concurrency: u32,
}

impl LocalLlmConfig {
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            idle_unload_secs: 300,
            max_concurrency: 1,
        }
    }
}

pub struct LocalLlmBackend {
    config: LocalLlmConfig,
}

impl LocalLlmBackend {
    /// Construct the backend. Does **not** load the model — load happens on
    /// first [`AnswerBackend::answer`] call.
    pub fn new(config: LocalLlmConfig) -> Self {
        Self { config }
    }

    /// True if the weights file exists on disk. Used by [`availability`] and
    /// by the first-run download UX.
    pub fn weights_present(&self) -> bool {
        self.config.model_path.exists()
    }
}

#[async_trait]
impl AnswerBackend for LocalLlmBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::LocalLlm
    }

    fn availability(&self) -> BackendAvailability {
        if self.weights_present() {
            BackendAvailability::Ready
        } else {
            BackendAvailability::NeedsDownload {
                approx_bytes: QWEN_1_5B_Q4_APPROX_BYTES,
            }
        }
    }

    async fn answer(&self, _req: &AnswerRequest) -> Result<AnswerResponse> {
        // TODO(TM-5.2-002): wire llama-cpp-2, lazy-load + idle-unload, prompt
        // templates per TaskKind, structured JSON decoding for extraction.
        Err(AnswerError::Unavailable(
            "tier-1 local LLM integration pending (TM-5.2-002)".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskKind;

    #[test]
    fn missing_weights_reports_needs_download() {
        let cfg = LocalLlmConfig::new("/nonexistent/model.gguf");
        let backend = LocalLlmBackend::new(cfg);
        match backend.availability() {
            BackendAvailability::NeedsDownload { approx_bytes } => {
                assert_eq!(approx_bytes, QWEN_1_5B_Q4_APPROX_BYTES);
            }
            other => panic!("expected NeedsDownload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn answer_returns_unavailable_until_wired() {
        let cfg = LocalLlmConfig::new("/nonexistent/model.gguf");
        let backend = LocalLlmBackend::new(cfg);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::Unavailable(_)));
    }
}
