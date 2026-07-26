//! `PropagateDelete` for the reflection store.
//!
//! Reflections summarise *sessions*, not individual memories, so a memory
//! id does not map cleanly to a reflection row. The correct behaviour for
//! this sprint is a truthful no-op: we neither delete a reflection nor
//! pretend one existed. Downstream sprints (I-P2) may add finer per-memory
//! attribution to reflections; when that happens this impl becomes the
//! place to prune the derived rows.
//!
//! Returning a report (rather than skipping the trait) is important — the
//! `Propagator` coordinator surfaces every crate's answer so auditors can
//! see that "the reflection layer was asked and had nothing to remove".

use tm_types::{PropagateDelete, PropagateReport, Result};
use uuid::Uuid;

use crate::reflexion::ReflexionStore;

impl PropagateDelete for ReflexionStore {
    fn propagate_delete(&self, _memory_id: Uuid) -> Result<PropagateReport> {
        Ok(PropagateReport::new("tm-reflect", 0)
            .with_note("session-scoped reflections; no per-memory rows to drop"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_zero_with_explanatory_note() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReflexionStore::open(dir.path().join("reflexions.jsonl")).unwrap();
        let report = store.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.crate_name, "tm-reflect");
        assert_eq!(report.rows_removed, 0);
        assert!(report.note.is_some());
    }
}
