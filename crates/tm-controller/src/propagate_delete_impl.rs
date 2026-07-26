//! `PropagateDelete` for the controller layer.
//!
//! Bandit state (`UcbBandit`, `LinUcbBandit`) is aggregated across all
//! queries — it does not carry per-memory identifiers. Deleting a memory
//! therefore leaves nothing in the bandit tables to remove. We implement
//! the trait anyway so the coordinator gets a truthful "0 rows, nothing
//! keyed on memory id here" report from every layer.

use tm_types::{PropagateDelete, PropagateReport, Result};
use uuid::Uuid;

use crate::bandit::{LinUcbBandit, UcbBandit};

impl PropagateDelete for UcbBandit {
    fn propagate_delete(&self, _memory_id: Uuid) -> Result<PropagateReport> {
        Ok(PropagateReport::new("tm-controller:ucb", 0)
            .with_note("aggregate arm counts; no per-memory rows"))
    }
}

impl PropagateDelete for LinUcbBandit {
    fn propagate_delete(&self, _memory_id: Uuid) -> Result<PropagateReport> {
        Ok(PropagateReport::new("tm-controller:linucb", 0)
            .with_note("per-arm weight matrices; no per-memory rows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ucb_reports_zero() {
        let b = UcbBandit::new();
        let report = b.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.crate_name, "tm-controller:ucb");
        assert_eq!(report.rows_removed, 0);
        assert!(report.note.is_some());
    }

    #[test]
    fn linucb_reports_zero() {
        let b = LinUcbBandit::new();
        let report = b.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.crate_name, "tm-controller:linucb");
        assert_eq!(report.rows_removed, 0);
    }
}
