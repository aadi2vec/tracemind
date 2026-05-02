//! Per-row world-model insights for the daily brief.
//!
//! The pattern detector in [`crate::pattern`] surfaces *cell-level*
//! divergences (e.g. "high-stakes evening commits trend Worse 38pp
//! above baseline"). That's structural — it tells you about a class
//! of decisions.
//!
//! This module surfaces *row-level* divergences from the world model.
//! For each open commitment whose outlook has been computed, we
//! compare the model's `positive_prob` (better ∪ as_expected) against
//! the user's overall completed-rate baseline. Rows where the delta
//! is large get hoisted into a focused `▸ insights` panel.
//!
//! This turns the brief from "20 outlook lines of equal weight" into
//! "the 3 open commitments where the model's read most diverges from
//! your average" — attention routing on top of f_outcome.
//!
//! Honest caveats:
//! - Insights only fire when n_priors ≥ `min_priors` (default 6) AND
//!   |delta| ≥ `min_abs_delta` (default 0.20). Below either threshold
//!   we stay silent — under-surfacing is the safe failure mode (`docs/
//!   INTENT_SYSTEM.md` §11 #4).
//! - The baseline is the global completed-rate, NOT a per-cell rate.
//!   A user who completes 80% of intents will see a "warning" insight
//!   only when the model predicts < 60% positive — which means the
//!   *features* of this specific commitment look unusually risky.
//! - We never claim causation. Tone is "this looks N pp riskier than
//!   your average", not "this will fail".

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::brief::{CommitmentBriefRow, CommitmentOutlook};

/// Configuration for the insight detector. Defaults match the
/// "under-surface rather than over-surface" stance: a pretty high
/// |delta| floor and a small cap so the brief stays scannable.
#[derive(Debug, Clone)]
pub struct InsightConfig {
    /// Minimum |model_positive - baseline_positive| for an open row to
    /// surface as an insight. 0.20 = "20 percentage points".
    pub min_abs_delta: f32,
    /// Cap on rows in the panel. The brief is a daily skim — three
    /// is the default attention budget.
    pub max_rows: usize,
    /// Minimum n_priors on the row's outlook before we trust the
    /// model enough to flag a divergence. Mirrors the threshold used
    /// for outlook attachment in `BriefBuilder`.
    pub min_priors: usize,
}

impl Default for InsightConfig {
    fn default() -> Self {
        Self {
            min_abs_delta: 0.20,
            max_rows: 3,
            min_priors: 6,
        }
    }
}

/// One surfaced insight row. Trimmed for the brief surface — we
/// re-derive the heavy fields (full distribution, kind, source) from
/// the underlying `CommitmentBriefRow` if a caller needs them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InsightBriefRow {
    pub commitment_id: Uuid,
    /// Trimmed statement for the panel render (full statement still
    /// lives on the open row).
    pub statement: String,
    /// Model's predicted P(better ∪ as_expected).
    pub model_positive: f32,
    /// User's global completed-rate of better ∪ as_expected. The
    /// baseline against which `delta` is computed.
    pub baseline_positive: f32,
    /// `model_positive - baseline_positive`. Negative = model thinks
    /// this is *worse* than your average; positive = better.
    pub delta: f32,
    /// Training set size carried through from the outlook so the
    /// caller sees the same n that gates outlook attachment.
    pub n_priors: usize,
    /// `"warning"` if delta < 0, `"tailwind"` if delta > 0. We never
    /// emit `"mixed"` here because we're already past the
    /// `min_abs_delta` gate — the sign is meaningful.
    pub tone: String,
    /// Pre-rendered one-liner the brief surface can print as-is.
    /// Stable wording: agents can route on `tone` instead of parsing
    /// the string.
    pub render: String,
}

/// Compute the global completed-rate of "positive" outcomes
/// (better ∪ as_expected). `n_completed` is the count of outcomes
/// counted — surfaces use it to refuse insights when the baseline
/// itself is statistically thin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaselineRate {
    pub n_completed: usize,
    pub positive: f32,
}

impl BaselineRate {
    /// Build a baseline from a slice of `(Commitment, Polarity)` —
    /// the same shape `IntentStore::list_completed_with_polarity`
    /// returns. `NoOutcome` is excluded from the denominator so an
    /// abandoned commitment doesn't drag down the rate.
    pub fn from_completed(rows: &[(tm_intent::Commitment, tm_intent::Polarity)]) -> Self {
        use tm_intent::Polarity;
        let mut positive = 0usize;
        let mut total = 0usize;
        for (_, p) in rows {
            match p {
                Polarity::Better | Polarity::AsExpected => {
                    positive += 1;
                    total += 1;
                }
                Polarity::Worse | Polarity::Mixed => {
                    total += 1;
                }
                Polarity::NoOutcome => {
                    // Exclude — this is the "we don't know" bucket.
                }
            }
        }
        let rate = if total == 0 {
            0.0
        } else {
            positive as f32 / total as f32
        };
        Self {
            n_completed: total,
            positive: rate,
        }
    }
}

/// Detect surfaceable insights from open brief rows + a baseline.
/// Pure: no I/O, no clock reads. Caller is responsible for ensuring
/// rows already carry an `outlook` (i.e. the world model was
/// attached to the [`crate::brief::BriefBuilder`]).
///
/// Selection:
/// 1. Skip rows without an outlook.
/// 2. Skip rows whose outlook has `n_priors < cfg.min_priors`.
/// 3. Compute `delta = outlook.positive_prob - baseline.positive`.
/// 4. Skip rows with `|delta| < cfg.min_abs_delta`.
/// 5. Sort remaining rows by `|delta|` descending.
/// 6. Take at most `cfg.max_rows`.
pub fn detect_insights(
    open_rows: &[CommitmentBriefRow],
    baseline: BaselineRate,
    cfg: &InsightConfig,
) -> Vec<InsightBriefRow> {
    if baseline.n_completed < cfg.min_priors {
        // The baseline itself isn't trustworthy yet — refuse to
        // compare against it. Same n cliff applied symmetrically.
        return Vec::new();
    }

    let mut scored: Vec<(f32, InsightBriefRow)> = Vec::new();
    for row in open_rows {
        let Some(outlook) = row.outlook.as_ref() else {
            continue;
        };
        if outlook.n_priors < cfg.min_priors {
            continue;
        }
        let delta = outlook.positive_prob - baseline.positive;
        let abs_delta = delta.abs();
        if abs_delta < cfg.min_abs_delta {
            continue;
        }
        let tone = if delta < 0.0 { "warning" } else { "tailwind" };
        let render = render_insight(&row.statement, outlook, delta, baseline.positive);
        scored.push((
            abs_delta,
            InsightBriefRow {
                commitment_id: row.id,
                statement: row.statement.clone(),
                model_positive: outlook.positive_prob,
                baseline_positive: baseline.positive,
                delta,
                n_priors: outlook.n_priors,
                tone: tone.to_string(),
                render,
            },
        ));
    }

    // Sort by absolute delta desc — strongest divergence first.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
        .into_iter()
        .take(cfg.max_rows)
        .map(|(_, row)| row)
        .collect()
}

/// Render the deterministic insight one-liner. Stable wording so
/// agents that key off the prefix don't drift across releases.
/// Examples (with baseline = 70%):
///   "looks 35pp riskier than your average (model 35% positive vs 70% baseline, n=12)"
///   "looks 22pp stronger than your average (model 92% positive vs 70% baseline, n=12)"
fn render_insight(
    statement: &str,
    outlook: &CommitmentOutlook,
    delta: f32,
    baseline_positive: f32,
) -> String {
    let pp = (delta.abs() * 100.0).round() as i32;
    let direction = if delta < 0.0 { "riskier" } else { "stronger" };
    let snippet = truncate(statement, 50);
    format!(
        "\"{}\" looks {}pp {} than your average \
         (model {:.0}% positive vs {:.0}% baseline, n={})",
        snippet,
        pp,
        direction,
        outlook.positive_prob * 100.0,
        baseline_positive * 100.0,
        outlook.n_priors,
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brief::{CommitmentBriefRow, CommitmentOutlook, OverdueClass};
    use chrono::Utc;
    use tm_intent::{CommitmentKind, Polarity, Source, State, Stakes};

    fn outlook(positive: f32, n: usize) -> CommitmentOutlook {
        CommitmentOutlook {
            n_priors: n,
            argmax: if positive > 0.5 { "better".into() } else { "worse".into() },
            positive_prob: positive,
            confidence: 0.8,
            better: positive,
            as_expected: 0.0,
            worse: 1.0 - positive,
            mixed: 0.0,
            tone: "warning".into(),
        }
    }

    fn row(stmt: &str, outlook_value: Option<CommitmentOutlook>) -> CommitmentBriefRow {
        CommitmentBriefRow {
            id: Uuid::new_v4(),
            kind: CommitmentKind::Intent,
            statement: stmt.into(),
            state: State::Open,
            stakes: Stakes::Medium,
            source: Source::Manual,
            made_at: Utc::now(),
            horizon: None,
            overdue_class: None as Option<OverdueClass>,
            tags: vec![],
            outlook: outlook_value,
        }
    }

    fn pol(p: Polarity) -> (tm_intent::Commitment, Polarity) {
        let c = tm_intent::Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        (c, p)
    }

    #[test]
    fn baseline_excludes_no_outcome_from_denominator() {
        let rows = vec![
            pol(Polarity::Better),
            pol(Polarity::AsExpected),
            pol(Polarity::Worse),
            pol(Polarity::NoOutcome), // excluded
        ];
        let b = BaselineRate::from_completed(&rows);
        assert_eq!(b.n_completed, 3, "NoOutcome must not count toward total");
        assert!((b.positive - (2.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn baseline_handles_zero_completed() {
        let b = BaselineRate::from_completed(&[]);
        assert_eq!(b.n_completed, 0);
        assert_eq!(b.positive, 0.0);
    }

    #[test]
    fn empty_open_rows_yields_no_insights() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.7 };
        let out = detect_insights(&[], baseline, &InsightConfig::default());
        assert!(out.is_empty());
    }

    #[test]
    fn rows_without_outlook_are_skipped() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.7 };
        let rows = vec![row("no outlook here", None)];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert!(out.is_empty(), "rows without outlook contribute nothing");
    }

    #[test]
    fn delta_below_threshold_is_filtered() {
        // Baseline 70% positive, model 75% positive → delta = +5pp.
        // Threshold is 20pp default — this row must NOT surface.
        let baseline = BaselineRate { n_completed: 100, positive: 0.70 };
        let rows = vec![row("just a tad above", Some(outlook(0.75, 10)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert!(out.is_empty(), "5pp delta is below 20pp threshold");
    }

    #[test]
    fn n_priors_below_min_filters_row_even_with_huge_delta() {
        // 50pp delta but only 3 priors — must NOT surface.
        let baseline = BaselineRate { n_completed: 100, positive: 0.70 };
        let rows = vec![row("flashy but unsupported", Some(outlook(0.20, 3)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert!(out.is_empty(), "n_priors=3 < min_priors gate");
    }

    #[test]
    fn baseline_under_min_priors_yields_no_insights() {
        // Even if the model is well-supported, an untrustworthy
        // baseline (n=2 completed) means we can't compare honestly.
        let baseline = BaselineRate { n_completed: 2, positive: 0.50 };
        let rows = vec![row("fine row", Some(outlook(0.10, 100)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert!(out.is_empty(), "thin baseline → refuse to compare");
    }

    #[test]
    fn warning_tone_for_below_baseline_row() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.70 };
        // Model says 30% positive, baseline 70% → -40pp.
        let rows = vec![row("ship migration friday", Some(outlook(0.30, 12)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tone, "warning");
        assert!((out[0].delta + 0.40).abs() < 1e-3);
        assert!(out[0].render.contains("riskier"));
        assert!(out[0].render.contains("n=12"));
    }

    #[test]
    fn tailwind_tone_for_above_baseline_row() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.50 };
        // Model says 90% positive, baseline 50% → +40pp.
        let rows = vec![row("morning writing block", Some(outlook(0.90, 12)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tone, "tailwind");
        assert!((out[0].delta - 0.40).abs() < 1e-3);
        assert!(out[0].render.contains("stronger"));
    }

    #[test]
    fn ranking_keeps_largest_absolute_delta_first() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.50 };
        let rows = vec![
            row("medium dip", Some(outlook(0.25, 10))),  // |Δ|=0.25
            row("huge spike", Some(outlook(0.95, 10))),  // |Δ|=0.45
            row("just-over-floor", Some(outlook(0.71, 10))), // |Δ|=0.21
        ];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].statement, "huge spike");
        assert_eq!(out[1].statement, "medium dip");
        assert_eq!(out[2].statement, "just-over-floor");
    }

    #[test]
    fn max_rows_caps_panel_size() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.50 };
        let mut rows = Vec::new();
        // 5 rows all over threshold; cap is 3 by default.
        for i in 0..5 {
            let positive = 0.05 + 0.01 * i as f32; // all way below baseline
            rows.push(row(&format!("risk-{i}"), Some(outlook(positive, 10))));
        }
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn config_min_abs_delta_is_respected() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.50 };
        let rows = vec![row("subtle", Some(outlook(0.45, 10)))]; // |Δ|=0.05
        let out = detect_insights(
            &rows,
            baseline,
            &InsightConfig {
                min_abs_delta: 0.04,
                ..Default::default()
            },
        );
        assert_eq!(out.len(), 1, "lowering the floor surfaces the subtle row");
    }

    #[test]
    fn render_truncates_long_statement() {
        let baseline = BaselineRate { n_completed: 100, positive: 0.50 };
        let long = "x".repeat(200);
        let rows = vec![row(&long, Some(outlook(0.10, 10)))];
        let out = detect_insights(&rows, baseline, &InsightConfig::default());
        assert_eq!(out.len(), 1);
        // 50 chars + truncation char → ≤ 51 chars in the snippet.
        // Check that the render carries the ellipsis.
        assert!(out[0].render.contains('…'));
    }
}
