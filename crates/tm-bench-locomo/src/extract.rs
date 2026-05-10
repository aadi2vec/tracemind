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
    /// No specialized extractor; return turn as-is.
    Generic,
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

    // Inline cues. "What was X's finish time?" — anchor on "time" with
    // a temporal modifier (finish/goal/race/elapsed/split).
    if lower.contains("finish time")
        || lower.contains("goal time")
        || lower.contains("race time")
        || lower.contains("split time")
        || lower.contains("elapsed time")
    {
        return QKind::Time;
    }

    QKind::Generic
}

/// Compose a tight prediction string for the question, given the best
/// candidate turn the retriever could find. Falls back to the turn
/// itself if no extractor produces a confident span.
pub fn compose_short_answer(question: &str, turn: &str) -> String {
    match classify_question(question) {
        QKind::YesNo => yes_no_answer(question, turn),
        QKind::Date => extract_date(turn).unwrap_or_else(|| turn.to_string()),
        QKind::Money => extract_money(turn).unwrap_or_else(|| turn.to_string()),
        QKind::Time => extract_time(turn).unwrap_or_else(|| turn.to_string()),
        QKind::Generic => turn.to_string(),
    }
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
    // Fallback: if the turn carries a money/date/time we'd normally
    // extract, surface it with a "No," prefix.
    if let Some(m) = extract_money(turn) {
        return format!("No, {m}");
    }
    if let Some(d) = extract_date(turn) {
        return format!("No, {d}");
    }
    if let Some(t) = extract_time(turn) {
        return format!("No, {t}");
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
