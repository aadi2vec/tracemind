//! Commitment state-machine transition rules.
//!
//! Canonical against `docs/INTENT_SYSTEM.md` §2:
//!
//! ```text
//!     Open ─── act ──▶ Acted ── outcome ─▶ Completed
//!     │
//!     ├─── abandon ─▶ Abandoned
//!     └─── supersede ─▶ Superseded
//! ```
//!
//! - `Open → Acted` either auto (action detected) or manual.
//! - `Acted → Completed` requires an [`Outcome`] record.
//! - `Open → Superseded` when a later commitment lists this one in
//!   `derived_from`.
//! - `Open → Abandoned` is user-initiated, or auto after 4× horizon
//!   with no signal of action (the auto rule lives in `tm-reflect`,
//!   not here — this module only validates transitions, not who
//!   triggers them).
//!
//! Terminal states ([`State::Completed`], [`State::Abandoned`],
//! [`State::Superseded`]) are sticky — no further transitions allowed.

use thiserror::Error;
use uuid::Uuid;

use crate::types::{Commitment, Outcome, State};

/// Reasons a transition was rejected. Surfaced verbatim in CLI / MCP
/// error responses so callers can diagnose without digging.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StateError {
    #[error("invalid transition: {from:?} → {to:?}")]
    InvalidTransition { from: State, to: State },
    #[error("cannot transition out of terminal state {0:?}")]
    Terminal(State),
    #[error("Acted → Completed requires an Outcome record (none attached)")]
    MissingOutcome,
    #[error("Outcome belongs to commitment {expected}, not {actual}")]
    OutcomeMismatch { expected: Uuid, actual: Uuid },
}

/// Whether `s` is a terminal (sticky) state.
pub fn is_terminal(s: State) -> bool {
    matches!(s, State::Completed | State::Abandoned | State::Superseded)
}

/// Validate and apply a transition to `commitment.state`. Returns the
/// new state on success. Mutates `commitment.state` and (for
/// `Completed`) `commitment.outcome_id` in-place.
///
/// `outcome` is required iff `to == State::Completed`.
pub fn transition(
    commitment: &mut Commitment,
    to: State,
    outcome: Option<&Outcome>,
) -> Result<State, StateError> {
    let from = commitment.state;

    if is_terminal(from) {
        return Err(StateError::Terminal(from));
    }

    let allowed = matches!(
        (from, to),
        (State::Open, State::Acted)
            | (State::Open, State::Abandoned)
            | (State::Open, State::Superseded)
            | (State::Open, State::Completed)   // permitted: skip Acted when outcome arrives directly
            | (State::Acted, State::Completed)
            | (State::Acted, State::Abandoned)
    );

    if !allowed {
        return Err(StateError::InvalidTransition { from, to });
    }

    if to == State::Completed {
        let o = outcome.ok_or(StateError::MissingOutcome)?;
        if o.commitment_id != commitment.id {
            return Err(StateError::OutcomeMismatch {
                expected: commitment.id,
                actual: o.commitment_id,
            });
        }
        commitment.outcome_id = Some(o.id);
    }

    commitment.state = to;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CommitmentKind, OutcomeSource, Polarity, Source};

    fn fresh() -> Commitment {
        Commitment::new(CommitmentKind::Intent, "ship v2 Friday", Source::Manual)
    }

    #[test]
    fn open_to_acted_is_allowed() {
        let mut c = fresh();
        let r = transition(&mut c, State::Acted, None).unwrap();
        assert_eq!(r, State::Acted);
        assert_eq!(c.state, State::Acted);
        assert!(c.outcome_id.is_none());
    }

    #[test]
    fn acted_to_completed_requires_outcome() {
        let mut c = fresh();
        transition(&mut c, State::Acted, None).unwrap();
        let err = transition(&mut c, State::Completed, None).unwrap_err();
        assert_eq!(err, StateError::MissingOutcome);
        assert_eq!(c.state, State::Acted);
    }

    #[test]
    fn acted_to_completed_with_matching_outcome_attaches_id() {
        let mut c = fresh();
        transition(&mut c, State::Acted, None).unwrap();
        let o = Outcome::new(c.id, Polarity::Better, "shipped early", OutcomeSource::UserPrompted);
        let r = transition(&mut c, State::Completed, Some(&o)).unwrap();
        assert_eq!(r, State::Completed);
        assert_eq!(c.outcome_id, Some(o.id));
    }

    #[test]
    fn outcome_mismatch_is_rejected() {
        let mut c = fresh();
        transition(&mut c, State::Acted, None).unwrap();
        // Outcome bound to a different commitment.
        let stranger = Outcome::new(Uuid::new_v4(), Polarity::Worse, "x", OutcomeSource::UserPrompted);
        let err = transition(&mut c, State::Completed, Some(&stranger)).unwrap_err();
        assert!(matches!(err, StateError::OutcomeMismatch { .. }));
        assert_eq!(c.state, State::Acted);
    }

    #[test]
    fn open_to_completed_is_allowed_when_outcome_lands_directly() {
        // Some flows skip Acted entirely (e.g. the user only realises
        // in retrospect): `Open → Completed` with an outcome must work.
        let mut c = fresh();
        let o = Outcome::new(c.id, Polarity::AsExpected, "fine", OutcomeSource::UserPrompted);
        transition(&mut c, State::Completed, Some(&o)).unwrap();
        assert_eq!(c.state, State::Completed);
    }

    #[test]
    fn terminal_states_reject_further_transitions() {
        let mut c = fresh();
        let o = Outcome::new(c.id, Polarity::Worse, "x", OutcomeSource::UserPrompted);
        transition(&mut c, State::Completed, Some(&o)).unwrap();
        let err = transition(&mut c, State::Abandoned, None).unwrap_err();
        assert_eq!(err, StateError::Terminal(State::Completed));
    }

    #[test]
    fn supersede_skips_outcome_requirement() {
        // When a later commitment supersedes this one we don't need
        // an Outcome — the new commitment carries the value.
        let mut c = fresh();
        transition(&mut c, State::Superseded, None).unwrap();
        assert_eq!(c.state, State::Superseded);
        assert!(c.outcome_id.is_none());
    }

    #[test]
    fn abandon_from_acted_is_allowed() {
        let mut c = fresh();
        transition(&mut c, State::Acted, None).unwrap();
        transition(&mut c, State::Abandoned, None).unwrap();
        assert_eq!(c.state, State::Abandoned);
    }

    #[test]
    fn weird_transitions_are_rejected() {
        let mut c = fresh();
        // Open → directly to Acted is fine; check the inverse rejection.
        let mut c2 = fresh();
        transition(&mut c2, State::Acted, None).unwrap();
        let err = transition(&mut c2, State::Open, None).unwrap_err();
        assert!(matches!(err, StateError::InvalidTransition { .. }));
        assert_eq!(c2.state, State::Acted);

        // Open → Open is also rejected.
        let err = transition(&mut c, State::Open, None).unwrap_err();
        assert!(matches!(err, StateError::InvalidTransition { .. }));
    }
}
