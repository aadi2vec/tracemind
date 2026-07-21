//! Q3.1 — Feedback signal fabric.
//!
//! Stores all three signal classes (explicit / implicit / behavioral) as
//! first-class memory entries with UUID provenance, so downstream consumers
//! (GEPA, user-behavior model, Curator) can read them as graph queries.
//!
//! Every retrieval response carries a `feedback_hook_id` (a UUID minted at
//! query time). Callers pass that UUID back when recording any signal so
//! signals are linkable to the originating retrieval without a separate
//! lookup table.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use tm_types::{FeedbackClass, FeedbackKind, FeedbackSignal, Result, TraceMindError};
use uuid::Uuid;

/// Install the feedback_signals table. Idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS feedback_signals (
            id              TEXT NOT NULL PRIMARY KEY,
            kind            TEXT NOT NULL,
            class           TEXT NOT NULL,
            feedback_hook_id TEXT NOT NULL,
            target_id       TEXT,
            score           REAL NOT NULL DEFAULT 0.0,
            verb            TEXT,
            host_id         TEXT,
            session_id      TEXT,
            recorded_at     TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_fs_hook
            ON feedback_signals(feedback_hook_id);
        CREATE INDEX IF NOT EXISTS idx_fs_class
            ON feedback_signals(class, recorded_at);
        CREATE INDEX IF NOT EXISTS idx_fs_kind
            ON feedback_signals(kind, recorded_at);
        CREATE INDEX IF NOT EXISTS idx_fs_verb
            ON feedback_signals(verb, recorded_at);",
    )
    .map_err(|e| TraceMindError::Storage(format!("feedback_signals schema: {e}")))?;
    Ok(())
}

/// Record a feedback signal. Returns the signal's UUID.
pub fn record_signal(conn: &Connection, signal: &FeedbackSignal) -> Result<Uuid> {
    conn.execute(
        "INSERT INTO feedback_signals
            (id, kind, class, feedback_hook_id, target_id, score,
             verb, host_id, session_id, recorded_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            signal.id.to_string(),
            signal.kind.as_str(),
            signal.class.as_str(),
            signal.feedback_hook_id.to_string(),
            signal.target_id.map(|u| u.to_string()),
            signal.score as f64,
            signal.verb,
            signal.host_id,
            signal.session_id.map(|u| u.to_string()),
            signal.recorded_at.to_rfc3339(),
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("insert feedback_signal: {e}")))?;
    Ok(signal.id)
}

/// Read signals for a specific feedback_hook_id.
pub fn signals_for_hook(conn: &Connection, hook_id: Uuid) -> Result<Vec<FeedbackSignal>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, class, feedback_hook_id, target_id, score,
                    verb, host_id, session_id, recorded_at
             FROM feedback_signals
             WHERE feedback_hook_id = ?1
             ORDER BY recorded_at ASC",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(params![hook_id.to_string()], row_to_signal)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    Ok(rows)
}

/// Read the most recent N signals of a given class.
pub fn recent_signals_by_class(
    conn: &Connection,
    class: FeedbackClass,
    limit: usize,
) -> Result<Vec<FeedbackSignal>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, class, feedback_hook_id, target_id, score,
                    verb, host_id, session_id, recorded_at
             FROM feedback_signals
             WHERE class = ?1
             ORDER BY recorded_at DESC
             LIMIT ?2",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(params![class.as_str(), limit as i64], row_to_signal)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    Ok(rows)
}

/// Verb affinity: count how many times each verb was invoked, weighted by score.
/// Returns (verb, weighted_count) pairs sorted by weight desc.
pub fn verb_affinity(conn: &Connection, limit: usize) -> Result<Vec<(String, f64)>> {
    let mut stmt = conn
        .prepare(
            "SELECT verb, SUM(score) as weight
             FROM feedback_signals
             WHERE kind = 'verb_invoked' AND verb IS NOT NULL
             GROUP BY verb
             ORDER BY weight DESC
             LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    Ok(rows)
}

fn row_to_signal(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeedbackSignal> {
    let id: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let _class_str: String = row.get(2)?;
    let hook_str: String = row.get(3)?;
    let target_str: Option<String> = row.get(4)?;
    let score: f64 = row.get(5)?;
    let verb: Option<String> = row.get(6)?;
    let host_id: Option<String> = row.get(7)?;
    let session_str: Option<String> = row.get(8)?;
    let recorded_at_str: String = row.get(9)?;

    let kind = FeedbackKind::from_str(&kind_str)
        .ok_or_else(|| rusqlite::Error::InvalidColumnType(1, kind_str.clone(), rusqlite::types::Type::Text))?;

    Ok(FeedbackSignal {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
        class: kind.class(),
        kind,
        feedback_hook_id: Uuid::parse_str(&hook_str).unwrap_or_else(|_| Uuid::nil()),
        target_id: target_str.and_then(|s| Uuid::parse_str(&s).ok()),
        score: score as f32,
        verb,
        host_id,
        session_id: session_str.and_then(|s| Uuid::parse_str(&s).ok()),
        recorded_at: recorded_at_str
            .parse::<DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now()),
    })
}
