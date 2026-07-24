//! MCP-server benchmark harness (holistic review §5 P0.1).
//!
//! Measures the surface the product actually ships on — the JSON-RPC MCP
//! server — which every other benchmark in the workspace ignores in favour
//! of the library path.

pub mod client;
pub mod optimize;
pub mod scenario;
pub mod selector;

pub use client::{percentile_ms, McpServer};
pub use optimize::{optimize_descriptions, OptimizeResult};
pub use scenario::{score_selection, Scenario, ScenarioSet, SelectionReport};
pub use selector::{ToolDef, ToolSelector};
