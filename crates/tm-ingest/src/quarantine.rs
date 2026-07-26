//! Quarantine tier — 48h holding pen with second-pass PII re-gate.
//!
//! Implements plan §3.2 and I2 from
//! `docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md`. Ambient captures enter
//! quarantine on write, sit there for up to 48h, then get re-gated by
//! `tm-governance` before being promoted. If the second pass detects PII
//! that snuck through the first gate, the item is discarded instead.
//!
//! Storage: a single SQLite table `quarantine_items` sitting inside the
//! standard `memory.db`. The store is tiny — a few thousand rows at most,
//! since items either promote to hot or discard within 48h.
//!
//! Callers own the "what is the content of memory X" question. The
//! [`QuarantineStore::promote_due`] method takes a closure so it does not
//! have to depend on `tm-graph` / `tm-vector` for lookup.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_governance::GovernanceFilter;
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// Default SQLite file used when opening under the standard data dir.
pub const MEMORY_DB_FILE: &str = "memory.db";

/// A single quarantined memory awaiting promotion or discard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineItem {
    pub memory_id: Uuid,
    pub at: DateTime<Utc>,
    pub source: String,
    pub expires_at: DateTime<Utc>,
    /// Result of the first-pass gate at ingest. `false` means the item was
    /// accepted despite failing a gate (should never happen, but the column
    /// keeps the audit story truthful).
    pub gate_first_pass: bool,
}

/// Result of re-gating an expired quarantine item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionAction {
    Promoted,
    /// The re-gate detected PII the first pass missed. Item is dropped from
    /// quarantine and the caller is expected to forget its derived data via
    /// [`crate::propagate::Propagator`].
    DiscardedPii,
    /// Content lookup returned `None` — content is gone (already forgotten
    /// or lost). We remove the quarantine row too.
    DiscardedMissing,
}

/// Per-item outcome of a [`QuarantineStore::promote_due`] pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionOutcome {
    pub memory_id: Uuid,
    pub action: PromotionAction,
}

pub struct QuarantineStore {
    conn: Connection,
    path: PathBuf,
}

impl QuarantineStore {
    /// Open (or create) the quarantine table inside `dir/memory.db`.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(MEMORY_DB_FILE);
        Self::open_path(path)
    }

    /// Open at an exact path (used by tests and non-standard layouts).
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| TraceMindError::Storage(format!("open {}: {e}", path.display())))?;
        Self::ensure_schema(&conn)?;
        Ok(Self { conn, path })
    }

    /// Open against an already-owned connection (useful when tm-graph is
    /// already using the same file — the caller passes a fresh handle).
    pub fn open_with_connection(conn: Connection) -> Result<Self> {
        Self::ensure_schema(&conn)?;
        Ok(Self {
            conn,
            path: PathBuf::new(),
        })
    }

    fn ensure_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS quarantine_items (
                memory_id        TEXT NOT NULL PRIMARY KEY,
                at               TEXT NOT NULL,
                source           TEXT NOT NULL,
                expires_at       TEXT NOT NULL,
                gate_first_pass  INTEGER NOT NULL DEFAULT 1
            );
            CREATE INDEX IF NOT EXISTS idx_quarantine_expires
                ON quarantine_items(expires_at);
            "#,
        )
        .map_err(|e| TraceMindError::Storage(format!("quarantine schema: {e}")))?;
        Ok(())
    }

    /// Insert a new quarantine row. Expiry is `at + 48h` per §3.2.
    /// Idempotent — repeat enqueues update the timestamps but keep the row.
    pub fn enqueue(
        &self,
        memory_id: Uuid,
        at: DateTime<Utc>,
        source: impl Into<String>,
    ) -> Result<()> {
        let expires = at + chrono::Duration::seconds(
            tm_types::MemoryTier::Quarantine
                .promote_after_secs()
                .unwrap_or(172_800) as i64,
        );
        self.conn
            .execute(
                r#"
                INSERT INTO quarantine_items (memory_id, at, source, expires_at, gate_first_pass)
                VALUES (?1, ?2, ?3, ?4, 1)
                ON CONFLICT(memory_id) DO UPDATE SET
                    at = excluded.at,
                    source = excluded.source,
                    expires_at = excluded.expires_at
                "#,
                params![
                    memory_id.to_string(),
                    at.to_rfc3339(),
                    source.into(),
                    expires.to_rfc3339(),
                ],
            )
            .map_err(|e| TraceMindError::Storage(format!("enqueue quarantine: {e}")))?;
        Ok(())
    }

    /// Items whose expiry is `> now` (still in the holding pen).
    pub fn pending_now(&self, now: DateTime<Utc>) -> Result<Vec<QuarantineItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, at, source, expires_at, gate_first_pass \
                 FROM quarantine_items WHERE expires_at > ?1 ORDER BY expires_at ASC",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![now.to_rfc3339()], parse_row)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(rows)
    }

    /// Items whose expiry is `<= now` (due for re-gate).
    pub fn due(&self, now: DateTime<Utc>) -> Result<Vec<QuarantineItem>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, at, source, expires_at, gate_first_pass \
                 FROM quarantine_items WHERE expires_at <= ?1 ORDER BY expires_at ASC",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![now.to_rfc3339()], parse_row)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(rows)
    }

    /// Mark an item as promoted by removing it from the quarantine table.
    pub fn promote(&self, memory_id: Uuid) -> Result<bool> {
        let removed = self
            .conn
            .execute(
                "DELETE FROM quarantine_items WHERE memory_id = ?1",
                params![memory_id.to_string()],
            )
            .map_err(|e| TraceMindError::Storage(format!("promote: {e}")))?;
        Ok(removed > 0)
    }

    /// Discard an item — same delete as promote but reserved for the "PII
    /// caught on second pass" path so callers can log intent cleanly.
    pub fn discard(&self, memory_id: Uuid) -> Result<bool> {
        let removed = self
            .conn
            .execute(
                "DELETE FROM quarantine_items WHERE memory_id = ?1",
                params![memory_id.to_string()],
            )
            .map_err(|e| TraceMindError::Storage(format!("discard: {e}")))?;
        Ok(removed > 0)
    }

    /// Second-pass PII gate over all items whose expiry has elapsed.
    ///
    /// `contents(memory_id)` returns the text to re-scan (or `None` when
    /// the memory is gone). Items that pass the gate are `Promoted`;
    /// items that fail are `DiscardedPii`; items with missing content are
    /// `DiscardedMissing`. Either way the quarantine row is removed.
    pub fn promote_due(
        &self,
        now: DateTime<Utc>,
        gate: &GovernanceFilter,
        contents: &dyn Fn(Uuid) -> Option<String>,
    ) -> Result<Vec<PromotionOutcome>> {
        let due = self.due(now)?;
        let mut outcomes = Vec::with_capacity(due.len());
        for item in due {
            let action = match contents(item.memory_id) {
                None => {
                    self.discard(item.memory_id)?;
                    PromotionAction::DiscardedMissing
                }
                Some(text) => {
                    // The public `check` runs PII + confidence; we only want
                    // the PII half here, so pass a confidence that clears
                    // the default threshold and interpret only the PII error.
                    match gate.check(&text, 1.0) {
                        Err(TraceMindError::PiiDetected) => {
                            self.discard(item.memory_id)?;
                            PromotionAction::DiscardedPii
                        }
                        _ => {
                            self.promote(item.memory_id)?;
                            PromotionAction::Promoted
                        }
                    }
                }
            };
            outcomes.push(PromotionOutcome {
                memory_id: item.memory_id,
                action,
            });
        }
        Ok(outcomes)
    }

    /// Path this store is backed by (empty when opened via `open_with_connection`).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn parse_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuarantineItem> {
    let memory_id: String = row.get(0)?;
    let at: String = row.get(1)?;
    let source: String = row.get(2)?;
    let expires_at: String = row.get(3)?;
    let gate_first_pass: i64 = row.get(4)?;
    Ok(QuarantineItem {
        memory_id: Uuid::parse_str(&memory_id).unwrap_or_else(|_| Uuid::nil()),
        at: at.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
        source,
        expires_at: expires_at
            .parse::<DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now()),
        gate_first_pass: gate_first_pass != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> QuarantineStore {
        let conn = Connection::open_in_memory().unwrap();
        QuarantineStore::open_with_connection(conn).unwrap()
    }

    #[test]
    fn enqueue_sets_48h_expiry() {
        let s = tmp_store();
        let id = Uuid::new_v4();
        let at: DateTime<Utc> = "2026-07-22T10:00:00Z".parse().unwrap();
        s.enqueue(id, at, "clipboard").unwrap();

        let all = s.pending_now(at).unwrap();
        assert_eq!(all.len(), 1);
        let expected = at + chrono::Duration::hours(48);
        assert_eq!(all[0].expires_at, expected);
        assert_eq!(all[0].source, "clipboard");
        assert!(all[0].gate_first_pass);
    }

    #[test]
    fn only_expired_items_appear_in_due() {
        let s = tmp_store();
        let base: DateTime<Utc> = "2026-07-22T10:00:00Z".parse().unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        s.enqueue(a, base, "clipboard").unwrap();
        s.enqueue(b, base + chrono::Duration::hours(24), "clipboard").unwrap();
        s.enqueue(c, base + chrono::Duration::hours(60), "clipboard").unwrap();

        // 49h after base: only `a` (base+48h) is due.
        let now = base + chrono::Duration::hours(49);
        let due = s.due(now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].memory_id, a);
        // `b` expires at base+72h; `c` at base+108h. Neither is due yet.
        let pending = s.pending_now(now).unwrap();
        assert_eq!(pending.len(), 2);
        let ids: Vec<_> = pending.iter().map(|i| i.memory_id).collect();
        assert!(ids.contains(&b));
        assert!(ids.contains(&c));
    }

    #[test]
    fn promote_and_discard_remove_rows() {
        let s = tmp_store();
        let id = Uuid::new_v4();
        let at = Utc::now();
        s.enqueue(id, at, "clipboard").unwrap();
        assert!(s.promote(id).unwrap());
        assert!(!s.promote(id).unwrap(), "second call is a no-op");

        let id2 = Uuid::new_v4();
        s.enqueue(id2, at, "clipboard").unwrap();
        assert!(s.discard(id2).unwrap());
    }

    #[test]
    fn promote_due_promotes_clean_and_discards_pii() {
        let s = tmp_store();
        let gate = GovernanceFilter::default();
        let base: DateTime<Utc> = "2026-07-22T10:00:00Z".parse().unwrap();

        let clean = Uuid::new_v4();
        let dirty = Uuid::new_v4();
        let missing = Uuid::new_v4();
        let not_due = Uuid::new_v4();

        s.enqueue(clean, base, "clipboard").unwrap();
        s.enqueue(dirty, base, "clipboard").unwrap();
        s.enqueue(missing, base, "clipboard").unwrap();
        // Enqueued far in the future — must not be touched.
        s.enqueue(not_due, base + chrono::Duration::hours(48), "clipboard")
            .unwrap();

        let now = base + chrono::Duration::hours(49);
        let contents = |id: Uuid| -> Option<String> {
            if id == clean {
                Some("the weather is nice".to_string())
            } else if id == dirty {
                // A phone number the first pass "missed".
                Some("call me at 415-555-0132".to_string())
            } else {
                None
            }
        };

        let outcomes = s.promote_due(now, &gate, &contents).unwrap();
        assert_eq!(outcomes.len(), 3);

        let by_id: std::collections::HashMap<_, _> = outcomes
            .iter()
            .map(|o| (o.memory_id, o.action.clone()))
            .collect();
        assert_eq!(by_id.get(&clean), Some(&PromotionAction::Promoted));
        assert_eq!(by_id.get(&dirty), Some(&PromotionAction::DiscardedPii));
        assert_eq!(by_id.get(&missing), Some(&PromotionAction::DiscardedMissing));

        // Not-due row must survive.
        let pending = s.pending_now(now).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].memory_id, not_due);
    }
}
