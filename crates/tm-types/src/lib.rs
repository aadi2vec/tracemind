//! TraceMind core types — shared across all crates.
//!
//! This crate contains every domain type: entities, triples, traces,
//! trajectories, procedures. Nothing in this crate has I/O.

pub mod entity;
pub mod triple;
pub mod trace;
pub mod trajectory;
pub mod procedure;
pub mod error;
pub mod memory_op;

pub use entity::*;
pub use triple::*;
pub use trace::*;
pub use trajectory::*;
pub use procedure::*;
pub use error::*;
pub use memory_op::*;
