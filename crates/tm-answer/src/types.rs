//! Shared types for the tiered answer layer.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which tier produced (or should produce) the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerTier {
    /// Tier 0 — extractive, no LLM.
    Extractive,
    /// Tier 1 — bundled local LLM (e.g. Qwen 2.5 1.5B Q4).
    LocalLlm,
    /// Tier 2 — Apple FoundationModels (macOS Tahoe 26+ AS).
    AppleFm,
}

/// Coarse task classification used for backend selection.
///
/// Structured tasks have tight output formats (JSON, bullet lists) where a 1B
/// model with careful prompting matches a 3B model. Open-ended tasks benefit
/// materially from the larger model when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Extract entities / relations / facts as structured output.
    StructuredExtraction,
    /// Detect whether two statements contradict each other.
    ContradictionCheck,
    /// Short factual answer grounded in retrieved chunks.
    ShortAnswer,
    /// Open-ended synthesis over many chunks; benefits from larger model.
    OpenEndedSynthesis,
    /// Summarize a single document / chunk.
    Summarization,
}

impl TaskKind {
    /// Returns true when this task is structured enough that Tier 1 is
    /// expected to match Tier 2 quality. Used by the dispatcher when Tier 2
    /// is technically available but Tier 1 is cheaper/faster.
    pub fn is_structured(self) -> bool {
        matches!(
            self,
            TaskKind::StructuredExtraction
                | TaskKind::ContradictionCheck
                | TaskKind::Summarization
        )
    }
}

/// A single retrieved chunk that the answerer can ground on. The `trace_id`
/// and `entity_ids` are copied verbatim into any [`Citation`] we emit —
/// every synthesized answer must be cite-able back to a trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingChunk {
    pub trace_id: String,
    pub entity_ids: Vec<String>,
    pub text: String,
    pub score: f32,
}

/// Request sent to an [`AnswerBackend`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerRequest {
    /// User's natural-language question (or prompt for extraction tasks).
    pub question: String,
    /// Chunks retrieved by `tm-retrieval`, already ranked.
    pub grounding: Vec<GroundingChunk>,
    pub task: TaskKind,
    /// Hard cap on output tokens. Backends should respect this.
    pub max_output_tokens: u32,
    /// Optional preferred tier; dispatcher may downgrade if unavailable.
    pub preferred_tier: Option<AnswerTier>,
    /// Topics the system *does* know about, when it cannot answer the
    /// question itself. Used to turn a dead end into a useful abstention:
    /// "nothing on that; closest topics are X, Y" beats an empty string,
    /// which is indistinguishable from a crash or a stopped daemon.
    pub nearby_topics: Vec<String>,
}

impl AnswerRequest {
    pub fn new(question: impl Into<String>, task: TaskKind) -> Self {
        Self {
            question: question.into(),
            grounding: Vec::new(),
            task,
            max_output_tokens: 256,
            preferred_tier: None,
            nearby_topics: Vec::new(),
        }
    }

    pub fn with_grounding(mut self, chunks: Vec<GroundingChunk>) -> Self {
        self.grounding = chunks;
        self
    }

    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_output_tokens = n;
        self
    }

    pub fn with_preferred_tier(mut self, tier: AnswerTier) -> Self {
        self.preferred_tier = Some(tier);
        self
    }
}

/// A citation from the grounding set back to a trace / entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub trace_id: String,
    pub entity_ids: Vec<String>,
    /// Which grounding chunk (0-indexed) this citation points at.
    pub chunk_index: usize,
}

/// Response from an [`AnswerBackend`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerResponse {
    pub text: String,
    pub citations: Vec<Citation>,
    pub tier: AnswerTier,
    /// Wall-clock milliseconds the backend spent producing the answer.
    pub latency_ms: u64,
}

#[derive(Debug, Error)]
pub enum AnswerError {
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    #[error("no grounding provided and task requires it")]
    NoGrounding,
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("model load failed: {0}")]
    ModelLoad(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AnswerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_task_detection() {
        assert!(TaskKind::StructuredExtraction.is_structured());
        assert!(TaskKind::ContradictionCheck.is_structured());
        assert!(TaskKind::Summarization.is_structured());
        assert!(!TaskKind::ShortAnswer.is_structured());
        assert!(!TaskKind::OpenEndedSynthesis.is_structured());
    }

    #[test]
    fn request_builder() {
        let req = AnswerRequest::new("who is alice?", TaskKind::ShortAnswer)
            .with_max_tokens(64)
            .with_preferred_tier(AnswerTier::LocalLlm);
        assert_eq!(req.max_output_tokens, 64);
        assert_eq!(req.preferred_tier, Some(AnswerTier::LocalLlm));
        assert!(req.grounding.is_empty());
    }
}
