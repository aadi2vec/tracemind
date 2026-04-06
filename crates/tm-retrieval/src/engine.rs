use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use tm_controller::bandit::RetrievalParams;
use tm_controller::UcbBandit;
use tm_episodic::TraceStore;
use tm_graph::GraphStore;
use tm_rerank::{ColbertReranker, RerankCandidate};
use tm_types::{Entity, Result, Trace, TraceEventType, TraceMindError, Triple};
use tm_vector::Embedder;
use tracing::info;
use uuid::Uuid;

pub struct RetrievalEngine {
    graph: GraphStore,
    trace_store: TraceStore,
    embedder: Embedder,
    bandit: UcbBandit,
    bandit_path: PathBuf,
    reranker: Option<ColbertReranker>,
}

#[derive(Debug)]
pub struct RetrievalResult {
    pub arm: u8,
    pub entities: Vec<Entity>,
    pub triples: Vec<Triple>,
    pub traces: Vec<Trace>,
    pub latency_ms: u32,
}

impl RetrievalEngine {
    /// Open a `RetrievalEngine` rooted at `db_path`.
    ///
    /// - Graph store  → `db_path`
    /// - Trace store  → `trace_path`
    pub fn open(db_path: &str, trace_path: &str, hash_embed: bool) -> Result<Self> {
        // Derive bandit path as sibling of db_path
        let bandit_path = PathBuf::from(db_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("bandit.json");

        let graph = GraphStore::open(db_path)?;
        let trace_store = TraceStore::open(trace_path)?;
        let embedder = if hash_embed {
            Embedder::new_hash()
        } else {
            Embedder::new()?
        };
        let bandit = UcbBandit::load(&bandit_path);

        Ok(Self {
            graph,
            trace_store,
            embedder,
            bandit,
            bandit_path,
            reranker: None,
        })
    }

    /// Attach a ColBERT reranker for higher-quality retrieval.
    ///
    /// When set, vector search returns a wider candidate set (3x top_k),
    /// which is then reranked with ColBERT MaxSim before entity loading.
    pub fn with_reranker(mut self, model_path: &str, tokenizer_path: &str, alpha: f32) -> Result<Self> {
        let reranker = ColbertReranker::new(model_path, tokenizer_path, alpha)?;
        self.reranker = Some(reranker);
        info!("[retrieval] ColBERT reranker attached (alpha={alpha})");
        Ok(self)
    }

    /// Run the 3-phase UCB-guided retrieval pipeline against `text`.
    pub fn query(&mut self, text: &str) -> Result<RetrievalResult> {
        let start = Instant::now();

        // Phase 1: bandit selects arm → retrieval parameters.
        let params: RetrievalParams = self.bandit.select();
        let arm = params.arm;

        // Phase 2: embed query, vector-search for candidate entity UUIDs,
        //          then load entities from the graph store.
        //          If reranker is available, fetch wider set and rerank.
        let embedding = self.embedder.embed(text);
        let search_k = if self.reranker.is_some() {
            params.top_k * 3 // wider pool for reranking
        } else {
            params.top_k
        };
        let mut candidates = self.graph.search_vectors(&embedding, search_k)?;

        // Phase 2.5: Optional ColBERT reranking
        if let Some(ref reranker) = self.reranker {
            // Build rerank candidates with entity names as text
            let mut rerank_inputs: Vec<(Uuid, RerankCandidate)> = Vec::new();
            for (id, score) in &candidates {
                if let Ok(entity) = self.graph.get_entity(*id) {
                    rerank_inputs.push((*id, RerankCandidate {
                        id: id.to_string(),
                        initial_score: *score,
                        text: entity.name.clone(),
                    }));
                }
            }

            if !rerank_inputs.is_empty() {
                let rerank_candidates: Vec<RerankCandidate> =
                    rerank_inputs.iter().map(|(_, c)| c.clone()).collect();

                match reranker.rerank(text, rerank_candidates) {
                    Ok(reranked) => {
                        // Replace candidates with reranked order, capped to top_k
                        candidates = reranked
                            .into_iter()
                            .take(params.top_k)
                            .filter_map(|r| {
                                Uuid::parse_str(&r.id).ok().map(|id| (id, r.combined_score))
                            })
                            .collect();
                    }
                    Err(e) => {
                        info!("[retrieval] reranker failed, using vector order: {e}");
                        candidates.truncate(params.top_k);
                    }
                }
            }
        }

        let mut seen_entity_ids: HashSet<Uuid> = HashSet::new();
        let mut entities: Vec<Entity> = Vec::new();

        for (id, _score) in candidates {
            if seen_entity_ids.contains(&id) {
                continue;
            }
            match self.graph.get_entity(id) {
                Ok(entity) => {
                    seen_entity_ids.insert(id);
                    entities.push(entity);
                }
                Err(TraceMindError::EntityNotFound(_)) => {
                    // Referenced by vector index but absent from graph — skip.
                }
                Err(TraceMindError::Storage(ref msg)) if msg.contains("no rows") => {
                    // rusqlite returns a Storage error wrapping "no rows returned"
                    // when a UUID is valid but not present in the entities table.
                }
                Err(e) => return Err(e),
            }
        }

        // Phase 3: k-hop graph expansion (only when params.hops > 0).
        if params.hops > 0 {
            // Snapshot the IDs of the seed entities so we can iterate over them
            // without borrowing `entities` mutably at the same time.
            let seed_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();

            for seed_id in seed_ids {
                let neighbors = self.graph.k_hop_neighbors(seed_id, params.hops)?;
                for neighbor in neighbors {
                    if !seen_entity_ids.contains(&neighbor.id) {
                        seen_entity_ids.insert(neighbor.id);
                        entities.push(neighbor);
                    }
                }
            }
        }

        // Phase 4: collect triples for every entity, deduplicated by triple id.
        //          Prefer typed predicates over generic RelatedTo; sort by confidence.
        let mut seen_triple_ids: HashSet<Uuid> = HashSet::new();
        let mut triples: Vec<Triple> = Vec::new();

        for entity in &entities {
            let entity_triples = self.graph.get_triples_for_entity(entity.id)?;
            for triple in entity_triples {
                if !seen_triple_ids.contains(&triple.id) {
                    seen_triple_ids.insert(triple.id);
                    triples.push(triple);
                }
            }
        }

        // Sort: typed predicates first (higher confidence), then by confidence descending.
        triples.sort_by(|a, b| {
            let a_typed = !matches!(a.predicate, tm_types::Predicate::RelatedTo);
            let b_typed = !matches!(b.predicate, tm_types::Predicate::RelatedTo);
            b_typed.cmp(&a_typed).then(b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });

        // Cap triples to avoid noise — keep all typed + up to 20 RelatedTo
        let typed_count = triples.iter().filter(|t| !matches!(t.predicate, tm_types::Predicate::RelatedTo)).count();
        triples.truncate(typed_count + 20);

        // Phase 5: optional episodic traces.
        let traces: Vec<Trace> = if params.include_episodic {
            self.trace_store.recent(50)?
        } else {
            vec![]
        };

        // Measure elapsed time.
        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        // Compute reward: non-zero result → 1.0, empty → 0.3.
        let reward = if !entities.is_empty() { 1.0_f64 } else { 0.3_f64 };
        self.bandit.register_reward(arm, reward);
        self.bandit.save(&self.bandit_path);

        // Persist a retrieval trace for the audit trail.
        let mut trace = Trace::new(Uuid::new_v4(), TraceEventType::Retrieve, "");
        trace.raw_text = Some(text.to_string());
        trace.entities_extracted = entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(arm);
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        Ok(RetrievalResult {
            arm,
            entities,
            triples,
            traces,
            latency_ms,
        })
    }

    /// Return per-arm `(pull_count, average_reward)` statistics from the bandit.
    pub fn bandit_stats(&self) -> [(u64, f64); 4] {
        self.bandit.arm_stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_returns_ok_and_updates_bandit() {
        let dir = std::env::temp_dir().join(format!("tm_ret_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();

        // Build engine manually with hash embedder (avoids model download in tests)
        let bandit_path = dir.join("bandit.json");
        let mut engine = RetrievalEngine {
            graph: GraphStore::open(&db).unwrap(),
            trace_store: TraceStore::open(&traces).unwrap(),
            embedder: Embedder::new_hash(),
            bandit: UcbBandit::new(),
            bandit_path: bandit_path.clone(),
            reranker: None,
        };

        let result = engine.query("hello world").unwrap();
        assert!(result.latency_ms < 5000);
        let stats = engine.bandit_stats();
        let total: u64 = stats.iter().map(|(c, _)| c).sum();
        assert_eq!(total, 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
