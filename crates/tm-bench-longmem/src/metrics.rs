//! Shared evaluation metrics for LongMemEval and BEAM.
//!
//! Uses SQuAD-style token F1 + exact-match, matching the LoCoMo harness in
//! `tm-bench-locomo` so scores are cross-comparable without LLM-judge overhead.

use std::collections::HashMap;

/// Aggregate metrics for a complete evaluation run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalMetrics {
    /// Macro-averaged token F1 across all cases (0.0–1.0).
    pub token_f1: f64,
    /// Fraction of cases with exact match (0.0–1.0).
    pub exact_match_rate: f64,
    /// Total number of cases evaluated.
    pub count: usize,
    /// Per-category breakdown keyed by category name string.
    pub by_category: HashMap<String, CategoryMetrics>,
}

/// Per-category aggregate metrics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CategoryMetrics {
    /// Macro-averaged token F1 for this category.
    pub token_f1: f64,
    /// Exact-match rate for this category.
    pub exact_match_rate: f64,
    /// Number of cases in this category.
    pub count: usize,
}

impl EvalMetrics {
    /// Build `EvalMetrics` from per-case `(category, token_f1, exact_match)` tuples.
    pub fn from_results(results: &[(String, f64, bool)]) -> Self {
        if results.is_empty() {
            return Self {
                token_f1: 0.0,
                exact_match_rate: 0.0,
                count: 0,
                by_category: HashMap::new(),
            };
        }

        // Accumulate per-category.
        let mut cat_map: HashMap<String, (f64, usize, usize)> = HashMap::new(); // (f1_sum, em_count, total)
        for (cat, f1, em) in results {
            let entry = cat_map.entry(cat.clone()).or_insert((0.0, 0, 0));
            entry.0 += f1;
            if *em {
                entry.1 += 1;
            }
            entry.2 += 1;
        }

        let by_category: HashMap<String, CategoryMetrics> = cat_map
            .into_iter()
            .map(|(cat, (f1_sum, em_count, total))| {
                (
                    cat,
                    CategoryMetrics {
                        token_f1: f1_sum / total as f64,
                        exact_match_rate: em_count as f64 / total as f64,
                        count: total,
                    },
                )
            })
            .collect();

        let total_f1: f64 = results.iter().map(|(_, f1, _)| f1).sum();
        let em_count = results.iter().filter(|(_, _, em)| *em).count();
        let count = results.len();

        Self {
            token_f1: total_f1 / count as f64,
            exact_match_rate: em_count as f64 / count as f64,
            count,
            by_category,
        }
    }
}

/// SQuAD-style normalization: lowercase, strip articles, strip ASCII punctuation,
/// collapse whitespace.
fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let no_punct: String = lower
        .chars()
        .map(|c| if c.is_ascii_punctuation() { ' ' } else { c })
        .collect();
    no_punct
        .split_whitespace()
        .filter(|tok| !matches!(*tok, "a" | "an" | "the"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokenize(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .map(|t| t.to_string())
        .collect()
}

/// SQuAD token F1 between `pred` and `gold` (both normalized).
///
/// Returns a value in \[0.0, 1.0\].
pub fn compute_token_f1(pred: &str, gold: &str) -> f64 {
    let pred_tokens = tokenize(pred);
    let gold_tokens = tokenize(gold);

    if pred_tokens.is_empty() || gold_tokens.is_empty() {
        // Both empty → perfect match; one empty → no match.
        return if pred_tokens.is_empty() && gold_tokens.is_empty() {
            1.0
        } else {
            0.0
        };
    }

    let mut pred_counts: HashMap<&str, usize> = HashMap::new();
    for t in &pred_tokens {
        *pred_counts.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut gold_counts: HashMap<&str, usize> = HashMap::new();
    for t in &gold_tokens {
        *gold_counts.entry(t.as_str()).or_insert(0) += 1;
    }

    let overlap: usize = pred_counts
        .iter()
        .filter_map(|(tok, &pc)| gold_counts.get(tok).map(|&gc| pc.min(gc)))
        .sum();

    if overlap == 0 {
        return 0.0;
    }

    let precision = overlap as f64 / pred_tokens.len() as f64;
    let recall = overlap as f64 / gold_tokens.len() as f64;
    2.0 * precision * recall / (precision + recall)
}

/// Best token F1 across multiple gold spans (SQuAD-style: take max).
pub fn compute_token_f1_best(pred: &str, gold_spans: &[String]) -> f64 {
    gold_spans
        .iter()
        .map(|g| compute_token_f1(pred, g))
        .fold(0.0f64, f64::max)
}

/// Exact match after normalization. Returns `true` if normalized strings are equal.
pub fn compute_em(pred: &str, gold: &str) -> bool {
    normalize(pred) == normalize(gold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f1_identical() {
        assert!((compute_token_f1("hello world", "hello world") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn f1_disjoint_is_zero() {
        assert_eq!(compute_token_f1("alpha beta", "gamma delta"), 0.0);
    }

    #[test]
    fn f1_partial_overlap() {
        // pred=[hello,world] gold=[hello,there] → overlap=1, p=0.5, r=0.5 → F1=0.5
        let score = compute_token_f1("hello world", "hello there");
        assert!((score - 0.5).abs() < 1e-9);
    }

    #[test]
    fn f1_ignores_articles() {
        assert!((compute_token_f1("the cat", "cat") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn f1_both_empty_is_one() {
        assert_eq!(compute_token_f1("", ""), 1.0);
    }

    #[test]
    fn f1_one_empty_is_zero() {
        assert_eq!(compute_token_f1("hello", ""), 0.0);
        assert_eq!(compute_token_f1("", "hello"), 0.0);
    }

    #[test]
    fn em_normalized_match() {
        assert!(compute_em("Acme Corp.", "acme corp"));
        assert!(!compute_em("Acme", "Beta"));
    }

    #[test]
    fn em_strips_articles() {
        assert!(compute_em("The quick fox", "quick fox"));
    }

    #[test]
    fn best_f1_takes_max() {
        let spans = vec!["Acme".to_string(), "Acme Corp".to_string()];
        let score = compute_token_f1_best("Acme Corp", &spans);
        assert!((score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn eval_metrics_from_results() {
        let results = vec![
            ("single_hop".to_string(), 1.0, true),
            ("single_hop".to_string(), 0.5, false),
            ("multi_hop".to_string(), 0.8, true),
        ];
        let metrics = EvalMetrics::from_results(&results);
        assert_eq!(metrics.count, 3);
        assert!((metrics.token_f1 - (1.0 + 0.5 + 0.8) / 3.0).abs() < 1e-9);
        assert!((metrics.exact_match_rate - 2.0 / 3.0).abs() < 1e-9);
        assert!(metrics.by_category.contains_key("single_hop"));
        assert!(metrics.by_category.contains_key("multi_hop"));
    }

    #[test]
    fn eval_metrics_empty() {
        let metrics = EvalMetrics::from_results(&[]);
        assert_eq!(metrics.count, 0);
        assert_eq!(metrics.token_f1, 0.0);
    }
}
