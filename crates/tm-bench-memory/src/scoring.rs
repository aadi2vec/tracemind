//! SQuAD-style token F1 + exact-match scorers.
//!
//! Kept deliberately separate from `tm-bench-locomo::scoring` rather than
//! cross-depended for two reasons: (1) the benches will likely diverge
//! (LoCoMo gets an LLM-judge follow-up; W-3 stays deterministic forever
//! so it can run in CI on every PR), and (2) one crate failing to build
//! shouldn't take the other down.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Score(pub f32);

impl Score {
    pub const ZERO: Score = Score(0.0);
    pub const ONE: Score = Score(1.0);
    pub fn percent(self) -> f32 {
        self.0 * 100.0
    }
}

impl From<f32> for Score {
    fn from(v: f32) -> Self {
        Score(v.clamp(0.0, 1.0))
    }
}

fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let no_punct: String = lower
        .chars()
        .map(|c| if c.is_ascii_punctuation() { ' ' } else { c })
        .collect();
    let stripped: Vec<&str> = no_punct
        .split_whitespace()
        .filter(|tok| !matches!(*tok, "a" | "an" | "the"))
        .collect();
    stripped.join(" ")
}

fn tokens(s: &str) -> Vec<String> {
    normalize(s).split_whitespace().map(str::to_string).collect()
}

pub fn exact_match(prediction: &str, reference: &str) -> Score {
    if normalize(prediction) == normalize(reference) {
        Score::ONE
    } else {
        Score::ZERO
    }
}

pub fn token_f1(prediction: &str, reference: &str) -> Score {
    let pred = tokens(prediction);
    let refr = tokens(reference);
    if pred.is_empty() || refr.is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_strips_articles() {
        assert_eq!(exact_match("The Tokyo", "tokyo").0, 1.0);
    }

    #[test]
    fn f1_identical() {
        assert!((token_f1("hello world", "hello world").0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn f1_partial() {
        assert!((token_f1("hello world", "hello there").0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn f1_disjoint() {
        assert_eq!(token_f1("a b", "c d").0, 0.0);
    }

    #[test]
    fn best_f1_takes_max() {
        let refs = vec!["may 3".into(), "may 3rd".into(), "2026-05-03".into()];
        assert!((best_f1("May 3rd", &refs).0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_clamps() {
        let s: Score = 1.5f32.into();
        assert_eq!(s.0, 1.0);
        let s: Score = (-1.0f32).into();
        assert_eq!(s.0, 0.0);
    }

    #[test]
    fn score_percent() {
        assert!((Score(0.873).percent() - 87.3).abs() < 1e-4);
    }
}
