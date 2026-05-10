//! LoCoMo benchmark harness for TraceMind.
//!
//! LoCoMo (Long Conversation Memory) evaluates memory systems on multi-session
//! dialogues with 5 question categories (single-hop, multi-hop, temporal,
//! open-domain, adversarial). Published baselines:
//!
//! | System                  | LoCoMo score |
//! |-------------------------|--------------|
//! | Honcho 3                | SOTA         |
//! | Mem0                    | 91.6         |
//! | SuperLocalMemory Mode C | 87.7         |
//! | Engram                  | 80.0         |
//! | Zep                     | 75.14        |
//! | Letta                   | 74.0         |
//! | **TraceMind target**    | **≥ 85**     |
//!
//! This crate provides:
//! - [`dataset`] — LoCoMo JSON schema + loader
//! - [`scoring`] — token F1 + exact-match scorers (reproducible without LLM-judge)
//! - [`runner`] — [`LocomoRunner`] trait that downstream integrations implement
//! - [`report`] — per-category breakdown + overall score + JSON serialization
//!
//! The CLI binary wires it all together and is CI-gated: any PR that drops
//! LoCoMo >0.5 points fails to merge. See `docs/LOCOMO_RESULTS.md`.

pub mod dataset;
pub mod extract;
pub mod report;
pub mod runner;
pub mod scoring;

#[cfg(feature = "tracemind")]
pub mod tracemind_runner;

#[cfg(feature = "tracemind")]
pub use tracemind_runner::{TraceMindConfig, TraceMindRunner};

pub use dataset::{LocomoDataset, LocomoError, LocomoQuestion, LocomoSample, LocomoSession, Turn};
pub use report::{BenchmarkReport, CategoryBreakdown, QuestionOutcome};
pub use runner::{LocomoRunner, RunnerContext};
pub use scoring::{exact_match, token_f1, Score};
