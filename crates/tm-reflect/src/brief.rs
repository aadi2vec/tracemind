//! Daily brief data shape + builder.
//!
//! [`DailyBrief`] is the canonical structure surfaced in
//! `tracemind brief` (CLI text), `memory_brief` (MCP JSON) and
//! eventually the Tauri timeline view. It is deterministic: given
//! the same intent-store snapshot and `now`, the same brief comes
//! out. No clocks read inside the builder — the caller passes
//! `now`, which makes tests trivial.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use tm_intent::{
    Commitment, CommitmentKind, IntentStore, Polarity, Source, Stakes, State,
};

use crate::pattern::{
    default_window_start, detect_patterns, CellKey, DetectedPattern, PatternConfig, PolarityDist,
};

/// Builder configuration. Sensible defaults; override only when a
/// surface (e.g. mobile brief) needs a tighter view.
#[derive(Debug, Clone)]
pub struct BriefConfig {
    /// Window for "recently resolved". Default: last 7 days.
    pub resolved_window: Duration,
    /// Cap on rows per section.
    pub limit_open: usize,
    pub limit_overdue: usize,
    pub limit_resolved: usize,
    pub limit_candidates: usize,
    /// Pattern-detector config. Defaults match `INTENT_SYSTEM.md`
    /// §5.1.3 (n ≥ 6, |lift| ≥ 0.25, etc.).
    pub patterns: PatternConfig,
    /// Lookback window for the pattern detector. Default: 365 days.
    /// We pull `Completed`-with-polarity commitments inside this
    /// window, then bucket into cells.
    pub patterns_window: Duration,
    /// Cap on completed commitments scanned per pattern run — keeps
    /// nightly compute bounded for power-users with thousands of
    /// outcomes. Default: 2000.
    pub patterns_scan_limit: usize,
}

impl Default for BriefConfig {
    fn default() -> Self {
        Self {
            resolved_window: Duration::days(7),
            limit_open: 20,
            limit_overdue: 20,
            limit_resolved: 10,
            limit_candidates: 10,
            patterns: PatternConfig::default(),
            patterns_window: Duration::days(365),
            patterns_scan_limit: 2000,
        }
    }
}

#[derive(Debug, Error)]
pub enum BriefError {
    #[error("intent store: {0}")]
    Store(#[from] tm_intent::store::StoreError),
}

/// One row about an open commitment in the brief. Trimmed view of
/// [`Commitment`] — we drop heavy fields (snapshot id, derived_from)
/// the surface won't show.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommitmentBriefRow {
    pub id: Uuid,
    pub kind: CommitmentKind,
    pub statement: String,
    pub state: State,
    pub stakes: Stakes,
    pub source: Source,
    pub made_at: DateTime<Utc>,
    pub horizon: Option<DateTime<Utc>>,
    /// Bucket relative to `now` — surfaces use this to colour rows.
    /// Always `None` on rows in the *open* section that are not
    /// overdue; populated on the *overdue* section.
    pub overdue_class: Option<OverdueClass>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverdueClass {
    /// horizon ∈ (now-24h, now]
    DueToday,
    /// horizon ∈ (now-7d, now-24h]
    OverdueRecent,
    /// horizon ≤ now-7d
    OverdueStale,
}

impl OverdueClass {
    pub fn classify(horizon: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let lag = now - horizon;
        if lag <= Duration::hours(24) {
            OverdueClass::DueToday
        } else if lag <= Duration::days(7) {
            OverdueClass::OverdueRecent
        } else {
            OverdueClass::OverdueStale
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedBriefRow {
    pub id: Uuid,
    pub kind: CommitmentKind,
    pub statement: String,
    pub state: State,
    /// `None` if the commitment is `Abandoned` / `Superseded` (no outcome).
    pub polarity: Option<Polarity>,
    pub made_at: DateTime<Utc>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateBriefRow {
    pub id: Uuid,
    pub kind: CommitmentKind,
    pub statement: String,
    pub matched_phrase: String,
    pub confidence: f32,
    pub created_at: DateTime<Utc>,
}

/// One detected pattern row, surfaced in §9.1 of `INTENT_SYSTEM.md`.
/// Carries the deterministic template render plus structured fields
/// the calibration panel uses to score the pattern detector itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatternBriefRow {
    /// Stable cell identity — the future calibration / silencing
    /// surface keys against this.
    pub cell: CellKey,
    /// 16-hex prefix of the canonical-JSON hash of `cell`. Surfaced
    /// to the user so they can run `tracemind patterns silence
    /// <cell_hash>`.
    pub cell_hash: String,
    pub n: usize,
    pub dist: PolarityDist,
    pub global_dist: PolarityDist,
    pub lift_worse: f32,
    pub lift_better: f32,
    pub support_lb: f32,
    /// Pre-rendered §5.1.4 template — caller may show as-is.
    pub render: String,
}

impl From<DetectedPattern> for PatternBriefRow {
    fn from(p: DetectedPattern) -> Self {
        let cell_hash = p.cell.cell_hash();
        Self {
            cell: p.cell,
            cell_hash,
            n: p.n,
            dist: p.dist,
            global_dist: p.global_dist,
            lift_worse: p.lift_worse,
            lift_better: p.lift_better,
            support_lb: p.support_lb,
            render: p.render,
        }
    }
}

/// The full daily brief. Section ordering matches §9.1 of
/// `INTENT_SYSTEM.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyBrief {
    pub generated_at: DateTime<Utc>,
    /// Subset of `open` whose `horizon ≤ generated_at`. Soonest first.
    pub overdue: Vec<CommitmentBriefRow>,
    /// `Open` + `Acted` commitments. Newest first. Excludes overdue
    /// rows (those go into `overdue` only — a row appears in exactly
    /// one section).
    pub open: Vec<CommitmentBriefRow>,
    /// Recently terminal commitments (Completed / Abandoned /
    /// Superseded), inside the `resolved_window`. Newest first.
    pub resolved: Vec<ResolvedBriefRow>,
    /// Pending mined candidates. Newest first.
    pub candidates: Vec<CandidateBriefRow>,
    /// Patterns the detector flagged as exceeding `min_abs_lift` and
    /// passing the Wilson-LB support gate. Sorted by `|lift|` desc.
    /// May be empty when the user has too few completed commitments
    /// for the global baseline to be trustworthy.
    pub patterns: Vec<PatternBriefRow>,
    /// Section counts for quick rendering of section headers.
    pub counts: BriefCounts,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BriefCounts {
    pub overdue: usize,
    pub open: usize,
    pub resolved: usize,
    pub candidates: usize,
    pub patterns: usize,
}

pub struct BriefBuilder<'a> {
    store: &'a IntentStore,
    config: BriefConfig,
}

impl<'a> BriefBuilder<'a> {
    pub fn new(store: &'a IntentStore) -> Self {
        Self {
            store,
            config: BriefConfig::default(),
        }
    }

    pub fn with_config(mut self, config: BriefConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the brief as of `now`. Pure: no internal clock reads.
    pub fn build(&self, now: DateTime<Utc>) -> Result<DailyBrief, BriefError> {
        let cfg = &self.config;

        // Overdue first — sorted by horizon ASC by the store query.
        let overdue_raw = self.store.list_overdue_open(now, cfg.limit_overdue)?;
        let overdue_ids: std::collections::HashSet<Uuid> =
            overdue_raw.iter().map(|c| c.id).collect();

        let overdue: Vec<CommitmentBriefRow> = overdue_raw
            .into_iter()
            .map(|c| commitment_row(c, now, /* tag_overdue */ true))
            .collect();

        // Open list — drop anything already shown in `overdue` so a
        // commitment never double-renders.
        let open_raw = self.store.list_open(cfg.limit_open + cfg.limit_overdue)?;
        let open: Vec<CommitmentBriefRow> = open_raw
            .into_iter()
            .filter(|c| !overdue_ids.contains(&c.id))
            .take(cfg.limit_open)
            .map(|c| commitment_row(c, now, /* tag_overdue */ false))
            .collect();

        let cutoff = now - cfg.resolved_window;
        let resolved_raw = self.store.list_recent_resolved(cutoff, cfg.limit_resolved)?;
        let resolved: Vec<ResolvedBriefRow> = resolved_raw
            .into_iter()
            .map(|c| {
                // Look up the outcome polarity if any. Best-effort —
                // a missing outcome means the commitment is in a
                // terminal state without a polarity (Abandoned /
                // Superseded).
                let polarity = c
                    .outcome_id
                    .and_then(|oid| self.store.get_outcome(oid).ok().flatten())
                    .map(|o| o.polarity);
                ResolvedBriefRow {
                    id: c.id,
                    kind: c.kind,
                    statement: c.statement,
                    state: c.state,
                    polarity,
                    made_at: c.made_at,
                    tags: c.tags,
                }
            })
            .collect();

        let candidates_raw = self
            .store
            .list_pending_candidates(cfg.limit_candidates)?;
        let candidates: Vec<CandidateBriefRow> = candidates_raw
            .into_iter()
            .map(|c| CandidateBriefRow {
                id: c.id,
                kind: c.kind,
                statement: c.statement,
                matched_phrase: c.matched_phrase,
                confidence: c.confidence,
                created_at: c.created_at,
            })
            .collect();

        // Pattern detector — read completed-with-polarity inside
        // the lookback window, run the deterministic detector, then
        // filter cells the user has actively silenced. Soft-fail:
        // if the detector errors (e.g. a corrupt outcome row) the
        // brief still renders without patterns rather than failing
        // whole.
        let pattern_window_start = now - cfg.patterns_window;
        let _ = default_window_start; // keep symbol live for callers
        let silenced: std::collections::HashSet<String> = self
            .store
            .list_active_pattern_silences(now)
            .map(|rows| rows.into_iter().map(|s| s.cell_hash).collect())
            .unwrap_or_default();
        let patterns: Vec<PatternBriefRow> = match self
            .store
            .list_completed_with_polarity(pattern_window_start, cfg.patterns_scan_limit)
        {
            Ok(completed) => detect_patterns(&completed, &cfg.patterns)
                .into_iter()
                .map(PatternBriefRow::from)
                .filter(|row| !silenced.contains(&row.cell_hash))
                .collect(),
            Err(_) => Vec::new(),
        };

        let counts = BriefCounts {
            overdue: overdue.len(),
            open: open.len(),
            resolved: resolved.len(),
            candidates: candidates.len(),
            patterns: patterns.len(),
        };

        Ok(DailyBrief {
            generated_at: now,
            overdue,
            open,
            resolved,
            candidates,
            patterns,
            counts,
        })
    }
}

fn commitment_row(c: Commitment, now: DateTime<Utc>, tag_overdue: bool) -> CommitmentBriefRow {
    let overdue_class = if tag_overdue {
        c.horizon.map(|h| OverdueClass::classify(h, now))
    } else {
        None
    };
    CommitmentBriefRow {
        id: c.id,
        kind: c.kind,
        statement: c.statement,
        state: c.state,
        stakes: c.stakes,
        source: c.source,
        made_at: c.made_at,
        horizon: c.horizon,
        overdue_class,
        tags: c.tags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tm_intent::{
        Commitment, CommitmentKind, IntentStore, Outcome, OutcomeSource, Polarity, Source, State,
        state::transition,
    };

    fn fresh_store() -> IntentStore {
        IntentStore::open_in_memory().expect("open in-memory")
    }

    #[test]
    fn empty_store_yields_empty_brief() {
        let store = fresh_store();
        let now = Utc::now();
        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.counts.overdue, 0);
        assert_eq!(brief.counts.open, 0);
        assert_eq!(brief.counts.resolved, 0);
        assert_eq!(brief.counts.candidates, 0);
        assert_eq!(brief.generated_at, now);
    }

    #[test]
    fn overdue_classification_buckets_correctly() {
        let now = Utc::now();
        // Due today: 2h ago
        let due = OverdueClass::classify(now - Duration::hours(2), now);
        assert_eq!(due, OverdueClass::DueToday);
        // Recent overdue: 3d ago
        let recent = OverdueClass::classify(now - Duration::days(3), now);
        assert_eq!(recent, OverdueClass::OverdueRecent);
        // Stale: 30d ago
        let stale = OverdueClass::classify(now - Duration::days(30), now);
        assert_eq!(stale, OverdueClass::OverdueStale);
        // Edge: exactly 24h ago is still DueToday
        let edge = OverdueClass::classify(now - Duration::hours(24), now);
        assert_eq!(edge, OverdueClass::DueToday);
        // Edge: exactly 7d ago is still OverdueRecent
        let edge2 = OverdueClass::classify(now - Duration::days(7), now);
        assert_eq!(edge2, OverdueClass::OverdueRecent);
    }

    #[test]
    fn overdue_row_excluded_from_open_section() {
        let store = fresh_store();
        let now = Utc::now();

        // Two commitments — one overdue (past horizon), one not.
        let mut overdue = Commitment::new(CommitmentKind::Intent, "ship v2", Source::Manual);
        overdue.horizon = Some(now - Duration::hours(2));
        store.insert_commitment(&overdue).unwrap();

        let mut soon = Commitment::new(CommitmentKind::Intent, "follow up", Source::Manual);
        soon.horizon = Some(now + Duration::days(2));
        store.insert_commitment(&soon).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.overdue.len(), 1);
        assert_eq!(brief.overdue[0].id, overdue.id);
        // the overdue row carries its bucket
        assert_eq!(brief.overdue[0].overdue_class, Some(OverdueClass::DueToday));

        assert_eq!(brief.open.len(), 1);
        assert_eq!(brief.open[0].id, soon.id);
        // open-section rows do not carry a bucket
        assert_eq!(brief.open[0].overdue_class, None);
    }

    #[test]
    fn resolved_section_carries_polarity_for_completed() {
        let store = fresh_store();
        let now = Utc::now();

        let mut c =
            Commitment::new(CommitmentKind::Intent, "ship Friday", Source::Manual);
        c.made_at = now - Duration::hours(3);
        store.insert_commitment(&c).unwrap();

        let o = Outcome::new(c.id, Polarity::Worse, "shipped Tue", OutcomeSource::UserPrompted);
        store.insert_outcome(&o).unwrap();
        transition(&mut c, State::Completed, Some(&o)).unwrap();
        store.update_state(c.id, c.state, c.outcome_id).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.resolved.len(), 1);
        let row = &brief.resolved[0];
        assert_eq!(row.id, c.id);
        assert_eq!(row.polarity, Some(Polarity::Worse));
        assert_eq!(row.state, State::Completed);
    }

    #[test]
    fn resolved_section_handles_abandoned_without_polarity() {
        let store = fresh_store();
        let now = Utc::now();

        let mut c = Commitment::new(CommitmentKind::Intent, "let's try X", Source::Manual);
        c.made_at = now - Duration::hours(5);
        store.insert_commitment(&c).unwrap();
        transition(&mut c, State::Abandoned, None).unwrap();
        store.update_state(c.id, c.state, None).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.resolved.len(), 1);
        assert_eq!(brief.resolved[0].state, State::Abandoned);
        assert_eq!(brief.resolved[0].polarity, None);
    }

    #[test]
    fn resolved_window_excludes_old_rows() {
        let mut store = fresh_store();
        let now = Utc::now();
        let cfg = BriefConfig {
            resolved_window: Duration::days(7),
            ..Default::default()
        };

        // Recent — included.
        let mut recent = Commitment::new(CommitmentKind::Intent, "recent", Source::Manual);
        recent.made_at = now - Duration::days(2);
        store.insert_commitment(&recent).unwrap();
        transition(&mut recent, State::Abandoned, None).unwrap();
        store.update_state(recent.id, recent.state, None).unwrap();

        // Old — excluded.
        let mut old = Commitment::new(CommitmentKind::Intent, "old", Source::Manual);
        old.made_at = now - Duration::days(60);
        store.insert_commitment(&old).unwrap();
        transition(&mut old, State::Abandoned, None).unwrap();
        store.update_state(old.id, old.state, None).unwrap();

        // sanity: regenerate brief with reduced cfg
        let brief = BriefBuilder::new(&store).with_config(cfg).build(now).unwrap();
        assert_eq!(brief.resolved.len(), 1);
        assert_eq!(brief.resolved[0].id, recent.id);

        // silence unused-mut warnings on `store` when the test compiles
        let _ = &mut store;
    }

    #[test]
    fn candidates_section_lists_pending_only() {
        use tm_intent::CandidateRecord;
        let mut store = fresh_store();
        let now = Utc::now();

        let cands: Vec<_> = tm_intent::mine("I'll ship Friday. We decided to use postgres.")
            .iter()
            .map(|m| CandidateRecord::from_mined(m, "I'll ship Friday. We decided to use postgres.".to_string()))
            .collect();
        assert_eq!(cands.len(), 2);
        store.insert_candidates(&cands).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.candidates.len(), 2);

        // Dismiss one — only the other remains.
        store.dismiss_candidate(cands[0].id).unwrap();
        let brief2 = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief2.candidates.len(), 1);
        assert_ne!(brief2.candidates[0].id, cands[0].id);
    }

    #[test]
    fn patterns_surface_when_a_cell_diverges_from_baseline() {
        // Six high-stakes morning commitments — all `Worse`. Plus
        // twelve medium-stakes afternoon commitments — all
        // `AsExpected`. Global p_worse ≈ 0.33; cell p_worse = 1.0;
        // lift = 0.67 ≫ 0.25.
        let store = fresh_store();
        let now = Utc::now();
        use chrono::TimeZone;

        for i in 0..6 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-deploy-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::High;
            c.tags = vec!["deploy".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap();
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::Worse,
                "regressed",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store.update_state(after.id, after.state, after.outcome_id).unwrap();
        }
        for i in 0..12 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("afternoon-research-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Medium;
            c.tags = vec!["research".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 1, 14, 0, 0).unwrap();
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::AsExpected,
                "ok",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store.update_state(after.id, after.state, after.outcome_id).unwrap();
        }

        // Use a wide patterns_window so the made_at timestamps fall
        // inside the lookback even though they're a few weeks old
        // relative to `now`.
        let cfg = BriefConfig {
            patterns_window: Duration::days(365 * 2),
            ..Default::default()
        };
        let brief = BriefBuilder::new(&store)
            .with_config(cfg)
            .build(now)
            .unwrap();

        assert!(
            !brief.patterns.is_empty(),
            "expected at least one pattern row; got {}",
            brief.counts.patterns
        );
        let p = &brief.patterns[0];
        assert!(p.lift_worse > 0.5, "lift_worse = {}", p.lift_worse);
        assert!(p.render.contains("worse"));
    }

    #[test]
    fn silenced_cell_is_filtered_from_brief_patterns() {
        // Same setup as the surface-test, but silence the
        // high-stakes morning cell first and confirm patterns drop.
        let store = fresh_store();
        let now = Utc::now();
        use chrono::TimeZone;
        for i in 0..6 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-deploy-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::High;
            c.tags = vec!["deploy".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap();
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(c.id, Polarity::Worse, "regressed", OutcomeSource::UserPrompted);
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store.update_state(after.id, after.state, after.outcome_id).unwrap();
        }
        for i in 0..12 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("afternoon-research-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Medium;
            c.tags = vec!["research".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 1, 14, 0, 0).unwrap();
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(c.id, Polarity::AsExpected, "ok", OutcomeSource::UserPrompted);
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store.update_state(after.id, after.state, after.outcome_id).unwrap();
        }
        let cfg = BriefConfig {
            patterns_window: Duration::days(365 * 2),
            ..Default::default()
        };

        // Find the cell hash from the first run, then silence it.
        let brief = BriefBuilder::new(&store).with_config(cfg.clone()).build(now).unwrap();
        assert!(!brief.patterns.is_empty());
        let cell_hash = brief.patterns[0].cell_hash.clone();
        let cell_label = brief.patterns[0].cell.label();

        store
            .upsert_pattern_silence(
                &cell_hash,
                &cell_label,
                now,
                now + Duration::days(90),
                Some("noisy"),
            )
            .unwrap();

        // Re-run — pattern should be filtered out.
        let brief2 = BriefBuilder::new(&store).with_config(cfg.clone()).build(now).unwrap();
        assert!(brief2.patterns.is_empty(), "silenced cell should be filtered");

        // Unsilence — pattern returns.
        let removed = store.remove_pattern_silence(&cell_hash).unwrap();
        assert!(removed);
        let brief3 = BriefBuilder::new(&store).with_config(cfg).build(now).unwrap();
        assert!(!brief3.patterns.is_empty());
    }

    #[test]
    fn expired_silence_does_not_filter_patterns() {
        let store = fresh_store();
        let now = Utc::now();
        // Silence already expired by `now`.
        store
            .upsert_pattern_silence(
                "deadbeefdeadbeef",
                "stale label",
                now - Duration::days(120),
                now - Duration::days(30),
                None,
            )
            .unwrap();
        let active = store.list_active_pattern_silences(now).unwrap();
        assert!(active.is_empty(), "expired silence should not be active");
    }

    #[test]
    fn brief_serializes_to_json_and_back() {
        let store = fresh_store();
        let now = Utc::now();
        let mut c = Commitment::new(CommitmentKind::Decision, "use bcrypt", Source::Manual);
        c.horizon = Some(now - Duration::hours(1));
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        let json = serde_json::to_string(&brief).expect("brief serializes");
        // ensure key fields survive a roundtrip
        let back: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back["counts"]["overdue"], 1);
        assert_eq!(back["overdue"][0]["overdue_class"], "due_today");
    }
}
