use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single capture feedback event, written to `~/.tracemind/recent.jsonl`
/// by the capture/ingest layers so that the UI (CLI/Tauri tray) can surface
/// "something just happened" to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentCapture {
    /// When the capture fired (client clock).
    pub timestamp: DateTime<Utc>,
    /// Where the capture came from: "clipboard", "history", "mcp", etc.
    pub source: String,
    /// Tier assigned by the priority gate (t1/t2/t3) if known.
    pub tier: Option<String>,
    /// Hash of normalized content — useful for deduping views.
    pub content_hash: String,
    /// If the capture was skipped, why (dedup-hit, gate-rejected, empty, etc.).
    /// `None` means the capture was accepted end-to-end.
    pub skipped_reason: Option<String>,
    /// Whether the capture reached the promotion / entity-extract path
    /// (true for accepted Tier 1/2 ingests, false for dedup-hits / Tier 3).
    pub promoted: bool,
    /// Short, human-readable snippet of the captured text (truncated).
    pub text_preview: String,
}

impl RecentCapture {
    /// Max preview length, in chars, when building from raw text.
    pub const PREVIEW_CHARS: usize = 120;

    pub fn new(
        source: impl Into<String>,
        content_hash: impl Into<String>,
        text: &str,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            source: source.into(),
            tier: None,
            content_hash: content_hash.into(),
            skipped_reason: None,
            promoted: false,
            text_preview: preview(text),
        }
    }

    pub fn with_tier(mut self, tier: impl Into<String>) -> Self {
        self.tier = Some(tier.into());
        self
    }

    pub fn with_skipped(mut self, reason: impl Into<String>) -> Self {
        self.skipped_reason = Some(reason.into());
        self
    }

    pub fn with_promoted(mut self, promoted: bool) -> Self {
        self.promoted = promoted;
        self
    }
}

fn preview(text: &str) -> String {
    let collapsed: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.chars().count() <= RecentCapture::PREVIEW_CHARS {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(RecentCapture::PREVIEW_CHARS).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_and_strips_control_chars() {
        let long = "a".repeat(500);
        let rc = RecentCapture::new("clipboard", "hash", &long);
        assert!(rc.text_preview.chars().count() <= RecentCapture::PREVIEW_CHARS + 1);
        assert!(rc.text_preview.ends_with('…'));

        let with_ctrl = "hello\nworld\r\ttest";
        let rc = RecentCapture::new("clipboard", "hash", with_ctrl);
        assert_eq!(rc.text_preview, "hello world  test");
    }

    #[test]
    fn builder_sets_fields() {
        let rc = RecentCapture::new("mcp", "h", "text")
            .with_tier("t1")
            .with_promoted(true);
        assert_eq!(rc.tier.as_deref(), Some("t1"));
        assert!(rc.promoted);
        assert!(rc.skipped_reason.is_none());

        let rc = RecentCapture::new("clipboard", "h", "text").with_skipped("dedup_hit");
        assert_eq!(rc.skipped_reason.as_deref(), Some("dedup_hit"));
    }
}
