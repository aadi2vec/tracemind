use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A named entity in the knowledge graph (person, concept, project, file, URL…)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: EntityType,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity {
    pub fn new(name: impl Into<String>, entity_type: EntityType, confidence: f64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            entity_type,
            confidence: confidence.clamp(0.0, 1.0),
            source_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn decay(&mut self, factor: f64) {
        self.confidence = (self.confidence * factor).max(0.0);
        self.updated_at = Utc::now();
    }

    pub fn reinforce(&mut self, amount: f64) {
        self.confidence = (self.confidence + amount).min(1.0);
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Organization,
    Project,
    File,
    Url,
    Concept,
    Technology,
    Decision,
    Event,
    /// LM-5c — first-class daily-note memory. One per local date,
    /// auto-named `YYYY-MM-DD`, populated by `tracemind today`.
    /// Existence is idempotent: re-running `today` upserts.
    DailyNote,
    Custom(String),
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntityType::Custom(s) => write!(f, "{}", s),
            other => write!(f, "{:?}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_confidence_clamps() {
        let e = Entity::new("test", EntityType::Concept, 1.5);
        assert_eq!(e.confidence, 1.0);
        let e = Entity::new("test", EntityType::Concept, -0.1);
        assert_eq!(e.confidence, 0.0);
    }

    #[test]
    fn entity_decay_works() {
        let mut e = Entity::new("test", EntityType::Concept, 1.0);
        e.decay(0.9);
        assert!((e.confidence - 0.9).abs() < 1e-10);
    }
}
