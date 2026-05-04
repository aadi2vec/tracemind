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

use crate::insights::{detect_insights, BaselineRate, InsightBriefRow, InsightConfig};
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
    /// Cap on outcome proposals surfaced. Default 10 — small because
    /// the brief is meant to show *attention-worthy* proposals, not a
    /// flood. The full list lives in `tracemind outcomes list`.
    pub limit_proposals: usize,
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
    /// Insight-detector config (`crate::insights`). Controls which
    /// open commitments get hoisted into the focused
    /// `▸ insights` panel based on world-model deviation from the
    /// completed-rate baseline.
    pub insights: InsightConfig,
    /// Trust gate on the world model's calibration. When the gate
    /// fails we suppress the insights panel and emit
    /// [`DailyBrief::model_quiet`] explaining why. Closes the loop
    /// opened by TM-INTENT-008 (read-only calibration view) per
    /// `INTENT_SYSTEM.md` §11 ("auto-quiet on bad accuracy").
    pub insight_gate: InsightGateConfig,
}

/// Calibration thresholds the brief uses to decide whether the world
/// model is trustworthy enough to surface insights.
///
/// All three checks are AND-gated — failing any one drops the panel
/// for this brief. Floors are deliberately conservative; we'd rather
/// stay quiet than mislead.
#[derive(Debug, Clone)]
pub struct InsightGateConfig {
    /// Minimum out-of-sample evaluations required before the gate
    /// will even consider opening. Below this we don't have enough
    /// signal to call the model trustworthy *or* untrustworthy — we
    /// stay quiet and say so. Default: 8 (one work-week of resolved
    /// commitments at typical pace).
    pub min_n_evaluated: usize,
    /// `accuracy` floor. Uniform-prediction baseline is 0.25 (4
    /// classes). Default: 0.35 — meaningfully above uniform without
    /// demanding heroic performance from the v0 linear model.
    pub accuracy_floor: f32,
    /// `warning_precision` floor. The trust knob on the L2 warning
    /// surface (`crate::insights`): when the model said "watch out,"
    /// how often was it right? Below 0.5 we're worse than a coin
    /// flip — pull the panel. Default: 0.5.
    pub warning_precision_floor: f32,
}

impl Default for InsightGateConfig {
    fn default() -> Self {
        Self {
            min_n_evaluated: 8,
            accuracy_floor: 0.35,
            warning_precision_floor: 0.5,
        }
    }
}

/// Why the brief pulled the insights panel. `None` on `DailyBrief`
/// means the gate either passed or never engaged (no model attached).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelQuietReason {
    /// We don't have enough resolved-after-train commitments to
    /// compute a calibration the user can trust. The brief surface
    /// renders "▸ model warming up — N of M completions evaluated".
    InsufficientEvaluations { n_evaluated: usize, required: usize },
    /// Top-1 accuracy fell below the floor.
    LowAccuracy { accuracy: f32, floor: f32, n_evaluated: usize },
    /// Warning precision fell below the floor — the model's "watch
    /// out" signals are noisier than coin flips.
    LowWarningPrecision { warning_precision: f32, floor: f32, n_evaluated: usize },
}

impl Default for BriefConfig {
    fn default() -> Self {
        Self {
            resolved_window: Duration::days(7),
            limit_open: 20,
            limit_overdue: 20,
            limit_resolved: 10,
            limit_candidates: 10,
            limit_proposals: 10,
            patterns: PatternConfig::default(),
            patterns_window: Duration::days(365),
            patterns_scan_limit: 2000,
            insights: InsightConfig::default(),
            insight_gate: InsightGateConfig::default(),
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
    /// World-model outlook — populated only when the brief builder was
    /// given a trained model with ≥6 priors. INTENT_SYSTEM.md §6.2.
    /// Stays `None` for the cold-start case (no model, dormant model,
    /// under-supported) so the brief is honest about uncertainty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlook: Option<CommitmentOutlook>,
}

/// Slim, JSON-friendly projection of [`tm_world_model::OutcomePrediction`]
/// for the brief surface. We carry only what the brief renders + the
/// `tone` agents route on (mirrors the MCP preflight schema in
/// `memory_commit` so external consumers see the same shape twice).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommitmentOutlook {
    pub n_priors: usize,
    pub argmax: String, // "better" / "as_expected" / "worse" / "mixed"
    pub positive_prob: f32,
    pub confidence: f32,
    pub better: f32,
    pub as_expected: f32,
    pub worse: f32,
    pub mixed: f32,
    pub tone: String, // "warning" | "tailwind" | "mixed"
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

/// One persisted outcome proposal surfaced in the brief — the
/// implicit text matcher saw a fresh capture that may have
/// described what happened to an open commitment, and the user
/// hasn't accepted/dismissed it yet. INTENT_SYSTEM.md §4.2 +
/// TM-INTENT-009. The brief uses this to ask "what happened with
/// X?" days after the capture moment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeProposalBriefRow {
    /// Proposal UUID — pass to `tracemind outcomes accept|dismiss`.
    pub id: Uuid,
    pub commitment_id: Uuid,
    /// Snapshot of the open commitment's statement so the brief
    /// can render the row without a second store hit.
    pub commitment_statement: String,
    pub proposed_polarity: Polarity,
    /// Short human description of *what fired the match* — usually
    /// the first ~80 chars of the originating capture text.
    pub description: String,
    /// 0.0 .. 1.0 — token-Jaccard score from the matcher.
    pub similarity: f32,
    pub proposed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
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
    /// Per-row world-model insights — open commitments whose outlook
    /// diverges most from the user's completed-rate baseline. Sorted
    /// by `|delta|` desc. Empty when no world model is attached, or
    /// when no row crosses the `min_abs_delta` floor. INTENT_SYSTEM
    /// §6.2 generalised to row-level "what's surprising".
    #[serde(default)]
    pub insights: Vec<InsightBriefRow>,
    /// Persistent outcome proposals — the implicit text matcher
    /// suspected a fresh capture closed an open commitment, and the
    /// user hasn't acted yet. Newest first. Empty when nothing is
    /// pending. TM-INTENT-009.
    #[serde(default)]
    pub proposals: Vec<OutcomeProposalBriefRow>,
    /// Section counts for quick rendering of section headers.
    pub counts: BriefCounts,
    /// Set when the calibration gate suppressed the insights panel.
    /// When this is `Some`, `insights` is empty by construction —
    /// surfaces should render a neutral banner explaining that the
    /// model is being quiet, *not* hide the fact that it has an
    /// opinion. TM-INTENT-010 / `INTENT_SYSTEM.md` §11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_quiet: Option<ModelQuietReason>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BriefCounts {
    pub overdue: usize,
    pub open: usize,
    pub resolved: usize,
    pub candidates: usize,
    pub patterns: usize,
    #[serde(default)]
    pub insights: usize,
    #[serde(default)]
    pub proposals: usize,
}

pub struct BriefBuilder<'a> {
    store: &'a IntentStore,
    config: BriefConfig,
    /// Optional world-model for outlook attachment. When `None`, every
    /// row's `outlook` stays `None` (cold-start surface). When `Some`
    /// but the model is dormant (`!is_trained()`) or under-supported
    /// (`n_train_examples < MIN_PRIORS_FOR_OUTLOOK`), we still skip
    /// attachment — same n-too-small cliff as CLI/MCP preflight.
    world_model: Option<&'a tm_world_model::OutcomeModel>,
}

impl<'a> BriefBuilder<'a> {
    pub fn new(store: &'a IntentStore) -> Self {
        Self {
            store,
            config: BriefConfig::default(),
            world_model: None,
        }
    }

    pub fn with_config(mut self, config: BriefConfig) -> Self {
        self.config = config;
        self
    }

    /// Attach a trained world model. Outlook fields stay `None` unless
    /// the model is trained AND has ≥ `MIN_PRIORS_FOR_OUTLOOK` priors.
    pub fn with_world_model(mut self, model: &'a tm_world_model::OutcomeModel) -> Self {
        self.world_model = Some(model);
        self
    }

    /// Build the brief as of `now`. Pure: no internal clock reads.
    pub fn build(&self, now: DateTime<Utc>) -> Result<DailyBrief, BriefError> {
        let cfg = &self.config;

        // World-model gate: only surface outlooks when the model is
        // actually trained and has enough priors to be honest. Same
        // threshold as `tm-cli` preflight + `tm-mcp` `memory_commit`
        // preflight so all three surfaces stay aligned.
        let active_model: Option<&tm_world_model::OutcomeModel> =
            self.world_model.filter(|m| {
                m.is_trained() && m.n_train_examples >= MIN_PRIORS_FOR_OUTLOOK
            });

        // Overdue first — sorted by horizon ASC by the store query.
        let overdue_raw = self.store.list_overdue_open(now, cfg.limit_overdue)?;
        let overdue_ids: std::collections::HashSet<Uuid> =
            overdue_raw.iter().map(|c| c.id).collect();

        let overdue: Vec<CommitmentBriefRow> = overdue_raw
            .into_iter()
            .map(|c| commitment_row(c, now, /* tag_overdue */ true, active_model))
            .collect();

        // Open list — drop anything already shown in `overdue` so a
        // commitment never double-renders.
        let open_raw = self.store.list_open(cfg.limit_open + cfg.limit_overdue)?;
        let open: Vec<CommitmentBriefRow> = open_raw
            .into_iter()
            .filter(|c| !overdue_ids.contains(&c.id))
            .take(cfg.limit_open)
            .map(|c| commitment_row(c, now, /* tag_overdue */ false, active_model))
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

        // Calibration gate (TM-INTENT-010). Evaluate the world model
        // against its own out-of-sample completions; if the gate
        // fails, drop the insights panel for this brief and tell the
        // caller why via `model_quiet`. Soft-fail: any store error
        // just leaves the gate open (matches the rest of the brief's
        // best-effort posture).
        let model_quiet: Option<ModelQuietReason> = match active_model {
            Some(model) => evaluate_insight_gate(
                self.store,
                model,
                &cfg.insight_gate,
                pattern_window_start,
                cfg.patterns_scan_limit,
            ),
            None => None,
        };

        // Insights — only compute when a world model is attached
        // (otherwise the open rows have no `outlook` to compare).
        // Pull the same completed-with-polarity slice as the pattern
        // detector to stay consistent on what counts as a "prior".
        // Soft-fail: if the store query errors we drop insights but
        // keep the rest of the brief.
        // The calibration gate above can also force-drop the panel —
        // we honour it by short-circuiting here.
        let insights: Vec<InsightBriefRow> = if active_model.is_some() && model_quiet.is_none() {
            // Active per-commitment silences — same soft-fail
            // semantics as `pattern_silences`: if the read errors we
            // drop insights for this brief rather than block.
            let insight_silenced: std::collections::HashSet<uuid::Uuid> = self
                .store
                .list_active_insight_silences(now)
                .map(|rows| rows.into_iter().map(|s| s.commitment_id).collect())
                .unwrap_or_default();
            match self
                .store
                .list_completed_with_polarity(pattern_window_start, cfg.patterns_scan_limit)
            {
                Ok(completed) => {
                    let baseline = BaselineRate::from_completed(&completed);
                    // Insights apply to overdue + open together — the
                    // overdue rows are the most attention-worthy ones,
                    // we shouldn't filter them out of the panel.
                    // `detect_insights` takes a slice by reference, so
                    // collect both sections into one owned Vec.
                    let combined: Vec<CommitmentBriefRow> = overdue
                        .iter()
                        .chain(open.iter())
                        .cloned()
                        .collect();
                    detect_insights(&combined, baseline, &cfg.insights)
                        .into_iter()
                        .filter(|row| !insight_silenced.contains(&row.commitment_id))
                        .collect()
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Persisted outcome proposals (TM-INTENT-009 / §4.2). Best-
        // effort flush of stale rows first so the brief's view stays
        // accurate even when no nightly job runs. Soft-fail: if the
        // proposals query errors we drop the section rather than
        // block the rest of the brief.
        let _ = self.store.expire_outcome_proposals(now);
        let proposals: Vec<OutcomeProposalBriefRow> = match self
            .store
            .list_active_outcome_proposals(now, cfg.limit_proposals)
        {
            Ok(rows) => rows
                .into_iter()
                .filter_map(|p| {
                    // Snapshot the commitment statement so the brief
                    // row is self-contained. If the commitment was
                    // deleted out from under us, drop the proposal —
                    // can't render it usefully.
                    let stmt = self
                        .store
                        .get_commitment(p.commitment_id)
                        .ok()
                        .flatten()
                        .map(|c| c.statement)?;
                    Some(OutcomeProposalBriefRow {
                        id: p.id,
                        commitment_id: p.commitment_id,
                        commitment_statement: stmt,
                        proposed_polarity: p.proposed_polarity,
                        description: p.description,
                        similarity: p.similarity,
                        proposed_at: p.proposed_at,
                        expires_at: p.expires_at,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        let counts = BriefCounts {
            overdue: overdue.len(),
            open: open.len(),
            resolved: resolved.len(),
            candidates: candidates.len(),
            patterns: patterns.len(),
            insights: insights.len(),
            proposals: proposals.len(),
        };

        Ok(DailyBrief {
            generated_at: now,
            overdue,
            open,
            resolved,
            candidates,
            patterns,
            insights,
            proposals,
            counts,
            model_quiet,
        })
    }
}

/// Run a calibration evaluation of `model` against its own
/// out-of-sample completions and decide whether the insights panel
/// should fire. Returns:
/// - `None` → gate passed; surface insights normally.
/// - `Some(reason)` → gate failed; suppress insights and let the
///   surface render a "model is being quiet" banner.
///
/// Soft-fail: any store error returns `None` (open the gate). The
/// rest of the brief stays best-effort; we don't punish the user for
/// a flaky read by suppressing their insights.
fn evaluate_insight_gate(
    store: &IntentStore,
    model: &tm_world_model::OutcomeModel,
    cfg: &InsightGateConfig,
    window_start: DateTime<Utc>,
    scan_limit: usize,
) -> Option<ModelQuietReason> {
    let pairs = match store.list_completed_with_outcome_meta(window_start, scan_limit) {
        Ok(p) => p,
        Err(_) => return None,
    };
    let (oos, _in_sample_skipped) = tm_world_model::split_out_of_sample(model, pairs);
    let report = tm_world_model::evaluate(model, &oos);
    let n = report.n_evaluated;
    if n < cfg.min_n_evaluated {
        return Some(ModelQuietReason::InsufficientEvaluations {
            n_evaluated: n,
            required: cfg.min_n_evaluated,
        });
    }
    if report.accuracy < cfg.accuracy_floor {
        return Some(ModelQuietReason::LowAccuracy {
            accuracy: report.accuracy,
            floor: cfg.accuracy_floor,
            n_evaluated: n,
        });
    }
    // Warning-precision is only evidence when the model actually
    // issued warnings during evaluation. `warning_precision = 0.0`
    // with `n_warning = 0` means "no signal", not "untrustworthy" —
    // don't punish the user for a quiet-but-correct model.
    if report.n_warning > 0 && report.warning_precision < cfg.warning_precision_floor {
        return Some(ModelQuietReason::LowWarningPrecision {
            warning_precision: report.warning_precision,
            floor: cfg.warning_precision_floor,
            n_evaluated: n,
        });
    }
    None
}

fn commitment_row(
    c: Commitment,
    now: DateTime<Utc>,
    tag_overdue: bool,
    model: Option<&tm_world_model::OutcomeModel>,
) -> CommitmentBriefRow {
    let overdue_class = if tag_overdue {
        c.horizon.map(|h| OverdueClass::classify(h, now))
    } else {
        None
    };
    let outlook = model.map(|m| {
        let pred = m.predict(&c);
        outlook_from_prediction(&pred, m.n_train_examples)
    });
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
        outlook,
    }
}

/// Minimum prior commitments required before the world model is
/// allowed to surface predictions in the brief. Mirrors the threshold
/// in `tm-cli` preflight + `tm-mcp` `memory_commit` preflight so all
/// three surfaces stay honest about the same "n is too small" cliff.
const MIN_PRIORS_FOR_OUTLOOK: usize = 6;

/// Map a [`tm_world_model::OutcomePrediction`] into the brief's
/// `CommitmentOutlook`. Tone routing matches the MCP preflight in
/// `tm-mcp::build_preflight_for` exactly: callers that key off `tone`
/// see one shape across CLI brief, MCP brief and MCP commit responses.
fn outlook_from_prediction(
    pred: &tm_world_model::OutcomePrediction,
    n_priors: usize,
) -> CommitmentOutlook {
    use tm_world_model::PolarityClass as P;
    let positive = pred.positive_prob;
    let tone = if matches!(pred.argmax, P::Worse) || positive < 0.40 {
        "warning"
    } else if positive > 0.65 {
        "tailwind"
    } else {
        "mixed"
    };
    let d = &pred.dist.0;
    CommitmentOutlook {
        n_priors,
        argmax: pred.argmax.label().to_string(),
        positive_prob: positive,
        confidence: pred.confidence,
        better: d[P::Better.index()],
        as_expected: d[P::AsExpected.index()],
        worse: d[P::Worse.index()],
        mixed: d[P::Mixed.index()],
        tone: tone.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
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

    /// Build a small linearly-separable training set using the same
    /// strategy as `tm-world-model`'s own tests: high-stakes evenings
    /// trend Worse, low-stakes mornings trend Better. We use this to
    /// fabricate a model the brief can then consult.
    fn trained_model() -> tm_world_model::OutcomeModel {
        use chrono::TimeZone;
        use tm_world_model::{train, Example, PolarityClass, TrainerConfig};
        let mk = |stakes: Stakes, hour: u32, target: PolarityClass| {
            let mut c = Commitment::new(CommitmentKind::Intent, "seed", Source::Cli);
            c.stakes = stakes;
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 28, hour, 0, 0).unwrap();
            Example { commitment: c, target }
        };
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(mk(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..6 {
            exs.push(mk(Stakes::Low, 9, PolarityClass::Better));
        }
        let (model, _) = train(&exs, &TrainerConfig::default());
        assert!(model.is_trained(), "fixture must produce a trained model");
        model
    }

    #[test]
    fn outlook_omitted_when_no_world_model_attached() {
        let store = fresh_store();
        let now = Utc::now();
        let mut c = Commitment::new(CommitmentKind::Intent, "draft outline", Source::Manual);
        c.horizon = Some(now + Duration::days(2));
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert_eq!(brief.open.len(), 1);
        assert!(brief.open[0].outlook.is_none(), "no model attached → no outlook");
    }

    #[test]
    fn outlook_omitted_when_model_below_min_priors() {
        // A "trained" model that lies about being trained but only saw
        // 1 example: BriefBuilder must still refuse to surface predictions.
        // We synthesize this by hand-constructing a model and clamping
        // its example count below the threshold.
        use tm_world_model::{OutcomeModel, TagVocab};
        let store = fresh_store();
        let now = Utc::now();
        let mut c = Commitment::new(CommitmentKind::Intent, "draft outline", Source::Manual);
        c.horizon = Some(now + Duration::days(2));
        store.insert_commitment(&c).unwrap();

        let mut model = OutcomeModel::fresh(TagVocab::default());
        model.n_train_examples = MIN_PRIORS_FOR_OUTLOOK - 1; // dormant by gate
        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(
            brief.open[0].outlook.is_none(),
            "model under min priors → outlook stays None even when attached"
        );
    }

    #[test]
    fn outlook_attaches_warning_tone_for_worse_trending_open_row() {
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();

        // Open commitment that *matches* the worse-trending cell
        // (high stakes, evening hour, no horizon → not overdue).
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc::now().with_timezone(&Utc);
        // Match the trained pattern by shifting hour into the evening.
        // BriefBuilder pulls c.made_at as-is so we set explicit hour.
        c.made_at = chrono::Utc
            .with_ymd_and_hms(2026, 4, 30, 19, 0, 0)
            .unwrap();
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert_eq!(brief.open.len(), 1);
        let outlook = brief.open[0]
            .outlook
            .as_ref()
            .expect("outlook attached when model trained + above min priors");
        assert_eq!(outlook.argmax, "worse");
        assert_eq!(outlook.tone, "warning");
        assert!(outlook.n_priors >= MIN_PRIORS_FOR_OUTLOOK);
        // Probabilities sum ~1.0
        let total = outlook.better + outlook.as_expected + outlook.worse + outlook.mixed;
        assert!((total - 1.0).abs() < 1e-3, "probs sum to ~1, got {total}");
    }

    #[test]
    fn outlook_attaches_tailwind_tone_for_better_trending_open_row() {
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();

        // Low-stakes morning commitment matches the Better cell.
        let mut c = Commitment::new(CommitmentKind::Intent, "draft note", Source::Manual);
        c.stakes = Stakes::Low;
        c.made_at = chrono::Utc
            .with_ymd_and_hms(2026, 4, 30, 9, 0, 0)
            .unwrap();
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        let outlook = brief.open[0].outlook.as_ref().expect("outlook attached");
        assert_eq!(outlook.argmax, "better");
        assert_eq!(outlook.tone, "tailwind");
        assert!(outlook.positive_prob > 0.65);
    }

    #[test]
    fn insights_surface_when_open_row_diverges_from_baseline() {
        // Seed a baseline of 8 Better outcomes (high positive rate),
        // then create one open commitment whose features (high
        // stakes, vendor tag) trigger a Worse-leaning model
        // prediction. The resulting delta should hoist this row into
        // the insights panel.
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();

        // 8 historical Better outcomes establish a strong positive
        // baseline (~80%+). They use a *different* shape than the
        // open row so the cell-level pattern detector won't conflate.
        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-task-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Low;
            c.tags = vec!["writing".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 15, 9, 0, 0).unwrap()
                + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::Better,
                "shipped",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }

        // Open row: high-stakes evening — model trained on the
        // separable set predicts Worse, so positive_prob is small,
        // delta vs the rosy baseline is large negative.
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();

        assert!(
            !brief.insights.is_empty(),
            "open row well below baseline should appear in insights, got {brief:?}"
        );
        let row = &brief.insights[0];
        assert_eq!(row.tone, "warning");
        assert!(row.delta < 0.0);
        assert_eq!(brief.counts.insights, brief.insights.len());
    }

    #[test]
    fn insights_omitted_when_no_world_model_attached() {
        let store = fresh_store();
        let now = Utc::now();
        // Even with a stack of completed commitments, no model = no
        // insights — the panel exists strictly to surface model
        // predictions, not raw cell statistics (that's `patterns`).
        for i in 0..10 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("seed-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Low;
            c.made_at = Utc::now() - Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(c.id, Polarity::Better, "ok", OutcomeSource::UserPrompted);
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }
        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert!(brief.insights.is_empty(), "no model → no insights");
        assert_eq!(brief.counts.insights, 0);
    }

    #[test]
    fn silenced_commitment_is_filtered_from_brief_insights() {
        // Reproduce the same setup as
        // `insights_surface_when_open_row_diverges_from_baseline`,
        // then silence the open commitment and confirm the insight
        // disappears while the row itself is still present.
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();

        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-task-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Low;
            c.tags = vec!["writing".into()];
            c.made_at =
                Utc.with_ymd_and_hms(2026, 4, 15, 9, 0, 0).unwrap() + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::Better,
                "shipped",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }

        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        let cid = c.id;
        store.insert_commitment(&c).unwrap();

        // Sanity: without a silence we still see the warning insight.
        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(!brief.insights.is_empty(), "pre-silence: insight should surface");
        assert!(
            brief.open.iter().any(|r| r.id == cid),
            "open row must still be present"
        );

        // Silence the commitment for 30 days.
        store
            .upsert_insight_silence(cid, now, now + chrono::Duration::days(30), Some("noted"))
            .unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(
            brief.insights.is_empty(),
            "post-silence: insight panel must be empty, got {brief:?}"
        );
        assert_eq!(brief.counts.insights, 0);
        // The underlying open row must NOT disappear — silence is on
        // the *insight surface*, not the commitment itself.
        assert!(
            brief.open.iter().any(|r| r.id == cid),
            "silencing the insight must NOT remove the open row"
        );
    }

    #[test]
    fn expired_insight_silence_resurfaces_insight() {
        // A silence with `silenced_until` in the past must NOT
        // suppress the insight — same correctness contract as
        // pattern silences (TTL-based, not permanent).
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();

        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-task-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Low;
            c.tags = vec!["writing".into()];
            c.made_at =
                Utc.with_ymd_and_hms(2026, 4, 15, 9, 0, 0).unwrap() + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::Better,
                "shipped",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }

        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        let cid = c.id;
        store.insert_commitment(&c).unwrap();

        // Silence already in the past.
        store
            .upsert_insight_silence(
                cid,
                now - chrono::Duration::days(60),
                now - chrono::Duration::days(1),
                None,
            )
            .unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(
            !brief.insights.is_empty(),
            "expired silence must not suppress the insight"
        );
    }

    /// Helper to seed a "diverging open row" scenario: 8 Better
    /// completions at the model's sweet spot and one open commitment
    /// at the model's Worse-shape (high stakes, evening). With a
    /// passing gate the brief surfaces an insight; we use this in
    /// the gate tests to assert that suppression is what's flipping
    /// the panel state.
    fn seed_diverging_scenario(store: &IntentStore) -> uuid::Uuid {
        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("morning-task-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::Low;
            c.tags = vec!["writing".into()];
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 15, 9, 0, 0).unwrap()
                + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(
                c.id,
                Polarity::Better,
                "shipped",
                OutcomeSource::UserPrompted,
            );
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        let cid = c.id;
        store.insert_commitment(&c).unwrap();
        cid
    }

    #[test]
    fn gate_quiets_brief_when_n_evaluated_below_floor() {
        // Stand up a model trained from data that has *not* been
        // resolved-after-train, so n_evaluated == 0. Even when the
        // open row would otherwise produce a fat insight, the panel
        // must stay empty and the brief must explain why.
        let model = trained_model();
        let store = fresh_store();
        // Single open row, no completions in the store at all → the
        // calibration evaluator gets zero pairs.
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        store.insert_commitment(&c).unwrap();

        let now = Utc::now();
        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(brief.insights.is_empty(), "thin-N must suppress insights");
        match brief.model_quiet {
            Some(ModelQuietReason::InsufficientEvaluations { n_evaluated, required }) => {
                assert_eq!(n_evaluated, 0);
                assert_eq!(required, InsightGateConfig::default().min_n_evaluated);
            }
            other => panic!("expected InsufficientEvaluations, got {other:?}"),
        }
    }

    #[test]
    fn gate_passes_when_model_aligns_with_completions() {
        // Seed completions at the model's Better-shape (Stakes::Low,
        // hour 9). Out-of-sample evaluation should give high
        // accuracy and zero warnings issued — the warning_precision
        // floor is correctly ignored when there's no evidence to
        // judge it on. Insights stay surfaced.
        let model = trained_model();
        let store = fresh_store();
        let _cid = seed_diverging_scenario(&store);
        let now = Utc::now();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(
            brief.model_quiet.is_none(),
            "well-aligned model should not auto-quiet, got {:?}",
            brief.model_quiet
        );
        assert!(
            !brief.insights.is_empty(),
            "well-aligned model should still surface insights"
        );
    }

    #[test]
    fn gate_quiets_brief_when_accuracy_below_floor() {
        // Seed 8 *adversarial* completions — Stakes::High/hour 19
        // (where the trained model predicts Worse) but actual
        // polarity Better. Model is wrong on every row → accuracy
        // = 0.0, well below the default floor. Insights must drop
        // with a `LowAccuracy` reason.
        let model = trained_model();
        let store = fresh_store();
        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("adversarial-{i}"),
                Source::Manual,
            );
            // Model's Worse-shape ...
            c.stakes = Stakes::High;
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 15, 19, 0, 0).unwrap()
                + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            // ... but actual outcome is Better. Model is wrong.
            let o = Outcome::new(c.id, Polarity::Better, "ok", OutcomeSource::UserPrompted);
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }
        // Plus an open row that would otherwise be the insight.
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        store.insert_commitment(&c).unwrap();

        let now = Utc::now();
        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        assert!(brief.insights.is_empty(), "accuracy gate must drop insights");
        match brief.model_quiet {
            Some(ModelQuietReason::LowAccuracy { accuracy, floor, n_evaluated }) => {
                assert!(accuracy < floor, "accuracy {accuracy} must be below floor {floor}");
                assert!(n_evaluated >= 8);
            }
            other => panic!("expected LowAccuracy, got {other:?}"),
        }
    }

    #[test]
    fn gate_quiets_brief_when_warning_precision_below_floor() {
        // Seed 8 commitments at the model's Worse-shape, but with
        // *actual* polarity Better. The model fires a warning on
        // each (positive_prob < 0.5) but the actual outcome is
        // positive — warning_precision = 0.0, n_warning = 8.
        // Even though accuracy might cross the default floor on
        // some rows, warning_precision must trip the gate.
        //
        // We crank `accuracy_floor` to 0.0 to isolate the
        // warning-precision check.
        let model = trained_model();
        let store = fresh_store();
        for i in 0..8 {
            let mut c = Commitment::new(
                CommitmentKind::Intent,
                format!("noisy-warn-{i}"),
                Source::Manual,
            );
            c.stakes = Stakes::High; // model predicts Worse → warns
            c.made_at = Utc.with_ymd_and_hms(2026, 4, 15, 19, 0, 0).unwrap()
                + Duration::days(i);
            store.insert_commitment(&c).unwrap();
            let o = Outcome::new(c.id, Polarity::Better, "ok", OutcomeSource::UserPrompted);
            store.insert_outcome(&o).unwrap();
            let mut after = c.clone();
            transition(&mut after, State::Completed, Some(&o)).unwrap();
            store
                .update_state(after.id, after.state, after.outcome_id)
                .unwrap();
        }
        let mut c = Commitment::new(CommitmentKind::Intent, "ship hot fix", Source::Manual);
        c.stakes = Stakes::High;
        c.made_at = Utc.with_ymd_and_hms(2026, 4, 30, 19, 0, 0).unwrap();
        store.insert_commitment(&c).unwrap();

        let cfg = BriefConfig {
            insight_gate: InsightGateConfig {
                min_n_evaluated: 1,
                accuracy_floor: 0.0,
                warning_precision_floor: 0.5,
            },
            ..BriefConfig::default()
        };
        let now = Utc::now();
        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .with_config(cfg)
            .build(now)
            .unwrap();
        assert!(brief.insights.is_empty(), "warning_precision gate must drop insights");
        match brief.model_quiet {
            Some(ModelQuietReason::LowWarningPrecision {
                warning_precision,
                floor,
                n_evaluated,
            }) => {
                assert!(warning_precision < floor);
                assert!(n_evaluated >= 8);
            }
            other => panic!("expected LowWarningPrecision, got {other:?}"),
        }
    }

    #[test]
    fn gate_does_not_engage_without_world_model() {
        // No model attached → no calibration, no quiet-reason. The
        // gate should never engage on the cold-start surface.
        let store = fresh_store();
        let _cid = seed_diverging_scenario(&store);
        let now = Utc::now();
        let brief = BriefBuilder::new(&store).build(now).unwrap();
        assert!(brief.insights.is_empty(), "no model → no insights");
        assert!(brief.model_quiet.is_none(), "no model → no quiet reason");
    }

    #[test]
    fn quiet_reason_round_trips_json() {
        let r = ModelQuietReason::LowAccuracy {
            accuracy: 0.18,
            floor: 0.35,
            n_evaluated: 12,
        };
        let s = serde_json::to_string(&r).unwrap();
        // Tag-driven discriminator so MCP / Tauri consumers can route.
        assert!(s.contains("\"kind\":\"low_accuracy\""), "tagged: {s}");
        let back: ModelQuietReason = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn outlook_serializes_into_brief_json() {
        let model = trained_model();
        let store = fresh_store();
        let now = Utc::now();
        let mut c = Commitment::new(CommitmentKind::Intent, "draft note", Source::Manual);
        c.stakes = Stakes::Low;
        c.made_at = chrono::Utc
            .with_ymd_and_hms(2026, 4, 30, 9, 0, 0)
            .unwrap();
        store.insert_commitment(&c).unwrap();

        let brief = BriefBuilder::new(&store)
            .with_world_model(&model)
            .build(now)
            .unwrap();
        let json = serde_json::to_value(&brief).unwrap();
        let row = &json["open"][0];
        assert!(row.get("outlook").is_some(), "outlook field present in JSON");
        assert_eq!(row["outlook"]["argmax"], "better");
        assert_eq!(row["outlook"]["tone"], "tailwind");
    }
}
