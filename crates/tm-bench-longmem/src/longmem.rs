//! LongMemEval types and dataset loader.
//!
//! LongMemEval evaluates memory systems on long-context question answering with
//! 5 difficulty categories. Each case provides a series of conversation turns to
//! ingest, then a question + gold answer pair to evaluate retrieval quality.

use std::path::Path;

/// Question category matching the LoCoMo taxonomy so results are cross-comparable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LongMemCategory {
    /// Single-hop fact retrieval: answer lies in exactly one ingested turn.
    SingleHop,
    /// Multi-hop: answer requires joining facts from two or more turns.
    MultiHop,
    /// Temporal: answer depends on ordering or timing of events.
    Temporal,
    /// Open-domain: answer may need general knowledge combined with memory.
    OpenDomain,
    /// Adversarial: distractor turns designed to confuse the retriever.
    Adversarial,
}

impl std::fmt::Display for LongMemCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LongMemCategory::SingleHop => "single_hop",
            LongMemCategory::MultiHop => "multi_hop",
            LongMemCategory::Temporal => "temporal",
            LongMemCategory::OpenDomain => "open_domain",
            LongMemCategory::Adversarial => "adversarial",
        };
        write!(f, "{}", s)
    }
}

/// One LongMemEval evaluation case.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LongMemCase {
    /// Unique case identifier (e.g. `"lme-001"`).
    pub id: String,
    /// Difficulty / question type.
    pub category: LongMemCategory,
    /// Ordered conversation turns to ingest before querying. Each string is one
    /// turn (may be multi-sentence). The runner should ingest them in order,
    /// all under the same `session_id` derived from `id`.
    pub context_turns: Vec<String>,
    /// Natural-language question posed after all turns are ingested.
    pub question: String,
    /// Primary gold answer string (used for exact-match).
    pub gold_answer: String,
    /// Token-level gold spans. For F1 computation each span is treated as an
    /// acceptable gold string; we take the max F1 across spans (SQuAD-style).
    /// Must contain at least the gold_answer.
    pub gold_spans: Vec<String>,
}

/// Per-case evaluation output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LongMemResult {
    /// Matches [`LongMemCase::id`].
    pub case_id: String,
    /// Matches [`LongMemCase::category`].
    pub category: LongMemCategory,
    /// The string returned by the runner for this question.
    pub predicted_answer: String,
    /// Best token F1 across `gold_spans` (0.0–1.0).
    pub token_f1: f64,
    /// Whether `predicted_answer` exactly matches `gold_answer` (normalized).
    pub exact_match: bool,
    /// Wall-clock latency for the query phase only (ingest time excluded).
    pub latency_ms: u64,
}

/// A collection of [`LongMemCase`] instances.
pub struct LongMemDataset {
    pub cases: Vec<LongMemCase>,
}

impl LongMemDataset {
    /// Load from a JSON file that is a top-level array of [`LongMemCase`].
    pub fn from_json(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {:?}: {}", path, e))?;
        let cases: Vec<LongMemCase> = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse longmem dataset: {}", e))?;
        Ok(Self { cases })
    }

    /// 10-case built-in fixture suitable for CI without an external file.
    ///
    /// Cases span all 5 categories (2 each). Answers are short phrases
    /// compatible with the token-F1 scorer in [`crate::metrics`].
    pub fn mini_fixture() -> Self {
        let cases = vec![
            // ── single_hop ────────────────────────────────────────────────────
            LongMemCase {
                id: "lme-001".into(),
                category: LongMemCategory::SingleHop,
                context_turns: vec![
                    "Alice joined Acme Corp as a software engineer in March 2024.".into(),
                    "She works on the infrastructure team and her manager is Bob.".into(),
                ],
                question: "What company does Alice work for?".into(),
                gold_answer: "Acme Corp".into(),
                gold_spans: vec!["Acme Corp".into(), "Acme".into()],
            },
            LongMemCase {
                id: "lme-002".into(),
                category: LongMemCategory::SingleHop,
                context_turns: vec![
                    "The project deadline was set for June 15th.".into(),
                    "The team decided to use Rust for the backend services.".into(),
                ],
                question: "What language was chosen for the backend?".into(),
                gold_answer: "Rust".into(),
                gold_spans: vec!["Rust".into()],
            },
            // ── multi_hop ─────────────────────────────────────────────────────
            LongMemCase {
                id: "lme-003".into(),
                category: LongMemCategory::MultiHop,
                context_turns: vec![
                    "Carol manages the data science team at Beta Inc.".into(),
                    "The data science team's primary tool stack includes Python and Spark.".into(),
                    "Beta Inc is headquartered in Austin, Texas.".into(),
                ],
                question: "What city is Carol's employer based in?".into(),
                gold_answer: "Austin".into(),
                gold_spans: vec!["Austin".into(), "Austin, Texas".into()],
            },
            LongMemCase {
                id: "lme-004".into(),
                category: LongMemCategory::MultiHop,
                context_turns: vec![
                    "Dave is the lead engineer on Project Falcon.".into(),
                    "Project Falcon aims to reduce cloud costs by 40%.".into(),
                    "Dave reports to the VP of Engineering, Elena.".into(),
                ],
                question: "Who is the manager of the Project Falcon lead?".into(),
                gold_answer: "Elena".into(),
                gold_spans: vec!["Elena".into()],
            },
            // ── temporal ─────────────────────────────────────────────────────
            LongMemCase {
                id: "lme-005".into(),
                category: LongMemCategory::Temporal,
                context_turns: vec![
                    "In January, the team shipped version 1.0.".into(),
                    "In April, version 2.0 introduced multi-tenant support.".into(),
                    "By December the team had shipped version 3.0 with AI features.".into(),
                ],
                question: "Which version introduced multi-tenant support?".into(),
                gold_answer: "version 2.0".into(),
                gold_spans: vec!["version 2.0".into(), "2.0".into()],
            },
            LongMemCase {
                id: "lme-006".into(),
                category: LongMemCategory::Temporal,
                context_turns: vec![
                    "The Q1 review happened on March 31st.".into(),
                    "The Q2 review was scheduled for June 30th.".into(),
                    "Frank joined the company after the Q1 review but before the Q2 review.".into(),
                ],
                question: "When did Frank join relative to company reviews?".into(),
                gold_answer: "after Q1 review but before Q2 review".into(),
                gold_spans: vec![
                    "after the Q1 review but before the Q2 review".into(),
                    "after Q1 and before Q2".into(),
                ],
            },
            // ── open_domain ───────────────────────────────────────────────────
            LongMemCase {
                id: "lme-007".into(),
                category: LongMemCategory::OpenDomain,
                context_turns: vec![
                    "Grace is building a recommendation system for e-commerce.".into(),
                    "She mentioned that collaborative filtering works well for their use case.".into(),
                ],
                question: "What ML approach is Grace using for recommendations?".into(),
                gold_answer: "collaborative filtering".into(),
                gold_spans: vec!["collaborative filtering".into()],
            },
            LongMemCase {
                id: "lme-008".into(),
                category: LongMemCategory::OpenDomain,
                context_turns: vec![
                    "Henry is evaluating vector databases for their RAG pipeline.".into(),
                    "He shortlisted Qdrant and Weaviate after initial benchmarking.".into(),
                ],
                question: "Which vector databases did Henry shortlist?".into(),
                gold_answer: "Qdrant and Weaviate".into(),
                gold_spans: vec!["Qdrant and Weaviate".into(), "Qdrant".into(), "Weaviate".into()],
            },
            // ── adversarial ──────────────────────────────────────────────────
            LongMemCase {
                id: "lme-009".into(),
                category: LongMemCategory::Adversarial,
                context_turns: vec![
                    "The system uses PostgreSQL for its primary datastore.".into(),
                    "Some teams have evaluated MySQL as an alternative.".into(),
                    "The final decision was to remain on PostgreSQL for consistency.".into(),
                ],
                question: "What database does the system use as its primary datastore?".into(),
                gold_answer: "PostgreSQL".into(),
                gold_spans: vec!["PostgreSQL".into()],
            },
            LongMemCase {
                id: "lme-010".into(),
                category: LongMemCategory::Adversarial,
                context_turns: vec![
                    "Ivan said the meeting is on Tuesday.".into(),
                    "Later, Jane said she thought the meeting might be Wednesday.".into(),
                    "Ivan confirmed: the meeting is definitely Tuesday.".into(),
                ],
                question: "What day is the meeting on?".into(),
                gold_answer: "Tuesday".into(),
                gold_spans: vec!["Tuesday".into()],
            },
        ];
        Self { cases }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_fixture_has_ten_cases() {
        let ds = LongMemDataset::mini_fixture();
        assert_eq!(ds.cases.len(), 10);
    }

    #[test]
    fn mini_fixture_covers_all_categories() {
        let ds = LongMemDataset::mini_fixture();
        let cats: std::collections::HashSet<String> =
            ds.cases.iter().map(|c| c.category.to_string()).collect();
        assert!(cats.contains("single_hop"));
        assert!(cats.contains("multi_hop"));
        assert!(cats.contains("temporal"));
        assert!(cats.contains("open_domain"));
        assert!(cats.contains("adversarial"));
    }

    #[test]
    fn mini_fixture_gold_spans_nonempty() {
        let ds = LongMemDataset::mini_fixture();
        for case in &ds.cases {
            assert!(
                !case.gold_spans.is_empty(),
                "case {} has empty gold_spans",
                case.id
            );
        }
    }

    #[test]
    fn roundtrip_json() {
        let ds = LongMemDataset::mini_fixture();
        let json = serde_json::to_string_pretty(&ds.cases).unwrap();
        let parsed: Vec<LongMemCase> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 10);
        assert_eq!(parsed[0].id, "lme-001");
    }
}
