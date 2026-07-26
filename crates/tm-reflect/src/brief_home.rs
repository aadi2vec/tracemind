//! X4 — 4-slot Brief home engine.
//!
//! Product-plan §4.1 mandates that the Brief has *exactly four* card
//! slots — Recall / Compose / Reconcile / Rehearse — and each card has
//! *exactly four* actions — Open / Pin / Dismiss / Why. Empty slots stay
//! `None` (silence is a valid state, no filler).
//!
//! This module is pure data. It takes small `*Input` structs (one per
//! slot) and returns a [`BriefHome`]. The wiring to `ComposedIndex`,
//! `Algebra`, `contradiction_rate`, and `IntentStore` lives in the
//! caller — that keeps `tm-reflect` free of runtime graph/retrieval
//! dependencies and makes the tests hermetic.
//!
//! Every emitted [`BriefCard`] carries a `hook_id` (Q3.1 feedback fabric
//! contract, X9): the four actions map to explicit
//! [`tm_types::FeedbackKind`]s so the reward loop can close deterministically.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use tm_graph::{init_feedback_hooks_schema, record_hook, HookedCard};
use tm_types::{FeedbackKind, FeedbackSignal, Result};

// ---------------------------------------------------------------------------
// Slots + actions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BriefSlot {
    /// "What did I do yesterday / this week."
    Recall,
    /// "You've been thinking about X across N sessions; bridge?"
    Compose,
    /// "You said A then B; resolve?"
    Reconcile,
    /// "Commitment X is due; here's the context."
    Rehearse,
}

impl BriefSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            BriefSlot::Recall => "recall",
            BriefSlot::Compose => "compose",
            BriefSlot::Reconcile => "reconcile",
            BriefSlot::Rehearse => "rehearse",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardAction {
    Open,
    Pin,
    Dismiss,
    Why,
}

impl CardAction {
    pub fn as_str(self) -> &'static str {
        match self {
            CardAction::Open => "open",
            CardAction::Pin => "pin",
            CardAction::Dismiss => "dismiss",
            CardAction::Why => "why",
        }
    }

    /// The feedback signal an action emits. `Open` and `Pin` are positive
    /// evidence; `Dismiss` is negative; `Why` is a neutral explainability
    /// tap that must not train the retrieval policy on its own.
    pub fn feedback_kind(self) -> Option<FeedbackKind> {
        match self {
            CardAction::Open => Some(FeedbackKind::RetrievalCited),
            CardAction::Pin => Some(FeedbackKind::CardAccepted),
            CardAction::Dismiss => Some(FeedbackKind::CardRejected),
            CardAction::Why => None,
        }
    }

    /// Scalar score for the emitted signal. Positive rewards `Open` /
    /// `Pin`, negative penalises `Dismiss`. See §2.1 of `MVP-STATUS`.
    pub fn feedback_score(self) -> f32 {
        match self {
            CardAction::Open => 1.0,
            CardAction::Pin => 1.0,
            CardAction::Dismiss => -1.0,
            CardAction::Why => 0.0,
        }
    }

    pub fn all() -> [CardAction; 4] {
        [
            CardAction::Open,
            CardAction::Pin,
            CardAction::Dismiss,
            CardAction::Why,
        ]
    }
}

// ---------------------------------------------------------------------------
// Card + Home
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefCard {
    /// Deterministic per (slot, title, refs) so replays produce stable
    /// ids — cards are content-addressed, not random.
    pub id: Uuid,
    /// Feedback hook the four actions write into (Q3.1). Random per
    /// card *instance* so re-showing the same content is a distinct
    /// impression from the bandit's view.
    pub hook_id: Uuid,
    pub slot: BriefSlot,
    pub title: String,
    pub body: String,
    /// Backing memory ids (entity, triple, signal, or commitment uuids
    /// as strings). Passed back on `Open` so the router can navigate.
    pub refs: Vec<String>,
    /// Always the same four actions. Kept in the payload so a
    /// downstream renderer can iterate without knowing the contract.
    pub actions: Vec<CardAction>,
    pub created_at: DateTime<Utc>,
}

impl BriefCard {
    fn build(
        slot: BriefSlot,
        title: String,
        body: String,
        refs: Vec<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let id = deterministic_id(slot, &title, &refs);
        Self {
            id,
            hook_id: Uuid::new_v4(),
            slot,
            title,
            body,
            refs,
            actions: CardAction::all().to_vec(),
            created_at: now,
        }
    }
}

/// The 4-slot home. Every slot is `Option`: `None` = silence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefHome {
    pub recall: Option<BriefCard>,
    pub compose: Option<BriefCard>,
    pub reconcile: Option<BriefCard>,
    pub rehearse: Option<BriefCard>,
    pub generated_at: DateTime<Utc>,
}

impl BriefHome {
    /// True when every slot is empty (product-plan §4.1: "silence is a
    /// valid product state").
    pub fn is_silent(&self) -> bool {
        self.recall.is_none()
            && self.compose.is_none()
            && self.reconcile.is_none()
            && self.rehearse.is_none()
    }

    /// Iterate over the populated cards.
    pub fn cards(&self) -> impl Iterator<Item = &BriefCard> {
        [
            self.recall.as_ref(),
            self.compose.as_ref(),
            self.reconcile.as_ref(),
            self.rehearse.as_ref(),
        ]
        .into_iter()
        .flatten()
    }
}

// ---------------------------------------------------------------------------
// Inputs (one per slot)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct RecallInput {
    /// Most-recent-first list of things the user's memory contains.
    /// Empty vec = no Recall card.
    pub items: Vec<RecallItem>,
}

#[derive(Debug, Clone)]
pub struct RecallItem {
    pub id: String,
    pub text: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct ComposeInput {
    pub bridges: Vec<Bridge>,
}

#[derive(Debug, Clone)]
pub struct Bridge {
    /// Human topic label ("the Q4 review", "Alice's onboarding").
    pub topic: String,
    /// Distinct session/host ids the topic appeared in. `len() < 2`
    /// suppresses the bridge (bridging needs at least two contexts).
    pub session_ids: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ReconcileInput {
    pub contradictions: Vec<Conflict>,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    /// Free-form topic ("launch date", "hire target").
    pub topic: String,
    /// The two values that disagree.
    pub a: String,
    pub b: String,
    /// When each was asserted.
    pub a_ts: DateTime<Utc>,
    pub b_ts: DateTime<Utc>,
    /// UUIDs of the two triples (for `Open`).
    pub a_ref: String,
    pub b_ref: String,
}

#[derive(Debug, Clone, Default)]
pub struct RehearseInput {
    pub due: Vec<DueItem>,
}

#[derive(Debug, Clone)]
pub struct DueItem {
    pub id: String,
    pub title: String,
    pub context: String,
    pub due_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Build the 4-slot home. Deterministic given inputs + `now`.
///
/// One rule per slot; all four are independent — a broken upstream on one
/// slot never suppresses the others.
pub fn build_home(
    recall: RecallInput,
    compose: ComposeInput,
    reconcile: ReconcileInput,
    rehearse: RehearseInput,
    now: DateTime<Utc>,
) -> BriefHome {
    BriefHome {
        recall: build_recall(recall, now),
        compose: build_compose(compose, now),
        reconcile: build_reconcile(reconcile, now),
        rehearse: build_rehearse(rehearse, now),
        generated_at: now,
    }
}

fn build_recall(input: RecallInput, now: DateTime<Utc>) -> Option<BriefCard> {
    if input.items.is_empty() {
        return None;
    }
    let top: &RecallItem = input.items.first()?;
    let title = format!("Recall from {}", top.source);
    let body = if input.items.len() == 1 {
        top.text.clone()
    } else {
        let n_more = input.items.len() - 1;
        format!("{}\n(+{n_more} more from the last day)", top.text)
    };
    let refs = input
        .items
        .iter()
        .map(|i| i.id.clone())
        .take(5)
        .collect::<Vec<_>>();
    Some(BriefCard::build(BriefSlot::Recall, title, body, refs, now))
}

fn build_compose(input: ComposeInput, now: DateTime<Utc>) -> Option<BriefCard> {
    // First bridge with ≥ 2 sessions wins. Bridges with fewer are
    // suppressed — bridging requires at least two contexts.
    let bridge = input.bridges.into_iter().find(|b| b.session_ids.len() >= 2)?;
    let title = format!("Bridge \"{}\"", bridge.topic);
    let body = format!(
        "You've been thinking about \"{}\" in {} sessions. Want the composed context for your next?",
        bridge.topic,
        bridge.session_ids.len()
    );
    Some(BriefCard::build(
        BriefSlot::Compose,
        title,
        body,
        bridge.session_ids,
        now,
    ))
}

fn build_reconcile(input: ReconcileInput, now: DateTime<Utc>) -> Option<BriefCard> {
    let c = input.contradictions.into_iter().next()?;
    let title = format!("Resolve \"{}\"", c.topic);
    let body = format!(
        "{} said {} on {}. Later you said {} on {}. Which is current?",
        c.topic,
        c.a,
        c.a_ts.format("%b %-d"),
        c.b,
        c.b_ts.format("%b %-d"),
    );
    Some(BriefCard::build(
        BriefSlot::Reconcile,
        title,
        body,
        vec![c.a_ref, c.b_ref],
        now,
    ))
}

fn build_rehearse(input: RehearseInput, now: DateTime<Utc>) -> Option<BriefCard> {
    // Prefer items due within the current day, else the most-overdue.
    let mut candidates = input.due;
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by_key(|d| d.due_at);
    let pick = candidates.into_iter().next()?;
    let hours_delta = (pick.due_at - now).num_hours();
    let when = match hours_delta {
        h if h <= 0 => format!("was due {}h ago", -h),
        h if h < 24 => format!("due in {h}h"),
        h => format!("due in {}d", h / 24),
    };
    let title = format!("Rehearse: {}", pick.title);
    let body = format!("{} — {}\n{}", pick.title, when, pick.context);
    Some(BriefCard::build(
        BriefSlot::Rehearse,
        title,
        body,
        vec![pick.id],
        now,
    ))
}

// ---------------------------------------------------------------------------
// Hook persistence (X9)
// ---------------------------------------------------------------------------

/// Persist every card's `hook_id` into the `feedback_hooks` table so a
/// later `memory_feedback` call can attribute the action back to the
/// impression. Idempotent — a repeated call for the same hook_id is a
/// no-op replace.
///
/// Callers pass the active host + session so the emitted feedback signals
/// carry attribution. Schema is installed on first call.
pub fn persist_hooks(
    conn: &Connection,
    home: &BriefHome,
    host_id: Option<&str>,
    session_id: Option<Uuid>,
) -> Result<usize> {
    init_feedback_hooks_schema(conn)?;
    let mut n = 0usize;
    for card in home.cards() {
        let hooked = HookedCard {
            hook_id: card.hook_id,
            slot: card.slot.as_str().to_string(),
            refs: card.refs.clone(),
            card_id: Some(card.id),
            host_id: host_id.map(str::to_string),
            session_id,
            created_at: card.created_at,
        };
        record_hook(conn, &hooked)?;
        n += 1;
    }
    Ok(n)
}

/// Given a `hook_id` and the `CardAction` the user pressed, build the
/// resulting [`FeedbackSignal`] (or `None` for actions with no signal —
/// currently just `Why`). The signal still needs to be written to the
/// `feedback_signals` table by the caller (typically via
/// [`tm_graph::record_feedback_signal`]).
pub fn signal_for_action(
    hook_id: Uuid,
    action: CardAction,
    target_id: Option<Uuid>,
    host: Option<&str>,
    session_id: Option<Uuid>,
    verb: Option<&str>,
) -> Option<FeedbackSignal> {
    let kind = action.feedback_kind()?;
    let mut s = FeedbackSignal::new(kind, hook_id, target_id, action.feedback_score());
    if let Some(h) = host {
        s = s.with_host(h);
    }
    if let Some(v) = verb {
        s = s.with_verb(v);
    }
    if let Some(sid) = session_id {
        s = s.with_session(sid);
    }
    Some(s)
}

fn deterministic_id(slot: BriefSlot, title: &str, refs: &[String]) -> Uuid {
    let mut h = Sha256::new();
    h.update(slot.as_str().as_bytes());
    h.update(b"|");
    h.update(title.as_bytes());
    h.update(b"|");
    for r in refs {
        h.update(r.as_bytes());
        h.update(b",");
    }
    let digest = h.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 23, hour, 0, 0).unwrap()
    }

    #[test]
    fn empty_inputs_produce_silent_home() {
        let h = build_home(
            RecallInput::default(),
            ComposeInput::default(),
            ReconcileInput::default(),
            RehearseInput::default(),
            t(9),
        );
        assert!(h.is_silent());
        assert_eq!(h.cards().count(), 0);
    }

    #[test]
    fn recall_only_populates_recall_slot() {
        let h = build_home(
            RecallInput {
                items: vec![RecallItem {
                    id: "r1".into(),
                    text: "you said the launch is Nov 5".into(),
                    source: "clipboard".into(),
                }],
            },
            ComposeInput::default(),
            ReconcileInput::default(),
            RehearseInput::default(),
            t(9),
        );
        assert!(h.recall.is_some());
        assert!(h.compose.is_none());
        assert!(h.reconcile.is_none());
        assert!(h.rehearse.is_none());
        let card = h.recall.unwrap();
        assert_eq!(card.slot, BriefSlot::Recall);
        assert_eq!(card.actions.len(), 4);
    }

    #[test]
    fn compose_suppressed_when_only_one_session() {
        let h = build_home(
            RecallInput::default(),
            ComposeInput {
                bridges: vec![Bridge {
                    topic: "the Q4 review".into(),
                    session_ids: vec!["s1".into()],
                }],
            },
            ReconcileInput::default(),
            RehearseInput::default(),
            t(9),
        );
        // Only one session — bridge suppressed.
        assert!(h.compose.is_none());
    }

    #[test]
    fn compose_fires_when_two_sessions() {
        let h = build_home(
            RecallInput::default(),
            ComposeInput {
                bridges: vec![Bridge {
                    topic: "the Q4 review".into(),
                    session_ids: vec!["s1".into(), "s2".into()],
                }],
            },
            ReconcileInput::default(),
            RehearseInput::default(),
            t(9),
        );
        let c = h.compose.unwrap();
        assert!(c.body.contains("Q4 review"));
        assert_eq!(c.refs.len(), 2);
    }

    #[test]
    fn reconcile_carries_both_refs() {
        let h = build_home(
            RecallInput::default(),
            ComposeInput::default(),
            ReconcileInput {
                contradictions: vec![Conflict {
                    topic: "launch date".into(),
                    a: "Nov 5".into(),
                    b: "Nov 12".into(),
                    a_ts: t(8),
                    b_ts: t(9),
                    a_ref: "tripleA".into(),
                    b_ref: "tripleB".into(),
                }],
            },
            RehearseInput::default(),
            t(10),
        );
        let card = h.reconcile.unwrap();
        assert_eq!(card.slot, BriefSlot::Reconcile);
        assert_eq!(card.refs, vec!["tripleA", "tripleB"]);
        assert!(card.body.contains("Nov 5"));
        assert!(card.body.contains("Nov 12"));
    }

    #[test]
    fn rehearse_picks_most_urgent() {
        let h = build_home(
            RecallInput::default(),
            ComposeInput::default(),
            ReconcileInput::default(),
            RehearseInput {
                due: vec![
                    DueItem {
                        id: "c1".into(),
                        title: "ship v2".into(),
                        context: "with Alice".into(),
                        due_at: t(12),
                    },
                    DueItem {
                        id: "c2".into(),
                        title: "send memo".into(),
                        context: "quarterly".into(),
                        due_at: t(8),
                    },
                ],
            },
            t(10),
        );
        let card = h.rehearse.unwrap();
        assert_eq!(card.refs, vec!["c2"]); // more urgent (already overdue by 2h)
        assert!(card.body.contains("was due"));
    }

    #[test]
    fn card_id_is_deterministic() {
        let refs = vec!["a".to_string(), "b".to_string()];
        let now = t(9);
        let a = BriefCard::build(BriefSlot::Recall, "t".into(), "b".into(), refs.clone(), now);
        let b = BriefCard::build(BriefSlot::Recall, "t".into(), "b".into(), refs.clone(), now);
        assert_eq!(a.id, b.id, "id must be deterministic per (slot,title,refs)");
        // hook_id, however, is per-impression: two cards must have distinct
        // hook_ids so the feedback fabric treats them as separate impressions.
        assert_ne!(a.hook_id, b.hook_id, "hook_id must be per-impression");
    }

    #[test]
    fn action_feedback_mapping_is_stable() {
        assert_eq!(
            CardAction::Open.feedback_kind(),
            Some(FeedbackKind::RetrievalCited)
        );
        assert_eq!(
            CardAction::Pin.feedback_kind(),
            Some(FeedbackKind::CardAccepted)
        );
        assert_eq!(
            CardAction::Dismiss.feedback_kind(),
            Some(FeedbackKind::CardRejected)
        );
        assert_eq!(CardAction::Why.feedback_kind(), None);
        assert!(CardAction::Open.feedback_score() > 0.0);
        assert!(CardAction::Dismiss.feedback_score() < 0.0);
        assert_eq!(CardAction::Why.feedback_score(), 0.0);
    }

    #[test]
    fn persist_hooks_writes_one_row_per_card() {
        let h = build_home(
            RecallInput {
                items: vec![RecallItem {
                    id: "r".into(),
                    text: "t".into(),
                    source: "s".into(),
                }],
            },
            ComposeInput::default(),
            ReconcileInput {
                contradictions: vec![Conflict {
                    topic: "x".into(),
                    a: "1".into(),
                    b: "2".into(),
                    a_ts: t(1),
                    b_ts: t(2),
                    a_ref: "a".into(),
                    b_ref: "b".into(),
                }],
            },
            RehearseInput::default(),
            t(9),
        );
        let conn = Connection::open_in_memory().unwrap();
        let n = persist_hooks(&conn, &h, Some("claude-code"), Some(Uuid::new_v4())).unwrap();
        assert_eq!(n, 2, "recall + reconcile cards");
        let recall = h.recall.unwrap();
        let looked = tm_graph::lookup_hook(&conn, recall.hook_id).unwrap().unwrap();
        assert_eq!(looked.slot, "recall");
        assert_eq!(looked.refs, vec!["r"]);
        assert_eq!(looked.card_id, Some(recall.id));
    }

    #[test]
    fn signal_for_action_maps_open_to_positive() {
        let hook = Uuid::new_v4();
        let s = signal_for_action(hook, CardAction::Open, None, Some("h"), None, Some("query"))
            .unwrap();
        assert_eq!(s.kind, FeedbackKind::RetrievalCited);
        assert_eq!(s.feedback_hook_id, hook);
        assert!(s.score > 0.0);
        assert_eq!(s.verb.as_deref(), Some("query"));
    }

    #[test]
    fn signal_for_action_maps_dismiss_to_negative() {
        let s =
            signal_for_action(Uuid::new_v4(), CardAction::Dismiss, None, None, None, None).unwrap();
        assert_eq!(s.kind, FeedbackKind::CardRejected);
        assert!(s.score < 0.0);
    }

    #[test]
    fn signal_for_action_returns_none_for_why() {
        assert!(signal_for_action(Uuid::new_v4(), CardAction::Why, None, None, None, None)
            .is_none());
    }

    #[test]
    fn all_slots_can_fire_simultaneously() {
        let h = build_home(
            RecallInput {
                items: vec![RecallItem {
                    id: "r1".into(),
                    text: "hello".into(),
                    source: "shell".into(),
                }],
            },
            ComposeInput {
                bridges: vec![Bridge {
                    topic: "topic".into(),
                    session_ids: vec!["s1".into(), "s2".into()],
                }],
            },
            ReconcileInput {
                contradictions: vec![Conflict {
                    topic: "x".into(),
                    a: "1".into(),
                    b: "2".into(),
                    a_ts: t(1),
                    b_ts: t(2),
                    a_ref: "a".into(),
                    b_ref: "b".into(),
                }],
            },
            RehearseInput {
                due: vec![DueItem {
                    id: "d".into(),
                    title: "T".into(),
                    context: "C".into(),
                    due_at: t(20),
                }],
            },
            t(9),
        );
        assert_eq!(h.cards().count(), 4);
        for card in h.cards() {
            assert_eq!(card.actions.len(), 4);
        }
    }
}
