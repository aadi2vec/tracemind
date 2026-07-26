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
pub mod time_range;
pub mod bundled;
pub mod recent;
pub mod working_memory;
pub mod capture_mode;
pub mod capture_permissions;
pub mod capture_receipt;
pub mod propagate_delete;
pub mod tier;
pub mod feedback;

pub use entity::*;
pub use triple::*;
pub use trace::*;
pub use trajectory::*;
pub use procedure::*;
pub use error::*;
pub use memory_op::*;
pub use time_range::*;
pub use recent::*;
pub use working_memory::*;
pub use capture_mode::{CaptureMode, ModeSession};
pub use capture_permissions::*;
pub use capture_receipt::{CaptureReceipt, GateDecisions};
pub use propagate_delete::{PropagateDelete, PropagateReport};
pub use tier::*;
pub use feedback::{FeedbackClass, FeedbackKind, FeedbackSignal};
