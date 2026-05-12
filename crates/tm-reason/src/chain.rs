//! Graph-of-Thought reasoning: multi-hop traversal that builds reasoning chains.
//!
//! Given a query set of entity IDs, the chain builder explores outward through
//! predicates, scoring each step by confidence and predicate relevance, to build
//! structured reasoning paths like:
//!
//!   Rust --[UsedBy]--> TraceMind --[PartOf]--> MemoryOS --[IsA]--> Software
//!
//! Each chain has a composite score = product of step confidences, with a decay
//! factor per hop to prefer shorter, more direct paths.

use std::collections::{HashMap, HashSet, VecDeque};
use serde::Serialize;
use uuid::Uuid;

use tm_graph::GraphStore;

/// A single step in a reasoning chain.
#[derive(Debug, Clone, Serialize)]
pub struct ReasoningStep {
    pub entity_id: Uuid,
    pub entity_name: String,
    pub entity_type: String,
    pub predicate: String,
    pub direction: StepDirection,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum StepDirection {
    Forward,  // entity is the subject
    Backward, // entity is the object
}

/// A complete reasoning chain from source to destination.
#[derive(Debug, Clone, Serialize)]
pub struct ReasoningChain {
    pub steps: Vec<ReasoningStep>,
    pub score: f64,
    pub source_id: Uuid,
    pub destination_id: Uuid,
}

/// Configuration for chain building.
pub struct ChainConfig {
    pub max_hops: usize,
    pub max_chains: usize,
    pub min_confidence: f64,
    pub hop_decay: f64,
    pub exclude_predicates: HashSet<String>,
}

impl Default for ChainConfig {
    fn default() -> Self {
        let mut exclude = HashSet::new();
        exclude.insert("RelatedTo".to_string()); // skip co-occurrence noise
        Self {
            max_hops: 4,
            max_chains: 10,
            min_confidence: 0.1,
            hop_decay: 0.85,
            exclude_predicates: exclude,
        }
    }
}

/// Builds reasoning chains over the knowledge graph.
pub struct ChainBuilder<'a> {
    graph: &'a GraphStore,
    config: ChainConfig,
}

#[derive(Clone)]
struct PathState {
    entity_id: Uuid,
    steps: Vec<ReasoningStep>,
    visited: HashSet<Uuid>,
    score: f64,
}

impl<'a> ChainBuilder<'a> {
    pub fn new(graph: &'a GraphStore, config: ChainConfig) -> Self {
        Self { graph, config }
    }

    pub fn with_defaults(graph: &'a GraphStore) -> Self {
        Self::new(graph, ChainConfig::default())
    }

    /// Find reasoning chains between two entities.
    pub fn find_chains(&self, source: Uuid, destination: Uuid) -> Vec<ReasoningChain> {
        let entity_cache = self.build_entity_cache(&[source, destination]);
        let mut results = Vec::new();
        let mut queue = VecDeque::new();

        let mut initial_visited = HashSet::new();
        initial_visited.insert(source);

        queue.push_back(PathState {
            entity_id: source,
            steps: Vec::new(),
            visited: initial_visited,
            score: 1.0,
        });

        while let Some(state) = queue.pop_front() {
            if results.len() >= self.config.max_chains {
                break;
            }
            if state.steps.len() >= self.config.max_hops {
                continue;
            }

            let triples = match self.graph.get_triples_for_entity(state.entity_id) {
                Ok(t) => t,
                Err(_) => continue,
            };

            for triple in &triples {
                let pred_str = format!("{:?}", triple.predicate);
                if self.config.exclude_predicates.contains(&pred_str) {
                    continue;
                }
                if triple.confidence < self.config.min_confidence {
                    continue;
                }

                let (next_id, direction) = if triple.subject_id == state.entity_id {
                    (triple.object_id, StepDirection::Forward)
                } else {
                    (triple.subject_id, StepDirection::Backward)
                };

                if state.visited.contains(&next_id) {
                    continue;
                }

                let (name, etype) = entity_cache
                    .get(&next_id)
                    .cloned()
                    .or_else(|| self.lookup_entity(next_id))
                    .unwrap_or_else(|| (next_id.to_string(), "Unknown".to_string()));

                let hop_score = triple.confidence * self.config.hop_decay.powi(state.steps.len() as i32);
                let new_score = state.score * hop_score;

                let mut new_steps = state.steps.clone();
                new_steps.push(ReasoningStep {
                    entity_id: next_id,
                    entity_name: name,
                    entity_type: etype,
                    predicate: pred_str,
                    direction,
                    confidence: triple.confidence,
                });

                if next_id == destination {
                    results.push(ReasoningChain {
                        steps: new_steps,
                        score: new_score,
                        source_id: source,
                        destination_id: destination,
                    });
                    continue;
                }

                let mut new_visited = state.visited.clone();
                new_visited.insert(next_id);

                queue.push_back(PathState {
                    entity_id: next_id,
                    steps: new_steps,
                    visited: new_visited,
                    score: new_score,
                });
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(self.config.max_chains);
        results
    }

    /// Explore outward from a set of seed entities, returning the best chains
    /// to any reachable entity. Used for "tell me what you know about X".
    pub fn explore(&self, seeds: &[Uuid], max_results: usize) -> Vec<ReasoningChain> {
        let mut all_chains = Vec::new();

        for &seed in seeds {
            let mut queue = VecDeque::new();
            let mut initial_visited = HashSet::new();
            initial_visited.insert(seed);

            queue.push_back(PathState {
                entity_id: seed,
                steps: Vec::new(),
                visited: initial_visited,
                score: 1.0,
            });

            while let Some(state) = queue.pop_front() {
                if state.steps.len() >= self.config.max_hops {
                    continue;
                }

                let triples = match self.graph.get_triples_for_entity(state.entity_id) {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                for triple in &triples {
                    let pred_str = format!("{:?}", triple.predicate);
                    if self.config.exclude_predicates.contains(&pred_str) {
                        continue;
                    }
                    if triple.confidence < self.config.min_confidence {
                        continue;
                    }

                    let (next_id, direction) = if triple.subject_id == state.entity_id {
                        (triple.object_id, StepDirection::Forward)
                    } else {
                        (triple.subject_id, StepDirection::Backward)
                    };

                    if state.visited.contains(&next_id) {
                        continue;
                    }

                    let (name, etype) = self.lookup_entity(next_id)
                        .unwrap_or_else(|| (next_id.to_string(), "Unknown".to_string()));

                    let hop_score = triple.confidence * self.config.hop_decay.powi(state.steps.len() as i32);
                    let new_score = state.score * hop_score;

                    let mut new_steps = state.steps.clone();
                    new_steps.push(ReasoningStep {
                        entity_id: next_id,
                        entity_name: name,
                        entity_type: etype,
                        predicate: pred_str,
                        direction,
                        confidence: triple.confidence,
                    });

                    // Record this as a chain endpoint
                    all_chains.push(ReasoningChain {
                        steps: new_steps.clone(),
                        score: new_score,
                        source_id: seed,
                        destination_id: next_id,
                    });

                    let mut new_visited = state.visited.clone();
                    new_visited.insert(next_id);

                    queue.push_back(PathState {
                        entity_id: next_id,
                        steps: new_steps,
                        visited: new_visited,
                        score: new_score,
                    });
                }
            }
        }

        all_chains.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        all_chains.truncate(max_results);
        all_chains
    }

    fn build_entity_cache(&self, ids: &[Uuid]) -> HashMap<Uuid, (String, String)> {
        let mut cache = HashMap::new();
        for &id in ids {
            if let Some(info) = self.lookup_entity(id) {
                cache.insert(id, info);
            }
        }
        cache
    }

    fn lookup_entity(&self, id: Uuid) -> Option<(String, String)> {
        self.graph
            .find_entity_by_id(id)
            .ok()
            .flatten()
            .map(|e| (e.name, format!("{:?}", e.entity_type)))
    }
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
    fn test_find_chain_simple() {
        let graph = test_graph();

        let e1 = Entity::new("Rust", EntityType::Technology, 0.9);
        let e2 = Entity::new("TraceMind", EntityType::Project, 0.9);
        let e3 = Entity::new("Memory", EntityType::Concept, 0.8);

        graph.upsert_entity(&e1).unwrap();
        graph.upsert_entity(&e2).unwrap();
        graph.upsert_entity(&e3).unwrap();

        let t1 = Triple {
            id: Uuid::new_v4(),
            subject_id: e2.id,
            predicate: Predicate::DependsOn,
            object_id: e1.id,
            confidence: 0.9,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            predicate_confidence: None,
        };
        let t2 = Triple {
            id: Uuid::new_v4(),
            subject_id: e2.id,
            predicate: Predicate::HasProperty,
            object_id: e3.id,
            confidence: 0.8,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            predicate_confidence: None,
        };

        graph.upsert_triple(&t1).unwrap();
        graph.upsert_triple(&t2).unwrap();

        let builder = ChainBuilder::with_defaults(&graph);
        let chains = builder.find_chains(e1.id, e3.id);

        assert!(!chains.is_empty(), "should find at least one chain");
        assert_eq!(chains[0].steps.len(), 2); // Rust -> TraceMind -> Memory
        assert!(chains[0].score > 0.0);
    }

    #[test]
    fn test_explore_from_seed() {
        let graph = test_graph();

        let e1 = Entity::new("Alice", EntityType::Person, 0.9);
        let e2 = Entity::new("Acme", EntityType::Organization, 0.9);

        graph.upsert_entity(&e1).unwrap();
        graph.upsert_entity(&e2).unwrap();

        let t1 = Triple {
            id: Uuid::new_v4(),
            subject_id: e1.id,
            predicate: Predicate::WorksAt,
            object_id: e2.id,
            confidence: 0.95,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            predicate_confidence: None,
        };
        graph.upsert_triple(&t1).unwrap();

        let builder = ChainBuilder::with_defaults(&graph);
        let chains = builder.explore(&[e1.id], 5);

        assert!(!chains.is_empty());
        assert_eq!(chains[0].destination_id, e2.id);
    }

    #[test]
    fn test_no_chain_through_related_to() {
        let graph = test_graph();

        let e1 = Entity::new("A", EntityType::Concept, 0.5);
        let e2 = Entity::new("B", EntityType::Concept, 0.5);

        graph.upsert_entity(&e1).unwrap();
        graph.upsert_entity(&e2).unwrap();

        let t1 = Triple {
            id: Uuid::new_v4(),
            subject_id: e1.id,
            predicate: Predicate::RelatedTo,
            object_id: e2.id,
            confidence: 0.5,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            predicate_confidence: None,
        };
        graph.upsert_triple(&t1).unwrap();

        let builder = ChainBuilder::with_defaults(&graph);
        let chains = builder.find_chains(e1.id, e2.id);

        // RelatedTo is excluded by default
        assert!(chains.is_empty());
    }
}
