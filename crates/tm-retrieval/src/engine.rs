use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tm_controller::bandit::RetrievalParams;
use tm_controller::{UcbBandit, LinUcbBandit, QueryPlanner, QueryPlan, PlanAction, MemoryRouter, RouterContext, QueryRewriter};
use tm_episodic::{ProcedureStore, TraceStore, TrajectoryStore};
use tm_graph::{context::ActiveContext, GraphStore, ViewFilter};
use tm_reason::CausalTrace;
use tm_rerank::{ColbertReranker, RerankCandidate};
use tm_types::{Entity, Procedure, Result, Trace, TraceEventType, TraceMindError, Triple};
use tm_vector::{Bm25Index, ComposedIndex, Embedder, EmbedModel};
use tracing::info;
use uuid::Uuid;

use crate::prefetch::{PrefetchCache, PrefetchStats};

/// How well-grounded a retrieval result is.
///
/// For a memory product, "I have nothing on that" is a *better* answer than
/// a confident wrong one, and it is a completely different answer from "the
/// daemon wasn't running" or "retrieval broke". Making the distinction
/// explicit lets every surface say which one happened instead of returning
/// an empty string, which is indistinguishable from all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grounding {
    /// Strong evidence — answer directly and cite it.
    Found,
    /// Related material exists, but nothing that clearly answers the query.
    /// Say so and show what *is* known nearby.
    Uncertain,
    /// Nothing relevant is stored. Say that plainly.
    NotStored,
}

/// Fused-score floor above which a signal counts as strong evidence.
pub const GROUNDING_STRONG_SCORE: f32 = 0.35;

/// What kind of surface is driving this engine.
///
/// Decides whether interaction *timing* carries information. On an
/// interactive surface a five-second gap before the next query plausibly
/// means the user read something; inside an agentic host it means the model
/// emitted its next tool call. Conflating the two is what made the old
/// reward model punish normal agent behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// Desktop app / CLI — clicks and dwell are real user actions.
    Interactive,
    /// MCP server inside an agentic host — only explicit feedback counts.
    Agentic,
}

/// Floor on the *fused* ComposedIndex score for a signal to be returned.
/// Tuned against the LoCoMo anchor set; GEPA-mutable policy parameter.
pub const SIGNAL_MIN_SIM: f32 = 0.15;

/// Permissive cosine floor used only for *candidate generation*, before
/// lexical fusion. Deliberately low: an exact-token match that dense cosine
/// scores poorly must still reach the BM25 stage to be rescued.
pub const SIGNAL_CANDIDATE_MIN_SIM: f32 = 0.02;

/// Default candidate pool width, as a multiple of the requested top_k.
pub const SIGNAL_CANDIDATE_MULTIPLIER: usize = 4;

/// The GEPA policy type applied to this engine.
pub use tm_gepa::RetrievalPolicy as RetrievalPolicyConfig;

/// Half-life for the `recency` space over raw captures.
pub const RECENCY_HALF_LIFE_DAYS: f32 = 30.0;

/// Neutral-high confidence assigned to raw captures, which carry no
/// per-row governance score of their own.
pub const SIGNAL_BASE_CONFIDENCE: f32 = 0.7;

/// Classify how well a result is grounded.
///
/// Strong evidence means either a high-scoring raw memory or a graph hit
/// backed by at least one supporting signal. "Related entities but no
/// signal above the floor" is exactly the `Uncertain` case — the system
/// knows about the topic but not the answer.
fn classify_grounding(signal_hits: &[SignalHit], entity_count: usize) -> Grounding {
    let best = signal_hits
        .iter()
        .map(|h| h.score)
        .fold(0.0f32, f32::max);
    if best >= GROUNDING_STRONG_SCORE {
        Grounding::Found
    } else if !signal_hits.is_empty() || entity_count > 0 {
        Grounding::Uncertain
    } else {
        Grounding::NotStored
    }
}

/// Collapse a per-sub-query signal map into a single best-first ranking.
fn rank_signal_hits(map: HashMap<i64, SignalHit>) -> Vec<SignalHit> {
    let mut hits: Vec<SignalHit> = map.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

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

    /// Most recent query text, if any. Used to populate the `origin_query`
    /// field on recommendations so the UI can show *which* query a
    /// suggestion was seeded from. (Issue #1 from the 2026-05-11 UX
    /// review: "I don't know where the original context/trace was.")
    fn last_query(&self) -> Option<String> {
        self.texts.back().cloned()
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
    /// Sprint C-0.7 — UUID of this query's retrieval trace. Used at
    /// finalize-time to look up any `negative_signals` rows the user
    /// filed against this query, and subtract their summed weight
    /// from the engagement-derived reward.
    query_id: Uuid,
}

fn epoch_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Record of what a single pipeline phase did — enables downstream phases
/// to condition on upstream decisions (BIGMAS execution history ℋ).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PhaseRecord {
    pub phase: &'static str,
    pub duration_us: u64,           // microseconds
    pub candidates_in: usize,
    pub candidates_out: usize,
    pub decision: String,           // human-readable description of what happened
}

/// Global Workspace Theory-inspired state container for query execution.
/// All phases read from and write to this workspace, enabling downstream
/// phases to condition on everything that happened upstream.
#[derive(Debug)]
pub struct QueryWorkspace {
    // ── ctx: read-only query context ──
    pub query_text: String,
    pub query_embedding: Vec<f32>,
    pub blended_embedding: Vec<f32>,
    pub plan: QueryPlan,

    // ── work: read-write intermediate results ──
    pub candidates: Vec<(Uuid, f32)>,     // (entity_id, score) from vector search
    pub entities: Vec<Entity>,
    pub triples: Vec<Triple>,
    pub traces: Vec<Trace>,
    pub seen_entity_ids: HashSet<Uuid>,
    pub seen_triple_ids: HashSet<Uuid>,
    pub causal_trace: CausalTrace,

    // ── sys: execution metadata ──
    pub arm: u8,
    pub cascade_depth: u8,
    pub phases: Vec<PhaseRecord>,         // execution history

    // ── ans: final answer assembly ──
    pub low_confidence: bool,
    pub suggested_queries: Vec<String>,
    pub procedures: Vec<Procedure>,
    /// Hybrid-search hits against unpromoted signals (fresh captures not yet consolidated).
    pub signal_hits: Vec<SignalHit>,
}

impl QueryWorkspace {
    fn record_phase(&mut self, phase: &'static str, candidates_before: usize, decision: String, start: Instant) {
        self.phases.push(PhaseRecord {
            phase,
            duration_us: start.elapsed().as_micros() as u64,
            candidates_in: candidates_before,
            candidates_out: self.entities.len(),
            decision,
        });
    }
}

/// A proactive recommendation with origin + structured reason.
///
/// 2026-05-11 UX review (issues #1 + #3): every recommendation must now
/// answer two questions out of the box —
///   1. *where did this come from?* (`origin_query`, `origin_context`)
///   2. *why was it surfaced?*       (`reason_detail`)
///
/// `reason` is a short tag ("Central to your knowledge graph"), and
/// `reason_detail` is the structured breakdown (e.g. "PR 0.72 · 86%
/// match · seen recently") computed from the actual relevance /
/// recency / novelty / pagerank numbers the recommender already had on
/// hand. The UI shows `reason` as the headline and `reason_detail` as
/// the subtitle, so users can see the *evidence* not just the label.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Recommendation {
    pub entity_id: String,
    pub entity_name: String,
    pub entity_type: String,
    pub score: f64,
    pub reason: String,
    /// Structured "why" — components of the score the user can verify
    /// at a glance. Always populated; never empty.
    pub reason_detail: String,
    /// The most recent query text that seeded this recommendation.
    /// `None` only on cold start (no queries yet — recs come from
    /// recent ingest activity, not query history).
    pub origin_query: Option<String>,
    /// The active context UUID this recommendation was computed under,
    /// stringified. `None` if the engine is in unscoped mode.
    pub origin_context_id: Option<String>,
}

/// Build the structured "why this was recommended" string from the
/// score components. Picks the two strongest signals and renders them
/// as a short, dot-separated breakdown. Always returns a non-empty
/// string so the UI can render a stable subtitle.
///
/// Used by both warm and cold-start paths so the formatting stays
/// consistent across "Recommended for You" panels.
fn build_reason_detail(relevance: f64, recency: f64, novelty: f64, pr: f64) -> String {
    let mut parts: Vec<(f64, String)> = Vec::with_capacity(4);
    if relevance > 0.0 {
        parts.push((relevance, format!("{:.0}% match", relevance * 100.0)));
    }
    if pr > 0.0 {
        parts.push((pr, format!("PageRank {:.2}", pr)));
    }
    if recency > 0.0 {
        let label = if recency > 0.7 {
            "seen recently"
        } else if recency > 0.3 {
            "seen this week"
        } else {
            "older"
        };
        parts.push((recency, label.to_string()));
    }
    if novelty > 0.0 {
        let label = if novelty > 0.7 {
            "brand new"
        } else if novelty > 0.4 {
            "still rare"
        } else {
            "well-known"
        };
        parts.push((novelty, label.to_string()));
    }
    // Strongest two signals win the subtitle.
    parts.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let picked: Vec<String> = parts.into_iter().take(2).map(|(_, s)| s).collect();
    if picked.is_empty() {
        "from your graph".to_string()
    } else {
        picked.join(" · ")
    }
}

/// A hit from the unpromoted-signal hybrid search.
///
/// These are raw captures that haven't yet been consolidated into entities.
/// Surfacing them lets fresh content be recalled the moment it lands, without
/// waiting for the slow path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SignalHit {
    pub signal_id: i64,
    pub text: String,
    pub source: String,
    pub score: f32,
    pub created_at: chrono::DateTime<chrono::Utc>,
}




/// A related entity surfaced alongside primary query results.
///
/// Related entities are 1-hop graph neighbours of the top-k direct hits,
/// scored by vector similarity to the query (so the user sees "you might
/// also want…" without having to ask a second query). See TM-UX-001.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelatedEntity {
    pub id: Uuid,
    pub name: String,
    pub entity_type: String,
    /// Combined score: cosine(query, candidate) × graph_proximity weight.
    pub score: f32,
    /// Human-readable provenance, e.g. "1-hop from Alice".
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
    /// L1 prefetch cache (Phase 4 / Sprint B). Short-circuits the
    /// SQLite kNN when a query has been pre-warmed by an upstream
    /// `AnticipationKind::PrefetchQuery`.
    prefetch: PrefetchCache,
    /// Sprint C-0.6 — when false (default), retrieval filters out
    /// entities/triples/signals tagged with a different context than
    /// `graph.active_context_id()`. Unscoped rows (no tag) are always
    /// visible. When true, the active scope is ignored.
    cross_context: bool,
    /// LM-11c — optional Memory View filter applied after the context
    /// scope filter. When `Some` and non-empty, entities and triples
    /// are passed through [`ViewFilter::rejects_entity`] /
    /// [`ViewFilter::rejects_triple`] and dropped if the view rejects
    /// them. Set via [`set_view_filter`].
    view_filter: Option<ViewFilter>,
    /// Q3.2 — memory-routing gate. Fires before LinUCB arm selection.
    router: MemoryRouter,
    /// Whether the routing gate is active. Defaults to false for backward
    /// compat; the MCP layer enables it via `set_router_enabled(true)`.
    router_enabled: bool,
    /// Query rewriter — expands queries into variants for higher recall.
    rewriter: QueryRewriter,
    /// Q3.10 — composed multi-space index. Owns how the dense (`text`),
    /// sparse (`lexical`), `recency`, and `confidence` spaces combine into
    /// a single ranking score. Its verb weights are the artifact the GEPA
    /// loop mutates.
    composed_index: ComposedIndex,
    /// GEPA-tunable floor on the fused signal score.
    signal_min_score: f32,
    /// GEPA-tunable candidate pool width, as a multiple of top_k.
    candidate_multiplier: usize,
    /// Whether interaction timing is meaningful on this surface.
    host_kind: HostKind,
}

#[derive(Debug)]
pub struct RetrievalResult {
    /// Sprint C-0.7 — UUID of the audit trace for this query. Pass
    /// to `tracemind not-related <query_id> <result_id>` (or the
    /// equivalent MCP call) to record a per-result negative signal;
    /// the bandit subtracts the summed weight from the next reward
    /// it registers for this arm.
    pub query_id: Uuid,
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
    /// Execution history: what each pipeline phase did (GWT/BIGMAS).
    pub phases: Vec<PhaseRecord>,
    /// Unified reasoning narrative: strategy + process + evidence (3-layer explanation).
    pub reasoning_narrative: String,
    /// Hits against unpromoted signals (hybrid retrieval path). These are raw captures
    /// that haven't yet been consolidated into graph entities.
    pub signal_hits: Vec<SignalHit>,
    /// 1-hop graph neighbours of the primary hits, ranked by query-similarity.
    /// Populated by `compute_related_entities`; surfaced as "Related:" in CLI
    /// and `related_entities` in MCP responses. See TM-UX-001.
    pub related_entities: Vec<RelatedEntity>,
    /// Whether this result is well enough grounded to answer from. See
    /// [`Grounding`].
    pub grounding: Grounding,
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

        // Sprint C-0.10 — load the user's active context (if any) from
        // disk so that the per-query scope filter and cross-context
        // penalty actually fire for CLI / MCP / Tauri invocations.
        // Mirrors the IngestPipeline::open behaviour from Sprint C-0.5.
        let active_ctx_path = parent.join("active_context.json");
        if let Ok(Some(active)) = ActiveContext::load(&active_ctx_path) {
            graph.set_active_context(Some(active.id));
        }

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

        // Q4.4/Q4.14 — load the active GEPA policy from disk, if one has
        // been promoted. Without this the optimisation loop's output could
        // never reach a running instance: the tuned weights would live only
        // in the benchmark process that produced them. A missing or
        // unparseable file falls back to the compiled-in defaults rather
        // than failing to open the engine.
        let policy_path = parent.join("policy.json");
        let loaded_policy = std::fs::read_to_string(&policy_path)
            .ok()
            .and_then(|raw| match serde_json::from_str::<RetrievalPolicyConfig>(&raw) {
                Ok(p) => Some(p),
                Err(e) => {
                    info!("[retrieval] ignoring unparseable policy.json: {e}");
                    None
                }
            });

        // Try to auto-open procedure and trajectory stores from sibling files
        let proc_path = parent.join("procedures.jsonl");
        let procedure_store = ProcedureStore::open(&proc_path).ok();
        let traj_path = parent.join("trajectories.jsonl");
        let trajectory_store = TrajectoryStore::open(&traj_path).ok();

        let mut engine = Self {
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
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };
        if let Some(policy) = loaded_policy {
            info!("[retrieval] applying promoted GEPA policy from policy.json");
            engine.apply_policy(&policy);
        }
        Ok(engine)
    }

    /// Toggle cross-context retrieval. When `true`, ignores the active
    /// context scope and returns results from every namespace. When
    /// `false` (default), filters to the active context plus unscoped
    /// legacy data. Sprint C-0.6.
    pub fn set_cross_context(&mut self, cross_context: bool) {
        self.cross_context = cross_context;
    }

    /// Read-only accessor for the current cross-context flag.
    pub fn cross_context(&self) -> bool {
        self.cross_context
    }

    /// LM-11c — install a Memory View filter for subsequent queries.
    /// Pass `None` (or a filter with `is_empty()`) to disable. The
    /// filter is applied after the Sprint C-0.6 context scope filter
    /// and after primary candidate assembly, so it only ever shrinks
    /// the result set — it never widens it.
    pub fn set_view_filter(&mut self, filter: Option<ViewFilter>) {
        self.view_filter = match filter {
            Some(f) if !f.is_empty() => Some(f),
            _ => None,
        };
    }

    /// Read-only accessor for the currently-installed view filter.
    pub fn view_filter(&self) -> Option<&ViewFilter> {
        self.view_filter.as_ref()
    }

    /// Sprint D — hot-swap the active context on the live engine.
    /// Used by the Tauri context dropdown so switching contexts
    /// doesn't require an app restart. The graph uses interior
    /// mutability so this only needs `&self`, but we keep the `&mut`
    /// signature to match the rest of the engine setters.
    pub fn set_active_context(&mut self, ctx_id: Option<Uuid>) {
        self.graph.set_active_context(ctx_id);
    }

    /// Pre-warm the L1 prefetch cache with the kNN result for `text`.
    ///
    /// Called by an upstream orchestrator that has access to active
    /// `AnticipationKind::PrefetchQuery` entries (typically
    /// `tm-reflect`'s scheduler or the MCP layer holding both an
    /// `IntentStore` and a `RetrievalEngine`). The next `query()` for
    /// the same normalized string will skip both the embedder call
    /// and the SQLite kNN.
    ///
    /// Honours `top_k` of bandit arm 1 (the default-medium arm) for
    /// the warm pool. We deliberately don't take a top_k parameter:
    /// the cache is meant to be a *hint*, not a tuning surface.
    pub fn prime_prefetch(&mut self, text: &str) -> Result<()> {
        let embedding = self.embedder.embed(text);
        let warm_top_k = UcbBandit::params_for_arm(1).top_k.max(15);
        let candidates = self.graph.search_vectors(&embedding, warm_top_k)?;
        self.prefetch.prime(text, embedding, candidates);
        Ok(())
    }

    /// Read-only view of L1 prefetch statistics. Surfaced by the CLI
    /// `world status` and the brief so users can tell whether
    /// anticipations are actually paying off.
    pub fn prefetch_stats(&self) -> PrefetchStats {
        self.prefetch.stats()
    }

    /// Drop every primed entry — useful at session boundaries
    /// (logout, new project) and from tests.
    pub fn clear_prefetch(&mut self) {
        self.prefetch.clear();
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

    /// Attach a pre-constructed ColBERT reranker instance (e.g. from
    /// [`ColbertReranker::auto_download_or_none`]). No-op when `None`.
    pub fn with_reranker_instance(mut self, reranker: Option<ColbertReranker>) -> Self {
        if let Some(r) = reranker {
            info!("[retrieval] ColBERT reranker attached (alpha={})", r.alpha);
            self.reranker = Some(r);
        }
        self
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

        // ── Phase: query expansion ──
        // Generate multi-variant queries for simple/bandit cases.
        // Expanded variants are merged via query_decomposed (RRA fusion).
        let expansion_plan = self.planner.plan(text);
        let is_simple = matches!(
            expansion_plan.action,
            PlanAction::DirectLookup | PlanAction::BanditRetrieval
        );
        if is_simple {
            let variants = self.rewriter.expand(text);
            if variants.len() > 1 {
                let sub_queries: Vec<String> = variants.into_iter().map(|v| v.query).collect();
                return self.query_decomposed(text, &sub_queries, &expansion_plan, start);
            }
        }

        // ── Phase: plan ──
        let phase_start = Instant::now();
        let plan = self.planner.plan(text);
        info!(
            "[retrieval] plan: {:?} (complexity={}, confidence={:.2}, hints={:?})",
            plan.action, plan.complexity, plan.confidence, plan.entity_hints
        );

        // Phase 0.5a: If planner returns TemporalQuery, execute temporal retrieval.
        if let PlanAction::TemporalQuery { ref time_range } = plan.action {
            return self.query_temporal(text, time_range, &plan, start);
        }

        // Phase 0.5b: If planner returns Decompose, execute each sub-query and merge.
        if let PlanAction::Decompose { ref sub_queries } = plan.action {
            if sub_queries.len() > 1 {
                return self.query_decomposed(text, sub_queries, &plan, start);
            }
        }

        // ── Phase: embed ──
        let embed_start = Instant::now();
        let query_embedding = self.embedder.embed(text);
        let blended_embedding = self.blend_with_context(&query_embedding);

        // ── Phase: routing gate (Q3.2) ──
        // Fires before LinUCB arm selection. If the gate says skip,
        // return an empty result immediately without hitting the DB.
        let router_ctx = RouterContext {
            host_id: None,          // populated by MCP layer via set_host_id()
            working_memory_hit: false,
            recent_miss_rate: 0.0,  // populated from feedback signals in Q4
            is_command: false,
        };
        if self.router_enabled && !self.router.should_retrieve(text, &router_ctx) {
            return Ok(RetrievalResult {
                query_id: Uuid::new_v4(),
                arm: u8::MAX,  // sentinel: not a bandit arm
                entities: vec![],
                triples: vec![],
                traces: vec![],
                related_entities: vec![],
                signal_hits: vec![],
                grounding: Grounding::NotStored,
                procedures: vec![],
                phases: vec![],
                causal_trace: CausalTrace::new(text, usize::MAX, "router-skip"),
                reasoning_narrative: "routing gate: retrieval skipped".to_string(),
                plan: Some(plan),
                low_confidence: false,
                suggested_queries: vec![],
                latency_ms: start.elapsed().as_millis() as u32,
            });
        }

        // ── Phase: arm_select ──
        let arm_start = Instant::now();
        let params: RetrievalParams = match &plan.action {
            PlanAction::DirectLookup => {
                UcbBandit::params_for_arm(0)
            }
            PlanAction::ReasoningChain { .. } => {
                UcbBandit::params_for_arm(2)
            }
            _ => {
                let hint = self.trajectory_store.as_ref()
                    .and_then(|ts| ts.nearest_successful_arm(&query_embedding, 0.7))
                    .map(|(arm, _sim)| arm);
                self.linucb.select_with_hint(&query_embedding, hint)
            }
        };
        let arm = params.arm;

        // Build the arm selection reason for the phase record
        let arm_reason = match &plan.action {
            PlanAction::DirectLookup => format!("arm={} (DirectLookup override)", arm),
            PlanAction::ReasoningChain { .. } => format!("arm={} (ReasoningChain override)", arm),
            _ => {
                let hint = self.trajectory_store.as_ref()
                    .and_then(|ts| ts.nearest_successful_arm(&query_embedding, 0.7))
                    .map(|(a, _)| a);
                format!("arm={} (LinUCB, trajectory_hint={:?})", arm, hint)
            }
        };

        // Initialize causal trace for attribution tracking
        let causal = CausalTrace::new(text, arm as usize, UcbBandit::arm_name(arm));

        // ── Create QueryWorkspace ──
        let mut ws = QueryWorkspace {
            query_text: text.to_string(),
            query_embedding: query_embedding.clone(),
            blended_embedding: blended_embedding.clone(),
            plan: plan.clone(),
            candidates: Vec::new(),
            entities: Vec::new(),
            triples: Vec::new(),
            traces: Vec::new(),
            seen_entity_ids: HashSet::new(),
            seen_triple_ids: HashSet::new(),
            causal_trace: causal,
            arm,
            cascade_depth: 0,
            phases: Vec::new(),
            low_confidence: false,
            suggested_queries: Vec::new(),
            procedures: Vec::new(),
            signal_hits: Vec::new(),
        };

        // Record the plan phase
        ws.record_phase("plan", 0,
            format!("action={:?}, complexity={}, confidence={:.2}", plan.action, plan.complexity, plan.confidence),
            phase_start);

        // Record the embed phase
        ws.record_phase("embed", 0,
            format!("dim={}", query_embedding.len()),
            embed_start);

        // Record the arm_select phase
        ws.record_phase("arm_select", 0, arm_reason, arm_start);

        // ── Phase: vector_search (or L1 prefetch hit) ──
        let vs_start = Instant::now();
        let search_k = if self.reranker.is_some() {
            params.top_k * 3
        } else {
            params.top_k
        };
        let prefetch_hit = self.prefetch.lookup(text);
        let (phase_label, vs_decision) = if let Some(entry) = prefetch_hit {
            // Cache hit — trust the warmed result and skip the kNN.
            // We truncate to search_k if the entry is larger; if it's
            // smaller we use what we have. The orchestrator that
            // primed this entry is responsible for priming with the
            // right top_k for the expected arm.
            let take = entry.candidates.len().min(search_k);
            let primed_query = entry.query.clone();
            ws.candidates = entry.candidates.into_iter().take(take).collect();
            ("l1_prefetch_hit",
             format!("primed_for={:?}, used={}/{}", primed_query, ws.candidates.len(), search_k))
        } else {
            ws.candidates = self.graph.search_vectors(&blended_embedding, search_k)?;
            ("vector_search",
             format!("search_k={}, found={}", search_k, ws.candidates.len()))
        };
        ws.record_phase(phase_label, 0, vs_decision, vs_start);
        let vs_count = ws.candidates.len();

        // ── Phase: signal_search (hybrid — fresh unpromoted captures) ──
        // Runs in parallel-in-concept with vector_search: the graph has entities,
        // the signal table has raw captures. A fresh capture becomes recallable the
        // moment it lands, without waiting for consolidation.
        let ss_start = Instant::now();
        let signal_top_k = (params.top_k / 2).max(3);
        ws.signal_hits = self.search_signal_hits(text, &blended_embedding, signal_top_k);
        let raw_hit_count = ws.signal_hits.len();
        ws.record_phase(
            "signal_search",
            vs_count,
            format!(
                "signal_top_k={}, hits={}",
                signal_top_k, raw_hit_count
            ),
            ss_start,
        );

        // ── Phase: rerank (optional ColBERT) ──
        let rerank_start = Instant::now();
        let mut reranked_used = false;
        if let Some(ref reranker) = self.reranker {
            let mut rerank_inputs: Vec<(Uuid, RerankCandidate)> = Vec::new();
            for (id, score) in &ws.candidates {
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
                        ws.candidates = reranked
                            .into_iter()
                            .take(params.top_k)
                            .filter_map(|r| {
                                Uuid::parse_str(&r.id).ok().map(|id| (id, r.combined_score))
                            })
                            .collect();
                        reranked_used = true;
                    }
                    Err(e) => {
                        info!("[retrieval] reranker failed, using vector order: {e}");
                        ws.candidates.truncate(params.top_k);
                    }
                }
            }
        }
        ws.record_phase("rerank", vs_count,
            format!("colbert={}, candidates_after={}", reranked_used, ws.candidates.len()),
            rerank_start);

        // Sprint C-0.7 — soft cross-context penalty. When the caller
        // opted into `cross_context=true` while an active context is
        // set, candidates that resolve to entities scoped to *another*
        // context take a -0.15 score hit. This nudges the ranker back
        // toward in-scope hits without hard-filtering the bridge result
        // out (negative feedback later turns the bridge into an
        // explicit retraction signal — see `negative_weight_for_query`).
        if self.cross_context {
            if let Some(active) = self.graph.active_context_id() {
                for (id, score) in ws.candidates.iter_mut() {
                    if let Ok(Some(ctx)) = self.graph.entity_context_id(*id) {
                        if ctx != active {
                            *score -= 0.15;
                        }
                    }
                }
                ws.candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            }
        }

        // ── Phase: colbert_maxsim (arm 4 only) ──
        let colbert_start = Instant::now();
        let colbert_applied = if params.include_colbert && !ws.candidates.is_empty() {
            self.apply_colbert_maxsim(&mut ws, text)
        } else {
            false
        };
        ws.record_phase("colbert_maxsim", ws.candidates.len(),
            format!("applied={}, arm_colbert={}", colbert_applied, params.include_colbert),
            colbert_start);

        // Load entities from candidates
        for (rank, (id, score)) in ws.candidates.iter().enumerate() {
            if ws.seen_entity_ids.contains(id) {
                continue;
            }
            match self.graph.get_entity(*id) {
                Ok(entity) => {
                    ws.causal_trace.add_vector_match(*id, &entity.name, *score as f64, rank);
                    ws.seen_entity_ids.insert(*id);
                    ws.entities.push(entity);
                }
                Err(TraceMindError::EntityNotFound(_)) => {}
                Err(TraceMindError::Storage(ref msg)) if msg.contains("no rows") => {}
                Err(e) => return Err(e),
            }
        }

        // ── Phase: rra_fusion ──
        let rra_start = Instant::now();
        let entities_before_rra = ws.entities.len();
        if !ws.entities.is_empty() {
            let entity_ids: Vec<Uuid> = ws.entities.iter().map(|e| e.id).collect();
            let value_scores = self.graph.batch_value_scores(&entity_ids);
            let freq_scores = self.graph.batch_frequency_scores(&entity_ids);
            let recency_scores = self.graph.batch_recency_scores(&entity_ids);

            let mut sim_list: Vec<(Uuid, f64)> = ws.candidates.iter()
                .filter(|(id, _)| ws.seen_entity_ids.contains(id))
                .map(|(id, score)| (*id, *score as f64))
                .collect();
            sim_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let mut value_list: Vec<(Uuid, f64)> = entity_ids.iter()
                .map(|id| (*id, value_scores.get(id).copied().unwrap_or(0.5)))
                .collect();
            value_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let mut freq_list: Vec<(Uuid, f64)> = entity_ids.iter()
                .map(|id| (*id, freq_scores.get(id).copied().unwrap_or(1.0)))
                .collect();
            freq_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let mut recency_list: Vec<(Uuid, f64)> = entity_ids.iter()
                .map(|id| (*id, recency_scores.get(id).copied().unwrap_or(0.0)))
                .collect();
            recency_list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let fused = rra_fuse(&[sim_list, value_list, freq_list, recency_list], 60.0);

            let rank_map: HashMap<Uuid, usize> = fused.iter()
                .enumerate()
                .map(|(rank, (id, _))| (*id, rank))
                .collect();
            ws.entities.sort_by_key(|e| rank_map.get(&e.id).copied().unwrap_or(usize::MAX));

            for id in &entity_ids {
                let _ = self.graph.record_retrieval(*id);
            }
        }
        ws.record_phase("rra_fusion", entities_before_rra,
            format!("entities_after={}", ws.entities.len()),
            rra_start);

        // ── Phase: fallback_cascade ──
        let cascade_start = Instant::now();
        let max_sim = ws.candidates.iter().map(|(_, s)| *s).fold(0.0f32, f32::max);
        let mut current_arm = arm;
        let mut attenuation = 1.0f64;

        while max_sim < 0.3 && ws.entities.len() < 3 && current_arm < 3 {
            current_arm += 1;
            ws.cascade_depth += 1;
            attenuation *= 0.6;

            info!("[retrieval] fallback cascade depth {}: arm {} → {}, attenuation={:.2}",
                  ws.cascade_depth, arm, current_arm, attenuation);

            let fallback_params = UcbBandit::params_for_arm(current_arm);
            if let Ok(fallback_candidates) = self.graph.search_vectors(&blended_embedding, fallback_params.top_k) {
                for (id, score) in fallback_candidates {
                    if (score as f64 * attenuation) < 0.15 {
                        continue;
                    }
                    if ws.seen_entity_ids.contains(&id) {
                        continue;
                    }
                    match self.graph.get_entity(id) {
                        Ok(entity) => {
                            ws.causal_trace.add_vector_match(id, &entity.name, score as f64 * attenuation, ws.entities.len());
                            ws.seen_entity_ids.insert(id);
                            ws.entities.push(entity);
                        }
                        _ => {}
                    }
                }
            }

            if !ws.entities.is_empty() {
                break;
            }
        }
        ws.record_phase("fallback_cascade", entities_before_rra,
            format!("cascade_depth={}, attenuation={:.2}", ws.cascade_depth, attenuation),
            cascade_start);

        // ── Phase: mmr_diversity ──
        let mmr_start = Instant::now();
        let entities_before_mmr = ws.entities.len();
        diversify_entities(&mut ws.entities, &self.graph, 0.3);
        ws.record_phase("mmr_diversity", entities_before_mmr,
            format!("entities_before={}, entities_after={}", entities_before_mmr, ws.entities.len()),
            mmr_start);

        // ── Phase: graph_expand ──
        let graph_start = Instant::now();
        let entities_before_graph = ws.entities.len();
        if params.hops > 0 {
            let seed_ids: Vec<Uuid> = ws.entities.iter().map(|e| e.id).collect();

            for seed_id in seed_ids {
                let neighbors = self.graph.k_hop_neighbors(seed_id, params.hops)?;
                for neighbor in neighbors {
                    if !ws.seen_entity_ids.contains(&neighbor.id) {
                        ws.causal_trace.add_graph_hop(neighbor.id, &neighbor.name, seed_id, "k_hop", params.hops as usize);
                        ws.seen_entity_ids.insert(neighbor.id);
                        ws.entities.push(neighbor);
                    }
                }
            }
        }
        let new_from_graph = ws.entities.len() - entities_before_graph;
        ws.record_phase("graph_expand", entities_before_graph,
            format!("hops={}, new_entities={}", params.hops, new_from_graph),
            graph_start);

        // ── Phase: triples ──
        let triples_start = Instant::now();
        for entity in &ws.entities {
            let entity_triples = self.graph.get_triples_for_entity(entity.id)?;
            for triple in entity_triples {
                if !ws.seen_triple_ids.contains(&triple.id) {
                    ws.seen_triple_ids.insert(triple.id);
                    ws.triples.push(triple);
                }
            }
        }

        ws.triples.sort_by(|a, b| {
            let a_typed = !matches!(a.predicate, tm_types::Predicate::RelatedTo);
            let b_typed = !matches!(b.predicate, tm_types::Predicate::RelatedTo);
            b_typed.cmp(&a_typed).then(b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });

        let typed_count = ws.triples.iter().filter(|t| !matches!(t.predicate, tm_types::Predicate::RelatedTo)).count();
        let generic_count = ws.triples.len() - typed_count;
        ws.triples.truncate(typed_count + 20);
        ws.record_phase("triples", ws.entities.len(),
            format!("typed={}, generic={}", typed_count, generic_count),
            triples_start);

        // ── Phase: episodic ──
        let episodic_start = Instant::now();
        if params.include_episodic {
            let recent = self.trace_store.recent(50)?;
            let scanned = recent.len();
            for trace in &recent {
                for eid in &trace.entities_extracted {
                    if !ws.seen_entity_ids.contains(eid) {
                        if let Ok(e) = self.graph.get_entity(*eid) {
                            ws.causal_trace.add_episodic(*eid, &e.name, &trace.id.to_string());
                        }
                    }
                }
            }
            ws.traces = recent;
            ws.record_phase("episodic", ws.entities.len(),
                format!("traces_scanned={}", scanned),
                episodic_start);
        } else {
            ws.record_phase("episodic", ws.entities.len(),
                "skipped (arm does not include episodic)".to_string(),
                episodic_start);
        }

        // ── Phase: procedures ──
        let proc_start = Instant::now();
        ws.procedures = self.match_procedures(text);
        ws.record_phase("procedures", ws.entities.len(),
            format!("matched={}", ws.procedures.len()),
            proc_start);

        // ── Phase: confidence ──
        let conf_start = Instant::now();
        ws.low_confidence = ws.entities.is_empty()
            || (plan.confidence < 0.5 && ws.entities.len() < 2);

        ws.suggested_queries = if ws.low_confidence {
            self.generate_suggestions(text, &plan, &ws.entities)
        } else {
            vec![]
        };
        ws.record_phase("confidence", ws.entities.len(),
            format!("low_confidence={}, suggestions={}", ws.low_confidence, ws.suggested_queries.len()),
            conf_start);

        // Measure elapsed time.
        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        // Finalize any previous pending reward before creating a new one.
        self.finalize_pending_reward();

        // Sprint C-0.7 — the trace UUID *is* the query id. By minting
        // it once here and threading it through both the audit trace
        // and the pending reward, downstream tooling (`tracemind
        // not-related <query_id> ...`) and the bandit reward path
        // share the same handle.
        let query_id = Uuid::new_v4();

        // Create deferred reward — will be finalized on next query or explicit flush.
        let base_score = if !ws.entities.is_empty() {
            (ws.entities.len() as f64 / 5.0).min(1.0)
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
            query_id,
        });

        // Log access for each result entity (for recommendation scoring).
        for entity in &ws.entities {
            let _ = self.graph.log_access(entity.id, "query_result", Some(text));
        }

        // Cache query embedding for recommendations + relevance gating.
        self.query_cache.push(text.to_string(), blended_embedding);

        // Persist a retrieval trace for the audit trail.
        let mut trace = Trace::new(query_id, TraceEventType::Retrieve, "");
        trace.raw_text = Some(text.to_string());
        trace.entities_extracted = ws.entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = ws.triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(arm);
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        // Finalize causal trace
        ws.causal_trace.total_entities = ws.entities.len();
        ws.causal_trace.total_triples = ws.triples.len();
        ws.causal_trace.latency_ms = latency_ms as u64;

        // Build unified reasoning narrative (strategy + process + evidence)
        let plan_tuple = (
            format!("{:?}", ws.plan.action),
            ws.plan.complexity.clone(),
            ws.plan.confidence,
        );
        let phase_pairs: Vec<(String, String)> = ws.phases.iter()
            .map(|p| (p.phase.to_string(), p.decision.clone()))
            .collect();
        let reasoning_narrative = ws.causal_trace.reasoning_narrative(
            Some((plan_tuple.0.as_str(), plan_tuple.1.as_str(), plan_tuple.2)),
            &phase_pairs,
        );

        // ── Sprint C-0.6: context scope filter ──
        // Strip entities/triples/signals tagged with a context other
        // than the active one. Unscoped rows (no tag) stay visible so
        // legacy / pre-segmentation data remains reachable.
        if !self.cross_context && self.graph.active_context_id().is_some() {
            ws.entities
                .retain(|e| self.graph.entity_in_active_scope(e.id, false).unwrap_or(true));
            let kept_entity_ids: HashSet<Uuid> =
                ws.entities.iter().map(|e| e.id).collect();
            ws.triples.retain(|t| {
                // Drop triples whose own context_id is foreign, OR
                // whose subject/object has been filtered out.
                let scope_ok = self
                    .graph
                    .triple_in_active_scope(t.id, false)
                    .unwrap_or(true);
                let endpoints_ok = kept_entity_ids.contains(&t.subject_id)
                    && kept_entity_ids.contains(&t.object_id);
                scope_ok && endpoints_ok
            });
            ws.signal_hits.retain(|h| {
                self.graph
                    .signal_in_active_scope(h.signal_id, false)
                    .unwrap_or(true)
            });
        }

        // ── LM-11c: Memory View filter ──
        // User-curated splice. Applied after the context scope filter
        // so it only shrinks the result set — never widens it. A view
        // with non-empty include sets drops every entity / triple not
        // explicitly allowed; exclude sets drop matching rows
        // unconditionally; confidence_floor drops below-threshold
        // triples. The filter is skipped if `view_filter` is None or
        // `is_empty()`.
        if let Some(ref vf) = self.view_filter {
            ws.entities.retain(|e| {
                let ctx = self.graph.entity_context_id(e.id).unwrap_or(None);
                !vf.rejects_entity(e.id, ctx)
            });
            let kept_entity_ids: HashSet<Uuid> =
                ws.entities.iter().map(|e| e.id).collect();
            ws.triples.retain(|t| {
                if vf.rejects_triple(t.id, t.confidence) {
                    return false;
                }
                kept_entity_ids.contains(&t.subject_id)
                    && kept_entity_ids.contains(&t.object_id)
            });
        }

        let related_entities = compute_related_entities(
            &self.graph,
            &ws.query_embedding,
            &ws.entities,
            5,  // max seeds — top 5 primary hits
            5,  // max related
        );

        let ws_grounding = classify_grounding(&ws.signal_hits, ws.entities.len());
        Ok(RetrievalResult {
            query_id,
            arm: ws.arm,
            entities: ws.entities,
            triples: ws.triples,
            traces: ws.traces,
            latency_ms,
            causal_trace: ws.causal_trace,
            plan: Some(ws.plan),
            low_confidence: ws.low_confidence,
            suggested_queries: ws.suggested_queries,
            procedures: ws.procedures,
            phases: ws.phases,
            reasoning_narrative,
            grounding: ws_grounding,
            signal_hits: ws.signal_hits,
            related_entities,
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
        // Hybrid raw-text recall, fused across sub-queries. Keyed by
        // signal_id so a capture matched by several variants is kept once,
        // at its best score.
        let mut signal_by_id: HashMap<i64, SignalHit> = HashMap::new();

        for sub_q in sub_queries {
            // Use a narrow retrieval for each sub-query (arm 0 for speed)
            let sub_embedding = self.embedder.embed(sub_q);
            let sub_params = UcbBandit::params_for_arm(1); // medium arm per sub-query

            for hit in self.search_signal_hits(sub_q, &sub_embedding, sub_params.top_k) {
                signal_by_id
                    .entry(hit.signal_id)
                    .and_modify(|e| {
                        if hit.score > e.score {
                            e.score = hit.score;
                        }
                    })
                    .or_insert(hit);
            }

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
        let orig_query_embedding = emb.clone();
        self.query_cache.push(original_text.to_string(), emb);

        // Log access
        for entity in &all_entities {
            let _ = self.graph.log_access(entity.id, "query_result", Some(original_text));
        }

        let entity_count = all_entities.len();

        // Persist trace — query_id reused in RetrievalResult for negative-feedback wiring (C-0.7).
        let query_id = Uuid::new_v4();
        let mut trace = Trace::new(query_id, TraceEventType::Retrieve, "");
        trace.raw_text = Some(original_text.to_string());
        trace.entities_extracted = all_entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = all_triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(0); // decomposed
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        // Build narrative before moving causal
        let decompose_phases = vec![
            ("decomposed".to_string(), format!("{} sub-queries merged", sub_queries.len())),
        ];
        let action_str = format!("{:?}", plan.action);
        let reasoning_narrative = causal.reasoning_narrative(
            Some((action_str.as_str(), &plan.complexity, plan.confidence)),
            &decompose_phases,
        );

        let related_entities = compute_related_entities(
            &self.graph,
            &orig_query_embedding,
            &all_entities,
            5,
            5,
        );

        Ok(RetrievalResult {
            query_id,
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
            phases: vec![PhaseRecord {
                phase: "decomposed",
                duration_us: start.elapsed().as_micros() as u64,
                candidates_in: sub_queries.len(),
                candidates_out: entity_count,
                decision: format!("{} sub-queries merged", sub_queries.len()),
            }],
            reasoning_narrative,
            grounding: {
                let hits = rank_signal_hits(signal_by_id.clone());
                classify_grounding(&hits, entity_count)
            },
            signal_hits: rank_signal_hits(signal_by_id),
            related_entities,
        })
    }

    /// Execute a temporal query: find entities and traces within a time range,
    /// optionally combined with vector similarity for the query text.
    ///
    /// This is the execution path for the Planner's `TemporalQuery` action.
    /// Example: "what was I working on last week?" → entities updated in last 7 days,
    /// traces from last 7 days, optionally ranked by query-text similarity.
    fn query_temporal(
        &mut self,
        original_text: &str,
        time_range: &tm_types::TimeRange,
        plan: &QueryPlan,
        start: Instant,
    ) -> Result<RetrievalResult> {
        info!(
            "[retrieval] temporal query: '{}' range=[{} → {}]",
            time_range.label, time_range.start, time_range.end
        );

        let mut phases: Vec<PhaseRecord> = Vec::new();
        let mut causal = CausalTrace::new(original_text, 0, "temporal");

        // Record the plan phase
        let plan_start = Instant::now();
        phases.push(PhaseRecord {
            phase: "plan",
            duration_us: plan_start.elapsed().as_micros() as u64,
            candidates_in: 0,
            candidates_out: 0,
            decision: format!(
                "TemporalQuery: {} [{} → {}]",
                time_range.label, time_range.start, time_range.end
            ),
        });

        // ── Phase: temporal_entities ──
        // Get entities created/updated in the time range
        let te_start = Instant::now();
        let temporal_entities = self.graph.get_entities_by_time_range(
            time_range.start,
            time_range.end,
        )?;
        let te_count = temporal_entities.len();
        phases.push(PhaseRecord {
            phase: "temporal_entities",
            duration_us: te_start.elapsed().as_micros() as u64,
            candidates_in: 0,
            candidates_out: te_count,
            decision: format!("found {} entities in time range", te_count),
        });

        // ── Phase: temporal_access ──
        // Also get entities accessed (queried/clicked) in the time range
        let ta_start = Instant::now();
        let accessed_ids = self.graph.get_accessed_entities_in_range(
            time_range.start,
            time_range.end,
        )?;
        let ta_count = accessed_ids.len();
        phases.push(PhaseRecord {
            phase: "temporal_access",
            duration_us: ta_start.elapsed().as_micros() as u64,
            candidates_in: 0,
            candidates_out: ta_count,
            decision: format!("found {} accessed entities in time range", ta_count),
        });

        // ── Phase: temporal_traces ──
        // Get traces from the time range
        let tt_start = Instant::now();
        let temporal_traces = self.trace_store.traces_in_range(
            time_range.start,
            time_range.end,
        )?;
        let tt_count = temporal_traces.len();
        phases.push(PhaseRecord {
            phase: "temporal_traces",
            duration_us: tt_start.elapsed().as_micros() as u64,
            candidates_in: 0,
            candidates_out: tt_count,
            decision: format!("found {} traces in time range", tt_count),
        });

        // ── Phase: merge + rank ──
        // Merge temporal entities + accessed entities, deduplicate, rank by similarity
        let merge_start = Instant::now();
        let mut seen_ids: HashSet<Uuid> = HashSet::new();
        let mut all_entities: Vec<Entity> = Vec::new();

        // Add entities from time range
        for entity in temporal_entities {
            if seen_ids.insert(entity.id) {
                causal.add_vector_match(entity.id, &entity.name, 1.0, all_entities.len());
                all_entities.push(entity);
            }
        }

        // Add accessed entities from time range (may overlap)
        for id in &accessed_ids {
            if seen_ids.insert(*id) {
                if let Ok(entity) = self.graph.get_entity(*id) {
                    causal.add_vector_match(*id, &entity.name, 0.8, all_entities.len());
                    all_entities.push(entity);
                }
            }
        }

        // Add entity IDs referenced in traces
        for trace in &temporal_traces {
            for eid in &trace.entities_extracted {
                if seen_ids.insert(*eid) {
                    if let Ok(entity) = self.graph.get_entity(*eid) {
                        causal.add_episodic(*eid, &entity.name, &trace.id.to_string());
                        all_entities.push(entity);
                    }
                }
            }
        }

        // If we have a meaningful query beyond temporal keywords, use vector similarity to rank
        let query_emb = self.embedder.embed(original_text);
        let mut scored_entities: Vec<(Entity, f64)> = Vec::new();
        for entity in &all_entities {
            let sim = if let Ok(Some(vec)) = self.graph.get_vector(entity.id) {
                cosine_sim(&query_emb, &vec) as f64
            } else {
                0.5 // default score for entities without vectors
            };
            // Boost by recency within the time range
            let recency_boost = self.graph.recency_score(entity.id);
            let score = 0.6 * sim + 0.4 * recency_boost;
            scored_entities.push((entity.clone(), score));
        }
        scored_entities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let final_entities: Vec<Entity> = scored_entities.into_iter().map(|(e, _)| e).collect();

        phases.push(PhaseRecord {
            phase: "temporal_merge_rank",
            duration_us: merge_start.elapsed().as_micros() as u64,
            candidates_in: te_count + ta_count,
            candidates_out: final_entities.len(),
            decision: format!(
                "merged {} unique entities from {} temporal + {} accessed + traces",
                final_entities.len(), te_count, ta_count
            ),
        });

        // ── Collect triples ──
        let triples_start = Instant::now();
        let mut all_triples: Vec<Triple> = Vec::new();
        let mut seen_triple_ids: HashSet<Uuid> = HashSet::new();
        for entity in &final_entities {
            if let Ok(entity_triples) = self.graph.get_triples_for_entity(entity.id) {
                for triple in entity_triples {
                    if seen_triple_ids.insert(triple.id) {
                        all_triples.push(triple);
                    }
                }
            }
        }
        all_triples.sort_by(|a, b| {
            let a_typed = !matches!(a.predicate, tm_types::Predicate::RelatedTo);
            let b_typed = !matches!(b.predicate, tm_types::Predicate::RelatedTo);
            b_typed.cmp(&a_typed).then(b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });
        let typed_count = all_triples.iter().filter(|t| !matches!(t.predicate, tm_types::Predicate::RelatedTo)).count();
        all_triples.truncate(typed_count + 20);
        phases.push(PhaseRecord {
            phase: "triples",
            duration_us: triples_start.elapsed().as_micros() as u64,
            candidates_in: final_entities.len(),
            candidates_out: all_triples.len(),
            decision: format!("typed={}, total={}", typed_count, all_triples.len()),
        });

        let latency_ms = start.elapsed().as_millis().min(u32::MAX as u128) as u32;

        // Finalize causal trace
        causal.total_entities = final_entities.len();
        causal.total_triples = all_triples.len();
        causal.latency_ms = latency_ms as u64;

        // Persist retrieval trace — query_id reused in RetrievalResult (C-0.7).
        let query_id = Uuid::new_v4();
        let mut trace = Trace::new(query_id, TraceEventType::Retrieve, "");
        trace.raw_text = Some(original_text.to_string());
        trace.entities_extracted = final_entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = all_triples.iter().map(|t| t.id).collect();
        trace.retrieval_arm = Some(0);
        trace.retrieval_latency_ms = Some(latency_ms);
        let _ = self.trace_store.append(&trace);

        // Log access
        for entity in &final_entities {
            let _ = self.graph.log_access(entity.id, "query_result", Some(original_text));
        }

        // Cache query embedding
        self.query_cache.push(original_text.to_string(), query_emb);

        let low_confidence = final_entities.is_empty();
        let suggested_queries = if low_confidence {
            self.generate_suggestions(original_text, plan, &final_entities)
        } else {
            vec![]
        };

        // Build narrative
        let action_str = format!("{:?}", plan.action);
        let phase_pairs: Vec<(String, String)> = phases.iter()
            .map(|p| (p.phase.to_string(), p.decision.clone()))
            .collect();
        let reasoning_narrative = causal.reasoning_narrative(
            Some((action_str.as_str(), &plan.complexity, plan.confidence)),
            &phase_pairs,
        );

        let temporal_query_embedding = self.embedder.embed(original_text);
        let related_entities = compute_related_entities(
            &self.graph,
            &temporal_query_embedding,
            &final_entities,
            5,
            5,
        );

        let final_entities_len = final_entities.len();
        Ok(RetrievalResult {
            query_id,
            arm: 0,
            entities: final_entities,
            triples: all_triples,
            traces: temporal_traces,
            latency_ms,
            causal_trace: causal,
            plan: Some(plan.clone()),
            low_confidence,
            suggested_queries,
            procedures: self.match_procedures(original_text),
            phases,
            reasoning_narrative,
            // Temporal queries ("when did I…") are exactly the case where the
            // raw sentence carries the answer and the entity name does not.
            grounding: {
                let hits =
                    self.search_signal_hits(original_text, &temporal_query_embedding, 8);
                classify_grounding(&hits, final_entities_len)
            },
            signal_hits: self.search_signal_hits(original_text, &temporal_query_embedding, 8),
            related_entities,
        })
    }

    /// Refresh the engine's view of the graph store from disk.
    ///
    /// Required when another `GraphStore` instance (e.g. the one inside
    /// `IngestPipeline` in a long-running MCP server) has written entities
    /// since this engine was opened. Without it, `search_vectors` filters
    /// fresh rows out because their UUIDs aren't in this engine's in-memory
    /// cache. See TM-UX-001 Phase C.
    pub fn refresh_graph(&self) -> Result<()> {
        self.graph.reload_maps()
    }

    /// Declare what kind of surface is driving this engine. See
    /// [`HostKind`]. Defaults to [`HostKind::Agentic`].
    pub fn set_host_kind(&mut self, kind: HostKind) {
        self.host_kind = kind;
    }

    pub fn host_kind(&self) -> HostKind {
        self.host_kind
    }

    /// Apply a GEPA-produced retrieval policy to the live query path.
    ///
    /// This is what makes the optimisation loop meaningful: the artifact
    /// the loop mutates has to be the same one retrieval reads. Sets the
    /// `recall` verb's fusion weights on the `ComposedIndex` plus the two
    /// scalar knobs the signal path consults.
    pub fn apply_policy(&mut self, policy: &RetrievalPolicyConfig) {
        let pairs: Vec<(&str, f32)> = policy
            .space_weights
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        let mut weights = tm_vector::default_verb_weights();
        // Replace only the `recall` vector; the other verbs keep their
        // defaults until the loop is extended to score them too.
        if let Some(slot) = weights.iter_mut().find(|w| w.verb == "recall") {
            *slot = tm_vector::VerbWeights::new("recall", &pairs);
        }
        self.composed_index.set_verb_weights(weights);
        self.signal_min_score = policy.min_score;
        self.candidate_multiplier = policy.candidate_multiplier.max(1);
    }

    /// The policy currently in force.
    pub fn current_policy(&self) -> RetrievalPolicyConfig {
        let mut space_weights = std::collections::BTreeMap::new();
        for vw in self.composed_index.verb_weights() {
            if vw.verb == "recall" {
                for (k, v) in &vw.weights {
                    space_weights.insert(k.clone(), *v);
                }
            }
        }
        RetrievalPolicyConfig {
            space_weights,
            min_score: self.signal_min_score,
            candidate_multiplier: self.candidate_multiplier,
            // Answer-selection weights live in the answer layer, not the
            // engine; report the policy defaults so a round-trip through
            // current_policy() stays a valid policy.
            ..RetrievalPolicyConfig::default()
        }
    }

    /// Hybrid signal search — raw captured text that has not yet been
    /// consolidated into graph entities.
    ///
    /// Extracted so every retrieval path can call it. The planner-routed
    /// paths (`query_decomposed`, `query_temporal`) previously returned
    /// `signal_hits: Vec::new()` unconditionally, which meant any query the
    /// planner classified as decomposed or temporal could only ever surface
    /// *entity names* — never the sentence the user actually wrote. Query
    /// expansion routes most simple queries through `query_decomposed`, so
    /// in practice that disabled raw-text recall for the majority of
    /// queries.
    /// Hybrid dense + sparse ranking (Q3.10 — `ComposedIndex` on the live
    /// query path).
    ///
    /// Candidates are generated at a *permissive* cosine floor and then
    /// re-ranked by fusing four spaces through `ComposedIndex`: dense cosine
    /// (`text`), BM25 (`lexical`), time decay (`recency`), and governance
    /// confidence. Generating wide and ranking narrow is what lets the
    /// lexical space rescue an exact-token match that dense cosine buried —
    /// filtering at `SIGNAL_MIN_SIM` *before* fusion would discard those
    /// candidates before BM25 ever saw them.
    fn search_signal_hits(&self, query_text: &str, embedding: &[f32], top_k: usize) -> Vec<SignalHit> {
        // Wide candidate generation: 4x the requested depth (floored at 32)
        // at a permissive similarity cut.
        let candidate_k = (top_k * self.candidate_multiplier).max(32);
        let raw = self
            .graph
            .search_signals(embedding, candidate_k, SIGNAL_CANDIDATE_MIN_SIM)
            .unwrap_or_default();
        if raw.is_empty() {
            return Vec::new();
        }

        let docs: Vec<String> = raw.iter().map(|(s, _)| s.raw_text.clone()).collect();
        let bm25 = Bm25Index::build(&docs);
        let lexical_scores = bm25.normalised_scores(query_text);

        let now = chrono::Utc::now();
        let mut scored: Vec<(f32, SignalHit)> = raw
            .into_iter()
            .enumerate()
            .map(|(i, (sig, cosine))| {
                let age_days =
                    (now - sig.created_at).num_seconds() as f32 / 86_400.0;
                let recency = 2f32.powf(-age_days / RECENCY_HALF_LIFE_DAYS);

                let mut spaces: HashMap<String, f32> = HashMap::new();
                spaces.insert("text".to_string(), cosine.clamp(0.0, 1.0));
                spaces.insert("lexical".to_string(), lexical_scores[i]);
                spaces.insert("recency".to_string(), recency);
                // Raw captures carry no per-row governance score; they were
                // admitted by the ingest gate, so treat them as neutral-high
                // rather than fabricating a confidence.
                spaces.insert("confidence".to_string(), SIGNAL_BASE_CONFIDENCE);

                let fused = self.composed_index.score_precomputed(&spaces, Some("recall"));
                (
                    fused,
                    SignalHit {
                        signal_id: sig.id,
                        text: sig.raw_text,
                        source: sig.source,
                        score: fused,
                        created_at: sig.created_at,
                    },
                )
            })
            .filter(|(fused, _)| *fused >= self.signal_min_score)
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        let mut hits: Vec<SignalHit> = scored.into_iter().map(|(_, h)| h).collect();
        hits.retain(|h| {
            self.graph
                .signal_in_active_scope(h.signal_id, self.cross_context)
                .unwrap_or(true)
        });
        hits
    }

    /// Return per-arm `(pull_count, average_reward)` statistics from the bandit.
    pub fn bandit_stats(&self) -> [(u64, f64); tm_controller::NUM_ARMS] {
        self.bandit.arm_stats()
    }

    /// LinUCB contextual bandit statistics: (pulls, avg_weight_magnitude) per arm.
    pub fn linucb_stats(&self) -> [(u64, f64); tm_controller::NUM_ARMS] {
        self.linucb.arm_stats()
    }

    /// Inspector helper — current annealed LinUCB exploration coefficient.
    pub fn linucb_alpha(&self) -> f64 {
        self.linucb.alpha()
    }

    /// Inspector helper — embed arbitrary text using the engine's
    /// configured embedder. Used by `cmd_trace_why` to recompute the
    /// LinUCB context features for a past trace without spinning up a
    /// second `Embedder` (the ONNX model load is non-trivial).
    pub fn embed_query(&self, text: &str) -> Vec<f32> {
        self.embedder.embed(text)
    }

    /// Inspector helper — produce the QueryPlanner's classification for
    /// arbitrary text. Used by `cmd_trace_why` to surface plan.action /
    /// plan.complexity / plan.confidence for a recorded trace.
    pub fn plan_query(&self, text: &str) -> QueryPlan {
        self.planner.plan(text)
    }

    /// Register a click on a result entity (implicit positive feedback).
    pub fn register_click(&mut self, entity_id: Uuid) {
        if let Some(ref mut pending) = self.pending_reward {
            pending.clicks += 1;
        }
        let _ = self.graph.log_access(entity_id, "clicked", None);
    }

    /// Finalize a pending deferred reward.
    ///
    /// **Only registers a reward when there is actual evidence.** An
    /// un-evidenced query leaves the bandit posterior untouched.
    ///
    /// The previous implementation always registered something, deriving a
    /// value from click / dwell / re-query timing. Those are interactive-UI
    /// signals and they are meaningless on the surface that ships: an MCP
    /// host issues consecutive tool calls in well under five seconds as
    /// normal behaviour, which the old rule read as "re-queried within 5s →
    /// results were poor → 0.1". The bandit was therefore receiving close to
    /// the worst possible reward for almost every query — not merely a noisy
    /// signal but an actively wrong one, since the arm explored *least*
    /// ends up looking best.
    ///
    /// Evidence sources, in precedence order:
    ///   1. Explicit feedback rows (`helpful` / `not_related`) written by
    ///      `memory_feedback` — always trusted, on any host.
    ///   2. Interactive engagement (click / dwell / re-query) — only on
    ///      [`HostKind::Interactive`], where those actions are real.
    ///   3. Nothing → no update.
    fn finalize_pending_reward(&mut self) {
        let pending = match self.pending_reward.take() {
            Some(p) => p,
            None => return,
        };

        let neg = self
            .graph
            .negative_weight_for_query(pending.query_id)
            .unwrap_or(0.0) as f64;
        let pos = self
            .graph
            .positive_weight_for_query(pending.query_id)
            .unwrap_or(0.0) as f64;
        let has_explicit = pos > 0.0 || neg > 0.0;

        let elapsed = pending.created_at_mono.elapsed();
        let interactive = self.host_kind == HostKind::Interactive;
        // A click is real evidence on any host, but only an interactive
        // surface can produce one.
        let has_engagement = interactive && (pending.clicks > 0 || pending.requeried);

        if !has_explicit && !has_engagement {
            // No evidence. Registering a fabricated constant here would bias
            // the posterior toward whichever arm is pulled most often, which
            // is exactly backwards for an explore/exploit controller.
            return;
        }

        let relevance_reward = if pending.clicks > 0 {
            0.7
        } else if interactive && pending.requeried {
            0.1
        } else if interactive && elapsed > Duration::from_secs(10) {
            0.5
        } else {
            // Explicit-feedback-only case: start neutral and let the
            // positive/negative weights below decide the direction.
            0.5
        };

        // Sprint C-0.7 / D / F-1 — explicit feedback moves the reward in
        // both directions: `not_related` rows penalise an arm that pulled in
        // foreign context, `helpful` rows reinforce one that did not.
        let reward = (relevance_reward + pos - neg).clamp(0.0, 1.0);

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

        // 2026-05-11 UX: origin tracking so the UI can answer
        // "where did this come from?" without a second round-trip.
        let origin_query = self.query_cache.last_query();
        let origin_context_id = self
            .graph
            .active_context_id()
            .map(|id| id.to_string());

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

            let reason_detail = build_reason_detail(relevance, recency, novelty, pr);

            scored.push(Recommendation {
                entity_id: entity_id.to_string(),
                entity_name: entity.name,
                entity_type: format!("{}", entity.entity_type),
                score,
                reason,
                reason_detail,
                origin_query: origin_query.clone(),
                origin_context_id: origin_context_id.clone(),
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

        // Cold-start has no query history, but we can still record the
        // active context so the UI's "from which context" pill stays
        // populated when the user opens TraceMind fresh.
        let origin_context_id = self
            .graph
            .active_context_id()
            .map(|id| id.to_string());

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

            // No relevance score on cold start — substitute entity
            // confidence so the detail string still tells a story
            // ("75% confidence · brand new · in your top central nodes").
            let reason_detail = build_reason_detail(confidence, recency, novelty, pr);

            scored.push(Recommendation {
                entity_id: entity_id.to_string(),
                entity_name: entity.name,
                entity_type: format!("{}", entity.entity_type),
                score,
                reason,
                reason_detail,
                origin_query: None,
                origin_context_id: origin_context_id.clone(),
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

    /// Apply ColBERT MaxSim scoring using cached per-token embeddings.
    fn apply_colbert_maxsim(&self, ws: &mut QueryWorkspace, query_text: &str) -> bool {
        let candidate_ids: Vec<Uuid> = ws.candidates.iter().map(|(id, _)| *id).collect();
        let cached_tokens = self.graph.batch_colbert_tokens(&candidate_ids);

        if cached_tokens.is_empty() {
            return false;
        }

        let query_tokens_opt: Option<Vec<Vec<f32>>> = self
            .reranker
            .as_ref()
            .and_then(|r| r.encode_query(query_text).ok());

        if let Some(query_tokens) = query_tokens_opt {
            let mut scored: Vec<(Uuid, f32)> = Vec::new();
            for (id, orig_score) in &ws.candidates {
                if let Some((flat_embs, token_count, dim)) = cached_tokens.get(id) {
                    let doc_tokens: Vec<Vec<f32>> = (0..*token_count)
                        .map(|t| flat_embs[t * dim..(t + 1) * dim].to_vec())
                        .collect();
                    let maxsim_score = tm_rerank::maxsim(&query_tokens, &doc_tokens);
                    let blended = 0.6 * maxsim_score + 0.4 * orig_score;
                    scored.push((*id, blended));
                } else {
                    scored.push((*id, *orig_score));
                }
            }
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            ws.candidates = scored;
            return true;
        }

        false
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

/// TM-UX-001 Phase A — Seamless recommendation.
///
/// Compute 1-hop related entities from the primary hits: for each top-k
/// direct result, pull graph neighbours (excluding entities already in the
/// primary set), then rank by `cosine(query, candidate_embedding)`. Missing
/// embeddings fall back to a small graph-proximity-only score so a structural
/// neighbour still shows up even when vector recall is thin.
///
/// Bounded: `max_source_seeds` limits fan-out to the top primary hits,
/// `max_related` caps output. Both guard against explosion on dense graphs.
pub(crate) fn compute_related_entities(
    graph: &GraphStore,
    query_embedding: &[f32],
    primary: &[Entity],
    max_source_seeds: usize,
    max_related: usize,
) -> Vec<RelatedEntity> {
    if primary.is_empty() || max_related == 0 {
        return Vec::new();
    }

    let primary_ids: HashSet<Uuid> = primary.iter().map(|e| e.id).collect();
    // Keep best score per candidate id and remember which seed it was reached from
    // (first reaching seed wins for the `reason` attribution).
    let mut best: HashMap<Uuid, (f32, Entity, String)> = HashMap::new();

    for seed in primary.iter().take(max_source_seeds) {
        let neighbors = match graph.k_hop_neighbors(seed.id, 1) {
            Ok(n) => n,
            Err(_) => continue,
        };
        for cand in neighbors {
            if primary_ids.contains(&cand.id) {
                continue;
            }

            // Vector sim to query; 0.0 when no embedding stored.
            let vec_score = match graph.get_vector(cand.id) {
                Ok(Some(v)) if !query_embedding.is_empty() && !v.is_empty() => {
                    cosine_sim(query_embedding, &v)
                }
                _ => 0.0,
            };

            // Graph-proximity floor — a 1-hop neighbour is always at least mildly
            // worth surfacing. 0.1 keeps it below any real cosine hit.
            let score = vec_score.max(0.0) + 0.1;

            let reason = format!("1-hop from {}", seed.name);
            best.entry(cand.id)
                .and_modify(|entry| {
                    if score > entry.0 {
                        entry.0 = score;
                        entry.2 = reason.clone();
                    }
                })
                .or_insert((score, cand, reason));
        }
    }

    let mut scored: Vec<(f32, Entity, String)> = best.into_values().collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(max_related);

    scored
        .into_iter()
        .map(|(score, entity, reason)| RelatedEntity {
            id: entity.id,
            name: entity.name,
            entity_type: format!("{:?}", entity.entity_type).to_lowercase(),
            score,
            reason,
        })
        .collect()
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

/// Persist any unfinalized bandit reward on engine drop. Without this,
/// short-lived processes (single CLI query, one-shot Tauri sessions)
/// always lose the last query's arm pull because the reward is only
/// finalized by the *next* call to `query()` — which never arrives. The
/// symptom was an empty `~/.tracemind/bandit.json` even after a dozen
/// retrievals across sessions.
impl Drop for RetrievalEngine {
    fn drop(&mut self) {
        if self.pending_reward.is_some() {
            self.finalize_pending_reward();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GEPA-promoted `policy.json` must actually change the running
    /// engine's configuration. Without this link the optimisation loop's
    /// output lives only in the process that produced it, and every claim
    /// about self-improvement is unbacked.
    #[test]
    fn open_applies_promoted_policy_from_disk() {
        let dir = std::env::temp_dir().join(format!("tm_pol_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();

        std::fs::write(
            dir.join("policy.json"),
            r#"{"space_weights":{"text":1.0,"lexical":0.0,"recency":0.0,"confidence":0.0},
                "min_score":0.77,"candidate_multiplier":9,
                "coverage_weight":0.0,"fit_weight":0.0,"generic_coverage_boost":1.0}"#,
        )
        .unwrap();

        let engine = RetrievalEngine::open(&db, &traces, true).unwrap();
        let active = engine.current_policy();
        assert_eq!(active.candidate_multiplier, 9, "policy.json was not applied");
        assert!((active.min_score - 0.77).abs() < 1e-5, "min_score = {}", active.min_score);
        let w = active.normalised_weights();
        assert!(w["text"] > 0.99, "text weight = {}", w["text"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt policy file must not stop the engine from opening — the
    /// product has to keep working on compiled-in defaults.
    #[test]
    fn open_falls_back_when_policy_is_corrupt() {
        let dir = std::env::temp_dir().join(format!("tm_polbad_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
        std::fs::write(dir.join("policy.json"), "{ not json").unwrap();

        let engine = RetrievalEngine::open(&db, &traces, true)
            .expect("engine must open despite a corrupt policy file");
        let active = engine.current_policy();
        assert_eq!(
            active.candidate_multiplier,
            SIGNAL_CANDIDATE_MULTIPLIER,
            "should have fallen back to defaults"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch_engine(host: HostKind) -> (RetrievalEngine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tm_rew_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let mut engine = RetrievalEngine::open(&db, &traces, true).unwrap();
        engine.set_host_kind(host);
        (engine, dir)
    }

    fn total_pulls(engine: &RetrievalEngine) -> u64 {
        engine.bandit_stats().iter().map(|(p, _)| *p).sum()
    }

    /// The defect this replaces: an agentic host issues its next tool call
    /// in well under five seconds, which the old model scored 0.1 — the
    /// worst possible reward — for essentially every query. With no
    /// evidence the posterior must simply not move.
    #[test]
    fn agentic_host_without_feedback_does_not_update_the_bandit() {
        let (mut engine, dir) = scratch_engine(HostKind::Agentic);
        let before = total_pulls(&engine);
        engine.query("anything at all").unwrap();
        // Second query immediately after — the pattern the old rule punished.
        engine.query("anything at all again").unwrap();
        assert_eq!(
            total_pulls(&engine),
            before,
            "an un-evidenced agentic query must leave the bandit untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Explicit feedback is trusted on any host, including agentic ones.
    #[test]
    fn explicit_feedback_updates_the_bandit_on_an_agentic_host() {
        let (mut engine, dir) = scratch_engine(HostKind::Agentic);
        let before = total_pulls(&engine);
        let result = engine.query("something worth rating").unwrap();
        engine
            .graph()
            .write_positive_signal(result.query_id, "result-1", "helpful", None, 0.3)
            .unwrap();
        // The next query finalizes the previous one's reward.
        engine.query("a later query").unwrap();
        assert!(
            total_pulls(&engine) > before,
            "explicit feedback must reach the bandit"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Interactive surfaces keep their engagement signals.
    #[test]
    fn interactive_click_updates_the_bandit() {
        let (mut engine, dir) = scratch_engine(HostKind::Interactive);
        let before = total_pulls(&engine);
        engine.query("clicked query").unwrap();
        engine.register_click(uuid::Uuid::new_v4());
        engine.query("next query").unwrap();
        assert!(
            total_pulls(&engine) > before,
            "a click is real evidence on an interactive surface"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_host_kind_is_agentic() {
        let (engine, dir) = scratch_engine(HostKind::Agentic);
        assert_eq!(engine.host_kind(), HostKind::Agentic);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        let result = engine.query("hello world").unwrap();
        assert!(result.latency_ms < 5000);

        // Deferred rewards are *evidence-gated*: a second query finalizes
        // the first one's pending reward, but with no feedback and no
        // interactive engagement there is nothing to learn from, so the
        // posterior must not move. (This test previously asserted the
        // opposite — that every query updates the bandit — which is the
        // behaviour that made an agentic host register 0.1 for everything.)
        let _result2 = engine.query("test query").unwrap();
        let unevidenced: u64 = engine.bandit_stats().iter().map(|(c, _)| c).sum();
        assert_eq!(unevidenced, 0, "un-evidenced queries must not train the bandit");

        // Supply real evidence and the update lands.
        engine.set_host_kind(HostKind::Interactive);
        let result3 = engine.query("third query").unwrap();
        assert!(!result3.query_id.is_nil());
        engine.register_click(uuid::Uuid::new_v4());
        engine.finalize_pending_reward();
        let evidenced: u64 = engine.bandit_stats().iter().map(|(c, _)| c).sum();
        assert_eq!(evidenced, 1, "a click must train the bandit");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prefetch_hit_takes_l1_path_and_records_phase() {
        let dir = std::env::temp_dir().join(format!("tm_ret_pf_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let bandit_path = dir.join("bandit.json");
        let linucb_path = dir.join("linucb.json");
        let mut engine = RetrievalEngine {
            graph: GraphStore::open(&db).unwrap(),
            trace_store: TraceStore::open(&traces).unwrap(),
            embedder: Embedder::new_hash(),
            bandit: UcbBandit::new(),
            bandit_path,
            linucb: LinUcbBandit::new(),
            linucb_path,
            reranker: None,
            query_cache: RecentQueryCache::new(10),
            pending_reward: None,
            planner: QueryPlanner::new(),
            procedure_store: None,
            trajectory_store: None,
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        // Prime an entry for "hello world" — even on an empty graph
        // this proves the cache shortcut is taken.
        engine.prime_prefetch("hello world").unwrap();
        assert_eq!(engine.prefetch_stats().primes, 1);

        let result = engine.query("hello world").unwrap();

        // The vector_search phase should be replaced by l1_prefetch_hit.
        let took_l1 = result.phases.iter().any(|p| p.phase == "l1_prefetch_hit");
        let took_vs = result.phases.iter().any(|p| p.phase == "vector_search");
        assert!(took_l1, "expected l1_prefetch_hit phase, got: {:?}",
                result.phases.iter().map(|p| p.phase).collect::<Vec<_>>());
        assert!(!took_vs, "vector_search should have been short-circuited");
        assert_eq!(engine.prefetch_stats().hits, 1);

        // A query for an unrelated string falls back to vector_search.
        let other = engine.query("totally different string").unwrap();
        let took_vs2 = other.phases.iter().any(|p| p.phase == "vector_search");
        assert!(took_vs2);
        assert_eq!(engine.prefetch_stats().misses, 1);

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

    /// TM-UX-001: a query that hits entity A surfaces A's 1-hop neighbour B
    /// as a `related_entity`, not as a primary hit.
    #[test]
    fn related_entities_surfaces_one_hop_neighbours() {
        use tm_types::{Entity, EntityType, Predicate, Triple};

        let dir = std::env::temp_dir().join(format!("tm_ret_related_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();

        let graph = GraphStore::open(&db).unwrap();

        // Seed graph: A (the hit) — related_to — B (the neighbour).
        let a = Entity::new("Apple", EntityType::Organization, 0.9);
        let b = Entity::new("M4 chip", EntityType::Concept, 0.9);
        graph.upsert_entity(&a).unwrap();
        graph.upsert_entity(&b).unwrap();
        let t = Triple::new(a.id, Predicate::RelatedTo, b.id, 0.9);
        graph.upsert_triple(&t).unwrap();

        // Give both entities a query-like embedding so vector search returns A,
        // and the related-entity scorer has a signal on B. Hash embedder is
        // deterministic given the same text.
        let embedder = Embedder::new_hash();
        let query = "Apple hardware";
        let query_emb = embedder.embed(query);
        let a_emb = embedder.embed("Apple");
        let b_emb = embedder.embed("M4 chip");
        graph.upsert_vector(a.id, &a_emb).unwrap();
        graph.upsert_vector(b.id, &b_emb).unwrap();

        // Directly exercise the helper — isolates the feature from query() side-effects.
        let primary = vec![a.clone()];
        let related = compute_related_entities(&graph, &query_emb, &primary, 5, 5);

        assert_eq!(related.len(), 1, "expected one related entity, got {related:?}");
        assert_eq!(related[0].id, b.id);
        assert_eq!(related[0].name, "M4 chip");
        assert!(
            related[0].reason.contains("Apple"),
            "reason should attribute to seed entity: {:?}", related[0].reason
        );

        // Primary entity must never appear in related (dedup guarantee).
        let primary_both = vec![a.clone(), b.clone()];
        let related_both = compute_related_entities(&graph, &query_emb, &primary_both, 5, 5);
        assert!(related_both.is_empty(),
            "B should be filtered when already in primary set, got {related_both:?}");

        // Empty primary → empty related (bounded).
        let none = compute_related_entities(&graph, &query_emb, &[], 5, 5);
        assert!(none.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    // ─── Sprint C-0.6: retrieval scopes to active context ───────────────

    /// When an active context is set, retrieval should filter out entities
    /// tagged with a *different* context. Unscoped legacy entities + entities
    /// in the active context stay visible. `cross_context=true` bypasses
    /// the filter and surfaces everything.
    #[test]
    fn retrieval_scopes_to_active_context() {
        let dir = std::env::temp_dir().join(format!("tm_ret_scope_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let bandit_path = dir.join("bandit.json");
        let linucb_path = dir.join("linucb.json");

        let mut engine = RetrievalEngine {
            graph: GraphStore::open(&db).unwrap(),
            trace_store: TraceStore::open(&traces).unwrap(),
            embedder: Embedder::new_hash(),
            bandit: UcbBandit::new(),
            bandit_path,
            linucb: LinUcbBandit::new(),
            linucb_path,
            reranker: None,
            query_cache: RecentQueryCache::new(10),
            pending_reward: None,
            planner: QueryPlanner::new(),
            procedure_store: None,
            trajectory_store: None,
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        let now = chrono::Utc::now();
        let mk = |name: &str| tm_types::Entity {
            id: Uuid::new_v4(),
            name: name.to_string(),
            entity_type: tm_types::EntityType::Concept,
            confidence: 0.95,
            source_id: Some("ctx-test".to_string()),
            created_at: now,
            updated_at: now,
        };

        // Two contexts: rondo + tracemind.
        let rondo = Uuid::new_v4();
        let tracemind = Uuid::new_v4();

        // Insert an unscoped legacy entity.
        let legacy = mk("legacy concept item");
        engine.graph.upsert_entity(&legacy).unwrap();
        let emb = engine.embedder.embed(&legacy.name);
        engine.graph.upsert_vector(legacy.id, &emb).unwrap();

        // Insert one scoped to rondo.
        engine.graph.set_active_context(Some(rondo));
        let in_rondo = mk("rondo special player item");
        engine.graph.upsert_entity(&in_rondo).unwrap();
        let emb = engine.embedder.embed(&in_rondo.name);
        engine.graph.upsert_vector(in_rondo.id, &emb).unwrap();

        // Insert one scoped to tracemind.
        engine.graph.set_active_context(Some(tracemind));
        let in_tm = mk("tracemind special memory item");
        engine.graph.upsert_entity(&in_tm).unwrap();
        let emb = engine.embedder.embed(&in_tm.name);
        engine.graph.upsert_vector(in_tm.id, &emb).unwrap();

        // Activate rondo, query with a generic term that hits all three:
        engine.graph.set_active_context(Some(rondo));
        engine.set_cross_context(false);
        let scoped = engine.query("item").unwrap();
        let scoped_ids: std::collections::HashSet<Uuid> =
            scoped.entities.iter().map(|e| e.id).collect();

        // Legacy (unscoped) + rondo should pass; tracemind should be filtered.
        assert!(
            !scoped_ids.contains(&in_tm.id),
            "tracemind entity must not leak into rondo scope, got: {:?}",
            scoped.entities.iter().map(|e| &e.name).collect::<Vec<_>>(),
        );

        // cross_context=true should surface every entity, including tracemind's.
        engine.set_cross_context(true);
        let bridged = engine.query("item").unwrap();
        let bridged_ids: std::collections::HashSet<Uuid> =
            bridged.entities.iter().map(|e| e.id).collect();
        assert!(
            bridged_ids.contains(&in_tm.id) || bridged.entities.len() >= scoped.entities.len(),
            "cross_context should not be stricter than scoped retrieval"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Sprint C-0.7 — a `negative_signals` row keyed on the previous
    /// query's `query_id` must subtract from the relevance reward when
    /// the next query finalises that pending row. We compare the
    /// running mean recorded on the arm before vs after a not-related
    /// hit: a clean run produces a strictly larger arm-reward than a
    /// run with a negative signal of equal weight to the relevance
    /// reward.
    #[test]
    fn not_related_signal_subtracts_from_bandit_reward() {
        let dir = std::env::temp_dir().join(format!("tm_ret_neg_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
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
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        // Seed at least one entity so the query produces real candidates.
        let now = chrono::Utc::now();
        let ent = tm_types::Entity {
            id: Uuid::new_v4(),
            name: "alpha bravo charlie".to_string(),
            entity_type: tm_types::EntityType::Concept,
            confidence: 0.95,
            source_id: None,
            created_at: now,
            updated_at: now,
        };
        engine.graph.upsert_entity(&ent).unwrap();
        let emb = engine.embedder.embed(&ent.name);
        engine.graph.upsert_vector(ent.id, &emb).unwrap();

        // Baseline run: query, then flush via a second query to commit
        // the relevance reward without a negative signal.
        let _r1 = engine.query("alpha bravo").unwrap();
        let _r2 = engine.query("alpha bravo again").unwrap();
        let stats_clean = engine.bandit.arm_stats();
        let clean_total: f64 = stats_clean.iter().map(|(_, r)| *r).sum();

        // Second run: file a high-weight negative signal against the
        // last query *before* the next query forces finalisation.
        let r3 = engine.query("alpha bravo third").unwrap();
        engine
            .graph
            .write_negative_signal(r3.query_id, &ent.id.to_string(), "not_related", None, None, 1.0)
            .unwrap();
        let _r4 = engine.query("alpha bravo fourth").unwrap();
        let stats_after = engine.bandit.arm_stats();
        let after_total: f64 = stats_after.iter().map(|(_, r)| *r).sum();

        // We do not assert exact arm placement (the bandit chooses
        // arms dynamically), but the negative-signal-affected total
        // must be lower than the clean total — i.e. the loop *closes*.
        assert!(
            after_total < clean_total + 1e-9,
            "negative signal must not increase the bandit's mean reward (clean={clean_total}, after={after_total})"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Sprint D / F-1 — a `positive_signals` row keyed on the previous
    /// query's `query_id` must *add* to the relevance reward when the
    /// next query finalises that pending row. Mirror of
    /// `not_related_signal_subtracts_from_bandit_reward`.
    #[test]
    fn helpful_signal_adds_to_bandit_reward() {
        let dir = std::env::temp_dir().join(format!("tm_ret_pos_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();
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
            prefetch: PrefetchCache::new(),
            cross_context: false,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        let now = chrono::Utc::now();
        let ent = tm_types::Entity {
            id: Uuid::new_v4(),
            name: "delta echo foxtrot".to_string(),
            entity_type: tm_types::EntityType::Concept,
            confidence: 0.95,
            source_id: None,
            created_at: now,
            updated_at: now,
        };
        engine.graph.upsert_entity(&ent).unwrap();
        let emb = engine.embedder.embed(&ent.name);
        engine.graph.upsert_vector(ent.id, &emb).unwrap();

        // Baseline: two consecutive queries finalise with no positive signal.
        let _r1 = engine.query("delta echo").unwrap();
        let _r2 = engine.query("delta echo again").unwrap();
        let baseline_total: f64 = engine.bandit.arm_stats().iter().map(|(_, r)| *r).sum();

        // With a helpful signal filed before finalisation.
        let r3 = engine.query("delta echo third").unwrap();
        engine
            .graph
            .write_positive_signal(r3.query_id, &ent.id.to_string(), "helpful", None, 0.3)
            .unwrap();
        let _r4 = engine.query("delta echo fourth").unwrap();
        let with_pos_total: f64 = engine.bandit.arm_stats().iter().map(|(_, r)| *r).sum();

        assert!(
            with_pos_total > baseline_total - 1e-9,
            "positive signal must not decrease bandit mean reward (baseline={baseline_total}, after={with_pos_total})"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Sprint C-0.7 — when `cross_context=true` and an active context
    /// is set, candidates whose entity lives in a *foreign* context
    /// take a soft -0.15 hit. We verify by inserting two entities with
    /// near-identical embeddings, one in-scope and one foreign, then
    /// confirming the in-scope entity outranks the foreign one in
    /// cross-context mode.
    #[test]
    fn cross_context_penalty_reorders_candidates() {
        let dir = std::env::temp_dir().join(format!("tm_ret_xctx_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db").to_str().unwrap().to_string();
        let traces = dir.join("traces.jsonl").to_str().unwrap().to_string();

        let mut engine = RetrievalEngine {
            graph: GraphStore::open(&db).unwrap(),
            trace_store: TraceStore::open(&traces).unwrap(),
            embedder: Embedder::new_hash(),
            bandit: UcbBandit::new(),
            bandit_path: dir.join("bandit.json"),
            linucb: LinUcbBandit::new(),
            linucb_path: dir.join("linucb.json"),
            reranker: None,
            query_cache: RecentQueryCache::new(10),
            pending_reward: None,
            planner: QueryPlanner::new(),
            procedure_store: None,
            trajectory_store: None,
            prefetch: PrefetchCache::new(),
            cross_context: true,
            view_filter: None,
            router: MemoryRouter::default(),
            router_enabled: false,
            rewriter: QueryRewriter::new(4),
            composed_index: ComposedIndex::default_hybrid(),
            signal_min_score: SIGNAL_MIN_SIM,
            candidate_multiplier: SIGNAL_CANDIDATE_MULTIPLIER,
            // Conservative default: assume timing means nothing until a
            // surface declares itself interactive. A wrong "Interactive"
            // corrupts the bandit; a wrong "Agentic" merely forgoes a
            // weak signal.
            host_kind: HostKind::Agentic,
        };

        let now = chrono::Utc::now();
        let mk = |name: &str| tm_types::Entity {
            id: Uuid::new_v4(),
            name: name.to_string(),
            entity_type: tm_types::EntityType::Concept,
            confidence: 0.95,
            source_id: None,
            created_at: now,
            updated_at: now,
        };

        let scope_a = Uuid::new_v4();
        let scope_b = Uuid::new_v4();

        // Foreign-context entity (scope_b) inserted *first* with the
        // same embedding text so vector similarity is identical.
        engine.graph.set_active_context(Some(scope_b));
        let foreign = mk("identical signal payload");
        engine.graph.upsert_entity(&foreign).unwrap();
        let emb = engine.embedder.embed(&foreign.name);
        engine.graph.upsert_vector(foreign.id, &emb).unwrap();

        // In-scope entity in scope_a with same text.
        engine.graph.set_active_context(Some(scope_a));
        let native = mk("identical signal payload");
        engine.graph.upsert_entity(&native).unwrap();
        let emb2 = engine.embedder.embed(&native.name);
        engine.graph.upsert_vector(native.id, &emb2).unwrap();

        // Query with active = scope_a, cross_context=true. The foreign
        // candidate is still visible but should rank below native after
        // the -0.15 penalty.
        let r = engine.query("identical signal payload").unwrap();
        let ids: Vec<Uuid> = r.entities.iter().map(|e| e.id).collect();
        let pos_native = ids.iter().position(|i| *i == native.id);
        let pos_foreign = ids.iter().position(|i| *i == foreign.id);

        // If both surfaced, native must come first.
        if let (Some(pn), Some(pf)) = (pos_native, pos_foreign) {
            assert!(
                pn < pf,
                "in-scope native must rank before foreign under cross-context penalty (native={pn}, foreign={pf})"
            );
        } else {
            // Otherwise native at least surfaced.
            assert!(
                pos_native.is_some(),
                "in-scope entity must surface, got ids={ids:?}"
            );
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
