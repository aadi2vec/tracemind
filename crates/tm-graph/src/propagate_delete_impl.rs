//! `PropagateDelete` for [`GraphStore`].
//!
//! Removes every graph-side row that references `memory_id`:
//!
//! * relations where the memory is subject or object (`kg_relations`)
//! * the entity itself (`kg_entities`)
//! * its embedding row (`kg_vectors`)
//! * its access log entries (`access_log`)
//! * its retrieval feedback tally (`retrieval_feedback`)
//! * any capture rows attributable to it (`captured_signals.promoted_entity`)
//!
//! Idempotent — repeat calls report `rows_removed: 0`.
//!
//! The implementation lives in its own file so `store.rs` (4900+ LOC) is
//! not touched by this sprint. It delegates to the same `KnowledgeGraph`
//! connection the store already owns.

use rusqlite::params;
use tm_types::{PropagateDelete, PropagateReport, Result, TraceMindError};
use uuid::Uuid;

use crate::store::GraphStore;

impl PropagateDelete for GraphStore {
    fn propagate_delete(&self, memory_id: Uuid) -> Result<PropagateReport> {
        let conn = self.raw_connection();

        // The uuid-keyed rows (access_log, retrieval_feedback, captured_signals)
        // can be dropped by the uuid string directly — no id-map lookup needed.
        let mut removed = 0usize;

        removed += conn
            .execute(
                "DELETE FROM access_log WHERE entity_id = ?1",
                params![memory_id.to_string()],
            )
            .map_err(|e| TraceMindError::Storage(format!("access_log: {e}")))?;

        removed += conn
            .execute(
                "DELETE FROM retrieval_feedback WHERE entity_id = ?1",
                params![memory_id.to_string()],
            )
            .map_err(|e| TraceMindError::Storage(format!("retrieval_feedback: {e}")))?;

        removed += conn
            .execute(
                "DELETE FROM captured_signals WHERE promoted_entity = ?1",
                params![memory_id.to_string()],
            )
            .map_err(|e| TraceMindError::Storage(format!("captured_signals: {e}")))?;

        // The kg_* tables are keyed by the skg i64 id. Look it up via the
        // uuid map; if the entity was never registered (e.g. dropped by a
        // previous propagate), just skip.
        if let Some(skg_id) = self.skg_id_for(memory_id) {
            removed += conn
                .execute(
                    "DELETE FROM kg_relations WHERE source_id = ?1 OR target_id = ?1",
                    params![skg_id],
                )
                .map_err(|e| TraceMindError::Storage(format!("kg_relations: {e}")))?;

            removed += conn
                .execute(
                    "DELETE FROM kg_vectors WHERE entity_id = ?1",
                    params![skg_id],
                )
                .map_err(|e| TraceMindError::Storage(format!("kg_vectors: {e}")))?;

            removed += conn
                .execute("DELETE FROM kg_entities WHERE id = ?1", params![skg_id])
                .map_err(|e| TraceMindError::Storage(format!("kg_entities: {e}")))?;

            self.forget_entity_id_mapping(memory_id);
        }

        Ok(PropagateReport::new("tm-graph", removed))
    }
}
