// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, State};
use uuid::Uuid;

use tm_controller::UcbBandit;
use tm_episodic::TraceStore;
use tm_graph::GraphStore;
use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;

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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ARM_NAMES: [&str; 5] = ["vector-only", "graph-heavy", "hybrid", "episodic", "colbert"];

fn arm_name(arm: u8) -> String {
    ARM_NAMES.get(arm as usize).unwrap_or(&"unknown").to_string()
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

    // Build name lookup
    let name_of: HashMap<Uuid, &str> = result.entities.iter()
        .map(|e| (e.id, e.name.as_str())).collect();

    let entities: Vec<EntityInfo> = result.entities.iter().map(|e| EntityInfo {
        id: e.id.to_string(),
        name: e.name.clone(),
        entity_type: format!("{}", e.entity_type),
        confidence: e.confidence,
    }).collect();

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
    let mut engine = state.retrieval.lock().map_err(|e| e.to_string())?;
    let result = engine.query(&text).map_err(|e| e.to_string())?;

    let name_of: HashMap<Uuid, &str> = result.entities.iter()
        .map(|e| (e.id, e.name.as_str())).collect();

    let entities: Vec<EntityInfo> = result.entities.iter().map(|e| EntityInfo {
        id: e.id.to_string(),
        name: e.name.clone(),
        entity_type: format!("{}", e.entity_type),
        confidence: e.confidence,
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

    // Generate recommendations from the query context
    let recs = engine.recommendations(5);
    let recommendations: Vec<RecommendationInfo> = recs.into_iter().map(|r| RecommendationInfo {
        entity_id: r.entity_id,
        entity_name: r.entity_name,
        entity_type: r.entity_type,
        score: r.score,
        reason: r.reason,
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
        created_at: t.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
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
        created_at: t.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
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

    Ok(GraphData { nodes, edges, community_count })
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
    Ok(recs.into_iter().map(|r| RecommendationInfo {
        entity_id: r.entity_id,
        entity_name: r.entity_name,
        entity_type: r.entity_type,
        score: r.score,
        reason: r.reason,
    }).collect())
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

/// Get "most surprising things" — recently ingested entities with high novelty.
/// These are entities that are new, not well-connected yet, and recently captured.
#[tauri::command]
fn cmd_surprising(limit: Option<usize>, state: State<AppState>) -> Result<Vec<SurprisingEntity>, String> {
    let graph = GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let entities = graph.list_all_entities().map_err(|e| e.to_string())?;

    let limit = limit.unwrap_or(5);

    // Batch-compute novelty and recency for all entities
    let entity_ids: Vec<Uuid> = entities.iter().map(|e| e.id).collect();
    let recency_map = graph.batch_recency_scores(&entity_ids);
    let novelty_map = graph.batch_novelty_scores(&entity_ids);

    let mut scored: Vec<SurprisingEntity> = entities.iter().map(|e| {
        let recency = recency_map.get(&e.id).copied().unwrap_or(0.0);
        let novelty = novelty_map.get(&e.id).copied().unwrap_or(1.0);
        // Surprising = high novelty (rarely seen) + high recency (just arrived)
        let score = 0.6 * novelty + 0.4 * recency;
        SurprisingEntity {
            entity_id: e.id.to_string(),
            entity_name: e.name.clone(),
            entity_type: format!("{}", e.entity_type),
            novelty,
            recency,
            score,
        }
    }).collect();

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

#[derive(Serialize)]
struct ConsolidationResult {
    entities_strengthened: usize,
    entities_decayed: usize,
    entities_pruned: usize,
    entities_merged: usize,
    triples_pruned: usize,
}

#[tauri::command]
fn cmd_consolidate(state: State<AppState>) -> Result<ConsolidationResult, String> {
    let graph = tm_graph::GraphStore::open(&state.db_path).map_err(|e| e.to_string())?;
    let consolidator = tm_reason::Consolidator::with_defaults(&graph);
    let report = consolidator.consolidate();

    Ok(ConsolidationResult {
        entities_strengthened: report.entities_strengthened,
        entities_decayed: report.entities_decayed,
        entities_pruned: report.entities_pruned,
        entities_merged: report.entities_merged,
        triples_pruned: report.triples_pruned,
    })
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
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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

    let ingest = IngestPipeline::open(&db_path, false)
        .expect("failed to open ingest pipeline");
    let retrieval = RetrievalEngine::open(&db_path, &trace_path, false)
        .expect("failed to open retrieval engine");
    let trace_store = TraceStore::open(&trace_path)
        .expect("failed to open trace store");

    let ingest = Arc::new(Mutex::new(ingest));
    let trace_store = Arc::new(Mutex::new(trace_store));
    let capture_enabled = Arc::new(Mutex::new(true));

    // Clone Arcs for capture thread before moving into AppState
    let cap_ingest = Arc::clone(&ingest);
    let cap_trace_store = Arc::clone(&trace_store);
    let cap_enabled = Arc::clone(&capture_enabled);

    let state = AppState {
        db_path: db_path.clone(),
        trace_path: trace_path.clone(),
        bandit_path,
        ingest,
        retrieval: Mutex::new(retrieval),
        trace_store,
        capture_enabled,
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
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            std::thread::spawn(move || {
                // Use shared pipeline + trace store (same instances as the IPC handlers)
                let pipeline = cap_ingest;
                let trace_store = cap_trace_store;
                let capture_flag = cap_enabled;

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
                                    let _ = handle.emit("capture-event", CaptureEvent {
                                        source: "clipboard".to_string(),
                                        text: if text.len() > 80 { format!("{}...", &text[..80]) } else { text.clone() },
                                        entities_count: result.entities.len(),
                                        triples_count: result.triples.len(),
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
