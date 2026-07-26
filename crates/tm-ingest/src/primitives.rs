//! Novel capture primitives — I15/I16/I17/I14/I18.
//!
//! Everything here is a small persistent side-store: chain bundles,
//! retro-capture windows, ephemeral (24h auto-delete) captures, time-locked
//! memories, journal + debrief entries, weekly review checkpoints. Each is
//! a JSONL log so third-party tools can read without a TraceMind binary.
//!
//! No I/O to the network, no OS calls. Everything is bounded and reversible
//! — matches the safety commitments in `docs/INGESTION_EXPERIENCE_PLAN §5`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// File names
// ---------------------------------------------------------------------------

pub const CHAIN_FILE_NAME: &str = "capture_chains.jsonl";
pub const EPHEMERAL_FILE_NAME: &str = "ephemeral.jsonl";
pub const TIME_LOCK_FILE_NAME: &str = "time_locked.jsonl";
pub const JOURNAL_FILE_NAME: &str = "journal.jsonl";
pub const RETRO_FILE_NAME: &str = "retro_windows.jsonl";
pub const WEEKLY_FILE_NAME: &str = "weekly_reviews.jsonl";
pub const CONTRADICTION_WATCH_FILE_NAME: &str = "contradiction_watch.jsonl";

// ---------------------------------------------------------------------------
// Chain bundles (I15 / N2.1)
// ---------------------------------------------------------------------------

/// A user-declared bundle of captures that surface together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CaptureChain {
    pub id: Uuid,
    pub name: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub member_capture_ids: Vec<Uuid>,
}

impl CaptureChain {
    pub fn is_open(&self) -> bool {
        self.ended_at.is_none()
    }
}

pub struct ChainStore {
    path: PathBuf,
    inner: Mutex<Vec<CaptureChain>>,
}

impl ChainStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(CHAIN_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn start(&self, name: impl Into<String>) -> Result<CaptureChain> {
        let mut items = self.inner.lock().expect("chain lock");
        let chain = CaptureChain {
            id: Uuid::new_v4(),
            name: name.into(),
            started_at: Utc::now(),
            ended_at: None,
            member_capture_ids: Vec::new(),
        };
        items.push(chain.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(chain)
    }

    pub fn add(&self, chain_id: Uuid, capture_id: Uuid) -> Result<Option<CaptureChain>> {
        let mut items = self.inner.lock().expect("chain lock");
        let out = if let Some(c) = items.iter_mut().find(|c| c.id == chain_id && c.is_open()) {
            if !c.member_capture_ids.contains(&capture_id) {
                c.member_capture_ids.push(capture_id);
            }
            Some(c.clone())
        } else {
            None
        };
        rewrite_jsonl(&self.path, &items)?;
        Ok(out)
    }

    pub fn end(&self, chain_id: Uuid) -> Result<Option<CaptureChain>> {
        let mut items = self.inner.lock().expect("chain lock");
        let out = if let Some(c) = items.iter_mut().find(|c| c.id == chain_id && c.is_open()) {
            c.ended_at = Some(Utc::now());
            Some(c.clone())
        } else {
            None
        };
        rewrite_jsonl(&self.path, &items)?;
        Ok(out)
    }

    pub fn list(&self) -> Vec<CaptureChain> {
        let items = self.inner.lock().expect("chain lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        v
    }

    pub fn active(&self) -> Vec<CaptureChain> {
        self.list().into_iter().filter(|c| c.is_open()).collect()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Ephemeral captures (I16 / N2.4)
// ---------------------------------------------------------------------------

/// A capture that self-destructs after `expires_at` unless promoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EphemeralItem {
    pub id: Uuid,
    pub content: String,
    pub source: String,
    pub captured_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub promoted: bool,
}

pub struct EphemeralStore {
    path: PathBuf,
    inner: Mutex<Vec<EphemeralItem>>,
}

impl EphemeralStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(EPHEMERAL_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Add an ephemeral capture. Default lifetime is 24h (§ N2.4).
    pub fn capture(
        &self,
        content: impl Into<String>,
        source: impl Into<String>,
        ttl_hours: i64,
    ) -> Result<EphemeralItem> {
        let now = Utc::now();
        let item = EphemeralItem {
            id: Uuid::new_v4(),
            content: content.into(),
            source: source.into(),
            captured_at: now,
            expires_at: now + Duration::hours(ttl_hours.max(1)),
            promoted: false,
        };
        let mut items = self.inner.lock().expect("ephemeral lock");
        items.push(item.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(item)
    }

    /// Promote an ephemeral to a permanent entry — caller is responsible
    /// for actually re-ingesting it through `IngestPipeline`.
    pub fn promote(&self, id: Uuid) -> Result<Option<EphemeralItem>> {
        let mut items = self.inner.lock().expect("ephemeral lock");
        let out = if let Some(item) = items.iter_mut().find(|i| i.id == id) {
            item.promoted = true;
            Some(item.clone())
        } else {
            None
        };
        rewrite_jsonl(&self.path, &items)?;
        Ok(out)
    }

    /// Drop every ephemeral whose `expires_at ≤ now` and which was not
    /// promoted. Returns the ids that were removed.
    pub fn sweep(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>> {
        let mut items = self.inner.lock().expect("ephemeral lock");
        let mut removed = Vec::new();
        items.retain(|i| {
            if !i.promoted && i.expires_at <= now {
                removed.push(i.id);
                false
            } else {
                true
            }
        });
        rewrite_jsonl(&self.path, &items)?;
        Ok(removed)
    }

    pub fn list(&self) -> Vec<EphemeralItem> {
        self.inner.lock().expect("ephemeral lock").clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Time-locked memories (I17 / N2.5)
// ---------------------------------------------------------------------------

/// A memory scheduled to become retrievable at `unlock_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimeLockedMemory {
    pub id: Uuid,
    /// The memory this record locks — often already stored, marked hidden.
    pub memory_id: Uuid,
    pub locked_at: DateTime<Utc>,
    pub unlock_at: DateTime<Utc>,
    pub note: String,
    pub unlocked: bool,
}

pub struct TimeLockStore {
    path: PathBuf,
    inner: Mutex<Vec<TimeLockedMemory>>,
}

impl TimeLockStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(TIME_LOCK_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn lock_until(
        &self,
        memory_id: Uuid,
        unlock_at: DateTime<Utc>,
        note: impl Into<String>,
    ) -> Result<TimeLockedMemory> {
        let entry = TimeLockedMemory {
            id: Uuid::new_v4(),
            memory_id,
            locked_at: Utc::now(),
            unlock_at,
            note: note.into(),
            unlocked: false,
        };
        let mut items = self.inner.lock().expect("timelock lock");
        items.push(entry.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(entry)
    }

    /// Sweep unlocked-due entries and mark them unlocked. Returns the
    /// memory ids that just became available.
    pub fn sweep(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>> {
        let mut items = self.inner.lock().expect("timelock lock");
        let mut unlocked = Vec::new();
        for entry in items.iter_mut() {
            if !entry.unlocked && entry.unlock_at <= now {
                entry.unlocked = true;
                unlocked.push(entry.memory_id);
            }
        }
        rewrite_jsonl(&self.path, &items)?;
        Ok(unlocked)
    }

    pub fn pending(&self) -> Vec<TimeLockedMemory> {
        self.inner
            .lock()
            .expect("timelock lock")
            .iter()
            .filter(|i| !i.unlocked)
            .cloned()
            .collect()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Contradiction-watch opt-in (I17 / N2.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContradictionWatchEntry {
    pub memory_id: Uuid,
    pub added_at: DateTime<Utc>,
    pub note: String,
}

pub struct ContradictionWatch {
    path: PathBuf,
    inner: Mutex<Vec<ContradictionWatchEntry>>,
}

impl ContradictionWatch {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(CONTRADICTION_WATCH_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn watch(&self, memory_id: Uuid, note: impl Into<String>) -> Result<ContradictionWatchEntry> {
        let mut items = self.inner.lock().expect("watch lock");
        if let Some(existing) = items.iter().find(|e| e.memory_id == memory_id) {
            return Ok(existing.clone());
        }
        let entry = ContradictionWatchEntry {
            memory_id,
            added_at: Utc::now(),
            note: note.into(),
        };
        items.push(entry.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(entry)
    }

    pub fn unwatch(&self, memory_id: Uuid) -> Result<bool> {
        let mut items = self.inner.lock().expect("watch lock");
        let before = items.len();
        items.retain(|e| e.memory_id != memory_id);
        let changed = items.len() != before;
        rewrite_jsonl(&self.path, &items)?;
        Ok(changed)
    }

    pub fn is_watched(&self, memory_id: Uuid) -> bool {
        self.inner
            .lock()
            .expect("watch lock")
            .iter()
            .any(|e| e.memory_id == memory_id)
    }

    pub fn list(&self) -> Vec<ContradictionWatchEntry> {
        self.inner.lock().expect("watch lock").clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Journal + debrief (I14)
// ---------------------------------------------------------------------------

/// A journal entry. `kind` distinguishes end-of-day journals from
/// post-meeting debriefs so the Brief can render them differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JournalEntry {
    pub id: Uuid,
    pub kind: JournalKind,
    pub at: DateTime<Utc>,
    pub prompt: String,
    pub body: String,
    pub linked_calendar_event: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    EndOfDay,
    PostMeeting,
    Freeform,
}

pub struct JournalStore {
    path: PathBuf,
    inner: Mutex<Vec<JournalEntry>>,
}

impl JournalStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(JOURNAL_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn write(
        &self,
        kind: JournalKind,
        prompt: impl Into<String>,
        body: impl Into<String>,
        linked_event: Option<String>,
    ) -> Result<JournalEntry> {
        let entry = JournalEntry {
            id: Uuid::new_v4(),
            kind,
            at: Utc::now(),
            prompt: prompt.into(),
            body: body.into(),
            linked_calendar_event: linked_event,
        };
        let mut items = self.inner.lock().expect("journal lock");
        items.push(entry.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(entry)
    }

    /// The N most recent entries (newest first).
    pub fn recent(&self, limit: usize) -> Vec<JournalEntry> {
        let items = self.inner.lock().expect("journal lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.at.cmp(&a.at));
        v.truncate(limit);
        v
    }

    pub fn entries_between(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<JournalEntry> {
        let items = self.inner.lock().expect("journal lock");
        items
            .iter()
            .filter(|e| e.at >= start && e.at < end)
            .cloned()
            .collect()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Retro-capture windows (I15 / N2.2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RetroWindow {
    pub id: Uuid,
    pub requested_at: DateTime<Utc>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub reprocess_prompt: String,
    /// Ids of captures re-derived under this window, populated by the
    /// re-processing job. Empty when the job has not yet run.
    pub reprocessed_capture_ids: Vec<Uuid>,
}

pub struct RetroWindowStore {
    path: PathBuf,
    inner: Mutex<Vec<RetroWindow>>,
}

impl RetroWindowStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(RETRO_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn request(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        prompt: impl Into<String>,
    ) -> Result<RetroWindow> {
        let w = RetroWindow {
            id: Uuid::new_v4(),
            requested_at: Utc::now(),
            start,
            end,
            reprocess_prompt: prompt.into(),
            reprocessed_capture_ids: Vec::new(),
        };
        let mut items = self.inner.lock().expect("retro lock");
        items.push(w.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(w)
    }

    pub fn record_reprocessed(&self, id: Uuid, capture_ids: Vec<Uuid>) -> Result<Option<RetroWindow>> {
        let mut items = self.inner.lock().expect("retro lock");
        let out = if let Some(w) = items.iter_mut().find(|w| w.id == id) {
            w.reprocessed_capture_ids = capture_ids;
            Some(w.clone())
        } else {
            None
        };
        rewrite_jsonl(&self.path, &items)?;
        Ok(out)
    }

    pub fn list(&self) -> Vec<RetroWindow> {
        let items = self.inner.lock().expect("retro lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.requested_at.cmp(&a.requested_at));
        v
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Weekly review checkpoints (I18 / N2.7)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WeeklyReview {
    pub id: Uuid,
    pub week_start: DateTime<Utc>,
    pub week_end: DateTime<Utc>,
    pub generated_at: DateTime<Utc>,
    pub kept: Vec<Uuid>,
    pub refined: Vec<Uuid>,
    pub forgotten: Vec<Uuid>,
    pub summary: String,
}

pub struct WeeklyReviewStore {
    path: PathBuf,
    inner: Mutex<Vec<WeeklyReview>>,
}

impl WeeklyReviewStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(io_err)?;
        let path = dir.join(WEEKLY_FILE_NAME);
        let inner = load_jsonl(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    pub fn record(
        &self,
        week_start: DateTime<Utc>,
        week_end: DateTime<Utc>,
        kept: Vec<Uuid>,
        refined: Vec<Uuid>,
        forgotten: Vec<Uuid>,
        summary: impl Into<String>,
    ) -> Result<WeeklyReview> {
        let review = WeeklyReview {
            id: Uuid::new_v4(),
            week_start,
            week_end,
            generated_at: Utc::now(),
            kept,
            refined,
            forgotten,
            summary: summary.into(),
        };
        let mut items = self.inner.lock().expect("weekly lock");
        items.push(review.clone());
        rewrite_jsonl(&self.path, &items)?;
        Ok(review)
    }

    pub fn list(&self) -> Vec<WeeklyReview> {
        let items = self.inner.lock().expect("weekly lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.generated_at.cmp(&a.generated_at));
        v
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn io_err(e: std::io::Error) -> TraceMindError {
    TraceMindError::Storage(e.to_string())
}

fn load_jsonl<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
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

fn rewrite_jsonl<T: Serialize>(path: &Path, items: &[T]) -> Result<()> {
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
        .map_err(io_err)?;
    file.write_all(buf.as_bytes()).map_err(io_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChainStore::open(dir.path()).unwrap();
        let chain = store.start("debug-build").unwrap();
        assert!(chain.is_open());

        let cap = Uuid::new_v4();
        let updated = store.add(chain.id, cap).unwrap().unwrap();
        assert_eq!(updated.member_capture_ids, vec![cap]);

        let ended = store.end(chain.id).unwrap().unwrap();
        assert!(ended.ended_at.is_some());
        assert_eq!(store.active().len(), 0);
    }

    #[test]
    fn chain_add_dedups_member_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChainStore::open(dir.path()).unwrap();
        let chain = store.start("x").unwrap();
        let cap = Uuid::new_v4();
        store.add(chain.id, cap).unwrap();
        let after = store.add(chain.id, cap).unwrap().unwrap();
        assert_eq!(after.member_capture_ids, vec![cap]);
    }

    #[test]
    fn ephemeral_sweep_removes_expired_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = EphemeralStore::open(dir.path()).unwrap();
        let a = store.capture("expiring", "clipboard", 1).unwrap();
        let _b = store.capture("live", "clipboard", 24).unwrap();
        let future = a.expires_at + Duration::minutes(1);
        let removed = store.sweep(future).unwrap();
        assert_eq!(removed, vec![a.id]);
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn ephemeral_promote_prevents_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let store = EphemeralStore::open(dir.path()).unwrap();
        let a = store.capture("keep me", "clipboard", 1).unwrap();
        store.promote(a.id).unwrap();
        let future = a.expires_at + Duration::hours(1);
        let removed = store.sweep(future).unwrap();
        assert!(removed.is_empty());
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn timelock_unlocks_only_after_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let store = TimeLockStore::open(dir.path()).unwrap();
        let now = Utc::now();
        let mem = Uuid::new_v4();
        store.lock_until(mem, now + Duration::days(30), "future note").unwrap();

        let unlocked_early = store.sweep(now).unwrap();
        assert!(unlocked_early.is_empty());
        assert_eq!(store.pending().len(), 1);

        let unlocked_late = store.sweep(now + Duration::days(31)).unwrap();
        assert_eq!(unlocked_late, vec![mem]);
        assert_eq!(store.pending().len(), 0);
    }

    #[test]
    fn contradiction_watch_add_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContradictionWatch::open(dir.path()).unwrap();
        let mem = Uuid::new_v4();
        store.watch(mem, "salary claim").unwrap();
        assert!(store.is_watched(mem));
        // Duplicate watch is a noop.
        store.watch(mem, "again").unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.unwatch(mem).unwrap());
        assert!(!store.is_watched(mem));
    }

    #[test]
    fn journal_write_and_recent() {
        let dir = tempfile::tempdir().unwrap();
        let store = JournalStore::open(dir.path()).unwrap();
        store.write(JournalKind::EndOfDay, "what happened?", "shipped X", None).unwrap();
        store.write(JournalKind::PostMeeting, "after standup", "unblock Sara", Some("evt-1".into())).unwrap();
        let recent = store.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].kind, JournalKind::PostMeeting);
    }

    #[test]
    fn journal_entries_between_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let store = JournalStore::open(dir.path()).unwrap();
        let e = store.write(JournalKind::Freeform, "p", "b", None).unwrap();
        let window = store.entries_between(e.at - Duration::seconds(1), e.at + Duration::seconds(1));
        assert_eq!(window.len(), 1);
        let empty = store.entries_between(e.at + Duration::hours(1), e.at + Duration::hours(2));
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn retro_window_records_and_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = RetroWindowStore::open(dir.path()).unwrap();
        let now = Utc::now();
        let w = store
            .request(now - Duration::hours(2), now, "extract action items")
            .unwrap();
        assert!(w.reprocessed_capture_ids.is_empty());
        let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let updated = store.record_reprocessed(w.id, ids.clone()).unwrap().unwrap();
        assert_eq!(updated.reprocessed_capture_ids, ids);
    }

    #[test]
    fn weekly_review_appends() {
        let dir = tempfile::tempdir().unwrap();
        let store = WeeklyReviewStore::open(dir.path()).unwrap();
        let now = Utc::now();
        let r = store
            .record(
                now - Duration::days(7),
                now,
                vec![Uuid::new_v4()],
                vec![],
                vec![],
                "shipped ingestion primitives",
            )
            .unwrap();
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.list()[0].id, r.id);
    }
}
