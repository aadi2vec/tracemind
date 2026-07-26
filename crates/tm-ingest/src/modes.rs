//! Ingestion mode manager — Ambient / Focus / Private (I-P2 §3.1).
//!
//! The [`ModeManager`] tracks the *currently active* mode plus any bounded
//! session (Focus name + duration, Private timer). State is persisted so a
//! daemon restart does not silently reset the user's intent to Ambient.
//!
//! State layout (`~/.tracemind/ingest_mode.json`):
//! ```json
//! {
//!   "mode": "focus",
//!   "session": {
//!     "id": "...",
//!     "mode": "focus",
//!     "name": "call-prep",
//!     "started_at": "2026-07-24T10:00:00Z",
//!     "duration_secs": 2700,
//!     "ended_at": null
//!   }
//! }
//! ```
//!
//! `ModeManager` never blocks and never touches the network. Every mutation
//! writes the whole file (small, cheap, atomic).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tm_types::{CaptureMode, ModeSession, Result, TraceMindError};

/// File name written under the data directory.
pub const MODE_FILE_NAME: &str = "ingest_mode.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ModeState {
    #[serde(default)]
    mode: CaptureMode,
    #[serde(default)]
    session: Option<ModeSession>,
}

/// Owns the mode file and gates every mode-related capture decision.
pub struct ModeManager {
    path: PathBuf,
    state: Mutex<ModeState>,
}

impl ModeManager {
    /// Open (or create) the manager rooted at `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(MODE_FILE_NAME);
        let state = Self::load_from(&path);
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn load_from(path: &Path) -> ModeState {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => ModeState::default(),
        }
    }

    fn save(&self, state: &ModeState) -> Result<()> {
        let json = serde_json::to_string_pretty(state).map_err(TraceMindError::from)?;
        std::fs::write(&self.path, json)
            .map_err(|e| TraceMindError::Storage(e.to_string()))
    }

    /// Effective mode at `now`. If the active session has expired, the mode
    /// silently falls back to Ambient — this is the S9 guarantee that
    /// time-boxed modes cannot outlive their deadline.
    pub fn current(&self, now: DateTime<Utc>) -> CaptureMode {
        let mut state = self.state.lock().expect("mode state lock");
        if let Some(session) = state.session.clone() {
            if session.is_expired(now) {
                // Auto-end the session and revert.
                let mut s = session.clone();
                s.end(now);
                state.session = Some(s);
                state.mode = CaptureMode::Ambient;
                // Best-effort persist; ignore write errors in the read path.
                let _ = self.save(&state);
            }
        }
        state.mode
    }

    /// Snapshot the active session, if any. Auto-expires when past its
    /// deadline (same semantics as [`current`]).
    pub fn active_session(&self, now: DateTime<Utc>) -> Option<ModeSession> {
        let _ = self.current(now); // side-effect: expires stale sessions.
        self.state.lock().expect("mode state lock").session.clone()
    }

    /// Enter Focus mode with an explicit intent name and duration in seconds.
    /// Overwrites any previous session.
    pub fn enter_focus(&self, intent: impl Into<String>, duration_secs: i64) -> Result<ModeSession> {
        let session = ModeSession::start(CaptureMode::Focus, intent, duration_secs.max(1));
        let mut state = self.state.lock().expect("mode state lock");
        state.mode = CaptureMode::Focus;
        state.session = Some(session.clone());
        self.save(&state)?;
        Ok(session)
    }

    /// Enter Private mode with a bounded duration. Watchers should pause
    /// while this session is active (see [`CaptureMode::pauses_watchers`]).
    pub fn enter_private(&self, duration_secs: i64) -> Result<ModeSession> {
        let session = ModeSession::start(CaptureMode::Private, "", duration_secs.max(1));
        let mut state = self.state.lock().expect("mode state lock");
        state.mode = CaptureMode::Private;
        state.session = Some(session.clone());
        self.save(&state)?;
        Ok(session)
    }

    /// Manually end the current session and revert to Ambient.
    pub fn end_session(&self, now: DateTime<Utc>) -> Result<Option<ModeSession>> {
        let mut state = self.state.lock().expect("mode state lock");
        let ended = if let Some(session) = state.session.as_mut() {
            session.end(now);
            let snapshot = session.clone();
            Some(snapshot)
        } else {
            None
        };
        state.mode = CaptureMode::Ambient;
        self.save(&state)?;
        Ok(ended)
    }

    /// Path this manager persists to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn default_mode_is_ambient() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Ambient);
        assert!(mgr.active_session(Utc::now()).is_none());
    }

    #[test]
    fn enter_focus_records_session() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        let s = mgr.enter_focus("call-prep", 60 * 30).unwrap();
        assert_eq!(s.mode, CaptureMode::Focus);
        assert_eq!(s.name, "call-prep");
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Focus);
    }

    #[test]
    fn enter_private_records_session() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        let s = mgr.enter_private(300).unwrap();
        assert_eq!(s.mode, CaptureMode::Private);
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Private);
    }

    #[test]
    fn expired_session_auto_reverts() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        let s = mgr.enter_focus("brief", 10).unwrap();
        // Simulate wall-clock past the deadline.
        let future = s.started_at + Duration::seconds(30);
        assert_eq!(mgr.current(future), CaptureMode::Ambient);
    }

    #[test]
    fn end_session_now_reverts_to_ambient() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        mgr.enter_focus("x", 300).unwrap();
        let ended = mgr.end_session(Utc::now()).unwrap();
        assert!(ended.is_some());
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Ambient);
    }

    #[test]
    fn state_persists_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mgr = ModeManager::open(dir.path()).unwrap();
            mgr.enter_focus("resume-test", 60 * 60).unwrap();
        }
        let mgr = ModeManager::open(dir.path()).unwrap();
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Focus);
        let session = mgr.active_session(Utc::now()).unwrap();
        assert_eq!(session.name, "resume-test");
    }

    #[test]
    fn end_session_with_no_session_still_reverts() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModeManager::open(dir.path()).unwrap();
        let ended = mgr.end_session(Utc::now()).unwrap();
        assert!(ended.is_none());
        assert_eq!(mgr.current(Utc::now()), CaptureMode::Ambient);
    }
}
