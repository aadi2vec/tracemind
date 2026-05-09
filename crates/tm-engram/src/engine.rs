use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

use tm_intent::{Action, ActionSource, IntentStore, Need, NeedSource};
use tm_temporal::TemporalStore;
use tm_tms::{BeliefStatus, Contradiction, PropagationResult, TmsEngine};

use crate::types::{
    AssertResult, BeliefKind, BeliefView, EngineConfig, Goal, GoalStatus, RetractResult,
    WorldSnapshot,
};

// ── Errors ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum EngramError {
    #[error("intent store: {0}")]
    Intent(#[from] tm_intent::store::StoreError),
    #[error("temporal store: {0}")]
    Temporal(#[from] tm_temporal::store::StoreError),
    #[error("tms: {0}")]
    Tms(#[from] tm_tms::engine::TmsError),
    #[error("belief not found: {0}")]
    BeliefNotFound(Uuid),
}

pub type Result<T> = std::result::Result<T, EngramError>;

// ── Engine ────────────────────────────────────────────────────────

pub struct Engram {
    intent_store: IntentStore,
    tms: TmsEngine,
    temporal_store: TemporalStore,
}

impl Engram {
    pub fn open(config: EngineConfig) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).ok();
        let intent_store = IntentStore::open(config.data_dir.join("intents.db"))?;
        let temporal_store = TemporalStore::open(config.data_dir.join("temporal.db"))?;
        let tms = TmsEngine::new();
        Ok(Self {
            intent_store,
            tms,
            temporal_store,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let intent_store = IntentStore::open_in_memory()?;
        let temporal_store = TemporalStore::open_in_memory()?;
        let tms = TmsEngine::new();
        Ok(Self {
            intent_store,
            tms,
            temporal_store,
        })
    }

    pub fn assert_belief(&mut self, statement: &str, confidence: f32) -> Result<AssertResult> {
        let belief_id = self.tms.assert_belief(statement, confidence);
        let status = self
            .tms
            .get_status(belief_id)
            .unwrap_or(BeliefStatus::In);

        // Record as a temporal fact.
        let now = Utc::now();
        let fact = serde_json::json!({
            "statement": statement,
            "confidence": confidence,
        });
        self.temporal_store.insert_fact(
            belief_id,
            "belief",
            &fact.to_string(),
            now,
            None,
        )?;

        Ok(AssertResult {
            belief_id,
            status,
            propagation: PropagationResult::default(),
        })
    }

    pub fn retract(&mut self, belief_id: Uuid) -> Result<RetractResult> {
        let propagation = self.tms.retract_belief(belief_id);

        let now = Utc::now();
        let fact = serde_json::json!({ "retracted": true });
        self.temporal_store.insert_fact(
            belief_id,
            "belief_retraction",
            &fact.to_string(),
            now,
            None,
        )?;

        Ok(RetractResult {
            belief_id,
            propagation,
        })
    }

    pub fn world_at(&self, as_of: DateTime<Utc>) -> Result<WorldSnapshot> {
        let beliefs: Vec<BeliefView> = self
            .tms
            .active_beliefs()
            .into_iter()
            .filter(|b| b.created_at <= as_of)
            .map(|b| BeliefView {
                id: b.id,
                statement: b.statement.clone(),
                confidence: b.confidence,
                status: b.status,
                created_at: b.created_at,
                kind: BeliefKind::Fact,
            })
            .collect();

        let contradictions = self.tms.contradictions().to_vec();

        Ok(WorldSnapshot {
            beliefs,
            contradictions,
            as_of,
        })
    }

    pub fn contradictions(&self) -> Vec<Contradiction> {
        self.tms.contradictions().to_vec()
    }

    pub fn history_of(&self, entity_id: Uuid) -> Result<Vec<BeliefView>> {
        let facts = self.temporal_store.history(entity_id, "belief")?;
        let mut views = Vec::new();
        for f in facts {
            let statement = f
                .fact
                .get("statement")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let confidence = f
                .fact
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            views.push(BeliefView {
                id: f.fact_id,
                statement,
                confidence,
                status: BeliefStatus::In,
                created_at: f.valid_time.from,
                kind: BeliefKind::Fact,
            });
        }
        Ok(views)
    }

    pub fn set_goal(&mut self, need_text: &str) -> Result<Goal> {
        let need = Need::new(need_text, NeedSource::Explicit);
        let goal = Goal {
            id: Uuid::new_v4(),
            need_id: need.id,
            description: need_text.to_string(),
            status: GoalStatus::Active,
            created_at: Utc::now(),
        };
        self.intent_store.insert_need(&need)?;

        // Also assert as a belief so the TMS tracks it.
        self.tms.assert_belief(need_text, 0.7);

        Ok(goal)
    }

    pub fn record_action(
        &mut self,
        description: &str,
        commitment_id: Option<Uuid>,
    ) -> Result<Action> {
        let mut action = Action::new(description, ActionSource::UserReported);
        action.commitment_id = commitment_id;
        self.intent_store.insert_action(&action)?;
        Ok(action)
    }

    pub fn observe(&mut self, text: &str) -> Result<Uuid> {
        let need = Need::new(text, NeedSource::Mined);
        let id = need.id;
        self.intent_store.insert_need(&need)?;
        Ok(id)
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_and_retract_round_trip() {
        let mut eng = Engram::open_in_memory().unwrap();
        let r = eng.assert_belief("Rust is fast", 0.95).unwrap();
        assert_eq!(r.status, BeliefStatus::In);

        let r2 = eng.retract(r.belief_id).unwrap();
        assert_eq!(r2.belief_id, r.belief_id);

        let status = eng.tms.get_status(r.belief_id).unwrap();
        assert_eq!(status, BeliefStatus::Out);
    }

    #[test]
    fn set_goal_creates_need() {
        let mut eng = Engram::open_in_memory().unwrap();
        let goal = eng.set_goal("learn Rust").unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.description, "learn Rust");

        let need = eng.intent_store.get_need(goal.need_id).unwrap();
        assert!(need.is_some());
        assert_eq!(need.unwrap().statement, "learn Rust");
    }

    #[test]
    fn record_action_persists() {
        let mut eng = Engram::open_in_memory().unwrap();
        let action = eng.record_action("pushed the fix", None).unwrap();
        assert_eq!(action.description, "pushed the fix");
        assert!(action.commitment_id.is_none());

        let loaded = eng.intent_store.get_action(action.id).unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().description, "pushed the fix");
    }

    #[test]
    fn world_snapshot_contains_asserted_beliefs() {
        let mut eng = Engram::open_in_memory().unwrap();
        eng.assert_belief("earth is round", 0.99).unwrap();
        eng.assert_belief("water is wet", 0.95).unwrap();

        let snap = eng.world_at(Utc::now()).unwrap();
        assert_eq!(snap.beliefs.len(), 2);
        let stmts: Vec<&str> = snap.beliefs.iter().map(|b| b.statement.as_str()).collect();
        assert!(stmts.contains(&"earth is round"));
        assert!(stmts.contains(&"water is wet"));
    }
}
