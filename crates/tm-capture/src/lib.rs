//! Public library surface of the capture crate — used by `tm-cli`'s
//! `capture doctor` subcommand to probe browser-history reachability
//! without spawning the daemon binary.
//!
//! The daemon lives in `main.rs`; this file just re-exports the modules
//! that other crates need as helpers.

pub mod browser_history;
pub mod modalities;
pub mod vision_ocr;
