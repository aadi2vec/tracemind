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
pub mod ontology_proposer;
pub mod outcome_prompt;
pub mod pattern;
pub mod reflexion;
pub mod working_memory;

pub use brief::{
    BriefBuilder, BriefConfig, BriefError, CandidateBriefRow, CommitmentBriefRow,
    CommitmentOutlook, DailyBrief, InsightGateConfig, ModelQuietReason, OverdueClass,
    OutcomeProposalBriefRow, PatternBriefRow, ResolvedBriefRow,
};
pub use insights::{detect_insights, BaselineRate, InsightBriefRow, InsightConfig};
pub use matcher::{propose_outcomes, MatcherConfig, OutcomeProposal};
pub use ontology_proposer::{
    persist_proposals as persist_object_type_proposals, propose_object_types,
    ProposedObjectType, ProposerConfig,
};
pub use outcome_prompt::{
    OutcomePrompt, OutcomePromptScheduler, PromptSchedulerConfig, PromptUrgency,
};
pub use pattern::{
    default_window_start, detect_patterns, wilson_lower_bound, CellKey, DetectedPattern,
    PatternConfig, PolarityDist, TimeBand,
};
pub use working_memory::{
    ensure_schema as ensure_wme_schema, proposals_from_sources, ActivityContext, BridgeInput,
    CandidateSources, Card, CardKind, CardProposal, CardSignals, CommitmentInput,
    ContradictionInput, FeedbackKind, OutlierInput, SimilarMemoryInput, WmeConfig, WmeError,
    WmeResult, WorkingMemoryEngine, DEFAULT_SCORE_FLOOR, MAX_CARDS_PER_HOUR,
    PER_TARGET_COOLDOWN_SECS,
};
pub use reflexion::{
    GepaSpikeConfig, GepaSpikeResult, ReflexionStore, SessionReflection,
    run_gepa_spike,
};
