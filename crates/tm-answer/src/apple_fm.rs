//! Tier 2 — Apple FoundationModels (macOS Tahoe 26+, Apple Silicon).
//!
//! **Status**: scaffolding. The `objc2-foundation-models` bridge lands in the
//! follow-up ticket; today we expose the shape + availability check.
//!
//! Runtime gating:
//! - compile-time: `#[cfg(target_os = "macos")]` + `apple-fm` feature
//! - run-time: check macOS version (≥ 26.0) and `NSProcessInfo` arch (arm64)
//!
//! On older macOS or Intel Macs we return [`BackendAvailability::Unsupported`]
//! and the dispatcher falls back to Tier 1 or Tier 0.

use async_trait::async_trait;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Result};

pub struct AppleFmBackend {
    /// Cached availability check; populated at construction.
    availability: BackendAvailability,
}

impl Default for AppleFmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleFmBackend {
    pub fn new() -> Self {
        Self {
            availability: probe_availability(),
        }
    }
}

#[async_trait]
impl AnswerBackend for AppleFmBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::AppleFm
    }

    fn availability(&self) -> BackendAvailability {
        self.availability.clone()
    }

    async fn answer(&self, _req: &AnswerRequest) -> Result<AnswerResponse> {
        // TODO(TM-5.2-003): wire objc2-foundation-models; implement
        // session-based prompting with structured output where supported.
        Err(AnswerError::Unavailable(
            "tier-2 Apple FoundationModels integration pending (TM-5.2-003)".into(),
        ))
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn probe_availability() -> BackendAvailability {
    // TODO(TM-5.2-003): runtime check NSProcessInfo.operatingSystemVersion
    // for major >= 26. For now, optimistically report Ready on Apple Silicon
    // macOS; the actual binding will flip this to Unsupported on < 26.
    BackendAvailability::Ready
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn probe_availability() -> BackendAvailability {
    BackendAvailability::Unsupported("Apple FoundationModels requires macOS + Apple Silicon")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskKind;

    #[test]
    fn tier_is_apple_fm() {
        let backend = AppleFmBackend::default();
        assert_eq!(backend.tier(), AnswerTier::AppleFm);
    }

    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    #[test]
    fn unsupported_on_non_apple_silicon() {
        let backend = AppleFmBackend::default();
        assert!(matches!(
            backend.availability(),
            BackendAvailability::Unsupported(_)
        ));
    }

    #[tokio::test]
    async fn answer_returns_unavailable_until_wired() {
        let backend = AppleFmBackend::default();
        let req = AnswerRequest::new("q", TaskKind::OpenEndedSynthesis);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::Unavailable(_)));
    }
}
