//! `tm-intent` — the system-of-intents primitive.
//!
//! This crate owns the **Commitment / Outcome / Anticipation** trio and
//! the state machine that connects them. It is the wedge primitive per
//! `docs/INTENT_SYSTEM.md`: a forward-leaning *intent* and a
//! backward-resolving *decision* are two phases of the same
//! [`Commitment`], and the system observes them, attaches outcomes,
//! and (later) learns patterns.
//!
//! Phase boundaries:
//!
//! - `types`  — pure data model + serde (no I/O, no DB)
//! - `state`  — state-machine transition rules (`Open → Acted →
//!              Completed`, plus `Abandoned` / `Superseded`)
//! - `store`  — SQLite persistence; the only module that touches I/O
//!
//! Out of scope for this crate (lives elsewhere):
//!
//! - the world model that produces [`Anticipation`] → `tm-reflect`
//! - retrieval-side L1 prefetch → `tm-retrieval`
//!
//! Phrase mining (`miner`) lives in this crate (not in `tm-capture`)
//! so the algorithm sits next to the data model it produces. The
//! `tm-capture` daemon re-exports / calls this directly; the spec's
//! `tm-capture::CommitmentMiner` reference is a structural label, not
//! a hard module path. See `miner` docs for `INTENT_SYSTEM.md` §3.1
//! provenance.

pub mod miner;
pub mod state;
pub mod store;
pub mod types;

pub use miner::{mine, MinedCandidate};
pub use state::{transition, StateError};
pub use store::{
    CandidateRecord, InsightSilence, IntentStore, OutcomeProposal, PatternSilence,
};
pub use types::{
    Action, ActionModality, ActionSource, AmbientState, Anticipation, AnticipationKind, Belief,
    Commitment, CommitmentDraft, CommitmentKind, ContextSnapshot, IntentArc, Need, NeedSource,
    Outcome, OutcomeSource, Polarity, Sentiment, SentimentSource, SentimentTarget, Source, Stakes,
    State, TriggerContext, TurnRef, UserResponse,
};
