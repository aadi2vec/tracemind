//! ONT-2 — Statistical Object Type proposer.
//!
//! Walks a corpus of clustered event/text signatures and emits proposed
//! [`tm_graph::OntologyProposal`] rows when a cluster is large enough and
//! its c-TF-IDF signature looks distinct from the existing schema.
//!
//! The proposer is *intentionally* conservative — it only suggests new
//! Object Types after the cluster crosses a `min_cluster_size` floor and
//! its top term is not already an Object Type name. The user accepts
//! or rejects in Settings; rejection is persisted so we won't badger.

use std::collections::HashMap;

use chrono::Utc;
use rusqlite::Connection;
use tm_graph::{
    labeler::{label_clusters, LabelerConfig},
    OntologyProposal, OntologyStore, ProposalKind, ProposalStatus, ProposalStore,
};
use tm_types::Result;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ProposerConfig {
    /// Minimum samples in a cluster before we'll propose anything.
    pub min_cluster_size: usize,
    /// Minimum c-TF-IDF score for the candidate top term.
    pub min_term_score: f64,
    /// Top-K labels to use as evidence.
    pub top_k_terms: usize,
}

impl Default for ProposerConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: 5,
            min_term_score: 0.02,
            top_k_terms: 5,
        }
    }
}

/// One emitted proposal — kept separate from [`tm_graph::OntologyProposal`]
/// so the proposer can be tested without a DB.
#[derive(Debug, Clone)]
pub struct ProposedObjectType {
    pub name: String,
    pub top_terms: Vec<String>,
    pub support: u32,
    pub cluster_id: i64,
}

/// Pure scoring — no I/O. Returns a list of candidates sorted by support
/// descending. The caller is responsible for de-duplicating against the
/// existing ontology and persisting via [`persist_proposals`].
pub fn propose_object_types(
    clusters: &HashMap<i64, Vec<String>>,
    cfg: &ProposerConfig,
) -> Vec<ProposedObjectType> {
    let labels = label_clusters(
        clusters,
        LabelerConfig {
            top_k: cfg.top_k_terms,
            ..LabelerConfig::default()
        },
    );

    let mut out = Vec::new();
    for (cid, samples) in clusters {
        if *cid < 0 || samples.len() < cfg.min_cluster_size {
            continue;
        }
        let Some(label) = labels.get(cid) else {
            continue;
        };
        let top = match label.terms.first() {
            Some(t) if t.score >= cfg.min_term_score => t,
            _ => continue,
        };

        // Convert "vector search" → "VectorSearch" — CamelCase Object Type names.
        let name = camelize(&top.term);
        if name.is_empty() {
            continue;
        }

        out.push(ProposedObjectType {
            name,
            top_terms: label.terms.iter().map(|t| t.term.clone()).collect(),
            support: samples.len() as u32,
            cluster_id: *cid,
        });
    }
    out.sort_by(|a, b| b.support.cmp(&a.support));
    out
}

/// CamelCase a free-form term — splits on space/underscore/dash and
/// capitalizes each chunk. Empty in → empty out.
fn camelize(term: &str) -> String {
    let mut out = String::new();
    for chunk in term.split(|c: char| c.is_whitespace() || c == '_' || c == '-') {
        let mut cs = chunk.chars();
        if let Some(first) = cs.next() {
            for c in first.to_uppercase() {
                out.push(c);
            }
            for c in cs {
                for c in c.to_lowercase() {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// Persist `proposals` against the DB, skipping names that already exist
/// as Object Types *or* were previously rejected (so we don't badger).
/// Duplicate pending proposals are merged via [`ProposalStore::upsert`]
/// (which bumps support_count).
pub fn persist_proposals(
    conn: &Connection,
    proposals: &[ProposedObjectType],
) -> Result<usize> {
    let existing_names: std::collections::HashSet<String> =
        OntologyStore::list_object_types(conn)?
            .into_iter()
            .map(|o| o.name)
            .collect();
    // Also skip anything the user already rejected. Accepted proposals
    // are already covered above (they show up as ObjectTypes).
    let rejected_names: std::collections::HashSet<String> =
        ProposalStore::list_with_status(conn, "rejected")?
            .into_iter()
            .map(|p| p.name)
            .collect();

    let mut written = 0usize;
    for p in proposals {
        if existing_names.contains(&p.name) || rejected_names.contains(&p.name) {
            continue;
        }
        let row = OntologyProposal {
            id: Uuid::new_v4(),
            kind: ProposalKind::ObjectType,
            name: p.name.clone(),
            evidence: serde_json::json!({
                "cluster_id": p.cluster_id,
                "top_terms": p.top_terms,
            }),
            support_count: p.support,
            status: ProposalStatus::Pending,
            created_at: Utc::now().to_rfc3339(),
            decided_at: None,
        };
        ProposalStore::upsert(conn, &row)?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelize_basic() {
        assert_eq!(camelize("vector search"), "VectorSearch");
        assert_eq!(camelize("Rondo"), "Rondo");
        assert_eq!(camelize("scout_report"), "ScoutReport");
        assert_eq!(camelize(""), "");
    }

    #[test]
    fn proposes_object_type_when_cluster_is_big_enough() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            (0..6)
                .map(|i| format!("rondo scout report number {i}"))
                .collect::<Vec<_>>(),
        );
        clusters.insert(
            1,
            (0..2)
                .map(|i| format!("standalone artifact {i}"))
                .collect::<Vec<_>>(),
        );
        clusters.insert(-1, vec!["outlier".into()]);

        let out = propose_object_types(&clusters, &ProposerConfig::default());
        // Only cluster 0 is big enough; outlier (-1) and cluster 1 skipped.
        assert_eq!(out.len(), 1);
        assert!(out[0].support >= 5);
    }

    #[test]
    fn small_clusters_produce_no_proposals() {
        let mut clusters = HashMap::new();
        clusters.insert(0, vec!["one".into(), "two".into()]);
        let out = propose_object_types(&clusters, &ProposerConfig::default());
        assert!(out.is_empty());
    }
}
