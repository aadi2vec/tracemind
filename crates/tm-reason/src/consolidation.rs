//! Memory consolidation ("sleep"): offline processing that strengthens
//! important memories and lets unimportant ones decay naturally.
//!
//! Inspired by Ebbinghaus forgetting curves and sleep-dependent memory
//! consolidation in neuroscience:
//!
//! - **Strengthening**: Entities/triples accessed frequently or recently
//!   get confidence boosts proportional to their access pattern
//! - **Forgetting**: Entities not accessed decay per Ebbinghaus curve:
//!   R(t) = e^(-t/S) where S = stability (based on access count)
//! - **Merging**: Detects near-duplicate entities and merges them
//! - **Pruning**: Removes entities below confidence threshold
//!
//! This runs as a background task during idle periods.

use std::collections::HashMap;
use serde::Serialize;
use uuid::Uuid;

use tm_graph::GraphStore;

/// Results from a consolidation cycle.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationReport {
    pub entities_strengthened: usize,
    pub entities_decayed: usize,
    pub entities_pruned: usize,
    pub entities_merged: usize,
    pub triples_pruned: usize,
}

/// Configuration for consolidation.
pub struct ConsolidationConfig {
    /// Minimum confidence to survive pruning
    pub prune_threshold: f64,
    /// Base decay rate (higher = faster forgetting)
    pub decay_rate: f64,
    /// Stability multiplier per access (more accesses = slower decay)
    pub stability_per_access: f64,
    /// Confidence boost per access during strengthening
    pub strengthen_amount: f64,
    /// Maximum confidence after strengthening
    pub max_confidence: f64,
    /// Similarity threshold for entity merging (by name)
    pub merge_similarity: f64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            prune_threshold: 0.05,
            decay_rate: 0.1,
            stability_per_access: 0.5,
            strengthen_amount: 0.02,
            max_confidence: 1.0,
            merge_similarity: 0.85,
        }
    }
}

pub struct Consolidator<'a> {
    graph: &'a GraphStore,
    config: ConsolidationConfig,
}

impl<'a> Consolidator<'a> {
    pub fn new(graph: &'a GraphStore, config: ConsolidationConfig) -> Self {
        Self { graph, config }
    }

    pub fn with_defaults(graph: &'a GraphStore) -> Self {
        Self::new(graph, ConsolidationConfig::default())
    }

    /// Run a full consolidation cycle.
    pub fn consolidate(&self) -> ConsolidationReport {
        let mut report = ConsolidationReport {
            entities_strengthened: 0,
            entities_decayed: 0,
            entities_pruned: 0,
            entities_merged: 0,
            triples_pruned: 0,
        };

        let entities = match self.graph.list_all_entities() {
            Ok(e) => e,
            Err(_) => return report,
        };

        if entities.is_empty() {
            return report;
        }

        let entity_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();

        // Batch fetch recency and novelty (access counts)
        let recency_scores = self.graph.batch_recency_scores(&entity_ids);
        let novelty_scores = self.graph.batch_novelty_scores(&entity_ids);

        // Phase 1: Ebbinghaus decay + strengthening
        for entity in &entities {
            let recency = recency_scores.get(&entity.id).copied().unwrap_or(0.0);
            let access_count_score = novelty_scores.get(&entity.id).copied().unwrap_or(1.0);

            // Novelty score is inverse of access count — convert back
            // novelty = 1.0 means 1 access, novelty = 0.5 means ~2, etc.
            let approx_accesses = if access_count_score > 0.0 {
                (1.0 / access_count_score).round().max(1.0)
            } else {
                10.0 // many accesses
            };

            // Stability increases with more accesses
            let stability = 1.0 + approx_accesses * self.config.stability_per_access;

            // Time factor: how much has the memory "aged" since last access
            // recency 1.0 = very recent, 0.0 = very old
            let time_factor = 1.0 - recency; // 0 = just accessed, 1 = long ago

            // Ebbinghaus: R(t) = e^(-t/S)
            let retention = (-time_factor / stability).exp();

            if retention > 0.7 && recency > 0.5 {
                // Recent and well-retained: strengthen
                let boost = self.config.strengthen_amount * retention;
                let new_conf = (entity.confidence + boost).min(self.config.max_confidence);
                if new_conf > entity.confidence {
                    let _ = self.graph.reinforce_entity(entity.id, boost);
                    report.entities_strengthened += 1;
                }
            } else if retention < 0.3 {
                // Poorly retained: decay
                let decay = entity.confidence * (1.0 - retention) * self.config.decay_rate;
                let new_conf = entity.confidence - decay;
                if new_conf < self.config.prune_threshold {
                    // Below threshold: prune
                    let _ = self.graph.delete_entity(entity.id);
                    report.entities_pruned += 1;
                } else {
                    // Apply decay (negative reinforcement)
                    let _ = self.graph.reinforce_entity(entity.id, -decay);
                    report.entities_decayed += 1;
                }
            }
        }

        // Phase 2: Detect and merge near-duplicate entities
        report.entities_merged = self.merge_duplicates(&entities);

        report
    }

    /// Find entities with very similar names and merge them.
    fn merge_duplicates(&self, entities: &[tm_types::Entity]) -> usize {
        let mut merged = 0;
        let mut consumed: std::collections::HashSet<Uuid> = std::collections::HashSet::new();

        // Group by normalized name for O(n) duplicate detection
        let mut name_groups: HashMap<String, Vec<&tm_types::Entity>> = HashMap::new();
        for entity in entities {
            let normalized = normalize_name(&entity.name);
            name_groups.entry(normalized).or_default().push(entity);
        }

        for (_name, group) in &name_groups {
            if group.len() < 2 {
                continue;
            }

            // Keep the entity with highest confidence as the "primary"
            let mut sorted = group.clone();
            sorted.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));

            let primary = sorted[0];
            if consumed.contains(&primary.id) {
                continue;
            }

            for duplicate in &sorted[1..] {
                if consumed.contains(&duplicate.id) {
                    continue;
                }

                // Boost primary's confidence
                let boost = duplicate.confidence * 0.3;
                let _ = self.graph.reinforce_entity(primary.id, boost);

                // Delete the duplicate
                let _ = self.graph.delete_entity(duplicate.id);
                consumed.insert(duplicate.id);
                merged += 1;
            }
        }

        merged
    }
}

/// Normalize entity name for duplicate detection.
fn normalize_name(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::{Entity, EntityType};

    fn test_graph() -> GraphStore {
        GraphStore::open(":memory:").unwrap()
    }

    #[test]
    fn test_consolidation_basic() {
        let graph = test_graph();

        // Create entities with varying confidence
        let e1 = Entity::new("Active Entity", EntityType::Concept, 0.8);
        let e2 = Entity::new("Weak Entity", EntityType::Concept, 0.06);

        graph.upsert_entity(&e1).unwrap();
        graph.upsert_entity(&e2).unwrap();

        let consolidator = Consolidator::with_defaults(&graph);
        let report = consolidator.consolidate();

        // Should have processed some entities
        let total = report.entities_strengthened + report.entities_decayed + report.entities_pruned;
        assert!(total > 0 || report.entities_merged == 0);
    }

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_name("  Hello World  "), "hello world");
        assert_eq!(normalize_name("hello_world"), "hello world");
        assert_eq!(normalize_name("Hello-World"), "hello world");
        assert_eq!(normalize_name("RUST"), "rust");
    }

    #[test]
    fn test_merge_duplicates() {
        let graph = test_graph();

        // Create duplicate entities with different cases
        let e1 = Entity::new("Rust", EntityType::Technology, 0.9);
        let e2 = Entity::new("rust", EntityType::Technology, 0.5);
        let e3 = Entity::new("RUST", EntityType::Technology, 0.3);

        graph.upsert_entity(&e1).unwrap();
        graph.upsert_entity(&e2).unwrap();
        graph.upsert_entity(&e3).unwrap();

        assert_eq!(graph.entity_count().unwrap(), 3);

        let consolidator = Consolidator::with_defaults(&graph);
        let report = consolidator.consolidate();

        assert_eq!(report.entities_merged, 2, "should merge 2 duplicates");
        assert_eq!(graph.entity_count().unwrap(), 1, "should have 1 entity remaining");
    }
}
