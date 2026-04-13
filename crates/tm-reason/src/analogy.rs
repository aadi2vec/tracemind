//! Analogical reasoning: find structurally similar entity neighborhoods.
//!
//! Uses a simplified Weisfeiler-Leman (WL) graph kernel approach:
//! 1. For each entity, compute a "neighborhood fingerprint" from its 1-hop
//!    and 2-hop predicate patterns
//! 2. Compare fingerprints to find entities with similar graph structure
//! 3. Generate "X is like Y because they share pattern Z" explanations
//!
//! This enables queries like "what else is like Rust?" → "Python, because
//! both are Technologies that are UsedBy Projects and have HasProperty links."

use std::collections::BTreeSet;
use serde::Serialize;
use uuid::Uuid;

use tm_graph::GraphStore;

/// Result of an analogy search.
#[derive(Debug, Clone, Serialize)]
pub struct AnalogyResult {
    pub source_id: Uuid,
    pub source_name: String,
    pub target_id: Uuid,
    pub target_name: String,
    pub similarity: f64,
    pub shared_patterns: Vec<String>,
    pub explanation: String,
}

/// Neighborhood fingerprint for WL-style hashing.
#[derive(Debug, Clone)]
struct Fingerprint {
    entity_id: Uuid,
    entity_name: String,
    entity_type: String,
    /// Sorted set of (predicate, direction, neighbor_type) at 1-hop
    hop1_patterns: BTreeSet<String>,
    /// Sorted set of (predicate, direction, neighbor_type, predicate2, direction2, neighbor_type2) at 2-hop
    hop2_patterns: BTreeSet<String>,
}

pub struct AnalogySolver<'a> {
    graph: &'a GraphStore,
}

impl<'a> AnalogySolver<'a> {
    pub fn new(graph: &'a GraphStore) -> Self {
        Self { graph }
    }

    /// Find entities structurally similar to the given entity.
    pub fn find_analogies(&self, entity_id: Uuid, max_results: usize) -> Vec<AnalogyResult> {
        let source_fp = match self.compute_fingerprint(entity_id) {
            Some(fp) => fp,
            None => return Vec::new(),
        };

        let all_entities = match self.graph.list_all_entities() {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        let mut results: Vec<AnalogyResult> = all_entities
            .iter()
            .filter(|e| e.id != entity_id)
            .filter_map(|e| {
                let target_fp = self.compute_fingerprint(e.id)?;
                let (sim, shared) = fingerprint_similarity(&source_fp, &target_fp);
                if sim < 0.1 {
                    return None;
                }
                let explanation = format!(
                    "{} is like {} because both are {} with similar relationship patterns: {}",
                    source_fp.entity_name,
                    target_fp.entity_name,
                    if source_fp.entity_type == target_fp.entity_type {
                        format!("{}s", source_fp.entity_type)
                    } else {
                        format!("{}/{}", source_fp.entity_type, target_fp.entity_type)
                    },
                    if shared.is_empty() { "structural similarity".to_string() } else { shared.join(", ") }
                );

                Some(AnalogyResult {
                    source_id: entity_id,
                    source_name: source_fp.entity_name.clone(),
                    target_id: e.id,
                    target_name: target_fp.entity_name.clone(),
                    similarity: sim,
                    shared_patterns: shared,
                    explanation,
                })
            })
            .collect();

        results.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(max_results);
        results
    }

    fn compute_fingerprint(&self, entity_id: Uuid) -> Option<Fingerprint> {
        let entity = self.graph.find_entity_by_id(entity_id).ok()??;
        let triples = self.graph.get_triples_for_entity(entity_id).ok()?;

        let mut hop1_patterns = BTreeSet::new();
        let mut hop1_neighbors: Vec<Uuid> = Vec::new();

        for triple in &triples {
            let pred = format!("{:?}", triple.predicate);
            if pred == "RelatedTo" {
                continue;
            }

            let (neighbor_id, direction) = if triple.subject_id == entity_id {
                (triple.object_id, "->")
            } else {
                (triple.subject_id, "<-")
            };

            let neighbor_type = self.graph
                .find_entity_by_id(neighbor_id)
                .ok()
                .flatten()
                .map(|e| format!("{:?}", e.entity_type))
                .unwrap_or_else(|| "Unknown".to_string());

            hop1_patterns.insert(format!("{}{}{}", pred, direction, neighbor_type));
            hop1_neighbors.push(neighbor_id);
        }

        // 2-hop patterns (limited to avoid explosion)
        let mut hop2_patterns = BTreeSet::new();
        for &neighbor_id in hop1_neighbors.iter().take(10) {
            if let Ok(neighbor_triples) = self.graph.get_triples_for_entity(neighbor_id) {
                for t2 in neighbor_triples.iter().take(10) {
                    let pred2 = format!("{:?}", t2.predicate);
                    if pred2 == "RelatedTo" {
                        continue;
                    }
                    let (next_id, dir2) = if t2.subject_id == neighbor_id {
                        (t2.object_id, "->")
                    } else {
                        (t2.subject_id, "<-")
                    };
                    if next_id == entity_id {
                        continue;
                    }
                    let next_type = self.graph
                        .find_entity_by_id(next_id)
                        .ok()
                        .flatten()
                        .map(|e| format!("{:?}", e.entity_type))
                        .unwrap_or_else(|| "Unknown".to_string());

                    hop2_patterns.insert(format!("*{}{}{}", pred2, dir2, next_type));
                }
            }
        }

        Some(Fingerprint {
            entity_id,
            entity_name: entity.name,
            entity_type: format!("{:?}", entity.entity_type),
            hop1_patterns,
            hop2_patterns,
        })
    }
}

/// Jaccard-like similarity between two fingerprints.
fn fingerprint_similarity(a: &Fingerprint, b: &Fingerprint) -> (f64, Vec<String>) {
    let hop1_shared: BTreeSet<_> = a.hop1_patterns.intersection(&b.hop1_patterns).cloned().collect();
    let hop1_union: BTreeSet<_> = a.hop1_patterns.union(&b.hop1_patterns).cloned().collect();

    let hop2_shared: BTreeSet<_> = a.hop2_patterns.intersection(&b.hop2_patterns).cloned().collect();
    let hop2_union: BTreeSet<_> = a.hop2_patterns.union(&b.hop2_patterns).cloned().collect();

    // Weight: 60% hop1, 40% hop2, plus 10% bonus if same type
    let hop1_sim = if hop1_union.is_empty() { 0.0 } else { hop1_shared.len() as f64 / hop1_union.len() as f64 };
    let hop2_sim = if hop2_union.is_empty() { 0.0 } else { hop2_shared.len() as f64 / hop2_union.len() as f64 };
    let type_bonus = if a.entity_type == b.entity_type { 0.1 } else { 0.0 };

    let sim = (hop1_sim * 0.6 + hop2_sim * 0.3 + type_bonus).min(1.0);

    let shared: Vec<String> = hop1_shared.into_iter().collect();
    (sim, shared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::{Entity, EntityType, Triple, Predicate};
    use chrono::Utc;

    fn test_graph() -> GraphStore {
        GraphStore::open(":memory:").unwrap()
    }

    #[test]
    fn test_analogy_similar_structure() {
        let graph = test_graph();

        // Create two technologies with similar relationship patterns
        let rust = Entity::new("Rust", EntityType::Technology, 0.9);
        let python = Entity::new("Python", EntityType::Technology, 0.9);
        let proj1 = Entity::new("TraceMind", EntityType::Project, 0.9);
        let proj2 = Entity::new("Django", EntityType::Project, 0.9);

        graph.upsert_entity(&rust).unwrap();
        graph.upsert_entity(&python).unwrap();
        graph.upsert_entity(&proj1).unwrap();
        graph.upsert_entity(&proj2).unwrap();

        // Rust --UsedBy--> TraceMind
        graph.upsert_triple(&Triple {
            id: Uuid::new_v4(),
            subject_id: proj1.id,
            predicate: Predicate::DependsOn,
            object_id: rust.id,
            confidence: 0.9,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }).unwrap();

        // Python --UsedBy--> Django
        graph.upsert_triple(&Triple {
            id: Uuid::new_v4(),
            subject_id: proj2.id,
            predicate: Predicate::DependsOn,
            object_id: python.id,
            confidence: 0.9,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }).unwrap();

        let solver = AnalogySolver::new(&graph);
        let analogies = solver.find_analogies(rust.id, 5);

        assert!(!analogies.is_empty(), "should find Python as analogous to Rust");
        assert_eq!(analogies[0].target_id, python.id);
        assert!(analogies[0].similarity > 0.0);
    }
}
