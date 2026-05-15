//! Sprint GRAPH — Tauri commands for the unified graph layer.
//!
//! Wires the new tm-graph modules (algebra, event_graph, thread_graph,
//! ontology_types, portable_export) and tm-pgm into the desktop UI.
//!
//! These commands are *additive* — none of the existing P5 commands or
//! Obsidian-style surfaces (backlinks, wikilinks, GraphView, MemoryGarden,
//! markdown export) are touched.

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use tm_graph::{
    algebra::{Algebra, GraphExpr, SetOp},
    event_graph::{EventEdgeKind, EventGraphStore, EventNode, EventNodeKind},
    ontology_proposals::ProposalStore,
    ontology_types::OntologyStore,
    portable_export::{approx_token_count, export_portable, write_to_path, PortableGraph},
    thread_graph::{Thread, ThreadGraph, ThreadGraphStore, ThreadSource},
    GraphStore,
};
use tm_pgm::{anticipate, observe, posterior, AnticipateRow, Observation, PgmStore, Variable, VariableKind};

use crate::AppState;

fn open_graph(state: &AppState) -> std::result::Result<GraphStore, String> {
    GraphStore::open(&state.db_path).map_err(|e| e.to_string())
}

// ─── threads ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ThreadStartReq {
    pub title: String,
    pub source: String,
    pub context_id: Option<String>,
}

#[derive(Serialize)]
pub struct ThreadDto {
    pub id: String,
    pub title: String,
    pub source: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub context_id: Option<String>,
}

fn to_dto(t: &Thread) -> ThreadDto {
    ThreadDto {
        id: t.id.to_string(),
        title: t.title.clone(),
        source: t.source.as_str().into(),
        started_at: t.started_at.to_rfc3339(),
        ended_at: t.ended_at.map(|d| d.to_rfc3339()),
        context_id: t.context_id.map(|c| c.to_string()),
    }
}

#[tauri::command]
pub fn cmd_thread_start(
    state: State<'_, AppState>,
    req: ThreadStartReq,
) -> std::result::Result<ThreadDto, String> {
    let mut t = Thread::new(req.title, ThreadSource::parse(&req.source));
    t.context_id = req.context_id.and_then(|s| Uuid::parse_str(&s).ok());
    let g = open_graph(&state)?;
    ThreadGraphStore::upsert(g.connection(), &t).map_err(|e| e.to_string())?;
    Ok(to_dto(&t))
}

#[tauri::command]
pub fn cmd_thread_end(
    state: State<'_, AppState>,
    thread_id: String,
) -> std::result::Result<(), String> {
    let id = Uuid::parse_str(&thread_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    ThreadGraphStore::end_thread(g.connection(), id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_threads_list(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> std::result::Result<Vec<ThreadDto>, String> {
    let g = open_graph(&state)?;
    let xs = ThreadGraphStore::list(g.connection(), limit.unwrap_or(50))
        .map_err(|e| e.to_string())?;
    Ok(xs.iter().map(to_dto).collect())
}

#[derive(Serialize)]
pub struct ThreadGraphDto {
    pub thread_id: String,
    pub event_node_ids: Vec<String>,
    pub entity_ids: Vec<String>,
    pub topic_clusters: Vec<i64>,
    pub commitment_ids: Vec<String>,
    pub capture_signal_ids: Vec<i64>,
}

fn graph_to_dto(g: &ThreadGraph) -> ThreadGraphDto {
    ThreadGraphDto {
        thread_id: g.thread_id.to_string(),
        event_node_ids: g.event_node_ids.iter().map(|u| u.to_string()).collect(),
        entity_ids: g.entity_ids.iter().map(|u| u.to_string()).collect(),
        topic_clusters: g.topic_clusters.clone(),
        commitment_ids: g.commitment_ids.iter().map(|u| u.to_string()).collect(),
        capture_signal_ids: g.capture_signal_ids.clone(),
    }
}

#[tauri::command]
pub fn cmd_thread_materialize(
    state: State<'_, AppState>,
    thread_id: String,
) -> std::result::Result<ThreadGraphDto, String> {
    let id = Uuid::parse_str(&thread_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    let tg = ThreadGraphStore::materialize(g.connection(), id).map_err(|e| e.to_string())?;
    Ok(graph_to_dto(&tg))
}

// ─── compose ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn cmd_graph_compose(
    state: State<'_, AppState>,
    expression: GraphExpr,
) -> std::result::Result<ThreadGraphDto, String> {
    let g = open_graph(&state)?;
    let result = Algebra::eval(g.connection(), &expression).map_err(|e| e.to_string())?;
    Ok(graph_to_dto(&result))
}

#[derive(Deserialize)]
pub struct ComposeSimpleReq {
    pub op: String,
    pub left: String,
    pub right: String,
}

#[tauri::command]
pub fn cmd_compose_simple(
    state: State<'_, AppState>,
    req: ComposeSimpleReq,
) -> std::result::Result<ThreadGraphDto, String> {
    let l = Uuid::parse_str(&req.left).map_err(|e| e.to_string())?;
    let r = Uuid::parse_str(&req.right).map_err(|e| e.to_string())?;
    let op = match req.op.as_str() {
        "union" => SetOp::Union,
        "intersect" => SetOp::Intersect,
        "diff" => SetOp::Diff,
        other => return Err(format!("unknown op {other}")),
    };
    let expr = GraphExpr::SetOp {
        op,
        left: Box::new(GraphExpr::Thread { thread_id: l }),
        right: Box::new(GraphExpr::Thread { thread_id: r }),
    };
    cmd_graph_compose(state, expr)
}

// ─── ontology ────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct OntologyDto {
    pub object_types: Vec<String>,
    pub link_types: Vec<LinkTypeDto>,
}

#[derive(Serialize)]
pub struct LinkTypeDto {
    pub name: String,
    pub from: String,
    pub to: String,
}

#[tauri::command]
pub fn cmd_ontology_list(
    state: State<'_, AppState>,
) -> std::result::Result<OntologyDto, String> {
    let g = open_graph(&state)?;
    let conn = g.connection();
    let ots = OntologyStore::list_object_types(conn).map_err(|e| e.to_string())?;
    let lts = OntologyStore::list_link_types(conn).map_err(|e| e.to_string())?;
    let ot_by_id: std::collections::HashMap<Uuid, String> =
        ots.iter().map(|o| (o.id, o.name.clone())).collect();
    let link_types = lts
        .into_iter()
        .map(|l| LinkTypeDto {
            name: l.name,
            from: ot_by_id
                .get(&l.from_object_type_id)
                .cloned()
                .unwrap_or_else(|| "?".into()),
            to: ot_by_id
                .get(&l.to_object_type_id)
                .cloned()
                .unwrap_or_else(|| "?".into()),
        })
        .collect();
    Ok(OntologyDto {
        object_types: ots.into_iter().map(|o| o.name).collect(),
        link_types,
    })
}

#[derive(Deserialize)]
pub struct OntologyAssignReq {
    pub entity_id: String,
    pub object_type: String,
}

#[tauri::command]
pub fn cmd_ontology_assign(
    state: State<'_, AppState>,
    req: OntologyAssignReq,
) -> std::result::Result<(), String> {
    let id = Uuid::parse_str(&req.entity_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    OntologyStore::assign_object_type(g.connection(), id, &req.object_type)
        .map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct OntologyCreateTypeReq {
    pub name: String,
    pub parent: Option<String>,
}

#[tauri::command]
pub fn cmd_ontology_create_object_type(
    state: State<'_, AppState>,
    req: OntologyCreateTypeReq,
) -> std::result::Result<String, String> {
    let g = open_graph(&state)?;
    let id = OntologyStore::create_object_type(
        g.connection(),
        &req.name,
        req.parent.as_deref(),
        serde_json::json!({}),
    )
    .map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

#[derive(Deserialize)]
pub struct OntologyCreateLinkReq {
    pub name: String,
    pub from: String,
    pub to: String,
    pub cardinality: Option<String>,
}

#[tauri::command]
pub fn cmd_ontology_create_link_type(
    state: State<'_, AppState>,
    req: OntologyCreateLinkReq,
) -> std::result::Result<String, String> {
    let g = open_graph(&state)?;
    let id = OntologyStore::create_link_type(
        g.connection(),
        &req.name,
        &req.from,
        &req.to,
        req.cardinality.as_deref().unwrap_or("many_to_many"),
    )
    .map_err(|e| e.to_string())?;
    Ok(id.to_string())
}

// ─── event graph ─────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct EventNodeDto {
    pub id: String,
    pub kind: String,
    pub ts: i64,
    pub payload_ref: String,
    pub cluster_id: Option<i64>,
    pub thread_id: Option<String>,
    pub salience: f64,
}

#[derive(Serialize)]
pub struct EventEdgeDto {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub strength: f64,
    pub support_count: u32,
}

#[derive(Serialize)]
pub struct EventGraphDto {
    pub nodes: Vec<EventNodeDto>,
    pub edges: Vec<EventEdgeDto>,
}

#[tauri::command]
pub fn cmd_event_graph(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> std::result::Result<EventGraphDto, String> {
    let g = open_graph(&state)?;
    let nodes = EventGraphStore::list_nodes(g.connection(), limit.unwrap_or(500))
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|n| EventNodeDto {
            id: n.id.to_string(),
            kind: n.kind.as_str().into(),
            ts: n.ts,
            payload_ref: n.payload_ref,
            cluster_id: n.cluster_id,
            thread_id: n.thread_id.map(|x| x.to_string()),
            salience: n.salience,
        })
        .collect();
    let edges = EventGraphStore::visible_edges(g.connection(), None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| EventEdgeDto {
            from: e.from_id.to_string(),
            to: e.to_id.to_string(),
            kind: e.kind.as_str().into(),
            strength: e.strength,
            support_count: e.support_count,
        })
        .collect();
    Ok(EventGraphDto { nodes, edges })
}

#[derive(Deserialize)]
pub struct EventNodeRecordReq {
    pub kind: String,
    pub payload_ref: String,
    pub thread_id: Option<String>,
    pub context_id: Option<String>,
    pub cluster_id: Option<i64>,
    pub salience: Option<f64>,
}

#[tauri::command]
pub fn cmd_event_record(
    state: State<'_, AppState>,
    req: EventNodeRecordReq,
) -> std::result::Result<String, String> {
    let kind =
        EventNodeKind::parse(&req.kind).ok_or_else(|| format!("bad kind {}", req.kind))?;
    let mut n = EventNode::new(kind, req.payload_ref);
    n.thread_id = req.thread_id.and_then(|s| Uuid::parse_str(&s).ok());
    n.context_id = req.context_id.and_then(|s| Uuid::parse_str(&s).ok());
    n.cluster_id = req.cluster_id;
    if let Some(s) = req.salience {
        n.salience = s;
    }
    let g = open_graph(&state)?;
    EventGraphStore::insert_node(g.connection(), &n).map_err(|e| e.to_string())?;
    Ok(n.id.to_string())
}

#[derive(Deserialize)]
pub struct EventEdgeRecordReq {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub strength: f64,
}

#[tauri::command]
pub fn cmd_event_edge(
    state: State<'_, AppState>,
    req: EventEdgeRecordReq,
) -> std::result::Result<(), String> {
    let from = Uuid::parse_str(&req.from).map_err(|e| e.to_string())?;
    let to = Uuid::parse_str(&req.to).map_err(|e| e.to_string())?;
    let kind =
        EventEdgeKind::parse(&req.kind).ok_or_else(|| format!("bad kind {}", req.kind))?;
    let g = open_graph(&state)?;
    EventGraphStore::upsert_edge(g.connection(), from, to, kind, req.strength, None)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn cmd_event_promote(state: State<'_, AppState>) -> std::result::Result<usize, String> {
    let g = open_graph(&state)?;
    EventGraphStore::promote_sequence_edges(g.connection()).map_err(|e| e.to_string())
}

// ─── LGM ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LgmVariableReq {
    pub name: String,
    pub kind: String,
    pub domain: Vec<String>,
}

#[tauri::command]
pub fn cmd_lgm_upsert_variable(
    state: State<'_, AppState>,
    req: LgmVariableReq,
) -> std::result::Result<String, String> {
    let v = Variable::new(req.name, VariableKind::parse(&req.kind), req.domain);
    let g = open_graph(&state)?;
    PgmStore::upsert_variable(g.connection(), &v).map_err(|e| e.to_string())?;
    Ok(v.id.to_string())
}

#[derive(Deserialize)]
pub struct LgmObserveReq {
    pub assignments: std::collections::HashMap<String, String>,
}

#[tauri::command]
pub fn cmd_lgm_observe(
    state: State<'_, AppState>,
    req: LgmObserveReq,
) -> std::result::Result<(), String> {
    let mut o = Observation::default();
    o.values = req.assignments;
    let g = open_graph(&state)?;
    observe(g.connection(), &o).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct LgmQueryReq {
    pub target: String,
    pub evidence: std::collections::HashMap<String, String>,
}

#[derive(Serialize)]
pub struct LgmPosteriorDto {
    pub value: String,
    pub probability: f64,
}

#[tauri::command]
pub fn cmd_lgm_posterior(
    state: State<'_, AppState>,
    req: LgmQueryReq,
) -> std::result::Result<Vec<LgmPosteriorDto>, String> {
    let mut o = Observation::default();
    o.values = req.evidence;
    let g = open_graph(&state)?;
    let pairs = posterior(g.connection(), &req.target, &o).map_err(|e| e.to_string())?;
    Ok(pairs
        .into_iter()
        .map(|(value, probability)| LgmPosteriorDto { value, probability })
        .collect())
}

#[derive(Serialize)]
pub struct LgmVariableDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub domain: Vec<String>,
}

#[tauri::command]
pub fn cmd_lgm_list(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<LgmVariableDto>, String> {
    let g = open_graph(&state)?;
    let vs = PgmStore::list_variables(g.connection()).map_err(|e| e.to_string())?;
    Ok(vs
        .into_iter()
        .map(|v| LgmVariableDto {
            id: v.id.to_string(),
            name: v.name,
            kind: v.kind.as_str().into(),
            domain: v.domain,
        })
        .collect())
}

// ─── portable export ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PortableExportReq {
    pub expression: GraphExpr,
    pub write_to_disk: Option<bool>,
}

#[derive(Serialize)]
pub struct PortableExportResp {
    pub graph: PortableGraph,
    pub approx_tokens: usize,
    pub path: Option<String>,
}

#[tauri::command]
pub fn cmd_export_portable(
    state: State<'_, AppState>,
    req: PortableExportReq,
) -> std::result::Result<PortableExportResp, String> {
    let g = open_graph(&state)?;
    let conn = g.connection();
    let tg = Algebra::eval(conn, &req.expression).map_err(|e| e.to_string())?;
    let portable = export_portable(conn, &tg).map_err(|e| e.to_string())?;
    let approx = approx_token_count(&portable);

    let path = if req.write_to_disk.unwrap_or(false) {
        let data_dir = std::env::var("TM_DATA_DIR")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                dirs_like_home().map(|h| h.join(".tracemind"))
            })
            .unwrap_or_else(|| std::path::PathBuf::from(".tracemind"));
        let dir = data_dir.join("portable");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!(
            "graph-{}.json",
            chrono::Utc::now().timestamp_millis()
        ));
        write_to_path(&portable, &path).map_err(|e| e.to_string())?;
        Some(path.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(PortableExportResp {
        graph: portable,
        approx_tokens: approx,
        path,
    })
}

fn dirs_like_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

// ─── attach view (for MCP query scoping) ─────────────────────────────

#[derive(Deserialize)]
pub struct AttachViewReq {
    pub thread_id: String,
    pub view_id: String,
}

#[tauri::command]
pub fn cmd_thread_attach_view(
    state: State<'_, AppState>,
    req: AttachViewReq,
) -> std::result::Result<(), String> {
    let tid = Uuid::parse_str(&req.thread_id).map_err(|e| e.to_string())?;
    let vid = Uuid::parse_str(&req.view_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    ThreadGraphStore::attach_view(g.connection(), tid, vid).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_thread_attached_view(
    state: State<'_, AppState>,
    thread_id: String,
) -> std::result::Result<Option<String>, String> {
    let tid = Uuid::parse_str(&thread_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    Ok(ThreadGraphStore::attached_view(g.connection(), tid)
        .map_err(|e| e.to_string())?
        .map(|u| u.to_string()))
}

// ─── ONT-2 — proposal accept/reject ──────────────────────────────────

#[derive(Serialize)]
pub struct OntologyProposalDto {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub top_terms: Vec<String>,
    pub support_count: u32,
    pub created_at: String,
}

#[tauri::command]
pub fn cmd_ontology_proposals(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<OntologyProposalDto>, String> {
    let g = open_graph(&state)?;
    let rows = ProposalStore::list_pending(g.connection()).map_err(|e| e.to_string())?;
    let out = rows
        .into_iter()
        .map(|p| {
            let top_terms: Vec<String> = p
                .evidence
                .get("top_terms")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            OntologyProposalDto {
                id: p.id.to_string(),
                kind: format!("{:?}", p.kind).to_lowercase(),
                name: p.name,
                top_terms,
                support_count: p.support_count,
                created_at: p.created_at,
            }
        })
        .collect();
    Ok(out)
}

#[tauri::command]
pub fn cmd_ontology_accept_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
) -> std::result::Result<(), String> {
    let id = Uuid::parse_str(&proposal_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    ProposalStore::accept(g.connection(), id).map_err(|e| e.to_string())
}

/// ONT-2 — run the statistical Object Type proposer over the current
/// captured-signal clusters. Pulls up to 25 samples per cluster, runs
/// the c-TF-IDF labeler, and persists any new proposals as `pending`.
/// Returns the number of new proposals written. Safe to call on a tick
/// (background) or from a UI "Run proposer" button.
#[tauri::command]
pub fn cmd_ontology_run_proposer(
    state: State<'_, AppState>,
) -> std::result::Result<usize, String> {
    let g = open_graph(&state)?;
    let clusters = g.cluster_sample_map(25).map_err(|e| e.to_string())?;
    let proposals = tm_reflect::propose_object_types(
        &clusters,
        &tm_reflect::ontology_proposer::ProposerConfig::default(),
    );
    let written = tm_reflect::persist_object_type_proposals(g.connection(), &proposals)
        .map_err(|e| e.to_string())?;
    Ok(written)
}

#[tauri::command]
pub fn cmd_ontology_reject_proposal(
    state: State<'_, AppState>,
    proposal_id: String,
) -> std::result::Result<(), String> {
    let id = Uuid::parse_str(&proposal_id).map_err(|e| e.to_string())?;
    let g = open_graph(&state)?;
    ProposalStore::reject(g.connection(), id).map_err(|e| e.to_string())
}

// ─── LGM-2 — Anticipate verb card ───────────────────────────────────

#[derive(Deserialize)]
pub struct AnticipateReq {
    pub target: String,
    /// `variable_name -> value`
    #[serde(default)]
    pub evidence: std::collections::HashMap<String, String>,
    #[serde(default = "default_anticipate_top_k")]
    pub top_k: usize,
}

fn default_anticipate_top_k() -> usize {
    3
}

#[derive(Serialize)]
pub struct AnticipateRowDto {
    pub value: String,
    pub probability: f64,
    pub support: u32,
}

#[tauri::command]
pub fn cmd_anticipate(
    state: State<'_, AppState>,
    req: AnticipateReq,
) -> std::result::Result<Vec<AnticipateRowDto>, String> {
    let g = open_graph(&state)?;
    let mut obs = Observation::default();
    for (k, v) in req.evidence {
        obs.values.insert(k, v);
    }
    let rows: Vec<AnticipateRow> =
        anticipate(g.connection(), &req.target, &obs, req.top_k).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| AnticipateRowDto {
            value: r.value,
            probability: r.probability,
            support: r.support,
        })
        .collect())
}

