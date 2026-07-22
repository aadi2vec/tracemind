//! Reflective mutation — proposing policy changes from failure evidence.
//!
//! The spike's mutation operator rotated the sentences of a prompt string
//! and perturbed a random weight. Sentence rotation has no gradient (it
//! cycles through `n` permutations), and a uniformly random weight step
//! ignores everything the evaluation just revealed.
//!
//! GEPA's actual mechanism is *reflection*: read the execution traces of
//! the failures, diagnose them in natural language, and propose a targeted
//! change. This module implements the diagnosis half — it turns a
//! [`ScoreReport`]'s failures into typed [`Diagnosis`] values with a
//! natural-language rationale, and proposes the policy edit each diagnosis
//! implies.
//!
//! The diagnoser is rule-based rather than LLM-driven. That is a deliberate
//! constraint for an on-device nightly loop: it is deterministic,
//! reproducible, costs no tokens, and needs no model resident in memory.
//! The [`Diagnosis::rationale`] strings are written to be exactly the input
//! an LLM reflection step would consume, so swapping in a Tier-1/Tier-2
//! reflector later is a drop-in replacement rather than a redesign.

use serde::{Deserialize, Serialize};

use crate::retrieval_policy::RetrievalPolicy;
use crate::scorer::ScoreReport;

/// What the evidence says went wrong, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Diagnosis {
    /// Retrieval never surfaced the answer-bearing memory. Widen.
    RecallStarved,
    /// The right memory was retrieved but ranked below noise. Rebalance
    /// toward exact-token matching.
    RankedTooLow,
    /// Too much irrelevant material came back. Tighten the floor.
    PrecisionStarved,
    /// Nothing clearly wrong; explore.
    Explore,
}

impl Diagnosis {
    pub fn rationale(&self) -> &'static str {
        match self {
            Diagnosis::RecallStarved => {
                "Most failures returned no grounding at all. The candidate pool or the \
                 score floor is excluding the answer-bearing memory before ranking runs. \
                 Widen candidate generation and lower the floor."
            }
            Diagnosis::RankedTooLow => {
                "Failures returned grounding, but not the memory holding the answer. \
                 Dense cosine is ranking a topically-similar memory above the one with \
                 the exact tokens. Shift weight toward the lexical space."
            }
            Diagnosis::PrecisionStarved => {
                "Failures returned many weakly-related memories, diluting the answer. \
                 Raise the score floor and narrow the candidate pool."
            }
            Diagnosis::Explore => {
                "No dominant failure mode. Take an exploratory step to escape a local \
                 optimum."
            }
        }
    }
}

/// Classify the dominant failure mode in a report.
///
/// `empty_rate` is the share of failures whose feedback indicates no
/// grounding was returned at all.
pub fn diagnose(report: &ScoreReport) -> Diagnosis {
    let failures = report.failures(0.5);
    if failures.is_empty() {
        return Diagnosis::Explore;
    }

    let n = failures.len() as f32;
    let empty = failures
        .iter()
        .filter(|f| f.feedback.contains("no-grounding"))
        .count() as f32;
    let diluted = failures
        .iter()
        .filter(|f| f.feedback.contains("low-precision"))
        .count() as f32;

    if empty / n >= 0.4 {
        Diagnosis::RecallStarved
    } else if diluted / n >= 0.4 {
        Diagnosis::PrecisionStarved
    } else {
        Diagnosis::RankedTooLow
    }
}

/// A proposed policy change with the reasoning that produced it.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub policy: RetrievalPolicy,
    pub diagnosis: Diagnosis,
    pub rationale: String,
}

/// Propose candidate policies from a parent and the evidence about it.
///
/// Returns several candidates around the diagnosed direction rather than
/// one, because the verifier is cheap relative to the value of not getting
/// stuck, and the per-instance archive can keep more than one of them.
pub fn propose(parent: &RetrievalPolicy, report: &ScoreReport, step: f32) -> Vec<Proposal> {
    let dx = diagnose(report);
    let mut out = Vec::new();

    let mut push = |policy: RetrievalPolicy, detail: &str| {
        out.push(Proposal {
            policy,
            diagnosis: dx.clone(),
            rationale: format!("{} [{}]", dx.rationale(), detail),
        });
    };

    match dx {
        Diagnosis::RecallStarved => {
            push(
                parent.perturb_min_score(-step).perturb_candidates(2),
                "lower floor, wider pool",
            );
            push(parent.perturb_candidates(4), "wider pool only");
            push(parent.perturb_min_score(-step * 2.0), "much lower floor");
        }
        Diagnosis::RankedTooLow => {
            push(parent.perturb_space("lexical", step), "boost lexical");
            push(parent.perturb_space("text", step), "boost dense");
            push(
                parent.perturb_space("lexical", step).perturb_space("recency", -step * 0.5),
                "lexical up, recency down",
            );
            // The reader's selection weights are as much a part of "which
            // memory answers this" as the retriever's fusion weights, so
            // they are searched in the same direction.
            push(parent.perturb_coverage(step * 4.0), "boost token coverage");
            push(parent.perturb_coverage(step * 16.0), "strongly boost token coverage");
            push(parent.perturb_coverage(-step * 4.0), "reduce token coverage");
            push(parent.perturb_fit(step * 4.0), "boost syntactic fit");
            push(parent.perturb_fit(-step * 4.0), "reduce syntactic fit");
            push(parent.perturb_generic_boost(step * 40.0), "boost untyped-question coverage");
            push(parent.perturb_generic_boost(step * 120.0), "strongly boost untyped-question coverage");
            push(parent.perturb_generic_boost(-step * 40.0), "reduce untyped-question coverage");
            push(parent.perturb_recency(step * 8.0), "prefer later memories more");
            push(parent.perturb_recency(-step * 8.0), "prefer later memories less");
        }
        Diagnosis::PrecisionStarved => {
            push(
                parent.perturb_min_score(step).perturb_candidates(-1),
                "raise floor, narrow pool",
            );
            push(parent.perturb_min_score(step * 2.0), "much higher floor");
        }
        Diagnosis::Explore => {
            push(parent.perturb_space("lexical", step), "explore lexical");
            push(parent.perturb_space("recency", step), "explore recency");
            push(parent.perturb_space("confidence", step), "explore confidence");
            push(parent.perturb_min_score(step), "explore floor");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::InstanceScore;

    fn rep(items: &[(&str, f32, &str)]) -> ScoreReport {
        ScoreReport::from_instances(
            items
                .iter()
                .map(|(id, f1, fb)| InstanceScore {
                    id: id.to_string(),
                    f1: *f1,
                    exact_match: 0.0,
                    feedback: fb.to_string(),
                })
                .collect(),
            10,
        )
    }

    #[test]
    fn all_passing_yields_explore() {
        let r = rep(&[("q1", 1.0, ""), ("q2", 0.9, "")]);
        assert_eq!(diagnose(&r), Diagnosis::Explore);
    }

    #[test]
    fn mostly_empty_grounding_is_recall_starved() {
        let r = rep(&[
            ("q1", 0.0, "no-grounding"),
            ("q2", 0.0, "no-grounding"),
            ("q3", 0.1, "wrong-span"),
        ]);
        assert_eq!(diagnose(&r), Diagnosis::RecallStarved);
    }

    #[test]
    fn diluted_results_are_precision_starved() {
        let r = rep(&[
            ("q1", 0.1, "low-precision"),
            ("q2", 0.2, "low-precision"),
            ("q3", 0.1, "wrong-span"),
        ]);
        assert_eq!(diagnose(&r), Diagnosis::PrecisionStarved);
    }

    #[test]
    fn wrong_span_failures_are_ranking_problems() {
        let r = rep(&[("q1", 0.1, "wrong-span"), ("q2", 0.2, "wrong-span")]);
        assert_eq!(diagnose(&r), Diagnosis::RankedTooLow);
    }

    #[test]
    fn recall_starved_proposals_widen_the_pool() {
        let parent = RetrievalPolicy::default();
        let r = rep(&[("q1", 0.0, "no-grounding"), ("q2", 0.0, "no-grounding")]);
        let props = propose(&parent, &r, 0.05);
        assert!(!props.is_empty());
        assert!(props
            .iter()
            .any(|p| p.policy.candidate_multiplier > parent.candidate_multiplier));
    }

    #[test]
    fn ranking_proposals_shift_lexical_weight() {
        let parent = RetrievalPolicy::default();
        let r = rep(&[("q1", 0.1, "wrong-span"), ("q2", 0.2, "wrong-span")]);
        let props = propose(&parent, &r, 0.1);
        assert!(props.iter().any(|p| {
            p.policy.normalised_weights()["lexical"]
                > parent.normalised_weights()["lexical"] + 1e-6
        }));
    }

    #[test]
    fn every_proposal_carries_a_rationale() {
        let parent = RetrievalPolicy::default();
        let r = rep(&[("q1", 0.0, "no-grounding")]);
        for p in propose(&parent, &r, 0.05) {
            assert!(!p.rationale.is_empty());
        }
    }
}
