//! NFC / QR physical anchor (I18 / N2.8).
//!
//! A tag (NFC UID, QR code payload, or any opaque string the OS integration
//! delivers) is bound to an intent name for a bounded window. Every capture
//! that arrives while the tag is active gets automatically tagged with the
//! anchor name so the "everything in the next hour is about this project"
//! use-case is a two-tap operation, not a manual intent hunt.
//!
//! Anchors compose with Focus mode — if both are active, both intents flow
//! into the capture receipt (Focus wins the receipt.user_intent slot;
//! anchor sits in `matched_anchor`). Keeps state in
//! `~/.tracemind/anchors.jsonl`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

pub const ANCHORS_FILE_NAME: &str = "anchors.jsonl";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PhysicalAnchor {
    pub id: Uuid,
    /// Opaque OS-delivered id — NFC UID, QR payload, iBeacon major/minor …
    pub tag: String,
    pub intent: String,
    pub scanned_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub retired: bool,
}

pub struct AnchorStore {
    path: PathBuf,
    inner: Mutex<Vec<PhysicalAnchor>>,
}

impl AnchorStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(ANCHORS_FILE_NAME);
        let inner = load(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Register a scan. Default window is one hour (§ N2.8) if `ttl_minutes`
    /// is zero or negative.
    pub fn scan(&self, tag: impl Into<String>, intent: impl Into<String>, ttl_minutes: i64) -> Result<PhysicalAnchor> {
        let now = Utc::now();
        let ttl = if ttl_minutes <= 0 { 60 } else { ttl_minutes };
        let anchor = PhysicalAnchor {
            id: Uuid::new_v4(),
            tag: tag.into(),
            intent: intent.into(),
            scanned_at: now,
            expires_at: now + Duration::minutes(ttl),
            retired: false,
        };
        let mut items = self.inner.lock().expect("anchor lock");
        items.push(anchor.clone());
        rewrite(&self.path, &items)?;
        Ok(anchor)
    }

    /// The active intent name for a capture that fires at `now`, if any.
    /// Returns the *most recent* active anchor when multiple exist — the
    /// user was closer to it in time so its intent wins.
    pub fn active_intent(&self, now: DateTime<Utc>) -> Option<String> {
        let items = self.inner.lock().expect("anchor lock");
        items
            .iter()
            .filter(|a| !a.retired && a.expires_at > now)
            .max_by(|a, b| a.scanned_at.cmp(&b.scanned_at))
            .map(|a| a.intent.clone())
    }

    pub fn retire(&self, id: Uuid) -> Result<Option<PhysicalAnchor>> {
        let mut items = self.inner.lock().expect("anchor lock");
        let out = if let Some(a) = items.iter_mut().find(|a| a.id == id) {
            a.retired = true;
            Some(a.clone())
        } else {
            None
        };
        rewrite(&self.path, &items)?;
        Ok(out)
    }

    pub fn list(&self) -> Vec<PhysicalAnchor> {
        let items = self.inner.lock().expect("anchor lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.scanned_at.cmp(&a.scanned_at));
        v
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn load(path: &Path) -> Result<Vec<PhysicalAnchor>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TraceMindError::Storage(e.to_string())),
    };
    Ok(raw
        .split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn rewrite(path: &Path, items: &[PhysicalAnchor]) -> Result<()> {
    let mut buf = String::new();
    for it in items {
        let line = serde_json::to_string(it)?;
        buf.push_str(&line);
        buf.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    file.write_all(buf.as_bytes())
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_registers_anchor_with_default_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnchorStore::open(dir.path()).unwrap();
        let a = store.scan("nfc:desk", "kitchen-renovation", 0).unwrap();
        assert_eq!(a.tag, "nfc:desk");
        assert!(a.expires_at > a.scanned_at);
    }

    #[test]
    fn active_intent_returns_none_when_no_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnchorStore::open(dir.path()).unwrap();
        assert_eq!(store.active_intent(Utc::now()), None);
    }

    #[test]
    fn active_intent_prefers_most_recent_scan() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnchorStore::open(dir.path()).unwrap();
        store.scan("nfc:a", "topic-a", 60).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        store.scan("nfc:b", "topic-b", 60).unwrap();
        assert_eq!(store.active_intent(Utc::now()), Some("topic-b".into()));
    }

    #[test]
    fn expired_anchor_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnchorStore::open(dir.path()).unwrap();
        let a = store.scan("nfc:c", "topic-c", 1).unwrap();
        let future = a.expires_at + Duration::minutes(5);
        assert_eq!(store.active_intent(future), None);
    }

    #[test]
    fn retire_removes_anchor_from_active_set() {
        let dir = tempfile::tempdir().unwrap();
        let store = AnchorStore::open(dir.path()).unwrap();
        let a = store.scan("nfc:d", "topic-d", 60).unwrap();
        store.retire(a.id).unwrap();
        assert_eq!(store.active_intent(Utc::now()), None);
    }
}
