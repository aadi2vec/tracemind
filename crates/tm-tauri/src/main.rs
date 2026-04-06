// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;
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
    ingest: Mutex<IngestPipeline>,
    retrieval: Mutex<RetrievalEngine>,
    trace_store: Mutex<TraceStore>,
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

#[derive(Serialize)]
struct QueryResponse {
    arm: u8,
    arm_name: String,
    latency_ms: u32,
    entities: Vec<EntityInfo>,
    triples: Vec<TripleInfo>,
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
}

#[derive(Serialize)]
struct DemoResult {
    texts_ingested: usize,
    total_entities: usize,
    total_triples: usize,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const ARM_NAMES: [&str; 4] = ["vector-only", "graph-heavy", "hybrid", "episodic"];

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

    Ok(QueryResponse {
        arm: result.arm,
        arm_name: arm_name(result.arm),
        latency_ms: result.latency_ms,
        entities,
        triples,
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
        // Parse the entity_type JSON string back to display format
        let etype = serde_json::from_str::<tm_types::EntityType>(&ent.entity_type)
            .map(|t| format!("{}", t))
            .unwrap_or_else(|_| ent.entity_type.clone());

        uuid_to_name.insert(uuid_str.clone(), ent.name.clone());

        nodes.push(GraphNode {
            id: uuid_str,
            name: ent.name.clone(),
            entity_type: etype,
            confidence,
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

    Ok(GraphData { nodes, edges })
}

/// Batch-ingest curated demo data to populate the knowledge graph.
#[tauri::command]
fn cmd_demo_ingest(state: State<AppState>) -> Result<DemoResult, String> {
    let demo_texts = [
        "Aaditya Srivathsan is the founder of TraceMind, a local-only memory OS for AI agents built entirely in Rust.",
        "TraceMind uses Rust for its core data plane because of memory safety, zero-cost abstractions, and tiny binary sizes.",
        "The TraceMind desktop app is built with Tauri, which provides native performance without Electron overhead.",
        "TraceMind stores everything in a single SQLite file using the sqlite-knowledge-graph crate for entities, relations, and vector embeddings.",
        "Claude Code integrates with TraceMind through an MCP server that exposes memory_store and memory_query tools over JSON-RPC.",
        "The retrieval engine uses a UCB1 bandit algorithm to select between four strategies: vector-only, graph-heavy, hybrid, and episodic.",
        "Aaditya works at the intersection of systems programming and machine learning, with deep expertise in Python, Rust, and PyTorch.",
        "TraceMind's NER pipeline extracts entities like Person, Organization, Technology, and Concept using a two-pass heuristic approach.",
        "The capture daemon monitors clipboard changes and shell history, automatically ingesting relevant content into TraceMind memory.",
        "Privacy is a core design tenet of TraceMind: no API calls, no telemetry, no cloud storage. Everything runs locally on the user's machine.",
        "The knowledge graph supports PageRank for entity importance scoring and Louvain community detection for topic clustering.",
        "TraceMind's procedural memory stores versioned how-to sequences with a lifecycle FSM: Active, Reinforced, Degraded, Deprecated.",
        "React and Tailwind CSS power the TraceMind frontend, with a dark theme featuring purple accents and real-time dashboard updates.",
        "Anthropic's Claude is the primary AI assistant that benefits from TraceMind's persistent memory across conversation sessions.",
        "The embedding model is all-MiniLM-L6-v2 running via ONNX on CPU, producing 384-dimensional vectors for semantic search.",
        "Kubernetes and Docker are planned for Phase 4 enterprise deployment, enabling multi-user governance with ACL and audit logs.",
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

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
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

    let state = AppState {
        db_path,
        trace_path,
        bandit_path,
        ingest: Mutex::new(ingest),
        retrieval: Mutex::new(retrieval),
        trace_store: Mutex::new(trace_store),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running TraceMind");
}
