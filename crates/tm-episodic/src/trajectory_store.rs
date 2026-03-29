use std::io::Write;
use std::path::PathBuf;
use tm_types::{Result, Trajectory, TraceMindError};

pub struct TrajectoryStore {
    path: PathBuf,
}

impl TrajectoryStore {
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

    /// Serialize `trajectory` as a single JSON line and append it to the store.
    pub fn append(&self, trajectory: &Trajectory) -> Result<()> {
        let mut line = serde_json::to_string(trajectory)?;
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

    /// Read the whole file and return the last `limit` trajectories.
    pub fn recent(&self, limit: usize) -> Result<Vec<Trajectory>> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let trajectories: Vec<Trajectory> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str::<Trajectory>(line).map_err(TraceMindError::from))
            .collect::<Result<Vec<_>>>()?;
        let start = trajectories.len().saturating_sub(limit);
        Ok(trajectories[start..].to_vec())
    }

    /// Read the whole file and return the last `limit` trajectories where
    /// `is_training_ready()` is true (i.e. `actual_outcome_embedding` is Some).
    pub fn pending_training(&self, limit: usize) -> Result<Vec<Trajectory>> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let trajectories: Vec<Trajectory> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str::<Trajectory>(line).map_err(TraceMindError::from))
            .collect::<Result<Vec<_>>>()?;
        let ready: Vec<Trajectory> = trajectories
            .into_iter()
            .filter(|t| t.is_training_ready())
            .collect();
        let start = ready.len().saturating_sub(limit);
        Ok(ready[start..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_trajectory(with_outcome: bool) -> Trajectory {
        let session = Uuid::new_v4();
        let trace_id = Uuid::new_v4();
        let mut t = Trajectory::new(
            session,
            trace_id,
            0,
            1.0,
            5,
            42,
            vec![0.0_f32; 384],
            "testhash",
        );
        if with_outcome {
            t.actual_outcome_embedding = Some(vec![0.0_f32; 384]);
        }
        t
    }

    #[test]
    fn pending_training_returns_only_ready_trajectories() {
        let uid = Uuid::new_v4().to_string().replace('-', "");
        let path = std::env::temp_dir().join(format!("tm_test_{}.jsonl", uid));

        let store = TrajectoryStore::open(&path).expect("open store");

        // 2 without outcome, 3 with outcome.
        store.append(&make_trajectory(false)).expect("append");
        store.append(&make_trajectory(false)).expect("append");
        let mut ready_ids = Vec::new();
        for _ in 0..3 {
            let t = make_trajectory(true);
            ready_ids.push(t.id);
            store.append(&t).expect("append");
        }

        let pending = store.pending_training(10).expect("pending_training");
        assert_eq!(pending.len(), 3);

        let actual_ids: Vec<Uuid> = pending.iter().map(|t| t.id).collect();
        assert_eq!(actual_ids, ready_ids);

        // Cleanup.
        let _ = std::fs::remove_file(&path);
    }
}
