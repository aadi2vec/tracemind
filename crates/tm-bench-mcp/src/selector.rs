//! Deterministic tool selector — a model-free proxy for the host LLM.
//!
//! A real host reads the tool descriptions and picks one to call. We cannot
//! put an LLM in a CI benchmark, but we can measure the property that
//! actually decides whether the LLM picks correctly: **are the descriptions
//! discriminative for the utterances that should trigger them?**
//!
//! The selector scores each tool for an utterance by IDF-weighted lexical
//! overlap between the utterance and the tool's `name + description`, where
//! IDF is computed across the tool set so a term that appears in *every*
//! description (e.g. "memory") carries no discriminating weight, while a
//! term unique to one tool (e.g. "contradict") is decisive. Argmax wins.
//!
//! This is deliberately the same shape as the retrieval scorer GEPA already
//! optimises: the mutable artifact is the set of descriptions, the metric is
//! selection accuracy, and a description improves when it gains the words its
//! trigger utterances use and sheds the words its rivals own. That makes
//! "GEPA over tool descriptions" (review P0.2) a drop-in reuse of this file.

use std::collections::HashMap;

/// A tool as the host sees it: the name and the description it must pick from.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
}

/// Selects among tools for an utterance using IDF-weighted overlap.
pub struct ToolSelector {
    tools: Vec<ToolDef>,
    /// term -> number of tools whose text contains it.
    doc_freq: HashMap<String, usize>,
    /// per-tool term sets.
    tool_terms: Vec<Vec<String>>,
}

impl ToolSelector {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let mut tool_terms = Vec::with_capacity(tools.len());
        for t in &tools {
            let terms = tokenize(&format!("{} {}", t.name, t.description));
            let uniq: std::collections::HashSet<&String> = terms.iter().collect();
            for term in uniq {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
            tool_terms.push(terms);
        }
        Self { tools, doc_freq, tool_terms }
    }

    fn idf(&self, term: &str) -> f32 {
        let n = self.tools.len().max(1) as f32;
        let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
        // Smoothed IDF: a term in every tool -> ~0; a term in one tool -> high.
        ((n + 1.0) / (df + 1.0)).ln().max(0.0)
    }

    /// Score every tool for `utterance`, best first.
    pub fn rank(&self, utterance: &str) -> Vec<(String, f32)> {
        let q_terms = tokenize(utterance);
        let mut scored: Vec<(String, f32)> = self
            .tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let tool_set: std::collections::HashSet<&String> =
                    self.tool_terms[i].iter().collect();
                let score: f32 = q_terms
                    .iter()
                    .filter(|qt| tool_set.contains(qt))
                    .map(|qt| self.idf(qt))
                    .sum();
                (t.name.clone(), score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// The single best tool for `utterance`, if any term matched.
    pub fn select(&self, utterance: &str) -> Option<String> {
        self.rank(utterance)
            .into_iter()
            .find(|(_, s)| *s > 0.0)
            .map(|(name, _)| name)
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

/// Lowercase alphanumeric tokens, stopwords removed. Underscores split so
/// `memory_contradict` contributes both `memory` and `contradict`.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .flat_map(|w| w.split('_'))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "you", "your", "with", "was", "are", "has", "have", "had", "this", "that",
    "from", "any", "all", "use", "used", "when", "what", "who", "did", "does", "into", "its", "get",
    "can", "will", "also", "not", "but", "out", "via",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn core_tools() -> Vec<ToolDef> {
        vec![
            ToolDef { name: "memory_store".into(), description: "Ingest text into memory, extracting entities and triples.".into() },
            ToolDef { name: "memory_query".into(), description: "Answer a question by retrieving relevant memories.".into() },
            ToolDef { name: "memory_contradict".into(), description: "Check whether a new statement contradicts a stored belief; the retraction beat.".into() },
            ToolDef { name: "memory_forget".into(), description: "Delete a memory and everything derived from it.".into() },
            ToolDef { name: "memory_compose".into(), description: "Compose context from past conversation threads for the next window.".into() },
            ToolDef { name: "memory_feedback".into(), description: "Record whether a retrieved memory was helpful or a miss.".into() },
        ]
    }

    #[test]
    fn selects_store_for_a_deposit_utterance() {
        let s = ToolSelector::new(core_tools());
        assert_eq!(s.select("ingest this into memory and extract entities").as_deref(), Some("memory_store"));
    }

    #[test]
    fn selects_contradict_for_a_conflict_utterance() {
        let s = ToolSelector::new(core_tools());
        assert_eq!(s.select("does this contradict what I said before").as_deref(), Some("memory_contradict"));
    }

    #[test]
    fn selects_forget_for_a_deletion_utterance() {
        let s = ToolSelector::new(core_tools());
        assert_eq!(s.select("delete that memory permanently").as_deref(), Some("memory_forget"));
    }

    #[test]
    fn shared_term_does_not_decide() {
        // "memory" appears in every tool, so an utterance that is *only*
        // "memory" cannot discriminate and must not confidently pick one.
        let s = ToolSelector::new(core_tools());
        let ranked = s.rank("memory");
        // All scores equal (and near zero after IDF), so no clear winner.
        let top = ranked[0].1;
        assert!(top < 0.5, "shared term should carry little weight, got {top}");
    }

    #[test]
    fn no_match_returns_none() {
        let s = ToolSelector::new(core_tools());
        assert_eq!(s.select("photosynthesis in cyanobacteria"), None);
    }

    #[test]
    fn tokenize_splits_underscores() {
        let t = tokenize("memory_contradict");
        assert!(t.contains(&"memory".to_string()));
        assert!(t.contains(&"contradict".to_string()));
    }
}
