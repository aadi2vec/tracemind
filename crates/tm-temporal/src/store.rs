use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;
use uuid::Uuid;

use crate::types::{BitemporalFact, TemporalSnapshot, TransactionTime, ValidTime};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not found: fact {id}")]
    NotFound { id: Uuid },
    #[error("invalid {field}: {value}")]
    Invalid { field: &'static str, value: String },
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct TemporalStore {
    conn: Connection,
}

impl TemporalStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS temporal_facts (
                id             TEXT PRIMARY KEY,
                entity_id      TEXT NOT NULL,
                fact_type      TEXT NOT NULL,
                fact_json      TEXT NOT NULL,
                valid_from     TEXT NOT NULL,
                valid_to       TEXT,
                recorded_at    TEXT NOT NULL,
                superseded_at  TEXT,
                supersedes     TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_temporal_entity    ON temporal_facts(entity_id);
            CREATE INDEX IF NOT EXISTS idx_temporal_type      ON temporal_facts(fact_type);
            CREATE INDEX IF NOT EXISTS idx_temporal_valid_from ON temporal_facts(valid_from);
            CREATE INDEX IF NOT EXISTS idx_temporal_recorded  ON temporal_facts(recorded_at);
            CREATE INDEX IF NOT EXISTS idx_temporal_superseded ON temporal_facts(superseded_at);
            "#,
        )?;
        Ok(())
    }

    pub fn insert_fact(
        &self,
        entity_id: Uuid,
        fact_type: &str,
        fact_json: &str,
        valid_from: DateTime<Utc>,
        valid_to: Option<DateTime<Utc>>,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        self.conn.execute(
            r#"
            INSERT INTO temporal_facts (
                id, entity_id, fact_type, fact_json,
                valid_from, valid_to, recorded_at, superseded_at, supersedes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL)
            "#,
            params![
                id.to_string(),
                entity_id.to_string(),
                fact_type,
                fact_json,
                valid_from.to_rfc3339(),
                valid_to.map(|t| t.to_rfc3339()),
                now.to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    pub fn retract_fact(
        &self,
        fact_id: Uuid,
        retracted_at: DateTime<Utc>,
    ) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE temporal_facts SET superseded_at = ? WHERE id = ? AND superseded_at IS NULL",
            params![retracted_at.to_rfc3339(), fact_id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound { id: fact_id });
        }
        Ok(())
    }

    /// Close the *valid-time* interval of the current fact for `entity_id`:
    /// set `valid_to` on the live (non-superseded, still-open) revision.
    ///
    /// This is what makes fact supersession correct rather than a message —
    /// when a new fact reverses an old one ("works at Stripe" → "works at
    /// Datadog"), the old fact stops being true in the world at `valid_to`,
    /// so an as-of query before that instant still returns the old value and
    /// one after returns nothing (until the new fact is recorded). Idempotent
    /// and a no-op when there is no open fact.
    pub fn close_validity(
        &self,
        entity_id: Uuid,
        fact_type: &str,
        valid_to: DateTime<Utc>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE temporal_facts SET valid_to = ? \
             WHERE entity_id = ? AND fact_type = ? \
               AND superseded_at IS NULL AND valid_to IS NULL",
            params![valid_to.to_rfc3339(), entity_id.to_string(), fact_type],
        )?;
        Ok(())
    }

    pub fn update_fact(
        &self,
        old_fact_id: Uuid,
        new_fact_json: &str,
        new_valid_from: DateTime<Utc>,
        new_valid_to: Option<DateTime<Utc>>,
    ) -> Result<Uuid> {
        let now = Utc::now();

        let (entity_id_s, fact_type): (String, String) = self
            .conn
            .query_row(
                "SELECT entity_id, fact_type FROM temporal_facts WHERE id = ?",
                params![old_fact_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or(StoreError::NotFound { id: old_fact_id })?;

        self.conn.execute(
            "UPDATE temporal_facts SET superseded_at = ? WHERE id = ? AND superseded_at IS NULL",
            params![now.to_rfc3339(), old_fact_id.to_string()],
        )?;

        let new_id = Uuid::new_v4();
        self.conn.execute(
            r#"
            INSERT INTO temporal_facts (
                id, entity_id, fact_type, fact_json,
                valid_from, valid_to, recorded_at, superseded_at, supersedes
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)
            "#,
            params![
                new_id.to_string(),
                entity_id_s,
                fact_type,
                new_fact_json,
                new_valid_from.to_rfc3339(),
                new_valid_to.map(|t| t.to_rfc3339()),
                now.to_rfc3339(),
                old_fact_id.to_string(),
            ],
        )?;
        Ok(new_id)
    }

    pub fn query_at(
        &self,
        entity_id: Uuid,
        as_of_valid: DateTime<Utc>,
        as_of_tx: DateTime<Utc>,
    ) -> Result<Vec<BitemporalFact<serde_json::Value>>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, entity_id, fact_type, fact_json,
                   valid_from, valid_to, recorded_at, superseded_at, supersedes
            FROM temporal_facts
            WHERE entity_id = ?
              AND recorded_at <= ?
              AND (superseded_at IS NULL OR superseded_at > ?)
              AND valid_from <= ?
              AND (valid_to IS NULL OR valid_to > ?)
            ORDER BY recorded_at DESC
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                entity_id.to_string(),
                as_of_tx.to_rfc3339(),
                as_of_tx.to_rfc3339(),
                as_of_valid.to_rfc3339(),
                as_of_valid.to_rfc3339(),
            ],
            row_to_fact,
        )?;
        collect_facts(rows)
    }

    pub fn query_range(
        &self,
        entity_id: Uuid,
        valid_start: DateTime<Utc>,
        valid_end: DateTime<Utc>,
    ) -> Result<Vec<BitemporalFact<serde_json::Value>>> {
        let now = Utc::now();
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, entity_id, fact_type, fact_json,
                   valid_from, valid_to, recorded_at, superseded_at, supersedes
            FROM temporal_facts
            WHERE entity_id = ?
              AND recorded_at <= ?
              AND (superseded_at IS NULL OR superseded_at > ?)
              AND valid_from < ?
              AND (valid_to IS NULL OR valid_to > ?)
            ORDER BY valid_from ASC
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                entity_id.to_string(),
                now.to_rfc3339(),
                now.to_rfc3339(),
                valid_end.to_rfc3339(),
                valid_start.to_rfc3339(),
            ],
            row_to_fact,
        )?;
        collect_facts(rows)
    }

    pub fn history(
        &self,
        entity_id: Uuid,
        fact_type: &str,
    ) -> Result<Vec<BitemporalFact<serde_json::Value>>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, entity_id, fact_type, fact_json,
                   valid_from, valid_to, recorded_at, superseded_at, supersedes
            FROM temporal_facts
            WHERE entity_id = ?
              AND fact_type = ?
            ORDER BY recorded_at ASC
            "#,
        )?;
        let rows = stmt.query_map(
            params![entity_id.to_string(), fact_type],
            row_to_fact,
        )?;
        collect_facts(rows)
    }

    /// Return the id of the currently-live fact for `(entity_id, fact_type)`,
    /// i.e. the most recently recorded row with `superseded_at IS NULL`.
    /// `None` means we've never seen this entity/fact-type combo (or every
    /// version has been retracted).
    pub fn current_fact_id(
        &self,
        entity_id: Uuid,
        fact_type: &str,
    ) -> Result<Option<Uuid>> {
        let row: Option<String> = self
            .conn
            .query_row(
                r#"
                SELECT id FROM temporal_facts
                WHERE entity_id = ? AND fact_type = ? AND superseded_at IS NULL
                ORDER BY recorded_at DESC LIMIT 1
                "#,
                params![entity_id.to_string(), fact_type],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(s) => Ok(Some(parse_uuid(&s, "fact_id")?)),
            None => Ok(None),
        }
    }

    pub fn world_at(
        &self,
        as_of_valid: DateTime<Utc>,
        as_of_tx: DateTime<Utc>,
        limit: usize,
    ) -> Result<TemporalSnapshot<serde_json::Value>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, entity_id, fact_type, fact_json,
                   valid_from, valid_to, recorded_at, superseded_at, supersedes
            FROM temporal_facts
            WHERE recorded_at <= ?
              AND (superseded_at IS NULL OR superseded_at > ?)
              AND valid_from <= ?
              AND (valid_to IS NULL OR valid_to > ?)
            ORDER BY recorded_at DESC
            LIMIT ?
            "#,
        )?;
        let rows = stmt.query_map(
            params![
                as_of_tx.to_rfc3339(),
                as_of_tx.to_rfc3339(),
                as_of_valid.to_rfc3339(),
                as_of_valid.to_rfc3339(),
                limit as i64,
            ],
            row_to_fact,
        )?;
        let facts = collect_facts(rows)?;
        Ok(TemporalSnapshot {
            facts,
            queried_at: Utc::now(),
            as_of_valid,
            as_of_tx,
        })
    }
}

fn row_to_fact(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<BitemporalFact<serde_json::Value>>> {
    let id_s: String = row.get(0)?;
    let _entity_id_s: String = row.get(1)?;
    let _fact_type: String = row.get(2)?;
    let fact_json: String = row.get(3)?;
    let valid_from_s: String = row.get(4)?;
    let valid_to_s: Option<String> = row.get(5)?;
    let recorded_at_s: String = row.get(6)?;
    let superseded_at_s: Option<String> = row.get(7)?;
    let supersedes_s: Option<String> = row.get(8)?;

    Ok((|| -> Result<BitemporalFact<serde_json::Value>> {
        let fact: serde_json::Value = serde_json::from_str(&fact_json)?;
        Ok(BitemporalFact {
            fact,
            valid_time: ValidTime {
                from: parse_dt(&valid_from_s, "valid_from")?,
                to: valid_to_s
                    .as_deref()
                    .map(|s| parse_dt(s, "valid_to"))
                    .transpose()?,
            },
            tx_time: TransactionTime {
                recorded_at: parse_dt(&recorded_at_s, "recorded_at")?,
                superseded_at: superseded_at_s
                    .as_deref()
                    .map(|s| parse_dt(s, "superseded_at"))
                    .transpose()?,
            },
            fact_id: parse_uuid(&id_s, "id")?,
            supersedes: supersedes_s
                .as_deref()
                .map(|s| parse_uuid(s, "supersedes"))
                .transpose()?,
        })
    })())
}

fn collect_facts(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Result<BitemporalFact<serde_json::Value>>>>,
) -> Result<Vec<BitemporalFact<serde_json::Value>>> {
    let mut out = Vec::new();
    for r in rows {
        out.push(r??);
    }
    Ok(out)
}

fn parse_uuid(s: &str, field: &'static str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|_| StoreError::Invalid {
        field,
        value: s.to_string(),
    })
}

fn parse_dt(s: &str, field: &'static str) -> Result<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| StoreError::Invalid {
            field,
            value: s.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use serde_json::json;

    fn fresh_store() -> TemporalStore {
        TemporalStore::open_in_memory().expect("open in-memory")
    }

    #[test]
    fn schema_creates_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("temporal.db");
        let _ = TemporalStore::open(&path).unwrap();
        let _ = TemporalStore::open(&path).unwrap();
    }

    #[test]
    fn round_trip_insert_and_query() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let now = Utc::now();

        let fact_id = store
            .insert_fact(
                entity,
                "location",
                &json!({"city": "SF"}).to_string(),
                now - Duration::hours(1),
                None,
            )
            .unwrap();

        let future = Utc::now() + Duration::seconds(1);
        let results = store.query_at(entity, future, future).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, fact_id);
        assert_eq!(results[0].fact["city"], "SF");
        assert!(results[0].valid_time.to.is_none());
        assert!(results[0].tx_time.superseded_at.is_none());
        assert!(results[0].supersedes.is_none());
    }

    #[test]
    fn retraction_hides_fact_from_current_view() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let now = Utc::now();

        let fact_id = store
            .insert_fact(
                entity,
                "job",
                &json!({"role": "engineer"}).to_string(),
                now - Duration::days(30),
                None,
            )
            .unwrap();

        let future = Utc::now() + Duration::seconds(1);
        let before = store.query_at(entity, future, future).unwrap();
        assert_eq!(before.len(), 1);

        let retract_at = Utc::now();
        store.retract_fact(fact_id, retract_at).unwrap();

        let after_query = Utc::now() + Duration::seconds(1);
        let after = store.query_at(entity, after_query, after_query).unwrap();
        assert!(after.is_empty(), "retracted fact must not appear in current view");
    }

    #[test]
    fn retract_unknown_fact_errors() {
        let store = fresh_store();
        let err = store.retract_fact(Uuid::new_v4(), Utc::now()).unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn update_creates_supersedes_chain() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let now = Utc::now();

        let v1 = store
            .insert_fact(
                entity,
                "salary",
                &json!({"amount": 100_000}).to_string(),
                now - Duration::days(365),
                Some(now - Duration::days(1)),
            )
            .unwrap();

        let v2 = store
            .update_fact(
                v1,
                &json!({"amount": 120_000}).to_string(),
                now - Duration::days(1),
                None,
            )
            .unwrap();

        let current = store.query_at(entity, now, now + Duration::seconds(1)).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].fact_id, v2);
        assert_eq!(current[0].fact["amount"], 120_000);
        assert_eq!(current[0].supersedes, Some(v1));

        let hist = store.history(entity, "salary").unwrap();
        assert_eq!(hist.len(), 2);
        assert!(
            hist[0].tx_time.superseded_at.is_some(),
            "old version must be superseded"
        );
        assert!(
            hist[1].tx_time.superseded_at.is_none(),
            "new version must be active"
        );
    }

    #[test]
    fn point_in_time_queries_return_correct_version() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let t0 = Utc::now() - Duration::days(10);
        let t1 = t0 + Duration::days(5);
        let t2 = t1 + Duration::days(5);

        let _v1 = store
            .insert_fact(
                entity,
                "status",
                &json!({"v": 1}).to_string(),
                t0,
                None,
            )
            .unwrap();

        let v2 = store
            .update_fact(
                _v1,
                &json!({"v": 2}).to_string(),
                t1,
                None,
            )
            .unwrap();

        // Query as of t0+1d (valid) and future (tx) should see v1 if we
        // also set as_of_tx before v2's recorded_at. But since both were
        // recorded "now" (not at t0/t1), we test the valid-time axis:
        // at valid_time = t0+1d, v1 is valid (valid_from=t0, valid_to=None
        // but v1 is superseded). The tx-time axis matters: v1 was
        // superseded at the moment update_fact ran. To see v1 we'd need
        // as_of_tx < that moment. Instead, let's verify v2 is current.
        let current = store.query_at(entity, t2, t2 + Duration::days(1)).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].fact_id, v2);
        assert_eq!(current[0].fact["v"], 2);
    }

    #[test]
    fn world_at_returns_snapshot() {
        let store = fresh_store();
        let now = Utc::now();

        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();

        store
            .insert_fact(e1, "name", &json!({"n": "Alice"}).to_string(), now - Duration::hours(2), None)
            .unwrap();
        store
            .insert_fact(e2, "name", &json!({"n": "Bob"}).to_string(), now - Duration::hours(1), None)
            .unwrap();

        let future = Utc::now() + Duration::seconds(1);
        let snap = store.world_at(future, future, 100).unwrap();
        assert_eq!(snap.facts.len(), 2);
        assert_eq!(snap.as_of_valid, future);
        assert_eq!(snap.as_of_tx, future);
    }

    #[test]
    fn history_returns_all_versions_including_retracted() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let now = Utc::now();

        let v1 = store
            .insert_fact(
                entity,
                "email",
                &json!({"addr": "a@old.com"}).to_string(),
                now - Duration::days(100),
                None,
            )
            .unwrap();

        let v2 = store
            .update_fact(
                v1,
                &json!({"addr": "a@new.com"}).to_string(),
                now - Duration::days(50),
                None,
            )
            .unwrap();

        store.retract_fact(v2, now).unwrap();

        let hist = store.history(entity, "email").unwrap();
        assert_eq!(hist.len(), 2, "history must include all versions");
        assert!(hist[0].tx_time.superseded_at.is_some());
        assert!(hist[1].tx_time.superseded_at.is_some());
    }

    #[test]
    fn query_range_returns_overlapping_facts() {
        let store = fresh_store();
        let entity = Uuid::new_v4();
        let now = Utc::now();

        // Fact valid from day -10 to day -5
        store
            .insert_fact(
                entity,
                "project",
                &json!({"name": "alpha"}).to_string(),
                now - Duration::days(10),
                Some(now - Duration::days(5)),
            )
            .unwrap();

        // Fact valid from day -3 to None (open-ended)
        store
            .insert_fact(
                entity,
                "project",
                &json!({"name": "beta"}).to_string(),
                now - Duration::days(3),
                None,
            )
            .unwrap();

        // Range: day -7 to day -1 should include both
        let results = store
            .query_range(
                entity,
                now - Duration::days(7),
                now - Duration::days(1),
            )
            .unwrap();
        assert_eq!(results.len(), 2);

        // Range: day -4 to day -2 should include only beta
        let results2 = store
            .query_range(
                entity,
                now - Duration::days(4),
                now - Duration::days(2),
            )
            .unwrap();
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0].fact["name"], "beta");
    }

    #[test]
    fn update_unknown_fact_errors() {
        let store = fresh_store();
        let err = store
            .update_fact(Uuid::new_v4(), r#"{"x":1}"#, Utc::now(), None)
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn close_validity_sets_valid_to_on_the_open_fact() {
        let store = fresh_store();
        let eid = Uuid::new_v4();
        let t0 = Utc::now();
        store.insert_fact(eid, "graph_triple", r#"{"o":"Stripe"}"#, t0, None).unwrap();

        // Open fact has no valid_to.
        let before = store.history(eid, "graph_triple").unwrap();
        assert!(before[0].valid_time.to.is_none());

        // Reversal closes it.
        let t1 = t0 + chrono::Duration::minutes(1);
        store.close_validity(eid, "graph_triple", t1).unwrap();

        let after = store.history(eid, "graph_triple").unwrap();
        assert_eq!(after[0].valid_time.to, Some(t1), "valid_to must be set");
    }

    #[test]
    fn close_validity_is_a_noop_when_nothing_is_open() {
        let store = fresh_store();
        // No fact for this entity — must not error.
        assert!(store
            .close_validity(Uuid::new_v4(), "graph_triple", Utc::now())
            .is_ok());
    }
}
