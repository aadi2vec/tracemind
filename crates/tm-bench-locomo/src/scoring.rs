//! Reproducible scorers for LoCoMo answers.
//!
//! We start with the SQuAD-style token F1 + exact-match pair because:
//! - **Reproducible**: no LLM-judge variability, runs deterministic in CI.
//! - **Well-understood**: every published LoCoMo baseline reports F1, so we
//!   can compare apples-to-apples.
//! - **Fast**: the CI gate needs to run in seconds, not minutes.
//!
//! An optional LLM-judge scorer (via Tier 1 / Tier 2 in `tm-answer`) will land
//! in a follow-up once Tier 1 is wired. Until then, F1 is our gate.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Scalar score in [0.0, 1.0].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Score(pub f32);

impl Score {
    pub const ZERO: Score = Score(0.0);
    pub const ONE: Score = Score(1.0);

    pub fn percent(self) -> f32 {
        self.0 * 100.0
    }
}

/// SQuAD-style normalization: lowercase, strip articles, strip punctuation,
/// collapse whitespace. This is the canonical prep for token F1 on QA.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // Drop punctuation.
    let no_punct: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_punctuation() {
                ' '
            } else {
                c
            }
        })
        .collect();
    // Strip articles and collapse whitespace.
    let stripped: Vec<&str> = no_punct
        .split_whitespace()
        .filter(|tok| !matches!(*tok, "a" | "an" | "the"))
        .collect();
    stripped.join(" ")
}

fn tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .map(|t| t.to_string())
        .collect()
}

/// Exact-match after normalization. 1.0 if equal, else 0.0.
pub fn exact_match(prediction: &str, reference: &str) -> Score {
    if normalize(prediction) == normalize(reference) {
        Score::ONE
    } else {
        Score::ZERO
    }
}

/// SQuAD token F1. Computes precision/recall over a bag of tokens and
/// returns their harmonic mean. Returns 0.0 if either side is empty.
pub fn token_f1(prediction: &str, reference: &str) -> Score {
    let pred = tokens(prediction);
    let refr = tokens(reference);
    if pred.is_empty() || refr.is_empty() {
        // SQuAD convention: if both are empty, treat as match; else no-match.
        return if pred.is_empty() && refr.is_empty() {
            Score::ONE
        } else {
            Score::ZERO
        };
    }
    let mut pred_counts: HashMap<&str, usize> = HashMap::new();
    for t in &pred {
        *pred_counts.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut ref_counts: HashMap<&str, usize> = HashMap::new();
    for t in &refr {
        *ref_counts.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut overlap = 0usize;
    for (tok, pc) in &pred_counts {
        if let Some(rc) = ref_counts.get(tok) {
            overlap += (*pc).min(*rc);
        }
    }
    if overlap == 0 {
        return Score::ZERO;
    }
    let precision = overlap as f32 / pred.len() as f32;
    let recall = overlap as f32 / refr.len() as f32;
    Score(2.0 * precision * recall / (precision + recall))
}

/// Best score across a set of reference answers. LoCoMo questions often list
/// multiple acceptable answers; per SQuAD convention we take the max.
pub fn best_f1(prediction: &str, references: &[String]) -> Score {
    references
        .iter()
        .map(|r| token_f1(prediction, r).0)
        .fold(0.0f32, f32::max)
        .into()
}

pub fn best_exact_match(prediction: &str, references: &[String]) -> Score {
    references
        .iter()
        .map(|r| exact_match(prediction, r).0)
        .fold(0.0f32, f32::max)
        .into()
}

impl From<f32> for Score {
    fn from(v: f32) -> Self {
        Score(v.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_identical() {
        assert_eq!(exact_match("Alice", "alice").0, 1.0);
    }

    #[test]
    fn exact_match_strips_articles_and_punct() {
        assert_eq!(exact_match("The cat.", "cat").0, 1.0);
        assert_eq!(exact_match("a dog!", "dog").0, 1.0);
    }

    #[test]
    fn exact_match_different() {
        assert_eq!(exact_match("Alice", "Bob").0, 0.0);
    }

    #[test]
    fn f1_identical_is_one() {
        assert!((token_f1("hello world", "hello world").0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f1_disjoint_is_zero() {
        assert_eq!(token_f1("alpha beta", "gamma delta").0, 0.0);
    }

    #[test]
    fn f1_partial_overlap() {
        // pred = [hello, world], ref = [hello, there]; overlap=1, p=0.5, r=0.5, F1=0.5
        let score = token_f1("hello world", "hello there").0;
        assert!((score - 0.5).abs() < 1e-6);
    }

    #[test]
    fn f1_ignores_articles() {
        // "the cat" vs "cat" → after normalize both = ["cat"]
        assert!((token_f1("the cat", "cat").0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f1_both_empty_is_one() {
        assert_eq!(token_f1("", "").0, 1.0);
    }

    #[test]
    fn f1_one_empty_is_zero() {
        assert_eq!(token_f1("hello", "").0, 0.0);
        assert_eq!(token_f1("", "hello").0, 0.0);
    }

    #[test]
    fn best_f1_takes_max() {
        let refs = vec!["blue".to_string(), "cerulean".to_string()];
        // pred matches second reference exactly
        assert!((best_f1("cerulean", &refs).0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn best_f1_empty_refs_is_zero() {
        assert_eq!(best_f1("anything", &[]).0, 0.0);
    }

    #[test]
    fn score_percent() {
        assert!((Score(0.873).percent() - 87.3).abs() < 1e-4);
    }

    #[test]
    fn score_clamps_from_f32() {
        let s: Score = 1.5f32.into();
        assert_eq!(s.0, 1.0);
        let s: Score = (-0.2f32).into();
        assert_eq!(s.0, 0.0);
    }
}
