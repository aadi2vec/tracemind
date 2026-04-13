//! Memory-R1 inspired CRUD operation types.
//!
//! At ingest time, instead of blindly adding every entity, we decide whether
//! to Add, Update, skip (Noop), or Delete based on similarity to existing
//! entities in the graph. This significantly improves memory quality by
//! preventing duplication and enabling entity merging.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The operation to perform on a memory entity during ingest.
///
/// Inspired by Memory-R1 (arXiv:2508.19828), which demonstrates that having
/// explicit ADD/UPDATE/DELETE/NOOP operations significantly improves memory
/// quality over naive append-only strategies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MemoryOp {
    /// Insert a brand-new entity (no similar entity exists).
    Add,

    /// Merge into an existing entity: reinforce confidence, average embeddings.
    Update { target_entity_id: Uuid },

    /// Skip — the entity is a near-duplicate; only lightly reinforce confidence.
    Noop { reason: String },

    /// Mark an existing entity for removal (reserved for future contradiction
    /// detection; not triggered during normal ingest).
    Delete {
        target_entity_id: Uuid,
        reason: String,
    },
}

impl std::fmt::Display for MemoryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryOp::Add => write!(f, "Add"),
            MemoryOp::Update { target_entity_id } => {
                write!(f, "Update(target={})", target_entity_id)
            }
            MemoryOp::Noop { reason } => write!(f, "Noop({})", reason),
            MemoryOp::Delete {
                target_entity_id,
                reason,
            } => write!(f, "Delete(target={}, reason={})", target_entity_id, reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_op_display() {
        let add = MemoryOp::Add;
        assert_eq!(format!("{}", add), "Add");

        let noop = MemoryOp::Noop {
            reason: "duplicate".into(),
        };
        assert!(format!("{}", noop).contains("duplicate"));
    }

    #[test]
    fn memory_op_serde_roundtrip() {
        let ops = vec![
            MemoryOp::Add,
            MemoryOp::Update {
                target_entity_id: Uuid::nil(),
            },
            MemoryOp::Noop {
                reason: "dup".into(),
            },
            MemoryOp::Delete {
                target_entity_id: Uuid::nil(),
                reason: "contradicted".into(),
            },
        ];

        for op in &ops {
            let json = serde_json::to_string(op).unwrap();
            let back: MemoryOp = serde_json::from_str(&json).unwrap();
            assert_eq!(*op, back);
        }
    }
}
