//! Core TMS data types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Belief status ──────────────────────────────────────────────────

/// Whether a belief is currently held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BeliefStatus {
    /// Believed — at least one supporting justification with all
    /// premises `In` and no active defeating justification.
    In,
    /// Not currently believed (no active support).
    Out,
    /// Explicitly contradicted by another belief.
    Contradicted,
}

// ── Belief ─────────────────────────────────────────────────────────

/// A single assertion tracked by the TMS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Belief {
    pub id: Uuid,
    pub statement: String,
    pub status: BeliefStatus,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ── Justification ──────────────────────────────────────────────────

/// The role a justification plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JustificationType {
    /// Premises *support* the conclusion — if all premises are `In`
    /// the conclusion should be `In`.
    Support,
    /// Premises *defeat* the conclusion — if all premises are `In`
    /// the conclusion should be `Out`.
    Defeat,
}

/// A directed dependency: *premises* jointly justify (or defeat) a
/// *conclusion*.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Justification {
    pub id: Uuid,
    /// The belief being justified.
    pub conclusion: Uuid,
    /// Beliefs that collectively form this justification.
    pub premises: Vec<Uuid>,
    pub justification_type: JustificationType,
    pub created_at: DateTime<Utc>,
}

// ── Contradiction ──────────────────────────────────────────────────

/// How a contradiction was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContradictionResolution {
    /// Retract belief A.
    RetractA,
    /// Retract belief B.
    RetractB,
    /// Retract both beliefs.
    RetractBoth,
    /// A human (or upstream agent) chose a specific belief to keep.
    UserOverride(Uuid),
}

/// Two beliefs that semantically contradict each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    pub id: Uuid,
    pub belief_a: Uuid,
    pub belief_b: Uuid,
    pub detected_at: DateTime<Utc>,
    pub resolution: Option<ContradictionResolution>,
    /// Cosine similarity between the two belief embeddings (negative
    /// values indicate opposition).
    pub cosine_similarity: f32,
}

// ── Propagation result ─────────────────────────────────────────────

/// Summary of what changed after a propagation pass.
#[derive(Debug, Clone, Default)]
pub struct PropagationResult {
    /// `(belief_id, new_status)` for every belief whose status changed.
    pub changed: Vec<(Uuid, BeliefStatus)>,
    /// Any new contradictions detected during the pass.
    pub contradictions: Vec<Contradiction>,
}
