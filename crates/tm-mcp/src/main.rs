use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use uuid::Uuid;

use tm_answer::TieredAnswerer;
use tm_episodic::{RecentStore, TraceStore};
use tm_graph::GraphStore;
use tm_ingest::IngestPipeline;
use tm_reason::{AnalogySolver, ChainBuilder, Consolidator};
use tm_rerank::ColbertReranker;
use tm_retrieval::RetrievalEngine;
use tm_types::RecentCapture;

mod answerer;
mod prefetch_orchestrator;

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
                "name": "memory_store_structured",
                "description": "Ingest pre-extracted entities and triples directly into TraceMind memory. Bypasses the heuristic NER — use this when the caller (typically an LLM) has already done extraction. Optional `text` is stored as the trace's raw text for provenance. Predicates use snake_case (related_to, is_a, part_of, has_property, works_at, collaborates_with, owns, depends_on, produces, references, has_procedure) or any custom string.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Optional source text (kept for provenance / trace audit)."
                        },
                        "entities": {
                            "type": "array",
                            "description": "Pre-extracted entities. Names are matched case-insensitively against the existing graph; duplicates merge.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "type": {
                                        "type": "string",
                                        "description": "One of: person, organization, project, file, url, concept, technology, decision, event. Anything else is stored as Custom."
                                    },
                                    "confidence": {
                                        "type": "number",
                                        "description": "0.0–1.0, defaults to 0.9 when omitted."
                                    }
                                },
                                "required": ["name", "type"]
                            }
                        },
                        "triples": {
                            "type": "array",
                            "description": "Pre-extracted triples. Subject/object names must appear in this call's `entities` array or already exist in the graph.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "subject": { "type": "string" },
                                    "predicate": { "type": "string" },
                                    "object": { "type": "string" },
                                    "confidence": { "type": "number" }
                                },
                                "required": ["subject", "predicate", "object"]
                            }
                        }
                    }
                }
            },
            {
                "name": "memory_commit",
                "description": "Record a commitment (intent / decision / hypothesis) into the system of intents. The wedge primitive — a forward-leaning intent and a backward-resolving decision are two phases of the same Commitment. See `docs/INTENT_SYSTEM.md` §1.1. Returns the commitment id; the caller can later attach an Outcome via `memory_resolve`. When a trained world model with ≥6 priors is available, the response also includes a `preflight` block with the user's track-record distribution for similar-shaped commitments and a `tone` ∈ {warning, mixed, tailwind} agents can route on.",
                "inputSchema": {
                    "type": "object",
                    "required": ["kind", "statement"],
                    "properties": {
                        "kind": {
                            "type": "string",
                            "enum": ["intent", "decision", "hypothesis"]
                        },
                        "statement": { "type": "string" },
                        "options": { "type": "array", "items": {"type": "string"} },
                        "chosen": { "type": "string", "description": "Which option won. Defaults to `statement`." },
                        "expected_outcome": { "type": "string" },
                        "horizon": { "type": "string", "format": "date-time", "description": "When we expect resolution. ISO-8601." },
                        "stakes": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "reversible"],
                            "default": "medium"
                        },
                        "confidence": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.7 },
                        "tags": { "type": "array", "items": {"type": "string"} },
                        "derived_from": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "UUIDs of prior commitments this one supersedes / refines."
                        }
                    }
                }
            },
            {
                "name": "memory_resolve",
                "description": "Attach an Outcome to a previously-recorded Commitment. Walks the state machine `Open|Acted → Completed`. `polarity: no_outcome` is a legitimate value for things that fizzled. See `docs/INTENT_SYSTEM.md` §1.2.",
                "inputSchema": {
                    "type": "object",
                    "required": ["commitment_id", "polarity"],
                    "properties": {
                        "commitment_id": { "type": "string" },
                        "polarity": {
                            "type": "string",
                            "enum": ["better", "as_expected", "worse", "mixed", "no_outcome"]
                        },
                        "description": { "type": "string" },
                        "evidence": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Trace UUIDs that prove the outcome."
                        },
                        "user_note": { "type": "string" }
                    }
                }
            },
            {
                "name": "memory_query",
                "description": "Query TraceMind memory with natural-language text. Returns relevant entities and triples. Defaults to the active context; set cross_context=true to bridge all contexts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Query text."
                        },
                        "cross_context": {
                            "type": "boolean",
                            "description": "Sprint C-0.6 — when true, ignores the active context and searches every namespace. Default false (scoped).",
                            "default": false
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
            },
            {
                "name": "memory_brief",
                "description": "Generate the daily brief — overdue / open / recently-resolved commitments + pending mined candidates. Read-only view of the system of intents. See `docs/INTENT_SYSTEM.md` §9.1.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "resolved_days": {
                            "type": "integer",
                            "description": "Look-back window for the resolved section in days (default 7)",
                            "default": 7
                        },
                        "limit_open": {
                            "type": "integer",
                            "description": "Cap on rows in each non-empty section (default 20)",
                            "default": 20
                        }
                    }
                }
            },
            {
                "name": "memory_insight_silence",
                "description": "Silence the insight-panel surface for one open commitment. Suppresses outlook-divergence highlights for the given commitment_id over a TTL window. Does NOT remove the commitment row itself — only the insight highlight. Idempotent: re-silencing only extends the window.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "commitment_id": {"type": "string", "description": "UUID of the open commitment to silence."},
                        "days":          {"type": "integer", "description": "Silence window in days (default 30).", "default": 30},
                        "reason":        {"type": "string", "description": "Optional free-text reason — stored alongside the silence."}
                    },
                    "required": ["commitment_id"]
                }
            },
            {
                "name": "memory_insight_unsilence",
                "description": "Remove an active insight silence for a commitment. No-op if no silence exists. Returns `{ removed: bool }`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "commitment_id": {"type": "string"}
                    },
                    "required": ["commitment_id"]
                }
            },
            {
                "name": "memory_insight_silences",
                "description": "List active (non-expired) insight silences as of `now`. Returns `{ silences: [{ commitment_id, silenced_at, silenced_until, reason }] }`.",
                "inputSchema": {"type": "object", "properties": {}}
            },
            {
                "name": "memory_pattern_silence",
                "description": "Silence a pattern-detector cell so it stops surfacing in the brief. `cell_hash` comes from a prior brief or `memory_pattern_silences` listing. Idempotent: extends an existing silence window, never shrinks it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cell_hash":  {"type": "string"},
                        "cell_label": {"type": "string", "description": "Optional human-readable label of the cell, stored for the silenced-list view."},
                        "days":       {"type": "integer", "description": "Silence window in days (default 90).", "default": 90},
                        "reason":     {"type": "string"}
                    },
                    "required": ["cell_hash"]
                }
            },
            {
                "name": "memory_pattern_unsilence",
                "description": "Remove an active pattern-cell silence. Returns `{ removed: bool }`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cell_hash": {"type": "string"}
                    },
                    "required": ["cell_hash"]
                }
            },
            {
                "name": "memory_pattern_silences",
                "description": "List active (non-expired) pattern-cell silences. Returns `{ silences: [{ cell_hash, cell_label, silenced_at, silenced_until, reason }] }`.",
                "inputSchema": {"type": "object", "properties": {}}
            },
            {
                "name": "memory_world_calibration",
                "description": "Score the world model's predictions against eventual resolution polarity. Out-of-sample by default (only commitments resolved after `model.trained_at` are scored) so the report is honest about generalization. Returns the full `CalibrationReport` (accuracy, positive_recall, warning_precision, multiclass Brier, per-class confusion-matrix breakdown, plus skip counters). See `docs/INTENT_SYSTEM.md` §7.2.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since_days": {"type": "integer", "description": "Look-back window in days for completed commitments (default 365).", "default": 365},
                        "limit":      {"type": "integer", "description": "Cap on rows fetched from the intent store (default 5000).", "default": 5000},
                        "all":        {"type": "boolean", "description": "Skip the out-of-sample filter and score every completed row, including rows the model trained on. Off by default.", "default": false}
                    }
                }
            },
            {
                "name": "memory_outcome_proposals",
                "description": "List active (pending, unexpired) outcome proposals — persisted suggestions from the implicit text matcher (`docs/INTENT_SYSTEM.md` §4.2) that a fresh capture may have described what happened to an open commitment. The brief shows these too; this tool gives agents a programmatic surface. Returns proposal id, commitment id + statement snapshot, proposed_polarity, description, similarity, proposed_at, expires_at.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "description": "Cap on proposals returned (default 20).", "default": 20}
                    }
                }
            },
            {
                "name": "memory_outcome_accept",
                "description": "Accept an outcome proposal — promotes it into a real `Outcome` row, transitions the underlying commitment to Completed, and links the proposal to the new outcome id. Idempotent: a non-pending proposal returns an error rather than re-resolving.",
                "inputSchema": {
                    "type": "object",
                    "required": ["proposal_id"],
                    "properties": {
                        "proposal_id": {"type": "string", "description": "Proposal UUID from memory_outcome_proposals."},
                        "note":        {"type": "string", "description": "Optional free-text user note attached to the resulting Outcome."}
                    }
                }
            },
            {
                "name": "memory_outcome_dismiss",
                "description": "Dismiss an outcome proposal — marks it terminal so the brief stops surfacing it. Does not touch the underlying commitment.",
                "inputSchema": {
                    "type": "object",
                    "required": ["proposal_id"],
                    "properties": {
                        "proposal_id": {"type": "string", "description": "Proposal UUID from memory_outcome_proposals."}
                    }
                }
            },
            {
                "name": "memory_need",
                "description": "Record a user need — the 'why' behind commitments. Needs drive the intent arc: Need → Sentiment → Commitment → Action → Outcome. Returns the need_id for linking to commitments later.",
                "inputSchema": {
                    "type": "object",
                    "required": ["statement"],
                    "properties": {
                        "statement":  {"type": "string", "description": "What the user needs (e.g. 'ship v2 by Friday', 'learn Rust async')."},
                        "urgency":    {"type": "number", "description": "0.0 (low) to 1.0 (critical). Default 0.5."},
                        "recurring":  {"type": "boolean", "description": "True for ongoing needs (health, learning); false for one-shot. Default false."},
                        "tags":       {"type": "array", "items": {"type": "string"}, "description": "Optional tags for grouping."},
                        "link_commitment": {"type": "string", "description": "Optional commitment UUID to link this need to."}
                    }
                }
            },
            {
                "name": "memory_sentiment",
                "description": "Record sentiment toward a target (commitment, need, entity, or topic). Captures affective signal that weights decisions in the intent arc.",
                "inputSchema": {
                    "type": "object",
                    "required": ["target_id", "target_type", "valence"],
                    "properties": {
                        "target_id":     {"type": "string", "description": "UUID of the target (commitment, need, etc)."},
                        "target_type":   {"type": "string", "enum": ["commitment", "need", "entity", "topic"], "description": "What kind of thing the target is."},
                        "valence":       {"type": "number", "description": "-1.0 (strongly negative) to +1.0 (strongly positive)."},
                        "intensity":     {"type": "number", "description": "0.0 (barely noticeable) to 1.0 (overwhelming). Defaults to abs(valence)."},
                        "evidence_text": {"type": "string", "description": "The text that triggered this sentiment reading."},
                        "evidence_trace":{"type": "string", "description": "Optional trace UUID linking to the originating trace."}
                    }
                }
            },
            {
                "name": "memory_action",
                "description": "Record an action taken toward a commitment. Actions are the 'what I did' that links commitments to outcomes in the intent arc.",
                "inputSchema": {
                    "type": "object",
                    "required": ["description"],
                    "properties": {
                        "description":   {"type": "string", "description": "What was done (e.g. 'merged PR #42', 'called the dentist')."},
                        "commitment_id": {"type": "string", "description": "Optional commitment UUID this action relates to."},
                        "modality":      {"type": "string", "enum": ["digital", "physical", "communication", "creation"], "description": "Action modality. Default 'digital'."},
                        "evidence":      {"type": "array", "items": {"type": "string"}, "description": "Optional trace UUIDs evidencing this action."}
                    }
                }
            },
            {
                "name": "memory_arc",
                "description": "Retrieve the full intent arc for a commitment — the materialized view showing Need → Sentiment → Commitment → Action → Outcome. Returns the commitment with all linked needs, sentiments, actions, and outcome.",
                "inputSchema": {
                    "type": "object",
                    "required": ["commitment_id"],
                    "properties": {
                        "commitment_id": {"type": "string", "description": "Commitment UUID to build the arc for."},
                        "sentiment_limit": {"type": "integer", "description": "Max sentiments to include (default 10).", "default": 10},
                        "action_limit":    {"type": "integer", "description": "Max actions to include (default 20).", "default": 20}
                    }
                }
            },
            {
                "name": "memory_feedback",
                "description": "Record per-result feedback that trains the retrieval bandit. `kind` selects the channel: `helpful` writes to `positive_signals` (F-1, default weight 0.3); `not_related` and `cross_context_bridge` write to `negative_signals` (C-0.7, default weight 1.0). All three kinds feed `finalize_pending_reward` so the bandit's reward = `(relevance + Σ positives − Σ negatives).clamp(0, 1)`. `query_id` is the UUID returned in the previous `memory_query` response.",
                "inputSchema": {
                    "type": "object",
                    "required": ["query_id", "result_id", "kind"],
                    "properties": {
                        "query_id":   {"type": "string", "description": "Query UUID returned by `memory_query`."},
                        "result_id":  {"type": "string", "description": "Stable id of the result row receiving feedback (entity UUID or triple UUID)."},
                        "kind":       {"type": "string", "enum": ["helpful", "not_related", "cross_context_bridge"], "description": "Feedback channel. `helpful` → positive_signals. `not_related` / `cross_context_bridge` → negative_signals."},
                        "weight":     {"type": "number", "description": "Override default weight (helpful default 0.3, negative kinds default 1.0). Must be ≥ 0."},
                        "context_id": {"type": "string", "description": "Optional context UUID for positive feedback (the active scope at click time)."},
                        "context_a":  {"type": "string", "description": "For negative feedback that crosses a context boundary: the bad-result context."},
                        "context_b":  {"type": "string", "description": "For negative feedback that crosses a context boundary: the query's active context."}
                    }
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
    intents_path: &str,
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
    drop(recent_store);

    // Sprint C / INTENT_SYSTEM.md §3.1 — mine MCP turn text for
    // commitment-shaped phrases. Each hit is persisted as a `pending`
    // candidate for the daily brief to confirm. Miner / store failures
    // are soft — they must never block the ingest response.
    // We open the intent store once, do both miner work and outcome
    // matching against the snapshot of open commitments. Both paths
    // are soft-fail — neither blocks the ingest response.
    let (mined_count, outcome_proposals): (usize, Vec<Value>) = {
        let mined = tm_intent::mine(text);
        match tm_intent::IntentStore::open(intents_path) {
            Ok(mut store) => {
                // Persist newly-mined candidates.
                let n = if mined.is_empty() {
                    0
                } else {
                    let records: Vec<tm_intent::store::CandidateRecord> = mined
                        .iter()
                        .map(|m| tm_intent::store::CandidateRecord::from_mined(m, text.to_string()))
                        .collect();
                    match store.insert_candidates(&records) {
                        Ok(n) => n,
                        Err(e) => {
                            tracing::debug!("[mcp/miner] persist failed: {e}");
                            0
                        }
                    }
                };

                // Sprint D / INTENT_SYSTEM.md §4.2 — implicit outcome
                // matching. Score the new text against open
                // commitments; surface proposals so the agent can
                // choose to call `memory_resolve`. Per TM-INTENT-009
                // we *also persist* every polarity-hinted proposal
                // into `outcome_proposals` so the daily brief can
                // carry it forward — the inline JSON response is
                // ephemeral, the persisted row is the source of
                // truth for "what happened with X?"
                let proposals = match store.list_open(50) {
                    Ok(opens) => {
                        let cfg = tm_reflect::MatcherConfig::default();
                        let now = chrono::Utc::now();
                        // 30d expiry mirrors `outcome_proposals.expires_at`
                        // doc on the schema; matches the §4.2 "fade if
                        // not acted on" rule.
                        let expiry = now + chrono::Duration::days(30);
                        tm_reflect::propose_outcomes(text, &opens, &cfg)
                            .into_iter()
                            .map(|p| {
                                // Persist if we have a polarity hint
                                // (no hint = the user must label, so
                                // there's no cell_key to dedup against
                                // and no actionable proposal yet).
                                let persisted_id = match p.polarity_hint {
                                    Some(polarity) => {
                                        let record = tm_intent::OutcomeProposal {
                                            id: Uuid::new_v4(),
                                            commitment_id: p.commitment_id,
                                            cell_key: tm_intent::OutcomeProposal::cell_key_for(
                                                p.commitment_id,
                                                polarity,
                                            ),
                                            proposed_polarity: polarity,
                                            description: p.reason.clone(),
                                            similarity: p.score,
                                            source_trace_id: None,
                                            proposed_at: now,
                                            expires_at: expiry,
                                            status: "pending".into(),
                                            resolved_at: None,
                                            resolved_outcome_id: None,
                                        };
                                        let id = record.id;
                                        match store.insert_outcome_proposal(&record) {
                                            Ok(true) => Some(id.to_string()),
                                            Ok(false) => None,
                                            Err(e) => {
                                                tracing::debug!(
                                                    "[mcp/matcher] persist proposal failed: {e}"
                                                );
                                                None
                                            }
                                        }
                                    }
                                    None => None,
                                };
                                json!({
                                    "id": persisted_id,
                                    "commitment_id": p.commitment_id.to_string(),
                                    "commitment_statement": p.commitment_statement,
                                    "score": p.score,
                                    "polarity_hint": p.polarity_hint.map(|x| match x {
                                        tm_intent::Polarity::Better => "better",
                                        tm_intent::Polarity::AsExpected => "as_expected",
                                        tm_intent::Polarity::Worse => "worse",
                                        tm_intent::Polarity::Mixed => "mixed",
                                        tm_intent::Polarity::NoOutcome => "no_outcome",
                                    }),
                                    "reason": p.reason,
                                })
                            })
                            .collect::<Vec<_>>()
                    }
                    Err(e) => {
                        tracing::debug!("[mcp/matcher] list_open failed: {e}");
                        Vec::new()
                    }
                };

                (n, proposals)
            }
            Err(e) => {
                tracing::debug!("[mcp/miner] open intents store failed: {e}");
                (0, Vec::new())
            }
        }
    };

    Ok(json!({
        "stored": true,
        "entities": result.entities.len(),
        "triples": result.triples.len(),
        "candidates_mined": mined_count,
        "outcome_proposals": outcome_proposals,
        "context": {
            "memories": context_memories,
            "related": context_related,
        }
    }))
}

/// `memory_store_structured` — typed-schema ingestion for callers that have
/// already done entity/triple extraction (an LLM, an MCP tool, a parser).
///
/// Skips the heuristic NER and the ingest pipeline's selective gate, but
/// keeps the same dedup / case-insensitive name matching the natural-language
/// path uses. A single `Trace` is written for the whole batch so audits can
/// reconstruct provenance.
async fn handle_memory_store_structured(
    params: &Value,
    db_path: &str,
    traces: &Arc<Mutex<TraceStore>>,
    recent: &Arc<Mutex<RecentStore>>,
    session_id: Uuid,
) -> Result<Value, String> {
    use tm_types::{Entity, Trace, TraceEventType, Triple};

    let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let raw_entities = params
        .get("entities")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let raw_triples = params
        .get("triples")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if raw_entities.is_empty() && raw_triples.is_empty() {
        return Err("at least one of `entities` or `triples` must be non-empty".to_string());
    }

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;

    // Resolve / upsert each entity. Build a lower-cased name → UUID map so
    // triples can reference subjects/objects by name regardless of casing.
    let mut name_to_id: std::collections::HashMap<String, Uuid> =
        std::collections::HashMap::new();
    let mut stored_entities: Vec<Entity> = Vec::new();
    let mut entity_outputs: Vec<Value> = Vec::new();

    for raw in &raw_entities {
        let name = raw
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "entity missing required field: name".to_string())?
            .trim()
            .to_string();
        if name.is_empty() {
            return Err("entity name must be non-empty".to_string());
        }
        let type_str = raw
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("entity '{name}' missing required field: type"))?;
        let entity_type = parse_entity_type(type_str);
        let confidence = raw
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.9);

        // Reuse existing entity by case-insensitive name match; only mint a
        // new UUID when this name is unknown.
        let resolved = if let Ok(Some(existing)) = graph.find_entity_by_name_icase(&name) {
            let _ = graph.reinforce_entity(existing.id, 0.05);
            existing
        } else {
            let mut e = Entity::new(name.clone(), entity_type, confidence);
            graph.upsert_entity(&e).map_err(|err| err.to_string())?;
            // Re-read so we have the canonical timestamps.
            if let Ok(Some(stored)) = graph.find_entity_by_name_icase(&name) {
                e = stored;
            }
            e
        };

        name_to_id.insert(name.to_lowercase(), resolved.id);
        entity_outputs.push(json!({
            "id": resolved.id.to_string(),
            "name": resolved.name,
            "type": format!("{:?}", resolved.entity_type).to_lowercase(),
        }));
        stored_entities.push(resolved);
    }

    // Resolve triple endpoints. Subjects/objects can either be entities we
    // just wrote in this call, or names already in the graph from earlier
    // calls. Anything else is reported back so the caller can correct it.
    let mut stored_triples: Vec<Triple> = Vec::new();
    let mut skipped_triples: Vec<Value> = Vec::new();

    let mut resolve = |name: &str| -> Option<Uuid> {
        let key = name.trim().to_lowercase();
        if let Some(id) = name_to_id.get(&key) {
            return Some(*id);
        }
        if let Ok(Some(existing)) = graph.find_entity_by_name_icase(name.trim()) {
            name_to_id.insert(key, existing.id);
            return Some(existing.id);
        }
        None
    };

    for raw in &raw_triples {
        let subj_name = raw.get("subject").and_then(|v| v.as_str()).unwrap_or("");
        let obj_name = raw.get("object").and_then(|v| v.as_str()).unwrap_or("");
        let pred_str = raw.get("predicate").and_then(|v| v.as_str()).unwrap_or("");
        let confidence = raw
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.85);

        if subj_name.is_empty() || obj_name.is_empty() || pred_str.is_empty() {
            skipped_triples.push(json!({
                "subject": subj_name,
                "predicate": pred_str,
                "object": obj_name,
                "reason": "missing subject/predicate/object",
            }));
            continue;
        }

        let subj_id = match resolve(subj_name) {
            Some(id) => id,
            None => {
                skipped_triples.push(json!({
                    "subject": subj_name,
                    "predicate": pred_str,
                    "object": obj_name,
                    "reason": "subject not in entities[] and not found in graph",
                }));
                continue;
            }
        };
        let obj_id = match resolve(obj_name) {
            Some(id) => id,
            None => {
                skipped_triples.push(json!({
                    "subject": subj_name,
                    "predicate": pred_str,
                    "object": obj_name,
                    "reason": "object not in entities[] and not found in graph",
                }));
                continue;
            }
        };

        let predicate = parse_predicate(pred_str);
        let triple = Triple::new(subj_id, predicate, obj_id, confidence);
        graph
            .upsert_triple(&triple)
            .map_err(|err| err.to_string())?;
        stored_triples.push(triple);
    }

    // One trace for the whole batch so audits can recover what went in.
    let content_hash = format!("structured:{:016x}", seahash_text(text));
    let mut trace = Trace::new(session_id, TraceEventType::Ingest, &content_hash);
    trace.raw_text = if text.is_empty() {
        Some(format!(
            "[structured ingest: {} entities, {} triples]",
            stored_entities.len(),
            stored_triples.len()
        ))
    } else {
        Some(text.to_string())
    };
    trace.entities_extracted = stored_entities.iter().map(|e| e.id).collect();
    trace.triples_extracted = stored_triples.iter().map(|t| t.id).collect();
    let trace_store = traces.lock().await;
    let _ = trace_store.append(&trace);
    drop(trace_store);

    // Capture-feedback ring buffer entry for parity with `memory_store`.
    let preview = if text.is_empty() {
        format!(
            "{} entities / {} triples",
            stored_entities.len(),
            stored_triples.len()
        )
    } else {
        text.to_string()
    };
    let event = RecentCapture::new("mcp-structured", &content_hash, &preview)
        .with_tier("structured")
        .with_promoted(true);
    let recent_store = recent.lock().await;
    if let Err(e) = recent_store.append(&event) {
        tracing::debug!("[mcp] failed to append structured recent capture: {e}");
    }

    Ok(json!({
        "stored": true,
        "trace_id": trace.id.to_string(),
        "entities": entity_outputs,
        "triples": stored_triples.len(),
        "skipped_triples": skipped_triples,
    }))
}

// ---------------------------------------------------------------------------
// Sprint B — system-of-intents handlers
// ---------------------------------------------------------------------------

/// `memory_commit` — record a Commitment (intent / decision / hypothesis).
/// Opens the intent store fresh per call; cheap on SQLite and avoids
/// holding a long-lived handle in the server state for what is still a
/// low-volume surface.
async fn handle_memory_commit(
    params: &Value,
    intents_path: &str,
) -> Result<Value, String> {
    use tm_intent::{Commitment, CommitmentKind, IntentStore, Source, Stakes};

    let kind_s = params
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: kind".to_string())?;
    let kind = match kind_s {
        "intent" => CommitmentKind::Intent,
        "decision" => CommitmentKind::Decision,
        "hypothesis" => CommitmentKind::Hypothesis,
        other => return Err(format!("invalid kind: {other}")),
    };

    let statement = params
        .get("statement")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: statement".to_string())?
        .trim()
        .to_string();
    if statement.is_empty() {
        return Err("statement must be non-empty".to_string());
    }

    let mut c = Commitment::new(kind, statement, Source::McpStructured);

    if let Some(opts) = params.get("options").and_then(|v| v.as_array()) {
        c.options_considered = opts
            .iter()
            .filter_map(|o| o.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(chosen) = params.get("chosen").and_then(|v| v.as_str()) {
        c.chosen = chosen.to_string();
    }
    if let Some(exp) = params.get("expected_outcome").and_then(|v| v.as_str()) {
        c.expected_outcome = Some(exp.to_string());
    }
    if let Some(h) = params.get("horizon").and_then(|v| v.as_str()) {
        c.horizon = Some(
            chrono::DateTime::parse_from_rfc3339(h)
                .map(|t| t.with_timezone(&chrono::Utc))
                .map_err(|e| format!("invalid horizon (need RFC3339): {e}"))?,
        );
    }
    if let Some(s) = params.get("stakes").and_then(|v| v.as_str()) {
        c.stakes = match s {
            "low" => Stakes::Low,
            "medium" => Stakes::Medium,
            "high" => Stakes::High,
            "reversible" => Stakes::Reversible,
            other => return Err(format!("invalid stakes: {other}")),
        };
    }
    if let Some(f) = params.get("confidence").and_then(|v| v.as_f64()) {
        if !(0.0..=1.0).contains(&f) {
            return Err(format!("confidence must be in [0,1], got {f}"));
        }
        c.confidence = f as f32;
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        c.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(refs) = params.get("derived_from").and_then(|v| v.as_array()) {
        let mut parsed = Vec::with_capacity(refs.len());
        for r in refs {
            let s = r.as_str().ok_or_else(|| "derived_from entries must be UUID strings".to_string())?;
            parsed.push(Uuid::parse_str(s).map_err(|e| format!("invalid derived_from uuid '{s}': {e}"))?);
        }
        c.derived_from = parsed;
    }

    let store = IntentStore::open(intents_path).map_err(|e| e.to_string())?;
    store.insert_commitment(&c).map_err(|e| e.to_string())?;

    // World-model preflight (INTENT_SYSTEM.md §6.2). The world model
    // lives at <dir>/world_model.json — sibling of the intents store.
    // Failure to load is silent: a missing/dormant model just omits
    // the preflight block from the response.
    let preflight = build_preflight_for(&c, intents_path);

    let mut resp = json!({
        "commitment_id": c.id.to_string(),
        "state": "open",
        "kind": kind_s,
    });
    if let Some(p) = preflight {
        resp["preflight"] = p;
    }
    Ok(resp)
}

/// Best-effort world-model preflight for `memory_commit`. Returns
/// `None` when the model is missing, untrained, or under-supported
/// (n_priors < 6), so the caller can opt in by simply checking for
/// the field.
fn build_preflight_for(
    c: &tm_intent::Commitment,
    intents_path: &str,
) -> Option<Value> {
    use tm_world_model::{load, PolarityClass};

    const MIN_PRIORS: usize = 6;

    let world_path = std::path::Path::new(intents_path)
        .parent()
        .map(|p| p.join("world_model.json"))?;
    let model = match load(&world_path) {
        Ok(Some(m)) if m.is_trained() && m.n_train_examples >= MIN_PRIORS => m,
        _ => return None,
    };
    let pred = model.predict(c);
    let dist = pred.dist.0;
    // Match the CLI's tone bands so MCP and CLI consumers see the
    // same "tone" signal — agents can branch on it without mirroring
    // the threshold logic.
    let positive = pred.positive_prob;
    let tone = if matches!(pred.argmax, PolarityClass::Worse) && positive < 0.40 {
        "warning"
    } else if positive > 0.65 {
        "tailwind"
    } else {
        "mixed"
    };
    Some(json!({
        "n_priors": pred.n_priors,
        "argmax": format!("{:?}", pred.argmax).to_lowercase(),
        "positive_prob": positive,
        "confidence": pred.confidence,
        "dist": {
            "better": dist[PolarityClass::Better.index()],
            "as_expected": dist[PolarityClass::AsExpected.index()],
            "worse": dist[PolarityClass::Worse.index()],
            "mixed": dist[PolarityClass::Mixed.index()],
        },
        "tone": tone,
    }))
}

/// `memory_resolve` — attach an Outcome and walk the state machine to
/// `Completed`. Rejects mismatched commitment ids and terminal states
/// at the state-machine layer.
async fn handle_memory_resolve(
    params: &Value,
    intents_path: &str,
) -> Result<Value, String> {
    use tm_intent::{state::transition, IntentStore, Outcome, OutcomeSource, Polarity, State};

    let cid = params
        .get("commitment_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: commitment_id".to_string())?;
    let cid = Uuid::parse_str(cid).map_err(|e| format!("invalid commitment_id: {e}"))?;

    let polarity_s = params
        .get("polarity")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: polarity".to_string())?;
    let polarity = match polarity_s {
        "better" => Polarity::Better,
        "as_expected" => Polarity::AsExpected,
        "worse" => Polarity::Worse,
        "mixed" => Polarity::Mixed,
        "no_outcome" => Polarity::NoOutcome,
        other => return Err(format!("invalid polarity: {other}")),
    };

    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user_note = params
        .get("user_note")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let evidence: Vec<Uuid> = params
        .get("evidence")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                .collect()
        })
        .unwrap_or_default();

    let store = IntentStore::open(intents_path).map_err(|e| e.to_string())?;
    let mut commitment = store
        .get_commitment(cid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("commitment {cid} not found"))?;

    let mut outcome = Outcome::new(cid, polarity, description, OutcomeSource::McpStructured);
    outcome.user_note = user_note;
    outcome.evidence_traces = evidence;

    transition(&mut commitment, State::Completed, Some(&outcome))
        .map_err(|e| format!("state transition rejected: {e}"))?;

    store.insert_outcome(&outcome).map_err(|e| e.to_string())?;
    store
        .update_state(commitment.id, commitment.state, commitment.outcome_id)
        .map_err(|e| e.to_string())?;

    // Ambient retrain — refresh `world_model.json` so the next
    // `memory_brief` / `memory_commit` preflight reflects this newly
    // observed outcome. Soft-fail; never blocks the resolve.
    let world_retrain = auto_retrain_world_model_for(intents_path);

    let mut resp = json!({
        "outcome_id": outcome.id.to_string(),
        "commitment_id": commitment.id.to_string(),
        "commitment_state": "completed",
        "polarity": polarity_s,
    });
    if let Some(msg) = world_retrain {
        resp["world_model"] = Value::String(msg);
    }
    Ok(resp)
}

/// MCP-side mirror of the CLI's `auto_retrain_world_model`. Same
/// hard-floor (`min_examples`), same window (365d), same persistence
/// path (sibling of the intents store). Returns `None` silently when
/// we don't have enough priors yet OR when persistence fails — the
/// caller never sees an error from this path.
fn auto_retrain_world_model_for(intents_path: &str) -> Option<String> {
    use tm_intent::IntentStore;
    use tm_world_model::{from_pairs, save, train, TrainerConfig};
    let world_path = std::path::Path::new(intents_path)
        .parent()
        .map(|p| p.join("world_model.json"))?;
    let store = IntentStore::open(intents_path).ok()?;
    let cfg = TrainerConfig::default();
    let since = chrono::Utc::now() - chrono::Duration::days(365);
    let rows = store.list_completed_with_polarity(since, 5000).ok()?;
    let (examples, _skipped) = from_pairs(rows);
    if examples.len() < cfg.min_examples {
        return None;
    }
    let (model, report) = train(&examples, &cfg);
    save(&model, &world_path).ok()?;
    Some(format!(
        "retrained on {} priors (acc {:.0}%)",
        report.n_examples,
        report.final_accuracy * 100.0
    ))
}

fn parse_entity_type(s: &str) -> tm_types::EntityType {
    use tm_types::EntityType;
    match s.trim().to_lowercase().as_str() {
        "person" => EntityType::Person,
        "organization" | "org" => EntityType::Organization,
        "project" => EntityType::Project,
        "file" => EntityType::File,
        "url" | "link" => EntityType::Url,
        "concept" => EntityType::Concept,
        "technology" | "tech" => EntityType::Technology,
        "decision" => EntityType::Decision,
        "event" => EntityType::Event,
        other => EntityType::Custom(other.to_string()),
    }
}

fn parse_predicate(s: &str) -> tm_types::Predicate {
    use tm_types::Predicate;
    match s.trim().to_lowercase().as_str() {
        "related_to" | "relatedto" | "related-to" => Predicate::RelatedTo,
        "is_a" | "isa" | "is-a" => Predicate::IsA,
        "part_of" | "partof" | "part-of" => Predicate::PartOf,
        "has_property" | "hasproperty" => Predicate::HasProperty,
        "works_at" | "worksat" => Predicate::WorksAt,
        "collaborates_with" | "collaborateswith" => Predicate::CollaboratesWith,
        "owns" => Predicate::Owns,
        "depends_on" | "dependson" => Predicate::DependsOn,
        "produces" => Predicate::Produces,
        "references" | "refs" => Predicate::References,
        "has_procedure" | "hasprocedure" => Predicate::HasProcedure,
        other => Predicate::Custom(other.to_string()),
    }
}

fn seahash_text(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut h = seahash::SeaHasher::default();
    h.write(s.as_bytes());
    h.finish()
}

async fn handle_memory_query(
    params: &Value,
    retrieval: &Arc<Mutex<RetrievalEngine>>,
    answerer: &Arc<TieredAnswerer>,
    db_path: &str,
    intents_path: &str,
) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: text".to_string())?;
    let cross_context = params
        .get("cross_context")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut engine = retrieval.lock().await;
    engine.set_cross_context(cross_context);
    // Same rationale as memory_store: the retrieval engine's GraphStore cache
    // is stale relative to writes from the ingest-side GraphStore in a
    // long-running MCP session. See TM-UX-001 Phase C.
    let _ = engine.refresh_graph();

    // Phase 4 / Sprint B: warm L1 prefetch from active anticipations.
    // The orchestrator pass is cheap (kNN per query, capped) and
    // idempotent (cache.prime overwrites in place), so running it
    // before each query is safe. Per docs/INTENT_SYSTEM.md §6.1, the
    // L1 layer fires "any time the user starts an interaction" — a
    // memory_query call is exactly that trigger.
    if let Ok(intent_store) = tm_intent::IntentStore::open(intents_path) {
        let _ = prefetch_orchestrator::warm_l1(
            &mut engine,
            &intent_store,
            chrono::Utc::now(),
            8,
        );
        // Failures here are non-fatal — the WarmReport already
        // records them; we just don't surface them on this path.
    }

    let result = engine.query(text).map_err(|e| e.to_string())?;
    drop(engine);

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

    // Sprint A: dispatch through the tiered answerer so MCP callers get a
    // grounded prose answer (Tier 0 baseline; Tier 1 when local-llm feature
    // is on and weights are present).
    let grounding = answerer::grounding_from(&result, 6);
    let req = answerer::short_answer_request(text, grounding);
    let answer_value: Value = match answerer.answer(&req).await {
        Ok(resp) => {
            let cites: Vec<Value> = resp
                .citations
                .iter()
                .map(|c| {
                    json!({
                        "trace_id": c.trace_id,
                        "entity_ids": c.entity_ids,
                        "chunk_index": c.chunk_index,
                    })
                })
                .collect();
            json!({
                "text": resp.text,
                "tier": format!("{:?}", resp.tier).to_lowercase(),
                "citations": cites,
                "latency_ms": resp.latency_ms,
            })
        }
        Err(e) => {
            json!({ "error": e.to_string() })
        }
    };

    // Auto-routing: if the planner detected a reasoning/analogy query,
    // enrich the response with supplemental reasoning data.
    let mut response = json!({
        "answer": answer_value,
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

/// `memory_feedback` — F-1 / C-0.7 per-result feedback channel.
///
/// `kind` routes the signal:
///   * `helpful`               → `positive_signals` (default weight 0.3)
///   * `not_related`           → `negative_signals` (default weight 1.0)
///   * `cross_context_bridge`  → `negative_signals` (default weight 1.0, expects
///                                `context_a` / `context_b`)
///
/// All three feed `finalize_pending_reward`, so the bandit reward composes as
/// `(relevance + Σ positives − Σ negatives).clamp(0, 1)`.
fn handle_memory_feedback(params: &Value, db_path: &str) -> Result<Value, String> {
    let query_id_s = params
        .get("query_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: query_id".to_string())?;
    let query_id = Uuid::parse_str(query_id_s)
        .map_err(|e| format!("invalid query_id '{query_id_s}': {e}"))?;

    let result_id = params
        .get("result_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: result_id".to_string())?;

    let kind = params
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: kind".to_string())?;

    let override_weight = params.get("weight").and_then(|v| v.as_f64()).map(|w| w as f32);
    if let Some(w) = override_weight {
        if !w.is_finite() || w < 0.0 {
            return Err(format!("weight must be a finite non-negative number, got {w}"));
        }
    }

    let parse_ctx = |key: &str| -> Result<Option<Uuid>, String> {
        match params.get(key).and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => Uuid::parse_str(s)
                .map(Some)
                .map_err(|e| format!("invalid {key} '{s}': {e}")),
            _ => Ok(None),
        }
    };

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;

    match kind {
        "helpful" => {
            let weight = override_weight.unwrap_or(0.3);
            let context_id = parse_ctx("context_id")?;
            let row_id = graph
                .write_positive_signal(query_id, result_id, kind, context_id, weight)
                .map_err(|e| format!("write_positive_signal: {e}"))?;
            Ok(json!({
                "ok": true,
                "channel": "positive_signals",
                "row_id": row_id,
                "kind": kind,
                "weight": weight,
            }))
        }
        "not_related" | "cross_context_bridge" => {
            let weight = override_weight.unwrap_or(1.0);
            let context_a = parse_ctx("context_a")?;
            let context_b = parse_ctx("context_b")?;
            let row_id = graph
                .write_negative_signal(query_id, result_id, kind, context_a, context_b, weight)
                .map_err(|e| format!("write_negative_signal: {e}"))?;
            Ok(json!({
                "ok": true,
                "channel": "negative_signals",
                "row_id": row_id,
                "kind": kind,
                "weight": weight,
            }))
        }
        other => Err(format!(
            "invalid kind '{other}': expected one of helpful, not_related, cross_context_bridge"
        )),
    }
}

/// `memory_brief` — render the daily brief from the system of intents
/// (`docs/INTENT_SYSTEM.md` §9.1).
///
/// Read-only: never mutates the intent store. Returns the
/// [`tm_reflect::DailyBrief`] structure verbatim as JSON.
fn handle_memory_brief(params: &Value, intents_path: &str) -> Result<Value, String> {
    use chrono::{Duration, Utc};
    use tm_intent::IntentStore;
    use tm_reflect::{BriefBuilder, BriefConfig};

    let resolved_days = params
        .get("resolved_days")
        .and_then(|v| v.as_i64())
        .unwrap_or(7);
    let limit_open = params
        .get("limit_open")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;

    let cfg = BriefConfig {
        resolved_window: Duration::days(resolved_days),
        limit_open,
        limit_overdue: limit_open,
        limit_resolved: limit_open.min(20),
        limit_candidates: limit_open.min(20),
        // Pattern detector defaults — n ≥ 6, |lift| ≥ 0.25, etc.
        // (`INTENT_SYSTEM.md` §5.1.3). The scan window is the
        // 12-month lookback the spec calls for.
        ..BriefConfig::default()
    };

    // Best-effort world-model load. If `world_model.json` is missing,
    // dormant, or schema-mismatched, the brief still renders without
    // outlook annotations — same cold-start contract as the CLI.
    let world_model = std::path::Path::new(intents_path)
        .parent()
        .and_then(|p| tm_world_model::load(&p.join("world_model.json")).ok().flatten());

    let mut builder = BriefBuilder::new(&store).with_config(cfg);
    if let Some(ref m) = world_model {
        builder = builder.with_world_model(m);
    }
    let brief = builder
        .build(Utc::now())
        .map_err(|e| format!("brief failed: {e}"))?;

    serde_json::to_value(&brief).map_err(|e| format!("brief serialization: {e}"))
}

// ---------------------------------------------------------------------------
// Insight + pattern silencing handlers (TM-INTENT-007)
// ---------------------------------------------------------------------------

fn handle_memory_insight_silence(params: &Value, intents_path: &str) -> Result<Value, String> {
    use chrono::{Duration, Utc};
    use tm_intent::IntentStore;

    let cid_s = params
        .get("commitment_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: commitment_id".to_string())?;
    let cid = Uuid::parse_str(cid_s).map_err(|e| format!("invalid commitment_id: {e}"))?;
    let days = params.get("days").and_then(|v| v.as_i64()).unwrap_or(30);
    let reason = params.get("reason").and_then(|v| v.as_str());

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let now = Utc::now();
    let until = now + Duration::days(days);
    store
        .upsert_insight_silence(cid, now, until, reason)
        .map_err(|e| format!("upsert_insight_silence: {e}"))?;
    Ok(json!({
        "ok": true,
        "commitment_id": cid.to_string(),
        "silenced_until": until.to_rfc3339(),
        "days": days,
    }))
}

fn handle_memory_insight_unsilence(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::IntentStore;

    let cid_s = params
        .get("commitment_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: commitment_id".to_string())?;
    let cid = Uuid::parse_str(cid_s).map_err(|e| format!("invalid commitment_id: {e}"))?;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let removed = store
        .remove_insight_silence(cid)
        .map_err(|e| format!("remove_insight_silence: {e}"))?;
    Ok(json!({ "removed": removed, "commitment_id": cid.to_string() }))
}

fn handle_memory_insight_silences(intents_path: &str) -> Result<Value, String> {
    use chrono::Utc;
    use tm_intent::IntentStore;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let rows = store
        .list_active_insight_silences(Utc::now())
        .map_err(|e| format!("list_active_insight_silences: {e}"))?;
    let silences: Vec<Value> = rows
        .into_iter()
        .map(|s| {
            json!({
                "commitment_id":  s.commitment_id.to_string(),
                "silenced_at":    s.silenced_at.to_rfc3339(),
                "silenced_until": s.silenced_until.to_rfc3339(),
                "reason":         s.reason,
            })
        })
        .collect();
    Ok(json!({ "silences": silences }))
}

fn handle_memory_pattern_silence(params: &Value, intents_path: &str) -> Result<Value, String> {
    use chrono::{Duration, Utc};
    use tm_intent::IntentStore;

    let cell_hash = params
        .get("cell_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: cell_hash".to_string())?;
    let cell_label = params
        .get("cell_label")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let days = params.get("days").and_then(|v| v.as_i64()).unwrap_or(90);
    let reason = params.get("reason").and_then(|v| v.as_str());

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let now = Utc::now();
    let until = now + Duration::days(days);
    store
        .upsert_pattern_silence(cell_hash, cell_label, now, until, reason)
        .map_err(|e| format!("upsert_pattern_silence: {e}"))?;
    Ok(json!({
        "ok": true,
        "cell_hash": cell_hash,
        "silenced_until": until.to_rfc3339(),
        "days": days,
    }))
}

fn handle_memory_pattern_unsilence(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::IntentStore;

    let cell_hash = params
        .get("cell_hash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: cell_hash".to_string())?;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let removed = store
        .remove_pattern_silence(cell_hash)
        .map_err(|e| format!("remove_pattern_silence: {e}"))?;
    Ok(json!({ "removed": removed, "cell_hash": cell_hash }))
}

fn handle_memory_pattern_silences(intents_path: &str) -> Result<Value, String> {
    use chrono::Utc;
    use tm_intent::IntentStore;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let rows = store
        .list_active_pattern_silences(Utc::now())
        .map_err(|e| format!("list_active_pattern_silences: {e}"))?;
    let silences: Vec<Value> = rows
        .into_iter()
        .map(|s| {
            json!({
                "cell_hash":      s.cell_hash,
                "cell_label":     s.cell_label,
                "silenced_at":    s.silenced_at.to_rfc3339(),
                "silenced_until": s.silenced_until.to_rfc3339(),
                "reason":         s.reason,
            })
        })
        .collect();
    Ok(json!({ "silences": silences }))
}

/// Out-of-sample calibration of `f_outcome` against eventual outcome
/// polarity. Mirrors the CLI `tracemind world calibration` subcommand
/// — same report shape, same defaults, same `--all` debug knob.
///
/// Resolves the world model from the intent-store sibling
/// `world_model.json` (matches the layout used by `memory_brief` /
/// `memory_commit`). Returns a 400-equivalent error string when no
/// model is on disk so the caller knows to run `world train` first.
fn handle_memory_world_calibration(
    params: &Value,
    intents_path: &str,
) -> Result<Value, String> {
    use tm_intent::IntentStore;
    use tm_world_model::{evaluate, load, split_out_of_sample};

    let since_days = params.get("since_days").and_then(|v| v.as_i64()).unwrap_or(365);
    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(5000);
    let include_all = params.get("all").and_then(|v| v.as_bool()).unwrap_or(false);

    let world_path = std::path::Path::new(intents_path)
        .parent()
        .map(|p| p.join("world_model.json"))
        .ok_or_else(|| "could not resolve world_model.json sibling path".to_string())?;
    let model = match load(&world_path) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return Err(format!(
                "no world model on disk at {} — run `tracemind world train` first",
                world_path.display()
            ));
        }
        Err(e) => return Err(format!("failed to load world model: {e}")),
    };

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let since = chrono::Utc::now() - chrono::Duration::days(since_days);
    let rows = store
        .list_completed_with_outcome_meta(since, limit)
        .map_err(|e| format!("list_completed_with_outcome_meta: {e}"))?;

    let (pairs, in_sample_skipped) = if include_all {
        let kept = rows.into_iter().map(|(c, p, _)| (c, p)).collect();
        (kept, 0usize)
    } else {
        split_out_of_sample(&model, rows)
    };

    let mut report = evaluate(&model, &pairs);
    report.n_in_sample_skipped = in_sample_skipped;

    serde_json::to_value(&report).map_err(|e| format!("serialize CalibrationReport: {e}"))
}

/// `memory_outcome_proposals` — list active proposals for the agent
/// surface. Mirrors `tracemind outcomes list --json` so the same
/// rows show up in CLI + MCP. TM-INTENT-009.
fn handle_memory_outcome_proposals(
    params: &Value,
    intents_path: &str,
) -> Result<Value, String> {
    use tm_intent::IntentStore;

    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(20);

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let now = chrono::Utc::now();
    // Best-effort sweep so callers see fresh state without a brief.
    let _ = store.expire_outcome_proposals(now);
    let rows = store
        .list_active_outcome_proposals(now, limit)
        .map_err(|e| format!("list_active_outcome_proposals: {e}"))?;

    let proposals: Vec<Value> = rows
        .into_iter()
        .map(|p| {
            // Snapshot the open commitment statement so the MCP
            // response is self-contained — agents shouldn't need a
            // second call just to render the row.
            let stmt = store
                .get_commitment(p.commitment_id)
                .ok()
                .flatten()
                .map(|c| c.statement)
                .unwrap_or_default();
            json!({
                "id": p.id.to_string(),
                "commitment_id": p.commitment_id.to_string(),
                "commitment_statement": stmt,
                "proposed_polarity": match p.proposed_polarity {
                    tm_intent::Polarity::Better => "better",
                    tm_intent::Polarity::AsExpected => "as_expected",
                    tm_intent::Polarity::Worse => "worse",
                    tm_intent::Polarity::Mixed => "mixed",
                    tm_intent::Polarity::NoOutcome => "no_outcome",
                },
                "description": p.description,
                "similarity": p.similarity,
                "proposed_at": p.proposed_at.to_rfc3339(),
                "expires_at": p.expires_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(json!({ "proposals": proposals, "count": proposals.len() }))
}

/// `memory_outcome_accept` — promote a pending proposal into a real
/// `Outcome` row, walk the commitment to Completed, link the new
/// outcome id back onto the proposal. TM-INTENT-009.
fn handle_memory_outcome_accept(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::{state::transition, IntentStore, Outcome, OutcomeSource, State};

    let pid_s = params
        .get("proposal_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: proposal_id".to_string())?;
    let pid = Uuid::parse_str(pid_s)
        .map_err(|e| format!("invalid proposal_id '{pid_s}': {e}"))?;
    let note = params
        .get("note")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;

    let proposal = store
        .get_outcome_proposal(pid)
        .map_err(|e| format!("lookup failed: {e}"))?
        .ok_or_else(|| format!("no proposal with id {pid}"))?;
    if proposal.status != "pending" {
        return Err(format!(
            "proposal {pid} is already {} (no-op)",
            proposal.status
        ));
    }

    let mut commitment = store
        .get_commitment(proposal.commitment_id)
        .map_err(|e| format!("commitment lookup failed: {e}"))?
        .ok_or_else(|| {
            format!(
                "commitment {} not found (proposal references a stale row)",
                proposal.commitment_id
            )
        })?;

    let mut outcome = Outcome::new(
        commitment.id,
        proposal.proposed_polarity,
        &proposal.description,
        OutcomeSource::ImplicitMatched,
    );
    if let Some(n) = note {
        outcome.user_note = Some(n);
    }
    transition(&mut commitment, State::Completed, Some(&outcome))
        .map_err(|e| format!("state transition rejected: {e}"))?;
    store
        .insert_outcome(&outcome)
        .map_err(|e| format!("failed to persist outcome: {e}"))?;
    store
        .update_state(commitment.id, commitment.state, commitment.outcome_id)
        .map_err(|e| format!("failed to update commitment state: {e}"))?;
    let now = chrono::Utc::now();
    if let Err(e) = store.mark_outcome_proposal_accepted(pid, outcome.id, now) {
        // Outcome already landed; surface the bookkeeping error
        // separately rather than rolling back, since the user-visible
        // resolution stuck.
        return Ok(json!({
            "accepted": true,
            "proposal_id": pid.to_string(),
            "commitment_id": commitment.id.to_string(),
            "outcome_id": outcome.id.to_string(),
            "warning": format!("proposal mark-accept failed: {e}"),
        }));
    }
    Ok(json!({
        "accepted": true,
        "proposal_id": pid.to_string(),
        "commitment_id": commitment.id.to_string(),
        "outcome_id": outcome.id.to_string(),
    }))
}

/// `memory_outcome_dismiss` — mark a pending proposal terminal so
/// the brief stops surfacing it. TM-INTENT-009.
fn handle_memory_outcome_dismiss(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::IntentStore;

    let pid_s = params
        .get("proposal_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: proposal_id".to_string())?;
    let pid = Uuid::parse_str(pid_s)
        .map_err(|e| format!("invalid proposal_id '{pid_s}': {e}"))?;

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    let now = chrono::Utc::now();
    let dismissed = store
        .mark_outcome_proposal_dismissed(pid, now)
        .map_err(|e| format!("dismiss failed: {e}"))?;
    if !dismissed {
        return Err(format!(
            "proposal {pid} is not pending (already terminal or unknown)"
        ));
    }
    Ok(json!({ "dismissed": true, "proposal_id": pid.to_string() }))
}

// ---------------------------------------------------------------------------
// Intent arc handlers (Need / Sentiment / Action / Arc)
// ---------------------------------------------------------------------------

fn handle_memory_need(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::{IntentStore, Need, NeedSource};

    let statement = params
        .get("statement")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: statement".to_string())?
        .trim()
        .to_string();
    if statement.is_empty() {
        return Err("statement must be non-empty".to_string());
    }

    let mut need = Need::new(statement, NeedSource::McpStructured);

    if let Some(u) = params.get("urgency").and_then(|v| v.as_f64()) {
        if !(0.0..=1.0).contains(&u) {
            return Err(format!("urgency must be in [0,1], got {u}"));
        }
        need.urgency = u as f32;
    }
    if let Some(r) = params.get("recurring").and_then(|v| v.as_bool()) {
        need.recurring = r;
    }
    if let Some(tags) = params.get("tags").and_then(|v| v.as_array()) {
        need.tags = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.to_string()))
            .collect();
    }

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    store.insert_need(&need).map_err(|e| format!("insert_need: {e}"))?;

    if let Some(cid_s) = params.get("link_commitment").and_then(|v| v.as_str()) {
        let cid = Uuid::parse_str(cid_s)
            .map_err(|e| format!("invalid link_commitment '{cid_s}': {e}"))?;
        store
            .link_need_to_commitment(need.id, cid)
            .map_err(|e| format!("link_need_to_commitment: {e}"))?;
    }

    Ok(json!({
        "need_id": need.id.to_string(),
        "statement": need.statement,
        "urgency": need.urgency,
        "recurring": need.recurring,
    }))
}

fn handle_memory_sentiment(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::{IntentStore, Sentiment, SentimentSource, SentimentTarget};

    let target_id_s = params
        .get("target_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: target_id".to_string())?;
    let target_id = Uuid::parse_str(target_id_s)
        .map_err(|e| format!("invalid target_id '{target_id_s}': {e}"))?;

    let target_type = match params
        .get("target_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: target_type".to_string())?
    {
        "commitment" => SentimentTarget::Commitment,
        "need" => SentimentTarget::Need,
        "entity" => SentimentTarget::Entity,
        "topic" => SentimentTarget::Topic,
        other => return Err(format!("invalid target_type: {other}")),
    };

    let valence = params
        .get("valence")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing required parameter: valence".to_string())? as f32;
    if !(-1.0..=1.0).contains(&valence) {
        return Err(format!("valence must be in [-1,1], got {valence}"));
    }

    let mut sentiment = Sentiment::new(target_id, target_type, valence, SentimentSource::McpStructured);

    if let Some(i) = params.get("intensity").and_then(|v| v.as_f64()) {
        if !(0.0..=1.0).contains(&i) {
            return Err(format!("intensity must be in [0,1], got {i}"));
        }
        sentiment.intensity = i as f32;
    }
    if let Some(et) = params.get("evidence_text").and_then(|v| v.as_str()) {
        sentiment.evidence_text = Some(et.to_string());
    }
    if let Some(tr) = params.get("evidence_trace").and_then(|v| v.as_str()) {
        sentiment.evidence_trace = Some(
            Uuid::parse_str(tr).map_err(|e| format!("invalid evidence_trace '{tr}': {e}"))?,
        );
    }

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    store
        .insert_sentiment(&sentiment)
        .map_err(|e| format!("insert_sentiment: {e}"))?;

    Ok(json!({
        "sentiment_id": sentiment.id.to_string(),
        "target_id": target_id.to_string(),
        "valence": sentiment.valence,
        "intensity": sentiment.intensity,
    }))
}

fn handle_memory_action(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::{Action, ActionModality, ActionSource, IntentStore};

    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: description".to_string())?
        .trim()
        .to_string();
    if description.is_empty() {
        return Err("description must be non-empty".to_string());
    }

    let mut action = Action::new(description.clone(), ActionSource::McpStructured);

    if let Some(cid_s) = params.get("commitment_id").and_then(|v| v.as_str()) {
        action.commitment_id = Some(
            Uuid::parse_str(cid_s)
                .map_err(|e| format!("invalid commitment_id '{cid_s}': {e}"))?,
        );
    }
    if let Some(m) = params.get("modality").and_then(|v| v.as_str()) {
        action.modality = match m {
            "digital" => ActionModality::Digital,
            "physical" => ActionModality::Physical,
            "communication" => ActionModality::Communication,
            "creation" => ActionModality::Creation,
            other => return Err(format!("invalid modality: {other}")),
        };
    }
    if let Some(ev) = params.get("evidence").and_then(|v| v.as_array()) {
        let mut parsed = Vec::with_capacity(ev.len());
        for e in ev {
            let s = e
                .as_str()
                .ok_or_else(|| "evidence entries must be UUID strings".to_string())?;
            parsed.push(
                Uuid::parse_str(s).map_err(|e| format!("invalid evidence uuid '{s}': {e}"))?,
            );
        }
        action.evidence = parsed;
    }

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;
    store
        .insert_action(&action)
        .map_err(|e| format!("insert_action: {e}"))?;

    Ok(json!({
        "action_id": action.id.to_string(),
        "description": action.description,
        "commitment_id": action.commitment_id.map(|u| u.to_string()),
        "modality": format!("{:?}", action.modality).to_lowercase(),
    }))
}

fn handle_memory_arc(params: &Value, intents_path: &str) -> Result<Value, String> {
    use tm_intent::IntentStore;

    let cid_s = params
        .get("commitment_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: commitment_id".to_string())?;
    let cid = Uuid::parse_str(cid_s)
        .map_err(|e| format!("invalid commitment_id '{cid_s}': {e}"))?;

    let sentiment_limit = params
        .get("sentiment_limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(10);
    let action_limit = params
        .get("action_limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(20);

    let store = IntentStore::open(intents_path)
        .map_err(|e| format!("failed to open intent store: {e}"))?;

    let commitment = store
        .get_commitment(cid)
        .map_err(|e| format!("get_commitment: {e}"))?
        .ok_or_else(|| format!("commitment {cid} not found"))?;

    // Gather linked needs
    let all_needs = store.list_needs(500).map_err(|e| format!("list_needs: {e}"))?;
    let linked_needs: Vec<Value> = all_needs
        .into_iter()
        .filter(|n| n.linked_commitments.contains(&cid))
        .map(|n| {
            json!({
                "need_id": n.id.to_string(),
                "statement": n.statement,
                "urgency": n.urgency,
                "recurring": n.recurring,
                "first_seen": n.first_seen.to_rfc3339(),
                "last_seen": n.last_seen.to_rfc3339(),
            })
        })
        .collect();

    // Sentiments on this commitment
    let sentiments: Vec<Value> = store
        .list_sentiments_for(cid, sentiment_limit)
        .map_err(|e| format!("list_sentiments_for: {e}"))?
        .into_iter()
        .map(|s| {
            json!({
                "sentiment_id": s.id.to_string(),
                "valence": s.valence,
                "intensity": s.intensity,
                "captured_at": s.captured_at.to_rfc3339(),
                "evidence_text": s.evidence_text,
            })
        })
        .collect();

    // Actions linked to this commitment
    let actions: Vec<Value> = store
        .list_actions_for_commitment(cid, action_limit)
        .map_err(|e| format!("list_actions_for_commitment: {e}"))?
        .into_iter()
        .map(|a| {
            json!({
                "action_id": a.id.to_string(),
                "description": a.description,
                "taken_at": a.taken_at.to_rfc3339(),
                "modality": format!("{:?}", a.modality).to_lowercase(),
            })
        })
        .collect();

    // Outcome if resolved
    let outcome = commitment
        .outcome_id
        .and_then(|oid| store.get_outcome(oid).ok().flatten())
        .map(|o| {
            json!({
                "outcome_id": o.id.to_string(),
                "polarity": format!("{:?}", o.polarity).to_lowercase(),
                "description": o.description,
                "observed_at": o.observed_at.to_rfc3339(),
            })
        });

    Ok(json!({
        "commitment": {
            "id": commitment.id.to_string(),
            "kind": format!("{:?}", commitment.kind).to_lowercase(),
            "statement": commitment.statement,
            "state": format!("{:?}", commitment.state).to_lowercase(),
            "confidence": commitment.confidence,
            "made_at": commitment.made_at.to_rfc3339(),
            "horizon": commitment.horizon.map(|h| h.to_rfc3339()),
        },
        "needs": linked_needs,
        "sentiments": sentiments,
        "actions": actions,
        "outcome": outcome,
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
    answerer: &Arc<TieredAnswerer>,
    traces: &Arc<Mutex<TraceStore>>,
    recent: &Arc<Mutex<RecentStore>>,
    session_id: Uuid,
    db_path: &str,
    intents_path: &str,
) -> Result<Value, anyhow::Error> {
    match method {
        "initialize" => {
            // MCP-6 (W-7) — propagate the canonical wedge sentence so every
            // host's LLM sees the same framing the first moment it connects.
            // The `instructions` field is part of the MCP 2024-11-05 spec for
            // exactly this kind of server-level priming.
            Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tracemind", "version": "0.1.0" },
                "instructions": "TraceMind is ambient memory for every AI you use — it captures what you do, scopes itself to the right context, learns your boundaries, and never uploads anything off-device. Call memory_store proactively when the user shares anything durable. Call memory_query before answering anything that references the past. Always surface contradictions returned by memory_store — that retraction beat is the point."
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
                    handle_memory_store(&args, ingest, retrieval, traces, recent, session_id, intents_path)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_store_structured" => {
                    handle_memory_store_structured(&args, db_path, traces, recent, session_id)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_query" => {
                    handle_memory_query(&args, retrieval, answerer, db_path, intents_path)
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
                "memory_commit" => {
                    handle_memory_commit(&args, intents_path)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_resolve" => {
                    handle_memory_resolve(&args, intents_path)
                        .await
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_brief" => {
                    handle_memory_brief(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_insight_silence" => {
                    handle_memory_insight_silence(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_insight_unsilence" => {
                    handle_memory_insight_unsilence(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_insight_silences" => {
                    handle_memory_insight_silences(intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_pattern_silence" => {
                    handle_memory_pattern_silence(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_pattern_unsilence" => {
                    handle_memory_pattern_unsilence(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_pattern_silences" => {
                    handle_memory_pattern_silences(intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_world_calibration" => {
                    handle_memory_world_calibration(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_outcome_proposals" => {
                    handle_memory_outcome_proposals(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_outcome_accept" => {
                    handle_memory_outcome_accept(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_outcome_dismiss" => {
                    handle_memory_outcome_dismiss(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_need" => {
                    handle_memory_need(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_sentiment" => {
                    handle_memory_sentiment(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_action" => {
                    handle_memory_action(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_arc" => {
                    handle_memory_arc(&args, intents_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_feedback" => {
                    handle_memory_feedback(&args, db_path)
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
    let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

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

    // Sprint A: build the tiered answerer once for the server lifetime.
    let answerer = Arc::new(answerer::build_answerer());

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
            &answerer,
            &traces,
            &recent,
            session_id,
            &db_path,
            &intents_path,
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
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let session = Uuid::new_v4();

        // First store seeds the graph.
        let first = handle_memory_store(
            &json!({"text": "Apple announced the M4 chip built on TSMC N3E."}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
            &intents_path,
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
            &intents_path,
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

    /// Sprint C / INTENT_SYSTEM.md §3.1 — `memory_store` mines the
    /// captured text for commitment-shaped phrases and persists pending
    /// candidates into the intent store.
    #[tokio::test]
    async fn memory_store_mines_commitment_candidates() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_mined_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();

        let _ = GraphStore::open(&db_path).expect("graph open");

        let ingest = Arc::new(Mutex::new(
            IngestPipeline::open(&db_path, true).expect("ingest open"),
        ));
        let retrieval = Arc::new(Mutex::new(
            RetrievalEngine::open(&db_path, &trace_path, true).expect("retrieval open"),
        ));
        let traces = Arc::new(Mutex::new(
            TraceStore::open(&trace_path).expect("traces open"),
        ));
        let recent = Arc::new(Mutex::new(
            RecentStore::open(&dir.join("recent.jsonl")).expect("recent open"),
        ));
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let session = Uuid::new_v4();

        let resp = handle_memory_store(
            &json!({"text": "I'm going to migrate to Postgres next sprint. I decided to use bcrypt for password hashing."}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
            &intents_path,
        )
        .await
        .expect("memory_store ok");

        // Two phrases ("i'm going to" + "i decided") → 2 candidates.
        assert_eq!(resp["candidates_mined"], json!(2), "expected 2 mined: {resp}");

        // Confirm they're queryable from the intent store.
        let store = tm_intent::IntentStore::open(&intents_path).expect("open intents");
        let pending = store.list_pending_candidates(10).expect("list pending");
        assert_eq!(pending.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Sprint A: typed-schema ingestion path. Stores entities + triples
    /// directly without invoking the heuristic NER, then reads them back via
    /// `GraphStore` to confirm the wire round-trip works.
    #[tokio::test]
    async fn memory_store_structured_writes_and_resolves() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_structured_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let recent_path = dir.join("recent.jsonl");

        // Touch the DB once so the tm-graph schema exists.
        let _ = GraphStore::open(&db_path).expect("graph open");

        let traces = Arc::new(Mutex::new(
            TraceStore::open(&trace_path).expect("traces open"),
        ));
        let recent = Arc::new(Mutex::new(
            RecentStore::open(&recent_path).expect("recent open"),
        ));
        let session = Uuid::new_v4();

        let resp = handle_memory_store_structured(
            &json!({
                "text": "Aaditya works at TraceMind, which depends on Rust.",
                "entities": [
                    {"name": "Aaditya", "type": "person", "confidence": 0.95},
                    {"name": "TraceMind", "type": "project"},
                    {"name": "Rust", "type": "technology"},
                ],
                "triples": [
                    {"subject": "Aaditya", "predicate": "works_at", "object": "TraceMind"},
                    {"subject": "TraceMind", "predicate": "depends_on", "object": "Rust"},
                    // Skipped: object not in entities or graph.
                    {"subject": "Aaditya", "predicate": "owns", "object": "MysteryThing"},
                ]
            }),
            &db_path,
            &traces,
            &recent,
            session,
        )
        .await
        .expect("structured ingest");

        assert_eq!(resp["stored"], json!(true));
        assert_eq!(resp["entities"].as_array().unwrap().len(), 3);
        assert_eq!(resp["triples"], json!(2));
        let skipped = resp["skipped_triples"].as_array().expect("skipped array");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0]["reason"]
            .as_str()
            .unwrap()
            .contains("object not in"));

        // Re-open the graph and confirm the entities are persisted.
        let g = GraphStore::open(&db_path).expect("graph reopen");
        let aad = g
            .find_entity_by_name_icase("Aaditya")
            .unwrap()
            .expect("Aaditya stored");
        assert!(matches!(aad.entity_type, tm_types::EntityType::Person));

        // A second call referencing only-by-name an existing entity should
        // resolve via the graph lookup, not require re-passing it.
        let resp2 = handle_memory_store_structured(
            &json!({
                "entities": [
                    {"name": "Phase 3", "type": "project"},
                ],
                "triples": [
                    {"subject": "Phase 3", "predicate": "part_of", "object": "TraceMind"},
                ]
            }),
            &db_path,
            &traces,
            &recent,
            session,
        )
        .await
        .expect("structured ingest 2");
        assert_eq!(resp2["triples"], json!(1));
        assert!(resp2["skipped_triples"].as_array().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_commit_then_resolve_drives_state_machine() {
        // Sprint B: end-to-end Commitment lifecycle through the MCP handlers.
        // commit (intent) → resolve (better outcome) → commitment is Completed
        // and the outcome row points back to the same id.
        let dir = std::env::temp_dir().join(format!("tm-mcp-intents-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        let commit = handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "ship Sprint B by Friday",
                "horizon": "2026-05-01T17:00:00Z",
                "stakes": "medium",
                "confidence": 0.8,
                "tags": ["sprint-b"]
            }),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let cid = commit["commitment_id"].as_str().expect("id").to_string();
        assert_eq!(commit["state"], json!("open"));
        assert_eq!(commit["kind"], json!("intent"));

        let resolved = handle_memory_resolve(
            &json!({
                "commitment_id": cid,
                "polarity": "better",
                "description": "shipped Wednesday"
            }),
            &intents_path,
        )
        .await
        .expect("resolve ok");

        assert_eq!(resolved["commitment_id"].as_str().unwrap(), cid);
        assert_eq!(resolved["commitment_state"], json!("completed"));
        assert_eq!(resolved["polarity"], json!("better"));
        assert!(resolved["outcome_id"].as_str().is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_store_surfaces_outcome_proposals_for_matching_text() {
        // Sprint D / INTENT_SYSTEM.md §4.2: when the new ingested text
        // overlaps an open Commitment's statement, memory_store
        // returns `outcome_proposals` so the agent can suggest
        // `memory_resolve`.
        let dir = std::env::temp_dir().join(format!("tm-mcp-match-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        // Seed an open commitment.
        handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "ship the locomo report by friday"
            }),
            &intents_path,
        )
        .await
        .expect("commit ok");

        // Build the rest of the harness for memory_store.
        let db_dir = std::env::temp_dir().join(format!("tm-mcp-store-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = db_dir.join("traces.jsonl").to_str().unwrap().to_string();
        let recent_path = db_dir.join("recent.jsonl").to_str().unwrap().to_string();

        let pipeline = IngestPipeline::open(&db_path, /* hash_embed */ true).expect("ingest");
        let engine = RetrievalEngine::open(&db_path, &trace_path, /* hash_embed */ true)
            .expect("retrieval");
        let traces = TraceStore::open(&trace_path).expect("traces");
        let recent = RecentStore::open_with_capacity(&recent_path, 100).expect("recent");

        let ingest_arc = Arc::new(Mutex::new(pipeline));
        let engine_arc = Arc::new(Mutex::new(engine));
        let traces_arc = Arc::new(Mutex::new(traces));
        let recent_arc = Arc::new(Mutex::new(recent));

        let result = handle_memory_store(
            &json!({ "text": "shipped the locomo report this morning" }),
            &ingest_arc,
            &engine_arc,
            &traces_arc,
            &recent_arc,
            Uuid::new_v4(),
            &intents_path,
        )
        .await
        .expect("memory_store ok");

        let proposals = result["outcome_proposals"]
            .as_array()
            .expect("proposals array");
        assert!(
            !proposals.is_empty(),
            "expected at least one outcome proposal, got: {result}"
        );
        let p = &proposals[0];
        assert_eq!(p["polarity_hint"], json!("better"));
        assert!(
            p["score"].as_f64().unwrap() >= 0.25,
            "score = {}",
            p["score"]
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&db_dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_returns_open_overdue_resolved_and_candidates() {
        // Sprint D: end-to-end brief surface. We exercise the read path
        // by seeding an intent store via the public MCP handlers (commit
        // + resolve), then assert the brief reflects state.
        let dir = std::env::temp_dir().join(format!("tm-mcp-brief-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        // 1. open commitment with future horizon → ends up in `open`
        let _open = handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "future work",
                "horizon": "2099-01-01T00:00:00Z",
                "stakes": "medium"
            }),
            &intents_path,
        )
        .await
        .expect("open commit ok");

        // 2. open commitment with past horizon → `overdue`
        let _overdue = handle_memory_commit(
            &json!({
                "kind": "decision",
                "statement": "should have been resolved",
                "horizon": "2000-01-01T00:00:00Z",
                "stakes": "high"
            }),
            &intents_path,
        )
        .await
        .expect("overdue commit ok");

        // 3. commit + resolve → `resolved` section
        let resolvable = handle_memory_commit(
            &json!({ "kind": "intent", "statement": "ship X" }),
            &intents_path,
        )
        .await
        .expect("commit ok");
        handle_memory_resolve(
            &json!({
                "commitment_id": resolvable["commitment_id"].as_str().unwrap(),
                "polarity": "as_expected",
                "description": "fine"
            }),
            &intents_path,
        )
        .await
        .expect("resolve ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let counts = &brief["counts"];
        assert_eq!(counts["open"], 1, "one future-horizon commitment in open");
        assert_eq!(counts["overdue"], 1, "one past-horizon commitment in overdue");
        assert_eq!(counts["resolved"], 1, "one completed commitment in resolved");

        // The overdue row carries an `overdue_class` bucket — we don't
        // assert which (DueToday vs Stale depends on the test's wall
        // clock); we just assert it's present.
        assert!(
            brief["overdue"][0]["overdue_class"].is_string(),
            "overdue rows must carry an overdue_class"
        );
        assert_eq!(
            brief["resolved"][0]["polarity"],
            json!("as_expected"),
            "resolved row carries polarity"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_commit_omits_preflight_when_no_world_model() {
        // Without a world_model.json sibling, the response must NOT
        // include a `preflight` field — agents check for its presence
        // to decide whether to render a track-record line.
        let dir = std::env::temp_dir().join(format!("tm-mcp-no-world-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        let resp = handle_memory_commit(
            &json!({"kind": "intent", "statement": "no world model present"}),
            &intents_path,
        )
        .await
        .expect("commit ok");

        assert!(resp["preflight"].is_null(), "preflight must be absent when model missing");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_commit_attaches_preflight_when_model_trained() {
        // When a trained, well-supported world model is on disk, the
        // commit response carries a preflight block with the user's
        // prior distribution. Agents can route on `tone`.
        use tm_intent::{Commitment, CommitmentKind, Source, Stakes};
        use tm_world_model::{save, train, Example, OutcomeModel, PolarityClass, TagVocab, TrainerConfig};

        let dir = std::env::temp_dir().join(format!("tm-mcp-preflight-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Hand-build a separable training set: high-stakes always Worse.
        let mut examples = Vec::new();
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
            c.stakes = Stakes::High;
            examples.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "y", Source::Cli);
            c.stakes = Stakes::Low;
            examples.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let cfg = TrainerConfig::default();
        let (model, _report): (OutcomeModel, _) = train(&examples, &cfg);
        // Use the workspace TagVocab default for vocab consistency check.
        let _ = TagVocab::default();
        save(&model, &world_path).expect("save world model");

        let resp = handle_memory_commit(
            &json!({"kind": "intent", "statement": "ship migration", "stakes": "high"}),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let pre = &resp["preflight"];
        assert!(!pre.is_null(), "preflight must be present after training");
        assert_eq!(pre["argmax"], json!("worse"));
        assert_eq!(pre["tone"], json!("warning"));
        let n = pre["n_priors"].as_u64().unwrap();
        assert_eq!(n, 12);
        let dist_worse = pre["dist"]["worse"].as_f64().unwrap();
        assert!(dist_worse > 0.5, "high-stakes probe should lean worse, got {dist_worse}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_resolve_auto_retrains_world_model_when_enough_priors() {
        // After 6 resolved commitments, calling memory_resolve a 7th
        // time should produce a `world_model` status string AND leave
        // a freshly-written `world_model.json` next to the intents db.
        let dir = std::env::temp_dir().join(format!("tm-mcp-retrain-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Seed + resolve 6 commitments (the trainer's `min_examples`).
        // The 6th resolve is the one that crosses the threshold and
        // therefore must trigger a retrain.
        let mut last_resp = json!({});
        for i in 0..6 {
            let commit = handle_memory_commit(
                &json!({
                    "kind": "intent",
                    "statement": format!("seed-{i}"),
                    "stakes": if i % 2 == 0 { "high" } else { "low" },
                }),
                &intents_path,
            )
            .await
            .expect("commit ok");
            let cid = commit["commitment_id"].as_str().expect("id").to_string();
            let pol = if i % 2 == 0 { "worse" } else { "better" };
            last_resp = handle_memory_resolve(
                &json!({
                    "commitment_id": cid,
                    "polarity": pol,
                    "description": "seed outcome"
                }),
                &intents_path,
            )
            .await
            .expect("resolve ok");
        }

        let msg = last_resp["world_model"].as_str();
        assert!(
            msg.is_some(),
            "6th resolve must trigger an auto-retrain; got resp = {last_resp}"
        );
        assert!(
            msg.unwrap().contains("retrained on 6 priors"),
            "expected '6 priors' in status, got: {msg:?}"
        );
        assert!(
            world_path.exists(),
            "auto-retrain must persist world_model.json"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_resolve_skips_retrain_silently_below_min_examples() {
        // First resolve (n=1) is well under min_examples=6: response
        // must NOT carry a `world_model` field (silent skip), and no
        // file should appear on disk.
        let dir = std::env::temp_dir().join(format!("tm-mcp-skipretrain-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        let commit = handle_memory_commit(
            &json!({"kind": "intent", "statement": "first ever"}),
            &intents_path,
        )
        .await
        .expect("commit ok");
        let cid = commit["commitment_id"].as_str().unwrap().to_string();
        let resp = handle_memory_resolve(
            &json!({
                "commitment_id": cid,
                "polarity": "better",
                "description": "shipped"
            }),
            &intents_path,
        )
        .await
        .expect("resolve ok");

        assert!(
            resp.get("world_model").is_none(),
            "below min_examples → no world_model status should be emitted"
        );
        assert!(
            !world_path.exists(),
            "below min_examples → no world_model.json should be written"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_attaches_outlook_to_open_rows_when_model_trained() {
        // With a trained world_model.json next to intents.db, the
        // brief response's `open[*].outlook` block must be populated.
        // Without it (or below min_priors), `outlook` must be omitted.
        use tm_intent::{Commitment, CommitmentKind, Source, Stakes};
        use tm_world_model::{save, train, Example, PolarityClass, TrainerConfig};

        let dir = std::env::temp_dir().join(format!("tm-mcp-brief-outlook-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Train a separable model and persist it.
        let mut examples = Vec::new();
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
            c.stakes = Stakes::High;
            examples.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "y", Source::Cli);
            c.stakes = Stakes::Low;
            examples.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let (model, _r) = train(&examples, &TrainerConfig::default());
        save(&model, &world_path).expect("save model");

        // Open commitment that should match the worse cell.
        handle_memory_commit(
            &json!({"kind": "intent", "statement": "ship migration", "stakes": "high"}),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let open = brief["open"].as_array().expect("open array");
        assert_eq!(open.len(), 1);
        let outlook = &open[0]["outlook"];
        assert!(!outlook.is_null(), "outlook must be present, got brief={brief}");
        assert_eq!(outlook["argmax"], json!("worse"));
        assert_eq!(outlook["tone"], json!("warning"));
        assert!(outlook["n_priors"].as_u64().unwrap() >= 6);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_omits_outlook_when_no_world_model() {
        let dir = std::env::temp_dir().join(format!("tm-mcp-brief-cold-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        handle_memory_commit(
            &json!({"kind": "intent", "statement": "no model present"}),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let open = brief["open"].as_array().expect("open array");
        assert_eq!(open.len(), 1);
        assert!(
            open[0].get("outlook").is_none(),
            "no model on disk → outlook field must be omitted (skip_serializing_if), got {:?}",
            open[0]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_surfaces_insights_when_open_row_diverges_from_baseline() {
        // With a trained world model AND a completed-rate baseline that
        // *diverges* from the model's per-row outlook, memory_brief
        // must surface the divergence in the top-level `insights` array
        // and bump `counts.insights`. Mirrors the tm-reflect integration
        // test, but exercises the full MCP JSON shape.
        use tm_world_model::{save, train, Example, PolarityClass, TrainerConfig};
        use uuid::Uuid as TestUuid;

        let dir = std::env::temp_dir().join(format!("tm-mcp-brief-insights-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // 1. Train a separable model: high stakes → worse, low stakes → better.
        let mut examples = Vec::new();
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "x",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::High;
            examples.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "y",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::Low;
            examples.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let (model, _r) = train(&examples, &TrainerConfig::default());
        save(&model, &world_path).expect("save model");

        // 2. Seed completed commitments to establish a *positive-leaning*
        //    baseline (≥ 6 priors so insights detector trusts it).
        //    8 better + 4 worse = 67% positive baseline.
        for i in 0..8 {
            let commit = handle_memory_commit(
                &json!({
                    "kind": "intent",
                    "statement": format!("low-stakes win {i}"),
                    "stakes": "low",
                }),
                &intents_path,
            )
            .await
            .expect("commit ok");
            let cid = commit["commitment_id"].as_str().unwrap().to_string();
            handle_memory_resolve(
                &json!({"commitment_id": cid, "polarity": "better", "description": "shipped"}),
                &intents_path,
            )
            .await
            .expect("resolve ok");
        }
        for i in 0..4 {
            let commit = handle_memory_commit(
                &json!({
                    "kind": "intent",
                    "statement": format!("high-stakes loss {i}"),
                    "stakes": "high",
                }),
                &intents_path,
            )
            .await
            .expect("commit ok");
            let cid = commit["commitment_id"].as_str().unwrap().to_string();
            handle_memory_resolve(
                &json!({"commitment_id": cid, "polarity": "worse", "description": "missed"}),
                &intents_path,
            )
            .await
            .expect("resolve ok");
        }

        // The auto-retrain on the last resolve overwrites our hand-crafted
        // separable model with one trained on the seeded baseline. Re-save
        // the separable model so the open-row outlook is sharply skewed.
        save(&model, &world_path).expect("re-save separable model");

        // 3. Open commitment that the model thinks is *worse* (high stakes)
        //    while the baseline is positive-leaning → must surface as a
        //    warning insight.
        handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "ship risky vendor migration",
                "stakes": "high",
            }),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let insights = brief["insights"].as_array().expect("insights array present");
        assert!(
            !insights.is_empty(),
            "model + diverging baseline must surface ≥ 1 insight, got brief={brief}"
        );
        let first = &insights[0];
        assert_eq!(first["tone"], json!("warning"));
        assert!(first["delta"].as_f64().unwrap() < -0.20);
        assert!(first["render"].as_str().unwrap().contains("riskier"));
        assert_eq!(brief["counts"]["insights"], json!(insights.len()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_omits_insights_when_no_world_model() {
        // Without a trained world model on disk, the insights array
        // must be empty (insight detection requires per-row outlooks).
        use uuid::Uuid as TestUuid;

        let dir = std::env::temp_dir().join(format!("tm-mcp-brief-noinsights-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        handle_memory_commit(
            &json!({"kind": "intent", "statement": "no model present"}),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let insights = brief["insights"].as_array().expect("insights array present");
        assert!(insights.is_empty(), "no model → no insights, got {brief}");
        assert_eq!(brief["counts"]["insights"], json!(0));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_brief_quiets_insights_when_model_uncalibrated() {
        // TM-INTENT-010: when the world model is attached but has no
        // out-of-sample completions yet (n_evaluated < min floor),
        // memory_brief must surface a `model_quiet` reason of
        // `insufficient_evaluations` AND keep the insights array
        // empty. This is the cold-start surface for a fresh user
        // who's just trained a model but hasn't resolved anything.
        use tm_world_model::{save, train, Example, PolarityClass, TrainerConfig};
        use uuid::Uuid as TestUuid;

        let dir = std::env::temp_dir()
            .join(format!("tm-mcp-brief-quiet-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Train + save a model — but no resolved commitments exist
        // yet, so calibration will see zero out-of-sample pairs.
        let mut examples = Vec::new();
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "x",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::High;
            examples.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "y",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::Low;
            examples.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let (model, _r) = train(&examples, &TrainerConfig::default());
        save(&model, &world_path).expect("save model");

        // One open commitment that the model would otherwise flag.
        handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "ship risky vendor migration",
                "stakes": "high",
            }),
            &intents_path,
        )
        .await
        .expect("commit ok");

        let brief = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        let insights = brief["insights"].as_array().expect("insights array present");
        assert!(
            insights.is_empty(),
            "untrustworthy model must suppress insights, got {brief}"
        );
        let quiet = brief
            .get("model_quiet")
            .expect("model_quiet field present when gate engages");
        assert_eq!(quiet["kind"], json!("insufficient_evaluations"));
        assert_eq!(quiet["n_evaluated"], json!(0));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_insight_silence_round_trips_via_mcp() {
        // memory_insight_silence + memory_insight_silences + brief
        // must agree: silenced commitment shows up in the silences
        // list AND the brief's insights array is filtered.
        use tm_world_model::{save, train, Example, PolarityClass, TrainerConfig};
        use uuid::Uuid as TestUuid;

        let dir = std::env::temp_dir().join(format!("tm-mcp-insilence-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Persist a separable model.
        let mut examples = Vec::new();
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "x",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::High;
            examples.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = tm_intent::Commitment::new(
                tm_intent::CommitmentKind::Intent,
                "y",
                tm_intent::Source::Cli,
            );
            c.stakes = tm_intent::Stakes::Low;
            examples.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let (model, _r) = train(&examples, &TrainerConfig::default());
        save(&model, &world_path).expect("save model");

        // Build a positive baseline (8 better / 4 worse) so the
        // high-stakes open row diverges enough to surface as warning.
        for i in 0..8 {
            let commit = handle_memory_commit(
                &json!({
                    "kind": "intent",
                    "statement": format!("low-stakes win {i}"),
                    "stakes": "low",
                }),
                &intents_path,
            )
            .await
            .expect("commit ok");
            let cid = commit["commitment_id"].as_str().unwrap().to_string();
            handle_memory_resolve(
                &json!({"commitment_id": cid, "polarity": "better", "description": "shipped"}),
                &intents_path,
            )
            .await
            .expect("resolve ok");
        }
        for i in 0..4 {
            let commit = handle_memory_commit(
                &json!({
                    "kind": "intent",
                    "statement": format!("high-stakes loss {i}"),
                    "stakes": "high",
                }),
                &intents_path,
            )
            .await
            .expect("commit ok");
            let cid = commit["commitment_id"].as_str().unwrap().to_string();
            handle_memory_resolve(
                &json!({"commitment_id": cid, "polarity": "worse", "description": "missed"}),
                &intents_path,
            )
            .await
            .expect("resolve ok");
        }
        // Restore the separable model — the auto-retrain on the last
        // resolve will have overwritten it with one trained on the
        // baseline (which is mixed and won't fire a sharp insight).
        save(&model, &world_path).expect("re-save separable model");

        // Open commitment that the model flags as worse.
        let commit = handle_memory_commit(
            &json!({
                "kind": "intent",
                "statement": "ship risky migration",
                "stakes": "high",
            }),
            &intents_path,
        )
        .await
        .expect("commit ok");
        let cid_str = commit["commitment_id"].as_str().unwrap().to_string();

        // Sanity: pre-silence brief surfaces the warning.
        let pre = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        assert!(
            !pre["insights"].as_array().unwrap().is_empty(),
            "pre-silence: insight must surface"
        );

        // Silence via MCP.
        let silenced = handle_memory_insight_silence(
            &json!({"commitment_id": cid_str, "days": 30, "reason": "I get it"}),
            &intents_path,
        )
        .expect("silence ok");
        assert_eq!(silenced["ok"], json!(true));
        assert_eq!(silenced["days"], json!(30));

        // Listing must show the silence.
        let listed = handle_memory_insight_silences(&intents_path).expect("list ok");
        let arr = listed["silences"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["commitment_id"], json!(cid_str));
        assert_eq!(arr[0]["reason"], json!("I get it"));

        // Brief now omits the insight even though the row is still open.
        let post = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        assert!(
            post["insights"].as_array().unwrap().is_empty(),
            "post-silence: insight must be filtered, got {post}"
        );
        assert!(
            post["open"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == json!(cid_str)),
            "open row must still be present after silencing the insight surface"
        );

        // Unsilence — round-trip back to surfaced.
        let removed = handle_memory_insight_unsilence(
            &json!({"commitment_id": cid_str}),
            &intents_path,
        )
        .expect("unsilence ok");
        assert_eq!(removed["removed"], json!(true));
        let brief2 = handle_memory_brief(&json!({}), &intents_path).expect("brief ok");
        assert!(
            !brief2["insights"].as_array().unwrap().is_empty(),
            "after unsilence: insight must resurface"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_insight_unsilence_returns_false_when_nothing_to_remove() {
        use uuid::Uuid as TestUuid;
        let dir = std::env::temp_dir().join(format!("tm-mcp-noopunsilence-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        // Force schema init.
        let _ = tm_intent::IntentStore::open(&intents_path).expect("open ok");

        let cid = TestUuid::new_v4().to_string();
        let r = handle_memory_insight_unsilence(
            &json!({"commitment_id": cid}),
            &intents_path,
        )
        .expect("unsilence ok");
        assert_eq!(r["removed"], json!(false));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_pattern_silence_round_trips_via_mcp() {
        // Pattern-silence parity test: storage already covered by
        // tm-intent's tests, but MCP tools need their own coverage so
        // the JSON contract doesn't drift.
        use uuid::Uuid as TestUuid;
        let dir = std::env::temp_dir().join(format!("tm-mcp-patsilence-{}", TestUuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let _ = tm_intent::IntentStore::open(&intents_path).expect("open ok");

        let resp = handle_memory_pattern_silence(
            &json!({
                "cell_hash": "abc123def4567890",
                "cell_label": "stakes=high · evening · vendor",
                "days": 60,
                "reason": "noisy",
            }),
            &intents_path,
        )
        .expect("silence ok");
        assert_eq!(resp["ok"], json!(true));
        assert_eq!(resp["days"], json!(60));

        let listed = handle_memory_pattern_silences(&intents_path).expect("list ok");
        let arr = listed["silences"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["cell_hash"], json!("abc123def4567890"));
        assert_eq!(arr[0]["cell_label"], json!("stakes=high · evening · vendor"));
        assert_eq!(arr[0]["reason"], json!("noisy"));

        let removed = handle_memory_pattern_unsilence(
            &json!({"cell_hash": "abc123def4567890"}),
            &intents_path,
        )
        .expect("unsilence ok");
        assert_eq!(removed["removed"], json!(true));

        let listed2 = handle_memory_pattern_silences(&intents_path).expect("list ok");
        assert!(listed2["silences"].as_array().unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_world_calibration_errors_when_no_model_on_disk() {
        // Without `world_model.json` next to the intent store, the
        // calibration tool must surface a clear "train first" error
        // — never silently return a degenerate report.
        let dir = std::env::temp_dir().join(format!("tm-mcp-calibration-no-model-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let _ = tm_intent::IntentStore::open(&intents_path).expect("open ok");

        let err = handle_memory_world_calibration(&json!({}), &intents_path)
            .expect_err("expected a no-model error");
        assert!(
            err.contains("no world model on disk"),
            "error should mention missing world model, got: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn memory_world_calibration_returns_full_report_with_all_flag() {
        // With a trained model on disk + a couple of resolved
        // commitments in the intent store, `--all` (which bypasses
        // the trained_at cutoff) should return a complete, parseable
        // CalibrationReport over those rows.
        use tm_intent::{Commitment, CommitmentKind, Source, Stakes};
        use tm_world_model::{save, train, Example, OutcomeModel, PolarityClass, TrainerConfig};

        let dir = std::env::temp_dir().join(format!("tm-mcp-calibration-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
        let world_path = dir.join("world_model.json");

        // Train a separable model: high-stakes → Worse, low-stakes → Better.
        let mut training = Vec::new();
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
            c.stakes = Stakes::High;
            training.push(Example { commitment: c, target: PolarityClass::Worse });
        }
        for _ in 0..6 {
            let mut c = Commitment::new(CommitmentKind::Intent, "y", Source::Cli);
            c.stakes = Stakes::Low;
            training.push(Example { commitment: c, target: PolarityClass::Better });
        }
        let (model, _report): (OutcomeModel, _) = train(&training, &TrainerConfig::default());
        save(&model, &world_path).expect("save world model");

        // Seed a couple of completed commitments (one of each polarity)
        // through the public MCP commit/resolve handlers.
        let high = handle_memory_commit(
            &json!({"kind": "intent", "statement": "ship risky migration", "stakes": "high"}),
            &intents_path,
        )
        .await
        .expect("commit ok");
        handle_memory_resolve(
            &json!({
                "commitment_id": high["commitment_id"].as_str().unwrap(),
                "polarity": "worse",
                "description": "rolled back"
            }),
            &intents_path,
        )
        .await
        .expect("resolve ok");

        let low = handle_memory_commit(
            &json!({"kind": "intent", "statement": "tidy up README", "stakes": "low"}),
            &intents_path,
        )
        .await
        .expect("commit ok");
        handle_memory_resolve(
            &json!({
                "commitment_id": low["commitment_id"].as_str().unwrap(),
                "polarity": "better",
                "description": "merged"
            }),
            &intents_path,
        )
        .await
        .expect("resolve ok");

        // `--all` skips the OOS filter, so both rows are scored even
        // though their `observed_at` is after `trained_at`.
        let report = handle_memory_world_calibration(
            &json!({"all": true, "since_days": 365}),
            &intents_path,
        )
        .expect("calibration ok");

        assert_eq!(report["n_evaluated"], json!(2));
        assert_eq!(report["n_in_sample_skipped"], json!(0));
        assert_eq!(report["n_no_outcome_skipped"], json!(0));
        // All four classes are present in per_class even when only two
        // were observed — surface code can render zeros without
        // tripping over missing keys.
        let per = report["per_class"].as_array().expect("per_class is an array");
        assert_eq!(per.len(), 4);
        let labels: Vec<&str> = per.iter().map(|e| e["label"].as_str().unwrap()).collect();
        assert!(labels.contains(&"better"));
        assert!(labels.contains(&"as_expected"));
        assert!(labels.contains(&"worse"));
        assert!(labels.contains(&"mixed"));
        // Trained model on a separable problem should classify both rows correctly.
        assert!(
            (report["accuracy"].as_f64().unwrap() - 1.0).abs() < 1e-3,
            "separable trained model should score perfect on these two rows, got {}",
            report["accuracy"]
        );
        assert!(report["trained_at"].is_string());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// TM-INTENT-009 — capture text that overlaps an open commitment
    /// must persist a proposal that surfaces in `memory_outcome_proposals`.
    #[tokio::test]
    async fn memory_outcome_proposals_round_trips_via_capture() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_proposals_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        let _ = GraphStore::open(&db_path).expect("graph open");
        let ingest = Arc::new(Mutex::new(
            IngestPipeline::open(&db_path, true).expect("ingest open"),
        ));
        let retrieval = Arc::new(Mutex::new(
            RetrievalEngine::open(&db_path, &trace_path, true).expect("retrieval open"),
        ));
        let traces = Arc::new(Mutex::new(
            TraceStore::open(&trace_path).expect("traces open"),
        ));
        let recent = Arc::new(Mutex::new(
            RecentStore::open(&dir.join("recent.jsonl")).expect("recent open"),
        ));
        let session = Uuid::new_v4();

        // Open a commitment via memory_commit so the matcher has a
        // target. Statement crafted to share content tokens with the
        // capture below.
        let commit = handle_memory_commit(
            &json!({"kind": "intent", "statement": "ship the locomo report by friday"}),
            &intents_path,
        )
        .await
        .expect("commit ok");
        let cid = commit["commitment_id"].as_str().unwrap().to_string();

        // Capture text that overlaps + carries a polarity phrase.
        let store_resp = handle_memory_store(
            &json!({"text": "shipped the locomo report this morning, sent to the team"}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
            &intents_path,
        )
        .await
        .expect("memory_store ok");

        let inline = store_resp["outcome_proposals"]
            .as_array()
            .expect("outcome_proposals is an array");
        assert!(!inline.is_empty(), "expected at least one proposal: {store_resp}");
        // Inline proposal carries the persisted id when we had a polarity hint.
        let inline_first = &inline[0];
        assert_eq!(inline_first["polarity_hint"], json!("better"));
        assert!(inline_first["id"].is_string(), "persisted proposal must include id");

        // memory_outcome_proposals returns the same row.
        let listed =
            handle_memory_outcome_proposals(&json!({}), &intents_path).expect("list ok");
        let proposals = listed["proposals"]
            .as_array()
            .expect("proposals array");
        assert_eq!(proposals.len(), 1);
        let row = &proposals[0];
        assert_eq!(row["commitment_id"].as_str().unwrap(), cid);
        assert_eq!(row["proposed_polarity"], json!("better"));

        let pid = row["id"].as_str().unwrap().to_string();

        // Accept it: outcome row created, proposal becomes terminal,
        // commitment goes to Completed.
        let accepted =
            handle_memory_outcome_accept(&json!({"proposal_id": pid}), &intents_path)
                .expect("accept ok");
        assert_eq!(accepted["accepted"], json!(true));
        assert!(accepted["outcome_id"].is_string());

        // Active list is now empty.
        let after =
            handle_memory_outcome_proposals(&json!({}), &intents_path).expect("list ok");
        assert_eq!(after["count"], json!(0));

        // Re-accept on terminal proposal must error.
        assert!(
            handle_memory_outcome_accept(&json!({"proposal_id": pid}), &intents_path).is_err(),
            "second accept on terminal proposal should error"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// TM-INTENT-009 — dismiss removes a proposal from the active
    /// list without touching the underlying commitment.
    #[tokio::test]
    async fn memory_outcome_dismiss_marks_terminal_only() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_dismiss_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();
        let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();
        let intents_path = dir.join("intents.db").to_str().unwrap().to_string();

        let _ = GraphStore::open(&db_path).expect("graph open");
        let ingest = Arc::new(Mutex::new(
            IngestPipeline::open(&db_path, true).expect("ingest open"),
        ));
        let retrieval = Arc::new(Mutex::new(
            RetrievalEngine::open(&db_path, &trace_path, true).expect("retrieval open"),
        ));
        let traces = Arc::new(Mutex::new(
            TraceStore::open(&trace_path).expect("traces open"),
        ));
        let recent = Arc::new(Mutex::new(
            RecentStore::open(&dir.join("recent.jsonl")).expect("recent open"),
        ));
        let session = Uuid::new_v4();

        let commit = handle_memory_commit(
            &json!({"kind": "intent", "statement": "ship the locomo report this week"}),
            &intents_path,
        )
        .await
        .expect("commit ok");
        let cid = commit["commitment_id"].as_str().unwrap().to_string();

        let _ = handle_memory_store(
            &json!({"text": "shipped the locomo report finally"}),
            &ingest,
            &retrieval,
            &traces,
            &recent,
            session,
            &intents_path,
        )
        .await
        .expect("memory_store ok");

        let listed =
            handle_memory_outcome_proposals(&json!({}), &intents_path).expect("list ok");
        let pid = listed["proposals"][0]["id"].as_str().unwrap().to_string();

        let dismissed =
            handle_memory_outcome_dismiss(&json!({"proposal_id": pid}), &intents_path)
                .expect("dismiss ok");
        assert_eq!(dismissed["dismissed"], json!(true));

        // Active list now empty.
        let after =
            handle_memory_outcome_proposals(&json!({}), &intents_path).expect("list ok");
        assert_eq!(after["count"], json!(0));

        // Commitment should still be Open (dismiss doesn't resolve).
        let store = tm_intent::IntentStore::open(&intents_path).expect("open intents");
        let c = store
            .get_commitment(Uuid::parse_str(&cid).unwrap())
            .unwrap()
            .unwrap();
        assert!(matches!(c.state, tm_intent::State::Open));

        std::fs::remove_dir_all(&dir).ok();
    }
}
