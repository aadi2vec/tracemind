//! Append-only writer for [`CaptureReceipt`] records.
//!
//! Every capture event MUST produce a receipt. The store writes one JSON
//! line per receipt to `~/.tracemind/receipts.jsonl`, mirroring the pattern
//! used by [`tm_episodic::TraceStore`].
//!
//! The receipt log is a first-class audit surface: the Ingestion Review
//! view (plan §3.3) reads it directly, and third-party auditors can grep
//! the file without a TraceMind binary.

use std::io::Write;
use std::path::{Path, PathBuf};

use tm_types::{CaptureReceipt, Result, TraceMindError};
use uuid::Uuid;

/// File name written under the data directory.
pub const RECEIPTS_FILE_NAME: &str = "receipts.jsonl";

/// Append-only JSONL store for capture receipts.
pub struct ReceiptStore {
    path: PathBuf,
}

impl ReceiptStore {
    /// Open (or create) the receipts log inside `dir`. Creates the parent
    /// directory as needed and touches the file so it exists from the start.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(RECEIPTS_FILE_NAME);
        Self::open_path(path)
    }

    /// Open at an exact path. Useful for tests and for callers that keep
    /// the receipt log outside the standard data directory.
    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
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
        Ok(Self { path })
    }

    /// Append a single receipt as one JSON line.
    pub fn append(&self, receipt: &CaptureReceipt) -> Result<()> {
        let mut line = serde_json::to_string(receipt)?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Return the last `limit` receipts in file order (oldest first among
    /// the returned slice, newest last — matches [`TraceStore::recent`]).
    pub fn recent(&self, limit: usize) -> Result<Vec<CaptureReceipt>> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(TraceMindError::Storage(e.to_string())),
        };
        let mut receipts: Vec<CaptureReceipt> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str::<CaptureReceipt>(line).ok())
            .collect();
        let start = receipts.len().saturating_sub(limit);
        Ok(receipts.drain(start..).collect())
    }

    /// Find a single receipt by capture id. Returns `None` when no receipt
    /// with that id has been logged (either because it never was, or because
    /// the JSON line is malformed — malformed lines are skipped silently to
    /// match every other JSONL reader in the workspace).
    pub fn by_capture_id(&self, id: Uuid) -> Result<Option<CaptureReceipt>> {
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TraceMindError::Storage(e.to_string())),
        };
        for line in raw.split('\n') {
            if line.is_empty() {
                continue;
            }
            if let Ok(r) = serde_json::from_str::<CaptureReceipt>(line) {
                if r.capture_id == id {
                    return Ok(Some(r));
                }
            }
        }
        Ok(None)
    }

    /// Path this store writes to (useful for diagnostics / tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::MemoryTier;

    fn tmp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn sample(source: &str) -> CaptureReceipt {
        CaptureReceipt::new(source, "text", 42, MemoryTier::Quarantine, "unit test")
    }

    #[test]
    fn append_and_read_back_one_receipt() {
        let dir = tmp_dir();
        let store = ReceiptStore::open(dir.path()).unwrap();
        let receipt = sample("clipboard");
        store.append(&receipt).unwrap();

        let read = store.recent(10).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], receipt);
    }

    #[test]
    fn recent_returns_last_n_only() {
        let dir = tmp_dir();
        let store = ReceiptStore::open(dir.path()).unwrap();
        let mut all = Vec::new();
        for i in 0..7 {
            let r = sample(&format!("src-{i}"));
            store.append(&r).unwrap();
            all.push(r);
        }
        let read = store.recent(3).unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read, all[4..].to_vec());
    }

    #[test]
    fn by_capture_id_finds_specific_receipt() {
        let dir = tmp_dir();
        let store = ReceiptStore::open(dir.path()).unwrap();
        let a = sample("a");
        let b = sample("b");
        let c = sample("c");
        store.append(&a).unwrap();
        store.append(&b).unwrap();
        store.append(&c).unwrap();

        let found = store.by_capture_id(b.capture_id).unwrap();
        assert_eq!(found, Some(b));

        let missing = store.by_capture_id(Uuid::new_v4()).unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let dir = tmp_dir();
        // Do not open — call recent on a non-existent path.
        let store = ReceiptStore {
            path: dir.path().join("does_not_exist.jsonl"),
        };
        assert_eq!(store.recent(10).unwrap().len(), 0);
        assert_eq!(store.by_capture_id(Uuid::new_v4()).unwrap(), None);
    }

    #[test]
    fn append_survives_across_reopens() {
        let dir = tmp_dir();
        {
            let s = ReceiptStore::open(dir.path()).unwrap();
            s.append(&sample("s1")).unwrap();
        }
        {
            let s = ReceiptStore::open(dir.path()).unwrap();
            s.append(&sample("s2")).unwrap();
            assert_eq!(s.recent(10).unwrap().len(), 2);
        }
    }

    #[test]
    fn malformed_line_is_skipped() {
        let dir = tmp_dir();
        let store = ReceiptStore::open(dir.path()).unwrap();
        let good = sample("clipboard");
        store.append(&good).unwrap();

        // Corrupt-append a garbage line.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .unwrap();
        writeln!(f, "{{not valid json").unwrap();
        drop(f);

        let read = store.recent(10).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], good);
    }
}
