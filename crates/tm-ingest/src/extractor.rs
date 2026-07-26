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

use crate::pipeline::{extract_entities, extract_keyphrases, extract_triples};

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
        let mut ents = extract_entities(text);

        // TM-NLP-003b — YAKE-lite keyphrase pass. Adds lowercase multi-word
        // concepts ("machine learning", "vector search") that the Title-Case
        // extractor misses. Deduplicate against whatever the NER pass
        // already produced so we don't double-emit the same phrase.
        let existing: std::collections::HashSet<String> =
            ents.iter().map(|e| e.name.to_lowercase()).collect();
        for kp in extract_keyphrases(text) {
            let key = kp.name.to_lowercase();
            if !existing.contains(&key) {
                ents.push(kp);
            }
        }
        ents
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

    #[test]
    fn browser_history_capture_yields_url_plus_host_organization() {
        // Regression for the 2026-07-25 quality report: browser-history
        // captures were producing junk entities ("Tickets" → Person,
        // "2026" → Concept, "Visit" → Person) and no host-level
        // Organization node. After the quality pass:
        //  - raw URL stays typed Url
        //  - the URL's registrable domain (`fifa.com`) surfaces as
        //    an Organization entity — the graph node users think of
        //    when they say "the FIFA site"
        //  - "2026" / "Tickets" / "Visit" no longer show up
        use tm_types::EntityType;
        let text = "https://www.fifa.com/tickets FIFA World Cup 2026 Tickets";
        let ents = extract_entities(text);
        let names: Vec<(&str, &EntityType)> =
            ents.iter().map(|e| (e.name.as_str(), &e.entity_type)).collect();

        // Must contain the URL as Url.
        assert!(
            names.iter().any(|(n, t)| *n == "https://www.fifa.com/tickets"
                && **t == EntityType::Url),
            "expected raw URL as Url, got {names:?}"
        );
        // Must contain the host as Organization.
        assert!(
            names.iter().any(|(n, t)| *n == "fifa.com" && **t == EntityType::Organization),
            "expected fifa.com as Organization, got {names:?}"
        );
        // Must NOT contain the year or generic English words as entities.
        for junk in ["2026", "Tickets", "Visit", "Home", "Login", "Buy"] {
            assert!(
                !names.iter().any(|(n, _)| n.eq_ignore_ascii_case(junk)),
                "junk entity {junk:?} slipped through: {names:?}"
            );
        }
    }

    #[test]
    fn url_and_host_are_not_collapsed_by_names_look_like_same_entity() {
        // Regression for the 2026-07-25 demo bug: `apnews.com` is a
        // substring of `https://apnews.com/…`, so the substring branch
        // of `names_look_like_same_entity` returned true and the
        // pipeline's `decide_memory_op` then emitted an Update that
        // pushed the URL row back into `kept_entities`. Result: two
        // URL entries and no Organization. The URL-vs-host asymmetry
        // guard fixes this.
        use crate::pipeline::names_look_like_same_entity_public;
        assert!(
            !names_look_like_same_entity_public(
                "apnews.com",
                "https://apnews.com/article/world-cup-2026-tickets"
            ),
            "URL and host must be distinct entities"
        );
        assert!(
            !names_look_like_same_entity_public(
                "https://fifa.com/tickets",
                "fifa.com"
            ),
            "URL and host must be distinct entities (order-independent)"
        );
        // Sanity: legitimate same-entity substring pairs still match.
        assert!(names_look_like_same_entity_public(
            "TraceMind",
            "TraceMind's"
        ));
        assert!(names_look_like_same_entity_public("fifa.com", "fifa.com"));
    }

    #[test]
    #[ignore = "diagnostic — dumps GLiNER output for a URL-first text"]
    fn debug_gliner_output_for_url() {
        use crate::gliner::GlinerExtractor;
        let Some(g) = GlinerExtractor::auto_download_default() else {
            eprintln!("GLiNER model unavailable — skip");
            return;
        };
        let text = "https://apnews.com/article/world-cup-2026-tickets Ticket resale prices";
        let ents = g.extract_entities(text);
        for (i, e) in ents.iter().enumerate() {
            eprintln!("  #{i}  [{:?}] {}", e.entity_type, e.name);
        }
    }

    #[test]
    fn url_host_extraction_handles_common_shapes() {
        use crate::pipeline::extract_registrable_domain;
        assert_eq!(
            extract_registrable_domain("https://www.fifa.com/tickets"),
            Some("fifa.com".into())
        );
        assert_eq!(
            extract_registrable_domain("http://buy.fifa.com/en"),
            Some("buy.fifa.com".into())
        );
        assert_eq!(
            extract_registrable_domain("www.fifa.com/tickets"),
            Some("fifa.com".into())
        );
        assert_eq!(
            extract_registrable_domain("https://Example.COM/PATH"),
            Some("example.com".into())
        );
        assert_eq!(
            extract_registrable_domain("https://example.com:8080/x"),
            Some("example.com".into())
        );
        // Not a URL
        assert_eq!(extract_registrable_domain("plain-text"), None);
        // Host has no dot — reject (never a domain).
        assert_eq!(extract_registrable_domain("http://localhost/x"), None);
    }
}
