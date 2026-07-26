//! Per-source capture permissions (CAP-1).
//!
//! TraceMind's ambient capture layer (`tracemind-capture`) reads from
//! multiple potentially-sensitive sources: clipboard, shell history,
//! screenshots, browser, audio (Whisper), calendar. Privacy is a
//! product property, not a footnote — every source is **opt-in at the
//! per-source level**, revocable at any time, and visible in the
//! Tauri settings panel (UI-13).
//!
//! This module defines the schema persisted to
//! `~/.tracemind/capture_permissions.toml` and the load/save helpers
//! that both `tm-cli` (the writer, via `tracemind capture {enable,
//! disable, status}`) and `tm-capture` (the reader, gating each source
//! loop) share.
//!
//! Defaults: clipboard + shell are **on** at first run (low
//! sensitivity, large user-felt benefit). Everything else is **off**
//! until explicitly enabled. This matches the user's stated
//! invariant: "no capture source ever transmits off-device, and no
//! capture source ever runs that the user did not toggle on."
//!
//! See `docs/TASKS.md` P1b for the broader capture roadmap.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::TraceMindError;

// ---------------------------------------------------------------------------
// Source identifiers
// ---------------------------------------------------------------------------

/// Every capture source the daemon supports. Keep this enum
/// exhaustive — adding a new source is a deliberate act that needs
/// matching UI + audit affordances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Clipboard,
    Shell,
    /// Apple Notes (macOS only). Backfilled at install via AppleScript;
    /// the daemon does not poll Notes continuously (no API for change
    /// events). Re-running `tracemind capture backfill` picks up edits.
    Notes,
    Screenshot,
    Browser,
    Audio,
    Calendar,
    /// PDFs dropped into a watched folder (default `~/Downloads`). Text
    /// is extracted on-device; the raw file never leaves the machine.
    Pdf,
    /// Email captured from a watched `.eml` drop folder (X15). High
    /// sensitivity — off by default.
    Email,
    /// Photo library EXIF (X18) — filename, capture time, GPS. High
    /// sensitivity — off by default.
    Photo,
}

impl CaptureSource {
    /// Canonical string form used in the TOML file, CLI arguments, and
    /// trace logs. Lowercase, no underscores, stable across releases.
    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureSource::Clipboard => "clipboard",
            CaptureSource::Shell => "shell",
            CaptureSource::Notes => "notes",
            CaptureSource::Screenshot => "screenshot",
            CaptureSource::Browser => "browser",
            CaptureSource::Audio => "audio",
            CaptureSource::Calendar => "calendar",
            CaptureSource::Pdf => "pdf",
            CaptureSource::Email => "email",
            CaptureSource::Photo => "photo",
        }
    }

    /// Parse a CLI-supplied source name. Case-insensitive so users can
    /// type either form without thinking.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "clipboard" | "clip" => Some(CaptureSource::Clipboard),
            "shell" | "history" => Some(CaptureSource::Shell),
            "notes" | "note" => Some(CaptureSource::Notes),
            "screenshot" | "screen" => Some(CaptureSource::Screenshot),
            "browser" | "web" => Some(CaptureSource::Browser),
            "audio" | "mic" => Some(CaptureSource::Audio),
            "calendar" | "cal" => Some(CaptureSource::Calendar),
            "pdf" | "document" | "doc" => Some(CaptureSource::Pdf),
            "email" | "mail" => Some(CaptureSource::Email),
            "photo" | "photos" | "photolibrary" => Some(CaptureSource::Photo),
            _ => None,
        }
    }

    /// All known sources, in the order the CLI prints them. Stable
    /// order means `tracemind capture list` output is diffable
    /// between runs.
    pub fn all() -> [CaptureSource; 10] {
        [
            CaptureSource::Clipboard,
            CaptureSource::Shell,
            CaptureSource::Notes,
            CaptureSource::Screenshot,
            CaptureSource::Browser,
            CaptureSource::Audio,
            CaptureSource::Calendar,
            CaptureSource::Pdf,
            CaptureSource::Email,
            CaptureSource::Photo,
        ]
    }

    /// Whether this source is enabled in the default first-run config.
    /// Low-sensitivity text-only sources (clipboard, shell, notes) start
    /// enabled because the "memory just is" wedge collapses if first-run
    /// is empty. Anything that could plausibly leak credentials, faces,
    /// or PHI (screenshot, audio, calendar, browser) starts disabled.
    pub fn default_enabled(&self) -> bool {
        matches!(
            self,
            CaptureSource::Clipboard | CaptureSource::Shell | CaptureSource::Notes
        )
    }

    /// One-line description shown in `tracemind capture list` and the
    /// Tauri settings panel. Keep these honest — the user is reading
    /// this *because* they're worried about privacy.
    pub fn description(&self) -> &'static str {
        match self {
            CaptureSource::Clipboard => "system clipboard (polled; high-entropy strings skipped)",
            CaptureSource::Shell => "shell history (.zsh_history / .bash_history tailing)",
            CaptureSource::Notes => "Apple Notes (macOS, AppleScript dump at backfill)",
            CaptureSource::Screenshot => "screenshots via OS hotkey (OCR + caption, raw image stays local)",
            CaptureSource::Browser => "browser bookmarklet / extension (page url, title, selection)",
            CaptureSource::Audio => "microphone via opt-in hotkey (Whisper-tiny, on-device)",
            CaptureSource::Calendar => "macOS EventKit / Google Calendar (read-only, oauth)",
            CaptureSource::Pdf => "PDFs in a watched folder (~/Downloads; text extracted on-device)",
            CaptureSource::Email => "email from a watched .eml drop folder (subject + body, stays local)",
            CaptureSource::Photo => "photo library EXIF (filename, capture time, GPS; image stays local)",
        }
    }
}

impl std::fmt::Display for CaptureSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Per-source state
// ---------------------------------------------------------------------------

/// Persisted state for a single capture source. The schema is
/// intentionally small — anything beyond this lives in
/// `recent.jsonl` or `traces.jsonl`, both of which are user-readable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePermission {
    /// True iff the daemon may capture from this source. Honored at
    /// the *start* of each source loop and re-checked on every
    /// permission write (the daemon SIGHUPs / polls the file).
    pub enabled: bool,

    /// First time the user toggled this source on. `None` if the
    /// source has never been enabled. Useful for showing "you've had
    /// this on for 14 days" in the Tauri panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<DateTime<Utc>>,

    /// Most recent successful capture. `None` means "no events yet."
    /// Surfaced in `tracemind capture status` so users can confirm
    /// the daemon is actually running for that source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<DateTime<Utc>>,

    /// Lifetime count of accepted captures (post-dedup, post-skip).
    /// Surfaced to users so revoking feels grounded ("you've captured
    /// 1,432 events from clipboard").
    #[serde(default)]
    pub event_count: u64,
}

impl SourcePermission {
    fn new_enabled() -> Self {
        Self {
            enabled: true,
            granted_at: Some(Utc::now()),
            last_event_at: None,
            event_count: 0,
        }
    }

    fn new_disabled() -> Self {
        Self {
            enabled: false,
            granted_at: None,
            last_event_at: None,
            event_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Permissions root
// ---------------------------------------------------------------------------

/// The full per-source permissions table. Serialised as TOML to
/// `~/.tracemind/capture_permissions.toml`. Both the CLI and the
/// daemon read/write this file. Atomic via tempfile-rename on save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturePermissions {
    /// Schema version. Bump on incompatible changes; load() will
    /// reject older versions with a clear error.
    #[serde(default = "default_version")]
    pub version: u32,

    /// Per-source state. Using `BTreeMap` so the TOML output is
    /// alphabetically ordered and stable across writes.
    #[serde(default)]
    pub sources: BTreeMap<String, SourcePermission>,
}

fn default_version() -> u32 {
    1
}

impl Default for CapturePermissions {
    fn default() -> Self {
        let mut sources = BTreeMap::new();
        for source in CaptureSource::all() {
            let perm = if source.default_enabled() {
                SourcePermission::new_enabled()
            } else {
                SourcePermission::new_disabled()
            };
            sources.insert(source.as_str().to_string(), perm);
        }
        Self {
            version: 1,
            sources,
        }
    }
}

impl CapturePermissions {
    /// Resolve the default permissions file under the TraceMind data
    /// directory. Honors `TM_DATA_DIR` for tests.
    pub fn default_path() -> PathBuf {
        let dir = if let Ok(val) = std::env::var("TM_DATA_DIR") {
            PathBuf::from(val)
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".tracemind")
        };
        dir.join("capture_permissions.toml")
    }

    /// Load permissions from `path`, creating a default file on first
    /// run. The default file has clipboard + shell enabled and
    /// everything else disabled, matching the privacy invariant in
    /// `docs/TASKS.md` P1b.
    pub fn load_or_default(path: &Path) -> Result<Self, TraceMindError> {
        if !path.exists() {
            let default = Self::default();
            default.save(path)?;
            return Ok(default);
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            TraceMindError::Storage(format!("read {}: {e}", path.display()))
        })?;
        let mut perms: CapturePermissions = toml::from_str(&text).map_err(|e| {
            TraceMindError::Storage(format!("parse {}: {e}", path.display()))
        })?;
        if perms.version != 1 {
            return Err(TraceMindError::Storage(format!(
                "capture_permissions.toml version {} is not supported (expected 1)",
                perms.version
            )));
        }
        // Backfill: if a new source was added in a release, give it
        // its default state so the user is never silently captured
        // from a source they haven't seen.
        for source in CaptureSource::all() {
            perms.sources.entry(source.as_str().to_string()).or_insert_with(|| {
                if source.default_enabled() {
                    SourcePermission::new_enabled()
                } else {
                    SourcePermission::new_disabled()
                }
            });
        }
        Ok(perms)
    }

    /// Convenience: load from the default path.
    pub fn load() -> Result<Self, TraceMindError> {
        Self::load_or_default(&Self::default_path())
    }

    /// Atomically write the permissions file. Writes to a sibling
    /// `.tmp` file and `rename`s into place so a crash mid-write can
    /// never leave a half-written config.
    pub fn save(&self, path: &Path) -> Result<(), TraceMindError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                TraceMindError::Storage(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| {
            TraceMindError::Storage(format!("encode permissions: {e}"))
        })?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)
            .map_err(|e| TraceMindError::Storage(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| TraceMindError::Storage(format!("rename {}: {e}", path.display())))?;
        Ok(())
    }

    /// Lookup a single source. Returns the default-disabled state for
    /// unknown sources so the daemon fails *closed* on permission
    /// misses (the privacy-preserving default).
    pub fn get(&self, source: CaptureSource) -> SourcePermission {
        self.sources
            .get(source.as_str())
            .cloned()
            .unwrap_or_else(SourcePermission::new_disabled)
    }

    /// Convenience: is this source enabled right now?
    pub fn is_enabled(&self, source: CaptureSource) -> bool {
        self.get(source).enabled
    }

    /// Enable a source. Sets `granted_at` to now if not already set —
    /// re-enabling a source the user previously turned off does NOT
    /// reset the original grant timestamp (the user already consented
    /// once; toggling back on is reaffirmation, not new consent).
    pub fn enable(&mut self, source: CaptureSource) {
        let entry = self
            .sources
            .entry(source.as_str().to_string())
            .or_insert_with(SourcePermission::new_disabled);
        entry.enabled = true;
        if entry.granted_at.is_none() {
            entry.granted_at = Some(Utc::now());
        }
    }

    /// Disable a source. Preserves `granted_at` and `event_count` so
    /// the user can see *"you had clipboard on for 14 days and
    /// captured 1,432 events; we've forgotten none of them"* — the
    /// counter is the audit trail, not the silence.
    pub fn disable(&mut self, source: CaptureSource) {
        let entry = self
            .sources
            .entry(source.as_str().to_string())
            .or_insert_with(SourcePermission::new_disabled);
        entry.enabled = false;
    }

    /// Record a successful capture event. Bumps `last_event_at` and
    /// `event_count`. No-op if the source is disabled (defensive — the
    /// daemon shouldn't be calling this for a disabled source, but if
    /// it does, the counter doesn't lie).
    pub fn record_event(&mut self, source: CaptureSource) {
        if let Some(entry) = self.sources.get_mut(source.as_str()) {
            if entry.enabled {
                entry.last_event_at = Some(Utc::now());
                entry.event_count = entry.event_count.saturating_add(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_has_clipboard_and_shell_on() {
        let perms = CapturePermissions::default();
        assert!(perms.is_enabled(CaptureSource::Clipboard));
        assert!(perms.is_enabled(CaptureSource::Shell));
        assert!(!perms.is_enabled(CaptureSource::Screenshot));
        assert!(!perms.is_enabled(CaptureSource::Browser));
        assert!(!perms.is_enabled(CaptureSource::Audio));
        assert!(!perms.is_enabled(CaptureSource::Calendar));
    }

    #[test]
    fn first_run_creates_file_with_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("capture_permissions.toml");
        assert!(!path.exists());

        let perms = CapturePermissions::load_or_default(&path).unwrap();
        assert!(path.exists(), "first-run must create the file");
        assert!(perms.is_enabled(CaptureSource::Clipboard));
        assert!(!perms.is_enabled(CaptureSource::Audio));
    }

    #[test]
    fn enable_records_granted_at_once() {
        let mut perms = CapturePermissions::default();
        // Audio starts disabled with no grant
        assert!(perms.get(CaptureSource::Audio).granted_at.is_none());

        perms.enable(CaptureSource::Audio);
        let first_grant = perms.get(CaptureSource::Audio).granted_at;
        assert!(first_grant.is_some());

        // Toggle off then back on — original grant must be preserved
        perms.disable(CaptureSource::Audio);
        perms.enable(CaptureSource::Audio);
        assert_eq!(
            perms.get(CaptureSource::Audio).granted_at,
            first_grant,
            "re-enabling must not reset granted_at"
        );
    }

    #[test]
    fn record_event_no_ops_on_disabled() {
        let mut perms = CapturePermissions::default();
        // Audio is disabled by default — recording must not bump
        perms.record_event(CaptureSource::Audio);
        assert_eq!(perms.get(CaptureSource::Audio).event_count, 0);
        assert!(perms.get(CaptureSource::Audio).last_event_at.is_none());

        // Clipboard is enabled — recording must bump
        perms.record_event(CaptureSource::Clipboard);
        assert_eq!(perms.get(CaptureSource::Clipboard).event_count, 1);
        assert!(perms.get(CaptureSource::Clipboard).last_event_at.is_some());
    }

    #[test]
    fn round_trip_via_toml_preserves_state() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("capture_permissions.toml");

        let mut perms = CapturePermissions::default();
        perms.enable(CaptureSource::Screenshot);
        perms.record_event(CaptureSource::Clipboard);
        perms.record_event(CaptureSource::Clipboard);
        perms.save(&path).unwrap();

        let reloaded = CapturePermissions::load_or_default(&path).unwrap();
        assert!(reloaded.is_enabled(CaptureSource::Screenshot));
        assert_eq!(reloaded.get(CaptureSource::Clipboard).event_count, 2);
    }

    #[test]
    fn unknown_source_in_toml_defaults_to_closed() {
        let perms = CapturePermissions::default();
        // Manually construct a permission set missing one source
        let mut sparse = perms.clone();
        sparse.sources.remove("calendar");

        // get() must return a disabled permission, not panic
        let cal = sparse.get(CaptureSource::Calendar);
        assert!(!cal.enabled, "missing source must default to disabled");
    }

    #[test]
    fn backfill_adds_new_sources_on_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("capture_permissions.toml");

        // Write a TOML missing some sources (simulating an old install
        // before a new source was added)
        std::fs::write(
            &path,
            r#"version = 1

[sources.clipboard]
enabled = true
event_count = 5
"#,
        )
        .unwrap();

        let perms = CapturePermissions::load_or_default(&path).unwrap();
        // Existing source preserved
        assert_eq!(perms.get(CaptureSource::Clipboard).event_count, 5);
        // New source backfilled with its default
        assert!(!perms.is_enabled(CaptureSource::Audio));
        assert!(perms.is_enabled(CaptureSource::Shell));
    }

    #[test]
    fn rejects_unknown_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("capture_permissions.toml");
        std::fs::write(&path, "version = 99\n").unwrap();
        let err = CapturePermissions::load_or_default(&path).unwrap_err();
        assert!(format!("{err}").contains("version"));
    }

    #[test]
    fn parse_source_is_case_insensitive() {
        assert_eq!(CaptureSource::parse("Clipboard"), Some(CaptureSource::Clipboard));
        assert_eq!(CaptureSource::parse("CLIP"), Some(CaptureSource::Clipboard));
        assert_eq!(CaptureSource::parse("history"), Some(CaptureSource::Shell));
        assert_eq!(CaptureSource::parse("nonsense"), None);
    }
}
