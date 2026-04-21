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
//! See `docs/PHASE3.md` for the full strategy.

pub mod backend;
pub mod extractive;
pub mod tiered;
pub mod types;

#[cfg(feature = "local-llm")]
pub mod local_llm;

#[cfg(feature = "apple-fm")]
pub mod apple_fm;

pub use backend::{AnswerBackend, BackendAvailability};
pub use extractive::ExtractiveBackend;
pub use tiered::TieredAnswerer;
pub use types::{AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, TaskKind};
