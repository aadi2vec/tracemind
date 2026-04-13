use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use tm_types::{Result, Trajectory, TrajectoryOutcome, TraceMindError};

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

    /// Tag a trajectory's outcome (success/failure) for contrastive storage.
    pub fn tag_outcome(&self, trajectory_id: uuid::Uuid, outcome: TrajectoryOutcome, query_class: Option<&str>) -> Result<()> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let mut trajectories: Vec<Trajectory> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        for t in &mut trajectories {
            if t.id == trajectory_id {
                t.outcome = outcome;
                if let Some(qc) = query_class {
                    t.query_class = Some(qc.to_string());
                }
            }
        }

        // Rewrite the file
        let mut file = std::fs::File::create(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        for t in &trajectories {
            let mut line = serde_json::to_string(t)?;
            line.push('\n');
            file.write_all(line.as_bytes())
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    /// MIA-inspired contrastive consolidation: for each query class, keep only
    /// the shortest successful trajectory + one random failed trajectory.
    /// Returns (kept, pruned) counts.
    pub fn consolidate_contrastive(&self) -> Result<(usize, usize)> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let trajectories: Vec<Trajectory> = raw
            .split('\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        let total = trajectories.len();

        // Group by query_class
        let mut by_class: HashMap<String, Vec<Trajectory>> = HashMap::new();
        let mut unclassed: Vec<Trajectory> = Vec::new();

        for t in trajectories {
            if let Some(ref qc) = t.query_class {
                by_class.entry(qc.clone()).or_default().push(t);
            } else {
                unclassed.push(t);
            }
        }

        let mut kept: Vec<Trajectory> = Vec::new();

        // For each class: shortest success + random failure
        for (_class, class_trajectories) in &by_class {
            let successes: Vec<&Trajectory> = class_trajectories.iter()
                .filter(|t| t.outcome == TrajectoryOutcome::Success)
                .collect();
            let failures: Vec<&Trajectory> = class_trajectories.iter()
                .filter(|t| t.outcome == TrajectoryOutcome::Failure)
                .collect();

            // Keep shortest success (by n_results as proxy for path length)
            if let Some(best) = successes.iter().min_by_key(|t| t.n_results) {
                kept.push((*best).clone());
            }

            // Keep one failure (most recent as "random" sample)
            if let Some(fail) = failures.last() {
                kept.push((*fail).clone());
            }

            // Keep all Unknown (not yet tagged)
            for t in class_trajectories {
                if t.outcome == TrajectoryOutcome::Unknown {
                    kept.push(t.clone());
                }
            }
        }

        // Keep all unclassed trajectories
        kept.extend(unclassed);

        let pruned = total.saturating_sub(kept.len());

        // Rewrite
        let mut file = std::fs::File::create(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        for t in &kept {
            let mut line = serde_json::to_string(t)?;
            line.push('\n');
            file.write_all(line.as_bytes())
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }

        Ok((kept.len(), pruned))
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
