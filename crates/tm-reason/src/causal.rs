//! Causal tracing: track which entities and triples contributed to a retrieval
//! result, enabling "why did you say that?" explanations.
//!
//! A CausalTrace records the full attribution chain:
//! - Which vector matches found which entities
//! - Which graph traversals expanded the result set
//! - Which triples connected the entities
//! - The bandit arm that was selected and why

use serde::Serialize;
use uuid::Uuid;

/// A single attribution — one piece of evidence that contributed to the result.
#[derive(Debug, Clone, Serialize)]
pub struct Attribution {
    pub entity_id: Uuid,
    pub entity_name: String,
    pub source: AttributionSource,
    pub weight: f64,
}

/// How this attribution was discovered.
#[derive(Debug, Clone, Serialize)]
pub enum AttributionSource {
    /// Found via vector similarity search
    VectorMatch { similarity: f64, rank: usize },
    /// Found via graph BFS hop from another entity
    GraphHop { from_entity: Uuid, predicate: String, hop: usize },
    /// Found via episodic trace scan
    EpisodicTrace { trace_id: String },
    /// Found via reasoning chain
    ReasoningChain { chain_score: f64, path_length: usize },
}

/// Full causal trace for a query result.
#[derive(Debug, Clone, Serialize)]
pub struct CausalTrace {
    pub query_id: Uuid,
    pub query_text: String,
    pub arm_index: usize,
    pub arm_name: String,
    pub attributions: Vec<Attribution>,
    pub total_entities: usize,
    pub total_triples: usize,
    pub latency_ms: u64,
}

impl CausalTrace {
    pub fn new(query_text: &str, arm_index: usize, arm_name: &str) -> Self {
        Self {
            query_id: Uuid::new_v4(),
            query_text: query_text.to_string(),
            arm_index,
            arm_name: arm_name.to_string(),
            attributions: Vec::new(),
            total_entities: 0,
            total_triples: 0,
            latency_ms: 0,
        }
    }

    pub fn add_vector_match(&mut self, entity_id: Uuid, entity_name: &str, similarity: f64, rank: usize) {
        self.attributions.push(Attribution {
            entity_id,
            entity_name: entity_name.to_string(),
            source: AttributionSource::VectorMatch { similarity, rank },
            weight: similarity,
        });
    }

    pub fn add_graph_hop(&mut self, entity_id: Uuid, entity_name: &str, from: Uuid, predicate: &str, hop: usize) {
        let weight = 1.0 / (1.0 + hop as f64); // decay by hop distance
        self.attributions.push(Attribution {
            entity_id,
            entity_name: entity_name.to_string(),
            source: AttributionSource::GraphHop {
                from_entity: from,
                predicate: predicate.to_string(),
                hop,
            },
            weight,
        });
    }

    pub fn add_episodic(&mut self, entity_id: Uuid, entity_name: &str, trace_id: &str) {
        self.attributions.push(Attribution {
            entity_id,
            entity_name: entity_name.to_string(),
            source: AttributionSource::EpisodicTrace {
                trace_id: trace_id.to_string(),
            },
            weight: 0.5,
        });
    }

    pub fn add_reasoning(&mut self, entity_id: Uuid, entity_name: &str, chain_score: f64, path_length: usize) {
        self.attributions.push(Attribution {
            entity_id,
            entity_name: entity_name.to_string(),
            source: AttributionSource::ReasoningChain { chain_score, path_length },
            weight: chain_score,
        });
    }

    /// Get top-N most influential attributions.
    pub fn top_attributions(&self, n: usize) -> Vec<&Attribution> {
        let mut sorted: Vec<&Attribution> = self.attributions.iter().collect();
        sorted.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(n);
        sorted
    }

    /// Generate a human-readable explanation.
    pub fn explain(&self) -> String {
        if self.attributions.is_empty() {
            return "No attributions recorded.".to_string();
        }

        let top = self.top_attributions(5);
        let mut lines = Vec::new();
        lines.push(format!("Query: \"{}\"", self.query_text));
        lines.push(format!("Strategy: {} (arm {})", self.arm_name, self.arm_index));
        lines.push(format!("Found {} entities via {} evidence paths:", self.total_entities, self.attributions.len()));

        for attr in top {
            let source_desc = match &attr.source {
                AttributionSource::VectorMatch { similarity, rank } =>
                    format!("vector match (rank #{}, similarity {:.2})", rank + 1, similarity),
                AttributionSource::GraphHop { predicate, hop, .. } =>
                    format!("graph hop #{} via {}", hop, predicate),
                AttributionSource::EpisodicTrace { trace_id } =>
                    format!("episodic trace {}", &trace_id[..8]),
                AttributionSource::ReasoningChain { chain_score, path_length } =>
                    format!("reasoning chain (length {}, score {:.2})", path_length, chain_score),
            };
            lines.push(format!("  - {} ({}): {}", attr.entity_name, attr.weight_pct(), source_desc));
        }

        lines.join("\n")
    }
}

impl Attribution {
    pub fn weight_pct(&self) -> String {
        format!("{:.0}%", self.weight * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_causal_trace_explain() {
        let mut trace = CausalTrace::new("What is Rust used for?", 1, "medium");
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();

        trace.add_vector_match(e1, "Rust", 0.92, 0);
        trace.add_graph_hop(e2, "TraceMind", e1, "Uses", 1);
        trace.total_entities = 2;
        trace.total_triples = 1;

        let explanation = trace.explain();
        assert!(explanation.contains("Rust"));
        assert!(explanation.contains("vector match"));
        assert!(explanation.contains("graph hop"));
    }

    #[test]
    fn test_top_attributions_ordering() {
        let mut trace = CausalTrace::new("test", 0, "narrow");
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();

        trace.add_vector_match(e1, "low", 0.3, 2);
        trace.add_vector_match(e2, "high", 0.9, 0);

        let top = trace.top_attributions(1);
        assert_eq!(top[0].entity_name, "high");
    }
}
