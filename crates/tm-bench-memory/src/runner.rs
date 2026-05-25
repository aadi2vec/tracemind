//! `PersistenceRunner` trait — anything benchmarkable plugs in here.
//!
//! The trait splits the workflow into `session_a_store` and
//! `session_b_query` (rather than LoCoMo's `ingest_sample` + `answer`)
//! because the boundary *between* the two calls is what the benchmark
//! is testing. Implementations are expected to drop any in-process
//! state at the end of `session_a_store` so that the query happens on
//! a fresh handle.

use async_trait::async_trait;

use crate::dataset::PersistencePair;

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

    /// Session B — open a fresh handle against the same backing store
    /// (whatever that means for the runner), run the query, return the
    /// answer string.
    async fn session_b_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String>;
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
    use crate::dataset::{Category, PersistencePair};

    fn pair() -> PersistencePair {
        PersistencePair {
            id: "p1".into(),
            category: Category::FactualRecall,
            store: vec!["X is Y.".into()],
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
