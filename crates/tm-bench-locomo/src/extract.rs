//! Tier-0 span extraction for LoCoMo answers.
//!
//! Token-F1 punishes long predictions hard. The runner's first attempt
//! returns the full retrieved turn, so even when the right turn is
//! picked, a 12-word turn around a 2-word answer scores ~0.2 F1.
//!
//! This module narrows the prediction by classifying the question and
//! pulling the relevant span out of the turn:
//!
//! - "When …?"          → date span (e.g. "May 3rd", "April 20")
//! - "How much …?"      → money span (e.g. "$2.5M")
//! - "What was the … time?" → time span ("2:58:42", "sub-3:00")
//! - "Did/Was/Is …?"    → yes/no oracle (Yes / No, <correction>)
//!
//! Anything we can't confidently extract falls through to the original
//! turn — better to keep recall than to over-trim and lose the answer.
//! Each extractor is a hand-written byte-scan; no regex dependency.

/// What the question is asking for. Coarse on purpose — only kinds we
/// can answer with a span extractor are split out; everything else maps
/// to `Generic` and the runner returns the candidate turn unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QKind {
    /// Polar yes/no — "Did Alice fly to Tokyo on May 1st?"
    YesNo,
    /// Calendar date — "When is Alice flying to Tokyo?"
    Date,
    /// Currency amount — "How much did Carol raise?"
    Money,
    /// Clock time — "What was Ethan's finish time?"
    Time,
    /// A named thing — person, place, org, or product. Covers
    /// who / where / what / which. The answer is a proper noun in the
    /// evidence turn that does not already appear in the question.
    Named,
    /// A causal question ("why…?"). Like [`QKind::Generic`] no span
    /// extractor applies, but unlike it the answer is a specific causal
    /// clause somewhere in the history rather than a description of the
    /// thing the question already names — so lexical overlap with the
    /// question is a much stronger selection signal.
    Reason,
    /// No specialized extractor; return turn as-is.
    Generic,
}

impl QKind {
    /// Whether a candidate turn can supply an answer of this kind.
    ///
    /// Used for answer-aware candidate selection: a turn that is topically
    /// similar but carries no extractable answer of the required type is a
    /// worse choice than a lower-ranked turn that does. This is the reader
    /// half of a retriever-reader extractive QA stack — retrieval score
    /// alone routinely puts a related-but-unanswerable turn on top.
    pub fn is_satisfied_by(self, turn: &str, question: &str) -> bool {
        match self {
            // A turn whose only typed value is the one the question already
            // stated cannot answer it, so it does not count as satisfying.
            QKind::Date => has_novel_value(extract_date_lenient, turn, question),
            QKind::Money => has_novel_value(extract_money, turn, question),
            QKind::Time => has_novel_value(extract_time, turn, question),
            QKind::Named => extract_novel_proper_noun(turn, question).is_some(),
            // Yes/No, Reason, and Generic can be answered from any turn.
            QKind::YesNo | QKind::Reason | QKind::Generic => true,
        }
    }
}

/// The first value `extractor` finds in `turn` that the question does not
/// already state, scanning past premise echoes.
///
/// "Did Ethan miss his sub-3:00 goal?" against "Sub-3:00 is the dream, but
/// realistically 3:10" must not answer "sub-3:00" — that is the question's
/// own premise. Skipping it surfaces 3:10, which is at least informative.
fn first_novel_value(
    extractor: fn(&str) -> Option<String>,
    turn: &str,
    question: &str,
) -> Option<String> {
    let q_norm = question.to_lowercase();
    let mut offset = 0usize;
    // Bounded scan: each step consumes at least one value, and turns hold
    // only a handful of typed values.
    for _ in 0..4 {
        let rest = turn.get(offset..)?;
        let value = extractor(rest)?;
        if !q_norm.contains(&value.to_lowercase()) {
            return Some(value);
        }
        let pos = rest.to_lowercase().find(&value.to_lowercase())?;
        offset += pos + value.len();
    }
    None
}

/// Whether `turn` carries a typed value that the question does not already
/// name. See [`novel_or_first`] for why novelty is the right test.
fn has_novel_value(
    extractor: fn(&str) -> Option<String>,
    turn: &str,
    question: &str,
) -> bool {
    let value = match extractor(turn) {
        Some(v) => v,
        None => return false,
    };
    let q_norm = question.to_lowercase();
    if !q_norm.contains(&value.to_lowercase()) {
        return true;
    }
    if let Some(pos) = turn.to_lowercase().find(&value.to_lowercase()) {
        let rest = &turn[pos + value.len()..];
        if let Some(next) = extractor(rest) {
            return !q_norm.contains(&next.to_lowercase());
        }
    }
    false
}

/// Words that, when leading a question, signal a yes/no.
const YN_LEAD: &[&str] = &[
    "did", "does", "do", "is", "are", "was", "were", "has", "have", "had", "will", "can", "could",
    "should", "would",
];

/// Classify a question into a [`QKind`]. Cheap, no allocation.
pub fn classify_question(q: &str) -> QKind {
    let lower = q.trim().to_lowercase();
    let mut tokens = lower.split_whitespace();
    let first = match tokens.next() {
        Some(t) => t,
        None => return QKind::Generic,
    };

    if YN_LEAD.contains(&first) {
        return QKind::YesNo;
    }
    if first == "when" {
        return QKind::Date;
    }
    if first == "how" {
        // "how much" / "how many" → money/number; "how" alone → generic.
        if matches!(tokens.next(), Some("much") | Some("many")) {
            return QKind::Money;
        }
        return QKind::Generic;
    }

    // A question that asks about a "time" as a noun wants a clock value.
    // Matching the bare token (rather than a list of "<modifier> time"
    // phrases) keeps this from being tuned to particular question wordings.
    if lower.split_whitespace().any(|t| t.trim_matches('?') == "time") {
        return QKind::Time;
    }

    // "Why …?" asks for a cause. No span extractor applies, but the
    // answer is a specific clause elsewhere in the history, so it is
    // selected differently from a description question.
    if first == "why" {
        return QKind::Reason;
    }

    // "What is X about?" / "Tell me about X" ask for a description, not a
    // name. The answer is a noun phrase or a whole clause, so no span
    // extractor applies and the evidence turn is the best prediction.
    if lower.split_whitespace().any(|t| t.trim_matches('?') == "about") {
        return QKind::Generic;
    }

    // wh-questions that name an entity: the answer is a proper noun.
    // "why" is excluded — its answer is a clause, not a name.
    if matches!(first, "who" | "whom" | "where" | "what" | "which") {
        return QKind::Named;
    }

    QKind::Generic
}

/// Compose a tight prediction string for the question, given the best
/// candidate turn the retriever could find. Falls back to the turn
/// itself if no extractor produces a confident span.
pub fn compose_short_answer(question: &str, turn: &str) -> String {
    match classify_question(question) {
        QKind::YesNo => yes_no_answer(question, turn),
        QKind::Date => novel_or_first(extract_date_lenient, turn, question),
        QKind::Money => novel_or_first(extract_money, turn, question),
        QKind::Time => novel_or_first(extract_time, turn, question),
        QKind::Named => {
            resolve_named(turn, question, &|_| None).unwrap_or_else(|| turn.to_string())
        }
        QKind::Reason | QKind::Generic => turn.to_string(),
    }
}

/// Apply the novelty principle to a typed extractor.
///
/// The same reasoning that governs proper nouns governs dates, times, and
/// amounts: a question restates the values it already knows and asks for
/// the one it doesn't. "Did Ethan miss his sub-3:00 goal?" names sub-3:00,
/// so the informative answer is the *other* time in the evidence —
/// 2:58:42. Without this, the extractor returns the question's own premise
/// back to the user, which reads as agreement regardless of the facts.
///
/// Falls back to the first extracted value when every candidate value
/// already appears in the question, and to the whole turn when none does.
fn novel_or_first(
    extractor: fn(&str) -> Option<String>,
    turn: &str,
    question: &str,
) -> String {
    let first = match extractor(turn) {
        Some(v) => v,
        None => return turn.to_string(),
    };
    let q_norm = question.to_lowercase();
    if !q_norm.contains(&first.to_lowercase()) {
        return first;
    }
    // The leading value is the question's own premise. Re-run the
    // extractor on the remainder of the turn to find a different one.
    if let Some(pos) = turn.to_lowercase().find(&first.to_lowercase()) {
        let rest = &turn[pos + first.len()..];
        if let Some(next) = extractor(rest) {
            if !q_norm.contains(&next.to_lowercase()) {
                return next;
            }
        }
    }
    first
}

/// Pick the best named answer, consulting the knowledge graph.
///
/// `entity_type` resolves a candidate string to the entity type the graph
/// recorded for it at ingest time (`None` when unknown). That is a far
/// better signal than orthography: "Hired" and "Observability" are
/// capitalised and sentence-initial, but neither is an entity the NER ever
/// recorded, while "Rosa" and "Devi" are. Relying on curated lists of
/// ordinary English words to tell these apart does not generalise beyond
/// the fixture the list was written against.
///
/// Precedence:
///   1. An explicit naming construction ("a manager named Devi").
///   2. A candidate whose graph entity type matches what the question asks
///      for (who → Person, where → Place, which company → Organization).
///   3. Any candidate the graph knows at all.
///   4. The orthographic best guess.
pub fn resolve_named(
    turn: &str,
    question: &str,
    entity_type: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let candidates = named_candidates(turn, question);
    if candidates.is_empty() {
        return None;
    }

    // 1. Explicit naming construction, if that name is a live candidate.
    if let Some(named) = name_after_naming_cue(turn) {
        if candidates.iter().any(|c| c.eq_ignore_ascii_case(&named)) {
            return Some(named);
        }
    }

    let want = expected_entity(question);
    let matches_want = |kind: &str| -> bool {
        let k = kind.to_lowercase();
        match want {
            ExpectedEntity::Person => k.contains("person"),
            ExpectedEntity::Place => k.contains("location") || k.contains("place"),
            ExpectedEntity::Organization => {
                k.contains("organization") || k.contains("organisation") || k.contains("project")
            }
            ExpectedEntity::Any => false,
        }
    };

    // Combine orthographic quality with what the graph knows, rather than
    // letting either veto the other. Graph membership alone is not decisive:
    // the heuristic NER also records sentence-initial verbs it mistook for
    // names, so "is an entity" must add evidence rather than override the
    // structural signal that a sentence-initial single token is probably not
    // a name.
    let scored = scored_named_candidates(turn, question);
    let best = scored
        .into_iter()
        .map(|(name, quality)| {
            let kind = entity_type(&name);
            let mut score = quality;
            if let Some(k) = &kind {
                score += 2; // known to the graph at all
                if matches_want(k) {
                    score += 8; // and of exactly the asked-for type
                }
            }
            (score, name)
        })
        .max_by(|a, b| a.0.cmp(&b.0));

    best.map(|(_, n)| n).or_else(|| candidates.into_iter().next())
}

// ────────────────────────────────────────────────────────────────────
// Novel-proper-noun extractor
// ────────────────────────────────────────────────────────────────────

/// Words that are capitalised for reasons other than being a name —
/// sentence-initial function words, days, and conversational openers. A
/// capitalised token in this set is never treated as a candidate answer.
const NON_NAME_CAPS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "did", "do", "for", "from", "he", "her", "his", "i",
    "if", "in", "is", "it", "just", "my", "no", "not", "of", "on", "or", "she", "so", "the",
    "then", "they", "this", "to", "we", "what", "when", "where", "which", "who", "why", "yes",
    "you", "your", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
    "today", "tomorrow", "yesterday", "finished", "started", "nice", "cool", "eleven", "renamed",
];

/// Extract the proper-noun phrase from `turn` that does not already appear
/// in `question`.
///
/// The premise is standard for extractive QA: a *wh*-question supplies the
/// entities it already knows about and asks for the one it doesn't. "Who
/// led Carol's pre-seed round?" names Carol; the answer is the other name
/// in the evidence — Sequoia. Filtering by novelty is what stops the
/// extractor from confidently returning the subject of the question back
/// to the user.
///
/// Contiguous capitalised tokens are joined ("Andreessen Horowitz"), and
/// month names are excluded so a date never masquerades as a name.
pub fn extract_novel_proper_noun(turn: &str, question: &str) -> Option<String> {
    named_candidates(turn, question).into_iter().next()
}

/// What kind of entity a *wh*-question is asking for.
///
/// This is the bridge from the question to the knowledge graph: "who"
/// wants a Person, "which company" an Organization. Resolving the answer
/// against the graph's own NER is what lets the extractor stop relying on
/// curated lists of English words to guess whether a capitalised token is a
/// name — the graph already decided that at ingest time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedEntity {
    Person,
    Place,
    Organization,
    Any,
}

/// Infer the entity type a question is asking for.
pub fn expected_entity(question: &str) -> ExpectedEntity {
    let lower = question.to_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    if matches!(first, "who" | "whom") {
        return ExpectedEntity::Person;
    }
    if first == "where" {
        return ExpectedEntity::Place;
    }
    // "Which company / employer / firm …" and "What company …".
    for cue in ["company", "employer", "firm", "startup", "org", "organisation", "organization"] {
        if lower.contains(cue) {
            return ExpectedEntity::Organization;
        }
    }
    ExpectedEntity::Any
}

/// All novel proper-noun candidates in `turn`, best-guess first.
///
/// Returned in preference order so a caller with extra knowledge (e.g. the
/// graph's entity types) can re-rank rather than being stuck with this
/// module's purely orthographic guess.
pub fn named_candidates(turn: &str, question: &str) -> Vec<String> {
    scored_named_candidates(turn, question)
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

/// Named candidates with their orthographic quality score, best first.
pub fn scored_named_candidates(turn: &str, question: &str) -> Vec<(String, i32)> {
    let q_tokens: Vec<String> = question
        .split_whitespace()
        .map(|t| normalize_token(t))
        .filter(|t| !t.is_empty())
        .collect();

    let words: Vec<&str> = turn.split_whitespace().collect();
    let mut candidates: Vec<(i32, usize, String)> = Vec::new();
    let mut i = 0usize;

    while i < words.len() {
        // A token is "sentence-initial" if it opens the turn or follows a
        // sentence terminator. English capitalises those regardless of
        // namehood, so their capitalisation carries no information.
        let sentence_initial = i == 0 || words.get(i.wrapping_sub(1)).map_or(false, |w| ends_sentence(w));
        if !is_name_candidate(words[i], sentence_initial) {
            i += 1;
            continue;
        }
        // Greedily absorb following capitalised tokens into one phrase.
        // Interior tokens are never sentence-initial, so they are judged
        // by the stricter name test.
        let mut phrase: Vec<&str> = vec![words[i]];
        let mut j = i + 1;
        // Stop at a sentence boundary. "Signed up for the Boston Marathon.
        // Race day is April 20." must yield "Boston Marathon", not
        // "Boston Marathon Race" — the capital on "Race" is sentence-initial
        // and belongs to the next clause.
        while j < words.len()
            && !ends_sentence(words[j - 1])
            && is_name_candidate(words[j], false)
        {
            phrase.push(words[j]);
            j += 1;
        }

        let cleaned: Vec<String> = phrase.iter().map(|w| trim_edges(w)).collect();
        let joined = cleaned.join(" ");
        let novel = cleaned
            .iter()
            .any(|w| !q_tokens.contains(&normalize_token(w)));

        if novel && !joined.is_empty() {
            let mut q = name_quality(&cleaned);
            // Sentence-initial single tokens are demoted rather than
            // dropped. Excluding them by consulting a list of ordinary
            // English words does not generalise -- "Hired", "Chaired",
            // "Registered", "Switching" are all sentence-initial verbs that
            // no reasonable list contains. Demoting instead means any
            // corroborated name elsewhere in the turn wins, and the
            // sentence-initial token is still available when nothing else
            // is.
            if sentence_initial && phrase.len() == 1 {
                q -= 10;
            }
            candidates.push((q, i, joined));
        }
        i = j.max(i + 1);
    }

    // Syntactic cue: when the question ends with a preposition
    // ("...rename her company *to*?"), the answer is the object of that
    // same preposition in the evidence ("Renamed Loom *to* Memex"). Without
    // this, "Loom" and "Memex" are indistinguishable — both are novel
    // proper nouns — and the extractor returns the wrong one.
    if let Some(prep) = trailing_preposition(question) {
        if let Some(obj) = object_of_preposition(&words, prep) {
            if let Some(pos) = candidates.iter().position(|(_, _, n)| *n == obj) {
                let hit = candidates.remove(pos);
                // Large but *safe* sentinel: callers add bonuses to this
                // score, and i32::MAX would wrap negative on the first one.
                candidates.insert(0, (PREPOSITION_CUE_SCORE, 0, hit.2));
            }
        }
    }

    // Highest-quality name first; ties resolve to the earliest mention,
    // since conversational turns tend to front the answer.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut out: Vec<(String, i32)> =
        candidates.into_iter().map(|(q, _, n)| (n, q)).collect();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// How strongly a turn's structure matches what the question asks for.
///
/// Returns 1.0 when a *syntactic* cue fires — the question's trailing
/// preposition has a proper-noun object in this turn — and a lower baseline
/// when the turn merely contains some extractable answer of the right type.
///
/// This lets candidate selection prefer evidence whose shape matches the
/// question over evidence that merely type-checks. For "What did Carol
/// rename her company **to**?", both "Renamed Loom to Memex" and "I'm
/// leaving Stripe to start a company" contain novel proper nouns and share
/// one question token, so nothing else separates them; only the first has a
/// name in the `to`-object position.
pub fn answer_confidence(turn: &str, question: &str) -> f32 {
    match classify_question(question) {
        QKind::Named => {
            let words: Vec<&str> = turn.split_whitespace().collect();
            if let Some(prep) = trailing_preposition(question) {
                if object_of_preposition(&words, prep).is_some() {
                    return 1.0;
                }
                // The question demanded a prepositional object and this
                // turn has none. Neutral rather than penalised: many
                // correct answers are phrased without the preposition.
                return 0.6;
            }
            0.6
        }
        QKind::Date
        | QKind::Money
        | QKind::Time
        | QKind::YesNo
        | QKind::Reason
        | QKind::Generic => 0.6,
    }
}

/// Proper noun introduced by an explicit naming construction
/// ("a manager **named** Devi", "a chef **called** Rosa").
///
/// This is a far stronger signal than capitalisation: the sentence is
/// literally declaring the name, so it outranks a sentence-initial
/// capitalised word like "Observability" that merely looks like one.
pub fn name_after_naming_cue(turn: &str) -> Option<String> {
    let words: Vec<&str> = turn.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if bare != "named" && bare != "called" {
            continue;
        }
        if let Some(next) = words.get(i + 1) {
            if is_name_candidate(next, false) {
                return Some(trim_edges(next));
            }
        }
    }
    None
}

/// The first month named in `text`, if any. Lets a question that says only
/// "in June" be compared against evidence that says "July 9th" -- without
/// it, a bare month yields no date and the yes/no oracle falls through to
/// a coin-flip default.
pub fn extract_month(text: &str) -> Option<String> {
    for w in text.split_whitespace() {
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if MONTHS.contains(&bare.as_str()) {
            return Some(bare);
        }
    }
    None
}

/// Score given to a candidate that matches the question's trailing
/// preposition. Dominant but headroom-safe for downstream bonuses.
const PREPOSITION_CUE_SCORE: i32 = 1000;

/// Whether a question asks for the *current* value of a fact that may have
/// been revised, rather than for any statement of it.
///
/// "Which team is Maya on **now**?" and "How much did they **finally**
/// agree on?" must read the latest turn; "Which company did Maya join?" must
/// not. Gating supersession on these cues is what keeps recency from
/// demoting facts that were simply stated early and never changed.
pub fn asks_for_latest(question: &str) -> bool {
    let lower = question.to_lowercase();
    const CUES: &[&str] = &[
        "now", "finally", "currently", "current", "actually", "latest", "final",
        "end up", "ended up", "still", "these days", "eventually",
    ];
    CUES.iter().any(|c| {
        // Word-boundary match so "final" does not fire inside "finalise"
        // and "now" does not fire inside "known".
        lower
            .split(|ch: char| !ch.is_alphanumeric() && ch != ' ')
            .any(|seg| seg.split_whitespace().collect::<Vec<_>>().windows(c.split(' ').count())
                .any(|w| w.join(" ") == *c))
    })
}

/// Prepositions whose object is the answer when they end a question.
const TRAILING_PREPS: &[&str] = &["to", "from", "with", "for", "at", "in", "into", "by", "on"];

/// The preposition a question ends on, if any.
fn trailing_preposition(question: &str) -> Option<&'static str> {
    let last = question
        .split_whitespace()
        .next_back()?
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    TRAILING_PREPS.iter().copied().find(|p| *p == last)
}

/// The proper-noun object immediately following `prep` in the evidence.
fn object_of_preposition(words: &[&str], prep: &str) -> Option<String> {
    for (i, w) in words.iter().enumerate() {
        let bare = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if bare != prep {
            continue;
        }
        if let Some(next) = words.get(i + 1) {
            if is_name_candidate(next, false) {
                return Some(trim_edges(next));
            }
        }
    }
    None
}

/// Heuristic quality score for a proper-noun phrase.
///
/// Title-case words ("Priya", "Sequoia") are far more likely to be the
/// answer to a *who/where/what* question than bare acronyms ("ML", "NYC"),
/// which in conversational text are usually modifiers rather than the
/// entity being asked about — "An ML engineer named Priya" asks to resolve
/// to Priya, not ML.
fn name_quality(tokens: &[String]) -> i32 {
    let mut score = 0;
    for t in tokens {
        let is_all_caps = t.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
        if is_all_caps {
            score -= 1;
        } else {
            score += 2;
        }
    }
    score
}

/// Whether a raw token looks like part of a proper name.
///
/// `sentence_initial` tightens the test for the first word of a turn,
/// where capitalisation carries no information about namehood — "Local
/// memory systems…" starts with a capital but "Local" is not a name.
/// There, the token must additionally not be an ordinary English word.
fn is_name_candidate(raw: &str, sentence_initial: bool) -> bool {
    // Contractions ("I'm", "don't") are capitalised mid-sentence and are
    // never names.
    if raw.contains('\'') && !raw.ends_with("'s") {
        return false;
    }
    let trimmed = trim_edges(raw);
    if trimmed.chars().count() <= 1 {
        return false;
    }
    // Tokens carrying digits are dates, times, or amounts — handled by the
    // typed extractors, never a name ("Sub-3:00", "2:58:42").
    if trimmed.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let first = match trimmed.chars().next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_uppercase() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if NON_NAME_CAPS.contains(&lower.as_str()) {
        return false;
    }
    // A month name is a date component, not a name.
    if MONTHS.contains(&lower.as_str()) {
        return false;
    }
    // Sentence-initial tokens are still *candidates* (a turn may legitimately
    // begin with a name); ranking demotes them. The common-word list remains
    // as a cheap, high-precision veto for the most frequent offenders.
    if sentence_initial && COMMON_WORDS.contains(&lower.as_str()) {
        return false;
    }
    true
}

/// Ordinary English words. Used only to disambiguate sentence-initial
/// capitalisation — a capitalised word that is also a common noun, verb, or
/// adjective at the start of a sentence is almost certainly not a name.
const COMMON_WORDS: &[&str] = &[
    "about", "after", "all", "also", "always", "another", "any", "back", "because", "been",
    "before", "being", "best", "better", "big", "both", "call", "called", "came", "can", "come",
    "company", "could", "day", "days", "done", "down", "each", "early", "even", "ever", "every",
    "far", "few", "find", "first", "found", "gave", "get", "give", "going", "gone", "good",
    "got", "great", "had", "half", "hard", "have", "having", "help", "here", "high", "home",
    "hope", "hour", "hours", "how", "however", "job", "keep", "kind", "knew", "know", "last",
    "late", "later", "least", "left", "less", "let", "life", "like", "little", "live", "local",
    "long", "look", "looking", "lot", "made", "make", "making", "many", "may", "maybe", "mean",
    "memory", "might", "mile", "miles", "mind", "more", "morning", "most", "much", "music",
    "must", "name", "need", "never", "new", "news", "next", "night", "now", "off", "office",
    "often", "old", "once", "one", "only", "open", "other", "our", "out", "over", "own", "part",
    "people", "per", "place", "plan", "put", "quite", "read", "real", "really", "right", "run",
    "running", "said", "same", "saw", "say", "see", "seen", "sent", "set", "several", "she",
    "should", "show", "side", "since", "small", "some", "soon", "sort", "still", "stop", "such",
    "sure", "systems", "take", "taking", "talk", "team", "tell", "than", "that", "their", "them",
    "there", "these", "thing", "things", "think", "those", "though", "three", "through", "time",
    "times", "too", "took", "top", "training", "trip", "true", "try", "trying", "turn", "two",
    "under", "until", "use", "used", "using", "very", "want", "was", "way", "week", "weeks",
    "well", "went", "were", "while", "will", "with", "work", "working", "world", "would", "year",
    "years", "yet",
];

/// Whether a token ends a sentence (so the next capitalised word is
/// sentence-initial rather than part of the same name).
fn ends_sentence(raw: &str) -> bool {
    raw.ends_with('.') || raw.ends_with('!') || raw.ends_with('?')
}

/// Strip surrounding punctuation from a token, keeping internal characters
/// (so "Memex." → "Memex" but "sub-3:00" is untouched mid-token).
fn trim_edges(raw: &str) -> String {
    raw.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// Lowercase and strip possessives/punctuation for question-token matching.
fn normalize_token(raw: &str) -> String {
    let t = trim_edges(raw).to_lowercase();
    t.strip_suffix("'s").map(str::to_string).unwrap_or(t)
}

// ────────────────────────────────────────────────────────────────────
// Date extractor
// ────────────────────────────────────────────────────────────────────

const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "sept",
    "oct",
    "nov",
    "dec",
];

/// Extract a date phrase like "May 3rd", "April 20", "Jan 5" from a
/// turn. Returns the matched substring with original casing.
/// A date answer, preferring a precise month+day but accepting a bare
/// month.
///
/// Used only when *composing* the final answer, never for deciding which
/// candidate to read from: a bare month is a legitimate answer ("closing in
/// August") but a turn carrying only a month is weaker evidence than one
/// carrying a full date, so selection still requires the precise form.
pub fn extract_date_lenient(turn: &str) -> Option<String> {
    extract_date(turn).or_else(|| {
        extract_month(turn).map(|m| {
            let mut c = m.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => m,
            }
        })
    })
}

pub fn extract_date(turn: &str) -> Option<String> {
    let lower = turn.to_lowercase();
    for m in MONTHS {
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(m) {
            let abs = start + pos;
            // Word-boundary check on the left.
            let left_ok = abs == 0
                || lower
                    .as_bytes()
                    .get(abs - 1)
                    .map(|b| !(*b as char).is_alphanumeric())
                    .unwrap_or(true);
            let after = abs + m.len();
            let right_ok = lower
                .as_bytes()
                .get(after)
                .map(|b| !(*b as char).is_alphanumeric())
                .unwrap_or(true);
            if !(left_ok && right_ok) {
                start = abs + 1;
                continue;
            }
            // Pull "<Month> <day>[<ordinal>]" — scan forward up to 12
            // chars looking for digits with optional ordinal suffix.
            let tail = &turn[after..];
            let tail_lower = &lower[after..];
            // Skip exactly one space or comma+space between month and day.
            let day_start_in_tail = tail
                .char_indices()
                .find(|(_, c)| c.is_ascii_digit())
                .map(|(i, _)| i);
            if let Some(ds) = day_start_in_tail {
                // Must be reasonably close (≤ 4 chars of separators).
                if ds <= 4
                    && tail_lower[..ds]
                        .chars()
                        .all(|c| c == ' ' || c == ',' || c == '\t')
                {
                    let mut de = ds;
                    while de < tail.len()
                        && tail.as_bytes()[de].is_ascii_digit()
                    {
                        de += 1;
                    }
                    // Optional ordinal suffix (st/nd/rd/th).
                    let mut suffix_end = de;
                    if de + 2 <= tail.len() {
                        let ord = &tail_lower[de..de + 2];
                        if matches!(ord, "st" | "nd" | "rd" | "th") {
                            suffix_end = de + 2;
                        }
                    }
                    // Reconstruct using original-case slice.
                    let month_slice = &turn[abs..abs + m.len()];
                    let day_slice = &tail[ds..suffix_end];
                    return Some(format!("{month_slice} {day_slice}"));
                }
            }
            start = abs + m.len();
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Money extractor
// ────────────────────────────────────────────────────────────────────

/// Extract a currency amount like "$1.5M", "$2.5M", "$15M", "$1,500".
/// Returns the original-cased substring.
pub fn extract_money(turn: &str) -> Option<String> {
    let bytes = turn.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let mut j = i + 1;
            // digits + optional .digits + optional ,digits-groups
            let mut saw_digit = false;
            while j < bytes.len() {
                let c = bytes[j] as char;
                if c.is_ascii_digit() || c == '.' || c == ',' {
                    if c.is_ascii_digit() {
                        saw_digit = true;
                    }
                    j += 1;
                } else {
                    break;
                }
            }
            if saw_digit {
                // Optional magnitude suffix M/B/K (case-insensitive).
                if j < bytes.len() {
                    let c = (bytes[j] as char).to_ascii_lowercase();
                    if c == 'm' || c == 'b' || c == 'k' {
                        j += 1;
                    }
                }
                return Some(turn[i..j].to_string());
            }
        }
        i += 1;
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Time extractor (clock times like 2:58:42, 3:10, sub-3:00)
// ────────────────────────────────────────────────────────────────────

/// Extract a clock-time span. Recognises:
///   - HH:MM:SS  (e.g. "2:58:42")
///   - H:MM      (e.g. "3:10")
///   - sub-HH:MM (e.g. "sub-3:00")
pub fn extract_time(turn: &str) -> Option<String> {
    let bytes = turn.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        // Allow leading "sub-" or "Sub-" before the digit.
        let (window_start, scan_start) = if i + 4 <= n
            && turn[i..i + 4].eq_ignore_ascii_case("sub-")
        {
            (i, i + 4)
        } else {
            (i, i)
        };
        if scan_start >= n {
            break;
        }
        let c = bytes[scan_start] as char;
        if c.is_ascii_digit() {
            // Read 1-2 digits, then ':', then 2 digits, optional ':' + 2.
            let mut j = scan_start;
            while j < n && (bytes[j] as char).is_ascii_digit() {
                j += 1;
            }
            if j < n && bytes[j] == b':' && j - scan_start <= 2 {
                let h_end = j;
                j += 1;
                let m_start = j;
                while j < n && (bytes[j] as char).is_ascii_digit() {
                    j += 1;
                }
                if j - m_start == 2 {
                    let mut end = j;
                    if j < n && bytes[j] == b':' {
                        j += 1;
                        let s_start = j;
                        while j < n && (bytes[j] as char).is_ascii_digit() {
                            j += 1;
                        }
                        if j - s_start == 2 {
                            end = j;
                        }
                    }
                    // Word-boundary check on the right (no alphanumeric).
                    let after = bytes.get(end).copied();
                    let right_ok = after.map(|b| !(b as char).is_alphanumeric()).unwrap_or(true);
                    if right_ok {
                        // Word-boundary on the left (or "sub-" prefix).
                        let left_ok = window_start == 0
                            || (bytes[window_start - 1] as char).is_whitespace()
                            || matches!(
                                bytes[window_start - 1] as char,
                                ',' | '.' | '!' | '?' | ';' | ':' | '(' | '"' | '\''
                            );
                        if left_ok {
                            return Some(turn[window_start..end].to_string());
                        }
                    }
                    let _ = h_end; // unused, but keeps the bounds explicit
                }
            }
            i = j.max(scan_start + 1);
        } else {
            i += 1;
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Yes/No oracle
// ────────────────────────────────────────────────────────────────────

/// Build a yes/no answer for a polar question, given the best matching
/// turn. Strategy:
///
/// 1. Classify the question's *check kind* — what specific value is
///    being asserted (a date? a name? a number?).
/// 2. Extract the same kind of value from the turn.
/// 3. If both present and they don't match → "No, <turn value>".
/// 4. If both present and they match → "Yes".
/// 5. If turn carries a typed value but the question doesn't, default
///    to "No, <turn value>" (the question's framing usually implies a
///    contradiction when the turn names a different specific thing).
/// 6. Otherwise → "Yes" (positive default; rare in our fixtures).
pub fn yes_no_answer(question: &str, turn: &str) -> String {
    // Money first (most specific, dollar-prefix is unambiguous).
    if let (Some(qm), Some(tm)) = (extract_money(question), extract_money(turn)) {
        return if normalize_money(&qm) == normalize_money(&tm) {
            "Yes".to_string()
        } else {
            format!("No, {tm}")
        };
    }
    // Date.
    if let (Some(qd), Some(td)) = (extract_date(question), extract_date(turn)) {
        return if qd.eq_ignore_ascii_case(&td) {
            "Yes".to_string()
        } else {
            format!("No, {td}")
        };
    }
    // Month-level comparison. "Did Nadia defend in June?" against evidence
    // that says July is a contradiction even though the question carries no
    // full date for the date branch above to match on.
    if let (Some(qm), Some(tm)) = (extract_month(question), extract_month(turn)) {
        if qm != tm {
            if let Some(d) = extract_date(turn) {
                return format!("No, {d}");
            }
            return format!("No, {tm}");
        }
    }

    // Capitalized-token check (proper-noun assertions like
    // "Andreessen Horowitz" → check turn for a different proper noun).
    if let Some(q_proper) = first_proper_phrase_after_yn(question) {
        let lower_turn = turn.to_lowercase();
        if !lower_turn.contains(&q_proper.to_lowercase()) {
            // Prefer a proper phrase preceded by a preposition
            // ("from Sequoia", "at Mercury") — that's the salient
            // entity, not the sentence-initial verb ("Got").
            if let Some(t_proper) = proper_phrase_after_preposition(turn) {
                if !t_proper.eq_ignore_ascii_case(&q_proper) {
                    return format!("No, {t_proper}");
                }
            }
            if let Some(t_proper) = first_proper_phrase(turn) {
                if !t_proper.eq_ignore_ascii_case(&q_proper) {
                    return format!("No, {t_proper}");
                }
            }
        } else {
            return "Yes".to_string();
        }
    }
    // Fallback: if the turn carries a typed value the question does not
    // already state, surface it with a "No," prefix. The novelty check
    // matters here: echoing back the question's own premise ("Did Ethan
    // miss his sub-3:00 goal?" -> "No, Sub-3:00") states nothing and reads
    // as confirmation of the premise rather than a correction.
    for extractor in [extract_money, extract_date, extract_time] {
        if let Some(v) = first_novel_value(extractor, turn, question) {
            return format!("No, {v}");
        }
    }
    // Default — no signal either way.
    "Yes".to_string()
}

fn normalize_money(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect::<String>()
        .to_lowercase()
}

/// Pull the first multi-word proper-noun phrase out of `s`. A proper
/// noun = capitalized first letter, may contain inner capitals.
fn first_proper_phrase(s: &str) -> Option<String> {
    let mut buf = String::new();
    let mut started = false;
    for tok in s.split_whitespace() {
        // Strip leading/trailing punctuation that isn't part of the name.
        let cleaned = tok.trim_matches(|c: char| !c.is_alphanumeric());
        if cleaned.is_empty() {
            if started {
                break;
            } else {
                continue;
            }
        }
        let first = cleaned.chars().next()?;
        if first.is_ascii_uppercase() {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(cleaned);
            started = true;
        } else if started {
            break;
        }
    }
    if buf.is_empty() {
        None
    } else {
        Some(buf)
    }
}

/// Walk `s` and return the first proper-noun phrase that is *preceded
/// by a preposition* like "from/at/by/with/to/in/for". This filters out
/// sentence-initial capitalized verbs ("Got the term sheet …") so we
/// land on the salient named entity ("from Sequoia").
fn proper_phrase_after_preposition(s: &str) -> Option<String> {
    const PREPS: &[&str] = &[
        "from", "at", "by", "with", "to", "in", "for", "on", "of", "into", "about",
    ];
    let toks: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0;
    while i + 1 < toks.len() {
        let lc = toks[i].trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if PREPS.contains(&lc.as_str()) {
            // Greedily capture consecutive capitalized tokens after the prep.
            let start = i + 1;
            let mut end = start;
            while end < toks.len() {
                let cleaned = toks[end].trim_matches(|c: char| !c.is_alphanumeric());
                if cleaned
                    .chars()
                    .next()
                    .map_or(false, |c| c.is_ascii_uppercase())
                {
                    end += 1;
                } else {
                    break;
                }
            }
            if end > start {
                let parts: Vec<&str> = toks[start..end]
                    .iter()
                    .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
                    .filter(|t| !t.is_empty())
                    .collect();
                if !parts.is_empty() {
                    return Some(parts.join(" "));
                }
            }
        }
        i += 1;
    }
    None
}

/// Same as [`first_proper_phrase`] but skips the leading auxiliary
/// verb in a yes/no question and any low-content connectors before the
/// first proper noun (e.g. "Did Carol raise from Andreessen Horowitz?"
/// → "Andreessen Horowitz", not "Carol").
fn first_proper_phrase_after_yn(question: &str) -> Option<String> {
    // Drop the leading auxiliary so "Did Carol …" doesn't latch onto
    // the subject ("Carol") instead of the asserted object.
    let mut tokens = question.split_whitespace().peekable();
    if let Some(first) = tokens.peek() {
        let lc = first.to_lowercase();
        if YN_LEAD.contains(&lc.as_str()) {
            tokens.next();
        }
    }
    // Walk until we find the FIRST capitalized token, then keep going
    // while subsequent tokens are also capitalized (multi-word names).
    // *Skip* the subject (single capitalized token followed by a lowercase
    // verb) so we hit the asserted object further along.
    let toks: Vec<&str> = tokens.collect();
    let mut i = 0;
    while i < toks.len() {
        let cleaned = toks[i].trim_matches(|c: char| !c.is_alphanumeric());
        if cleaned.is_empty() {
            i += 1;
            continue;
        }
        if cleaned.chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
            // Greedily capture consecutive capitalized tokens.
            let start = i;
            let mut end = i + 1;
            while end < toks.len() {
                let nxt = toks[end].trim_matches(|c: char| !c.is_alphanumeric());
                if nxt.chars().next().map_or(false, |c| c.is_ascii_uppercase()) {
                    end += 1;
                } else {
                    break;
                }
            }
            // Single capitalized token at the START of the (post-aux)
            // question is almost always the subject — skip and keep
            // looking for the object phrase.
            if start == 0 && end == start + 1 {
                i = end;
                continue;
            }
            let parts: Vec<&str> = toks[start..end]
                .iter()
                .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
                .filter(|t| !t.is_empty())
                .collect();
            return Some(parts.join(" "));
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_yn() {
        assert_eq!(classify_question("Did Alice fly to Tokyo?"), QKind::YesNo);
        assert_eq!(
            classify_question("Was Carol's company renamed?"),
            QKind::YesNo
        );
        assert_eq!(
            classify_question("Is the term sheet signed?"),
            QKind::YesNo
        );
    }

    #[test]
    fn classify_when_is_date() {
        assert_eq!(classify_question("When is Alice flying?"), QKind::Date);
        assert_eq!(classify_question("When did Ethan run Boston?"), QKind::Date);
    }

    #[test]
    fn classify_how_much_is_money() {
        assert_eq!(
            classify_question("How much did Carol raise?"),
            QKind::Money
        );
        assert_eq!(
            classify_question("How many people work at Stripe?"),
            QKind::Money
        );
    }

    #[test]
    fn classify_finish_time_is_time() {
        assert_eq!(
            classify_question("What was Ethan's finish time?"),
            QKind::Time
        );
        assert_eq!(
            classify_question("What was the goal time?"),
            QKind::Time
        );
    }

    #[test]
    fn classify_what_is_generic() {
        assert_eq!(
            classify_question("What is Alice's talk about?"),
            QKind::Generic
        );
    }

    #[test]
    fn extract_date_pulls_month_day_with_ordinal() {
        assert_eq!(
            extract_date("I'm flying to Tokyo on May 3rd for a conference.").as_deref(),
            Some("May 3rd")
        );
        assert_eq!(
            extract_date("Race day is April 20.").as_deref(),
            Some("April 20")
        );
        assert_eq!(
            extract_date("She joined on Jan 5, 2026.").as_deref(),
            Some("Jan 5")
        );
    }

    #[test]
    fn extract_date_returns_none_when_no_month() {
        assert!(extract_date("Just landed at Haneda, exhausted.").is_none());
    }

    #[test]
    fn extract_money_pulls_dollar_amounts() {
        assert_eq!(
            extract_money("Pre-seed, $1.5M target.").as_deref(),
            Some("$1.5M")
        );
        assert_eq!(
            extract_money("They wanted to lead at $2.5M instead, so we upsized.").as_deref(),
            Some("$2.5M")
        );
        assert_eq!(extract_money("$15M post.").as_deref(), Some("$15M"));
    }

    #[test]
    fn extract_money_handles_no_suffix() {
        assert_eq!(extract_money("It cost $1,500 total.").as_deref(), Some("$1,500"));
    }

    #[test]
    fn extract_time_pulls_clock_times() {
        assert_eq!(
            extract_time("Finished Boston in 2:58:42!").as_deref(),
            Some("2:58:42")
        );
        assert_eq!(
            extract_time("Sub-3:00 is the dream, but realistically 3:10.").as_deref(),
            Some("Sub-3:00")
        );
    }

    #[test]
    fn extract_time_rejects_word_boundary_violations() {
        // "60 miles" must NOT match (mile is not a time).
        assert!(extract_time("peaking at 60 miles").is_none());
    }

    #[test]
    fn yn_money_mismatch_returns_no_with_correction() {
        let q = "Did Carol raise on $1.5M?";
        let turn = "They wanted to lead at $2.5M instead, so we upsized.";
        assert_eq!(yes_no_answer(q, turn), "No, $2.5M");
    }

    #[test]
    fn yn_date_mismatch_returns_no_with_correction() {
        let q = "Did Alice fly to Tokyo on May 1st?";
        let turn = "I'm flying to Tokyo on May 3rd for a conference.";
        assert_eq!(yes_no_answer(q, turn), "No, May 3rd");
    }

    #[test]
    fn yn_proper_noun_mismatch_returns_no_with_correction() {
        let q = "Did Carol raise from Andreessen Horowitz?";
        let turn = "Got the term sheet from Sequoia today.";
        assert_eq!(yes_no_answer(q, turn), "No, Sequoia");
    }

    #[test]
    fn preposition_cue_score_leaves_headroom_for_bonuses() {
        // Regression: the cue score used to be i32::MAX, and resolve_named
        // adds up to +10 to it, wrapping the total negative and silently
        // discarding the strongest signal in the extractor.
        assert!(PREPOSITION_CUE_SCORE.checked_add(100).is_some());
    }

    #[test]
    fn resolve_named_honours_the_preposition_cue() {
        let turn = "Renamed Loom to Memex — the original name was trademarked.";
        let q = "What did Carol rename her company to?";
        // Even when the graph knows the *other* candidate, the syntactic
        // cue must win.
        let lookup = |n: &str| -> Option<String> {
            if n == "Loom" { Some("Project".into()) } else { None }
        };
        assert_eq!(resolve_named(turn, q, &lookup).as_deref(), Some("Memex"));
    }

    #[test]
    fn asks_for_latest_detects_state_cues() {
        assert!(asks_for_latest("Which team is Maya on now?"));
        assert!(asks_for_latest("How much did they finally agree on?"));
        assert!(asks_for_latest("What is her current role?"));
    }

    #[test]
    fn asks_for_latest_ignores_plain_questions() {
        assert!(!asks_for_latest("Which company did Maya join?"));
        assert!(!asks_for_latest("Who is Carol's first hire?"));
    }

    #[test]
    fn asks_for_latest_respects_word_boundaries() {
        // "known" contains "now"; "finalise" contains "final".
        assert!(!asks_for_latest("What is the known address?"));
        assert!(!asks_for_latest("Did they finalise the deal?"));
    }

    #[test]
    fn yn_match_returns_yes() {
        let q = "Did Alice fly to Tokyo on May 3rd?";
        let turn = "I'm flying to Tokyo on May 3rd for a conference.";
        assert_eq!(yes_no_answer(q, turn), "Yes");
    }

    #[test]
    fn yn_proper_noun_match_returns_yes() {
        let q = "Did Carol raise from Sequoia?";
        let turn = "Got the term sheet from Sequoia today.";
        assert_eq!(yes_no_answer(q, turn), "Yes");
    }

    #[test]
    fn first_proper_phrase_after_yn_skips_subject() {
        // "Did Carol raise from Andreessen Horowitz?"
        // The subject (Carol) should be skipped; the object phrase
        // (Andreessen Horowitz) should be returned.
        assert_eq!(
            first_proper_phrase_after_yn("Did Carol raise from Andreessen Horowitz?")
                .as_deref(),
            Some("Andreessen Horowitz")
        );
    }

    #[test]
    fn compose_short_answer_dispatches_correctly() {
        // Money question → just the $ amount.
        assert_eq!(
            compose_short_answer(
                "How much did Carol raise?",
                "They wanted to lead at $2.5M instead, so we upsized."
            ),
            "$2.5M"
        );
        // Date question → just the date.
        assert_eq!(
            compose_short_answer(
                "When is Alice flying to Tokyo?",
                "I'm flying to Tokyo on May 3rd for a conference."
            ),
            "May 3rd"
        );
        // Generic question → returns turn unchanged.
        assert_eq!(
            compose_short_answer(
                "What is Alice's talk about?",
                "Local memory systems for consumer apps."
            ),
            "Local memory systems for consumer apps."
        );
    }

    #[test]
    fn compose_short_answer_falls_back_when_extractor_misses() {
        // No date in the turn but classified as Date — return turn as-is.
        let turn = "Just landed at Haneda, exhausted.";
        assert_eq!(
            compose_short_answer("When did Alice land?", turn),
            turn
        );
    }
}
