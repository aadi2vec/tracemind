//! Q4.12 — User-behavior model extension: verb affinity + temporal + host patterns.
//!
//! Extends the existing f_outcome logistic regression with new feature dimensions:
//! - verb_used: which MCP verb was invoked
//! - time_of_day: morning / afternoon / evening / night
//! - host_id: which AI host originated the signal
//!
//! New prediction target: verb_affinity_score — how much does this user lean on
//! each verb relative to median usage? A score > 1.0 means above-median usage;
//! < 1.0 means below-median.
//!
//! This drives L2 space weight personalization (charter Pillar 7): if the user
//! invokes `contradict` twice as often as median, the ComposedIndex weights the
//! contradiction space higher in their queries.

use std::collections::HashMap;
use chrono::{DateTime, Utc, Timelike};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Time-band classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBand {
    Morning,    // 06-12
    Afternoon,  // 12-18
    Evening,    // 18-22
    Night,      // 22-06
}

impl TimeBand {
    pub fn from_dt(dt: &DateTime<Utc>) -> Self {
        match dt.hour() {
            6..=11 => TimeBand::Morning,
            12..=17 => TimeBand::Afternoon,
            18..=21 => TimeBand::Evening,
            _ => TimeBand::Night,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TimeBand::Morning => "morning",
            TimeBand::Afternoon => "afternoon",
            TimeBand::Evening => "evening",
            TimeBand::Night => "night",
        }
    }
}

// ---------------------------------------------------------------------------
// Verb usage observation
// ---------------------------------------------------------------------------

/// One observed verb invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerbObservation {
    pub verb: String,
    pub host_id: Option<String>,
    pub time_band: TimeBand,
    pub session_id: Option<uuid::Uuid>,
    pub observed_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Verb affinity model
// ---------------------------------------------------------------------------

/// Per-verb affinity scores relative to the user's own baseline.
/// score = user_rate / (median_rate across all verbs).
/// A score > 1.0 means above-average usage for this user.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VerbAffinityModel {
    /// Total observations ingested.
    pub total_observations: usize,
    /// Raw counts per verb.
    pub verb_counts: HashMap<String, usize>,
    /// Counts per (host_id, verb) pair.
    pub host_verb_counts: HashMap<String, HashMap<String, usize>>,
    /// Counts per (time_band, verb) pair.
    pub timeband_verb_counts: HashMap<String, HashMap<String, usize>>,
    pub last_updated: Option<DateTime<Utc>>,
}

impl VerbAffinityModel {
    pub fn new() -> Self { Self::default() }

    /// Ingest a verb observation.
    pub fn observe(&mut self, obs: &VerbObservation) {
        self.total_observations += 1;
        *self.verb_counts.entry(obs.verb.clone()).or_insert(0) += 1;

        if let Some(ref host) = obs.host_id {
            *self.host_verb_counts
                .entry(host.clone())
                .or_default()
                .entry(obs.verb.clone())
                .or_insert(0) += 1;
        }

        *self.timeband_verb_counts
            .entry(obs.time_band.as_str().to_string())
            .or_default()
            .entry(obs.verb.clone())
            .or_insert(0) += 1;

        self.last_updated = Some(Utc::now());
    }

    /// Compute affinity scores: verb → score relative to uniform baseline (1/N verbs).
    /// Returns sorted (verb, score) pairs, highest affinity first.
    /// A score > 1.0 means above-average usage for this verb.
    pub fn affinity_scores(&self) -> Vec<(String, f64)> {
        if self.total_observations == 0 || self.verb_counts.is_empty() {
            return vec![];
        }

        let n_verbs = self.verb_counts.len() as f64;
        // Uniform baseline: every verb equally likely → rate = 1/n_verbs
        let uniform_rate = 1.0 / n_verbs;
        let total = self.total_observations as f64;

        let mut scores: Vec<(String, f64)> = self.verb_counts
            .iter()
            .map(|(verb, &count)| {
                let rate = count as f64 / total;
                // Score relative to uniform: how much more (or less) than average?
                let score = if uniform_rate > 0.0 { rate / uniform_rate } else { 1.0 };
                (verb.clone(), score)
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }

    /// Top N verbs by affinity score.
    pub fn top_verbs(&self, n: usize) -> Vec<(String, f64)> {
        self.affinity_scores().into_iter().take(n).collect()
    }

    /// Host-specific affinity: which verbs does this user prefer on a given host?
    pub fn host_affinity(&self, host_id: &str) -> Vec<(String, f64)> {
        let host_counts = match self.host_verb_counts.get(host_id) {
            Some(c) => c,
            None => return vec![],
        };
        let total = host_counts.values().sum::<usize>() as f64;
        if total == 0.0 { return vec![]; }

        let mut scores: Vec<(String, f64)> = host_counts
            .iter()
            .map(|(verb, &count)| (verb.clone(), count as f64 / total))
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }

    /// Suggest ComposedIndex space weight adjustments based on verb affinity.
    /// Returns (space_name, weight_multiplier) pairs.
    pub fn space_weight_adjustments(&self) -> Vec<(String, f32)> {
        let affinities = self.affinity_scores();
        let mut adjustments = Vec::new();

        for (verb, score) in &affinities {
            let multiplier = score.min(3.0) as f32; // cap at 3× to prevent extreme skew
            let space = match verb.as_str() {
                "memory_contradict" | "contradict" => Some("confidence"),
                "memory_reflect" | "reflect" => Some("recency"),
                "memory_query" | "recall" => Some("text"),
                "memory_pin" | "plan" => Some("recency"),
                _ => None,
            };
            if let Some(space_name) = space {
                adjustments.push((space_name.to_string(), multiplier));
            }
        }

        adjustments
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Save the verb affinity model to a JSON file.
pub fn save(model: &VerbAffinityModel, path: &std::path::Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(model)?;
    std::fs::write(path, json)
}

/// Load the verb affinity model from a JSON file.
pub fn load(path: &std::path::Path) -> std::io::Result<VerbAffinityModel> {
    let json = std::fs::read_to_string(path)?;
    serde_json::from_str(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn default_path(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("verb_affinity_model.json")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(verb: &str, host: Option<&str>, band: TimeBand) -> VerbObservation {
        VerbObservation {
            verb: verb.to_string(),
            host_id: host.map(str::to_string),
            time_band: band,
            session_id: None,
            observed_at: Utc::now(),
        }
    }

    #[test]
    fn empty_model_has_no_scores() {
        let m = VerbAffinityModel::new();
        assert!(m.affinity_scores().is_empty());
    }

    #[test]
    fn high_use_verb_has_high_affinity() {
        let mut m = VerbAffinityModel::new();
        for _ in 0..10 {
            m.observe(&obs("contradict", None, TimeBand::Morning));
        }
        for _ in 0..2 {
            m.observe(&obs("recall", None, TimeBand::Afternoon));
        }
        let scores = m.affinity_scores();
        let contradict_score = scores.iter().find(|(v, _)| v == "contradict").map(|(_, s)| *s);
        assert!(contradict_score.unwrap_or(0.0) > 1.0, "contradict should be above median");
    }

    #[test]
    fn host_affinity_isolated() {
        let mut m = VerbAffinityModel::new();
        m.observe(&obs("contradict", Some("claude-code"), TimeBand::Morning));
        m.observe(&obs("recall", Some("goose"), TimeBand::Afternoon));
        let cc_scores = m.host_affinity("claude-code");
        assert_eq!(cc_scores.len(), 1);
        assert_eq!(cc_scores[0].0, "contradict");
    }

    #[test]
    fn space_adjustments_for_contradict_heavy_user() {
        let mut m = VerbAffinityModel::new();
        for _ in 0..8 { m.observe(&obs("memory_contradict", None, TimeBand::Night)); }
        for _ in 0..2 { m.observe(&obs("memory_query", None, TimeBand::Morning)); }
        let adj = m.space_weight_adjustments();
        let confidence_adj = adj.iter().find(|(s, _)| s == "confidence").map(|(_, w)| *w);
        assert!(confidence_adj.unwrap_or(0.0) > 1.0, "contradict heavy → confidence space boost");
    }

    #[test]
    fn time_band_from_hour() {
        use chrono::{NaiveDateTime, TimeZone};
        let morning = Utc.from_utc_datetime(&NaiveDateTime::parse_from_str("2026-07-21 09:00:00", "%Y-%m-%d %H:%M:%S").unwrap());
        let night   = Utc.from_utc_datetime(&NaiveDateTime::parse_from_str("2026-07-21 23:00:00", "%Y-%m-%d %H:%M:%S").unwrap());
        assert_eq!(TimeBand::from_dt(&morning), TimeBand::Morning);
        assert_eq!(TimeBand::from_dt(&night),   TimeBand::Night);
    }
}
