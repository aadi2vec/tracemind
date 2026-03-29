use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A versioned, executable sequence of steps for accomplishing a task.
/// Linked to entities via HAS_PROCEDURE triples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Procedure {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub version: u32,
    pub status: ProcedureStatus,
    pub steps: Vec<ProcedureStep>,
    pub success_count: u32,
    pub failure_count: u32,
    pub confidence: f64,
    pub parent_id: Option<Uuid>,   // previous version, if revised
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Procedure {
    pub fn new(name: impl Into<String>, description: impl Into<String>, steps: Vec<ProcedureStep>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: description.into(),
            version: 1,
            status: ProcedureStatus::Active,
            steps,
            success_count: 0,
            failure_count: 0,
            confidence: 1.0,
            parent_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn record_success(&mut self) {
        self.success_count += 1;
        let total = (self.success_count + self.failure_count) as f64;
        self.confidence = self.success_count as f64 / total;
        if self.confidence >= 0.8 {
            self.status = ProcedureStatus::Reinforced;
        }
        self.updated_at = Utc::now();
    }

    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        let total = (self.success_count + self.failure_count) as f64;
        self.confidence = self.success_count as f64 / total;
        if self.confidence < 0.3 {
            self.status = ProcedureStatus::Degraded;
        }
        self.updated_at = Utc::now();
    }

    pub fn deprecate(&mut self) {
        self.status = ProcedureStatus::Deprecated;
        self.updated_at = Utc::now();
    }

    /// Create a new version of this procedure with updated steps.
    pub fn revise(&self, steps: Vec<ProcedureStep>) -> Procedure {
        let now = Utc::now();
        Procedure {
            id: Uuid::new_v4(),
            name: self.name.clone(),
            description: self.description.clone(),
            version: self.version + 1,
            status: ProcedureStatus::Active,
            steps,
            success_count: 0,
            failure_count: 0,
            confidence: 1.0,
            parent_id: Some(self.id),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    Active,
    Reinforced,
    Degraded,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureStep {
    pub ordinal: u32,
    pub action: String,
    pub description: Option<String>,
    /// Shell command or tool call to execute in live mode. None = manual step.
    pub executable: Option<String>,
    pub expected_output: Option<String>,
}

impl ProcedureStep {
    pub fn new(ordinal: u32, action: impl Into<String>) -> Self {
        Self {
            ordinal,
            action: action.into(),
            description: None,
            executable: None,
            expected_output: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_proc() -> Procedure {
        Procedure::new("deploy", "Deploy the app", vec![
            ProcedureStep::new(1, "build"),
            ProcedureStep::new(2, "push"),
        ])
    }

    #[test]
    fn reinforced_after_successes() {
        let mut p = make_proc();
        for _ in 0..8 { p.record_success(); }
        for _ in 0..2 { p.record_failure(); }
        assert_eq!(p.status, ProcedureStatus::Reinforced);
    }

    #[test]
    fn degraded_after_failures() {
        let mut p = make_proc();
        for _ in 0..1 { p.record_success(); }
        for _ in 0..9 { p.record_failure(); }
        assert_eq!(p.status, ProcedureStatus::Degraded);
    }

    #[test]
    fn revise_bumps_version() {
        let p = make_proc();
        let p2 = p.revise(vec![ProcedureStep::new(1, "new step")]);
        assert_eq!(p2.version, 2);
        assert_eq!(p2.parent_id, Some(p.id));
    }
}
