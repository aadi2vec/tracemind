use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Full provenance record for a single ingestion or retrieval event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub id: Uuid,
    pub session_id: Uuid,
    pub event_type: TraceEventType,
    pub content_hash: String,
    pub raw_text: Option<String>,
    pub entities_extracted: Vec<Uuid>,
    pub triples_extracted: Vec<Uuid>,
    pub retrieval_arm: Option<u8>,
    pub retrieval_latency_ms: Option<u32>,
    pub confidence_gate_passed: bool,
    pub created_at: DateTime<Utc>,
}

impl Trace {
    pub fn new(session_id: Uuid, event_type: TraceEventType, content_hash: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            event_type,
            content_hash: content_hash.into(),
            raw_text: None,
            entities_extracted: Vec::new(),
            triples_extracted: Vec::new(),
            retrieval_arm: None,
            retrieval_latency_ms: None,
            confidence_gate_passed: true,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceEventType {
    Ingest,
    Retrieve,
    Feedback,
    ProcedureExec,
    Decay,
}
