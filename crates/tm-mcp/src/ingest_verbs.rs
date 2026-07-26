//! MCP verbs for the ingestion-experience primitives (I6/I9, I10, I12–I18).
//!
//! Kept in one module so the rest of `main.rs` only grows by two lines: one
//! call to append descriptors into `tools/list`, one match-arm that dispatches
//! any unknown tool through [`dispatch`]. Every handler is synchronous —
//! these verbs touch JSONL / JSON-file stores only, no async work.
//!
//! All handlers accept `(&Value, &Path)` where the path is the TraceMind data
//! directory (usually `~/.tracemind/`). No handler touches the network or the
//! shell.

use std::path::Path;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use tm_governance::{AntiGoalRules, AppScopingPolicy};
use tm_ingest::{
    anchor::AnchorStore,
    merkle::{ExportLeaf, ExportManifest},
    modes::ModeManager,
    primitives::{
        ChainStore, ContradictionWatch, EphemeralStore, JournalKind, JournalStore,
        RetroWindowStore, TimeLockStore, WeeklyReviewStore,
    },
    question_queue::QuestionQueue,
    receipts::ReceiptStore,
};

/// Every tool name this module handles. Kept in sync with [`descriptors`] and
/// [`dispatch`] so a change to one triggers a diff on the other two.
pub const INGEST_TOOL_NAMES: &[&str] = &[
    "capture_mode_current",
    "capture_mode_set",
    "capture_mode_end",
    "question_pin",
    "question_list",
    "question_resolve",
    "anti_goal_list",
    "anti_goal_add",
    "anti_goal_remove",
    "app_scope_set",
    "app_scope_list",
    "chain_start",
    "chain_add",
    "chain_end",
    "chain_list",
    "capture_ephemeral",
    "ephemeral_sweep",
    "capture_time_lock",
    "time_lock_sweep",
    "contradiction_watch",
    "contradiction_unwatch",
    "journal_write",
    "journal_recent",
    "retro_request",
    "weekly_review_record",
    "weekly_review_list",
    "anchor_scan",
    "anchor_active",
    "anchor_retire",
    "receipt_recent",
    "memory_export_manifest",
];

/// JSON tool descriptors ready to append into the `tools/list` response.
pub fn descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "capture_mode_current",
            "description": "I6 — return the active ingestion mode (ambient / focus / private) and the current session (if any).",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "capture_mode_set",
            "description": "I6 — enter Focus or Private mode with a bounded duration. `mode` = 'focus' | 'private'. Focus requires `intent`. Duration defaults to 45 min (focus) / 15 min (private).",
            "inputSchema": {
                "type": "object",
                "required": ["mode"],
                "properties": {
                    "mode": {"type": "string", "enum": ["focus", "private", "ambient"]},
                    "intent": {"type": "string"},
                    "duration_secs": {"type": "integer"}
                }
            }
        }),
        json!({
            "name": "capture_mode_end",
            "description": "I6 — end the active Focus/Private session and revert to Ambient.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "question_pin",
            "description": "I10 / N1.2 — pin an open question so every future ingestion is scanned for it.",
            "inputSchema": {"type": "object", "required": ["text"], "properties": {"text": {"type": "string"}}}
        }),
        json!({
            "name": "question_list",
            "description": "I10 — list pinned questions, newest first. `status` = 'open' | 'all' (default 'open').",
            "inputSchema": {"type": "object", "properties": {"status": {"type": "string"}}}
        }),
        json!({
            "name": "question_resolve",
            "description": "I10 — mark a pinned question as resolved.",
            "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}}
        }),
        json!({
            "name": "anti_goal_list",
            "description": "I12 / N1.7 — list current anti-goal rules (url substrings, content tokens, source names).",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "anti_goal_add",
            "description": "I12 — add an anti-goal rule. `kind` = 'url' | 'token' | 'source'.",
            "inputSchema": {
                "type": "object",
                "required": ["kind", "value"],
                "properties": {"kind": {"type": "string"}, "value": {"type": "string"}}
            }
        }),
        json!({
            "name": "anti_goal_remove",
            "description": "I12 — remove an anti-goal rule.",
            "inputSchema": {
                "type": "object",
                "required": ["kind", "value"],
                "properties": {"kind": {"type": "string"}, "value": {"type": "string"}}
            }
        }),
        json!({
            "name": "app_scope_set",
            "description": "I13 / N1.8 — set per-app disposition. `disposition` = 'allow' | 'deny' | 'ask'.",
            "inputSchema": {
                "type": "object",
                "required": ["app", "disposition"],
                "properties": {"app": {"type": "string"}, "disposition": {"type": "string"}}
            }
        }),
        json!({
            "name": "app_scope_list",
            "description": "I13 — list per-app dispositions.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "chain_start",
            "description": "I15 / N2.1 — start a capture chain bundle. Returns chain_id.",
            "inputSchema": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}
        }),
        json!({
            "name": "chain_add",
            "description": "I15 — attach a capture id to an open chain.",
            "inputSchema": {
                "type": "object",
                "required": ["chain_id", "capture_id"],
                "properties": {"chain_id": {"type": "string"}, "capture_id": {"type": "string"}}
            }
        }),
        json!({
            "name": "chain_end",
            "description": "I15 — close an open chain bundle.",
            "inputSchema": {"type": "object", "required": ["chain_id"], "properties": {"chain_id": {"type": "string"}}}
        }),
        json!({
            "name": "chain_list",
            "description": "I15 — list chain bundles. `status` = 'active' | 'all' (default 'all').",
            "inputSchema": {"type": "object", "properties": {"status": {"type": "string"}}}
        }),
        json!({
            "name": "capture_ephemeral",
            "description": "I16 / N2.4 — capture a note that auto-deletes after `ttl_hours` (default 24) unless promoted.",
            "inputSchema": {
                "type": "object",
                "required": ["content"],
                "properties": {
                    "content": {"type": "string"},
                    "source": {"type": "string"},
                    "ttl_hours": {"type": "integer"}
                }
            }
        }),
        json!({
            "name": "ephemeral_sweep",
            "description": "I16 — drop every unpromoted ephemeral capture whose deadline has passed. Returns removed ids.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "capture_time_lock",
            "description": "I17 / N2.5 — schedule a memory to become retrievable at `unlock_at` (RFC3339).",
            "inputSchema": {
                "type": "object",
                "required": ["memory_id", "unlock_at"],
                "properties": {
                    "memory_id": {"type": "string"},
                    "unlock_at": {"type": "string"},
                    "note": {"type": "string"}
                }
            }
        }),
        json!({
            "name": "time_lock_sweep",
            "description": "I17 — mark every due time-locked memory as unlocked. Returns ids.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "contradiction_watch",
            "description": "I17 / N2.6 — mark a memory as high-priority for contradiction detection.",
            "inputSchema": {
                "type": "object",
                "required": ["memory_id"],
                "properties": {"memory_id": {"type": "string"}, "note": {"type": "string"}}
            }
        }),
        json!({
            "name": "contradiction_unwatch",
            "description": "I17 — remove a memory from the contradiction-watch list.",
            "inputSchema": {
                "type": "object",
                "required": ["memory_id"],
                "properties": {"memory_id": {"type": "string"}}
            }
        }),
        json!({
            "name": "journal_write",
            "description": "I14 / N1.5–N1.6 — append a journal entry. `kind` = 'end_of_day' | 'post_meeting' | 'freeform'.",
            "inputSchema": {
                "type": "object",
                "required": ["kind", "body"],
                "properties": {
                    "kind": {"type": "string"},
                    "prompt": {"type": "string"},
                    "body": {"type": "string"},
                    "linked_calendar_event": {"type": "string"}
                }
            }
        }),
        json!({
            "name": "journal_recent",
            "description": "I14 — most recent journal entries.",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}}}
        }),
        json!({
            "name": "retro_request",
            "description": "I15 / N2.2 — schedule re-derivation over an existing capture window. `start`/`end` RFC3339.",
            "inputSchema": {
                "type": "object",
                "required": ["start", "end", "prompt"],
                "properties": {
                    "start": {"type": "string"},
                    "end": {"type": "string"},
                    "prompt": {"type": "string"}
                }
            }
        }),
        json!({
            "name": "weekly_review_record",
            "description": "I18 / N2.7 — record the outcome of a Sunday-morning batch triage.",
            "inputSchema": {
                "type": "object",
                "required": ["week_start", "week_end", "summary"],
                "properties": {
                    "week_start": {"type": "string"},
                    "week_end": {"type": "string"},
                    "kept": {"type": "array", "items": {"type": "string"}},
                    "refined": {"type": "array", "items": {"type": "string"}},
                    "forgotten": {"type": "array", "items": {"type": "string"}},
                    "summary": {"type": "string"}
                }
            }
        }),
        json!({
            "name": "weekly_review_list",
            "description": "I18 — list recorded weekly-review checkpoints (newest first).",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "anchor_scan",
            "description": "I18 / N2.8 — register an NFC/QR anchor with an intent and ttl_minutes (default 60).",
            "inputSchema": {
                "type": "object",
                "required": ["tag", "intent"],
                "properties": {
                    "tag": {"type": "string"},
                    "intent": {"type": "string"},
                    "ttl_minutes": {"type": "integer"}
                }
            }
        }),
        json!({
            "name": "anchor_active",
            "description": "I18 — active intent driven by a live anchor, if any.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "anchor_retire",
            "description": "I18 — retire an anchor by id.",
            "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}}
        }),
        json!({
            "name": "receipt_recent",
            "description": "I1 (audit) — return the last N capture receipts from ~/.tracemind/receipts.jsonl.",
            "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer"}}}
        }),
        json!({
            "name": "memory_export_manifest",
            "description": "I18 / S10 — build a Merkle-proof manifest over caller-supplied leaves and return the root. Leaves: [{id, hash_hex}, ...]. Callers hash their own payloads with SHA-256.",
            "inputSchema": {
                "type": "object",
                "required": ["leaves"],
                "properties": {
                    "leaves": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["id", "hash"],
                            "properties": {"id": {"type": "string"}, "hash": {"type": "string"}}
                        }
                    }
                }
            }
        }),
    ]
}

/// Dispatch — returns `Some(...)` when `tool` is one of our verbs, `None`
/// otherwise (so the caller can fall through to its existing "unknown tool"
/// error).
pub fn dispatch(tool: &str, params: &Value, data_dir: &Path) -> Option<Result<Value, String>> {
    let out = match tool {
        "capture_mode_current" => Some(capture_mode_current(data_dir)),
        "capture_mode_set" => Some(capture_mode_set(params, data_dir)),
        "capture_mode_end" => Some(capture_mode_end(data_dir)),
        "question_pin" => Some(question_pin(params, data_dir)),
        "question_list" => Some(question_list(params, data_dir)),
        "question_resolve" => Some(question_resolve(params, data_dir)),
        "anti_goal_list" => Some(anti_goal_list(data_dir)),
        "anti_goal_add" => Some(anti_goal_add(params, data_dir)),
        "anti_goal_remove" => Some(anti_goal_remove(params, data_dir)),
        "app_scope_set" => Some(app_scope_set(params, data_dir)),
        "app_scope_list" => Some(app_scope_list(data_dir)),
        "chain_start" => Some(chain_start(params, data_dir)),
        "chain_add" => Some(chain_add(params, data_dir)),
        "chain_end" => Some(chain_end(params, data_dir)),
        "chain_list" => Some(chain_list(params, data_dir)),
        "capture_ephemeral" => Some(capture_ephemeral(params, data_dir)),
        "ephemeral_sweep" => Some(ephemeral_sweep(data_dir)),
        "capture_time_lock" => Some(capture_time_lock(params, data_dir)),
        "time_lock_sweep" => Some(time_lock_sweep(data_dir)),
        "contradiction_watch" => Some(contradiction_watch(params, data_dir)),
        "contradiction_unwatch" => Some(contradiction_unwatch(params, data_dir)),
        "journal_write" => Some(journal_write(params, data_dir)),
        "journal_recent" => Some(journal_recent(params, data_dir)),
        "retro_request" => Some(retro_request(params, data_dir)),
        "weekly_review_record" => Some(weekly_review_record(params, data_dir)),
        "weekly_review_list" => Some(weekly_review_list(data_dir)),
        "anchor_scan" => Some(anchor_scan(params, data_dir)),
        "anchor_active" => Some(anchor_active(data_dir)),
        "anchor_retire" => Some(anchor_retire(params, data_dir)),
        "receipt_recent" => Some(receipt_recent(params, data_dir)),
        "memory_export_manifest" => Some(memory_export_manifest(params)),
        _ => None,
    };
    out
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn err(msg: impl std::fmt::Display) -> String {
    msg.to_string()
}

fn parse_uuid(params: &Value, key: &str) -> Result<Uuid, String> {
    let raw = params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{key}'"))?;
    Uuid::parse_str(raw).map_err(|e| format!("invalid uuid for '{key}': {e}"))
}

fn parse_rfc3339(params: &Value, key: &str) -> Result<DateTime<Utc>, String> {
    let raw = params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing '{key}'"))?;
    DateTime::parse_from_rfc3339(raw)
        .map_err(|e| format!("invalid RFC3339 '{key}': {e}"))
        .map(|dt| dt.with_timezone(&Utc))
}

fn parse_uuid_array(params: &Value, key: &str) -> Vec<Uuid> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| Uuid::parse_str(s).ok())
                .collect()
        })
        .unwrap_or_default()
}

// --- modes ---

fn capture_mode_current(data_dir: &Path) -> Result<Value, String> {
    let mgr = ModeManager::open(data_dir).map_err(err)?;
    let now = Utc::now();
    let mode = mgr.current(now);
    let session = mgr.active_session(now);
    Ok(json!({
        "mode": mode.as_str(),
        "session": session,
    }))
}

fn capture_mode_set(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .ok_or("missing 'mode'")?;
    let mgr = ModeManager::open(data_dir).map_err(err)?;
    let duration_secs = params
        .get("duration_secs")
        .and_then(|v| v.as_i64());
    let session = match mode.to_ascii_lowercase().as_str() {
        "focus" => {
            let intent = params
                .get("intent")
                .and_then(|v| v.as_str())
                .unwrap_or("focus");
            let secs = duration_secs.unwrap_or(45 * 60);
            mgr.enter_focus(intent, secs).map_err(err)?
        }
        "private" => {
            let secs = duration_secs.unwrap_or(15 * 60);
            mgr.enter_private(secs).map_err(err)?
        }
        "ambient" => {
            mgr.end_session(Utc::now()).map_err(err)?;
            return Ok(json!({"mode": "ambient", "session": Value::Null}));
        }
        other => return Err(format!("unknown mode '{other}'")),
    };
    Ok(json!({"mode": session.mode.as_str(), "session": session}))
}

fn capture_mode_end(data_dir: &Path) -> Result<Value, String> {
    let mgr = ModeManager::open(data_dir).map_err(err)?;
    let ended = mgr.end_session(Utc::now()).map_err(err)?;
    Ok(json!({"ended": ended}))
}

// --- question queue ---

fn question_pin(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("missing 'text'")?;
    let q = QuestionQueue::open(data_dir).map_err(err)?;
    let pinned = q.pin(text).map_err(err)?;
    Ok(json!({"question": pinned}))
}

fn question_list(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let status = params.get("status").and_then(|v| v.as_str()).unwrap_or("open");
    let q = QuestionQueue::open(data_dir).map_err(err)?;
    let list = if status == "all" { q.list() } else { q.open_questions() };
    Ok(json!({"questions": list}))
}

fn question_resolve(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let id = parse_uuid(params, "id")?;
    let q = QuestionQueue::open(data_dir).map_err(err)?;
    let resolved = q.resolve(id).map_err(err)?;
    Ok(json!({"resolved": resolved}))
}

// --- anti-goal rules ---

fn anti_goal_list(data_dir: &Path) -> Result<Value, String> {
    let rules = AntiGoalRules::load_or_default(data_dir);
    Ok(json!({
        "url_substrings": rules.url_substrings,
        "content_tokens": rules.content_tokens,
        "source_names": rules.source_names,
    }))
}

fn anti_goal_add(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let kind = params.get("kind").and_then(|v| v.as_str()).ok_or("missing 'kind'")?;
    let value = params.get("value").and_then(|v| v.as_str()).ok_or("missing 'value'")?;
    let mut rules = AntiGoalRules::load_or_default(data_dir);
    match kind {
        "url" => rules.add_url(value),
        "token" => rules.add_token(value),
        "source" => rules.add_source(value),
        other => return Err(format!("unknown kind '{other}'")),
    }
    rules.save(data_dir).map_err(err)?;
    Ok(json!({"added": {"kind": kind, "value": value}}))
}

fn anti_goal_remove(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let kind = params.get("kind").and_then(|v| v.as_str()).ok_or("missing 'kind'")?;
    let value = params.get("value").and_then(|v| v.as_str()).ok_or("missing 'value'")?;
    let mut rules = AntiGoalRules::load_or_default(data_dir);
    let removed = match kind {
        "url" => rules.remove_url(value),
        "token" => rules.remove_token(value),
        "source" => rules.remove_source(value),
        other => return Err(format!("unknown kind '{other}'")),
    };
    rules.save(data_dir).map_err(err)?;
    Ok(json!({"removed": removed, "kind": kind, "value": value}))
}

// --- app scoping ---

fn app_scope_set(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let app = params.get("app").and_then(|v| v.as_str()).ok_or("missing 'app'")?;
    let disposition = params
        .get("disposition")
        .and_then(|v| v.as_str())
        .ok_or("missing 'disposition'")?;
    let mut policy = AppScopingPolicy::load_or_default(data_dir);
    match disposition {
        "allow" => policy.allow(app),
        "deny" => policy.deny(app),
        "ask" => policy.ask(app),
        other => return Err(format!("unknown disposition '{other}'")),
    }
    policy.save(data_dir).map_err(err)?;
    Ok(json!({"app": app, "disposition": disposition}))
}

fn app_scope_list(data_dir: &Path) -> Result<Value, String> {
    let policy = AppScopingPolicy::load_or_default(data_dir);
    Ok(json!({
        "rules": policy.rules,
        "prompted": policy.prompted,
        "default_disposition": policy.default_disposition,
    }))
}

// --- chain bundles ---

fn chain_start(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let name = params.get("name").and_then(|v| v.as_str()).ok_or("missing 'name'")?;
    let store = ChainStore::open(data_dir).map_err(err)?;
    let chain = store.start(name).map_err(err)?;
    Ok(json!({"chain": chain}))
}

fn chain_add(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let cid = parse_uuid(params, "chain_id")?;
    let cap = parse_uuid(params, "capture_id")?;
    let store = ChainStore::open(data_dir).map_err(err)?;
    let updated = store.add(cid, cap).map_err(err)?;
    Ok(json!({"chain": updated}))
}

fn chain_end(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let cid = parse_uuid(params, "chain_id")?;
    let store = ChainStore::open(data_dir).map_err(err)?;
    let ended = store.end(cid).map_err(err)?;
    Ok(json!({"chain": ended}))
}

fn chain_list(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let status = params.get("status").and_then(|v| v.as_str()).unwrap_or("all");
    let store = ChainStore::open(data_dir).map_err(err)?;
    let list = if status == "active" { store.active() } else { store.list() };
    Ok(json!({"chains": list}))
}

// --- ephemeral ---

fn capture_ephemeral(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let content = params.get("content").and_then(|v| v.as_str()).ok_or("missing 'content'")?;
    let source = params.get("source").and_then(|v| v.as_str()).unwrap_or("mcp");
    let ttl_hours = params.get("ttl_hours").and_then(|v| v.as_i64()).unwrap_or(24);
    let store = EphemeralStore::open(data_dir).map_err(err)?;
    let item = store.capture(content, source, ttl_hours).map_err(err)?;
    Ok(json!({"ephemeral": item}))
}

fn ephemeral_sweep(data_dir: &Path) -> Result<Value, String> {
    let store = EphemeralStore::open(data_dir).map_err(err)?;
    let removed = store.sweep(Utc::now()).map_err(err)?;
    Ok(json!({"removed": removed}))
}

// --- time-locked ---

fn capture_time_lock(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let mem = parse_uuid(params, "memory_id")?;
    let unlock = parse_rfc3339(params, "unlock_at")?;
    let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
    let store = TimeLockStore::open(data_dir).map_err(err)?;
    let entry = store.lock_until(mem, unlock, note).map_err(err)?;
    Ok(json!({"lock": entry}))
}

fn time_lock_sweep(data_dir: &Path) -> Result<Value, String> {
    let store = TimeLockStore::open(data_dir).map_err(err)?;
    let ids = store.sweep(Utc::now()).map_err(err)?;
    Ok(json!({"unlocked": ids}))
}

// --- contradiction watch ---

fn contradiction_watch(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let mem = parse_uuid(params, "memory_id")?;
    let note = params.get("note").and_then(|v| v.as_str()).unwrap_or("");
    let store = ContradictionWatch::open(data_dir).map_err(err)?;
    let entry = store.watch(mem, note).map_err(err)?;
    Ok(json!({"watch": entry}))
}

fn contradiction_unwatch(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let mem = parse_uuid(params, "memory_id")?;
    let store = ContradictionWatch::open(data_dir).map_err(err)?;
    let removed = store.unwatch(mem).map_err(err)?;
    Ok(json!({"removed": removed}))
}

// --- journal ---

fn journal_write(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let kind_str = params.get("kind").and_then(|v| v.as_str()).ok_or("missing 'kind'")?;
    let kind = match kind_str {
        "end_of_day" => JournalKind::EndOfDay,
        "post_meeting" => JournalKind::PostMeeting,
        "freeform" => JournalKind::Freeform,
        other => return Err(format!("unknown journal kind '{other}'")),
    };
    let body = params.get("body").and_then(|v| v.as_str()).ok_or("missing 'body'")?;
    let prompt = params.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let linked = params.get("linked_calendar_event").and_then(|v| v.as_str()).map(str::to_string);
    let store = JournalStore::open(data_dir).map_err(err)?;
    let entry = store.write(kind, prompt, body, linked).map_err(err)?;
    Ok(json!({"entry": entry}))
}

fn journal_recent(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let store = JournalStore::open(data_dir).map_err(err)?;
    Ok(json!({"entries": store.recent(limit)}))
}

// --- retro ---

fn retro_request(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let start = parse_rfc3339(params, "start")?;
    let end = parse_rfc3339(params, "end")?;
    let prompt = params.get("prompt").and_then(|v| v.as_str()).ok_or("missing 'prompt'")?;
    if end <= start {
        return Err("end must be after start".into());
    }
    let store = RetroWindowStore::open(data_dir).map_err(err)?;
    let w = store.request(start, end, prompt).map_err(err)?;
    Ok(json!({"retro": w}))
}

// --- weekly review ---

fn weekly_review_record(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let start = parse_rfc3339(params, "week_start")?;
    let end = parse_rfc3339(params, "week_end")?;
    let summary = params.get("summary").and_then(|v| v.as_str()).ok_or("missing 'summary'")?;
    let kept = parse_uuid_array(params, "kept");
    let refined = parse_uuid_array(params, "refined");
    let forgotten = parse_uuid_array(params, "forgotten");
    let store = WeeklyReviewStore::open(data_dir).map_err(err)?;
    let review = store.record(start, end, kept, refined, forgotten, summary).map_err(err)?;
    Ok(json!({"review": review}))
}

fn weekly_review_list(data_dir: &Path) -> Result<Value, String> {
    let store = WeeklyReviewStore::open(data_dir).map_err(err)?;
    Ok(json!({"reviews": store.list()}))
}

// --- anchor ---

fn anchor_scan(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let tag = params.get("tag").and_then(|v| v.as_str()).ok_or("missing 'tag'")?;
    let intent = params.get("intent").and_then(|v| v.as_str()).ok_or("missing 'intent'")?;
    let ttl = params.get("ttl_minutes").and_then(|v| v.as_i64()).unwrap_or(60);
    let store = AnchorStore::open(data_dir).map_err(err)?;
    let anchor = store.scan(tag, intent, ttl).map_err(err)?;
    Ok(json!({"anchor": anchor}))
}

fn anchor_active(data_dir: &Path) -> Result<Value, String> {
    let store = AnchorStore::open(data_dir).map_err(err)?;
    Ok(json!({"intent": store.active_intent(Utc::now())}))
}

fn anchor_retire(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let id = parse_uuid(params, "id")?;
    let store = AnchorStore::open(data_dir).map_err(err)?;
    let out = store.retire(id).map_err(err)?;
    Ok(json!({"retired": out}))
}

// --- receipts ---

fn receipt_recent(params: &Value, data_dir: &Path) -> Result<Value, String> {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    let store = ReceiptStore::open(data_dir).map_err(err)?;
    let recent = store.recent(limit).map_err(err)?;
    Ok(json!({"receipts": recent}))
}

// --- merkle export ---

fn memory_export_manifest(params: &Value) -> Result<Value, String> {
    let leaves_raw = params
        .get("leaves")
        .and_then(|v| v.as_array())
        .ok_or("missing 'leaves' array")?;
    let mut leaves = Vec::with_capacity(leaves_raw.len());
    for entry in leaves_raw {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("leaf missing 'id'")?
            .to_string();
        let hash = entry
            .get("hash")
            .and_then(|v| v.as_str())
            .ok_or("leaf missing 'hash'")?
            .to_string();
        leaves.push(ExportLeaf { id, hash });
    }
    let manifest = ExportManifest::build(leaves);
    Ok(json!({"manifest": manifest, "verified": manifest.verify()}))
}

// Silence unused-import lints when the module compiles without every helper
// exercised — `Duration`/`TimeZone` are convenient for future handlers.
#[allow(dead_code)]
fn _keep_imports(_: Duration, _: chrono::LocalResult<DateTime<Utc>>) {}
#[allow(dead_code)]
fn _tz_probe() {
    let _ = Utc.timestamp_opt(0, 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn descriptor_names_match_dispatch() {
        let names: Vec<String> = descriptors()
            .iter()
            .map(|v| v.get("name").and_then(|n| n.as_str()).unwrap().to_string())
            .collect();
        let expected: Vec<String> = INGEST_TOOL_NAMES.iter().map(|s| s.to_string()).collect();
        assert_eq!(names.len(), expected.len(), "descriptor / name-list length");
        // Every declared name has a descriptor.
        for name in &expected {
            assert!(names.contains(name), "missing descriptor for {name}");
        }
    }

    #[test]
    fn dispatch_returns_none_for_unknown() {
        let dir = tempdir().unwrap();
        let res = dispatch("this_verb_does_not_exist", &json!({}), dir.path());
        assert!(res.is_none());
    }

    #[test]
    fn capture_mode_set_focus_and_current() {
        let dir = tempdir().unwrap();
        let r = dispatch(
            "capture_mode_set",
            &json!({"mode": "focus", "intent": "call-prep", "duration_secs": 60}),
            dir.path(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(r["mode"], "focus");
        let cur = dispatch("capture_mode_current", &json!({}), dir.path()).unwrap().unwrap();
        assert_eq!(cur["mode"], "focus");
    }

    #[test]
    fn question_pin_and_list_roundtrip() {
        let dir = tempdir().unwrap();
        dispatch("question_pin", &json!({"text": "who owns billing?"}), dir.path())
            .unwrap()
            .unwrap();
        let list = dispatch("question_list", &json!({}), dir.path()).unwrap().unwrap();
        let arr = list["questions"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["text"], "who owns billing?");
    }

    #[test]
    fn anti_goal_add_and_list() {
        let dir = tempdir().unwrap();
        dispatch(
            "anti_goal_add",
            &json!({"kind": "url", "value": "banking"}),
            dir.path(),
        )
        .unwrap()
        .unwrap();
        let list = dispatch("anti_goal_list", &json!({}), dir.path()).unwrap().unwrap();
        let urls = list["url_substrings"].as_array().unwrap();
        assert!(urls.iter().any(|v| v == "banking"));
    }

    #[test]
    fn chain_lifecycle_via_dispatch() {
        let dir = tempdir().unwrap();
        let started = dispatch("chain_start", &json!({"name": "debug"}), dir.path())
            .unwrap()
            .unwrap();
        let cid = started["chain"]["id"].as_str().unwrap();
        let cap = Uuid::new_v4().to_string();
        dispatch("chain_add", &json!({"chain_id": cid, "capture_id": cap}), dir.path())
            .unwrap()
            .unwrap();
        dispatch("chain_end", &json!({"chain_id": cid}), dir.path())
            .unwrap()
            .unwrap();
        let list = dispatch("chain_list", &json!({"status": "active"}), dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(list["chains"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn merkle_export_manifest_returns_root_and_verifies() {
        let dir = tempdir().unwrap();
        let _ = dir;
        let params = json!({
            "leaves": [
                {"id": "a", "hash": "0000000000000000000000000000000000000000000000000000000000000001"},
                {"id": "b", "hash": "0000000000000000000000000000000000000000000000000000000000000002"}
            ]
        });
        let res = dispatch("memory_export_manifest", &params, std::env::temp_dir().as_path())
            .unwrap()
            .unwrap();
        assert_eq!(res["verified"], true);
        assert!(res["manifest"]["root"].as_str().is_some());
    }

    #[test]
    fn journal_write_and_recent() {
        let dir = tempdir().unwrap();
        dispatch(
            "journal_write",
            &json!({"kind": "end_of_day", "body": "shipped modes", "prompt": "what?"}),
            dir.path(),
        )
        .unwrap()
        .unwrap();
        let recent = dispatch("journal_recent", &json!({"limit": 5}), dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(recent["entries"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn capture_ephemeral_and_sweep() {
        let dir = tempdir().unwrap();
        dispatch(
            "capture_ephemeral",
            &json!({"content": "expiring", "ttl_hours": 1}),
            dir.path(),
        )
        .unwrap()
        .unwrap();
        // Not expired yet.
        let swept = dispatch("ephemeral_sweep", &json!({}), dir.path()).unwrap().unwrap();
        assert!(swept["removed"].as_array().unwrap().is_empty());
    }

    #[test]
    fn anchor_scan_and_active() {
        let dir = tempdir().unwrap();
        dispatch(
            "anchor_scan",
            &json!({"tag": "nfc:desk", "intent": "kitchen", "ttl_minutes": 60}),
            dir.path(),
        )
        .unwrap()
        .unwrap();
        let active = dispatch("anchor_active", &json!({}), dir.path()).unwrap().unwrap();
        assert_eq!(active["intent"], "kitchen");
    }
}
