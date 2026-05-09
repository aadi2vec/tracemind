use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidTime {
    pub from: DateTime<Utc>,
    pub to: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionTime {
    pub recorded_at: DateTime<Utc>,
    pub superseded_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BitemporalFact<T> {
    pub fact: T,
    pub valid_time: ValidTime,
    pub tx_time: TransactionTime,
    pub fact_id: Uuid,
    pub supersedes: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporalQuery {
    pub as_of_valid: Option<DateTime<Utc>>,
    pub as_of_tx: Option<DateTime<Utc>>,
    pub entity_id: Option<Uuid>,
    pub range: Option<TimeRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalSnapshot<T> {
    pub facts: Vec<BitemporalFact<T>>,
    pub queried_at: DateTime<Utc>,
    pub as_of_valid: DateTime<Utc>,
    pub as_of_tx: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactChange<T> {
    pub old: Option<BitemporalFact<T>>,
    pub new: BitemporalFact<T>,
    pub change_type: ChangeType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Created,
    Updated,
    Retracted,
    Corrected,
}
