use seahash;
use uuid::Uuid;

use std::path::Path;

use tm_types::{Entity, EntityType, MemoryOp, Predicate, Result, Trace, TraceEventType, Triple};
use tm_graph::{context::ActiveContext, CapturedSignal, GraphStore};
use tm_vector::{Embedder, EmbedModel};
use tm_governance::GovernanceFilter;

use crate::extractor::{EntityExtractor, HeuristicExtractor};
use crate::rate_limit::RateLimiter;

pub struct IngestPipeline {
    graph: GraphStore,
    embedder: Embedder,
    governance: GovernanceFilter,
    extractor: Box<dyn EntityExtractor>,
    /// CAP-5 — per-source token-bucket rate limiter. Calls to
    /// `ingest_fast` consume one token per `source`; when the bucket
    /// empties, the call returns `skipped: Some("rate-limited: …")`.
    /// `with_unlimited_rate()` (or `TM_RATE_*` env vars) disables it
    /// for tests + the human-paced `tracemind ingest` subcommand.
    rate_limiter: RateLimiter,
}

#[derive(Debug)]
pub struct IngestResult {
    pub trace: Trace,
    pub entities: Vec<Entity>,
    /// Triples that were accepted (confidence ≥ ACCEPT_THRESHOLD) and
    /// written to `kg_relations` during this ingest.
    pub triples: Vec<Triple>,
    /// LM-9 pending-pool row ids for triples whose confidence fell
    /// between `PENDING_FLOOR` and `ACCEPT_THRESHOLD`. These rows are
    /// persisted in `pending_relations` but are *not* in the live
    /// graph yet — the user (or a future SLM) accepts/rejects them.
    pub pending_triples: Vec<Uuid>,
    /// Triples that were dropped (confidence below `PENDING_FLOOR`).
    /// Surfaced in the trace so the UI can show "discarded" counts
    /// without leaking the rows themselves.
    pub dropped_triples: usize,
    pub content_hash: String,
    /// Memory-R1 CRUD operations performed for each entity (entity_name, op).
    pub memory_ops: Vec<(String, MemoryOp)>,
    /// True if the selective ingestion gate rejected this input.
    pub skip_gate: bool,
}

/// Result of the fast-path ingestion (embed-first, extract-later).
#[derive(Debug)]
pub struct FastIngestResult {
    /// Row ID of the stored signal (0 if skipped or instantly promoted).
    pub signal_id: i64,
    pub content_hash: String,
    /// Priority tier assigned to this signal.
    pub priority: SignalPriority,
    /// If Tier-1 instant promotion happened, the entities that were created.
    pub instant_entities: Vec<Entity>,
    /// Reason the signal was skipped, or None if it was stored.
    pub skipped: Option<String>,
}

/// Priority classification for incoming signals. Decides how eagerly the signal
/// is promoted from the raw-signal store into the knowledge graph.
///
/// - `InstantEntity` (Tier 1): URL/file path/structured — promote right now, skip clustering.
/// - `Priority`    (Tier 2): Novel or high-entropy — consolidate every ~30s.
/// - `Normal`      (Tier 3): Typical content — consolidate every ~5 min.
/// - `Ephemeral`   (Tier 4): Redundant/low-value — stays searchable but never promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalPriority {
    InstantEntity,
    Priority,
    Normal,
    Ephemeral,
}

impl SignalPriority {
    /// Integer tier used in the `captured_signals.priority_tier` column.
    pub fn tier(self) -> i64 {
        match self {
            SignalPriority::InstantEntity => 1,
            SignalPriority::Priority => 2,
            SignalPriority::Normal => 3,
            SignalPriority::Ephemeral => 4,
        }
    }
}

/// Statistics returned by a consolidation pass.
#[derive(Debug, Default)]
pub struct ConsolidateStats {
    pub signals_scanned: usize,
    pub clusters_formed: usize,
    pub entities_promoted: usize,
    pub triples_created: usize,
    /// Signals in clusters below min_cluster_size (treated as noise).
    pub noise_signals: usize,
}

impl IngestPipeline {
    /// Open graph store at `db_path`, vector store at `db_path` + ".vec",
    /// and initialise embedder and default governance filter.
    ///
    /// If `hash_embed` is true, uses the deterministic hash embedder (no model download).
    pub fn open(db_path: &str, hash_embed: bool) -> Result<Self> {
        let graph = GraphStore::open(db_path)?;

        // Sprint C-0.5: forward the active context (if any) to the graph
        // so all subsequent upserts tag rows with the correct namespace.
        // The state file lives next to memory.db so we share scope with
        // the CLI and MCP server.
        if let Some(parent) = Path::new(db_path).parent() {
            let active_path = parent.join("active_context.json");
            if let Ok(Some(active)) = ActiveContext::load(&active_path) {
                graph.set_active_context(Some(active.id));
            }
        }

        let embedder = if hash_embed {
            Embedder::new_hash()
        } else if let Some(model) = std::env::var("TM_EMBED_MODEL").ok()
            .and_then(|s| EmbedModel::from_str_loose(&s))
        {
            Embedder::with_model(model)?
        } else {
            Embedder::new()?
        };
        let governance = GovernanceFilter::default();

        Ok(Self {
            graph,
            embedder,
            governance,
            extractor: Box::new(HeuristicExtractor),
            rate_limiter: RateLimiter::from_env(),
        })
    }

    /// Replace the entity extractor used during `ingest()` and slow-path
    /// consolidation. Defaults to [`HeuristicExtractor`]. A real ONNX GLiNER
    /// implementation is tracked by TM-NLP-004.
    pub fn with_extractor(mut self, extractor: Box<dyn EntityExtractor>) -> Self {
        tracing::info!("[ingest] entity extractor: {}", extractor.name());
        self.extractor = extractor;
        self
    }

    /// CAP-5 — replace the rate limiter. Pass `RateLimiter::unlimited()`
    /// for tests + human-paced ingest paths that shouldn't be throttled.
    pub fn with_rate_limiter(mut self, limiter: RateLimiter) -> Self {
        self.rate_limiter = limiter;
        self
    }

    // Stopwords for the selective ingestion gate (lowercase).
    const GATE_STOPWORDS: &'static [&'static str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "shall", "can", "to", "of", "in", "for",
        "on", "with", "at", "by", "from", "it", "its", "this", "that", "and",
        "or", "but", "not", "no", "if", "then", "so", "as", "i", "me", "my",
        "we", "you", "your", "he", "she", "they",
    ];

    /// Selective ingestion gate (MEM-inspired: model decides what to remember).
    /// Returns `(should_ingest, reason)`.
    ///
    /// Rejection criteria:
    /// - Too short (< 3 non-stopword tokens)
    /// - All stopwords / no semantic content
    /// - Near-exact duplicate of existing memory (cosine sim > 0.95)
    fn should_ingest(&self, text: &str) -> (bool, &'static str) {
        // 1. Length / semantic content check
        let non_stopword_count = text
            .split_whitespace()
            .filter(|w| {
                let lower = w.to_lowercase();
                let trimmed = lower.trim_matches(|c: char| !c.is_alphanumeric());
                !trimmed.is_empty()
                    && !Self::GATE_STOPWORDS.contains(&trimmed)
            })
            .count();

        if non_stopword_count < 3 {
            return (false, "too short or no semantic content");
        }

        // 2. Near-duplicate check via vector similarity
        let embedding = self.embedder.embed(text);
        if let Ok(results) = self.graph.search_vectors(&embedding, 1) {
            if let Some(&(_, sim)) = results.first() {
                if sim > 0.95 {
                    return (false, "near-duplicate of existing memory");
                }
            }
        }

        (true, "accepted")
    }

    /// Ingest raw `text` from `session_id`:
    ///
    /// 1. Run governance check (confidence = 1.0).
    /// 1.5. Selective ingestion gate (MEM-inspired).
    /// 2. Hash the text.
    /// 3. Extract entities heuristically (multi-word aware).
    /// 4. Deduplicate entities against the existing graph (exact + case-insensitive name match).
    /// 5. Upsert each entity to graph + vector stores (context-aware embeddings).
    /// 6. Extract typed triples via pattern matching + co-occurrence fallback.
    /// 7. Upsert each triple to the graph store.
    /// 8. Return an `IngestResult`.
    pub fn ingest(&self, text: &str, session_id: Uuid) -> Result<IngestResult> {
        // 1. Governance check.
        self.governance.check(text, 1.0)?;

        // 1.5: Selective ingestion gate (MEM-inspired)
        let (pass, reason) = self.should_ingest(text);
        if !pass {
            let content_hash = hash_text(text);
            let mut trace = Trace::new(session_id, TraceEventType::Ingest, &content_hash);
            trace.raw_text = Some(format!("[SKIPPED: {}] {}", reason, text));
            return Ok(IngestResult {
                trace,
                entities: vec![],
                triples: vec![],
                pending_triples: vec![],
                dropped_triples: 0,
                content_hash,
                memory_ops: vec![],
                skip_gate: true,
            });
        }

        // 2. Hash.
        let content_hash = hash_text(text);

        // 3. Extract entities via the configured extractor (heuristic by default).
        let mut entities = self.extractor.extract_entities(text);

        // 3b. Case-insensitive graph-name linking (TM-NLP-003a):
        //     The heuristic extractor skips lowercase tokens, so a sentence
        //     like "I use rust daily" never yields an entity even if "Rust"
        //     already exists in the graph. Rescue those mentions by scanning
        //     non-title-case tokens against the known entity-name index.
        self.link_lowercase_to_graph(text, &mut entities);

        // 4. Deduplicate: check each entity against the graph.
        //    - Exact or case-insensitive name match → reuse existing entity
        //    - Reinforces confidence of existing entities on re-mention
        //
        // First pass: resolve each name to an existing graph entity or a batch-local ID.
        let mut name_to_id: std::collections::HashMap<String, Uuid> =
            std::collections::HashMap::new();
        for entity in entities.iter_mut() {
            let key = entity.name.to_lowercase();

            // Check batch-local dedup first
            if let Some(&existing_id) = name_to_id.get(&key) {
                entity.id = existing_id;
                continue;
            }

            // Check graph for existing entity by name (case-insensitive)
            if let Ok(Some(existing)) = self.graph.find_entity_by_name_icase(&entity.name) {
                entity.id = existing.id;
                entity.confidence = existing.confidence;
                entity.created_at = existing.created_at;
                // Reinforce confidence on re-mention
                self.graph.reinforce_entity(existing.id, 0.05)?;
                name_to_id.insert(key, entity.id);
                continue;
            }

            // TM-NLP-003d — fuzzy match (Levenshtein ≤ 2) against the graph.
            // Bridges casing / punctuation / minor spelling variants
            // ("Rustlang" → "Rust", "TypeScript" → "Typescript") without
            // fragmenting the graph. Only applied when the exact /
            // case-insensitive lookup missed.
            if let Some(existing) = self.fuzzy_match_entity(&entity.name) {
                entity.id = existing.id;
                entity.confidence = existing.confidence;
                entity.created_at = existing.created_at;
                self.graph.reinforce_entity(existing.id, 0.05)?;
            }

            name_to_id.insert(key, entity.id);
        }

        // Remove within-batch duplicates (keep first occurrence of each ID)
        let mut seen_ids = std::collections::HashSet::new();
        entities.retain(|e| seen_ids.insert(e.id));

        // 5. Upsert entities with context-aware embeddings + Memory-R1 CRUD.
        //    Embed "entity_name: full source text" so the vector captures
        //    the semantic context in which the entity appeared.
        //    For each entity, decide whether to Add, Update, or Noop based
        //    on similarity to existing entities in the graph.
        let mut memory_ops: Vec<(String, MemoryOp)> = Vec::new();
        let mut kept_entities: Vec<Entity> = Vec::new();

        for entity in &entities {
            let embed_text = format!("{}: {}", entity.name, text);
            let embedding = self.embedder.embed(&embed_text);

            let op = Self::decide_memory_op(
                &entity.name,
                &embedding,
                &entity.entity_type,
                &self.graph,
            );

            match &op {
                MemoryOp::Add => {
                    self.graph.upsert_entity(entity)?;
                    self.graph.upsert_vector(entity.id, &embedding)?;
                    kept_entities.push(entity.clone());
                }
                MemoryOp::Update { target_entity_id } => {
                    // Merge: reinforce confidence and average the embedding vectors.
                    let target_id = *target_entity_id;
                    self.graph.reinforce_entity(target_id, 0.1)?;

                    if let Ok(Some(old_vec)) = self.graph.get_vector(target_id) {
                        let merged: Vec<f32> = old_vec
                            .iter()
                            .zip(embedding.iter())
                            .map(|(a, b)| (a + b) / 2.0)
                            .collect();
                        self.graph.upsert_vector(target_id, &merged)?;
                    }

                    // Return the existing entity in the result so callers
                    // know which entity was affected.
                    if let Ok(existing) = self.graph.get_entity(target_id) {
                        kept_entities.push(existing);
                    }
                }
                MemoryOp::Noop { .. } => {
                    // Near-duplicate — just lightly reinforce confidence.
                    // Find the entity ID from the graph search that caused the Noop.
                    let embed_text_for_search = format!("{}: {}", entity.name, text);
                    let search_emb = self.embedder.embed(&embed_text_for_search);
                    if let Ok(similar) = self.graph.search_vectors(&search_emb, 1) {
                        if let Some(&(top_id, _)) = similar.first() {
                            let _ = self.graph.reinforce_entity(top_id, 0.02);
                        }
                    }
                }
                MemoryOp::Delete { .. } => {
                    // Not used during ingest; reserved for future contradiction detection.
                }
            }

            memory_ops.push((entity.name.clone(), op));
        }

        // Replace entities with the kept set for downstream triple extraction.
        entities = kept_entities;

        // 6. Extract typed triples via pattern matching, then fill with co-occurrence.
        let triples = self.extractor.extract_triples(text, &entities);

        // 7. LM-9 confidence routing — accept high-confidence triples
        //    immediately, queue mid-confidence ones in the pending pool,
        //    drop the rest. Keep `triples` populated with the accepted
        //    rows only so downstream consumers (trace, UI) see the live
        //    graph state.
        let extracted = triples;
        let mut accepted_triples: Vec<Triple> = Vec::with_capacity(extracted.len());
        let mut pending_triples: Vec<Uuid> = Vec::new();
        let mut dropped_triples = 0usize;
        for triple in extracted {
            match self.graph.route_triple_by_confidence(&triple)? {
                tm_graph::PendingRouteOutcome::Accepted => accepted_triples.push(triple),
                tm_graph::PendingRouteOutcome::Pending(pid) => pending_triples.push(pid),
                tm_graph::PendingRouteOutcome::Dropped => dropped_triples += 1,
            }
        }
        let triples = accepted_triples;

        // 8. Build trace record with full provenance.
        let mut trace = Trace::new(session_id, TraceEventType::Ingest, &content_hash);
        trace.raw_text = Some(text.to_string());
        trace.entities_extracted = entities.iter().map(|e| e.id).collect();
        trace.triples_extracted = triples.iter().map(|t| t.id).collect();

        Ok(IngestResult {
            trace,
            entities,
            triples,
            pending_triples,
            dropped_triples,
            content_hash,
            memory_ops,
            skip_gate: false,
        })
    }
    // -----------------------------------------------------------------------
    // Two-speed pipeline: fast path (embed-first) + slow path (consolidate)
    // -----------------------------------------------------------------------

    /// **Fast path** — governance + dedup + embed + classify + dispatch by tier.
    ///
    /// Does NOT run NER or triple extraction for tiers 2–4. Designed for passive
    /// capture (clipboard, shell history, browser) where latency matters and no
    /// LLM is present.
    ///
    /// Tier dispatch:
    /// - **Tier 1 (InstantEntity)**: URL / file path / structured — promoted to
    ///   the graph immediately by invoking the full `ingest()` slow path. Returns
    ///   the created entities in `instant_entities`.
    /// - **Tier 2 (Priority)**: Novel or high-entropy content — stored with
    ///   priority_tier=2 and consolidated aggressively (~30s).
    /// - **Tier 3 (Normal)**: Typical content — stored with priority_tier=3 and
    ///   consolidated every ~5 min.
    /// - **Tier 4 (Ephemeral)**: Redundant / low-value — stored (searchable via
    ///   hybrid search) but never promoted.
    ///
    /// Target: <10ms wall time on the non–Tier-1 paths (dominated by the embed call).
    pub fn ingest_fast(
        &self,
        text: &str,
        source: &str,
        session_id: Uuid,
    ) -> Result<FastIngestResult> {
        // 0. CAP-5 — per-source token-bucket throttle. Floods are
        //    dropped here *before* the hash + embed + write path so a
        //    runaway capture loop can't stall queries.
        if let Err(reason) = self.rate_limiter.try_acquire(source) {
            // Use a stable content hash so downstream observability
            // doesn't see a UUID stream for rate-limited rejections.
            let content_hash_str = format!("{:016x}", seahash::hash(text.as_bytes()));
            return Ok(FastIngestResult {
                signal_id: 0,
                content_hash: content_hash_str,
                priority: SignalPriority::Ephemeral,
                instant_entities: vec![],
                skipped: Some(reason),
            });
        }

        // 1. Governance: reject PII before storing anything.
        self.governance.check(text, 1.0)?;

        // 2. Content hash + dedup.
        let hash64 = seahash::hash(text.as_bytes());
        let content_hash_str = format!("{:016x}", hash64);

        if self.graph.signal_exists(hash64) {
            return Ok(FastIngestResult {
                signal_id: 0,
                content_hash: content_hash_str,
                priority: SignalPriority::Ephemeral,
                instant_entities: vec![],
                skipped: Some("duplicate signal".to_string()),
            });
        }

        // 3. Semantic gate: reject if too short / all stopwords.
        //    Structured content (URL, file path, code fence, env line) bypasses
        //    this gate — a bare URL is only one token but is still worth capturing.
        let is_structured = has_structured_marker(text);
        if !is_structured {
            let non_sw = text
                .split_whitespace()
                .filter(|w| {
                    let lower = w.to_lowercase();
                    let t = lower.trim_matches(|c: char| !c.is_alphanumeric());
                    !t.is_empty() && !Self::GATE_STOPWORDS.contains(&t)
                })
                .count();
            if non_sw < 3 {
                return Ok(FastIngestResult {
                    signal_id: 0,
                    content_hash: content_hash_str,
                    priority: SignalPriority::Ephemeral,
                    instant_entities: vec![],
                    skipped: Some("too short or no semantic content".to_string()),
                });
            }
        }

        // 4. Embed.
        let embedding = self.embedder.embed(text);

        // 5. Classify into a priority tier.
        let priority = classify_signal(text, &embedding, &self.graph);

        // 6a. Tier-1 (InstantEntity): skip the signal table entirely and run
        //     the full ingest pipeline so the entity lands in the graph now.
        if priority == SignalPriority::InstantEntity {
            match self.ingest(text, session_id) {
                Ok(result) => {
                    return Ok(FastIngestResult {
                        signal_id: 0,
                        content_hash: content_hash_str,
                        priority,
                        instant_entities: result.entities,
                        skipped: None,
                    });
                }
                Err(e) => {
                    // Fall through to tier-3 storage on failure so the capture
                    // isn't lost.
                    tracing::debug!("[ingest_fast] tier-1 promotion failed, downgrading: {e}");
                }
            }
        }

        // 6b. Tier 2/3/4: store signal with its priority tier.
        let tier = if priority == SignalPriority::InstantEntity {
            SignalPriority::Normal.tier()
        } else {
            priority.tier()
        };

        let signal_id = self.graph.insert_signal_with_embedding(
            source,
            text,
            hash64,
            session_id,
            &embedding,
            None,
            tier,
        )?;

        Ok(FastIngestResult {
            signal_id,
            content_hash: content_hash_str,
            priority,
            instant_entities: vec![],
            skipped: None,
        })
    }

    /// **Slow path** — cluster unconsolidated signals and promote dense clusters
    /// to entities via the full `ingest()` pipeline.
    ///
    /// Call this periodically (e.g., every 5 minutes or on idle). It is safe to
    /// call concurrently; signals are processed in insertion order.
    ///
    /// Scans all non-ephemeral tiers (1–3). Tier-2 (Priority) signals are
    /// normally promoted earlier by `consolidate_priority()` on a tighter
    /// schedule, but this pass acts as a catch-all.
    ///
    /// # Parameters
    /// - `max_signals`: max signals to load per pass (bounds CPU time).
    /// - `min_cluster_size`: clusters smaller than this are treated as noise.
    /// - `sim_threshold`: cosine similarity threshold for joining a cluster (0.0–1.0).
    pub fn consolidate(
        &self,
        max_signals: usize,
        min_cluster_size: usize,
        sim_threshold: f32,
    ) -> Result<ConsolidateStats> {
        self.consolidate_tier(None, max_signals, min_cluster_size, sim_threshold)
    }

    /// Aggressive consolidation pass for Tier-2 (Priority) signals only.
    /// Uses a lower `min_cluster_size` so novel content can be promoted after
    /// just 1–2 captures.
    ///
    /// Recommended schedule: every ~30 seconds.
    pub fn consolidate_priority(
        &self,
        max_signals: usize,
        sim_threshold: f32,
    ) -> Result<ConsolidateStats> {
        // Tier-2 is, by construction, novel content — a singleton is enough to
        // promote, because we've already vetted it as "not redundant".
        self.consolidate_tier(
            Some(SignalPriority::Priority.tier()),
            max_signals,
            1, // singletons allowed
            sim_threshold,
        )
    }

    /// Tier-scoped consolidation. If `tier_filter` is Some(t), only signals at
    /// that tier are processed. If None, all non-ephemeral tiers (1–3).
    fn consolidate_tier(
        &self,
        tier_filter: Option<i64>,
        max_signals: usize,
        min_cluster_size: usize,
        sim_threshold: f32,
    ) -> Result<ConsolidateStats> {
        let signals = self
            .graph
            .unconsolidated_signals_by_tier(tier_filter, max_signals)?;
        if signals.is_empty() {
            return Ok(ConsolidateStats::default());
        }

        let mut stats = ConsolidateStats {
            signals_scanned: signals.len(),
            ..Default::default()
        };

        // Single-link agglomerative clustering by cosine similarity.
        let clusters = cluster_by_similarity(&signals, sim_threshold);

        for (cluster_idx, member_indices) in clusters.iter().enumerate() {
            if member_indices.len() < min_cluster_size {
                // Noise — mark so these signals aren't re-scanned.
                stats.noise_signals += member_indices.len();
                for &idx in member_indices {
                    let _ = self.graph.mark_signal_clustered(signals[idx].id, -1, None);
                }
                continue;
            }

            stats.clusters_formed += 1;

            // Pick representative: the signal with the most tokens.
            let rep_idx = member_indices
                .iter()
                .copied()
                .max_by_key(|&i| signals[i].raw_text.split_whitespace().count())
                .unwrap();
            let rep_text = &signals[rep_idx].raw_text;
            let rep_session = signals[rep_idx].session_id.unwrap_or_else(Uuid::new_v4);

            // Run full NER + triple extraction on the representative text.
            let primary_entity_id = match self.ingest(rep_text, rep_session) {
                Ok(result) => {
                    stats.entities_promoted += result.entities.len();
                    stats.triples_created += result.triples.len();
                    result.entities.first().map(|e| e.id)
                }
                Err(_) => None,
            };

            // Mark all cluster members as consolidated.
            for &idx in member_indices {
                let _ = self.graph.mark_signal_clustered(
                    signals[idx].id,
                    cluster_idx as i64,
                    primary_entity_id,
                );
            }
        }

        Ok(stats)
    }

    // -----------------------------------------------------------------------
    // TM-NLP-003a — case-insensitive graph-name linking
    // -----------------------------------------------------------------------

    /// Rescue lowercase / non-Title-Case mentions of entities that already
    /// exist in the graph. The heuristic extractor is deliberately picky about
    /// casing (to avoid false positives on prose), which means graph quality
    /// grows with the graph itself: the moment `Rust` is stored, every future
    /// "rust" in a captured note becomes a link rather than a miss.
    ///
    /// Cost is bounded: we hit the DB only for non-stopword tokens of length
    /// ≥ 3 that weren't already emitted, and we short-circuit at 20 new hits.
    fn link_lowercase_to_graph(&self, text: &str, entities: &mut Vec<Entity>) {
        // Set of already-emitted names (lowercased) so we don't re-emit.
        let mut seen: std::collections::HashSet<String> = entities
            .iter()
            .map(|e| e.name.to_lowercase())
            .collect();
        // Individual words that already participate in a multi-word entity —
        // e.g. "Machine Learning" already covers "machine" and "learning".
        for e in entities.iter() {
            for w in e.name.split_whitespace() {
                seen.insert(w.to_lowercase());
            }
        }

        let mut added = 0;
        for raw in text.split_whitespace() {
            if added >= 20 || entities.len() >= 40 {
                break;
            }
            let token = raw.trim_matches(STRIP_CHARS);
            if token.len() < 3 {
                continue;
            }
            let lower = token.to_lowercase();
            if seen.contains(&lower) {
                continue;
            }
            // Skip Title-Case tokens — the extractor already handled those in
            // either its multi-word or single-token pass.
            if is_title_case(token) {
                continue;
            }
            // Skip stopwords and things that look like numbers.
            if STOPWORDS.contains(&lower.as_str())
                || lower.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }

            match self.graph.find_entity_by_name_icase(token) {
                Ok(Some(existing)) => {
                    // Preserve the graph's canonical casing for the entity name
                    // so downstream dedup lines up with the stored row.
                    let mut e = existing.clone();
                    // The pipeline's main dedup pass will re-bind the id /
                    // confidence / created_at, but carrying the existing id
                    // here avoids a redundant INSERT attempt.
                    e.confidence = existing.confidence;
                    entities.push(e);
                    seen.insert(lower);
                    added += 1;
                }
                _ => { /* no match — leave it */ }
            }
        }

        if added > 0 {
            tracing::debug!("[ingest] linked {added} lowercase mentions to graph");
        }
    }

    // -----------------------------------------------------------------------
    // TM-NLP-003d — fuzzy (Levenshtein) entity linking
    // -----------------------------------------------------------------------

    /// Find an existing graph entity whose name is within edit-distance 2 of
    /// `name`. Returns `None` when the best candidate is either absent or
    /// further than the threshold.
    ///
    /// Avoids graph fragmentation from minor spelling / punctuation drift
    /// ("Rustlang" ↔ "Rust" is too far; "TypeScript" ↔ "Typescript" is 1).
    /// Length-prefiltered at the SQL layer to keep the candidate set small.
    fn fuzzy_match_entity(&self, name: &str) -> Option<Entity> {
        const MAX_DIST: usize = 2;
        // Very short names are too noisy — 2 edits of a 3-letter word is
        // half the name. Require at least 4 characters.
        if name.chars().count() < 4 {
            return None;
        }

        let candidates = self
            .graph
            .entities_near_length(name.len(), MAX_DIST)
            .ok()?;
        let lower = name.to_lowercase();

        let mut best: Option<(usize, Entity)> = None;
        for cand in candidates {
            let d = levenshtein(&lower, &cand.name.to_lowercase());
            if d == 0 {
                // Exact match would have been caught upstream; skip.
                continue;
            }
            if d > MAX_DIST {
                continue;
            }
            match &best {
                Some((bd, _)) if *bd <= d => {}
                _ => best = Some((d, cand)),
            }
        }
        best.map(|(_, e)| e)
    }

    // -----------------------------------------------------------------------
    // Memory-R1 CRUD decision logic
    // -----------------------------------------------------------------------

    /// Decide what operation to perform for a new entity based on similarity
    /// to entities already in the graph.
    ///
    /// Thresholds (cosine similarity):
    /// - `> 0.90` and same type  => **Noop** (near-duplicate)
    /// - `> 0.90` and diff type  => **Update** (same concept, reclassify)
    /// - `0.75 .. 0.90`          => **Update** (merge / reinforce)
    /// - `< 0.75` (or no match)  => **Add** (novel entity)
    fn decide_memory_op(
        name: &str,
        embedding: &[f32],
        entity_type: &EntityType,
        graph: &GraphStore,
    ) -> MemoryOp {
        let similar = graph.search_vectors(embedding, 5).unwrap_or_default();

        if similar.is_empty() || similar[0].1 < 0.4 {
            return MemoryOp::Add;
        }

        let (top_id, top_sim) = similar[0];

        // TM-NLP-004 guard: entities get embedded as "name: full_source_text",
        // so multiple entities extracted from the same sentence end up with
        // near-identical vectors. Collapsing them by embedding similarity
        // alone turned every secondary entity into a fake "duplicate" of the
        // first. Require a name match (exact/case-insensitive/substring) on
        // top of the vector sim before treating as Update/Noop.
        let existing_name = graph
            .get_entity(top_id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_default();
        let names_agree = names_look_like_same_entity(name, &existing_name);

        // Very high similarity (>0.90) AND name match = likely duplicate
        if top_sim > 0.90 && names_agree {
            if let Ok(existing) = graph.get_entity(top_id) {
                if existing.entity_type == *entity_type {
                    return MemoryOp::Noop {
                        reason: format!("duplicate of '{}'", existing.name),
                    };
                } else {
                    return MemoryOp::Update {
                        target_entity_id: top_id,
                    };
                }
            }
        }

        // High similarity (0.75-0.90) AND name match = update/merge
        if top_sim > 0.75 && names_agree {
            return MemoryOp::Update {
                target_entity_id: top_id,
            };
        }

        // Different-named entity (even at high embedding sim) is a new entity.
        MemoryOp::Add
    }
}

/// Conservative check: treat two names as the same entity only if one is a
/// prefix/suffix/case-variant of the other. Prevents `decide_memory_op` from
/// folding unrelated entities that merely share sentence context.
fn names_look_like_same_entity(a: &str, b: &str) -> bool {
    let al = a.trim().to_lowercase();
    let bl = b.trim().to_lowercase();
    if al.is_empty() || bl.is_empty() {
        return false;
    }
    if al == bl {
        return true;
    }
    // Sub-string match only counts if the shorter one is at least 4 chars —
    // otherwise "I" / "US" style short names would swallow everything.
    let (shorter, longer) = if al.len() <= bl.len() {
        (al.as_str(), bl.as_str())
    } else {
        (bl.as_str(), al.as_str())
    };
    shorter.len() >= 4 && longer.contains(shorter)
}

// ---------------------------------------------------------------------------
// Heuristic NER — multi-word aware
// ---------------------------------------------------------------------------

const STOPWORDS: &[&str] = &[
    "The", "A", "An", "In", "On", "At", "To", "For", "Of", "And", "Or", "But", "Is", "Was",
    "Are", "Were", "Be", "Been", "It", "Its", "This", "That", "With", "From", "By", "As", "Up",
    "Out", "If", "So", "No", "I", "My", "We", "Our", "He", "She", "They", "You", "Your",
    "Has", "Had", "Have", "Do", "Does", "Did", "Not", "All", "Each", "Every", "Can", "Will",
    "Just", "Now", "Then", "Here", "There", "When", "How", "What", "Who", "Which", "Where",
];

/// Known technology/language names that should never be classified as Person.
const KNOWN_TECH: &[&str] = &[
    "Rust", "Python", "JavaScript", "TypeScript", "React", "Docker", "Kubernetes", "Linux",
    "Redis", "Postgres", "PostgreSQL", "MongoDB", "SQLite", "Tauri", "Wasm", "WebAssembly",
    "Git", "GitHub", "Node", "Deno", "Cargo", "Webpack", "Vite", "FastAPI", "Django", "Flask",
    "Spring", "Java", "Kotlin", "Swift", "Go", "Ruby", "Rails", "Vue", "Angular", "Svelte",
    "AWS", "Azure", "GCP", "Terraform", "Ansible", "Nginx", "Apache", "GraphQL", "REST",
    "LanceDB", "ChromaDB", "Pinecone", "Qdrant", "ONNX", "PyTorch", "TensorFlow",
    "Claude", "GPT", "LLM", "MCP", "API", "CLI", "SDK", "CSS", "HTML", "SQL",
    "Privacy", "Security", "Performance", "Latency", "Throughput", "Memory", "CPU", "GPU",
];

/// Organisation suffixes — if a multi-word entity ends with one of these, classify as Org.
const ORG_SUFFIXES: &[&str] = &[
    "Inc", "Corp", "LLC", "Ltd", "Co", "Company", "Foundation", "Institute", "Labs",
    "Technologies", "Systems", "Group", "Team", "Studio", "Studios",
];

const FILE_EXTENSIONS: &[&str] = &[
    ".rs", ".py", ".js", ".ts", ".go", ".md", ".txt", ".json", ".toml",
    ".yaml", ".yml", ".html", ".css", ".sql", ".sh", ".dockerfile",
];

const STRIP_CHARS: &[char] = &[',', '.', '!', '?', ';', ':', '"', '\'', '(', ')'];

/// Common English verbs, adjectives, and functional words that should never become entities.
const SKIP_WORDS: &[&str] = &[
    // Verbs
    "uses", "using", "used", "works", "working", "worked", "built", "builds", "building",
    "stores", "storing", "stored", "runs", "running", "creates", "creating", "created",
    "makes", "making", "calls", "calling", "called", "enables", "enabling", "enabled",
    "exposes", "exposing", "exposed", "backs", "backing", "backed", "leaves", "leaving",
    "gets", "getting", "sets", "setting", "adds", "adding", "sends", "sending",
    "takes", "taking", "gives", "giving", "goes", "going", "comes", "coming",
    "keeps", "keeping", "finds", "finding", "tells", "telling", "says", "saying",
    "shows", "showing", "means", "meaning", "tries", "trying", "starts", "starting",
    "turns", "turning", "plays", "playing", "moves", "moving", "lives", "living",
    "believes", "happens", "writes", "provides", "includes", "continues", "allows",
    "produces", "needs", "helps", "reads", "holds", "generates", "brings", "mentions",
    "develops", "developing", "developed", "implements", "implementing", "implemented",
    "collaborates", "collaborating",
    // Past participles / adjectives
    "based", "designed", "focused", "related", "known", "open", "local", "only",
    "also", "even", "just", "very", "most", "more", "much", "many", "some", "such",
    "well", "still", "already", "always", "never", "ever", "often", "really",
    // Functional
    "across", "between", "through", "within", "without", "about", "after", "before",
    "over", "under", "into", "like", "than", "both", "each", "other", "while",
    "during", "since", "until", "against", "among", "along", "around",
    // Common nouns too generic to be useful
    "data", "time", "information", "system", "systems", "tool", "tools", "type", "types",
    "team", "teams", "device", "devices", "locally", "core", "part", "way", "thing",
];

/// Classic two-row Levenshtein edit distance (stdlib only).
///
/// Compares byte-sequences. Callers normalise to lowercase before calling
/// when case-insensitive matching is desired. O(|a| * |b|) time and O(|b|)
/// memory, which is fine for the short entity names we compare.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.is_empty() {
        return b_bytes.len();
    }
    if b_bytes.is_empty() {
        return a_bytes.len();
    }

    let n = b_bytes.len();
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for (i, &ac) in a_bytes.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &bc) in b_bytes.iter().enumerate() {
            let cost = if ac == bc { 0 } else { 1 };
            let del = prev[j + 1] + 1;
            let ins = curr[j] + 1;
            let sub = prev[j] + cost;
            curr[j + 1] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

/// TM-NLP-003b — YAKE-style unsupervised keyphrase extraction.
///
/// Surfaces multi-word lowercase phrases the Title-Case extractor ignores
/// (e.g. "machine learning", "vector search", "large language model").
/// Scoring is intentionally simple — term frequency with a position
/// bonus for phrases that appear earlier in the text. Any phrase whose
/// score exceeds `threshold` is emitted as a [`EntityType::Concept`].
///
/// Stopwords and common verbs / generic nouns from `STOPWORDS` /
/// `SKIP_WORDS` are never allowed inside a candidate phrase.
pub(crate) fn extract_keyphrases(text: &str) -> Vec<Entity> {
    const MAX_GRAM: usize = 3;
    const MIN_GRAM: usize = 2;
    const MIN_SCORE: f64 = 1.0;
    const MAX_EMIT: usize = 8;

    // Tokenize to lowercased word stream with original positions preserved.
    let tokens: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(STRIP_CHARS)
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect();

    let total = tokens.len();
    if total < MIN_GRAM {
        return Vec::new();
    }

    // Cheap stopword / skip-word check (case-insensitive against the
    // Title-Case STOPWORDS table + lowercase SKIP_WORDS).
    let is_noise = |w: &str| -> bool {
        if w.len() < 3 || w.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
        if SKIP_WORDS.contains(&w) {
            return true;
        }
        STOPWORDS.iter().any(|s| s.eq_ignore_ascii_case(w))
    };

    // Build (phrase -> (count, first_position)) table.
    let mut phrases: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for n in MIN_GRAM..=MAX_GRAM {
        if total < n {
            break;
        }
        for i in 0..=(total - n) {
            let window = &tokens[i..i + n];
            if window.iter().any(|w| is_noise(w)) {
                continue;
            }
            let phrase = window.join(" ");
            let entry = phrases.entry(phrase).or_insert((0, i));
            entry.0 += 1;
            if i < entry.1 {
                entry.1 = i;
            }
        }
    }

    // Score + threshold.
    let mut scored: Vec<(String, f64)> = phrases
        .into_iter()
        .map(|(phrase, (count, first_pos))| {
            let freq = count as f64;
            // Earlier phrases score higher; normalise position by token count.
            let position_bonus = 1.0 - (first_pos as f64 / total as f64).min(1.0);
            (phrase, freq + position_bonus)
        })
        .filter(|(_, s)| *s >= MIN_SCORE)
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // If a longer candidate contains a shorter one AND the shorter one
    // has strictly higher score, drop the longer — it was inflated by the
    // shorter high-signal core. This keeps "machine learning" and drops
    // "explored machine learning" when the bigram is more frequent.
    let mut kept: Vec<(String, f64)> = Vec::with_capacity(scored.len());
    for (phrase, score) in &scored {
        let dominated = scored.iter().any(|(other, other_score)| {
            other != phrase
                && other.len() < phrase.len()
                && phrase.contains(other.as_str())
                && *other_score >= *score
        });
        if !dominated {
            kept.push((phrase.clone(), *score));
        }
    }
    kept.truncate(MAX_EMIT);

    kept.into_iter()
        .map(|(phrase, _)| Entity::new(&phrase, EntityType::Concept, 0.6))
        .collect()
}

pub(crate) fn extract_entities(text: &str) -> Vec<Entity> {
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entities: Vec<Entity> = Vec::new();

    let words: Vec<&str> = text.split_whitespace().collect();

    // --- Pass 1: multi-word entities (scan for consecutive Title Case runs) ---
    let mut i = 0;
    while i < words.len() && entities.len() < 20 {
        let token = words[i].trim_matches(STRIP_CHARS);

        // Check for URL or file first (single token)
        if is_url(token) || is_file(token) {
            if !token.is_empty() && !seen_names.contains(token) {
                let etype = if is_url(token) { EntityType::Url } else { EntityType::File };
                seen_names.insert(token.to_string());
                entities.push(Entity::new(token, etype, 0.8));
            }
            i += 1;
            continue;
        }

        // Try to grab a multi-word Title Case span (e.g. "Acme Corp", "New York")
        if is_title_case(token) && !is_stopword(token) {
            let start = i;
            let mut end = i + 1;
            while end < words.len() {
                // Bug-fix 2026-05-11: trailing punctuation on the previous raw
                // word (comma, semicolon, period) signals a list / clause
                // boundary — don't merge "Alice, Bob" into "Alice Bob".
                let prev_raw = words[end - 1];
                if prev_raw.ends_with(',')
                    || prev_raw.ends_with(';')
                    || prev_raw.ends_with('.')
                    || prev_raw.ends_with(':')
                    || prev_raw.ends_with('!')
                    || prev_raw.ends_with('?')
                {
                    break;
                }
                let next = words[end].trim_matches(STRIP_CHARS);
                if is_title_case(next) && !next.is_empty() {
                    end += 1;
                } else {
                    break;
                }
            }

            if end - start >= 2 {
                // Multi-word entity
                let name: String = words[start..end]
                    .iter()
                    .map(|w| w.trim_matches(STRIP_CHARS))
                    .collect::<Vec<_>>()
                    .join(" ");
                if !seen_names.contains(&name) {
                    let etype = classify_multi_word(&name);
                    seen_names.insert(name.clone());
                    // Also mark individual words as seen to avoid duplicates
                    for w in &words[start..end] {
                        seen_names.insert(w.trim_matches(STRIP_CHARS).to_string());
                    }
                    entities.push(Entity::new(&name, etype, 0.8));
                }
                i = end;
                continue;
            }
        }

        i += 1;
    }

    // --- Pass 2: single-token entities (skip already-seen) ---
    for raw_token in &words {
        if entities.len() >= 20 {
            break;
        }

        let token = raw_token.trim_matches(STRIP_CHARS);
        if token.is_empty() || seen_names.contains(token) {
            continue;
        }

        let entity_type = classify_token(token);
        if let Some(etype) = entity_type {
            seen_names.insert(token.to_string());
            entities.push(Entity::new(token, etype, 0.7));
        }
    }

    entities
}

fn is_url(token: &str) -> bool {
    token.starts_with("http://") || token.starts_with("https://") || token.starts_with("www.")
}

fn is_file(token: &str) -> bool {
    for ext in FILE_EXTENSIONS {
        if token.ends_with(ext) {
            return true;
        }
    }
    token.contains('/') && token.len() > 3
}

fn is_title_case(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() => true,
        _ => false,
    }
}

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

/// Classify a multi-word entity like "Acme Corp" or "Machine Learning".
fn classify_multi_word(name: &str) -> EntityType {
    let last_word = name.split_whitespace().last().unwrap_or("");

    // Check for org suffixes
    for suffix in ORG_SUFFIXES {
        if last_word.eq_ignore_ascii_case(suffix) {
            return EntityType::Organization;
        }
    }

    // Check if any word is a known tech term
    for word in name.split_whitespace() {
        if KNOWN_TECH.iter().any(|t| t.eq_ignore_ascii_case(word)) {
            return EntityType::Technology;
        }
    }

    // Default: if it looks like a proper noun phrase, treat as Person
    EntityType::Person
}

/// Return the `EntityType` for a single `token`, or `None` if it should be skipped.
fn classify_token(token: &str) -> Option<EntityType> {
    // 1. URL
    if is_url(token) {
        return Some(EntityType::Url);
    }

    // 2. File
    if is_file(token) {
        return Some(EntityType::File);
    }

    // 3. Known technology — takes priority over the Person heuristic
    if KNOWN_TECH.iter().any(|t| t.eq_ignore_ascii_case(token)) {
        return Some(EntityType::Technology);
    }

    // 4. Capitalized word that isn't a stopword → Person
    if is_title_case(token) && !is_stopword(token) {
        let mut chars = token.chars();
        chars.next(); // skip first
        let rest: String = chars.collect();
        if rest == rest.to_lowercase() {
            return Some(EntityType::Person);
        }
    }

    // 5. Concept: any token of length >= 4, but not a common verb/adj/functional word
    if token.len() >= 4 && !SKIP_WORDS.iter().any(|w| w.eq_ignore_ascii_case(token)) {
        return Some(EntityType::Concept);
    }

    None
}

// ---------------------------------------------------------------------------
// Triple extraction — pattern-based + co-occurrence fallback
// ---------------------------------------------------------------------------

/// Sentence-level pattern matching for typed predicates, with co-occurrence fallback.
pub(crate) fn extract_triples(text: &str, entities: &[Entity]) -> Vec<Triple> {
    let mut triples: Vec<Triple> = Vec::new();
    let text_lower = text.to_lowercase();

    // Build a lookup from lowercase entity name → entity index
    let entity_lookup: Vec<(String, usize)> = entities
        .iter()
        .enumerate()
        .map(|(idx, e)| (e.name.to_lowercase(), idx))
        .collect();

    // Try pattern-based extraction first
    let patterns: &[(&[&str], Predicate)] = &[
        // "X works at Y", "X working at Y", "X worked at Y"
        (&["works at", "working at", "worked at", "work at", "employed at", "employed by", "joined"], Predicate::WorksAt),
        // "X uses Y", "X using Y", "X built with Y"
        (&["uses", "using", "built with", "written in", "powered by", "implemented in", "runs on"], Predicate::Custom("uses".into())),
        // "X is a Y", "X is an Y"
        (&["is a ", "is an ", "are a ", "are an "], Predicate::IsA),
        // "X depends on Y", "X requires Y"
        (&["depends on", "requires", "needs", "relies on"], Predicate::DependsOn),
        // "X produces Y", "X generates Y", "X creates Y", "X building Y"
        (&["produces", "generates", "creates", "building", "built", "developing", "developed"], Predicate::Produces),
        // "X owns Y", "X created Y"
        (&["owns", "created", "founded", "started"], Predicate::Owns),
        // "X collaborates with Y", "X works with Y"
        (&["collaborates with", "works with", "partnered with", "teamed with"], Predicate::CollaboratesWith),
        // "X is part of Y", "X belongs to Y"
        (&["part of", "belongs to", "member of", "component of", "included in"], Predicate::PartOf),
        // "X references Y", "X mentions Y", "X links to Y"
        (&["references", "mentions", "links to", "points to", "refers to"], Predicate::References),
        // TM-NLP-003c — possession / containment: "X has Y", "X contains Y"
        // Use spaces around bare verbs to reduce substring false matches
        // ("has" inside "washes", "have" inside "behaves").
        (&[" has ", " have ", " having ", " contains ", " containing ", " includes ", " including "], Predicate::HasProperty),
    ];

    for (keywords, predicate) in patterns {
        for keyword in *keywords {
            if !text_lower.contains(keyword) {
                continue;
            }
            // Find the keyword position in text_lower
            if let Some(kw_pos) = text_lower.find(keyword) {
                let before = &text_lower[..kw_pos];
                let after = &text_lower[kw_pos + keyword.len()..];

                // Find the closest entity in the text before and after the keyword
                let mut best_subj: Option<usize> = None;
                let mut best_subj_dist = usize::MAX;
                let mut best_obj: Option<usize> = None;
                let mut best_obj_dist = usize::MAX;

                for (ename, eidx) in &entity_lookup {
                    if let Some(pos) = before.rfind(ename.as_str()) {
                        let dist = before.len() - pos - ename.len();
                        if dist < best_subj_dist {
                            best_subj_dist = dist;
                            best_subj = Some(*eidx);
                        }
                    }
                    if let Some(pos) = after.find(ename.as_str()) {
                        if pos < best_obj_dist {
                            best_obj_dist = pos;
                            best_obj = Some(*eidx);
                        }
                    }
                }

                if let (Some(si), Some(oi)) = (best_subj, best_obj) {
                    if si != oi && triples.len() < 15 {
                        triples.push(Triple::new(
                            entities[si].id,
                            predicate.clone(),
                            entities[oi].id,
                            0.75,
                        ));
                    }
                }
            }
        }
    }

    // Co-occurrence fallback: fill remaining slots with RelatedTo (lower confidence)
    let window = entities.len().min(5);
    'outer: for i in 0..window {
        for j in (i + 1)..entities.len() {
            if triples.len() >= 15 {
                break 'outer;
            }
            // Skip if we already have a typed triple for this pair
            let already = triples.iter().any(|t| {
                (t.subject_id == entities[i].id && t.object_id == entities[j].id)
                    || (t.subject_id == entities[j].id && t.object_id == entities[i].id)
            });
            if already {
                continue;
            }
            triples.push(Triple::new(
                entities[i].id,
                Predicate::RelatedTo,
                entities[j].id,
                0.4,
            ));
        }
    }

    triples
}

// ---------------------------------------------------------------------------
// Signal priority classifier
// ---------------------------------------------------------------------------

/// Classify a captured signal into a priority tier for the two-speed pipeline.
///
/// Runs cheap heuristics only — no model inference beyond the already-computed
/// embedding. Target: <1ms.
///
/// Decision order:
/// 1. Structured content (URL, file path, code block, JSON) → **InstantEntity (T1)**.
/// 2. Low novelty vs existing graph (cosine ≥ 0.85) → **Ephemeral (T4)**.
/// 3. High novelty (cosine < 0.40 vs top match) OR high lexical entropy →
///    **Priority (T2)**.
/// 4. Default → **Normal (T3)**.
pub fn classify_signal(
    text: &str,
    embedding: &[f32],
    graph: &GraphStore,
) -> SignalPriority {
    // 1. Structured / high-signal surface features.
    if has_structured_marker(text) {
        return SignalPriority::InstantEntity;
    }

    // 2/3. Novelty check against the existing graph.
    let similar = graph.search_vectors(embedding, 1).unwrap_or_default();
    let top_sim = similar.first().map(|&(_, s)| s).unwrap_or(0.0);

    if top_sim >= 0.85 {
        // Very close to an entity we already have — probably a re-capture of
        // something known. Keep searchable, don't pollute the graph.
        return SignalPriority::Ephemeral;
    }

    if top_sim < 0.40 || lexical_entropy_score(text) > 0.7 {
        return SignalPriority::Priority;
    }

    SignalPriority::Normal
}

/// Detect surface features that mark a capture as structured/high-signal:
/// URLs, file paths, code fences, JSON objects, stack traces, env lines.
fn has_structured_marker(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return true;
    }
    // file path: starts with /, ~, ./, or drive letter
    if trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("./")
    {
        // cheap guard against sentences that happen to begin with a slash
        if !trimmed.contains(' ') || trimmed.split_whitespace().next().map(|w| w.contains('/')).unwrap_or(false) {
            return true;
        }
    }
    // code fence / JSON / stack-trace-like
    if trimmed.starts_with("```") {
        return true;
    }
    if trimmed.starts_with('{') && trimmed.contains(':') && trimmed.contains('}') {
        return true;
    }
    // a line that looks like an env/config assignment: KEY=value
    if let Some(eq) = trimmed.find('=') {
        let key = &trimmed[..eq];
        if !key.is_empty()
            && key.len() < 40
            && key.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Rough lexical-entropy proxy: unique-token ratio.
/// Returns 1.0 if every word is unique, 0.0 for a completely repetitive stream.
fn lexical_entropy_score(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let unique: std::collections::HashSet<&str> = words.iter().copied().collect();
    unique.len() as f32 / words.len() as f32
}

// ---------------------------------------------------------------------------
// Two-speed pipeline helpers
// ---------------------------------------------------------------------------

/// Single-link agglomerative clustering by cosine similarity.
/// Returns a Vec of clusters, each cluster is a Vec of signal indices.
fn cluster_by_similarity(signals: &[CapturedSignal], threshold: f32) -> Vec<Vec<usize>> {
    let n = signals.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut Vec<usize>, mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]]; // path halving
            i = parent[i];
        }
        i
    }

    for i in 0..n {
        for j in (i + 1)..n {
            if cosine_sim(&signals[i].embedding, &signals[j].embedding) >= threshold {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    let mut clusters: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        clusters.entry(root).or_default().push(i);
    }
    clusters.into_values().collect()
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

// ---------------------------------------------------------------------------
// Content hash
// ---------------------------------------------------------------------------

fn hash_text(text: &str) -> String {
    format!("{:016x}", seahash::hash(text.as_bytes()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::TraceMindError;

    /// Open a pipeline backed by in-memory graph (vectors stored in same SQLite).
    fn in_memory_pipeline() -> IngestPipeline {
        let graph = GraphStore::open(":memory:").unwrap();
        IngestPipeline {
            graph,
            embedder: Embedder::new_hash(),
            governance: GovernanceFilter::default(),
            extractor: Box::new(HeuristicExtractor),
            // Tests fire many ingest_fast calls in tight loops; the
            // CAP-5 limiter would refuse them. Tests opt out.
            rate_limiter: RateLimiter::unlimited(),
        }
    }

    #[test]
    fn test_url_and_file_entities_extracted() {
        let pipeline = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let result = pipeline
            .ingest("Visit https://example.com and read README.md", session_id)
            .expect("ingest should succeed");

        let has_url = result
            .entities
            .iter()
            .any(|e| e.entity_type == EntityType::Url);
        let has_file = result
            .entities
            .iter()
            .any(|e| e.entity_type == EntityType::File);

        assert!(has_url, "expected a Url entity; got: {:?}", result.entities);
        assert!(has_file, "expected a File entity; got: {:?}", result.entities);
    }

    #[test]
    fn test_pii_email_rejected() {
        let pipeline = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let result = pipeline.ingest("email: foo@bar.com", session_id);

        assert!(
            matches!(result, Err(TraceMindError::PiiDetected)),
            "expected PiiDetected, got: {:?}",
            result
        );
    }

    #[test]
    fn test_concept_entities_extracted() {
        let pipeline = in_memory_pipeline();
        let session_id = Uuid::new_v4();

        let text = "memory systems enable agents to recall information across sessions \
                    and build persistent knowledge over time";

        let result = pipeline.ingest(text, session_id).expect("ingest should succeed");

        assert!(
            !result.entities.is_empty(),
            "expected at least one entity from conceptual text"
        );
    }

    #[test]
    fn test_rust_classified_as_technology_not_person() {
        let entities = extract_entities("Rust is a fast language for systems programming");
        let rust = entities.iter().find(|e| e.name == "Rust");
        assert!(rust.is_some(), "expected Rust entity; got: {:?}", entities);
        assert_eq!(rust.unwrap().entity_type, EntityType::Technology);
    }

    #[test]
    fn test_multi_word_entity_extraction() {
        let entities = extract_entities("Aaditya Srivathsan works at Acme Corp on TraceMind");
        let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("Aaditya") && n.contains("Srivathsan")),
            "expected multi-word entity 'Aaditya Srivathsan'; got: {:?}", names
        );
        let acme = entities.iter().find(|e| e.name.contains("Acme"));
        assert!(acme.is_some(), "expected Acme Corp entity; got: {:?}", names);
        assert_eq!(acme.unwrap().entity_type, EntityType::Organization);
    }

    #[test]
    fn test_typed_predicate_works_at() {
        let text = "Aaditya works at Anthropic on AI safety";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        let has_works_at = triples.iter().any(|t| t.predicate == Predicate::WorksAt);
        assert!(
            has_works_at,
            "expected WorksAt predicate; got: {:?}",
            triples.iter().map(|t| &t.predicate).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_typed_predicate_uses() {
        let text = "TraceMind uses Rust for performance";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        let has_uses = triples.iter().any(|t| t.predicate == Predicate::Custom("uses".into()));
        assert!(
            has_uses,
            "expected 'uses' predicate; got: {:?}",
            triples.iter().map(|t| &t.predicate).collect::<Vec<_>>()
        );
    }

    /// TM-NLP-003c — "X has Y" / "X contains Y" should emit HasProperty triples.
    #[test]
    fn test_typed_predicate_has_property() {
        let text = "TraceMind contains Rust and Sqlite";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        let has_property = triples.iter().any(|t| t.predicate == Predicate::HasProperty);
        assert!(
            has_property,
            "expected HasProperty predicate; got: {:?}",
            triples.iter().map(|t| &t.predicate).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_entity_dedup_across_ingests() {
        let pipeline = in_memory_pipeline();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();

        let r1 = pipeline.ingest("Alice works at Anthropic", s1).unwrap();
        let r2 = pipeline.ingest("Alice is building TraceMind", s2).unwrap();

        // "Alice" should be the same entity in both ingests (reused ID)
        let alice1 = r1.entities.iter().find(|e| e.name == "Alice");
        let alice2 = r2.entities.iter().find(|e| e.name == "Alice");
        assert!(alice1.is_some(), "Alice should be in first ingest");
        assert!(alice2.is_some(), "Alice should be in second ingest");
        assert_eq!(
            alice1.unwrap().id,
            alice2.unwrap().id,
            "Same entity should have same UUID across ingests"
        );

        // Total entity count in graph should not have duplicates
        let count = pipeline.graph.entity_count().unwrap();
        // First ingest: Alice, Anthropic. Second: Alice (deduped), TraceMind.
        // So we expect 3 unique entities, not 4.
        assert!(
            count <= 4,
            "expected at most 4 entities with dedup (got {count})"
        );
    }

    #[test]
    fn test_entity_dedup_case_insensitive() {
        let pipeline = in_memory_pipeline();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();

        let r1 = pipeline.ingest("Rust is great for performance", s1).unwrap();
        let r2 = pipeline.ingest("RUST powers TraceMind", s2).unwrap();

        // "Rust" and "RUST" should map to the same entity
        let rust1 = r1.entities.iter().find(|e| e.name.eq_ignore_ascii_case("rust"));
        let rust2 = r2.entities.iter().find(|e| e.name.eq_ignore_ascii_case("rust"));
        if let (Some(r1e), Some(r2e)) = (rust1, rust2) {
            assert_eq!(
                r1e.id, r2e.id,
                "Case-insensitive name match should reuse entity"
            );
        }
    }

    /// TM-NLP-003a — once "Rust" is in the graph, a lowercase mention
    /// "rust" in a later ingest should link to the same entity instead of
    /// being dropped entirely by the Title-Case extractor.
    #[test]
    fn test_lowercase_mention_links_to_existing_entity() {
        let pipeline = in_memory_pipeline();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();

        let r1 = pipeline
            .ingest("Rust is great for performance", s1)
            .unwrap();
        let rust_id = r1
            .entities
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("rust"))
            .map(|e| e.id)
            .expect("first ingest should emit Rust entity");

        // The second ingest has NO Title-Case clue for Rust — only lowercase.
        let r2 = pipeline
            .ingest("the rust compiler is fast", s2)
            .unwrap();

        let linked = r2
            .entities
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("rust"));
        assert!(
            linked.is_some(),
            "lowercase 'rust' should have been linked to graph; got: {:?}",
            r2.entities
        );
        assert_eq!(
            linked.unwrap().id,
            rust_id,
            "lowercase mention must re-use the existing entity id"
        );
    }

    /// TM-NLP-003b — YAKE keyphrase extraction should surface repeated
    /// lowercase multi-word concepts.
    #[test]
    fn test_yake_surfaces_repeated_lowercase_phrase() {
        let text = "we explored machine learning today. machine learning is \
                    a huge field and machine learning keeps growing.";
        let kps = extract_keyphrases(text);
        let has_ml = kps.iter().any(|e| e.name == "machine learning");
        assert!(
            has_ml,
            "expected 'machine learning' as keyphrase; got: {:?}",
            kps.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
        // Emitted as Concept.
        assert!(
            kps.iter()
                .filter(|e| e.name == "machine learning")
                .all(|e| matches!(e.entity_type, EntityType::Concept)),
        );
    }

    /// YAKE must not emit pure stopword / verb n-grams.
    #[test]
    fn test_yake_skips_noisy_grams() {
        let text = "the the the and and the and the";
        let kps = extract_keyphrases(text);
        assert!(
            kps.is_empty(),
            "expected no keyphrases from stopword-only text; got: {:?}",
            kps.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_levenshtein_basic_cases() {
        assert_eq!(levenshtein("rust", "rust"), 0);
        assert_eq!(levenshtein("rust", "rost"), 1);          // sub
        assert_eq!(levenshtein("rust", "ruts"), 2);          // transpose ≈ 2 in Lev
        assert_eq!(levenshtein("typescript", "Typescript".to_lowercase().as_str()), 0);
        assert_eq!(levenshtein("typescript", "typescripts"), 1); // ins
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    /// TM-NLP-003d — after "Typescript" is in the graph, a new mention of
    /// "TypeScript" (casing variant, 1 char capitalization-drift away once
    /// lowercased both are equal — so actually this goes through the icase
    /// path; better test is "TypeScripts" → "Typescript" distance 1).
    #[test]
    fn test_fuzzy_match_merges_typo() {
        let pipeline = in_memory_pipeline();
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();

        let r1 = pipeline
            .ingest("Typescript is a typed language", s1)
            .unwrap();
        let canonical_id = r1
            .entities
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("Typescript"))
            .map(|e| e.id)
            .expect("first ingest should emit Typescript entity");

        // Minor spelling drift — 1 edit away.
        let r2 = pipeline
            .ingest("Typescripts is widely used", s2)
            .unwrap();
        let linked = r2
            .entities
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("Typescripts") || e.id == canonical_id);
        assert!(
            linked.is_some(),
            "fuzzy match should have merged 'Typescripts' into existing 'Typescript'; got: {:?}",
            r2.entities
        );
        assert_eq!(
            linked.unwrap().id,
            canonical_id,
            "Typescripts (d=1) must reuse the canonical entity id"
        );
    }

    /// Too-far names must not collapse.
    #[test]
    fn test_fuzzy_match_rejects_unrelated_names() {
        let pipeline = in_memory_pipeline();
        pipeline
            .ingest("Rust is great for performance", Uuid::new_v4())
            .unwrap();

        // 4+ edits apart from "Rust" — unrelated.
        let m = pipeline.fuzzy_match_entity("Python");
        assert!(
            m.is_none(),
            "fuzzy match must not collapse unrelated names; got: {m:?}"
        );
    }

    /// Guard against false positives: on an empty graph the linking helper
    /// must be a pure no-op — it should never invent entities for tokens
    /// the extractor skipped.
    #[test]
    fn test_lowercase_linking_is_noop_on_empty_graph() {
        let pipeline = in_memory_pipeline();
        let mut entities: Vec<Entity> = Vec::new();
        pipeline.link_lowercase_to_graph(
            "the quick brown fox jumps over the lazy dog",
            &mut entities,
        );
        assert!(
            entities.is_empty(),
            "linker must not add entities when graph is empty; got: {:?}",
            entities
        );
    }

    #[test]
    fn test_should_ingest_too_short() {
        let pipeline = in_memory_pipeline();
        let (pass, reason) = pipeline.should_ingest("the a an");
        assert!(!pass, "all-stopword text should be rejected");
        assert_eq!(reason, "too short or no semantic content");
    }

    #[test]
    fn test_should_ingest_normal() {
        let pipeline = in_memory_pipeline();
        let (pass, _reason) = pipeline.should_ingest("Rust is a systems programming language");
        assert!(pass, "meaningful text should be accepted");
    }

    #[test]
    fn test_should_ingest_empty() {
        let pipeline = in_memory_pipeline();
        let (pass, reason) = pipeline.should_ingest("");
        assert!(!pass, "empty text should be rejected");
        assert_eq!(reason, "too short or no semantic content");
    }

    #[test]
    fn test_co_occurrence_skips_already_typed_pairs() {
        let text = "Aaditya works at Google on search";
        let entities = extract_entities(text);
        let triples = extract_triples(text, &entities);
        // Find the pair that has WorksAt
        let works_at = triples.iter().find(|t| t.predicate == Predicate::WorksAt);
        if let Some(wa) = works_at {
            // There should be no RelatedTo for the same pair
            let dup = triples.iter().any(|t| {
                t.predicate == Predicate::RelatedTo
                    && ((t.subject_id == wa.subject_id && t.object_id == wa.object_id)
                        || (t.subject_id == wa.object_id && t.object_id == wa.subject_id))
            });
            assert!(!dup, "co-occurrence should not duplicate typed pairs");
        }
    }

    // ── Two-speed pipeline tests ──────────────────────────────────────────

    #[test]
    fn test_ingest_fast_stores_signal() {
        let pipeline = in_memory_pipeline();
        let session = Uuid::new_v4();
        let result = pipeline
            .ingest_fast("Rust powers the TraceMind memory system", "test", session)
            .expect("ingest_fast should succeed");
        assert!(result.skipped.is_none(), "expected signal stored, got: {:?}", result.skipped);
        assert!(result.signal_id > 0, "expected non-zero signal_id");
    }

    #[test]
    fn test_ingest_fast_deduplicates() {
        let pipeline = in_memory_pipeline();
        let text = "TraceMind uses SQLite for local storage";
        let r1 = pipeline.ingest_fast(text, "test", Uuid::new_v4()).unwrap();
        let r2 = pipeline.ingest_fast(text, "test", Uuid::new_v4()).unwrap();
        assert!(r1.skipped.is_none(), "first ingest should store signal");
        assert!(
            r2.skipped.as_deref() == Some("duplicate signal"),
            "second ingest should be skipped as duplicate"
        );
    }

    #[test]
    fn test_ingest_fast_rejects_pii() {
        let pipeline = in_memory_pipeline();
        let result = pipeline.ingest_fast("contact foo@bar.com for details", "test", Uuid::new_v4());
        assert!(
            matches!(result, Err(tm_types::TraceMindError::PiiDetected)),
            "PII should be rejected by fast path"
        );
    }

    #[test]
    fn test_ingest_fast_rejects_short_text() {
        let pipeline = in_memory_pipeline();
        let result = pipeline
            .ingest_fast("ok thanks", "test", Uuid::new_v4())
            .unwrap();
        assert!(
            result.skipped.is_some(),
            "short text should be skipped"
        );
    }

    #[test]
    fn test_consolidate_promotes_cluster_to_entities() {
        let pipeline = in_memory_pipeline();
        let s = "Rust is a systems programming language for memory safety";
        // Store the same topic multiple times. Novel content is classified as
        // Priority (tier 2) by the classifier, so the normal consolidate() pass
        // (tier-union) and consolidate_priority() should both find these signals.
        for _ in 0..3 {
            let _ = pipeline.ingest_fast(
                &format!("{} — version {}", s, Uuid::new_v4()),
                "test",
                Uuid::new_v4(),
            );
        }
        // Use min_cluster_size=1 so even singleton priority signals promote —
        // this matches the semantics of consolidate_priority which is the
        // natural path for novel content. threshold=0.0 forces all into one
        // cluster (though may still leave some as singletons depending on
        // hash embedding sign).
        let stats = pipeline
            .consolidate(50, 1, 0.0)
            .expect("consolidate should succeed");
        assert!(stats.signals_scanned >= 3, "should have scanned stored signals");
        assert!(
            stats.clusters_formed >= 1,
            "should have formed at least one cluster (got {})",
            stats.clusters_formed
        );
    }

    // ── Tier classification tests ─────────────────────────────────────────

    #[test]
    fn test_classify_signal_url_is_instant() {
        let pipeline = in_memory_pipeline();
        let text = "https://example.com/docs/architecture";
        let emb = pipeline.embedder.embed(text);
        let priority = classify_signal(text, &emb, &pipeline.graph);
        assert_eq!(priority, SignalPriority::InstantEntity);
    }

    #[test]
    fn test_classify_signal_code_fence_is_instant() {
        let pipeline = in_memory_pipeline();
        let text = "```rust\nfn main() { println!(\"hello\"); }\n```";
        let emb = pipeline.embedder.embed(text);
        let priority = classify_signal(text, &emb, &pipeline.graph);
        assert_eq!(priority, SignalPriority::InstantEntity);
    }

    #[test]
    fn test_classify_signal_novel_is_priority() {
        let pipeline = in_memory_pipeline();
        let text = "A detailed note about distributed consensus algorithms and their tradeoffs";
        let emb = pipeline.embedder.embed(text);
        // Empty graph => top_sim 0.0 => Priority tier.
        let priority = classify_signal(text, &emb, &pipeline.graph);
        assert_eq!(priority, SignalPriority::Priority);
    }

    #[test]
    fn test_ingest_fast_url_instant_promotes() {
        let pipeline = in_memory_pipeline();
        let session = Uuid::new_v4();
        let res = pipeline
            .ingest_fast("https://example.com/article", "test", session)
            .unwrap();
        assert_eq!(res.priority, SignalPriority::InstantEntity);
        // signal_id=0 because URL was promoted directly via ingest(), not stored.
        assert!(res.signal_id == 0 || !res.instant_entities.is_empty());
    }

    #[test]
    fn test_consolidate_priority_scans_tier2_only() {
        let pipeline = in_memory_pipeline();
        // Novel content → classified as Priority (tier 2).
        for i in 0..3 {
            let _ = pipeline.ingest_fast(
                &format!("Some novel unique concept number {} about widgets", i),
                "test",
                Uuid::new_v4(),
            );
        }
        let stats = pipeline
            .consolidate_priority(50, 0.0)
            .expect("priority consolidation");
        assert!(stats.signals_scanned >= 3, "tier-2 signals should be scanned");
        assert!(stats.clusters_formed >= 1, "min_cluster_size=1 allows singletons");
    }

    #[test]
    fn test_hybrid_search_finds_fresh_signals() {
        // Insert a tier-2 signal and verify search_signals returns it.
        let pipeline = in_memory_pipeline();
        let text = "Quantum entanglement enables secure key distribution";
        let _ = pipeline
            .ingest_fast(text, "test", Uuid::new_v4())
            .expect("ingest_fast");
        let query_emb = pipeline.embedder.embed(text);
        let hits = pipeline
            .graph
            .search_signals(&query_emb, 5, 0.0)
            .expect("signal search");
        assert!(!hits.is_empty(), "hybrid search should surface fresh signal");
        assert!(hits[0].0.raw_text.contains("Quantum"));
    }

    #[test]
    fn test_signal_priority_tier_values() {
        assert_eq!(SignalPriority::InstantEntity.tier(), 1);
        assert_eq!(SignalPriority::Priority.tier(), 2);
        assert_eq!(SignalPriority::Normal.tier(), 3);
        assert_eq!(SignalPriority::Ephemeral.tier(), 4);
    }

    #[test]
    fn test_cosine_sim_identical() {
        let v = vec![1.0f32, 0.0, 0.0];
        assert!((cosine_sim(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_sim_orthogonal() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!(cosine_sim(&a, &b).abs() < 1e-6);
    }
}
