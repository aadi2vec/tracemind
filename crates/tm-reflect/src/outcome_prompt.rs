//! Outcome-prompt scheduler — Sprint B skeleton per
//! `docs/INTENT_SYSTEM.md` §4.1.
//!
//! When a [`Commitment`]'s `horizon` passes, the daily brief is
//! supposed to ask "what happened with X?". This module is the pure-
//! logic side of that question: given a store snapshot and a wall
//! clock, return the list of [`OutcomePrompt`]s the surface should
//! render.
//!
//! Scope (Sprint B):
//!
//! - select overdue Open / Acted commitments,
//! - skip ones already covered by an active
//!   [`tm_intent::OutcomeProposal`] (the implicit matcher has already
//!   guessed the polarity — no need to also ask blind),
//! - classify by lag past horizon ([`PromptUrgency`]),
//! - cap by `max_per_run`, soonest-horizon first.
//!
//! Out of scope here:
//!
//! - tracking dismissals across briefs (the surface owns that),
//! - turning the prompt into an `Anticipation` row,
//! - voice/UI flows.
//!
//! These belong to Sprint C+ and the Tauri layer respectively.
//!
//! ## Why pure logic
//!
//! Same shape as `BriefBuilder`: deterministic against the store
//! state + a `now` parameter, no I/O beyond the store reads, no
//! side effects. Lets the same scheduler back the CLI brief, the MCP
//! `daily_brief` tool, and the Tauri timeline without each surface
//! re-implementing the rules.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use tm_intent::{store::StoreError, Commitment, IntentStore};

/// How urgent the prompt is, by lag past `horizon`. Mirrors the
/// brief's `OverdueClass` boundaries (24h / 7d / older) but is a
/// separate type so the two surfaces can evolve independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptUrgency {
    /// Lag ≤ 24h — fresh, ask once, gently.
    Due,
    /// 24h < lag ≤ 7d — user has had time to forget.
    Stale,
    /// lag > 7d — cold; the brief should also offer "abandon?".
    Cold,
}

impl PromptUrgency {
    /// Classify by lag past `horizon`. `now < horizon` is treated as
    /// `Due` for safety, but callers don't pass non-overdue rows
    /// here in practice.
    pub fn classify(horizon: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let lag = now - horizon;
        if lag <= Duration::hours(24) {
            Self::Due
        } else if lag <= Duration::days(7) {
            Self::Stale
        } else {
            Self::Cold
        }
    }
}

/// One render-ready outcome prompt. Carries the statement so the
/// surface can show the row without a second store hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomePrompt {
    pub commitment_id: Uuid,
    pub statement: String,
    pub horizon: DateTime<Utc>,
    pub urgency: PromptUrgency,
    /// `now - horizon` in whole days. Negative is impossible by
    /// construction; floor at 0.
    pub age_days: i64,
}

/// Scheduler tuning knobs.
#[derive(Debug, Clone, Copy)]
pub struct PromptSchedulerConfig {
    /// Max prompts surfaced per call. Default 5 — the brief is meant
    /// to feel manageable.
    pub max_per_run: usize,
    /// Don't surface anything older than this. Avoids surfacing a
    /// year-old fizzled commitment every brief forever.
    pub overdue_window: Duration,
}

impl Default for PromptSchedulerConfig {
    fn default() -> Self {
        Self {
            max_per_run: 5,
            overdue_window: Duration::days(180),
        }
    }
}

/// Pure-logic scheduler over an [`IntentStore`].
pub struct OutcomePromptScheduler<'a> {
    store: &'a IntentStore,
}

impl<'a> OutcomePromptScheduler<'a> {
    pub fn new(store: &'a IntentStore) -> Self {
        Self { store }
    }

    /// Compute the prompts to surface at `now`.
    ///
    /// Algorithm:
    ///
    /// 1. Pull overdue Open / Acted commitments (more than we'll
    ///    keep, so the post-filter has room).
    /// 2. Drop anything older than `cfg.overdue_window`.
    /// 3. Drop anything already covered by an active
    ///    [`tm_intent::OutcomeProposal`].
    /// 4. Sort soonest-horizon-first.
    /// 5. Truncate to `max_per_run`.
    pub fn due_prompts(
        &self,
        now: DateTime<Utc>,
        cfg: PromptSchedulerConfig,
    ) -> Result<Vec<OutcomePrompt>, StoreError> {
        // Pull a generous prefix so the post-filter has slack — we
        // strip rows by window + proposal-coverage below and don't
        // want to come up short.
        let pull_limit = cfg.max_per_run.saturating_mul(4).max(20);
        let raw = self.store.list_overdue_open(now, pull_limit)?;

        // Existing proposals already cover the "what happened" question
        // for their commitment. Avoid double-asking.
        let proposals = self.store.list_active_outcome_proposals(now, 256)?;
        let covered: HashSet<Uuid> = proposals.iter().map(|p| p.commitment_id).collect();

        let cutoff = now - cfg.overdue_window;
        let mut out: Vec<OutcomePrompt> = raw
            .into_iter()
            .filter_map(|c| {
                let h = c.horizon?;
                if h < cutoff {
                    return None;
                }
                if covered.contains(&c.id) {
                    return None;
                }
                Some(build_prompt(&c, now, h))
            })
            .collect();

        // Re-sort defensively — `list_overdue_open` already sorts by
        // horizon ASC, but a future store change shouldn't silently
        // change scheduler ordering.
        out.sort_by(|a, b| a.horizon.cmp(&b.horizon));
        out.truncate(cfg.max_per_run);
        Ok(out)
    }
}

fn build_prompt(c: &Commitment, now: DateTime<Utc>, horizon: DateTime<Utc>) -> OutcomePrompt {
    let age_days = (now - horizon).num_days().max(0);
    OutcomePrompt {
        commitment_id: c.id,
        statement: c.statement.clone(),
        horizon,
        urgency: PromptUrgency::classify(horizon, now),
        age_days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_intent::types::{CommitmentKind, Polarity, Source};

    fn fresh_store() -> IntentStore {
        IntentStore::open_in_memory().expect("open in-memory")
    }

    fn mk_commitment(statement: &str, horizon: Option<DateTime<Utc>>) -> Commitment {
        let mut c = Commitment::new(CommitmentKind::Intent, statement, Source::Manual);
        c.horizon = horizon;
        c
    }

    fn mk_proposal(
        commitment_id: Uuid,
        polarity: Polarity,
        proposed_at: DateTime<Utc>,
    ) -> tm_intent::OutcomeProposal {
        tm_intent::OutcomeProposal {
            id: Uuid::new_v4(),
            commitment_id,
            cell_key: tm_intent::OutcomeProposal::cell_key_for(commitment_id, polarity),
            proposed_polarity: polarity,
            description: "matched capture".into(),
            similarity: 0.85,
            source_trace_id: None,
            proposed_at,
            expires_at: proposed_at + Duration::days(14),
            status: "pending".into(),
            resolved_at: None,
            resolved_outcome_id: None,
        }
    }

    #[test]
    fn classify_buckets_by_lag() {
        let h = Utc::now() - Duration::days(10);
        let now = Utc::now();
        assert_eq!(PromptUrgency::classify(now, now), PromptUrgency::Due); // lag=0
        assert_eq!(
            PromptUrgency::classify(now - Duration::hours(12), now),
            PromptUrgency::Due
        );
        assert_eq!(
            PromptUrgency::classify(now - Duration::days(2), now),
            PromptUrgency::Stale
        );
        assert_eq!(PromptUrgency::classify(h, now), PromptUrgency::Cold);
    }

    #[test]
    fn surfaces_overdue_open_commitment() {
        let store = fresh_store();
        let now = Utc::now();
        let c = mk_commitment("ship v2", Some(now - Duration::hours(3)));
        store.insert_commitment(&c).unwrap();

        let sched = OutcomePromptScheduler::new(&store);
        let prompts = sched.due_prompts(now, PromptSchedulerConfig::default()).unwrap();

        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].commitment_id, c.id);
        assert_eq!(prompts[0].statement, "ship v2");
        assert_eq!(prompts[0].urgency, PromptUrgency::Due);
        assert_eq!(prompts[0].age_days, 0);
    }

    #[test]
    fn skips_commitments_with_active_proposal() {
        let store = fresh_store();
        let now = Utc::now();
        let covered = mk_commitment("covered", Some(now - Duration::days(2)));
        let bare = mk_commitment("bare", Some(now - Duration::days(2)));
        store.insert_commitment(&covered).unwrap();
        store.insert_commitment(&bare).unwrap();

        let p = mk_proposal(covered.id, Polarity::Better, now - Duration::hours(6));
        store.insert_outcome_proposal(&p).unwrap();

        let prompts = OutcomePromptScheduler::new(&store)
            .due_prompts(now, PromptSchedulerConfig::default())
            .unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].commitment_id, bare.id);
    }

    #[test]
    fn skips_not_overdue() {
        let store = fresh_store();
        let now = Utc::now();
        let future = mk_commitment("not yet", Some(now + Duration::days(2)));
        store.insert_commitment(&future).unwrap();
        let prompts = OutcomePromptScheduler::new(&store)
            .due_prompts(now, PromptSchedulerConfig::default())
            .unwrap();
        assert!(prompts.is_empty());
    }

    #[test]
    fn skips_commitments_past_window() {
        let store = fresh_store();
        let now = Utc::now();
        let very_old = mk_commitment("ancient", Some(now - Duration::days(400)));
        let still_in_window = mk_commitment("recent", Some(now - Duration::days(30)));
        store.insert_commitment(&very_old).unwrap();
        store.insert_commitment(&still_in_window).unwrap();
        let prompts = OutcomePromptScheduler::new(&store)
            .due_prompts(now, PromptSchedulerConfig::default())
            .unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].commitment_id, still_in_window.id);
    }

    #[test]
    fn caps_at_max_per_run_soonest_first() {
        let store = fresh_store();
        let now = Utc::now();
        for i in 1..=5_i64 {
            // horizons -1d, -2d, -3d, -4d, -5d
            let c = mk_commitment(&format!("c{i}"), Some(now - Duration::days(i)));
            store.insert_commitment(&c).unwrap();
        }

        let cfg = PromptSchedulerConfig {
            max_per_run: 3,
            ..Default::default()
        };
        let prompts = OutcomePromptScheduler::new(&store).due_prompts(now, cfg).unwrap();
        assert_eq!(prompts.len(), 3);
        // Soonest horizon first means *most* overdue first — c5 (-5d), c4, c3.
        assert_eq!(prompts[0].statement, "c5");
        assert_eq!(prompts[1].statement, "c4");
        assert_eq!(prompts[2].statement, "c3");
    }

    #[test]
    fn skips_commitment_without_horizon() {
        let store = fresh_store();
        let now = Utc::now();
        let no_h = mk_commitment("open-ended", None);
        store.insert_commitment(&no_h).unwrap();
        let prompts = OutcomePromptScheduler::new(&store)
            .due_prompts(now, PromptSchedulerConfig::default())
            .unwrap();
        assert!(prompts.is_empty());
    }

    #[test]
    fn age_days_is_floored_at_zero_and_increases_with_lag() {
        let store = fresh_store();
        let now = Utc::now();
        let just = mk_commitment("just", Some(now - Duration::minutes(5)));
        let week = mk_commitment("week", Some(now - Duration::days(8)));
        store.insert_commitment(&just).unwrap();
        store.insert_commitment(&week).unwrap();

        let prompts = OutcomePromptScheduler::new(&store)
            .due_prompts(now, PromptSchedulerConfig::default())
            .unwrap();
        // sorted soonest-horizon-first → "week" (older horizon) first
        assert_eq!(prompts[0].statement, "week");
        assert!(prompts[0].age_days >= 7);
        assert_eq!(prompts[0].urgency, PromptUrgency::Cold);

        assert_eq!(prompts[1].statement, "just");
        assert_eq!(prompts[1].age_days, 0);
        assert_eq!(prompts[1].urgency, PromptUrgency::Due);
    }
}
