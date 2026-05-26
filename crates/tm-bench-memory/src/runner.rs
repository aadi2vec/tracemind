//! `PersistenceRunner` trait — anything benchmarkable plugs in here.
//!
//! Two modes:
//!
//! 1. **Per-pair** (`session_a_store` → `session_b_query`). The legacy
//!    layout from the early seed-gate work: each pair gets a fresh DB.
//!    Trivially solvable (no cross-pair contamination, retrieval has
//!    1–2 candidates per query), kept for back-compat smoke tests.
//!
//! 2. **Shared-DB bulk** (`bulk_ingest` once, then `query` per pair).
//!    The honest mode: one process writes the WHOLE corpus into ONE DB,
//!    drops, then a fresh process opens the same DB once and answers
//!    every query against it. Retrieval has to find the needle among
//!    hundreds of sentences from other pairs + the noise corpus.
//!
//! Default impls bridge the two — runners only need to implement one
//! side.

use async_trait::async_trait;

use crate::dataset::{PersistenceDataset, PersistencePair};

pub struct RunnerContext<'a> {
    pub pair: &'a PersistencePair,
}

#[async_trait]
pub trait PersistenceRunner: Send {
    /// Human-readable name for the report.
    fn name(&self) -> &str;

    /// Session A — ingest the pair's `store` sentences into a *fresh*
    /// per-pair backing store. Runners are expected to close all
    /// in-memory handles before returning so that `session_b_query`
    /// genuinely exercises a cold reopen.
    async fn session_a_store(&mut self, ctx: &RunnerContext<'_>) -> Result<(), String>;

    /// Session B — open a fresh handle against the same backing store,
    /// run the query, return the answer string.
    async fn session_b_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String>;

    /// **Shared-DB Session A.** Ingest the entire dataset (every
    /// pair's `store` + `distractors` + the global `noise_corpus`) into
    /// ONE backing store and then close it. The default impl falls
    /// back to repeated per-pair ingest, which works for stateless
    /// runners (Echo / Null) but isn't realistic for storage runners
    /// — override it.
    async fn bulk_ingest(&mut self, dataset: &PersistenceDataset) -> Result<(), String> {
        for pair in &dataset.pairs {
            let ctx = RunnerContext { pair };
            self.session_a_store(&ctx).await?;
        }
        Ok(())
    }

    /// **Shared-DB Session B.** Open the engine once and run a query.
    /// Engines that cache across calls override this; the default just
    /// delegates to `session_b_query`.
    async fn query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        self.session_b_query(ctx).await
    }
}

/// Trivial runner that returns the first reference answer verbatim.
/// F1 = 1.0 on every pair — used to smoke-test the harness plumbing.
pub struct EchoRunner;

#[async_trait]
impl PersistenceRunner for EchoRunner {
    fn name(&self) -> &str {
        "echo-oracle"
    }
    async fn session_a_store(&mut self, _ctx: &RunnerContext<'_>) -> Result<(), String> {
        Ok(())
    }
    async fn session_b_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        Ok(ctx
            .pair
            .answers
            .first()
            .cloned()
            .unwrap_or_default())
    }
}

/// Trivial runner that returns an empty string. F1 = 0.0 on every
/// pair — used to smoke-test the CI gate failure path.
pub struct NullRunner;

#[async_trait]
impl PersistenceRunner for NullRunner {
    fn name(&self) -> &str {
        "null-baseline"
    }
    async fn session_a_store(&mut self, _ctx: &RunnerContext<'_>) -> Result<(), String> {
        Ok(())
    }
    async fn session_b_query(&mut self, _ctx: &RunnerContext<'_>) -> Result<String, String> {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{Category, PersistencePair, Split};

    fn pair() -> PersistencePair {
        PersistencePair {
            id: "p1".into(),
            category: Category::FactualRecall,
            split: Split::Test,
            store: vec!["X is Y.".into()],
            distractors: vec![],
            query: "What is X?".into(),
            answers: vec!["Y".into()],
            note: None,
        }
    }

    #[tokio::test]
    async fn echo_returns_first_answer() {
        let p = pair();
        let ctx = RunnerContext { pair: &p };
        let mut r = EchoRunner;
        r.session_a_store(&ctx).await.unwrap();
        let out = r.session_b_query(&ctx).await.unwrap();
        assert_eq!(out, "Y");
    }

    #[tokio::test]
    async fn null_returns_empty() {
        let p = pair();
        let ctx = RunnerContext { pair: &p };
        let mut r = NullRunner;
        assert_eq!(r.session_b_query(&ctx).await.unwrap(), "");
    }
}
