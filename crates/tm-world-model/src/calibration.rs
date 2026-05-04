//! Calibration view for [`OutcomeModel`] — score `f_outcome`'s
//! predictions against eventual resolution polarity.
//!
//! Honest scope per `docs/INTENT_SYSTEM.md` §7.2 ("the world model
//! must earn user trust over time"):
//!
//! - Pure data-in / data-out. No I/O, no DB. Caller assembles the
//!   `(Commitment, Polarity)` pairs (typically via
//!   `tm_intent::IntentStore::list_completed_with_polarity`).
//! - Out-of-sample is the *caller's* responsibility — we expose
//!   [`split_out_of_sample`] for the common case (filter by
//!   `outcome.observed_at > model.trained_at`), but this module
//!   never fetches anything itself.
//! - Metrics are deliberately small + interpretable: top-1 accuracy,
//!   positive recall, warning precision, multiclass Brier, and a
//!   4×{actual,predicted,correct} per-class breakdown. No ROC, no
//!   AUC — those need calibrated probabilities, which v0 does not
//!   promise to deliver.
//!
//! Output goes to two surfaces:
//! - `tracemind world calibration` (human + `--json`)
//! - `memory_world_calibration` MCP tool (always JSON)
//!
//! "Warning" is defined as `positive_prob < 0.5`. That mirrors the L2
//! pattern surface contract — when the model would surface a tailwind
//! / warning insight, did the actual outcome line up with the call?

use serde::{Deserialize, Serialize};
use tm_intent::{Commitment, Polarity};

use crate::predictor::{OutcomeModel, N_CLASSES};
use crate::types::PolarityClass;

/// Per-class confusion-matrix breakdown. `n_actual` and `n_predicted`
/// don't have to agree; their difference is the model's bias for /
/// against that class. `n_correct` is the diagonal — true-positives
/// for that class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerClass {
    pub class: PolarityClass,
    pub label: String,
    pub n_actual: usize,
    pub n_predicted: usize,
    pub n_correct: usize,
}

/// Score card returned by [`evaluate`]. Sized to fit in a brief block.
///
/// `trained_at` is propagated from the model so the surface can render
/// "model trained 2026-04-26, evaluated on N completions since". When
/// `n_evaluated == 0` the report is degenerate but still safe to read
/// (all metrics 0.0, per_class zeroed) — the CLI prints a "not enough
/// out-of-sample data" hint instead of zeros.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReport {
    pub n_evaluated: usize,
    /// Pairs filtered out because their outcome was observed
    /// at-or-before `trained_at` (i.e. the model saw them at training
    /// time). Always 0 when the caller doesn't filter.
    pub n_in_sample_skipped: usize,
    /// Pairs dropped because `Polarity::NoOutcome` is not predictable.
    pub n_no_outcome_skipped: usize,
    /// Number of evaluated rows where the model's `positive_prob <
    /// 0.5` — i.e. the model would have *fired a warning* on this
    /// row. Used by the trust gate to distinguish "no warnings
    /// issued" (no evidence) from "warnings issued but inaccurate"
    /// (untrustworthy). Always `<= n_evaluated`.
    #[serde(default)]
    pub n_warning: usize,
    /// Number of evaluated rows whose actual polarity was Better or
    /// AsExpected — denominator of `positive_recall`.
    #[serde(default)]
    pub n_actual_positive: usize,
    pub accuracy: f32,
    /// P(positive_prob >= 0.5 | actual ∈ {Better, AsExpected}). The
    /// rate at which the model correctly signals tailwind on rows
    /// that *did* land positive.
    pub positive_recall: f32,
    /// P(actual ∈ {Worse, Mixed} | positive_prob < 0.5). The rate at
    /// which the model's warnings are right — the trust knob for the
    /// L2 warning surface.
    pub warning_precision: f32,
    /// Multiclass Brier: mean over examples of Σ_c (p_c - 1{c==y})².
    /// 0.0 = perfect, 2.0 = worst (mirror class). v0 gives this raw —
    /// no reliability-diagram bucketing yet.
    pub brier_score: f32,
    pub per_class: [PerClass; N_CLASSES],
    pub trained_at: Option<String>,
    pub evaluated_at: String,
}

impl CalibrationReport {
    fn empty(trained_at: Option<String>) -> Self {
        let per_class = [
            PolarityClass::Better,
            PolarityClass::AsExpected,
            PolarityClass::Worse,
            PolarityClass::Mixed,
        ]
        .map(|c| PerClass {
            class: c,
            label: c.label().to_string(),
            n_actual: 0,
            n_predicted: 0,
            n_correct: 0,
        });
        Self {
            n_evaluated: 0,
            n_in_sample_skipped: 0,
            n_no_outcome_skipped: 0,
            n_warning: 0,
            n_actual_positive: 0,
            accuracy: 0.0,
            positive_recall: 0.0,
            warning_precision: 0.0,
            brier_score: 0.0,
            per_class,
            trained_at,
            evaluated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Split a pair list into `(out_of_sample, in_sample_count)` against
/// `model.trained_at`. Pairs whose `observed_at` is `> trained_at`
/// are kept; everything else is counted as in-sample-skipped.
///
/// If the model has no `trained_at` (untrained, or pre-`trained_at`
/// snapshot), every pair is treated as out-of-sample — there's no
/// honest cutoff to enforce.
pub fn split_out_of_sample(
    model: &OutcomeModel,
    pairs: Vec<(Commitment, Polarity, chrono::DateTime<chrono::Utc>)>,
) -> (Vec<(Commitment, Polarity)>, usize) {
    let cutoff = match model
        .trained_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        Some(t) => t.with_timezone(&chrono::Utc),
        None => {
            // No cutoff → everything is "out-of-sample" by default.
            let oos = pairs.into_iter().map(|(c, p, _)| (c, p)).collect();
            return (oos, 0);
        }
    };
    let mut oos = Vec::with_capacity(pairs.len());
    let mut skipped = 0usize;
    for (c, p, observed_at) in pairs {
        if observed_at > cutoff {
            oos.push((c, p));
        } else {
            skipped += 1;
        }
    }
    (oos, skipped)
}

/// Run the model over `pairs` and roll up the metrics. `NoOutcome`
/// rows are dropped (counted into `n_no_outcome_skipped`); everything
/// else contributes to all metrics.
///
/// Untrained models still return a valid (degenerate) report — the
/// surface can decide whether to suppress it.
pub fn evaluate(model: &OutcomeModel, pairs: &[(Commitment, Polarity)]) -> CalibrationReport {
    let mut report = CalibrationReport::empty(model.trained_at.clone());

    let mut n_correct = 0usize;
    let mut n_actual_pos = 0usize;
    let mut n_recall_hits = 0usize; // model said positive AND actual positive
    let mut n_warn = 0usize; // model warned (positive_prob < 0.5)
    let mut n_warn_correct = 0usize; // model warned AND actual was negative
    let mut brier_sum = 0.0_f32;

    for (commitment, polarity) in pairs {
        let target = match PolarityClass::from_polarity(*polarity) {
            Some(c) => c,
            None => {
                report.n_no_outcome_skipped += 1;
                continue;
            }
        };

        let pred = model.predict(commitment);
        let target_idx = target.index();

        // Top-1 accuracy.
        if pred.argmax == target {
            n_correct += 1;
            report.per_class[target_idx].n_correct += 1;
        }

        // Per-class actual / predicted counters.
        report.per_class[target_idx].n_actual += 1;
        report.per_class[pred.argmax.index()].n_predicted += 1;

        // Positive recall + warning precision.
        let actual_positive = matches!(target, PolarityClass::Better | PolarityClass::AsExpected);
        let model_positive = pred.positive_prob >= 0.5;
        if actual_positive {
            n_actual_pos += 1;
            if model_positive {
                n_recall_hits += 1;
            }
        }
        if !model_positive {
            n_warn += 1;
            if !actual_positive {
                n_warn_correct += 1;
            }
        }

        // Multiclass Brier: Σ_c (p_c - 1{c==y})².
        let mut sse = 0.0_f32;
        for c in 0..N_CLASSES {
            let p = pred.dist.0[c];
            let y_one_hot = if c == target_idx { 1.0 } else { 0.0 };
            let d = p - y_one_hot;
            sse += d * d;
        }
        brier_sum += sse;

        report.n_evaluated += 1;
    }

    let n = report.n_evaluated;
    if n > 0 {
        report.accuracy = n_correct as f32 / n as f32;
        report.brier_score = brier_sum / n as f32;
    }
    report.n_warning = n_warn;
    report.n_actual_positive = n_actual_pos;
    report.positive_recall = if n_actual_pos > 0 {
        n_recall_hits as f32 / n_actual_pos as f32
    } else {
        0.0
    };
    report.warning_precision = if n_warn > 0 {
        n_warn_correct as f32 / n_warn as f32
    } else {
        0.0
    };

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::TagVocab;
    use crate::trainer::{train, Example, TrainerConfig};
    use chrono::TimeZone;
    use tm_intent::{CommitmentKind, Source, Stakes};

    fn ex(stakes: Stakes, hour: u32, target: PolarityClass) -> Example {
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = stakes;
        c.made_at = chrono::Utc.with_ymd_and_hms(2026, 4, 28, hour, 0, 0).unwrap();
        Example { commitment: c, target }
    }

    fn separable_model() -> OutcomeModel {
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..6 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let (model, _) = train(&exs, &TrainerConfig::default());
        model
    }

    #[test]
    fn empty_pairs_returns_safe_zero_report() {
        let model = OutcomeModel::fresh(TagVocab::default());
        let report = evaluate(&model, &[]);
        assert_eq!(report.n_evaluated, 0);
        assert_eq!(report.accuracy, 0.0);
        assert_eq!(report.positive_recall, 0.0);
        assert_eq!(report.warning_precision, 0.0);
        assert_eq!(report.brier_score, 0.0);
        // labels are wired right.
        assert_eq!(report.per_class[0].label, "better");
        assert_eq!(report.per_class[2].label, "worse");
    }

    #[test]
    fn no_outcome_rows_are_skipped_and_counted() {
        let model = OutcomeModel::fresh(TagVocab::default());
        let pairs = vec![
            (ex(Stakes::Low, 9, PolarityClass::Better).commitment, Polarity::NoOutcome),
            (ex(Stakes::Low, 9, PolarityClass::Better).commitment, Polarity::NoOutcome),
        ];
        let report = evaluate(&model, &pairs);
        assert_eq!(report.n_no_outcome_skipped, 2);
        assert_eq!(report.n_evaluated, 0);
    }

    #[test]
    fn perfect_model_on_seen_pattern_scores_perfect() {
        let model = separable_model();
        // Same generator as training — these are *in-sample* but useful
        // to confirm the metric arithmetic.
        let pairs = vec![
            (ex(Stakes::High, 19, PolarityClass::Worse).commitment, Polarity::Worse),
            (ex(Stakes::High, 19, PolarityClass::Worse).commitment, Polarity::Worse),
            (ex(Stakes::Low, 9, PolarityClass::Better).commitment, Polarity::Better),
            (ex(Stakes::Low, 9, PolarityClass::Better).commitment, Polarity::Better),
        ];
        let report = evaluate(&model, &pairs);
        assert_eq!(report.n_evaluated, 4);
        assert!(
            (report.accuracy - 1.0).abs() < 1e-3,
            "accuracy on seen pattern should be ~1.0, got {}",
            report.accuracy
        );
        // Worse rows: actual negative, model warns → warning_precision=1.0
        assert!(
            (report.warning_precision - 1.0).abs() < 1e-3,
            "warning_precision should be 1.0 on perfect-warning rows, got {}",
            report.warning_precision
        );
        // Better rows: actual positive, model says positive → recall=1.0
        assert!(
            (report.positive_recall - 1.0).abs() < 1e-3,
            "positive_recall should be 1.0 on perfect-tailwind rows, got {}",
            report.positive_recall
        );
        // Brier on a near-perfect classifier should be small.
        assert!(report.brier_score < 0.2, "brier should be small, got {}", report.brier_score);
    }

    #[test]
    fn untrained_model_yields_uniform_metrics() {
        let model = OutcomeModel::fresh(TagVocab::default());
        let pairs = vec![
            (ex(Stakes::High, 19, PolarityClass::Worse).commitment, Polarity::Worse),
            (ex(Stakes::Low, 9, PolarityClass::Better).commitment, Polarity::Better),
            (ex(Stakes::Medium, 12, PolarityClass::Mixed).commitment, Polarity::Mixed),
            (ex(Stakes::Medium, 14, PolarityClass::AsExpected).commitment, Polarity::AsExpected),
        ];
        let report = evaluate(&model, &pairs);
        assert_eq!(report.n_evaluated, 4);
        // Uniform 0.25 → argmax falls to class 0 (Better) deterministically;
        // accuracy = 1/4 (one row was actually Better).
        assert!(
            (report.accuracy - 0.25).abs() < 1e-3,
            "uniform model should accidentally hit 1 of 4, got {}",
            report.accuracy
        );
        // Brier under uniform: Σ_c (0.25 - 1{c==y})² = 3*0.0625 + 0.5625 = 0.75.
        assert!(
            (report.brier_score - 0.75).abs() < 1e-3,
            "uniform brier should be 0.75, got {}",
            report.brier_score
        );
        // Uniform model has positive_prob = 0.5 → never warns → warning_precision = 0.0.
        assert_eq!(report.warning_precision, 0.0);
        // positive_prob = 0.5 satisfies "model_positive" (>=0.5) → recall = 1.0
        // on the 2 actual-positive rows.
        assert!(
            (report.positive_recall - 1.0).abs() < 1e-3,
            "uniform positive_prob 0.5 should always trigger model_positive, got {}",
            report.positive_recall
        );
    }

    #[test]
    fn out_of_sample_split_drops_rows_at_or_before_trained_at() {
        let mut model = OutcomeModel::fresh(TagVocab::default());
        model.n_train_examples = 5;
        model.trained_at = Some("2026-04-28T00:00:00Z".into());

        let mk = |observed: chrono::DateTime<chrono::Utc>| {
            (
                ex(Stakes::Low, 9, PolarityClass::Better).commitment,
                Polarity::Better,
                observed,
            )
        };
        let before = mk(chrono::Utc.with_ymd_and_hms(2026, 4, 27, 0, 0, 0).unwrap());
        let exact = mk(chrono::Utc.with_ymd_and_hms(2026, 4, 28, 0, 0, 0).unwrap());
        let after = mk(chrono::Utc.with_ymd_and_hms(2026, 4, 29, 0, 0, 0).unwrap());

        let (oos, skipped) = split_out_of_sample(&model, vec![before, exact, after]);
        // `> trained_at` is strict, so the row exactly at the cutoff is in-sample.
        assert_eq!(oos.len(), 1);
        assert_eq!(skipped, 2);
    }

    #[test]
    fn out_of_sample_split_with_no_trained_at_keeps_everything() {
        let model = OutcomeModel::fresh(TagVocab::default());
        let mk = |observed: chrono::DateTime<chrono::Utc>| {
            (
                ex(Stakes::Low, 9, PolarityClass::Better).commitment,
                Polarity::Better,
                observed,
            )
        };
        let pairs = vec![
            mk(chrono::Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
            mk(chrono::Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
        ];
        let (oos, skipped) = split_out_of_sample(&model, pairs);
        assert_eq!(oos.len(), 2);
        assert_eq!(skipped, 0);
    }
}
