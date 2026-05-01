//! Public types for `tm-world-model`.
//!
//! v0 keeps the API tiny on purpose. Everything externally observable goes
//! through three types:
//!
//! - [`PolarityClass`] — the four predictable outcome polarities (we drop
//!   `NoOutcome` from training because the pattern detector excludes it
//!   anyway and it's a structural state, not a quality signal).
//! - [`PolarityDist`] — the predictor's softmax output.
//! - [`OutcomePrediction`] — distribution + top class + scalar confidence
//!   + count of nearest priors used (for "show your work").
//!
//! These types are pure data + serde. The math lives in
//! [`crate::predictor`] and [`crate::trainer`].

use serde::{Deserialize, Serialize};
use tm_intent::Polarity;

/// The four polarities the world model predicts. `Polarity::NoOutcome`
/// is *legitimate* in the data model but excluded from prediction
/// targets — see `INTENT_SYSTEM.md` §5.2 (pattern detector skips it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolarityClass {
    Better,
    AsExpected,
    Worse,
    Mixed,
}

impl PolarityClass {
    /// All classes in canonical index order. The `usize` is stable on disk.
    pub const ALL: [PolarityClass; 4] = [
        PolarityClass::Better,
        PolarityClass::AsExpected,
        PolarityClass::Worse,
        PolarityClass::Mixed,
    ];

    pub fn index(self) -> usize {
        match self {
            PolarityClass::Better => 0,
            PolarityClass::AsExpected => 1,
            PolarityClass::Worse => 2,
            PolarityClass::Mixed => 3,
        }
    }

    pub fn from_index(i: usize) -> Option<PolarityClass> {
        Self::ALL.get(i).copied()
    }

    /// Map a `tm-intent::Polarity` into a `PolarityClass`. Returns `None`
    /// for `NoOutcome` — caller decides whether to skip the example.
    pub fn from_polarity(p: Polarity) -> Option<PolarityClass> {
        match p {
            Polarity::Better => Some(PolarityClass::Better),
            Polarity::AsExpected => Some(PolarityClass::AsExpected),
            Polarity::Worse => Some(PolarityClass::Worse),
            Polarity::Mixed => Some(PolarityClass::Mixed),
            Polarity::NoOutcome => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PolarityClass::Better => "better",
            PolarityClass::AsExpected => "as_expected",
            PolarityClass::Worse => "worse",
            PolarityClass::Mixed => "mixed",
        }
    }
}

/// Softmax distribution over the four polarity classes, indexed by
/// [`PolarityClass::index`]. Always sums to ~1.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarityDist(pub [f32; 4]);

impl PolarityDist {
    pub fn uniform() -> Self {
        Self([0.25; 4])
    }

    pub fn argmax(&self) -> PolarityClass {
        let (mut best_i, mut best_v) = (0usize, self.0[0]);
        for (i, v) in self.0.iter().enumerate().skip(1) {
            if *v > best_v {
                best_v = *v;
                best_i = i;
            }
        }
        PolarityClass::from_index(best_i).unwrap_or(PolarityClass::Mixed)
    }

    /// Probability of "positive" (Better + AsExpected). The single most
    /// useful scalar — drives the L3 surface ("you're 0.7 likely to close
    /// this positive") and is what the pattern detector compares against
    /// the global rate.
    pub fn positive_prob(&self) -> f32 {
        self.0[PolarityClass::Better.index()] + self.0[PolarityClass::AsExpected.index()]
    }
}

/// The full prediction surface. `confidence` is the max class probability
/// (a poor proxy for calibration but honest about it). `n_priors` is how
/// many training examples the model has seen — the brief uses this to
/// decide whether to surface predictions at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomePrediction {
    pub dist: PolarityDist,
    pub argmax: PolarityClass,
    pub positive_prob: f32,
    pub confidence: f32,
    pub n_priors: usize,
}

impl OutcomePrediction {
    pub fn from_dist(dist: PolarityDist, n_priors: usize) -> Self {
        let argmax = dist.argmax();
        let positive_prob = dist.positive_prob();
        let confidence = dist.0[argmax.index()];
        Self {
            dist,
            argmax,
            positive_prob,
            confidence,
            n_priors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polarity_class_round_trip() {
        for c in PolarityClass::ALL {
            assert_eq!(PolarityClass::from_index(c.index()), Some(c));
        }
    }

    #[test]
    fn polarity_class_skips_no_outcome() {
        assert!(PolarityClass::from_polarity(Polarity::NoOutcome).is_none());
        assert_eq!(
            PolarityClass::from_polarity(Polarity::Worse),
            Some(PolarityClass::Worse),
        );
    }

    #[test]
    fn dist_uniform_sums_to_one() {
        let d = PolarityDist::uniform();
        let s: f32 = d.0.iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dist_argmax_picks_largest() {
        let d = PolarityDist([0.1, 0.6, 0.2, 0.1]);
        assert_eq!(d.argmax(), PolarityClass::AsExpected);
    }

    #[test]
    fn positive_prob_combines_better_and_as_expected() {
        let d = PolarityDist([0.3, 0.4, 0.2, 0.1]);
        // 0.3 + 0.4 = 0.7
        assert!((d.positive_prob() - 0.7).abs() < 1e-6);
    }
}
