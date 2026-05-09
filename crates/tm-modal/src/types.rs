//! Core cross-modal types: nodes, edges, metadata, and search results.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The modality of a piece of content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Code,
    StructuredData,
}

/// Modality-specific metadata attached to a [`ModalNode`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModalMetadata {
    TextMeta {
        word_count: usize,
        language: Option<String>,
    },
    ImageMeta {
        width: u32,
        height: u32,
        format: String,
    },
    AudioMeta {
        duration_secs: f32,
        sample_rate: u32,
    },
    CodeMeta {
        language: String,
        ast_hash: Option<u64>,
        lines: usize,
    },
    StructuredMeta {
        schema: Option<String>,
        row_count: Option<usize>,
    },
}

/// A node in the cross-modal graph, representing one piece of content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalNode {
    pub id: Uuid,
    pub modality: Modality,
    pub source_uri: Option<String>,
    pub content_hash: u64,
    pub embedding: Option<Vec<f32>>,
    pub projected_embedding: Option<Vec<f32>>,
    pub metadata: ModalMetadata,
    pub created_at: DateTime<Utc>,
}

/// The type of relationship between two cross-modal nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossModalEdgeType {
    /// Temporal proximity — nodes appeared close in time.
    CoOccurrence,
    /// Explicit mention — one node references the other.
    Reference,
    /// One was derived from the other (e.g. transcript from audio).
    Derivation,
    /// Embedding proximity in the shared projected space.
    Similarity,
}

/// An edge linking two [`ModalNode`]s across (or within) modalities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossModalEdge {
    pub id: Uuid,
    pub source: Uuid,
    pub target: Uuid,
    pub source_modality: Modality,
    pub target_modality: Modality,
    pub edge_type: CrossModalEdgeType,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
}

/// A search result returned when querying the cross-modal graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalSearchResult {
    pub node: ModalNode,
    pub score: f32,
    pub matched_modality: Modality,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modality_round_trip_serde() {
        let m = Modality::StructuredData;
        let json = serde_json::to_string(&m).unwrap();
        let back: Modality = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn modal_node_round_trip_serde() {
        let node = ModalNode {
            id: Uuid::new_v4(),
            modality: Modality::Text,
            source_uri: Some("file:///notes.md".into()),
            content_hash: 0xDEAD_BEEF,
            embedding: Some(vec![0.1, 0.2, 0.3]),
            projected_embedding: None,
            metadata: ModalMetadata::TextMeta {
                word_count: 42,
                language: Some("en".into()),
            },
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: ModalNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, node.id);
        assert_eq!(back.modality, Modality::Text);
    }

    #[test]
    fn cross_modal_edge_round_trip_serde() {
        let edge = CrossModalEdge {
            id: Uuid::new_v4(),
            source: Uuid::new_v4(),
            target: Uuid::new_v4(),
            source_modality: Modality::Audio,
            target_modality: Modality::Text,
            edge_type: CrossModalEdgeType::Derivation,
            confidence: 0.95,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let back: CrossModalEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(back.edge_type, CrossModalEdgeType::Derivation);
        assert_eq!(back.source, edge.source);
    }

    #[test]
    fn modal_metadata_variants() {
        let metas = vec![
            ModalMetadata::ImageMeta {
                width: 1920,
                height: 1080,
                format: "png".into(),
            },
            ModalMetadata::AudioMeta {
                duration_secs: 3.5,
                sample_rate: 44100,
            },
            ModalMetadata::CodeMeta {
                language: "rust".into(),
                ast_hash: Some(123456),
                lines: 100,
            },
            ModalMetadata::StructuredMeta {
                schema: Some("users".into()),
                row_count: Some(500),
            },
        ];
        for meta in metas {
            let json = serde_json::to_string(&meta).unwrap();
            let _back: ModalMetadata = serde_json::from_str(&json).unwrap();
        }
    }
}
