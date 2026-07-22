//! The concrete artifact the GEPA loop optimises.
//!
//! The original spike mutated an abstract per-arm weight vector that no
//! crate consumed, so a "policy" could never change what retrieval actually
//! did. [`RetrievalPolicy`] instead holds exactly the knobs the live
//! retrieval path reads:
//!
//! - `space_weights` — the `ComposedIndex` fusion weights for the `recall`
//!   verb (`text` dense cosine, `lexical` BM25, `recency`, `confidence`).
//! - `min_score` — floor on the fused score for a signal to be returned.
//! - `candidate_multiplier` — how wide candidate generation goes before
//!   fusion re-ranks.
//!
//! Applying a policy is therefore a real configuration change with a
//! measurable effect, which is the precondition for the verifier gate in
//! [`crate::verifier`] to mean anything.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Space names the fusion policy can weight.
pub const SPACE_NAMES: &[&str] = &["text", "lexical", "recency", "confidence"];

/// A concrete, executable retrieval configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalPolicy {
    /// Fusion weights per space. `BTreeMap` so serialisation and equality
    /// are deterministic (candidate dedup in the archive relies on it).
    pub space_weights: BTreeMap<String, f32>,
    /// Floor on the fused ComposedIndex score.
    pub min_score: f32,
    /// Candidate pool width, as a multiple of the requested top_k.
    pub candidate_multiplier: usize,
    /// How much question-token coverage of a candidate's text boosts it
    /// during answer selection, relative to its retrieval score.
    pub coverage_weight: f32,
    /// How much a syntactic match between question and candidate structure
    /// (e.g. the question's trailing preposition having an object in the
    /// candidate) boosts it during answer selection.
    pub fit_weight: f32,
    /// Multiplier applied to `coverage_weight` for questions with no
    /// answer-type constraint ("why…?"). There, every candidate passes the
    /// type filter, so lexical overlap is the only signal separating them
    /// and deserves far more weight than it does for typed questions.
    pub generic_coverage_boost: f32,
}

impl Default for RetrievalPolicy {
    fn default() -> Self {
        // Values below are the output of a GEPA run against the LoCoMo
        // anchor set (docs/REVIEW-2026-07.md), not hand-picked: the loop
        // moved mean F1 from 69.91 to 75.49 over 75 executed rollouts, with
        // the final gain coming from the system-aware merge step.
        let mut space_weights = BTreeMap::new();
        space_weights.insert("text".to_string(), 0.478);
        space_weights.insert("lexical".to_string(), 0.348);
        space_weights.insert("recency".to_string(), 0.087);
        space_weights.insert("confidence".to_string(), 0.087);
        Self {
            space_weights,
            min_score: 0.15,
            candidate_multiplier: 4,
            coverage_weight: 0.08,
            fit_weight: 1.0,
            generic_coverage_boost: 15.4,
        }
    }
}

impl RetrievalPolicy {
    /// Weights renormalised to sum to 1.0.
    pub fn normalised_weights(&self) -> BTreeMap<String, f32> {
        let total: f32 = self.space_weights.values().sum();
        if total <= 0.0 {
            return self.space_weights.clone();
        }
        self.space_weights
            .iter()
            .map(|(k, v)| (k.clone(), v / total))
            .collect()
    }

    /// Perturb one space weight, clamped and renormalised.
    pub fn perturb_space(&self, space: &str, delta: f32) -> Self {
        let mut next = self.clone();
        if let Some(w) = next.space_weights.get_mut(space) {
            *w = (*w + delta).clamp(0.0, 1.0);
        }
        let total: f32 = next.space_weights.values().sum();
        if total > 0.0 {
            for w in next.space_weights.values_mut() {
                *w /= total;
            }
        }
        next
    }

    /// Perturb the fused-score floor.
    pub fn perturb_min_score(&self, delta: f32) -> Self {
        let mut next = self.clone();
        next.min_score = (next.min_score + delta).clamp(0.0, 0.9);
        next
    }

    /// Perturb the answer-selection coverage boost.
    pub fn perturb_coverage(&self, delta: f32) -> Self {
        let mut next = self.clone();
        next.coverage_weight = (next.coverage_weight + delta).clamp(0.0, 3.0);
        next
    }

    /// Perturb the answer-selection syntactic-fit boost.
    pub fn perturb_fit(&self, delta: f32) -> Self {
        let mut next = self.clone();
        next.fit_weight = (next.fit_weight + delta).clamp(0.0, 3.0);
        next
    }

    /// Perturb the untyped-question coverage boost.
    pub fn perturb_generic_boost(&self, delta: f32) -> Self {
        let mut next = self.clone();
        next.generic_coverage_boost = (next.generic_coverage_boost + delta).clamp(0.0, 60.0);
        next
    }

    /// Widen or narrow candidate generation.
    pub fn perturb_candidates(&self, delta: i32) -> Self {
        let mut next = self.clone();
        let m = next.candidate_multiplier as i32 + delta;
        next.candidate_multiplier = m.clamp(1, 16) as usize;
        next
    }

    /// Merge two policies field-by-field.
    ///
    /// GEPA's "system-aware merge": when two archived candidates each solve
    /// instances the other misses, their wins usually come from *different*
    /// knobs, so a policy taking each knob from one parent or the other can
    /// beat both. Mutation alone cannot reach such a point, because getting
    /// there requires moving several knobs at once in directions that are
    /// individually neutral. `bias` in [0,1] picks how much of `other` to
    /// take on the continuous fields.
    pub fn merge(&self, other: &Self, bias: f32) -> Self {
        let b = bias.clamp(0.0, 1.0);
        let lerp = |x: f32, y: f32| x + (y - x) * b;

        let mut space_weights = self.space_weights.clone();
        for (k, v) in &mut space_weights {
            let o = other.space_weights.get(k).copied().unwrap_or(*v);
            *v = lerp(*v, o);
        }
        Self {
            space_weights,
            min_score: lerp(self.min_score, other.min_score),
            candidate_multiplier: if b >= 0.5 {
                other.candidate_multiplier
            } else {
                self.candidate_multiplier
            },
            coverage_weight: lerp(self.coverage_weight, other.coverage_weight),
            fit_weight: lerp(self.fit_weight, other.fit_weight),
            generic_coverage_boost: lerp(
                self.generic_coverage_boost,
                other.generic_coverage_boost,
            ),
        }
    }

    /// Stable key for archive dedup — two policies that round to the same
    /// configuration should not both occupy archive slots.
    pub fn fingerprint(&self) -> String {
        let w = self.normalised_weights();
        let parts: Vec<String> = w
            .iter()
            .map(|(k, v)| format!("{k}={:.3}", v))
            .collect();
        format!(
            "{}|min={:.3}|cm={}|cov={:.3}|fit={:.3}|gcb={:.3}",
            parts.join(","),
            self.min_score,
            self.candidate_multiplier,
            self.coverage_weight,
            self.fit_weight,
            self.generic_coverage_boost
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one() {
        let p = RetrievalPolicy::default();
        let total: f32 = p.normalised_weights().values().sum();
        assert!((total - 1.0).abs() < 1e-5, "total = {total}");
    }

    #[test]
    fn perturb_space_renormalises() {
        let p = RetrievalPolicy::default().perturb_space("lexical", 0.3);
        let total: f32 = p.space_weights.values().sum();
        assert!((total - 1.0).abs() < 1e-5, "total = {total}");
        assert!(p.space_weights["lexical"] > p.space_weights["text"]);
    }

    #[test]
    fn perturb_space_ignores_unknown_space() {
        let p = RetrievalPolicy::default();
        let q = p.perturb_space("nonexistent", 0.5);
        assert_eq!(p.normalised_weights(), q.normalised_weights());
    }

    #[test]
    fn min_score_is_clamped() {
        let p = RetrievalPolicy::default().perturb_min_score(-5.0);
        assert_eq!(p.min_score, 0.0);
        let p = RetrievalPolicy::default().perturb_min_score(5.0);
        assert_eq!(p.min_score, 0.9);
    }

    #[test]
    fn candidate_multiplier_is_clamped() {
        assert_eq!(
            RetrievalPolicy::default().perturb_candidates(-100).candidate_multiplier,
            1
        );
        assert_eq!(
            RetrievalPolicy::default().perturb_candidates(100).candidate_multiplier,
            16
        );
    }

    #[test]
    fn fingerprint_distinguishes_policies() {
        let a = RetrievalPolicy::default();
        let b = a.perturb_space("lexical", 0.2);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn merge_at_zero_bias_returns_self() {
        let a = RetrievalPolicy::default();
        let b = a.perturb_coverage(1.0).perturb_min_score(0.3);
        assert_eq!(a.merge(&b, 0.0).fingerprint(), a.fingerprint());
    }

    #[test]
    fn merge_at_full_bias_returns_other() {
        let a = RetrievalPolicy::default();
        let b = a.perturb_coverage(1.0).perturb_min_score(0.3);
        assert_eq!(a.merge(&b, 1.0).fingerprint(), b.fingerprint());
    }

    #[test]
    fn merge_interpolates_continuous_fields() {
        let a = RetrievalPolicy::default();
        let b = a.perturb_coverage(1.0);
        let m = a.merge(&b, 0.5);
        assert!(m.coverage_weight > a.coverage_weight);
        assert!(m.coverage_weight < b.coverage_weight);
    }

    #[test]
    fn fingerprint_is_stable() {
        let a = RetrievalPolicy::default();
        assert_eq!(a.fingerprint(), a.clone().fingerprint());
    }
}
