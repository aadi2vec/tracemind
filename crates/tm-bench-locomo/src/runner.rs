//! The [`LocomoRunner`] trait — plugged-in memory system under test.
//!
//! The harness is intentionally decoupled from the full TraceMind retrieval
//! stack: any struct that implements [`LocomoRunner`] can be benchmarked. The
//! first real runner (wiring `tm-ingest` + `tm-retrieval` + `tm-answer`)
//! lands in a follow-up; until then, [`EchoRunner`] provides a deterministic
//! trivial baseline for CI to exercise the full pipeline end-to-end.

use async_trait::async_trait;

use crate::dataset::{LocomoQuestion, LocomoSample};

/// Runtime context for a single sample run. The runner is expected to ingest
/// the sample's sessions (conversation turns) into its memory, then answer
/// each question in order. The harness hands the sample in once, then calls
/// [`LocomoRunner::answer`] per question.
pub struct RunnerContext<'a> {
    pub sample: &'a LocomoSample,
}

#[async_trait]
/// Implementations are run sequentially by the harness, so we only require
/// [`Send`] (not `Sync`). This lets backends hold `!Sync` resources like
/// rusqlite connections directly.
pub trait LocomoRunner: Send {
    /// Human-readable name for the report (e.g. "tracemind-v0.1").
    fn name(&self) -> &str;

    /// Ingest the sample's conversation into the system under test. Called
    /// once per sample before any [`LocomoRunner::answer`] calls. Runners
    /// that need to isolate state between samples should reset here.
    async fn ingest_sample(&mut self, ctx: &RunnerContext<'_>) -> Result<(), String>;

    /// Produce an answer to a single question.
    async fn answer(
        &mut self,
        ctx: &RunnerContext<'_>,
        question: &LocomoQuestion,
    ) -> Result<String, String>;
}

/// Trivial runner used in tests and as a smoke-check baseline. Returns the
/// first reference answer verbatim so F1 = 1.0 on every question. Useful for
/// validating the harness plumbing without pulling in the full stack.
pub struct EchoRunner;

#[async_trait]
impl LocomoRunner for EchoRunner {
    fn name(&self) -> &str {
        "echo-oracle"
    }

    async fn ingest_sample(&mut self, _ctx: &RunnerContext<'_>) -> Result<(), String> {
        Ok(())
    }

    async fn answer(
        &mut self,
        _ctx: &RunnerContext<'_>,
        question: &LocomoQuestion,
    ) -> Result<String, String> {
        Ok(question
            .answers
            .first()
            .cloned()
            .unwrap_or_default())
    }
}

/// Trivial runner that always returns an empty string. F1 = 0.0 on every
/// question. Used to verify the CI gate fails loudly when the system breaks.
pub struct NullRunner;

#[async_trait]
impl LocomoRunner for NullRunner {
    fn name(&self) -> &str {
        "null-baseline"
    }

    async fn ingest_sample(&mut self, _ctx: &RunnerContext<'_>) -> Result<(), String> {
        Ok(())
    }

    async fn answer(
        &mut self,
        _ctx: &RunnerContext<'_>,
        _question: &LocomoQuestion,
    ) -> Result<String, String> {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{Category, LocomoQuestion, LocomoSample};

    fn sample() -> LocomoSample {
        LocomoSample {
            sample_id: "s1".into(),
            sessions: vec![],
            questions: vec![LocomoQuestion {
                id: "q1".into(),
                question: "?".into(),
                answers: vec!["hello".into()],
                category: Category::SingleHop,
                evidence: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn echo_returns_first_answer() {
        let s = sample();
        let ctx = RunnerContext { sample: &s };
        let mut r = EchoRunner;
        r.ingest_sample(&ctx).await.unwrap();
        let out = r.answer(&ctx, &s.questions[0]).await.unwrap();
        assert_eq!(out, "hello");
        assert_eq!(r.name(), "echo-oracle");
    }

    #[tokio::test]
    async fn null_returns_empty() {
        let s = sample();
        let ctx = RunnerContext { sample: &s };
        let mut r = NullRunner;
        let out = r.answer(&ctx, &s.questions[0]).await.unwrap();
        assert_eq!(out, "");
        assert_eq!(r.name(), "null-baseline");
    }
}
