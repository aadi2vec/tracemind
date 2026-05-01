//! `f_outcome` v0 — multinomial logistic regression.
//!
//! Forward pass:
//! ```text
//!   logits = W · φ + b              // W: 4×D, b: 4
//!   probs  = softmax(logits)        // 4 dims, sums to 1
//! ```
//!
//! Honest scope: this is *not* a neural net. It's a 4-class linear
//! classifier over engineered features (see [`crate::features`]). v1
//! will add a BGE topic embedding + a small MLP layer; v2 will replace
//! the linear head with a transformer trained on contrastive pairs.
//!
//! For the v0 ask "what does my own track record imply about this
//! commitment?", linear-on-metadata is the correct choice — fast,
//! tiny, interpretable, can't catastrophically overfit on N<100 rows.

use serde::{Deserialize, Serialize};
use tm_intent::Commitment;

use crate::features::{extract, feature_dim, TagVocab, FIXED_FEATURES};
use crate::types::{OutcomePrediction, PolarityClass, PolarityDist};

/// Number of polarity classes the predictor emits. Pinned to keep the
/// disk format stable.
pub const N_CLASSES: usize = 4;

/// Trained model weights + the metadata needed to reproduce φ.
///
/// Layout:
/// - `weights`: row-major `N_CLASSES × feature_dim` matrix.
/// - `bias`: length `N_CLASSES`.
/// - `tag_vocab`: pinned at training time.
/// - `n_train_examples`: how much data the model has seen — surfaces
///   in the calibration panel and gates the L3 recommendation surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeModel {
    /// Schema version of the on-disk format. Bump when changing the
    /// feature layout — older snapshots become unreadable rather than
    /// silently misaligned.
    pub schema_version: u32,
    pub tag_vocab: TagVocab,
    /// Row-major: `weights[c * D + j]`.
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
    pub n_train_examples: usize,
    /// Wall-clock RFC3339 of the last train call. Optional so empty
    /// models read cleanly. Useful for "model staleness" hints.
    pub trained_at: Option<String>,
}

/// Current schema version. Increment whenever feature layout changes.
pub const SCHEMA_VERSION: u32 = 1;

impl OutcomeModel {
    /// Build a fresh, untrained model with zero weights for the given
    /// vocab. Predictions from such a model are uniform.
    pub fn fresh(vocab: TagVocab) -> Self {
        let d = feature_dim(&vocab);
        Self {
            schema_version: SCHEMA_VERSION,
            tag_vocab: vocab,
            weights: vec![0.0; N_CLASSES * d],
            bias: vec![0.0; N_CLASSES],
            n_train_examples: 0,
            trained_at: None,
        }
    }

    /// Total feature dimension this model was trained against.
    pub fn feature_dim(&self) -> usize {
        feature_dim(&self.tag_vocab)
    }

    /// `true` once the trainer has seen at least one example.
    pub fn is_trained(&self) -> bool {
        self.n_train_examples > 0
    }

    /// Forward pass. Returns the softmax distribution over polarity
    /// classes. An untrained model returns uniform regardless of input.
    pub fn forward(&self, x: &[f32]) -> PolarityDist {
        if !self.is_trained() {
            return PolarityDist::uniform();
        }
        let d = self.feature_dim();
        if x.len() != d {
            // Defensive: feature dim drift (vocab changed) → uniform
            // rather than panic. Caller should re-train.
            return PolarityDist::uniform();
        }

        let mut logits = [0.0_f32; N_CLASSES];
        for c in 0..N_CLASSES {
            let row_start = c * d;
            let mut s = self.bias[c];
            for j in 0..d {
                s += self.weights[row_start + j] * x[j];
            }
            logits[c] = s;
        }
        PolarityDist(softmax(&logits))
    }

    /// Convenience: extract φ + run forward + wrap as `OutcomePrediction`.
    pub fn predict(&self, c: &Commitment) -> OutcomePrediction {
        let x = extract(c, &self.tag_vocab);
        let dist = self.forward(&x);
        OutcomePrediction::from_dist(dist, self.n_train_examples)
    }

    /// Get a `(class, feature) -> weight` triple. Used by tests and the
    /// human-readable feature attribution in `world status`.
    pub fn weight(&self, class_idx: usize, feature_idx: usize) -> f32 {
        let d = self.feature_dim();
        self.weights[class_idx * d + feature_idx]
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

/// Human-readable feature attribution for a single commitment + class —
/// used by `tracemind world predict --explain`. Returns the top-K
/// features ranked by their contribution to that class's logit (= w_j *
/// x_j). Bias is reported separately.
pub fn explain_top_k(
    model: &OutcomeModel,
    commitment: &Commitment,
    class: PolarityClass,
    k: usize,
) -> Vec<FeatureContribution> {
    let x = extract(commitment, &model.tag_vocab);
    let d = model.feature_dim();
    let class_idx = class.index();
    let mut contribs: Vec<FeatureContribution> = (0..d)
        .map(|j| FeatureContribution {
            label: feature_label(j, &model.tag_vocab),
            value: x[j],
            weight: model.weights[class_idx * d + j],
            contribution: model.weights[class_idx * d + j] * x[j],
        })
        .collect();
    // Sort by absolute contribution descending; non-zero φ values bubble up.
    contribs.sort_by(|a, b| b.contribution.abs().partial_cmp(&a.contribution.abs()).unwrap_or(std::cmp::Ordering::Equal));
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
            weights: vec![0.1; N_CLASSES * 15],
            bias: vec![0.0; N_CLASSES],
            n_train_examples: 5,
            trained_at: None,
        };
        // Wrong-length φ → uniform, not panic.
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
        m.weights[0 * d + 0] = 5.0;
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
        m.weights[PolarityClass::Worse.index() * d + 2] = 2.0;
        m.weights[PolarityClass::Worse.index() * d + FIXED_FEATURES] = 1.5;

        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = Stakes::High;
        c.tags = vec!["vendor".into()];

        let top = explain_top_k(&m, &c, PolarityClass::Worse, 3);
        // Top contributor must be active (φ_j != 0) and the top weight.
        assert!(top.iter().any(|f| f.label == "stakes:high" && f.contribution > 0.0));
        assert!(top.iter().any(|f| f.label == "tag:vendor" && f.contribution > 0.0));
    }
}
