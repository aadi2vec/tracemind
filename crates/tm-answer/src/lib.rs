//! Tiered answer layer for TraceMind.
//!
//! Three backends share a single [`AnswerBackend`] trait:
//!
//! | Tier | Backend               | Install cost        | Used for                                   |
//! |------|-----------------------|---------------------|--------------------------------------------|
//! | 0    | [`ExtractiveBackend`] | 0 MB (always on)    | Templated answers from retrieved chunks    |
//! | 1    | [`LocalLlmBackend`]   | ~900 MB opt-in GGUF | Extraction, contradictions, short Q&A      |
//! | 2    | [`AppleFmBackend`]    | 0 MB on macOS 26 AS | Open-ended synthesis, long-context answers |
//!
//! [`TieredAnswerer`] composes all three and dispatches by task kind + availability.
//! See `docs/PHASE4_DELIGHT.md` for the current strategy (Phase 3 completed; superseded).

pub mod backend;
pub mod extractive;
pub mod tiered;
pub mod types;

// `local_llm` is gated on the `local-llm` feature for *inference*. The
// module itself (config, prompt builder, default paths, lifecycle skeleton)
// is always compiled so that downstream crates can reference the types and
// run prompt golden-tests without pulling llama.cpp.
pub mod local_llm;

#[cfg(feature = "apple-fm")]
pub mod apple_fm;

pub use backend::{AnswerBackend, BackendAvailability};
pub use extractive::ExtractiveBackend;
pub use local_llm::{
    default_model_path, LocalLlmBackend, LocalLlmConfig, HF_FILE_MOBILE, HF_FILE_PRIMARY,
    HF_REPO_MOBILE, HF_REPO_PRIMARY, QWEN_1_5B_Q4_APPROX_BYTES,
};
pub use tiered::TieredAnswerer;
pub use types::{
    AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, GroundingChunk, TaskKind,
};
