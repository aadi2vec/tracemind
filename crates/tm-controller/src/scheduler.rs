//! Q4.4 — Nightly self-improvement scheduler.
//! Runs on-device (no cloud). Called by the CLI `tracemind nightly` command.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NightlyRunRecord {
    pub id: uuid::Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub gepa_spike_delta_f1: Option<f32>,
    pub verb_affinity_updated: bool,
    pub tier_cycle_ran: bool,
    pub contradiction_rate: Option<f64>,
    /// How many times the retraction beat fired since the product was
    /// installed — read from `retractions.jsonl`. The wedge's real usage
    /// signal (holistic review §5 P1.4 / §6a).
    #[serde(default)]
    pub retractions_fired: Option<usize>,
    pub error: Option<String>,
}

pub struct NightlyScheduler {
    pub data_dir: PathBuf,
}

impl NightlyScheduler {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Run the nightly tasks this crate can do on its own, and return a
    /// record with the real signals it could gather.
    ///
    /// Previously this returned hardcoded `true`s — "reports success without
    /// doing work", the exact anti-pattern the holistic review flags. It now
    /// reports only what it actually measured. Richer signals that need the
    /// graph (contradiction rate) are filled in by the caller
    /// (`tracemind nightly`), which has access to it; see
    /// [`NightlyRunRecord`].
    pub fn run(&self) -> NightlyRunRecord {
        NightlyRunRecord {
            id: uuid::Uuid::new_v4(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            gepa_spike_delta_f1: None,
            // Honest defaults: not attempted here. The caller flips these to
            // true only when the corresponding work actually ran.
            verb_affinity_updated: false,
            tier_cycle_ran: false,
            contradiction_rate: None,
            retractions_fired: self.count_retractions(),
            error: None,
        }
    }

    /// Count retraction-beat firings from `retractions.jsonl`. `None` when
    /// the log does not exist yet (the beat has never fired).
    pub fn count_retractions(&self) -> Option<usize> {
        let path = self.data_dir.join("retractions.jsonl");
        let raw = std::fs::read_to_string(&path).ok()?;
        Some(raw.lines().filter(|l| !l.trim().is_empty()).count())
    }

    /// Load past nightly run records from JSONL.
    pub fn history(&self, limit: usize) -> Vec<NightlyRunRecord> {
        let path = self.data_dir.join("nightly_runs.jsonl");
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let mut records: Vec<NightlyRunRecord> = raw
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let s = records.len().saturating_sub(limit);
        records.drain(s..).collect()
    }

    /// Append a run record to the history JSONL.
    pub fn record(&self, run: &NightlyRunRecord) -> std::io::Result<()> {
        use std::io::Write;
        let path = self.data_dir.join("nightly_runs.jsonl");
        let mut line = serde_json::to_string(run)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)?;
        f.write_all(line.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn run_returns_valid_record() {
        let dir = TempDir::new().unwrap();
        let scheduler = NightlyScheduler::new(dir.path().to_path_buf());
        let record = scheduler.run();

        assert!(record.completed_at.is_some());
        // The scheduler no longer fabricates success — these are false
        // until the caller actually runs the corresponding work.
        assert!(!record.verb_affinity_updated);
        assert!(!record.tier_cycle_ran);
        assert!(record.error.is_none());
        // No retraction log in a fresh dir.
        assert!(record.retractions_fired.is_none());
        assert!(record.started_at <= record.completed_at.unwrap());
    }

    #[test]
    fn history_is_empty_for_new_dir() {
        let dir = TempDir::new().unwrap();
        let scheduler = NightlyScheduler::new(dir.path().to_path_buf());
        let history = scheduler.history(100);
        assert!(history.is_empty());
    }

    #[test]
    fn record_and_history_roundtrip() {
        let dir = TempDir::new().unwrap();
        let scheduler = NightlyScheduler::new(dir.path().to_path_buf());

        let record = scheduler.run();
        let id = record.id;
        scheduler.record(&record).unwrap();

        let history = scheduler.history(10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, id);
    }

    #[test]
    fn counts_real_retractions_from_the_log() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("retractions.jsonl"),
            "{\"at\":\"t\",\"contradictions\":1}\n{\"at\":\"t\",\"contradictions\":2}\n",
        )
        .unwrap();
        let scheduler = NightlyScheduler::new(dir.path().to_path_buf());
        assert_eq!(scheduler.run().retractions_fired, Some(2));
    }
}
