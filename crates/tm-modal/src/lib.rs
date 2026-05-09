//! Cross-modal reasoning foundation for TraceMind.
//!
//! This crate provides the type system, encoder traits, and edge detectors
//! needed to reason across modalities (text, image, audio, code, structured data).

pub mod types;
pub mod encoder;
pub mod cooccurrence;

pub use types::*;
pub use encoder::{ModalEncoder, EncoderError, TextEncoder, EncoderRegistry};
pub use cooccurrence::CoOccurrenceDetector;
