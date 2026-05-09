//! `tm-tms` — Justification-based Truth Maintenance System (JTMS).
//!
//! Tracks beliefs, their justifications (support / defeat), and
//! propagates status changes through the dependency graph. When a
//! premise is retracted every conclusion that depended *solely* on
//! that premise transitions to `Out`.
//!
//! The engine is purely in-memory — no database, no I/O.  Persistence
//! (if needed) is the caller's responsibility via serde round-trips.
//!
//! Core concepts:
//!
//! - **Belief** — a statement with a status (`In`, `Out`, `Contradicted`)
//!   and a confidence score.
//! - **Justification** — a directed edge: a set of premise beliefs that
//!   either *support* or *defeat* a conclusion belief.
//! - **Contradiction** — two beliefs whose cosine similarity is below
//!   −0.8, indicating semantic opposition.
//! - **Propagation** — BFS from changed beliefs; a conclusion is `In`
//!   iff at least one supporting justification has all premises `In`
//!   and no defeating justification has all premises `In`.

pub mod engine;
pub mod types;

pub use engine::TmsEngine;
pub use types::{
    Belief, BeliefStatus, Contradiction, ContradictionResolution, Justification,
    JustificationType, PropagationResult,
};
