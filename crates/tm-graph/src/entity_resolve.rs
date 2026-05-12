//! LM-2 — resolve entity mentions inside a free-form text body.
//!
//! The motivating use-case is **inline auto-rendered `[[wikilinks]]`**:
//! Karpathy-style Obsidian PKM, but the user never types `[[`. Instead
//! we scan every memory body against the graph and surface a list of
//! `(span, entity_id)` pairs the UI uses to render the link decoration.
//!
//! The matching rules are intentionally simple — exact, case-insensitive
//! string match with word-boundary checks. We do *not* attempt nickname
//! resolution, plural normalization, or semantic similarity here. That
//! kind of fuzzy linking is the job of the retrieval engine; this
//! function exists to give the *visible* text its zero-tax PKM behaviour.
//!
//! ## Algorithm
//!
//! 1. Pull the candidate entity name set (optionally filtered to a
//!    context).
//! 2. Sort candidates by `name.len()` descending so that "Acme Corp"
//!    beats "Acme" when both share a prefix.
//! 3. For each candidate, scan the text for case-insensitive matches.
//!    A match is *only* a hit when the position is at a word boundary
//!    on both sides (no alphanumeric / underscore neighbour).
//! 4. Skip any match that overlaps a previously claimed span. Because
//!    we processed the longest names first, this gives us the
//!    "longest-match-wins" behaviour callers expect.
//! 5. Return the spans in start-offset order so the UI can iterate
//!    once when injecting decorations.

use crate::store::GraphStore;
use tm_types::{Entity, Result};
use uuid::Uuid;

/// One resolved entity mention.
///
/// `start..end` are byte offsets into the input string. We keep byte
/// offsets (not char offsets) because Rust's `&str` slicing and most
/// downstream renderers (DOM range, ProseMirror) accept the byte form
/// directly; converting to char offsets would just be a UI concern.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntityMention {
    pub entity_id: Uuid,
    /// Display name from the graph (canonical casing — *not* the
    /// matched casing from the text).
    pub name: String,
    /// Byte offset where the match starts.
    pub start: usize,
    /// Byte offset one past the match end (exclusive).
    pub end: usize,
}

/// Resolve entity mentions in `text` against the supplied candidate
/// set. Pure function — no I/O, easy to fuzz. Callers usually go
/// through [`GraphStore::resolve_entity_in_text`] which fetches the
/// candidate set from the graph.
///
/// Spans are returned in ascending `start` order and never overlap.
pub fn resolve_in_text(text: &str, candidates: &[Entity]) -> Vec<EntityMention> {
    if text.is_empty() || candidates.is_empty() {
        return Vec::new();
    }

    // Sort by name length desc — longest match wins. Stable order on
    // ties keeps the output deterministic.
    let mut sorted: Vec<&Entity> = candidates
        .iter()
        .filter(|e| !e.name.trim().is_empty())
        .collect();
    sorted.sort_by(|a, b| b.name.len().cmp(&a.name.len()));

    let lower_text = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let lower_bytes = lower_text.as_bytes();
    let mut mentions: Vec<EntityMention> = Vec::new();

    for ent in &sorted {
        let needle = ent.name.to_ascii_lowercase();
        if needle.is_empty() || needle.len() > lower_bytes.len() {
            continue;
        }
        let needle_bytes = needle.as_bytes();
        // Walk through every occurrence in the lowercased text. We use
        // `memchr`-equivalent scanning by hand to avoid a dep — texts
        // are small (a single memory body), so the naive O(n*m) is fine.
        let mut search_from = 0usize;
        while search_from + needle_bytes.len() <= lower_bytes.len() {
            let Some(rel) = find_substr(&lower_bytes[search_from..], needle_bytes) else {
                break;
            };
            let pos = search_from + rel;
            let end = pos + needle_bytes.len();
            // Word-boundary checks — neighbours must not be
            // alphanumeric or `_`. ASCII-only is enough; entity names
            // with non-ASCII boundaries (CJK, emoji) skip the check.
            if !is_word_boundary(bytes, pos) || !is_word_boundary(bytes, end) {
                search_from = pos + 1;
                continue;
            }
            // Check for overlap with already-claimed spans.
            if overlaps_any(&mentions, pos, end) {
                search_from = pos + 1;
                continue;
            }
            mentions.push(EntityMention {
                entity_id: ent.id,
                name: ent.name.clone(),
                start: pos,
                end,
            });
            // Advance past this match — we don't allow self-overlap.
            search_from = end;
        }
    }

    mentions.sort_by_key(|m| m.start);
    mentions
}

/// Returns true iff `pos` is at a word boundary in `bytes`. By
/// convention, the start and end of the string are word boundaries.
/// "Word" chars are `[A-Za-z0-9_]` — the same definition used by
/// `\b` in PCRE / Rust regex.
fn is_word_boundary(bytes: &[u8], pos: usize) -> bool {
    let left = if pos == 0 { None } else { bytes.get(pos - 1) };
    let right = bytes.get(pos);
    let is_word = |b: Option<&u8>| match b {
        None => false,
        Some(c) => c.is_ascii_alphanumeric() || *c == b'_',
    };
    // A boundary exists when exactly one side is a word char.
    is_word(left) != is_word(right)
}

/// `[u8]::windows`-style substring search. Returns the byte offset of
/// the first match of `needle` in `haystack`, or `None`.
fn find_substr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True if `[start, end)` intersects with any previously claimed
/// mention span.
fn overlaps_any(claimed: &[EntityMention], start: usize, end: usize) -> bool {
    claimed
        .iter()
        .any(|m| !(end <= m.start || start >= m.end))
}

impl GraphStore {
    /// LM-2 entry point. Pulls candidate entities from the graph
    /// (optionally filtered to a context) and resolves mentions in
    /// `text`.
    ///
    /// `context_id = None` scans every entity in the graph — fine for
    /// small personal graphs and for the "global" PKM render. Pass
    /// `Some(ctx)` to keep cross-context bleed contained, matching
    /// the decoupled-by-default retrieval stance.
    pub fn resolve_entity_in_text(
        &self,
        text: &str,
        context_id: Option<Uuid>,
    ) -> Result<Vec<EntityMention>> {
        let candidates = match context_id {
            None => self.list_all_entities()?,
            Some(ctx) => {
                let mut all = self.list_all_entities()?;
                all.retain(|e| {
                    self.entity_context_id(e.id)
                        .ok()
                        .flatten()
                        .map(|c| c == ctx)
                        .unwrap_or(false)
                });
                all
            }
        };
        Ok(resolve_in_text(text, &candidates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::EntityType;

    fn ent(name: &str) -> Entity {
        Entity::new(name, EntityType::Person, 0.9)
    }

    #[test]
    fn empty_inputs() {
        assert!(resolve_in_text("", &[ent("Alice")]).is_empty());
        assert!(resolve_in_text("hello", &[]).is_empty());
    }

    #[test]
    fn exact_match_case_insensitive() {
        let alice = ent("Alice");
        let m = resolve_in_text("Met alice today", &[alice.clone()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].entity_id, alice.id);
        assert_eq!(m[0].start, 4);
        assert_eq!(m[0].end, 9);
    }

    #[test]
    fn word_boundary_rejects_substring_inside_word() {
        // "Acme" inside "Acmeville" must NOT match.
        let acme = ent("Acme");
        let m = resolve_in_text("visiting Acmeville", &[acme]);
        assert!(m.is_empty(), "substring inside a longer word must not hit");
    }

    #[test]
    fn longest_match_wins() {
        let acme = ent("Acme");
        let acme_corp = ent("Acme Corp");
        let m = resolve_in_text("at Acme Corp HQ", &[acme.clone(), acme_corp.clone()]);
        assert_eq!(m.len(), 1, "only the longer span should claim the text");
        assert_eq!(m[0].entity_id, acme_corp.id);
        assert_eq!(&m[0].name, "Acme Corp");
    }

    #[test]
    fn multiple_mentions_returned_in_order() {
        let alice = ent("Alice");
        let bob = ent("Bob");
        let m = resolve_in_text("Bob met Alice. Alice replied.", &[alice.clone(), bob.clone()]);
        assert_eq!(m.len(), 3);
        // Spans are ordered by start offset.
        assert!(m[0].start < m[1].start);
        assert!(m[1].start < m[2].start);
        assert_eq!(m[0].entity_id, bob.id);
        assert_eq!(m[1].entity_id, alice.id);
        assert_eq!(m[2].entity_id, alice.id);
    }

    #[test]
    fn no_overlap_between_results() {
        let foo = ent("foo");
        let foobar = ent("foobar"); // contains "foo"
        let m = resolve_in_text("foobar then foo here", &[foo.clone(), foobar.clone()]);
        // Expect: "foobar" at 0..6, "foo" at 12..15. The "foo" inside
        // "foobar" must not be reported separately.
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].entity_id, foobar.id);
        assert_eq!(m[0].start, 0);
        assert_eq!(m[0].end, 6);
        assert_eq!(m[1].entity_id, foo.id);
        assert_eq!(m[1].start, 12);
        assert_eq!(m[1].end, 15);
    }

    #[test]
    fn punctuation_is_a_word_boundary() {
        let alice = ent("Alice");
        let m = resolve_in_text("(Alice)", &[alice.clone()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start, 1);
        assert_eq!(m[0].end, 6);
    }

    #[test]
    fn store_lookup_respects_context_filter() {
        use crate::context::Context;
        let store = GraphStore::open(":memory:").expect("open mem db");

        let ctx_a = Context::new("work", "venture");
        let ctx_b = Context::new("personal", "life");
        store.create_context(&ctx_a).expect("upsert ctx_a");
        store.create_context(&ctx_b).expect("upsert ctx_b");

        // Sprint C-0 stamps the active context on entities at upsert
        // time. Bounce the active context between upserts to land Alice
        // in work and Acme in personal.
        store.set_active_context(Some(ctx_a.id));
        let alice = ent("Alice");
        store.upsert_entity(&alice).expect("upsert alice");

        store.set_active_context(Some(ctx_b.id));
        let acme = ent("Acme");
        store.upsert_entity(&acme).expect("upsert acme");

        store.set_active_context(None);

        let text = "Alice met Acme today";
        let all = store
            .resolve_entity_in_text(text, None)
            .expect("resolve none");
        assert_eq!(all.len(), 2);

        let only_work = store
            .resolve_entity_in_text(text, Some(ctx_a.id))
            .expect("resolve work");
        assert_eq!(only_work.len(), 1);
        assert_eq!(only_work[0].entity_id, alice.id);

        let only_personal = store
            .resolve_entity_in_text(text, Some(ctx_b.id))
            .expect("resolve personal");
        assert_eq!(only_personal.len(), 1);
        assert_eq!(only_personal[0].entity_id, acme.id);
    }
}
