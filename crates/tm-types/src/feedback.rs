//! Feedback signal types for the Pillar 7 signal fabric (Q3.1).
//!
//! Three signal classes, all ingested as first-class memories:
//! - Explicit: accept/reject on brief cards, thumbs on retrievals
//! - Implicit: memory-reuse rate, dwell/follow-up as miss-signal
//! - Behavioral: verb invocations, host patterns, session timing

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Which class of signal this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackClass {
    /// User explicitly accepted, rejected, or thumbed a result.
    Explicit,
    /// System-inferred signal (reuse rate, silence after proposal).
    Implicit,
    /// Which verb the user invoked and when.
    Behavioral,
}

impl FeedbackClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FeedbackClass::Explicit => "explicit",
            FeedbackClass::Implicit => "implicit",
            FeedbackClass::Behavioral => "behavioral",
        }
    }
}

/// Specific kind within the class.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    // Explicit
    Helpful,
    NotRelated,
    CrossContextBridge,
    CardAccepted,
    CardRejected,
    OutcomeEdited,
    // Implicit
    RetrievalCited,       // retrieval result was referenced in follow-up text
    RetrievalMiss,        // silence after retrieval — no follow-up within threshold
    ProposalSilenced,     // brief card silenced without interaction
    // Behavioral
    VerbInvoked,          // user called a specific MCP verb
    SessionStarted,
    SessionEnded,
}

impl FeedbackKind {
    pub fn class(&self) -> FeedbackClass {
        match self {
            FeedbackKind::Helpful
            | FeedbackKind::NotRelated
            | FeedbackKind::CrossContextBridge
            | FeedbackKind::CardAccepted
            | FeedbackKind::CardRejected
            | FeedbackKind::OutcomeEdited => FeedbackClass::Explicit,

            FeedbackKind::RetrievalCited
            | FeedbackKind::RetrievalMiss
            | FeedbackKind::ProposalSilenced => FeedbackClass::Implicit,

            FeedbackKind::VerbInvoked
            | FeedbackKind::SessionStarted
            | FeedbackKind::SessionEnded => FeedbackClass::Behavioral,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FeedbackKind::Helpful => "helpful",
            FeedbackKind::NotRelated => "not_related",
            FeedbackKind::CrossContextBridge => "cross_context_bridge",
            FeedbackKind::CardAccepted => "card_accepted",
            FeedbackKind::CardRejected => "card_rejected",
            FeedbackKind::OutcomeEdited => "outcome_edited",
            FeedbackKind::RetrievalCited => "retrieval_cited",
            FeedbackKind::RetrievalMiss => "retrieval_miss",
            FeedbackKind::ProposalSilenced => "proposal_silenced",
            FeedbackKind::VerbInvoked => "verb_invoked",
            FeedbackKind::SessionStarted => "session_started",
            FeedbackKind::SessionEnded => "session_ended",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "helpful" => FeedbackKind::Helpful,
            "not_related" => FeedbackKind::NotRelated,
            "cross_context_bridge" => FeedbackKind::CrossContextBridge,
            "card_accepted" => FeedbackKind::CardAccepted,
            "card_rejected" => FeedbackKind::CardRejected,
            "outcome_edited" => FeedbackKind::OutcomeEdited,
            "retrieval_cited" => FeedbackKind::RetrievalCited,
            "retrieval_miss" => FeedbackKind::RetrievalMiss,
            "proposal_silenced" => FeedbackKind::ProposalSilenced,
            "verb_invoked" => FeedbackKind::VerbInvoked,
            "session_started" => FeedbackKind::SessionStarted,
            "session_ended" => FeedbackKind::SessionEnded,
            _ => return None,
        })
    }
}

/// A single feedback signal — the unit that the signal fabric stores.
/// Ingested as a first-class memory with source provenance.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeedbackSignal {
    pub id: Uuid,
    pub kind: FeedbackKind,
    pub class: FeedbackClass,
    /// The retrieval/query that generated this feedback opportunity.
    pub feedback_hook_id: Uuid,
    /// The specific result row being judged (entity/triple UUID), if applicable.
    pub target_id: Option<Uuid>,
    /// Scalar score: positive = good signal, negative = bad signal, 0 = neutral.
    pub score: f32,
    /// For behavioral signals: which verb was invoked.
    pub verb: Option<String>,
    /// Which MCP host originated this signal.
    pub host_id: Option<String>,
    /// Active session at time of signal.
    pub session_id: Option<Uuid>,
    pub recorded_at: DateTime<Utc>,
}

impl FeedbackSignal {
    pub fn new(
        kind: FeedbackKind,
        feedback_hook_id: Uuid,
        target_id: Option<Uuid>,
        score: f32,
    ) -> Self {
        let class = kind.class();
        Self {
            id: Uuid::new_v4(),
            kind,
            class,
            feedback_hook_id,
            target_id,
            score,
            verb: None,
            host_id: None,
            session_id: None,
            recorded_at: Utc::now(),
        }
    }

    pub fn with_verb(mut self, verb: impl Into<String>) -> Self {
        self.verb = Some(verb.into());
        self
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host_id = Some(host.into());
        self
    }

    pub fn with_session(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }
}
