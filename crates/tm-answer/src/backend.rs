//! [`AnswerBackend`] trait — the contract every tier implements.

use async_trait::async_trait;

use crate::types::{AnswerRequest, AnswerResponse, AnswerTier, Result};

/// Runtime availability of a backend. The dispatcher uses this to pick a
/// tier without attempting init on every call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendAvailability {
    /// Backend is ready to serve requests immediately.
    Ready,
    /// Backend is compiled in but weights/model not yet downloaded.
    NeedsDownload { approx_bytes: u64 },
    /// Backend was excluded at compile time or the host doesn't support it
    /// (e.g. Apple FM on Intel macOS 15).
    Unsupported(&'static str),
}

#[async_trait]
pub trait AnswerBackend: Send + Sync {
    /// Which tier this backend represents.
    fn tier(&self) -> AnswerTier;

    /// Cheap introspection — must not perform I/O.
    fn availability(&self) -> BackendAvailability;

    /// Produce an answer. Implementations must populate
    /// [`AnswerResponse::tier`] with [`Self::tier`] and measure latency.
    async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse>;
}
