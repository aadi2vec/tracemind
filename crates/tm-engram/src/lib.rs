//! `tm-engram` -- Belief-native API for AI agents.
//!
//! The Engram SDK is the primary entry point for AI agents that want to
//! interact with TraceMind's memory system through beliefs rather than
//! raw ingest/query cycles. It unifies the TMS (truth maintenance),
//! intent store (needs, actions, commitments), and temporal store
//! (bitemporal fact tracking) behind a single `Engram` handle.
//!
//! ```ignore
//! let engine = Engram::open_in_memory()?;
//! let r = engine.assert_belief("Rust is fast", 0.95)?;
//! assert_eq!(r.status, BeliefStatus::In);
//! ```

pub mod engine;
pub mod types;

pub use engine::Engram;
pub use types::{
    AssertResult, BeliefKind, BeliefView, EngineConfig, Goal, GoalStatus, RetractResult,
    WorldSnapshot,
};
