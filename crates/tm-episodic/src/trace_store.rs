use std::io::Write;
use std::path::PathBuf;
use tm_types::{Result, Trace, TraceMindError};

pub struct TraceStore {
    path: PathBuf,
}

impl TraceStore {
    /// Opens (or creates) the JSONL store at `path`, creating parent dirs as needed.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        // Touch the file so it exists from the start.
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(Self { path })
    }

    /// Serialize `trace` as a single JSON line and append it to the store.
    pub fn append(&self, trace: &Trace) -> Result<()> {
        let mut line = serde_json::to_string(trace)?;
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

    /// Read the whole file and return the last `limit` traces.
    pub fn recent(&self, limit: usize) -> Result<Vec<Trace>> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let traces: Vec<Trace> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str::<Trace>(line).map_err(TraceMindError::from))
            .collect::<Result<Vec<_>>>()?;
        let start = traces.len().saturating_sub(limit);
        Ok(traces[start..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::{Trace, TraceEventType};
    use uuid::Uuid;

    #[test]
    fn recent_returns_last_n_traces() {
        let uid = Uuid::new_v4().to_string().replace('-', "");
        let path = std::env::temp_dir().join(format!("tm_test_{}.jsonl", uid));

        let store = TraceStore::open(&path).expect("open store");

        let session = Uuid::new_v4();
        let mut appended: Vec<Trace> = Vec::new();
        for _ in 0..5 {
            let trace = Trace::new(session, TraceEventType::Ingest, "hash");
            store.append(&trace).expect("append");
            appended.push(trace);
        }

        let recent = store.recent(3).expect("recent");
        assert_eq!(recent.len(), 3);

        let expected_ids: Vec<Uuid> = appended[2..].iter().map(|t| t.id).collect();
        let actual_ids: Vec<Uuid> = recent.iter().map(|t| t.id).collect();
        assert_eq!(actual_ids, expected_ids);

        // Cleanup.
        let _ = std::fs::remove_file(&path);
    }
}
