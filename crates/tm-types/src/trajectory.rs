use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// RL training record: captures context, decision, outcome, and JEPA/WM surprise signal.
/// Schema is stable from day-1 so Phase-3 model training requires no migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trajectory {
    pub id: Uuid,
    pub session_id: Uuid,
    pub trace_id: Uuid,

    // Bandit decision
    pub arm_chosen: u8,
    pub reward: f64,
    pub n_results: u32,
    pub latency_ms: u32,

    // Embeddings (384-dim all-MiniLM-L6-v2). Stored as flat Vec<f32>.
    pub context_embedding: Vec<f32>,
    pub memory_snapshot_hash: String,

    // JEPA / World Model training fields (Phase 3). None until models are trained.
    pub predicted_outcome: Option<Vec<f32>>,
    pub actual_outcome_embedding: Option<Vec<f32>>,
    pub surprise_score: Option<f32>,

    pub created_at: DateTime<Utc>,
}

impl Trajectory {
    pub fn new(
        session_id: Uuid,
        trace_id: Uuid,
        arm_chosen: u8,
        reward: f64,
        n_results: u32,
        latency_ms: u32,
        context_embedding: Vec<f32>,
        memory_snapshot_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            session_id,
            trace_id,
            arm_chosen,
            reward,
            n_results,
            latency_ms,
            context_embedding,
            memory_snapshot_hash: memory_snapshot_hash.into(),
            predicted_outcome: None,
            actual_outcome_embedding: None,
            surprise_score: None,
            created_at: Utc::now(),
        }
    }

    /// Returns true when this trajectory has the data needed for JEPA/WM training.
    pub fn is_training_ready(&self) -> bool {
        self.actual_outcome_embedding.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trajectory_not_training_ready_by_default() {
        let t = Trajectory::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            0,
            1.0,
            5,
            42,
            vec![0.0; 384],
            "abc123",
        );
        assert!(!t.is_training_ready());
    }
}
