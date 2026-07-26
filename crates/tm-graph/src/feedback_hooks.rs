//! X9 — Card feedback hook lookup table.
//!
//! Every card the Brief home surfaces carries a `hook_id`. When the user
//! clicks an action, the MCP verb `memory_feedback` uses that hook_id to
//! recover the card's slot and backing memory refs so a
//! [`tm_types::FeedbackSignal`] can be written with the right shape.
//!
//! Kept alongside `feedback_fabric` because both tables serve the same
//! purpose (closing the retrieval reward loop) — one records the
//! *impression*, the other records the *response*.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// A hooked-card record — enough context to attribute a later action
/// back to a specific brief card impression.
#[derive(Debug, Clone)]
pub struct HookedCard {
    pub hook_id: Uuid,
    pub slot: String,
    /// UUIDs / hashes of the memories the card referred to. Passed
    /// through as JSON so the shape stays flexible without a second
    /// join table.
    pub refs: Vec<String>,
    /// Deterministic card id (see `tm_reflect::brief_home::BriefCard::id`).
    /// Optional so pre-existing hooks can be back-filled without breaking
    /// the write path.
    pub card_id: Option<Uuid>,
    pub host_id: Option<String>,
    pub session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS feedback_hooks (
            hook_id     TEXT NOT NULL PRIMARY KEY,
            slot        TEXT NOT NULL,
            refs_json   TEXT NOT NULL,
            card_id     TEXT,
            host_id     TEXT,
            session_id  TEXT,
            created_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_hooks_slot
            ON feedback_hooks(slot, created_at);
        CREATE INDEX IF NOT EXISTS idx_hooks_card
            ON feedback_hooks(card_id);",
    )
    .map_err(|e| TraceMindError::Storage(format!("feedback_hooks schema: {e}")))?;
    Ok(())
}

pub fn record_hook(conn: &Connection, h: &HookedCard) -> Result<()> {
    let refs_json = serde_json::to_string(&h.refs)
        .map_err(|e| TraceMindError::Storage(format!("hook refs serialize: {e}")))?;
    conn.execute(
        "INSERT OR REPLACE INTO feedback_hooks
            (hook_id, slot, refs_json, card_id, host_id, session_id, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            h.hook_id.to_string(),
            h.slot,
            refs_json,
            h.card_id.map(|u| u.to_string()),
            h.host_id,
            h.session_id.map(|u| u.to_string()),
            h.created_at.to_rfc3339(),
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("hook insert: {e}")))?;
    Ok(())
}

pub fn lookup_hook(conn: &Connection, hook_id: Uuid) -> Result<Option<HookedCard>> {
    let mut stmt = conn
        .prepare(
            "SELECT hook_id, slot, refs_json, card_id, host_id, session_id, created_at
             FROM feedback_hooks WHERE hook_id = ?1",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let mut rows = stmt
        .query(params![hook_id.to_string()])
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    if let Some(row) = rows.next().map_err(|e| TraceMindError::Storage(e.to_string()))? {
        let hook_s: String = row.get(0).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let slot: String = row.get(1).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let refs_s: String = row.get(2).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let card_s: Option<String> = row.get(3).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let host: Option<String> = row.get(4).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let sess_s: Option<String> = row.get(5).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let ts_s: String = row.get(6).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(Some(HookedCard {
            hook_id: Uuid::parse_str(&hook_s).unwrap_or(hook_id),
            slot,
            refs: serde_json::from_str(&refs_s).unwrap_or_default(),
            card_id: card_s.and_then(|s| Uuid::parse_str(&s).ok()),
            host_id: host,
            session_id: sess_s.and_then(|s| Uuid::parse_str(&s).ok()),
            created_at: ts_s
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now()),
        }))
    } else {
        Ok(None)
    }
}

pub fn recent_hooks(conn: &Connection, limit: usize) -> Result<Vec<HookedCard>> {
    let mut stmt = conn
        .prepare(
            "SELECT hook_id, slot, refs_json, card_id, host_id, session_id, created_at
             FROM feedback_hooks ORDER BY created_at DESC LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
        .filter_map(|r| r.ok())
        .map(|(hook_s, slot, refs_s, card_s, host, sess_s, ts_s)| HookedCard {
            hook_id: Uuid::parse_str(&hook_s).unwrap_or_else(|_| Uuid::nil()),
            slot,
            refs: serde_json::from_str(&refs_s).unwrap_or_default(),
            card_id: card_s.and_then(|s| Uuid::parse_str(&s).ok()),
            host_id: host,
            session_id: sess_s.and_then(|s| Uuid::parse_str(&s).ok()),
            created_at: ts_s
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now()),
        })
        .collect();
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init_schema(&c).unwrap();
        c
    }

    #[test]
    fn roundtrip_hook() {
        let c = db();
        let hook = HookedCard {
            hook_id: Uuid::new_v4(),
            slot: "recall".into(),
            refs: vec!["m1".into(), "m2".into()],
            card_id: Some(Uuid::new_v4()),
            host_id: Some("claude-code".into()),
            session_id: Some(Uuid::new_v4()),
            created_at: Utc::now(),
        };
        record_hook(&c, &hook).unwrap();
        let back = lookup_hook(&c, hook.hook_id).unwrap().unwrap();
        assert_eq!(back.slot, "recall");
        assert_eq!(back.refs, vec!["m1", "m2"]);
        assert_eq!(back.card_id, hook.card_id);
    }

    #[test]
    fn missing_hook_returns_none() {
        let c = db();
        assert!(lookup_hook(&c, Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn recent_orders_desc() {
        let c = db();
        for i in 0..3 {
            let h = HookedCard {
                hook_id: Uuid::new_v4(),
                slot: "rehearse".into(),
                refs: vec![format!("r{i}")],
                card_id: None,
                host_id: None,
                session_id: None,
                created_at: Utc::now() + chrono::Duration::seconds(i as i64),
            };
            record_hook(&c, &h).unwrap();
        }
        let rows = recent_hooks(&c, 10).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].refs, vec!["r2"]);
    }
}
