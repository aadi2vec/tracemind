//! Feature extraction φ: [`Commitment`] → `Vec<f32>`.
//!
//! v0 is **metadata-only** — no BGE topic embedding, no learned text
//! representation. We extract:
//!
//! ```text
//!   stakes one-hot (4)         low / medium / high / reversible
//!   time-band one-hot (4)      morning / afternoon / evening / night
//!   has-horizon (1)            1.0 if Commitment.horizon.is_some()
//!   has-options (1)            1.0 if options_considered > 1
//!   user_confidence (1)        Commitment.confidence (already 0..=1)
//!   kind one-hot (3)           intent / decision / hypothesis
//!   tag-vocab presence (V)     V = top-K most-frequent tags in training
//! ```
//!
//! That's `14 + V` features. With V=16 we land on a 30-dim feature
//! vector — plenty of capacity for a logistic regression on the user's
//! own (small) commitment history without overfitting badly.
//!
//! The tag vocabulary is **learned at training time** and pinned into
//! the saved weights. Inference uses the same vocab; unseen tags are
//! simply ignored.

use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tm_intent::{Commitment, CommitmentKind, Stakes};

/// Fixed-size sections that are independent of the tag vocabulary.
/// 14 dims = 4 (stakes) + 4 (time band) + 1 (horizon) + 1 (options) +
/// 1 (confidence) + 3 (kind).
pub const FIXED_FEATURES: usize = 14;

/// Default cap on tag-vocabulary entries surfaced in φ. Tunable; kept
/// small because the data is small.
pub const DEFAULT_TAG_VOCAB_SIZE: usize = 16;

/// Tag → feature-index map. Persisted alongside the weights so
/// inference uses the same column ordering as training.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagVocab {
    /// Tag string → index inside the tag-vocab section (i.e. NOT the
    /// global feature index — caller adds [`FIXED_FEATURES`]).
    map: HashMap<String, usize>,
    /// Reverse mapping for human-readable feature attribution.
    inverse: Vec<String>,
}

impl TagVocab {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a vocab from training commitments by frequency. Ties are
    /// broken alphabetically so the result is deterministic regardless
    /// of insertion order.
    pub fn from_training<'a, I>(commitments: I, max_size: usize) -> Self
    where
        I: IntoIterator<Item = &'a Commitment>,
    {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for c in commitments {
            for t in &c.tags {
                let key = normalize_tag(t);
                if !key.is_empty() {
                    *counts.entry(key).or_insert(0) += 1;
                }
            }
        }
        let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(max_size);

        let mut map = HashMap::new();
        let mut inverse = Vec::with_capacity(ranked.len());
        for (i, (tag, _)) in ranked.into_iter().enumerate() {
            map.insert(tag.clone(), i);
            inverse.push(tag);
        }
        TagVocab { map, inverse }
    }

    pub fn len(&self) -> usize {
        self.inverse.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inverse.is_empty()
    }

    pub fn get(&self, tag: &str) -> Option<usize> {
        self.map.get(&normalize_tag(tag)).copied()
    }

    pub fn entries(&self) -> &[String] {
        &self.inverse
    }
}

/// Total feature-vector length given the vocab in use.
pub fn feature_dim(vocab: &TagVocab) -> usize {
    FIXED_FEATURES + vocab.len()
}

/// Build the dense feature vector for a single commitment. Stable
/// against vocab evolution — unseen tags drop out, the fixed section
/// is always identical.
pub fn extract(commitment: &Commitment, vocab: &TagVocab) -> Vec<f32> {
    let mut x = vec![0.0_f32; feature_dim(vocab)];

    // ---- stakes one-hot (offset 0..4) ----
    let stakes_idx = match commitment.stakes {
        Stakes::Low => 0,
        Stakes::Medium => 1,
        Stakes::High => 2,
        Stakes::Reversible => 3,
    };
    x[stakes_idx] = 1.0;

    // ---- time-band one-hot (offset 4..8) ----
    // Same banding as the pattern detector: morning 5-11, afternoon
    // 12-16, evening 17-21, night 22-04.
    let hour = commitment.made_at.hour();
    let band_idx = 4 + match hour {
        5..=11 => 0,
        12..=16 => 1,
        17..=21 => 2,
        _ => 3,
    };
    x[band_idx] = 1.0;

    // ---- has-horizon (offset 8) ----
    if commitment.horizon.is_some() {
        x[8] = 1.0;
    }

    // ---- has-options (offset 9) ----
    if commitment.options_considered.len() > 1 {
        x[9] = 1.0;
    }

    // ---- user_confidence (offset 10) ----
    x[10] = commitment.confidence.clamp(0.0, 1.0);

    // ---- kind one-hot (offset 11..14) ----
    let kind_idx = 11 + match commitment.kind {
        CommitmentKind::Intent => 0,
        CommitmentKind::Decision => 1,
        CommitmentKind::Hypothesis => 2,
    };
    x[kind_idx] = 1.0;

    // ---- tag presence (offset 14..14+V) ----
    for tag in &commitment.tags {
        if let Some(local_idx) = vocab.get(tag) {
            x[FIXED_FEATURES + local_idx] = 1.0;
        }
    }

    // Day-of-week is intentionally NOT in φ for v0. It carries weak
    // signal in small datasets and we can add it after we have ≥ 50
    // outcomes per band. Documented as upgrade path.
    let _dow_unused = commitment.made_at.weekday();

    x
}

/// Lower-case + trim for tag matching. Tags are user-typed strings;
/// without normalization we'd treat "Vendor" and "vendor" as different
/// columns and waste capacity.
pub fn normalize_tag(t: &str) -> String {
    t.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tm_intent::{CommitmentKind, Source};

    fn fresh(stakes: Stakes, hour: u32, tags: Vec<&str>) -> Commitment {
        let mut c = Commitment::new(CommitmentKind::Intent, "x", Source::Cli);
        c.stakes = stakes;
        c.made_at = chrono::Utc.with_ymd_and_hms(2026, 4, 28, hour, 0, 0).unwrap();
        c.tags = tags.into_iter().map(String::from).collect();
        c.confidence = 0.6;
        c
    }

    #[test]
    fn fixed_features_constant() {
        // If anyone changes the feature layout, training data goes
        // stale silently — guard the expected width.
        assert_eq!(FIXED_FEATURES, 14);
    }

    #[test]
    fn vocab_from_training_picks_most_frequent() {
        let cs = vec![
            fresh(Stakes::Medium, 10, vec!["vendor", "deploy"]),
            fresh(Stakes::Medium, 10, vec!["vendor"]),
            fresh(Stakes::Medium, 10, vec!["deploy", "vendor"]),
        ];
        let v = TagVocab::from_training(cs.iter(), 16);
        // vendor=3, deploy=2 → vendor first.
        assert_eq!(v.entries(), &["vendor".to_string(), "deploy".to_string()]);
    }

    #[test]
    fn vocab_normalizes_case() {
        let cs = vec![fresh(Stakes::Medium, 10, vec!["Vendor", "vendor", "VENDOR"])];
        let v = TagVocab::from_training(cs.iter(), 16);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn extract_sets_stakes_band_and_kind() {
        let c = fresh(Stakes::High, 18, vec!["vendor"]);
        let v = TagVocab::from_training([c.clone()].iter(), 16);
        let x = extract(&c, &v);

        // Width = 14 + 1 (vocab).
        assert_eq!(x.len(), 15);
        // Stakes: high → idx 2.
        assert_eq!(x[0..4], [0.0, 0.0, 1.0, 0.0]);
        // Hour 18 → evening (offset 4 + 2 = 6).
        assert_eq!(x[4..8], [0.0, 0.0, 1.0, 0.0]);
        // No horizon, no options.
        assert_eq!(x[8], 0.0);
        assert_eq!(x[9], 0.0);
        // Confidence carried through.
        assert!((x[10] - 0.6).abs() < 1e-6);
        // Kind=Intent → offset 11.
        assert_eq!(x[11..14], [1.0, 0.0, 0.0]);
        // Vocab[0] = vendor → offset 14.
        assert_eq!(x[14], 1.0);
    }

    #[test]
    fn extract_unseen_tags_drop_out() {
        let c = fresh(Stakes::Medium, 10, vec!["never_seen"]);
        let v = TagVocab::from_training([fresh(Stakes::Medium, 10, vec!["vendor"])].iter(), 16);
        let x = extract(&c, &v);
        // Vocab section all zero.
        assert_eq!(x[14], 0.0);
    }

    #[test]
    fn extract_band_morning_afternoon_evening_night() {
        let v = TagVocab::default();
        for (hour, expected_band_idx) in [(7, 0), (14, 1), (20, 2), (3, 3)] {
            let c = fresh(Stakes::Medium, hour, vec![]);
            let x = extract(&c, &v);
            for i in 0..4 {
                let want = if i == expected_band_idx { 1.0 } else { 0.0 };
                assert!(
                    (x[4 + i] - want).abs() < 1e-6,
                    "hour {} band {} mismatch",
                    hour,
                    i
                );
            }
        }
    }

    #[test]
    fn extract_horizon_and_options_flags() {
        let mut c = fresh(Stakes::Medium, 10, vec![]);
        c.horizon = Some(chrono::Utc::now());
        c.options_considered = vec!["a".into(), "b".into(), "c".into()];
        let v = TagVocab::default();
        let x = extract(&c, &v);
        assert_eq!(x[8], 1.0);
        assert_eq!(x[9], 1.0);
    }

    #[test]
    fn extract_kind_one_hot_distinct() {
        let v = TagVocab::default();
        let mut intent = fresh(Stakes::Medium, 10, vec![]);
        intent.kind = CommitmentKind::Intent;
        let mut decision = fresh(Stakes::Medium, 10, vec![]);
        decision.kind = CommitmentKind::Decision;
        let mut hypothesis = fresh(Stakes::Medium, 10, vec![]);
        hypothesis.kind = CommitmentKind::Hypothesis;
        assert_eq!(extract(&intent, &v)[11..14], [1.0, 0.0, 0.0]);
        assert_eq!(extract(&decision, &v)[11..14], [0.0, 1.0, 0.0]);
        assert_eq!(extract(&hypothesis, &v)[11..14], [0.0, 0.0, 1.0]);
    }
}
