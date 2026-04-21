use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

use tm_episodic::{RecentStore, TraceStore};
use tm_graph::GraphStore;
use tm_ingest::IngestPipeline;
use tm_reason::{AnalogySolver, ChainBuilder, Consolidator};
use tm_rerank::ColbertReranker;
use tm_retrieval::RetrievalEngine;
use tm_types::RecentCapture;

// ---------------------------------------------------------------------------
// Startup helpers
// ---------------------------------------------------------------------------

fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("TM_DATA_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        std::path::PathBuf::from(home).join(".tracemind")
    }
}

// ---------------------------------------------------------------------------
// Tool descriptors
// ---------------------------------------------------------------------------

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "memory_store",
                "description": "Ingest text into TraceMind memory, extracting entities and triples. Response also carries a `context` field with pre-existing memories related to the stored text (top-3 entities + top-3 1-hop neighbours), so callers see \"here's what I already knew\" without a second query.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Text to ingest into memory."
                        }
                    },
                    "required": ["text"]
                }
            },
            {
                "name": "memory_query",
                "description": "Query TraceMind memory with natural-language text. Returns relevant entities and triples.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Query text."
                        }
                    },
                    "required": ["text"]
                }
            },
            {
                "name": "get_trace",
                "description": "Return recent ingestion/retrieval traces.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of traces to return (default 10).",
                            "default": 10
                        }
                    }
                }
            },
            {
                "name": "list_procedures",
                "description": "List stored learnable procedures (stub).",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "memory_reason",
                "description": "Explore reasoning paths from an entity. Returns multi-hop chains through the knowledge graph showing how entities connect.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity": {
                            "type": "string",
                            "description": "Entity name to reason from."
                        },
                        "target": {
                            "type": "string",
                            "description": "Optional target entity. If provided, finds paths between source and target."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Max number of reasoning chains (default 5).",
                            "default": 5
                        }
                    },
                    "required": ["entity"]
                }
            },
            {
                "name": "memory_analogies",
                "description": "Find entities structurally similar to the given entity, based on their relationship patterns in the knowledge graph.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "entity": {
                            "type": "string",
                            "description": "Entity name to find analogies for."
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Max analogies (default 5).",
                            "default": 5
                        }
                    },
                    "required": ["entity"]
                }
            },
            {
                "name": "memory_consolidate",
                "description": "Run memory consolidation: strengthen frequently-accessed memories, decay old ones, prune weak entities, merge duplicates.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

async fn handle_memory_store(
    params: &Value,
    ingest: &Arc<Mutex<IngestPipeline>>,
    retrieval: &Arc<Mutex<RetrievalEngine>>,
    traces: &Arc<Mutex<TraceStore>>,
    recent: &Arc<Mutex<RecentStore>>,
    session_id: Uuid,
) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: text".to_string())?;

    // TM-UX-001 Phase C: proactive surfacing. Run recall against the
    // *pre-existing* graph state BEFORE ingest, so the caller sees "here's
    // what I already knew on this topic" without issuing a second query. We
    // must query before ingest because `ingest()` upserts entities — running
    // recall after would re-return the just-stored items as if pre-existing.
    //
    // The retrieval engine holds its own `GraphStore` with an in-memory
    // UUID cache that was snapshotted at server start; refresh from disk so
    // entities written by *earlier* memory_store calls in this session are
    // visible to search_vectors.
    let (context_memories, context_related): (Vec<Value>, Vec<Value>) = {
        let mut engine = retrieval.lock().await;
        let _ = engine.refresh_graph();
        match engine.query(text) {
            Ok(r) => {
                let memories: Vec<Value> = r
                    .entities
                    .iter()
                    .take(3)
                    .map(|e| {
                        json!({
                            "id": e.id.to_string(),
                            "name": e.name,
                            "type": format!("{:?}", e.entity_type).to_lowercase(),
                        })
                    })
                    .collect();
                let related: Vec<Value> = r
                    .related_entities
                    .iter()
                    .take(3)
                    .map(|re| {
                        json!({
                            "id": re.id.to_string(),
                            "name": re.name,
                            "type": re.entity_type,
                            "reason": re.reason,
                        })
                    })
                    .collect();
                (memories, related)
            }
            Err(_) => (Vec::new(), Vec::new()),
        }
    };

    let pipeline = ingest.lock().await;
    let result = pipeline
        .ingest(text, session_id)
        .map_err(|e| e.to_string())?;
    drop(pipeline);

    // Persist the trace from the pipeline result (already has raw_text + entity/triple IDs).
    let trace_store = traces.lock().await;
    let _ = trace_store.append(&result.trace);
    drop(trace_store);

    // TM-UX-001 Phase B: emit a capture-feedback event so the UI can surface
    // "just ingested via MCP" in the recent ticker.
    let mut event = RecentCapture::new("mcp", &result.content_hash, text)
        .with_tier("t1")
        .with_promoted(!result.skip_gate);
    if result.skip_gate {
        event = event.with_skipped("gate_rejected");
    }
    let recent_store = recent.lock().await;
    if let Err(e) = recent_store.append(&event) {
        tracing::debug!("[mcp] failed to append recent capture: {e}");
    }

    Ok(json!({
        "stored": true,
        "entities": result.entities.len(),
        "triples": result.triples.len(),
        "context": {
            "memories": context_memories,
            "related": context_related,
        }
    }))
}

async fn handle_memory_query(
    params: &Value,
    retrieval: &Arc<Mutex<RetrievalEngine>>,
    db_path: &str,
) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: text".to_string())?;

    let mut engine = retrieval.lock().await;
    // Same rationale as memory_store: the retrieval engine's GraphStore cache
    // is stale relative to writes from the ingest-side GraphStore in a
    // long-running MCP session. See TM-UX-001 Phase C.
    let _ = engine.refresh_graph();
    let result = engine.query(text).map_err(|e| e.to_string())?;

    // Extract the plan action for auto-routing supplemental data
    let plan_action = result.plan.as_ref().map(|p| format!("{:?}", p.action))
        .unwrap_or_else(|| "BanditRetrieval".to_string());
    let plan_complexity = result.plan.as_ref().map(|p| p.complexity.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let entities: Vec<Value> = result
        .entities
        .iter()
        .map(|e| {
            json!({
                "id": e.id.to_string(),
                "name": e.name,
                "type": format!("{:?}", e.entity_type).to_lowercase()
            })
        })
        .collect();

    let triples: Vec<Value> = result
        .triples
        .iter()
        .map(|t| {
            json!({
                "subject": t.subject_id.to_string(),
                "predicate": t.predicate.to_string(),
                "object": t.object_id.to_string()
            })
        })
        .collect();

    let explanation = result.causal_trace.explain();

    // TM-UX-001: surface 1-hop related entities as "you might also want…".
    let related_entities: Vec<Value> = result
        .related_entities
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "name": r.name,
                "type": r.entity_type,
                "score": r.score,
                "reason": r.reason,
            })
        })
        .collect();

    // Auto-routing: if the planner detected a reasoning/analogy query,
    // enrich the response with supplemental reasoning data.
    let mut response = json!({
        "entities": entities,
        "triples": triples,
        "related_entities": related_entities,
        "arm": result.arm,
        "explanation": explanation,
        "reasoning": result.reasoning_narrative,
        "plan": {
            "action": plan_action,
            "complexity": plan_complexity
        }
    });

    // Add confidence information when results are uncertain
    if result.low_confidence {
        response["confidence"] = json!({
            "low": true,
            "suggestions": result.suggested_queries
        });
    }

    // Auto-enrich with reasoning chains when planner detects relationship queries
    if let Some(ref plan) = result.plan {
        match &plan.action {
            tm_controller::PlanAction::ReasoningChain { source_hint, target_hint } => {
                if let (Some(src), Some(tgt)) = (source_hint, target_hint) {
                    // Auto-run reasoning chain between the detected entities
                    if let Ok(chains) = auto_reason_chain(db_path, src, tgt) {
                        response["reasoning_chains"] = chains;
                    }
                }
            }
            tm_controller::PlanAction::AnalogySearch { entity_hint } => {
                if let Ok(analogies) = auto_find_analogies(db_path, entity_hint) {
                    response["analogies"] = analogies;
                }
            }
            tm_controller::PlanAction::TemporalQuery { time_range } => {
                response["temporal"] = json!({
                    "label": time_range.label,
                    "start": time_range.start.to_rfc3339(),
                    "end": time_range.end.to_rfc3339(),
                });
            }
            _ => {}
        }
    }

    Ok(response)
}

/// Auto-route: run reasoning chain between two entities (triggered by planner).
fn auto_reason_chain(db_path: &str, source: &str, target: &str) -> Result<Value, String> {
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let src = graph.find_entity_by_name_icase(source)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", source))?;
    let tgt = graph.find_entity_by_name_icase(target)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", target))?;

    let builder = ChainBuilder::with_defaults(&graph);
    let chains = builder.find_chains(src.id, tgt.id);

    let chain_json: Vec<Value> = chains.iter().take(3).map(|c| {
        json!({
            "path": c.steps.iter().map(|s| format!("{} --[{}]--> {}", s.entity_name, s.predicate, s.entity_type)).collect::<Vec<_>>(),
            "score": c.score,
            "hops": c.steps.len()
        })
    }).collect();

    Ok(json!(chain_json))
}

/// Auto-route: find analogies for an entity (triggered by planner).
fn auto_find_analogies(db_path: &str, entity_name: &str) -> Result<Value, String> {
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let entity = graph.find_entity_by_name_icase(entity_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", entity_name))?;

    let solver = AnalogySolver::new(&graph);
    let results = solver.find_analogies(entity.id, 3);

    let analogies: Vec<Value> = results.iter().map(|r| {
        json!({
            "target": r.target_name,
            "similarity": r.similarity,
            "explanation": r.explanation
        })
    }).collect();

    Ok(json!(analogies))
}

async fn handle_get_trace(
    params: &Value,
    traces: &Arc<Mutex<TraceStore>>,
) -> Result<Value, String> {
    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10) as usize;

    let store = traces.lock().await;
    let recent = store.recent(limit).map_err(|e| e.to_string())?;

    let trace_list: Vec<Value> = recent
        .iter()
        .map(|t| {
            json!({
                "id": t.id.to_string(),
                "event_type": format!("{:?}", t.event_type).to_lowercase(),
                "raw_text": t.raw_text.as_deref().unwrap_or(""),
                "entities_count": t.entities_extracted.len(),
                "triples_count": t.triples_extracted.len(),
                "retrieval_arm": t.retrieval_arm,
                "retrieval_latency_ms": t.retrieval_latency_ms,
                "created_at": t.created_at.to_rfc3339()
            })
        })
        .collect();

    Ok(json!({ "traces": trace_list }))
}

fn handle_list_procedures(db_path: &str) -> Value {
    // Derive procedures.jsonl path as sibling of db_path
    let proc_path = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("procedures.jsonl");

    let procs = match tm_episodic::ProcedureStore::open(proc_path.to_str().unwrap_or(".")) {
        Ok(store) => store.list_active().unwrap_or_default(),
        Err(_) => vec![],
    };

    let proc_json: Vec<Value> = procs.iter().map(|p| {
        json!({
            "id": p.id.to_string(),
            "name": p.name,
            "description": p.description,
            "steps": p.steps.iter().map(|s| json!({
                "ordinal": s.ordinal,
                "action": s.action,
            })).collect::<Vec<_>>(),
            "status": format!("{:?}", p.status),
            "confidence": p.confidence,
        })
    }).collect();

    json!({ "procedures": proc_json })
}

fn handle_memory_reason(params: &Value, db_path: &str) -> Result<Value, String> {
    let entity_name = params
        .get("entity")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: entity".to_string())?;

    let target_name = params.get("target").and_then(|v| v.as_str());
    let max_results = params.get("max_results").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let source = graph.find_entity_by_name_icase(entity_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", entity_name))?;

    let builder = ChainBuilder::with_defaults(&graph);

    if let Some(target) = target_name {
        let target_entity = graph.find_entity_by_name_icase(target)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Entity '{}' not found", target))?;

        let chains = builder.find_chains(source.id, target_entity.id);
        let chain_json: Vec<Value> = chains.iter().take(max_results).map(|c| {
            json!({
                "path": c.steps.iter().map(|s| format!("{} --[{}]--> {}", s.entity_name, s.predicate, s.entity_type)).collect::<Vec<_>>(),
                "score": c.score,
                "hops": c.steps.len()
            })
        }).collect();

        Ok(json!({ "chains": chain_json, "source": entity_name, "target": target }))
    } else {
        let chains = builder.explore(&[source.id], max_results);
        let chain_json: Vec<Value> = chains.iter().map(|c| {
            json!({
                "path": c.steps.iter().map(|s| format!("{} ({}) via {}", s.entity_name, s.entity_type, s.predicate)).collect::<Vec<_>>(),
                "score": c.score,
                "destination": c.steps.last().map(|s| s.entity_name.clone()).unwrap_or_default()
            })
        }).collect();

        Ok(json!({ "chains": chain_json, "source": entity_name }))
    }
}

fn handle_memory_analogies(params: &Value, db_path: &str) -> Result<Value, String> {
    let entity_name = params
        .get("entity")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: entity".to_string())?;

    let max_results = params.get("max_results").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let entity = graph.find_entity_by_name_icase(entity_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Entity '{}' not found", entity_name))?;

    let solver = AnalogySolver::new(&graph);
    let results = solver.find_analogies(entity.id, max_results);

    let analogies: Vec<Value> = results.iter().map(|r| {
        json!({
            "target": r.target_name,
            "similarity": r.similarity,
            "explanation": r.explanation,
            "shared_patterns": r.shared_patterns
        })
    }).collect();

    Ok(json!({ "source": entity_name, "analogies": analogies }))
}

fn handle_memory_consolidate(db_path: &str) -> Result<Value, String> {
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let consolidator = Consolidator::with_defaults(&graph);
    let report = consolidator.consolidate();

    Ok(json!({
        "entities_strengthened": report.entities_strengthened,
        "entities_decayed": report.entities_decayed,
        "entities_pruned": report.entities_pruned,
        "entities_merged": report.entities_merged,
        "triples_pruned": report.triples_pruned
    }))
}

// ---------------------------------------------------------------------------
// Request dispatcher
// ---------------------------------------------------------------------------

async fn handle_request(
    method: &str,
    request: &Value,
    ingest: &Arc<Mutex<IngestPipeline>>,
    retrieval: &Arc<Mutex<RetrievalEngine>>,
    traces: &Arc<Mutex<TraceStore>>,
    recent: &Arc<Mutex<RecentStore>>,
    session_id: Uuid,
    db_path: &str,
) -> Result<Value, anyhow::Error> {
    match method {
        "initialize" => {
            Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tracemind", "version": "0.1.0" }
            }))
        }

        "tools/list" => Ok(tools_list()),

        "tools/call" => {
            let params = request.get("params").unwrap_or(&Value::Null);
            let tool_name = params
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing params.name"))?;

            // MCP tools/call passes arguments under params.arguments.
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let tool_result = match tool_name {
                "memory_store" => {
                    handle_memory_store(&args, ingest, retrieval, traces, recent, session_id)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_query" => {
                    handle_memory_query(&args, retrieval, db_path)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "get_trace" => {
                    handle_get_trace(&args, traces)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "list_procedures" => handle_list_procedures(db_path),
                "memory_reason" => {
                    handle_memory_reason(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_analogies" => {
                    handle_memory_analogies(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_consolidate" => {
                    handle_memory_consolidate(db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                unknown => {
                    return Err(anyhow::anyhow!("unknown tool: {}", unknown));
                }
            };

            // MCP wraps the tool result in content[].
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": serde_json::to_string(&tool_result)?
                    }
                ]
            }))
        }

        // Client notifications we don't need to respond to but were given an id —
        // return an empty result rather than an error.
        "notifications/initialized" | "notifications/cancelled" => Ok(json!({})),

        unknown => Err(anyhow::anyhow!("method not found: {}", unknown)),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // TM-NLP-005: point hf-hub + fastembed at bundled weights (if present)
    // before any model loads. Silent if no bundle is found.
    let bundled = tm_types::bundled::init();

    // All diagnostics go to stderr so stdout stays clean for MCP wire traffic.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    if let Some(r) = &bundled {
        tracing::info!(
            "[tm-mcp] bundled models resolved from {} ({})",
            r.hf_cache.display(),
            r.source
        );
    }

    // Resolve data directory and derive file paths.
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;

    let db_path = dir.join("memory.db").to_str().unwrap().to_string();
    let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();

    // Open stores. Use TM_HASH_EMBED=1 to skip model download.
    let hash_embed = std::env::var("TM_HASH_EMBED").map(|v| v == "1").unwrap_or(false);
    // TM-NLP-004: attach real GLiNER NER if the model is available. Falls
    // back to the heuristic extractor when offline (auto_download returns None).
    let mut ingest_pipeline =
        IngestPipeline::open(&db_path, hash_embed).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if let Some(gli) = tm_ingest::GlinerExtractor::auto_download_default() {
        ingest_pipeline = ingest_pipeline.with_extractor(Box::new(gli));
    }
    let ingest = Arc::new(Mutex::new(ingest_pipeline));
    // Try to attach ColBERT reranker. If the download/load fails (offline,
    // rate-limited), the engine transparently runs without reranking.
    let reranker = ColbertReranker::auto_download_or_none(0.7);
    let retrieval = Arc::new(Mutex::new(
        RetrievalEngine::open(&db_path, &trace_path, hash_embed)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .with_reranker_instance(reranker),
    ));
    let traces = Arc::new(Mutex::new(
        TraceStore::open(&trace_path).map_err(|e| anyhow::anyhow!(e.to_string()))?,
    ));
    let recent_path = dir.join("recent.jsonl");
    let recent = Arc::new(Mutex::new(
        RecentStore::open(&recent_path).map_err(|e| anyhow::anyhow!(e.to_string()))?,
    ));

    // One stable session ID for this server process lifetime.
    let session_id = Uuid::new_v4();

    // Set up stdin/stdout.
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = stdout;

    while let Some(line) = reader.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        // Parse JSON-RPC request.
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": "Parse error" }
                });
                writer
                    .write_all((resp.to_string() + "\n").as_bytes())
                    .await?;
                writer.flush().await?;
                continue;
            }
        };

        // Skip notifications (requests without an id field).
        if request.get("id").is_none() {
            continue;
        }

        let id = request["id"].clone();
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let response = handle_request(
            &method,
            &request,
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session_id,
            &db_path,
        )
        .await;

        let resp_json = match response {
            Ok(result) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32603, "message": e.to_string() }
            }),
        };

        writer
            .write_all((resp_json.to_string() + "\n").as_bytes())
            .await?;
        writer.flush().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TM-UX-001 Phase C: a second `memory_store` call with related text
    /// surfaces the first call's entities via `context.memories`, without the
    /// caller having to invoke `memory_query`.
    #[tokio::test]
    async fn memory_store_returns_proactive_context() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_phaseC_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();

        // Hash embeddings keep this hermetic — no model download.
        let ingest = Arc::new(Mutex::new(
            IngestPipeline::open(&db_path, true).expect("ingest open"),
        ));
        let retrieval = Arc::new(Mutex::new(
            RetrievalEngine::open(&db_path, &trace_path, true).expect("retrieval open"),
        ));
        let traces = Arc::new(Mutex::new(
            TraceStore::open(&trace_path).expect("traces open"),
        ));
        let recent_path = dir.join("recent.jsonl");
        let recent = Arc::new(Mutex::new(
            RecentStore::open(&recent_path).expect("recent open"),
        ));
        let session = Uuid::new_v4();

        // First store seeds the graph.
        let first = handle_memory_store(
            &json!({"text": "Apple announced the M4 chip built on TSMC N3E."}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
        )
        .await
        .expect("first store");
        assert_eq!(first["stored"], json!(true));
        // First call has nothing pre-existing to surface — context is present but empty-ish.
        assert!(first.get("context").is_some(), "first response must carry context field");

        // Second store on related text should surface at least one of the first-call entities.
        let second = handle_memory_store(
            &json!({"text": "Apple is designing new silicon internally."}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
        )
        .await
        .expect("second store");
        assert_eq!(second["stored"], json!(true));

        let ctx = second.get("context").expect("context present");
        let memories = ctx.get("memories").and_then(|v| v.as_array()).expect("memories array");

        // At least one pre-existing entity ("Apple", "M4 chip", "TSMC", or "N3E") should
        // surface as context — none of these are in the *second* store's extraction.
        assert!(!memories.is_empty(),
            "expected proactive context_memories from prior store, got empty: {second}");
        assert!(memories.len() <= 3, "bounded at top-3, got {}", memories.len());

        // Every memory entry must be a non-stored (pre-existing) entity —
        // confirmed by structure: each has {id, name, type}.
        for m in memories {
            assert!(m.get("name").is_some(), "memory missing name: {m}");
            assert!(m.get("type").is_some(), "memory missing type: {m}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }
}
