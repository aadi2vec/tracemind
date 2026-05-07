//! Trainer for [`OutcomeModel`] — full-batch SGD on cross-entropy
//! with L2 weight decay. Supports two architectures:
//!
//! - **Linear** (default, back-compat): multinomial logistic regression.
//! - **MLP**: 1-hidden-layer with ReLU. Backprop trains both layers
//!   end-to-end. Same loss, same optimizer, same shape of report.
//!
//! Honest scope: training data here is *small*. Most users have
//! 10-200 completed commitments, not 10k. So we don't need momentum,
//! Adam, or learning-rate schedules — vanilla SGD with weight decay
//! converges in a few hundred passes and stays well-conditioned.
//!
//! Quality controls (added in TM-INTENT-011):
//!
//! 1. **Held-out validation split**. When the dataset has at least
//!    `2 * min_examples` rows AND `validation_split > 0`, we shuffle
//!    *deterministically* (seeded), peel off a validation chunk, and
//!    train only on the train chunk. Lets the report carry an
//!    out-of-sample loss / accuracy that the calibration gate can
//!    tighten its eyes on.
//! 2. **Early stopping**. If `early_stop_patience > 0`, we track
//!    validation loss and bail when it fails to improve for that
//!    many epochs. Combined with the L2 floor this keeps tiny-N runs
//!    from overfitting the train chunk.
//! 3. **Best-checkpoint restore**. On early stop, we revert weights
//!    to the snapshot that posted the best validation loss — not the
//!    latest. The model the user sees is always the one their data
//!    most strongly supports.
//!
//! Loss (per example):
//! ```text
//!   L = -log P(y | x) + (λ/2) ||W||²
//! ```
//! Linear gradient:
//! ```text
//!   ∂L/∂z_c   = p_c - 1{c == y}
//!   ∂L/∂w1_cj = (p_c - 1{c == y}) * x_j  + λ * w1_cj
//!   ∂L/∂b1_c  = (p_c - 1{c == y})
//! ```
//! MLP gradient (1 hidden layer, ReLU):
//! ```text
//!   z      = W2·h + b2,  h = relu(W1·x + b1)
//!   ∂L/∂z  = p - one_hot(y)
//!   ∂L/∂W2 = ∂L/∂z ⊗ h        + λ·W2
//!   ∂L/∂b2 = ∂L/∂z
//!   ∂L/∂h  = W2ᵀ · ∂L/∂z
//!   ∂L/∂a  = ∂L/∂h ⊙ 1{h_pre > 0}    // ReLU mask
//!   ∂L/∂W1 = ∂L/∂a ⊗ x        + λ·W1
//!   ∂L/∂b1 = ∂L/∂a
//! ```

use serde::{Deserialize, Serialize};
use tm_intent::{Commitment, Polarity};
use tracing::debug;

use crate::features::{extract, TagVocab, DEFAULT_TAG_VOCAB_SIZE};
use crate::predictor::{
    softmax, Architecture, OutcomeModel, DEFAULT_MLP_HIDDEN, N_CLASSES,
};
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
    /// Architecture to train. Default is [`Architecture::Linear`] for
    /// back-compat; `Mlp` requires backprop through the hidden layer.
    #[serde(default = "default_arch")]
    pub architecture: Architecture,
    /// Fraction of examples held out for validation. `0.0` disables
    /// the split entirely (legacy behavior). Default 0.2.
    #[serde(default = "default_validation_split")]
    pub validation_split: f32,
    /// Stop early if validation loss has not improved for this many
    /// epochs. `0` disables early stopping. Default 25.
    #[serde(default = "default_early_stop_patience")]
    pub early_stop_patience: usize,
    /// Seed for the deterministic shuffle that drives the train /
    /// validation split. Stable across runs so re-training the same
    /// data is reproducible.
    #[serde(default = "default_seed")]
    pub seed: u64,
}

fn default_arch() -> Architecture {
    Architecture::Linear
}
fn default_validation_split() -> f32 {
    0.2
}
fn default_early_stop_patience() -> usize {
    25
}
fn default_seed() -> u64 {
    0xA17DA_u64
}

impl TrainerConfig {
    /// Convenience: trainer config that produces an MLP with the
    /// default hidden width. Mirrors the CLI `--arch mlp` shortcut.
    pub fn mlp_default() -> Self {
        Self {
            architecture: Architecture::Mlp { hidden_dim: DEFAULT_MLP_HIDDEN },
            ..Self::default()
        }
    }
}

impl Default for TrainerConfig {
    fn default() -> Self {
        Self {
            epochs: 400,
            lr: 0.1,
            l2: 1e-3,
            tag_vocab_size: DEFAULT_TAG_VOCAB_SIZE,
            min_examples: 6,
            architecture: default_arch(),
            validation_split: default_validation_split(),
            early_stop_patience: default_early_stop_patience(),
            seed: default_seed(),
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

/// Training metrics — what the CLI prints after `world train`. The
/// `val_*` fields are only populated when [`TrainerConfig::validation_split`]
/// > 0 *and* the dataset is big enough to support a held-out chunk.
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
    /// Held-out size. `0` when no split was taken.
    #[serde(default)]
    pub n_validation: usize,
    /// Held-out loss at the snapshot we kept. `None` when no split.
    #[serde(default)]
    pub val_loss: Option<f32>,
    /// Held-out accuracy at the snapshot we kept. `None` when no split.
    #[serde(default)]
    pub val_accuracy: Option<f32>,
    /// `true` when training halted before `cfg.epochs` because val
    /// loss stopped improving.
    #[serde(default)]
    pub early_stopped: bool,
    /// Architecture label that produced this report — `"linear"` or
    /// `"mlp(h=N)"`. Surfaced verbatim in the `world status` card so
    /// the user can tell which head is in flight.
    #[serde(default)]
    pub architecture: String,
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
    let mut model = OutcomeModel::fresh_with(vocab, cfg.architecture);
    let n_classes_seen = examples
        .iter()
        .map(|e| e.target.index())
        .collect::<std::collections::HashSet<_>>()
        .len();

    let arch_label = cfg.architecture.label();
    let report_skeleton = TrainReport {
        n_examples: examples.len(),
        n_classes_seen,
        epochs_run: 0,
        final_loss: 0.0,
        final_accuracy: 0.0,
        vocab_size: model.tag_vocab.len(),
        feature_dim: model.feature_dim(),
        skipped_no_outcome: 0,
        n_validation: 0,
        val_loss: None,
        val_accuracy: None,
        early_stopped: false,
        architecture: arch_label,
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

    // Optional held-out split. Only take a validation chunk if we
    // have at least 2 × min_examples rows — peeling off a chunk from
    // a tiny dataset just makes both sides too small to be useful.
    let (train_idx, val_idx) =
        split_train_val(examples.len(), cfg.validation_split, cfg.min_examples, cfg.seed);

    match cfg.architecture {
        Architecture::Linear => train_linear(
            &mut model,
            &phis,
            &targets,
            &train_idx,
            &val_idx,
            cfg,
            report_skeleton,
        ),
        Architecture::Mlp { hidden_dim } => train_mlp(
            &mut model,
            &phis,
            &targets,
            &train_idx,
            &val_idx,
            cfg,
            hidden_dim,
            report_skeleton,
        ),
    }
}

/// Deterministic train / validation split. Returns `(train_idx,
/// val_idx)`. Indices are into `examples`. Falls back to "no
/// validation" (full set in train) when the dataset is too small or
/// `validation_split == 0`.
fn split_train_val(
    n: usize,
    validation_split: f32,
    min_examples: usize,
    seed: u64,
) -> (Vec<usize>, Vec<usize>) {
    if validation_split <= 0.0 || n < 2 * min_examples {
        return ((0..n).collect(), Vec::new());
    }
    let val_n = ((n as f32) * validation_split).round() as usize;
    let val_n = val_n.max(1).min(n.saturating_sub(min_examples));
    if val_n == 0 {
        return ((0..n).collect(), Vec::new());
    }

    // Seeded LCG shuffle. We don't need crypto-grade randomness —
    // we just need stable + deterministic across runs.
    let mut indices: Vec<usize> = (0..n).collect();
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    for i in (1..n).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = (state >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
    let val_idx: Vec<usize> = indices[..val_n].to_vec();
    let train_idx: Vec<usize> = indices[val_n..].to_vec();
    (train_idx, val_idx)
}

/// Cross-entropy loss + top-1 hits for a model over a subset of
/// indices into `(phis, targets)`.
fn eval_loss_acc(
    forward: impl Fn(&[f32]) -> [f32; N_CLASSES],
    phis: &[Vec<f32>],
    targets: &[usize],
    idx: &[usize],
) -> (f32, f32) {
    if idx.is_empty() {
        return (0.0, 0.0);
    }
    let mut loss = 0.0_f32;
    let mut correct = 0usize;
    for &i in idx {
        let probs = softmax(&forward(&phis[i]));
        let y = targets[i];
        loss += -probs[y].max(1e-12).ln();
        let pred = (0..N_CLASSES)
            .max_by(|a, b| probs[*a].partial_cmp(&probs[*b]).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0);
        if pred == y {
            correct += 1;
        }
    }
    let n = idx.len() as f32;
    (loss / n, correct as f32 / n)
}

fn train_linear(
    model: &mut OutcomeModel,
    phis: &[Vec<f32>],
    targets: &[usize],
    train_idx: &[usize],
    val_idx: &[usize],
    cfg: &TrainerConfig,
    skeleton: TrainReport,
) -> (OutcomeModel, TrainReport) {
    let d = model.feature_dim();

    let mut last_loss = 0.0_f32;
    let mut last_acc = 0.0_f32;
    let mut best_val_loss = f32::INFINITY;
    let mut best_w1 = model.w1.clone();
    let mut best_b1 = model.b1.clone();
    let mut best_val_acc: Option<f32> = None;
    let mut epochs_no_improve = 0usize;
    let mut epochs_run = 0usize;
    let mut early_stopped = false;

    for epoch in 0..cfg.epochs {
        epochs_run = epoch + 1;
        let mut total_loss = 0.0_f32;
        let mut correct = 0usize;

        let mut grad_w = vec![0.0_f32; N_CLASSES * d];
        let mut grad_b = vec![0.0_f32; N_CLASSES];

        for &i in train_idx {
            let x = &phis[i];
            let y = targets[i];

            // Forward.
            let mut logits = [0.0_f32; N_CLASSES];
            for c in 0..N_CLASSES {
                let mut s = model.b1[c];
                for j in 0..d {
                    s += model.w1[c * d + j] * x[j];
                }
                logits[c] = s;
            }
            let probs = softmax(&logits);
            total_loss += -probs[y].max(1e-12).ln();
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
        let n = train_idx.len() as f32;
        for c in 0..N_CLASSES {
            let row = c * d;
            for j in 0..d {
                let g = grad_w[row + j] / n + cfg.l2 * model.w1[row + j];
                model.w1[row + j] -= cfg.lr * g;
            }
            model.b1[c] -= cfg.lr * (grad_b[c] / n);
        }

        last_loss = total_loss / n;
        last_acc = correct as f32 / n;

        // Validation + early-stop bookkeeping.
        if !val_idx.is_empty() {
            let snap = (model.w1.clone(), model.b1.clone());
            let forward = |x: &[f32]| linear_forward(&snap.0, &snap.1, x, d);
            let (vl, va) = eval_loss_acc(forward, phis, targets, val_idx);
            if vl + 1e-4 < best_val_loss {
                best_val_loss = vl;
                best_val_acc = Some(va);
                best_w1 = model.w1.clone();
                best_b1 = model.b1.clone();
                epochs_no_improve = 0;
            } else {
                epochs_no_improve += 1;
                if cfg.early_stop_patience > 0 && epochs_no_improve >= cfg.early_stop_patience {
                    early_stopped = true;
                    break;
                }
            }
        }
    }

    // If we tracked validation, restore the best snapshot. Otherwise
    // keep the final epoch's weights.
    if !val_idx.is_empty() && best_val_loss.is_finite() {
        model.w1 = best_w1;
        model.b1 = best_b1;
    }

    // n_train_examples reports the *input* count — the user's "this
    // model is informed by N priors" — not the post-split chunk size.
    // The validation chunk is internal bookkeeping; the user owns
    // every row that fed into training.
    model.n_train_examples = phis.len();
    model.trained_at = Some(chrono::Utc::now().to_rfc3339());

    let (val_loss, val_accuracy) = if val_idx.is_empty() {
        (None, None)
    } else {
        (Some(best_val_loss), best_val_acc)
    };

    let mut report = skeleton;
    report.epochs_run = epochs_run;
    report.final_loss = last_loss;
    report.final_accuracy = last_acc;
    report.n_validation = val_idx.len();
    report.val_loss = val_loss;
    report.val_accuracy = val_accuracy;
    report.early_stopped = early_stopped;
    (model.clone(), report)
}

fn linear_forward(w1: &[f32], b1: &[f32], x: &[f32], d: usize) -> [f32; N_CLASSES] {
    let mut logits = [0.0_f32; N_CLASSES];
    for c in 0..N_CLASSES {
        let mut s = b1[c];
        for j in 0..d {
            s += w1[c * d + j] * x[j];
        }
        logits[c] = s;
    }
    logits
}

fn mlp_forward(
    w1: &[f32],
    b1: &[f32],
    w2: &[f32],
    b2: &[f32],
    x: &[f32],
    d: usize,
    h_dim: usize,
) -> ([f32; N_CLASSES], Vec<f32>, Vec<f32>) {
    let mut h_pre = vec![0.0_f32; h_dim];
    let mut h = vec![0.0_f32; h_dim];
    for k in 0..h_dim {
        let mut s = b1[k];
        for j in 0..d {
            s += w1[k * d + j] * x[j];
        }
        h_pre[k] = s;
        h[k] = if s > 0.0 { s } else { 0.0 };
    }
    let mut logits = [0.0_f32; N_CLASSES];
    for c in 0..N_CLASSES {
        let mut s = b2[c];
        for k in 0..h_dim {
            s += w2[c * h_dim + k] * h[k];
        }
        logits[c] = s;
    }
    (logits, h, h_pre)
}

#[allow(clippy::too_many_arguments)]
fn train_mlp(
    model: &mut OutcomeModel,
    phis: &[Vec<f32>],
    targets: &[usize],
    train_idx: &[usize],
    val_idx: &[usize],
    cfg: &TrainerConfig,
    h_dim: usize,
    skeleton: TrainReport,
) -> (OutcomeModel, TrainReport) {
    let d = model.feature_dim();

    // Initialize weights non-zero so hidden units don't all collapse
    // to the same gradient. Tiny variance tied to seed + (out, in)
    // — deterministic, no rng dep.
    init_mlp_weights(model, d, h_dim, cfg.seed);

    let mut last_loss = 0.0_f32;
    let mut last_acc = 0.0_f32;
    let mut best_val_loss = f32::INFINITY;
    let mut best_val_acc: Option<f32> = None;
    let mut best_w1 = model.w1.clone();
    let mut best_b1 = model.b1.clone();
    let mut best_w2 = model.w2.clone().unwrap_or_default();
    let mut best_b2 = model.b2.clone().unwrap_or_default();
    let mut epochs_no_improve = 0usize;
    let mut epochs_run = 0usize;
    let mut early_stopped = false;

    for epoch in 0..cfg.epochs {
        epochs_run = epoch + 1;
        let mut total_loss = 0.0_f32;
        let mut correct = 0usize;

        let mut grad_w1 = vec![0.0_f32; h_dim * d];
        let mut grad_b1 = vec![0.0_f32; h_dim];
        let mut grad_w2 = vec![0.0_f32; N_CLASSES * h_dim];
        let mut grad_b2 = vec![0.0_f32; N_CLASSES];

        let w2_ref = model.w2.as_ref().expect("MLP w2 must exist after init");
        let b2_ref = model.b2.as_ref().expect("MLP b2 must exist after init");

        for &i in train_idx {
            let x = &phis[i];
            let y = targets[i];

            let (logits, h, h_pre) =
                mlp_forward(&model.w1, &model.b1, w2_ref, b2_ref, x, d, h_dim);
            let probs = softmax(&logits);
            total_loss += -probs[y].max(1e-12).ln();
            let pred = (0..N_CLASSES)
                .max_by(|a, b| probs[*a].partial_cmp(&probs[*b]).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap_or(0);
            if pred == y {
                correct += 1;
            }

            // dL/dz = p - one_hot(y)
            let mut dz = [0.0_f32; N_CLASSES];
            for c in 0..N_CLASSES {
                dz[c] = probs[c] - if c == y { 1.0 } else { 0.0 };
            }

            // dL/dW2 = dz ⊗ h ; dL/db2 = dz
            for c in 0..N_CLASSES {
                grad_b2[c] += dz[c];
                let row = c * h_dim;
                for k in 0..h_dim {
                    grad_w2[row + k] += dz[c] * h[k];
                }
            }

            // dL/dh = W2ᵀ · dz
            // dL/da = dh ⊙ 1{h_pre > 0}
            let mut da = vec![0.0_f32; h_dim];
            for k in 0..h_dim {
                if h_pre[k] <= 0.0 {
                    continue;
                }
                let mut s = 0.0_f32;
                for c in 0..N_CLASSES {
                    s += w2_ref[c * h_dim + k] * dz[c];
                }
                da[k] = s;
            }

            // dL/dW1 = da ⊗ x ; dL/db1 = da
            for k in 0..h_dim {
                grad_b1[k] += da[k];
                let row = k * d;
                for j in 0..d {
                    grad_w1[row + j] += da[k] * x[j];
                }
            }
        }

        let n = train_idx.len() as f32;

        // Apply layer 2.
        {
            let w2_mut = model.w2.as_mut().expect("MLP w2 must exist after init");
            let b2_mut = model.b2.as_mut().expect("MLP b2 must exist after init");
            for c in 0..N_CLASSES {
                let row = c * h_dim;
                for k in 0..h_dim {
                    let g = grad_w2[row + k] / n + cfg.l2 * w2_mut[row + k];
                    w2_mut[row + k] -= cfg.lr * g;
                }
                b2_mut[c] -= cfg.lr * (grad_b2[c] / n);
            }
        }
        // Apply layer 1.
        for k in 0..h_dim {
            let row = k * d;
            for j in 0..d {
                let g = grad_w1[row + j] / n + cfg.l2 * model.w1[row + j];
                model.w1[row + j] -= cfg.lr * g;
            }
            model.b1[k] -= cfg.lr * (grad_b1[k] / n);
        }

        last_loss = total_loss / n;
        last_acc = correct as f32 / n;

        // Validation.
        if !val_idx.is_empty() {
            let w1s = model.w1.clone();
            let b1s = model.b1.clone();
            let w2s = model.w2.clone().unwrap();
            let b2s = model.b2.clone().unwrap();
            let forward = |x: &[f32]| mlp_forward(&w1s, &b1s, &w2s, &b2s, x, d, h_dim).0;
            let (vl, va) = eval_loss_acc(forward, phis, targets, val_idx);
            if vl + 1e-4 < best_val_loss {
                best_val_loss = vl;
                best_val_acc = Some(va);
                best_w1 = model.w1.clone();
                best_b1 = model.b1.clone();
                best_w2 = w2s;
                best_b2 = b2s;
                epochs_no_improve = 0;
            } else {
                epochs_no_improve += 1;
                if cfg.early_stop_patience > 0 && epochs_no_improve >= cfg.early_stop_patience {
                    early_stopped = true;
                    break;
                }
            }
        }
    }

    if !val_idx.is_empty() && best_val_loss.is_finite() {
        model.w1 = best_w1;
        model.b1 = best_b1;
        model.w2 = Some(best_w2);
        model.b2 = Some(best_b2);
    }

    // n_train_examples reports the *input* count — the user's "this
    // model is informed by N priors" — not the post-split chunk size.
    // The validation chunk is internal bookkeeping; the user owns
    // every row that fed into training.
    model.n_train_examples = phis.len();
    model.trained_at = Some(chrono::Utc::now().to_rfc3339());

    let (val_loss, val_accuracy) = if val_idx.is_empty() {
        (None, None)
    } else {
        (Some(best_val_loss), best_val_acc)
    };

    let mut report = skeleton;
    report.epochs_run = epochs_run;
    report.final_loss = last_loss;
    report.final_accuracy = last_acc;
    report.n_validation = val_idx.len();
    report.val_loss = val_loss;
    report.val_accuracy = val_accuracy;
    report.early_stopped = early_stopped;
    (model.clone(), report)
}

/// Deterministic small-noise init so MLP hidden units start with
/// distinct gradients. Seeded so re-runs are reproducible. Magnitude
/// is tied to fan-in (sqrt(1/d) and sqrt(1/h_dim)) — keeps logits in
/// the linear region of softmax at start.
fn init_mlp_weights(model: &mut OutcomeModel, d: usize, h_dim: usize, seed: u64) {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let scale1 = (1.0 / (d as f32)).sqrt();
    let scale2 = (1.0 / (h_dim as f32)).sqrt();

    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Take 31 high bits → [0, 2^31) → [0, 1) → [-1, 1).
        let bits = (state >> 33) as u32;
        let u01 = (bits as f32) / (1u64 << 31) as f32;
        u01 * 2.0 - 1.0
    };

    for v in model.w1.iter_mut() {
        *v = next() * scale1;
    }
    if let Some(w2) = model.w2.as_mut() {
        for v in w2.iter_mut() {
            *v = next() * scale2;
        }
    }
    // Biases stay zero.
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
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..6 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        // Disable the held-out split: with 12 rows and a 0.2 split
        // we'd train on ~10 — still works, but the assertion below
        // is on training accuracy and we want it on the full set
        // for reproducibility.
        let cfg = TrainerConfig {
            validation_split: 0.0,
            ..TrainerConfig::default()
        };
        let (model, report) = train(&exs, &cfg);

        assert!(model.is_trained());
        assert_eq!(report.n_examples, 12);
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
        assert_eq!(report.architecture, "linear");
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
        let cfg = TrainerConfig {
            validation_split: 0.0,
            ..TrainerConfig::default()
        };
        let (model, _) = train(&exs, &cfg);
        let probe = ex(Stakes::High, 19, PolarityClass::Worse).commitment;
        let pred = model.predict(&probe);
        assert_eq!(pred.argmax, PolarityClass::Worse);
        assert!(
            pred.positive_prob < 0.3,
            "positive_prob should be low for worse-trending pattern, got {}",
            pred.positive_prob
        );
    }

    #[test]
    fn validation_split_reports_holdout_metrics() {
        // 24 rows so 0.2 split = 5 → both sides clear min_examples.
        let mut exs = Vec::new();
        for _ in 0..12 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..12 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let cfg = TrainerConfig {
            validation_split: 0.25,
            ..TrainerConfig::default()
        };
        let (_, report) = train(&exs, &cfg);
        assert!(report.n_validation > 0, "should have held out a chunk");
        assert!(
            report.val_loss.is_some(),
            "val_loss should be reported when split > 0"
        );
        // On a perfectly separable split, held-out accuracy should be
        // very high.
        let va = report.val_accuracy.unwrap_or(0.0);
        assert!(va > 0.8, "val_accuracy should be high on separable data, got {va}");
    }

    #[test]
    fn mlp_trains_and_predicts_on_separable_pattern() {
        let mut exs = Vec::new();
        for _ in 0..12 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..12 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let cfg = TrainerConfig {
            architecture: Architecture::Mlp { hidden_dim: 4 },
            validation_split: 0.0,
            // MLP needs more epochs to settle from the random init.
            epochs: 800,
            lr: 0.2,
            ..TrainerConfig::default()
        };
        let (model, report) = train(&exs, &cfg);
        assert!(model.is_trained());
        assert_eq!(report.architecture, "mlp(h=4)");
        assert!(
            report.final_accuracy > 0.95,
            "MLP should fit linearly-separable data near-perfectly, got {}",
            report.final_accuracy
        );
        // Round-trip prediction.
        let probe = ex(Stakes::High, 19, PolarityClass::Worse).commitment;
        let pred = model.predict(&probe);
        assert_eq!(pred.argmax, PolarityClass::Worse);
    }

    #[test]
    fn mlp_can_fit_a_non_separable_xor_pattern() {
        // XOR-shaped: (high, evening) → Worse; (low, morning) → Worse;
        // (high, morning) → Better; (low, evening) → Better. No
        // half-plane in (stakes, time) splits this — the linear model
        // can't match it but the MLP should.
        let mut exs = Vec::new();
        for _ in 0..6 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
            exs.push(ex(Stakes::Low, 9, PolarityClass::Worse));
            exs.push(ex(Stakes::High, 9, PolarityClass::Better));
            exs.push(ex(Stakes::Low, 19, PolarityClass::Better));
        }
        let cfg = TrainerConfig {
            architecture: Architecture::Mlp { hidden_dim: 8 },
            validation_split: 0.0,
            epochs: 1500,
            lr: 0.3,
            l2: 1e-4,
            ..TrainerConfig::default()
        };
        let (_, report) = train(&exs, &cfg);
        // The MLP should match the pattern much better than chance
        // (which on 4 classes is 0.25). We don't insist on perfect
        // because the engineered features collapse stakes:medium and
        // there can be tag-vocab interplay at small N.
        assert!(
            report.final_accuracy >= 0.6,
            "MLP should beat chance on XOR-shaped data, got {}",
            report.final_accuracy
        );
    }

    #[test]
    fn early_stopping_triggers_when_validation_plateaus() {
        let mut exs = Vec::new();
        for _ in 0..12 {
            exs.push(ex(Stakes::High, 19, PolarityClass::Worse));
        }
        for _ in 0..12 {
            exs.push(ex(Stakes::Low, 9, PolarityClass::Better));
        }
        let cfg = TrainerConfig {
            validation_split: 0.25,
            epochs: 2000,
            early_stop_patience: 20,
            ..TrainerConfig::default()
        };
        let (_, report) = train(&exs, &cfg);
        assert!(
            report.epochs_run < 2000,
            "should have stopped early, ran {} epochs",
            report.epochs_run
        );
        assert!(report.early_stopped, "early_stopped flag should be set");
    }
}
