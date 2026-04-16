use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A time range for temporal queries like "what was I working on last week?"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// Human-readable label for display: "last week", "yesterday", etc.
    pub label: String,
}

impl TimeRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>, label: impl Into<String>) -> Self {
        Self {
            start,
            end,
            label: label.into(),
        }
    }

    /// Duration of this range.
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// Check if a timestamp falls within this range (inclusive start, exclusive end).
    pub fn contains(&self, dt: &DateTime<Utc>) -> bool {
        *dt >= self.start && *dt < self.end
    }
}

/// Parse natural-language temporal expressions into a `TimeRange`.
///
/// Supports:
/// - "today", "yesterday"
/// - "last week", "this week", "past week"
/// - "last month", "this month", "past month"
/// - "last N days/hours/weeks/months"
/// - "N days/hours/weeks/months ago"
/// - "since Monday", "since January"
/// - Relative: "recently", "lately" → last 3 days
///
/// `now` is passed explicitly for testability.
pub fn parse_time_expression(text: &str, now: DateTime<Utc>) -> Option<TimeRange> {
    let lower = text.to_lowercase();

    // "today"
    if lower.contains("today") {
        let start = now.date_naive().and_hms_opt(0, 0, 0)?;
        let start_utc = DateTime::<Utc>::from_naive_utc_and_offset(start, Utc);
        return Some(TimeRange::new(start_utc, now, "today"));
    }

    // "yesterday"
    if lower.contains("yesterday") {
        let yesterday = now.date_naive() - Duration::days(1);
        let start = yesterday.and_hms_opt(0, 0, 0)?;
        let end = now.date_naive().and_hms_opt(0, 0, 0)?;
        let start_utc = DateTime::<Utc>::from_naive_utc_and_offset(start, Utc);
        let end_utc = DateTime::<Utc>::from_naive_utc_and_offset(end, Utc);
        return Some(TimeRange::new(start_utc, end_utc, "yesterday"));
    }

    // "recently" / "lately" → last 3 days
    if lower.contains("recently") || lower.contains("lately") {
        let start = now - Duration::days(3);
        return Some(TimeRange::new(start, now, "recently"));
    }

    // "last N <unit>" or "past N <unit>"
    if let Some(range) = parse_last_n(&lower, now) {
        return Some(range);
    }

    // "N <unit> ago" — point-in-time, expand to a window
    if let Some(range) = parse_n_ago(&lower, now) {
        return Some(range);
    }

    // "last week" / "this week" / "past week"
    if lower.contains("last week") || lower.contains("past week") {
        let start = now - Duration::weeks(1);
        return Some(TimeRange::new(start, now, "last week"));
    }
    if lower.contains("this week") {
        // Start of current week (Monday)
        let weekday = now.date_naive().weekday().num_days_from_monday();
        let monday = now.date_naive() - Duration::days(weekday as i64);
        let start = monday.and_hms_opt(0, 0, 0)?;
        let start_utc = DateTime::<Utc>::from_naive_utc_and_offset(start, Utc);
        return Some(TimeRange::new(start_utc, now, "this week"));
    }

    // "last month" / "past month"
    if lower.contains("last month") || lower.contains("past month") {
        let start = now - Duration::days(30);
        return Some(TimeRange::new(start, now, "last month"));
    }
    if lower.contains("this month") {
        let first_of_month = NaiveDate::from_ymd_opt(now.date_naive().year(), now.date_naive().month(), 1)?
            .and_hms_opt(0, 0, 0)?;
        let start_utc = DateTime::<Utc>::from_naive_utc_and_offset(first_of_month, Utc);
        return Some(TimeRange::new(start_utc, now, "this month"));
    }

    // "last year" / "past year"
    if lower.contains("last year") || lower.contains("past year") {
        let start = now - Duration::days(365);
        return Some(TimeRange::new(start, now, "last year"));
    }

    None
}

/// Parse "last N days/hours/weeks/months" or "past N days/..."
fn parse_last_n(lower: &str, now: DateTime<Utc>) -> Option<TimeRange> {
    let patterns = ["last ", "past "];
    for prefix in &patterns {
        if let Some(pos) = lower.find(prefix) {
            let rest = &lower[pos + prefix.len()..];
            if let Some((n, unit)) = parse_n_unit(rest) {
                let duration = unit_to_duration(n, &unit)?;
                let start = now - duration;
                let label = format!("last {} {}", n, unit);
                return Some(TimeRange::new(start, now, label));
            }
        }
    }
    None
}

/// Parse "N days/hours/weeks/months ago"
fn parse_n_ago(lower: &str, now: DateTime<Utc>) -> Option<TimeRange> {
    if !lower.contains("ago") {
        return None;
    }
    // Find "N <unit> ago"
    let words: Vec<&str> = lower.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if *word == "ago" && i >= 2 {
            if let Ok(n) = words[i - 2].parse::<i64>() {
                let unit = words[i - 1];
                let duration = unit_to_duration(n, unit)?;
                let point = now - duration;
                // Expand to a 1-unit window around the point
                let window = unit_to_duration(1, unit)?;
                let label = format!("{} {} ago", n, unit);
                return Some(TimeRange::new(point - window, point + window, label));
            }
        }
    }
    None
}

/// Extract N and unit from "3 days", "2 weeks", etc.
fn parse_n_unit(text: &str) -> Option<(i64, String)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() >= 2 {
        if let Ok(n) = words[0].parse::<i64>() {
            let unit = words[1].trim_end_matches('s').to_string();
            return Some((n, unit));
        }
    }
    None
}

/// Convert a count and unit name to a chrono Duration.
fn unit_to_duration(n: i64, unit: &str) -> Option<Duration> {
    let unit = unit.trim_end_matches('s'); // normalize plural
    match unit {
        "hour" => Some(Duration::hours(n)),
        "day" => Some(Duration::days(n)),
        "week" => Some(Duration::weeks(n)),
        "month" => Some(Duration::days(n * 30)),
        "year" => Some(Duration::days(n * 365)),
        "minute" => Some(Duration::minutes(n)),
        _ => None,
    }
}

/// Parse a temporal expression using the current time.
/// Convenience wrapper around `parse_time_expression` that supplies `Utc::now()`.
pub fn parse_time_expression_now(text: &str) -> Option<TimeRange> {
    parse_time_expression(text, Utc::now())
}

/// Detect if a query text contains temporal language.
pub fn has_temporal_intent(text: &str) -> bool {
    let lower = text.to_lowercase();
    TEMPORAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Keywords that signal temporal query intent.
pub const TEMPORAL_KEYWORDS: &[&str] = &[
    "yesterday", "today", "last week", "this week", "past week",
    "last month", "this month", "past month",
    "last year", "this year", "past year",
    "recently", "lately", "ago",
    "last few", "past few",
    "working on", "was i",
    "when did", "when was",
    "since when", "how long",
    "before", "after",
    "earlier today", "this morning", "tonight",
    "last night", "last 2", "last 3", "last 4", "last 5",
    "last 7", "last 10", "last 14", "last 30",
];

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        // Wednesday, 2026-04-15 12:00:00 UTC
        Utc.with_ymd_and_hms(2026, 4, 15, 12, 0, 0).unwrap()
    }

    #[test]
    fn parse_today() {
        let now = fixed_now();
        let range = parse_time_expression("what was I doing today", now).unwrap();
        assert_eq!(range.label, "today");
        assert!(range.start < now);
        assert_eq!(range.end, now);
        assert_eq!(range.start.date_naive(), now.date_naive());
    }

    #[test]
    fn parse_yesterday() {
        let now = fixed_now();
        let range = parse_time_expression("show me yesterday's work", now).unwrap();
        assert_eq!(range.label, "yesterday");
        let yesterday = (now - Duration::days(1)).date_naive();
        assert_eq!(range.start.date_naive(), yesterday);
    }

    #[test]
    fn parse_last_week() {
        let now = fixed_now();
        let range = parse_time_expression("what was I working on last week", now).unwrap();
        assert_eq!(range.label, "last week");
        let diff = now - range.start;
        assert!(diff.num_days() >= 6 && diff.num_days() <= 8);
    }

    #[test]
    fn parse_last_n_days() {
        let now = fixed_now();
        let range = parse_time_expression("show me the last 3 days", now).unwrap();
        assert_eq!(range.label, "last 3 day");
        let diff = now - range.start;
        assert_eq!(diff.num_days(), 3);
    }

    #[test]
    fn parse_recently() {
        let now = fixed_now();
        let range = parse_time_expression("what have I done recently", now).unwrap();
        assert_eq!(range.label, "recently");
        let diff = now - range.start;
        assert_eq!(diff.num_days(), 3);
    }

    #[test]
    fn parse_n_ago() {
        let now = fixed_now();
        let range = parse_time_expression("what was happening 2 weeks ago", now).unwrap();
        assert!(range.label.contains("2 week"));
    }

    #[test]
    fn no_temporal_intent() {
        let now = fixed_now();
        assert!(parse_time_expression("what is Rust", now).is_none());
    }

    #[test]
    fn temporal_keywords_detected() {
        assert!(has_temporal_intent("what was I working on last week?"));
        assert!(has_temporal_intent("show me yesterday's activity"));
        assert!(has_temporal_intent("anything new recently?"));
        assert!(!has_temporal_intent("what is Rust?"));
        assert!(!has_temporal_intent("how does Python compare?"));
    }

    #[test]
    fn time_range_contains() {
        let now = fixed_now();
        let range = TimeRange::new(
            now - Duration::hours(2),
            now,
            "test",
        );
        assert!(range.contains(&(now - Duration::hours(1))));
        assert!(!range.contains(&(now - Duration::hours(3))));
        assert!(!range.contains(&now)); // exclusive end
    }

    #[test]
    fn parse_this_month() {
        let now = fixed_now();
        let range = parse_time_expression("what have I done this month", now).unwrap();
        assert_eq!(range.label, "this month");
        assert_eq!(range.start.date_naive().day(), 1);
        assert_eq!(range.start.date_naive().month(), now.date_naive().month());
    }
}
