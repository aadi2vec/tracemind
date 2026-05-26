// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};
use uuid::Uuid;

use tm_controller::UcbBandit;
use tm_episodic::TraceStore;
use tm_graph::GraphStore;
use tm_ingest::{IngestPipeline, TripleJob, TripleWorker, TripleWorkerHandle, WorkerDb};
use tm_retrieval::RetrievalEngine;

mod sprint_commands;
mod wme_commands;

#[cfg(feature = "local-llm")]
mod community_label_llm;

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct AppState {
    db_path: String,
    #[allow(dead_code)]
    trace_path: String,
    bandit_path: PathBuf,
    ingest: Arc<Mutex<IngestPipeline>>,
    retrieval: Mutex<RetrievalEngine>,
    trace_store: Arc<Mutex<TraceStore>>,
    capture_enabled: Arc<Mutex<bool>>,
    /// LM-8 — async triple-extraction worker. The hot ingest path
    /// enqueues a [`TripleJob`] after fast-path entity extraction
    /// returns; the worker thread runs the (slower) triple extractor
    /// + `route_triple_by_confidence` off the main thread so the UI
    /// stays snappy. `Arc` so capture/ingest paths can share one
    /// handle.
    triple_worker: Arc<TripleWorkerHandle>,
    /// Whether the Qwen LLM triple extractor is loaded for this process.
    /// Surfaced to the UI via `cmd_llm_status` so the user can tell when
    /// they're getting heuristic-only relation extraction vs the LLM tier.
    llm_active: Arc<std::sync::atomic::AtomicBool>,
    /// CTX-EVG Slice A — the currently "active" thread. Mutated by
    /// `cmd_thread_start` / `cmd_thread_end`; read by `cmd_ingest` and
    /// `cmd_query` so that every capture/query event written to
    /// `event_nodes` carries the right `thread_id`. `None` means
    /// captures/queries are unthreaded (the legacy behavior).
    active_thread_id: Arc<Mutex<Option<Uuid>>>,
}

// ---------------------------------------------------------------------------
// IPC response types
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct EntityInfo {
    id: String,
    name: String,
    entity_type: String,
    confidence: f64,
    /// 2026-05-11 UX (#1) — the originating context this entity was
    /// ingested under, resolved to a human-readable name. `None` if
    /// the entity is unscoped (legacy pre-Sprint-C-0 data) or the
    /// store has no contexts at all. Surfaced as a small pill in the
    /// QueryView entity rows so the user always sees *which*
    /// project/venture a hit came from.
    context_name: Option<String>,
}

#[derive(Serialize, Clone)]
struct TripleInfo {
    subject: String,
    predicate: String,
    object: String,
    confidence: f64,
}

#[derive(Serialize)]
struct IngestResponse {
    trace_id: String,
    entities: Vec<EntityInfo>,
    typed_triples: Vec<TripleInfo>,
    co_occurrence_count: usize,
}

#[derive(Serialize, Clone)]
struct AttributionInfo {
    entity_id: String,
    entity_name: String,
    source: String,
    weight: f64,
}

#[derive(Serialize)]
struct QueryResponse {
    /// Sprint C-0.7 / F-1 — stable id of this query for `helpful` /
    /// `not_related` feedback. Frontend stashes this with every row so
    /// inline buttons can file the right `positive_signals` /
    /// `negative_signals` row against the originating bandit pull.
    query_id: String,
    arm: u8,
    arm_name: String,
    latency_ms: u32,
    entities: Vec<EntityInfo>,
    triples: Vec<TripleInfo>,
    recommendations: Vec<RecommendationInfo>,
    explanation: String,
    attributions: Vec<AttributionInfo>,
}

#[derive(Serialize)]
struct TraceInfo {
    id: String,
    event_type: String,
    raw_text: String,
    entities_count: usize,
    triples_count: usize,
    retrieval_arm: Option<u8>,
    retrieval_arm_name: Option<String>,
    retrieval_latency_ms: Option<u32>,
    created_at: String,
}

#[derive(Serialize)]
struct DashboardStats {
    entity_count: usize,
    triple_count: usize,
    trace_count: usize,
    bandit_arms: Vec<BanditArmInfo>,
    recent_traces: Vec<TraceInfo>,
}

#[derive(Serialize)]
struct BanditArmInfo {
    arm: u8,
    name: String,
    pulls: u64,
    avg_reward: f64,
}

#[derive(Serialize)]
struct GraphNode {
    id: String,
    name: String,
    entity_type: String,
    confidence: f64,
    community: Option<i32>,
}

#[derive(Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    predicate: String,
    confidence: f64,
}

#[derive(Serialize)]
struct GraphData {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    community_count: usize,
    /// 2026-05-12 — searchable community labels. Maps stringified
    /// community_id → a "Top1 · Top2 · Top3" string built from the
    /// highest-degree entity names in that community. Lets the user
    /// scan the legend and know what each community *is* instead of
    /// reading "Community 0" / "Community 1" / ...
    community_labels: HashMap<String, String>,
}

#[derive(Serialize)]
struct SurprisingEntity {
    entity_id: String,
    entity_name: String,
    entity_type: String,
    novelty: f64,
    recency: f64,
    score: f64,
}

#[derive(Serialize)]
struct DemoResult {
    texts_ingested: usize,
    total_entities: usize,
    total_triples: usize,
}

#[derive(Serialize)]
struct RecommendationInfo {
    entity_id: String,
    entity_name: String,
    entity_type: String,
    score: f64,
    reason: String,
    /// 2026-05-11 UX (#3) — structured breakdown ("PR 0.72 · 86%
    /// match"). Subtitle below the `reason` headline so the user can
    /// see *which* signals fired, not just the label.
    reason_detail: String,
    /// 2026-05-11 UX (#1) — the query text that seeded this rec.
    /// `None` on cold start (rec came from recent ingest, not a
    /// query).
    origin_query: Option<String>,
    /// Human-readable name of the context this rec was computed
    /// under. `None` if unscoped.
    origin_context: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// Delegate to `tm_controller::UcbBandit::arm_name` so the UI label
// stays in sync with the bandit's real arm count (NUM_ARMS = 6 incl.
// the subgraph_colbert arm). The old hand-written 5-element table
// produced "Unknown" for arm 5 in the Calibration view.
fn arm_name(arm: u8) -> String {
    UcbBandit::arm_name(arm).to_string()
}

/// Build a `Uuid → context_name` lookup map from the graph. Used by
/// `cmd_query` / `cmd_ingest` to populate the `context_name` field on
/// every returned `EntityInfo`. Cheap: contexts are small (< 100 rows
/// in practice) and the call is one SELECT.
/// 2026-05-12 — build a "Top1 · Top2 · Top3" label from an ordered list
/// of entity names (caller passes them already ranked by degree /
/// recency). Returns the empty string for an empty input. Names are
/// taken verbatim — community labels read best when they're the actual
/// entities ("Aaditya · TraceMind · Rust") rather than tokenised
/// keywords. Used by both `cmd_community_overlay` and (indirectly)
/// `cmd_graph`.
fn label_from_names(names: &[String]) -> String {
    let mut seen = std::collections::HashSet::new();
    let picked: Vec<String> = names
        .iter()
        .filter(|n| !n.trim().is_empty())
        .filter(|n| seen.insert(n.to_lowercase()))
        .take(3)
        .cloned()
        .collect();
    picked.join(" · ")
}

fn context_name_map(graph: &GraphStore) -> HashMap<Uuid, String> {
    graph
        .list_contexts()
        .map(|ctxs| ctxs.into_iter().map(|c| (c.id, c.name)).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// IPC commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn cmd_ingest(text: String, state: State<AppState>) -> Result<IngestResponse, String> {
    let pipeline = state.ingest.lock().map_err(|e| e.to_string())?;
    let session_id = Uuid::new_v4();
    let result = pipeline.ingest(&text, session_id).map_err(|e| e.to_string())?;

    // Persist trace
    let trace_store = state.trace_store.lock().map_err(|e| e.to_string())?;
    let _ = trace_store.append(&result.trace);

    // CTX-EVG Slice A — write a `Capture` event node tied to the
    // currently-active thread (if any). `payload_ref = trace_id` so
    // downstream consumers can deref to the full ingest payload via
    // the trace store. `salience` = mean entity confidence as a coarse
    // proxy until WME-2 ships a real salience scorer. Best-effort: a
    // failure here must NOT break the ingest IPC contract.
    {
        let active = state.active_thread_id.lock().ok().and_then(|g| *g);
        let mean_conf = if result.entities.is_empty() {
            0.0
        } else {
            result.entities.iter().map(|e| e.confidence).sum::<f64>()
                / result.entities.len() as f64
        };
        if let Ok(g) = GraphStore::open(&state.db_path) {
            let mut node = tm_graph::event_graph::EventNode::new(
                tm_graph::event_graph::EventNodeKind::Capture,
                result.trace.id.to_string(),
            );
            node.thread_id = active;
            node.salience = mean_conf;
            let _ = tm_graph::event_graph::EventGraphStore::insert_node(g.connection(), &node);

            // CTX-EVG-C — if the captured text reads like a commitment,
            // also write a `Commitment` event node so the Ledger surface
            // has something to count. Auto-create only at ≥0.85 confidence
            // (the "no-friction" path from the spec); below that the UI
            // can later propose. Best-effort — never breaks ingest.
            if let Some(cand) = tm_ingest::detect_commitment(&text) {
                if cand.confidence >= 0.85 {
                    let mut commit_node =
                        tm_graph::event_graph::EventNode::commitment(
                            result.trace.id.to_string(),
                            cand.due_at,
                        );
                    commit_node.thread_id = active;
                    commit_node.salience = cand.confidence as f64;
                    let _ = tm_graph::event_graph::EventGraphStore::insert_node(
                        g.connection(),
                        &commit_node,
                    );
                }
            }

            // Opportunistic sweep so an overdue commitment surfaces as
            // `broken` without a separate cron. Cheap (single indexed
            // UPDATE) — runs on every ingest.
            let _ = tm_graph::event_graph::EventGraphStore::sweep_broken(
                g.connection(),
                chrono::Utc::now().timestamp_millis(),
            );
        }
    }

    // LM-8 — kick the async triple worker. Best-effort: if the
    // channel is full we drop the job (counter tracks the backpressure
    // event). The synchronous pipeline above already produced the
    // co-occurrence triples; this worker re-runs the (slower) typed
    // extractor + `route_triple_by_confidence` off the hot path so
    // mid-confidence triples accumulate in the pending pool over time.
    let _ = state.triple_worker.try_enqueue(TripleJob {
        text: text.clone(),
        entities: result.entities.clone(),
        session_id,
    });

    // 2026-05-11 — mine intent phrases from the ingested text and persist
    // any hits as `pending` candidates. The capture daemon already does
    // this on clipboard / shell events; doing it here too means any text
    // the user types into the Query/Ingest view also feeds the Confirm
    // cards on the dashboard. Errors are best-effort — we never want a
    // miner failure to break ingest.
    let mined = tm_intent::mine(&text);
    if !mined.is_empty() {
        let dir = data_dir(&state);
        let intents_path = dir.join("intents.db");
        if let Ok(mut store) =
            tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
        {
            let records: Vec<tm_intent::store::CandidateRecord> = mined
                .iter()
                .map(|m| tm_intent::store::CandidateRecord::from_mined(m, text.clone()))
                .collect();
            let _ = store.insert_candidates(&records);
        }
    }

    // Build name lookup
    let name_of: HashMap<Uuid, &str> = result.entities.iter()
        .map(|e| (e.id, e.name.as_str())).collect();

    // Resolve context-name pills for the returned entities so the UI
    // can show "[dev] Alice" without a second IPC round-trip. One
    // shared graph handle, one contexts query, then per-entity
    // property lookups (cheap — already in memory).
    let entities: Vec<EntityInfo> = match GraphStore::open(&state.db_path) {
        Ok(g) => {
            let ctx_names = context_name_map(&g);
            result.entities.iter().map(|e| {
                let context_name = g.entity_context_id(e.id).ok().flatten()
                    .and_then(|id| ctx_names.get(&id).cloned());
                EntityInfo {
                    id: e.id.to_string(),
                    name: e.name.clone(),
                    entity_type: format!("{}", e.entity_type),
                    confidence: e.confidence,
                    context_name,
                }
            }).collect()
        }
        Err(_) => result.entities.iter().map(|e| EntityInfo {
            id: e.id.to_string(),
            name: e.name.clone(),
            entity_type: format!("{}", e.entity_type),
            confidence: e.confidence,
            context_name: None,
        }).collect(),
    };

    let mut typed_triples = Vec::new();
    let mut co_occurrence_count = 0;
    for t in &result.triples {
        if matches!(t.predicate, tm_types::Predicate::RelatedTo) {
            co_occurrence_count += 1;
        } else {
            typed_triples.push(TripleInfo {
                subject: name_of.get(&t.subject_id).unwrap_or(&"?").to_string(),
                predicate: format!("{}", t.predicate),
                object: name_of.get(&t.object_id).unwrap_or(&"?").to_string(),
                confidence: t.confidence,
            });
        }
    }

    Ok(IngestResponse {
        trace_id: result.trace.id.to_string(),
        entities,
        typed_triples,
        co_occurrence_count,
    })
}

#[tauri::command]
fn cmd_query(text: String, state: State<AppState>) -> Result<QueryResponse, String> {
    bump_usage(&state, |s| {
        s.last_query_at = Some(chrono::Utc::now().to_rfc3339());
        s.total_queries = s.total_queries.saturating_add(1);
    });
    let mut engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    // Refresh the engine's UUID↔skg-id cache from disk before querying.
    // Without this, any entity written by the external `tracemind-capture`
    // daemon (separate process, same SQLite file) is filtered out by
    // `search_vectors` because its UUID isn't in this engine's in-memory map.
    // See tm-graph::store::reload_maps doc comment.
    let _ = engine.refresh_graph();
    let result = engine.query(&text).map_err(|e| e.to_string())?;

    // CTX-EVG Slice A — write a `Query` event node tied to the
    // currently-active thread. `payload_ref = query_id` so consumers
    // can join back to the recorded trace + arm choice. Salience is
    // the bandit arm number (normalized) — close enough as a coarse
    // proxy until WME-2 ships a real query-salience scorer.
    {
        let active = state.active_thread_id.lock().ok().and_then(|g| *g);
        if let Ok(g) = GraphStore::open(&state.db_path) {
            let mut node = tm_graph::event_graph::EventNode::new(
                tm_graph::event_graph::EventNodeKind::Query,
                result.query_id.to_string(),
            );
            node.thread_id = active;
            node.salience = (result.arm as f64) / 4.0; // 5 arms (0..=4) → [0,1]
            let _ = tm_graph::event_graph::EventGraphStore::insert_node(g.connection(), &node);
        }
    }

    let name_of: HashMap<Uuid, &str> = result.entities.iter()
        .map(|e| (e.id, e.name.as_str())).collect();

    // Context-name lookup shared between entities and recommendations.
    // The retrieval engine holds its own GraphStore but doesn't expose
    // `entity_context_id` over its public API yet; opening a separate
    // read-only handle is cheap (SQLite WAL — no contention).
    let aux_graph = GraphStore::open(&state.db_path).ok();
    let ctx_names: HashMap<Uuid, String> = aux_graph
        .as_ref()
        .map(context_name_map)
        .unwrap_or_default();
    let ctx_name_for = |entity_id: Uuid| -> Option<String> {
        aux_graph.as_ref()
            .and_then(|g| g.entity_context_id(entity_id).ok().flatten())
            .and_then(|cid| ctx_names.get(&cid).cloned())
    };

    let entities: Vec<EntityInfo> = result.entities.iter().map(|e| EntityInfo {
        id: e.id.to_string(),
        name: e.name.clone(),
        entity_type: format!("{}", e.entity_type),
        confidence: e.confidence,
        context_name: ctx_name_for(e.id),
    }).collect();

    let triples: Vec<TripleInfo> = result.triples.iter().filter_map(|t| {
        let s = name_of.get(&t.subject_id)?;
        let o = name_of.get(&t.object_id)?;
        Some(TripleInfo {
            subject: s.to_string(),
            predicate: format!("{}", t.predicate),
            object: o.to_string(),
            confidence: t.confidence,
        })
    }).collect();

    // Generate recommendations from the query context. Resolve the
    // origin_context_id UUID → context_name once per response so the
    // UI doesn't need a second IPC round-trip.
    let recs = engine.recommendations(5);
    let recommendations: Vec<RecommendationInfo> = recs.into_iter().map(|r| {
        let origin_context = r.origin_context_id.as_ref()
            .and_then(|id_str| Uuid::parse_str(id_str).ok())
            .and_then(|id| ctx_names.get(&id).cloned());
        RecommendationInfo {
            entity_id: r.entity_id,
            entity_name: r.entity_name,
            entity_type: r.entity_type,
            score: r.score,
            reason: r.reason,
            reason_detail: r.reason_detail,
            origin_query: r.origin_query,
            origin_context,
        }
    }).collect();

    // Build causal attribution info
    let explanation = result.causal_trace.explain();
    let attributions: Vec<AttributionInfo> = result.causal_trace.top_attributions(10)
        .into_iter()
        .map(|a| {
            let source = match &a.source {
                tm_reason::causal::AttributionSource::VectorMatch { similarity, rank } =>
                    format!("vector (rank #{}, sim {:.0}%)", rank + 1, similarity * 100.0),
                tm_reason::causal::AttributionSource::GraphHop { predicate, hop, .. } =>
                    format!("graph hop #{} via {}", hop, predicate),
                tm_reason::causal::AttributionSource::EpisodicTrace { trace_id } =>
                    format!("episodic ({})", &trace_id[..8.min(trace_id.len())]),
                tm_reason::causal::AttributionSource::ReasoningChain { chain_score, path_length } =>
                    format!("reasoning (len {}, score {:.0}%)", path_length, chain_score * 100.0),
            };
            AttributionInfo {
                entity_id: a.entity_id.to_string(),
                entity_name: a.entity_name.clone(),
                source,
                weight: a.weight,
            }
        })
        .collect();

    Ok(QueryResponse {
        query_id: result.query_id.to_string(),
        arm: result.arm,
        arm_name: arm_name(result.arm),
        latency_ms: result.latency_ms,
        entities,
        triples,
        recommendations,
        explanation,
        attributions,
    })
}

#[tauri::command]
fn cmd_dashboard(state: State<AppState>) -> Result<DashboardStats, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    let entity_count: usize = graph.entity_count().unwrap_or(0);
    let triple_count: usize = graph.triple_count().unwrap_or(0);

    let trace_store = state.trace_store.lock().map_err(|e| e.to_string())?;
    let all_traces = trace_store.recent(10000).unwrap_or_default();
    let trace_count = all_traces.len();

    let recent: Vec<TraceInfo> = all_traces.iter().rev().take(20).map(|t| TraceInfo {
        id: t.id.to_string()[..8].to_string(),
        event_type: format!("{:?}", t.event_type),
        raw_text: t.raw_text.as_deref().unwrap_or("").to_string(),
        entities_count: t.entities_extracted.len(),
        triples_count: t.triples_extracted.len(),
        retrieval_arm: t.retrieval_arm,
        retrieval_arm_name: t.retrieval_arm.map(arm_name),
        retrieval_latency_ms: t.retrieval_latency_ms,
        created_at: t.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string(),
    }).collect();

    let bandit = UcbBandit::load(&state.bandit_path);
    let stats = bandit.arm_stats();
    let bandit_arms: Vec<BanditArmInfo> = stats.iter().enumerate().map(|(i, (pulls, avg))| {
        BanditArmInfo {
            arm: i as u8,
            name: arm_name(i as u8),
            pulls: *pulls,
            avg_reward: *avg,
        }
    }).collect();

    Ok(DashboardStats {
        entity_count,
        triple_count,
        trace_count,
        bandit_arms,
        recent_traces: recent,
    })
}

#[tauri::command]
fn cmd_traces(limit: Option<usize>, state: State<AppState>) -> Result<Vec<TraceInfo>, String> {
    let trace_store = state.trace_store.lock().map_err(|e| e.to_string())?;
    let traces = trace_store.recent(limit.unwrap_or(50)).unwrap_or_default();

    Ok(traces.iter().rev().map(|t| TraceInfo {
        id: t.id.to_string(),
        event_type: format!("{:?}", t.event_type),
        raw_text: t.raw_text.as_deref().unwrap_or("").to_string(),
        entities_count: t.entities_extracted.len(),
        triples_count: t.triples_extracted.len(),
        retrieval_arm: t.retrieval_arm,
        retrieval_arm_name: t.retrieval_arm.map(arm_name),
        retrieval_latency_ms: t.retrieval_latency_ms,
        created_at: t.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string(),
    }).collect())
}

#[tauri::command]
fn cmd_decay(factor: f64, threshold: f64, state: State<AppState>) -> Result<String, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let below = graph.decay_all(factor, threshold).map_err(|e| e.to_string())?;
    Ok(format!("Decay applied (factor={factor}). {below} entities below {threshold} threshold."))
}

/// Return the full knowledge graph for mind-map visualization.
#[tauri::command]
fn cmd_graph(state: State<AppState>) -> Result<GraphData, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    // Run Louvain community detection
    let communities = graph.louvain().unwrap_or_default();
    let community_count = communities.values().collect::<std::collections::HashSet<_>>().len();

    // Get all entities via skg
    let skg_entities = graph.inner().list_entities(None, None)
        .map_err(|e| format!("list entities: {e}"))?;

    let mut nodes = Vec::new();
    let mut uuid_to_name: HashMap<String, String> = HashMap::new();

    for ent in &skg_entities {
        let uuid_str = ent.get_property("uuid")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let confidence = ent.get_property("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        let etype = serde_json::from_str::<tm_types::EntityType>(&ent.entity_type)
            .map(|t| format!("{}", t))
            .unwrap_or_else(|_| ent.entity_type.clone());

        // Look up community for this entity
        let community = uuid::Uuid::parse_str(&uuid_str)
            .ok()
            .and_then(|u| communities.get(&u).copied());

        uuid_to_name.insert(uuid_str.clone(), ent.name.clone());

        nodes.push(GraphNode {
            id: uuid_str,
            name: ent.name.clone(),
            entity_type: etype,
            confidence,
            community,
        });
    }

    // Get all relations via raw SQL
    let conn = graph.inner().connection();
    let mut stmt = conn.prepare(
        "SELECT source_id, target_id, rel_type, weight, properties FROM kg_relations"
    ).map_err(|e| format!("prepare: {e}"))?;

    // Build skg_id → uuid map
    let mut skg_to_uuid: HashMap<i64, String> = HashMap::new();
    for ent in &skg_entities {
        if let Some(skg_id) = ent.id {
            let uuid_str = ent.get_property("uuid")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            skg_to_uuid.insert(skg_id, uuid_str);
        }
    }

    let mut edges = Vec::new();
    let rows = stmt.query_map([], |row| {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let rel_type: String = row.get(2)?;
        let weight: f64 = row.get(3)?;
        Ok((source_id, target_id, rel_type, weight))
    }).map_err(|e| format!("query: {e}"))?;

    for row in rows {
        let (src, tgt, rel_type, weight) = row.map_err(|e| format!("row: {e}"))?;
        let source_uuid = skg_to_uuid.get(&src).cloned().unwrap_or_default();
        let target_uuid = skg_to_uuid.get(&tgt).cloned().unwrap_or_default();
        if source_uuid.is_empty() || target_uuid.is_empty() {
            continue;
        }
        let predicate = serde_json::from_str::<tm_types::Predicate>(&rel_type)
            .map(|p| format!("{}", p))
            .unwrap_or(rel_type);
        edges.push(GraphEdge {
            source: source_uuid,
            target: target_uuid,
            predicate,
            confidence: weight,
        });
    }

    // 2026-05-12 — searchable community labels. Count degree per node
    // from the edge list, group (name, degree) by community, sort each
    // group by degree desc, take top-3 names. The resulting label is
    // both a glance description ("Aaditya · TraceMind · Rust" tells you
    // instantly what Community 0 is about) and a search query the user
    // can paste into the Query view to drill into the community's
    // captures.
    let mut degree: HashMap<String, usize> = HashMap::new();
    for e in &edges {
        *degree.entry(e.source.clone()).or_insert(0) += 1;
        *degree.entry(e.target.clone()).or_insert(0) += 1;
    }
    let mut by_community: HashMap<i32, Vec<(String, usize)>> = HashMap::new();
    for n in &nodes {
        if let Some(cid) = n.community {
            let d = degree.get(&n.id).copied().unwrap_or(0);
            by_community
                .entry(cid)
                .or_default()
                .push((n.name.clone(), d));
        }
    }
    let mut community_labels: HashMap<String, String> = HashMap::new();
    for (cid, mut members) in by_community {
        // Sort by degree desc, then by name asc for determinism.
        members.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top: Vec<String> = members.into_iter().take(3).map(|(n, _)| n).collect();
        if !top.is_empty() {
            community_labels.insert(cid.to_string(), top.join(" · "));
        }
    }

    Ok(GraphData { nodes, edges, community_count, community_labels })
}

/// Batch-ingest curated demo data to populate the knowledge graph.
#[tauri::command]
fn cmd_demo_ingest(state: State<AppState>) -> Result<DemoResult, String> {
    let demo_texts = [
        // About TraceMind & founder
        "Aaditya Srivathsan is the founder of TraceMind, a local-only memory OS for AI agents built entirely in Rust.",
        "TraceMind uses Rust for its core data plane because of memory safety, zero-cost abstractions, and tiny binary sizes.",
        "TraceMind stores everything in a single SQLite file using the sqlite-knowledge-graph crate for entities, relations, and vector embeddings.",
        // Claude conversation context
        "Claude helped Aaditya rewrite the tm-graph crate from raw rusqlite to sqlite-knowledge-graph, consolidating entities, triples, and vectors into a single file.",
        "During the Phase 2.5 sprint, Claude implemented the ColBERT reranker using mxbai-edge-colbert-v0-17m with ONNX Runtime for late-interaction retrieval.",
        "Claude and Aaditya debugged an ort crate API mismatch where Session was at ort::session::Session instead of ort::Session in version 2.0.0-rc.10.",
        "The LanceDB dependency was removed from TraceMind because sqlite-knowledge-graph handles vector storage natively with brute-force cosine search.",
        "Aaditya decided to always use real fastembed ONNX embeddings instead of hash embeddings, even during development and testing.",
        // Architecture decisions
        "The retrieval engine uses a UCB1 bandit algorithm to select between four strategies: vector-only, graph-heavy, hybrid, and episodic.",
        "Claude Code integrates with TraceMind through an MCP server that exposes memory_store and memory_query tools over JSON-RPC.",
        "The Tauri desktop app was built with React and Tailwind CSS, providing a native experience without Electron overhead.",
        "The capture daemon monitors clipboard changes via pbpaste and shell history from zsh_history, auto-ingesting into TraceMind.",
        // Technical details from conversations
        "The NER pipeline uses a two-pass heuristic: first extracting multi-word Title Case spans, then single tokens matched against 80 plus known technology terms.",
        "Privacy is a core design tenet: no API calls, no telemetry, no cloud storage. Every piece of data stays on the local machine.",
        "The embedding model is all-MiniLM-L6-v2 running via ONNX on CPU, producing 384-dimensional vectors for semantic search.",
        "PageRank weights entity importance and Louvain community detection clusters related concepts in the knowledge graph.",
        // Procedural memory examples
        "The deployment procedure for TraceMind is: git pull, cargo build release, then restart the service with systemctl.",
        "To run the full test suite: cargo test workspace with all 55 tests across 9 crates covering NER, graph, embeddings, governance, and retrieval.",
        // Project context
        "Aaditya has a demo scheduled and needs the Tauri UI to showcase memory ingestion, knowledge graph visualization, and trace auditing.",
        "Phase 3 of TraceMind will add a JEPA encoder with VICReg loss for surprise-based ingestion and world model prediction.",
    ];

    let pipeline = state.ingest.lock().map_err(|e| e.to_string())?;
    let trace_store = state.trace_store.lock().map_err(|e| e.to_string())?;

    let mut total_entities = 0;
    let mut total_triples = 0;
    let mut texts_ingested = 0;

    for text in &demo_texts {
        let session_id = Uuid::new_v4();
        match pipeline.ingest(text, session_id) {
            Ok(result) => {
                total_entities += result.entities.len();
                total_triples += result.triples.len();
                let _ = trace_store.append(&result.trace);
                texts_ingested += 1;
            }
            Err(_) => continue, // skip PII or other failures
        }
    }

    Ok(DemoResult {
        texts_ingested,
        total_entities,
        total_triples,
    })
}

/// Register a click on an entity — implicit positive feedback for the bandit.
#[tauri::command]
fn cmd_entity_click(entity_id: String, state: State<AppState>) -> Result<(), String> {
    let uuid = Uuid::parse_str(&entity_id).map_err(|e| e.to_string())?;
    let mut engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    engine.register_click(uuid);
    Ok(())
}

/// Get proactive recommendations based on recent query context.
#[tauri::command]
fn cmd_recommendations(limit: Option<usize>, state: State<AppState>) -> Result<Vec<RecommendationInfo>, String> {
    let engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    let recs = engine.recommendations(limit.unwrap_or(5));

    // Same origin-context resolution as `cmd_query` — needs a read-only
    // graph handle to translate context UUID → name.
    let aux_graph = GraphStore::open(&state.db_path).ok();
    let ctx_names: HashMap<Uuid, String> = aux_graph
        .as_ref()
        .map(context_name_map)
        .unwrap_or_default();

    Ok(recs.into_iter().map(|r| {
        let origin_context = r.origin_context_id.as_ref()
            .and_then(|id_str| Uuid::parse_str(id_str).ok())
            .and_then(|id| ctx_names.get(&id).cloned());
        RecommendationInfo {
            entity_id: r.entity_id,
            entity_name: r.entity_name,
            entity_type: r.entity_type,
            score: r.score,
            reason: r.reason,
            reason_detail: r.reason_detail,
            origin_query: r.origin_query,
            origin_context,
        }
    }).collect())
}

/// 2026-05-11 UX (#2) — cold-start seed for the Reasoning Engine.
///
/// Returns the highest-PageRank entity in the active context (or
/// globally, if unscoped) plus a short "why this entity" rationale.
/// The ReasonView calls this on mount and immediately runs an
/// `explore` from the seed, so the user lands on a populated graph
/// view instead of an empty input prompt.
#[derive(Serialize)]
struct ReasonSeed {
    entity_id: String,
    entity_name: String,
    entity_type: String,
    /// Short rationale e.g. "most-connected entity in your graph
    /// (PageRank 0.72)". Always non-empty.
    why: String,
}

#[tauri::command]
fn cmd_reason_seed(state: State<AppState>) -> Result<Option<ReasonSeed>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let pr = graph.pagerank().map_err(|e| e.to_string())?;
    if pr.is_empty() {
        return Ok(None);
    }

    // Prefer an entity visible under the active context. If none of
    // the top PR entities are in scope we fall back to the overall
    // top — better to surface *something* than to leave the view
    // empty.
    let mut ranked: Vec<(Uuid, f64)> = pr.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let pick = ranked.iter()
        .find(|(id, _)| graph.entity_in_active_scope(*id, false).unwrap_or(false))
        .or_else(|| ranked.first())
        .copied();

    let Some((entity_id, score)) = pick else {
        return Ok(None);
    };

    let entity = match graph.get_entity(entity_id) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let why = format!(
        "most-connected entity{} (PageRank {:.2})",
        if graph.active_context_id().is_some() { " in this context" } else { "" },
        score,
    );

    Ok(Some(ReasonSeed {
        entity_id: entity_id.to_string(),
        entity_name: entity.name,
        entity_type: format!("{}", entity.entity_type),
        why,
    }))
}

/// 2026-05-11 UX (#4) — recent retrieval queries for the QueryView
/// sticky panel. Reads the trace log and filters to Retrieve events,
/// returning the natural-language query text and timing info. Used to
/// render "you recently asked …" pills above the search bar so the
/// user can see continuity across sessions.
#[derive(Serialize)]
struct RecentQueryInfo {
    trace_id: String,
    query_text: String,
    arm_name: Option<String>,
    entities_count: usize,
    created_at: String,
}

#[tauri::command]
fn cmd_query_recent(
    limit: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<RecentQueryInfo>, String> {
    let trace_store = state.trace_store.lock().map_err(|e| e.to_string())?;
    // Pull a generous window then filter to Retrieve traces with a
    // non-empty `raw_text` (== the query string).
    let traces = trace_store.recent(200).unwrap_or_default();
    let want = limit.unwrap_or(5);

    let mut out = Vec::with_capacity(want);
    let mut seen_text: HashSet<String> = HashSet::new();
    for t in traces.iter().rev() {
        if !matches!(t.event_type, tm_types::TraceEventType::Retrieve) {
            continue;
        }
        let Some(text) = t.raw_text.as_deref() else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        // Deduplicate consecutive identical queries — users often
        // re-run the same string while tweaking results.
        if !seen_text.insert(text.to_string()) {
            continue;
        }
        out.push(RecentQueryInfo {
            trace_id: t.id.to_string(),
            query_text: text.to_string(),
            arm_name: t.retrieval_arm.map(arm_name),
            entities_count: t.entities_extracted.len(),
            created_at: t.created_at.with_timezone(&chrono::Local).format("%H:%M").to_string(),
        });
        if out.len() >= want {
            break;
        }
    }
    Ok(out)
}

/// Explicit user feedback on query results (thumbs up = 1.0, thumbs down = 0.0).
#[tauri::command]
fn cmd_feedback(score: f64, state: State<AppState>) -> Result<(), String> {
    let mut engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    engine.explicit_feedback(score);
    Ok(())
}

/// Check if text is relevant enough for auto-ingestion.
#[tauri::command]
fn cmd_check_relevance(text: String, state: State<AppState>) -> Result<bool, String> {
    let engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    Ok(engine.is_relevant_for_ingestion(&text))
}

/// Delete an entity and all its associated relations, vectors, and access logs.
#[tauri::command]
fn cmd_delete_entity(entity_id: String, state: State<AppState>) -> Result<(), String> {
    let uuid = Uuid::parse_str(&entity_id).map_err(|e| e.to_string())?;
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    graph.delete_entity(uuid).map_err(|e| e.to_string())?;
    Ok(())
}

/// Quality gate for entities surfaced in *proactive* panels (Drift, Connect,
/// relation suggestions). Heuristic NER (`HeuristicExtractor`) over ambient
/// capture grabs sentence-initial Title Case words and YAKE keyphrase
/// fragments — useful for retrieval, but garbage when the user sees
/// "Connect: your" or "Review: especially" on the Dashboard.
///
/// This gate is *only* applied at the panel layer — the underlying graph
/// keeps every extracted entity so retrieval / signal hybrid paths are
/// unchanged. 2026-05-11 audit, post user-feedback: dashboard panels were
/// half-noise; this gate ships the cheapest fix while we wait on GLiNER
/// to be wired into the default pipeline.
fn is_proactive_quality_entity(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.len() < 3 {
        return false;
    }
    if !trimmed.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    let lower = trimmed.to_lowercase();

    // Hard stopword / sentence-initial junk list — pulled from the
    // entities currently leaking into the panels plus the standard
    // English closed-class set. Kept in-line so the filter has no
    // dependency on the ingest crate's `GATE_STOPWORDS` (which has a
    // different purpose — gating ingestion, not surfacing).
    const STOP: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "not", "if", "then", "so", "as", "of", "to",
        "for", "with", "by", "in", "on", "at", "from", "into", "onto", "upon", "over",
        "under", "this", "that", "these", "those", "it", "its", "they", "them", "their",
        "there", "i", "me", "my", "we", "us", "our", "you", "your", "yours", "he", "him",
        "his", "she", "her", "hers", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could", "should",
        "may", "might", "shall", "can", "must", "what", "when", "where", "why", "how",
        "who", "whom", "which",
        // Sentence-initial adverbs / fillers seen leaking into panels
        "especially", "really", "very", "much", "more", "most", "less", "least",
        "always", "never", "often", "sometimes", "usually", "rarely", "quite",
        "well", "yes", "no", "ok", "okay", "good", "bad", "fine", "still", "just",
        // Common verbs misread as proper nouns at sentence start
        "sounds", "looks", "feels", "seems", "pull", "push", "make", "made", "take",
        "took", "give", "gave", "get", "got", "see", "saw", "use", "used", "let",
        "lets", "say", "said", "tell", "told", "ask", "asked", "want", "wanted",
        "need", "needed", "try", "tried", "find", "found", "show", "shown",
        // Determiners / quantifiers
        "another", "one", "some", "any", "all", "each", "every", "few", "many",
        "both", "either", "neither", "such",
    ];
    if STOP.contains(&lower.as_str()) {
        return false;
    }

    // Single-word adjective / adverb suffixes — high false-positive rate
    // for proper-noun extraction. Multi-word phrases bypass this filter
    // (e.g. "Implementable Plan" stays, but bare "implementable" goes).
    let is_single = !trimmed.contains(char::is_whitespace);
    if is_single {
        let suffixes = ["ly", "able", "ible", "ive", "ous", "ical", "ier", "iest", "ment"];
        // "ment" gives some false negatives ("comment", "moment") but
        // catches the more annoying "constraint"-style fragment-tokens.
        // The cost of the FN is "doesn't appear in a tiny proactive
        // panel" — fine. The cost of FP is user sees garbage.
        for sfx in suffixes {
            if lower.ends_with(sfx) && lower.len() > sfx.len() + 2 {
                return false;
            }
        }
    }

    true
}

/// Get "most surprising things" — entities that are *semantically* far from
/// the user's typical topic centroid, blended with recency so it stays
/// "today-feeling". 2026-05-11 rewrite: the old definition keyed off
/// `access_log` count, which collapsed to "newest 5 entities" whenever the
/// user hadn't queried anything yet. The new definition computes the
/// centroid of every stored entity embedding, then ranks entities by how
/// far their own embedding sits from that centroid (cosine distance). An
/// entity is "surprising" when it doesn't fit the user's usual semantic
/// neighbourhood — which is what the label promises.
#[tauri::command]
fn cmd_surprising(limit: Option<usize>, state: State<AppState>) -> Result<Vec<SurprisingEntity>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let entities = graph.list_all_entities().map_err(|e| e.to_string())?;

    let limit = limit.unwrap_or(5);
    if entities.is_empty() {
        return Ok(Vec::new());
    }

    let entity_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();
    let recency_map = graph.batch_recency_scores(&entity_ids);

    // Pull every entity that has an embedding. With 378 entities + indexed
    // PK lookup this takes <50ms; well within the 30s panel refresh cadence.
    let mut vec_rows: Vec<(usize, Vec<f32>)> = Vec::new();
    for (i, e) in entities.iter().enumerate() {
        if let Ok(Some(v)) = graph.get_vector(e.id) {
            if !v.is_empty() {
                vec_rows.push((i, v));
            }
        }
    }
    // Fallback: if nothing has a vector yet (fresh DB / hash-embed disabled),
    // we can't compute a centroid — return empty rather than the misleading
    // "novelty = 1.0 for everything" behaviour.
    if vec_rows.is_empty() {
        return Ok(Vec::new());
    }

    // Centroid = element-wise mean of all embeddings, then L2-normalised so
    // we can compare with cosine.
    let dim = vec_rows[0].1.len();
    let mut centroid = vec![0.0f32; dim];
    for (_, v) in &vec_rows {
        // Defensive: skip vectors whose dimension drifts (shouldn't happen,
        // but old DBs that switched embedders might mix lengths).
        if v.len() != dim {
            continue;
        }
        for k in 0..dim {
            centroid[k] += v[k];
        }
    }
    let n = vec_rows.len() as f32;
    for c in centroid.iter_mut() {
        *c /= n;
    }
    let cnorm: f32 = centroid.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    let centroid_unit: Vec<f32> = centroid.iter().map(|x| x / cnorm).collect();

    let mut scored: Vec<SurprisingEntity> = vec_rows
        .iter()
        .filter_map(|(idx, v)| {
            if v.len() != dim {
                return None;
            }
            let e = &entities[*idx];
            // Quality gate: drop NER fragments / stopwords from the
            // surface even though they still count toward the centroid.
            // Keeping them in the centroid is correct (it's the "shape"
            // of the user's corpus); silencing them at the panel level
            // is correct (no one wants to "Review: especially").
            if !is_proactive_quality_entity(&e.name) {
                return None;
            }
            let vnorm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            let cos: f32 = v
                .iter()
                .zip(centroid_unit.iter())
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / vnorm;
            // Cosine distance from the topic centroid, clamped to [0, 1].
            let surprise = (1.0 - cos as f64).clamp(0.0, 1.0);
            let recency = recency_map.get(&e.id).copied().unwrap_or(0.0);
            // 70% semantic outlier-ness + 30% recency. The recency tail
            // breaks ties towards "weird-and-recent" over "weird-and-old".
            let score = 0.7 * surprise + 0.3 * recency;
            Some(SurprisingEntity {
                entity_id: e.id.to_string(),
                entity_name: e.name.clone(),
                entity_type: format!("{}", e.entity_type),
                // `novelty` field is now repurposed as semantic outlier-ness
                // (cosine distance from centroid). UI label stays "novelty".
                novelty: surprise,
                recency,
                score,
            })
        })
        .collect();

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

/// Get entity frequency over recent time windows (for sparkline trends).
#[tauri::command]
fn cmd_entity_trends(state: State<AppState>) -> Result<Vec<TrendPoint>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let conn = graph.inner().connection();

    // Count entities created per day over the last 7 days
    let mut stmt = conn.prepare(
        "SELECT date(json_extract(properties, '$.created_at')) as day, COUNT(*) as cnt \
         FROM kg_entities \
         WHERE json_extract(properties, '$.created_at') >= datetime('now', '-7 days') \
         GROUP BY day ORDER BY day"
    ).map_err(|e| format!("prepare: {e}"))?;

    let rows = stmt.query_map([], |row| {
        let day: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((day, count))
    }).map_err(|e| format!("query: {e}"))?;

    let mut points = Vec::new();
    for row in rows.flatten() {
        points.push(TrendPoint {
            date: row.0,
            entity_count: row.1 as usize,
        });
    }

    Ok(points)
}

#[derive(Serialize)]
struct TrendPoint {
    date: String,
    entity_count: usize,
}

// ---------------------------------------------------------------------------
// Live capture event (emitted to frontend)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct CaptureEvent {
    source: String,
    text: String,
    entities_count: usize,
    triples_count: usize,
}

/// LM-4 — emitted whenever an ingest touches one or more entities so
/// that any open entity drawer can refresh itself without a poll.
/// Listeners filter on `entity_ids` for the entity they're showing.
#[derive(Clone, Serialize)]
struct EntityUpdatedEvent {
    /// The entities (as stringified UUIDs) the most recent ingest
    /// upserted or referenced.
    entity_ids: Vec<String>,
    /// Ingest source label so the UI can show a small chip.
    source: String,
}

/// Toggle live capture on/off.
#[tauri::command]
fn cmd_toggle_capture(enabled: bool, state: State<AppState>) -> Result<(), String> {
    tracing::info!("[capture] toggle: enabled={}", enabled);
    let mut flag = state.capture_enabled.lock().map_err(|e| e.to_string())?;
    *flag = enabled;
    Ok(())
}

#[tauri::command]
fn cmd_capture_status(state: State<AppState>) -> Result<bool, String> {
    let flag = state.capture_enabled.lock().map_err(|e| e.to_string())?;
    Ok(*flag)
}

// ---------------------------------------------------------------------------
// Reasoning commands
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ReasoningStepInfo {
    entity_id: String,
    entity_name: String,
    entity_type: String,
    predicate: String,
    direction: String,
    confidence: f64,
}

#[derive(Serialize)]
struct ReasoningChainInfo {
    steps: Vec<ReasoningStepInfo>,
    score: f64,
    source_id: String,
    destination_id: String,
}

#[derive(Serialize)]
struct AnalogyInfo {
    source_name: String,
    target_id: String,
    target_name: String,
    similarity: f64,
    shared_patterns: Vec<String>,
    explanation: String,
}

#[tauri::command]
fn cmd_reason_chain(
    source_name: String,
    target_name: String,
    state: State<AppState>,
) -> Result<Vec<ReasoningChainInfo>, String> {
    let graph = tm_graph::GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    let source = graph.find_entity_by_name_icase(&source_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", source_name))?;

    let target = graph.find_entity_by_name_icase(&target_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", target_name))?;

    let builder = tm_reason::ChainBuilder::with_defaults(&graph);
    let chains = builder.find_chains(source.id, target.id);

    Ok(chains.iter().map(|c| ReasoningChainInfo {
        steps: c.steps.iter().map(|s| ReasoningStepInfo {
            entity_id: s.entity_id.to_string(),
            entity_name: s.entity_name.clone(),
            entity_type: s.entity_type.clone(),
            predicate: s.predicate.clone(),
            direction: format!("{:?}", s.direction),
            confidence: s.confidence,
        }).collect(),
        score: c.score,
        source_id: c.source_id.to_string(),
        destination_id: c.destination_id.to_string(),
    }).collect())
}

#[tauri::command]
fn cmd_reason_explore(
    entity_name: String,
    max_results: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<ReasoningChainInfo>, String> {
    let graph = tm_graph::GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    let entity = graph.find_entity_by_name_icase(&entity_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", entity_name))?;

    let builder = tm_reason::ChainBuilder::with_defaults(&graph);
    let chains = builder.explore(&[entity.id], max_results.unwrap_or(10));

    Ok(chains.iter().map(|c| ReasoningChainInfo {
        steps: c.steps.iter().map(|s| ReasoningStepInfo {
            entity_id: s.entity_id.to_string(),
            entity_name: s.entity_name.clone(),
            entity_type: s.entity_type.clone(),
            predicate: s.predicate.clone(),
            direction: format!("{:?}", s.direction),
            confidence: s.confidence,
        }).collect(),
        score: c.score,
        source_id: c.source_id.to_string(),
        destination_id: c.destination_id.to_string(),
    }).collect())
}

#[tauri::command]
fn cmd_find_analogies(
    entity_name: String,
    max_results: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<AnalogyInfo>, String> {
    let graph = tm_graph::GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    let entity = graph.find_entity_by_name_icase(&entity_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", entity_name))?;

    let solver = tm_reason::AnalogySolver::new(&graph);
    let results = solver.find_analogies(entity.id, max_results.unwrap_or(5));

    Ok(results.iter().map(|r| AnalogyInfo {
        source_name: r.source_name.clone(),
        target_id: r.target_id.to_string(),
        target_name: r.target_name.clone(),
        similarity: r.similarity,
        shared_patterns: r.shared_patterns.clone(),
        explanation: r.explanation.clone(),
    }).collect())
}

#[derive(Serialize, Default)]
struct ConsolidationResult {
    entities_strengthened: usize,
    entities_decayed: usize,
    entities_pruned: usize,
    entities_merged: usize,
    triples_pruned: usize,
    // CLU-2 — periodic full HDBSCAN re-cluster results.
    cluster_n_clusters: usize,
    cluster_n_outliers: usize,
    cluster_n_assigned: usize,
    cluster_skipped_reason: Option<String>,
    // CLU-5 — Louvain entity communities.
    community_n_communities: usize,
    community_modularity: f64,
    // CLU-5b — c-TF-IDF community labels.
    community_n_labeled: usize,
    /// Top-N community names (by member count) for the UI status line.
    community_top_labels: Vec<String>,
    // SALIENCE — entity importance scores.
    salience_n_scored: usize,
}

#[tauri::command]
async fn cmd_consolidate(state: State<'_, AppState>) -> Result<ConsolidationResult, String> {
    // Tauri commands require `Send` futures, but `GraphStore` holds a
    // `rusqlite::Connection` which is `!Send`. Scope every direct
    // `GraphStore` use inside synchronous blocks so no non-Send value
    // straddles the LLM `.await` below.
    let db_path = state.db_path.clone();

    let mut result = {
        let graph = tm_graph::GraphStore::open(&db_path).map_err(|e| e.to_string())?;
        let consolidator = tm_reason::Consolidator::with_defaults(&graph);
        let report = consolidator.consolidate();

        let mut result = ConsolidationResult {
            entities_strengthened: report.entities_strengthened,
            entities_decayed: report.entities_decayed,
            entities_pruned: report.entities_pruned,
            entities_merged: report.entities_merged,
            triples_pruned: report.triples_pruned,
            ..Default::default()
        };

    // CLU-2 — periodic full re-cluster. Pull every captured signal that
    // still has an embedding and feed it to HDBSCAN. Skipped silently
    // when the corpus is too small or no embeddings are present (e.g.
    // hash-embed only); we surface the reason in the result for the UI.
    const RECLUSTER_LIMIT: usize = 10_000;
    match graph.all_signals_with_embeddings(RECLUSTER_LIMIT) {
        Ok(pairs) if pairs.is_empty() => {
            result.cluster_skipped_reason = Some("no embedded signals".into());
        }
        Ok(pairs) => match tm_cluster::Clusterer::open(&state.db_path) {
            Ok(clusterer) => match clusterer.recluster(pairs) {
                Ok(cs) => {
                    result.cluster_n_clusters = cs.n_clusters;
                    result.cluster_n_outliers = cs.n_outliers;
                    result.cluster_n_assigned = cs.n_assigned;
                }
                Err(tm_cluster::ClusterError::InsufficientSamples { needed, got }) => {
                    result.cluster_skipped_reason =
                        Some(format!("insufficient samples: need {needed}, got {got}"));
                }
                Err(e) => {
                    result.cluster_skipped_reason = Some(format!("recluster failed: {e}"));
                }
            },
            Err(e) => {
                result.cluster_skipped_reason = Some(format!("open clusterer: {e}"));
            }
        },
        Err(e) => {
            result.cluster_skipped_reason = Some(format!("read signals: {e}"));
        }
    }

    // CLU-5 — Louvain entity-community recompute. Cheap (≤ 10k entities)
    // and idempotent; safe to run on every consolidation cycle.
    match graph.recompute_communities() {
        Ok(cs) => {
            result.community_n_communities = cs.n_communities;
            result.community_modularity = cs.modularity;
        }
        Err(e) => {
            tracing::warn!(error = %e, "recompute_communities failed");
        }
    }

    // CLU-5b — c-TF-IDF community labels (fallback / always-on tier).
    // Replaces "c-0 / c-1 / …" with phrases like "vector search /
    // embeddings / retrieval". Must run after `recompute_communities`
    // since it reads the freshly written community_id assignments.
    match graph.recompute_community_labels() {
        Ok(ls) => {
            result.community_n_labeled = ls.n_labeled;
        }
        Err(e) => {
            tracing::warn!(error = %e, "recompute_community_labels failed");
        }
    }

        // Close the synchronous scope here so the `!Send` GraphStore /
        // Consolidator are dropped before the LLM `.await` below.
        result
    };

    // CLU-5c — Tier-1 LLM relabel pass over the top-N largest
    // communities. Overwrites the TF-IDF fallback for the few
    // communities the user actually sees in the status line / Garden.
    // Cheap (~150 ms per call on Apple Silicon, top-10 by default)
    // and completely skipped at compile time when local-llm is off.
    // Runtime-skipped (with a logged reason) when weights are missing.
    #[cfg(feature = "local-llm")]
    {
        const LLM_RELABEL_TOP_N: usize = 10;
        // The labeler opens its own short-lived `GraphStore` instances in
        // sync sub-scopes — we just hand it the db path so nothing
        // `!Send` straddles the await here.
        let llm_stats = community_label_llm::relabel_top_communities(
            &db_path,
            LLM_RELABEL_TOP_N,
        )
        .await;
        if let Some(reason) = &llm_stats.skipped_reason {
            tracing::info!(reason = %reason, "LLM community labeler: skipped");
        } else {
            tracing::info!(
                attempted = llm_stats.n_attempted,
                succeeded = llm_stats.n_succeeded,
                "LLM community labeler"
            );
        }
    }

    // Final read-back + salience pass — reopen the graph in another
    // synchronous scope; no await touches `result` after this point.
    {
        let graph = tm_graph::GraphStore::open(&db_path).map_err(|e| e.to_string())?;

        // Read back the persisted map and pull the top-3 by size so the UI
        // can show "communities:8 (Q=0.42) → Database internals · Anthropic
        // relationships · Bug repro" instead of a bare number. This runs
        // after both the TF-IDF and LLM passes, so it picks up whichever
        // label is freshest for each community.
        if let Ok(map) = graph.community_label_map() {
            let mut entries: Vec<_> = map.values().cloned().collect();
            entries.sort_by(|a, b| b.size.cmp(&a.size));
            result.community_top_labels = entries
                .iter()
                .take(3)
                .map(|e| e.label.clone())
                .collect();
        }

        // Salience — recompute degree/recency-weighted entity importance.
        match graph.recompute_salience() {
            Ok(ss) => {
                result.salience_n_scored = ss.n_nodes;
            }
            Err(e) => {
                tracing::warn!(error = %e, "recompute_salience failed");
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Clipboard + window title helpers
// ---------------------------------------------------------------------------

fn get_clipboard() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.len() < 15 || text.len() > 10_000 {
            return None;
        }
        // Skip high-entropy (passwords)
        if text.len() < 50 && !text.contains(' ') {
            let unique: HashSet<char> = text.chars().collect();
            if unique.len() as f64 / text.len() as f64 > 0.7 {
                return None;
            }
        }
        Some(text)
    } else {
        None
    }
}

fn get_active_window() -> Option<String> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to get {name, title of front window} of first process whose frontmost is true")
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.len() > 5 && text != "missing value" {
            Some(text)
        } else {
            None
        }
    } else {
        None
    }
}

/// Scrape the last few messages from ChatGPT / Claude in Safari or Chrome.
/// Returns vec of (source_label, conversation_text) pairs.
fn scrape_ai_conversations() -> Vec<(String, String)> {
    let mut results = Vec::new();

    // --- Safari ---
    // Safari lazy-loads background tabs so their DOM may be empty.
    // Strategy: try JS injection first; if it returns nothing, fall back to
    // the tab's `name` property (title) which is always available and often
    // contains the conversation topic (e.g. "Explain Rust lifetimes — ChatGPT").
    let safari_script = r#"
tell application "Safari"
    set output to ""
    repeat with w in windows
        repeat with t in tabs of w
            set tabURL to URL of t
            set tabName to name of t
            if tabURL contains "chatgpt.com" then
                set gotContent to false
                try
                    set msgText to do JavaScript "(() => { var msgs = document.querySelectorAll('[data-message-author-role]'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { var role = el.getAttribute('data-message-author-role'); var text = el.innerText.trim().substring(0, 600); return role + ': ' + text; }).join('\\n---\\n'); })()" in t
                    if msgText is not missing value and length of msgText > 10 then
                        set output to output & "CHATGPT||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "CHATGPT||" & "ChatGPT conversation: " & tabName & "||||"
                end if
            else if tabURL contains "claude.ai" then
                set gotContent to false
                try
                    set msgText to do JavaScript "(() => { var msgs = document.querySelectorAll('.prose, .font-claude-message, .font-user-message'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { return el.innerText.trim().substring(0, 600); }).join('\\n---\\n'); })()" in t
                    if msgText is not missing value and length of msgText > 10 then
                        set output to output & "CLAUDE||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "CLAUDE||" & "Claude conversation: " & tabName & "||||"
                end if
            else if tabURL contains "gemini.google.com" then
                set gotContent to false
                try
                    set msgText to do JavaScript "(() => { var msgs = document.querySelectorAll('message-content, .model-response-text, .query-text'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { return el.innerText.trim().substring(0, 600); }).join('\\n---\\n'); })()" in t
                    if msgText is not missing value and length of msgText > 10 then
                        set output to output & "GEMINI||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "GEMINI||" & "Gemini conversation: " & tabName & "||||"
                end if
            end if
        end repeat
    end repeat
    return output
end tell
"#;

    if let Ok(output) = Command::new("osascript").arg("-e").arg(safari_script).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            for chunk in text.split("||||") {
                let chunk = chunk.trim();
                if chunk.starts_with("CHATGPT||") {
                    let content = chunk.trim_start_matches("CHATGPT||").trim();
                    if content.len() > 20 {
                        results.push(("ChatGPT".to_string(), content.to_string()));
                    }
                } else if chunk.starts_with("CLAUDE||") {
                    let content = chunk.trim_start_matches("CLAUDE||").trim();
                    if content.len() > 20 {
                        results.push(("Claude".to_string(), content.to_string()));
                    }
                } else if chunk.starts_with("GEMINI||") {
                    let content = chunk.trim_start_matches("GEMINI||").trim();
                    if content.len() > 20 {
                        results.push(("Gemini".to_string(), content.to_string()));
                    }
                }
            }
        }
    }

    // --- Chrome (also covers Arc / Brave / Chromium-based) ---
    let chrome_script = r#"
tell application "Google Chrome"
    if (count of windows) = 0 then return ""
    set output to ""
    repeat with w in windows
        repeat with t in tabs of w
            set tabURL to URL of t
            set tabName to title of t
            if tabURL contains "chatgpt.com" then
                set gotContent to false
                try
                    set msgText to execute t javascript "(() => { var msgs = document.querySelectorAll('[data-message-author-role]'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { return el.getAttribute('data-message-author-role') + ': ' + el.innerText.trim().substring(0, 600); }).join('\\n---\\n'); })()"
                    if length of msgText > 10 then
                        set output to output & "CHATGPT||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "CHATGPT||" & "ChatGPT conversation: " & tabName & "||||"
                end if
            else if tabURL contains "claude.ai" then
                set gotContent to false
                try
                    set msgText to execute t javascript "(() => { var msgs = document.querySelectorAll('.prose, .font-claude-message, .font-user-message'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { return el.innerText.trim().substring(0, 600); }).join('\\n---\\n'); })()"
                    if length of msgText > 10 then
                        set output to output & "CLAUDE||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "CLAUDE||" & "Claude conversation: " & tabName & "||||"
                end if
            else if tabURL contains "gemini.google.com" then
                set gotContent to false
                try
                    set msgText to execute t javascript "(() => { var msgs = document.querySelectorAll('message-content, .model-response-text, .query-text'); if (!msgs.length) return ''; return Array.from(msgs).slice(-4).map(function(el) { return el.innerText.trim().substring(0, 600); }).join('\\n---\\n'); })()"
                    if length of msgText > 10 then
                        set output to output & "GEMINI||" & msgText & "||||"
                        set gotContent to true
                    end if
                end try
                if not gotContent and length of tabName > 5 then
                    set output to output & "GEMINI||" & "Gemini conversation: " & tabName & "||||"
                end if
            end if
        end repeat
    end repeat
    return output
end tell
"#;

    if let Ok(output) = Command::new("osascript").arg("-e").arg(chrome_script).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            for chunk in text.split("||||") {
                let chunk = chunk.trim();
                if chunk.starts_with("CHATGPT||") {
                    let content = chunk.trim_start_matches("CHATGPT||").trim();
                    if content.len() > 20 {
                        results.push(("ChatGPT (Chrome)".to_string(), content.to_string()));
                    }
                } else if chunk.starts_with("CLAUDE||") {
                    let content = chunk.trim_start_matches("CLAUDE||").trim();
                    if content.len() > 20 {
                        results.push(("Claude (Chrome)".to_string(), content.to_string()));
                    }
                } else if chunk.starts_with("GEMINI||") {
                    let content = chunk.trim_start_matches("GEMINI||").trim();
                    if content.len() > 20 {
                        results.push(("Gemini".to_string(), content.to_string()));
                    }
                }
            }
        }
    }

    results
}

/// Scrape Cursor/VS Code active file info from window title.
fn scrape_editor_context() -> Option<String> {
    // Cursor and VS Code show "filename — project" in title
    let output = Command::new("osascript")
        .arg("-e")
        .arg(r#"
tell application "System Events"
    set procs to every process whose background only is false
    set info to ""
    repeat with p in procs
        set pName to name of p
        if pName is "Cursor" or pName is "Code" then
            try
                set wTitle to title of front window of p
                set info to info & pName & ": " & wTitle & "\n"
            end try
        end if
    end repeat
    return info
end tell
"#)
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.len() > 5 { Some(text) } else { None }
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Daily brief (D-4)
// ---------------------------------------------------------------------------
//
// Surfaces the same DailyBrief the CLI / MCP renders, so the recordable
// demo can show the "retraction beat" inside the Tauri shell instead of
// a terminal. Mirrors the structure of cmd_brief in tm-cli but returns
// JSON-friendly types so the React side can render directly.

#[derive(Serialize)]
struct BriefCountsView {
    overdue: usize,
    open: usize,
    resolved: usize,
    candidates: usize,
    patterns: usize,
    insights: usize,
    proposals: usize,
    outcome_prompts: usize,
    contradictions: usize,
}

#[derive(Serialize)]
struct BriefRowView {
    id: String,
    title: String,
    horizon: Option<String>,
    state: String,
    polarity: Option<String>,
    overdue_class: Option<String>,
}

#[derive(Serialize)]
struct ContradictionRowView {
    id: String,
    triple_a: String,
    triple_b: String,
    detected_at: String,
    cosine_similarity: f32,
}

#[derive(Serialize)]
struct CandidateRowView {
    id: String,
    kind: String,
    statement: String,
    matched_phrase: String,
    confidence: f32,
    created_at: String,
}

#[derive(Serialize)]
struct BriefView {
    generated_at: String,
    counts: BriefCountsView,
    overdue: Vec<BriefRowView>,
    open: Vec<BriefRowView>,
    resolved: Vec<BriefRowView>,
    candidates: Vec<CandidateRowView>,
    contradictions: Vec<ContradictionRowView>,
}

fn data_dir(state: &AppState) -> PathBuf {
    PathBuf::from(&state.db_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tauri::command]
fn cmd_brief(state: State<AppState>) -> Result<BriefView, String> {
    tracing::info!("[cmd_brief] invoked");
    let dir = data_dir(&state);
    let intents_path = dir.join("intents.db");
    let intents = tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
        .map_err(|e| format!("open intents: {e}"))?;

    // Attach the graph for JTMS contradictions — this is what gives the
    // brief its retraction-beat punch in the demo.
    let graph = GraphStore::open(&state.db_path).ok();

    // Opportunistic structural contradiction scan — same logic as in
    // cmd_next_actions. Cheap (deduped against already-recorded pairs);
    // running it on every Brief load means the default view surfaces
    // functional-predicate clashes (e.g. WorksAt(X,A) vs WorksAt(X,B))
    // without requiring the user to navigate to Dashboard first.
    if let Some(ref g) = graph {
        let new_count = scan_functional_contradictions(g);
        tracing::info!("[cmd_brief] structural-contradiction-scan new_count={}", new_count);
    }

    let mut builder = tm_reflect::BriefBuilder::new(&intents);
    if let Some(ref g) = graph {
        builder = builder.with_graph(g);
    }
    let brief = builder
        .build(chrono::Utc::now())
        .map_err(|e| format!("build brief: {e}"))?;

    let row_view = |r: &tm_reflect::CommitmentBriefRow| BriefRowView {
        id: r.id.to_string(),
        title: r.statement.clone(),
        horizon: r.horizon.map(|h| h.format("%Y-%m-%d").to_string()),
        state: format!("{:?}", r.state),
        polarity: None,
        overdue_class: r.overdue_class.map(|c| format!("{:?}", c)),
    };
    let resolved_view = |r: &tm_reflect::ResolvedBriefRow| BriefRowView {
        id: r.id.to_string(),
        title: r.statement.clone(),
        horizon: None,
        state: format!("{:?}", r.state),
        polarity: r.polarity.map(|p| format!("{:?}", p)),
        overdue_class: None,
    };

    Ok(BriefView {
        generated_at: brief.generated_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string(),
        counts: BriefCountsView {
            overdue: brief.counts.overdue,
            open: brief.counts.open,
            resolved: brief.counts.resolved,
            candidates: brief.counts.candidates,
            patterns: brief.counts.patterns,
            insights: brief.counts.insights,
            proposals: brief.counts.proposals,
            outcome_prompts: brief.counts.outcome_prompts,
            contradictions: brief.counts.contradictions,
        },
        overdue: brief.overdue.iter().map(row_view).collect(),
        open: brief.open.iter().map(row_view).collect(),
        resolved: brief.resolved.iter().map(resolved_view).collect(),
        candidates: brief
            .candidates
            .iter()
            .map(|c| CandidateRowView {
                id: c.id.to_string(),
                kind: format!("{:?}", c.kind),
                statement: c.statement.clone(),
                matched_phrase: c.matched_phrase.clone(),
                confidence: c.confidence,
                created_at: c
                    .created_at
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string(),
            })
            .collect(),
        contradictions: brief
            .contradictions
            .iter()
            .map(|c| ContradictionRowView {
                id: c.id.to_string(),
                triple_a: c.triple_a.to_string(),
                triple_b: c.triple_b.to_string(),
                detected_at: c.detected_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
                cosine_similarity: c.cosine_similarity,
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Next Actions — proactive action feed (2026-05-11)
// ---------------------------------------------------------------------------
//
// Replaces Dashboard "Suggested for You" entity recs with verb-first action
// cards. Pulls signals from the same BriefBuilder that backs `cmd_brief`:
//   - Overdue commitments → Resolve cards
//   - Open commitments    → FollowUp cards
//   - Contradictions      → Review cards
//
// Sorted by priority (overdue first), then recency. Capped at 12 cards.

#[derive(Serialize)]
struct NextActionInfo {
    /// Stable id within this feed (commitment uuid or contradiction uuid).
    id: String,
    /// Verb-class for the surface to route on. One of:
    /// "Resolve" | "FollowUp" | "Review" | "Confirm" | "Connect".
    kind: String,
    /// Short verb label, e.g. "Resolve", "Follow up", "Review".
    verb: String,
    /// Full action title, e.g. "Resolve: send the deck by Friday".
    title: String,
    /// Optional sub-line (triple text, deadline, context).
    subtitle: Option<String>,
    /// Target entity for the click action.
    target_id: String,
    /// What kind of target — "commitment" | "contradiction" | "entity".
    target_kind: String,
    /// "overdue" | "normal" | "low" — controls surface badge.
    priority: String,
}

#[tauri::command]
fn cmd_next_actions(state: State<AppState>) -> Result<Vec<NextActionInfo>, String> {
    tracing::info!("[cmd_next_actions] invoked");
    let dir = data_dir(&state);
    let intents_path = dir.join("intents.db");
    let intents = tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
        .map_err(|e| format!("open intents: {e}"))?;

    let graph = GraphStore::open(&state.db_path).ok();

    // Opportunistic contradiction scan — populates `brief.contradictions`
    // before the builder runs. Cheap (O(n) on triples, deduped against
    // already-recorded pairs), so we can re-run on every dashboard load.
    if let Some(ref g) = graph {
        let _ = scan_functional_contradictions(g);
    }

    let mut builder = tm_reflect::BriefBuilder::new(&intents);
    if let Some(ref g) = graph {
        builder = builder.with_graph(g);
    }
    let brief = builder
        .build(chrono::Utc::now())
        .map_err(|e| format!("build brief: {e}"))?;

    let mut out: Vec<NextActionInfo> = Vec::new();

    // Overdue commitments → Resolve (priority).
    for row in brief.overdue.iter().take(6) {
        let horizon_str = row
            .horizon
            .map(|h| format!("due {}", h.format("%Y-%m-%d")))
            .unwrap_or_else(|| "no deadline".to_string());
        out.push(NextActionInfo {
            id: row.id.to_string(),
            kind: "Resolve".into(),
            verb: "Resolve".into(),
            title: format!("Resolve: {}", row.statement),
            subtitle: Some(horizon_str),
            target_id: row.id.to_string(),
            target_kind: "commitment".into(),
            priority: "overdue".into(),
        });
    }

    // Contradictions → Review.
    // Resolve the triple endpoints to natural-language form so the user
    // sees the actual clash (e.g. "Aaditya works_at Anthropic ↔
    // Aaditya works_at Google") instead of opaque UUID prefixes.
    for row in brief.contradictions.iter().take(4) {
        let triple_a_text = graph
            .as_ref()
            .and_then(|g| g.triple_detail(row.triple_a).ok().flatten())
            .map(|d| format!("{} {} {}", d.subject_name, d.predicate, d.object_name));
        let triple_b_text = graph
            .as_ref()
            .and_then(|g| g.triple_detail(row.triple_b).ok().flatten())
            .map(|d| format!("{} {} {}", d.subject_name, d.predicate, d.object_name));
        let (title, subtitle) = match (triple_a_text, triple_b_text) {
            (Some(a), Some(b)) => (
                format!("Review: {} ↔ {}", a, b),
                Some("conflicting facts — pick the one you believe".into()),
            ),
            _ => (
                "Review contradiction".into(),
                Some(format!(
                    "{} ↔ {}",
                    short_uuid(&row.triple_a.to_string()),
                    short_uuid(&row.triple_b.to_string())
                )),
            ),
        };
        out.push(NextActionInfo {
            id: row.id.to_string(),
            kind: "Review".into(),
            verb: "Review".into(),
            title,
            subtitle,
            target_id: row.id.to_string(),
            target_kind: "contradiction".into(),
            priority: "normal".into(),
        });
    }

    // Pending mined candidates → Confirm. These are phrase-mined intents
    // from ambient capture (clipboard / shell / MCP turns) — the user
    // confirms / dismisses them in the Brief view. Surfacing them on the
    // dashboard means day-1 users with no manually-entered commitments
    // still see real actionable cards.
    for cand in brief.candidates.iter().take(4) {
        if out.len() >= 12 {
            break;
        }
        let kind_label = match cand.kind {
            tm_intent::types::CommitmentKind::Intent => "intent",
            tm_intent::types::CommitmentKind::Decision => "decision",
            tm_intent::types::CommitmentKind::Hypothesis => "hypothesis",
        };
        out.push(NextActionInfo {
            id: cand.id.to_string(),
            kind: "Confirm".into(),
            verb: "Confirm".into(),
            title: format!("Confirm: {}", cand.statement),
            subtitle: Some(format!(
                "{} · matched '{}' · {:.0}% confidence",
                kind_label,
                cand.matched_phrase,
                cand.confidence * 100.0
            )),
            target_id: cand.id.to_string(),
            target_kind: "candidate".into(),
            priority: "normal".into(),
        });
    }

    // Open commitments → Follow up (lower priority filler).
    for row in brief.open.iter().take(6) {
        if out.len() >= 12 {
            break;
        }
        let horizon_str = row
            .horizon
            .map(|h| format!("due {}", h.format("%Y-%m-%d")))
            .unwrap_or_else(|| "open".to_string());
        out.push(NextActionInfo {
            id: row.id.to_string(),
            kind: "FollowUp".into(),
            verb: "Follow up".into(),
            title: format!("Follow up: {}", row.statement),
            subtitle: Some(horizon_str),
            target_id: row.id.to_string(),
            target_kind: "commitment".into(),
            priority: "normal".into(),
        });
    }

    // 2026-05-11 v2 — Connect cards. Prefer concrete relation suggestions
    // (high-cosine pairs without an edge) — those are one-click actions.
    // Fall back to quality-gated orphan Connect cards only if no
    // suggestion pairs exist (very sparse / brand-new graph).
    if let Some(ref g) = graph {
        if out.len() < 12 {
            if let Ok(pairs) = relation_suggestions(g, 4, 0.75) {
                for (a_id, a_name, b_id, b_name, sim) in pairs {
                    if out.len() >= 12 {
                        break;
                    }
                    // Encode both ids in target_id so the click handler can
                    // route to either node (UI splits on '|').
                    out.push(NextActionInfo {
                        id: format!("{}|{}", a_id, b_id),
                        kind: "Connect".into(),
                        verb: "Connect".into(),
                        title: format!("Link? {} ↔ {}", a_name, b_name),
                        subtitle: Some(format!(
                            "{:.0}% similar · no relation recorded",
                            sim * 100.0
                        )),
                        target_id: a_id.to_string(),
                        target_kind: "entity".into(),
                        priority: "low".into(),
                    });
                }
            }
            // Fallback: only if we still have headroom AND no suggestion
            // pairs surfaced, drop in a couple of quality-gated orphans.
            if out.iter().filter(|a| a.kind == "Connect").count() == 0
                && out.len() < 12
            {
                if let Ok(orphans) = orphan_entities(g, 3) {
                    for (id, name) in orphans {
                        if out.len() >= 12 {
                            break;
                        }
                        out.push(NextActionInfo {
                            id: id.to_string(),
                            kind: "Connect".into(),
                            verb: "Connect".into(),
                            title: format!("Connect: {}", name),
                            subtitle: Some(
                                "0 relations — link into your graph".into(),
                            ),
                            target_id: id.to_string(),
                            target_kind: "entity".into(),
                            priority: "low".into(),
                        });
                    }
                }
            }
        }
    }

    Ok(out)
}

/// Promote a pending mined candidate to an Open commitment. Returns the
/// new commitment uuid as a string so the UI can navigate to the
/// resulting commitment row without re-fetching the brief.
#[tauri::command]
fn cmd_accept_candidate(id: String, state: State<AppState>) -> Result<String, String> {
    let dir = data_dir(&state);
    let intents_path = dir.join("intents.db");
    let mut store = tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
        .map_err(|e| format!("open intents: {e}"))?;
    let cid = Uuid::parse_str(&id).map_err(|e| format!("bad uuid: {e}"))?;
    let new_id = store
        .accept_candidate(cid)
        .map_err(|e| format!("accept: {e}"))?;
    Ok(new_id.to_string())
}

/// LLM backend status — surfaces whether the Qwen tier is active so the
/// UI can render a "LLM on / heuristic only" badge. `feature_compiled`
/// reflects build-time `local-llm` gate; `weights_present` reflects
/// runtime weights at `~/.tracemind/models/qwen2.5-1.5b-instruct-q4_k_m.gguf`;
/// `active` is true only when both are true (the worker actually loaded).
#[derive(Serialize)]
struct LlmStatusView {
    feature_compiled: bool,
    weights_present: bool,
    weights_path: String,
    active: bool,
}

#[tauri::command]
fn cmd_llm_status(state: State<AppState>) -> Result<LlmStatusView, String> {
    let dir = data_dir(&state);
    let path = dir
        .join("models")
        .join("qwen2.5-1.5b-instruct-q4_k_m.gguf");
    let feature_compiled = cfg!(feature = "local-llm");
    let weights_present = path.exists();
    let active = state.llm_active.load(std::sync::atomic::Ordering::Relaxed);
    Ok(LlmStatusView {
        feature_compiled,
        weights_present,
        weights_path: path.to_string_lossy().to_string(),
        active,
    })
}

/// Download (or verify) the Qwen 2.5 1.5B Q4_K_M GGUF on demand. Blocking,
/// ~900 MB first run. Returns the post-download `LlmStatusView` so the UI
/// can flip the "HEURISTIC ONLY" badge without a follow-up call. The active
/// bit is updated optimistically — the worker still needs to load weights
/// on its next consolidation tick before LLM extraction actually fires.
#[tauri::command]
async fn cmd_llm_download(state: State<'_, AppState>) -> Result<LlmStatusView, String> {
    let dir = data_dir(&state);
    let llm_active = state.llm_active.clone();
    let path = tokio::task::spawn_blocking(move || tm_ingest::ensure_qwen_weights(&dir))
        .await
        .map_err(|e| format!("join: {e}"))?
        .map_err(|e| e.to_string())?;
    if cfg!(feature = "local-llm") {
        llm_active.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(LlmStatusView {
        feature_compiled: cfg!(feature = "local-llm"),
        weights_present: true,
        weights_path: path.to_string_lossy().to_string(),
        active: llm_active.load(std::sync::atomic::Ordering::Relaxed),
    })
}

// ── Storage maintenance ──────────────────────────────────────────────
// Settings → Storage panel. Mirrors `tm_graph::maintenance` 1:1 so the
// CLI and the UI share the same surface area. Every destructive op
// returns a `CleanupReport` the UI renders verbatim ("freed 1.2 MB,
// deleted 47 rows").

#[tauri::command]
fn cmd_storage_stats(state: State<AppState>) -> Result<tm_graph::StorageStats, String> {
    let dir = data_dir(&state);
    tm_graph::storage_stats(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_storage_vacuum(state: State<AppState>) -> Result<tm_graph::CleanupReport, String> {
    let dir = data_dir(&state);
    tm_graph::vacuum_all(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_storage_clean_ephemeral(state: State<AppState>) -> Result<tm_graph::CleanupReport, String> {
    let dir = data_dir(&state);
    tm_graph::clean_ephemeral(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_storage_truncate_traces(
    state: State<AppState>,
    keep_recent: usize,
) -> Result<tm_graph::CleanupReport, String> {
    let dir = data_dir(&state);
    tm_graph::truncate_traces(&dir, keep_recent).map_err(|e| e.to_string())
}

/// Dismiss a pending mined candidate (marks it `dismissed` so it never
/// resurfaces). Returns whether the row was actually flipped (false if
/// already accepted / dismissed). Idempotent.
#[tauri::command]
fn cmd_dismiss_candidate(id: String, state: State<AppState>) -> Result<bool, String> {
    let dir = data_dir(&state);
    let intents_path = dir.join("intents.db");
    let store = tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
        .map_err(|e| format!("open intents: {e}"))?;
    let cid = Uuid::parse_str(&id).map_err(|e| format!("bad uuid: {e}"))?;
    store
        .dismiss_candidate(cid)
        .map_err(|e| format!("dismiss: {e}"))
}

/// Return up to `limit` entities that have no relations in either direction
/// AND pass the proactive-panel quality gate. Used as a fallback when the
/// relation-suggestion path can't fill the Connect slot. We over-fetch (3×)
/// and post-filter through `is_proactive_quality_entity` because the
/// majority of orphans are NER fragments — pre-filtering at the SQL layer
/// would require duplicating the stopword list in SQL.
fn orphan_entities(graph: &GraphStore, limit: usize) -> Result<Vec<(Uuid, String)>, String> {
    let conn = graph.inner().connection();
    let fetch_limit = (limit * 4).max(8) as i64;
    let sql = format!(
        "SELECT name, properties FROM kg_entities \
         WHERE id NOT IN ( \
            SELECT source_id FROM kg_relations \
            UNION \
            SELECT target_id FROM kg_relations \
         ) \
         ORDER BY created_at DESC LIMIT {}",
        fetch_limit
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare orphans: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let props: String = row.get(1)?;
            Ok((name, props))
        })
        .map_err(|e| format!("query orphans: {e}"))?;
    let mut out = Vec::new();
    for r in rows.flatten() {
        if !is_proactive_quality_entity(&r.0) {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&r.1) {
            if let Some(id_str) = json.get("uuid").and_then(|v| v.as_str()) {
                if let Ok(uuid) = Uuid::parse_str(id_str) {
                    out.push((uuid, r.0));
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Find pairs of high-quality entities whose embeddings are highly similar
/// but which have no edge in `kg_relations`. These are concrete "Link X to
/// Y?" suggestions — much more actionable than bare "Connect: <name>"
/// orphans because the user has a target to wire to.
///
/// Returns `(a_id, a_name, b_id, b_name, cosine_sim)` tuples sorted by
/// similarity descending, capped at `limit`. Uses `graph.search_vectors`
/// (skg's native nearest-neighbor) so each entity does one indexed
/// top-k probe rather than an O(n²) scan.
///
/// `min_sim` should be ≥ 0.7 in practice — below that the pairs are noisy
/// and the user gets junk. 2026-05-11: starting at 0.75.
fn relation_suggestions(
    graph: &GraphStore,
    limit: usize,
    min_sim: f32,
) -> Result<Vec<(Uuid, String, Uuid, String, f32)>, String> {
    // 1. Quality entities only — every other path here gets these and
    //    we want the same surface across panels.
    let entities = graph
        .list_all_entities()
        .map_err(|e| format!("list entities: {e}"))?;
    let mut name_by_uuid: HashMap<Uuid, String> = HashMap::new();
    for e in &entities {
        if is_proactive_quality_entity(&e.name) {
            name_by_uuid.insert(e.id, e.name.clone());
        }
    }
    if name_by_uuid.len() < 2 {
        return Ok(Vec::new());
    }

    // 2. Build the set of currently-linked UUID pairs (undirected).
    //    kg_relations uses internal skg i64 IDs; we JOIN through
    //    kg_entities to recover the TM UUIDs in `properties.uuid`.
    let conn = graph.inner().connection();
    let mut linked: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut stmt = conn
        .prepare(
            "SELECT s.properties, t.properties \
             FROM kg_relations r \
             JOIN kg_entities s ON s.id = r.source_id \
             JOIN kg_entities t ON t.id = r.target_id",
        )
        .map_err(|e| format!("prepare relations: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let sp: String = row.get(0)?;
            let tp: String = row.get(1)?;
            Ok((sp, tp))
        })
        .map_err(|e| format!("query relations: {e}"))?;
    let extract_uuid = |props: &str| -> Option<Uuid> {
        let json: serde_json::Value = serde_json::from_str(props).ok()?;
        let s = json.get("uuid").and_then(|v| v.as_str())?;
        Uuid::parse_str(s).ok()
    };
    for r in rows.flatten() {
        if let (Some(a), Some(b)) = (extract_uuid(&r.0), extract_uuid(&r.1)) {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            linked.insert((lo, hi));
        }
    }

    // 3. For each quality entity, top-K nearest-neighbor probe. Each
    //    candidate pair gets canonicalised (lo < hi) and deduped — the
    //    probe will surface both (A→B) and (B→A) otherwise.
    //
    //    We also dedup by lowercased name-pair so duplicate-entity
    //    rows in the graph (e.g. two UUIDs both named "reranking") do
    //    not produce "Link? reranking ↔ reranking" cards. This is a
    //    UI-layer guard — the real fix is graph-level entity merge.
    let mut seen: HashSet<(Uuid, Uuid)> = HashSet::new();
    let mut seen_names: HashSet<(String, String)> = HashSet::new();
    let mut candidates: Vec<(Uuid, String, Uuid, String, f32)> = Vec::new();
    // Pre-sort entity ids for stable iteration order (so identical DB
    // state yields identical card order — easier to verify in the UI).
    let mut ordered: Vec<Uuid> = name_by_uuid.keys().copied().collect();
    ordered.sort();
    for id in &ordered {
        let v = match graph.get_vector(*id) {
            Ok(Some(v)) if !v.is_empty() => v,
            _ => continue,
        };
        // top_k=6 → self + ≤5 candidates. We may need to look slightly
        // deeper if the top neighbors are all noise entities filtered
        // out by the quality gate, but in practice 6 is plenty.
        let neighbors = match graph.search_vectors(&v, 6) {
            Ok(n) => n,
            Err(_) => continue,
        };
        for (nid, sim) in neighbors {
            if nid == *id {
                continue;
            }
            if sim < min_sim {
                continue;
            }
            let Some(nname) = name_by_uuid.get(&nid) else {
                continue;
            };
            let (lo, hi) = if *id < nid { (*id, nid) } else { (nid, *id) };
            if linked.contains(&(lo, hi)) {
                continue;
            }
            if !seen.insert((lo, hi)) {
                continue;
            }
            let (a_id, a_name, b_id, b_name) = if *id < nid {
                let aname = name_by_uuid
                    .get(id)
                    .cloned()
                    .unwrap_or_default();
                (*id, aname, nid, nname.clone())
            } else {
                let bname = name_by_uuid
                    .get(id)
                    .cloned()
                    .unwrap_or_default();
                (nid, nname.clone(), *id, bname)
            };
            // Skip same-name and duplicate-name-pair cards.
            let a_lo = a_name.to_lowercase();
            let b_lo = b_name.to_lowercase();
            if a_lo == b_lo {
                continue;
            }
            let name_key = if a_lo < b_lo {
                (a_lo.clone(), b_lo.clone())
            } else {
                (b_lo.clone(), a_lo.clone())
            };
            if !seen_names.insert(name_key) {
                continue;
            }
            candidates.push((a_id, a_name, b_id, b_name, sim));
        }
    }
    candidates.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(limit);
    Ok(candidates)
}

fn short_uuid(s: &str) -> String {
    s.chars().take(8).collect()
}

/// Scan for *functional-predicate* contradictions: two triples with the
/// same subject + predicate but different objects. Examples that fire:
/// "Cat is_a Animal" + "Cat is_a Plant"; "Aaditya works_at Anthropic"
/// + "Aaditya works_at Google".
///
/// Only triples with predicates in `FUNCTIONAL` are considered — those
/// are predicates where multiple objects on the same subject would be a
/// genuine logical clash. `related_to`, `collaborates_with`,
/// `references` etc. legitimately have many objects, so they're skipped.
///
/// New pairs are persisted via `graph.record_contradiction(..)` with a
/// forced cosine of `-0.99` so the JTMS detector fires. Already-recorded
/// pairs are deduped against `graph.contradictions()`.
///
/// Returns the number of *new* contradictions recorded.
fn scan_functional_contradictions(graph: &GraphStore) -> usize {
    // Functional predicates — stored in `kg_relations.rel_type` as the
    // JSON serialization of the `Predicate` enum, which for unit
    // variants is just the snake_case string wrapped in quotes (e.g.
    // Predicate::IsA → "\"is_a\"").
    const FUNCTIONAL: &[&str] = &[
        "\"is_a\"",
        "\"works_at\"",
        "\"part_of\"",
    ];
    tracing::info!("[contradiction-scan] starting");

    let conn = graph.inner().connection();
    let mut stmt = match conn.prepare(
        "SELECT properties, source_id, target_id, rel_type \
         FROM kg_relations",
    ) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let rows: Vec<(String, i64, i64, String)> = match stmt.query_map([], |row| {
        let p: String = row.get(0)?;
        let s: i64 = row.get(1)?;
        let t: i64 = row.get(2)?;
        let r: String = row.get(3)?;
        Ok((p, s, t, r))
    }) {
        Ok(it) => it.filter_map(|r| r.ok()).collect(),
        Err(_) => return 0,
    };
    drop(stmt);

    // Group functional triples by (subject_skg_id, predicate) → (triple_uuid, object_skg_id).
    let mut grouped: HashMap<(i64, String), Vec<(Uuid, i64)>> = HashMap::new();
    for (props_str, src, tgt, rel) in &rows {
        if !FUNCTIONAL.contains(&rel.as_str()) {
            continue;
        }
        let props: serde_json::Value = match serde_json::from_str(props_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let triple_uuid = match props.get("uuid").and_then(|v| v.as_str()) {
            Some(s) => match Uuid::parse_str(s) {
                Ok(u) => u,
                Err(_) => continue,
            },
            None => continue,
        };
        grouped
            .entry((*src, rel.clone()))
            .or_default()
            .push((triple_uuid, *tgt));
    }

    // Pre-compute the set of already-recorded contradiction pairs so we
    // don't re-insert them every dashboard refresh.
    let mut already: HashSet<(Uuid, Uuid)> = HashSet::new();
    for c in graph.contradictions() {
        let (lo, hi) = if c.triple_a < c.triple_b {
            (c.triple_a, c.triple_b)
        } else {
            (c.triple_b, c.triple_a)
        };
        already.insert((lo, hi));
    }

    tracing::info!(
        "[contradiction-scan] groups={} already_recorded={}",
        grouped.len(),
        already.len()
    );

    let mut new_count = 0;
    for ((subj, pred), items) in grouped.iter() {
        if items.len() < 2 {
            continue;
        }
        tracing::info!(
            "[contradiction-scan] candidate group subj_skg={} pred={} size={}",
            subj,
            pred,
            items.len()
        );
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                // Different objects = candidate contradiction.
                if items[i].1 == items[j].1 {
                    continue;
                }
                let (lo, hi) = if items[i].0 < items[j].0 {
                    (items[i].0, items[j].0)
                } else {
                    (items[j].0, items[i].0)
                };
                if already.contains(&(lo, hi)) {
                    continue;
                }
                // Force cosine = -0.99 so the JTMS detector accepts it.
                // The structural same-subject-same-predicate signal is
                // the actual contradiction evidence.
                let res = graph.record_contradiction(lo, hi, -0.99);
                tracing::info!(
                    "[contradiction-scan] record_contradiction({}, {}) -> {}",
                    lo,
                    hi,
                    res.is_some()
                );
                if res.is_some() {
                    already.insert((lo, hi));
                    new_count += 1;
                }
            }
        }
    }
    tracing::info!("[contradiction-scan] done new_count={}", new_count);
    new_count
}

// ---------------------------------------------------------------------------
// Contradiction drawer — D-4 / E series
// ---------------------------------------------------------------------------
//
// The brief surfaces a contradiction row; the user clicks it; the drawer
// renders both triples with names + ingest dates, and offers the three
// resolution buttons. These two commands back that drawer.

#[derive(Serialize)]
struct TripleDetailView {
    triple_id: String,
    subject_id: String,
    subject_name: String,
    subject_type: String,
    predicate: String,
    object_id: String,
    object_name: String,
    object_type: String,
    confidence: f64,
    source_id: Option<String>,
    ingested_at: String,
    status: Option<String>,
}

impl From<tm_graph::TripleDetail> for TripleDetailView {
    fn from(d: tm_graph::TripleDetail) -> Self {
        Self {
            triple_id: d.triple_id.to_string(),
            subject_id: d.subject_id.to_string(),
            subject_name: d.subject_name,
            subject_type: d.subject_type,
            predicate: d.predicate,
            object_id: d.object_id.to_string(),
            object_name: d.object_name,
            object_type: d.object_type,
            confidence: d.confidence,
            source_id: d.source_id,
            ingested_at: d.ingested_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
            status: d.status.map(|s| format!("{:?}", s)),
        }
    }
}

#[tauri::command]
fn cmd_triple_detail(
    state: State<AppState>,
    triple_id: String,
) -> Result<Option<TripleDetailView>, String> {
    let id = uuid::Uuid::parse_str(&triple_id).map_err(|e| format!("bad uuid: {e}"))?;
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let detail = graph
        .triple_detail(id)
        .map_err(|e| format!("triple_detail: {e}"))?;
    Ok(detail.map(TripleDetailView::from))
}

#[derive(Serialize)]
struct ResolveContradictionResult {
    retracted: Vec<String>,
    kept: Vec<String>,
}

#[tauri::command]
fn cmd_resolve_contradiction(
    state: State<AppState>,
    triple_a: String,
    triple_b: String,
    choice: String,
) -> Result<ResolveContradictionResult, String> {
    let ta = uuid::Uuid::parse_str(&triple_a).map_err(|e| format!("bad triple_a uuid: {e}"))?;
    let tb = uuid::Uuid::parse_str(&triple_b).map_err(|e| format!("bad triple_b uuid: {e}"))?;
    let ch = match choice.as_str() {
        "keep_a" | "KeepA" => tm_graph::ResolveChoice::KeepA,
        "keep_b" | "KeepB" => tm_graph::ResolveChoice::KeepB,
        "keep_both" | "KeepBoth" => tm_graph::ResolveChoice::KeepBoth,
        other => return Err(format!("unknown choice: {other}")),
    };
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let (retracted, kept) = graph
        .resolve_contradiction_by_triples(ta, tb, ch)
        .ok_or_else(|| "no matching contradiction".to_string())?;
    Ok(ResolveContradictionResult {
        retracted: retracted.into_iter().map(|u| u.to_string()).collect(),
        kept: kept.into_iter().map(|u| u.to_string()).collect(),
    })
}

// ---------------------------------------------------------------------------
// Outcome-prompt drawer — D-4 / E series (Shot 4 of demo)
// ---------------------------------------------------------------------------
//
// User clicks an overdue commitment in the brief, the drawer opens with
// "Did you ship it? What was the outcome?" and the user picks completed +
// polarity. We insert an Outcome and transition the commitment state, in
// the same shape `tm-mcp::commit_outcome` uses.

#[derive(Serialize)]
struct RecordOutcomeResult {
    outcome_id: String,
    commitment_id: String,
    commitment_state: String,
    polarity: String,
}

#[tauri::command]
fn cmd_record_outcome(
    state: State<AppState>,
    commitment_id: String,
    polarity: String,
    description: Option<String>,
    user_note: Option<String>,
) -> Result<RecordOutcomeResult, String> {
    use tm_intent::{IntentStore, Outcome, OutcomeSource, Polarity, State as CState};
    use tm_intent::state::transition;

    let cid =
        uuid::Uuid::parse_str(&commitment_id).map_err(|e| format!("bad commitment uuid: {e}"))?;
    let polarity_norm = polarity.to_lowercase();
    let pol = match polarity_norm.as_str() {
        "better" | "positive" => Polarity::Better,
        "as_expected" | "neutral" => Polarity::AsExpected,
        "worse" | "negative" => Polarity::Worse,
        "mixed" => Polarity::Mixed,
        "no_outcome" => Polarity::NoOutcome,
        other => return Err(format!("invalid polarity: {other}")),
    };

    let dir = data_dir(&state);
    let intents_path = dir.join("intents.db");
    let store = IntentStore::open(intents_path.to_str().unwrap_or_default())
        .map_err(|e| format!("open intents: {e}"))?;
    let mut commitment = store
        .get_commitment(cid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("commitment {cid} not found"))?;

    let mut outcome = Outcome::new(
        cid,
        pol,
        description.unwrap_or_default(),
        OutcomeSource::UserPrompted,
    );
    outcome.user_note = user_note;

    transition(&mut commitment, CState::Completed, Some(&outcome))
        .map_err(|e| format!("state transition rejected: {e}"))?;
    store.insert_outcome(&outcome).map_err(|e| e.to_string())?;
    store
        .update_state(commitment.id, commitment.state, commitment.outcome_id)
        .map_err(|e| e.to_string())?;

    Ok(RecordOutcomeResult {
        outcome_id: outcome.id.to_string(),
        commitment_id: commitment.id.to_string(),
        commitment_state: format!("{:?}", commitment.state).to_lowercase(),
        polarity: polarity_norm,
    })
}

// ---------------------------------------------------------------------------
// Sprint D / F-1 — feedback channels + context CRUD
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct FeedbackAck {
    row_id: i64,
    kind: String,
}

/// 👍 button — write a row to `positive_signals`. Bandit reward for
/// the originating query gets a soft additive nudge on next finalisation.
#[tauri::command]
fn cmd_helpful(
    state: State<AppState>,
    query_id: String,
    result_id: String,
    weight: Option<f32>,
    kind: Option<String>,
) -> Result<FeedbackAck, String> {
    let qid = Uuid::parse_str(&query_id).map_err(|e| format!("bad query_id: {e}"))?;
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = graph.active_context_id();
    let kind_s = kind.unwrap_or_else(|| "helpful".to_string());
    let w = weight.unwrap_or(0.3);
    let row_id = graph
        .write_positive_signal(qid, &result_id, &kind_s, ctx, w)
        .map_err(|e| e.to_string())?;
    bump_usage(&state, |s| {
        s.last_helpful_at = Some(chrono::Utc::now().to_rfc3339());
        s.total_helpful = s.total_helpful.saturating_add(1);
    });
    Ok(FeedbackAck { row_id, kind: kind_s })
}

/// 👎 / "wrong context" button — write a row to `negative_signals`.
#[tauri::command]
fn cmd_not_related(
    state: State<AppState>,
    query_id: String,
    result_id: String,
    weight: Option<f32>,
    kind: Option<String>,
) -> Result<FeedbackAck, String> {
    let qid = Uuid::parse_str(&query_id).map_err(|e| format!("bad query_id: {e}"))?;
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = graph.active_context_id();
    let kind_s = kind.unwrap_or_else(|| "not_related".to_string());
    let w = weight.unwrap_or(1.0);
    // For now, both context_a and context_b = active. A future iteration
    // can resolve the offending result's owning context from the graph
    // and surface the actual cross-context pair to the rerank learner.
    let row_id = graph
        .write_negative_signal(qid, &result_id, &kind_s, ctx, ctx, w)
        .map_err(|e| e.to_string())?;
    bump_usage(&state, |s| {
        s.last_negative_at = Some(chrono::Utc::now().to_rfc3339());
        s.total_negative = s.total_negative.saturating_add(1);
    });
    Ok(FeedbackAck { row_id, kind: kind_s })
}

#[derive(Serialize)]
struct ContextInfo {
    id: String,
    name: String,
    tags: String,
    is_active: bool,
}

#[tauri::command]
fn cmd_context_list(state: State<AppState>) -> Result<Vec<ContextInfo>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let active = graph.active_context_id();
    let rows = graph.list_contexts().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|c| ContextInfo {
            id: c.id.to_string(),
            name: c.name,
            tags: c.tags,
            is_active: Some(c.id) == active,
        })
        .collect())
}

#[tauri::command]
fn cmd_context_current(state: State<AppState>) -> Result<Option<ContextInfo>, String> {
    let dir = data_dir(&state);
    let p = dir.join("active_context.json");
    match tm_graph::context::ActiveContext::load(&p).map_err(|e| e.to_string())? {
        Some(a) => Ok(Some(ContextInfo {
            id: a.id.to_string(),
            name: a.name,
            tags: String::new(),
            is_active: true,
        })),
        None => Ok(None),
    }
}

/// Switch the active context by name. Writes `active_context.json`
/// atomically so the next IngestPipeline / RetrievalEngine open picks
/// it up. The in-process retrieval engine is also updated so the
/// switch takes effect immediately for the current run.
#[tauri::command]
fn cmd_context_use(state: State<AppState>, name: String) -> Result<ContextInfo, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = graph
        .get_context_by_name(&name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no context named '{name}'"))?;

    let active = tm_graph::context::ActiveContext { id: ctx.id, name: ctx.name.clone() };
    let dir = data_dir(&state);
    let p = dir.join("active_context.json");
    active.save(&p).map_err(|e| e.to_string())?;

    // Propagate to the live retrieval engine + ingest pipeline so the
    // switch is hot — no app restart required.
    if let Ok(mut engine) = state.retrieval.lock() {
        engine.set_active_context(Some(ctx.id));
    }

    Ok(ContextInfo {
        id: ctx.id.to_string(),
        name: ctx.name,
        tags: ctx.tags,
        is_active: true,
    })
}

#[tauri::command]
fn cmd_context_create(
    state: State<AppState>,
    name: String,
    tags: Option<String>,
) -> Result<ContextInfo, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = tm_graph::Context::new(&name, tags.unwrap_or_default());
    graph.create_context(&ctx).map_err(|e| e.to_string())?;
    // Re-fetch in case the row already existed (ON CONFLICT DO NOTHING).
    let existing = graph
        .get_context_by_name(&name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("context '{name}' not created"))?;
    Ok(ContextInfo {
        id: existing.id.to_string(),
        name: existing.name,
        tags: existing.tags,
        is_active: false,
    })
}

#[tauri::command]
fn cmd_context_clear(state: State<AppState>) -> Result<(), String> {
    let dir = data_dir(&state);
    let p = dir.join("active_context.json");
    tm_graph::context::ActiveContext::clear(&p).map_err(|e| e.to_string())?;
    if let Ok(mut engine) = state.retrieval.lock() {
        engine.set_active_context(None);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LM-17 — Tauri "Export this context" command
//
// Wraps `GraphStore::snapshot_context` so the Settings panel / context-
// switcher "Export" menu item can save a `.tmctx` JSON bundle without
// shelling out to the CLI. The frontend opens a save dialog and passes
// the chosen path here.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct ExportContextResult {
    /// Where the file was written (echoed back so the UI can show a
    /// "saved to" toast).
    output_path: String,
    /// Canonical UUID of the context that was exported.
    context_id: String,
    /// Number of entities included in the snapshot.
    entity_count: u64,
    /// Number of triples included in the snapshot.
    triple_count: u64,
    /// Bytes on disk after pretty-printing.
    bytes_written: u64,
}

/// LM-17-be — write a `.tmctx` snapshot of the named context to
/// `output_path`. Errors when the context is unknown or the write
/// fails. The frontend supplies `output_path` from a Tauri save dialog,
/// so the backend never picks a path on its own.
#[tauri::command]
fn cmd_export_context(
    state: State<AppState>,
    name: String,
    output_path: String,
) -> Result<ExportContextResult, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = graph
        .get_context_by_name(&name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no context named '{name}'"))?;
    let snapshot = graph.snapshot_context(&ctx).map_err(|e| e.to_string())?;
    let pretty =
        serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize: {e}"))?;
    let bytes = pretty.len() as u64;
    std::fs::write(&output_path, pretty)
        .map_err(|e| format!("write {output_path}: {e}"))?;

    Ok(ExportContextResult {
        output_path,
        context_id: ctx.id.to_string(),
        entity_count: snapshot["counts"]["entities"].as_u64().unwrap_or(0),
        triple_count: snapshot["counts"]["triples"].as_u64().unwrap_or(0),
        bytes_written: bytes,
    })
}

// ---------------------------------------------------------------------------
// LM-1 — Backlinks panel (P5a Obsidian-parity)
//
// Returns every entity that points at the target via any non-RelatedTo
// predicate (sorted by triple confidence). The UI renders these as the
// "linked from" panel in the entity drawer (LM-3 entity-drawer rewrite).
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct BacklinkRow {
    triple_id: String,
    source_id: String,
    source_name: String,
    source_type: String,
    predicate: String,
    confidence: f64,
}

/// LM-1 — list incoming-edge backlinks for an entity.
///
/// `entity_id` accepts a UUID or an exact-match entity name. Returns at
/// most `limit` rows (default 50). `include_related_to=false` filters
/// out the noisy generic `RelatedTo` co-mention edges so the panel
/// shows only meaningful relations.
#[tauri::command]
fn cmd_entity_backlinks(
    state: State<AppState>,
    entity_id: String,
    limit: Option<usize>,
    include_related_to: Option<bool>,
) -> Result<Vec<BacklinkRow>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let target = match Uuid::parse_str(&entity_id) {
        Ok(u) => u,
        Err(_) => graph
            .find_entity_by_name_icase(&entity_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no entity matches '{entity_id}'"))?
            .id,
    };
    let rows = graph
        .backlinks(target, limit.or(Some(50)), include_related_to.unwrap_or(false))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|b| BacklinkRow {
            triple_id: b.triple_id.to_string(),
            source_id: b.source.id.to_string(),
            source_name: b.source.name,
            source_type: b.source.entity_type.to_string(),
            predicate: b.predicate.to_string(),
            confidence: b.confidence,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// LM-2 — Inline auto-rendered [[wikilinks]] (P5a Obsidian-parity)
//
// The UI sends the raw memory text; the backend returns byte-offset
// spans that the renderer turns into `<a>` chips. User never types
// `[[`.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct WikilinkSpan {
    entity_id: String,
    entity_name: String,
    entity_type: String,
    start: usize,
    end: usize,
}

/// LM-2 — resolve entity mentions in `text`. Returns one span per
/// non-overlapping mention. Pass `context_id` (UUID) to limit candidate
/// entities to one context — matches the decoupled-by-default stance.
#[tauri::command]
fn cmd_resolve_wikilinks(
    state: State<AppState>,
    text: String,
    context_id: Option<String>,
) -> Result<Vec<WikilinkSpan>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let ctx = match context_id {
        Some(s) if !s.is_empty() => {
            Some(Uuid::parse_str(&s).map_err(|e| format!("bad context_id: {e}"))?)
        }
        _ => None,
    };
    let mentions = graph
        .resolve_entity_in_text(&text, ctx)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(mentions.len());
    for m in mentions {
        // fetch the canonical entity name + type so the UI doesn't
        // need a second round-trip per span.
        let ent = match graph.get_entity(m.entity_id) {
            Ok(e) => e,
            Err(_) => continue,
        };
        out.push(WikilinkSpan {
            entity_id: m.entity_id.to_string(),
            entity_name: ent.name,
            entity_type: ent.entity_type.to_string(),
            start: m.start,
            end: m.end,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// LM-11d-fe — Memory Views Tauri command surface (P5c user-power)
//
// Mirrors `memory_views_*` MCP tools so Tauri threads can edit splices
// without shelling out to the CLI. The frontend renders these in the
// thread sidebar (LM-11e session-scoped splice).
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct ViewSummary {
    id: String,
    name: String,
    description: String,
    confidence_floor: f32,
    include_pending: bool,
    updated_at: String,
    member_count: usize,
}

#[derive(Serialize, Clone)]
struct ViewMemberRow {
    kind: String,
    member_type: String,
    member_id: String,
    added_at: String,
}

#[derive(Serialize, Clone)]
struct ViewDetail {
    view: ViewSummary,
    members: Vec<ViewMemberRow>,
}

fn resolve_view(
    graph: &GraphStore,
    name_or_id: &str,
) -> Result<tm_graph::MemoryView, String> {
    if let Ok(id) = Uuid::parse_str(name_or_id) {
        if let Some(v) = graph.get_view(id).map_err(|e| e.to_string())? {
            return Ok(v);
        }
    }
    graph
        .get_view_by_name(name_or_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no view named '{name_or_id}'"))
}

#[tauri::command]
fn cmd_views_list(state: State<AppState>) -> Result<Vec<ViewSummary>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let views = graph.list_views().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(views.len());
    for v in views {
        let members = graph.list_view_members(v.id).map_err(|e| e.to_string())?;
        out.push(ViewSummary {
            id: v.id.to_string(),
            name: v.name,
            description: v.description,
            confidence_floor: v.confidence_floor,
            include_pending: v.include_pending,
            updated_at: v.updated_at.to_rfc3339(),
            member_count: members.len(),
        });
    }
    Ok(out)
}

#[tauri::command]
fn cmd_views_show(state: State<AppState>, name: String) -> Result<ViewDetail, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let v = resolve_view(&graph, &name)?;
    let members = graph.list_view_members(v.id).map_err(|e| e.to_string())?;
    Ok(ViewDetail {
        view: ViewSummary {
            id: v.id.to_string(),
            name: v.name,
            description: v.description,
            confidence_floor: v.confidence_floor,
            include_pending: v.include_pending,
            updated_at: v.updated_at.to_rfc3339(),
            member_count: members.len(),
        },
        members: members
            .into_iter()
            .map(|m| ViewMemberRow {
                kind: m.kind.as_str().to_string(),
                member_type: m.member_type.as_str().to_string(),
                member_id: m.member_id.to_string(),
                added_at: m.added_at.to_rfc3339(),
            })
            .collect(),
    })
}

#[tauri::command]
fn cmd_views_create(
    state: State<AppState>,
    name: String,
    description: Option<String>,
) -> Result<ViewSummary, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let v = tm_graph::MemoryView::new(name, description.unwrap_or_default());
    graph.create_view(&v).map_err(|e| e.to_string())?;
    Ok(ViewSummary {
        id: v.id.to_string(),
        name: v.name,
        description: v.description,
        confidence_floor: v.confidence_floor,
        include_pending: v.include_pending,
        updated_at: v.updated_at.to_rfc3339(),
        member_count: 0,
    })
}

#[tauri::command]
fn cmd_views_add(
    state: State<AppState>,
    name: String,
    kind: String,
    member_type: String,
    member_id: String,
) -> Result<(), String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let v = resolve_view(&graph, &name)?;
    let k = tm_graph::MemberKind::parse(&kind).map_err(|e| e.to_string())?;
    let mt = tm_graph::MemberType::parse(&member_type).map_err(|e| e.to_string())?;
    let mid = Uuid::parse_str(&member_id).map_err(|e| format!("bad member_id: {e}"))?;
    graph.add_view_member(v.id, k, mt, mid).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn cmd_views_remove(
    state: State<AppState>,
    name: String,
    kind: String,
    member_type: String,
    member_id: String,
) -> Result<bool, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let v = resolve_view(&graph, &name)?;
    let k = tm_graph::MemberKind::parse(&kind).map_err(|e| e.to_string())?;
    let mt = tm_graph::MemberType::parse(&member_type).map_err(|e| e.to_string())?;
    let mid = Uuid::parse_str(&member_id).map_err(|e| format!("bad member_id: {e}"))?;
    graph
        .remove_view_member(v.id, k, mt, mid)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cmd_views_delete(state: State<AppState>, name: String) -> Result<bool, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let v = resolve_view(&graph, &name)?;
    graph.delete_view(v.id).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// LM-5c-fe — `Today` view (DailyNote surface)
//
// Returns the entity_id of today's auto-generated DailyNote so the
// Tauri "Today" tab can deep-link into it. Read-only — creation is
// done by the `tracemind today` CLI / capture loop (LM-5c-be).
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct DailyNotePointer {
    entity_id: String,
    entity_name: String,
    backlink_count: usize,
}

#[tauri::command]
fn cmd_daily_note_today(
    state: State<AppState>,
) -> Result<Option<DailyNotePointer>, String> {
    use chrono::Local;
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let today = Local::now().date_naive().to_string();
    let ent = match graph
        .find_entity_by_name_icase(&today)
        .map_err(|e| e.to_string())?
    {
        Some(e) => e,
        None => return Ok(None),
    };
    // Only return it if it really is a DailyNote (guard against a
    // collision with some other entity that happens to have a date
    // string as its name).
    if !matches!(ent.entity_type, tm_types::EntityType::DailyNote) {
        return Ok(None);
    }
    let backlinks = graph
        .backlinks(ent.id, Some(500), true)
        .map_err(|e| e.to_string())?;
    Ok(Some(DailyNotePointer {
        entity_id: ent.id.to_string(),
        entity_name: ent.name,
        backlink_count: backlinks.len(),
    }))
}

// ---------------------------------------------------------------------------
// LM-3 — Entity drawer composite view (header + backlinks + relations + tags)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct EntityDrawerHeader {
    entity_id: String,
    name: String,
    entity_type: String,
    ontological_domain: String,
    confidence: f64,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize, Clone)]
struct EntityDrawerBacklink {
    triple_id: String,
    source_id: String,
    source_name: String,
    predicate: String,
    confidence: f64,
}

#[derive(Serialize, Clone)]
struct EntityDrawerRelation {
    triple_id: String,
    target_id: String,
    target_name: String,
    predicate: String,
    confidence: f64,
}

#[derive(Serialize, Clone)]
struct EntityDrawerView {
    header: EntityDrawerHeader,
    backlinks: Vec<EntityDrawerBacklink>,
    relations: Vec<EntityDrawerRelation>,
    tags: Vec<String>,
}

/// LM-3 — composite entity drawer payload. Resolves `entity_id` (UUID
/// or name), fetches header + backlinks + outgoing relations + tags
/// in one call so the frontend isn't waterfalling 4 invokes.
#[tauri::command]
fn cmd_entity_drawer(
    entity_id: String,
    backlink_limit: Option<usize>,
    state: State<AppState>,
) -> Result<EntityDrawerView, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    // Resolve UUID-or-name.
    let ent = if let Ok(id) = Uuid::parse_str(&entity_id) {
        graph.get_entity(id).map_err(|e| e.to_string())?
    } else {
        graph
            .find_entity_by_name_icase(&entity_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("entity not found: {entity_id}"))?
    };

    let domain = tm_graph::classify_ontology(&ent.entity_type, &ent.name);
    let header = EntityDrawerHeader {
        entity_id: ent.id.to_string(),
        name: ent.name.clone(),
        entity_type: format!("{:?}", ent.entity_type),
        ontological_domain: domain.to_string(),
        confidence: ent.confidence,
        created_at: ent.created_at.to_rfc3339(),
        updated_at: ent.updated_at.to_rfc3339(),
    };

    let mut backlinks = Vec::new();
    for b in graph
        .backlinks(ent.id, Some(backlink_limit.unwrap_or(50)), true)
        .map_err(|e| e.to_string())?
    {
        backlinks.push(EntityDrawerBacklink {
            triple_id: b.triple_id.to_string(),
            source_id: b.source.id.to_string(),
            source_name: b.source.name,
            predicate: format!("{}", b.predicate),
            confidence: b.confidence,
        });
    }

    let mut relations = Vec::new();
    let outgoing = graph
        .get_triples_for_entity(ent.id)
        .map_err(|e| e.to_string())?;
    for t in outgoing {
        if t.subject_id != ent.id {
            continue;
        }
        let target_name = graph
            .get_entity(t.object_id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_else(|| t.object_id.to_string());
        relations.push(EntityDrawerRelation {
            triple_id: t.id.to_string(),
            target_id: t.object_id.to_string(),
            target_name,
            predicate: format!("{}", t.predicate),
            confidence: t.confidence,
        });
    }

    // Tags: just the entity-type fallback today. When LM-5b's
    // ingest-time tag persistence is added, swap in the stored set.
    let tags = vec![tm_ingest::entity_type_tag(&ent.entity_type)];

    Ok(EntityDrawerView {
        header,
        backlinks,
        relations,
        tags,
    })
}

// ---------------------------------------------------------------------------
// LM-5a — Transclusion resolver (`![[entity_id]]` → inline memory text)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct TransclusionSpan {
    /// Byte offset of the `![[` in the source string.
    start: usize,
    /// Byte offset just after the closing `]]`.
    end: usize,
    /// Resolved entity (None when the target id doesn't resolve).
    entity_id: Option<String>,
    entity_name: Option<String>,
    /// Short preview of the target entity's own description / source.
    preview: Option<String>,
}

/// LM-5a — find every `![[entity_id]]` transclusion in `text` and
/// return resolved spans the frontend can replace inline.
#[tauri::command]
fn cmd_resolve_transclusion(
    text: String,
    state: State<AppState>,
) -> Result<Vec<TransclusionSpan>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let mut out: Vec<TransclusionSpan> = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"![[" {
            // Find the closing `]]`.
            if let Some(rel_end) = text[i + 3..].find("]]") {
                let body_start = i + 3;
                let body_end = body_start + rel_end;
                let body = &text[body_start..body_end];
                let span_end = body_end + 2;
                let resolved = if let Ok(uuid) = Uuid::parse_str(body.trim()) {
                    graph.get_entity(uuid).ok()
                } else {
                    graph.find_entity_by_name_icase(body.trim()).ok().flatten()
                };
                let (entity_id, entity_name, preview) = match resolved {
                    Some(e) => {
                        let preview = e.source_id.clone();
                        (Some(e.id.to_string()), Some(e.name), preview)
                    }
                    None => (None, None, None),
                };
                out.push(TransclusionSpan {
                    start: i,
                    end: span_end,
                    entity_id,
                    entity_name,
                    preview,
                });
                i = span_end;
                continue;
            }
        }
        i += 1;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// LM-11e — Session-scoped splice: per-thread saved view state
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
struct ThreadViewState {
    view_name: Option<String>,
    include_ids: Vec<String>,
    exclude_ids: Vec<String>,
}

fn thread_views_path(state: &AppState) -> PathBuf {
    data_dir(state).join("thread_views.json")
}

fn load_thread_views(
    state: &AppState,
) -> Result<std::collections::HashMap<String, ThreadViewState>, String> {
    let path = thread_views_path(state);
    if !path.exists() {
        return Ok(Default::default());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Ok(Default::default());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn save_thread_views(
    state: &AppState,
    map: &std::collections::HashMap<String, ThreadViewState>,
) -> Result<(), String> {
    let path = thread_views_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

/// LM-11e — persist the active splice for `thread_id`.
#[tauri::command]
fn cmd_thread_view_save(
    thread_id: String,
    view_name: Option<String>,
    include_ids: Vec<String>,
    exclude_ids: Vec<String>,
    state: State<AppState>,
) -> Result<(), String> {
    let mut map = load_thread_views(&state)?;
    map.insert(
        thread_id,
        ThreadViewState {
            view_name,
            include_ids,
            exclude_ids,
        },
    );
    save_thread_views(&state, &map)
}

/// LM-11e — load the splice previously saved for `thread_id` (returns
/// an empty state when the thread has never been spliced).
#[tauri::command]
fn cmd_thread_view_load(
    thread_id: String,
    state: State<AppState>,
) -> Result<ThreadViewState, String> {
    let map = load_thread_views(&state)?;
    Ok(map.get(&thread_id).cloned().unwrap_or_default())
}

/// LM-11e — forget the per-thread state (e.g. when the thread is
/// closed permanently).
#[tauri::command]
fn cmd_thread_view_clear(thread_id: String, state: State<AppState>) -> Result<bool, String> {
    let mut map = load_thread_views(&state)?;
    let removed = map.remove(&thread_id).is_some();
    save_thread_views(&state, &map)?;
    Ok(removed)
}

/// Listed view row for the Views sidebar surface.
#[derive(Serialize, Clone)]
struct ThreadViewRow {
    thread_id: String,
    view_name: Option<String>,
    include_count: usize,
    exclude_count: usize,
}

/// LM-11e — list every saved per-thread splice. Powers the Views surface
/// so saved splices are discoverable, not buried behind a query.
/// Named views come first (sorted by name), unnamed views after.
#[tauri::command]
fn cmd_thread_views_list(state: State<AppState>) -> Result<Vec<ThreadViewRow>, String> {
    let map = load_thread_views(&state)?;
    let mut rows: Vec<ThreadViewRow> = map
        .into_iter()
        .filter(|(_, v)| !v.include_ids.is_empty() || !v.exclude_ids.is_empty() || v.view_name.is_some())
        .map(|(thread_id, v)| ThreadViewRow {
            thread_id,
            view_name: v.view_name,
            include_count: v.include_ids.len(),
            exclude_count: v.exclude_ids.len(),
        })
        .collect();
    rows.sort_by(|a, b| match (&a.view_name, &b.view_name) {
        (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.thread_id.cmp(&b.thread_id),
    });
    Ok(rows)
}

// ---------------------------------------------------------------------------
// LM-20/22/23 — Memory Garden, outliers, community overlay
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct GardenCard {
    /// Cluster bucket; `None` for "unclustered" / outlier.
    cluster_id: Option<i64>,
    /// Human label. Until CLU-6 lands this is `"cluster {id}"` or
    /// `"unsorted"`.
    label: String,
    count: usize,
    sample_texts: Vec<String>,
}

/// LM-20 — Memory Garden cards. Groups `captured_signals` by their
/// `cluster_id` (the column already exists; populated by the future
/// `tm-cluster` HDBSCAN pass). A `cluster_id = -1` or `NULL` row is
/// treated as an outlier and shown as the "Unsorted" tray (LM-22
/// reads the same data).
#[tauri::command]
fn cmd_memory_garden(state: State<AppState>) -> Result<Vec<GardenCard>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let buckets = graph.cluster_buckets(50).map_err(|e| e.to_string())?;
    // LM-21 — c-TF-IDF labels for real clusters. Outlier buckets get
    // the static "unsorted" / "ignored" labels below.
    let labels = graph.cluster_labels(25).map_err(|e| e.to_string())?;
    let mut cards = Vec::new();
    for (cluster_id, count) in buckets {
        let (label, is_outlier) = match cluster_id {
            None => ("unsorted".to_string(), true),
            Some(-1) => ("unsorted".to_string(), true),
            Some(-2) => ("ignored".to_string(), true),
            Some(id) => {
                let label = labels
                    .get(&id)
                    .map(|cl| cl.label.clone())
                    .unwrap_or_else(|| format!("cluster {id}"));
                (label, false)
            }
        };
        let lookup = if is_outlier { None } else { cluster_id };
        let raw_samples = graph
            .cluster_samples(lookup, 3)
            .map_err(|e| e.to_string())?;
        let samples: Vec<String> = raw_samples
            .into_iter()
            .map(|t| if t.len() > 120 { format!("{}…", &t[..120]) } else { t })
            .collect();
        cards.push(GardenCard {
            cluster_id,
            label,
            count,
            sample_texts: samples,
        });
    }
    Ok(cards)
}

#[derive(Serialize, Clone)]
struct OutlierRow {
    signal_id: i64,
    raw_text: String,
    source: String,
    created_at: String,
}

/// LM-22 — list the outlier tray (captured signals with no cluster).
#[tauri::command]
fn cmd_outliers_list(
    limit: Option<usize>,
    state: State<AppState>,
) -> Result<Vec<OutlierRow>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let lim = limit.unwrap_or(50).min(500);
    let rows = graph
        .list_outlier_signals(lim)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(signal_id, raw_text, source, created_at)| OutlierRow {
            signal_id,
            raw_text,
            source,
            created_at,
        })
        .collect())
}

/// LM-22 — triage one outlier into an existing cluster, into a new
/// cluster, or mark "ignore" (which sets `cluster_id = -2` so the
/// next HDBSCAN pass treats it as a deliberate single-point group).
#[tauri::command]
fn cmd_outlier_triage(
    signal_id: i64,
    action: String,         // "add_to" | "new_cluster" | "ignore"
    target_cluster: Option<i64>,
    state: State<AppState>,
) -> Result<i64, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let new_cluster: i64 = match action.as_str() {
        "add_to" => target_cluster
            .ok_or_else(|| "add_to action requires target_cluster".to_string())?,
        "new_cluster" => graph.next_cluster_id().map_err(|e| e.to_string())?,
        "ignore" => -2,
        other => return Err(format!("unknown triage action: {other}")),
    };
    graph
        .assign_signal_to_cluster(signal_id, new_cluster)
        .map_err(|e| e.to_string())?;
    Ok(new_cluster)
}

#[derive(Serialize, Clone)]
struct CommunityRow {
    /// Louvain `community_id`. `None` means the entity hasn't been
    /// community-tagged yet (waiting on CLU-* work in `tm-graph`).
    community_id: Option<i64>,
    entity_count: usize,
    sample_names: Vec<String>,
    /// 2026-05-12 — short "Top1 · Top2 · Top3" label for this
    /// community, picked from `sample_names`. Lets the Garden card
    /// say "Aaditya · TraceMind · Rust" instead of "Community 0".
    /// Empty string for the unassigned bucket.
    label: String,
}

/// LM-23 — community-overlay toggle. Groups entities by their stored
/// Louvain `community_id`. Until that column is populated this
/// returns a single `None` bucket covering every entity — the call
/// stays stable, the data shape doesn't change when CLU-* lands.
#[tauri::command]
fn cmd_community_overlay(state: State<AppState>) -> Result<Vec<CommunityRow>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let buckets = graph.community_buckets(50).map_err(|e| e.to_string())?;
    // Prefer the persisted label (c-TF-IDF first pass; LLM relabel for top
    // communities). Falls back to a member-name composite when the row is
    // missing — keeps the API stable for fresh DBs without any consolidate.
    let stored_labels: std::collections::HashMap<i64, String> = graph
        .community_label_map()
        .map(|m| m.into_iter().map(|(k, v)| (k as i64, v.label)).collect())
        .unwrap_or_default();
    let pick_label = |cid: i64, names: &[String]| -> String {
        stored_labels
            .get(&cid)
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| label_from_names(names))
    };
    // LM-22 fix — if the stored column hasn't been populated yet (no CLU-*
    // pass has run), `community_buckets` returns a single `None` bucket.
    // The Graph view computes Louvain on-the-fly via `graph.louvain()` and
    // surfaces 12+ communities; the Garden overlay was lagging behind.
    // Fall back to on-the-fly Louvain so the two views agree and so the
    // "view in graph" drill-down actually has something to drill into.
    let unassigned =
        buckets.len() == 1 && buckets[0].0.is_none();
    if !buckets.is_empty() && !unassigned {
        return Ok(buckets
            .into_iter()
            .map(|(community_id, entity_count, sample_names)| {
                let label = community_id
                    .map(|cid| pick_label(cid, &sample_names))
                    .unwrap_or_default();
                CommunityRow {
                    community_id,
                    entity_count,
                    sample_names,
                    label,
                }
            })
            .collect());
    }

    // On-the-fly Louvain path. Mirrors `cmd_graph` so node communities
    // match exactly.
    let communities = graph.louvain().unwrap_or_default();
    if communities.is_empty() {
        // No graph data yet — propagate the original single-bucket result.
        return Ok(buckets
            .into_iter()
            .map(|(community_id, entity_count, sample_names)| CommunityRow {
                community_id,
                entity_count,
                sample_names,
                label: String::new(),
            })
            .collect());
    }
    let entities = graph
        .inner()
        .list_entities(None, None)
        .map_err(|e| format!("list entities: {e}"))?;
    use std::collections::HashMap;
    let mut by_community: HashMap<i32, Vec<String>> = HashMap::new();
    for ent in &entities {
        let uuid_str = ent
            .get_property("uuid")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let Ok(u) = uuid::Uuid::parse_str(&uuid_str) else { continue };
        if let Some(&cid) = communities.get(&u) {
            by_community.entry(cid).or_default().push(ent.name.clone());
        }
    }
    let mut rows: Vec<CommunityRow> = by_community
        .into_iter()
        .map(|(cid, mut names)| {
            let entity_count = names.len();
            names.truncate(8);
            let label = pick_label(cid as i64, &names);
            CommunityRow {
                community_id: Some(cid as i64),
                entity_count,
                sample_names: names,
                label,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.entity_count.cmp(&a.entity_count));
    rows.truncate(50);
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Context Dashboard (post-P5) — one-call fat payload that surfaces every
// brain layer for a single entity: header, decay, graph neighborhoods,
// vector neighbors, k-hop, community, belief status, contradictions,
// bitemporal provenance, recent traces, and signal-level neighbors.
//
// Why a single command:
//   - The previous `cmd_entity_drawer` only returned header + relations +
//     backlinks + tags. The user feedback (May 2026) was "I want all the
//     context we have on X" — graph is *one* part of the brain. The fat
//     dump joins data the UI was previously waterfalling 5–6 invokes for,
//     and surfaces decay scores (recency / novelty / value / frequency)
//     prominently so the user can see "what TraceMind still values" at a
//     glance.
//   - No new storage; every field maps to an existing GraphStore /
//     TraceStore method. Safe to add — `cmd_entity_drawer` is untouched.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct ContextDecay {
    /// exp(-0.05 * hours_since_last_access). 1.0 = just touched, 0.0 = cold.
    recency: f64,
    /// 1.0 / (1.0 + ln(1 + access_count)). Higher = less seen.
    novelty: f64,
    /// retrieval feedback value score (0..1). 0 = never given feedback.
    value: f64,
    /// 1.0 / (retrieved + 1). 1.0 = never retrieved, decays toward 0.
    frequency: f64,
    /// Total entries in access_log for this entity.
    access_count: i64,
    /// RFC-3339 timestamp of the most recent access, or None.
    last_access: Option<String>,
}

#[derive(Serialize, Clone)]
struct ContextNeighbor {
    entity_id: String,
    name: String,
    entity_type: String,
    similarity: f64,
}

#[derive(Serialize, Clone)]
struct ContextHop {
    entity_id: String,
    name: String,
    entity_type: String,
}

#[derive(Serialize, Clone)]
struct ContextBeliefRow {
    triple_id: String,
    subject: String,
    predicate: String,
    object: String,
    /// "In" | "Out" | "Contradicted" | "Unknown"
    status: String,
}

#[derive(Serialize, Clone)]
struct ContextContradiction {
    id: String,
    triple_a: String,
    triple_b: String,
    detected_at: String,
    cosine_similarity: f32,
    /// "KeepA" | "KeepB" | "KeepBoth" | None
    resolution: Option<String>,
}

#[derive(Serialize, Clone)]
struct ContextProvenanceRow {
    name: String,
    entity_type: String,
    confidence: f64,
    valid_from: String,
    recorded_at: String,
    superseded_at: Option<String>,
}

#[derive(Serialize, Clone)]
struct ContextTraceRow {
    trace_id: String,
    event_type: String,
    raw_text: Option<String>,
    retrieval_arm: Option<u8>,
    created_at: String,
}

#[derive(Serialize, Clone)]
struct ContextSignal {
    signal_id: i64,
    raw_text: String,
    source: String,
    similarity: f32,
    created_at: String,
}

#[derive(Serialize, Clone)]
struct ContextCommunity {
    /// Louvain community id, None if community detection hasn't been run
    /// or the entity is orphaned in the graph.
    community_id: Option<i64>,
    /// "Top1 · Top2 · Top3" label drawn from siblings, "" if unassigned.
    label: String,
    sibling_count: usize,
    /// Up to 8 sibling entity names (excluding self).
    sibling_names: Vec<String>,
}

#[derive(Serialize, Clone)]
struct ContextReasoningStep {
    entity_id: String,
    entity_name: String,
    predicate: String,
    direction: String,
    confidence: f64,
}

#[derive(Serialize, Clone)]
struct ContextReasoningChain {
    target_id: String,
    target_name: String,
    score: f64,
    steps: Vec<ContextReasoningStep>,
}

#[derive(Serialize, Clone)]
struct ContextAnalogy {
    target_id: String,
    target_name: String,
    similarity: f64,
    shared_patterns: Vec<String>,
    explanation: String,
}

#[derive(Serialize, Clone)]
struct ContextBanditUse {
    arm: u8,
    arm_name: String,
    pulls: u64,
}

#[derive(Serialize, Clone)]
struct ContextIntentRow {
    id: String,
    statement: String,
    state: String,
    horizon: Option<String>,
}

#[derive(Serialize, Clone)]
struct EntityContextDump {
    header: EntityDrawerHeader,
    decay: ContextDecay,
    relations_out: Vec<EntityDrawerRelation>,
    relations_in: Vec<EntityDrawerBacklink>,
    vector_neighbors: Vec<ContextNeighbor>,
    k_hop_neighbors: Vec<ContextHop>,
    community: ContextCommunity,
    belief_rows: Vec<ContextBeliefRow>,
    contradictions: Vec<ContextContradiction>,
    provenance: Vec<ContextProvenanceRow>,
    recent_traces: Vec<ContextTraceRow>,
    signal_neighbors: Vec<ContextSignal>,
    /// Inspector P3 — multi-hop reasoning chains starting from this entity
    /// (top N by score). Surfaced as the "Reasoning paths" panel.
    reasoning_chains: Vec<ContextReasoningChain>,
    /// Inspector P3 — structural analogies for this entity.
    analogies: Vec<ContextAnalogy>,
    /// Inspector P3 — which retrieval arms have returned this entity, by
    /// pull count. Derived from scanning recent retrieval traces whose
    /// `entities_extracted` set includes this entity.
    bandit_arms_used: Vec<ContextBanditUse>,
    /// Inspector P3 — commitments / intents whose statement mentions this
    /// entity name. Empty if no such intent exists.
    related_intents: Vec<ContextIntentRow>,
}

/// Returns the full context dump for one entity in a single call.
/// Resolves `entity_id` as either a UUID or a case-insensitive name.
#[tauri::command]
fn cmd_entity_context_dump(
    entity_id: String,
    state: State<AppState>,
) -> Result<EntityContextDump, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    // ── Resolve UUID-or-name ─────────────────────────────────────────────
    let ent = if let Ok(id) = Uuid::parse_str(&entity_id) {
        graph.get_entity(id).map_err(|e| e.to_string())?
    } else {
        graph
            .find_entity_by_name_icase(&entity_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("entity not found: {entity_id}"))?
    };

    // ── Header ───────────────────────────────────────────────────────────
    let domain = tm_graph::classify_ontology(&ent.entity_type, &ent.name);
    let header = EntityDrawerHeader {
        entity_id: ent.id.to_string(),
        name: ent.name.clone(),
        entity_type: format!("{:?}", ent.entity_type),
        ontological_domain: domain.to_string(),
        confidence: ent.confidence,
        created_at: ent.created_at.to_rfc3339(),
        updated_at: ent.updated_at.to_rfc3339(),
    };

    // ── Decay: recency / novelty / value / frequency ─────────────────────
    let recency = graph.recency_score(ent.id);
    let novelty = graph.novelty_score(ent.id);
    let value_map = graph.batch_value_scores(&[ent.id]);
    let freq_map = graph.batch_frequency_scores(&[ent.id]);
    let value = value_map.get(&ent.id).copied().unwrap_or(0.0);
    let frequency = freq_map.get(&ent.id).copied().unwrap_or(1.0);
    // Back-fill access_count via the novelty formula:
    // novelty = 1/(1 + ln(1 + n))  ⇒  n = exp(1/novelty - 1) - 1.
    // recency_score (just queried above) already covers `last_access`
    // semantically — we expose the raw count + leave last_access None
    // because GraphStore doesn't yet expose a public accessor for the
    // access_log timestamp; adding one is a follow-up if the UI needs it.
    let access_count: i64 = if novelty > 0.0 && novelty < 1.0 {
        let n = ((1.0 / novelty) - 1.0).exp() - 1.0;
        n.round() as i64
    } else {
        0
    };
    let last_access: Option<String> = None;
    let decay = ContextDecay {
        recency,
        novelty,
        value,
        frequency,
        access_count,
        last_access,
    };

    // ── Relations (outgoing) + backlinks ─────────────────────────────────
    let mut relations_out = Vec::new();
    let outgoing = graph
        .get_triples_for_entity(ent.id)
        .map_err(|e| e.to_string())?;
    // Keep the triple list around for belief/contradiction lookups so we
    // only pay one `get_triples_for_entity` per call.
    let mut owned_triples: Vec<tm_types::Triple> = Vec::new();
    for t in &outgoing {
        if t.subject_id != ent.id {
            continue;
        }
        let target_name = graph
            .get_entity(t.object_id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_else(|| t.object_id.to_string());
        relations_out.push(EntityDrawerRelation {
            triple_id: t.id.to_string(),
            target_id: t.object_id.to_string(),
            target_name,
            predicate: format!("{}", t.predicate),
            confidence: t.confidence,
        });
        owned_triples.push(t.clone());
    }

    let mut relations_in = Vec::new();
    for b in graph
        .backlinks(ent.id, Some(50), true)
        .map_err(|e| e.to_string())?
    {
        relations_in.push(EntityDrawerBacklink {
            triple_id: b.triple_id.to_string(),
            source_id: b.source.id.to_string(),
            source_name: b.source.name,
            predicate: format!("{}", b.predicate),
            confidence: b.confidence,
        });
    }

    // ── Vector neighbors ────────────────────────────────────────────────
    let mut vector_neighbors = Vec::new();
    let self_vector = graph.get_vector(ent.id).map_err(|e| e.to_string())?;
    if let Some(v) = &self_vector {
        let hits = graph.search_vectors(v, 11).map_err(|e| e.to_string())?;
        for (uid, sim) in hits {
            if uid == ent.id {
                continue;
            }
            if let Ok(e) = graph.get_entity(uid) {
                vector_neighbors.push(ContextNeighbor {
                    entity_id: uid.to_string(),
                    name: e.name,
                    entity_type: format!("{:?}", e.entity_type),
                    similarity: sim as f64,
                });
            }
            if vector_neighbors.len() >= 10 {
                break;
            }
        }
    }

    // ── K-hop graph neighbors (2 hops, capped) ──────────────────────────
    let mut k_hop_neighbors = Vec::new();
    if let Ok(hops) = graph.k_hop_neighbors(ent.id, 2) {
        for e in hops.into_iter().take(20) {
            if e.id == ent.id {
                continue;
            }
            k_hop_neighbors.push(ContextHop {
                entity_id: e.id.to_string(),
                name: e.name,
                entity_type: format!("{:?}", e.entity_type),
            });
        }
    }

    // ── Community ────────────────────────────────────────────────────────
    let community = match graph.louvain() {
        Ok(map) => {
            let my_cid = map.get(&ent.id).copied();
            if let Some(cid) = my_cid {
                let mut siblings: Vec<String> = map
                    .iter()
                    .filter(|(uid, &c)| **uid != ent.id && c == cid)
                    .filter_map(|(uid, _)| graph.get_entity(*uid).ok().map(|e| e.name))
                    .collect();
                let sibling_count = siblings.len();
                siblings.sort();
                siblings.truncate(8);
                let label = label_from_names(&siblings);
                ContextCommunity {
                    community_id: Some(cid as i64),
                    label,
                    sibling_count,
                    sibling_names: siblings,
                }
            } else {
                ContextCommunity {
                    community_id: None,
                    label: String::new(),
                    sibling_count: 0,
                    sibling_names: vec![],
                }
            }
        }
        Err(_) => ContextCommunity {
            community_id: None,
            label: String::new(),
            sibling_count: 0,
            sibling_names: vec![],
        },
    };

    // ── Belief status per triple touching this entity ───────────────────
    let mut belief_rows = Vec::new();
    let mut triple_id_set: HashSet<Uuid> = HashSet::new();
    for t in &owned_triples {
        triple_id_set.insert(t.id);
        let subj_name = graph
            .get_entity(t.subject_id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_else(|| t.subject_id.to_string());
        let obj_name = graph
            .get_entity(t.object_id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_else(|| t.object_id.to_string());
        let status = match graph.belief_status_for(t.id) {
            Some(tm_graph::BeliefStatus::In) => "In",
            Some(tm_graph::BeliefStatus::Out) => "Out",
            Some(tm_graph::BeliefStatus::Contradicted) => "Contradicted",
            None => "Unknown",
        }
        .to_string();
        belief_rows.push(ContextBeliefRow {
            triple_id: t.id.to_string(),
            subject: subj_name,
            predicate: format!("{}", t.predicate),
            object: obj_name,
            status,
        });
    }
    // Belief rows for incoming triples (where this entity is the object).
    for b in &relations_in {
        if let Ok(tid) = Uuid::parse_str(&b.triple_id) {
            if !triple_id_set.contains(&tid) {
                triple_id_set.insert(tid);
                let status = match graph.belief_status_for(tid) {
                    Some(tm_graph::BeliefStatus::In) => "In",
                    Some(tm_graph::BeliefStatus::Out) => "Out",
                    Some(tm_graph::BeliefStatus::Contradicted) => "Contradicted",
                    None => "Unknown",
                }
                .to_string();
                belief_rows.push(ContextBeliefRow {
                    triple_id: b.triple_id.clone(),
                    subject: b.source_name.clone(),
                    predicate: b.predicate.clone(),
                    object: header.name.clone(),
                    status,
                });
            }
        }
    }

    // ── Contradictions involving any of this entity's triples ───────────
    let mut contradictions = Vec::new();
    for c in graph.contradictions() {
        if triple_id_set.contains(&c.triple_a) || triple_id_set.contains(&c.triple_b) {
            contradictions.push(ContextContradiction {
                id: c.id.to_string(),
                triple_a: c.triple_a.to_string(),
                triple_b: c.triple_b.to_string(),
                detected_at: c.detected_at.to_rfc3339(),
                cosine_similarity: c.cosine_similarity,
                resolution: c.resolution.map(|r| format!("{:?}", r)),
            });
        }
    }

    // ── Provenance: full version history for this entity ────────────────
    let mut provenance = Vec::new();
    if let Ok(history) = graph.entity_history(ent.id) {
        for (e, valid_from, recorded_at, superseded_at) in history {
            provenance.push(ContextProvenanceRow {
                name: e.name,
                entity_type: format!("{:?}", e.entity_type),
                confidence: e.confidence,
                valid_from: valid_from.to_rfc3339(),
                recorded_at: recorded_at.to_rfc3339(),
                superseded_at: superseded_at.map(|t| t.to_rfc3339()),
            });
        }
    }

    // ── Recent traces touching this entity ──────────────────────────────
    let mut recent_traces = Vec::new();
    if let Ok(ts) = state.trace_store.lock() {
        if let Ok(traces) = ts.recent(500) {
            for tr in traces {
                if tr.entities_extracted.contains(&ent.id) {
                    recent_traces.push(ContextTraceRow {
                        trace_id: tr.id.to_string(),
                        event_type: format!("{:?}", tr.event_type),
                        raw_text: tr.raw_text.clone(),
                        retrieval_arm: tr.retrieval_arm,
                        created_at: tr.created_at.to_rfc3339(),
                    });
                    if recent_traces.len() >= 30 {
                        break;
                    }
                }
            }
        }
    }

    // ── Signal neighbors: raw captures semantically near this entity ────
    let mut signal_neighbors = Vec::new();
    if let Some(v) = &self_vector {
        if let Ok(hits) = graph.search_signals(v, 10, 0.35) {
            for (s, sim) in hits {
                signal_neighbors.push(ContextSignal {
                    signal_id: s.id,
                    raw_text: if s.raw_text.len() > 240 {
                        format!("{}…", &s.raw_text[..240])
                    } else {
                        s.raw_text
                    },
                    source: s.source,
                    similarity: sim,
                    created_at: s.created_at.to_rfc3339(),
                });
            }
        }
    }

    // ── Reasoning chains: explore outward from this entity ──────────────
    let mut reasoning_chains: Vec<ContextReasoningChain> = Vec::new();
    {
        let builder = tm_reason::ChainBuilder::with_defaults(&graph);
        let chains = builder.explore(&[ent.id], 6);
        for c in chains.iter().take(6) {
            let steps: Vec<ContextReasoningStep> = c
                .steps
                .iter()
                .map(|s| ContextReasoningStep {
                    entity_id: s.entity_id.to_string(),
                    entity_name: s.entity_name.clone(),
                    predicate: s.predicate.clone(),
                    direction: format!("{:?}", s.direction),
                    confidence: s.confidence,
                })
                .collect();
            let target_name = steps
                .last()
                .map(|s| s.entity_name.clone())
                .unwrap_or_else(|| header.name.clone());
            reasoning_chains.push(ContextReasoningChain {
                target_id: c.destination_id.to_string(),
                target_name,
                score: c.score,
                steps,
            });
        }
    }

    // ── Analogies: structural similarity ────────────────────────────────
    let mut analogies: Vec<ContextAnalogy> = Vec::new();
    {
        let solver = tm_reason::AnalogySolver::new(&graph);
        let results = solver.find_analogies(ent.id, 5);
        for r in results {
            analogies.push(ContextAnalogy {
                target_id: r.target_id.to_string(),
                target_name: r.target_name,
                similarity: r.similarity,
                shared_patterns: r.shared_patterns,
                explanation: r.explanation,
            });
        }
    }

    // ── Bandit arms used: count retrievals that returned this entity ────
    let bandit_arms_used: Vec<ContextBanditUse> = {
        let mut counts: HashMap<u8, u64> = HashMap::new();
        if let Ok(ts) = state.trace_store.lock() {
            if let Ok(traces) = ts.recent(5000) {
                for tr in &traces {
                    if matches!(tr.event_type, tm_types::TraceEventType::Retrieve)
                        && tr.entities_extracted.contains(&ent.id)
                    {
                        if let Some(a) = tr.retrieval_arm {
                            *counts.entry(a).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        let mut by_arm: Vec<ContextBanditUse> = counts
            .into_iter()
            .map(|(arm, pulls)| ContextBanditUse {
                arm,
                arm_name: UcbBandit::arm_name(arm).to_string(),
                pulls,
            })
            .collect();
        by_arm.sort_by(|a, b| b.pulls.cmp(&a.pulls));
        by_arm
    };

    // ── Related intents: commitments whose statement mentions this name ─
    let mut related_intents: Vec<ContextIntentRow> = Vec::new();
    {
        let dir = data_dir(&state);
        let intents_path = dir.join("intents.db");
        if let Ok(store) = tm_intent::IntentStore::open(
            intents_path.to_str().unwrap_or_default(),
        ) {
            if let Ok(open) = store.list_open(200) {
                let needle = header.name.to_lowercase();
                for c in open
                    .into_iter()
                    .filter(|c| c.statement.to_lowercase().contains(&needle))
                    .take(10)
                {
                    related_intents.push(ContextIntentRow {
                        id: c.id.to_string(),
                        statement: c.statement,
                        state: format!("{:?}", c.state),
                        horizon: c.horizon.map(|h| h.format("%Y-%m-%d").to_string()),
                    });
                }
            }
        }
    }

    Ok(EntityContextDump {
        header,
        decay,
        relations_out,
        relations_in,
        vector_neighbors,
        k_hop_neighbors,
        community,
        belief_rows,
        contradictions,
        provenance,
        recent_traces,
        signal_neighbors,
        reasoning_chains,
        analogies,
        bandit_arms_used,
        related_intents,
    })
}

// ---------------------------------------------------------------------------
// Inspector — Brain Snapshot, Why This Answer, Layer Browser, Entity Export
// (dev_mode gated UI)
//
// The Inspector is the developer-mode surface for staring directly at every
// layer of TraceMind's brain at once. It piggy-backs on real state from
// disk + the running engine — no stubs, no mocks. Three of its four panels
// are commands defined here; the fourth (Context dump) reuses the existing
// `cmd_entity_context_dump`.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BrainArmRow {
    arm: u8,
    name: String,
    ucb_pulls: u64,
    ucb_avg_reward: f64,
    linucb_pulls: u64,
    linucb_weight_mag: f64,
}

#[derive(Serialize)]
struct BrainEventBreakdown {
    event_type: String,
    count: u64,
}

#[derive(Serialize)]
struct BrainSnapshot {
    /// RFC-3339 generation timestamp.
    generated_at: String,
    /// Absolute path to the data dir.
    data_dir: String,

    // Graph layer
    entity_count: usize,
    triple_count: usize,
    contradiction_count: usize,
    pending_relation_count: usize,
    /// Louvain community count (0 if not yet detected).
    community_count: usize,

    // Vector layer
    vector_dim: usize,
    /// Number of entities with embeddings populated.
    entities_with_vector: usize,

    // Bandit layer
    bandit_arms: Vec<BrainArmRow>,
    /// Live annealed exploration coefficient from LinUCB.
    linucb_alpha: f64,

    // Episodic layer
    total_traces: u64,
    by_event: Vec<BrainEventBreakdown>,
    recent_buffer_size: usize,
    /// Capacity of the ring buffer (`RecentStore::DEFAULT_CAPACITY` unless
    /// overridden).
    recent_buffer_capacity: usize,

    // Governance / Captures
    governance_blocks_today: u64,
    /// Captures since the start of the local day.
    captures_today: u64,

    // Intent layer
    open_commitments: usize,
    overdue_commitments: usize,
    pending_candidates: usize,
    pattern_silences_active: usize,
}

#[tauri::command]
fn cmd_brain_snapshot(state: State<AppState>) -> Result<BrainSnapshot, String> {
    let dir = data_dir(&state);
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;

    let entity_count = graph.entity_count().unwrap_or(0);
    let triple_count = graph.triple_count().unwrap_or(0);
    let contradiction_count = graph.contradictions().len();
    let pending_relation_count = graph
        .list_pending(None, None)
        .map(|v: Vec<tm_graph::PendingRelation>| v.len())
        .unwrap_or(0);
    let community_count = graph
        .louvain()
        .map(|m| {
            let mut set: HashSet<i64> = HashSet::new();
            for &c in m.values() {
                set.insert(c as i64);
            }
            set.len()
        })
        .unwrap_or(0);

    // Vector layer: count entities that have an embedding. GraphStore does
    // not expose a direct count, so iterate (bounded — entity_count is
    // already small in practice for local-only mode).
    let entities_with_vector: usize = graph
        .list_all_entities()
        .map(|ents| {
            ents.into_iter()
                .filter(|e| graph.get_vector(e.id).map(|v| v.is_some()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);

    // Bandit layer — load from disk for the most-recent persisted state.
    let ucb = UcbBandit::load(&state.bandit_path);
    let linucb_path = dir.join("linucb.json");
    let linucb = tm_controller::LinUcbBandit::load(&linucb_path);
    let ucb_stats = ucb.arm_stats();
    let linucb_stats = linucb.arm_stats();
    let linucb_alpha = linucb.alpha();
    let bandit_arms: Vec<BrainArmRow> = (0..tm_controller::NUM_ARMS as u8)
        .map(|arm| {
            let i = arm as usize;
            BrainArmRow {
                arm,
                name: UcbBandit::arm_name(arm).to_string(),
                ucb_pulls: ucb_stats[i].0,
                ucb_avg_reward: ucb_stats[i].1,
                linucb_pulls: linucb_stats[i].0,
                linucb_weight_mag: linucb_stats[i].1,
            }
        })
        .collect();

    // Episodic layer — break down by event type and compute today's
    // governance blocks + capture totals.
    let mut by_event_map: HashMap<String, u64> = HashMap::new();
    let mut total_traces: u64 = 0;
    let mut governance_blocks_today: u64 = 0;
    let mut captures_today: u64 = 0;
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    if let Ok(ts) = state.trace_store.lock() {
        if let Ok(traces) = ts.recent(usize::MAX) {
            total_traces = traces.len() as u64;
            for t in &traces {
                let ev = format!("{:?}", t.event_type);
                *by_event_map.entry(ev).or_insert(0) += 1;
                if t.created_at >= today_start {
                    if matches!(t.event_type, tm_types::TraceEventType::Ingest) {
                        captures_today += 1;
                        if !t.confidence_gate_passed {
                            governance_blocks_today += 1;
                        }
                    }
                }
            }
        }
    }
    let mut by_event: Vec<BrainEventBreakdown> = by_event_map
        .into_iter()
        .map(|(k, v)| BrainEventBreakdown {
            event_type: k,
            count: v,
        })
        .collect();
    by_event.sort_by(|a, b| b.count.cmp(&a.count));

    // Recent ring buffer depth — open the same path the capture daemon
    // writes to. Missing file → depth 0.
    let recent_path = dir.join("recent.jsonl");
    let (recent_buffer_size, recent_buffer_capacity) =
        match tm_episodic::RecentStore::open(&recent_path) {
            Ok(rs) => {
                let depth = rs.read_all().map(|v| v.len()).unwrap_or(0);
                (depth, rs.capacity())
            }
            Err(_) => (0, tm_episodic::RECENT_DEFAULT_CAPACITY),
        };

    // Intent layer — open the intents.db and pull counts. Each call is one
    // bounded SELECT.
    let mut open_commitments = 0usize;
    let mut overdue_commitments = 0usize;
    let mut pending_candidates = 0usize;
    let mut pattern_silences_active = 0usize;
    let intents_path = dir.join("intents.db");
    if let Ok(store) =
        tm_intent::IntentStore::open(intents_path.to_str().unwrap_or_default())
    {
        open_commitments = store.list_open(10_000).map(|v| v.len()).unwrap_or(0);
        overdue_commitments = store
            .list_overdue_open(chrono::Utc::now(), 10_000)
            .map(|v| v.len())
            .unwrap_or(0);
        pending_candidates = store
            .list_pending_candidates(10_000)
            .map(|v| v.len())
            .unwrap_or(0);
        pattern_silences_active = store
            .list_active_pattern_silences(chrono::Utc::now())
            .map(|v| v.len())
            .unwrap_or(0);
    }

    Ok(BrainSnapshot {
        generated_at: chrono::Utc::now().to_rfc3339(),
        data_dir: dir.to_string_lossy().to_string(),
        entity_count,
        triple_count,
        contradiction_count,
        pending_relation_count,
        community_count,
        vector_dim: 384,
        entities_with_vector,
        bandit_arms,
        linucb_alpha,
        total_traces,
        by_event,
        recent_buffer_size,
        recent_buffer_capacity,
        governance_blocks_today,
        captures_today,
        open_commitments,
        overdue_commitments,
        pending_candidates,
        pattern_silences_active,
    })
}

// ---------------------------------------------------------------------------
// Why-this-answer: per-trace deep dive (Inspector Panel 2)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct WhyArmRow {
    arm: u8,
    name: String,
    top_k: usize,
    hops: u32,
    include_episodic: bool,
    include_colbert: bool,
    /// LinUCB exploit term (w_a · x).
    exploit_score: f64,
    /// LinUCB exploration term (α · sqrt(Σ x²/(v+1))).
    explore_score: f64,
    /// Sum: exploit + α·explore.
    total_score: f64,
    /// Current per-arm pull count from the persisted LinUCB state.
    pulls: u64,
}

#[derive(Serialize)]
struct WhyEntityRow {
    entity_id: String,
    name: String,
    entity_type: String,
    confidence: f64,
}

#[derive(Serialize)]
struct WhyTraceView {
    /// Selected trace's UUID, echoed back.
    trace_id: String,
    /// Trace event type (Ingest, Retrieve, Feedback, ...).
    event_type: String,
    /// Trace creation timestamp (RFC-3339).
    created_at: String,
    /// Raw query / capture text (None if `raw_text` was None in trace).
    raw_text: Option<String>,
    /// QueryPlanner.plan(raw_text) output (action / complexity / confidence
    /// / entity_hints) when raw_text is present. None otherwise.
    plan_action: Option<String>,
    plan_complexity: Option<String>,
    plan_confidence: Option<f64>,
    plan_entity_hints: Vec<String>,
    /// Arm the trace was issued under (only present for Retrieve traces).
    selected_arm: Option<u8>,
    selected_arm_name: Option<String>,
    /// Per-arm LinUCB scores computed from the *current* LinUCB state +
    /// the trace's raw_text re-embedded. Empty when raw_text is None.
    arm_scores: Vec<WhyArmRow>,
    /// Entities the trace returned (resolved to current names; entities
    /// that have since been deleted are skipped).
    entities: Vec<WhyEntityRow>,
    /// Recorded retrieval latency in ms (Retrieve traces only).
    latency_ms: Option<u32>,
    /// Whether ingest's PII / confidence gate passed (Ingest traces only).
    confidence_gate_passed: bool,
    /// Current annealed LinUCB α — context for the explore_score column.
    linucb_alpha: f64,
}

/// Inspector Panel 2 — "Why this answer?" for a single trace.
///
/// Given a trace UUID, returns:
///  * The trace's event type / arm / latency / text
///  * The planner classification for the raw text (re-run now)
///  * Per-arm LinUCB scores recomputed from the *current* LinUCB state
///    using the raw text's embedding as the context vector
///  * The entities the trace returned, resolved to current names
///
/// We can't perfectly reconstruct the *historical* per-arm scores at the
/// moment of decision (we don't store them). We surface the *current*
/// LinUCB state's read on the same context, which lets the user reason
/// about why the model would choose differently today.
#[tauri::command]
fn cmd_trace_why(
    trace_id: String,
    state: State<AppState>,
) -> Result<WhyTraceView, String> {
    let target_uuid = Uuid::parse_str(&trace_id)
        .map_err(|_| format!("invalid trace_id: {trace_id}"))?;

    // ── Locate the trace ────────────────────────────────────────────────
    let trace = {
        let ts = state.trace_store.lock().map_err(|e| e.to_string())?;
        let traces = ts.recent(20_000).map_err(|e| e.to_string())?;
        traces
            .into_iter()
            .find(|t| t.id == target_uuid)
            .ok_or_else(|| format!("trace not found: {trace_id}"))?
    };

    let raw_text = trace.raw_text.clone();
    let selected_arm = trace.retrieval_arm;
    let selected_arm_name = selected_arm.map(|a| UcbBandit::arm_name(a).to_string());

    // ── Planner classification (only if raw_text is available) ──────────
    let (plan_action, plan_complexity, plan_confidence, plan_entity_hints) =
        if let Some(text) = raw_text.as_ref() {
            let engine = state.retrieval.lock().map_err(|e| e.to_string())?;
            let plan = engine.plan_query(text);
            (
                Some(format!("{:?}", plan.action)),
                Some(plan.complexity),
                Some(plan.confidence),
                plan.entity_hints,
            )
        } else {
            (None, None, None, Vec::new())
        };

    // ── Per-arm LinUCB score recomputation ──────────────────────────────
    let dir = data_dir(&state);
    let linucb_path = dir.join("linucb.json");
    let linucb_state: serde_json::Value = std::fs::read_to_string(&linucb_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);

    let alpha = linucb_state
        .get("alpha")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);
    let weights = linucb_state
        .get("weights")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let variances = linucb_state
        .get("variances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let arm_bias = linucb_state
        .get("arm_bias")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let counts_arr = linucb_state
        .get("counts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut arm_scores: Vec<WhyArmRow> = Vec::new();
    if let Some(text) = raw_text.as_ref() {
        let engine = state.retrieval.lock().map_err(|e| e.to_string())?;
        let ctx: Vec<f32> = engine.embed_query(text);
        if ctx.len() == 384 {
            let x: Vec<f64> = ctx.iter().map(|&v| v as f64).collect();
            for arm in 0..tm_controller::NUM_ARMS as u8 {
                let i = arm as usize;
                let params = UcbBandit::params_for_arm(arm);
                let w_row = weights.get(i).and_then(|v| v.as_array());
                let v_row = variances.get(i).and_then(|v| v.as_array());
                let bias = arm_bias
                    .get(i)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let pulls = counts_arr
                    .get(i)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let (exploit, explore) = match (w_row, v_row) {
                    (Some(w), Some(v)) if w.len() == 384 && v.len() == 384 => {
                        let exploit: f64 = w
                            .iter()
                            .zip(x.iter())
                            .map(|(wj, xj)| {
                                wj.as_f64().unwrap_or(0.0) * xj
                            })
                            .sum::<f64>()
                            + bias;
                        let explore: f64 = x
                            .iter()
                            .zip(v.iter())
                            .map(|(xj, vj)| {
                                let vv = vj.as_f64().unwrap_or(1.0);
                                (xj * xj) / (vv + 1.0)
                            })
                            .sum::<f64>()
                            .sqrt();
                        (exploit, explore)
                    }
                    _ => (bias, 0.0),
                };
                let total = exploit + alpha * explore;
                arm_scores.push(WhyArmRow {
                    arm,
                    name: UcbBandit::arm_name(arm).to_string(),
                    top_k: params.top_k,
                    hops: params.hops,
                    include_episodic: params.include_episodic,
                    include_colbert: params.include_colbert,
                    exploit_score: exploit,
                    explore_score: explore,
                    total_score: total,
                    pulls,
                });
            }
        }
    }

    // ── Resolve entities the trace returned ─────────────────────────────
    let mut entities: Vec<WhyEntityRow> = Vec::new();
    if !trace.entities_extracted.is_empty() {
        let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
        for eid in &trace.entities_extracted {
            if let Ok(e) = graph.get_entity(*eid) {
                entities.push(WhyEntityRow {
                    entity_id: e.id.to_string(),
                    name: e.name,
                    entity_type: format!("{:?}", e.entity_type),
                    confidence: e.confidence,
                });
            }
        }
    }

    Ok(WhyTraceView {
        trace_id: trace.id.to_string(),
        event_type: format!("{:?}", trace.event_type),
        created_at: trace.created_at.to_rfc3339(),
        raw_text,
        plan_action,
        plan_complexity,
        plan_confidence,
        plan_entity_hints,
        selected_arm,
        selected_arm_name,
        arm_scores,
        entities,
        latency_ms: trace.retrieval_latency_ms,
        confidence_gate_passed: trace.confidence_gate_passed,
        linucb_alpha: alpha,
    })
}

// ---------------------------------------------------------------------------
// Layer browser: raw per-layer state inspection (Inspector Panel 4)
// ---------------------------------------------------------------------------
//
// `cmd_inspector_layer(layer)` returns a JSON object describing the raw
// state of one layer. The frontend renders it as a pretty-printed payload
// + a few summary stats. Layers: "vector", "graph", "episodic", "bandit",
// "governance", "reason".
//
// We return `serde_json::Value` so each layer can ship whatever shape
// fits — the layer browser's job is to expose the truth, not normalize
// it. The frontend renders the JSON in a syntax-highlighted block.

#[tauri::command]
fn cmd_inspector_layer(
    layer: String,
    state: State<AppState>,
) -> Result<serde_json::Value, String> {
    let dir = data_dir(&state);
    match layer.as_str() {
        "vector" => {
            let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
            let entity_count = graph.entity_count().unwrap_or(0);
            // Walk entities and check each for a vector. Bounded by entity_count.
            let all_ents = graph.list_all_entities().unwrap_or_default();
            let with_vectors = all_ents
                .iter()
                .filter(|e| graph.get_vector(e.id).map(|v| v.is_some()).unwrap_or(false))
                .count();
            // Sample 25 most-recently-created entities for a "neighborhood preview".
            let mut sorted = all_ents;
            sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let sample_names = sorted
                .into_iter()
                .take(25)
                .map(|e| e.name)
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "layer": "vector",
                "dimension": 384,
                "model": "BGE-small-en-v1.5 (fastembed)",
                "entity_count": entity_count,
                "entities_with_vector": with_vectors,
                "coverage_pct": if entity_count > 0 {
                    (with_vectors as f64 / entity_count as f64) * 100.0
                } else { 0.0 },
                "sample_entities": sample_names,
            }))
        }
        "graph" => {
            let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
            let entity_count = graph.entity_count().unwrap_or(0);
            let triple_count = graph.triple_count().unwrap_or(0);
            let contradictions = graph.contradictions();
            let pending = graph
                .list_pending(None, Some(50))
                .unwrap_or_default();
            let louvain = graph.louvain().unwrap_or_default();
            let mut by_cluster: HashMap<i64, usize> = HashMap::new();
            for &c in louvain.values() {
                *by_cluster.entry(c as i64).or_insert(0) += 1;
            }
            let mut community_sizes: Vec<(i64, usize)> =
                by_cluster.into_iter().collect();
            community_sizes.sort_by(|a, b| b.1.cmp(&a.1));
            community_sizes.truncate(20);
            Ok(serde_json::json!({
                "layer": "graph",
                "entity_count": entity_count,
                "triple_count": triple_count,
                "contradiction_count": contradictions.len(),
                "pending_relations": pending.len(),
                "community_count": community_sizes.len(),
                "top_communities": community_sizes
                    .iter()
                    .map(|(c, n)| serde_json::json!({"community_id": c, "size": n}))
                    .collect::<Vec<_>>(),
            }))
        }
        "episodic" => {
            let mut total = 0usize;
            let mut by_type: HashMap<String, u64> = HashMap::new();
            let mut recent_arm: HashMap<u8, u64> = HashMap::new();
            let mut last_at: Option<String> = None;
            if let Ok(ts) = state.trace_store.lock() {
                if let Ok(traces) = ts.recent(usize::MAX) {
                    total = traces.len();
                    for t in &traces {
                        *by_type.entry(format!("{:?}", t.event_type)).or_insert(0) += 1;
                        if let Some(a) = t.retrieval_arm {
                            *recent_arm.entry(a).or_insert(0) += 1;
                        }
                    }
                    if let Some(last) = traces.last() {
                        last_at = Some(last.created_at.to_rfc3339());
                    }
                }
            }
            let recent_path = dir.join("recent.jsonl");
            let recent_depth = tm_episodic::RecentStore::open(&recent_path)
                .ok()
                .and_then(|rs| rs.read_all().ok())
                .map(|v| v.len())
                .unwrap_or(0);
            Ok(serde_json::json!({
                "layer": "episodic",
                "total_traces": total,
                "last_trace_at": last_at,
                "by_event_type": by_type,
                "by_retrieval_arm": recent_arm
                    .into_iter()
                    .map(|(arm, n)| serde_json::json!({
                        "arm": arm,
                        "name": UcbBandit::arm_name(arm),
                        "pulls": n,
                    }))
                    .collect::<Vec<_>>(),
                "recent_buffer_size": recent_depth,
                "recent_buffer_capacity": tm_episodic::RECENT_DEFAULT_CAPACITY,
            }))
        }
        "bandit" => {
            let ucb = UcbBandit::load(&state.bandit_path);
            let linucb_path = dir.join("linucb.json");
            let linucb = tm_controller::LinUcbBandit::load(&linucb_path);
            let ucb_stats = ucb.arm_stats();
            let linucb_stats = linucb.arm_stats();
            let arms: Vec<serde_json::Value> = (0..tm_controller::NUM_ARMS as u8)
                .map(|arm| {
                    let i = arm as usize;
                    let p = UcbBandit::params_for_arm(arm);
                    serde_json::json!({
                        "arm": arm,
                        "name": UcbBandit::arm_name(arm),
                        "config": {
                            "top_k": p.top_k,
                            "hops": p.hops,
                            "include_episodic": p.include_episodic,
                            "include_colbert": p.include_colbert,
                        },
                        "ucb": {
                            "pulls": ucb_stats[i].0,
                            "avg_reward": ucb_stats[i].1,
                        },
                        "linucb": {
                            "pulls": linucb_stats[i].0,
                            "avg_weight_magnitude": linucb_stats[i].1,
                        },
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "layer": "bandit",
                "ucb_state_path": state.bandit_path.to_string_lossy(),
                "linucb_state_path": linucb_path.to_string_lossy(),
                "linucb_alpha": linucb.alpha(),
                "linucb_dim": 384,
                "arms": arms,
            }))
        }
        "governance" => {
            // Walk the trace log and bucket gate failures by source / day.
            let mut blocked_total = 0u64;
            let mut blocked_today = 0u64;
            let today_start = chrono::Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .single()
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);
            let mut last_blocked: Option<String> = None;
            if let Ok(ts) = state.trace_store.lock() {
                if let Ok(traces) = ts.recent(usize::MAX) {
                    for t in &traces {
                        if !t.confidence_gate_passed {
                            blocked_total += 1;
                            if t.created_at >= today_start {
                                blocked_today += 1;
                            }
                            last_blocked = Some(t.created_at.to_rfc3339());
                        }
                    }
                }
            }
            Ok(serde_json::json!({
                "layer": "governance",
                "policy": "PII regex + confidence gate (stdlib-only)",
                "blocked_total": blocked_total,
                "blocked_today": blocked_today,
                "last_blocked_at": last_blocked,
                "trace_log": dir.join("traces.jsonl").to_string_lossy(),
            }))
        }
        "reason" => {
            // Surface what the reasoning surface can do over the current graph:
            // contradictions detected, MOC clusters, ChainBuilder hops.
            let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
            let contradictions = graph.contradictions();
            let resolved = contradictions
                .iter()
                .filter(|c| c.resolution.is_some())
                .count();
            let unresolved = contradictions.len() - resolved;
            // Recent entities to seed an "explore from" — give the user a
            // tangible hint of what reason chains exist *right now*. Walk
            // list_all_entities() and sort by created_at desc (bounded).
            let mut all_ents = graph.list_all_entities().unwrap_or_default();
            all_ents.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let seeds = all_ents
                .into_iter()
                .take(8)
                .map(|e| {
                    serde_json::json!({"id": e.id.to_string(), "name": e.name})
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "layer": "reason",
                "contradictions_total": contradictions.len(),
                "contradictions_resolved": resolved,
                "contradictions_open": unresolved,
                "chain_builder": "tm_reason::ChainBuilder (BFS over typed predicates)",
                "analogy_solver": "tm_reason::AnalogySolver (shared pattern matching)",
                "seed_entities": seeds,
            }))
        }
        other => Err(format!(
            "unknown layer '{other}' — expected one of: vector, graph, episodic, bandit, governance, reason"
        )),
    }
}

// ---------------------------------------------------------------------------
// Inspector Panel 3 — markdown export for one entity's full context
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ExportEntityMarkdownResult {
    /// Where the file was written on disk.
    output_path: String,
    /// Bytes written.
    bytes_written: u64,
    /// UUID of the entity that was exported.
    entity_id: String,
}

/// Take the same dump cmd_entity_context_dump produces and render it as
/// portable markdown. Used by the Inspector's "export" button — the user
/// can drop the file straight into Obsidian / Bear / any plain-text PKM.
#[tauri::command]
fn cmd_export_entity_markdown(
    entity_id: String,
    output_path: String,
    state: State<AppState>,
) -> Result<ExportEntityMarkdownResult, String> {
    // Resolve relative output_path against the TraceMind data dir.
    // When launched from Finder, CWD is "/" (read-only), so a bare
    // "markdown/..." path would EROFS. Captured before state moves.
    let data_root = data_dir(&state);
    tracing::info!("[export-md] data_root={} output_path={}", data_root.display(), output_path);
    let dump = cmd_entity_context_dump(entity_id.clone(), state)?;

    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", dump.header.name));
    md.push_str(&format!(
        "- **Entity ID**: `{}`\n- **Type**: {}\n- **Ontological domain**: {}\n- **Confidence**: {:.2}\n- **Created**: {}\n- **Updated**: {}\n\n",
        dump.header.entity_id,
        dump.header.entity_type,
        dump.header.ontological_domain,
        dump.header.confidence,
        dump.header.created_at,
        dump.header.updated_at,
    ));

    md.push_str("## Temporal Decay\n\n");
    md.push_str(&format!(
        "| Axis | Value |\n|---|---|\n| Recency | {:.2} |\n| Novelty | {:.2} |\n| Value (feedback) | {:.2} |\n| Frequency | {:.2} |\n| Access count | {} |\n\n",
        dump.decay.recency,
        dump.decay.novelty,
        dump.decay.value,
        dump.decay.frequency,
        dump.decay.access_count,
    ));

    if dump.community.community_id.is_some() {
        md.push_str(&format!(
            "## Community\n\n**{}** (#{}) — {} siblings\n\n{}\n\n",
            if dump.community.label.is_empty() {
                "(unlabelled)".to_string()
            } else {
                dump.community.label.clone()
            },
            dump.community.community_id.unwrap_or(-1),
            dump.community.sibling_count,
            dump.community
                .sibling_names
                .iter()
                .map(|n| format!("- [[{}]]", n))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }

    if !dump.relations_out.is_empty() {
        md.push_str("## Outgoing relations\n\n");
        for r in &dump.relations_out {
            md.push_str(&format!(
                "- `{}` → [[{}]] _(conf {:.2})_\n",
                r.predicate, r.target_name, r.confidence
            ));
        }
        md.push('\n');
    }

    if !dump.relations_in.is_empty() {
        md.push_str("## Backlinks\n\n");
        for b in &dump.relations_in {
            md.push_str(&format!(
                "- [[{}]] `{}` _(conf {:.2})_\n",
                b.source_name, b.predicate, b.confidence
            ));
        }
        md.push('\n');
    }

    if !dump.vector_neighbors.is_empty() {
        md.push_str("## Vector neighbors\n\n");
        for n in &dump.vector_neighbors {
            md.push_str(&format!(
                "- [[{}]] _({}, sim {:.0}%)_\n",
                n.name,
                n.entity_type,
                n.similarity * 100.0,
            ));
        }
        md.push('\n');
    }

    if !dump.k_hop_neighbors.is_empty() {
        md.push_str("## 2-hop neighborhood\n\n");
        for h in &dump.k_hop_neighbors {
            md.push_str(&format!("- [[{}]] _({})_\n", h.name, h.entity_type));
        }
        md.push('\n');
    }

    if !dump.belief_rows.is_empty() {
        md.push_str("## Belief state\n\n");
        for r in &dump.belief_rows {
            md.push_str(&format!(
                "- **{}** {} {} {} — _{}_\n",
                r.status, r.subject, r.predicate, r.object, r.triple_id
            ));
        }
        md.push('\n');
    }

    if !dump.contradictions.is_empty() {
        md.push_str("## Contradictions\n\n");
        for c in &dump.contradictions {
            md.push_str(&format!(
                "- `{}` ⇄ `{}` (sim {:.2}, detected {}{})\n",
                &c.triple_a[..8.min(c.triple_a.len())],
                &c.triple_b[..8.min(c.triple_b.len())],
                c.cosine_similarity,
                &c.detected_at[..10.min(c.detected_at.len())],
                c.resolution
                    .as_ref()
                    .map(|r| format!(" → {}", r))
                    .unwrap_or_default(),
            ));
        }
        md.push('\n');
    }

    if !dump.reasoning_chains.is_empty() {
        md.push_str("## Reasoning paths\n\n");
        for c in &dump.reasoning_chains {
            md.push_str(&format!(
                "- _(score {:.2})_ → [[{}]]: ",
                c.score, c.target_name
            ));
            let path = c
                .steps
                .iter()
                .map(|s| format!("[[{}]]({})", s.entity_name, s.predicate))
                .collect::<Vec<_>>()
                .join(" → ");
            md.push_str(&path);
            md.push('\n');
        }
        md.push('\n');
    }

    if !dump.analogies.is_empty() {
        md.push_str("## Analogies\n\n");
        for a in &dump.analogies {
            md.push_str(&format!(
                "- [[{}]] _(sim {:.2})_ — {}\n",
                a.target_name, a.similarity, a.explanation
            ));
        }
        md.push('\n');
    }

    if !dump.bandit_arms_used.is_empty() {
        md.push_str("## Bandit arms that retrieved this\n\n");
        for a in &dump.bandit_arms_used {
            md.push_str(&format!(
                "- arm {} ({}) — {} pulls\n",
                a.arm, a.arm_name, a.pulls
            ));
        }
        md.push('\n');
    }

    if !dump.related_intents.is_empty() {
        md.push_str("## Related commitments\n\n");
        for i in &dump.related_intents {
            md.push_str(&format!(
                "- _{}_ — {}{}\n",
                i.state,
                i.statement,
                i.horizon
                    .as_ref()
                    .map(|h| format!(" (by {h})"))
                    .unwrap_or_default(),
            ));
        }
        md.push('\n');
    }

    if !dump.provenance.is_empty() {
        md.push_str("## Provenance\n\n");
        for p in &dump.provenance {
            md.push_str(&format!(
                "- `{}` — {} _(conf {:.2}{})_\n",
                &p.recorded_at[..16.min(p.recorded_at.len())],
                p.name,
                p.confidence,
                p.superseded_at
                    .as_ref()
                    .map(|_| ", superseded".to_string())
                    .unwrap_or_default(),
            ));
        }
        md.push('\n');
    }

    if !dump.recent_traces.is_empty() {
        md.push_str("## Recent traces\n\n");
        for t in &dump.recent_traces {
            md.push_str(&format!(
                "- `{}` {} — {}\n",
                &t.created_at[..16.min(t.created_at.len())],
                t.event_type,
                t.raw_text
                    .as_ref()
                    .map(|s| {
                        let s = s.replace('\n', " ");
                        if s.len() > 140 {
                            format!("{}…", &s[..140])
                        } else {
                            s
                        }
                    })
                    .unwrap_or_default(),
            ));
        }
        md.push('\n');
    }

    if !dump.signal_neighbors.is_empty() {
        md.push_str("## Raw-signal neighbors\n\n");
        for s in &dump.signal_neighbors {
            let snippet = if s.raw_text.len() > 200 {
                format!("{}…", &s.raw_text[..200])
            } else {
                s.raw_text.clone()
            };
            md.push_str(&format!(
                "- `{}` _({}, sim {:.0}%)_ — {}\n",
                s.source,
                &s.created_at[..16.min(s.created_at.len())],
                s.similarity * 100.0,
                snippet,
            ));
        }
        md.push('\n');
    }

    let bytes = md.len() as u64;
    // Resolve relative paths against the data dir so Finder-launched
    // app bundles (CWD = "/") don't try to write under root.
    let path_buf = std::path::Path::new(&output_path);
    let final_path = if path_buf.is_absolute() {
        path_buf.to_path_buf()
    } else {
        data_root.join(path_buf)
    };
    if let Some(parent) = final_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(&final_path, &md)
        .map_err(|e| format!("write {}: {e}", final_path.display()))?;

    Ok(ExportEntityMarkdownResult {
        output_path: final_path.to_string_lossy().to_string(),
        bytes_written: bytes,
        entity_id: dump.header.entity_id,
    })
}

// ---------------------------------------------------------------------------
// UI-13 — Capture permissions panel (per-source toggles + audit)
// ---------------------------------------------------------------------------

/// One row in the capture-permissions panel. Mirrors
/// `tm_types::SourcePermission` plus a human-readable description
/// and the canonical source string so the frontend doesn't need to
/// re-derive labels.
#[derive(Serialize, Clone)]
struct CapturePermissionRow {
    source: String,
    description: String,
    enabled: bool,
    default_enabled: bool,
    granted_at: Option<String>,
    last_event_at: Option<String>,
    event_count: u64,
}

fn permissions_path(state: &AppState) -> PathBuf {
    data_dir(state).join("capture_permissions.toml")
}

#[tauri::command]
fn cmd_capture_permissions_list(
    state: State<AppState>,
) -> Result<Vec<CapturePermissionRow>, String> {
    use tm_types::capture_permissions::{CapturePermissions, CaptureSource};
    let path = permissions_path(&state);
    let perms = CapturePermissions::load_or_default(&path).map_err(|e| e.to_string())?;
    let rows = CaptureSource::all()
        .iter()
        .map(|s| {
            let entry = perms.get(*s);
            CapturePermissionRow {
                source: s.as_str().to_string(),
                description: s.description().to_string(),
                enabled: entry.enabled,
                default_enabled: s.default_enabled(),
                granted_at: entry.granted_at.map(|t| t.to_rfc3339()),
                last_event_at: entry.last_event_at.map(|t| t.to_rfc3339()),
                event_count: entry.event_count,
            }
        })
        .collect();
    Ok(rows)
}

#[tauri::command]
fn cmd_capture_permissions_set(
    source: String,
    enabled: bool,
    state: State<AppState>,
) -> Result<(), String> {
    use tm_types::capture_permissions::{CapturePermissions, CaptureSource};
    let src = CaptureSource::parse(&source).ok_or_else(|| format!("unknown source: {source}"))?;
    let path = permissions_path(&state);
    let mut perms = CapturePermissions::load_or_default(&path).map_err(|e| e.to_string())?;
    if enabled {
        perms.enable(src);
    } else {
        perms.disable(src);
    }
    perms.save(&path).map_err(|e| e.to_string())?;
    tracing::info!("[capture] permission {} = {}", src, enabled);
    Ok(())
}

#[derive(Serialize, Clone)]
struct ForgetSourceResult {
    source: String,
    entities_removed: usize,
    traces_redacted: usize,
}

/// UI-13 — "forget all captures from this source". We don't yet have
/// a per-source entity index in the graph, so this is a best-effort
/// audit-log redaction: counts (and zero-fills) every `recent.jsonl`
/// entry tagged with the source, and resets the per-source counters
/// so the panel reflects the wipe. Real graph-side purge lands when
/// CAP-1 entity tagging closes the loop (Q2 task — tracked).
#[tauri::command]
fn cmd_capture_forget_source(
    source: String,
    state: State<AppState>,
) -> Result<ForgetSourceResult, String> {
    use tm_types::capture_permissions::{CapturePermissions, CaptureSource};
    let src = CaptureSource::parse(&source).ok_or_else(|| format!("unknown source: {source}"))?;
    let path = permissions_path(&state);
    let mut perms = CapturePermissions::load_or_default(&path).map_err(|e| e.to_string())?;

    // Reset the per-source counters so the panel reflects the wipe.
    if let Some(entry) = perms.sources.get_mut(src.as_str()) {
        entry.event_count = 0;
        entry.last_event_at = None;
    }
    perms.save(&path).map_err(|e| e.to_string())?;

    // Best-effort: rewrite recent.jsonl, dropping lines that name this
    // source. The audit trail (traces.jsonl) is preserved — users who
    // want a true wipe can `tracemind decay` or remove the DB.
    let recent_path = data_dir(&state).join("recent.jsonl");
    let mut traces_redacted = 0usize;
    if recent_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&recent_path) {
            let kept: Vec<&str> = text
                .lines()
                .filter(|line| {
                    let drop_this = line.contains(&format!("\"source\":\"{}\"", src.as_str()));
                    if drop_this {
                        traces_redacted += 1;
                    }
                    !drop_this
                })
                .collect();
            let new = kept.join("\n");
            let _ = std::fs::write(&recent_path, new);
        }
    }
    Ok(ForgetSourceResult {
        source: src.as_str().to_string(),
        entities_removed: 0, // tracked, ships with graph-side purge
        traces_redacted,
    })
}

// ---------------------------------------------------------------------------
// DP-3 — Local-only usage instrumentation
// ---------------------------------------------------------------------------
//
// `~/.tracemind/usage.json` is the privacy-preserving day-active
// counter. The desktop app (and the MCP server, when wired) bumps
// it whenever the user runs a query, marks something helpful, or
// flags a result as wrong-context. No upload, no telemetry. Users
// share via `tracemind share-usage --to <email>` (CLI) which prints
// the JSON for them to paste back.

#[derive(Serialize, serde::Deserialize, Clone, Default)]
struct UsageStats {
    /// First time the desktop / MCP saw the user.
    #[serde(default)]
    first_seen: Option<String>,
    /// Most recent query.
    #[serde(default)]
    last_query_at: Option<String>,
    /// Most recent positive (helpful) signal.
    #[serde(default)]
    last_helpful_at: Option<String>,
    /// Most recent negative (not-related / wrong-context) signal.
    #[serde(default)]
    last_negative_at: Option<String>,
    /// Lifetime counters.
    #[serde(default)]
    total_queries: u64,
    #[serde(default)]
    total_helpful: u64,
    #[serde(default)]
    total_negative: u64,
    /// Sorted unique calendar dates (YYYY-MM-DD, UTC) with at least
    /// one query. Capped at 90 entries; older days drop off.
    #[serde(default)]
    active_days: Vec<String>,
}

fn usage_path(state: &AppState) -> PathBuf {
    data_dir(state).join("usage.json")
}

fn load_usage(state: &AppState) -> UsageStats {
    let p = usage_path(state);
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_usage(state: &AppState, stats: &UsageStats) {
    if let Ok(text) = serde_json::to_string_pretty(stats) {
        let p = usage_path(state);
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&p, text);
    }
}

fn bump_usage<F: FnOnce(&mut UsageStats)>(state: &AppState, mutate: F) {
    let mut stats = load_usage(state);
    if stats.first_seen.is_none() {
        stats.first_seen = Some(chrono::Utc::now().to_rfc3339());
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if !stats.active_days.contains(&today) {
        stats.active_days.push(today);
        stats.active_days.sort();
        if stats.active_days.len() > 90 {
            let drop = stats.active_days.len() - 90;
            stats.active_days.drain(0..drop);
        }
    }
    mutate(&mut stats);
    save_usage(state, &stats);
}

#[tauri::command]
fn cmd_usage_stats(state: State<AppState>) -> Result<UsageStats, String> {
    Ok(load_usage(&state))
}

/// Returns a copy-pasteable JSON blob the user can email back to the
/// founder if they opt in. No automatic upload anywhere.
#[tauri::command]
fn cmd_usage_share_payload(state: State<AppState>) -> Result<String, String> {
    let stats = load_usage(&state);
    serde_json::to_string_pretty(&stats).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// UI-14 — Context-switch suggestion (CTX-2 stub)
// ---------------------------------------------------------------------------
//
// Lightweight implementation: when a query returns very few hits in
// the active context (≤ 1 entity, or top-confidence < 0.4) but the
// caller asked for a check, we look at the user's recent traces
// across other contexts and propose a switch if any other context
// has > 3 mentions of the query's primary tokens within the last
// 30 days. Real adaptive learning (per-pair thresholds, classifier)
// is the proper CTX-1..CTX-4 work — this is the seed-pitch shell.

#[derive(Serialize, Clone)]
struct ContextSuggestion {
    suggested_context: String,
    confidence: f64,
    reason: String,
}

#[tauri::command]
fn cmd_context_suggest(
    query_text: String,
    state: State<AppState>,
) -> Result<Option<ContextSuggestion>, String> {
    let graph = match GraphStore::open(&state.db_path) {
        Ok(g) => g,
        Err(_) => return Ok(None),
    };
    let contexts = graph.list_contexts().unwrap_or_default();
    if contexts.len() < 2 {
        return Ok(None);
    }
    let active_id = graph.active_context_id();
    let ctx_by_id: HashMap<Uuid, String> = contexts.iter().map(|c| (c.id, c.name.clone())).collect();

    // Naive token bag for matching. Lowercase + filter short tokens.
    let qtokens: Vec<String> = query_text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 4)
        .map(|t| t.to_string())
        .collect();
    if qtokens.is_empty() {
        return Ok(None);
    }

    // Walk all entities, bucketed by context. For each non-active
    // context, count entities whose name contains any query token.
    let entities = graph.list_all_entities().map_err(|e| e.to_string())?;
    let mut per_ctx: HashMap<Uuid, usize> = HashMap::new();
    for entity in &entities {
        let name = entity.name.to_lowercase();
        if !qtokens.iter().any(|t| name.contains(t)) {
            continue;
        }
        if let Ok(Some(ctx_id)) = graph.entity_context_id(entity.id) {
            if Some(ctx_id) == active_id {
                continue;
            }
            *per_ctx.entry(ctx_id).or_insert(0) += 1;
        }
    }
    let best = per_ctx
        .into_iter()
        .filter(|(_, n)| *n >= 3)
        .max_by_key(|(_, n)| *n);
    Ok(best.and_then(|(cid, n)| {
        ctx_by_id.get(&cid).map(|name| ContextSuggestion {
            suggested_context: name.clone(),
            confidence: ((n as f64) / 10.0).min(0.95),
            reason: format!("{n} entities in '{name}' match your query"),
        })
    }))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    // TM-NLP-005: bundled-model resolution must happen before any hf-hub /
    // fastembed code runs. In a packaged .app the Tauri bundler places
    // model weights under Contents/Resources/models/; the resolver finds
    // them via an exe-relative walk.
    let bundled = tm_types::bundled::init();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if let Some(r) = &bundled {
        tracing::info!(
            "[tm-tauri] bundled models resolved from {} ({})",
            r.hf_cache.display(),
            r.source
        );
    }

    let dir = if let Ok(val) = std::env::var("TM_DATA_DIR") {
        PathBuf::from(val)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".tracemind")
    };
    std::fs::create_dir_all(&dir).expect("failed to create data dir");

    let db_path = dir.join("memory.db").to_str().unwrap().to_string();
    let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();
    let bandit_path = dir.join("bandit.json");

    let mut ingest = IngestPipeline::open(&db_path, false)
        .expect("failed to open ingest pipeline");
    // TM-NLP-004: real GLiNER NER when model is available; heuristic fallback otherwise.
    if let Some(gli) = tm_ingest::GlinerExtractor::auto_download_default() {
        ingest = ingest.with_extractor(Box::new(gli));
    }
    let retrieval = RetrievalEngine::open(&db_path, &trace_path, false)
        .expect("failed to open retrieval engine");
    let trace_store = TraceStore::open(&trace_path)
        .expect("failed to open trace store");

    let ingest = Arc::new(Mutex::new(ingest));
    let trace_store = Arc::new(Mutex::new(trace_store));
    let capture_enabled = Arc::new(Mutex::new(true));

    // LM-8 — spawn the async triple worker before anything else can
    // race ingest. Capacity 64 is generous: even bulk ingest should
    // not produce sustained backpressure, and the bounded channel
    // means a stuck worker can never balloon RAM. The worker opens
    // its own GraphStore handle (SQLite WAL handles concurrency).
    // LM-6: Qwen LLM extractor on the slow path when weights are present.
    // Falls back to heuristic when the GGUF is missing so the worker
    // always runs. Drop the GGUF at ~/.tracemind/models/qwen2.5-1.5b-instruct-q4_k_m.gguf
    // to enable LLM-quality triples; NER stays deterministic on the hot path.
    // Track whether the LLM extractor was successfully loaded so the UI
    // can surface "LLM on" / "heuristic only" without re-probing the
    // filesystem on every call.
    let llm_active = std::sync::atomic::AtomicBool::new(false);
    let model_path = dir
        .join("models")
        .join("qwen2.5-1.5b-instruct-q4_k_m.gguf");
    if cfg!(feature = "local-llm") && model_path.exists() {
        llm_active.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let llm_active = Arc::new(llm_active);

    let triple_worker = Arc::new(TripleWorker::spawn_qwen_or_default(
        WorkerDb::Path(db_path.clone()),
        64,
        &dir,
    ));
    let cap_triple_worker = Arc::clone(&triple_worker);

    // Clone Arcs for capture thread before moving into AppState
    let cap_ingest = Arc::clone(&ingest);
    let cap_trace_store = Arc::clone(&trace_store);
    let cap_enabled = Arc::clone(&capture_enabled);

    // CTX-EVG Slice A — shared active-thread cell. The capture daemon
    // needs a clone so background clipboard/window captures land in
    // the same thread the user explicitly opened.
    let active_thread_id: Arc<Mutex<Option<Uuid>>> = Arc::new(Mutex::new(None));
    let cap_active_thread = Arc::clone(&active_thread_id);
    let cap_db_path = db_path.clone();

    let state = AppState {
        db_path: db_path.clone(),
        trace_path: trace_path.clone(),
        bandit_path,
        ingest,
        retrieval: Mutex::new(retrieval),
        trace_store,
        capture_enabled,
        triple_worker,
        llm_active,
        active_thread_id,
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            cmd_ingest,
            cmd_query,
            cmd_dashboard,
            cmd_traces,
            cmd_decay,
            cmd_graph,
            cmd_demo_ingest,
            cmd_entity_click,
            cmd_recommendations,
            cmd_reason_seed,
            cmd_query_recent,
            cmd_feedback,
            cmd_check_relevance,
            cmd_delete_entity,
            cmd_surprising,
            cmd_entity_trends,
            cmd_toggle_capture,
            cmd_capture_status,
            cmd_reason_chain,
            cmd_reason_explore,
            cmd_find_analogies,
            cmd_consolidate,
            cmd_brief,
            cmd_next_actions,
            cmd_accept_candidate,
            cmd_dismiss_candidate,
            cmd_llm_status,
            cmd_llm_download,
            cmd_triple_detail,
            cmd_resolve_contradiction,
            cmd_record_outcome,
            cmd_helpful,
            cmd_not_related,
            cmd_context_list,
            cmd_context_current,
            cmd_context_use,
            cmd_context_create,
            cmd_context_clear,
            cmd_export_context,
            cmd_entity_backlinks,
            cmd_resolve_wikilinks,
            cmd_views_list,
            cmd_views_show,
            cmd_views_create,
            cmd_views_add,
            cmd_views_remove,
            cmd_views_delete,
            cmd_daily_note_today,
            cmd_capture_permissions_list,
            cmd_capture_permissions_set,
            cmd_capture_forget_source,
            cmd_usage_stats,
            cmd_usage_share_payload,
            cmd_context_suggest,
            cmd_entity_drawer,
            cmd_resolve_transclusion,
            cmd_thread_view_save,
            cmd_thread_view_load,
            cmd_thread_view_clear,
            cmd_thread_views_list,
            cmd_memory_garden,
            cmd_outliers_list,
            cmd_outlier_triage,
            cmd_community_overlay,
            cmd_entity_context_dump,
            cmd_brain_snapshot,
            cmd_trace_why,
            cmd_inspector_layer,
            cmd_export_entity_markdown,
            // ─── Sprint GRAPH ─────────────────────────────────────
            sprint_commands::cmd_thread_start,
            sprint_commands::cmd_thread_end,
            sprint_commands::cmd_thread_active,
            sprint_commands::cmd_threads_list,
            sprint_commands::cmd_thread_materialize,
            sprint_commands::cmd_graph_compose,
            sprint_commands::cmd_compose_simple,
            sprint_commands::cmd_ontology_list,
            sprint_commands::cmd_ontology_assign,
            sprint_commands::cmd_ontology_create_object_type,
            sprint_commands::cmd_ontology_create_link_type,
            sprint_commands::cmd_event_graph,
            sprint_commands::cmd_event_record,
            sprint_commands::cmd_event_edge,
            sprint_commands::cmd_event_promote,
            sprint_commands::cmd_lgm_upsert_variable,
            sprint_commands::cmd_lgm_observe,
            sprint_commands::cmd_lgm_posterior,
            sprint_commands::cmd_lgm_list,
            sprint_commands::cmd_export_portable,
            sprint_commands::cmd_thread_attach_view,
            sprint_commands::cmd_thread_attached_view,
            sprint_commands::cmd_ontology_proposals,
            sprint_commands::cmd_ontology_accept_proposal,
            sprint_commands::cmd_ontology_reject_proposal,
            sprint_commands::cmd_ontology_run_proposer,
            sprint_commands::cmd_anticipate,
            // ─── CTX-EVG-C — Commitment Ledger ────────────────────
            sprint_commands::cmd_commitment_ledger,
            sprint_commands::cmd_commitment_resolve,
            sprint_commands::cmd_commitment_set_state,
            sprint_commands::cmd_commitment_propose_resolution,
            sprint_commands::cmd_commitment_create,
            cmd_storage_stats,
            cmd_storage_vacuum,
            cmd_storage_clean_ephemeral,
            cmd_storage_truncate_traces,
            // ─── WME-5 — verb-first working-memory cards ──────────
            wme_commands::cmd_wme_cards,
            wme_commands::cmd_wme_feedback,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            std::thread::spawn(move || {
                // Use shared pipeline + trace store (same instances as the IPC handlers)
                let pipeline = cap_ingest;
                let trace_store = cap_trace_store;
                let capture_flag = cap_enabled;
                // LM-8 — capture-loop ingests also feed the async
                // triple worker so background extraction runs over
                // clipboard/window/editor text too, not just user-typed
                // IPC ingests.
                let triple_worker = cap_triple_worker;
                // CTX-EVG Slice A — background captures should land in
                // the same `active_thread` the IPC ingest path uses, so
                // a user-opened thread accumulates *everything* it sees
                // (typed ingests + clipboard + window + editor) until
                // explicitly ended.
                let active_thread = cap_active_thread;
                let db_path_for_events = cap_db_path;
                // Helper: write a `Capture` event node bound to the
                // currently-active thread. Best-effort — never breaks
                // the capture loop if the DB is locked.
                let write_capture_event = |trace_id: uuid::Uuid, mean_conf: f64| {
                    let thread = active_thread.lock().ok().and_then(|g| *g);
                    if let Ok(g) = GraphStore::open(&db_path_for_events) {
                        let mut node = tm_graph::event_graph::EventNode::new(
                            tm_graph::event_graph::EventNodeKind::Capture,
                            trace_id.to_string(),
                        );
                        node.thread_id = thread;
                        node.salience = mean_conf;
                        let _ = tm_graph::event_graph::EventGraphStore::insert_node(
                            g.connection(),
                            &node,
                        );
                    }
                };

                let mut seen_hashes: HashSet<u64> = HashSet::new();
                let mut last_clip_hash: u64 = 0;
                let mut last_window: String = String::new();
                let mut window_dwell_count: u32 = 0;

                tracing::info!("[capture] thread started, waiting 3s for init...");
                std::thread::sleep(Duration::from_secs(3));
                tracing::info!("[capture] thread ready, polling every 2s");

                loop {
                    std::thread::sleep(Duration::from_secs(2));

                    // Check if capture is enabled
                    let enabled = capture_flag.lock().map(|f| *f).unwrap_or(false);
                    if !enabled {
                        continue;
                    }
                    tracing::debug!("[capture] tick cycle={}", window_dwell_count);

                    // --- Clipboard capture ---
                    let clip = get_clipboard();
                    tracing::debug!("[capture] clipboard: {:?}", clip.as_ref().map(|t| &t[..t.len().min(40)]));
                    if let Some(text) = clip {
                        let hash = seahash::hash(text.as_bytes());
                        if hash != last_clip_hash && !seen_hashes.contains(&hash) {
                            last_clip_hash = hash;
                            seen_hashes.insert(hash);
                            if seen_hashes.len() > 1000 {
                                seen_hashes.clear();
                                seen_hashes.insert(hash);
                            }

                            if let Ok(pl) = pipeline.lock() {
                                let session = Uuid::new_v4();
                                if let Ok(result) = pl.ingest(&text, session) {
                                    if let Ok(ts) = trace_store.lock() {
                                        let _ = ts.append(&result.trace);
                                    }
                                    // CTX-EVG Slice A — record Capture event
                                    let mean_conf = if result.entities.is_empty() {
                                        0.0
                                    } else {
                                        result.entities.iter().map(|e| e.confidence).sum::<f64>()
                                            / result.entities.len() as f64
                                    };
                                    write_capture_event(result.trace.id, mean_conf);
                                    // LM-8 — enqueue background triple extraction
                                    let _ = triple_worker.try_enqueue(TripleJob {
                                        text: text.clone(),
                                        entities: result.entities.clone(),
                                        session_id: session,
                                    });
                                    let _ = handle.emit("capture-event", CaptureEvent {
                                        source: "clipboard".to_string(),
                                        text: if text.len() > 80 { format!("{}...", &text[..80]) } else { text.clone() },
                                        entities_count: result.entities.len(),
                                        triples_count: result.triples.len(),
                                    });
                                    // LM-4 — also emit per-entity refresh so any
                                    // open entity drawer can self-update.
                                    let _ = handle.emit("entity-updated", EntityUpdatedEvent {
                                        entity_ids: result.entities.iter()
                                            .map(|e| e.id.to_string())
                                            .collect(),
                                        source: "clipboard".to_string(),
                                    });
                                }
                            }
                        }
                    }

                    // --- Active window + editor capture (every ~30s = 15 cycles of 2s) ---
                    window_dwell_count += 1;
                    if window_dwell_count >= 15 {
                        window_dwell_count = 0;

                        // Window title
                        if let Some(window) = get_active_window() {
                            if window != last_window && !window.contains("TraceMind") {
                                let hash = seahash::hash(window.as_bytes());
                                if !seen_hashes.contains(&hash) {
                                    seen_hashes.insert(hash);
                                    last_window = window.clone();

                                    if let Ok(pl) = pipeline.lock() {
                                        let context = format!("active context: {}", window);
                                        let session = Uuid::new_v4();
                                        if let Ok(result) = pl.ingest(&context, session) {
                                            if let Ok(ts) = trace_store.lock() {
                                                let _ = ts.append(&result.trace);
                                            }
                                            // CTX-EVG Slice A — record Capture event
                                            let mean_conf = if result.entities.is_empty() {
                                                0.0
                                            } else {
                                                result.entities.iter().map(|e| e.confidence).sum::<f64>()
                                                    / result.entities.len() as f64
                                            };
                                            write_capture_event(result.trace.id, mean_conf);
                                            // LM-8 — slow-path triple extraction off the capture loop.
                                            let _ = triple_worker.try_enqueue(TripleJob {
                                                text: context.clone(),
                                                entities: result.entities.clone(),
                                                session_id: session,
                                            });
                                            let _ = handle.emit("capture-event", CaptureEvent {
                                                source: "window".to_string(),
                                                text: window,
                                                entities_count: result.entities.len(),
                                                triples_count: result.triples.len(),
                                            });
                                        }
                                    }
                                }
                            }
                        }

                        // Cursor / VS Code editor context
                        if let Some(editor_ctx) = scrape_editor_context() {
                            let hash = seahash::hash(editor_ctx.as_bytes());
                            if !seen_hashes.contains(&hash) {
                                seen_hashes.insert(hash);
                                if let Ok(pl) = pipeline.lock() {
                                    let context = format!("editor context: {}", editor_ctx);
                                    let session = Uuid::new_v4();
                                    if let Ok(result) = pl.ingest(&context, session) {
                                        if let Ok(ts) = trace_store.lock() {
                                            let _ = ts.append(&result.trace);
                                        }
                                        // CTX-EVG Slice A — record Capture event
                                        let mean_conf = if result.entities.is_empty() {
                                            0.0
                                        } else {
                                            result.entities.iter().map(|e| e.confidence).sum::<f64>()
                                                / result.entities.len() as f64
                                        };
                                        write_capture_event(result.trace.id, mean_conf);
                                        // LM-8 — enqueue slow-path triple extraction.
                                        let _ = triple_worker.try_enqueue(TripleJob {
                                            text: context.clone(),
                                            entities: result.entities.clone(),
                                            session_id: session,
                                        });
                                        let _ = handle.emit("capture-event", CaptureEvent {
                                            source: "editor".to_string(),
                                            text: editor_ctx,
                                            entities_count: result.entities.len(),
                                            triples_count: result.triples.len(),
                                        });
                                    }
                                }
                            }
                        }

                        // ChatGPT / Claude conversation scraping
                        let conversations = scrape_ai_conversations();
                        tracing::info!("[capture] scraped {} AI conversations", conversations.len());
                        for (source, content) in conversations {
                            let hash = seahash::hash(content.as_bytes());
                            if !seen_hashes.contains(&hash) {
                                seen_hashes.insert(hash);
                                let prefixed = format!("{} conversation: {}", source, content);
                                let mut total_entities = 0;
                                let mut total_triples = 0;
                                // Chunk long content
                                let chunks: Vec<String> = if prefixed.len() > 2000 {
                                    prefixed.as_bytes()
                                        .chunks(2000)
                                        .map(|c| std::str::from_utf8(c).unwrap_or("").to_string())
                                        .collect()
                                } else {
                                    vec![prefixed]
                                };
                                // Lock/unlock per chunk to avoid starving IPC handlers
                                for chunk in &chunks {
                                    if chunk.len() < 20 { continue; }
                                    let session = Uuid::new_v4();
                                    if let Ok(pl) = pipeline.lock() {
                                        if let Ok(result) = pl.ingest(chunk, session) {
                                            if let Ok(ts) = trace_store.lock() {
                                                let _ = ts.append(&result.trace);
                                            }
                                            // LM-8 — fan out slow-path triple extraction per chunk.
                                            let _ = triple_worker.try_enqueue(TripleJob {
                                                text: chunk.clone(),
                                                entities: result.entities.clone(),
                                                session_id: session,
                                            });
                                            total_entities += result.entities.len();
                                            total_triples += result.triples.len();
                                        }
                                    } // mutex released here per chunk
                                }
                                if total_entities > 0 || total_triples > 0 {
                                    let _ = handle.emit("capture-event", CaptureEvent {
                                        source: source.to_lowercase(),
                                        text: if content.len() > 100 {
                                            format!("{}...", &content[..100])
                                        } else {
                                            content.clone()
                                        },
                                        entities_count: total_entities,
                                        triples_count: total_triples,
                                    });
                                }
                            }
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TraceMind");
}
