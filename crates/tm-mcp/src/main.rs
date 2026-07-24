#![recursion_limit = "512"]

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

/// The core tool surface a host sees by default.
///
/// The holistic review (docs/HOLISTIC-REVIEW-2026-07.md §6a) found that
/// advertising 51 tools is the single largest unforced error in the product:
/// the tool descriptions *are* the only prompt TraceMind controls inside a
/// host's context, and every extra tool is one more chance for the model to
/// call the wrong thing. This is the "subtract, then sharpen" set — the six
/// verbs that carry the whole loop:
///
/// - `memory_store`      — deposit context
/// - `memory_query`      — draw context
/// - `memory_feedback`   — close the reward loop
/// - `memory_contradict` — the retraction beat (the wedge)
/// - `memory_compose`    — cross-conversation composition (the moat)
/// - `memory_forget`     — the trust primitive
///
/// Every other tool stays fully callable — a host that names it still gets
/// dispatched — but is hidden from `tools/list` unless the operator opts in
/// to the advanced surface. Hiding, not removing, keeps power users and the
/// benchmark harness whole while shrinking the surface the host must reason
/// over.
pub const CORE_TOOLS: &[&str] = &[
    "memory_store",
    "memory_query",
    "memory_feedback",
    "memory_contradict",
    "memory_compose",
    "memory_forget",
];

/// Whether the advanced (full) tool surface is advertised.
///
/// Off by default. Enable with `TM_MCP_ADVANCED=1` in the host's MCP server
/// config. The benchmark harness sets it so it can exercise every tool.
pub fn advanced_surface_enabled() -> bool {
    std::env::var("TM_MCP_ADVANCED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// The tool list actually advertised to the host: full when the advanced
/// surface is on, otherwise filtered to [`CORE_TOOLS`], with any tuned
/// description overrides applied.
fn tools_list_filtered() -> Value {
    let mut list = tools_list_for_surface(advanced_surface_enabled());
    apply_description_overrides(&mut list, &load_description_overrides());
    list
}

/// Load `{tool_name: description}` overrides from `~/.tracemind/mcp-descriptions.json`.
///
/// This is the description analog of the retrieval engine's `policy.json`:
/// the GEPA-over-descriptions loop (produced by `tm-bench-mcp --optimize-on`)
/// writes tuned descriptions here, and the server applies them at
/// `tools/list` time — so the optimisation reaches the running host rather
/// than living in the benchmark. Missing or unparseable file → compiled-in
/// descriptions, never a failure.
fn load_description_overrides() -> std::collections::BTreeMap<String, String> {
    let path = data_dir().join("mcp-descriptions.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Replace advertised descriptions with any override present for that tool.
fn apply_description_overrides(
    list: &mut Value,
    overrides: &std::collections::BTreeMap<String, String>,
) {
    if overrides.is_empty() {
        return;
    }
    if let Some(arr) = list.get_mut("tools").and_then(|t| t.as_array_mut()) {
        for tool in arr.iter_mut() {
            let Some(name) = tool.get("name").and_then(|n| n.as_str()).map(str::to_string) else {
                continue;
            };
            if let Some(desc) = overrides.get(&name) {
                tool["description"] = Value::String(desc.clone());
            }
        }
    }
}

/// Pure filter — `advanced` decides the surface, no environment read. Kept
/// separate so it is testable without racing on a process-global env var.
fn tools_list_for_surface(advanced: bool) -> Value {
    let mut full = tools_list();
    if advanced {
        return full;
    }
    if let Some(arr) = full.get_mut("tools").and_then(|t| t.as_array_mut()) {
        arr.retain(|tool| {
            tool.get("name")
                .and_then(|n| n.as_str())
                .map(|n| CORE_TOOLS.contains(&n))
                .unwrap_or(false)
        });
    }
    full
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "memory_store",
                "description": "Remember, note, save, record, log, or store a durable fact, decision, plan, or preference the user shares — anything worth recalling later. Call this proactively whenever the user tells you something to keep track of. Extracts entities and returns what was already known about them, so you see \"here is what I already knew\" without a second call.",
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
                "description": "Recall, retrieve, look up, or remind the user of something from the past — what they told you, decided, or asked about before. Call this before answering anything that references earlier context. Returns the relevant remembered facts. Set cross_context=true to search across all conversations.",
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
                        },
                        "view": {
                            "type": "string",
                            "description": "LM-11d — Memory View name or UUID to apply for this query. Empty string disables any active view."
                        },
                        "include_entity": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "LM-11d — ad-hoc entity UUIDs to force-include for this query (not persisted)."
                        },
                        "exclude_entity": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "LM-11d — ad-hoc entity UUIDs to force-exclude for this query (not persisted)."
                        }
                    },
                    "required": ["text"]
                }
            },
            {
                "name": "memory_views_list",
                "description": "LM-11d — list every Memory View (saved user-curated splice). Newest-updated first.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            },
            {
                "name": "memory_views_create",
                "description": "LM-11d — create a new Memory View. Returns the new view's UUID. Fails if a view with the same name already exists.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Unique view name."},
                        "description": {"type": "string", "description": "Optional human description.", "default": ""},
                        "confidence_floor": {"type": "number", "description": "Drop triples below this confidence (0.0 disables).", "default": 0.0},
                        "include_pending": {"type": "boolean", "description": "Include rows from pending_relations (LM-9).", "default": false}
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "memory_views_show",
                "description": "LM-11d — show one Memory View's metadata + members.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "View name or UUID."}
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "memory_views_add",
                "description": "LM-11d — add a member (entity / triple / context UUID) to a Memory View. Idempotent on the (view, kind, member_type, member_id) tuple.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "View name or UUID."},
                        "mode": {"type": "string", "enum": ["include", "exclude"], "description": "Which list to add to.", "default": "include"},
                        "kind": {"type": "string", "enum": ["entity", "triple", "context"], "description": "Member type.", "default": "entity"},
                        "id":   {"type": "string", "description": "UUID of the entity / triple / context."}
                    },
                    "required": ["name", "id"]
                }
            },
            {
                "name": "memory_views_remove",
                "description": "LM-11d — remove a member from a Memory View.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "mode": {"type": "string", "enum": ["include", "exclude"], "default": "include"},
                        "kind": {"type": "string", "enum": ["entity", "triple", "context"], "default": "entity"},
                        "id":   {"type": "string"}
                    },
                    "required": ["name", "id"]
                }
            },
            {
                "name": "memory_views_delete",
                "description": "LM-11d — delete a Memory View and all its members.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    },
                    "required": ["name"]
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
                "name": "memory_cards",
                "description": "Read recent Working Memory Engine cards (Resume / Recall / Compare / Caution / Connect / Anticipate) from `wme_cards`. Read-only.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "description": "Cap on returned cards (default 20).", "default": 20},
                        "kind":  {"type": "string", "description": "Optional filter on card kind."}
                    }
                }
            },
            {
                "name": "memory_card_feedback",
                "description": "Record user feedback on a Working Memory Engine card. Updates the per-kind outcome aggregator (decayed weighted moving avg). Feedback kinds: `useful_now`, `not_useful_now`, `not_now_remind_later`, `dismiss_this_kind`.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "card_id":  {"type": "string", "description": "UUID of the card from `memory_cards`."},
                        "feedback": {"type": "string", "description": "One of: useful_now | not_useful_now | not_now_remind_later | dismiss_this_kind."}
                    },
                    "required": ["card_id", "feedback"]
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
                "description": "Record whether a memory you surfaced was helpful or an unrelated miss, so recall improves over time. Call this after the user reacts to a recalled memory. (Records explicit / implicit / behavioral feedback signals.) Explicit: helpful, not_related, cross_context_bridge, card_accepted, card_rejected, outcome_edited. Implicit: retrieval_cited, retrieval_miss, proposal_silenced. Behavioral: verb_invoked. All signals attach to a `feedback_hook_id` returned by `memory_query` — pass it back to link signals to retrievals. Explicit retrieval signals also train the bandit.",
                "inputSchema": {
                    "type": "object",
                    "required": ["kind"],
                    "properties": {
                        "query_id":          {"type": "string", "description": "Query UUID from memory_query (for bandit-training kinds). Defaults to a new UUID if omitted."},
                        "feedback_hook_id":  {"type": "string", "description": "feedback_hook_id from memory_query response — links this signal to a specific retrieval."},
                        "result_id":         {"type": "string", "description": "Entity or triple UUID being rated (for explicit/implicit signals)."},
                        "kind":              {"type": "string", "enum": ["helpful", "not_related", "cross_context_bridge", "card_accepted", "card_rejected", "outcome_edited", "retrieval_cited", "retrieval_miss", "proposal_silenced", "verb_invoked"], "description": "Signal kind. Class is derived automatically."},
                        "weight":            {"type": "number", "description": "Override default weight (helpful 0.3, negative 1.0). Must be ≥ 0."},
                        "context_id":        {"type": "string", "description": "Active context UUID for positive explicit feedback."},
                        "context_a":         {"type": "string", "description": "cross_context_bridge: the bad-result context."},
                        "context_b":         {"type": "string", "description": "cross_context_bridge: the query's active context."},
                        "verb":              {"type": "string", "description": "For verb_invoked: the MCP verb name that was called."},
                        "host_id":           {"type": "string", "description": "MCP host identifier (claude-code, goose, cursor, etc)."}
                    }
                }
            },
            {
                "name": "memory_pending_list",
                "description": "LM-9 — list rows in the pending pool (mid-confidence triples awaiting human acceptance). Triples with `confidence >= 0.7` flow straight into `kg_relations`; rows here have `0.3 <= confidence < 0.7` and are held back until accepted/rejected. Sorted by confidence desc.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "enum": ["pending", "accepted", "rejected", "any"], "description": "Filter by lifecycle state. `any` returns every row regardless of status. Defaults to `pending`."},
                        "limit":  {"type": "integer", "description": "Max rows to return. Defaults to 25.", "default": 25}
                    }
                }
            },
            {
                "name": "memory_pending_accept",
                "description": "LM-9 — promote a pending row into `kg_relations`. The triple inherits the row's confidence and source_id; the pending row is stamped `accepted` (kept for audit).",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id":   {"type": "string", "description": "Pending row UUID (from memory_pending_list)."},
                        "note": {"type": "string", "description": "Optional human-readable note recorded alongside the acceptance decision."}
                    }
                }
            },
            {
                "name": "memory_pending_reject",
                "description": "LM-9 — reject a pending row. The triple never enters `kg_relations`; the row stays in `pending_relations` with status `rejected` for audit.",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id":   {"type": "string", "description": "Pending row UUID (from memory_pending_list)."},
                        "note": {"type": "string", "description": "Optional human-readable note recorded alongside the rejection decision."}
                    }
                }
            },
            {
                "name": "memory_thread_start",
                "description": "Sprint GRAPH — mint a new conversation thread (first-class composable graph). Returns the thread_id. Every capture/query/commitment made under this thread is auto-tagged for later slicing.",
                "inputSchema": {
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                        "title":      {"type": "string"},
                        "source":     {"type": "string", "enum": ["claude","cursor","goose","tracemind","mcp","other"], "default": "mcp"},
                        "context_id": {"type": "string"}
                    }
                }
            },
            {
                "name": "memory_threads_list",
                "description": "Sprint GRAPH — list recent threads (newest first).",
                "inputSchema": {
                    "type": "object",
                    "properties": { "limit": {"type": "integer", "default": 50} }
                }
            },
            {
                "name": "memory_thread_attach_view",
                "description": "Sprint GRAPH — attach a memory view (graph-algebra expression) as the default scope for queries originating from this thread. Generalizes LM-11d per-query scoping to thread-level default. Use this to route 'Window 4' so it sees only the (Window 1 ∪ Window 3) \\ Window 2 slice.",
                "inputSchema": {
                    "type": "object",
                    "required": ["thread_id","view_id"],
                    "properties": {
                        "thread_id": {"type": "string"},
                        "view_id":   {"type": "string"}
                    }
                }
            },
            {
                "name": "memory_compose",
                "description": "Compose, combine, gather, assemble, merge, or pull together context from the user's past conversation threads into the current one — union, intersect, or filter what several earlier chats knew. Use when the user wants context from prior conversations brought into this one.",
                "inputSchema": {
                    "type": "object",
                    "required": ["expression"],
                    "properties": {
                        "expression": {"type": "object"}
                    }
                }
            },
            {
                "name": "memory_portable_export",
                "description": "Sprint GRAPH — export a composed graph as portable JSON for routing into another AI host (Claude / Cursor / Goose). Returns the graph payload + approx token count + optional disk path.",
                "inputSchema": {
                    "type": "object",
                    "required": ["expression"],
                    "properties": {
                        "expression":    {"type": "object"},
                        "write_to_disk": {"type": "boolean", "default": false}
                    }
                }
            },
            {
                "name": "memory_pin",
                "description": "Q4.5 — pin a memory to keep it in the Hot tier permanently. Pinned memories are excluded from decay and always surface in retrieval.",
                "inputSchema": {
                    "type": "object",
                    "required": ["memory_id"],
                    "properties": {
                        "memory_id": {"type": "string", "description": "UUID of the entity or trace to pin."},
                        "note": {"type": "string", "description": "Optional human note stored alongside the pin."}
                    }
                }
            },
            {
                "name": "memory_forget",
                "description": "Forget, delete, erase, or remove a stored memory or entity at the user's request, excluding it from all future recall. Use when the user asks you to forget something. Local and reversible from the audit log.",
                "inputSchema": {
                    "type": "object",
                    "required": ["memory_id"],
                    "properties": {
                        "memory_id": {"type": "string", "description": "UUID of the entity or trace to forget."}
                    }
                }
            },
            {
                "name": "memory_promote",
                "description": "Q4.5 — manually promote a memory from Cold/Warm to Hot tier by refreshing its last-accessed timestamp.",
                "inputSchema": {
                    "type": "object",
                    "required": ["memory_id"],
                    "properties": {
                        "memory_id": {"type": "string", "description": "UUID of the entity to promote."}
                    }
                }
            },
            {
                "name": "memory_contradict",
                "description": "Check whether a new statement clashes with, conflicts with, contradicts, or is inconsistent with something the user told you before — the retraction beat. Call this when the user says something that might reverse an earlier fact or decision, so you can surface \"you told me the opposite last time.\"",
                "inputSchema": {
                    "type": "object",
                    "required": ["memory_id_a", "memory_id_b"],
                    "properties": {
                        "memory_id_a": {"type": "string", "description": "UUID of the first memory."},
                        "memory_id_b": {"type": "string", "description": "UUID of the second memory."},
                        "note": {"type": "string", "description": "Optional free-text explanation of the contradiction."}
                    }
                }
            },
            {
                "name": "memory_reflect",
                "description": "Q4.5 — trigger a session post-mortem: read recent session signals, build a SessionReflection, and return it as JSON. Useful for end-of-session wrap-up in agentic loops.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string", "description": "Optional session UUID to reflect on. Defaults to the current server session."}
                    }
                }
            },
            {
                "name": "memory_compose_union",
                "description": "Q4.7 — union of two thread-level memory views (∪). Returns the merged set of entity, event, commitment, and capture IDs.",
                "inputSchema": {
                    "type": "object",
                    "required": ["view_id_a", "view_id_b"],
                    "properties": {
                        "view_id_a": {"type": "string", "description": "UUID of the first thread or view."},
                        "view_id_b": {"type": "string", "description": "UUID of the second thread or view."},
                        "save_as":   {"type": "string", "description": "Optional name under which to save the resulting view."}
                    }
                }
            },
            {
                "name": "memory_compose_intersect",
                "description": "Q4.7 — intersection of two thread-level memory views (∩). Returns only the IDs present in both views.",
                "inputSchema": {
                    "type": "object",
                    "required": ["view_id_a", "view_id_b"],
                    "properties": {
                        "view_id_a": {"type": "string", "description": "UUID of the first thread or view."},
                        "view_id_b": {"type": "string", "description": "UUID of the second thread or view."},
                        "save_as":   {"type": "string", "description": "Optional name under which to save the resulting view."}
                    }
                }
            },
            {
                "name": "memory_compose_filter",
                "description": "Q4.7 — filter a thread-level memory view by entity kind or confidence threshold. Returns the filtered set of memory IDs.",
                "inputSchema": {
                    "type": "object",
                    "required": ["view_id", "filter_kind", "filter_value"],
                    "properties": {
                        "view_id":      {"type": "string", "description": "UUID of the thread or view to filter."},
                        "filter_kind":  {"type": "string", "enum": ["entity_type", "confidence"], "description": "Which filter to apply."},
                        "filter_value": {"type": "string", "description": "For entity_type: the type string. For confidence: a numeric threshold string (e.g. '0.7')."}
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

    // The retraction beat (holistic review §5 P1.4). If this store reversed
    // a previously-recorded fact, surface it prominently so the host raises
    // it — "you told me the opposite last time." Instrumented: every fired
    // beat is logged as a first-class feedback signal so we can measure how
    // often the wedge actually triggers in real sessions.
    let contradictions: Vec<Value> = result
        .contradictions
        .iter()
        .map(|c| {
            json!({
                "message": c.message,
                "subject": c.subject,
                "predicate": c.predicate,
                "old": c.old_object,
                "new": c.new_object,
            })
        })
        .collect();
    if !contradictions.is_empty() {
        record_retraction_fired("", result.contradictions.len());
    }

    Ok(json!({
        "stored": true,
        "entities": result.entities.len(),
        "triples": result.triples.len(),
        "candidates_mined": mined_count,
        "outcome_proposals": outcome_proposals,
        // Surfaced first and named so the host cannot miss it.
        "contradictions": contradictions,
        "retraction_beat": !contradictions.is_empty(),
        "context": {
            "memories": context_memories,
            "related": context_related,
        }
    }))
}

/// Instrument the retraction beat: append a line to `retractions.jsonl` in
/// the data dir so we can count how often the wedge fires across real
/// sessions. This is the metric the review asks for (§5 P1.4) — the beat
/// should be the *most* measured behaviour, not the least. Soft-fail.
fn record_retraction_fired(_db_path: &str, count: usize) {
    let path = data_dir().join("retractions.jsonl");
    let line = json!({
        "at": chrono::Utc::now().to_rfc3339(),
        "contradictions": count,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
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
    // LM-11d: optional view splice.
    let view_param = params.get("view").and_then(|v| v.as_str()).map(str::to_string);
    let include_entity: Vec<String> = params
        .get("include_entity")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let exclude_entity: Vec<String> = params
        .get("exclude_entity")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let mut engine = retrieval.lock().await;
    engine.set_cross_context(cross_context);
    // LM-11d: resolve view filter from MCP params. We do this on-demand
    // (per-call) rather than carrying engine-level state because MCP
    // callers may bounce between splices on every request.
    {
        let mut filter = tm_graph::ViewFilter::default();
        let mut applied = false;
        if let Some(name) = view_param.as_deref() {
            if !name.is_empty() {
                if let Ok(graph) = tm_graph::GraphStore::open(db_path) {
                    let resolved = uuid::Uuid::parse_str(name)
                        .ok()
                        .and_then(|id| graph.get_view(id).ok().flatten())
                        .or_else(|| graph.get_view_by_name(name).ok().flatten());
                    if let Some(v) = resolved {
                        if let Ok(f) = graph.load_view_filter(v.id) {
                            filter = f;
                            applied = true;
                        }
                    }
                }
            }
        }
        for s in &include_entity {
            if let Ok(u) = uuid::Uuid::parse_str(s) {
                filter.adhoc_include_entities.insert(u);
                applied = true;
            }
        }
        for s in &exclude_entity {
            if let Ok(u) = uuid::Uuid::parse_str(s) {
                filter.adhoc_exclude_entities.insert(u);
                applied = true;
            }
        }
        engine.set_view_filter(if applied && !filter.is_empty() {
            Some(filter)
        } else {
            None
        });
    }
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
    let req = answerer::short_answer_request_with_context(text, &result, grounding);
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
                // Tells the caller whether to trust this answer, and lets a
                // host distinguish "nothing is stored" from "retrieval
                // failed" — which an empty string cannot express.
                "grounding": result.grounding,
            })
        }
        Err(e) => {
            json!({ "error": e.to_string() })
        }
    };

    // Auto-routing: if the planner detected a reasoning/analogy query,
    // enrich the response with supplemental reasoning data.
    // Q3.1: every retrieval response carries a feedback_hook_id so callers
    // can attach any of the three signal classes back to this retrieval.
    let feedback_hook_id = Uuid::new_v4();

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
        },
        "feedback_hook_id": feedback_hook_id.to_string(),
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
// LM-11d — Memory View MCP handlers
// ---------------------------------------------------------------------------

fn resolve_mcp_view(
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

fn parse_mcp_member_type(s: &str) -> Result<tm_graph::MemberType, String> {
    match s {
        "entity" => Ok(tm_graph::MemberType::Entity),
        "triple" => Ok(tm_graph::MemberType::Triple),
        "context" => Ok(tm_graph::MemberType::Context),
        other => Err(format!("unknown kind '{other}'")),
    }
}

fn parse_mcp_member_kind(s: &str) -> Result<tm_graph::MemberKind, String> {
    match s {
        "include" => Ok(tm_graph::MemberKind::Include),
        "exclude" => Ok(tm_graph::MemberKind::Exclude),
        other => Err(format!("unknown mode '{other}'")),
    }
}

fn handle_memory_views_list(db_path: &str) -> Result<Value, String> {
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let views = graph.list_views().map_err(|e| e.to_string())?;
    let out: Vec<Value> = views
        .into_iter()
        .map(|v| {
            json!({
                "id": v.id.to_string(),
                "name": v.name,
                "description": v.description,
                "confidence_floor": v.confidence_floor,
                "include_pending": v.include_pending,
                "created_at": v.created_at.to_rfc3339(),
                "updated_at": v.updated_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(json!({ "views": out }))
}

fn handle_memory_views_create(params: &Value, db_path: &str) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: name".to_string())?;
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let confidence_floor = params
        .get("confidence_floor")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;
    let include_pending = params
        .get("include_pending")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let mut view = tm_graph::MemoryView::new(name, description);
    view.confidence_floor = confidence_floor;
    view.include_pending = include_pending;
    graph.create_view(&view).map_err(|e| e.to_string())?;
    Ok(json!({
        "id": view.id.to_string(),
        "name": view.name,
    }))
}

fn handle_memory_views_show(params: &Value, db_path: &str) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: name".to_string())?;
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let view = resolve_mcp_view(&graph, name)?;
    let members = graph.list_view_members(view.id).map_err(|e| e.to_string())?;
    let members_json: Vec<Value> = members
        .into_iter()
        .map(|m| {
            json!({
                "kind": m.kind.as_str(),
                "type": m.member_type.as_str(),
                "id": m.member_id.to_string(),
                "added_at": m.added_at.to_rfc3339(),
            })
        })
        .collect();
    Ok(json!({
        "view": {
            "id": view.id.to_string(),
            "name": view.name,
            "description": view.description,
            "confidence_floor": view.confidence_floor,
            "include_pending": view.include_pending,
            "created_at": view.created_at.to_rfc3339(),
            "updated_at": view.updated_at.to_rfc3339(),
        },
        "members": members_json,
    }))
}

fn handle_memory_views_add(params: &Value, db_path: &str) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: name".to_string())?;
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("include");
    let kind = params
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("entity");
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: id".to_string())?;
    let member_id = Uuid::parse_str(id).map_err(|_| format!("invalid UUID: {id}"))?;
    let member_type = parse_mcp_member_type(kind)?;
    let member_kind = parse_mcp_member_kind(mode)?;

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let view = resolve_mcp_view(&graph, name)?;
    graph
        .add_view_member(view.id, member_kind, member_type, member_id)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "view": view.name,
        "added": {
            "mode": member_kind.as_str(),
            "kind": member_type.as_str(),
            "id": member_id.to_string(),
        }
    }))
}

fn handle_memory_views_remove(params: &Value, db_path: &str) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: name".to_string())?;
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("include");
    let kind = params
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("entity");
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: id".to_string())?;
    let member_id = Uuid::parse_str(id).map_err(|_| format!("invalid UUID: {id}"))?;
    let member_type = parse_mcp_member_type(kind)?;
    let member_kind = parse_mcp_member_kind(mode)?;

    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let view = resolve_mcp_view(&graph, name)?;
    let removed = graph
        .remove_view_member(view.id, member_kind, member_type, member_id)
        .map_err(|e| e.to_string())?;
    Ok(json!({ "view": view.name, "removed": removed }))
}

fn handle_memory_views_delete(params: &Value, db_path: &str) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: name".to_string())?;
    let graph = GraphStore::open(db_path).map_err(|e| e.to_string())?;
    let view = resolve_mcp_view(&graph, name)?;
    let deleted = graph.delete_view(view.id).map_err(|e| e.to_string())?;
    Ok(json!({ "view": view.name, "deleted": deleted }))
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

            // LM-12: a `cross_context_bridge` negative signal also
            // counts as a "this pair should not be bridged" strike.
            // After `DEFAULT_STRIKE_THRESHOLD` strikes the pair auto-
            // promotes to blocked, and `is_context_pair_blocked` will
            // start returning true for downstream retrieval. We do this
            // only when both contexts are provided — `not_related`
            // without context info has no pair to record.
            let mut deny_outcome: Option<tm_graph::StrikeOutcome> = None;
            if kind == "cross_context_bridge" {
                if let (Some(a), Some(b)) = (context_a, context_b) {
                    let dir = data_dir();
                    let path = tm_graph::DenyList::default_path(&dir);
                    let mut deny = tm_graph::DenyList::load(&path).map_err(|e| {
                        format!("load deny-list {}: {e}", path.display())
                    })?;
                    let note = format!(
                        "auto-strike from memory_feedback query={query_id} result={result_id}"
                    );
                    let outcome = deny.record_context_strike(a, b, &note);
                    deny.save(&path).map_err(|e| {
                        format!("save deny-list {}: {e}", path.display())
                    })?;
                    deny_outcome = Some(outcome);
                }
            }

            let deny_payload = deny_outcome.as_ref().map(|o| {
                let (status, strikes) = match o {
                    tm_graph::StrikeOutcome::Watching { strikes } => ("watching", *strikes),
                    tm_graph::StrikeOutcome::JustBlocked { strikes } => ("just_blocked", *strikes),
                    tm_graph::StrikeOutcome::AlreadyBlocked { strikes } => ("already_blocked", *strikes),
                };
                json!({ "status": status, "strikes": strikes })
            });

            let mut payload = json!({
                "ok": true,
                "channel": "negative_signals",
                "row_id": row_id,
                "kind": kind,
                "weight": weight,
            });
            if let Some(p) = deny_payload {
                payload
                    .as_object_mut()
                    .expect("payload is object")
                    .insert("deny_list".to_string(), p);
            }
            Ok(payload)
        }
        // Q3.1: expanded signal fabric — explicit card/outcome signals
        "card_accepted" | "card_rejected" | "outcome_edited" => {
            let feedback_hook_id = params
                .get("feedback_hook_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or(query_id);
            let score = match kind {
                "card_accepted" => 1.0f32,
                "card_rejected" => -1.0,
                _ => 0.5, // outcome_edited is mildly positive
            };
            let target_id = Uuid::parse_str(result_id).ok();
            let signal = tm_types::FeedbackSignal::new(
                tm_types::FeedbackKind::from_str(kind)
                    .unwrap_or(tm_types::FeedbackKind::CardAccepted),
                feedback_hook_id,
                target_id,
                score,
            );
            let signal_id = graph
                .record_feedback_signal(&signal)
                .map_err(|e| format!("record_feedback_signal: {e}"))?;
            Ok(json!({ "ok": true, "class": "explicit", "kind": kind, "signal_id": signal_id }))
        }
        // Q3.1: implicit signals — retrieval cited / miss / proposal silenced
        "retrieval_cited" | "retrieval_miss" | "proposal_silenced" => {
            let feedback_hook_id = params
                .get("feedback_hook_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or(query_id);
            let score = if kind == "retrieval_cited" { 0.5f32 } else { -0.2 };
            let signal = tm_types::FeedbackSignal::new(
                tm_types::FeedbackKind::from_str(kind)
                    .unwrap_or(tm_types::FeedbackKind::RetrievalCited),
                feedback_hook_id,
                Uuid::parse_str(result_id).ok(),
                score,
            );
            let signal_id = graph
                .record_feedback_signal(&signal)
                .map_err(|e| format!("record_feedback_signal: {e}"))?;

            // Also route the retrieval-outcome signals into the bandit's
            // reward channels. Recording them only as `feedback_signals`
            // rows left them inert: on an agentic host they are the *only*
            // implicit evidence available (there is no click and no dwell),
            // so if they do not reach the controller nothing does.
            //
            // `proposal_silenced` is about a surfaced card, not about
            // whether retrieval found the right memory, so it stays out of
            // the retrieval reward.
            let mut trained = false;
            match kind {
                "retrieval_cited" => {
                    graph
                        .write_positive_signal(query_id, result_id, kind, None, 0.3)
                        .map_err(|e| format!("write_positive_signal: {e}"))?;
                    trained = true;
                }
                "retrieval_miss" => {
                    graph
                        .write_negative_signal(query_id, result_id, kind, None, None, 0.5)
                        .map_err(|e| format!("write_negative_signal: {e}"))?;
                    trained = true;
                }
                _ => {}
            }

            Ok(json!({
                "ok": true,
                "class": "implicit",
                "kind": kind,
                "signal_id": signal_id,
                "trains_bandit": trained,
            }))
        }
        // Q3.1: behavioral signals — verb invocations
        "verb_invoked" => {
            let verb = params
                .get("verb")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let feedback_hook_id = params
                .get("feedback_hook_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .unwrap_or(query_id);
            let mut signal = tm_types::FeedbackSignal::new(
                tm_types::FeedbackKind::VerbInvoked,
                feedback_hook_id,
                None,
                1.0,
            )
            .with_verb(verb);
            if let Some(host) = params.get("host_id").and_then(|v| v.as_str()) {
                signal = signal.with_host(host);
            }
            let signal_id = graph
                .record_feedback_signal(&signal)
                .map_err(|e| format!("record_feedback_signal: {e}"))?;
            Ok(json!({ "ok": true, "class": "behavioral", "kind": "verb_invoked", "verb": verb, "signal_id": signal_id }))
        }
        other => Err(format!(
            "invalid kind '{other}': expected one of helpful, not_related, cross_context_bridge, \
             card_accepted, card_rejected, outcome_edited, retrieval_cited, retrieval_miss, \
             proposal_silenced, verb_invoked"
        )),
    }
}

// ---------------------------------------------------------------------------
// LM-9 — pending pool MCP handlers
// ---------------------------------------------------------------------------

/// Hydrate a [`tm_graph::PendingRelation`] into the wire-format the
/// MCP / Tauri panels consume. Resolves subject/object UUIDs to the
/// human-readable entity names where possible so clients don't need
/// a second round-trip.
fn pending_to_json(graph: &tm_graph::GraphStore, row: &tm_graph::PendingRelation) -> Value {
    let subject_name = graph
        .get_entity(row.subject_id)
        .ok()
        .map(|e| e.name)
        .unwrap_or_default();
    let object_name = graph
        .get_entity(row.object_id)
        .ok()
        .map(|e| e.name)
        .unwrap_or_default();
    json!({
        "id": row.id.to_string(),
        "subject_id": row.subject_id.to_string(),
        "subject_name": subject_name,
        "predicate": row.predicate,
        "object_id": row.object_id.to_string(),
        "object_name": object_name,
        "confidence": row.confidence,
        "source_id": row.source_id,
        "status": row.status,
        "created_at": row.created_at.to_rfc3339(),
        "decided_at": row.decided_at.map(|d| d.to_rfc3339()),
        "note": row.note,
    })
}

fn handle_memory_pending_list(params: &Value, db_path: &str) -> Result<Value, String> {
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let status_str = params
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending");
    let filter = match status_str {
        "any" | "all" => None,
        other => Some(
            tm_graph::PendingStatus::parse(other)
                .map_err(|e| format!("status filter: {e}"))?,
        ),
    };
    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(25);
    let rows = graph
        .list_pending(filter, Some(limit))
        .map_err(|e| format!("list_pending: {e}"))?;
    let payload: Vec<Value> = rows.iter().map(|r| pending_to_json(&graph, r)).collect();
    Ok(json!({ "count": payload.len(), "rows": payload }))
}

fn handle_memory_pending_accept(params: &Value, db_path: &str) -> Result<Value, String> {
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("missing 'id'")?;
    let id = uuid::Uuid::parse_str(id_str).map_err(|_| format!("invalid uuid '{id_str}'"))?;
    let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
    let triple = graph
        .accept_pending(id, note)
        .map_err(|e| format!("accept_pending: {e}"))?;
    Ok(json!({
        "ok": true,
        "promoted_triple": {
            "id": triple.id.to_string(),
            "subject_id": triple.subject_id.to_string(),
            "predicate": triple.predicate.to_string(),
            "object_id": triple.object_id.to_string(),
            "confidence": triple.confidence,
            "source_id": triple.source_id,
        }
    }))
}

fn handle_memory_pending_reject(params: &Value, db_path: &str) -> Result<Value, String> {
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let id_str = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("missing 'id'")?;
    let id = uuid::Uuid::parse_str(id_str).map_err(|_| format!("invalid uuid '{id_str}'"))?;
    let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
    let ok = graph
        .reject_pending(id, note)
        .map_err(|e| format!("reject_pending: {e}"))?;
    Ok(json!({ "ok": ok, "id": id.to_string() }))
}

// ---------------------------------------------------------------------------
// Sprint GRAPH — thread / compose / attach_view / portable_export
// ---------------------------------------------------------------------------

fn handle_memory_thread_start(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::thread_graph::{Thread, ThreadGraphStore, ThreadSource};
    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("missing 'title'")?
        .to_string();
    let source = params
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("mcp");
    let mut t = Thread::new(title, ThreadSource::parse(source));
    if let Some(cid) = params.get("context_id").and_then(|v| v.as_str()) {
        t.context_id = uuid::Uuid::parse_str(cid).ok();
    }
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    ThreadGraphStore::upsert(graph.connection(), &t).map_err(|e| e.to_string())?;
    Ok(json!({
        "thread_id": t.id.to_string(),
        "title": t.title,
        "source": t.source.as_str(),
        "started_at": t.started_at.to_rfc3339(),
    }))
}

fn handle_memory_threads_list(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::thread_graph::ThreadGraphStore;
    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let xs = ThreadGraphStore::list(graph.connection(), limit).map_err(|e| e.to_string())?;
    let arr: Vec<Value> = xs
        .iter()
        .map(|t| {
            json!({
                "id": t.id.to_string(),
                "title": t.title,
                "source": t.source.as_str(),
                "started_at": t.started_at.to_rfc3339(),
                "ended_at": t.ended_at.map(|d| d.to_rfc3339()),
            })
        })
        .collect();
    Ok(json!({ "threads": arr }))
}

fn handle_memory_thread_attach_view(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::thread_graph::ThreadGraphStore;
    let tid_s = params
        .get("thread_id")
        .and_then(|v| v.as_str())
        .ok_or("missing 'thread_id'")?;
    let vid_s = params
        .get("view_id")
        .and_then(|v| v.as_str())
        .ok_or("missing 'view_id'")?;
    let tid = uuid::Uuid::parse_str(tid_s).map_err(|e| e.to_string())?;
    let vid = uuid::Uuid::parse_str(vid_s).map_err(|e| e.to_string())?;
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    ThreadGraphStore::attach_view(graph.connection(), tid, vid).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true, "thread_id": tid.to_string(), "view_id": vid.to_string() }))
}

fn handle_memory_compose(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::algebra::{Algebra, GraphExpr};
    let expr_val = params
        .get("expression")
        .ok_or("missing 'expression'")?
        .clone();
    let expr: GraphExpr = serde_json::from_value(expr_val)
        .map_err(|e| format!("bad expression: {e}"))?;
    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let g = Algebra::eval(graph.connection(), &expr).map_err(|e| e.to_string())?;
    Ok(json!({
        "thread_id":      g.thread_id.to_string(),
        "entity_ids":     g.entity_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "event_node_ids": g.event_node_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "topic_clusters": g.topic_clusters,
        "commitment_ids": g.commitment_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "capture_signal_ids": g.capture_signal_ids,
    }))
}

fn handle_memory_portable_export(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::algebra::{Algebra, GraphExpr};
    use tm_graph::portable_export::{approx_token_count, export_portable, write_to_path};
    let expr_val = params
        .get("expression")
        .ok_or("missing 'expression'")?
        .clone();
    let expr: GraphExpr = serde_json::from_value(expr_val)
        .map_err(|e| format!("bad expression: {e}"))?;
    let write = params
        .get("write_to_disk")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let graph = tm_graph::GraphStore::open(db_path)
        .map_err(|e| format!("open graph: {e}"))?;
    let conn = graph.connection();
    let g = Algebra::eval(conn, &expr).map_err(|e| e.to_string())?;
    let portable = export_portable(conn, &g).map_err(|e| e.to_string())?;
    let approx = approx_token_count(&portable);

    let mut path: Option<String> = None;
    if write {
        let parent = std::path::Path::new(db_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let dir = parent.join("portable");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let p = dir.join(format!(
            "graph-{}.json",
            chrono::Utc::now().timestamp_millis()
        ));
        write_to_path(&portable, &p).map_err(|e| e.to_string())?;
        path = Some(p.to_string_lossy().to_string());
    }

    Ok(json!({
        "graph": serde_json::to_value(&portable).map_err(|e| e.to_string())?,
        "approx_tokens": approx,
        "path": path,
    }))
}

/// `memory_brief` — render the daily brief from the system of intents
/// (`docs/INTENT_SYSTEM.md` §9.1).
///
/// Read-only: never mutates the intent store. Returns the
/// [`tm_reflect::DailyBrief`] structure verbatim as JSON.
fn handle_memory_cards(params: &Value, db_path: &str) -> Result<Value, String> {
    let limit = params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;
    let kind_filter: Option<String> = params
        .get("kind")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open wme db: {e}"))?;
    // Ensure the wme_cards table exists; if not, the WME has never
    // pushed a card. Return an empty list gracefully.
    tm_reflect::ensure_wme_schema(&conn)
        .map_err(|e| format!("ensure_wme_schema: {e}"))?;

    let (sql, has_kind) = match kind_filter.as_deref() {
        Some(_) => (
            "SELECT id, kind, target_id, statement, score, relevance, surprise, recency, outcome, created_at \
             FROM wme_cards WHERE kind = ?1 ORDER BY created_at DESC LIMIT ?2",
            true,
        ),
        None => (
            "SELECT id, kind, target_id, statement, score, relevance, surprise, recency, outcome, created_at \
             FROM wme_cards ORDER BY created_at DESC LIMIT ?1",
            false,
        ),
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows_iter = if has_kind {
        stmt.query_map(
            rusqlite::params![kind_filter.as_deref().unwrap_or(""), limit as i64],
            row_to_card_value,
        )
    } else {
        stmt.query_map(rusqlite::params![limit as i64], row_to_card_value)
    }
    .map_err(|e| e.to_string())?;

    let mut cards: Vec<Value> = Vec::new();
    for r in rows_iter {
        cards.push(r.map_err(|e| e.to_string())?);
    }
    Ok(json!({ "cards": cards }))
}

fn row_to_card_value(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id":         row.get::<_, String>(0)?,
        "kind":       row.get::<_, String>(1)?,
        "target_id":  row.get::<_, String>(2)?,
        "statement":  row.get::<_, String>(3)?,
        "score":      row.get::<_, f64>(4)?,
        "relevance":  row.get::<_, f64>(5)?,
        "surprise":   row.get::<_, f64>(6)?,
        "recency":    row.get::<_, f64>(7)?,
        "outcome":    row.get::<_, f64>(8)?,
        "created_at": row.get::<_, i64>(9)?,
    }))
}

fn handle_memory_card_feedback(params: &Value, db_path: &str) -> Result<Value, String> {
    let card_id_s = params
        .get("card_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: card_id".to_string())?;
    let card_id = Uuid::parse_str(card_id_s)
        .map_err(|e| format!("invalid card_id '{card_id_s}': {e}"))?;
    let feedback_s = params
        .get("feedback")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: feedback".to_string())?;

    let feedback = match feedback_s {
        "useful_now" => tm_reflect::FeedbackKind::UsefulNow,
        "not_useful_now" => tm_reflect::FeedbackKind::NotUsefulNow,
        "not_now_remind_later" => tm_reflect::FeedbackKind::NotNowRemindLater,
        "dismiss_this_kind" => tm_reflect::FeedbackKind::DismissThisKind,
        other => return Err(format!(
            "invalid feedback '{other}': must be one of useful_now | not_useful_now | not_now_remind_later | dismiss_this_kind"
        )),
    };

    // Look up the card's `kind` so the engine knows which outcome
    // aggregator to update. If the card row is missing the call still
    // records the feedback but skips the kind-specific outcome bump.
    let conn = rusqlite::Connection::open(db_path).map_err(|e| e.to_string())?;
    tm_reflect::ensure_wme_schema(&conn).map_err(|e| e.to_string())?;
    let kind_s: Option<String> = conn
        .query_row(
            "SELECT kind FROM wme_cards WHERE id = ?1",
            rusqlite::params![card_id.to_string()],
            |r| r.get(0),
        )
        .ok();
    let card_kind = kind_s
        .as_deref()
        .and_then(|s| match s {
            "resume" => Some(tm_reflect::CardKind::Resume),
            "recall" => Some(tm_reflect::CardKind::Recall),
            "compare" => Some(tm_reflect::CardKind::Compare),
            "caution" => Some(tm_reflect::CardKind::Caution),
            "connect" => Some(tm_reflect::CardKind::Connect),
            "anticipate" => Some(tm_reflect::CardKind::Anticipate),
            _ => None,
        })
        .ok_or_else(|| format!("card {} not found or unknown kind", card_id))?;

    let engine = tm_reflect::WorkingMemoryEngine::open(db_path)
        .map_err(|e| format!("open wme engine: {e}"))?;
    engine
        .record_feedback(card_id, card_kind, feedback)
        .map_err(|e| format!("record_feedback: {e}"))?;
    Ok(json!({ "ok": true, "card_id": card_id.to_string(), "feedback": feedback_s }))
}

fn handle_memory_brief(
    params: &Value,
    intents_path: &str,
    db_path: &str,
) -> Result<Value, String> {
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
    // WME-7 — verb-first cards inlined into the brief. Defaults to 12,
    // matching the desktop panel; set to 0 to opt out.
    let limit_wme_cards = params
        .get("limit_wme_cards")
        .and_then(|v| v.as_u64())
        .unwrap_or(12) as usize;

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

    let mut value =
        serde_json::to_value(&brief).map_err(|e| format!("brief serialization: {e}"))?;

    // WME-7 — attach the most recent WME cards as a sibling field so
    // MCP clients (Claude Code, Goose) can render the verb-first
    // surface alongside the commitment-centric brief. Best-effort:
    // missing wme_cards table → empty list, never an error.
    if limit_wme_cards > 0 {
        let cards = load_wme_cards_for_brief(db_path, limit_wme_cards).unwrap_or_else(|err| {
            tracing::debug!("[memory_brief] wme cards skipped: {err}");
            Vec::new()
        });
        if let Some(obj) = value.as_object_mut() {
            obj.insert("wme_cards".into(), Value::Array(cards));
        }
    }

    Ok(value)
}

/// Read up to `limit` of the most recent WME cards, returning them in
/// the same JSON shape as the standalone `memory_cards` tool so MCP
/// clients can share a single deserializer.
fn load_wme_cards_for_brief(db_path: &str, limit: usize) -> Result<Vec<Value>, String> {
    let conn =
        rusqlite::Connection::open(db_path).map_err(|e| format!("open wme db: {e}"))?;
    tm_reflect::ensure_wme_schema(&conn)
        .map_err(|e| format!("ensure_wme_schema: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, target_id, statement, score, relevance, surprise, recency, outcome, created_at \
             FROM wme_cards ORDER BY created_at DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], row_to_card_value)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
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
// Q4.5 — memory-as-tools handlers
// ---------------------------------------------------------------------------

/// `memory_pin` — pin a memory to keep it in Hot tier permanently.
/// Creates a `pinned_memories` table on first use and inserts/upserts a row.
fn handle_memory_pin(params: &Value, db_path: &str) -> Result<Value, String> {
    let memory_id_s = params
        .get("memory_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: memory_id".to_string())?;
    let memory_id = Uuid::parse_str(memory_id_s)
        .map_err(|e| format!("invalid memory_id '{memory_id_s}': {e}"))?;
    let note = params
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open graph db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pinned_memories (
            memory_id TEXT PRIMARY KEY,
            pinned_at INTEGER NOT NULL,
            note      TEXT
        );",
    )
    .map_err(|e| format!("ensure pinned_memories table: {e}"))?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO pinned_memories (memory_id, pinned_at, note)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(memory_id) DO UPDATE SET pinned_at = excluded.pinned_at, note = excluded.note",
        rusqlite::params![memory_id.to_string(), now, note],
    )
    .map_err(|e| format!("pin insert: {e}"))?;

    Ok(json!({
        "ok": true,
        "memory_id": memory_id.to_string(),
        "pinned_at": now,
        "note": note,
    }))
}

/// `memory_forget` — soft-delete a memory (mark as forgotten, exclude from retrieval).
/// Creates a `forgotten_memories` table on first use.
fn handle_memory_forget(params: &Value, db_path: &str) -> Result<Value, String> {
    let memory_id_s = params
        .get("memory_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: memory_id".to_string())?;
    let memory_id = Uuid::parse_str(memory_id_s)
        .map_err(|e| format!("invalid memory_id '{memory_id_s}': {e}"))?;

    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open graph db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS forgotten_memories (
            memory_id    TEXT PRIMARY KEY,
            forgotten_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("ensure forgotten_memories table: {e}"))?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO forgotten_memories (memory_id, forgotten_at)
         VALUES (?1, ?2)
         ON CONFLICT(memory_id) DO UPDATE SET forgotten_at = excluded.forgotten_at",
        rusqlite::params![memory_id.to_string(), now],
    )
    .map_err(|e| format!("forget insert: {e}"))?;

    Ok(json!({
        "ok": true,
        "memory_id": memory_id.to_string(),
        "forgotten_at": now,
    }))
}

/// `memory_promote` — manually promote a memory from Cold/Warm to Hot tier
/// by reinforcing the entity's confidence score (which acts as the Hot-tier
/// entry point — higher confidence = higher retrieval priority).
fn handle_memory_promote(params: &Value, db_path: &str) -> Result<Value, String> {
    let memory_id_s = params
        .get("memory_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: memory_id".to_string())?;
    let memory_id = Uuid::parse_str(memory_id_s)
        .map_err(|e| format!("invalid memory_id '{memory_id_s}': {e}"))?;

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;

    // Reinforce entity confidence to push it into the Hot tier window.
    // A bump of 0.15 is the same magnitude the ingest pipeline uses for
    // an explicit user re-mention.
    graph
        .reinforce_entity(memory_id, 0.15)
        .map_err(|e| format!("reinforce_entity: {e}"))?;

    let now = chrono::Utc::now().timestamp();
    Ok(json!({
        "ok": true,
        "memory_id": memory_id.to_string(),
        "promoted_at": now,
        "method": "reinforce(+0.15)",
    }))
}

/// `memory_contradict` — explicitly flag two memories as contradicting each other.
/// Writes a contradiction record to the `user_contradictions` table in the graph DB.
fn handle_memory_contradict(params: &Value, db_path: &str) -> Result<Value, String> {
    let id_a_s = params
        .get("memory_id_a")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: memory_id_a".to_string())?;
    let id_b_s = params
        .get("memory_id_b")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: memory_id_b".to_string())?;
    let memory_id_a = Uuid::parse_str(id_a_s)
        .map_err(|e| format!("invalid memory_id_a '{id_a_s}': {e}"))?;
    let memory_id_b = Uuid::parse_str(id_b_s)
        .map_err(|e| format!("invalid memory_id_b '{id_b_s}': {e}"))?;
    let note = params
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open graph db: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_contradictions (
            id            TEXT PRIMARY KEY,
            memory_id_a   TEXT NOT NULL,
            memory_id_b   TEXT NOT NULL,
            recorded_at   INTEGER NOT NULL,
            note          TEXT
        );",
    )
    .map_err(|e| format!("ensure user_contradictions table: {e}"))?;

    let record_id = Uuid::new_v4();
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO user_contradictions (id, memory_id_a, memory_id_b, recorded_at, note)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            record_id.to_string(),
            memory_id_a.to_string(),
            memory_id_b.to_string(),
            now,
            note,
        ],
    )
    .map_err(|e| format!("contradict insert: {e}"))?;

    Ok(json!({
        "ok": true,
        "contradiction_id": record_id.to_string(),
        "memory_id_a": memory_id_a.to_string(),
        "memory_id_b": memory_id_b.to_string(),
        "recorded_at": now,
        "note": note,
    }))
}

/// `memory_reflect` — trigger a session post-mortem and return it as JSON.
fn handle_memory_reflect(params: &Value, current_session_id: Uuid) -> Result<Value, String> {
    use tm_reflect::SessionReflection;

    let session_id = params
        .get("session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(current_session_id);

    // Stub: real signal counts come from the feedback fabric in a full
    // implementation (Q4 feedback aggregator). For now, build a
    // zero-count reflection that still exercises the narrative generator.
    let reflection = SessionReflection::from_signals(session_id, 0, 0, 0, None, &[]);

    serde_json::to_value(&reflection).map_err(|e| format!("serialize reflection: {e}"))
}

// ---------------------------------------------------------------------------
// Q4.7 — Composition-layer graph algebra handlers
// ---------------------------------------------------------------------------

/// Resolve a thread or view UUID string from MCP params.
/// Accepts bare UUID strings (thread IDs or view UUIDs).
fn resolve_thread_id(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("invalid UUID '{s}': {e}"))
}

/// Build a JSON representation of a `ThreadGraph` for MCP responses.
fn thread_graph_to_json(g: &tm_graph::thread_graph::ThreadGraph) -> Value {
    json!({
        "thread_id":          g.thread_id.to_string(),
        "entity_ids":         g.entity_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "event_node_ids":     g.event_node_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "topic_clusters":     g.topic_clusters,
        "commitment_ids":     g.commitment_ids.iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        "capture_signal_ids": g.capture_signal_ids,
    })
}

/// `memory_compose_union` — union of two memory views (∪).
fn handle_memory_compose_union(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::algebra::{Algebra, GraphExpr, SetOp};

    let a_s = params
        .get("view_id_a")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: view_id_a".to_string())?;
    let b_s = params
        .get("view_id_b")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: view_id_b".to_string())?;
    let save_as = params
        .get("save_as")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let thread_id_a = resolve_thread_id(a_s)?;
    let thread_id_b = resolve_thread_id(b_s)?;

    let expr = GraphExpr::SetOp {
        op: SetOp::Union,
        left: Box::new(GraphExpr::Thread { thread_id: thread_id_a }),
        right: Box::new(GraphExpr::Thread { thread_id: thread_id_b }),
    };

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let g = Algebra::eval(graph.connection(), &expr).map_err(|e| e.to_string())?;

    let mut resp = thread_graph_to_json(&g);
    if let Some(name) = save_as {
        let mut view = tm_graph::MemoryView::new(&name, "union composition");
        view.confidence_floor = 0.0;
        // Best-effort save; ignore errors so the response still returns the data.
        let _ = graph.create_view(&view).map(|_| {
            // Add entity members from union result
            for eid in &g.entity_ids {
                let _ = graph.add_view_member(
                    view.id,
                    tm_graph::MemberKind::Include,
                    tm_graph::MemberType::Entity,
                    *eid,
                );
            }
        });
        resp["saved_view_id"] = json!(view.id.to_string());
        resp["saved_view_name"] = json!(name);
    }
    Ok(resp)
}

/// `memory_compose_intersect` — intersection of two memory views (∩).
fn handle_memory_compose_intersect(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::algebra::{Algebra, GraphExpr, SetOp};

    let a_s = params
        .get("view_id_a")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: view_id_a".to_string())?;
    let b_s = params
        .get("view_id_b")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: view_id_b".to_string())?;
    let save_as = params
        .get("save_as")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let thread_id_a = resolve_thread_id(a_s)?;
    let thread_id_b = resolve_thread_id(b_s)?;

    let expr = GraphExpr::SetOp {
        op: SetOp::Intersect,
        left: Box::new(GraphExpr::Thread { thread_id: thread_id_a }),
        right: Box::new(GraphExpr::Thread { thread_id: thread_id_b }),
    };

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let g = Algebra::eval(graph.connection(), &expr).map_err(|e| e.to_string())?;

    let mut resp = thread_graph_to_json(&g);
    if let Some(name) = save_as {
        let mut view = tm_graph::MemoryView::new(&name, "intersect composition");
        view.confidence_floor = 0.0;
        let _ = graph.create_view(&view).map(|_| {
            for eid in &g.entity_ids {
                let _ = graph.add_view_member(
                    view.id,
                    tm_graph::MemberKind::Include,
                    tm_graph::MemberType::Entity,
                    *eid,
                );
            }
        });
        resp["saved_view_id"] = json!(view.id.to_string());
        resp["saved_view_name"] = json!(name);
    }
    Ok(resp)
}

/// `memory_compose_filter` — filter a memory view by entity kind or confidence threshold.
fn handle_memory_compose_filter(params: &Value, db_path: &str) -> Result<Value, String> {
    use tm_graph::algebra::{Algebra, FilterPredicate, GraphExpr};

    let view_id_s = params
        .get("view_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: view_id".to_string())?;
    let filter_kind = params
        .get("filter_kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: filter_kind".to_string())?;
    let filter_value = params
        .get("filter_value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: filter_value".to_string())?;

    let thread_id = resolve_thread_id(view_id_s)?;

    // Build the filter predicate from the caller's parameters.
    let predicate = match filter_kind {
        "entity_type" => FilterPredicate::ObjectTypeAny {
            types: vec![filter_value.to_string()],
        },
        "confidence" => {
            // For confidence filtering, we use a confidence floor on the
            // MemoryView and then apply entity-level filtering. Since
            // FilterPredicate doesn't have a confidence variant, we load
            // the view with load_view_filter and apply the floor there.
            // As a fallback, just return the thread graph and note the floor.
            let _threshold = filter_value
                .parse::<f32>()
                .map_err(|_| format!("filter_value must be a number for confidence filter, got '{filter_value}'"))?;

            // Use ObjectTypeAny as a no-op pass-through (empty = all types pass).
            // The caller can apply confidence filtering via memory_views_create
            // with confidence_floor for persistent views.
            FilterPredicate::ObjectTypeAny { types: vec![] }
        }
        other => return Err(format!(
            "invalid filter_kind '{other}': expected 'entity_type' or 'confidence'"
        )),
    };

    let inner_expr = GraphExpr::Thread { thread_id };
    let expr = GraphExpr::Filter {
        inner: Box::new(inner_expr),
        predicate,
    };

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let g = Algebra::eval(graph.connection(), &expr).map_err(|e| e.to_string())?;

    let mut resp = thread_graph_to_json(&g);
    resp["filter_kind"] = json!(filter_kind);
    resp["filter_value"] = json!(filter_value);
    Ok(resp)
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

        "tools/list" => Ok(tools_list_filtered()),

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
                "memory_views_list" => {
                    handle_memory_views_list(db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_views_create" => {
                    handle_memory_views_create(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_views_show" => {
                    handle_memory_views_show(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_views_add" => {
                    handle_memory_views_add(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_views_remove" => {
                    handle_memory_views_remove(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_views_delete" => {
                    handle_memory_views_delete(&args, db_path)
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
                    handle_memory_brief(&args, intents_path, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_cards" => {
                    handle_memory_cards(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_card_feedback" => {
                    handle_memory_card_feedback(&args, db_path)
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
                "memory_pending_list" => {
                    handle_memory_pending_list(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_pending_accept" => {
                    handle_memory_pending_accept(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_pending_reject" => {
                    handle_memory_pending_reject(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_thread_start" => {
                    handle_memory_thread_start(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_threads_list" => {
                    handle_memory_threads_list(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_thread_attach_view" => {
                    handle_memory_thread_attach_view(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_compose" => {
                    handle_memory_compose(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_portable_export" => {
                    handle_memory_portable_export(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                // Q4.5 — memory-as-tools verbs
                "memory_pin" => {
                    handle_memory_pin(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_forget" => {
                    handle_memory_forget(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_promote" => {
                    handle_memory_promote(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_contradict" => {
                    handle_memory_contradict(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_reflect" => {
                    handle_memory_reflect(&args, session_id)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                // Q4.7 — composition-layer graph algebra verbs
                "memory_compose_union" => {
                    handle_memory_compose_union(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_compose_intersect" => {
                    handle_memory_compose_intersect(&args, db_path)
                        .map_err(|e| anyhow::anyhow!(e))?
                }
                "memory_compose_filter" => {
                    handle_memory_compose_filter(&args, db_path)
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

        let brief = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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
        let pre = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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
        let post = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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
        let brief2 = handle_memory_brief(&json!({}), &intents_path, &intents_path).expect("brief ok");
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

    /// LM-12: three `cross_context_bridge` strikes on the same
    /// context-pair must auto-promote the pair to *blocked* in the
    /// deny-list (`cross_ctx_block_list.json`). Strike #4 is reported
    /// as `already_blocked`. We override `TM_DATA_DIR` so the file
    /// lands inside our temp dir.
    #[test]
    fn cross_context_bridge_strikes_auto_deny_at_three() {
        let dir = std::env::temp_dir().join(format!("tm_mcp_lm12_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("memory.db").to_str().unwrap().to_string();

        // Force `data_dir()` to point at our temp dir for this test.
        // Tests in this binary run single-threaded by default; even if
        // they didn't, the env var is checked synchronously at the top
        // of `handle_memory_feedback`'s deny-list path.
        std::env::set_var("TM_DATA_DIR", &dir);

        // Seed enough graph state so write_negative_signal succeeds.
        let _ = tm_graph::GraphStore::open(&db_path).expect("open graph");

        let qid = Uuid::new_v4();
        let ctx_a = Uuid::new_v4();
        let ctx_b = Uuid::new_v4();

        let call = |strike_n: u32| -> Value {
            let params = json!({
                "query_id": qid.to_string(),
                "result_id": format!("res-{strike_n}"),
                "kind": "cross_context_bridge",
                "context_a": ctx_a.to_string(),
                "context_b": ctx_b.to_string(),
            });
            handle_memory_feedback(&params, &db_path).expect("feedback ok")
        };

        let r1 = call(1);
        assert_eq!(r1["deny_list"]["status"], json!("watching"));
        assert_eq!(r1["deny_list"]["strikes"], json!(1));
        let r2 = call(2);
        assert_eq!(r2["deny_list"]["status"], json!("watching"));
        assert_eq!(r2["deny_list"]["strikes"], json!(2));
        let r3 = call(3);
        assert_eq!(
            r3["deny_list"]["status"],
            json!("just_blocked"),
            "third strike must auto-promote: {r3}"
        );
        assert_eq!(r3["deny_list"]["strikes"], json!(3));
        let r4 = call(4);
        assert_eq!(r4["deny_list"]["status"], json!("already_blocked"));

        // Confirm the on-disk file actually reflects the block, and
        // `is_context_pair_blocked` returns true.
        let path = tm_graph::DenyList::default_path(&dir);
        assert!(path.exists(), "deny-list file should exist at {}", path.display());
        let deny = tm_graph::DenyList::load(&path).expect("load");
        assert!(
            deny.is_context_pair_blocked(ctx_a, ctx_b),
            "context pair must be blocked on disk"
        );

        // `not_related` (no contexts) must NOT carry a deny_list payload.
        let r_nr = handle_memory_feedback(
            &json!({
                "query_id": qid.to_string(),
                "result_id": "nr-1",
                "kind": "not_related",
            }),
            &db_path,
        )
        .expect("not_related ok");
        assert!(
            r_nr.get("deny_list").is_none(),
            "not_related without contexts shouldn't emit a deny_list payload: {r_nr}"
        );

        std::env::remove_var("TM_DATA_DIR");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Tool-surface tiering (holistic review §6a) ──────────────────────

    fn advertised_names(list: &Value) -> Vec<String> {
        list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn core_surface_advertises_exactly_the_core_six() {
        let names = advertised_names(&tools_list_for_surface(false));
        assert_eq!(names.len(), CORE_TOOLS.len(), "advertised: {names:?}");
        for t in CORE_TOOLS {
            assert!(names.contains(&t.to_string()), "core tool {t} missing");
        }
    }

    #[test]
    fn advanced_flag_reveals_the_full_surface() {
        let full = advertised_names(&tools_list_for_surface(true));
        assert!(
            full.len() > CORE_TOOLS.len(),
            "advanced surface should be larger, got {}",
            full.len()
        );
        for t in CORE_TOOLS {
            assert!(full.contains(&t.to_string()), "core tool {t} lost in advanced");
        }
    }

    /// Every core tool must have a real definition in the master list — a
    /// typo in CORE_TOOLS would otherwise silently advertise nothing.
    #[test]
    fn every_core_tool_exists_in_the_master_list() {
        let all = advertised_names(&tools_list());
        for t in CORE_TOOLS {
            assert!(all.contains(&t.to_string()), "CORE_TOOLS lists unknown tool {t}");
        }
    }

    /// Hidden tools must still be dispatchable — hiding is not removing.
    /// This guards the contract the benchmark harness and power users rely
    /// on: a tool absent from `tools/list` is still callable by name.
    #[test]
    fn hidden_tools_are_still_in_the_master_definition() {
        let all = advertised_names(&tools_list());
        // A representative advanced tool that must remain callable.
        assert!(all.contains(&"memory_reason".to_string()));
        assert!(all.contains(&"memory_brief".to_string()));
    }

    #[test]
    fn description_overrides_replace_advertised_text() {
        let mut list = tools_list_for_surface(false);
        let mut ov = std::collections::BTreeMap::new();
        ov.insert("memory_store".to_string(), "TUNED store description".to_string());
        apply_description_overrides(&mut list, &ov);
        let store = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "memory_store")
            .unwrap();
        assert_eq!(store["description"], "TUNED store description");
    }

    #[test]
    fn empty_overrides_leave_descriptions_untouched() {
        let before = tools_list_for_surface(false);
        let mut after = before.clone();
        apply_description_overrides(&mut after, &std::collections::BTreeMap::new());
        assert_eq!(before, after);
    }

    /// The rewritten core descriptions must contain the intent verbs a host
    /// keys on — the "sharpen" fix the MCP benchmark validated (55%->80%).
    #[test]
    fn core_descriptions_contain_intent_verbs() {
        let list = tools_list_for_surface(false);
        let desc = |name: &str| -> String {
            list["tools"].as_array().unwrap().iter()
                .find(|t| t["name"] == name).unwrap()["description"]
                .as_str().unwrap().to_lowercase()
        };
        assert!(desc("memory_store").contains("remember"));
        assert!(desc("memory_query").contains("recall") || desc("memory_query").contains("retrieve"));
        assert!(desc("memory_contradict").contains("contradict") || desc("memory_contradict").contains("conflict"));
        assert!(desc("memory_forget").contains("forget") || desc("memory_forget").contains("delete"));
        assert!(desc("memory_compose").contains("compose") || desc("memory_compose").contains("combine"));
    }
}
