use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

use tm_episodic::TraceStore;
use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;

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
                "description": "Ingest text into TraceMind memory, extracting entities and triples.",
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
    traces: &Arc<Mutex<TraceStore>>,
    session_id: Uuid,
) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: text".to_string())?;

    let pipeline = ingest.lock().await;
    let result = pipeline
        .ingest(text, session_id)
        .map_err(|e| e.to_string())?;

    // Persist the trace from the pipeline result (already has raw_text + entity/triple IDs).
    let trace_store = traces.lock().await;
    let _ = trace_store.append(&result.trace);

    Ok(json!({
        "stored": true,
        "entities": result.entities.len(),
        "triples": result.triples.len()
    }))
}

async fn handle_memory_query(
    params: &Value,
    retrieval: &Arc<Mutex<RetrievalEngine>>,
) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: text".to_string())?;

    let mut engine = retrieval.lock().await;
    let result = engine.query(text).map_err(|e| e.to_string())?;

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

    Ok(json!({
        "entities": entities,
        "triples": triples,
        "arm": result.arm
    }))
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

fn handle_list_procedures() -> Value {
    json!({ "procedures": [] })
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
    session_id: Uuid,
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
                    handle_memory_store(&args, ingest, traces, session_id)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_query" => {
                    handle_memory_query(&args, retrieval)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "get_trace" => {
                    handle_get_trace(&args, traces)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "list_procedures" => handle_list_procedures(),
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
    // All diagnostics go to stderr so stdout stays clean for MCP wire traffic.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    // Resolve data directory and derive file paths.
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;

    let db_path = dir.join("memory.db").to_str().unwrap().to_string();
    let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();

    // Open stores. Use TM_HASH_EMBED=1 to skip model download.
    let hash_embed = std::env::var("TM_HASH_EMBED").map(|v| v == "1").unwrap_or(false);
    let ingest = Arc::new(Mutex::new(
        IngestPipeline::open(&db_path, hash_embed).map_err(|e| anyhow::anyhow!(e.to_string()))?,
    ));
    let retrieval = Arc::new(Mutex::new(
        RetrievalEngine::open(&db_path, &trace_path, hash_embed)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
    ));
    let traces = Arc::new(Mutex::new(
        TraceStore::open(&trace_path).map_err(|e| anyhow::anyhow!(e.to_string()))?,
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

        let response =
            handle_request(&method, &request, &ingest, &retrieval, &traces, session_id).await;

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
