//! Ingestion Review (I5/I7) — Tauri commands backing `IngestionReviewView.tsx`.
//!
//! The plan (docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md §3.3) makes the
//! "librarian, not wire-tap" contract concrete by giving the user a daily
//! digest of every capture receipt and three per-item actions:
//!
//! * **Keep** — no-op; the capture stays as-is. Surfaced as a button rather
//!   than a silent default so the review is *interactive* — the user's own
//!   engagement is a signal we can eventually count against the receipt log.
//! * **Refine** — the user rewrites the captured text. We create a fresh
//!   ingest (so trace provenance is preserved) and record a *refined-from*
//!   pointer on the new receipt.
//! * **Discard** — the user marks the capture as unwanted. We do NOT
//!   silently rewrite history: the original receipt stays in
//!   `receipts.jsonl` (the S1 commandment forbids losing audit records).
//!   A companion `discarded.jsonl` log captures the discard event so the
//!   Review view can hide it on the next render and the auditor can see
//!   both actions in order.
//!
//! Persistence is intentionally file-based: keeping this off the SQLite
//! schema means the Review view is legible without a running daemon and
//! the same JSONL an operator can grep is what the UI reads.
//!
//! The `IngestionPipeline` reference is threaded in through `AppState` so
//! Refine goes through the exact same governance / entity extraction that
//! a first-time capture would — no shadow path.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use tm_ingest::{ModeManager, ReceiptStore};
use tm_types::{CaptureMode, ModeSession};

use crate::AppState;

pub const DISCARDED_FILE_NAME: &str = "discarded.jsonl";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// One row rendered by `IngestionReviewView.tsx`. Kept flat (no nested
/// objects) so the React list can render without a schema library.
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptRow {
    pub capture_id: String,
    pub at: DateTime<Utc>,
    pub source: String,
    pub app_context: Option<String>,
    pub modality: String,
    pub size_bytes: usize,
    pub user_intent: Option<String>,
    pub why_captured: String,
    /// True when the user already discarded this row. The UI hides these
    /// by default but the Review view can toggle to show the full log.
    pub discarded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DiscardRecord {
    pub capture_id: Uuid,
    pub at: DateTime<Utc>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefineResult {
    pub new_trace_id: String,
    pub entities_extracted: usize,
    pub refined_from: String,
}

// ---------------------------------------------------------------------------
// Discard log
// ---------------------------------------------------------------------------

fn discarded_path(dir: &Path) -> PathBuf {
    dir.join(DISCARDED_FILE_NAME)
}

fn load_discarded(dir: &Path) -> Vec<DiscardRecord> {
    let path = discarded_path(dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(_) => return Vec::new(),
    };
    raw.split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<DiscardRecord>(l).ok())
        .collect()
}

fn append_discarded(dir: &Path, rec: &DiscardRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = discarded_path(dir);
    let mut line = serde_json::to_string(rec).unwrap_or_default();
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;
    f.write_all(line.as_bytes())
}

// ---------------------------------------------------------------------------
// Path resolver
// ---------------------------------------------------------------------------

/// Data directory the receipt log lives in. Matches `data_dir()` in the
/// MCP binary — env-override + `~/.tracemind/` fallback — so both surfaces
/// read the same file.
fn data_dir_from_state(state: &AppState) -> PathBuf {
    if let Ok(dir) = std::env::var("TM_DATA_DIR") {
        return PathBuf::from(dir);
    }
    // db_path is `<dir>/memory.db`; the receipt log is a sibling.
    let db = PathBuf::from(&state.db_path);
    db.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Return the last `limit` receipts with a `discarded` flag applied.
///
/// The digest is capped (default 200) so the review view can render without
/// paging — the plan calls out that daily review is a short ritual, not an
/// admin console.
#[tauri::command]
pub fn cmd_ingestion_review_recent(
    limit: Option<usize>,
    include_discarded: Option<bool>,
    state: State<AppState>,
) -> Result<Vec<ReceiptRow>, String> {
    let dir = data_dir_from_state(&state);
    let store = ReceiptStore::open(&dir).map_err(|e| e.to_string())?;
    let receipts = store
        .recent(limit.unwrap_or(200))
        .map_err(|e| e.to_string())?;
    let discarded: std::collections::HashSet<Uuid> = load_discarded(&dir)
        .into_iter()
        .map(|d| d.capture_id)
        .collect();
    let hide = !include_discarded.unwrap_or(false);
    let mut rows: Vec<ReceiptRow> = receipts
        .into_iter()
        .filter_map(|r| {
            let is_discarded = discarded.contains(&r.capture_id);
            if hide && is_discarded {
                return None;
            }
            Some(ReceiptRow {
                capture_id: r.capture_id.to_string(),
                at: r.at,
                source: r.source,
                app_context: r.app_context,
                modality: r.modality,
                size_bytes: r.size_bytes,
                user_intent: r.user_intent,
                why_captured: r.why_captured,
                discarded: is_discarded,
            })
        })
        .collect();
    // Newest first for the review view.
    rows.reverse();
    Ok(rows)
}

/// Mark a capture as discarded. Idempotent — a repeat call rewrites nothing
/// (we still append but the load-side dedupes on `capture_id`).
#[tauri::command]
pub fn cmd_ingestion_discard(
    capture_id: String,
    reason: Option<String>,
    state: State<AppState>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&capture_id).map_err(|e| e.to_string())?;
    let dir = data_dir_from_state(&state);
    let rec = DiscardRecord {
        capture_id: id,
        at: Utc::now(),
        reason,
    };
    append_discarded(&dir, &rec).map_err(|e| e.to_string())?;
    Ok(())
}

/// Refine a capture by re-ingesting user-edited text. The original receipt
/// is preserved (S1). The new capture goes through the exact same
/// `IngestPipeline` a first-time capture would — no shadow path.
///
/// Returns the new trace id + entity count so the UI can render "refined
/// into <n> entities" inline without a follow-up call.
#[tauri::command]
pub fn cmd_ingestion_refine(
    capture_id: String,
    new_text: String,
    state: State<AppState>,
) -> Result<RefineResult, String> {
    let original_id = Uuid::parse_str(&capture_id).map_err(|e| e.to_string())?;
    let trimmed = new_text.trim();
    if trimmed.is_empty() {
        return Err("refine text is empty".into());
    }
    let pipeline = state.ingest.lock().map_err(|e| e.to_string())?;
    let session_id = Uuid::new_v4();
    let result = pipeline
        .ingest(trimmed, session_id)
        .map_err(|e| e.to_string())?;

    // Best-effort: record the refine as a discard of the original so it
    // drops out of the default review view. If the discard write fails
    // (disk full, permissions) we still return success — the refine
    // ingest is the real work.
    let dir = data_dir_from_state(&state);
    let _ = append_discarded(
        &dir,
        &DiscardRecord {
            capture_id: original_id,
            at: Utc::now(),
            reason: Some("refined".into()),
        },
    );

    Ok(RefineResult {
        new_trace_id: result.trace.id.to_string(),
        entities_extracted: result.entities.len(),
        refined_from: capture_id,
    })
}

// ---------------------------------------------------------------------------
// I6/I9 — Three-mode selector (Ambient / Focus / Private)
// ---------------------------------------------------------------------------

/// Snapshot of the current ingestion mode + active session, if any.
#[derive(Debug, Clone, Serialize)]
pub struct ModeStatus {
    pub mode: String,
    pub session: Option<SessionRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: String,
    pub mode: String,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub duration_secs: i64,
    pub ended_at: Option<DateTime<Utc>>,
}

impl From<ModeSession> for SessionRow {
    fn from(s: ModeSession) -> Self {
        Self {
            id: s.id.to_string(),
            mode: s.mode.as_str().to_string(),
            name: s.name,
            started_at: s.started_at,
            duration_secs: s.duration_secs,
            ended_at: s.ended_at,
        }
    }
}

fn open_mode_mgr(state: &AppState) -> Result<ModeManager, String> {
    let dir = data_dir_from_state(state);
    ModeManager::open(&dir).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_mode_current(state: State<AppState>) -> Result<ModeStatus, String> {
    let mgr = open_mode_mgr(&state)?;
    let now = Utc::now();
    let mode = mgr.current(now);
    let session = mgr.active_session(now).map(SessionRow::from);
    Ok(ModeStatus {
        mode: mode.as_str().to_string(),
        session,
    })
}

#[tauri::command]
pub fn cmd_mode_enter_focus(
    intent: String,
    duration_secs: Option<i64>,
    state: State<AppState>,
) -> Result<ModeStatus, String> {
    let mgr = open_mode_mgr(&state)?;
    let secs = duration_secs.unwrap_or(30 * 60);
    let session = mgr.enter_focus(intent, secs).map_err(|e| e.to_string())?;
    Ok(ModeStatus {
        mode: CaptureMode::Focus.as_str().to_string(),
        session: Some(session.into()),
    })
}

#[tauri::command]
pub fn cmd_mode_enter_private(
    duration_secs: Option<i64>,
    state: State<AppState>,
) -> Result<ModeStatus, String> {
    let mgr = open_mode_mgr(&state)?;
    let secs = duration_secs.unwrap_or(15 * 60);
    let session = mgr.enter_private(secs).map_err(|e| e.to_string())?;
    Ok(ModeStatus {
        mode: CaptureMode::Private.as_str().to_string(),
        session: Some(session.into()),
    })
}

#[tauri::command]
pub fn cmd_mode_end(state: State<AppState>) -> Result<ModeStatus, String> {
    let mgr = open_mode_mgr(&state)?;
    let ended = mgr.end_session(Utc::now()).map_err(|e| e.to_string())?;
    Ok(ModeStatus {
        mode: CaptureMode::Ambient.as_str().to_string(),
        session: ended.map(SessionRow::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discard_append_and_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        append_discarded(
            dir.path(),
            &DiscardRecord {
                capture_id: id,
                at: Utc::now(),
                reason: Some("noise".into()),
            },
        )
        .unwrap();
        let loaded = load_discarded(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].capture_id, id);
        assert_eq!(loaded[0].reason.as_deref(), Some("noise"));
    }

    #[test]
    fn load_discarded_from_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_discarded(dir.path()).is_empty());
    }
}
