//! LongMemEval + BEAM benchmark harnesses for TraceMind.
//!
//! **LongMemEval** evaluates multi-hop and temporal reasoning over long memory
//! contexts across 5 question categories (single-hop, multi-hop, temporal,
//! open-domain, adversarial). Complements the LoCoMo harness by focusing on
//! retrieval fidelity over longer session histories.
//!
//! **BEAM** (Belief Edit And Memory) stress-tests contradiction handling: ingest
//! an initial claim, then ingest a contradicting claim, then query the fact.
//! A correct system returns the updated answer (or flags the contradiction).
//!
//! This crate provides:
//! - [`longmem`] — LongMemEval types and dataset loader
//! - [`beam`]    — BEAM types and dataset loader
//! - [`runner`]  — async runner traits + `MockRunner` for CI
//! - [`metrics`] — shared token F1 + exact-match scorers

pub mod beam;
pub mod longmem;
pub mod metrics;
pub mod runner;

pub use beam::{BeamCase, BeamDataset, BeamResult};
pub use longmem::{LongMemCase, LongMemCategory, LongMemDataset, LongMemResult};
pub use metrics::{compute_em, compute_token_f1, CategoryMetrics, EvalMetrics};
pub use runner::{BeamRunner, LongMemRunner, MockRunner};
