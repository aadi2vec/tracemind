//! LoCoMo dataset schema and JSON loader.
//!
//! Matches the public LoCoMo schema (snap-research/locomo): a list of samples,
//! each with a multi-session conversation and a set of QA pairs. We parse
//! leniently — extra fields are ignored, and malformed samples are reported
//! rather than crashing the whole load.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// The five standard LoCoMo question categories. Serialized as the integer
/// codes used in the public dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    SingleHop,
    MultiHop,
    Temporal,
    OpenDomain,
    Adversarial,
}

impl Category {
    /// Canonical code from the public dataset (1..5). Adversarial is 5 per
    /// the LoCoMo paper convention.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::SingleHop),
            2 => Some(Self::MultiHop),
            3 => Some(Self::Temporal),
            4 => Some(Self::OpenDomain),
            5 => Some(Self::Adversarial),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleHop => "single_hop",
            Self::MultiHop => "multi_hop",
            Self::Temporal => "temporal",
            Self::OpenDomain => "open_domain",
            Self::Adversarial => "adversarial",
        }
    }
}

/// A single turn in a conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: String,
    pub text: String,
    /// Optional timestamp in the dataset's original format; opaque to the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// One session of a multi-session conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocomoSession {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub turns: Vec<Turn>,
}

/// A single QA pair to evaluate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocomoQuestion {
    pub id: String,
    pub question: String,
    /// One or more acceptable answers. The scorer takes the best score across
    /// these as the per-question score.
    pub answers: Vec<String>,
    pub category: Category,
    /// Optional evidence turn indices used by some LoCoMo variants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// One sample (= one multi-session conversation + its QA set).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocomoSample {
    pub sample_id: String,
    pub sessions: Vec<LocomoSession>,
    pub questions: Vec<LocomoQuestion>,
}

/// Top-level dataset — a collection of samples.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocomoDataset {
    pub samples: Vec<LocomoSample>,
}

impl LocomoDataset {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, LocomoError> {
        let raw = std::fs::read_to_string(path.as_ref()).map_err(LocomoError::Io)?;
        Self::load_from_str(&raw)
    }

    /// Parse from a JSON string. Accepts either:
    /// - a top-level object: `{"samples": [...]}`
    /// - or a top-level array of samples
    pub fn load_from_str(raw: &str) -> Result<Self, LocomoError> {
        // First, try the wrapped form.
        if let Ok(ds) = serde_json::from_str::<LocomoDataset>(raw) {
            return Ok(ds);
        }
        // Fall back to bare-array form.
        let samples: Vec<LocomoSample> =
            serde_json::from_str(raw).map_err(LocomoError::Parse)?;
        Ok(Self { samples })
    }

    /// Total number of questions across all samples.
    pub fn total_questions(&self) -> usize {
        self.samples.iter().map(|s| s.questions.len()).sum()
    }

    /// Total number of conversation turns across all samples.
    pub fn total_turns(&self) -> usize {
        self.samples
            .iter()
            .flat_map(|s| s.sessions.iter())
            .map(|sess| sess.turns.len())
            .sum()
    }
}

#[derive(Debug, Error)]
pub enum LocomoError {
    #[error("io: {0}")]
    Io(#[source] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("scoring: {0}")]
    Scoring(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_code_roundtrip() {
        for code in 1..=5u8 {
            assert!(Category::from_code(code).is_some());
        }
        assert!(Category::from_code(0).is_none());
        assert!(Category::from_code(6).is_none());
    }

    #[test]
    fn loads_wrapped_object_form() {
        let raw = r#"{
          "samples": [{
            "sample_id": "s1",
            "sessions": [{
              "session_id": "sess1",
              "turns": [{"speaker": "A", "text": "hi"}]
            }],
            "questions": [{
              "id": "q1",
              "question": "who said hi?",
              "answers": ["A"],
              "category": "single_hop"
            }]
          }]
        }"#;
        let ds = LocomoDataset::load_from_str(raw).unwrap();
        assert_eq!(ds.samples.len(), 1);
        assert_eq!(ds.total_questions(), 1);
        assert_eq!(ds.total_turns(), 1);
        assert_eq!(ds.samples[0].questions[0].category, Category::SingleHop);
    }

    #[test]
    fn loads_bare_array_form() {
        let raw = r#"[{
          "sample_id": "s1",
          "sessions": [],
          "questions": []
        }]"#;
        let ds = LocomoDataset::load_from_str(raw).unwrap();
        assert_eq!(ds.samples.len(), 1);
        assert_eq!(ds.total_questions(), 0);
    }

    #[test]
    fn rejects_malformed_json() {
        let err = LocomoDataset::load_from_str("{not json").unwrap_err();
        assert!(matches!(err, LocomoError::Parse(_)));
    }

    #[test]
    fn ignores_unknown_fields() {
        let raw = r#"{
          "samples": [{
            "sample_id": "s1",
            "sessions": [],
            "questions": [],
            "extra_field": "ignored"
          }],
          "dataset_version": "v1"
        }"#;
        let ds = LocomoDataset::load_from_str(raw).unwrap();
        assert_eq!(ds.samples.len(), 1);
    }
}
