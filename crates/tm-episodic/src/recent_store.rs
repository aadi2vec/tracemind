//! Bounded ring buffer for recent capture events.
//!
//! Writes to `~/.tracemind/recent.jsonl` (configurable). Each call to
//! [`RecentStore::append`] appends a new line and then truncates the file
//! to the most recent `capacity` entries. Capture volume is low (human
//! clipboard + MCP calls), so rewriting the file per append is cheap.

use std::io::Write;
use std::path::PathBuf;

use tm_types::{RecentCapture, Result, TraceMindError};

/// Default ring capacity when the user does not override it.
pub const DEFAULT_CAPACITY: usize = 100;

pub struct RecentStore {
    path: PathBuf,
    capacity: usize,
}

impl RecentStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_with_capacity(path, DEFAULT_CAPACITY)
    }

    pub fn open_with_capacity(path: impl Into<PathBuf>, capacity: usize) -> Result<Self> {
        let capacity = capacity.max(1);
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(Self { path, capacity })
    }

    /// Append `event` and keep only the last `capacity` entries.
    pub fn append(&self, event: &RecentCapture) -> Result<()> {
        let existing = self.read_all()?;
        let mut events: Vec<RecentCapture> = existing;
        events.push(event.clone());
        let start = events.len().saturating_sub(self.capacity);
        let tail = &events[start..];

        let mut out = String::with_capacity(tail.len() * 128);
        for ev in tail {
            out.push_str(&serde_json::to_string(ev)?);
            out.push('\n');
        }

        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = std::fs::File::create(&tmp)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            file.write_all(out.as_bytes())
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            file.sync_all()
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Read the entire ring (skips malformed lines).
    pub fn read_all(&self) -> Result<Vec<RecentCapture>> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(TraceMindError::Storage(e.to_string())),
        };
        Ok(raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str::<RecentCapture>(line).ok())
            .collect())
    }

    /// Return the most recent `limit` entries, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<RecentCapture>> {
        let mut all = self.read_all()?;
        all.reverse();
        all.truncate(limit);
        Ok(all)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn tmp_path() -> PathBuf {
        let uid = Uuid::new_v4().to_string().replace('-', "");
        std::env::temp_dir().join(format!("tm_recent_{}.jsonl", uid))
    }

    #[test]
    fn ring_caps_at_capacity() {
        let path = tmp_path();
        let store = RecentStore::open_with_capacity(&path, 5).unwrap();
        for i in 0..12 {
            let ev = RecentCapture::new("clipboard", format!("hash-{i}"), &format!("text {i}"));
            store.append(&ev).unwrap();
        }
        let all = store.read_all().unwrap();
        assert_eq!(all.len(), 5, "file must be capped at capacity");
        // Should contain the last 5 hashes (hash-7 through hash-11) in order.
        let hashes: Vec<_> = all.iter().map(|e| e.content_hash.as_str()).collect();
        assert_eq!(hashes, vec!["hash-7", "hash-8", "hash-9", "hash-10", "hash-11"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recent_returns_newest_first() {
        let path = tmp_path();
        let store = RecentStore::open_with_capacity(&path, 10).unwrap();
        for i in 0..4 {
            let ev = RecentCapture::new("mcp", format!("h{i}"), "text");
            store.append(&ev).unwrap();
        }
        let recent = store.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content_hash, "h3");
        assert_eq!(recent[1].content_hash, "h2");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_all_is_ok_when_file_missing() {
        let path = tmp_path();
        // Do not create.
        let store = RecentStore::open(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(store.read_all().unwrap().len(), 0);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let path = tmp_path();
        let store = RecentStore::open_with_capacity(&path, 10).unwrap();
        let good = RecentCapture::new("clipboard", "h1", "text");
        store.append(&good).unwrap();

        // Corrupt-append a malformed line.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{not valid json").unwrap();
        drop(f);

        let all = store.read_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content_hash, "h1");
        std::fs::remove_file(&path).ok();
    }
}
