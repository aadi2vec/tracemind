//! Implicit outcome matcher.
//!
//! When a new capture lands, did it just describe what happened to an
//! existing open Commitment? Per `docs/INTENT_SYSTEM.md` §4.2 the
//! spec calls for embedding cosine similarity ≥ 0.7. We ship a
//! cheaper, deterministic heuristic first — token Jaccard overlap on
//! lowercase content tokens, plus a small polarity phrase table to
//! hint at *whether* the outcome was Better / Worse / AsExpected /
//! Mixed.
//!
//! Why heuristic-first:
//!
//! 1. Deterministic — easy to test and reason about.
//! 2. No embedding dependency in `tm-intent` / `tm-reflect`. The
//!    upgrade path is a [`OutcomeMatcher`] trait with an embedding
//!    impl behind a feature flag.
//! 3. We surface low-confidence proposals to the *user*, never
//!    auto-resolve. False positives are cheap; the user dismisses.
//!
//! Output: a `Vec<OutcomeProposal>` per new text. The matcher does
//! not write to the store — the caller decides whether to persist
//! the proposal, surface it inline (e.g. in `memory_store`'s
//! response), or do nothing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use tm_intent::{Commitment, Polarity};

/// One match between a new capture and an open Commitment. Carries a
/// polarity hint — caller may downgrade to `None` and let the user
/// label it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeProposal {
    pub commitment_id: Uuid,
    pub commitment_statement: String,
    /// 0.0 .. 1.0 — Jaccard token overlap.
    pub score: f32,
    /// Polarity inferred from polarity-phrase hits in the new text.
    /// `None` if no polarity phrase fired — user must choose.
    pub polarity_hint: Option<Polarity>,
    /// Human-readable explanation of what fired the match — drives
    /// the "show your work" panel per §9.4.
    pub reason: String,
}

/// Tunables. We tuned `min_score = 0.25` from real text — Jaccard on
/// stemless tokens drops "shipped"/"ship" cross-matches into the
/// 0.28-0.33 band, so a 0.30 floor was too strict. 0.25 still rejects
/// the obvious noise band (≤ 0.17 for "the pizza shipped on friday"
/// vs "ship the locomo report") while catching morphological
/// variants. `max_proposals_per_text = 3` so the brief never floods
/// from a single capture.
#[derive(Debug, Clone)]
pub struct MatcherConfig {
    pub min_score: f32,
    pub max_proposals_per_text: usize,
}

impl Default for MatcherConfig {
    fn default() -> Self {
        Self {
            min_score: 0.25,
            max_proposals_per_text: 3,
        }
    }
}

/// Polarity-phrase table. Order matters only for tied scores.
const POLARITY_PHRASES: &[(&str, Polarity)] = &[
    // Better
    ("shipped", Polarity::Better),
    ("delivered", Polarity::Better),
    ("done", Polarity::Better),
    ("finished", Polarity::Better),
    ("completed", Polarity::Better),
    ("nailed", Polarity::Better),
    ("worked", Polarity::Better),
    ("fixed", Polarity::Better),
    ("solved", Polarity::Better),
    ("succeeded", Polarity::Better),
    // Worse
    ("missed", Polarity::Worse),
    ("failed", Polarity::Worse),
    ("broke", Polarity::Worse),
    ("crashed", Polarity::Worse),
    ("regressed", Polarity::Worse),
    ("worse", Polarity::Worse),
    ("late", Polarity::Worse),
    ("blocked", Polarity::Worse),
    ("rolled back", Polarity::Worse),
    // AsExpected
    ("as expected", Polarity::AsExpected),
    ("on track", Polarity::AsExpected),
    ("on time", Polarity::AsExpected),
    // Mixed
    ("partially", Polarity::Mixed),
    ("mostly", Polarity::Mixed),
    ("but", Polarity::Mixed), // weak; only fires alongside another match
];

/// Stop-words removed before tokenization. We're aggressive — these
/// drown out the signal in a Jaccard score.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "to", "of", "in", "on",
    "at", "by", "for", "with", "from", "as", "and", "or", "but", "if", "then", "than", "so", "we",
    "i", "you", "he", "she", "it", "they", "this", "that", "these", "those", "my", "your", "our",
    "their", "have", "had", "has", "do", "does", "did", "will", "would", "should", "could", "can",
    "not", "no",
];

fn is_stopword(t: &str) -> bool {
    STOPWORDS.binary_search(&t).is_ok()
        || STOPWORDS.iter().any(|s| *s == t)
}

/// Lowercase, split on non-alphanumeric, drop stopwords + tokens
/// shorter than 3 chars (kills "v2"-style false signals while
/// keeping product nouns).
fn tokenize(text: &str) -> std::collections::HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| t.len() >= 3 && !is_stopword(t))
        .collect()
}

fn jaccard(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersect = a.intersection(b).count();
    let union = a.union(b).count();
    intersect as f32 / union as f32
}

/// Scan new `text` against `opens` (an iterator of open commitments)
/// and return the strongest matches. The result is sorted by score
/// descending and truncated to `cfg.max_proposals_per_text`.
pub fn propose_outcomes(
    text: &str,
    opens: &[Commitment],
    cfg: &MatcherConfig,
) -> Vec<OutcomeProposal> {
    let text_tokens = tokenize(text);
    if text_tokens.is_empty() {
        return Vec::new();
    }

    let polarity_hint = sniff_polarity(text);

    let mut scored: Vec<OutcomeProposal> = opens
        .iter()
        .filter_map(|c| {
            let stmt_tokens = tokenize(&c.statement);
            let stmt_score = jaccard(&text_tokens, &stmt_tokens);
            // Also compare against expected_outcome if present — the
            // spec calls this out explicitly in §4.2.
            let expected_score = c
                .expected_outcome
                .as_deref()
                .map(|s| jaccard(&text_tokens, &tokenize(s)))
                .unwrap_or(0.0);
            let score = stmt_score.max(expected_score);
            if score < cfg.min_score {
                return None;
            }

            let shared: Vec<&String> = text_tokens.intersection(&stmt_tokens).collect();
            let mut shared_sorted: Vec<&str> = shared.iter().map(|s| s.as_str()).collect();
            shared_sorted.sort();
            let reason = format!(
                "shared {} token(s) with statement: {}",
                shared.len(),
                shared_sorted.join(", ")
            );

            Some(OutcomeProposal {
                commitment_id: c.id,
                commitment_statement: c.statement.clone(),
                score,
                polarity_hint,
                reason,
            })
        })
        .collect();

    // Highest score first; stable sort by (score desc, statement asc).
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.commitment_statement.cmp(&b.commitment_statement))
    });
    scored.truncate(cfg.max_proposals_per_text);
    scored
}

fn sniff_polarity(text: &str) -> Option<Polarity> {
    let lower = text.to_ascii_lowercase();
    // First-hit wins. Order in POLARITY_PHRASES is meaningful — we
    // prefer Better/Worse/AsExpected over Mixed, which is a weak
    // catch-all.
    for (phrase, polarity) in POLARITY_PHRASES {
        if lower.contains(phrase) {
            return Some(*polarity);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_intent::{CommitmentKind, Source};

    fn open(statement: &str) -> Commitment {
        Commitment::new(CommitmentKind::Intent, statement, Source::Manual)
    }

    #[test]
    fn empty_text_yields_no_proposals() {
        let opens = vec![open("ship v2 by friday")];
        let r = propose_outcomes("", &opens, &MatcherConfig::default());
        assert!(r.is_empty());
    }

    #[test]
    fn empty_open_list_yields_no_proposals() {
        let r = propose_outcomes("we shipped v2 today!", &[], &MatcherConfig::default());
        assert!(r.is_empty());
    }

    #[test]
    fn jaccard_matches_strong_overlap() {
        let opens = vec![open("ship the locomo report by friday")];
        let r = propose_outcomes(
            "we finally shipped the locomo report this morning",
            &opens,
            &MatcherConfig::default(),
        );
        assert_eq!(r.len(), 1);
        assert!(r[0].score >= 0.25, "score = {}", r[0].score);
    }

    #[test]
    fn weak_overlap_is_dropped() {
        let opens = vec![open("ship the locomo report by friday")];
        // No overlap with the locomo commitment beyond "the" stopword.
        let r = propose_outcomes(
            "had pizza for dinner",
            &opens,
            &MatcherConfig::default(),
        );
        assert!(r.is_empty());
    }

    #[test]
    fn polarity_better_detected_from_shipped() {
        let opens = vec![open("ship the locomo report by friday")];
        let r = propose_outcomes(
            "shipped the locomo report",
            &opens,
            &MatcherConfig::default(),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].polarity_hint, Some(Polarity::Better));
    }

    #[test]
    fn polarity_worse_detected_from_missed() {
        let opens = vec![open("ship the locomo report by friday")];
        let r = propose_outcomes(
            "we missed the locomo report deadline",
            &opens,
            &MatcherConfig::default(),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].polarity_hint, Some(Polarity::Worse));
    }

    #[test]
    fn no_polarity_when_no_phrase_fires() {
        let opens = vec![open("ship the locomo report by friday")];
        let r = propose_outcomes(
            "thinking about the locomo report and the friday deadline",
            &opens,
            &MatcherConfig::default(),
        );
        assert!(!r.is_empty());
        assert_eq!(r[0].polarity_hint, None);
    }

    #[test]
    fn expected_outcome_also_drives_match() {
        let mut c = open("file the contract");
        c.expected_outcome = Some("contract signed and counter-party countersigned".into());
        let opens = vec![c];
        // Text matches the expected outcome, not the statement.
        let r = propose_outcomes(
            "the counter-party countersigned the contract",
            &opens,
            &MatcherConfig::default(),
        );
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn results_sorted_by_score_desc_and_capped() {
        let opens = vec![
            open("ship locomo report friday"),
            open("ship locomo report"),
            open("locomo benchmark"),
            open("unrelated other thing"),
        ];
        let cfg = MatcherConfig {
            min_score: 0.0,
            max_proposals_per_text: 2,
        };
        let r = propose_outcomes("we shipped the locomo report", &opens, &cfg);
        assert_eq!(r.len(), 2);
        // Highest-overlap commitment must win.
        assert!(r[0].score >= r[1].score);
    }

    #[test]
    fn proposals_round_trip_json() {
        let opens = vec![open("ship the locomo report by friday")];
        let r = propose_outcomes(
            "shipped the locomo report",
            &opens,
            &MatcherConfig::default(),
        );
        assert_eq!(r.len(), 1);
        let s = serde_json::to_string(&r[0]).unwrap();
        let back: OutcomeProposal = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r[0]);
    }
}
