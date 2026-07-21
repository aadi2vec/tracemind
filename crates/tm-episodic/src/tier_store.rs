//! SQLite-backed store for hierarchical memory tier assignments.
//!
//! Three tiers are maintained:
//!
//! * **Hot**  — accessed within the last 24 h, or explicitly pinned.
//! * **Warm** — accessed within the last 7 days.
//! * **Cold** — not accessed in 7+ days.
//!
//! Callers drive the lifecycle via [`TierStore::record_access`] on every
//! retrieval hit and [`TierStore::run_promotion_cycle`] periodically (e.g.
//! once per query or on a background timer) to recompute tiers from elapsed
//! time.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use uuid::Uuid;

use tm_types::{MemoryTier, Result, TraceMindError};

// ─── public output types ────────────────────────────────────────────────────

/// A single row returned by [`TierStore::get_all_in_tier`].
#[derive(Debug, Clone)]
pub struct TierEntry {
    pub memory_id: Uuid,
    pub memory_kind: String,
    pub tier: MemoryTier,
    pub access_count: u64,
    pub last_accessed_at: DateTime<Utc>,
    pub pinned: bool,
}

/// Summary produced by [`TierStore::run_promotion_cycle`].
#[derive(Debug, Clone, Default)]
pub struct PromotionReport {
    pub promoted_to_hot: usize,
    pub demoted_to_warm: usize,
    pub demoted_to_cold: usize,
}

// ─── store ───────────────────────────────────────────────────────────────────

pub struct TierStore {
    conn: Connection,
}

impl TierStore {
    /// Open (or create) a tier database at `path`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.ensure_schema()?;
        Ok(store)
    }

    /// Open an in-memory database (useful for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let store = Self { conn };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS memory_tiers (
                memory_id       TEXT NOT NULL PRIMARY KEY,
                memory_kind     TEXT NOT NULL,
                tier            TEXT NOT NULL DEFAULT 'warm',
                access_count    INTEGER NOT NULL DEFAULT 0,
                last_accessed_at TEXT NOT NULL,
                pinned          INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memory_tiers_tier
                ON memory_tiers(tier);
            "#,
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))
    }

    // ─── writes ──────────────────────────────────────────────────────────────

    /// Record an access event for `memory_id`.
    ///
    /// If the entry does not yet exist it is created with `kind` and
    /// immediately placed in the `hot` tier.  Subsequent calls increment
    /// `access_count`, refresh `last_accessed_at`, and recompute the tier.
    pub fn record_access(&self, memory_id: Uuid, kind: &str) -> Result<()> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let id_str = memory_id.to_string();
        let tier = MemoryTier::Hot.as_str(); // freshly accessed → hot

        self.conn
            .execute(
                r#"
                INSERT INTO memory_tiers
                    (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                VALUES (?1, ?2, ?3, 1, ?4, 0, ?4)
                ON CONFLICT(memory_id) DO UPDATE SET
                    access_count     = access_count + 1,
                    last_accessed_at = excluded.last_accessed_at,
                    tier             = CASE WHEN pinned = 1 THEN 'hot' ELSE excluded.tier END
                "#,
                params![id_str, kind, tier, now_str],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Pin `memory_id` so it always stays in the `hot` tier regardless of
    /// access recency.  Creates the entry if it does not exist.
    pub fn pin(&self, memory_id: Uuid) -> Result<()> {
        let now_str = Utc::now().to_rfc3339();
        let id_str = memory_id.to_string();

        self.conn
            .execute(
                r#"
                INSERT INTO memory_tiers
                    (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                VALUES (?1, 'unknown', 'hot', 0, ?2, 1, ?2)
                ON CONFLICT(memory_id) DO UPDATE SET
                    pinned = 1,
                    tier   = 'hot'
                "#,
                params![id_str, now_str],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Remove the pin from `memory_id`.  The tier will be recomputed on the
    /// next [`run_promotion_cycle`](Self::run_promotion_cycle) call.
    pub fn unpin(&self, memory_id: Uuid) -> Result<()> {
        let id_str = memory_id.to_string();
        self.conn
            .execute(
                "UPDATE memory_tiers SET pinned = 0 WHERE memory_id = ?1",
                params![id_str],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    // ─── reads ───────────────────────────────────────────────────────────────

    /// Return the current tier for `memory_id`.
    ///
    /// If no record exists the entry is treated as `Warm` (never accessed but
    /// not yet expired).
    pub fn get_tier(&self, memory_id: Uuid) -> Result<MemoryTier> {
        let id_str = memory_id.to_string();
        let result = self.conn.query_row(
            "SELECT tier FROM memory_tiers WHERE memory_id = ?1",
            params![id_str],
            |row| row.get::<_, String>(0),
        );
        match result {
            Ok(tier_str) => MemoryTier::from_str(&tier_str)
                .ok_or_else(|| TraceMindError::Storage(format!("unknown tier value: {tier_str}"))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(MemoryTier::Warm),
            Err(e) => Err(TraceMindError::Storage(e.to_string())),
        }
    }

    /// Return all entries currently assigned to `tier`.
    pub fn get_all_in_tier(&self, tier: MemoryTier) -> Result<Vec<TierEntry>> {
        let tier_str = tier.as_str();
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT memory_id, memory_kind, tier, access_count,
                       last_accessed_at, pinned
                FROM memory_tiers
                WHERE tier = ?1
                ORDER BY last_accessed_at DESC
                "#,
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![tier_str], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (id_str, kind, tier_str, access_count, last_str, pinned) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;

            let memory_id = Uuid::parse_str(&id_str)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let tier = MemoryTier::from_str(&tier_str).ok_or_else(|| {
                TraceMindError::Storage(format!("unknown tier value: {tier_str}"))
            })?;
            let last_accessed_at = last_str
                .parse::<DateTime<Utc>>()
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;

            out.push(TierEntry {
                memory_id,
                memory_kind: kind,
                tier,
                access_count: access_count as u64,
                last_accessed_at,
                pinned: pinned != 0,
            });
        }
        Ok(out)
    }

    // ─── maintenance ─────────────────────────────────────────────────────────

    /// Scan every entry and recompute its tier from `last_accessed_at` vs now.
    ///
    /// Pinned entries are always forced to `hot`.  Returns a [`PromotionReport`]
    /// with counts of how many entries changed tier.
    pub fn run_promotion_cycle(&self) -> Result<PromotionReport> {
        let now = Utc::now();

        // Pull every row we might need to update.
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT memory_id, tier, last_accessed_at, pinned
                FROM memory_tiers
                "#,
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        struct Row {
            memory_id: String,
            old_tier: String,
            last_accessed_at: String,
            pinned: bool,
        }

        let rows: Vec<Row> = stmt
            .query_map([], |row| {
                Ok(Row {
                    memory_id: row.get(0)?,
                    old_tier: row.get(1)?,
                    last_accessed_at: row.get(2)?,
                    pinned: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut report = PromotionReport::default();

        for row in rows {
            let new_tier = if row.pinned {
                MemoryTier::Hot
            } else {
                let last = row
                    .last_accessed_at
                    .parse::<DateTime<Utc>>()
                    .unwrap_or(now);
                let secs_ago = now
                    .signed_duration_since(last)
                    .num_seconds()
                    .max(0) as u64;
                MemoryTier::from_access_recency(secs_ago)
            };

            let old_tier = MemoryTier::from_str(&row.old_tier).unwrap_or(MemoryTier::Warm);
            if new_tier == old_tier {
                continue;
            }

            // Update the tier in-place.
            self.conn
                .execute(
                    "UPDATE memory_tiers SET tier = ?1 WHERE memory_id = ?2",
                    params![new_tier.as_str(), row.memory_id],
                )
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;

            // Tally the direction of change.
            match (old_tier, new_tier) {
                (_, MemoryTier::Hot) => report.promoted_to_hot += 1,
                (MemoryTier::Hot, MemoryTier::Warm) | (MemoryTier::Cold, MemoryTier::Warm) => {
                    report.demoted_to_warm += 1
                }
                (_, MemoryTier::Cold) => report.demoted_to_cold += 1,
                _ => {}
            }
        }

        Ok(report)
    }

    /// Force all entries whose computed tier is `Cold` to be written as `cold`
    /// in the database.  Returns the number of entries that were updated.
    ///
    /// This is a targeted variant of [`run_promotion_cycle`](Self::run_promotion_cycle)
    /// that only handles demotion to cold and is cheaper when callers only care
    /// about freeing cold-tier budget.
    pub fn demote_cold(&self) -> Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::seconds(604_800);
        let cutoff_str = cutoff.to_rfc3339();

        let count = self
            .conn
            .execute(
                r#"
                UPDATE memory_tiers
                SET tier = 'cold'
                WHERE pinned = 0
                  AND tier != 'cold'
                  AND last_accessed_at <= ?1
                "#,
                params![cutoff_str],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(count)
    }
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn store() -> TierStore {
        TierStore::open_in_memory().expect("in-memory store")
    }

    // ── record_access promotes to Hot ───────────────────────────────────────

    #[test]
    fn record_access_creates_hot_entry() {
        let ts = store();
        let id = Uuid::new_v4();
        ts.record_access(id, "trace").expect("record_access");
        assert_eq!(ts.get_tier(id).expect("get_tier"), MemoryTier::Hot);
    }

    #[test]
    fn record_access_increments_count() {
        let ts = store();
        let id = Uuid::new_v4();
        for _ in 0..3 {
            ts.record_access(id, "entity").expect("record_access");
        }
        let entries = ts.get_all_in_tier(MemoryTier::Hot).expect("get_all_in_tier");
        let entry = entries.iter().find(|e| e.memory_id == id).expect("entry");
        assert_eq!(entry.access_count, 3);
    }

    // ── run_promotion_cycle demotes stale entries ────────────────────────────

    #[test]
    fn promotion_cycle_demotes_old_entries_to_cold() {
        let ts = store();
        let id = Uuid::new_v4();

        // Insert directly with an ancient last_accessed_at.
        let ancient = (Utc::now() - Duration::days(30)).to_rfc3339();
        let now_str = Utc::now().to_rfc3339();
        ts.conn
            .execute(
                r#"INSERT INTO memory_tiers
                   (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                   VALUES (?1, 'trace', 'hot', 1, ?2, 0, ?3)"#,
                params![id.to_string(), ancient, now_str],
            )
            .unwrap();

        let report = ts.run_promotion_cycle().expect("promotion_cycle");
        assert!(report.demoted_to_cold >= 1, "expected at least one demotion to cold");
        assert_eq!(ts.get_tier(id).expect("get_tier"), MemoryTier::Cold);
    }

    #[test]
    fn promotion_cycle_warm_to_cold_after_7_days() {
        let ts = store();
        let id = Uuid::new_v4();

        // 8 days ago → cold.
        let old = (Utc::now() - Duration::days(8)).to_rfc3339();
        let now_str = Utc::now().to_rfc3339();
        ts.conn
            .execute(
                r#"INSERT INTO memory_tiers
                   (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                   VALUES (?1, 'entity', 'warm', 5, ?2, 0, ?3)"#,
                params![id.to_string(), old, now_str],
            )
            .unwrap();

        let report = ts.run_promotion_cycle().expect("promotion_cycle");
        assert!(report.demoted_to_cold >= 1);
        assert_eq!(ts.get_tier(id).unwrap(), MemoryTier::Cold);
    }

    // ── pin keeps Hot regardless of access time ──────────────────────────────

    #[test]
    fn pin_keeps_hot_after_promotion_cycle() {
        let ts = store();
        let id = Uuid::new_v4();

        // Insert with ancient timestamp but pinned.
        let ancient = (Utc::now() - Duration::days(30)).to_rfc3339();
        let now_str = Utc::now().to_rfc3339();
        ts.conn
            .execute(
                r#"INSERT INTO memory_tiers
                   (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                   VALUES (?1, 'trace', 'hot', 1, ?2, 1, ?3)"#,
                params![id.to_string(), ancient, now_str],
            )
            .unwrap();

        ts.run_promotion_cycle().expect("promotion_cycle");
        // Pinned entry must still be hot.
        assert_eq!(ts.get_tier(id).expect("get_tier"), MemoryTier::Hot);
    }

    #[test]
    fn pin_api_forces_hot() {
        let ts = store();
        let id = Uuid::new_v4();

        // Record access (creates hot entry), back-date it, unpin shouldn't help yet.
        ts.record_access(id, "triple").unwrap();
        // Pin via API.
        ts.pin(id).unwrap();

        // Back-date last_accessed_at to simulate stale.
        let ancient = (Utc::now() - Duration::days(30)).to_rfc3339();
        ts.conn
            .execute(
                "UPDATE memory_tiers SET last_accessed_at = ?1 WHERE memory_id = ?2",
                params![ancient, id.to_string()],
            )
            .unwrap();

        ts.run_promotion_cycle().unwrap();
        // Must remain hot because pinned.
        assert_eq!(ts.get_tier(id).unwrap(), MemoryTier::Hot);
    }

    #[test]
    fn unpin_allows_demotion() {
        let ts = store();
        let id = Uuid::new_v4();

        // Create pinned entry with ancient access time.
        let ancient = (Utc::now() - Duration::days(30)).to_rfc3339();
        let now_str = Utc::now().to_rfc3339();
        ts.conn
            .execute(
                r#"INSERT INTO memory_tiers
                   (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                   VALUES (?1, 'trace', 'hot', 1, ?2, 1, ?3)"#,
                params![id.to_string(), ancient, now_str],
            )
            .unwrap();

        // Unpin then run cycle — should demote to cold.
        ts.unpin(id).unwrap();
        ts.run_promotion_cycle().unwrap();
        assert_eq!(ts.get_tier(id).unwrap(), MemoryTier::Cold);
    }

    // ── demote_cold ──────────────────────────────────────────────────────────

    #[test]
    fn demote_cold_skips_recent_and_pinned() {
        let ts = store();

        // Recent entry (should stay hot after demote_cold).
        let recent_id = Uuid::new_v4();
        ts.record_access(recent_id, "trace").unwrap();

        // Stale entry (should become cold).
        let stale_id = Uuid::new_v4();
        let old = (Utc::now() - Duration::days(10)).to_rfc3339();
        let now_str = Utc::now().to_rfc3339();
        ts.conn
            .execute(
                r#"INSERT INTO memory_tiers
                   (memory_id, memory_kind, tier, access_count, last_accessed_at, pinned, created_at)
                   VALUES (?1, 'entity', 'warm', 2, ?2, 0, ?3)"#,
                params![stale_id.to_string(), old, now_str],
            )
            .unwrap();

        let count = ts.demote_cold().expect("demote_cold");
        assert_eq!(count, 1, "only the stale entry should be demoted");
        assert_eq!(ts.get_tier(stale_id).unwrap(), MemoryTier::Cold);
        assert_eq!(ts.get_tier(recent_id).unwrap(), MemoryTier::Hot);
    }

    // ── get_tier default ─────────────────────────────────────────────────────

    #[test]
    fn get_tier_defaults_to_warm_for_unknown_id() {
        let ts = store();
        let id = Uuid::new_v4();
        assert_eq!(ts.get_tier(id).unwrap(), MemoryTier::Warm);
    }

    // ── get_all_in_tier ──────────────────────────────────────────────────────

    #[test]
    fn get_all_in_tier_returns_correct_entries() {
        let ts = store();
        let hot1 = Uuid::new_v4();
        let hot2 = Uuid::new_v4();
        ts.record_access(hot1, "trace").unwrap();
        ts.record_access(hot2, "entity").unwrap();

        let hot_entries = ts.get_all_in_tier(MemoryTier::Hot).unwrap();
        let ids: Vec<Uuid> = hot_entries.iter().map(|e| e.memory_id).collect();
        assert!(ids.contains(&hot1));
        assert!(ids.contains(&hot2));

        let cold_entries = ts.get_all_in_tier(MemoryTier::Cold).unwrap();
        assert!(cold_entries.is_empty());
    }
}
