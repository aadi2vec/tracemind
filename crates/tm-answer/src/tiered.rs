//! [`TieredAnswerer`] — dispatches requests to the best available tier.
//!
//! Selection policy:
//! 1. If `req.preferred_tier` is set and that backend is `Ready`, use it.
//! 2. If task is structured ([`TaskKind::is_structured`]) and Tier 1 is
//!    `Ready`, prefer Tier 1 (cheaper than FM, equally accurate for
//!    constrained outputs).
//! 3. Else prefer Tier 2 → Tier 1 → Tier 0 in order of availability.
//!
//! Tier 0 is always available, so the dispatcher never fails to find a backend.

use std::sync::Arc;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{AnswerRequest, AnswerResponse, AnswerTier, Result};

pub struct TieredAnswerer {
    extractive: Arc<dyn AnswerBackend>,
    local_llm: Option<Arc<dyn AnswerBackend>>,
    apple_fm: Option<Arc<dyn AnswerBackend>>,
}

impl TieredAnswerer {
    /// Minimal constructor — just the always-on Tier 0. Use the builder-style
    /// `with_*` setters to attach optional tiers.
    pub fn new(extractive: Arc<dyn AnswerBackend>) -> Self {
        Self {
            extractive,
            local_llm: None,
            apple_fm: None,
        }
    }

    pub fn with_local_llm(mut self, backend: Arc<dyn AnswerBackend>) -> Self {
        self.local_llm = Some(backend);
        self
    }

    pub fn with_apple_fm(mut self, backend: Arc<dyn AnswerBackend>) -> Self {
        self.apple_fm = Some(backend);
        self
    }

    /// Report which tiers are currently ready. Useful for UI that shows
    /// "answers: ready / sleeping / needs-download".
    pub fn tier_status(&self) -> Vec<(AnswerTier, BackendAvailability)> {
        let mut out = vec![(AnswerTier::Extractive, self.extractive.availability())];
        if let Some(b) = &self.local_llm {
            out.push((AnswerTier::LocalLlm, b.availability()));
        }
        if let Some(b) = &self.apple_fm {
            out.push((AnswerTier::AppleFm, b.availability()));
        }
        out
    }

    fn select(&self, req: &AnswerRequest) -> Arc<dyn AnswerBackend> {
        // 1. Explicit preference wins if ready.
        if let Some(tier) = req.preferred_tier {
            if let Some(b) = self.backend_for(tier) {
                if matches!(b.availability(), BackendAvailability::Ready) {
                    return b;
                }
            }
        }

        // 2. Structured tasks prefer Tier 1 over Tier 2 when ready.
        if req.task.is_structured() {
            if let Some(b) = self.local_llm.as_ref() {
                if matches!(b.availability(), BackendAvailability::Ready) {
                    return b.clone();
                }
            }
        }

        // 3. Open-ended / fallback: Tier 2 → Tier 1 → Tier 0.
        if let Some(b) = self.apple_fm.as_ref() {
            if matches!(b.availability(), BackendAvailability::Ready) {
                return b.clone();
            }
        }
        if let Some(b) = self.local_llm.as_ref() {
            if matches!(b.availability(), BackendAvailability::Ready) {
                return b.clone();
            }
        }
        self.extractive.clone()
    }

    fn backend_for(&self, tier: AnswerTier) -> Option<Arc<dyn AnswerBackend>> {
        match tier {
            AnswerTier::Extractive => Some(self.extractive.clone()),
            AnswerTier::LocalLlm => self.local_llm.clone(),
            AnswerTier::AppleFm => self.apple_fm.clone(),
        }
    }

    /// Dispatch to the selected tier. Never fails to select — Tier 0 is
    /// guaranteed ready.
    pub async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse> {
        let backend = self.select(req);
        backend.answer(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractive::ExtractiveBackend;
    use crate::types::{AnswerError, GroundingChunk, TaskKind};
    use async_trait::async_trait;

    /// Test double: a backend whose availability and answer behavior are
    /// controlled by the test.
    struct FakeBackend {
        tier: AnswerTier,
        availability: BackendAvailability,
        response_text: String,
    }

    #[async_trait]
    impl AnswerBackend for FakeBackend {
        fn tier(&self) -> AnswerTier {
            self.tier
        }
        fn availability(&self) -> BackendAvailability {
            self.availability.clone()
        }
        async fn answer(&self, _req: &AnswerRequest) -> Result<AnswerResponse> {
            if !matches!(self.availability, BackendAvailability::Ready) {
                return Err(AnswerError::Unavailable(format!("{:?}", self.availability)));
            }
            Ok(AnswerResponse {
                text: self.response_text.clone(),
                citations: vec![],
                tier: self.tier,
                latency_ms: 0,
            })
        }
    }

    fn extractive() -> Arc<dyn AnswerBackend> {
        Arc::new(ExtractiveBackend::default())
    }

    fn fake(tier: AnswerTier, avail: BackendAvailability, text: &str) -> Arc<dyn AnswerBackend> {
        Arc::new(FakeBackend {
            tier,
            availability: avail,
            response_text: text.to_string(),
        })
    }

    #[tokio::test]
    async fn falls_back_to_extractive_when_no_llm_tiers() {
        let t = TieredAnswerer::new(extractive());
        let req = AnswerRequest::new("q", TaskKind::OpenEndedSynthesis)
            .with_grounding(vec![GroundingChunk {
                trace_id: "t1".into(),
                entity_ids: vec![],
                text: "hello".into(),
                score: 1.0,
            }]);
        let resp = t.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::Extractive);
    }

    #[tokio::test]
    async fn open_ended_prefers_apple_fm_when_ready() {
        let t = TieredAnswerer::new(extractive())
            .with_local_llm(fake(AnswerTier::LocalLlm, BackendAvailability::Ready, "L1"))
            .with_apple_fm(fake(AnswerTier::AppleFm, BackendAvailability::Ready, "L2"));
        let req = AnswerRequest::new("q", TaskKind::OpenEndedSynthesis);
        let resp = t.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::AppleFm);
    }

    #[tokio::test]
    async fn structured_task_prefers_local_llm_over_apple_fm() {
        let t = TieredAnswerer::new(extractive())
            .with_local_llm(fake(AnswerTier::LocalLlm, BackendAvailability::Ready, "L1"))
            .with_apple_fm(fake(AnswerTier::AppleFm, BackendAvailability::Ready, "L2"));
        let req = AnswerRequest::new("extract entities", TaskKind::StructuredExtraction);
        let resp = t.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::LocalLlm);
    }

    #[tokio::test]
    async fn explicit_preference_honoured_when_ready() {
        let t = TieredAnswerer::new(extractive())
            .with_local_llm(fake(AnswerTier::LocalLlm, BackendAvailability::Ready, "L1"))
            .with_apple_fm(fake(AnswerTier::AppleFm, BackendAvailability::Ready, "L2"));
        let req = AnswerRequest::new("q", TaskKind::OpenEndedSynthesis)
            .with_preferred_tier(AnswerTier::LocalLlm);
        let resp = t.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::LocalLlm);
    }

    #[tokio::test]
    async fn explicit_preference_downgrades_if_not_ready() {
        let t = TieredAnswerer::new(extractive()).with_local_llm(fake(
            AnswerTier::LocalLlm,
            BackendAvailability::NeedsDownload {
                approx_bytes: 900_000_000,
            },
            "L1",
        ));
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_preferred_tier(AnswerTier::LocalLlm)
            .with_grounding(vec![GroundingChunk {
                trace_id: "t1".into(),
                entity_ids: vec![],
                text: "hello".into(),
                score: 1.0,
            }]);
        let resp = t.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::Extractive);
    }

    #[tokio::test]
    async fn tier_status_lists_all_attached_backends() {
        let t = TieredAnswerer::new(extractive())
            .with_local_llm(fake(
                AnswerTier::LocalLlm,
                BackendAvailability::NeedsDownload {
                    approx_bytes: 900_000_000,
                },
                "L1",
            ))
            .with_apple_fm(fake(
                AnswerTier::AppleFm,
                BackendAvailability::Unsupported("test"),
                "L2",
            ));
        let status = t.tier_status();
        assert_eq!(status.len(), 3);
        assert_eq!(status[0].0, AnswerTier::Extractive);
        assert_eq!(status[0].1, BackendAvailability::Ready);
        assert!(matches!(
            status[1].1,
            BackendAvailability::NeedsDownload { .. }
        ));
        assert!(matches!(status[2].1, BackendAvailability::Unsupported(_)));
    }
}
