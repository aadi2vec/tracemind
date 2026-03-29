use thiserror::Error;

#[derive(Debug, Error)]
pub enum TraceMindError {
    #[error("entity not found: {0}")]
    EntityNotFound(String),

    #[error("triple not found: {0}")]
    TripleNotFound(String),

    #[error("procedure not found: {0}")]
    ProcedureNotFound(String),

    #[error("confidence below threshold: {confidence:.2} < {threshold:.2}")]
    ConfidenceBelowThreshold { confidence: f64, threshold: f64 },

    #[error("PII detected in input")]
    PiiDetected,

    #[error("governance filter rejected input: {reason}")]
    GovernanceRejected { reason: String },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("procedure step failed at ordinal {ordinal}: {reason}")]
    ProcedureStepFailed { ordinal: u32, reason: String },

    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, TraceMindError>;
