//! Q4.14 — Policy provenance: every mutation writes a provenance edge;
//! rollback events feed back as negative signals.
//! Schema: policy_mutations and policy_rollbacks tables.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS policy_mutations (
            id TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NOT NULL,
            mutation_kind TEXT NOT NULL,
            delta_f1 REAL NOT NULL DEFAULT 0.0,
            delta_latency REAL NOT NULL DEFAULT 0.0,
            delta_contradiction REAL NOT NULL DEFAULT 0.0,
            delta_multihop REAL NOT NULL DEFAULT 0.0,
            accepted INTEGER NOT NULL DEFAULT 0,
            evidence TEXT,  -- JSON array of trace UUIDs
            generated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS policy_rollbacks (
            id TEXT NOT NULL PRIMARY KEY,
            mutation_id TEXT NOT NULL,
            mutation_kind TEXT NOT NULL,
            rolled_back_at TEXT NOT NULL,
            user_note TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_mutations_parent ON policy_mutations(parent_id);
        CREATE INDEX IF NOT EXISTS idx_rollbacks_mutation ON policy_rollbacks(mutation_id);",
    )
    .map_err(|e| TraceMindError::Storage(format!("policy_provenance schema: {e}")))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMutation {
    pub id: Uuid,
    pub parent_id: Uuid,
    pub mutation_kind: String,
    pub delta_scores: [f32; 4],
    pub accepted: bool,
    pub evidence: Vec<Uuid>,
    pub generated_at: DateTime<Utc>,
}

pub fn record_mutation(conn: &Connection, m: &StoredMutation) -> Result<()> {
    let evidence_json = serde_json::to_string(&m.evidence).unwrap_or_default();
    conn.execute(
        "INSERT OR REPLACE INTO policy_mutations
         (id, parent_id, mutation_kind, delta_f1, delta_latency, delta_contradiction, delta_multihop, accepted, evidence, generated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            m.id.to_string(),
            m.parent_id.to_string(),
            m.mutation_kind,
            m.delta_scores[0] as f64,
            m.delta_scores[1] as f64,
            m.delta_scores[2] as f64,
            m.delta_scores[3] as f64,
            m.accepted as i64,
            evidence_json,
            m.generated_at.to_rfc3339(),
        ],
    )
    .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(())
}

pub fn record_rollback(
    conn: &Connection,
    mutation_id: Uuid,
    kind: &str,
    note: Option<&str>,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    conn.execute(
        "INSERT INTO policy_rollbacks (id, mutation_id, mutation_kind, rolled_back_at, user_note)
         VALUES (?1,?2,?3,?4,?5)",
        params![
            id.to_string(),
            mutation_id.to_string(),
            kind,
            Utc::now().to_rfc3339(),
            note
        ],
    )
    .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(id)
}

pub fn recent_mutations(conn: &Connection, limit: usize) -> Result<Vec<StoredMutation>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, parent_id, mutation_kind, delta_f1, delta_latency, delta_contradiction, delta_multihop, accepted, evidence, generated_at
         FROM policy_mutations ORDER BY generated_at DESC LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let rows = stmt
        .query_map(params![limit as i64], |row| {
            let id_s: String = row.get(0)?;
            let parent_s: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let df1: f64 = row.get(3)?;
            let dlat: f64 = row.get(4)?;
            let dcon: f64 = row.get(5)?;
            let dmh: f64 = row.get(6)?;
            let accepted: i64 = row.get(7)?;
            let evidence_s: String = row.get(8).unwrap_or_default();
            let gen_s: String = row.get(9)?;
            Ok((
                id_s,
                parent_s,
                kind,
                [df1 as f32, dlat as f32, dcon as f32, dmh as f32],
                accepted != 0,
                evidence_s,
                gen_s,
            ))
        })
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .filter_map(
            |(id_s, par_s, kind, scores, accepted, ev_s, gen_s)| {
                let id = Uuid::parse_str(&id_s).ok()?;
                let parent_id = Uuid::parse_str(&par_s).ok()?;
                let evidence: Vec<Uuid> = serde_json::from_str(&ev_s).unwrap_or_default();
                let generated_at = gen_s.parse::<DateTime<Utc>>().ok()?;
                Some(StoredMutation {
                    id,
                    parent_id,
                    mutation_kind: kind,
                    delta_scores: scores,
                    accepted,
                    evidence,
                    generated_at,
                })
            },
        )
        .collect();
    Ok(rows)
}

pub fn rollback_count_by_kind(conn: &Connection) -> Result<Vec<(String, usize)>> {
    let mut stmt = conn
        .prepare(
            "SELECT mutation_kind, COUNT(*) as cnt FROM policy_rollbacks GROUP BY mutation_kind ORDER BY cnt DESC",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn record_and_retrieve_mutation() {
        let conn = open_test_db();
        let m = StoredMutation {
            id: Uuid::new_v4(),
            parent_id: Uuid::new_v4(),
            mutation_kind: "arm_weight_update".to_string(),
            delta_scores: [0.05, -0.01, 0.0, 0.02],
            accepted: true,
            evidence: vec![Uuid::new_v4()],
            generated_at: Utc::now(),
        };
        record_mutation(&conn, &m).unwrap();

        let mutations = recent_mutations(&conn, 10).unwrap();
        assert_eq!(mutations.len(), 1);
        assert_eq!(mutations[0].id, m.id);
        assert_eq!(mutations[0].mutation_kind, "arm_weight_update");
        assert!(mutations[0].accepted);
        assert!((mutations[0].delta_scores[0] - 0.05).abs() < 1e-5);
    }

    #[test]
    fn record_rollback_and_count_by_kind() {
        let conn = open_test_db();

        // Insert some mutations first so we have valid mutation_ids
        let m1 = StoredMutation {
            id: Uuid::new_v4(),
            parent_id: Uuid::new_v4(),
            mutation_kind: "gepa_spike".to_string(),
            delta_scores: [0.0; 4],
            accepted: false,
            evidence: vec![],
            generated_at: Utc::now(),
        };
        let m2 = StoredMutation {
            id: Uuid::new_v4(),
            parent_id: Uuid::new_v4(),
            mutation_kind: "gepa_spike".to_string(),
            delta_scores: [0.0; 4],
            accepted: false,
            evidence: vec![],
            generated_at: Utc::now(),
        };
        record_mutation(&conn, &m1).unwrap();
        record_mutation(&conn, &m2).unwrap();

        let rb1 = record_rollback(&conn, m1.id, "gepa_spike", Some("worse than baseline")).unwrap();
        let rb2 = record_rollback(&conn, m2.id, "gepa_spike", None).unwrap();

        // Two distinct rollback IDs
        assert_ne!(rb1, rb2);

        let counts = rollback_count_by_kind(&conn).unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].0, "gepa_spike");
        assert_eq!(counts[0].1, 2);
    }

    #[test]
    fn recent_mutations_order_and_limit() {
        let conn = open_test_db();
        let parent = Uuid::new_v4();

        // Insert 5 mutations
        for i in 0..5u8 {
            let m = StoredMutation {
                id: Uuid::new_v4(),
                parent_id: parent,
                mutation_kind: format!("kind_{i}"),
                delta_scores: [i as f32; 4],
                accepted: i % 2 == 0,
                evidence: vec![],
                generated_at: Utc::now(),
            };
            record_mutation(&conn, &m).unwrap();
        }

        // Limit to 3
        let mutations = recent_mutations(&conn, 3).unwrap();
        assert_eq!(mutations.len(), 3);

        // All must share the same parent_id
        for m in &mutations {
            assert_eq!(m.parent_id, parent);
        }
    }
}
