//! SQLite persistence for `tm-cluster`.
//!
//! Two tables, additive to `memory.db`:
//!
//! ```text
//! clusters(id, label_text, centroid_blob, dim, n_members,
//!          persistence, is_outlier, updated_at)
//! event_clusters(event_id, cluster_id, membership_prob, assigned_at)
//! ```

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::assign::Assignment;
use crate::{ClusterError, ClusterResult, RecentCentroid, OUTLIER_ID};

/// One row of the `clusters` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterRow {
    pub id: i64,
    pub label: Option<String>,
    pub centroid: Vec<f32>,
    pub n_members: usize,
    pub persistence: f32,
    pub updated_at: DateTime<Utc>,
}

/// Compact centroid type used by callers that don't need the full row.
#[derive(Debug, Clone)]
pub struct Centroid {
    pub id: i64,
    pub vec: Vec<f32>,
    pub n_members: usize,
}

pub(crate) fn ensure_schema(conn: &Connection) -> ClusterResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS clusters (
            id              INTEGER PRIMARY KEY,
            label_text      TEXT,
            centroid_blob   BLOB NOT NULL,
            dim             INTEGER NOT NULL,
            n_members       INTEGER NOT NULL,
            persistence     REAL NOT NULL,
            is_outlier      INTEGER NOT NULL DEFAULT 0,
            updated_at      INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS event_clusters (
            event_id        TEXT PRIMARY KEY,
            cluster_id      INTEGER NOT NULL,
            membership_prob REAL NOT NULL,
            assigned_at     INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_event_clusters_cluster
            ON event_clusters(cluster_id);
        CREATE INDEX IF NOT EXISTS idx_event_clusters_assigned
            ON event_clusters(assigned_at);
        "#,
    )?;
    Ok(())
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(b.len() / 4);
    for chunk in b.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    out
}

pub(crate) fn list_clusters(conn: &Connection) -> ClusterResult<Vec<ClusterRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, label_text, centroid_blob, n_members, persistence, updated_at \
         FROM clusters WHERE is_outlier = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        let blob: Vec<u8> = row.get(2)?;
        let ts: i64 = row.get(5)?;
        Ok(ClusterRow {
            id: row.get(0)?,
            label: row.get(1)?,
            centroid: blob_to_vec(&blob),
            n_members: row.get::<_, i64>(3)? as usize,
            persistence: row.get::<_, f64>(4)? as f32,
            updated_at: DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now),
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub(crate) fn load_centroids(conn: &Connection) -> ClusterResult<Vec<Centroid>> {
    let mut stmt = conn.prepare(
        "SELECT id, centroid_blob, n_members \
         FROM clusters WHERE is_outlier = 0",
    )?;
    let rows = stmt.query_map([], |row| {
        let blob: Vec<u8> = row.get(1)?;
        Ok(Centroid {
            id: row.get(0)?,
            vec: blob_to_vec(&blob),
            n_members: row.get::<_, i64>(2)? as usize,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub(crate) fn upsert_event_assignment(
    conn: &Connection,
    event_id: &str,
    cluster_id: i64,
    membership_prob: f32,
) -> ClusterResult<()> {
    let now = Utc::now().timestamp();
    conn.execute(
        "INSERT INTO event_clusters(event_id, cluster_id, membership_prob, assigned_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(event_id) DO UPDATE SET \
             cluster_id = excluded.cluster_id, \
             membership_prob = excluded.membership_prob, \
             assigned_at = excluded.assigned_at",
        params![event_id, cluster_id, membership_prob as f64, now],
    )?;
    Ok(())
}

pub(crate) fn assignment_for(
    conn: &Connection,
    event_id: &str,
) -> ClusterResult<Option<Assignment>> {
    let mut stmt = conn.prepare(
        "SELECT cluster_id, membership_prob FROM event_clusters WHERE event_id = ?1",
    )?;
    let mut rows = stmt.query(params![event_id])?;
    if let Some(row) = rows.next()? {
        let cluster_id: i64 = row.get(0)?;
        let prob: f64 = row.get(1)?;
        Ok(Some(Assignment {
            cluster_id,
            membership_prob: prob as f32,
            is_outlier: cluster_id == OUTLIER_ID,
        }))
    } else {
        Ok(None)
    }
}

pub(crate) fn outliers(conn: &Connection, window: Duration) -> ClusterResult<Vec<String>> {
    let cutoff = (Utc::now() - window).timestamp();
    let mut stmt = conn.prepare(
        "SELECT event_id FROM event_clusters WHERE cluster_id = ?1 AND assigned_at >= ?2 \
         ORDER BY assigned_at DESC",
    )?;
    let rows = stmt.query_map(params![OUTLIER_ID, cutoff], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub(crate) fn recent_centroids(
    conn: &Connection,
    window: Duration,
) -> ClusterResult<Vec<RecentCentroid>> {
    let window_secs = window.num_seconds().max(1);
    let half_life = (window_secs / 2).max(1) as f32;
    let cutoff = (Utc::now() - window).timestamp();
    // Pull cluster ids that had any activity in the window.
    let mut stmt = conn.prepare(
        "SELECT cluster_id, MAX(assigned_at) as last_seen \
         FROM event_clusters \
         WHERE cluster_id != ?1 AND assigned_at >= ?2 \
         GROUP BY cluster_id \
         ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map(params![OUTLIER_ID, cutoff], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut hits: Vec<(i64, i64)> = Vec::new();
    for r in rows {
        hits.push(r?);
    }
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    // Join with cluster rows for centroid + size.
    let now_ts = Utc::now().timestamp();
    let mut out = Vec::with_capacity(hits.len());
    for (cid, last_seen) in hits {
        let mut stmt2 = conn.prepare(
            "SELECT centroid_blob, n_members, updated_at FROM clusters WHERE id = ?1",
        )?;
        let row = stmt2.query_row(params![cid], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        });
        match row {
            Ok((blob, n_members, updated_at)) => {
                let age = (now_ts - last_seen).max(0) as f32;
                let recency_weight = (-age / half_life).exp();
                out.push(RecentCentroid {
                    cluster_id: cid,
                    centroid: blob_to_vec(&blob),
                    recency_weight,
                    n_members: n_members as usize,
                    updated_at: DateTime::from_timestamp(updated_at, 0).unwrap_or_else(Utc::now),
                });
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(e) => return Err(ClusterError::Sql(e)),
        }
    }
    Ok(out)
}

pub(crate) fn set_label(conn: &Connection, cluster_id: i64, label: &str) -> ClusterResult<()> {
    conn.execute(
        "UPDATE clusters SET label_text = ?1 WHERE id = ?2",
        params![label, cluster_id],
    )?;
    Ok(())
}

/// Persist a full HDBSCAN pass. Wipes existing clusters and reinserts.
/// `labels_with_prob[i]` corresponds to `pairs[i]`. Labels of `-1` are
/// outliers (`OUTLIER_ID`).
pub(crate) fn persist_recluster(
    conn: &mut Connection,
    pairs: &[(String, Vec<f32>)],
    labels_with_prob: &[(i64, f32, f32)], // (label, membership_prob, persistence_or_0)
    dim: usize,
) -> ClusterResult<crate::ClusterStats> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM clusters", [])?;
    tx.execute("DELETE FROM event_clusters", [])?;

    // Group embeddings by cluster id.
    let mut by_cluster: HashMap<i64, (Vec<usize>, f32)> = HashMap::new();
    for (i, (label, _, persistence)) in labels_with_prob.iter().enumerate() {
        let e = by_cluster.entry(*label).or_insert_with(|| (Vec::new(), 0.0));
        e.0.push(i);
        // Persistence is per-cluster so just take the max we see.
        if *persistence > e.1 {
            e.1 = *persistence;
        }
    }

    let now = Utc::now().timestamp();
    let mut n_clusters = 0;
    let mut n_outliers = 0;
    for (cluster_id, (indices, persistence)) in &by_cluster {
        if *cluster_id == OUTLIER_ID {
            n_outliers += indices.len();
            // Outlier centroid is the zero vector — never used; included
            // for schema completeness so foreign-key style joins work.
            let zero = vec![0.0f32; dim];
            tx.execute(
                "INSERT INTO clusters(id, label_text, centroid_blob, dim, n_members, persistence, is_outlier, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7) \
                 ON CONFLICT(id) DO UPDATE SET n_members = excluded.n_members, updated_at = excluded.updated_at",
                params![
                    *cluster_id,
                    "unsorted".to_string(),
                    vec_to_blob(&zero),
                    dim as i64,
                    indices.len() as i64,
                    0.0_f64,
                    now,
                ],
            )?;
            continue;
        }
        // Compute centroid = mean of member embeddings, L2-normalised.
        let mut centroid = vec![0.0f32; dim];
        for idx in indices {
            for (j, x) in pairs[*idx].1.iter().enumerate() {
                centroid[j] += x;
            }
        }
        for v in centroid.iter_mut() {
            *v /= indices.len() as f32;
        }
        let norm: f32 = centroid.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-6 {
            for v in centroid.iter_mut() {
                *v /= norm;
            }
        }
        tx.execute(
            "INSERT INTO clusters(id, label_text, centroid_blob, dim, n_members, persistence, is_outlier, updated_at) \
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, 0, ?6)",
            params![
                *cluster_id,
                vec_to_blob(&centroid),
                dim as i64,
                indices.len() as i64,
                *persistence as f64,
                now,
            ],
        )?;
        n_clusters += 1;
    }

    // Insert per-event assignments.
    for (i, (event_id, _)) in pairs.iter().enumerate() {
        let (label, prob, _) = labels_with_prob[i];
        tx.execute(
            "INSERT INTO event_clusters(event_id, cluster_id, membership_prob, assigned_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(event_id) DO UPDATE SET cluster_id = excluded.cluster_id, \
                 membership_prob = excluded.membership_prob, assigned_at = excluded.assigned_at",
            params![event_id, label, prob as f64, now],
        )?;
    }

    tx.commit()?;
    Ok(crate::ClusterStats {
        n_clusters,
        n_outliers,
        n_assigned: pairs.len(),
        dim,
        took_ms: 0,
    })
}
