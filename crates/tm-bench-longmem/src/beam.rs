//! BEAM (Belief Edit And Memory) contradiction stress-test harness.
//!
//! A BEAM case exercises whether a memory system correctly handles fact updates:
//!
//! 1. **Ingest** `initial_claim` — establishes the original belief.
//! 2. **Ingest** `contradicting_claim` — overwrites / contradicts the first.
//! 3. **Query** `query` — the system must return `expected_answer` (the updated
//!    fact) and optionally surface a contradiction flag.
//!
//! Correct behaviour: the system returns the *updated* answer. Bonus: surfaces
//! the contradiction so the user can review the conflict.

use std::path::Path;

/// One BEAM evaluation case.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeamCase {
    /// Unique case identifier (e.g. `"beam-001"`).
    pub id: String,
    /// The original claim to ingest first (e.g. "Alice works at Acme").
    pub initial_claim: String,
    /// A claim that directly contradicts `initial_claim`
    /// (e.g. "Alice works at Beta Corp").
    pub contradicting_claim: String,
    /// Natural-language query to evaluate after both claims are ingested
    /// (e.g. "Where does Alice work?").
    pub query: String,
    /// The expected answer — should match the *updated* (contradicting) claim.
    pub expected_answer: String,
    /// Whether the system is expected to surface a contradiction signal in
    /// addition to returning the updated answer.
    pub contradiction_should_surface: bool,
}

/// Per-case BEAM evaluation output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BeamResult {
    /// Matches [`BeamCase::id`].
    pub case_id: String,
    /// The string returned by the runner for `BeamCase::query`.
    pub predicted_answer: String,
    /// Whether the runner signalled a contradiction during the query.
    pub contradiction_surfaced: bool,
    /// Exact-match against `BeamCase::expected_answer` (normalized).
    pub exact_match: bool,
    /// Token F1 against `BeamCase::expected_answer`.
    pub token_f1: f64,
}

/// A collection of [`BeamCase`] instances.
pub struct BeamDataset {
    pub cases: Vec<BeamCase>,
}

impl BeamDataset {
    /// Load from a JSON file that is a top-level array of [`BeamCase`].
    pub fn from_json(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {:?}: {}", path, e))?;
        let cases: Vec<BeamCase> = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse beam dataset: {}", e))?;
        Ok(Self { cases })
    }

    /// 5-case built-in fixture for CI testing without an external file.
    ///
    /// Covers diverse entity types: employer, location, role, preference, date.
    pub fn mini_fixture() -> Self {
        let cases = vec![
            BeamCase {
                id: "beam-001".into(),
                initial_claim: "Alice works at Acme Corp.".into(),
                contradicting_claim: "Alice now works at Beta Corp after leaving Acme.".into(),
                query: "Where does Alice work?".into(),
                expected_answer: "Beta Corp".into(),
                contradiction_should_surface: true,
            },
            BeamCase {
                id: "beam-002".into(),
                initial_claim: "The team meeting is scheduled for Monday at 2pm.".into(),
                contradicting_claim: "The team meeting has been moved to Wednesday at 3pm.".into(),
                query: "When is the team meeting?".into(),
                expected_answer: "Wednesday at 3pm".into(),
                contradiction_should_surface: true,
            },
            BeamCase {
                id: "beam-003".into(),
                initial_claim: "Bob is the senior backend engineer on the Falcon project.".into(),
                contradicting_claim: "Bob was promoted to Staff Engineer last week.".into(),
                query: "What is Bob's role?".into(),
                expected_answer: "Staff Engineer".into(),
                contradiction_should_surface: true,
            },
            BeamCase {
                id: "beam-004".into(),
                initial_claim: "The production database is hosted in us-east-1.".into(),
                contradicting_claim: "The production database was migrated to eu-west-1 for GDPR compliance.".into(),
                query: "Which region hosts the production database?".into(),
                expected_answer: "eu-west-1".into(),
                contradiction_should_surface: true,
            },
            BeamCase {
                id: "beam-005".into(),
                initial_claim: "Carol prefers to use Python for data pipelines.".into(),
                contradicting_claim: "Carol switched to Rust for all new data pipeline work.".into(),
                query: "What language does Carol prefer for data pipelines?".into(),
                expected_answer: "Rust".into(),
                contradiction_should_surface: true,
            },
        ];
        Self { cases }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_fixture_has_five_cases() {
        let ds = BeamDataset::mini_fixture();
        assert_eq!(ds.cases.len(), 5);
    }

    #[test]
    fn mini_fixture_all_surface_contradiction() {
        let ds = BeamDataset::mini_fixture();
        for case in &ds.cases {
            assert!(
                case.contradiction_should_surface,
                "case {} should surface contradiction",
                case.id
            );
        }
    }

    #[test]
    fn mini_fixture_nonempty_claims() {
        let ds = BeamDataset::mini_fixture();
        for case in &ds.cases {
            assert!(!case.initial_claim.is_empty());
            assert!(!case.contradicting_claim.is_empty());
            assert!(!case.query.is_empty());
            assert!(!case.expected_answer.is_empty());
        }
    }

    #[test]
    fn roundtrip_json() {
        let ds = BeamDataset::mini_fixture();
        let json = serde_json::to_string_pretty(&ds.cases).unwrap();
        let parsed: Vec<BeamCase> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[0].id, "beam-001");
    }
}
