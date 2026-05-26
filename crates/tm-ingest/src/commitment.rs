//! CTX-EVG-C — heuristic commitment detector.
//!
//! Runs in the ingest hot path, before LLM detection. Conservative on
//! purpose: false positives = noisy ledger = product death. Detection
//! confidence is exposed so the Tauri / CLI layer can decide whether to
//! auto-create a `Commitment` event node or prompt the user.
//!
//! Detection covers four pattern families:
//!
//! 1. First-person futures: "I'll …", "I will …", "I'm going to …",
//!    "I'm gonna …", "let me …"
//! 2. Imperative-to-self: "remind me to …", "todo: …", "todo - …"
//! 3. Promise verbs: "promise to …", "commit to …"
//! 4. Explicit due hints: "by Friday", "by 2026-06-01", "before tomorrow",
//!    "in 2 days", "this week"
//!
//! No NLP libs — just lowercase string matching + a tiny date parser.
//! Anything more sophisticated is Tier-1 LLM territory and out of scope
//! for the hot path.

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Utc, Weekday};

#[derive(Debug, Clone, PartialEq)]
pub struct CommitmentCandidate {
    /// The original text the candidate was detected in.
    pub text: String,
    /// Confidence in [0.0, 1.0]. ≥0.85 → auto-create without prompt;
    /// [0.5, 0.85) → propose; <0.5 → ignore.
    pub confidence: f32,
    /// Resolved due timestamp, millis since unix epoch (UTC). `None`
    /// means no explicit deadline was detected.
    pub due_at: Option<i64>,
    /// The matched pattern, for telemetry / debugging.
    pub matched_pattern: &'static str,
}

/// Heuristic entry point. Returns `None` when the text doesn't look
/// like a commitment at all.
pub fn detect_commitment(text: &str) -> Option<CommitmentCandidate> {
    detect_commitment_with_now(text, Utc::now().timestamp_millis())
}

/// Same as [`detect_commitment`] but takes the current time so tests can
/// pin determinism.
pub fn detect_commitment_with_now(text: &str, now_ms: i64) -> Option<CommitmentCandidate> {
    let lower = text.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }

    let (confidence, pattern) = score_intent(&lower)?;
    let due_at = parse_due_hint(&lower, now_ms);

    // Having an explicit deadline lifts confidence — these are unambiguous.
    let confidence = if due_at.is_some() {
        (confidence + 0.1).min(0.95)
    } else {
        confidence
    };

    Some(CommitmentCandidate {
        text: text.to_string(),
        confidence,
        due_at,
        matched_pattern: pattern,
    })
}

/// Score the *intent* portion of the text. Returns `(confidence,
/// pattern_label)` for the strongest match, or `None` if nothing fired.
fn score_intent(lower: &str) -> Option<(f32, &'static str)> {
    // Strong patterns — start-of-utterance, low false-positive rate.
    const STRONG_PREFIXES: &[(&str, &str)] = &[
        ("i'll ", "first_person_future_contracted"),
        ("i will ", "first_person_future"),
        ("i'm going to ", "first_person_going_to"),
        ("i am going to ", "first_person_going_to"),
        ("i'm gonna ", "first_person_gonna"),
        ("let me ", "let_me"),
        ("remind me to ", "remind_me"),
        ("todo: ", "todo_colon"),
        ("todo - ", "todo_dash"),
        ("todo ", "todo_bare"),
        ("i promise to ", "promise"),
        ("i commit to ", "commit"),
    ];
    for (prefix, label) in STRONG_PREFIXES {
        if lower.starts_with(prefix) {
            return Some((0.8, label));
        }
    }

    // Weaker mid-sentence variants. These pick up things like
    // "Aaditya, I'll ship this on Friday" without exploding on quoted
    // text or third-person reportage.
    const MEDIUM_PATTERNS: &[(&str, &str)] = &[
        (" i'll ", "first_person_future_contracted_mid"),
        (" i will ", "first_person_future_mid"),
        (" i'm going to ", "first_person_going_to_mid"),
    ];
    for (needle, label) in MEDIUM_PATTERNS {
        if lower.contains(needle) {
            return Some((0.6, label));
        }
    }

    None
}

/// Parse an explicit due hint, if any. Conservative — recognised hints:
///   - "tomorrow"
///   - "today"
///   - "tonight"
///   - "by <weekday>"
///   - "next <weekday>"
///   - "this week" / "by end of week" / "eow"
///   - "by <YYYY-MM-DD>"
///   - "in N day(s)" / "in N week(s)"
pub fn parse_due_hint(lower: &str, now_ms: i64) -> Option<i64> {
    let now = Utc.timestamp_millis_opt(now_ms).single()?;
    let today_local = Local
        .from_utc_datetime(&now.naive_utc())
        .date_naive();

    // Explicit ISO date: "by 2026-06-01" or "on 2026-06-01"
    for needle in ["by ", "on ", "before "] {
        if let Some(start) = lower.find(needle) {
            let tail = &lower[start + needle.len()..];
            if let Some(d) = parse_iso_date_prefix(tail) {
                return Some(end_of_day_millis(d));
            }
        }
    }

    if lower.contains("tonight") {
        return Some(end_of_day_millis(today_local));
    }
    if lower.contains("today") {
        return Some(end_of_day_millis(today_local));
    }
    if lower.contains("tomorrow") {
        return Some(end_of_day_millis(today_local + Duration::days(1)));
    }
    if lower.contains("this week") || lower.contains("eow") || lower.contains("end of week") {
        let days_to_friday = days_until(today_local.weekday(), Weekday::Fri);
        return Some(end_of_day_millis(today_local + Duration::days(days_to_friday)));
    }

    // "in N day(s) / week(s)"
    if let Some(pos) = lower.find("in ") {
        let tail = &lower[pos + 3..];
        if let Some((n, unit_pos)) = parse_leading_uint(tail) {
            let unit = tail[unit_pos..].trim_start();
            if unit.starts_with("day") {
                return Some(end_of_day_millis(today_local + Duration::days(n as i64)));
            }
            if unit.starts_with("week") {
                return Some(end_of_day_millis(today_local + Duration::weeks(n as i64)));
            }
        }
    }

    // "by <weekday>" / "next <weekday>"
    for (needle, allow_same_week) in [("by ", true), ("next ", false), ("this ", true)] {
        if let Some(pos) = lower.find(needle) {
            let tail = &lower[pos + needle.len()..];
            if let Some(wd) = parse_weekday(tail) {
                let mut days = days_until(today_local.weekday(), wd);
                if !allow_same_week && days < 7 {
                    days += 7;
                }
                return Some(end_of_day_millis(today_local + Duration::days(days)));
            }
        }
    }

    None
}

fn end_of_day_millis(d: NaiveDate) -> i64 {
    // 23:59:59.999 local → UTC millis
    let ndt = d.and_hms_opt(23, 59, 59).expect("valid hms");
    Local
        .from_local_datetime(&ndt)
        .single()
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

fn days_until(from: Weekday, to: Weekday) -> i64 {
    let f = from.num_days_from_monday() as i64;
    let t = to.num_days_from_monday() as i64;
    let d = (t - f).rem_euclid(7);
    if d == 0 {
        7
    } else {
        d
    }
}

fn parse_iso_date_prefix(s: &str) -> Option<NaiveDate> {
    // Take the leading "YYYY-MM-DD"
    let bytes = s.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let candidate = &s[..10];
    NaiveDate::parse_from_str(candidate, "%Y-%m-%d").ok()
}

fn parse_leading_uint(s: &str) -> Option<(u32, usize)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let n: u32 = s[..i].parse().ok()?;
    // skip whitespace before unit
    let mut j = i;
    while j < bytes.len() && bytes[j] == b' ' {
        j += 1;
    }
    Some((n, j))
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    let s = s.trim_start();
    let map: &[(&str, Weekday)] = &[
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
        ("mon", Weekday::Mon),
        ("tue", Weekday::Tue),
        ("wed", Weekday::Wed),
        ("thu", Weekday::Thu),
        ("fri", Weekday::Fri),
        ("sat", Weekday::Sat),
        ("sun", Weekday::Sun),
    ];
    for (name, wd) in map {
        if s.starts_with(name) {
            return Some(*wd);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_fixed() -> i64 {
        // Pin to 2026-05-25 Monday for determinism.
        let d = NaiveDate::from_ymd_opt(2026, 5, 25).unwrap();
        let ndt = d.and_hms_opt(12, 0, 0).unwrap();
        Utc.from_utc_datetime(&ndt).timestamp_millis()
    }

    #[test]
    fn detects_first_person_future() {
        let c = detect_commitment_with_now("I'll review the PR tomorrow", now_fixed())
            .expect("should detect");
        assert!(c.confidence >= 0.85, "due_at lift expected: {}", c.confidence);
        assert!(c.due_at.is_some(), "tomorrow should parse");
        assert_eq!(c.matched_pattern, "first_person_future_contracted");
    }

    #[test]
    fn detects_todo_prefix() {
        let c = detect_commitment_with_now("TODO: ship CTX-EVG-C", now_fixed())
            .expect("should detect");
        assert_eq!(c.matched_pattern, "todo_colon");
        assert!(c.due_at.is_none());
    }

    #[test]
    fn detects_remind_me() {
        let c = detect_commitment_with_now("Remind me to email Sarah", now_fixed())
            .expect("should detect");
        assert_eq!(c.matched_pattern, "remind_me");
    }

    #[test]
    fn parses_iso_date_due() {
        let c =
            detect_commitment_with_now("I'll ship the demo by 2026-06-01", now_fixed())
                .expect("should detect");
        assert!(c.due_at.is_some());
    }

    #[test]
    fn parses_in_n_days() {
        let c = detect_commitment_with_now("I will submit in 3 days", now_fixed())
            .expect("should detect");
        assert!(c.due_at.is_some());
    }

    #[test]
    fn parses_by_weekday() {
        let c = detect_commitment_with_now("I'll have it ready by Friday", now_fixed())
            .expect("should detect");
        assert!(c.due_at.is_some());
    }

    #[test]
    fn rejects_questions_and_third_person() {
        // No first-person future / todo / remind-me here.
        assert!(detect_commitment_with_now("Will Aaditya ship it?", now_fixed()).is_none());
        assert!(detect_commitment_with_now("Sarah said she'll handle it", now_fixed())
            .is_none());
        assert!(detect_commitment_with_now("", now_fixed()).is_none());
    }
}
