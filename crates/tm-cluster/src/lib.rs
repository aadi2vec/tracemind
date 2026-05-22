//! `tm-cluster` — HDBSCAN-first event clustering substrate.
//!
//! Indexes the same capture-event stream as `tm-graph` and `tm-vector`
//! from a third angle: *topical/temporal*. Same SQLite file. CPU-only.
//! No hardcoded `k` (HDBSCAN-first; KMeans rejected per P4a constraint).
//!
//! ## Public surface
//!
//! * [`Clusterer::open`] — open / create the cluster tables in the
//!   given SQLite file (same DB as `tm-graph` and `tm-vector`).
//! * [`Clusterer::assign`] — online nearest-centroid assignment for a
//!   newly-ingested event. Adds ≤ 2 ms to the ingest path.
//! * [`Clusterer::recluster`] — full HDBSCAN pass on the accumulated
//!   embeddings. Background task; never blocks the request path.
//! * [`Clusterer::recent_centroids`] — top centroids in a time window;
//!   feeds the WME L1 topic vector.
//! * [`Clusterer::outliers`] — events that didn't fit any cluster.
//!   First-class signal — feeds the "Anticipate" verb and the
//!   "Unsorted" tray.
//!
//! ## SQLite schema
//!
//! Two tables, additive to the existing `memory.db`:
//!
//! ```text
//! clusters(
//!     id              INTEGER PRIMARY KEY,
//!     label_text      TEXT,          -- c-TF-IDF auto-label (CLU-6)
//!     centroid_blob   BLOB NOT NULL, -- f32 LE bytes
//!     dim             INTEGER NOT NULL,
//!     n_members       INTEGER NOT NULL,
//!     persistence     REAL NOT NULL, -- HDBSCAN cluster persistence
//!     is_outlier      INTEGER NOT NULL DEFAULT 0,
//!     updated_at      INTEGER NOT NULL
//! )
//! event_clusters(
//!     event_id        TEXT PRIMARY KEY,
//!     cluster_id      INTEGER NOT NULL,  -- -1 for outliers
//!     membership_prob REAL NOT NULL,
//!     assigned_at     INTEGER NOT NULL
//! )
//! ```
//!
//! ## Outlier convention
//!
//! `cluster_id = -1` denotes outliers (consistent with HDBSCAN's noise
//! label). Outliers are **first-class signal**, not noise — they feed
//! the Anticipate verb and the user-facing "Unsorted" tray.

pub mod assign;
pub mod hdbscan_impl;
pub mod store;

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use crate::assign::Assignment;
pub use crate::store::{Centroid, ClusterRow};

/// Default minimum cluster size for HDBSCAN. Five is the smallest size
/// where a cluster reads as a "topic" rather than a coincidence on real
/// capture streams.
pub const DEFAULT_MIN_CLUSTER_SIZE: usize = 5;

/// Default minimum samples for HDBSCAN — tightens cluster boundaries
/// when the embedding space is noisy.
pub const DEFAULT_MIN_SAMPLES: usize = 3;

/// Sentinel for the outlier bucket. Matches HDBSCAN's noise label.
pub const OUTLIER_ID: i64 = -1;

/// Errors surfaced from the clusterer.
#[derive(Debug, Error)]
pub enum ClusterError {
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("hdbscan error: {0}")]
    Hdbscan(String),
    #[error("invalid embedding dim: expected {expected}, got {got}")]
    Dim { expected: usize, got: usize },
    #[error("insufficient samples for re-cluster: need ≥ {needed}, got {got}")]
    InsufficientSamples { needed: usize, got: usize },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type ClusterResult<T> = std::result::Result<T, ClusterError>;

/// Configuration for the clusterer.
#[derive(Debug, Clone)]
pub struct ClustererConfig {
    pub min_cluster_size: usize,
    pub min_samples: usize,
    /// Re-cluster every N new events (or whichever of `recluster_every_n`
    /// / `recluster_every` triggers first).
    pub recluster_every_n: usize,
    /// Re-cluster every `recluster_every` seconds.
    pub recluster_every: Duration,
}

impl Default for ClustererConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: DEFAULT_MIN_CLUSTER_SIZE,
            min_samples: DEFAULT_MIN_SAMPLES,
            recluster_every_n: 100,
            recluster_every: Duration::minutes(10),
        }
    }
}

/// One row of the recent-centroid feed. Feeds the WME L1 topic vector
/// (EMA of these, decayed by recency).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentCentroid {
    pub cluster_id: i64,
    pub centroid: Vec<f32>,
    pub recency_weight: f32,
    pub n_members: usize,
    pub updated_at: DateTime<Utc>,
}

/// Stats returned from a full re-cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterStats {
    pub n_clusters: usize,
    pub n_outliers: usize,
    pub n_assigned: usize,
    pub dim: usize,
    pub took_ms: u64,
}

/// The main clusterer. Holds an open SQLite connection wrapped in a
/// `Mutex` so it can be shared safely across the ingest fast path and
/// the background re-cluster thread.
pub struct Clusterer {
    db_path: std::path::PathBuf,
    conn: Mutex<Connection>,
    config: ClustererConfig,
}

impl Clusterer {
    /// Open (or create) the clusterer-backing tables in the given SQLite
    /// file. Idempotent.
    pub fn open(db_path: impl AsRef<Path>) -> ClusterResult<Self> {
        Self::open_with(db_path, ClustererConfig::default())
    }

    pub fn open_with(
        db_path: impl AsRef<Path>,
        config: ClustererConfig,
    ) -> ClusterResult<Self> {
        let path = db_path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        store::ensure_schema(&conn)?;
        Ok(Self {
            db_path: path,
            conn: Mutex::new(conn),
            config,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Online nearest-centroid assignment for a freshly-ingested event.
    /// Called from `IngestPipeline::ingest_fast` after `VectorStore.embed`.
    /// Adds ≤ 2 ms on a laptop (target enforced by PERF-1 / PERF-2).
    pub fn assign(&self, event_id: &str, embedding: &[f32]) -> ClusterResult<Assignment> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        assign::assign_event(&conn, event_id, embedding)
    }

    /// Full HDBSCAN pass. Background work; never call from the ingest
    /// path. Reads every (event_id, embedding) pair from `event_clusters`
    /// + the caller-provided embedding source, runs HDBSCAN, persists
    /// the resulting clusters and per-event assignments.
    pub fn recluster<I>(&self, events: I) -> ClusterResult<ClusterStats>
    where
        I: IntoIterator<Item = (String, Vec<f32>)>,
    {
        let started = std::time::Instant::now();
        let pairs: Vec<(String, Vec<f32>)> = events.into_iter().collect();
        if pairs.len() < self.config.min_cluster_size {
            return Err(ClusterError::InsufficientSamples {
                needed: self.config.min_cluster_size,
                got: pairs.len(),
            });
        }
        let dim = pairs[0].1.len();
        for (idx, (_, emb)) in pairs.iter().enumerate() {
            if emb.len() != dim {
                return Err(ClusterError::Dim {
                    expected: dim,
                    got: emb.len(),
                });
            }
            if emb.is_empty() {
                return Err(ClusterError::Dim {
                    expected: 1,
                    got: 0,
                });
            }
            let _ = idx;
        }
        let labels_with_prob = hdbscan_impl::run(
            pairs.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
            self.config.min_cluster_size,
            self.config.min_samples,
        )?;
        let mut conn = self.conn.lock().expect("cluster mutex poisoned");
        let stats = store::persist_recluster(&mut conn, &pairs, &labels_with_prob, dim)?;
        let took_ms = started.elapsed().as_millis() as u64;
        Ok(ClusterStats { took_ms, ..stats })
    }

    /// Top centroids in a recent window, weighted by recency. The
    /// `recency_weight` field is `exp(-age_secs / half_life_secs)`
    /// (half-life = window / 2 — so the head of the window weighs ~1
    /// and the tail weighs ~0.25).
    pub fn recent_centroids(&self, window: Duration) -> ClusterResult<Vec<RecentCentroid>> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        store::recent_centroids(&conn, window)
    }

    /// Events that landed in the outlier bucket (`cluster_id = -1`)
    /// within the given window. Feeds the WME Anticipate verb.
    pub fn outliers(&self, window: Duration) -> ClusterResult<Vec<String>> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        store::outliers(&conn, window)
    }

    /// Read a single event's assignment, if any.
    pub fn assignment_for(&self, event_id: &str) -> ClusterResult<Option<Assignment>> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        store::assignment_for(&conn, event_id)
    }

    /// All current clusters (excluding the outlier bucket).
    pub fn clusters(&self) -> ClusterResult<Vec<ClusterRow>> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        store::list_clusters(&conn)
    }

    /// Persist a c-TF-IDF label for the given cluster. Called from the
    /// labeler in `tm-graph::labeler` after a re-cluster.
    pub fn set_label(&self, cluster_id: i64, label: &str) -> ClusterResult<()> {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        store::set_label(&conn, cluster_id, label)
    }

    /// CLU-6 — relabel every (non-outlier) cluster using c-TF-IDF over
    /// its member event texts. Caller supplies a closure that resolves
    /// `event_id -> Option<String>`. Returns the number of clusters
    /// successfully relabelled.
    pub fn relabel_with<F>(&self, mut text_for: F) -> ClusterResult<usize>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let texts_by_cluster = self.cluster_texts(&mut text_for)?;
        if texts_by_cluster.is_empty() {
            return Ok(0);
        }
        let cfg = tm_graph::labeler::LabelerConfig::default();
        let labels = tm_graph::labeler::label_clusters(&texts_by_cluster, cfg);
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        let mut updated = 0_usize;
        for (cid, lbl) in labels.iter() {
            if lbl.terms.is_empty() {
                continue;
            }
            store::set_label(&conn, *cid, &lbl.label)?;
            updated += 1;
        }
        Ok(updated)
    }

    /// Texts for each cluster's member events. Caller supplies a
    /// `text_for` closure that resolves an event_id to its text. Used
    /// by the c-TF-IDF labeler.
    pub fn cluster_texts<F>(&self, mut text_for: F) -> ClusterResult<std::collections::HashMap<i64, Vec<String>>>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let conn = self.conn.lock().expect("cluster mutex poisoned");
        let mut out: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT cluster_id, event_id FROM event_clusters WHERE cluster_id != ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![OUTLIER_ID], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows {
            let (cid, eid) = r?;
            if let Some(text) = text_for(&eid) {
                out.entry(cid).or_default().push(text);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_embed(seed: u64) -> Vec<f32> {
        let mut v = vec![0.0f32; 8];
        let mut s = seed.wrapping_mul(2654435761);
        for x in v.iter_mut() {
            s = s.wrapping_mul(2654435761);
            *x = ((s as i32) as f32) / (i32::MAX as f32);
        }
        // L2-normalise
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in v.iter_mut() {
            *x /= n.max(1e-6);
        }
        v
    }

    #[test]
    fn open_creates_schema() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("m.db");
        let c = Clusterer::open(&db).unwrap();
        assert_eq!(c.clusters().unwrap().len(), 0);
    }

    #[test]
    fn assign_returns_outlier_when_no_clusters() {
        let dir = tempdir().unwrap();
        let c = Clusterer::open(dir.path().join("m.db")).unwrap();
        let a = c.assign("e1", &make_embed(1)).unwrap();
        assert_eq!(a.cluster_id, OUTLIER_ID);
    }

    #[test]
    fn recluster_groups_repeated_vectors() {
        let dir = tempdir().unwrap();
        let c = Clusterer::open(dir.path().join("m.db")).unwrap();
        // Two clusters of 6 each, well-separated
        let mut pairs = Vec::new();
        // cluster A — small perturbations around make_embed(1)
        let base_a = make_embed(1);
        for i in 0..6 {
            let mut v = base_a.clone();
            v[0] += 0.001 * i as f32;
            pairs.push((format!("a{i}"), v));
        }
        // cluster B
        let base_b: Vec<f32> = base_a.iter().map(|x| -x).collect();
        for i in 0..6 {
            let mut v = base_b.clone();
            v[0] += 0.001 * i as f32;
            pairs.push((format!("b{i}"), v));
        }
        let stats = c.recluster(pairs.into_iter()).unwrap();
        // HDBSCAN may also produce outliers; we only assert that at least
        // one real cluster was formed.
        assert!(stats.n_clusters >= 1, "got {:?}", stats);
        assert_eq!(c.clusters().unwrap().len(), stats.n_clusters);
    }
}
