//! LGS-1 — Salience as a first-class layer.
//!
//! Investor framing (2026-05-15): "what we know" lives in `kg_entities`
//! / `kg_relations`; "what we care about right now" is a separate
//! concern. Previously freshness + confidence were per-row fields on
//! the relations table; promoting them to their own keyed index lets
//! WME L2 candidate weighting, Memory Garden node sizing, and
//! deny-list expiry all read from one place.
//!
//! ## Schema
//!
//! ```text
//! salience(
//!     node_id          INTEGER PRIMARY KEY,  -- kg_entities.id
//!     score            REAL NOT NULL,        -- composite, in [0, 1]
//!     last_touched     INTEGER NOT NULL,     -- unix seconds
//!     recency_decay    REAL NOT NULL,        -- 0..1 (older → 0)
//!     importance       REAL NOT NULL,        -- 0..1 (degree-derived)
//!     updated_at       INTEGER NOT NULL
//! )
//! ```
//!
//! ## Scoring
//!
//! `score = 0.5 * recency_decay + 0.5 * importance`.
//!
//! * **recency_decay** = `exp(-age_days / 14)` where `age_days` is the
//!   gap from now to the most recent triple touching the node. Half-life
//!   = 14 days, so a node not touched for two weeks ≈ 0.5, four weeks
//!   ≈ 0.25, etc.
//! * **importance** = `tanh(degree / 8)` where `degree` is the count of
//!   `kg_relations` rows mentioning the node (in or out). tanh keeps the
//!   value in `[0, 1)` and saturates around 8 connections.
//!
//! Recomputed nightly by `consolidate`. Cheap enough (a single SQL
//! scan + N upserts) that it can also be triggered on-demand.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sqlite_knowledge_graph::KnowledgeGraph;
use tm_types::{Result, TraceMindError};

/// Half-life for the recency decay. 14 days.
pub const RECENCY_HALF_LIFE_DAYS: f64 = 14.0;

/// Importance saturation: degree above this saturates `tanh(degree / SATURATION_DEGREE)`.
pub const SATURATION_DEGREE: f64 = 8.0;

/// One row of the `salience` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalienceRow {
    pub node_id: i64,
    pub score: f64,
    pub last_touched: DateTime<Utc>,
    pub recency_decay: f64,
    pub importance: f64,
    pub updated_at: DateTime<Utc>,
}

/// Summary stats from a full recompute pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalienceStats {
    pub n_nodes: usize,
    pub n_touched_recent: usize,
    pub mean_score: f64,
    pub max_score: f64,
}

pub fn ensure_schema(kg: &KnowledgeGraph) -> Result<()> {
    let conn = kg.connection();
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS salience (
            node_id        INTEGER PRIMARY KEY,
            score          REAL NOT NULL,
            last_touched   INTEGER NOT NULL,
            recency_decay  REAL NOT NULL,
            importance     REAL NOT NULL,
            updated_at     INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_salience_score ON salience(score DESC);
        "#,
    )
    .map_err(|e| TraceMindError::Storage(format!("salience schema: {e}")))?;
    Ok(())
}

/// Recompute salience for every entity in the live graph. Runs over
/// every `kg_relations` row to compute degree + last_touched per node.
pub fn recompute(kg: &KnowledgeGraph) -> Result<SalienceStats> {
    ensure_schema(kg)?;
    let conn = kg.connection();

    // 1. Load every entity id.
    let mut node_stmt = conn
        .prepare("SELECT id FROM kg_entities")
        .map_err(|e| TraceMindError::Storage(format!("salience node prep: {e}")))?;
    let node_ids: Vec<i64> = node_stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| TraceMindError::Storage(format!("salience node q: {e}")))?
        .filter_map(|r| r.ok())
        .collect();
    drop(node_stmt);
    if node_ids.is_empty() {
        return Ok(SalienceStats {
            n_nodes: 0,
            n_touched_recent: 0,
            mean_score: 0.0,
            max_score: 0.0,
        });
    }

    // 2. Walk kg_relations to derive (degree, last_touched) per entity.
    let mut degree: HashMap<i64, u32> = HashMap::new();
    let mut last_touched: HashMap<i64, i64> = HashMap::new();

    let mut rel_stmt = conn
        .prepare("SELECT properties FROM kg_relations")
        .map_err(|e| TraceMindError::Storage(format!("salience rel prep: {e}")))?;
    let rows = rel_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| TraceMindError::Storage(format!("salience rel q: {e}")))?;
    for r in rows {
        let props = match r {
            Ok(s) => s,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&props) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let src = v.get("source_entity_id").and_then(|x| x.as_i64());
        let tgt = v.get("target_entity_id").and_then(|x| x.as_i64());
        let touched: Option<i64> = v
            .get("ingested_at")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp());
        for opt in [src, tgt] {
            if let Some(id) = opt {
                *degree.entry(id).or_insert(0) += 1;
                if let Some(ts) = touched {
                    let cur = last_touched.entry(id).or_insert(0);
                    if ts > *cur {
                        *cur = ts;
                    }
                }
            }
        }
    }
    drop(rel_stmt);

    // 3. Compute score per node + upsert.
    let now = Utc::now();
    let now_ts = now.timestamp();
    let half_life_secs = RECENCY_HALF_LIFE_DAYS * 86_400.0;

    let mut stmt_up = conn
        .prepare(
            "INSERT INTO salience(node_id, score, last_touched, recency_decay, importance, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(node_id) DO UPDATE SET \
                 score = excluded.score, \
                 last_touched = excluded.last_touched, \
                 recency_decay = excluded.recency_decay, \
                 importance = excluded.importance, \
                 updated_at = excluded.updated_at",
        )
        .map_err(|e| TraceMindError::Storage(format!("salience upsert prep: {e}")))?;

    let mut sum_score = 0.0_f64;
    let mut max_score = 0.0_f64;
    let mut n_touched_recent = 0_usize;
    let seven_days_secs: i64 = 7 * 86_400;

    for &id in node_ids.iter() {
        let deg = *degree.get(&id).unwrap_or(&0) as f64;
        let last_ts = *last_touched.get(&id).unwrap_or(&now_ts);
        let age_secs = (now_ts - last_ts).max(0) as f64;
        let recency_decay = (-age_secs * std::f64::consts::LN_2 / half_life_secs)
            .exp()
            .clamp(0.0, 1.0);
        let importance = (deg / SATURATION_DEGREE).tanh().clamp(0.0, 1.0);
        let score = 0.5 * recency_decay + 0.5 * importance;
        if now_ts - last_ts <= seven_days_secs {
            n_touched_recent += 1;
        }
        sum_score += score;
        if score > max_score {
            max_score = score;
        }
        stmt_up
            .execute(params![id, score, last_ts, recency_decay, importance, now_ts])
            .map_err(|e| TraceMindError::Storage(format!("salience upsert: {e}")))?;
    }

    let mean_score = if node_ids.is_empty() {
        0.0
    } else {
        sum_score / node_ids.len() as f64
    };
    Ok(SalienceStats {
        n_nodes: node_ids.len(),
        n_touched_recent,
        mean_score,
        max_score,
    })
}

/// Read the salience row for a single node.
pub fn score_for(kg: &KnowledgeGraph, node_id: i64) -> Result<Option<SalienceRow>> {
    let conn = kg.connection();
    let row = conn
        .query_row(
            "SELECT node_id, score, last_touched, recency_decay, importance, updated_at \
             FROM salience WHERE node_id = ?1",
            params![node_id],
            |row| {
                let last_ts: i64 = row.get(2)?;
                let updated_ts: i64 = row.get(5)?;
                Ok(SalienceRow {
                    node_id: row.get(0)?,
                    score: row.get(1)?,
                    last_touched: DateTime::from_timestamp(last_ts, 0).unwrap_or_else(Utc::now),
                    recency_decay: row.get(3)?,
                    importance: row.get(4)?,
                    updated_at: DateTime::from_timestamp(updated_ts, 0).unwrap_or_else(Utc::now),
                })
            },
        )
        .ok();
    Ok(row)
}

/// Top-K most salient nodes. Used by the Memory Garden and WME L2.
pub fn top_k(kg: &KnowledgeGraph, k: usize) -> Result<Vec<SalienceRow>> {
    let conn = kg.connection();
    let mut stmt = conn
        .prepare(
            "SELECT node_id, score, last_touched, recency_decay, importance, updated_at \
             FROM salience ORDER BY score DESC LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(format!("salience top_k prep: {e}")))?;
    let rows = stmt
        .query_map(params![k as i64], |row| {
            let last_ts: i64 = row.get(2)?;
            let updated_ts: i64 = row.get(5)?;
            Ok(SalienceRow {
                node_id: row.get(0)?,
                score: row.get(1)?,
                last_touched: DateTime::from_timestamp(last_ts, 0).unwrap_or_else(Utc::now),
                recency_decay: row.get(3)?,
                importance: row.get(4)?,
                updated_at: DateTime::from_timestamp(updated_ts, 0).unwrap_or_else(Utc::now),
            })
        })
        .map_err(|e| TraceMindError::Storage(format!("salience top_k q: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        match r {
            Ok(v) => out.push(v),
            Err(_) => continue,
        }
    }
    Ok(out)
}

/// Test helper — recency decay calculation in isolation.
pub fn recency_decay_at_age(age: Duration) -> f64 {
    let age_secs = age.num_seconds().max(0) as f64;
    let half_life_secs = RECENCY_HALF_LIFE_DAYS * 86_400.0;
    (-age_secs * std::f64::consts::LN_2 / half_life_secs)
        .exp()
        .clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_decay_half_at_14_days() {
        let d = recency_decay_at_age(Duration::days(14));
        assert!((d - 0.5).abs() < 0.01, "expected ~0.5, got {d}");
    }

    #[test]
    fn recency_decay_one_at_zero() {
        assert!((recency_decay_at_age(Duration::zero()) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recency_decay_low_after_eight_weeks() {
        let d = recency_decay_at_age(Duration::days(8 * 7));
        assert!(d < 0.1, "expected < 0.1 after 8 weeks, got {d}");
    }
}
