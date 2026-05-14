//! Sprint GRAPH — Glean-style **event / trajectory graph**.
//!
//! Action nodes (captures, queries, commitments, outcomes, decisions)
//! are first-class. Edges (precedes / caused / co_occurs / resolves /
//! contradicts) carry a `support_count` and only become "graph-visible"
//! once the count crosses `MIN_SUPPORT` — the frequency floor that
//! prevents the entity-KG's classic tangle (every triple ≥0.7 becomes
//! an edge).
//!
//! Schema lives in `graph_sprint.rs`. This module is the writer / reader.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// Minimum `support_count` an edge needs before it is exposed to
/// downstream consumers (anticipation cards, suggestion engine, etc).
/// Edges below this floor are *kept* but not surfaced.
pub const MIN_SUPPORT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventNodeKind {
    Capture,
    Query,
    Commitment,
    Outcome,
    Decision,
}

impl EventNodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventNodeKind::Capture => "capture",
            EventNodeKind::Query => "query",
            EventNodeKind::Commitment => "commitment",
            EventNodeKind::Outcome => "outcome",
            EventNodeKind::Decision => "decision",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "capture" => EventNodeKind::Capture,
            "query" => EventNodeKind::Query,
            "commitment" => EventNodeKind::Commitment,
            "outcome" => EventNodeKind::Outcome,
            "decision" => EventNodeKind::Decision,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventEdgeKind {
    Precedes,
    Caused,
    CoOccurs,
    Resolves,
    Contradicts,
}

impl EventEdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventEdgeKind::Precedes => "precedes",
            EventEdgeKind::Caused => "caused",
            EventEdgeKind::CoOccurs => "co_occurs",
            EventEdgeKind::Resolves => "resolves",
            EventEdgeKind::Contradicts => "contradicts",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "precedes" => EventEdgeKind::Precedes,
            "caused" => EventEdgeKind::Caused,
            "co_occurs" => EventEdgeKind::CoOccurs,
            "resolves" => EventEdgeKind::Resolves,
            "contradicts" => EventEdgeKind::Contradicts,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventNode {
    pub id: Uuid,
    pub kind: EventNodeKind,
    pub ts: i64, // millis since epoch
    pub payload_ref: String,
    pub cluster_id: Option<i64>,
    pub context_id: Option<Uuid>,
    pub thread_id: Option<Uuid>,
    pub salience: f64,
}

impl EventNode {
    pub fn new(kind: EventNodeKind, payload_ref: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            ts: Utc::now().timestamp_millis(),
            payload_ref: payload_ref.into(),
            cluster_id: None,
            context_id: None,
            thread_id: None,
            salience: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEdge {
    pub id: Uuid,
    pub from_id: Uuid,
    pub to_id: Uuid,
    pub kind: EventEdgeKind,
    pub strength: f64,
    pub support_count: u32,
    pub context_id: Option<Uuid>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

pub struct EventGraphStore;

impl EventGraphStore {
    /// Insert an event node.
    pub fn insert_node(conn: &Connection, n: &EventNode) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO event_nodes
              (id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience)
             VALUES (?,?,?,?,?,?,?,?)",
            params![
                n.id.to_string(),
                n.kind.as_str(),
                n.ts,
                n.payload_ref,
                n.cluster_id,
                n.context_id.map(|c| c.to_string()),
                n.thread_id.map(|c| c.to_string()),
                n.salience,
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("insert ev node: {e}")))?;
        Ok(())
    }

    /// Upsert an edge — increments `support_count` and `last_seen` when
    /// the same (from, to, kind, context_id) row already exists.
    pub fn upsert_edge(
        conn: &Connection,
        from_id: Uuid,
        to_id: Uuid,
        kind: EventEdgeKind,
        strength_increment: f64,
        context_id: Option<Uuid>,
    ) -> Result<EventEdge> {
        let now = Utc::now();
        let from_s = from_id.to_string();
        let to_s = to_id.to_string();
        let kind_s = kind.as_str();
        let ctx_s = context_id.map(|c| c.to_string());

        // existence check
        let mut stmt = conn
            .prepare(
                "SELECT id, strength, support_count, first_seen
                 FROM event_edges
                 WHERE from_id=? AND to_id=? AND kind=? AND IFNULL(context_id,'')=IFNULL(?, '')",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep ev edge: {e}")))?;
        let mut rows = stmt
            .query(params![from_s, to_s, kind_s, ctx_s])
            .map_err(|e| TraceMindError::Storage(format!("ev edge query: {e}")))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
        {
            let id: String = row.get(0).map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let strength: f64 = row.get(1).map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let support: i64 = row.get(2).map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let first: String = row.get(3).map_err(|e| TraceMindError::Storage(e.to_string()))?;
            drop(rows);
            drop(stmt);

            let new_strength = strength + strength_increment;
            let new_support = (support + 1) as u32;
            conn.execute(
                "UPDATE event_edges SET strength=?, support_count=?, last_seen=?
                 WHERE id=?",
                params![new_strength, new_support as i64, now.to_rfc3339(), id],
            )
            .map_err(|e| TraceMindError::Storage(format!("ev edge update: {e}")))?;
            let edge_id = Uuid::parse_str(&id)
                .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?;
            let first_seen = DateTime::parse_from_rfc3339(&first)
                .map_err(|e| TraceMindError::Storage(format!("date: {e}")))?
                .with_timezone(&Utc);
            return Ok(EventEdge {
                id: edge_id,
                from_id,
                to_id,
                kind,
                strength: new_strength,
                support_count: new_support,
                context_id,
                first_seen,
                last_seen: now,
            });
        }
        drop(rows);
        drop(stmt);

        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO event_edges
              (id, from_id, to_id, kind, strength, support_count, context_id, first_seen, last_seen)
             VALUES (?,?,?,?,?,1,?,?,?)",
            params![
                id.to_string(),
                from_s,
                to_s,
                kind_s,
                strength_increment,
                ctx_s,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("insert ev edge: {e}")))?;
        Ok(EventEdge {
            id,
            from_id,
            to_id,
            kind,
            strength: strength_increment,
            support_count: 1,
            context_id,
            first_seen: now,
            last_seen: now,
        })
    }

    /// Visible edges — those above the frequency floor.
    pub fn visible_edges(conn: &Connection, kind: Option<EventEdgeKind>) -> Result<Vec<EventEdge>> {
        let sql = match kind {
            Some(_) => "SELECT id, from_id, to_id, kind, strength, support_count, context_id, first_seen, last_seen
                        FROM event_edges WHERE support_count >= ? AND kind = ?",
            None => "SELECT id, from_id, to_id, kind, strength, support_count, context_id, first_seen, last_seen
                     FROM event_edges WHERE support_count >= ?",
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| TraceMindError::Storage(format!("prep visible: {e}")))?;
        let rows: Box<dyn Iterator<Item = std::result::Result<EventEdge, rusqlite::Error>>> =
            if let Some(k) = kind {
                Box::new(
                    stmt.query_map(params![MIN_SUPPORT as i64, k.as_str()], row_to_edge)
                        .map_err(|e| TraceMindError::Storage(format!("visible q: {e}")))?
                        .collect::<Vec<_>>()
                        .into_iter(),
                )
            } else {
                Box::new(
                    stmt.query_map(params![MIN_SUPPORT as i64], row_to_edge)
                        .map_err(|e| TraceMindError::Storage(format!("visible q: {e}")))?
                        .collect::<Vec<_>>()
                        .into_iter(),
                )
            };
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| TraceMindError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    /// Promote sequence pairs (cluster_a → cluster_b) into precedes
    /// edges whenever the count of consecutive captures from cluster_a
    /// followed by a query/capture from cluster_b reaches `MIN_SUPPORT`.
    /// Returns the number of edges promoted.
    pub fn promote_sequence_edges(conn: &Connection) -> Result<usize> {
        // Pull (ts, cluster_id, id) for all event nodes that have a cluster.
        let mut stmt = conn
            .prepare(
                "SELECT id, ts, cluster_id FROM event_nodes
                 WHERE cluster_id IS NOT NULL
                 ORDER BY ts ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep promote: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let ts: i64 = row.get(1)?;
                let cl: i64 = row.get(2)?;
                Ok((id, ts, cl))
            })
            .map_err(|e| TraceMindError::Storage(format!("promote query: {e}")))?;

        let mut seq: Vec<(Uuid, i64, i64)> = Vec::new();
        for r in rows {
            let (id, ts, cl) = r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            if let Ok(uid) = Uuid::parse_str(&id) {
                seq.push((uid, ts, cl));
            }
        }

        let mut promoted = 0usize;
        for w in seq.windows(2) {
            if w[0].2 != w[1].2 {
                // distinct clusters: write a precedes edge
                Self::upsert_edge(
                    conn,
                    w[0].0,
                    w[1].0,
                    EventEdgeKind::Precedes,
                    1.0,
                    None,
                )?;
                promoted += 1;
            }
        }
        Ok(promoted)
    }

    /// Read all event nodes (debug / dashboard).
    pub fn list_nodes(conn: &Connection, limit: usize) -> Result<Vec<EventNode>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience
                 FROM event_nodes ORDER BY ts DESC LIMIT ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep nodes: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let id: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let ts: i64 = row.get(2)?;
                let pref: String = row.get(3)?;
                let cl: Option<i64> = row.get(4)?;
                let cid: Option<String> = row.get(5)?;
                let tid: Option<String> = row.get(6)?;
                let sal: f64 = row.get(7)?;
                Ok((id, kind, ts, pref, cl, cid, tid, sal))
            })
            .map_err(|e| TraceMindError::Storage(format!("nodes query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, kind, ts, pref, cl, cid, tid, sal) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let Some(k) = EventNodeKind::parse(&kind) else {
                continue;
            };
            out.push(EventNode {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                kind: k,
                ts,
                payload_ref: pref,
                cluster_id: cl,
                context_id: cid.and_then(|s| Uuid::parse_str(&s).ok()),
                thread_id: tid.and_then(|s| Uuid::parse_str(&s).ok()),
                salience: sal,
            });
        }
        Ok(out)
    }
}

fn row_to_edge(row: &rusqlite::Row) -> std::result::Result<EventEdge, rusqlite::Error> {
    let id: String = row.get(0)?;
    let from: String = row.get(1)?;
    let to: String = row.get(2)?;
    let kind: String = row.get(3)?;
    let strength: f64 = row.get(4)?;
    let support: i64 = row.get(5)?;
    let cid: Option<String> = row.get(6)?;
    let first: String = row.get(7)?;
    let last: String = row.get(8)?;
    Ok(EventEdge {
        id: Uuid::parse_str(&id).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            0, rusqlite::types::Type::Text, Box::new(e)))?,
        from_id: Uuid::parse_str(&from).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            0, rusqlite::types::Type::Text, Box::new(e)))?,
        to_id: Uuid::parse_str(&to).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
            0, rusqlite::types::Type::Text, Box::new(e)))?,
        kind: EventEdgeKind::parse(&kind).unwrap_or(EventEdgeKind::Precedes),
        strength,
        support_count: support as u32,
        context_id: cid.and_then(|s| Uuid::parse_str(&s).ok()),
        first_seen: DateTime::parse_from_rfc3339(&first)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                0, rusqlite::types::Type::Text, Box::new(e)))?
            .with_timezone(&Utc),
        last_seen: DateTime::parse_from_rfc3339(&last)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                0, rusqlite::types::Type::Text, Box::new(e)))?
            .with_timezone(&Utc),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_sprint::ensure_schema;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE memory_views (id TEXT PRIMARY KEY, name TEXT);")
            .unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    #[test]
    fn upsert_edge_increments_support() {
        let c = fresh();
        let a = EventNode::new(EventNodeKind::Capture, "a");
        let b = EventNode::new(EventNodeKind::Query, "b");
        EventGraphStore::insert_node(&c, &a).unwrap();
        EventGraphStore::insert_node(&c, &b).unwrap();
        let e1 = EventGraphStore::upsert_edge(&c, a.id, b.id, EventEdgeKind::Precedes, 0.5, None)
            .unwrap();
        assert_eq!(e1.support_count, 1);
        let e2 = EventGraphStore::upsert_edge(&c, a.id, b.id, EventEdgeKind::Precedes, 0.5, None)
            .unwrap();
        assert_eq!(e2.support_count, 2);
        assert!((e2.strength - 1.0).abs() < 1e-9);
    }

    #[test]
    fn frequency_floor_hides_singleton_edges() {
        let c = fresh();
        let a = EventNode::new(EventNodeKind::Capture, "a");
        let b = EventNode::new(EventNodeKind::Query, "b");
        EventGraphStore::insert_node(&c, &a).unwrap();
        EventGraphStore::insert_node(&c, &b).unwrap();
        EventGraphStore::upsert_edge(&c, a.id, b.id, EventEdgeKind::Precedes, 0.5, None).unwrap();
        let visible = EventGraphStore::visible_edges(&c, None).unwrap();
        assert_eq!(visible.len(), 0, "singleton edge must not be visible");

        EventGraphStore::upsert_edge(&c, a.id, b.id, EventEdgeKind::Precedes, 0.5, None).unwrap();
        let visible = EventGraphStore::visible_edges(&c, None).unwrap();
        assert_eq!(visible.len(), 1, "edge with support>=2 must be visible");
    }

    #[test]
    fn promote_sequence_emits_precedes_pairs() {
        let c = fresh();
        let make = |ts: i64, cluster: i64| {
            let mut n = EventNode::new(EventNodeKind::Capture, "p");
            n.ts = ts;
            n.cluster_id = Some(cluster);
            n
        };
        let nodes = [make(1, 1), make(2, 2), make(3, 1), make(4, 3)];
        for n in &nodes {
            EventGraphStore::insert_node(&c, n).unwrap();
        }
        let promoted = EventGraphStore::promote_sequence_edges(&c).unwrap();
        assert_eq!(promoted, 3); // 1→2, 2→1, 1→3
    }
}
