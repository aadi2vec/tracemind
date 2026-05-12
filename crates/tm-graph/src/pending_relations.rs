//! LM-9 — confidence-routed triple pending pool.
//!
//! Triples extracted by the ingest pipeline come with a confidence
//! score. High-confidence relations (`>= ACCEPT_THRESHOLD`) flow
//! straight into `kg_relations` — they get the same trust as
//! hand-curated edges. Low-confidence relations (`< ACCEPT_THRESHOLD`)
//! land here instead: they're persisted but kept *out* of the live
//! graph until a human (or a future SLM) accepts them.
//!
//! The Tauri "pending" panel reads from this table, surfacing the
//! "did we get this right?" decisions the user actually wants to
//! touch. The deny-list takes care of *negative* user feedback;
//! pending_relations takes care of *positive but uncertain* extraction.
//!
//! ## Schema
//!
//! ```text
//! pending_relations (
//!     id           TEXT PRIMARY KEY,
//!     subject_id   TEXT NOT NULL,
//!     predicate    TEXT NOT NULL,      -- open-vocab string (LM-7)
//!     object_id    TEXT NOT NULL,
//!     confidence   REAL NOT NULL,
//!     source_id    TEXT,
//!     status       TEXT NOT NULL,      -- 'pending' | 'accepted' | 'rejected'
//!     created_at   TEXT NOT NULL,
//!     decided_at   TEXT,
//!     note         TEXT NOT NULL DEFAULT ''
//! )
//! ```
//!
//! `status='accepted'` rows stay in the table for audit (alongside the
//! promoted triple in `kg_relations`). `status='rejected'` rows also
//! stay so the user — or a future SLM — can revisit decisions.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// Default acceptance threshold. Triples with `confidence >=` this
/// value bypass the pending pool and write directly to `kg_relations`.
/// Confidence below this lands in `pending_relations`.
pub const ACCEPT_THRESHOLD: f64 = 0.7;

/// Default floor below which we drop the triple entirely (don't even
/// queue it). Keeps the pending panel from getting flooded with garbage.
pub const PENDING_FLOOR: f64 = 0.3;

/// Schema version stamp on the table (we don't reuse the SKG schema
/// version because this is our own table).
pub const SCHEMA_VERSION: u32 = 1;

/// Lifecycle states for a pending relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingStatus {
    Pending,
    Accepted,
    Rejected,
}

impl PendingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PendingStatus::Pending => "pending",
            PendingStatus::Accepted => "accepted",
            PendingStatus::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(PendingStatus::Pending),
            "accepted" => Ok(PendingStatus::Accepted),
            "rejected" => Ok(PendingStatus::Rejected),
            other => Err(TraceMindError::Storage(format!(
                "invalid pending status '{other}'"
            ))),
        }
    }
}

/// One row of the pending pool. `predicate` is an open-vocabulary
/// string so we don't have to teach this table about every new
/// predicate enum variant.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingRelation {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate: String,
    pub object_id: Uuid,
    pub confidence: f64,
    pub source_id: Option<String>,
    pub status: PendingStatus,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub note: String,
}

impl PendingRelation {
    pub fn new(
        subject_id: Uuid,
        predicate: impl Into<String>,
        object_id: Uuid,
        confidence: f64,
        source_id: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            subject_id,
            predicate: predicate.into(),
            object_id,
            confidence,
            source_id,
            status: PendingStatus::Pending,
            created_at: Utc::now(),
            decided_at: None,
            note: String::new(),
        }
    }
}

/// Initialize the `pending_relations` table. Idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pending_relations (
            id           TEXT PRIMARY KEY,
            subject_id   TEXT NOT NULL,
            predicate    TEXT NOT NULL,
            object_id    TEXT NOT NULL,
            confidence   REAL NOT NULL,
            source_id    TEXT,
            status       TEXT NOT NULL DEFAULT 'pending',
            created_at   TEXT NOT NULL,
            decided_at   TEXT,
            note         TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_pending_relations_status
            ON pending_relations(status, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_pending_relations_subject
            ON pending_relations(subject_id);
        CREATE INDEX IF NOT EXISTS idx_pending_relations_object
            ON pending_relations(object_id);
        ",
    )
    .map_err(|e| TraceMindError::Storage(format!("init pending_relations: {e}")))?;
    Ok(())
}

/// Insert a new pending relation. Caller is responsible for confidence
/// gating — `pending_relations::insert` doesn't enforce a floor.
pub fn insert(conn: &Connection, row: &PendingRelation) -> Result<()> {
    conn.execute(
        "INSERT INTO pending_relations
            (id, subject_id, predicate, object_id, confidence,
             source_id, status, created_at, decided_at, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            row.id.to_string(),
            row.subject_id.to_string(),
            row.predicate,
            row.object_id.to_string(),
            row.confidence,
            row.source_id,
            row.status.as_str(),
            row.created_at.to_rfc3339(),
            row.decided_at.as_ref().map(|t| t.to_rfc3339()),
            row.note,
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("insert pending: {e}")))?;
    Ok(())
}

/// List pending relations. `status_filter = None` returns every status.
/// Rows are ordered by (confidence desc, created_at desc) so the
/// Tauri panel shows the most-likely-correct candidates first.
pub fn list(
    conn: &Connection,
    status_filter: Option<PendingStatus>,
    limit: Option<usize>,
) -> Result<Vec<PendingRelation>> {
    let limit_sql = limit.unwrap_or(usize::MAX);
    let (sql, has_filter) = match status_filter {
        Some(_) => (
            "SELECT id, subject_id, predicate, object_id, confidence,
                    source_id, status, created_at, decided_at, note
             FROM pending_relations
             WHERE status = ?1
             ORDER BY confidence DESC, created_at DESC
             LIMIT ?2",
            true,
        ),
        None => (
            "SELECT id, subject_id, predicate, object_id, confidence,
                    source_id, status, created_at, decided_at, note
             FROM pending_relations
             ORDER BY confidence DESC, created_at DESC
             LIMIT ?1",
            false,
        ),
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| TraceMindError::Storage(format!("prepare pending list: {e}")))?;

    let rows = if has_filter {
        stmt.query_map(
            params![status_filter.unwrap().as_str(), limit_sql as i64],
            row_to_pending,
        )
    } else {
        stmt.query_map(params![limit_sql as i64], row_to_pending)
    }
    .map_err(|e| TraceMindError::Storage(format!("query pending: {e}")))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| TraceMindError::Storage(format!("row: {e}")))?);
    }
    Ok(out)
}

/// Fetch a single pending relation by id.
pub fn get(conn: &Connection, id: Uuid) -> Result<Option<PendingRelation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, subject_id, predicate, object_id, confidence,
                    source_id, status, created_at, decided_at, note
             FROM pending_relations
             WHERE id = ?1
             LIMIT 1",
        )
        .map_err(|e| TraceMindError::Storage(format!("prepare get pending: {e}")))?;
    let mut rows = stmt
        .query_map(params![id.to_string()], row_to_pending)
        .map_err(|e| TraceMindError::Storage(format!("query get pending: {e}")))?;
    match rows.next() {
        Some(r) => Ok(Some(
            r.map_err(|e| TraceMindError::Storage(format!("row: {e}")))?,
        )),
        None => Ok(None),
    }
}

/// Mark a pending row as accepted/rejected. Doesn't promote to
/// `kg_relations` on its own — `GraphStore::accept_pending` does that.
pub fn set_status(
    conn: &Connection,
    id: Uuid,
    status: PendingStatus,
    note: &str,
) -> Result<bool> {
    let now = Utc::now().to_rfc3339();
    let affected = conn
        .execute(
            "UPDATE pending_relations
             SET status = ?1, decided_at = ?2, note = ?3
             WHERE id = ?4",
            params![status.as_str(), now, note, id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("update pending: {e}")))?;
    Ok(affected > 0)
}

/// Delete pending rows in a terminal state older than `days` days.
/// Use to keep the panel from growing without bound.
pub fn purge_decided_older_than(conn: &Connection, days: i64) -> Result<usize> {
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let affected = conn
        .execute(
            "DELETE FROM pending_relations
             WHERE status IN ('accepted', 'rejected')
               AND decided_at IS NOT NULL
               AND decided_at < ?1",
            params![cutoff.to_rfc3339()],
        )
        .map_err(|e| TraceMindError::Storage(format!("purge pending: {e}")))?;
    Ok(affected)
}

fn row_to_pending(row: &rusqlite::Row) -> rusqlite::Result<PendingRelation> {
    let id_s: String = row.get(0)?;
    let subj_s: String = row.get(1)?;
    let pred: String = row.get(2)?;
    let obj_s: String = row.get(3)?;
    let conf: f64 = row.get(4)?;
    let source_id: Option<String> = row.get(5)?;
    let status_s: String = row.get(6)?;
    let created_at_s: String = row.get(7)?;
    let decided_at_s: Option<String> = row.get(8)?;
    let note: String = row.get(9)?;

    let id = Uuid::parse_str(&id_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let subject_id = Uuid::parse_str(&subj_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let object_id = Uuid::parse_str(&obj_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let status = match status_s.as_str() {
        "pending" => PendingStatus::Pending,
        "accepted" => PendingStatus::Accepted,
        "rejected" => PendingStatus::Rejected,
        _ => PendingStatus::Pending,
    };
    let created_at = DateTime::parse_from_rfc3339(&created_at_s)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
        })?
        .with_timezone(&Utc);
    let decided_at = decided_at_s
        .as_ref()
        .map(|s| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        8,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
        })
        .transpose()?;

    Ok(PendingRelation {
        id,
        subject_id,
        predicate: pred,
        object_id,
        confidence: conf,
        source_id,
        status,
        created_at,
        decided_at,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open mem");
        init_schema(&conn).expect("init");
        conn
    }

    #[test]
    fn insert_and_list_pending() {
        let conn = open_db();
        let row = PendingRelation::new(
            Uuid::new_v4(),
            "worksAt",
            Uuid::new_v4(),
            0.45,
            Some("trace-1".into()),
        );
        insert(&conn, &row).expect("insert");

        let all = list(&conn, None, None).expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, row.id);
        assert_eq!(all[0].predicate, "worksAt");
        assert_eq!(all[0].status, PendingStatus::Pending);

        let pending = list(&conn, Some(PendingStatus::Pending), None).expect("pending");
        assert_eq!(pending.len(), 1);

        let accepted = list(&conn, Some(PendingStatus::Accepted), None).expect("none accepted");
        assert!(accepted.is_empty());
    }

    #[test]
    fn list_orders_by_confidence_desc() {
        let conn = open_db();
        let lo = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.35, None);
        let hi = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.65, None);
        let mid = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.5, None);
        insert(&conn, &lo).expect("lo");
        insert(&conn, &mid).expect("mid");
        insert(&conn, &hi).expect("hi");

        let all = list(&conn, None, None).expect("list");
        assert_eq!(all[0].id, hi.id);
        assert_eq!(all[1].id, mid.id);
        assert_eq!(all[2].id, lo.id);

        let top = list(&conn, None, Some(1)).expect("limit");
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].id, hi.id);
    }

    #[test]
    fn set_status_updates_row_and_decided_at() {
        let conn = open_db();
        let row = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.4, None);
        insert(&conn, &row).expect("insert");
        let updated =
            set_status(&conn, row.id, PendingStatus::Accepted, "user-confirmed").expect("update");
        assert!(updated);

        let fetched = get(&conn, row.id).expect("get").expect("some");
        assert_eq!(fetched.status, PendingStatus::Accepted);
        assert_eq!(fetched.note, "user-confirmed");
        assert!(fetched.decided_at.is_some());

        // No-op on a non-existent id.
        let missing = set_status(&conn, Uuid::new_v4(), PendingStatus::Rejected, "").expect("none");
        assert!(!missing);
    }

    #[test]
    fn purge_only_removes_old_terminal_rows() {
        let conn = open_db();
        let pending = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.4, None);
        let accepted = PendingRelation::new(Uuid::new_v4(), "p", Uuid::new_v4(), 0.8, None);
        insert(&conn, &pending).expect("pending");
        insert(&conn, &accepted).expect("accepted");

        // Hand-stamp `accepted` decided_at to 100 days ago.
        let old = (Utc::now() - chrono::Duration::days(100)).to_rfc3339();
        conn.execute(
            "UPDATE pending_relations SET status = 'accepted', decided_at = ?1 WHERE id = ?2",
            params![old, accepted.id.to_string()],
        )
        .expect("hand-stamp");

        let purged = purge_decided_older_than(&conn, 30).expect("purge");
        assert_eq!(purged, 1);

        let remaining = list(&conn, None, None).expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, pending.id);
    }

    #[test]
    fn parse_status_roundtrips() {
        for s in [PendingStatus::Pending, PendingStatus::Accepted, PendingStatus::Rejected] {
            assert_eq!(PendingStatus::parse(s.as_str()).unwrap(), s);
        }
        assert!(PendingStatus::parse("nope").is_err());
    }
}
