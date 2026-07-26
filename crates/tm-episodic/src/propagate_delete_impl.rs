//! `PropagateDelete` for the episodic stores.
//!
//! Two impls live here:
//!
//! * [`TraceStore`] — the immutable audit log. Rows are **preserved on
//!   purpose** (S1 in the plan says every capture leaves an auditable
//!   record). The impl reports `rows_removed: 0` with a note so the
//!   coordinator can surface the intent to callers.
//!
//! * [`RecentStore`] — the ring buffer of recent captures. We rewrite the
//!   file dropping any line whose `content_hash` matches the memory. The
//!   ring buffer keys captures by content hash, not uuid, so callers must
//!   pass the same hex-encoded uuid we recorded on ingest (matches the
//!   convention used by `tm-ingest::pipeline` for `RecentCapture::from_memory`).

use tm_types::{PropagateDelete, PropagateReport, Result, TraceMindError};
use uuid::Uuid;

use crate::recent_store::RecentStore;
use crate::trace_store::TraceStore;

impl PropagateDelete for TraceStore {
    fn propagate_delete(&self, _memory_id: Uuid) -> Result<PropagateReport> {
        Ok(PropagateReport::new("tm-episodic:trace", 0)
            .with_note("immutable audit log preserved"))
    }
}

impl PropagateDelete for RecentStore {
    fn propagate_delete(&self, memory_id: Uuid) -> Result<PropagateReport> {
        let needle = memory_id.to_string();
        let all = self.read_all()?;
        let before = all.len();
        let kept: Vec<_> = all
            .into_iter()
            .filter(|c| {
                // Drop rows whose content_hash equals the memory id.
                // Callers that use a different mapping (e.g. hash of text)
                // can still call `propagate_delete` safely — it will simply
                // report zero rows removed.
                c.content_hash != needle
            })
            .collect();
        let removed = before - kept.len();
        if removed == 0 {
            return Ok(PropagateReport::new("tm-episodic:recent", 0));
        }

        // Rewrite the file atomically via the same tmp-rename dance the
        // ring buffer uses for its append path.
        use std::io::Write;
        let path = self.path().to_path_buf();
        let mut out = String::with_capacity(kept.len() * 128);
        for ev in &kept {
            out.push_str(&serde_json::to_string(ev)?);
            out.push('\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        {
            let mut file = std::fs::File::create(&tmp)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            file.write_all(out.as_bytes())
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            file.sync_all()
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        std::fs::rename(&tmp, &path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        Ok(PropagateReport::new("tm-episodic:recent", removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::RecentCapture;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let uid = Uuid::new_v4().to_string().replace('-', "");
        std::env::temp_dir().join(format!("tm_{name}_{uid}.jsonl"))
    }

    #[test]
    fn trace_store_reports_zero_with_note() {
        let path = tmp_path("trace");
        let store = TraceStore::open(&path).unwrap();
        let report = store.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.rows_removed, 0);
        assert_eq!(
            report.note.as_deref(),
            Some("immutable audit log preserved")
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recent_store_drops_matching_hash() {
        let path = tmp_path("recent");
        let store = RecentStore::open_with_capacity(&path, 10).unwrap();
        let target = Uuid::new_v4();
        let a = RecentCapture::new("clipboard", target.to_string(), "keep");
        let b = RecentCapture::new("clipboard", "other-hash", "keep");
        let c = RecentCapture::new("clipboard", target.to_string(), "keep");
        store.append(&a).unwrap();
        store.append(&b).unwrap();
        store.append(&c).unwrap();

        let report = store.propagate_delete(target).unwrap();
        assert_eq!(report.rows_removed, 2);

        let after = store.read_all().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].content_hash, "other-hash");

        // Second call is a no-op.
        let again = store.propagate_delete(target).unwrap();
        assert_eq!(again.rows_removed, 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn recent_store_unknown_id_is_noop() {
        let path = tmp_path("recent_noop");
        let store = RecentStore::open_with_capacity(&path, 10).unwrap();
        store
            .append(&RecentCapture::new("clipboard", "aaa", "text"))
            .unwrap();
        let report = store.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.rows_removed, 0);
        assert_eq!(store.read_all().unwrap().len(), 1);
        std::fs::remove_file(&path).ok();
    }
}
