//! `f_outcome` predictor — multinomial logistic regression (Linear)
//! or 1-hidden-layer ReLU MLP, switchable via [`Architecture`].
//!
//! Linear forward pass:
//! ```text
//!   logits = W1 · φ + b1            // W1: 4×D, b1: 4
//!   probs  = softmax(logits)        // 4 dims, sums to 1
//! ```
//!
//! MLP forward pass:
//! ```text
//!   h_pre  = W1 · φ + b1            // W1: H×D, b1: H
//!   h      = relu(h_pre)            // H
//!   logits = W2 · h  + b2           // W2: 4×H, b2: 4
//!   probs  = softmax(logits)
//! ```
//!
//! Honest scope: linear-on-metadata stays the right default for tiny
//! datasets (N < 20). The MLP path unlocks non-linearly-separable
//! patterns the user genuinely has — e.g. "high stakes is fine in
//! the morning but disastrous in the evening" requires interaction
//! between two engineered features and the linear classifier can't
//! learn it. Hidden width is small (default 8) to stay honest with
//! tiny-N — too many parameters and we just memorize. v2 will swap
//! the input block for a BGE topic embedding; the MLP head stays.
//!
//! Why ship MLP into v0 instead of waiting for v1: the upgrade is
//! cheap (≈80 lines of backprop), the disk format already version-
//! gates against schema drift, and the calibration gate (TM-INTENT-
//! 010) auto-quiets a bad model — so we can let users opt into the
//! richer architecture without risking the L2 surface.

use serde::{Deserialize, Serialize};
use tm_intent::Commitment;

use crate::features::{extract, feature_dim, TagVocab, FIXED_FEATURES};
use crate::types::{OutcomePrediction, PolarityClass, PolarityDist};

/// Number of polarity classes the predictor emits. Pinned to keep the
/// disk format stable.
pub const N_CLASSES: usize = 4;

/// Default hidden-layer width for the MLP architecture. Small on
/// purpose: typical user has < 200 completions, and a wider hidden
/// layer just memorizes. Power-users with thousands of completions
/// can override at train time.
pub const DEFAULT_MLP_HIDDEN: usize = 8;

/// Architecture switch — chooses between the linear classifier and a
/// 1-hidden-layer ReLU MLP. Stored on disk so inference is consistent
/// with training; mismatched code paths can never silently disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Architecture {
    /// Multinomial logistic regression. `w1` is `N_CLASSES × D`;
    /// `w2` / `b2` are absent.
    Linear,
    /// 1-hidden-layer MLP with ReLU. `w1` is `hidden_dim × D`,
    /// `w2` is `N_CLASSES × hidden_dim`.
    Mlp { hidden_dim: usize },
}

impl Architecture {
    /// Output dimension of layer 1 — `N_CLASSES` for Linear, the
    /// hidden width for MLP.
    pub fn layer1_out(self) -> usize {
        match self {
            Architecture::Linear => N_CLASSES,
            Architecture::Mlp { hidden_dim } => hidden_dim,
        }
    }

    /// Human-readable label used in the `world status` surface and
    /// the calibration card.
    pub fn label(self) -> String {
        match self {
            Architecture::Linear => "linear".into(),
            Architecture::Mlp { hidden_dim } => format!("mlp(h={hidden_dim})"),
        }
    }
}

/// Trained model weights + the metadata needed to reproduce φ.
///
/// Layout depends on [`Architecture`]:
/// - **Linear**: `w1` is row-major `N_CLASSES × feature_dim`; `b1`
///   has length `N_CLASSES`. `w2` and `b2` are `None`.
/// - **MLP**: `w1` is row-major `hidden_dim × feature_dim`; `b1` has
///   length `hidden_dim`. `w2` is row-major `N_CLASSES × hidden_dim`;
///   `b2` has length `N_CLASSES`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeModel {
    /// Schema version of the on-disk format. Bump when changing the
    /// feature layout or architecture wire format.
    pub schema_version: u32,
    pub tag_vocab: TagVocab,
    /// Architecture switch. Defaults to [`Architecture::Linear`] on
    /// the legacy code path (`fresh`) for back-compat.
    #[serde(default = "default_architecture")]
    pub architecture: Architecture,
    /// Layer-1 weights. Row-major shape: `layer1_out × feature_dim`
    /// where `layer1_out = N_CLASSES` (Linear) or `hidden_dim` (MLP).
    pub w1: Vec<f32>,
    /// Layer-1 biases — one per layer-1 output unit.
    pub b1: Vec<f32>,
    /// Layer-2 weights. `None` for Linear; for MLP, row-major shape
    /// `N_CLASSES × hidden_dim`.
    #[serde(default)]
    pub w2: Option<Vec<f32>>,
    /// Layer-2 biases. `None` for Linear; for MLP, length `N_CLASSES`.
    #[serde(default)]
    pub b2: Option<Vec<f32>>,
    pub n_train_examples: usize,
    /// Wall-clock RFC3339 of the last train call. Optional so empty
    /// models read cleanly. Useful for "model staleness" hints.
    pub trained_at: Option<String>,
}

fn default_architecture() -> Architecture {
    Architecture::Linear
}

/// Current schema version. Increment whenever feature layout changes.
///
/// History:
/// - v1: linear classifier (`weights` / `bias` flat fields).
/// - v2: introduced [`Architecture`] + `w1/b1/w2/b2` fields. Old v1
///   files become unreadable on purpose — we use `schema_version` to
///   force a clean retrain rather than silently misalign matrices.
pub const SCHEMA_VERSION: u32 = 2;

impl OutcomeModel {
    /// Build a fresh, untrained Linear model with zero weights for
    /// the given vocab. Predictions from such a model are uniform.
    /// Equivalent to `fresh_with(vocab, Architecture::Linear)`.
    pub fn fresh(vocab: TagVocab) -> Self {
        Self::fresh_with(vocab, Architecture::Linear)
    }

    /// Build a fresh, untrained model with the chosen architecture.
    pub fn fresh_with(vocab: TagVocab, architecture: Architecture) -> Self {
        let d = feature_dim(&vocab);
        let layer1_out = architecture.layer1_out();
        let (w2, b2) = match architecture {
            Architecture::Linear => (None, None),
            Architecture::Mlp { hidden_dim } => (
                Some(vec![0.0; N_CLASSES * hidden_dim]),
                Some(vec![0.0; N_CLASSES]),
            ),
        };
        Self {
            schema_version: SCHEMA_VERSION,
            tag_vocab: vocab,
            architecture,
            w1: vec![0.0; layer1_out * d],
            b1: vec![0.0; layer1_out],
            w2,
            b2,
            n_train_examples: 0,
            trained_at: None,
        }
    }

    /// Total feature dimension (input φ length) this model was
    /// trained against.
    pub fn feature_dim(&self) -> usize {
        feature_dim(&self.tag_vocab)
    }

    /// Hidden-layer width when the architecture is MLP, else 0.
    pub fn hidden_dim(&self) -> usize {
        match self.architecture {
            Architecture::Linear => 0,
            Architecture::Mlp { hidden_dim } => hidden_dim,
        }
    }

    /// `true` once the trainer has seen at least one example.
    pub fn is_trained(&self) -> bool {
        self.n_train_examples > 0
    }

    /// Forward pass. Returns the softmax distribution over polarity
    /// classes. An untrained model returns uniform regardless of input.
    /// Defensive: dim drift (e.g. vocab changed under the user) →
    /// uniform rather than panic; caller should retrain.
    pub fn forward(&self, x: &[f32]) -> PolarityDist {
        if !self.is_trained() {
            return PolarityDist::uniform();
        }
        let d = self.feature_dim();
        if x.len() != d {
            return PolarityDist::uniform();
        }

        let logits = match self.architecture {
            Architecture::Linear => self.forward_linear(x, d),
            Architecture::Mlp { hidden_dim } => match self.forward_mlp(x, d, hidden_dim) {
                Some(l) => l,
                None => return PolarityDist::uniform(), // shape sanity failed
            },
        };
        PolarityDist(softmax(&logits))
    }

    fn forward_linear(&self, x: &[f32], d: usize) -> [f32; N_CLASSES] {
        let mut logits = [0.0_f32; N_CLASSES];
        for c in 0..N_CLASSES {
            let row = c * d;
            let mut s = self.b1[c];
            for j in 0..d {
                s += self.w1[row + j] * x[j];
            }
            logits[c] = s;
        }
        logits
    }

    /// MLP forward. Returns `None` if `w2`/`b2` are missing or have
    /// the wrong shape — defensive guard against hand-edited or
    /// half-migrated model files.
    fn forward_mlp(&self, x: &[f32], d: usize, hidden_dim: usize) -> Option<[f32; N_CLASSES]> {
        let w2 = self.w2.as_ref()?;
        let b2 = self.b2.as_ref()?;
        if self.w1.len() != hidden_dim * d
            || self.b1.len() != hidden_dim
            || w2.len() != N_CLASSES * hidden_dim
            || b2.len() != N_CLASSES
        {
            return None;
        }

        // Layer 1: h_pre = W1·x + b1, h = relu(h_pre)
        let mut h = vec![0.0_f32; hidden_dim];
        for k in 0..hidden_dim {
            let row = k * d;
            let mut s = self.b1[k];
            for j in 0..d {
                s += self.w1[row + j] * x[j];
            }
            // ReLU
            h[k] = if s > 0.0 { s } else { 0.0 };
        }

        // Layer 2: logits = W2·h + b2
        let mut logits = [0.0_f32; N_CLASSES];
        for c in 0..N_CLASSES {
            let row = c * hidden_dim;
            let mut s = b2[c];
            for k in 0..hidden_dim {
                s += w2[row + k] * h[k];
            }
            logits[c] = s;
        }
        Some(logits)
    }

    /// Convenience: extract φ + run forward + wrap as `OutcomePrediction`.
    pub fn predict(&self, c: &Commitment) -> OutcomePrediction {
        let x = extract(c, &self.tag_vocab);
        let dist = self.forward(&x);
        OutcomePrediction::from_dist(dist, self.n_train_examples)
    }

    /// Get the layer-1 `(out, feature) -> weight` triple. For Linear
    /// this is the `(class, feature)` weight that drives that class's
    /// logit directly. For MLP this is `(hidden_unit, feature)` — use
    /// [`explain_top_k`] for class-level attribution that handles the
    /// hidden layer.
    pub fn weight(&self, out_idx: usize, feature_idx: usize) -> f32 {
        let d = self.feature_dim();
        self.w1[out_idx * d + feature_idx]
    }
}

/// Numerically-stable softmax over a fixed-length array.
pub fn softmax(logits: &[f32; N_CLASSES]) -> [f32; N_CLASSES] {
    let max = logits
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    let mut out = [0.0_f32; N_CLASSES];
    let mut sum = 0.0_f32;
    for (i, l) in logits.iter().enumerate() {
        let e = (l - max).exp();
        out[i] = e;
        sum += e;
    }
    if sum <= 0.0 || !sum.is_finite() {
        return [0.25; N_CLASSES];
    }
    for v in &mut out {
        *v /= sum;
    }
    out
}

/// Human-readable feature attribution for a single commitment + class.
///
/// For Linear: `contribution = w[class, j] * x[j]`.
///
/// For MLP: per-input *linearization* through the active ReLU mask —
/// `contribution_j = Σ_h W2[c, h] * 1{h_pre[h] > 0} * W1[h, j] * x[j]`.
/// This is the gradient-based explanation: how much would `x[j]`
/// pushing up by 1 change `logit_c`, *holding the active hidden
/// units fixed*. It's the right thing to show the user — they care
/// about "given this commitment shape, why did the model think
/// what it did", not the global linear effect.
pub fn explain_top_k(
    model: &OutcomeModel,
    commitment: &Commitment,
    class: PolarityClass,
    k: usize,
) -> Vec<FeatureContribution> {
    let x = extract(commitment, &model.tag_vocab);
    let d = model.feature_dim();
    let class_idx = class.index();
    let mut contribs: Vec<FeatureContribution> = match model.architecture {
        Architecture::Linear => (0..d)
            .map(|j| FeatureContribution {
                label: feature_label(j, &model.tag_vocab),
                value: x[j],
                weight: model.w1[class_idx * d + j],
                contribution: model.w1[class_idx * d + j] * x[j],
            })
            .collect(),
        Architecture::Mlp { hidden_dim } => {
            // Effective per-feature weight under the active ReLU mask
            // for *this* input.
            let w2 = match model.w2.as_ref() {
                Some(w) => w,
                None => return Vec::new(),
            };
            // h_pre + active mask
            let mut active = vec![false; hidden_dim];
            for h in 0..hidden_dim {
                let row = h * d;
                let mut s = model.b1[h];
                for j in 0..d {
                    s += model.w1[row + j] * x[j];
                }
                active[h] = s > 0.0;
            }
            (0..d)
                .map(|j| {
                    let mut effective = 0.0_f32;
                    for h in 0..hidden_dim {
                        if !active[h] {
                            continue;
                        }
                        // chain: w2[c,h] * w1[h,j]
                        effective += w2[class_idx * hidden_dim + h] * model.w1[h * d + j];
                    }
                    FeatureContribution {
                        label: feature_label(j, &model.tag_vocab),
                        value: x[j],
                        weight: effective,
                        contribution: effective * x[j],
                    }
                })
                .collect()
        }
    };
    contribs.sort_by(|a, b| {
        b.contribution
            .abs()
            .partial_cmp(&a.contribution.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    contribs.truncate(k);
    contribs
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureContribution {
    pub label: String,
    pub value: f32,
    pub weight: f32,
    pub contribution: f32,
}

fn feature_label(j: usize, vocab: &TagVocab) -> String {
    match j {
        0 => "stakes:low".into(),
        1 => "stakes:medium".into(),
        2 => "stakes:high".into(),
        3 => "stakes:reversible".into(),
        4 => "time:morning".into(),
        5 => "time:afternoon".into(),
        6 => "time:evening".into(),
        7 => "time:night".into(),
        8 => "has_horizon".into(),
        9 => "has_options".into(),
        10 => "user_confidence".into(),
        11 => "kind:intent".into(),
        12 => "kind:decision".into(),
        13 => "kind:hypothesis".into(),
        n if n >= FIXED_FEATURES => {
            let local = n - FIXED_FEATURES;
            vocab
                .entries()
                .get(local)
                .map(|t| format!("tag:{}", t))
                .unwrap_or_else(|| format!("tag:#{}", local))
        }
        _ => format!("feat:#{}", j),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_intent::{CommitmentKind, Source, Stakes};

    fn vocab_with(tags: &[&str]) -> TagVocab {
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.tags = tags.iter().map(|s| s.to_string()).collect();
        TagVocab::from_training([c].iter(), 16)
    }

    #[test]
    fn untrained_model_is_uniform() {
        let m = OutcomeModel::fresh(TagVocab::default());
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::High;
        let dist = m.predict(&c).dist;
        for v in dist.0 {
            assert!((v - 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn untrained_mlp_is_uniform() {
        let m = OutcomeModel::fresh_with(
            TagVocab::default(),
            Architecture::Mlp { hidden_dim: 4 },
        );
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::High;
        let dist = m.predict(&c).dist;
        for v in dist.0 {
            assert!((v - 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn softmax_sums_to_one_and_max_class_wins() {
        let s = softmax(&[2.0, 1.0, 0.5, -1.0]);
        let total: f32 = s.iter().sum();
        assert!((total - 1.0).abs() < 1e-5);
        assert!(s[0] > s[1] && s[1] > s[2] && s[2] > s[3]);
    }

    #[test]
    fn forward_against_dim_mismatch_returns_uniform_not_panic() {
        let m = OutcomeModel {
            schema_version: SCHEMA_VERSION,
            tag_vocab: vocab_with(&["vendor"]),
            architecture: Architecture::Linear,
            w1: vec![0.1; N_CLASSES * 15],
            b1: vec![0.0; N_CLASSES],
            w2: None,
            b2: None,
            n_train_examples: 5,
            trained_at: None,
        };
        let dist = m.forward(&vec![1.0; 9]);
        for v in dist.0 {
            assert!((v - 0.25).abs() < 1e-6);
        }
    }

    #[test]
    fn forward_with_set_weights_respects_class_logits() {
        // Hand-pick weights: feature 0 (stakes:low) heavily weighted to class Better.
        let mut m = OutcomeModel::fresh(TagVocab::default());
        m.n_train_examples = 1;
        let d = m.feature_dim();
        // class 0 = Better, feature 0 = stakes:low.
        m.w1[0 * d + 0] = 5.0;
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::Low;
        let pred = m.predict(&c);
        assert_eq!(pred.argmax, PolarityClass::Better);
    }

    #[test]
    fn mlp_forward_with_set_weights_routes_through_hidden_layer() {
        // Hand-build an MLP that fires hidden unit 0 only on
        // stakes:low (feat 0), and routes hidden unit 0 to class
        // Better. Confirms the chain works without training.
        let hidden = 2;
        let mut m = OutcomeModel::fresh_with(
            TagVocab::default(),
            Architecture::Mlp { hidden_dim: hidden },
        );
        m.n_train_examples = 1;
        let d = m.feature_dim();
        // w1[hidden_unit=0, feature=0 (stakes:low)] = 5.0 — fires
        // strongly when stakes:low is set.
        m.w1[0 * d + 0] = 5.0;
        // w2[class=Better=0, hidden=0] = 3.0 — pulls Better up when
        // hidden unit 0 fires.
        if let Some(ref mut w2) = m.w2 {
            w2[0 * hidden + 0] = 3.0;
        }
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::Low;
        let pred = m.predict(&c);
        assert_eq!(pred.argmax, PolarityClass::Better);
    }

    #[test]
    fn explain_includes_active_features_only() {
        let mut m = OutcomeModel::fresh(vocab_with(&["vendor"]));
        m.n_train_examples = 5;
        let d = m.feature_dim();
        // Set stakes:high (idx 2) and tag:vendor (idx FIXED_FEATURES) for class Worse.
        m.w1[PolarityClass::Worse.index() * d + 2] = 2.0;
        m.w1[PolarityClass::Worse.index() * d + FIXED_FEATURES] = 1.5;

        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::High;
        c.tags = vec!["vendor".into()];

        let top = explain_top_k(&m, &c, PolarityClass::Worse, 3);
        // Top contributor must be active (φ_j != 0) and the top weight.
        assert!(top.iter().any(|f| f.label == "stakes:high" && f.contribution > 0.0));
        assert!(top.iter().any(|f| f.label == "tag:vendor" && f.contribution > 0.0));
    }

    #[test]
    fn architecture_label_renders_hidden_dim() {
        assert_eq!(Architecture::Linear.label(), "linear");
        assert_eq!(
            Architecture::Mlp { hidden_dim: 8 }.label(),
            "mlp(h=8)"
        );
    }

    #[test]
    fn architecture_layer1_out_is_n_classes_for_linear_and_hidden_for_mlp() {
        assert_eq!(Architecture::Linear.layer1_out(), N_CLASSES);
        assert_eq!(Architecture::Mlp { hidden_dim: 7 }.layer1_out(), 7);
    }
}
