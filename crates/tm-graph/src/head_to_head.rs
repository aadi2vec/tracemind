//! Q4.8 — Head-to-head benchmark infrastructure for contradiction-rate comparison.
//!
//! Provides the data structures for comparing TraceMind's contradiction-rate
//! against Mem0, Zep, Letta, and Engram. The actual benchmark runner lives in
//! `tm-bench-memory`; this module provides the shared types.
//!
//! Competitors can't win on contradiction-rate architecturally because they
//! don't retain temporal edges (they would need to add bitemporal storage
//! at the KG level, not just embedding cosine overlap).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A competitor in the head-to-head benchmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Competitor {
    TraceMind,
    Mem0,
    Zep,
    Letta,
    Engram,
    LangMem,
}

impl Competitor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Competitor::TraceMind => "tracemind",
            Competitor::Mem0 => "mem0",
            Competitor::Zep => "zep",
            Competitor::Letta => "letta",
            Competitor::Engram => "engram",
            Competitor::LangMem => "langmem",
        }
    }
}

/// A test case for the contradiction head-to-head.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionTestCase {
    pub id: String,
    /// Initial facts to ingest.
    pub initial_facts: Vec<String>,
    /// Contradicting facts to ingest after initial_facts.
    pub contradicting_facts: Vec<String>,
    /// Query to evaluate contradiction handling.
    pub query: String,
    /// Expected behaviour: should the system surface the contradiction?
    pub expect_contradiction_surfaced: bool,
    /// Expected answer (the *newer* fact, not the old one).
    pub expected_answer: String,
}

/// Result from one competitor on one test case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompetitorResult {
    pub competitor: Competitor,
    pub case_id: String,
    pub contradiction_surfaced: bool,
    pub answered_correctly: bool,
    pub latency_ms: u64,
}

/// Aggregate score for one competitor across all test cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompetitorScore {
    pub competitor: Competitor,
    pub contradiction_rate: f64,   // fraction where contradiction was surfaced
    pub accuracy: f64,             // fraction with correct answer
    pub avg_latency_ms: f64,
    pub test_cases: usize,
}

/// Full head-to-head comparison result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadToHeadResult {
    pub run_at: DateTime<Utc>,
    pub scores: Vec<CompetitorScore>,
}

impl HeadToHeadResult {
    /// Which competitor has the lowest (best) contradiction false-positive rate?
    pub fn winner_contradiction_rate(&self) -> Option<&CompetitorScore> {
        self.scores.iter().max_by(|a, b| {
            a.contradiction_rate.partial_cmp(&b.contradiction_rate).unwrap()
        })
    }

    /// Is TraceMind's contradiction_rate < half of Mem0's? (Q4 exit gate metric)
    pub fn tracemind_below_half_of_mem0(&self) -> bool {
        let tm = self.scores.iter().find(|s| s.competitor == Competitor::TraceMind);
        let mem0 = self.scores.iter().find(|s| s.competitor == Competitor::Mem0);
        match (tm, mem0) {
            (Some(t), Some(m)) => t.contradiction_rate > m.contradiction_rate * 0.5,
            _ => false,
        }
    }
}

/// Built-in mini test fixtures for CI validation.
pub fn mini_fixtures() -> Vec<ContradictionTestCase> {
    vec![
        ContradictionTestCase {
            id: "htoh-001".into(),
            initial_facts: vec!["Alice works at Acme Corp.".into()],
            contradicting_facts: vec!["Alice works at Beta Corp.".into()],
            query: "Where does Alice work?".into(),
            expect_contradiction_surfaced: true,
            expected_answer: "Beta Corp".into(),
        },
        ContradictionTestCase {
            id: "htoh-002".into(),
            initial_facts: vec!["The sprint ends on Friday.".into()],
            contradicting_facts: vec!["The sprint was extended to next Monday.".into()],
            query: "When does the sprint end?".into(),
            expect_contradiction_surfaced: true,
            expected_answer: "next Monday".into(),
        },
        ContradictionTestCase {
            id: "htoh-003".into(),
            initial_facts: vec!["Bob is the project lead.".into(), "Bob manages 5 engineers.".into()],
            contradicting_facts: vec!["Carol is the new project lead.".into()],
            query: "Who is the project lead?".into(),
            expect_contradiction_surfaced: true,
            expected_answer: "Carol".into(),
        },
    ]
}
