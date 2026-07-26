//! Reversibility with propagation — the S3 commandment.
//!
//! When the user forgets a memory, every crate that stored anything *derived*
//! from that memory must drop its rows too. Row-level deletion is not enough:
//! embeddings, triples, salience rows, feedback fabric entries all trail
//! behind and must vanish together.
//!
//! Each downstream crate implements [`PropagateDelete`] on its store type
//! (`GraphStore`, `VectorStore`, `RecentStore`, `ReflexionStore`, bandit state).
//! `tm-ingest::propagate::Propagator` calls every implementation in order and
//! collects the [`PropagateReport`]s so callers can log, surface, or assert on
//! them.
//!
//! Immutable append-only logs (e.g. `traces.jsonl`) return `rows_removed: 0`
//! with an explanatory note — the audit trail is preserved on purpose.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;

/// One crate's answer to "how many rows related to this memory did you drop?"
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PropagateReport {
    /// Identifier of the reporting crate — used for logs / metrics
    /// (`"tm-graph"`, `"tm-vector"`, `"tm-episodic:recent"`, etc.).
    /// Owned `String` so the report round-trips through JSON cleanly.
    pub crate_name: String,
    pub rows_removed: usize,
    /// Optional context, e.g. "immutable audit log preserved".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl PropagateReport {
    pub fn new(crate_name: impl Into<String>, rows_removed: usize) -> Self {
        Self {
            crate_name: crate_name.into(),
            rows_removed,
            note: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Implemented by every store that holds data derived from a memory.
///
/// The `memory_id` is a Uuid — the same identifier the graph uses for the
/// entity representing the captured memory. Implementations should be
/// idempotent: calling `propagate_delete(id)` twice is safe and returns
/// `rows_removed: 0` the second time.
pub trait PropagateDelete {
    fn propagate_delete(&self, memory_id: Uuid) -> Result<PropagateReport>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_round_trip() {
        let r = PropagateReport::new("tm-graph", 3);
        let s = serde_json::to_string(&r).unwrap();
        let d: PropagateReport = serde_json::from_str(&s).unwrap();
        assert_eq!(r, d);
        assert!(!s.contains("note"), "note omitted when None");
    }

    #[test]
    fn report_with_note_serialises() {
        let r = PropagateReport::new("tm-episodic:trace", 0)
            .with_note("immutable audit log preserved");
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["crate_name"], "tm-episodic:trace");
        assert_eq!(v["rows_removed"], 0);
        assert_eq!(v["note"], "immutable audit log preserved");
    }
}
