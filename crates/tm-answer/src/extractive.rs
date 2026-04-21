//! Tier 0 — extractive, templated, no LLM.
//!
//! This is always available. It produces answers by concatenating the most
//! relevant retrieved chunks with a light template. It is the baseline we
//! measure Tier 1 / Tier 2 gains against and it is what ships on day one
//! before the user opts into any model download.

use async_trait::async_trait;
use std::time::Instant;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{
    AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, Result, TaskKind,
};

pub struct ExtractiveBackend {
    /// Maximum number of grounding chunks to splice into the answer.
    max_chunks: usize,
}

impl Default for ExtractiveBackend {
    fn default() -> Self {
        Self { max_chunks: 3 }
    }
}

impl ExtractiveBackend {
    pub fn new(max_chunks: usize) -> Self {
        Self {
            max_chunks: max_chunks.max(1),
        }
    }

    fn format(&self, req: &AnswerRequest) -> String {
        if req.grounding.is_empty() {
            return format!("No memories retrieved for: {}", req.question);
        }

        let n = self.max_chunks.min(req.grounding.len());
        let mut out = String::new();
        match req.task {
            TaskKind::ShortAnswer | TaskKind::OpenEndedSynthesis => {
                out.push_str("Relevant memories:\n");
            }
            TaskKind::Summarization => {
                out.push_str("Summary (extractive):\n");
            }
            TaskKind::StructuredExtraction => {
                out.push_str("Candidate facts:\n");
            }
            TaskKind::ContradictionCheck => {
                out.push_str("Comparison (no LLM — review manually):\n");
            }
        }
        for (i, chunk) in req.grounding.iter().take(n).enumerate() {
            out.push_str(&format!("[{}] {}\n", i + 1, truncate(&chunk.text, 280)));
        }
        out
    }
}

#[async_trait]
impl AnswerBackend for ExtractiveBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::Extractive
    }

    fn availability(&self) -> BackendAvailability {
        BackendAvailability::Ready
    }

    async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse> {
        let start = Instant::now();
        if req.grounding.is_empty() && matches!(req.task, TaskKind::ContradictionCheck) {
            return Err(AnswerError::NoGrounding);
        }
        let text = self.format(req);
        let citations = req
            .grounding
            .iter()
            .take(self.max_chunks)
            .enumerate()
            .map(|(i, c)| Citation {
                trace_id: c.trace_id.clone(),
                entity_ids: c.entity_ids.clone(),
                chunk_index: i,
            })
            .collect();
        Ok(AnswerResponse {
            text,
            citations,
            tier: self.tier(),
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GroundingChunk;

    fn chunk(id: &str, text: &str, score: f32) -> GroundingChunk {
        GroundingChunk {
            trace_id: id.to_string(),
            entity_ids: vec![format!("{id}-ent")],
            text: text.to_string(),
            score,
        }
    }

    #[tokio::test]
    async fn extractive_without_grounding_short_answer_ok() {
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("who is alice?", TaskKind::ShortAnswer);
        let resp = backend.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::Extractive);
        assert!(resp.text.contains("No memories"));
        assert!(resp.citations.is_empty());
    }

    #[tokio::test]
    async fn extractive_contradiction_needs_grounding() {
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("does A contradict B?", TaskKind::ContradictionCheck);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::NoGrounding));
    }

    #[tokio::test]
    async fn extractive_cites_up_to_max_chunks() {
        let backend = ExtractiveBackend::new(2);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer).with_grounding(vec![
            chunk("t1", "first memory", 0.9),
            chunk("t2", "second memory", 0.8),
            chunk("t3", "third memory", 0.7),
        ]);
        let resp = backend.answer(&req).await.unwrap();
        assert_eq!(resp.citations.len(), 2);
        assert_eq!(resp.citations[0].trace_id, "t1");
        assert_eq!(resp.citations[1].trace_id, "t2");
        assert!(resp.text.contains("first memory"));
        assert!(resp.text.contains("second memory"));
        assert!(!resp.text.contains("third memory"));
    }

    #[tokio::test]
    async fn extractive_truncates_long_chunks() {
        let long = "x".repeat(500);
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", &long, 1.0)]);
        let resp = backend.answer(&req).await.unwrap();
        assert!(resp.text.contains('…'));
    }

    #[test]
    fn availability_always_ready() {
        let backend = ExtractiveBackend::default();
        assert_eq!(backend.availability(), BackendAvailability::Ready);
    }
}
