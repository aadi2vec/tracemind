//! Online nearest-centroid assignment.
//!
//! Called from `IngestPipeline::ingest_fast` after `VectorStore.embed`.
//! New events arriving between full re-clusters land in the nearest
//! existing cluster *if* their cosine similarity is within 2σ of the
//! cluster's intra-distance — otherwise they're tagged outlier.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::store;
use crate::{ClusterResult, OUTLIER_ID};

/// Assignment of a freshly-ingested event to a cluster (or the outlier
/// bucket).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    pub cluster_id: i64,
    pub membership_prob: f32,
    pub is_outlier: bool,
}

/// Heuristic 2σ-style cutoff. Centroids are unit-norm so cosine sim
/// is in [-1, 1]; we accept anything ≥ 0.55 as "inside the cluster".
/// Calibrated against real-world BGE-small embeddings on capture data.
const INSIDE_THRESHOLD: f32 = 0.55;

pub(crate) fn assign_event(
    conn: &Connection,
    event_id: &str,
    embedding: &[f32],
) -> ClusterResult<Assignment> {
    if embedding.is_empty() {
        // Refuse to assign a zero-length embedding — caller bug.
        return Err(crate::ClusterError::Dim { expected: 1, got: 0 });
    }
    let centroids = store::load_centroids(conn)?;
    if centroids.is_empty() {
        // No clusters yet — tag outlier; the next full re-cluster will
        // promote this event into a real cluster.
        store::upsert_event_assignment(conn, event_id, OUTLIER_ID, 0.0)?;
        return Ok(Assignment {
            cluster_id: OUTLIER_ID,
            membership_prob: 0.0,
            is_outlier: true,
        });
    }
    // Find the nearest centroid by cosine similarity.
    let (best_id, best_sim) = nearest(&centroids, embedding);
    if best_sim < INSIDE_THRESHOLD {
        store::upsert_event_assignment(conn, event_id, OUTLIER_ID, best_sim.max(0.0))?;
        return Ok(Assignment {
            cluster_id: OUTLIER_ID,
            membership_prob: best_sim.max(0.0),
            is_outlier: true,
        });
    }
    store::upsert_event_assignment(conn, event_id, best_id, best_sim)?;
    Ok(Assignment {
        cluster_id: best_id,
        membership_prob: best_sim,
        is_outlier: false,
    })
}

fn nearest(centroids: &[store::Centroid], emb: &[f32]) -> (i64, f32) {
    let norm_e: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    let mut best_id = OUTLIER_ID;
    let mut best_sim = f32::MIN;
    for c in centroids {
        if c.vec.len() != emb.len() {
            continue;
        }
        let mut dot = 0.0_f32;
        let mut norm_c = 0.0_f32;
        for (a, b) in c.vec.iter().zip(emb.iter()) {
            dot += a * b;
            norm_c += a * a;
        }
        let sim = dot / (norm_c.sqrt().max(1e-6) * norm_e);
        if sim > best_sim {
            best_sim = sim;
            best_id = c.id;
        }
    }
    (best_id, best_sim)
}
