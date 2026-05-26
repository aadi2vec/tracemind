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

/// CTX-EVG-C — fate of a commitment. Empty string is the schema sentinel
/// meaning "not a commitment"; only commitment nodes use the populated
/// variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentState {
    None,
    Pending,
    Kept,
    Broken,
    Abandoned,
}

impl CommitmentState {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitmentState::None => "",
            CommitmentState::Pending => "pending",
            CommitmentState::Kept => "kept",
            CommitmentState::Broken => "broken",
            CommitmentState::Abandoned => "abandoned",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "pending" => CommitmentState::Pending,
            "kept" => CommitmentState::Kept,
            "broken" => CommitmentState::Broken,
            "abandoned" => CommitmentState::Abandoned,
            _ => CommitmentState::None,
        }
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
    /// Commitment due timestamp (millis since epoch). None for non-commitments
    /// or open-ended commitments.
    #[serde(default)]
    pub due_at: Option<i64>,
    /// Commitment fate. `None` for non-commitments.
    #[serde(default = "CommitmentState::default_none")]
    pub state: CommitmentState,
}

impl CommitmentState {
    fn default_none() -> Self {
        CommitmentState::None
    }
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
            due_at: None,
            state: CommitmentState::None,
        }
    }

    /// CTX-EVG-C — convenience constructor for `Commitment` nodes. Starts
    /// in `Pending` state; `due_at` is millis since epoch (None for
    /// open-ended commitments).
    pub fn commitment(payload_ref: impl Into<String>, due_at: Option<i64>) -> Self {
        let mut n = Self::new(EventNodeKind::Commitment, payload_ref);
        n.due_at = due_at;
        n.state = CommitmentState::Pending;
        n
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
              (id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience, due_at, state)
             VALUES (?,?,?,?,?,?,?,?,?,?)",
            params![
                n.id.to_string(),
                n.kind.as_str(),
                n.ts,
                n.payload_ref,
                n.cluster_id,
                n.context_id.map(|c| c.to_string()),
                n.thread_id.map(|c| c.to_string()),
                n.salience,
                n.due_at,
                n.state.as_str(),
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

    /// CTX-EVG Slice B — promote `Precedes` edges *within a single
    /// thread*. Walks the thread's event nodes in `ts` order and adds
    /// a `Precedes` edge for every consecutive pair (a → b). Each pair
    /// goes through `upsert_edge`, so calling this twice on the same
    /// thread strengthens existing edges rather than duplicating them.
    /// Returns the count of edges upserted.
    ///
    /// This is the *thread-scoped* counterpart to
    /// `promote_sequence_edges`, which is global + cluster-keyed. We
    /// need both: clusters give us topical structure, threads give us
    /// session structure, and the EVG view wants to render both.
    pub fn promote_thread_sequence_edges(
        conn: &Connection,
        thread_id: Uuid,
    ) -> Result<usize> {
        let mut stmt = conn
            .prepare(
                "SELECT id, ts FROM event_nodes
                 WHERE thread_id = ?
                 ORDER BY ts ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep thread promote: {e}")))?;
        let rows = stmt
            .query_map(params![thread_id.to_string()], |row| {
                let id: String = row.get(0)?;
                let ts: i64 = row.get(1)?;
                Ok((id, ts))
            })
            .map_err(|e| TraceMindError::Storage(format!("thread promote query: {e}")))?;

        let mut seq: Vec<(Uuid, i64)> = Vec::new();
        for r in rows {
            let (id, ts) = r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            if let Ok(uid) = Uuid::parse_str(&id) {
                seq.push((uid, ts));
            }
        }
        drop(stmt);

        let mut promoted = 0usize;
        for w in seq.windows(2) {
            if w[0].0 == w[1].0 {
                continue; // never self-loop
            }
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
        Ok(promoted)
    }

    /// Read all event nodes (debug / dashboard).
    pub fn list_nodes(conn: &Connection, limit: usize) -> Result<Vec<EventNode>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience, due_at, state
                 FROM event_nodes ORDER BY ts DESC LIMIT ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep nodes: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_event_node)
            .map_err(|e| TraceMindError::Storage(format!("nodes query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            match r {
                Ok(Some(n)) => out.push(n),
                Ok(None) => continue,
                Err(e) => return Err(TraceMindError::Storage(e.to_string())),
            }
        }
        Ok(out)
    }

    /// Fetch a single event node by id.
    pub fn get_node(conn: &Connection, id: Uuid) -> Result<Option<EventNode>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience, due_at, state
                 FROM event_nodes WHERE id = ?",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep get_node: {e}")))?;
        let mut rows = stmt
            .query_map(params![id.to_string()], row_to_event_node)
            .map_err(|e| TraceMindError::Storage(format!("get_node query: {e}")))?;
        match rows.next() {
            Some(Ok(Some(n))) => Ok(Some(n)),
            Some(Ok(None)) => Ok(None),
            Some(Err(e)) => Err(TraceMindError::Storage(e.to_string())),
            None => Ok(None),
        }
    }

    /// CTX-EVG-C — set a commitment node's `state`. No-op if the node is
    /// not a commitment.
    pub fn set_commitment_state(
        conn: &Connection,
        commitment_id: Uuid,
        state: CommitmentState,
    ) -> Result<()> {
        conn.execute(
            "UPDATE event_nodes SET state = ?
             WHERE id = ? AND kind = 'commitment'",
            params![state.as_str(), commitment_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("set_commitment_state: {e}")))?;
        Ok(())
    }

    /// CTX-EVG-C — record an outcome's resolution of a commitment. Upserts
    /// a `Resolves` edge (outcome → commitment) with strength = polarity
    /// (+1.0 kept / -1.0 broken) and atomically flips the commitment's
    /// state. Returns the resulting commitment state.
    pub fn resolve_commitment(
        conn: &Connection,
        outcome_id: Uuid,
        commitment_id: Uuid,
        polarity: f64,
    ) -> Result<CommitmentState> {
        Self::upsert_edge(
            conn,
            outcome_id,
            commitment_id,
            EventEdgeKind::Resolves,
            polarity,
            None,
        )?;
        let new_state = if polarity >= 0.0 {
            CommitmentState::Kept
        } else {
            CommitmentState::Broken
        };
        Self::set_commitment_state(conn, commitment_id, new_state)?;
        Ok(new_state)
    }

    /// CTX-EVG-C — flip pending commitments whose `due_at` has lapsed and
    /// which have no `Resolves` edge into `broken` state. Idempotent.
    /// Returns the count of rows flipped on this call.
    pub fn sweep_broken(conn: &Connection, now_ms: i64) -> Result<usize> {
        let updated = conn
            .execute(
                "UPDATE event_nodes
                 SET state = 'broken'
                 WHERE kind = 'commitment'
                   AND state = 'pending'
                   AND due_at IS NOT NULL
                   AND due_at < ?
                   AND id NOT IN (
                     SELECT to_id FROM event_edges WHERE kind = 'resolves'
                   )",
                params![now_ms],
            )
            .map_err(|e| TraceMindError::Storage(format!("sweep_broken: {e}")))?;
        Ok(updated)
    }

    /// CTX-EVG-C — commitment ledger summary for a time window.
    /// Counts commitments by state where the commitment's `ts` falls in
    /// `[since_ms, until_ms)`. Also returns the visible node IDs so the
    /// UI can render the subgraph beneath the score card.
    pub fn commitment_ledger(
        conn: &Connection,
        since_ms: i64,
        until_ms: i64,
    ) -> Result<LedgerSummary> {
        let mut stmt = conn
            .prepare(
                "SELECT id, state FROM event_nodes
                 WHERE kind = 'commitment' AND ts >= ? AND ts < ?
                 ORDER BY ts DESC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep ledger: {e}")))?;
        let rows = stmt
            .query_map(params![since_ms, until_ms], |row| {
                let id: String = row.get(0)?;
                let st: String = row.get(1)?;
                Ok((id, st))
            })
            .map_err(|e| TraceMindError::Storage(format!("ledger query: {e}")))?;
        let mut summary = LedgerSummary::default();
        for r in rows {
            let (id, st) = r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let uid = match Uuid::parse_str(&id) {
                Ok(u) => u,
                Err(_) => continue,
            };
            summary.commitment_ids.push(uid);
            match CommitmentState::parse(&st) {
                CommitmentState::Kept => summary.kept += 1,
                CommitmentState::Broken => summary.broken += 1,
                CommitmentState::Pending => summary.pending += 1,
                CommitmentState::Abandoned => summary.abandoned += 1,
                CommitmentState::None => {}
            }
        }
        Ok(summary)
    }

    /// CTX-EVG-C — list pending commitments in a thread (or all threads
    /// when `thread_id` is None), ordered by `due_at` ascending (NULLs
    /// last). Used by the resolution prompt to pick candidates.
    pub fn pending_commitments(
        conn: &Connection,
        thread_id: Option<Uuid>,
        limit: usize,
    ) -> Result<Vec<EventNode>> {
        let (sql, has_thread) = if thread_id.is_some() {
            (
                "SELECT id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience, due_at, state
                 FROM event_nodes
                 WHERE kind='commitment' AND state='pending' AND thread_id = ?
                 ORDER BY (due_at IS NULL), due_at ASC, ts DESC
                 LIMIT ?",
                true,
            )
        } else {
            (
                "SELECT id, kind, ts, payload_ref, cluster_id, context_id, thread_id, salience, due_at, state
                 FROM event_nodes
                 WHERE kind='commitment' AND state='pending'
                 ORDER BY (due_at IS NULL), due_at ASC, ts DESC
                 LIMIT ?",
                false,
            )
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| TraceMindError::Storage(format!("prep pending: {e}")))?;
        let rows = if has_thread {
            stmt.query_map(
                params![thread_id.unwrap().to_string(), limit as i64],
                row_to_event_node,
            )
        } else {
            stmt.query_map(params![limit as i64], row_to_event_node)
        }
        .map_err(|e| TraceMindError::Storage(format!("pending query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            match r {
                Ok(Some(n)) => out.push(n),
                Ok(None) => continue,
                Err(e) => return Err(TraceMindError::Storage(e.to_string())),
            }
        }
        Ok(out)
    }
}

/// CTX-EVG-C — counts for the commitment ledger window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub kept: u32,
    pub broken: u32,
    pub pending: u32,
    pub abandoned: u32,
    pub commitment_ids: Vec<Uuid>,
}

fn row_to_event_node(
    row: &rusqlite::Row,
) -> std::result::Result<Option<EventNode>, rusqlite::Error> {
    let id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let ts: i64 = row.get(2)?;
    let pref: String = row.get(3)?;
    let cl: Option<i64> = row.get(4)?;
    let cid: Option<String> = row.get(5)?;
    let tid: Option<String> = row.get(6)?;
    let sal: f64 = row.get(7)?;
    let due_at: Option<i64> = row.get(8)?;
    let state: String = row.get(9)?;
    let Some(k) = EventNodeKind::parse(&kind) else {
        return Ok(None);
    };
    let Ok(uid) = Uuid::parse_str(&id) else {
        return Ok(None);
    };
    Ok(Some(EventNode {
        id: uid,
        kind: k,
        ts,
        payload_ref: pref,
        cluster_id: cl,
        context_id: cid.and_then(|s| Uuid::parse_str(&s).ok()),
        thread_id: tid.and_then(|s| Uuid::parse_str(&s).ok()),
        salience: sal,
        due_at,
        state: CommitmentState::parse(&state),
    }))
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

    #[test]
    fn commitment_lifecycle_pending_to_kept_via_resolve() {
        let c = fresh();
        let commit = EventNode::commitment("c:1", Some(2_000));
        let cid = commit.id;
        EventGraphStore::insert_node(&c, &commit).unwrap();

        let fetched = EventGraphStore::get_node(&c, cid).unwrap().unwrap();
        assert_eq!(fetched.state, CommitmentState::Pending);
        assert_eq!(fetched.due_at, Some(2_000));

        let mut outcome = EventNode::new(EventNodeKind::Outcome, "o:1");
        outcome.ts = 3_000;
        EventGraphStore::insert_node(&c, &outcome).unwrap();

        let new_state = EventGraphStore::resolve_commitment(&c, outcome.id, cid, 1.0)
            .unwrap();
        assert_eq!(new_state, CommitmentState::Kept);

        // edge exists with strength +1.0
        let edges = EventGraphStore::visible_edges(&c, Some(EventEdgeKind::Resolves))
            .unwrap();
        // singleton edge — below MIN_SUPPORT — must not be visible yet
        assert_eq!(edges.len(), 0);

        // second resolution (e.g. user revises) strengthens but stays Kept
        let new_state2 = EventGraphStore::resolve_commitment(&c, outcome.id, cid, 1.0)
            .unwrap();
        assert_eq!(new_state2, CommitmentState::Kept);
        let edges = EventGraphStore::visible_edges(&c, Some(EventEdgeKind::Resolves))
            .unwrap();
        assert_eq!(edges.len(), 1);

        let fetched = EventGraphStore::get_node(&c, cid).unwrap().unwrap();
        assert_eq!(fetched.state, CommitmentState::Kept);
    }

    #[test]
    fn resolve_negative_polarity_marks_broken() {
        let c = fresh();
        let commit = EventNode::commitment("c:1", Some(2_000));
        let cid = commit.id;
        EventGraphStore::insert_node(&c, &commit).unwrap();
        let mut outcome = EventNode::new(EventNodeKind::Outcome, "o:1");
        outcome.ts = 3_000;
        EventGraphStore::insert_node(&c, &outcome).unwrap();

        let s = EventGraphStore::resolve_commitment(&c, outcome.id, cid, -1.0).unwrap();
        assert_eq!(s, CommitmentState::Broken);
        let fetched = EventGraphStore::get_node(&c, cid).unwrap().unwrap();
        assert_eq!(fetched.state, CommitmentState::Broken);
    }

    #[test]
    fn sweep_broken_flips_overdue_unresolved_only() {
        let c = fresh();
        // overdue, unresolved → should flip
        let a = EventNode::commitment("a", Some(1_000));
        // overdue but resolved → must NOT flip
        let b = EventNode::commitment("b", Some(1_000));
        // future due → must NOT flip
        let c1 = EventNode::commitment("c", Some(10_000));
        // no due_at → must NOT flip
        let d = EventNode::commitment("d", None);
        EventGraphStore::insert_node(&c, &a).unwrap();
        EventGraphStore::insert_node(&c, &b).unwrap();
        EventGraphStore::insert_node(&c, &c1).unwrap();
        EventGraphStore::insert_node(&c, &d).unwrap();

        let outcome = EventNode::new(EventNodeKind::Outcome, "o");
        EventGraphStore::insert_node(&c, &outcome).unwrap();
        EventGraphStore::resolve_commitment(&c, outcome.id, b.id, 1.0).unwrap();

        let flipped = EventGraphStore::sweep_broken(&c, 5_000).unwrap();
        assert_eq!(flipped, 1, "only the unresolved overdue commitment should flip");

        assert_eq!(
            EventGraphStore::get_node(&c, a.id).unwrap().unwrap().state,
            CommitmentState::Broken
        );
        assert_eq!(
            EventGraphStore::get_node(&c, b.id).unwrap().unwrap().state,
            CommitmentState::Kept
        );
        assert_eq!(
            EventGraphStore::get_node(&c, c1.id).unwrap().unwrap().state,
            CommitmentState::Pending
        );
        assert_eq!(
            EventGraphStore::get_node(&c, d.id).unwrap().unwrap().state,
            CommitmentState::Pending
        );

        // idempotency: second sweep flips nothing new
        let flipped2 = EventGraphStore::sweep_broken(&c, 5_000).unwrap();
        assert_eq!(flipped2, 0);
    }

    #[test]
    fn commitment_ledger_counts_by_state_in_window() {
        let c = fresh();
        let mut kept = EventNode::commitment("k", Some(100));
        kept.ts = 50;
        kept.state = CommitmentState::Kept;
        let mut broken = EventNode::commitment("b", Some(100));
        broken.ts = 60;
        broken.state = CommitmentState::Broken;
        let mut pending = EventNode::commitment("p", Some(9_999));
        pending.ts = 70;
        let mut out_of_window = EventNode::commitment("ow", Some(100));
        out_of_window.ts = 9_999;
        for n in [&kept, &broken, &pending, &out_of_window] {
            EventGraphStore::insert_node(&c, n).unwrap();
        }
        let s = EventGraphStore::commitment_ledger(&c, 0, 1_000).unwrap();
        assert_eq!(s.kept, 1);
        assert_eq!(s.broken, 1);
        assert_eq!(s.pending, 1);
        assert_eq!(s.commitment_ids.len(), 3);
    }

    #[test]
    fn pending_commitments_orders_by_due() {
        let c = fresh();
        let mut a = EventNode::commitment("a", Some(3_000));
        a.ts = 10;
        let mut b = EventNode::commitment("b", Some(1_000));
        b.ts = 20;
        let mut c2 = EventNode::commitment("c", None);
        c2.ts = 30;
        EventGraphStore::insert_node(&c, &a).unwrap();
        EventGraphStore::insert_node(&c, &b).unwrap();
        EventGraphStore::insert_node(&c, &c2).unwrap();

        let p = EventGraphStore::pending_commitments(&c, None, 10).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].id, b.id); // due=1000 first
        assert_eq!(p[1].id, a.id); // due=3000
        assert_eq!(p[2].id, c2.id); // no due_at last
    }

    #[test]
    fn promote_thread_sequence_walks_thread_only() {
        let c = fresh();
        let thread_a = Uuid::new_v4();
        let thread_b = Uuid::new_v4();
        let make = |ts: i64, thread: Option<Uuid>| {
            let mut n = EventNode::new(EventNodeKind::Capture, "p");
            n.ts = ts;
            n.thread_id = thread;
            n
        };
        // Thread A has 3 nodes; Thread B has 2; one node is unthreaded.
        let nodes = [
            make(1, Some(thread_a)),
            make(2, Some(thread_b)),
            make(3, Some(thread_a)),
            make(4, None),
            make(5, Some(thread_a)),
            make(6, Some(thread_b)),
        ];
        for n in &nodes {
            EventGraphStore::insert_node(&c, n).unwrap();
        }
        let promoted = EventGraphStore::promote_thread_sequence_edges(&c, thread_a).unwrap();
        // Thread A has 3 nodes → 2 consecutive pairs → 2 edges.
        // Unthreaded + thread-B nodes must not contribute.
        assert_eq!(promoted, 2);

        // Calling again strengthens, doesn't duplicate.
        let promoted2 = EventGraphStore::promote_thread_sequence_edges(&c, thread_a).unwrap();
        assert_eq!(promoted2, 2);
        let visible = EventGraphStore::visible_edges(&c, None).unwrap();
        assert_eq!(
            visible.len(),
            2,
            "second pass should strengthen, not duplicate"
        );
    }
}
