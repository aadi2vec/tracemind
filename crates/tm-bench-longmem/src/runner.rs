//! Async runner traits for LongMemEval and BEAM harnesses.
//!
//! Implement [`LongMemRunner`] or [`BeamRunner`] to plug in a memory backend.
//! The [`MockRunner`] satisfies both traits for deterministic CI testing without
//! real inference or embedding.

/// Runner trait for LongMemEval cases.
///
/// The harness calls [`ingest`] for each context turn (in order), then calls
/// [`query`] once per case. Runners are responsible for maintaining any
/// session state between calls within a single case.
#[async_trait::async_trait]
pub trait LongMemRunner: Send + Sync {
    /// Ingest a single conversation turn.
    ///
    /// `session_id` is derived from the case ID and is stable across all turns
    /// of one case, allowing session-aware memory systems to group them.
    async fn ingest(&mut self, text: &str, session_id: &str) -> anyhow::Result<()>;

    /// Pose a question and return the predicted answer string.
    async fn query(&mut self, question: &str) -> anyhow::Result<String>;

    /// Human-readable runner name included in benchmark reports.
    fn name(&self) -> &str;
}

/// Runner trait for BEAM contradiction stress-test cases.
///
/// The harness calls [`ingest`] twice per case (initial claim, then
/// contradicting claim), then calls [`query`] once. The query return value is
/// `(answer, contradiction_surfaced)`.
#[async_trait::async_trait]
pub trait BeamRunner: Send + Sync {
    /// Ingest a claim (initial or contradicting).
    async fn ingest(&mut self, text: &str) -> anyhow::Result<()>;

    /// Pose a query and return `(predicted_answer, contradiction_surfaced)`.
    ///
    /// `contradiction_surfaced` should be `true` when the system explicitly
    /// detects and signals that conflicting facts exist for the queried entity.
    async fn query(&mut self, question: &str) -> anyhow::Result<(String, bool)>;

    /// Human-readable runner name.
    fn name(&self) -> &str;
}

/// Mock runner for CI — no real inference.
///
/// - [`LongMemRunner::query`] echoes the last ingested turn (tests plumbing).
/// - [`BeamRunner::query`] echoes the last ingested text and signals `false`
///   for contradiction (baseline: system does not detect contradictions).
pub struct MockRunner {
    pub runner_name: String,
    last_ingested: String,
}

impl MockRunner {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            runner_name: name.into(),
            last_ingested: String::new(),
        }
    }
}

impl Default for MockRunner {
    fn default() -> Self {
        Self::new("mock")
    }
}

#[async_trait::async_trait]
impl LongMemRunner for MockRunner {
    async fn ingest(&mut self, text: &str, _session_id: &str) -> anyhow::Result<()> {
        self.last_ingested = text.to_string();
        Ok(())
    }

    async fn query(&mut self, _question: &str) -> anyhow::Result<String> {
        // Return the last ingested turn so harness logic exercises the full
        // scoring path without any LLM. F1 will be low but non-zero in cases
        // where the last turn contains the answer token.
        Ok(self.last_ingested.clone())
    }

    fn name(&self) -> &str {
        &self.runner_name
    }
}

#[async_trait::async_trait]
impl BeamRunner for MockRunner {
    async fn ingest(&mut self, text: &str) -> anyhow::Result<()> {
        self.last_ingested = text.to_string();
        Ok(())
    }

    async fn query(&mut self, _question: &str) -> anyhow::Result<(String, bool)> {
        // Returns last ingested text. Contradiction NOT surfaced (baseline).
        Ok((self.last_ingested.clone(), false))
    }

    fn name(&self) -> &str {
        &self.runner_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_longmem_runner_roundtrip() {
        let mut runner = MockRunner::default();
        LongMemRunner::ingest(&mut runner, "Alice works at Acme.", "session-1").await.unwrap();
        LongMemRunner::ingest(&mut runner, "Bob is her manager.", "session-1").await.unwrap();
        let answer = LongMemRunner::query(&mut runner, "Who is Alice's manager?").await.unwrap();
        // Last ingested turn is "Bob is her manager."
        assert_eq!(answer, "Bob is her manager.");
    }

    #[tokio::test]
    async fn mock_beam_runner_roundtrip() {
        let mut runner = MockRunner::default();
        BeamRunner::ingest(&mut runner, "Alice works at Acme.").await.unwrap();
        BeamRunner::ingest(&mut runner, "Alice now works at Beta Corp.").await.unwrap();
        let (answer, contradiction) = BeamRunner::query(&mut runner, "Where does Alice work?").await.unwrap();
        assert_eq!(answer, "Alice now works at Beta Corp.");
        // MockRunner never surfaces contradiction.
        assert!(!contradiction);
    }

    #[tokio::test]
    async fn mock_runner_name() {
        let runner = MockRunner::new("my-mock");
        assert_eq!(LongMemRunner::name(&runner), "my-mock");
    }
}
