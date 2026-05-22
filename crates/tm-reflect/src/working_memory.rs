//! WME-1..7 — the **Working Memory Engine**.
//!
//! Per PROJECT_2026.md §1b: memory surfaces a *correct* card before
//! the user asks. The WME consumes the three storage primitives
//! (`tm-graph`, `tm-vector`, `tm-cluster`) and produces verb-typed
//! [`Card`]s that the Brief and Dashboard render.
//!
//! Six verbs (this module is **the** authoritative list):
//!
//! * **Resume**   — pick back up an unfinished commitment (open intent)
//! * **Recall**   — surface a relevant memory the user hasn't seen this session
//! * **Compare**  — two memories disagree (contradiction, JTMS `Contradicted`)
//! * **Caution**  — an active commitment is at risk (overdue / outcome mismatch)
//! * **Connect**  — two clusters or entities share a non-obvious bridge
//! * **Anticipate** — a *new* topic emerged (outlier cluster, novel community)
//!
//! Commitment + contradiction become **filtered outputs of Caution +
//! Compare** — they are not separate surfaces.
//!
//! ## Architecture
//!
//! The engine is **pure data-flow**: it takes references to data
//! sources at call time and returns a `Vec<Card>`. It never holds
//! mutable global state. The cooldown / spam-cap layer is the
//! [`CardQueue`] which *is* stateful — it lives on disk so cooldowns
//! survive restarts.
//!
//! ## Scoring
//!
//! `score = relevance × surprise × recency_of_decision × outcome_signal`
//!
//! * **relevance** — cosine similarity between the card's target text
//!   and the L1 topic vector, in `[0, 1]`.
//! * **surprise** — how unusual the card is given recent surfacing
//!   history; cards similar to ones recently shown have lower surprise.
//! * **recency_of_decision** — `exp(-age / 24h)` for the underlying
//!   decision artefact (commitment / triple / capture).
//! * **outcome_signal** — `1 + 0.5 * sum(recent_feedback)` for cards
//!   of this kind, clipped to `[0.5, 1.5]`. Learned per user.
//!
//! ## Anti-spam
//!
//! [`CardQueue::push`] consults a per-`(kind, target_id)` cooldown +
//! a global hourly cap. WME-9 asserts ≤ 8 cards/hour even under a
//! 1000-event synthetic flood.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Hard cap on cards surfaced in any rolling 60-minute window
/// (WME-9 anti-spam regression test).
pub const MAX_CARDS_PER_HOUR: usize = 8;

/// Per-`(kind, target_id)` cooldown. A card with the same kind +
/// target within this many seconds will be silently dropped from
/// [`CardQueue::push`].
pub const PER_TARGET_COOLDOWN_SECS: i64 = 30 * 60; // 30 minutes

/// Score threshold below which cards are not surfaced.
pub const DEFAULT_SCORE_FLOOR: f32 = 0.18;

/// The six verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CardKind {
    Resume,
    Recall,
    Compare,
    Caution,
    Connect,
    Anticipate,
}

impl CardKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            CardKind::Resume => "resume",
            CardKind::Recall => "recall",
            CardKind::Compare => "compare",
            CardKind::Caution => "caution",
            CardKind::Connect => "connect",
            CardKind::Anticipate => "anticipate",
        }
    }
}

/// A single card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: Uuid,
    pub kind: CardKind,
    /// Stable identifier of what this card is *about*. For Resume +
    /// Caution this is a commitment id; for Compare it's a triple id;
    /// for Recall + Connect it's an entity id; for Anticipate it's a
    /// cluster id (as a uuid-namespaced string).
    pub target_id: String,
    pub statement: String,
    pub score: f32,
    pub created_at: DateTime<Utc>,
    /// The component signals that produced the score, kept around
    /// for the explainability drawer.
    pub signals: CardSignals,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardSignals {
    pub relevance: f32,
    pub surprise: f32,
    pub recency: f32,
    pub outcome: f32,
}

/// Feedback signal from the user (or an autonomous host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    UsefulNow,
    NotUsefulNow,
    NotNowRemindLater,
    DismissThisKind,
}

impl FeedbackKind {
    /// Numeric weight used in the outcome-signal aggregation. Bounded
    /// in `[-1, 1]`.
    pub fn weight(self) -> f32 {
        match self {
            FeedbackKind::UsefulNow => 1.0,
            FeedbackKind::NotUsefulNow => -0.5,
            FeedbackKind::NotNowRemindLater => -0.1,
            FeedbackKind::DismissThisKind => -1.0,
        }
    }
}

#[derive(Debug, Error)]
pub enum WmeError {
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid feedback: {0}")]
    Feedback(String),
}

pub type WmeResult<T> = Result<T, WmeError>;

/// A snapshot of recent recent capture events used to build the L1
/// topic vector. Constructed by the caller from `tm_cluster::Clusterer`.
#[derive(Debug, Clone)]
pub struct ActivityContext {
    /// Recent cluster centroids weighted by recency. Result of
    /// `Clusterer::recent_centroids(window)`.
    pub centroids: Vec<tm_cluster::RecentCentroid>,
    /// Entity ids the user has recently interacted with (graph
    /// upserts touching them; capture mentions). Optional — pass an
    /// empty vec if none.
    pub active_entities: Vec<i64>,
    /// Event ids of recent outliers — feed for the Anticipate verb.
    pub recent_outliers: Vec<String>,
}

/// Configuration for the engine.
#[derive(Debug, Clone)]
pub struct WmeConfig {
    pub topic_window: Duration,
    pub score_floor: f32,
    pub max_cards: usize,
    pub max_cards_per_hour: usize,
    pub per_target_cooldown: Duration,
}

impl Default for WmeConfig {
    fn default() -> Self {
        Self {
            topic_window: Duration::minutes(15),
            score_floor: DEFAULT_SCORE_FLOOR,
            max_cards: 8,
            max_cards_per_hour: MAX_CARDS_PER_HOUR,
            per_target_cooldown: Duration::seconds(PER_TARGET_COOLDOWN_SECS),
        }
    }
}

/// A single proposed card — what one of the per-verb proposers emits
/// before scoring + the cooldown queue trims them.
#[derive(Debug, Clone)]
pub struct CardProposal {
    pub kind: CardKind,
    pub target_id: String,
    pub statement: String,
    /// Free-form relevance score in `[0, 1]`. Set to `1.0` for things
    /// that are unconditionally salient (e.g. overdue commitment).
    pub relevance: f32,
    /// Optional embedding of the proposal's statement — if provided,
    /// the engine will combine it with the topic vector to refine
    /// relevance. Set to `None` to skip.
    pub statement_embedding: Option<Vec<f32>>,
    /// Underlying decision artefact age. Used to compute the recency
    /// signal. `None` means "now".
    pub age: Option<Duration>,
}

/// The Working Memory Engine. Holds the persistent queue + per-kind
/// feedback aggregates. Pure functions live as free `pub fn`s.
pub struct WorkingMemoryEngine {
    queue: Mutex<CardQueueInner>,
    feedback: Mutex<FeedbackAggregator>,
    config: WmeConfig,
    db_path: std::path::PathBuf,
}

impl WorkingMemoryEngine {
    pub fn open(db_path: impl AsRef<Path>) -> WmeResult<Self> {
        Self::open_with(db_path, WmeConfig::default())
    }

    pub fn open_with(db_path: impl AsRef<Path>, config: WmeConfig) -> WmeResult<Self> {
        let path = db_path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        ensure_schema(&conn)?;
        let queue = CardQueueInner::load(&conn)?;
        let feedback = FeedbackAggregator::load(&conn)?;
        Ok(Self {
            queue: Mutex::new(queue),
            feedback: Mutex::new(feedback),
            config,
            db_path: path,
        })
    }

    pub fn config(&self) -> &WmeConfig {
        &self.config
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// **WME-1** — recent topic vector. EMA of recent cluster centroids
    /// weighted by recency. Pure function; deterministic given the
    /// activity context. Returns an empty vec if there's no recent
    /// activity.
    pub fn topic_vector(activity: &ActivityContext) -> Vec<f32> {
        if activity.centroids.is_empty() {
            return Vec::new();
        }
        let dim = activity.centroids[0].centroid.len();
        if dim == 0 {
            return Vec::new();
        }
        let mut acc = vec![0.0_f32; dim];
        let mut total_weight = 0.0_f32;
        for c in &activity.centroids {
            if c.centroid.len() != dim {
                continue;
            }
            let w = c.recency_weight.max(0.0);
            total_weight += w;
            for (i, x) in c.centroid.iter().enumerate() {
                acc[i] += w * x;
            }
        }
        if total_weight < 1e-6 {
            return Vec::new();
        }
        for v in acc.iter_mut() {
            *v /= total_weight;
        }
        // L2-normalise for cosine sim downstream.
        let norm: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in acc.iter_mut() {
                *v /= norm;
            }
        }
        acc
    }

    /// **WME-3** — score a single proposal against the live topic
    /// vector + the user's historical feedback for this card kind.
    /// Pure; suitable for unit testing.
    pub fn score_proposal(
        &self,
        proposal: &CardProposal,
        topic_vec: &[f32],
    ) -> CardSignals {
        let relevance = if let Some(emb) = &proposal.statement_embedding {
            if topic_vec.is_empty() || emb.len() != topic_vec.len() {
                proposal.relevance.clamp(0.0, 1.0)
            } else {
                cosine_sim(emb, topic_vec).clamp(0.0, 1.0)
            }
        } else {
            proposal.relevance.clamp(0.0, 1.0)
        };
        let surprise = {
            let queue = self.queue.lock().expect("wme queue poisoned");
            queue.surprise_for(proposal.kind, &proposal.target_id)
        };
        let recency = match proposal.age {
            None => 1.0,
            Some(d) => {
                let hrs = d.num_seconds().max(0) as f32 / 3600.0;
                (-hrs / 24.0 * std::f32::consts::LN_2).exp().clamp(0.0, 1.0)
            }
        };
        let outcome = {
            let fb = self.feedback.lock().expect("wme feedback poisoned");
            fb.outcome_for(proposal.kind)
        };
        CardSignals { relevance, surprise, recency, outcome }
    }

    /// Compute the final scalar score for a `CardSignals`.
    pub fn final_score(s: &CardSignals) -> f32 {
        s.relevance * s.surprise * s.recency * s.outcome
    }

    /// **WME-2 → WME-3 → WME-4** — turn a stream of proposals into
    /// a final ranked + cooldown-respecting card list.
    pub fn rank_and_push(
        &self,
        proposals: Vec<CardProposal>,
        topic_vec: &[f32],
    ) -> WmeResult<Vec<Card>> {
        // 1. Score everything.
        let mut scored: Vec<(CardProposal, CardSignals, f32)> = proposals
            .into_iter()
            .map(|p| {
                let sig = self.score_proposal(&p, topic_vec);
                let s = Self::final_score(&sig);
                (p, sig, s)
            })
            .filter(|(_, _, s)| *s >= self.config.score_floor)
            .collect();
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // 2. Push through the cooldown queue.
        let now = Utc::now();
        let mut out = Vec::new();
        for (p, sig, score) in scored {
            if out.len() >= self.config.max_cards {
                break;
            }
            let card = Card {
                id: Uuid::new_v4(),
                kind: p.kind,
                target_id: p.target_id.clone(),
                statement: p.statement.clone(),
                score,
                created_at: now,
                signals: sig,
            };
            let pushed = {
                let mut q = self.queue.lock().expect("wme queue poisoned");
                q.try_push(&card, now, &self.config)?
            };
            if pushed {
                self.persist_card(&card)?;
                out.push(card);
            }
        }
        Ok(out)
    }

    fn persist_card(&self, card: &Card) -> WmeResult<()> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute(
            "INSERT INTO wme_cards(id, kind, target_id, statement, score, \
                relevance, surprise, recency, outcome, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                card.id.to_string(),
                card.kind.as_str(),
                card.target_id,
                card.statement,
                card.score as f64,
                card.signals.relevance as f64,
                card.signals.surprise as f64,
                card.signals.recency as f64,
                card.signals.outcome as f64,
                card.created_at.timestamp(),
            ],
        )?;
        Ok(())
    }

    /// **WME-6** — record one piece of feedback against a card. Updates
    /// the per-kind outcome aggregator (decayed weighted moving average).
    pub fn record_feedback(
        &self,
        card_id: Uuid,
        kind: CardKind,
        feedback: FeedbackKind,
    ) -> WmeResult<()> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute(
            "INSERT INTO wme_feedback(card_id, kind, feedback, recorded_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                card_id.to_string(),
                kind.as_str(),
                feedback_to_str(feedback),
                Utc::now().timestamp(),
            ],
        )?;
        let mut fb = self.feedback.lock().expect("wme feedback poisoned");
        fb.observe(kind, feedback);
        Ok(())
    }

    /// Read the most recent N persisted cards (for the Brief + the MCP
    /// `memory_brief` tool).
    pub fn recent_cards(&self, limit: usize) -> WmeResult<Vec<Card>> {
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, target_id, statement, score, \
                    relevance, surprise, recency, outcome, created_at \
             FROM wme_cards ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let id_s: String = row.get(0)?;
            let kind_s: String = row.get(1)?;
            let target_id: String = row.get(2)?;
            let statement: String = row.get(3)?;
            let score: f64 = row.get(4)?;
            let relevance: f64 = row.get(5)?;
            let surprise: f64 = row.get(6)?;
            let recency: f64 = row.get(7)?;
            let outcome: f64 = row.get(8)?;
            let created_at: i64 = row.get(9)?;
            // Parse the id strictly. The previous implementation fell back
            // to `Uuid::nil()` on parse failure, which silently collapsed
            // every malformed row to the same id — poisoning the UI's
            // per-card optimistic-hide set so clicking feedback on one
            // card hid them all. Return `None` here and let the loop
            // below filter / log instead.
            let id_opt = Uuid::parse_str(&id_s).ok();
            Ok((
                id_opt,
                id_s,
                Card {
                    id: id_opt.unwrap_or_else(Uuid::nil),
                    kind: kind_from_str(&kind_s),
                    target_id,
                    statement,
                    score: score as f32,
                    created_at: DateTime::from_timestamp(created_at, 0).unwrap_or_else(Utc::now),
                    signals: CardSignals {
                        relevance: relevance as f32,
                        surprise: surprise as f32,
                        recency: recency as f32,
                        outcome: outcome as f32,
                    },
                },
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id_opt, id_s, card) = r?;
            if id_opt.is_none() {
                eprintln!(
                    "tm_reflect::wme: skipping wme_cards row with non-UUID id {id_s:?}"
                );
                continue;
            }
            out.push(card);
        }
        Ok(out)
    }

    /// Count cards surfaced in the last hour. Used by anti-spam
    /// regression tests.
    pub fn cards_in_last_hour(&self) -> WmeResult<usize> {
        let q = self.queue.lock().expect("wme queue poisoned");
        Ok(q.cards_in_window(Duration::hours(1), Utc::now()))
    }
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    dot / (na.sqrt().max(1e-6) * nb.sqrt().max(1e-6))
}

fn feedback_to_str(f: FeedbackKind) -> &'static str {
    match f {
        FeedbackKind::UsefulNow => "useful_now",
        FeedbackKind::NotUsefulNow => "not_useful_now",
        FeedbackKind::NotNowRemindLater => "not_now_remind_later",
        FeedbackKind::DismissThisKind => "dismiss_this_kind",
    }
}

fn kind_from_str(s: &str) -> CardKind {
    match s {
        "resume" => CardKind::Resume,
        "recall" => CardKind::Recall,
        "compare" => CardKind::Compare,
        "caution" => CardKind::Caution,
        "connect" => CardKind::Connect,
        "anticipate" => CardKind::Anticipate,
        _ => CardKind::Recall,
    }
}

/// **WME-4** — cooldown queue. In-memory ring + persistent log.
struct CardQueueInner {
    /// Recent surfaces, oldest first.
    recent: Vec<(CardKind, String, DateTime<Utc>)>,
}

impl CardQueueInner {
    fn load(conn: &Connection) -> WmeResult<Self> {
        let mut stmt = conn.prepare(
            "SELECT kind, target_id, created_at FROM wme_cards ORDER BY created_at DESC LIMIT 256",
        )?;
        let rows = stmt.query_map([], |row| {
            let kind: String = row.get(0)?;
            let tid: String = row.get(1)?;
            let ts: i64 = row.get(2)?;
            Ok((
                kind_from_str(&kind),
                tid,
                DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now),
            ))
        })?;
        let mut v: Vec<_> = Vec::new();
        for r in rows {
            v.push(r?);
        }
        v.reverse(); // ascending
        Ok(Self { recent: v })
    }

    fn try_push(
        &mut self,
        card: &Card,
        now: DateTime<Utc>,
        cfg: &WmeConfig,
    ) -> WmeResult<bool> {
        // Per-target cooldown.
        for (k, tid, ts) in self.recent.iter().rev() {
            if *k == card.kind
                && tid == &card.target_id
                && now - *ts < cfg.per_target_cooldown
            {
                return Ok(false);
            }
        }
        // Hourly cap.
        if self.cards_in_window(Duration::hours(1), now) >= cfg.max_cards_per_hour {
            return Ok(false);
        }
        self.recent
            .push((card.kind, card.target_id.clone(), card.created_at));
        if self.recent.len() > 256 {
            // Drop oldest 64 to keep memory bounded.
            self.recent.drain(0..64);
        }
        Ok(true)
    }

    fn cards_in_window(&self, window: Duration, now: DateTime<Utc>) -> usize {
        let cutoff = now - window;
        self.recent.iter().filter(|(_, _, ts)| *ts >= cutoff).count()
    }

    /// Surprise = 1.0 if we haven't surfaced this `(kind, target)`
    /// recently, decaying toward 0.2 as it gets shown repeatedly.
    fn surprise_for(&self, kind: CardKind, target_id: &str) -> f32 {
        let mut prior_count = 0_u32;
        for (k, tid, _) in self.recent.iter() {
            if *k == kind && tid == target_id {
                prior_count += 1;
            }
        }
        let s = 1.0 - 0.2 * prior_count as f32;
        s.clamp(0.2, 1.0)
    }
}

/// Per-card-kind feedback aggregator. Decaying weighted moving avg
/// over the last N observations.
struct FeedbackAggregator {
    by_kind: HashMap<CardKind, f32>,
}

impl FeedbackAggregator {
    fn load(conn: &Connection) -> WmeResult<Self> {
        let mut stmt = conn.prepare(
            "SELECT kind, feedback FROM wme_feedback ORDER BY recorded_at DESC LIMIT 256",
        )?;
        let rows = stmt.query_map([], |row| {
            let k: String = row.get(0)?;
            let f: String = row.get(1)?;
            Ok((k, f))
        })?;
        let mut by_kind: HashMap<CardKind, Vec<FeedbackKind>> = HashMap::new();
        for r in rows {
            let (k_s, f_s) = r?;
            let k = kind_from_str(&k_s);
            let fb = match f_s.as_str() {
                "useful_now" => FeedbackKind::UsefulNow,
                "not_useful_now" => FeedbackKind::NotUsefulNow,
                "not_now_remind_later" => FeedbackKind::NotNowRemindLater,
                "dismiss_this_kind" => FeedbackKind::DismissThisKind,
                _ => continue,
            };
            by_kind.entry(k).or_default().push(fb);
        }
        let mut by_kind_f = HashMap::new();
        for (k, fbs) in by_kind {
            by_kind_f.insert(k, Self::aggregate(&fbs));
        }
        Ok(Self { by_kind: by_kind_f })
    }

    fn aggregate(fbs: &[FeedbackKind]) -> f32 {
        if fbs.is_empty() {
            return 1.0;
        }
        // Decay-weighted mean: newer feedback weighs more (0.9^i).
        let mut num = 0.0_f32;
        let mut den = 0.0_f32;
        for (i, fb) in fbs.iter().enumerate() {
            let w = 0.9_f32.powi(i as i32);
            num += fb.weight() * w;
            den += w;
        }
        let mean = if den > 0.0 { num / den } else { 0.0 };
        (1.0 + 0.5 * mean).clamp(0.5, 1.5)
    }

    fn observe(&mut self, kind: CardKind, fb: FeedbackKind) {
        let cur = *self.by_kind.get(&kind).unwrap_or(&1.0);
        // Online EMA with α = 0.2.
        let target = (1.0 + 0.5 * fb.weight()).clamp(0.5, 1.5);
        let new = 0.8 * cur + 0.2 * target;
        self.by_kind.insert(kind, new);
    }

    fn outcome_for(&self, kind: CardKind) -> f32 {
        *self.by_kind.get(&kind).unwrap_or(&1.0)
    }
}

/// Set up the WME-owned tables.
pub fn ensure_schema(conn: &Connection) -> WmeResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS wme_cards (
            id           TEXT PRIMARY KEY,
            kind         TEXT NOT NULL,
            target_id    TEXT NOT NULL,
            statement    TEXT NOT NULL,
            score        REAL NOT NULL,
            relevance    REAL NOT NULL,
            surprise     REAL NOT NULL,
            recency      REAL NOT NULL,
            outcome      REAL NOT NULL,
            created_at   INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wme_cards_created
            ON wme_cards(created_at);
        CREATE INDEX IF NOT EXISTS idx_wme_cards_kind_target
            ON wme_cards(kind, target_id);
        CREATE TABLE IF NOT EXISTS wme_feedback (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            card_id      TEXT NOT NULL,
            kind         TEXT NOT NULL,
            feedback     TEXT NOT NULL,
            recorded_at  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_wme_feedback_kind
            ON wme_feedback(kind);
        "#,
    )?;
    Ok(())
}

/// **WME-2** — gather raw proposals from each data source. Free
/// function so callers can compose their own input set.
///
/// All inputs are optional: pass empty vecs for any unavailable source.
pub struct CandidateSources<'a> {
    /// Open commitments / intents (Resume + Caution feed).
    pub open_commitments: &'a [CommitmentInput],
    /// Contradictions surfaced by the JTMS (Compare feed).
    pub contradictions: &'a [ContradictionInput],
    /// Memories most similar to the topic vector (Recall feed).
    pub similar_memories: &'a [SimilarMemoryInput],
    /// Bridges between high-salience but rarely co-mentioned entities
    /// (Connect feed).
    pub bridges: &'a [BridgeInput],
    /// Outlier event ids (Anticipate feed).
    pub outliers: &'a [OutlierInput],
}

/// One open commitment for the Resume / Caution feed.
#[derive(Debug, Clone)]
pub struct CommitmentInput {
    pub commitment_id: String,
    pub statement: String,
    /// True if past its `due_at` — flips this commitment from a
    /// Resume to a Caution.
    pub overdue: bool,
    pub age: Duration,
    pub statement_embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct ContradictionInput {
    pub triple_id: String,
    pub statement: String,
    pub age: Duration,
    pub statement_embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct SimilarMemoryInput {
    pub memory_id: String,
    pub statement: String,
    pub relevance: f32,
    pub age: Duration,
    pub statement_embedding: Option<Vec<f32>>,
    /// True if we've already shown this memory in the current session
    /// — gates it out of Recall.
    pub seen_this_session: bool,
}

#[derive(Debug, Clone)]
pub struct BridgeInput {
    pub bridge_key: String,
    pub statement: String,
    pub relevance: f32,
    pub age: Duration,
    pub statement_embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct OutlierInput {
    pub cluster_id: String,
    pub statement: String,
    pub age: Duration,
}

/// **WME-2** — turn typed inputs into untyped proposals, ready for
/// scoring + the cooldown queue.
pub fn proposals_from_sources(sources: CandidateSources<'_>) -> Vec<CardProposal> {
    let mut out: Vec<CardProposal> = Vec::new();
    let mut seen_targets: HashSet<(CardKind, String)> = HashSet::new();
    let mut push = |p: CardProposal, seen: &mut HashSet<(CardKind, String)>| {
        let key = (p.kind, p.target_id.clone());
        if seen.insert(key) {
            out.push(p);
        }
    };

    for c in sources.open_commitments {
        let kind = if c.overdue { CardKind::Caution } else { CardKind::Resume };
        let prefix = if c.overdue { "Overdue" } else { "Resume" };
        push(
            CardProposal {
                kind,
                target_id: c.commitment_id.clone(),
                statement: format!("{prefix}: {}", c.statement),
                relevance: 0.9,
                statement_embedding: c.statement_embedding.clone(),
                age: Some(c.age),
            },
            &mut seen_targets,
        );
    }
    for ct in sources.contradictions {
        push(
            CardProposal {
                kind: CardKind::Compare,
                target_id: ct.triple_id.clone(),
                statement: format!("Compare: {}", ct.statement),
                relevance: 0.85,
                statement_embedding: ct.statement_embedding.clone(),
                age: Some(ct.age),
            },
            &mut seen_targets,
        );
    }
    for m in sources.similar_memories {
        if m.seen_this_session {
            continue;
        }
        push(
            CardProposal {
                kind: CardKind::Recall,
                target_id: m.memory_id.clone(),
                statement: format!("Recall: {}", m.statement),
                relevance: m.relevance.clamp(0.0, 1.0),
                statement_embedding: m.statement_embedding.clone(),
                age: Some(m.age),
            },
            &mut seen_targets,
        );
    }
    for b in sources.bridges {
        push(
            CardProposal {
                kind: CardKind::Connect,
                target_id: b.bridge_key.clone(),
                statement: format!("Connect: {}", b.statement),
                relevance: b.relevance.clamp(0.0, 1.0),
                statement_embedding: b.statement_embedding.clone(),
                age: Some(b.age),
            },
            &mut seen_targets,
        );
    }
    for o in sources.outliers {
        push(
            CardProposal {
                kind: CardKind::Anticipate,
                target_id: o.cluster_id.clone(),
                statement: format!("Anticipate: {}", o.statement),
                relevance: 0.7,
                statement_embedding: None,
                age: Some(o.age),
            },
            &mut seen_targets,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_engine() -> WorkingMemoryEngine {
        let dir = tempdir().unwrap();
        let db = dir.path().join("m.db");
        // Leak the tempdir so it sticks around for the lifetime of the test process.
        Box::leak(Box::new(dir));
        WorkingMemoryEngine::open(&db).unwrap()
    }

    #[test]
    fn topic_vector_empty_for_no_activity() {
        let activity = ActivityContext {
            centroids: vec![],
            active_entities: vec![],
            recent_outliers: vec![],
        };
        assert!(WorkingMemoryEngine::topic_vector(&activity).is_empty());
    }

    #[test]
    fn topic_vector_weighted_blend_is_unit_norm() {
        let activity = ActivityContext {
            centroids: vec![
                tm_cluster::RecentCentroid {
                    cluster_id: 1,
                    centroid: vec![1.0, 0.0, 0.0],
                    recency_weight: 1.0,
                    n_members: 5,
                    updated_at: Utc::now(),
                },
                tm_cluster::RecentCentroid {
                    cluster_id: 2,
                    centroid: vec![0.0, 1.0, 0.0],
                    recency_weight: 0.5,
                    n_members: 3,
                    updated_at: Utc::now(),
                },
            ],
            active_entities: vec![],
            recent_outliers: vec![],
        };
        let v = WorkingMemoryEngine::topic_vector(&activity);
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-5, "{v:?} has norm {n}");
    }

    #[test]
    fn rank_respects_cooldown_per_target() {
        let eng = fresh_engine();
        let proposal = || CardProposal {
            kind: CardKind::Recall,
            target_id: "mem-1".into(),
            statement: "..".into(),
            relevance: 0.9,
            statement_embedding: None,
            age: Some(Duration::seconds(1)),
        };
        let topic = vec![1.0, 0.0, 0.0];
        let first = eng.rank_and_push(vec![proposal()], &topic).unwrap();
        assert_eq!(first.len(), 1);
        let second = eng.rank_and_push(vec![proposal()], &topic).unwrap();
        assert!(second.is_empty(), "cooldown should have suppressed");
    }

    #[test]
    fn anti_spam_caps_at_eight_per_hour() {
        let eng = fresh_engine();
        // Push 100 *different* targets — only 8 should land.
        let proposals: Vec<CardProposal> = (0..100)
            .map(|i| CardProposal {
                kind: CardKind::Recall,
                target_id: format!("mem-{i}"),
                statement: format!("statement {i}"),
                relevance: 0.9,
                statement_embedding: None,
                age: Some(Duration::seconds(1)),
            })
            .collect();
        let topic = vec![1.0, 0.0, 0.0];
        let surfaced = eng.rank_and_push(proposals, &topic).unwrap();
        assert!(surfaced.len() <= MAX_CARDS_PER_HOUR);
        assert!(eng.cards_in_last_hour().unwrap() <= MAX_CARDS_PER_HOUR);
    }

    #[test]
    fn feedback_lowers_outcome_for_dismissed_kind() {
        let eng = fresh_engine();
        let p = CardProposal {
            kind: CardKind::Recall,
            target_id: "mem-1".into(),
            statement: "..".into(),
            relevance: 0.9,
            statement_embedding: None,
            age: Some(Duration::seconds(1)),
        };
        let topic = vec![1.0, 0.0, 0.0];
        let card = eng.rank_and_push(vec![p.clone()], &topic).unwrap()[0].clone();
        let before = eng.score_proposal(&p, &topic).outcome;
        eng.record_feedback(card.id, CardKind::Recall, FeedbackKind::DismissThisKind)
            .unwrap();
        let after = eng.score_proposal(&p, &topic).outcome;
        assert!(after < before, "expected outcome to drop, {before} -> {after}");
    }

    #[test]
    fn proposals_from_sources_picks_correct_verb() {
        let commits = [CommitmentInput {
            commitment_id: "c-1".into(),
            statement: "ship demo".into(),
            overdue: true,
            age: Duration::hours(2),
            statement_embedding: None,
        }];
        let contra = [ContradictionInput {
            triple_id: "t-1".into(),
            statement: "x knows y vs x doesn't know y".into(),
            age: Duration::hours(1),
            statement_embedding: None,
        }];
        let mems: [SimilarMemoryInput; 0] = [];
        let bridges: [BridgeInput; 0] = [];
        let outliers = [OutlierInput {
            cluster_id: "cl-7".into(),
            statement: "new topic: vector compression".into(),
            age: Duration::minutes(5),
        }];
        let sources = CandidateSources {
            open_commitments: &commits,
            contradictions: &contra,
            similar_memories: &mems,
            bridges: &bridges,
            outliers: &outliers,
        };
        let ps = proposals_from_sources(sources);
        let kinds: Vec<_> = ps.iter().map(|p| p.kind).collect();
        assert!(kinds.contains(&CardKind::Caution));
        assert!(kinds.contains(&CardKind::Compare));
        assert!(kinds.contains(&CardKind::Anticipate));
    }
}
