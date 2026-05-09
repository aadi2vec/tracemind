//! Co-occurrence edge detector — links nodes that appeared close in time.

use chrono::Utc;
use uuid::Uuid;

use crate::types::{CrossModalEdge, CrossModalEdgeType, ModalNode};

/// Detects temporal co-occurrence between [`ModalNode`]s.
///
/// Any two nodes whose `created_at` timestamps differ by at most `window_secs`
/// will be linked with a [`CrossModalEdgeType::CoOccurrence`] edge. Confidence
/// is inversely proportional to the time gap (closer = higher confidence).
pub struct CoOccurrenceDetector {
    /// Maximum gap (in seconds) for two nodes to be considered co-occurring.
    pub window_secs: u64,
}

impl CoOccurrenceDetector {
    pub fn new(window_secs: u64) -> Self {
        Self { window_secs }
    }

    /// Scan all node pairs and emit co-occurrence edges for those within the window.
    pub fn detect(&self, nodes: &[ModalNode]) -> Vec<CrossModalEdge> {
        let mut edges = Vec::new();

        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                let a = &nodes[i];
                let b = &nodes[j];
                let gap = (a.created_at - b.created_at)
                    .num_seconds()
                    .unsigned_abs();

                if gap <= self.window_secs {
                    let confidence = if self.window_secs == 0 {
                        1.0
                    } else {
                        1.0 - (gap as f32 / self.window_secs as f32)
                    };

                    edges.push(CrossModalEdge {
                        id: Uuid::new_v4(),
                        source: a.id,
                        target: b.id,
                        source_modality: a.modality,
                        target_modality: b.modality,
                        edge_type: CrossModalEdgeType::CoOccurrence,
                        confidence,
                        created_at: Utc::now(),
                    });
                }
            }
        }

        edges
    }
}

impl Default for CoOccurrenceDetector {
    fn default() -> Self {
        Self::new(30)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ModalMetadata, Modality};
    use chrono::Duration;

    fn make_node(modality: Modality, offset_secs: i64) -> ModalNode {
        ModalNode {
            id: Uuid::new_v4(),
            modality,
            source_uri: None,
            content_hash: 0,
            embedding: None,
            projected_embedding: None,
            metadata: ModalMetadata::TextMeta {
                word_count: 10,
                language: None,
            },
            created_at: Utc::now() + Duration::seconds(offset_secs),
        }
    }

    #[test]
    fn nodes_within_window_create_edge() {
        let detector = CoOccurrenceDetector::new(30);
        let a = make_node(Modality::Text, 0);
        let b = make_node(Modality::Image, 10);

        let edges = detector.detect(&[a.clone(), b.clone()]);
        assert_eq!(edges.len(), 1);

        let e = &edges[0];
        assert_eq!(e.source, a.id);
        assert_eq!(e.target, b.id);
        assert_eq!(e.edge_type, CrossModalEdgeType::CoOccurrence);
        assert_eq!(e.source_modality, Modality::Text);
        assert_eq!(e.target_modality, Modality::Image);
        // 10s gap in a 30s window → confidence ~0.67
        assert!(e.confidence > 0.6 && e.confidence < 0.75);
    }

    #[test]
    fn nodes_outside_window_no_edge() {
        let detector = CoOccurrenceDetector::new(30);
        let a = make_node(Modality::Text, 0);
        let b = make_node(Modality::Audio, 60);

        let edges = detector.detect(&[a, b]);
        assert!(edges.is_empty());
    }

    #[test]
    fn simultaneous_nodes_max_confidence() {
        let detector = CoOccurrenceDetector::new(30);
        let now = Utc::now();
        let a = ModalNode {
            id: Uuid::new_v4(),
            modality: Modality::Code,
            source_uri: None,
            content_hash: 1,
            embedding: None,
            projected_embedding: None,
            metadata: ModalMetadata::CodeMeta {
                language: "rust".into(),
                ast_hash: None,
                lines: 50,
            },
            created_at: now,
        };
        let b = ModalNode {
            id: Uuid::new_v4(),
            modality: Modality::Text,
            source_uri: None,
            content_hash: 2,
            embedding: None,
            projected_embedding: None,
            metadata: ModalMetadata::TextMeta {
                word_count: 5,
                language: None,
            },
            created_at: now,
        };

        let edges = detector.detect(&[a, b]);
        assert_eq!(edges.len(), 1);
        assert!((edges[0].confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn default_window_is_30_seconds() {
        let d = CoOccurrenceDetector::default();
        assert_eq!(d.window_secs, 30);
    }

    #[test]
    fn multiple_nodes_pairwise() {
        let detector = CoOccurrenceDetector::new(30);
        let a = make_node(Modality::Text, 0);
        let b = make_node(Modality::Image, 5);
        let c = make_node(Modality::Audio, 10);

        let edges = detector.detect(&[a, b, c]);
        // All three pairs are within 30s → 3 edges
        assert_eq!(edges.len(), 3);
    }
}
