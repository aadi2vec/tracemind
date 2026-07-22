//! Q3.4 — Contradiction-rate metric via temporal-KG conflict detection.
//!
//! The contradiction rate is defined as the fraction of (subject, predicate)
//! pairs that have two or more conflicting object values with overlapping
//! validity windows. This is the head-to-head Pareto axis that cloud
//! competitors (Mem0, Zep, Letta) cannot win on architecturally because
//! they don't retain temporal edges.
//!
//! Contradiction detection uses temporal-KG windows (valid_from / valid_to)
//! rather than string-diff or embedding cosine — per the charter's explicit
//! decision in §9 R4.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tm_types::{Result, TraceMindError};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A detected temporal contradiction: two triples with the same
/// (subject, predicate) pair whose object values differ and whose
/// validity windows overlap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalConflict {
    pub subject_id: Uuid,
    pub predicate: String,
    /// Earlier-recorded object.
    pub object_a_id: Uuid,
    pub object_a_valid_from: Option<DateTime<Utc>>,
    pub object_a_valid_to: Option<DateTime<Utc>>,
    /// Later-recorded object.
    pub object_b_id: Uuid,
    pub object_b_valid_from: Option<DateTime<Utc>>,
    pub object_b_valid_to: Option<DateTime<Utc>>,
    pub detected_at: DateTime<Utc>,
}

/// Aggregate contradiction-rate statistics for a graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionRateStats {
    /// Total (subject, predicate) pairs examined.
    pub pairs_examined: usize,
    /// Pairs with at least one temporal conflict.
    pub conflicting_pairs: usize,
    /// Contradiction rate: conflicting_pairs / pairs_examined.
    pub rate: f64,
    /// Total distinct conflicts detected.
    pub total_conflicts: usize,
    pub computed_at: DateTime<Utc>,
}

impl ContradictionRateStats {
    /// Whether the rate is below the target threshold.
    /// Target: < half of Mem0 baseline (Q4 exit gate).
    pub fn below_target(&self, target_rate: f64) -> bool {
        self.rate < target_rate
    }
}

// ---------------------------------------------------------------------------
// Schema (valid_from / valid_to on kg_relations)
// ---------------------------------------------------------------------------

/// Ensure `valid_from` and `valid_to` columns exist on `kg_relations`.
/// Idempotent — uses `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` pattern.
pub fn ensure_temporal_columns(conn: &Connection) -> Result<()> {
    // SQLite doesn't support IF NOT EXISTS on ALTER TABLE, so we ignore errors.
    let _ = conn.execute(
        "ALTER TABLE kg_relations ADD COLUMN valid_from TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE kg_relations ADD COLUMN valid_to TEXT",
        [],
    );
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_relations_valid_from
             ON kg_relations(source_id, rel_type, valid_from);
         CREATE INDEX IF NOT EXISTS idx_relations_valid_to
             ON kg_relations(source_id, rel_type, valid_to);",
    )
    .map_err(|e| TraceMindError::Storage(format!("temporal columns index: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

/// Detect temporal conflicts in the knowledge graph.
///
/// A conflict exists when two triples share the same (source, predicate)
/// but have different targets and their validity windows overlap.
///
/// Windows overlap when: max(a.from, b.from) <= min(a.to, b.to)
/// where NULL valid_from = -∞ and NULL valid_to = +∞.
pub fn detect_conflicts(conn: &Connection, limit: usize) -> Result<Vec<TemporalConflict>> {
    // Self-join on kg_relations to find (subject, predicate) pairs
    // with different targets and overlapping time windows.
    let mut stmt = conn
        .prepare(
            "SELECT
                r1.source_id   AS subject_id,
                r1.rel_type    AS predicate,
                r1.target_id   AS obj_a,
                r1.valid_from  AS vf_a,
                r1.valid_to    AS vt_a,
                r2.target_id   AS obj_b,
                r2.valid_from  AS vf_b,
                r2.valid_to    AS vt_b
             FROM kg_relations r1
             JOIN kg_relations r2
               ON  r1.source_id = r2.source_id
               AND r1.rel_type  = r2.rel_type
               AND r1.target_id < r2.target_id
             WHERE
               -- Overlap check: [vf_a, vt_a] ∩ [vf_b, vt_b] ≠ ∅
               -- NULL vf → -∞, NULL vt → +∞
               (r1.valid_to   IS NULL OR r2.valid_from IS NULL OR r1.valid_to   >= r2.valid_from)
               AND
               (r2.valid_to   IS NULL OR r1.valid_from IS NULL OR r2.valid_to   >= r1.valid_from)
             LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let parse_dt = |s: Option<String>| -> Option<DateTime<Utc>> {
        s.and_then(|s| s.parse::<DateTime<Utc>>().ok())
    };

    let conflicts = stmt
        .query_map(params![limit as i64], |row| {
            let subject_str: String = row.get(0)?;
            let predicate: String = row.get(1)?;
            let obj_a_str: String = row.get(2)?;
            let vf_a: Option<String> = row.get(3)?;
            let vt_a: Option<String> = row.get(4)?;
            let obj_b_str: String = row.get(5)?;
            let vf_b: Option<String> = row.get(6)?;
            let vt_b: Option<String> = row.get(7)?;
            Ok((subject_str, predicate, obj_a_str, vf_a, vt_a, obj_b_str, vf_b, vt_b))
        })
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .filter_map(|(subj, pred, a, vf_a, vt_a, b, vf_b, vt_b)| {
            let subject_id = Uuid::parse_str(&subj).ok()?;
            let object_a_id = Uuid::parse_str(&a).ok()?;
            let object_b_id = Uuid::parse_str(&b).ok()?;
            Some(TemporalConflict {
                subject_id,
                predicate: pred,
                object_a_id,
                object_a_valid_from: parse_dt(vf_a),
                object_a_valid_to: parse_dt(vt_a),
                object_b_id,
                object_b_valid_from: parse_dt(vf_b),
                object_b_valid_to: parse_dt(vt_b),
                detected_at: Utc::now(),
            })
        })
        .collect::<Vec<_>>();

    Ok(conflicts)
}

/// Compute the contradiction-rate metric for the entire graph.
pub fn compute_contradiction_rate(conn: &Connection) -> Result<ContradictionRateStats> {
    // Count distinct (source, predicate) pairs total
    let pairs_examined: usize = conn
        .query_row(
            "SELECT COUNT(DISTINCT source_id || '|' || rel_type) FROM kg_relations",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|n| n as usize)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

    let conflicts = detect_conflicts(conn, 10_000)?;

    let conflicting_pairs: std::collections::HashSet<_> = conflicts
        .iter()
        .map(|c| (c.subject_id, c.predicate.clone()))
        .collect();

    let conflicting_count = conflicting_pairs.len();
    let rate = if pairs_examined == 0 {
        0.0
    } else {
        conflicting_count as f64 / pairs_examined as f64
    };

    Ok(ContradictionRateStats {
        pairs_examined,
        conflicting_pairs: conflicting_count,
        rate,
        total_conflicts: conflicts.len(),
        computed_at: Utc::now(),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE kg_relations (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                rel_type TEXT NOT NULL,
                target_id TEXT NOT NULL,
                valid_from TEXT,
                valid_to TEXT
            )",
        )
        .unwrap();
        conn
    }

    #[test]
    fn no_conflicts_when_empty() {
        let conn = setup_db();
        let stats = compute_contradiction_rate(&conn).unwrap();
        assert_eq!(stats.rate, 0.0);
        assert_eq!(stats.total_conflicts, 0);
    }

    #[test]
    fn detects_overlapping_temporal_conflict() {
        let conn = setup_db();
        let subj = Uuid::new_v4().to_string();
        let obj_a = Uuid::new_v4().to_string();
        let obj_b = Uuid::new_v4().to_string();

        // Two WorksAt facts for same subject, different objects, overlapping windows
        conn.execute(
            "INSERT INTO kg_relations (id, source_id, rel_type, target_id, valid_from, valid_to)
             VALUES (?1, ?2, 'WorksAt', ?3, '2026-01-01T00:00:00Z', '2026-12-31T00:00:00Z')",
            params![Uuid::new_v4().to_string(), subj, obj_a],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kg_relations (id, source_id, rel_type, target_id, valid_from, valid_to)
             VALUES (?1, ?2, 'WorksAt', ?3, '2026-06-01T00:00:00Z', NULL)",
            params![Uuid::new_v4().to_string(), subj, obj_b],
        )
        .unwrap();

        let conflicts = detect_conflicts(&conn, 100).unwrap();
        assert_eq!(conflicts.len(), 1, "expected one conflict: {conflicts:?}");
        assert_eq!(conflicts[0].predicate, "WorksAt");

        let stats = compute_contradiction_rate(&conn).unwrap();
        assert_eq!(stats.conflicting_pairs, 1);
        assert!((stats.rate - 1.0).abs() < 1e-9);
    }

    #[test]
    fn no_conflict_when_windows_do_not_overlap() {
        let conn = setup_db();
        let subj = Uuid::new_v4().to_string();
        let obj_a = Uuid::new_v4().to_string();
        let obj_b = Uuid::new_v4().to_string();

        // Two WorksAt facts with non-overlapping windows
        conn.execute(
            "INSERT INTO kg_relations (id, source_id, rel_type, target_id, valid_from, valid_to)
             VALUES (?1, ?2, 'WorksAt', ?3, '2025-01-01T00:00:00Z', '2025-12-31T00:00:00Z')",
            params![Uuid::new_v4().to_string(), subj, obj_a],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kg_relations (id, source_id, rel_type, target_id, valid_from, valid_to)
             VALUES (?1, ?2, 'WorksAt', ?3, '2026-01-01T00:00:00Z', NULL)",
            params![Uuid::new_v4().to_string(), subj, obj_b],
        )
        .unwrap();

        let conflicts = detect_conflicts(&conn, 100).unwrap();
        assert_eq!(conflicts.len(), 0, "no overlap → no conflict");
    }
}
