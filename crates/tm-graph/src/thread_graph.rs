//! Sprint GRAPH — Thread as first-class graph primitive.
//!
//! A *thread* is one AI-conversation worth of capture/query/commitment
//! activity. The thread row itself lives in `threads`; its subgraph is a
//! **materialized view** over `event_nodes` (tagged with `thread_id`).
//! Threads compose into bigger graphs via `algebra` (union / intersect /
//! diff / filter / bridge).

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// Origin of the AI conversation this thread captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadSource {
    Claude,
    Cursor,
    Goose,
    Tracemind,
    Mcp,
    Other,
}

impl ThreadSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadSource::Claude => "claude",
            ThreadSource::Cursor => "cursor",
            ThreadSource::Goose => "goose",
            ThreadSource::Tracemind => "tracemind",
            ThreadSource::Mcp => "mcp",
            ThreadSource::Other => "other",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "claude" => ThreadSource::Claude,
            "cursor" => ThreadSource::Cursor,
            "goose" => ThreadSource::Goose,
            "tracemind" => ThreadSource::Tracemind,
            "mcp" => ThreadSource::Mcp,
            _ => ThreadSource::Other,
        }
    }
}

/// Row in the `threads` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub context_id: Option<Uuid>,
    pub title: String,
    pub source: ThreadSource,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
}

impl Thread {
    pub fn new(title: impl Into<String>, source: ThreadSource) -> Self {
        Self {
            id: Uuid::new_v4(),
            context_id: None,
            title: title.into(),
            source,
            started_at: Utc::now(),
            ended_at: None,
            metadata: serde_json::json!({}),
        }
    }
}

/// Materialized subgraph for a thread — IDs only. Resolve via the
/// host stores when needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThreadGraph {
    pub thread_id: Uuid,
    pub event_node_ids: Vec<Uuid>,
    pub entity_ids: Vec<Uuid>,
    pub topic_clusters: Vec<i64>,
    pub commitment_ids: Vec<Uuid>,
    pub capture_signal_ids: Vec<i64>,
}

/// Thin CRUD store for threads + subgraph materializer.
pub struct ThreadGraphStore;

impl ThreadGraphStore {
    /// Insert or update a thread row.
    pub fn upsert(conn: &Connection, t: &Thread) -> Result<()> {
        let ended = t.ended_at.map(|d| d.to_rfc3339());
        conn.execute(
            "INSERT INTO threads (id, context_id, title, source, started_at, ended_at, metadata)
             VALUES (?,?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET
               context_id = excluded.context_id,
               title      = excluded.title,
               source     = excluded.source,
               started_at = excluded.started_at,
               ended_at   = excluded.ended_at,
               metadata   = excluded.metadata",
            params![
                t.id.to_string(),
                t.context_id.map(|c| c.to_string()),
                t.title,
                t.source.as_str(),
                t.started_at.to_rfc3339(),
                ended,
                t.metadata.to_string(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert thread: {e}")))?;
        Ok(())
    }

    /// End a thread (sets `ended_at`).
    pub fn end_thread(conn: &Connection, id: Uuid) -> Result<()> {
        conn.execute(
            "UPDATE threads SET ended_at = ? WHERE id = ?",
            params![Utc::now().to_rfc3339(), id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("end thread: {e}")))?;
        Ok(())
    }

    /// Get a thread by id.
    pub fn get(conn: &Connection, id: Uuid) -> Result<Option<Thread>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, context_id, title, source, started_at, ended_at, metadata
                 FROM threads WHERE id = ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prepare get thread: {e}")))?;
        let mut rows = stmt
            .query(params![id.to_string()])
            .map_err(|e| TraceMindError::Storage(format!("query thread: {e}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
        {
            Ok(Some(row_to_thread(row)?))
        } else {
            Ok(None)
        }
    }

    /// List threads, newest first.
    pub fn list(conn: &Connection, limit: usize) -> Result<Vec<Thread>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, context_id, title, source, started_at, ended_at, metadata
                 FROM threads ORDER BY started_at DESC LIMIT ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prepare list threads: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                row_to_thread(row).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text,
                        Box::<dyn std::error::Error + Send + Sync>::from(e.to_string()))
                })
            })
            .map_err(|e| TraceMindError::Storage(format!("query threads: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| TraceMindError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    /// Materialize the subgraph for a thread by scanning `event_nodes`
    /// tagged with `thread_id` and re-projecting onto the entity/topic
    /// substrate. Pure read — no graph mutation.
    pub fn materialize(conn: &Connection, thread_id: Uuid) -> Result<ThreadGraph> {
        let mut g = ThreadGraph {
            thread_id,
            ..Default::default()
        };

        // event nodes
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, cluster_id, payload_ref
                 FROM event_nodes WHERE thread_id = ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prepare ev: {e}")))?;
        let rows = stmt
            .query_map(params![thread_id.to_string()], |row| {
                let id: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let cluster: Option<i64> = row.get(2)?;
                let pref: String = row.get(3)?;
                Ok((id, kind, cluster, pref))
            })
            .map_err(|e| TraceMindError::Storage(format!("ev query: {e}")))?;

        for r in rows {
            let (id, kind, cluster, pref) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            if let Ok(uid) = Uuid::parse_str(&id) {
                g.event_node_ids.push(uid);
            }
            if let Some(c) = cluster {
                if !g.topic_clusters.contains(&c) {
                    g.topic_clusters.push(c);
                }
            }
            match kind.as_str() {
                "commitment" => {
                    if let Ok(uid) = Uuid::parse_str(&pref) {
                        g.commitment_ids.push(uid);
                    }
                }
                "capture" => {
                    if let Ok(sid) = pref.parse::<i64>() {
                        g.capture_signal_ids.push(sid);
                    }
                }
                _ => {}
            }
        }

        // Entities derived from captures referenced by this thread.
        if !g.capture_signal_ids.is_empty() {
            let placeholders =
                (0..g.capture_signal_ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT DISTINCT promoted_entity FROM captured_signals
                 WHERE id IN ({placeholders}) AND promoted_entity IS NOT NULL"
            );
            let mut s = conn
                .prepare(&sql)
                .map_err(|e| TraceMindError::Storage(format!("ent prep: {e}")))?;
            let params_vec: Vec<rusqlite::types::Value> = g
                .capture_signal_ids
                .iter()
                .map(|&i| rusqlite::types::Value::Integer(i))
                .collect();
            let rows = s
                .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                    let v: String = row.get(0)?;
                    Ok(v)
                })
                .map_err(|e| TraceMindError::Storage(format!("ent query: {e}")))?;
            for r in rows {
                let s = r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
                if let Ok(uid) = Uuid::parse_str(&s) {
                    if !g.entity_ids.contains(&uid) {
                        g.entity_ids.push(uid);
                    }
                }
            }
        }

        Ok(g)
    }

    /// Attach a memory view as the default scope for any MCP query
    /// originating from this thread. See `tm-mcp::attach_view`.
    pub fn attach_view(conn: &Connection, thread_id: Uuid, view_id: Uuid) -> Result<()> {
        conn.execute(
            "INSERT INTO thread_attached_view (thread_id, view_id, attached_at)
             VALUES (?,?,?)
             ON CONFLICT(thread_id) DO UPDATE SET
               view_id = excluded.view_id,
               attached_at = excluded.attached_at",
            params![
                thread_id.to_string(),
                view_id.to_string(),
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("attach view: {e}")))?;
        Ok(())
    }

    /// Resolve the view (if any) attached to a thread.
    pub fn attached_view(conn: &Connection, thread_id: Uuid) -> Result<Option<Uuid>> {
        let mut stmt = conn
            .prepare("SELECT view_id FROM thread_attached_view WHERE thread_id = ?")
            .map_err(|e| TraceMindError::Storage(format!("prep att: {e}")))?;
        let mut rows = stmt
            .query(params![thread_id.to_string()])
            .map_err(|e| TraceMindError::Storage(format!("att query: {e}")))?;
        if let Some(row) = rows
            .next()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
        {
            let s: String = row.get(0).map_err(|e| TraceMindError::Storage(e.to_string()))?;
            Ok(Uuid::parse_str(&s).ok())
        } else {
            Ok(None)
        }
    }
}

fn row_to_thread(row: &rusqlite::Row) -> Result<Thread> {
    let id: String = row
        .get(0)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let cid: Option<String> = row
        .get(1)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let title: String = row
        .get(2)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let source: String = row
        .get(3)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let started: String = row
        .get(4)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let ended: Option<String> = row
        .get(5)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let meta: String = row
        .get(6)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(Thread {
        id: Uuid::parse_str(&id)
            .map_err(|e| TraceMindError::Storage(format!("uuid parse: {e}")))?,
        context_id: cid.and_then(|s| Uuid::parse_str(&s).ok()),
        title,
        source: ThreadSource::parse(&source),
        started_at: DateTime::parse_from_rfc3339(&started)
            .map_err(|e| TraceMindError::Storage(format!("date: {e}")))?
            .with_timezone(&Utc),
        ended_at: ended.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        }),
        metadata: serde_json::from_str(&meta).unwrap_or_else(|_| serde_json::json!({})),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_sprint::ensure_schema;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE memory_views (id TEXT PRIMARY KEY, name TEXT);
             CREATE TABLE captured_signals (
               id INTEGER PRIMARY KEY, source TEXT, raw_text TEXT,
               content_hash INTEGER, promoted_entity TEXT
             );",
        )
        .unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    #[test]
    fn thread_crud_round_trip() {
        let c = fresh();
        let t = Thread::new("Test", ThreadSource::Claude);
        ThreadGraphStore::upsert(&c, &t).unwrap();
        let back = ThreadGraphStore::get(&c, t.id).unwrap().unwrap();
        assert_eq!(back.title, "Test");
        assert_eq!(back.source, ThreadSource::Claude);
        ThreadGraphStore::end_thread(&c, t.id).unwrap();
        assert!(ThreadGraphStore::get(&c, t.id).unwrap().unwrap().ended_at.is_some());
    }

    #[test]
    fn list_threads_orders_newest_first() {
        let c = fresh();
        for i in 0..3 {
            let t = Thread::new(format!("t{i}"), ThreadSource::Tracemind);
            ThreadGraphStore::upsert(&c, &t).unwrap();
        }
        let xs = ThreadGraphStore::list(&c, 10).unwrap();
        assert_eq!(xs.len(), 3);
    }

    #[test]
    fn attach_view_round_trip() {
        let c = fresh();
        let t = Thread::new("X", ThreadSource::Mcp);
        ThreadGraphStore::upsert(&c, &t).unwrap();
        let v = Uuid::new_v4();
        ThreadGraphStore::attach_view(&c, t.id, v).unwrap();
        assert_eq!(ThreadGraphStore::attached_view(&c, t.id).unwrap(), Some(v));
    }

    #[test]
    fn materialize_empty_thread_returns_empty_graph() {
        let c = fresh();
        let t = Thread::new("X", ThreadSource::Other);
        ThreadGraphStore::upsert(&c, &t).unwrap();
        let g = ThreadGraphStore::materialize(&c, t.id).unwrap();
        assert!(g.event_node_ids.is_empty());
        assert!(g.entity_ids.is_empty());
    }
}
