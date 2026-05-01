//! Trainer for [`OutcomeModel`] — full-batch (or mini-batch) SGD on
//! cross-entropy loss with L2 weight decay.
//!
//! Honest scope: training data here is *small*. Most users will have
//! 10-100 completed commitments, not 10k. So we don't need momentum,
//! Adam, or learning-rate schedules — vanilla SGD with weight decay
//! converges in a few hundred passes and stays well-conditioned.
//!
//! Loss:
//! ```text
//!   L = -log P(y | x) + (λ/2) ||W||²
//! ```
//! Gradient (per example):
//! ```text
//!   ∂L/∂z_c   = p_c - 1{c == y}        // softmax cross-entropy
//!   ∂L/∂w_cj  = (p_c - 1{c == y}) * x_j  + λ * w_cj
//!   ∂L/∂b_c   = (p_c - 1{c == y})
//! ```

use serde::{Deserialize, Serialize};
use tm_intent::{Commitment, Polarity};
use tracing::debug;

use crate::features::{extract, TagVocab, DEFAULT_TAG_VOCAB_SIZE};
use crate::predictor::{softmax, OutcomeModel, N_CLASSES};
use crate::types::PolarityClass;

/// Trainer hyperparameters. Defaults tuned for tiny datasets (<200
/// completed commitments) — wide enough that the model can fit signal,
/// regularized enough that it doesn't memorize on N=10.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainerConfig {
    pub epochs: usize,
    pub lr: f32,
    pub l2: f32,
    pub tag_vocab_size: usize,
    /// Hard floor: skip training entirely below this many usable
    /// examples. The brief and CLI surface this as "world model
    /// dormant — N more commitments to wake it up".
    pub min_examples: usize,
}

impl Default for TrainerConfig {
    fn default() -> Self {
        Self {
            epochs: 400,
            lr: 0.1,
            l2: 1e-3,
            tag_vocab_size: DEFAULT_TAG_VOCAB_SIZE,
            min_examples: 6,
        }
    }
}

/// One training example: `(Commitment, target class)`. We use this
/// shape so the trainer doesn't have to know about `tm-intent::Polarity`
/// directly — `NoOutcome` rows are filtered upstream by [`from_pairs`].
#[derive(Debug, Clone)]
pub struct Example {
    pub commitment: Commitment,
    pub target: PolarityClass,
}

/// Training metrics — what the CLI prints after `world train`.
#[derive(Debug, Clone, Serialize)]
pub struct TrainReport {
    pub n_examples: usize,
    pub n_classes_seen: usize,
    pub epochs_run: usize,
    pub final_loss: f32,
    pub final_accuracy: f32,
    pub vocab_size: usize,
    pub feature_dim: usize,
    pub skipped_no_outcome: usize,
}

/// Convert `(Commitment, Polarity)` rows from the intent store into
/// trainer-ready examples. `NoOutcome` is dropped — counted into
/// `skipped` for the report. Caller decides whether to surface that.
pub fn from_pairs(rows: Vec<(Commitment, Polarity)>) -> (Vec<Example>, usize) {
    let mut out = Vec::with_capacity(rows.len());
    let mut skipped = 0usize;
    for (c, p) in rows {
        match PolarityClass::from_polarity(p) {
            Some(target) => out.push(Example { commitment: c, target }),
            None => skipped += 1,
        }
    }
    (out, skipped)
}

/// Train (or refuse to train) and return a populated [`OutcomeModel`].
///
/// If we don't have at least `cfg.min_examples` usable rows, returns
/// an untrained model with the in-place vocab so inference stays
/// uniform but the calibration panel still has something to render.
pub fn train(examples: &[Example], cfg: &TrainerConfig) -> (OutcomeModel, TrainReport) {
    // Build vocab from the *training set* only. Inference uses the
    // same vocab so this is fine — keeps φ deterministic.
    let vocab = TagVocab::from_training(examples.iter().map(|e| &e.commitment), cfg.tag_vocab_size);
    let mut model = OutcomeModel::fresh(vocab);
    let n_classes_seen = examples
        .iter()
        .map(|e| e.target.index())
        .collect::<std::collections::HashSet<_>>()
        .len();

    let report_skeleton = TrainReport {
        n_examples: examples.len(),
        n_classes_seen,
        epochs_run: 0,
        final_loss: 0.0,
        final_accuracy: 0.0,
        vocab_size: model.tag_vocab.len(),
        feature_dim: model.feature_dim(),
        skipped_no_outcome: 0,
    };

    if examples.len() < cfg.min_examples {
        debug!(
            "[world-model] only {} examples, < min_examples {}; staying uniform",
            examples.len(),
            cfg.min_examples
        );
        return (model, report_skeleton);
    }

    // Cache φ once per example to avoid re-extracting every epoch.
    let phis: Vec<Vec<f32>> = examples
        .iter()
        .map(|ex| extract(&ex.commitment, &model.tag_vocab))
        .collect();
    let targets: Vec<usize> = examples.iter().map(|ex| ex.target.index()).collect();

    let d = model.feature_dim();
    let mut last_loss = 0.0_f32;
    let mut last_acc = 0.0_f32;

    for _epoch in 0..cfg.epochs {
        let mut total_loss = 0.0_f32;
        let mut correct = 0usize;

        // Full-batch SGD: accumulate gradients across the whole set,
        // then apply one update. Tiny N → batch == set is cheaper and
        // more stable than mini-batches.
        let mut grad_w = vec![0.0_f32; N_CLASSES * d];
        let mut grad_b = vec![0.0_f32; N_CLASSES];

        for (x, &y) in phis.iter().zip(targets.iter()) {
            // Forward.
            let mut logits = [0.0_f32; N_CLASSES];
            for c in 0..N_CLASSES {
                let mut s = model.bias[c];
                for j in 0..d {
                    s += model.weights[c * d + j] * x[j];
                }
                logits[c] = s;
            }
            let probs = softmax(&logits);

            // Loss + accuracy bookkeeping.
            total_loss += -((probs[y]).max(1e-12)).ln();
            let pred = (0..N_CLASSES)
                .max_by(|a, b| probs[*a].partial_cmp(&probs[*b]).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(0);
            if pred == y {
                correct += 1;
            }

            // Gradient: dL/dz_c = p_c - 1{c==y}.
            for c in 0..N_CLASSES {
                let dz = probs[c] - if c == y { 1.0 } else { 0.0 };
                grad_b[c] += dz;
                let row = c * d;
                for j in 0..d {
                    grad_w[row + j] += dz * x[j];
                }
            }
        }

        // Apply: w -= lr * (grad / N + l2 * w); b -= lr * (grad / N).
        let n = examples.len() as f32;
        for c in 0..N_CLASSES {
            let row = c * d;
            for j in 0..d {
                let g = grad_w[row + j] / n + cfg.l2 * model.weights[row + j];
                model.weights[row + j] -= cfg.lr * g;
            }
            model.bias[c] -= cfg.lr * (grad_b[c] / n);
        }

        last_loss = total_loss / n;
        last_acc = correct as f32 / n;
    }

    model.n_train_examples = examples.len();
    model.trained_at = Some(chrono::Utc::now().to_rfc3339());

    let mut report = report_skeleton;
    report.epochs_run = cfg.epochs;
    report.final_loss = last_loss;
    report.final_accuracy = last_acc;
    (model, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_intent::{CommitmentKind, Source, Stakes};

    fn ex(stakes: Stakes, hour: u32, target: PolarityClass) -> Example {
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = stakes;
        c.made_at = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 4, 28, hour, 0, 0).unwrap();
        Example { commitment: c, target }
    }

    #[test]
    fn from_pairs_drops_no_outcome() {
        let mut c1 = Commitment::new(CommitmentKind::Intent, "a", Source::Cli);
        c1.stakes = Stakes::Medium;
        let mut c2 = Commitment::new(CommitmentKind::Intent, "b", Source::Cli);
        c2.stakes = Stakes::High;

        let (exs, skipped) = from_pairs(vec![
            (c1, Polarity::Better),
            (c2, Polarity::NoOutcome),
        ]);
        assert_eq!(exs.len(), 1);
        assert_eq!(skipped, 1);
        assert_eq!(exs[0].target, PolarityClass::Better);
    }

    #[test]
    fn train_below_min_examples_returns_uniform_model() {
        let exs = vec![ex(Stakes::High, 18, PolarityClass::Worse)];
        let cfg = TrainerConfig {
            min_examples: 6,
            ..TrainerConfig::default()
        };
        let (model, report) = train(&exs, &cfg);
        assert!(!model.is_trained());
        assert_eq!(report.epochs_run, 0);
    }

    #[test]
    fn train_separable_set_drives_loss_down() {
        // Build an obviously-separable set: high-stakes evening always
        // ends Worse, low-stakes morning always ends Better.
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..6 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let cfg = TrainerConfig::default();
        let (model, report) = train(&exs, &cfg);

        assert!(model.is_trained());
        assert_eq!(report.n_examples, 12);
        // After 400 epochs on a linearly-separable problem, loss
        // should be small and accuracy near perfect.
        assert!(
            report.final_loss < 0.3,
            "loss should be low on separable data, got {}",
            report.final_loss
        );
        assert!(
            report.final_accuracy > 0.95,
            "accuracy should be near-perfect on separable data, got {}",
            report.final_accuracy
        );
    }

    #[test]
    fn trained_model_predicts_correct_class_on_seen_pattern() {
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..6 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let (model, _) = train(&exs, &TrainerConfig::default());
        // New, unseen-but-on-pattern commitment → predicts the right class.
        let probe = ex(Stakes::High, 19, PolarityClass::Worse).commitment;
        let pred = model.predict(&probe);
        assert_eq!(pred.argmax, PolarityClass::Worse);
        // And the *positive* prob should be low for a worse-trending pattern.
        assert!(
            pred.positive_prob < 0.3,
            "positive_prob should be low for worse-trending pattern, got {}",
            pred.positive_prob
        );
    }
}
