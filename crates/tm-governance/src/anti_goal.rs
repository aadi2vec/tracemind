//! Anti-goal rules and application scoping (I12 / I13).
//!
//! Two related policies live here:
//!
//! * [`AntiGoalRules`] — the user-declared negative list ("never capture
//!   URLs matching \*banking\*, never capture the token 'password'"). This
//!   is orthogonal to the sensitive-app blocklist ([`crate::SensitiveAppPolicy`])
//!   which is source-scoped; anti-goal rules are content-scoped.
//! * [`AppScopingPolicy`] — per-app policy (always-capture, never-capture,
//!   ask-on-first-capture). Layers on top of the sensitive-app blocklist:
//!   blocklist is the "safety floor" of apps the user cannot silently
//!   enable; app scoping is the day-to-day preference above that floor.
//!
//! Both are pure Rust — no OS calls. Persistence uses the same load/save
//! pattern as `SensitiveAppPolicy`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};

// ---------------------------------------------------------------------------
// Anti-goal rules — I12
// ---------------------------------------------------------------------------

pub const ANTI_GOAL_FILE_NAME: &str = "anti_goal.json";

/// Live-configurable deny list. Rules are cheap glob-ish substrings — we do
/// not pull in a regex crate here to keep governance dependency-thin.
///
/// * `url_substrings` — if any of these appears in the source URL (case-
///   insensitive), the capture is skipped and the receipt records `skipped`.
/// * `content_tokens` — if the raw content contains any of these tokens
///   (case-insensitive), the capture is skipped.
/// * `source_names` — if the capture source (e.g. "clipboard", "web") is in
///   this set, the capture is skipped. Coarse but handy for the "no more
///   clipboard captures today" case.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AntiGoalRules {
    pub url_substrings: HashSet<String>,
    pub content_tokens: HashSet<String>,
    pub source_names: HashSet<String>,
}

fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

impl AntiGoalRules {
    /// True when either the URL, the content, or the source matches any
    /// rule. Returns the matching rule as a `String` so the receipt can
    /// name *why* the capture was skipped.
    pub fn matches(&self, url: Option<&str>, content: &str, source: &str) -> Option<String> {
        if let Some(u) = url {
            let key = u.to_ascii_lowercase();
            for rule in &self.url_substrings {
                if key.contains(rule) {
                    return Some(format!("url:{rule}"));
                }
            }
        }
        let body = content.to_ascii_lowercase();
        for token in &self.content_tokens {
            if body.contains(token) {
                return Some(format!("content:{token}"));
            }
        }
        let src = norm(source);
        if self.source_names.contains(&src) {
            return Some(format!("source:{src}"));
        }
        None
    }

    pub fn add_url(&mut self, s: impl AsRef<str>) {
        self.url_substrings.insert(norm(s.as_ref()));
    }

    pub fn add_token(&mut self, s: impl AsRef<str>) {
        self.content_tokens.insert(norm(s.as_ref()));
    }

    pub fn add_source(&mut self, s: impl AsRef<str>) {
        self.source_names.insert(norm(s.as_ref()));
    }

    pub fn remove_url(&mut self, s: &str) -> bool {
        self.url_substrings.remove(&norm(s))
    }

    pub fn remove_token(&mut self, s: &str) -> bool {
        self.content_tokens.remove(&norm(s))
    }

    pub fn remove_source(&mut self, s: &str) -> bool {
        self.source_names.remove(&norm(s))
    }

    pub fn load_or_default(dir: &Path) -> Self {
        let path = dir.join(ANTI_GOAL_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(ANTI_GOAL_FILE_NAME);
        let json = serde_json::to_string_pretty(self).map_err(TraceMindError::from)?;
        std::fs::write(&path, json).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// App scoping — I13
// ---------------------------------------------------------------------------

pub const APP_SCOPING_FILE_NAME: &str = "app_scoping.json";

/// Per-app disposition. `Ask` is the interesting state: it means the app
/// has not yet been decided, and the daemon should surface a Brief prompt
/// on the first capture attempt so the user can make an explicit call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppDisposition {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AppScopingPolicy {
    /// Explicit per-app dispositions.
    pub rules: HashMap<String, AppDisposition>,
    /// Apps we have already prompted the user about — prevents repeat asks
    /// after the user closes the prompt without answering.
    pub prompted: HashSet<String>,
    /// Default for apps not in `rules`. Ambient-safe out of the box.
    pub default_disposition: Option<AppDisposition>,
}

impl AppScopingPolicy {
    /// Effective decision for a given foreground app.
    pub fn decide(&self, app: &str) -> AppDisposition {
        let key = norm(app);
        if let Some(d) = self.rules.get(&key) {
            return *d;
        }
        self.default_disposition.unwrap_or(AppDisposition::Ask)
    }

    /// True when the daemon should surface a Brief prompt for this app
    /// (unknown app + not yet prompted).
    pub fn should_prompt(&self, app: &str) -> bool {
        let key = norm(app);
        if self.rules.contains_key(&key) {
            return false;
        }
        !self.prompted.contains(&key)
    }

    pub fn allow(&mut self, app: impl AsRef<str>) {
        let key = norm(app.as_ref());
        self.rules.insert(key.clone(), AppDisposition::Allow);
        self.prompted.insert(key);
    }

    pub fn deny(&mut self, app: impl AsRef<str>) {
        let key = norm(app.as_ref());
        self.rules.insert(key.clone(), AppDisposition::Deny);
        self.prompted.insert(key);
    }

    pub fn ask(&mut self, app: impl AsRef<str>) {
        let key = norm(app.as_ref());
        self.rules.insert(key.clone(), AppDisposition::Ask);
    }

    /// Record that we already prompted the user for this app so we do not
    /// re-prompt indefinitely.
    pub fn mark_prompted(&mut self, app: impl AsRef<str>) {
        self.prompted.insert(norm(app.as_ref()));
    }

    pub fn load_or_default(dir: &Path) -> Self {
        let path = dir.join(APP_SCOPING_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(APP_SCOPING_FILE_NAME);
        let json = serde_json::to_string_pretty(self).map_err(TraceMindError::from)?;
        std::fs::write(&path, json).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anti_goal_url_match_case_insensitive() {
        let mut r = AntiGoalRules::default();
        r.add_url("banking");
        let hit = r.matches(Some("https://Chase.Banking.example/x"), "hello", "web");
        assert_eq!(hit.as_deref(), Some("url:banking"));
    }

    #[test]
    fn anti_goal_content_token_match() {
        let mut r = AntiGoalRules::default();
        r.add_token("password");
        let hit = r.matches(None, "my Password is hunter2", "clipboard");
        assert_eq!(hit.as_deref(), Some("content:password"));
    }

    #[test]
    fn anti_goal_source_name_match() {
        let mut r = AntiGoalRules::default();
        r.add_source("Clipboard");
        let hit = r.matches(None, "anything", "clipboard");
        assert_eq!(hit.as_deref(), Some("source:clipboard"));
    }

    #[test]
    fn anti_goal_no_match_returns_none() {
        let r = AntiGoalRules::default();
        assert!(r.matches(Some("https://example.com"), "hello", "clipboard").is_none());
    }

    #[test]
    fn anti_goal_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = AntiGoalRules::default();
        r.add_url("banking");
        r.add_token("ssn");
        r.save(dir.path()).unwrap();
        let loaded = AntiGoalRules::load_or_default(dir.path());
        assert!(loaded.matches(Some("//banking"), "hi", "web").is_some());
        assert!(loaded.matches(None, "SSN=…", "clipboard").is_some());
    }

    #[test]
    fn anti_goal_remove_undoes_add() {
        let mut r = AntiGoalRules::default();
        r.add_url("banking");
        assert!(r.remove_url("banking"));
        assert!(r.matches(Some("//banking"), "", "web").is_none());
    }

    #[test]
    fn app_scoping_default_is_ask() {
        let p = AppScopingPolicy::default();
        assert_eq!(p.decide("Xcode"), AppDisposition::Ask);
        assert!(p.should_prompt("Xcode"));
    }

    #[test]
    fn app_scoping_allow_persists_and_stops_prompt() {
        let mut p = AppScopingPolicy::default();
        p.allow("Xcode");
        assert_eq!(p.decide("xcode"), AppDisposition::Allow);
        assert!(!p.should_prompt("Xcode"));
    }

    #[test]
    fn app_scoping_deny_records_disposition() {
        let mut p = AppScopingPolicy::default();
        p.deny("Messages");
        assert_eq!(p.decide("messages"), AppDisposition::Deny);
        assert!(!p.should_prompt("Messages"));
    }

    #[test]
    fn app_scoping_mark_prompted_prevents_second_prompt() {
        let mut p = AppScopingPolicy::default();
        assert!(p.should_prompt("Notion"));
        p.mark_prompted("Notion");
        assert!(!p.should_prompt("Notion"));
        // But decide still falls back to Ask.
        assert_eq!(p.decide("Notion"), AppDisposition::Ask);
    }

    #[test]
    fn app_scoping_save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = AppScopingPolicy::default();
        p.allow("Xcode");
        p.deny("Messages");
        p.mark_prompted("Notion");
        p.save(dir.path()).unwrap();

        let loaded = AppScopingPolicy::load_or_default(dir.path());
        assert_eq!(loaded.decide("xcode"), AppDisposition::Allow);
        assert_eq!(loaded.decide("Messages"), AppDisposition::Deny);
        assert!(!loaded.should_prompt("Notion"));
    }

    #[test]
    fn app_scoping_default_disposition_overrides_ask() {
        let mut p = AppScopingPolicy::default();
        p.default_disposition = Some(AppDisposition::Allow);
        assert_eq!(p.decide("Anything"), AppDisposition::Allow);
    }
}
