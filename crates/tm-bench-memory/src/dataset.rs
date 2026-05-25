//! Persistence-pair schema + JSON loader.
//!
//! The schema is intentionally simpler than LoCoMo: each pair is a
//! single (store, query, answers) triple plus a category tag. No multi-
//! session structure — the *persistence* of the store across a fresh
//! process is the whole point of the benchmark.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// The five persistence categories. Different categories stress
/// different parts of the stack:
///
/// - `FactualRecall` exercises plain vector + entity recall.
/// - `NameResolution` exercises co-reference + entity merging.
/// - `DecisionHistory` exercises the commitment / event-graph layer.
/// - `ContradictionRecovery` exercises TMS + retraction.
/// - `TemporalPinning` exercises date extractors + temporal queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    FactualRecall,
    NameResolution,
    DecisionHistory,
    ContradictionRecovery,
    TemporalPinning,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FactualRecall => "factual_recall",
            Self::NameResolution => "name_resolution",
            Self::DecisionHistory => "decision_history",
            Self::ContradictionRecovery => "contradiction_recovery",
            Self::TemporalPinning => "temporal_pinning",
        }
    }
}

/// One persistence pair.
///
/// `store` is the list of sentences ingested in Session A (closing the
/// store between sentences is not required — within-session ordering is
/// part of what we ingest). `query` is what we ask in Session B.
/// `answers` is the list of acceptable reference answers; the scorer
/// takes the best F1 / EM across the list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistencePair {
    pub id: String,
    pub category: Category,
    pub store: Vec<String>,
    pub query: String,
    pub answers: Vec<String>,
    /// Optional free-form note for fixture authors (e.g. why the pair
    /// is adversarial, what failure mode it exercises). Ignored by the
    /// scorer; preserved in the JSON for debuggability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistenceDataset {
    pub pairs: Vec<PersistencePair>,
}

impl PersistenceDataset {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let raw = std::fs::read_to_string(path.as_ref()).map_err(PersistenceError::Io)?;
        Self::load_from_str(&raw)
    }

    pub fn load_from_str(raw: &str) -> Result<Self, PersistenceError> {
        // Accept either `{"pairs": [...]}` or a bare array.
        if let Ok(ds) = serde_json::from_str::<PersistenceDataset>(raw) {
            return Ok(ds);
        }
        let pairs: Vec<PersistencePair> =
            serde_json::from_str(raw).map_err(PersistenceError::Parse)?;
        Ok(Self { pairs })
    }
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("io: {0}")]
    Io(#[source] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("runner: {0}")]
    Runner(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_wrapped() {
        let raw = r#"{
          "pairs": [{
            "id": "p1",
            "category": "factual_recall",
            "store": ["X is Y."],
            "query": "What is X?",
            "answers": ["Y"]
          }]
        }"#;
        let ds = PersistenceDataset::load_from_str(raw).unwrap();
        assert_eq!(ds.pairs.len(), 1);
        assert_eq!(ds.pairs[0].category, Category::FactualRecall);
    }

    #[test]
    fn loads_bare_array() {
        let raw = r#"[{
          "id": "p1",
          "category": "temporal_pinning",
          "store": ["My flight is May 3."],
          "query": "When is my flight?",
          "answers": ["May 3"]
        }]"#;
        let ds = PersistenceDataset::load_from_str(raw).unwrap();
        assert_eq!(ds.pairs.len(), 1);
        assert_eq!(ds.pairs[0].category, Category::TemporalPinning);
    }

    #[test]
    fn rejects_malformed() {
        assert!(matches!(
            PersistenceDataset::load_from_str("not json").unwrap_err(),
            PersistenceError::Parse(_)
        ));
    }

    #[test]
    fn category_strings() {
        assert_eq!(Category::FactualRecall.as_str(), "factual_recall");
        assert_eq!(Category::ContradictionRecovery.as_str(), "contradiction_recovery");
    }
}
