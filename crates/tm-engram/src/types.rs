use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use tm_tms::{BeliefStatus, Contradiction, PropagationResult};

// ── Config ────────────────────────────────────────────────────────

pub struct EngineConfig {
    pub data_dir: PathBuf,
    pub enable_tms: bool,
    pub enable_temporal: bool,
}

// ── Results ───────────────────────────────────────────────────────

pub struct AssertResult {
    pub belief_id: Uuid,
    pub status: BeliefStatus,
    pub propagation: PropagationResult,
}

pub struct RetractResult {
    pub belief_id: Uuid,
    pub propagation: PropagationResult,
}

// ── World snapshot ────────────────────────────────────────────────

pub struct WorldSnapshot {
    pub beliefs: Vec<BeliefView>,
    pub contradictions: Vec<Contradiction>,
    pub as_of: DateTime<Utc>,
}

pub struct BeliefView {
    pub id: Uuid,
    pub statement: String,
    pub confidence: f32,
    pub status: BeliefStatus,
    pub created_at: DateTime<Utc>,
    pub kind: BeliefKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefKind {
    Commitment,
    Need,
    Sentiment,
    Action,
    Fact,
}

// ── Goals ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Achieved,
    Abandoned,
}

pub struct Goal {
    pub id: Uuid,
    pub need_id: Uuid,
    pub description: String,
    pub status: GoalStatus,
    pub created_at: DateTime<Utc>,
}
