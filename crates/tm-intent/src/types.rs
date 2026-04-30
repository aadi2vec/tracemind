//! Data model for the system of intents.
//!
//! All types here are pure data + serde. No I/O, no DB. The
//! state-machine rules live in [`crate::state`]; persistence in
//! [`crate::store`]. This module is canonical against
//! `docs/INTENT_SYSTEM.md` §1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Commitment
// ---------------------------------------------------------------------------

/// A forward-leaning *intent* and a backward-resolving *decision* are
/// two phases of the same primitive — both are [`Commitment`]s with
/// different [`CommitmentKind`].
///
/// See `docs/INTENT_SYSTEM.md` §1.1 for the full spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commitment {
    pub id: Uuid,
    pub kind: CommitmentKind,
    /// One-line statement: "ship v2 by Friday", "use Postgres for v2".
    pub statement: String,
    pub made_at: DateTime<Utc>,
    /// When we expect resolution. `None` = open-ended.
    pub horizon: Option<DateTime<Utc>>,
    /// Optional pointer into the snapshot store. `None` until a
    /// snapshot is captured (separate path so the snapshot blob doesn't
    /// have to load with the commitment).
    pub context_snapshot_id: Option<Uuid>,
    pub options_considered: Vec<String>,
    /// Which option won. For single-option intents = `statement`.
    pub chosen: String,
    /// User-declared 0..=1.
    pub confidence: f32,
    pub expected_outcome: Option<String>,
    pub stakes: Stakes,
    pub state: State,
    /// Set once an [`Outcome`] is attached. See state machine.
    pub outcome_id: Option<Uuid>,
    /// Chain: this supersedes / refines prior commitments.
    pub derived_from: Vec<Uuid>,
    pub tags: Vec<String>,
    pub source: Source,
}

impl Commitment {
    /// Build a fresh [`State::Open`] commitment with sensible defaults.
    /// Caller fills in horizon / options / tags as needed.
    pub fn new(kind: CommitmentKind, statement: impl Into<String>, source: Source) -> Self {
        let statement = statement.into();
        let chosen = statement.clone();
        Self {
            id: Uuid::new_v4(),
            kind,
            statement,
            made_at: Utc::now(),
            horizon: None,
            context_snapshot_id: None,
            options_considered: Vec::new(),
            chosen,
            confidence: 0.7,
            expected_outcome: None,
            stakes: Stakes::Medium,
            state: State::Open,
            outcome_id: None,
            derived_from: Vec::new(),
            tags: Vec::new(),
            source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentKind {
    /// Forward-leaning, hasn't been acted on yet.
    Intent,
    /// Already chosen / executed.
    Decision,
    /// Tentative belief to be validated ("I think Postgres will scale").
    Hypothesis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Active, no outcome yet.
    Open,
    /// User has acted on it but outcome not assessed.
    Acted,
    /// Outcome attached.
    Completed,
    /// User explicitly walked away.
    Abandoned,
    /// A later Commitment in `derived_from` supersedes this.
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stakes {
    /// Routine; pattern detector ignores unless asked.
    Low,
    /// Default.
    Medium,
    /// Amplified weight in pattern detection + prediction.
    High,
    /// Even if high stakes, the user can undo (drives different
    /// surfacing tone).
    Reversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// CLI / UI form.
    Manual,
    /// ⌘⇧Space + Whisper STT.
    VoiceCapture,
    /// Detected from clipboard/shell/MCP turns by phrase mining.
    ImplicitMined,
    /// Emitted by an LLM via `memory_commit` MCP tool.
    McpStructured,
    /// `tracemind commit` CLI subcommand.
    Cli,
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// What actually happened after a [`Commitment`]. Resolution is
/// optional — a commitment can complete with [`Polarity::NoOutcome`].
/// See `docs/INTENT_SYSTEM.md` §1.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub id: Uuid,
    pub commitment_id: Uuid,
    pub observed_at: DateTime<Utc>,
    pub description: String,
    pub polarity: Polarity,
    /// `||predicted_embed - actual_embed||` — populated by the
    /// world-model surprise scorer in `tm-reflect`. 0.0 until then.
    pub surprise: f32,
    /// Pointers to traces / signals that prove the outcome.
    pub evidence_traces: Vec<Uuid>,
    pub user_note: Option<String>,
    pub source: OutcomeSource,
}

impl Outcome {
    pub fn new(
        commitment_id: Uuid,
        polarity: Polarity,
        description: impl Into<String>,
        source: OutcomeSource,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            commitment_id,
            observed_at: Utc::now(),
            description: description.into(),
            polarity,
            surprise: 0.0,
            evidence_traces: Vec::new(),
            user_note: None,
            source,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// Outcome exceeded expected.
    Better,
    AsExpected,
    Worse,
    /// Some good, some bad.
    Mixed,
    /// Commitment ended without measurable result. *Legitimate* — many
    /// real commitments fizzle; the pattern detector excludes these.
    NoOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSource {
    /// Brief asked "what happened with X?" → user answered.
    UserPrompted,
    /// Detected via embedding similarity to later capture text.
    ImplicitMatched,
    /// Explicit `memory_resolve` MCP call.
    McpStructured,
    /// `tracemind resolve` CLI subcommand.
    Cli,
}

// ---------------------------------------------------------------------------
// Anticipation
// ---------------------------------------------------------------------------

/// The world-model output. First-class so we can score the model's
/// calibration over time. See `docs/INTENT_SYSTEM.md` §1.3.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Anticipation {
    pub id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub trigger: TriggerContext,
    pub kind: AnticipationKind,
    pub predicted_commitment: Option<CommitmentDraft>,
    /// Existing [`Commitment`]s backing this prediction.
    pub grounded_in: Vec<Uuid>,
    pub confidence: f32,
    pub surfaced_at: Option<DateTime<Utc>>,
    pub user_response: Option<UserResponse>,
    /// Did a real Commitment match this prediction?
    pub eventual_match: Option<Uuid>,
    /// Hard expiry; auto-dismiss after this.
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnticipationKind {
    /// L1: silent — pre-warm retrieval.
    PrefetchQuery,
    /// L2: visible — "this looks like a prior pattern".
    PatternMatch,
    /// L3: opt-in — "what would you do?".
    Recommendation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerContext {
    /// Hash of the recent N turns at trigger time.
    pub working_memory_hash: String,
    /// Embedding centroid of current focus (384-dim).
    pub topic_centroid: Vec<f32>,
    pub time_of_day: u8,
    pub source_app: Option<String>,
    /// What we mined, if implicit.
    pub matched_phrase: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserResponse {
    /// User actioned the prediction.
    Accepted,
    /// User closed it (negative training signal).
    Dismissed,
    /// User explicitly liked it (strong positive).
    Starred,
    /// User wants no more like this — Loop 2 hard suppression.
    Silenced,
    /// Expired without response (weak signal).
    Ignored,
}

/// A draft of a predicted [`Commitment`] — what the world model thinks
/// the user is about to commit to. Not stored as a Commitment until /
/// unless the user accepts it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentDraft {
    pub kind: CommitmentKind,
    pub statement: String,
    pub options_considered: Vec<String>,
    pub horizon: Option<DateTime<Utc>>,
    pub stakes: Stakes,
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// ContextSnapshot
// ---------------------------------------------------------------------------

/// Freeze-frame of the user's focus at commitment time. Stored
/// separately from [`Commitment`] because it's heavy and we don't
/// always need it loaded. See `docs/INTENT_SYSTEM.md` §1.4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub working_memory: Vec<TurnRef>,
    /// `(entity_id, score)` at decision time.
    pub top_k_entities: Vec<(Uuid, f32)>,
    /// Recent traces.
    pub top_k_traces: Vec<Uuid>,
    /// 384-dim BGE centroid.
    pub topic_embedding: Vec<f32>,
    pub ambient: AmbientState,
}

/// A pointer to a captured turn. Stored as a ref so the snapshot
/// stays small and we can resolve the full text from the trace store
/// when needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRef {
    pub trace_id: Uuid,
    pub speaker: Option<String>,
    pub when: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientState {
    pub time_of_day: u8,
    pub day_of_week: u8,
    /// Captures per hour, last 4h.
    pub recent_capture_density: f32,
    pub recent_query_density: f32,
    pub source_app: Option<String>,
    // Explicitly NOT collected: location, emotion, biometrics — out of scope.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_new_starts_open() {
        let c = Commitment::new(
            CommitmentKind::Intent,
            "ship v2 by Friday",
            Source::Manual,
        );
        assert_eq!(c.state, State::Open);
        assert_eq!(c.kind, CommitmentKind::Intent);
        assert_eq!(c.chosen, "ship v2 by Friday");
        assert_eq!(c.statement, "ship v2 by Friday");
        assert!(c.outcome_id.is_none());
        assert!(c.derived_from.is_empty());
    }

    #[test]
    fn outcome_new_carries_through_polarity() {
        let cid = Uuid::new_v4();
        let o = Outcome::new(cid, Polarity::Worse, "shipped Tuesday, 4 days late", OutcomeSource::UserPrompted);
        assert_eq!(o.commitment_id, cid);
        assert_eq!(o.polarity, Polarity::Worse);
        assert!(o.evidence_traces.is_empty());
        assert_eq!(o.surprise, 0.0);
    }

    /// Our snake_case serde rename matches the JSON schema in
    /// `INTENT_SYSTEM.md` §8.1 — the MCP tool descriptor is the
    /// contract, so this is a guard against drift.
    #[test]
    fn kind_serde_uses_snake_case() {
        let json = serde_json::to_string(&CommitmentKind::Hypothesis).unwrap();
        assert_eq!(json, "\"hypothesis\"");
        let parsed: CommitmentKind = serde_json::from_str("\"decision\"").unwrap();
        assert_eq!(parsed, CommitmentKind::Decision);
    }

    #[test]
    fn polarity_serde_uses_snake_case() {
        // `as_expected` is the only multi-word case; this catches any
        // accidental switch to `AsExpected` etc.
        let json = serde_json::to_string(&Polarity::AsExpected).unwrap();
        assert_eq!(json, "\"as_expected\"");
        let parsed: Polarity = serde_json::from_str("\"no_outcome\"").unwrap();
        assert_eq!(parsed, Polarity::NoOutcome);
    }
}
