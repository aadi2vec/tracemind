use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A typed relationship between two entities (subject → predicate → object).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Triple {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate: Predicate,
    pub object_id: Uuid,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Triple {
    pub fn new(
        subject_id: Uuid,
        predicate: Predicate,
        object_id: Uuid,
        confidence: f64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            subject_id,
            predicate,
            object_id,
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
pub enum Predicate {
    // Generic
    RelatedTo,
    IsA,
    PartOf,
    HasProperty,
    // People / org
    WorksAt,
    CollaboratesWith,
    Owns,
    // Projects / files
    DependsOn,
    Produces,
    References,
    // Procedural
    HasProcedure,
    // Custom
    Custom(String),
}

impl std::fmt::Display for Predicate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Predicate::Custom(s) => write!(f, "{}", s),
            other => write!(f, "{:?}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_confidence_clamps() {
        let t = Triple::new(Uuid::new_v4(), Predicate::RelatedTo, Uuid::new_v4(), 1.5);
        assert_eq!(t.confidence, 1.0);
    }

    #[test]
    fn triple_display() {
        assert_eq!(Predicate::Custom("foo".into()).to_string(), "foo");
        assert_eq!(Predicate::RelatedTo.to_string(), "RelatedTo");
    }
}
