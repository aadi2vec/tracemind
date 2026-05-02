//! `tm-reflect` — the surfacing layer of the system of intents.
//!
//! Per `docs/INTENT_SYSTEM.md` §9.1, the **daily brief** is the
//! primary user-visible output of the wedge. This crate owns the
//! pure, deterministic data shape behind that brief and a tiny
//! [`BriefBuilder`] that pulls from [`tm_intent::IntentStore`] and
//! returns a structured [`DailyBrief`].
//!
//! Rendering (CLI text, MCP JSON, future Tauri view) lives in the
//! caller — this crate stays presentation-agnostic so all surfaces
//! agree on what's *in* a brief even when they disagree on how to
//! show it.
//!
//! Phase boundaries:
//!
//! - `brief`   — `DailyBrief` data shape + `BriefBuilder`
//!
//! Out of scope (yet):
//!
//! - pattern detector (`PatternMatch` anticipations) — Sprint C/D
//! - outcome-prompt scheduler   — depends on Anticipation table
//! - world-model rendering / narrate hand-off
//!
//! The brief is intentionally *read-only* against the intent store:
//! it never writes back. The user's "accept / dismiss / silence"
//! actions go through the existing `IntentStore` mutators.

pub mod brief;
pub mod insights;
pub mod matcher;
pub mod pattern;

pub use brief::{
    BriefBuilder, BriefConfig, BriefError, CandidateBriefRow, CommitmentBriefRow,
    CommitmentOutlook, DailyBrief, OverdueClass, PatternBriefRow, ResolvedBriefRow,
};
pub use insights::{detect_insights, BaselineRate, InsightBriefRow, InsightConfig};
pub use matcher::{propose_outcomes, MatcherConfig, OutcomeProposal};
pub use pattern::{
    default_window_start, detect_patterns, wilson_lower_bound, CellKey, DetectedPattern,
    PatternConfig, PolarityDist, TimeBand,
};
