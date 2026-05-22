//! WME-5 — Tauri IPC bridge for the Working Memory Engine.
//!
//! These commands expose the verb-first card surface
//! (Resume / Recall / Compare / Caution / Connect / Anticipate)
//! to the desktop UI so the Brief and Dashboard can render the
//! same cards the MCP `memory_cards` tool returns.
//!
//! Read path: `cmd_wme_cards` reads `wme_cards` directly via the
//! [`WorkingMemoryEngine::recent_cards`] API. No producer side-effects.
//!
//! Feedback path: `cmd_wme_feedback` writes a row to `wme_feedback`
//! and bumps the per-kind decayed outcome aggregator.

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use tm_reflect::{CardKind, FeedbackKind, WorkingMemoryEngine};

use crate::AppState;

/// Wire-format card shipped to the UI. Mirrors the JSON shape the MCP
/// `memory_cards` tool returns so a single TS interface can back both.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WmeCardView {
    pub id: String,
    /// One of "resume" | "recall" | "compare" | "caution" | "connect" | "anticipate".
    pub kind: String,
    pub target_id: String,
    pub statement: String,
    pub score: f32,
    pub relevance: f32,
    pub surprise: f32,
    pub recency: f32,
    pub outcome: f32,
    /// ISO-8601 local-time string, ready to render.
    pub created_at: String,
}

fn card_to_view(c: tm_reflect::Card) -> WmeCardView {
    WmeCardView {
        id: c.id.to_string(),
        kind: c.kind.as_str().to_string(),
        target_id: c.target_id,
        statement: c.statement,
        score: c.score,
        relevance: c.signals.relevance,
        surprise: c.signals.surprise,
        recency: c.signals.recency,
        outcome: c.signals.outcome,
        created_at: c
            .created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    }
}

fn parse_kind(s: &str) -> Result<CardKind, String> {
    match s {
        "resume" => Ok(CardKind::Resume),
        "recall" => Ok(CardKind::Recall),
        "compare" => Ok(CardKind::Compare),
        "caution" => Ok(CardKind::Caution),
        "connect" => Ok(CardKind::Connect),
        "anticipate" => Ok(CardKind::Anticipate),
        other => Err(format!(
            "invalid card kind '{other}': must be one of resume | recall | compare | caution | connect | anticipate"
        )),
    }
}

fn parse_feedback(s: &str) -> Result<FeedbackKind, String> {
    match s {
        "useful_now" => Ok(FeedbackKind::UsefulNow),
        "not_useful_now" => Ok(FeedbackKind::NotUsefulNow),
        "not_now_remind_later" => Ok(FeedbackKind::NotNowRemindLater),
        "dismiss_this_kind" => Ok(FeedbackKind::DismissThisKind),
        other => Err(format!(
            "invalid feedback '{other}': must be one of useful_now | not_useful_now | not_now_remind_later | dismiss_this_kind"
        )),
    }
}

/// Return the most recent persisted WME cards. Empty list (not an
/// error) if the WME has never pushed a card.
#[tauri::command]
pub fn cmd_wme_cards(
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<WmeCardView>, String> {
    let limit = limit.unwrap_or(20).clamp(1, 200);
    let engine = WorkingMemoryEngine::open(&state.db_path)
        .map_err(|e| format!("open wme engine: {e}"))?;
    let cards = engine
        .recent_cards(limit)
        .map_err(|e| format!("recent_cards: {e}"))?;
    Ok(cards.into_iter().map(card_to_view).collect())
}

/// Record one piece of feedback against a previously-surfaced card.
/// The UI already knows the card's `kind` from the render — passing
/// it through saves a SQLite roundtrip.
#[tauri::command]
pub fn cmd_wme_feedback(
    card_id: String,
    kind: String,
    feedback: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&card_id)
        .map_err(|e| format!("invalid card_id '{card_id}': {e}"))?;
    let card_kind = parse_kind(&kind)?;
    let fb = parse_feedback(&feedback)?;
    let engine = WorkingMemoryEngine::open(&state.db_path)
        .map_err(|e| format!("open wme engine: {e}"))?;
    engine
        .record_feedback(id, card_kind, fb)
        .map_err(|e| format!("record_feedback: {e}"))?;
    Ok(())
}
