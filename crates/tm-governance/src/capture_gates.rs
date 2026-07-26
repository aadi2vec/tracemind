//! Sensitive-app blocklist — S5 from the Ingestion Experience Plan.
//!
//! Some applications should never contribute to ambient capture unless the
//! user explicitly opts in per-app: messengers, password managers, banking,
//! health. This module owns the bundled deny-list, the per-user overrides,
//! and the persistence hook.
//!
//! The policy is deliberately case-insensitive. `"1password"`, `"1Password"`,
//! and `"1PASSWORD"` all resolve to the same rule so that heterogenous
//! app-context strings (bundle names, window titles, browser tabs) do not
//! silently bypass the block.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};

/// File name written under the data directory.
pub const POLICY_FILE_NAME: &str = "sensitive_apps.json";

/// The apps that ship blocked out of the box.
///
/// Grouped by intent so future maintainers can see *why* each entry is here:
/// - **messengers** — high false-positive risk on personal conversation
/// - **password / secret managers** — capturing here is the definitional bug
/// - **banking / brokerage** — regulatory + trust cliff
/// - **health / mindfulness** — sensitive personal state
pub const DEFAULT_BLOCKLIST: &[&str] = &[
    "Signal",
    "Messages",
    "1Password",
    "Bitwarden",
    "Bank of America",
    "Chase",
    "Wells Fargo",
    "Robinhood",
    "Coinbase",
    "Health",
    "Fitness",
    "Mental Health",
    "Calm",
    "Headspace",
];

fn normalise(app: &str) -> String {
    app.trim().to_ascii_lowercase()
}

/// Per-user policy layered on top of the bundled defaults.
///
/// * `blocklist` — the effective set of apps whose captures are dropped.
/// * `allowlist_overrides` — entries the user explicitly re-enabled so a
///   bundled default no longer applies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SensitiveAppPolicy {
    pub blocklist: HashSet<String>,
    pub allowlist_overrides: HashSet<String>,
}

impl SensitiveAppPolicy {
    /// Bundled defaults — the deny-list from plan §5 S5.
    pub fn bundled() -> Self {
        let mut blocklist = HashSet::new();
        for app in DEFAULT_BLOCKLIST {
            blocklist.insert(normalise(app));
        }
        Self {
            blocklist,
            allowlist_overrides: HashSet::new(),
        }
    }

    /// True when the app is on the blocklist and the user has NOT overridden it.
    pub fn is_blocked(&self, app: &str) -> bool {
        let key = normalise(app);
        if self.allowlist_overrides.contains(&key) {
            return false;
        }
        self.blocklist.contains(&key)
    }

    /// User override — re-enable an app the default blocked.
    pub fn allow(&mut self, app: impl AsRef<str>) {
        let key = normalise(app.as_ref());
        self.allowlist_overrides.insert(key);
    }

    /// User override — block an app that wasn't on the default deny-list
    /// (or re-block one that was previously allowed).
    pub fn deny(&mut self, app: impl AsRef<str>) {
        let key = normalise(app.as_ref());
        self.allowlist_overrides.remove(&key);
        self.blocklist.insert(key);
    }

    /// Number of currently blocked apps (defaults + user additions, minus
    /// overrides).
    pub fn effective_len(&self) -> usize {
        self.blocklist
            .iter()
            .filter(|k| !self.allowlist_overrides.contains(*k))
            .count()
    }

    /// Load from `dir/sensitive_apps.json`, falling back to bundled defaults
    /// when the file is missing or corrupt.
    pub fn load_or_default(dir: &Path) -> Self {
        let path = dir.join(POLICY_FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<SensitiveAppPolicy>(&raw)
                .unwrap_or_else(|_| Self::bundled()),
            Err(_) => Self::bundled(),
        }
    }

    /// Persist to `dir/sensitive_apps.json`. Creates the directory if needed.
    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(POLICY_FILE_NAME);
        let json = serde_json::to_string_pretty(self)
            .map_err(TraceMindError::from)?;
        std::fs::write(&path, json)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Convenience — resolve the full path this policy loads from.
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(POLICY_FILE_NAME)
    }
}

impl Default for &SensitiveAppPolicy {
    fn default() -> Self {
        static BUNDLED: std::sync::OnceLock<SensitiveAppPolicy> = std::sync::OnceLock::new();
        BUNDLED.get_or_init(SensitiveAppPolicy::bundled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_blocks_default_apps() {
        let p = SensitiveAppPolicy::bundled();
        for app in DEFAULT_BLOCKLIST {
            assert!(p.is_blocked(app), "default deny missed: {app}");
        }
    }

    #[test]
    fn is_blocked_is_case_insensitive() {
        let p = SensitiveAppPolicy::bundled();
        assert!(p.is_blocked("signal"));
        assert!(p.is_blocked("SIGNAL"));
        assert!(p.is_blocked("  Signal  "));
        assert!(p.is_blocked("1password"));
    }

    #[test]
    fn allowlist_override_wins() {
        let mut p = SensitiveAppPolicy::bundled();
        assert!(p.is_blocked("Signal"));
        p.allow("Signal");
        assert!(!p.is_blocked("Signal"));
        // Case still folds after the override.
        assert!(!p.is_blocked("signal"));
    }

    #[test]
    fn deny_adds_user_specified_apps() {
        let mut p = SensitiveAppPolicy::bundled();
        assert!(!p.is_blocked("Personal.app"));
        p.deny("Personal.app");
        assert!(p.is_blocked("Personal.app"));
        assert!(p.is_blocked("personal.app"));
    }

    #[test]
    fn deny_reverses_an_earlier_allow() {
        let mut p = SensitiveAppPolicy::bundled();
        p.allow("Signal");
        assert!(!p.is_blocked("Signal"));
        p.deny("Signal");
        assert!(p.is_blocked("Signal"));
    }

    #[test]
    fn effective_len_reflects_overrides() {
        let mut p = SensitiveAppPolicy::bundled();
        let base = p.effective_len();
        assert_eq!(base, DEFAULT_BLOCKLIST.len());
        p.allow("Signal");
        assert_eq!(p.effective_len(), base - 1);
        p.deny("MyApp");
        assert_eq!(p.effective_len(), base);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = SensitiveAppPolicy::bundled();
        p.allow("Signal");
        p.deny("Personal.app");
        p.save(dir.path()).unwrap();

        let loaded = SensitiveAppPolicy::load_or_default(dir.path());
        assert!(!loaded.is_blocked("Signal"), "override persisted");
        assert!(loaded.is_blocked("Personal.app"), "deny persisted");
        // Untouched defaults still block.
        assert!(loaded.is_blocked("1Password"));
    }

    #[test]
    fn load_falls_back_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = SensitiveAppPolicy::load_or_default(dir.path());
        assert!(loaded.is_blocked("Signal"));
    }

    #[test]
    fn load_falls_back_when_file_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(POLICY_FILE_NAME), "{not valid json").unwrap();
        let loaded = SensitiveAppPolicy::load_or_default(dir.path());
        assert!(loaded.is_blocked("Signal"));
    }
}
