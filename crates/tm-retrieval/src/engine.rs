use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tm_controller::bandit::RetrievalParams;
use tm_controller::{UcbBandit, LinUcbBandit, QueryPlanner, QueryPlan, PlanAction};
use tm_episodic::{ProcedureStore, TraceStore, TrajectoryStore};
use tm_graph::GraphStore;
use tm_reason::CausalTrace;
use tm_rerank::{ColbertReranker, RerankCandidate};
use tm_types::{Entity, Procedure, Result, Trace, TraceEventType, TraceMindError, Triple};
use tm_vector::{Embedder, EmbedModel};
use tracing::info;
use uuid::Uuid;

/// Recent query embedding cache for relevance gating and recommendations.
struct RecentQueryCache {
    embeddings: VecDeque<Vec<f32>>,
    texts: VecDeque<String>,
    max_size: usize,
}

impl RecentQueryCache {
    fn new(max_size: usize) -> Self {
        Self {
            embeddings: VecDeque::with_capacity(max_size),
            texts: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, text: String, embedding: Vec<f32>) {
        if self.embeddings.len() >= self.max_size {
            self.embeddings.pop_front();
            self.texts.pop_front();
        }
        self.embeddings.push_back(embedding);
        self.texts.push_back(text);
    }

    fn context_text(&self) -> String {
        self.texts.iter().rev().take(3).cloned().collect::<Vec<_>>().join(" ")
    }
}

/// Pending reward for deferred bandit feedback.
/// Uses wall-clock time (not Instant) so dwell measurement survives across app lifecycle.
#[allow(dead_code)]
struct PendingReward {
    arm: u8,
    base_score: f64,
    created_at_mono: Instant,
    created_at_epoch_ms: u64,
    clicks: u32,
    requeried: bool,
    /// Query embedding for LinUCB contextual reward update
    context: Vec<f32>,
}

fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// A proactive recommendation with reason.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Recommendation {
    pub entity_id: String,
    pub entity_name: String,
    pub entity_type: String,
    pub score: f64,
    pub reason: String,
}

pub struct RetrievalEngine {
    graph: GraphStore,
    trace_store: TraceStore,
    embedder: Embedder,
    bandit: UcbBandit,
    bandit_path: PathBuf,
    linucb: LinUcbBandit,
    linucb_path: PathBuf,
    reranker: Option<ColbertReranker>,
    query_cache: RecentQueryCache,
    pending_reward: Option<PendingReward>,
    planner: QueryPlanner,
    procedure_store: Option<ProcedureStore>,
    trajectory_store: Option<TrajectoryStore>,
}

#[derive(Debug)]
pub struct RetrievalResult {
    pub arm: u8,
    pub entities: Vec<Entity>,
    pub triples: Vec<Triple>,
    pub traces: Vec<Trace>,
    pub latency_ms: u32,
    pub causal_trace: CausalTrace,
    /// The plan that was executed for this query.
    pub plan: Option<QueryPlan>,
    /// When true, the system is not confident in these results.
    pub low_confidence: bool,
    /// Suggested follow-up queries when confidence is low.
    pub suggested_queries: Vec<String>,
    /// Procedures matching the query (Phase 6: procedural memory).
    pub procedures: Vec<Procedure>,
}

impl RetrievalEngine {
    /// Open a `RetrievalEngine` rooted at `db_path`.
    ///
    /// - Graph store  → `db_path`
    /// - Trace store  → `trace_path`
    pub fn open(db_path: &str, trace_path: &str, hash_embed: bool) -> Result<Self> {
        // Derive bandit paths as sibling of db_path
        let parent = PathBuf::from(db_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let bandit_path = parent.join("bandit.json");
        let linucb_path = parent.join("linucb.json");

        let graph = GraphStore::open(db_path)?;
        let trace_store = TraceStore::open(trace_path)?;
        let embedder = if hash_embed {
            Embedder::new_hash()
        } else if let Some(model) = std::env::var("TM_EMBED_MODEL").ok()
            .and_then(|s| EmbedModel::from_str_loose(&s))
        {
            Embedder::with_model(model)?
        } else {
            Embedder::new()?
        };
        let bandit = UcbBandit::load(&bandit_path);
        let linucb = LinUcbBandit::load(&linucb_path);

        // Try to auto-open procedure and trajectory stores from sibling files
        let proc_path = parent.join("procedures.jsonl");
        let procedure_store = ProcedureStore::open(&proc_path).ok();
        let traj_path = parent.join("trajectories.jsonl");
        let trajectory_store = TrajectoryStore::open(&traj_path).ok();

        Ok(Self {
            graph,
            trace_store,
            embedder,
            bandit,
            bandit_path,
            linucb,
            linucb_path,
            reranker: None,
            query_cache: RecentQueryCache::new(10),
            pending_reward: None,
            planner: QueryPlanner::new(),
            procedure_store,
            trajectory_store,
        })
    }

    /// Attach a ProcedureStore for procedural memory matching.
    pub fn with_procedures(mut self, path: &str) -> Result<Self> {
        let store = ProcedureStore::open(path)?;
        self.procedure_store = Some(store);
        Ok(self)
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

    /// Match stored procedures against a query string.
    ///
    /// Scoring:
    /// - Exact name substring match → 1.0
    /// - Word overlap (Jaccard) between query words and procedure name words
    /// - 0.5 * Jaccard of query words vs procedure step action texts
    ///
    /// Returns procedures scoring > 0.2, sorted descending, limited to top 3.
    fn match_procedures(&self, query: &str) -> Vec<Procedure> {
        let store = match &self.procedure_store {
            Some(s) => s,
            None => return vec![],
        };
        let active = match store.list_active() {
            Ok(procs) => procs,
            Err(_) => return vec![],
        };

        let query_lower = query.to_lowercase();
        let query_words: HashSet<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(f64, Procedure)> = Vec::new();
        for proc in active {
            let name_lower = proc.name.to_lowercase();

            // (a) exact substring match
            if query_lower.contains(&name_lower) || name_lower.contains(&query_lower) {
                scored.push((1.0, proc));
                continue;
            }

            // (b) Jaccard of query words vs name words
            let name_words: HashSet<&str> = name_lower.split_whitespace().collect();
            let name_jaccard = if query_words.is_empty() && name_words.is_empty() {
                0.0
            } else {
                let intersection = query_words.intersection(&name_words).count() as f64;
                let union = query_words.union(&name_words).count() as f64;
                if union > 0.0 { intersection / union } else { 0.0 }
            };

            // (c) Jaccard of query words vs step action words
            let step_text: String = proc.steps.iter()
                .map(|s| s.action.to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            let step_words: HashSet<&str> = step_text.split_whitespace().collect();
            let step_jaccard = if query_words.is_empty() && step_words.is_empty() {
                0.0
            } else {
                let intersection = query_words.intersection(&step_words).count() as f64;
                let union = query_words.union(&step_words).count() as f64;
                if union > 0.0 { intersection / union } else { 0.0 }
            };

            let score = name_jaccard + 0.5 * step_jaccard;
            if score > 0.2 {
                scored.push((score, proc));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(3).map(|(_, p)| p).collect()
    }

    /// Generate follow-up query suggestions when confidence is low.
    fn generate_suggestions(&self, query: &str, plan: &QueryPlan, entities: &[Entity]) -> Vec<String> {
        let mut suggestions: Vec<String> = Vec::new();

        // If entities were found, suggest learning more about the top one
        if let Some(top) = entities.first() {
            suggestions.push(format!("Tell me more about {}", top.name));
        }

        // If BanditRetrieval and results were sparse, suggest narrowing
        if matches!(plan.action, PlanAction::BanditRetrieval) && entities.len() < 3 {
            let first_word = query.split_whitespace()
                .find(|w| w.len() > 2)
                .unwrap_or(query);
            suggestions.push(format!("What do you know about {}?", first_word));
        }

        // If the query had entity hints, suggest a relationship query
        if plan.entity_hints.len() >= 2 {
            suggestions.push(format!(
                "How does {} relate to {}?",
                plan.entity_hints[0], plan.entity_hints[1]
            ));
        }

        // Always add an analogy suggestion based on the main topic
        let main_topic = plan.entity_hints.first()
            .map(|s| s.as_str())
            .unwrap_or_else(|| {
                query.split_whitespace()
                    .find(|w| w.len() > 3)
                    .unwrap_or(query)
            });
        suggestions.push(format!("What's similar to {}?", main_topic));

        // Deduplicate and cap at 3
        let mut seen = HashSet::new();
        suggestions.retain(|s| seen.insert(s.clone()));
        suggestions.truncate(3);
        suggestions
    }

    /// Run the MIA-inspired retrieval pipeline against `text`.
    ///
    /// Improvements over vanilla pipeline:
    /// 1. Session context blending (80% query, 20% session context) per MIA paper
    /// 2. Composite scoring: 0.7*Sim + 0.15*Value + 0.15*Frequency per MIA
    /// 3. Fallback cascade: if results score poorly, try next bandit arm
    pub fn query(&mut self, text: &str) -> Result<RetrievalResult> {
        let start = Instant::now();

        // Phase 0: Planner assesses query and selects strategy (Graph-R1 "think" step).
        let plan = self.planner.plan(text);
        info!(
            "[retrieval] plan: {:?} (complexity={}, confidence={:.2}, hints={:?})",
            plan.action, plan.complexity, plan.confidence, plan.entity_hints
        );

        // Phase 0.5: If planner returns Decompose, execute each sub-query and merge.
        if let PlanAction::Decompose { ref sub_queries } = plan.action {
            if sub_queries.len() > 1 {
                return self.query_decomposed(text, sub_queries, &plan, start);
            }
        }

        // Phase 1: embed query first (needed for LinUCB context)
        let query_embedding = self.embedder.embed(text);

        // Phase 1.5: select retrieval parameters based on plan.
        // Uses LinUCB (contextual bandit) when the bandit decides — learns
        // "for ML queries use wide arm, for lookups use narrow arm."
        let params: RetrievalParams = match &plan.action {
            PlanAction::DirectLookup => {
                // Simple query → force narrow arm for speed
                UcbBandit::params_for_arm(0)
            }
            PlanAction::ReasoningChain { .. } => {
                // Reasoning needs graph hops → force hybrid or deep arm
                UcbBandit::params_for_arm(2)
            }
            _ => {
                // BanditRetrieval, Analogy, Consolidate → LinUCB with context
                // + trajectory nearest-neighbor hint for non-parametric prior
                let hint = self.trajectory_store.as_ref()
                    .and_then(|ts| ts.nearest_successful_arm(&query_embedding, 0.7))
                    .map(|(arm, _sim)| arm);
                self.linucb.select_with_hint(&query_embedding, hint)
            }
        };
        let arm = params.arm;
        let embedding = self.blend_with_context(&query_embedding);
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

        // Initialize causal trace for attribution tracking
        let arm_names = ["vector-only", "graph-heavy", "hybrid", "episodic"];
        let mut causal = CausalTrace::new(text, arm as usize, arm_names.get(arm as usize).unwrap_or(&"unknown"));

        let mut seen_entity_ids: HashSet<Uuid> = HashSet::new();
        let mut entities: Vec<Entity> = Vec::new();

        for (rank, (id, score)) in candidates.iter().enumerate() {
            if seen_entity_ids.contains(id) {
                continue;
            }
            match self.graph.get_entity(*id) {
                Ok(entity) => {
                    causal.add_vector_match(*id, &entity.name, *score as f64, rank);
                    seen_entity_ids.insert(*id);
                    entities.push(entity);
                }
                Err(TraceMindError::EntityNotFound(_)) => {}
                Err(TraceMindError::Storage(ref msg)) if msg.contains("no rows") => {}
                Err(e) => return Err(e),
            }
        }

        // Phase 2.7: Reciprocal Rank Aggregation (Graph-R1) — parameter-free rank fusion
        //            RRA_score(id) = Σ_lists 1/(k + rank(id) + 1), k=60
        // Legacy MIA linear: 0.7*Sim + 0.15*Value + 0.15*Freq
        if !entities.is_empty() {
            let entity_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();
            let value_scores = self.graph.batch_value_scores(&entity_ids);
            let freq_scores = self.graph.batch_frequency_scores(&entity_ids);

            // Build 3 ranked lists for RRA fusion
            // List 1: entities ranked by similarity score (descending)
            let mut sim_list: Vec<(Uuid, f64)> = candidates.iter()
                .filter(|(id, _)| seen_entity_ids.contains(id))
                .map(|(id, score)| (*id, *score as f64))
                .collect();
            sim_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            // List 2: entities ranked by value score (descending)
            let mut value_list: Vec<(Uuid, f64)> = entity_ids.iter()
                .map(|id| (*id, value_scores.get(id).copied().unwrap_or(0.5)))
                .collect();
            value_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            // List 3: entities ranked by frequency score (descending)
            let mut freq_list: Vec<(Uuid, f64)> = entity_ids.iter()
                .map(|id| (*id, freq_scores.get(id).copied().unwrap_or(1.0)))
                .collect();
            freq_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let fused = rra_fuse(&[sim_list, value_list, freq_list], 60.0);

            // Reorder entities by fused ranking
            let rank_map: HashMap<Uuid, usize> = fused.iter()
                .enumerate()
                .map(|(rank, (id, _))| (*id, rank))
                .collect();
            entities.sort_by_key(|e| rank_map.get(&e.id).copied().unwrap_or(usize::MAX));

            // Record retrieval for value tracking
            for id in &entity_ids {
                let _ = self.graph.record_retrieval(*id);
            }
        }

        // Phase 2.8: Progressive fallback attenuation (GraphRAG-R1)
        // Each cascade step is worth 60% of the previous — prevents over-retrieval
        let max_sim = candidates.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
        let mut cascade_depth: u8 = 0;
        let mut current_arm = arm;
        let mut attenuation = 1.0f64;

        while max_sim < 0.3 && entities.len() < 3 && current_arm < 3 {
            current_arm += 1;
            cascade_depth += 1;
            attenuation *= 0.6;

            info!("[retrieval] fallback cascade depth {}: arm {} → {}, attenuation={:.2}",
                  cascade_depth, arm, current_arm, attenuation);

            let fallback_params = UcbBandit::params_for_arm(current_arm);
            if let Ok(fallback_candidates) = self.graph.search_vectors(&embedding, fallback_params.top_k) {
                for (id, score) in fallback_candidates {
                    // Only accept if attenuated score passes threshold
                    if (score as f64 * attenuation) < 0.15 {
                        continue;
                    }
                    if seen_entity_ids.contains(&id) {
                        continue;
                    }
                    match self.graph.get_entity(id) {
                        Ok(entity) => {
                            causal.add_vector_match(id, &entity.name, score as f64 * attenuation, entities.len());
                            seen_entity_ids.insert(id);
                            entities.push(entity);
                        }
                        _ => {}
                    }
                }
            }

            // Re-check: if we got some results, stop cascading
            if !entities.is_empty() {
                break;
            }
        }

        // Phase 2.9: Diversity penalty (MMR-style greedy reranking).
        // Prevents returning 3 near-identical entities when coverage matters.
        // λ=0.3 is mild — preserves relevance ordering but breaks ties toward diversity.
        diversify_entities(&mut entities, &self.graph, 0.3);

        // Phase 3: k-hop graph expansion (only when params.hops > 0).
        if params.hops > 0 {
            // Snapshot the IDs of the seed entities so we can iterate over them
            // without borrowing `entities` mutably at the same time.
            let seed_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();

            for seed_id in seed_ids {
                let neighbors = self.graph.k_hop_neighbors(seed_id, params.hops)?;
                for neighbor in neighbors {
                    if !seen_entity_ids.contains(&neighbor.id) {
                        causal.add_graph_hop(neighbor.id, &neighbor.name, seed_id, "k_hop", params.hops as usize);
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
            let recent = self.trace_store.recent(50)?;
            for trace in &recent {
                for eid in &trace.entities_extracted {
                    if !seen_entity_ids.contains(eid) {
                        if let Ok(e) = self.graph.get_entity(*eid) {
                            causal.add_episodic(*eid, &e.name, &trace.id.to_string());
                        }
                    }
                }
            }
            recent
        } else {
            vec![]
        };

        // Measure elapsed time.
        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        // Finalize any previous pending reward before creating a new one.
        self.finalize_pending_reward();

        // Create deferred reward — will be finalized on next query or explicit flush.
        let base_score = if !entities.is_empty() {
            (entities.len() as f64 / 5.0).min(1.0)
        } else {
            0.0
        };
        self.pending_reward = Some(PendingReward {
            arm,
            base_score,
            created_at_mono: Instant::now(),
            created_at_epoch_ms: epoch_ms_now(),
            clicks: 0,
            requeried: false,
            context: query_embedding.clone(),
        });

        // Log access for each result entity (for recommendation scoring).
        for entity in &entities {
            let _ = self.graph.log_access(entity.id, "query_result", Some(text));
        }

        // Cache query embedding for recommendations + relevance gating.
        self.query_cache.push(text.to_string(), embedding.clone());

        // Persist a retrieval trace for the audit trail.
        let mut trace = Trace::new(Uuid::new_v4(), TraceEventType::Retrieve, "");
        trace.raw_text = Some(text.to_string());
        trace.entities_extracted = entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(arm);
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        // Finalize causal trace
        causal.total_entities = entities.len();
        causal.total_triples = triples.len();
        causal.latency_ms = latency_ms as u64;

        // Confidence assessment: low when plan confidence is weak and few entities, or no entities at all
        let low_confidence = entities.is_empty()
            || (plan.confidence < 0.5 && entities.len() < 2);

        let suggested_queries = if low_confidence {
            self.generate_suggestions(text, &plan, &entities)
        } else {
            vec![]
        };

        // Phase 6: procedural memory matching
        let procedures = self.match_procedures(text);

        Ok(RetrievalResult {
            arm,
            entities,
            triples,
            traces,
            latency_ms,
            causal_trace: causal,
            plan: Some(plan),
            low_confidence,
            suggested_queries,
            procedures,
        })
    }

    /// Execute a decomposed query: run each sub-query independently, merge + deduplicate results.
    ///
    /// This is the execution path for the Planner's `Decompose` action.
    /// Example: "Compare Rust and Python" → sub-queries ["Rust", "Python"]
    /// → retrieve each → merge entities/triples → deduplicate → return unified result.
    fn query_decomposed(
        &mut self,
        original_text: &str,
        sub_queries: &[String],
        plan: &QueryPlan,
        start: Instant,
    ) -> Result<RetrievalResult> {
        info!("[retrieval] decomposed query: {} sub-queries", sub_queries.len());

        let mut all_entities: Vec<Entity> = Vec::new();
        let mut all_triples: Vec<Triple> = Vec::new();
        let all_traces: Vec<Trace> = Vec::new();
        let mut seen_entity_ids: HashSet<Uuid> = HashSet::new();
        let mut seen_triple_ids: HashSet<Uuid> = HashSet::new();
        let mut causal = CausalTrace::new(original_text, 0, "decomposed");

        for sub_q in sub_queries {
            // Use a narrow retrieval for each sub-query (arm 0 for speed)
            let sub_embedding = self.embedder.embed(sub_q);
            let sub_params = UcbBandit::params_for_arm(1); // medium arm per sub-query

            if let Ok(candidates) = self.graph.search_vectors(&sub_embedding, sub_params.top_k) {
                for (rank, (id, score)) in candidates.iter().enumerate() {
                    if seen_entity_ids.contains(id) {
                        continue;
                    }
                    match self.graph.get_entity(*id) {
                        Ok(entity) => {
                            causal.add_vector_match(*id, &entity.name, *score as f64, rank);
                            seen_entity_ids.insert(*id);
                            all_entities.push(entity);
                        }
                        Err(TraceMindError::EntityNotFound(_)) => {}
                        Err(TraceMindError::Storage(ref msg)) if msg.contains("no rows") => {}
                        Err(e) => return Err(e),
                    }
                }
            }

            // 1-hop graph expansion for each sub-query's results
            let seed_ids: Vec<Uuid> = all_entities.iter()
                .filter(|e| !seen_entity_ids.contains(&e.id) || true)
                .map(|e| e.id)
                .collect();
            for seed_id in seed_ids.iter().take(5) {
                if let Ok(neighbors) = self.graph.k_hop_neighbors(*seed_id, 1) {
                    for neighbor in neighbors {
                        if !seen_entity_ids.contains(&neighbor.id) {
                            causal.add_graph_hop(neighbor.id, &neighbor.name, *seed_id, "decompose_hop", 1);
                            seen_entity_ids.insert(neighbor.id);
                            all_entities.push(neighbor);
                        }
                    }
                }
            }
        }

        // Collect triples for all entities
        for entity in &all_entities {
            if let Ok(entity_triples) = self.graph.get_triples_for_entity(entity.id) {
                for triple in entity_triples {
                    if seen_triple_ids.insert(triple.id) {
                        all_triples.push(triple);
                    }
                }
            }
        }

        // Sort triples: typed first, then by confidence
        all_triples.sort_by(|a, b| {
            let a_typed = !matches!(a.predicate, tm_types::Predicate::RelatedTo);
            let b_typed = !matches!(b.predicate, tm_types::Predicate::RelatedTo);
            b_typed.cmp(&a_typed).then(b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });
        let typed_count = all_triples.iter().filter(|t| !matches!(t.predicate, tm_types::Predicate::RelatedTo)).count();
        all_triples.truncate(typed_count + 20);

        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        // Finalize causal trace
        causal.total_entities = all_entities.len();
        causal.total_triples = all_triples.len();
        causal.latency_ms = latency_ms as u64;

        // Cache the original query embedding
        let emb = self.embedder.embed(original_text);
        self.query_cache.push(original_text.to_string(), emb);

        // Log access
        for entity in &all_entities {
            let _ = self.graph.log_access(entity.id, "query_result", Some(original_text));
        }

        // Persist trace
        let mut trace = Trace::new(Uuid::new_v4(), TraceEventType::Retrieve, "");
        trace.raw_text = Some(original_text.to_string());
        trace.entities_extracted = all_entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = all_triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(0); // decomposed
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        Ok(RetrievalResult {
            arm: 0,
            entities: all_entities,
            triples: all_triples,
            traces: all_traces,
            latency_ms,
            causal_trace: causal,
            plan: Some(plan.clone()),
            low_confidence: false,
            suggested_queries: vec![],
            procedures: vec![],
        })
    }

    /// Return per-arm `(pull_count, average_reward)` statistics from the bandit.
    pub fn bandit_stats(&self) -> [(u64, f64); 4] {
        self.bandit.arm_stats()
    }

    /// LinUCB contextual bandit statistics: (pulls, avg_weight_magnitude) per arm.
    pub fn linucb_stats(&self) -> [(u64, f64); 4] {
        self.linucb.arm_stats()
    }

    /// Register a click on a result entity (implicit positive feedback).
    pub fn register_click(&mut self, entity_id: Uuid) {
        if let Some(ref mut pending) = self.pending_reward {
            pending.clicks += 1;
        }
        let _ = self.graph.log_access(entity_id, "clicked", None);
    }

    /// Finalize a pending deferred reward using simplified outcome-aligned signal.
    ///
    /// R1 papers consistently show that simpler, outcome-aligned rewards outperform
    /// complex composite proxies (Memory-R1, Graph-R1, GraphRAG-R1).
    ///
    /// Signal mapping:
    ///   - User clicked a result          → 0.7  (positive engagement)
    ///   - Dwelled >10s without re-query  → 0.5  (probably useful)
    ///   - Re-queried within 5s           → 0.1  (results were poor)
    ///   - Otherwise                      → 0.3  (ambiguous)
    ///
    /// Legacy composite: 0.2*base + 0.4*click + 0.2*dwell + 0.2*(1-requery)
    fn finalize_pending_reward(&mut self) {
        let pending = match self.pending_reward.take() {
            Some(p) => p,
            None => return,
        };

        let elapsed = pending.created_at_mono.elapsed();
        let rapid_requery = pending.requeried || elapsed < Duration::from_secs(5);

        let reward = if pending.clicks > 0 {
            0.7
        } else if rapid_requery {
            0.1
        } else if elapsed > Duration::from_secs(10) {
            0.5
        } else {
            0.3
        };

        self.bandit.register_reward(pending.arm, reward);
        self.bandit.save(&self.bandit_path);

        // Also update LinUCB with contextual reward
        self.linucb.register_reward(pending.arm, reward, &pending.context);
        self.linucb.save(&self.linucb_path);
    }

    /// Compute PageRank scores (cached lazily per query cycle).
    fn pagerank_scores(&self) -> HashMap<Uuid, f64> {
        self.graph.pagerank().unwrap_or_default()
    }

    /// Generate proactive recommendations based on recent queries and graph structure.
    ///
    /// Scoring: 0.4*relevance + 0.2*recency + 0.2*novelty + 0.2*pagerank
    /// Cold start (no queries): uses recently ingested entities scored by recency + novelty + pagerank.
    pub fn recommendations(&self, limit: usize) -> Vec<Recommendation> {
        let context_text = self.query_cache.context_text();

        if context_text.is_empty() {
            return self.cold_start_recommendations(limit);
        }

        let context_emb = self.embedder.embed(&context_text);

        let candidates = match self.graph.search_vectors(&context_emb, limit * 4) {
            Ok(c) => c,
            Err(_) => return self.cold_start_recommendations(limit),
        };

        if candidates.is_empty() {
            return self.cold_start_recommendations(limit);
        }

        let pr_scores = self.pagerank_scores();
        let pr_max = pr_scores.values().copied().fold(0.0f64, f64::max).max(1e-9);

        // Batch fetch recency/novelty scores (single SQL each instead of N+1)
        let entity_ids: Vec<Uuid> = candidates.iter().map(|(id, _)| *id).collect();
        let recency_map = self.graph.batch_recency_scores(&entity_ids);
        let novelty_map = self.graph.batch_novelty_scores(&entity_ids);

        let mut scored: Vec<Recommendation> = Vec::new();

        for (entity_id, sim) in candidates {
            let entity = match self.graph.get_entity(entity_id) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let relevance = sim as f64;
            let recency = recency_map.get(&entity_id).copied().unwrap_or(0.0);
            let novelty = novelty_map.get(&entity_id).copied().unwrap_or(1.0);
            let pr = pr_scores.get(&entity_id).copied().unwrap_or(0.0) / pr_max;

            let score = 0.4 * relevance + 0.2 * recency + 0.2 * novelty + 0.2 * pr;

            let reason = if pr > 0.5 && relevance > 0.3 {
                "Central to your knowledge graph".to_string()
            } else if relevance > 0.6 {
                "Related to your recent queries".to_string()
            } else if recency > 0.7 {
                "Recently accessed".to_string()
            } else if novelty > 0.5 && relevance > 0.3 {
                "You haven't looked at this recently".to_string()
            } else {
                "Connected in your knowledge graph".to_string()
            };

            scored.push(Recommendation {
                entity_id: entity_id.to_string(),
                entity_name: entity.name,
                entity_type: format!("{}", entity.entity_type),
                score,
                reason,
            });
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    /// Cold-start recommendations when no queries exist yet.
    /// Scored by 0.3*recency + 0.2*novelty + 0.2*confidence + 0.3*pagerank.
    fn cold_start_recommendations(&self, limit: usize) -> Vec<Recommendation> {
        let recent_traces = match self.trace_store.recent(20) {
            Ok(t) => t,
            Err(_) => return vec![],
        };

        let pr_scores = self.pagerank_scores();
        let pr_max = pr_scores.values().copied().fold(0.0f64, f64::max).max(1e-9);

        // Collect unique entity IDs first, then batch-fetch scores
        let mut seen = HashSet::new();
        let mut entity_ids_ordered: Vec<Uuid> = Vec::new();
        for trace in &recent_traces {
            for entity_id in &trace.entities_extracted {
                if seen.insert(*entity_id) {
                    entity_ids_ordered.push(*entity_id);
                }
            }
        }

        let recency_map = self.graph.batch_recency_scores(&entity_ids_ordered);
        let novelty_map = self.graph.batch_novelty_scores(&entity_ids_ordered);

        let mut scored: Vec<Recommendation> = Vec::new();

        for entity_id in &entity_ids_ordered {
            let entity = match self.graph.get_entity(*entity_id) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let recency = recency_map.get(entity_id).copied().unwrap_or(0.0);
            let novelty = novelty_map.get(entity_id).copied().unwrap_or(1.0);
            let confidence = entity.confidence;
            let pr = pr_scores.get(entity_id).copied().unwrap_or(0.0) / pr_max;

            let score = 0.3 * recency + 0.2 * novelty + 0.2 * confidence + 0.3 * pr;

            let reason = if pr > 0.5 {
                "Central to your knowledge graph".to_string()
            } else if recency > 0.7 {
                "Recently captured".to_string()
            } else if novelty > 0.7 {
                "New in your knowledge graph".to_string()
            } else {
                "From your recent activity".to_string()
            };

            scored.push(Recommendation {
                entity_id: entity_id.to_string(),
                entity_name: entity.name,
                entity_type: format!("{}", entity.entity_type),
                score,
                reason,
            });
        }

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    /// Check if text is relevant enough to auto-ingest (for capture daemon / clipboard).
    ///
    /// Returns true if the text is similar to recent queries (cosine > 0.35)
    /// or existing graph knowledge (cosine > 0.50).
    pub fn is_relevant_for_ingestion(&self, text: &str) -> bool {
        let emb = self.embedder.embed(text);

        // Check against recent query embeddings
        let max_query_sim = self.query_cache.embeddings.iter()
            .map(|q| cosine_sim(&emb, q))
            .fold(0.0f32, f32::max);

        if max_query_sim > 0.35 {
            return true;
        }

        // Check against graph vectors
        if let Ok(hits) = self.graph.search_vectors(&emb, 1) {
            if let Some((_, sim)) = hits.first() {
                if *sim > 0.50 {
                    return true;
                }
            }
        }

        false
    }

    /// Explicit user feedback (thumbs up/down) on the most recent query result.
    /// `score` should be in [0.0, 1.0]: 1.0 = positive, 0.0 = negative.
    ///
    /// MIA-inspired: also credits individual entities with success/failure
    /// to build value scores for composite retrieval ranking.
    pub fn explicit_feedback(&mut self, score: f64) {
        if let Some(pending) = self.pending_reward.take() {
            let reward = score.clamp(0.0, 1.0);
            self.bandit.register_reward(pending.arm, reward);
            self.bandit.save(&self.bandit_path);

            // Also update LinUCB with contextual reward
            self.linucb.register_reward(pending.arm, reward, &pending.context);
            self.linucb.save(&self.linucb_path);

            // Credit entities: if positive feedback, record success for all
            // entities that were in the result set (via last retrieval trace)
            if reward > 0.5 {
                if let Ok(recent) = self.trace_store.recent(1) {
                    if let Some(last) = recent.last() {
                        let _ = self.graph.record_success(&last.entities_extracted);
                    }
                }
            }

            info!(
                "[retrieval] explicit feedback: arm={} reward={:.2}",
                pending.arm, reward
            );
        }
    }

    /// Blend query embedding with session context (MIA: 80% query, 20% context).
    fn blend_with_context(&self, query_emb: &[f32]) -> Vec<f32> {
        if self.query_cache.embeddings.is_empty() {
            return query_emb.to_vec();
        }

        // Compute session context as centroid of recent query embeddings
        let dim = query_emb.len();
        let mut context = vec![0.0f32; dim];
        let n = self.query_cache.embeddings.len() as f32;
        for emb in &self.query_cache.embeddings {
            for (i, v) in emb.iter().enumerate() {
                if i < dim {
                    context[i] += v / n;
                }
            }
        }

        // Blend: 0.8 * query + 0.2 * context
        let mut blended = vec![0.0f32; dim];
        for i in 0..dim {
            blended[i] = 0.8 * query_emb[i] + 0.2 * context[i];
        }

        // Normalize
        let norm: f32 = blended.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut blended {
                *v /= norm;
            }
        }

        blended
    }

    /// Access the underlying graph store (for IPC commands that need it).
    pub fn graph(&self) -> &GraphStore {
        &self.graph
    }
}

/// Reciprocal Rank Aggregation (Graph-R1): parameter-free fusion of multiple ranked lists.
/// RRA_score(id) = Σ_lists 1/(k + rank_in_list(id) + 1)
/// k=60 is standard smoothing constant.
fn rra_fuse(ranked_lists: &[Vec<(Uuid, f64)>], k: f64) -> Vec<(Uuid, f64)> {
    let mut scores: HashMap<Uuid, f64> = HashMap::new();
    for list in ranked_lists {
        for (rank, (id, _)) in list.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (k + rank as f64 + 1.0);
        }
    }
    let mut results: Vec<_> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Apply diversity penalty to a ranked list of entities.
///
/// After initial ranking, penalize each entity by its maximum similarity
/// to any already-selected entity: `score -= λ * max_sim_to_selected`.
/// This is Maximal Marginal Relevance (MMR) without the full O(n²) reranking —
/// just a single pass greedy selection.
///
/// Returns reordered entity list with improved coverage / less redundancy.
fn diversify_entities(entities: &mut Vec<Entity>, graph: &GraphStore, lambda: f64) {
    if entities.len() < 3 || lambda <= 0.0 {
        return;
    }

    // Collect embeddings for all entities
    let embeddings: Vec<Option<Vec<f32>>> = entities.iter()
        .map(|e| graph.get_vector(e.id).ok().flatten())
        .collect();

    let mut selected: Vec<usize> = vec![0]; // always keep the top-ranked entity
    let mut remaining: Vec<usize> = (1..entities.len()).collect();

    while !remaining.is_empty() && selected.len() < entities.len() {
        let mut best_idx = 0;
        let mut best_score = f64::NEG_INFINITY;

        for (ri, &cand) in remaining.iter().enumerate() {
            // Max similarity to any already-selected entity
            let max_sim = selected.iter()
                .filter_map(|&si| {
                    match (&embeddings[cand], &embeddings[si]) {
                        (Some(a), Some(b)) => Some(cosine_sim(a, b) as f64),
                        _ => None,
                    }
                })
                .fold(0.0f64, f64::max);

            // Original rank score (higher = earlier in original ordering)
            let rank_score = 1.0 / (cand as f64 + 1.0);

            // MMR: relevance - λ * redundancy
            let mmr = rank_score - lambda * max_sim;

            if mmr > best_score {
                best_score = mmr;
                best_idx = ri;
            }
        }

        selected.push(remaining.remove(best_idx));
    }

    // Reorder entities according to selected order
    let reordered: Vec<Entity> = selected.into_iter()
        .map(|i| entities[i].clone())
        .collect();
    *entities = reordered;
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
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
        let linucb_path = dir.join("linucb.json");
        let mut engine = RetrievalEngine {
            graph: GraphStore::open(&db).unwrap(),
            trace_store: TraceStore::open(&traces).unwrap(),
            embedder: Embedder::new_hash(),
            bandit: UcbBandit::new(),
            bandit_path: bandit_path.clone(),
            linucb: LinUcbBandit::new(),
            linucb_path: linucb_path.clone(),
            reranker: None,
            query_cache: RecentQueryCache::new(10),
            pending_reward: None,
            planner: QueryPlanner::new(),
            procedure_store: None,
            trajectory_store: None,
        };

        let result = engine.query("hello world").unwrap();
        assert!(result.latency_ms < 5000);

        // With deferred rewards, the first query creates a pending reward.
        // A second query finalizes the first one's reward.
        let _result2 = engine.query("test query").unwrap();
        let stats = engine.bandit_stats();
        let total: u64 = stats.iter().map(|(c, _)| c).sum();
        assert_eq!(total, 1); // first query's reward now finalized

        // Finalize the second query's pending reward too
        engine.finalize_pending_reward();
        let stats2 = engine.bandit_stats();
        let total2: u64 = stats2.iter().map(|(c, _)| c).sum();
        assert_eq!(total2, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rra_fuse_ranks_correctly() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();

        // `a` appears at rank 0 in all 3 lists — should score highest
        // `b` appears at rank 1 in all 3 lists
        // `c` appears at rank 0 in only 1 list
        let list1 = vec![(a, 0.9), (b, 0.8)];
        let list2 = vec![(a, 0.7), (b, 0.6)];
        let list3 = vec![(a, 0.5), (b, 0.4), (c, 0.3)];

        let fused = rra_fuse(&[list1, list2, list3], 60.0);

        // `a` should be first (rank 0 in all 3 lists)
        assert_eq!(fused[0].0, a);

        // `b` should be second (rank 1 in all 3 lists)
        assert_eq!(fused[1].0, b);

        // `c` only in 1 list at rank 2 — should score lower than both a and b
        assert_eq!(fused[2].0, c);
        assert!(fused[0].1 > fused[2].1, "entity in all lists should score higher than entity in 1 list");

        // Verify scores: a = 3 * 1/(60+0+1) = 3/61 ≈ 0.04918
        let expected_a = 3.0 / 61.0;
        assert!((fused[0].1 - expected_a).abs() < 1e-10);

        // c = 1/(60+2+1) = 1/63 ≈ 0.01587
        let expected_c = 1.0 / 63.0;
        assert!((fused[2].1 - expected_c).abs() < 1e-10);
    }

    #[test]
    fn rra_fuse_empty_lists() {
        let result = rra_fuse(&[], 60.0);
        assert!(result.is_empty());

        let result2 = rra_fuse(&[vec![], vec![]], 60.0);
        assert!(result2.is_empty());
    }
}
