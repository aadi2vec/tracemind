//! Pluggable entity + relation extraction.
//!
//! The slow-path consolidation pipeline (Tier-2 / Tier-3) runs NER on the
//! representative signal of each cluster. Historically this was a single
//! hand-written heuristic pass (multi-word Title Case + URL/file detection +
//! pattern-matched triples). That gets us roughly 55% F1 on PER/ORG/TECH
//! entities which is good enough to bootstrap a graph but leaves meaningful
//! headroom on the table.
//!
//! TM-5.1-001c introduced an [`EntityExtractor`] trait so we can swap in a
//! stronger model (real ONNX GLiNER ~85% F1) without touching the hot path.
//! The heuristic implementation is the default; a real GLiNER extractor is
//! tracked by TM-NLP-004 and is gated on a local model + evaluation fixture
//! (no scaffold / placeholder implementation ships).
//!
//! The trait is deliberately small — two methods mirroring the existing
//! `extract_entities` / `extract_triples` pair — so plugging in a new model is
//! a matter of implementing NER and (optionally) relation classification. If
//! a model only does NER, it can delegate to the heuristic triple pass.

use tm_types::{Entity, Triple};

use crate::pipeline::{extract_entities, extract_triples};

/// Something that can turn raw text into entities + relations.
///
/// Implementations MUST be `Send + Sync` because the pipeline is shared across
/// the fast-path capture loop and the slow-path consolidation loops.
pub trait EntityExtractor: Send + Sync {
    /// Extract named entities from `text`.
    fn extract_entities(&self, text: &str) -> Vec<Entity>;

    /// Extract typed triples. The `entities` slice is the output of
    /// [`Self::extract_entities`] *after* graph-level dedup, so extractors
    /// should treat it as the authoritative node set.
    fn extract_triples(&self, text: &str, entities: &[Entity]) -> Vec<Triple>;

    /// Human-readable name for logging / telemetry.
    fn name(&self) -> &'static str;
}

/// Default extractor — stdlib-only heuristics. Zero dependencies, zero model
/// weights, ~55% F1. Wraps the free functions in [`crate::pipeline`] so the
/// call sites look uniform regardless of which extractor is active.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicExtractor;

impl EntityExtractor for HeuristicExtractor {
    fn extract_entities(&self, text: &str) -> Vec<Entity> {
        extract_entities(text)
    }

    fn extract_triples(&self, text: &str, entities: &[Entity]) -> Vec<Triple> {
        extract_triples(text, entities)
    }

    fn name(&self) -> &'static str {
        "heuristic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_extractor_matches_free_functions() {
        let text = "Aaditya Srivathsan works at Acme Corp on TraceMind";
        let ext = HeuristicExtractor;

        let from_trait_entities = ext.extract_entities(text);
        let from_free_entities = extract_entities(text);

        assert_eq!(from_trait_entities.len(), from_free_entities.len());
        // Compare by (name, entity_type) — UUIDs are random per call.
        let lhs: Vec<_> = from_trait_entities
            .iter()
            .map(|e| (e.name.clone(), e.entity_type.clone()))
            .collect();
        let rhs: Vec<_> = from_free_entities
            .iter()
            .map(|e| (e.name.clone(), e.entity_type.clone()))
            .collect();
        assert_eq!(lhs, rhs);

        let triples = ext.extract_triples(text, &from_trait_entities);
        // Sanity: at least something got extracted from a rich sentence.
        assert!(!triples.is_empty());
    }

    #[test]
    fn heuristic_extractor_name() {
        assert_eq!(HeuristicExtractor.name(), "heuristic");
    }
}
