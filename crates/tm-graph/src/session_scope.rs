//! Q3.5 — session_id / host_id scoping across MCP surface.
//!
//! Every memory operation (ingest, query, retrieval) can be scoped to a
//! specific session + host pair. This lets TraceMind segregate context
//! from Claude Code sessions vs Goose sessions vs the capture daemon,
//! and retrieve only memories relevant to the current interaction context.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// A session scope record — created when a new MCP session starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionScope {
    pub id: Uuid,
    /// MCP host that initiated this session (e.g., "claude-code", "goose").
    pub host_id: String,
    /// Human-readable session label (often the first query of the session).
    pub label: Option<String>,
    pub started_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    /// Number of MCP calls in this session.
    pub call_count: u32,
}

impl SessionScope {
    pub fn new(host_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            host_id: host_id.into(),
            label: None,
            started_at: now,
            last_activity_at: now,
            call_count: 0,
        }
    }
}

/// Schema for session scope tracking. Idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_scopes (
            id              TEXT NOT NULL PRIMARY KEY,
            host_id         TEXT NOT NULL,
            label           TEXT,
            started_at      TEXT NOT NULL,
            last_activity   TEXT NOT NULL,
            call_count      INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_host ON session_scopes(host_id, started_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_start ON session_scopes(started_at);"
    ).map_err(|e| TraceMindError::Storage(format!("session_scopes schema: {e}")))?;
    Ok(())
}

/// Upsert (create or touch) a session scope. Returns the session UUID.
pub fn upsert_session(conn: &Connection, session: &SessionScope) -> Result<Uuid> {
    conn.execute(
        "INSERT INTO session_scopes (id, host_id, label, started_at, last_activity, call_count)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(id) DO UPDATE SET
           last_activity = excluded.last_activity,
           call_count = call_count + 1,
           label = COALESCE(excluded.label, label)",
        params![
            session.id.to_string(),
            session.host_id,
            session.label,
            session.started_at.to_rfc3339(),
            session.last_activity_at.to_rfc3339(),
            session.call_count,
        ],
    ).map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(session.id)
}

/// Get a session by ID.
pub fn get_session(conn: &Connection, id: Uuid) -> Result<Option<SessionScope>> {
    let mut stmt = conn.prepare(
        "SELECT id, host_id, label, started_at, last_activity, call_count
         FROM session_scopes WHERE id = ?1",
    ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let result = stmt.query_row(params![id.to_string()], |row| {
        let id_s: String = row.get(0)?;
        let host_id: String = row.get(1)?;
        let label: Option<String> = row.get(2)?;
        let started_s: String = row.get(3)?;
        let last_s: String = row.get(4)?;
        let call_count: u32 = row.get(5)?;
        Ok((id_s, host_id, label, started_s, last_s, call_count))
    });

    match result {
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(TraceMindError::Storage(e.to_string())),
        Ok((id_s, host_id, label, started_s, last_s, call_count)) => {
            Ok(Some(SessionScope {
                id: Uuid::parse_str(&id_s).unwrap_or_else(|_| Uuid::nil()),
                host_id,
                label,
                started_at: started_s.parse().unwrap_or_else(|_| Utc::now()),
                last_activity_at: last_s.parse().unwrap_or_else(|_| Utc::now()),
                call_count,
            }))
        }
    }
}

/// List recent sessions, newest first.
pub fn recent_sessions(
    conn: &Connection,
    host_id: Option<&str>,
    limit: usize,
) -> Result<Vec<SessionScope>> {
    let (sql, host_filter) = if let Some(h) = host_id {
        (
            "SELECT id, host_id, label, started_at, last_activity, call_count
             FROM session_scopes WHERE host_id = ?1 ORDER BY started_at DESC LIMIT ?2",
            h.to_string(),
        )
    } else {
        (
            "SELECT id, host_id, label, started_at, last_activity, call_count
             FROM session_scopes ORDER BY started_at DESC LIMIT ?1",
            limit.to_string(),
        )
    };

    let mut stmt = conn.prepare(sql)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let rows = if host_id.is_some() {
        stmt.query_map(params![host_filter, limit as i64], row_to_scope)
    } else {
        stmt.query_map(params![limit as i64], row_to_scope)
    }.map_err(|e| TraceMindError::Storage(e.to_string()))?
    .filter_map(|r| r.ok())
    .collect();

    Ok(rows)
}

fn row_to_scope(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionScope> {
    let id_s: String = row.get(0)?;
    let host_id: String = row.get(1)?;
    let label: Option<String> = row.get(2)?;
    let started_s: String = row.get(3)?;
    let last_s: String = row.get(4)?;
    let call_count: u32 = row.get(5)?;
    Ok(SessionScope {
        id: Uuid::parse_str(&id_s).unwrap_or_else(|_| Uuid::nil()),
        host_id,
        label,
        started_at: started_s.parse().unwrap_or_else(|_| Utc::now()),
        last_activity_at: last_s.parse().unwrap_or_else(|_| Utc::now()),
        call_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn upsert_creates_session() {
        let conn = setup();
        let session = SessionScope::new("claude-code");
        let id = upsert_session(&conn, &session).unwrap();
        let loaded = get_session(&conn, id).unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().host_id, "claude-code");
    }

    #[test]
    fn upsert_increments_call_count() {
        let conn = setup();
        let session = SessionScope::new("goose");
        upsert_session(&conn, &session).unwrap();
        let mut s2 = session.clone();
        s2.call_count = 0;
        upsert_session(&conn, &s2).unwrap();
        let loaded = get_session(&conn, session.id).unwrap().unwrap();
        assert_eq!(loaded.call_count, 1); // incremented by ON CONFLICT
    }

    #[test]
    fn recent_sessions_filters_by_host() {
        let conn = setup();
        upsert_session(&conn, &SessionScope::new("claude-code")).unwrap();
        upsert_session(&conn, &SessionScope::new("goose")).unwrap();
        upsert_session(&conn, &SessionScope::new("claude-code")).unwrap();
        let cc = recent_sessions(&conn, Some("claude-code"), 10).unwrap();
        assert_eq!(cc.len(), 2);
        assert!(cc.iter().all(|s| s.host_id == "claude-code"));
    }
}
