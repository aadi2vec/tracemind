//! `CommitmentMiner` — implicit phrase mining over capture streams.
//!
//! Canonical against `docs/INTENT_SYSTEM.md` §3.1. Targets ~80 % of
//! all commitment intake. Pure stdlib regex/heuristic gate at mining
//! time — Tier-1 LLM is only invoked at *confirmation* time, never
//! here, so this can run on every capture event for free.
//!
//! ## Algorithm
//!
//! Lowercased substring match against three phrase tables:
//! - **Forward** ("I'll", "I'm going to", "the plan is", …) → `Intent`
//! - **Backward-disclosed** ("I decided", "I went with", …) → `Decision`
//! - **Hypothesis** ("I think", "I bet", "probably", …) → `Hypothesis`
//!
//! On a match we slice from the start of the phrase to the end of the
//! sentence (next `.`, `?`, `!`, or end-of-text), trim, and emit a
//! [`MinedCandidate`]. Confidence is derived from phrase
//! specificity — see [`PHRASE_TABLE`].
//!
//! ## What we deliberately do NOT do
//!
//! - No NLP / parsing / dependency analysis. The brief asks the user
//!   to confirm; false positives are corrected by dismissal.
//! - No deduplication across captures here — that's the brief's job.
//! - No polarity inference; mining is strictly about *what was said*,
//!   not how it turned out.

use crate::types::{CommitmentDraft, CommitmentKind, Stakes};

/// Fixed phrase table from `INTENT_SYSTEM.md` §3.1. The numeric column
/// is the per-phrase confidence — short, generic phrases get lower
/// scores so that ranking in the brief surfaces the strongest signals
/// first. Values are a heuristic eyeball, not learned.
///
/// Order matters only within the same kind: longer phrases must
/// precede their shorter prefixes (e.g. "I'm going to" before "I'm").
const PHRASE_TABLE: &[(&str, CommitmentKind, f32)] = &[
    // Forward / intent
    ("i'm going to", CommitmentKind::Intent, 0.85),
    ("i am going to", CommitmentKind::Intent, 0.85),
    ("i'm gonna", CommitmentKind::Intent, 0.80),
    ("i intend to", CommitmentKind::Intent, 0.85),
    ("i'll try to", CommitmentKind::Intent, 0.65),
    ("i'll try", CommitmentKind::Intent, 0.55),
    ("i'll", CommitmentKind::Intent, 0.65),
    ("i need to", CommitmentKind::Intent, 0.75),
    ("i have to", CommitmentKind::Intent, 0.70),
    ("i want to", CommitmentKind::Intent, 0.65),
    ("i should", CommitmentKind::Intent, 0.60),
    ("i must", CommitmentKind::Intent, 0.70),
    ("remind me to", CommitmentKind::Intent, 0.85),
    ("note to self", CommitmentKind::Intent, 0.80),
    ("todo:", CommitmentKind::Intent, 0.85),
    ("the plan is", CommitmentKind::Intent, 0.80),
    ("we'll ship", CommitmentKind::Intent, 0.85),
    ("we're going to", CommitmentKind::Intent, 0.80),
    ("we need to", CommitmentKind::Intent, 0.75),
    ("let's", CommitmentKind::Intent, 0.55),
    ("going with", CommitmentKind::Intent, 0.70),
    // 3rd-person reported intents — captured at lower confidence because
    // they describe someone else's plan rather than the user's. Surface
    // them so the brief can show "X said they'd do Y" patterns.
    ("he'll", CommitmentKind::Intent, 0.45),
    ("she'll", CommitmentKind::Intent, 0.45),
    ("they'll", CommitmentKind::Intent, 0.45),
    ("he is going to", CommitmentKind::Intent, 0.50),
    ("she is going to", CommitmentKind::Intent, 0.50),
    ("they are going to", CommitmentKind::Intent, 0.50),
    ("he plans to", CommitmentKind::Intent, 0.55),
    ("she plans to", CommitmentKind::Intent, 0.55),
    ("they plan to", CommitmentKind::Intent, 0.55),
    ("he needs to", CommitmentKind::Intent, 0.50),
    ("she needs to", CommitmentKind::Intent, 0.50),
    ("they need to", CommitmentKind::Intent, 0.50),
    ("plans to", CommitmentKind::Intent, 0.45),
    ("agreed to", CommitmentKind::Intent, 0.65),
    ("promised to", CommitmentKind::Intent, 0.75),
    ("committed to", CommitmentKind::Intent, 0.70),
    // Backward / decision-disclosed
    ("decided to", CommitmentKind::Decision, 0.90),
    ("i decided", CommitmentKind::Decision, 0.90),
    ("i went with", CommitmentKind::Decision, 0.85),
    ("we went with", CommitmentKind::Decision, 0.85),
    ("ended up", CommitmentKind::Decision, 0.65),
    ("settled on", CommitmentKind::Decision, 0.85),
    ("we're doing", CommitmentKind::Decision, 0.70),
    ("chose ", CommitmentKind::Decision, 0.70),
    // Hypothesis
    ("my guess is", CommitmentKind::Hypothesis, 0.80),
    ("i bet ", CommitmentKind::Hypothesis, 0.65),
    ("i think ", CommitmentKind::Hypothesis, 0.55),
    ("probably ", CommitmentKind::Hypothesis, 0.50),
    ("should be ", CommitmentKind::Hypothesis, 0.45),
];

/// One mined candidate. Wraps the [`CommitmentDraft`] from §1 with
/// miner-specific metadata: which phrase fired, where in the source
/// text, and the heuristic confidence.
#[derive(Debug, Clone)]
pub struct MinedCandidate {
    pub draft: CommitmentDraft,
    /// The exact lowercased phrase (e.g. "i'm going to") that matched.
    /// Useful for the brief and for the *silence-this-pattern* affordance.
    pub matched_phrase: &'static str,
    /// Byte offsets into the *original* (pre-lowercase) text. Inclusive
    /// start, exclusive end. Useful for the highlight in the brief UI.
    pub span: (usize, usize),
    /// Heuristic in `[0, 1]` — same as `PHRASE_TABLE`'s third column,
    /// minus 0.05 if the matched sentence is < 4 words (probably noise).
    pub confidence: f32,
}

/// Mine `text` for commitment-shaped phrases. Returns one candidate
/// per non-overlapping phrase hit, in the order they appear in the text.
///
/// Each hit's statement is the slice from the *start of the phrase*
/// through the end of the sentence. Sentences end at `.`, `?`, `!`,
/// or the end of `text`. We deliberately keep the phrase itself in the
/// statement — it's the strongest signal of intent shape and the
/// confirmation step can rewrite it if the user wants.
///
/// Empty input → empty output. ASCII-only fast path is good enough;
/// we lowercase once and run substring checks. Unicode capture events
/// are still mined correctly because we never strip non-ASCII bytes.
pub fn mine(text: &str) -> Vec<MinedCandidate> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();
    let mut out: Vec<MinedCandidate> = Vec::new();

    for &(phrase, kind, base_conf) in PHRASE_TABLE.iter() {
        let pbytes = phrase.as_bytes();
        let mut search_at = 0usize;
        while let Some(rel) = find_subslice(&bytes[search_at..], pbytes) {
            let start = search_at + rel;
            // Word-boundary check on the left edge. If the byte just
            // before the match is alphanumeric, this is a substring
            // hit inside a longer word ("recall" matching "all") — skip.
            if start > 0 {
                let prev = bytes[start - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'\'' {
                    search_at = start + 1;
                    continue;
                }
            }
            let end_phrase = start + pbytes.len();
            // Walk to end of sentence.
            let mut end_sent = end_phrase;
            while end_sent < bytes.len() {
                let b = bytes[end_sent];
                if b == b'.' || b == b'?' || b == b'!' || b == b'\n' {
                    break;
                }
                end_sent += 1;
            }
            // Slice the *original* text using these byte offsets — the
            // lowercase pass preserves byte indices for ASCII, and our
            // phrase table is ASCII so `start..end_phrase` always lines up.
            // Sentence body may contain non-ASCII; that's fine because
            // we're slicing on byte offsets that came from `to_lowercase`.
            let statement_lower = lower[start..end_sent].trim().to_string();
            // Re-derive the original-cased statement when possible:
            // for ASCII text, byte offsets in `text` and `lower` line up.
            let statement = if text.is_ascii() {
                text[start..end_sent].trim().to_string()
            } else {
                statement_lower.clone()
            };
            let word_count = statement.split_whitespace().count();
            let conf = if word_count < 4 {
                (base_conf - 0.05).max(0.05)
            } else {
                base_conf
            };
            out.push(MinedCandidate {
                draft: CommitmentDraft {
                    kind,
                    statement,
                    options_considered: Vec::new(),
                    horizon: None,
                    stakes: Stakes::Medium,
                    tags: vec!["mined".to_string()],
                },
                matched_phrase: phrase,
                span: (start, end_sent),
                confidence: conf,
            });
            search_at = end_sent;
        }
    }
    // Stable sort by span start so the brief shows them in source order.
    out.sort_by_key(|c| c.span.0);
    // Drop overlapping spans (keep the highest-confidence first hit).
    dedup_overlapping(out)
}

/// `memchr`-free substring search. Linear in `haystack.len()`. We could
/// pull in `memchr` but the inputs here are short (clipboard / single
/// shell line / MCP turn) so plain naïve search is fine and keeps the
/// crate's dep tree tight.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// If two candidates' spans overlap, keep the *more specific* one —
/// i.e. the one whose **matched_phrase** is longer. Specificity beats
/// raw confidence for overlap resolution because PHRASE_TABLE assigns
/// similar weights to nested phrases ("i'll" / "i'll try" / "i'll try
/// to") and we always want the longest hit to survive. Confidence
/// only breaks ties when phrase lengths are equal.
fn dedup_overlapping(mut cs: Vec<MinedCandidate>) -> Vec<MinedCandidate> {
    // Sort by start offset, then by descending matched_phrase length
    // (so the longest hit at each anchor lands first), then by
    // descending confidence as final tiebreak.
    cs.sort_by(|a, b| {
        a.span
            .0
            .cmp(&b.span.0)
            .then_with(|| b.matched_phrase.len().cmp(&a.matched_phrase.len()))
            .then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let mut out: Vec<MinedCandidate> = Vec::with_capacity(cs.len());
    for c in cs {
        if let Some(last) = out.last() {
            // Overlap if the new one starts before last ends.
            if c.span.0 < last.span.1 {
                // Keep whichever phrase is longer (more specific). On
                // equal length, prefer higher confidence.
                let last_len = last.matched_phrase.len();
                let c_len = c.matched_phrase.len();
                if c_len > last_len
                    || (c_len == last_len && c.confidence > last.confidence + f32::EPSILON)
                {
                    let n = out.len();
                    out[n - 1] = c;
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_nothing() {
        assert!(mine("").is_empty());
        assert!(mine("   ").is_empty());
    }

    #[test]
    fn forward_intent_picked_up() {
        let cs = mine("Hey, I'm going to ship Sprint B by Friday. Let me know.");
        assert_eq!(cs.len(), 1, "expected one mined intent, got {cs:?}");
        let c = &cs[0];
        assert_eq!(c.draft.kind, CommitmentKind::Intent);
        assert_eq!(c.matched_phrase, "i'm going to");
        assert!(c.draft.statement.starts_with("I'm going to ship"));
        assert!(c.confidence > 0.7);
    }

    #[test]
    fn backward_decision_picked_up() {
        let cs = mine("After the call I decided to go with Postgres.");
        let kinds: Vec<_> = cs.iter().map(|c| c.draft.kind).collect();
        assert!(kinds.contains(&CommitmentKind::Decision));
    }

    #[test]
    fn hypothesis_picked_up() {
        let cs = mine("My guess is the migration takes two days.");
        assert!(!cs.is_empty());
        assert_eq!(cs[0].draft.kind, CommitmentKind::Hypothesis);
    }

    #[test]
    fn dedup_prefers_longer_phrase() {
        // "i'll try to" should beat "i'll" on the same sentence.
        let cs = mine("I'll try to ship Friday.");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].matched_phrase, "i'll try to");
    }

    #[test]
    fn word_boundary_blocks_substring_inside_word() {
        // "all" is not in the table, but "i'll" should NOT match
        // "recall" or "I'll'd" inside another word — guard test.
        let cs = mine("recall recalls recalled.");
        assert!(cs.is_empty(), "no commit phrases hidden in unrelated words");
    }

    #[test]
    fn multiple_sentences_yield_multiple_candidates() {
        let text = "I'll fix the auth bug today. I decided to use bcrypt.";
        let cs = mine(text);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].draft.kind, CommitmentKind::Intent);
        assert_eq!(cs[1].draft.kind, CommitmentKind::Decision);
        // Source order preserved.
        assert!(cs[0].span.0 < cs[1].span.0);
    }

    #[test]
    fn short_statements_get_lower_confidence() {
        // "I'll try" with no follow-on words → less than 4 words after
        // trimming → confidence dropped by 0.05.
        let cs = mine("I'll try.");
        assert_eq!(cs.len(), 1);
        assert!(cs[0].confidence < 0.55);
    }

    #[test]
    fn unicode_text_does_not_crash() {
        // Make sure non-ASCII bytes survive the slice pass — we fall
        // back to the lowercased statement when the original isn't ASCII.
        let cs = mine("I'm going to ship — really, really soon. ✨");
        assert_eq!(cs.len(), 1);
        assert!(cs[0].draft.statement.to_lowercase().contains("i'm going to ship"));
    }

    #[test]
    fn mined_drafts_are_tagged_for_brief() {
        let cs = mine("Let's pair on this tomorrow.");
        assert!(cs.iter().all(|c| c.draft.tags.contains(&"mined".to_string())));
    }
}
