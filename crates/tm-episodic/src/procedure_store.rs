use std::io::Write;
use std::path::PathBuf;
use tm_types::{Procedure, ProcedureStatus, Result, TraceMindError};
use uuid::Uuid;

/// JSONL-backed store for procedures.
pub struct ProcedureStore {
    path: PathBuf,
}

impl ProcedureStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
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

    /// Append a procedure to the store.
    pub fn save(&self, proc: &Procedure) -> Result<()> {
        let mut line = serde_json::to_string(proc)?;
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

    /// Load all procedures, deduplicating by id (last write wins).
    pub fn load_all(&self) -> Result<Vec<Procedure>> {
        let raw = std::fs::read_to_string(&self.path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut map = std::collections::HashMap::new();
        for line in raw.split('\n').filter(|l| !l.is_empty()) {
            let proc: Procedure = serde_json::from_str(line).map_err(TraceMindError::from)?;
            map.insert(proc.id, proc);
        }

        let mut procs: Vec<Procedure> = map.into_values().collect();
        procs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(procs)
    }

    /// Get a procedure by id.
    pub fn get(&self, id: Uuid) -> Result<Option<Procedure>> {
        let all = self.load_all()?;
        Ok(all.into_iter().find(|p| p.id == id))
    }

    /// Get a procedure by name (latest version).
    pub fn get_by_name(&self, name: &str) -> Result<Option<Procedure>> {
        let all = self.load_all()?;
        Ok(all.into_iter().filter(|p| p.name == name).last())
    }

    /// List active (non-deprecated) procedures.
    pub fn list_active(&self) -> Result<Vec<Procedure>> {
        let all = self.load_all()?;
        Ok(all
            .into_iter()
            .filter(|p| p.status != ProcedureStatus::Deprecated)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Dry-run executor
// ---------------------------------------------------------------------------

/// Dry-run a procedure: prints each step without executing.
/// Returns the list of step descriptions for testing.
pub fn dry_run(proc: &Procedure) -> Vec<String> {
    let mut output = Vec::new();

    let header = format!(
        "=== Dry-run: {} (v{}, {:?}) ===",
        proc.name, proc.version, proc.status
    );
    output.push(header);

    for step in &proc.steps {
        let mut line = format!("  [{}] {}", step.ordinal, step.action);
        if let Some(desc) = &step.description {
            line.push_str(&format!(" — {desc}"));
        }
        if let Some(cmd) = &step.executable {
            line.push_str(&format!("  $ {cmd}"));
        }
        output.push(line);
    }

    let footer = format!("=== {} steps total ===", proc.steps.len());
    output.push(footer);

    output
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::ProcedureStep;

    fn make_proc(name: &str) -> Procedure {
        Procedure::new(
            name,
            "test procedure",
            vec![
                ProcedureStep::new(1, "step one"),
                ProcedureStep::new(2, "step two"),
            ],
        )
    }

    #[test]
    fn save_and_load_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "tm_proc_{}.jsonl",
            Uuid::new_v4().to_string().replace('-', "")
        ));
        let store = ProcedureStore::open(&path).unwrap();

        let p1 = make_proc("deploy");
        let p2 = make_proc("rollback");
        store.save(&p1).unwrap();
        store.save(&p2).unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 2);

        let found = store.get(p1.id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "deploy");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn get_by_name_returns_latest() {
        let path = std::env::temp_dir().join(format!(
            "tm_proc_{}.jsonl",
            Uuid::new_v4().to_string().replace('-', "")
        ));
        let store = ProcedureStore::open(&path).unwrap();

        let p1 = make_proc("deploy");
        let p2 = p1.revise(vec![ProcedureStep::new(1, "new step")]);
        store.save(&p1).unwrap();
        store.save(&p2).unwrap();

        let found = store.get_by_name("deploy").unwrap().unwrap();
        assert_eq!(found.version, 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn upsert_deduplicates_by_id() {
        let path = std::env::temp_dir().join(format!(
            "tm_proc_{}.jsonl",
            Uuid::new_v4().to_string().replace('-', "")
        ));
        let store = ProcedureStore::open(&path).unwrap();

        let mut p = make_proc("deploy");
        store.save(&p).unwrap();

        p.record_success();
        store.save(&p).unwrap();

        let all = store.load_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].success_count, 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn list_active_excludes_deprecated() {
        let path = std::env::temp_dir().join(format!(
            "tm_proc_{}.jsonl",
            Uuid::new_v4().to_string().replace('-', "")
        ));
        let store = ProcedureStore::open(&path).unwrap();

        let p1 = make_proc("deploy");
        let mut p2 = make_proc("old");
        p2.deprecate();
        store.save(&p1).unwrap();
        store.save(&p2).unwrap();

        let active = store.list_active().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "deploy");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dry_run_prints_all_steps() {
        let mut p = make_proc("deploy");
        p.steps[0].executable = Some("cargo build --release".into());
        p.steps[1].description = Some("push to registry".into());

        let output = dry_run(&p);
        assert_eq!(output.len(), 4); // header + 2 steps + footer
        assert!(output[0].contains("deploy"));
        assert!(output[1].contains("cargo build"));
        assert!(output[2].contains("push to registry"));
        assert!(output[3].contains("2 steps"));
    }
}
