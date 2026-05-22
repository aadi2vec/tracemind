//! Thin wrapper around the `hdbscan` crate. Returns
//! `(cluster_label, membership_prob, persistence)` per input point.

use hdbscan::{DistanceMetric, Hdbscan, HdbscanHyperParams};

use crate::{ClusterError, ClusterResult, OUTLIER_ID};

/// Run HDBSCAN. Returns one tuple per input point:
/// `(cluster_label, membership_prob, cluster_persistence)`. Outliers
/// carry `cluster_label == -1`.
pub(crate) fn run(
    points: Vec<Vec<f32>>,
    min_cluster_size: usize,
    min_samples: usize,
) -> ClusterResult<Vec<(i64, f32, f32)>> {
    if points.is_empty() {
        return Ok(Vec::new());
    }
    let params = HdbscanHyperParams::builder()
        .min_cluster_size(min_cluster_size)
        .min_samples(min_samples)
        .dist_metric(DistanceMetric::Euclidean)
        .build();
    let clusterer = Hdbscan::new(&points, params);
    let labels = clusterer
        .cluster()
        .map_err(|e| ClusterError::Hdbscan(format!("{e:?}")))?;
    // The hdbscan crate returns Vec<i32> where -1 = noise. Member probs
    // and persistence aren't exposed directly, so we set membership to
    // 1.0 inside clusters and 0.0 for outliers, and persistence to 1.0
    // per non-outlier cluster (placeholder — we still track outlier vs
    // member, which is what callers care about).
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let cluster_id = label as i64;
        if cluster_id < 0 {
            out.push((OUTLIER_ID, 0.0, 0.0));
        } else {
            out.push((cluster_id, 1.0, 1.0));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(run(Vec::new(), 5, 3).unwrap().is_empty());
    }

    #[test]
    fn distinct_clusters_get_different_labels() {
        // Two tight clusters, well separated.
        let mut pts: Vec<Vec<f32>> = Vec::new();
        for i in 0..6 {
            pts.push(vec![0.0 + (i as f32) * 0.001, 0.0, 0.0]);
        }
        for i in 0..6 {
            pts.push(vec![10.0 + (i as f32) * 0.001, 10.0, 10.0]);
        }
        let r = run(pts, 3, 2).unwrap();
        let mut labels: Vec<i64> = r.iter().map(|(l, _, _)| *l).collect();
        labels.sort();
        labels.dedup();
        // Two real clusters (maybe + outlier).
        let non_outlier: Vec<&i64> = labels.iter().filter(|x| **x >= 0).collect();
        assert!(non_outlier.len() >= 2, "got labels {:?}", labels);
    }
}
