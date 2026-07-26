//! Three-mode ingestion — Ambient / Focus / Private (I-P2 §3.1).
//!
//! The mode is a user-visible switch that changes what capture does. It is
//! not a governance gate: gates still run on every write. Modes are the
//! **intent** attached to captures so retrieval can honour purpose limitation
//! (S8 in the plan).
//!
//! * **Ambient** — the default; ambient watchers run, everything lands in
//!   quarantine tier.
//! * **Focus** — user-declared intent; capture rate goes up, every capture
//!   gets tagged with the focus name, promotes to Hot at session end.
//! * **Private** — all watchers pause; menu-bar goes red; manual capture
//!   still allowed but its timestamps do not feed the behavioural model.
//!
//! This module owns the pure data shape. Persistence lives in
//! `tm_ingest::modes::ModeManager`.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which of the three ingestion modes is active right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Ambient,
    Focus,
    Private,
}

impl Default for CaptureMode {
    fn default() -> Self {
        CaptureMode::Ambient
    }
}

impl CaptureMode {
    /// Wire-facing string; kept out of `Display` so callers cannot depend on
    /// the debug representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureMode::Ambient => "ambient",
            CaptureMode::Focus => "focus",
            CaptureMode::Private => "private",
        }
    }

    /// Parse the wire-facing string. Returns `None` for unknown values so
    /// callers can decide whether to fall back to Ambient or reject the
    /// request.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ambient" => Some(CaptureMode::Ambient),
            "focus" => Some(CaptureMode::Focus),
            "private" => Some(CaptureMode::Private),
            _ => None,
        }
    }

    /// True when this mode should *pause* ambient watchers.
    pub fn pauses_watchers(&self) -> bool {
        matches!(self, CaptureMode::Private)
    }

    /// True when this mode should tag every capture with an intent name.
    pub fn tags_intent(&self) -> bool {
        matches!(self, CaptureMode::Focus)
    }
}

/// A named focus (or private) session with an explicit deadline.
///
/// Sessions have a `started_at`, a `duration`, and — once ended — an
/// `ended_at`. Storing both lets us reconstruct the session boundaries even
/// if the daemon crashes mid-run and only the persisted session is available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModeSession {
    pub id: Uuid,
    pub mode: CaptureMode,
    /// Human-readable name (Focus mode: the intent; Private: usually blank).
    pub name: String,
    pub started_at: DateTime<Utc>,
    /// Duration in seconds. Sessions must always have a bounded duration so
    /// they cannot silently outlive the user's attention (S9).
    pub duration_secs: i64,
    /// Set once the session ends (either at its deadline or by user action).
    pub ended_at: Option<DateTime<Utc>>,
}

impl ModeSession {
    /// Start a new session `now` with the requested duration.
    pub fn start(mode: CaptureMode, name: impl Into<String>, duration_secs: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            mode,
            name: name.into(),
            started_at: Utc::now(),
            duration_secs,
            ended_at: None,
        }
    }

    /// The wall-clock instant this session expires.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.started_at + Duration::seconds(self.duration_secs)
    }

    /// True when the session has already been ended or the deadline has
    /// passed (S9 — time-boxed captures default off).
    pub fn is_expired(&self, at: DateTime<Utc>) -> bool {
        if self.ended_at.is_some() {
            return true;
        }
        at >= self.expires_at()
    }

    /// End the session at the given time. Idempotent.
    pub fn end(&mut self, at: DateTime<Utc>) {
        if self.ended_at.is_none() {
            self.ended_at = Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_ambient() {
        assert_eq!(CaptureMode::default(), CaptureMode::Ambient);
    }

    #[test]
    fn from_str_roundtrips_all_modes() {
        for m in [CaptureMode::Ambient, CaptureMode::Focus, CaptureMode::Private] {
            let parsed = CaptureMode::from_str(m.as_str()).unwrap();
            assert_eq!(parsed, m);
        }
        assert_eq!(CaptureMode::from_str("AMBIENT"), Some(CaptureMode::Ambient));
        assert_eq!(CaptureMode::from_str("  focus "), Some(CaptureMode::Focus));
        assert!(CaptureMode::from_str("nope").is_none());
    }

    #[test]
    fn pauses_watchers_only_in_private() {
        assert!(!CaptureMode::Ambient.pauses_watchers());
        assert!(!CaptureMode::Focus.pauses_watchers());
        assert!(CaptureMode::Private.pauses_watchers());
    }

    #[test]
    fn tags_intent_only_in_focus() {
        assert!(!CaptureMode::Ambient.tags_intent());
        assert!(CaptureMode::Focus.tags_intent());
        assert!(!CaptureMode::Private.tags_intent());
    }

    #[test]
    fn session_expires_after_deadline() {
        let mut s = ModeSession::start(CaptureMode::Focus, "prep", 60);
        assert!(!s.is_expired(s.started_at));
        assert!(!s.is_expired(s.started_at + Duration::seconds(30)));
        assert!(s.is_expired(s.started_at + Duration::seconds(60)));
        assert!(s.is_expired(s.started_at + Duration::seconds(120)));

        s.end(s.started_at + Duration::seconds(15));
        // Once ended, is_expired returns true from that point.
        assert!(s.is_expired(s.started_at + Duration::seconds(15)));
    }

    #[test]
    fn end_is_idempotent() {
        let mut s = ModeSession::start(CaptureMode::Private, "quiet", 300);
        let first = s.started_at + Duration::seconds(10);
        let second = s.started_at + Duration::seconds(20);
        s.end(first);
        s.end(second);
        assert_eq!(s.ended_at, Some(first));
    }

    #[test]
    fn json_round_trip() {
        let s = ModeSession::start(CaptureMode::Focus, "call-prep", 45 * 60);
        let encoded = serde_json::to_string(&s).unwrap();
        let decoded: ModeSession = serde_json::from_str(&encoded).unwrap();
        assert_eq!(s, decoded);
    }
}
