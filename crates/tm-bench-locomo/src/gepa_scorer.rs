//! A [`PolicyScorer`] backed by the real ingest + retrieval + answer stack.
//!
//! This is what makes the verifier gate honest: every candidate policy is
//! applied to a live `RetrievalEngine`, the anchor questions are answered
//! through the same code path the product uses, and the resulting
//! predictions are scored with the same token-F1 the benchmark reports.
//!
//! Per-instance feedback strings are emitted in the vocabulary
//! [`tm_gepa::reflect::diagnose`] consumes (`no-grounding`, `wrong-span`,
//! `low-precision`), so the reflection step reads a real diagnosis of a
//! real failure rather than a scalar.

use std::time::Instant;

use tm_gepa::{InstanceScore, PolicyScorer, RetrievalPolicy, ScoreReport};

use crate::dataset::LocomoDataset;
use crate::scoring::{best_exact_match, best_f1};
use crate::tracemind_runner::{TraceMindConfig, TraceMindRunner};

/// Scores retrieval policies against a LoCoMo dataset using the real stack.
pub struct LocomoPolicyScorer {
    dataset: LocomoDataset,
    config: TraceMindConfig,
    anchor_count: usize,
}

impl LocomoPolicyScorer {
    pub fn new(dataset: LocomoDataset, config: TraceMindConfig) -> Self {
        let anchor_count = dataset
            .samples
            .iter()
            .map(|s| s.questions.len())
            .sum();
        Self {
            dataset,
            config,
            anchor_count,
        }
    }
}

impl PolicyScorer for LocomoPolicyScorer {
    fn score(&mut self, policy: &RetrievalPolicy) -> ScoreReport {
        let start = Instant::now();
        let mut instances = Vec::with_capacity(self.anchor_count);

        // A fresh runner per evaluation: the policy is applied to the
        // engine at open time, and reusing a runner across policies would
        // leak the previous configuration's cache state into the result.
        let mut runner = TraceMindRunner::new(self.config.clone());

        for sample in &self.dataset.samples {
            if runner.ingest_sample_sync(sample).is_err() {
                // Ingest failure scores every question in the sample as a
                // total miss rather than silently shrinking the anchor set,
                // which would make a broken policy look better.
                for q in &sample.questions {
                    instances.push(InstanceScore {
                        id: format!("{}::{}", sample.sample_id, q.id),
                        f1: 0.0,
                        exact_match: 0.0,
                        feedback: "no-grounding (ingest failed)".to_string(),
                    });
                }
                continue;
            }
            runner.apply_policy(policy);

            for q in &sample.questions {
                let (prediction, grounding_count) = runner.answer_sync(q);
                let f1 = best_f1(&prediction, &q.answers).0;
                let em = best_exact_match(&prediction, &q.answers).0;

                instances.push(InstanceScore {
                    id: format!("{}::{}", sample.sample_id, q.id),
                    f1,
                    exact_match: em,
                    feedback: classify_failure(f1, grounding_count, &prediction),
                });
            }
        }

        ScoreReport::from_instances(instances, start.elapsed().as_millis() as u64)
    }

    fn anchor_count(&self) -> usize {
        self.anchor_count
    }
}

/// Map an outcome onto the failure vocabulary the reflection step reads.
fn classify_failure(f1: f32, grounding_count: usize, prediction: &str) -> String {
    if f1 >= 0.5 {
        return String::new();
    }
    if grounding_count == 0 || prediction.trim().is_empty() {
        // Retrieval surfaced nothing to answer from.
        "no-grounding".to_string()
    } else if grounding_count >= 8 && prediction.split_whitespace().count() > 12 {
        // Lots of candidates and a long, diluted answer.
        "low-precision".to_string()
    } else {
        // Grounding existed but the chosen span was wrong.
        "wrong-span".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passing_instance_has_no_feedback() {
        assert!(classify_failure(0.9, 3, "Haneda").is_empty());
    }

    #[test]
    fn empty_prediction_is_no_grounding() {
        assert_eq!(classify_failure(0.0, 0, ""), "no-grounding");
    }

    #[test]
    fn zero_grounding_is_no_grounding_even_with_text() {
        assert_eq!(classify_failure(0.0, 0, "something"), "no-grounding");
    }

    #[test]
    fn long_answer_with_many_candidates_is_low_precision() {
        let long = "a b c d e f g h i j k l m n o p";
        assert_eq!(classify_failure(0.1, 10, long), "low-precision");
    }

    #[test]
    fn short_wrong_answer_is_wrong_span() {
        assert_eq!(classify_failure(0.0, 3, "Loom"), "wrong-span");
    }
}
