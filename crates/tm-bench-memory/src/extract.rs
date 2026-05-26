//! Tight span extractor for persistence answers.
//!
//! Same problem token-F1 always has: a long candidate dilutes precision.
//! For W-3 the candidate is the *single most-relevant retrieved
//! sentence* from Session A's ingest, but a 12-word sentence around a
//! 2-word answer still scores poorly. So we classify the query and
//! pull the tightest span we can defend.
//!
//! Kept hand-written (no regex crate) for two reasons: smaller binary
//! and no dependency on `regex`'s build, which matters in the CI gate
//! path that needs to be fast.

use std::collections::HashSet;

/// What the query is asking for. Same idea as the LoCoMo extractor's
/// `QKind`, but tuned for the W-3 fixture (which has more
/// name-resolution and decision-history shapes than LoCoMo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QKind {
    /// Calendar date — "When …?", "What date …?"
    Date,
    /// Person name — "Who …?", "With whom …?"
    Person,
    /// Place / location — "Where …?"
    Place,
    /// Cardinal number / count — "How many …?"
    Number,
    /// Currency — "How much did X cost?"
    Money,
    /// Storage / memory amount — "How much memory does X have?"
    Quantity,
    /// Duration — "How long is X?"
    Duration,
    /// Decision / choice — "What did I pick?", "Which X did I choose?"
    Choice,
    /// No specialized extractor; return candidate as-is and let the
    /// scorer + normalization sort it out.
    Generic,
}

const YN_LEAD: &[&str] = &[
    "did", "does", "do", "is", "are", "was", "were", "has", "have", "had", "will", "can", "could",
    "should", "would",
];

/// Words like "is X?" that don't disambiguate intent; reserved.
const QUANTITY_HINTS: &[&str] = &[
    "memory", "ram", "storage", "disk", "capacity", "size", "space",
];

const DURATION_HINTS: &[&str] = &[
    "long", "sprint", "cycle", "period",
];

const CHOICE_HINTS: &[&str] = &[
    "language", "stack", "platform", "framework", "algorithm",
    "model", "library", "tool", "approach", "school", "billing",
    "embedding", "clustering", "styling", "style",
    // Decision-history shapes that aren't quite Choice but
    // benefit from the same marker-driven extraction.
    "host", "cadence", "method", "system", "scheme", "process",
];

pub fn classify(q: &str) -> QKind {
    let lower = q.trim().to_lowercase();
    let mut tokens = lower.split_whitespace();
    let first = match tokens.next() {
        Some(t) => t,
        None => return QKind::Generic,
    };

    // "Which X did I pick?" / "What X did I choose?" always Choice
    if lower.contains("did i pick")
        || lower.contains("did i choose")
        || lower.contains("did i decide")
        || lower.contains("did i go with")
        || lower.contains("did i settle")
        || lower.contains("did i commit")
        || lower.contains("did i set")
        || lower.contains("am i shipping")
        || lower.contains("am i using")
        || lower.contains("am i choosing")
        || lower.contains("are we using")
        || lower.contains("are we shipping")
        || lower.contains("are we choosing")
        || lower.contains("did we pick")
        || lower.contains("did we choose")
        || lower.contains("did we decide")
    {
        return QKind::Choice;
    }

    if first == "which" {
        return QKind::Choice;
    }

    if first == "when"
        || lower.contains("what date")
        || lower.contains("which date")
        || lower.contains("what time")
        || lower.contains("at what time")
        || lower.contains("which day")
        || lower.contains("what day")
    {
        return QKind::Date;
    }
    if first == "who" || lower.contains("with whom") {
        return QKind::Person;
    }
    if first == "where" {
        return QKind::Place;
    }
    if first == "how" {
        match tokens.next() {
            Some("many") => return QKind::Number,
            Some("much") => {
                // "how much memory" / "how much storage" → quantity
                if QUANTITY_HINTS.iter().any(|h| lower.contains(h)) {
                    return QKind::Quantity;
                }
                return QKind::Money;
            }
            Some("long") => return QKind::Duration,
            _ => {}
        }
    }

    // What/X queries that point at a choice-y noun → Choice
    if first == "what" && CHOICE_HINTS.iter().any(|h| lower.contains(h)) {
        return QKind::Choice;
    }

    if YN_LEAD.contains(&first) {
        return QKind::Generic;
    }

    QKind::Generic
}

/// If the candidate is a retraction sentence containing a
/// "no — actually" / "actually" / "scratch that" marker, slice it down
/// to the tail after the LAST such marker. So
/// `"We changed it to Lisbon, no — actually to Porto."` becomes
/// `"to Porto."` — the post-retraction content. Idempotent.
fn slice_after_retraction(candidate: &str) -> String {
    let lower = candidate.to_lowercase();
    let markers: &[&str] = &[
        "no — actually ",
        "no, actually ",
        "no - actually ",
        "scratch that — ",
        "scratch that, ",
        "scratch that ",
        "actually, ",
        "actually ",
    ];
    let mut latest: Option<usize> = None;
    for m in markers {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(m) {
            let abs = from + rel + m.len();
            latest = Some(latest.map_or(abs, |l| l.max(abs)));
            from = abs;
        }
    }
    if let Some(start) = latest {
        if start < candidate.len() {
            return candidate[start..].to_string();
        }
    }
    candidate.to_string()
}

/// Compose a short prediction from a single candidate sentence. Falls
/// back to the candidate itself when no extractor fires confidently.
pub fn compose(query: &str, raw_candidate: &str) -> String {
    let sliced = slice_after_retraction(raw_candidate);
    let candidate = sliced.as_str();
    match classify(query) {
        QKind::Date => extract_date(candidate).unwrap_or_else(|| candidate.to_string()),
        QKind::Number => extract_number(candidate).unwrap_or_else(|| candidate.to_string()),
        QKind::Money => extract_money(candidate)
            .or_else(|| extract_number(candidate))
            .unwrap_or_else(|| candidate.to_string()),
        QKind::Quantity => extract_quantity(candidate)
            .or_else(|| extract_number(candidate))
            .unwrap_or_else(|| candidate.to_string()),
        QKind::Duration => extract_duration(candidate).unwrap_or_else(|| candidate.to_string()),
        // For Person/Place/Choice/Generic: try choice marker first
        // (handles retraction-aware "going with X" / "set X to Y"),
        // then value patterns ("X is Y"), then capitalized clusters.
        QKind::Person => extract_choice(candidate, query)
            .filter(|s| !s.is_empty())
            .or_else(|| extract_person(candidate, query))
            .or_else(|| extract_value(query, candidate))
            .unwrap_or_else(|| candidate.to_string()),
        QKind::Place => extract_place_pattern(candidate)
            .or_else(|| extract_choice(candidate, query).filter(|s| !s.is_empty()))
            .or_else(|| extract_person(candidate, query))
            .or_else(|| extract_value(query, candidate))
            .unwrap_or_else(|| candidate.to_string()),
        QKind::Choice => extract_topic_is(query, candidate)
            .or_else(|| extract_choice(candidate, query).filter(|s| !s.is_empty()))
            .or_else(|| extract_place_pattern(candidate))
            .or_else(|| extract_person(candidate, query))
            .or_else(|| extract_value(query, candidate))
            .unwrap_or_else(|| candidate.to_string()),
        QKind::Generic => extract_choice(candidate, query)
            .filter(|s| !s.is_empty())
            .or_else(|| extract_value(query, candidate))
            .or_else(|| extract_money(candidate))
            .or_else(|| extract_topic_suffix(query, candidate))
            .or_else(|| extract_person(candidate, query))
            .unwrap_or_else(|| candidate.to_string()),
    }
}

// ────────────────────────────────────────────────────────────────────
// Topic-suffix extractor — for Generic queries.
//
// When the query has a clear topic noun (e.g., "coffee" in
// "How do I prefer my coffee?") and that noun appears in the candidate,
// the answer often follows: "I take my coffee black with no sugar." →
// the part after "coffee " is the answer.
//
// We only fire when (a) the candidate contains the topic, (b) there's
// text after it, and (c) that text isn't just a sentence terminator.
// ────────────────────────────────────────────────────────────────────

const TOPIC_STOPWORDS: &[&str] = &[
    "what", "when", "where", "which", "who", "whom", "whose", "why", "how",
    "did", "does", "do", "was", "were", "are", "is", "be", "been",
    "the", "a", "an", "my", "your", "our", "his", "her", "their",
    "and", "or", "but", "of", "to", "for", "from", "with", "in", "on", "at",
    "i", "you", "we", "they", "he", "she", "it", "me", "him", "us", "them",
    "this", "that", "these", "those",
    "have", "has", "had", "will", "would", "should", "could", "can",
    "prefer", "like", "want", "need", "take", "use", "make", "find",
    "ship", "start", "ship",
];

fn topic_word(query: &str) -> Option<String> {
    // Pick the LAST content word in the query (often the topic).
    topic_words(query).into_iter().last()
}

/// All content words from the query in order. Used by callers that want
/// to try multiple topic candidates (e.g., extract_topic_is iterates so
/// "What style does my yoga teacher Anna practice?" can match on
/// "style" even though "practice" is the last word).
fn topic_words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty() && w.len() >= 3 && !TOPIC_STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Find the wh-focus noun in a query: the noun immediately following
/// "which" or "what". Returns None for other shapes.
///
/// "Which city does my friend live in?" → Some("city")
/// "What style does Anna practice?"      → Some("style")
/// "How do I prefer my coffee?"          → None (Generic; no wh-focus)
fn wh_focus_noun(query: &str) -> Option<String> {
    let lower = query.to_lowercase();
    let words: Vec<String> = lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .collect();
    for i in 0..words.len().saturating_sub(1) {
        if words[i] == "which" || words[i] == "what" {
            let next = &words[i + 1];
            // Skip "kind of X" / "type of X" → use X.
            if (next == "kind" || next == "type" || next == "sort")
                && i + 3 < words.len()
                && words[i + 2] == "of"
            {
                let cand = &words[i + 3];
                if cand.len() >= 3 && !TOPIC_STOPWORDS.contains(&cand.as_str()) {
                    return Some(cand.clone());
                }
            }
            if next.len() >= 3 && !TOPIC_STOPWORDS.contains(&next.as_str()) {
                return Some(next.clone());
            }
        }
    }
    None
}

/// Find a " <focus> is X" / "<Focus> is X" pattern in `candidate` where
/// `focus` is the wh-target noun from the query. Returns X.
///
/// Only fires for wh-shaped queries — "Which style is X?" /
/// "What language are we using?". Skips Generic queries entirely so it
/// can't grab unrelated " X is Y" matches.
pub fn extract_topic_is(query: &str, candidate: &str) -> Option<String> {
    let focus = wh_focus_noun(query)?;
    let lower = candidate.to_lowercase();
    for sep in [" is ", " was ", " are ", " were "] {
        let pat = format!(" {}{}", focus, sep);
        if let Some(idx) = lower.find(&pat) {
            let start = idx + pat.len();
            if start < candidate.len() {
                let tail = &candidate[start..];
                let span = take_until_stop(tail);
                if !span.is_empty() {
                    return Some(span);
                }
            }
        }
        // Start-of-sentence variant: "Style is X."
        let pat2 = format!("{}{}", focus, sep);
        if lower.starts_with(&pat2) {
            let start = pat2.len();
            let tail = &candidate[start..];
            let span = take_until_stop(tail);
            if !span.is_empty() {
                return Some(span);
            }
        }
        // Possessive variant: "Her style is X" / "His tool is X" — find
        // " <pronoun> <focus> is X" after the wh-noun anchor.
        for poss in [" her ", " his ", " their ", " our ", " my ", " your "] {
            let pat3 = format!("{}{}{}", poss, focus, sep);
            if let Some(idx) = lower.find(&pat3) {
                let start = idx + pat3.len();
                if start < candidate.len() {
                    let tail = &candidate[start..];
                    let span = take_until_stop(tail);
                    if !span.is_empty() {
                        return Some(span);
                    }
                }
            }
            // sentence-start: "Her style is X."
            let pat4 = format!("{}{}{}", poss.trim_start(), focus, sep);
            if lower.starts_with(&pat4) {
                let start = pat4.len();
                let tail = &candidate[start..];
                let span = take_until_stop(tail);
                if !span.is_empty() {
                    return Some(span);
                }
            }
        }
    }
    None
}

pub fn extract_topic_suffix(query: &str, candidate: &str) -> Option<String> {
    let topic = topic_word(query)?;
    if topic.len() < 3 {
        return None;
    }
    let lower = candidate.to_lowercase();
    let idx = find_word(&lower, &topic)?;
    let start = idx + topic.len();
    if start >= candidate.len() {
        return None;
    }
    let tail = &candidate[start..];
    let tail = tail.trim_start();
    if tail.is_empty() {
        return None;
    }
    // Stop at next sentence terminator.
    let span = take_until_stop(tail);
    if span.is_empty() || span.len() < 2 {
        return None;
    }
    Some(span)
}

// ────────────────────────────────────────────────────────────────────
// Date extractor
// ────────────────────────────────────────────────────────────────────

const MONTHS: &[&str] = &[
    "january", "february", "march", "april", "may", "june", "july",
    "august", "september", "october", "november", "december",
    "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec",
];

const WEEKDAYS: &[&str] = &[
    "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
];

const RELATIVE_DAY: &[&str] = &["today", "tomorrow", "tonight", "yesterday"];

const TIME_OF_DAY: &[&str] = &["morning", "afternoon", "evening", "night"];

pub fn extract_date(s: &str) -> Option<String> {
    let lower = s.to_lowercase();

    // ISO date: 2026-05-03
    if let Some(span) = scan_iso_date(&lower) {
        return Some(verbatim_span(s, &lower, &span));
    }

    // "May 3", "May 3rd", "May 3, 2026"
    for m in MONTHS {
        if let Some(idx) = lower.find(m) {
            let end_month = idx + m.len();
            let after = &lower[end_month..];
            // Need a space + a digit to follow for a date.
            let after_trim = after.trim_start();
            if after_trim.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                let mut end = end_month;
                while end < lower.len() && lower.as_bytes()[end].is_ascii_whitespace() {
                    end += 1;
                }
                while end < lower.len() && lower.as_bytes()[end].is_ascii_digit() {
                    end += 1;
                }
                // Optional "st/nd/rd/th"
                if end + 2 <= lower.len() {
                    let rest = &lower[end..end + 2];
                    if matches!(rest, "st" | "nd" | "rd" | "th") {
                        end += 2;
                    }
                }
                // Optional ", year"
                let tail = &lower[end..];
                if let Some(stripped) = tail.strip_prefix(", ") {
                    let mut yend = end + 2;
                    let mut yc = 0;
                    for c in stripped.chars() {
                        if c.is_ascii_digit() && yc < 4 {
                            yend += 1;
                            yc += 1;
                        } else {
                            break;
                        }
                    }
                    if yc == 4 {
                        end = yend;
                    }
                }
                return Some(verbatim_span(s, &lower, &(idx, end)));
            }
        }
    }

    // "next Tuesday", "this Friday", "by Monday", "on Wednesday",
    // optionally followed by " morning/afternoon/evening"
    for w in WEEKDAYS {
        if let Some(idx) = lower.find(w) {
            let start = preceding_qualifier(&lower, idx).unwrap_or(idx);
            let mut end = idx + w.len();
            // optional time-of-day
            let tail = &lower[end..];
            for t in TIME_OF_DAY {
                let space_t = format!(" {}", t);
                if tail.starts_with(&space_t) {
                    end += space_t.len();
                    break;
                }
            }
            return Some(verbatim_span(s, &lower, &(start, end)));
        }
    }

    // "today", "tomorrow", "yesterday", "tonight"
    for r in RELATIVE_DAY {
        if let Some(idx) = lower.find(r) {
            let end = idx + r.len();
            return Some(verbatim_span(s, &lower, &(idx, end)));
        }
    }

    // Clock times: "2pm", "11am", "10:30am", "2:00 pm". Run BEFORE the
    // "in N weeks" branch — "2pm" is a more useful answer than "in 2pm".
    if let Some(span) = scan_clock_time(&lower) {
        return Some(verbatim_span(s, &lower, &span));
    }

    // Bare month name with no day after it: "moved to June.".
    // Has to run AFTER the "May 3" branch above and AFTER the weekday
    // branch (which catches "Friday at 11am"). Skip "may" as a bare
    // month because "may" is also a modal verb — too many false hits.
    for m in MONTHS {
        if *m == "may" {
            continue;
        }
        if let Some(idx) = find_word(&lower, m) {
            let end = idx + m.len();
            return Some(verbatim_span(s, &lower, &(idx, end)));
        }
    }

    // "in N days/weeks/months"
    if let Some(idx) = lower.find("in ") {
        let after_in = idx + 3;
        if after_in < lower.len() {
            let mut p = after_in;
            let digit_start = p;
            while p < lower.len() && lower.as_bytes()[p].is_ascii_digit() {
                p += 1;
            }
            if p == digit_start {
                for word_num in [
                    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
                ] {
                    if lower[after_in..].starts_with(word_num) {
                        p = after_in + word_num.len();
                        break;
                    }
                }
            }
            if p > digit_start {
                while p < lower.len() && lower.as_bytes()[p].is_ascii_whitespace() {
                    p += 1;
                }
                for unit in [
                    "days", "day", "weeks", "week", "months", "month", "years", "year",
                ] {
                    if lower[p..].starts_with(unit) {
                        let end = p + unit.len();
                        return Some(verbatim_span(s, &lower, &(idx, end)));
                    }
                }
            }
        }
    }

    None
}

fn scan_iso_date(lower: &str) -> Option<(usize, usize)> {
    let bytes = lower.as_bytes();
    let n = bytes.len();
    if n < 10 {
        return None;
    }
    for i in 0..=(n - 10) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
            && bytes[i + 4] == b'-'
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6].is_ascii_digit()
            && bytes[i + 7] == b'-'
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
        {
            return Some((i, i + 10));
        }
    }
    None
}

/// Match clock times: `2pm`, `11am`, `10:30am`, `2:00 pm`.
fn scan_clock_time(lower: &str) -> Option<(usize, usize)> {
    let bytes = lower.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // optional :MM
            if i + 2 < n && bytes[i] == b':' && bytes[i + 1].is_ascii_digit() && bytes[i + 2].is_ascii_digit() {
                i += 3;
            }
            // optional space
            let mut after = i;
            while after < n && bytes[after] == b' ' {
                after += 1;
            }
            if after + 1 < n + 1 {
                let tail = &lower[after..];
                if tail.starts_with("am") || tail.starts_with("pm") {
                    return Some((start, after + 2));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Find a whole word — must be preceded and followed by non-alpha.
fn find_word(haystack: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let abs = from + rel;
        let before_ok = abs == 0
            || !haystack[..abs]
                .chars()
                .last()
                .map(|c| c.is_alphabetic())
                .unwrap_or(false);
        let end = abs + needle.len();
        let after_ok = end == haystack.len()
            || !haystack[end..]
                .chars()
                .next()
                .map(|c| c.is_alphabetic())
                .unwrap_or(false);
        if before_ok && after_ok {
            return Some(abs);
        }
        from = abs + 1;
    }
    None
}

fn preceding_qualifier(lower: &str, idx: usize) -> Option<usize> {
    if idx == 0 {
        return None;
    }
    let head = &lower[..idx];
    // "on " is intentionally excluded — "on Friday" should normalize to
    // "Friday" since references almost always list the bare weekday.
    for q in ["next ", "this ", "by ", "last "] {
        if head.ends_with(q) {
            return Some(idx - q.len());
        }
    }
    None
}

fn verbatim_span(orig: &str, lower: &str, span: &(usize, usize)) -> String {
    let (a, b) = (span.0.min(orig.len()), span.1.min(orig.len()));
    if orig.is_char_boundary(a) && orig.is_char_boundary(b) {
        orig[a..b].to_string()
    } else {
        lower[span.0..span.1].to_string()
    }
}

// ────────────────────────────────────────────────────────────────────
// Person / Place extractor — capitalized clusters, refined.
// ────────────────────────────────────────────────────────────────────

const PRONOUNS: &[&str] = &[
    "he", "she", "it", "they", "his", "her", "their", "them",
    "my", "your", "our", "us", "we", "i", "you", "me", "him",
    // Contraction forms — non-alnum stripping turns "I'm" → "Im",
    // "you're" → "youre", "she's" → "shes", etc. Without these the
    // capitalized-cluster pass returns the contraction itself
    // ("I'm pitching X" → "I'm") instead of walking past to the
    // proper noun.
    "im", "youre", "were", "theyre", "hes", "shes", "its",
    "id", "youd", "wed", "theyd", "hed", "shed",
    "ive", "youve", "weve", "theyve",
    "ill", "youll", "well", "theyll", "hell", "shell",
];

const SKIP_LEADING: &[&str] = &[
    "wait", "actually", "sorry", "scratch", "no", "yes", "ok", "okay",
    "well", "look", "hey", "oh",
    "met", "hired", "got", "set", "made", "took", "saw", "did",
    "picked", "chose", "going", "settled", "committed", "decided",
    "the", "a", "an",
    "updated", "bumped", "moved", "changed", "rescheduled",
    "adopted", "started", "booked",
];

fn alnum_lower(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn is_pronoun(w: &str) -> bool {
    let key = alnum_lower(w);
    PRONOUNS.contains(&key.as_str())
}

fn is_skip_leading(w: &str) -> bool {
    let key = alnum_lower(w);
    SKIP_LEADING.contains(&key.as_str())
}

fn q_tokens_lower(query: &str) -> HashSet<String> {
    query
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Collect runs of consecutive capitalized tokens from `s` (separated
/// by lowercase tokens / punctuation).
fn capitalized_clusters(s: &str) -> Vec<Vec<String>> {
    let mut clusters: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for tok in s.split_whitespace() {
        let stripped = tok.trim_matches(|c: char| !c.is_alphanumeric());
        if stripped.is_empty() {
            if !cur.is_empty() {
                clusters.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let first_upper = stripped
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false);
        if first_upper {
            cur.push(stripped.to_string());
        } else if !cur.is_empty() {
            clusters.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        clusters.push(cur);
    }
    clusters
}

/// Strip leading discourse / verb / pronoun tokens from a cluster.
fn refine_cluster(c: Vec<String>) -> Vec<String> {
    let mut result = c;
    while let Some(first) = result.first() {
        if is_pronoun(first) || is_skip_leading(first) {
            result.remove(0);
        } else {
            break;
        }
    }
    result
}

pub fn extract_person(candidate: &str, query: &str) -> Option<String> {
    let q_tokens = q_tokens_lower(query);
    for cluster in capitalized_clusters(candidate) {
        let refined = refine_cluster(cluster);
        if refined.is_empty() {
            continue;
        }
        // Skip cluster fully covered by the query.
        if refined.iter().all(|t| q_tokens.contains(&t.to_lowercase())) {
            continue;
        }
        return Some(refined.join(" "));
    }
    None
}

/// Backwards-compatible alias (used in tests).
pub fn extract_place(candidate: &str, query: &str) -> Option<String> {
    extract_person(candidate, query)
}

// ────────────────────────────────────────────────────────────────────
// Number extractor — first integer, optional comma-grouping.
// ────────────────────────────────────────────────────────────────────

pub fn extract_number(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_digit() {
            start = Some(i);
            break;
        }
    }
    let start = start?;
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b',') {
        end += 1;
    }
    if end > start && bytes[end - 1] == b',' {
        end -= 1;
    }
    Some(s[start..end].to_string())
}

// ────────────────────────────────────────────────────────────────────
// Money extractor — $X, $X.YY, $XM, $XK, $X million / billion
// ────────────────────────────────────────────────────────────────────

pub fn extract_money(s: &str) -> Option<String> {
    if let Some(dollar) = s.find('$') {
        let rest = &s[dollar..];
        let mut end = 1; // include $
        let bytes = rest.as_bytes();
        while end < bytes.len()
            && (bytes[end].is_ascii_digit()
                || bytes[end] == b'.'
                || bytes[end] == b','
                || bytes[end] == b'k'
                || bytes[end] == b'K'
                || bytes[end] == b'm'
                || bytes[end] == b'M'
                || bytes[end] == b'b'
                || bytes[end] == b'B')
        {
            end += 1;
        }
        if end > 1 {
            return Some(rest[..end].to_string());
        }
    }
    let lower = s.to_lowercase();
    for unit in ["million", "billion", "thousand"] {
        if let Some(idx) = lower.find(unit) {
            let head = lower[..idx].trim_end();
            // Walk back to find the number word/digits prefix.
            let num_start = head.rfind(|c: char| c.is_whitespace()).map(|i| i + 1).unwrap_or(0);
            let num = head[num_start..].trim();
            if !num.is_empty()
                && (num.chars().any(|c| c.is_ascii_digit()) || is_number_word(num))
            {
                return Some(format!("{} {}", num, unit));
            }
        }
    }
    None
}

fn is_number_word(w: &str) -> bool {
    matches!(
        w.to_lowercase().as_str(),
        "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine" | "ten"
            | "eleven" | "twelve" | "thirteen" | "fourteen" | "fifteen" | "sixteen"
            | "seventeen" | "eighteen" | "nineteen"
            | "twenty" | "thirty" | "forty" | "fifty" | "sixty" | "seventy" | "eighty" | "ninety"
            | "hundred"
    )
}

// ────────────────────────────────────────────────────────────────────
// Quantity extractor — "64GB", "16TB", "32MB"
// ────────────────────────────────────────────────────────────────────

const STORAGE_UNITS: &[&str] = &["TB", "GB", "MB", "KB", "tb", "gb", "mb", "kb"];

pub fn extract_quantity(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // Optional space.
            let after_digits = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            // Match unit
            for u in STORAGE_UNITS {
                if s[i..].starts_with(u) {
                    let end = i + u.len();
                    // Use the original (preserves "64GB" or "64 GB").
                    return Some(s[start..end].to_string());
                }
            }
            // No unit — back off and continue scanning.
            i = after_digits;
        } else {
            i += 1;
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Duration extractor — "6 weeks", "6-week", "3 months", "2 hours"
// ────────────────────────────────────────────────────────────────────

const DURATION_UNITS: &[&str] = &[
    "weeks", "week", "days", "day", "months", "month", "years", "year",
    "hours", "hour", "minutes", "minute", "seconds", "second",
];

pub fn extract_duration(s: &str) -> Option<String> {
    let lower = s.to_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            // separator: ' ' or '-'
            let sep_start = i;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'-') {
                i += 1;
            }
            if i > sep_start {
                for u in DURATION_UNITS {
                    if lower[i..].starts_with(u) {
                        let end = i + u.len();
                        // Re-form span with original case but normalize
                        // "6-week" → "6 weeks" so tokenizer matches refs.
                        let raw = &s[start..end];
                        return Some(normalize_duration(raw));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn normalize_duration(raw: &str) -> String {
    // "6-week" → "6 weeks"; "6 week" → "6 weeks"; "6 weeks" → "6 weeks".
    let lower = raw.to_lowercase();
    let with_space = lower.replace('-', " ");
    // pluralize unit if needed
    let parts: Vec<&str> = with_space.split_whitespace().collect();
    if parts.len() == 2 {
        let num = parts[0];
        let unit = parts[1];
        if !unit.ends_with('s') {
            // Only pluralize when num != 1
            let plural = if num == "1" {
                unit.to_string()
            } else {
                format!("{}s", unit)
            };
            return format!("{} {}", num, plural);
        }
    }
    with_space
}

// ────────────────────────────────────────────────────────────────────
// Decision / choice extractor.
// ────────────────────────────────────────────────────────────────────

pub fn extract_choice(candidate: &str, _query: &str) -> Option<String> {
    let lower = candidate.to_lowercase();

    // Markers that introduce a chosen option.
    // Order matters — most-specific multi-word patterns first.
    let markers: &[(&str, MarkerKind)] = &[
        ("going with ", MarkerKind::Choice),
        ("we're using ", MarkerKind::Choice),
        ("settled on ", MarkerKind::Choice),
        // More-specific "decided to <verb> on/at" patterns BEFORE the
        // generic "decided to <verb>" FirstWord fallback.
        ("decided to launch on ", MarkerKind::Choice),
        ("decided to ship on ", MarkerKind::Choice),
        ("decided to ship it ", MarkerKind::Choice),
        ("decided to use ", MarkerKind::Choice),
        ("decided to ", MarkerKind::FirstWord),
        ("ship it ", MarkerKind::Choice),
        ("launch on ", MarkerKind::Choice),
        ("committed to ", MarkerKind::Choice),
        ("picked ", MarkerKind::Choice),
        ("chose ", MarkerKind::Choice),
        ("using ", MarkerKind::Choice),
        ("ask to ", MarkerKind::Choice),
        ("set to ", MarkerKind::Choice),
        ("set the ", MarkerKind::ToOrComma),
        ("updated it to ", MarkerKind::Choice),
        ("updated to ", MarkerKind::Choice),
        ("bumped to ", MarkerKind::Choice),
        ("moved everything to ", MarkerKind::Choice),
        ("moved it to ", MarkerKind::Choice),
        ("moved to ", MarkerKind::Choice),
        ("now with ", MarkerKind::Choice),
        ("deal is now with ", MarkerKind::Choice),
        // Employment / location-of-work markers — useful for Choice
        // queries like "which company did X work at?".
        ("used to work at ", MarkerKind::Choice),
        ("worked at ", MarkerKind::Choice),
        ("works at ", MarkerKind::Choice),
        ("work at ", MarkerKind::Choice),
    ];

    for (marker, kind) in markers {
        if let Some(idx) = lower.find(marker) {
            let start = idx + marker.len();
            if start >= candidate.len() {
                continue;
            }
            let after = &candidate[start..];
            match kind {
                MarkerKind::Choice => {
                    let span = take_until_stop(after);
                    return Some(prefer_acronym(&span));
                }
                MarkerKind::FirstWord => {
                    let first = first_word(after);
                    if !first.is_empty() {
                        return Some(first);
                    }
                }
                MarkerKind::ToOrComma => {
                    // For "Set the seed round ask to $1M, not $3M.":
                    // skip to next " to " then take_until_stop.
                    if let Some(t) = after.to_lowercase().find(" to ") {
                        let s2 = t + 4;
                        if s2 < after.len() {
                            return Some(take_until_stop(&after[s2..]));
                        }
                    }
                }
            }
        }
    }
    None
}

/// Pick the head of a choice span.
///
/// Strategy:
///   1. Strip leading determiners ("the freemium GTM" → "freemium GTM",
///      "the MCP integration" → "MCP integration").
///   2. If the remaining tokens are ALL Title Case (likely a proper-noun
///      phrase like "Claude Code"), return the whole span unchanged —
///      these are usually single-entity answers.
///   3. Otherwise (mixed case or all-lowercase + acronym), return just
///      the first content token. So "freemium GTM" → "freemium",
///      "MCP integration" → "MCP", "annual billing only" → "annual".
fn prefer_acronym(span: &str) -> String {
    let stripped = strip_leading_determiners(span);
    let tokens: Vec<&str> = stripped.split_whitespace().collect();
    if tokens.len() <= 1 {
        return stripped;
    }
    let alnum = |t: &str| -> String { t.chars().filter(|c| c.is_alphanumeric()).collect() };
    let alnum_low = |t: &str| -> String {
        t.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
    };
    // Magnitude-pair carve-out: "twenty million", "fifteen million",
    // "10 million", "$1.5B" — keep number + magnitude together.
    if tokens.len() >= 2 {
        let first_key = alnum_low(tokens[0]);
        let second_key = alnum_low(tokens[1]);
        let first_is_num = is_number_word(&first_key) || first_key.chars().all(|c| c.is_ascii_digit());
        let is_magnitude = matches!(
            second_key.as_str(),
            "million" | "millions" | "billion" | "billions" | "thousand" | "thousands" | "hundred"
        );
        if first_is_num && is_magnitude {
            return format!("{} {}", alnum(tokens[0]), alnum(tokens[1]));
        }
    }
    // Prepositional / adverbial phrase carve-out: spans like
    // "on by default", "in production", "at scale" — return the whole
    // stripped span, not just the leading preposition.
    let first_key = alnum_low(tokens[0]);
    const LEADING_PREPS: &[&str] = &["on", "in", "at", "by", "off", "out", "into", "with"];
    if LEADING_PREPS.contains(&first_key.as_str()) {
        return stripped;
    }
    let all_title_case = tokens.iter().all(|t| {
        let s = alnum(t);
        !s.is_empty() && s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
    });
    if all_title_case {
        return stripped;
    }
    // Mixed-case: return the first non-filler token.
    for t in &tokens {
        let key = alnum_low(t);
        if !key.is_empty() && !FILLER_WORDS.contains(&key.as_str()) {
            return alnum(t);
        }
    }
    stripped
}

const FILLER_WORDS: &[&str] = &[
    "the", "a", "an", "my", "our", "your", "his", "her", "their",
    "this", "that", "these", "those",
];

fn strip_leading_determiners(span: &str) -> String {
    let lower = span.to_lowercase();
    for det in ["the ", "a ", "an ", "my ", "our ", "your ", "his ", "her ", "their "] {
        if lower.starts_with(det) {
            return span[det.len()..].to_string();
        }
    }
    span.to_string()
}

#[derive(Clone, Copy)]
enum MarkerKind {
    Choice,
    FirstWord,
    ToOrComma,
}

fn first_word(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

fn take_until_stop(s: &str) -> String {
    // Word-boundary stops (require surrounding space to avoid hitting
    // decimals like "$1.5M" or compounds like "BGE-small").
    let stops: &[&str] = &[
        " over ", " for ", " not ", " instead", " before ", " after ",
        " starting ", " given ", " to keep ", " because ", " — ", " - ",
        " from ", " with ", " at ", " in ",
        " as the ", " as our ", " as my ", " as primary ", " as a ",
        " only ", " only.", " only,",
        " now ", " now.", " now,",
        " first ", " first.", " first,",
        ". ", ", ", "; ",
    ];
    let lower = s.to_lowercase();
    let mut end = s.len();
    for stop in stops {
        if let Some(i) = lower.find(stop) {
            if i < end {
                end = i;
            }
        }
    }
    // Trim trailing terminator only if at very end (sentence period).
    s[..end]
        .trim()
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '—' | '-') && false || c.is_whitespace())
        .trim_end_matches(|c: char| matches!(c, '.' | ',' | ';'))
        .to_string()
}

// ────────────────────────────────────────────────────────────────────
// Place-pattern extractor — " from X", " on the X of the Y", " in X".
// ────────────────────────────────────────────────────────────────────

pub fn extract_place_pattern(candidate: &str) -> Option<String> {
    let lower = candidate.to_lowercase();
    // " came from X" / " from X" — prefer the longer prefix.
    let markers: &[&str] = &[
        " came from ",
        " from ",
        " on the ",
        " in the ",
        " at the ",
        " lives in ",
        " moved to ",
        " used to work at ",
        " works at ",
        " worked at ",
        " work at ",
        " based in ",
    ];
    for m in markers {
        if let Some(idx) = lower.find(m) {
            let start = idx + m.len();
            if start >= candidate.len() {
                continue;
            }
            let after = &candidate[start..];
            let span = take_until_stop(after);
            if span.is_empty() {
                continue;
            }
            // " in X" / " at X" must be a proper noun for Place. The
            // span must start with an uppercase letter to count.
            let needs_upper = matches!(
                *m,
                " from "
                    | " in "
                    | " at "
                    | " works at "
                    | " worked at "
                    | " work at "
                    | " used to work at "
                    | " based in "
            );
            if needs_upper
                && !span.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
            {
                continue;
            }
            return Some(span);
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// Generic value extractor — "X is Y", "X starts with Y", etc.
// ────────────────────────────────────────────────────────────────────

pub fn extract_value(_query: &str, candidate: &str) -> Option<String> {
    let lower = candidate.to_lowercase();
    let patterns: &[&str] = &[
        " starts with ",
        " helps with ",
        " keeps ",
        " keep ",
        " named ",
        " called ",
        " is at ",
        " is ",
        " are ",
        " was ",
        " were ",
    ];
    for p in patterns {
        if let Some(idx) = lower.find(p) {
            let start = idx + p.len();
            if start >= candidate.len() {
                continue;
            }
            let rest = candidate[start..]
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '$' && c != '-' && c != '_');
            if rest.is_empty() {
                continue;
            }
            // Strip leading "at " / "the " when " is " matched.
            let cleaned = rest
                .strip_prefix("at ")
                .or_else(|| rest.strip_prefix("the "))
                .unwrap_or(rest);
            return Some(cleaned.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_when_is_date() {
        assert_eq!(classify("When is the flight?"), QKind::Date);
        assert_eq!(classify("What date is the meeting?"), QKind::Date);
    }

    #[test]
    fn classify_who_is_person() {
        assert_eq!(classify("Who founded TraceMind?"), QKind::Person);
        assert_eq!(classify("With whom is Aaditya meeting?"), QKind::Person);
    }

    #[test]
    fn classify_which_is_choice() {
        assert_eq!(classify("Which school did I pick?"), QKind::Choice);
    }

    #[test]
    fn classify_how_long_is_duration() {
        assert_eq!(classify("How long are my sprint cycles?"), QKind::Duration);
    }

    #[test]
    fn classify_how_much_memory_is_quantity() {
        assert_eq!(
            classify("How much unified memory does the MacBook have?"),
            QKind::Quantity
        );
    }

    #[test]
    fn classify_yn_is_generic() {
        assert_eq!(classify("Did Alice ship the spec?"), QKind::Generic);
    }

    #[test]
    fn date_iso() {
        assert_eq!(
            extract_date("Flight is 2026-05-03 morning"),
            Some("2026-05-03".into())
        );
    }

    #[test]
    fn date_month_day() {
        assert_eq!(
            extract_date("My flight to Tokyo is on May 3rd"),
            Some("May 3rd".into())
        );
    }

    #[test]
    fn date_month_day_year() {
        assert_eq!(
            extract_date("Conference is October 15, 2026"),
            Some("October 15, 2026".into())
        );
    }

    #[test]
    fn date_weekday_with_qualifier_and_time_of_day() {
        assert_eq!(
            extract_date("Wait, the meeting moved to Wednesday morning."),
            Some("Wednesday morning".into())
        );
    }

    #[test]
    fn date_relative() {
        assert_eq!(extract_date("Ship the demo tomorrow"), Some("tomorrow".into()));
    }

    #[test]
    fn date_in_n_weeks() {
        assert_eq!(extract_date("Will publish in 3 weeks"), Some("in 3 weeks".into()));
    }

    #[test]
    fn date_in_n_months() {
        assert_eq!(
            extract_date("The fixed-rate period ends in 18 months."),
            Some("in 18 months".into())
        );
    }

    #[test]
    fn person_extracts_capitalized_cluster() {
        assert_eq!(
            extract_person("Aaditya works at TraceMind.", "Where does Aaditya work?"),
            Some("TraceMind".into())
        );
    }

    #[test]
    fn person_drops_leading_verb() {
        assert_eq!(
            extract_person(
                "Met Dr Patel at the conference yesterday.",
                "Who suggested I read the paper?"
            ),
            Some("Dr Patel".into())
        );
    }

    #[test]
    fn person_drops_leading_pronoun() {
        assert_eq!(
            extract_person(
                "My friend Lila moved to Brooklyn last year.",
                "Where does Lila live now?"
            ),
            Some("Brooklyn".into())
        );
    }

    #[test]
    fn person_drops_discourse_marker() {
        assert_eq!(
            extract_person(
                "Wait, it's Lyra Coffee, not Cafe Vega.",
                "Where's the coffee meeting?"
            ),
            Some("Lyra Coffee".into())
        );
    }

    #[test]
    fn topic_is_extracts_value_for_wh_focus() {
        // Regression: wh-focus topic_is must beat place_pattern when
        // both fire, so "What style ... practice?" returns "Ashtanga"
        // from "Her style is traditional Ashtanga." instead of "Mysore"
        // from "trained in Mysore." in the sibling sentence.
        assert_eq!(
            super::extract_topic_is(
                "What style does my yoga teacher Anna practice?",
                "Her style is traditional Ashtanga."
            ),
            Some("traditional Ashtanga".into())
        );
    }

    #[test]
    fn topic_is_skips_unrelated_is_patterns() {
        // "Which city does my friend live in?" — focus="city" — must
        // NOT match " friend is Karan" in the sibling sentence.
        assert_eq!(
            super::extract_topic_is(
                "Which city does my oldest friend live in?",
                "My oldest friend is Karan."
            ),
            None
        );
    }

    #[test]
    fn person_drops_contraction_im() {
        // Regression: mid-token apostrophe was leaving "I'm" stripped to
        // "I'm" (not "im"), so the PRONOUNS lookup missed it and the
        // extractor returned "I'm" as the answer.
        assert_eq!(
            extract_person(
                "The investor I'm pitching tomorrow is Maya Reyes.",
                "Who am I meeting for the pitch tomorrow?"
            ),
            Some("Maya Reyes".into())
        );
    }

    #[test]
    fn person_handles_sorry_no_actually() {
        assert_eq!(
            extract_person(
                "Sorry, no, actually it's Mike Chen, not Sarah.",
                "Who's the new VP of engineering?"
            ),
            Some("Mike Chen".into())
        );
    }

    #[test]
    fn number_extracts_first_int() {
        assert_eq!(extract_number("3 dogs and 7 cats"), Some("3".into()));
    }

    #[test]
    fn money_dollar_amount() {
        assert_eq!(extract_money("She raised $2.5M"), Some("$2.5M".into()));
    }

    #[test]
    fn money_million_words() {
        assert_eq!(
            extract_money("Raised 25 million in seed"),
            Some("25 million".into())
        );
    }

    #[test]
    fn money_two_million_words() {
        assert_eq!(
            extract_money("The seed round target is two million dollars."),
            Some("two million".into())
        );
    }

    #[test]
    fn quantity_gb() {
        assert_eq!(
            extract_quantity("It has 64GB of unified memory."),
            Some("64GB".into())
        );
    }

    #[test]
    fn quantity_gb_space() {
        assert_eq!(extract_quantity("16 GB RAM"), Some("16 GB".into()));
    }

    #[test]
    fn duration_dash_unit_normalises() {
        assert_eq!(
            extract_duration("Committed to a 6-week sprint cycle starting Monday."),
            Some("6 weeks".into())
        );
    }

    #[test]
    fn duration_space_unit() {
        assert_eq!(
            extract_duration("Sprints are 6 weeks long."),
            Some("6 weeks".into())
        );
    }

    #[test]
    fn choice_picked_over() {
        assert_eq!(
            extract_choice(
                "Picked HDBSCAN over KMeans for the clustering substrate.",
                "Which algorithm did I pick?"
            ),
            Some("HDBSCAN".into())
        );
    }

    #[test]
    fn choice_going_with() {
        assert_eq!(
            extract_choice(
                "Going with Stripe over Lago for billing infrastructure.",
                "Which billing platform?"
            ),
            Some("Stripe".into())
        );
    }

    #[test]
    fn choice_settled_on() {
        assert_eq!(
            extract_choice(
                "Settled on Tailwind for styling instead of CSS modules.",
                "What did I pick for styling?"
            ),
            Some("Tailwind".into())
        );
    }

    #[test]
    fn choice_decided_to_use() {
        assert_eq!(
            extract_choice(
                "Decided to use Rust for the core engine, not Go.",
                "What language?"
            ),
            Some("Rust".into())
        );
    }

    #[test]
    fn choice_decided_to_verb() {
        assert_eq!(
            extract_choice(
                "Decided to delete the Reason nav item and surface chains as verb cards.",
                "What did I decide about the Reason nav?"
            ),
            Some("delete".into())
        );
    }

    #[test]
    fn choice_set_to() {
        assert_eq!(
            extract_choice(
                "Set the seed round ask to $1M, not $3M.",
                "What is the seed round ask?"
            ),
            Some("$1M".into())
        );
    }

    #[test]
    fn choice_using_after_scratch() {
        assert_eq!(
            extract_choice(
                "Scratch that, we're using Lago instead.",
                "Which billing platform?"
            ),
            Some("Lago".into())
        );
    }

    #[test]
    fn value_is_pattern() {
        assert_eq!(
            extract_value("What is my favourite tea?", "My favourite tea is matcha."),
            Some("matcha".into())
        );
    }

    #[test]
    fn value_starts_with_pattern() {
        assert_eq!(
            extract_value(
                "What does my OpenRouter API key start with?",
                "My API key for OpenRouter starts with sk-or-v1."
            ),
            Some("sk-or-v1".into())
        );
    }

    #[test]
    fn value_is_at_strips_at() {
        assert_eq!(
            extract_value(
                "Where is the Tauri app binary?",
                "The Tauri app binary is at target/release/tracemind-app."
            ),
            Some("target/release/tracemind-app".into())
        );
    }

    #[test]
    fn compose_when_with_date() {
        assert_eq!(
            compose("When is my flight?", "Flight is May 3rd."),
            "May 3rd".to_string()
        );
    }

    #[test]
    fn compose_who_with_person() {
        assert_eq!(
            compose("Who founded TraceMind?", "Aaditya is the founder of TraceMind."),
            "Aaditya".to_string()
        );
    }

    #[test]
    fn compose_choice_with_choice() {
        assert_eq!(
            compose(
                "Which algorithm did I pick?",
                "Picked HDBSCAN over KMeans for the clustering substrate."
            ),
            "HDBSCAN".to_string()
        );
    }

    #[test]
    fn compose_quantity_with_storage() {
        assert_eq!(
            compose("How much memory does my laptop have?", "It has 64GB of unified memory."),
            "64GB".to_string()
        );
    }

    #[test]
    fn compose_duration_normalizes() {
        assert_eq!(
            compose("How long are my sprints?", "Committed to a 6-week sprint cycle starting Monday."),
            "6 weeks".to_string()
        );
    }

    #[test]
    fn compose_generic_value() {
        assert_eq!(
            compose("What is my favourite tea?", "My favourite tea is matcha."),
            "matcha".to_string()
        );
    }
}

