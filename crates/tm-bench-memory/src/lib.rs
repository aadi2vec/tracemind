//! Cross-session persistence microbenchmark (W-3 from `docs/TASKS.md`).
//!
//! **What this measures, and why it's different from `tm-bench-locomo`.**
//!
//! LoCoMo is a *long-conversation* benchmark — it asks whether a memory
//! system can answer questions about a multi-session dialogue that's all
//! loaded into one process. The persistence we care about for the seed
//! wedge is narrower and harsher: can TraceMind recall a fact captured in
//! one process when a *different* process — different CWD, different
//! ingest session — asks about it later?
//!
//! Mem0 / Letta / Zep all keep their session state in volatile per-process
//! caches; restart the client and the fact is gone unless the user
//! explicitly persisted it. TraceMind's whole pitch is that *every*
//! ingest goes through the local SQLite + ColBERT graph, so a fresh
//! process opening the same `TM_DATA_DIR` can recall it. W-3 is the
//! microbenchmark that turns that pitch into a number.
//!
//! **Pair shape.** Each pair carries one or more `store` sentences (what
//! gets ingested in Session A) and one `query` to run in Session B, with
//! a list of acceptable answers. Categories:
//!
//! | Category                | What we're testing                       |
//! |-------------------------|------------------------------------------|
//! | `factual_recall`        | Simple "X = Y" facts survive a restart   |
//! | `name_resolution`       | Co-referenced names resolve cross-process|
//! | `decision_history`      | Past decisions remain queryable          |
//! | `contradiction_recovery`| Retractions stick across the restart     |
//! | `temporal_pinning`      | Date hints survive (May 3rd → May 3rd)   |
//!
//! **Why a fresh process matters.** A pure in-memory test that holds the
//! `RetrievalEngine` open between ingest and query is too lenient — it
//! catches retrieval bugs but not persistence bugs. The
//! `TraceMindRunner` here drops `IngestPipeline`, lets the WAL flush,
//! then reopens `RetrievalEngine` against the same SQLite file. That's
//! the actual claim we make to design partners.
//!
//! See `crates/tm-bench-memory/fixtures/persistence-mini.json` for the
//! hand-labeled 50-pair set used as the seed-gate baseline.

pub mod dataset;
pub mod extract;
pub mod report;
pub mod runner;
pub mod scoring;

#[cfg(feature = "tracemind")]
pub mod tracemind_runner;

#[cfg(feature = "tracemind")]
pub use tracemind_runner::{TraceMindConfig, TraceMindPersistenceRunner};

pub use dataset::{Category, PersistenceDataset, PersistenceError, PersistencePair};
pub use report::{BenchmarkReport, CategoryBreakdown, PairOutcome};
pub use runner::{EchoRunner, NullRunner, PersistenceRunner, RunnerContext};
pub use scoring::{best_exact_match, best_f1, exact_match, token_f1, Score};
