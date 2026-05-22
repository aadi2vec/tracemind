//! CLU-5 — Louvain community detection over `kg_entities` + `kg_relations`.
//!
//! Sibling to the HDBSCAN clusters in `tm-cluster`. HDBSCAN clusters
//! **events** (topical / temporal). Louvain clusters **entities**
//! (relational). Both feed WME and the Memory Garden surface.
//!
//! Algorithm: classic Louvain modularity maximisation, single
//! aggregation level for simplicity. Each pass:
//!
//! 1. Initialise each node into its own community.
//! 2. For each node (in shuffled order), move it to the neighbour
//!    community that gives the largest modularity gain (if positive).
//! 3. Repeat until no node moves.
//!
//! We deliberately keep this single-level — the typical TraceMind graph
//! is small enough (≤ 10k entities) that a single Louvain pass is both
//! cheap and produces stable communities. We can add the
//! aggregation/recursion step in a later sprint if needed.
//!
//! Inputs come straight from `kg_relations` JSON via [`GraphStore`];
//! outputs are written back to `kg_entities.community_id`.

use std::collections::HashMap;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use sqlite_knowledge_graph::KnowledgeGraph;
use tm_types::{Result, TraceMindError};

/// One row of community-recompute output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityAssignment {
    pub entity_id: i64,
    pub community_id: i64,
}

/// Summary stats from a full Louvain pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityStats {
    pub n_entities: usize,
    pub n_edges: usize,
    pub n_communities: usize,
    pub modularity: f64,
    pub iterations: usize,
}

/// Run Louvain over the live graph. Persists `community_id` on every
/// `kg_entities` row. Idempotent — repeated calls converge on the same
/// (up-to-relabelling) partition for the same input.
pub fn recompute_communities(kg: &KnowledgeGraph) -> Result<CommunityStats> {
    ensure_community_column(kg)?;

    // 1. Load nodes (entity ids only — we don't need labels here).
    let conn = kg.connection();
    let mut node_stmt = conn
        .prepare("SELECT id FROM kg_entities")
        .map_err(|e| TraceMindError::Storage(format!("community node prep: {e}")))?;
    let node_ids: Vec<i64> = node_stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| TraceMindError::Storage(format!("community node q: {e}")))?
        .filter_map(|r| r.ok())
        .collect();
    drop(node_stmt);
    if node_ids.is_empty() {
        return Ok(CommunityStats {
            n_entities: 0,
            n_edges: 0,
            n_communities: 0,
            modularity: 0.0,
            iterations: 0,
        });
    }

    // Map entity id → dense index 0..N for the adjacency table.
    let id_to_idx: HashMap<i64, usize> =
        node_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let n = node_ids.len();

    // 2. Build undirected weighted edge list from kg_relations. The
    //    relation properties JSON carries source/target/confidence; we
    //    use confidence as the edge weight (clamped to [0.05, 1.0] to
    //    avoid zero weights dropping nodes out of the graph).
    let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
    let mut total_weight = 0.0_f64;
    let mut n_edges = 0_usize;

    // kg_relations stores src/tgt/weight as top-level columns (not JSON
    // props). Earlier revisions of this loop read `source_entity_id` /
    // `target_entity_id` / `confidence` from the `properties` blob, which
    // never matched the real schema — every relation returned None, the
    // graph appeared empty, and every entity ended up in its own
    // singleton community (modularity 0.000). Fixed: read columns
    // directly, clamp weight into a sane range so zero/missing weights
    // don't drop edges silently.
    let mut rel_stmt = conn
        .prepare("SELECT source_id, target_id, weight FROM kg_relations")
        .map_err(|e| TraceMindError::Storage(format!("community rel prep: {e}")))?;
    let rows = rel_stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2).unwrap_or(0.5),
            ))
        })
        .map_err(|e| TraceMindError::Storage(format!("community rel q: {e}")))?;
    for r in rows {
        let (s, t, w_raw) = match r {
            Ok(tuple) => tuple,
            Err(_) => continue,
        };
        if s == t {
            continue;
        }
        let w = w_raw.clamp(0.05, 1.0);
        if let (Some(&si), Some(&ti)) = (id_to_idx.get(&s), id_to_idx.get(&t)) {
            *adj[si].entry(ti).or_insert(0.0) += w;
            *adj[ti].entry(si).or_insert(0.0) += w;
            total_weight += w;
            n_edges += 1;
        }
    }
    drop(rel_stmt);

    if total_weight == 0.0 {
        // Disconnected graph: every node is its own community.
        let stats = CommunityStats {
            n_entities: n,
            n_edges: 0,
            n_communities: n,
            modularity: 0.0,
            iterations: 0,
        };
        persist_assignments(
            kg,
            &node_ids.iter().enumerate().map(|(i, &id)| (id, i as i64)).collect::<Vec<_>>(),
        )?;
        return Ok(stats);
    }

    // Two-m for the modularity formula.
    let two_m = 2.0 * total_weight;
    let node_strength: Vec<f64> =
        adj.iter().map(|m| m.values().sum::<f64>()).collect();

    // 3. Run Louvain.
    let mut community: Vec<usize> = (0..n).collect();
    let mut community_strength: Vec<f64> = node_strength.clone();
    let mut iterations = 0_usize;

    // Deterministic node order — sort by id so test outputs are stable.
    let visit_order: Vec<usize> = (0..n).collect();

    loop {
        iterations += 1;
        let mut moved = false;
        for &i in visit_order.iter() {
            // Strength of node i.
            let k_i = node_strength[i];
            // Sum of edge weights from i to each neighbour community.
            let mut to_comm: HashMap<usize, f64> = HashMap::new();
            for (&j, &w) in adj[i].iter() {
                *to_comm.entry(community[j]).or_insert(0.0) += w;
            }
            let curr_comm = community[i];
            // Remove i from its current community (so we compare against "what if i left").
            let strength_curr_without_i = community_strength[curr_comm] - k_i;

            let mut best_comm = curr_comm;
            let mut best_gain = 0.0_f64;
            for (&c, &k_i_to_c) in to_comm.iter() {
                let strength_c = if c == curr_comm {
                    strength_curr_without_i
                } else {
                    community_strength[c]
                };
                // Modularity delta when moving i into c:
                //   ΔQ = (k_i_to_c / m) - (strength_c * k_i) / (2 m^2)
                let gain = (k_i_to_c / total_weight)
                    - (strength_c * k_i) / (two_m * total_weight);
                if gain > best_gain + 1e-12 {
                    best_gain = gain;
                    best_comm = c;
                }
            }
            if best_comm != curr_comm {
                community_strength[curr_comm] -= k_i;
                community_strength[best_comm] += k_i;
                community[i] = best_comm;
                moved = true;
            }
        }
        if !moved || iterations >= 50 {
            break;
        }
    }

    // 4. Relabel community ids to a dense 0..K range.
    let mut relabel: HashMap<usize, i64> = HashMap::new();
    let mut next_id: i64 = 0;
    for &c in community.iter() {
        relabel.entry(c).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
    }
    let assignments: Vec<(i64, i64)> = node_ids
        .iter()
        .enumerate()
        .map(|(i, &eid)| (eid, *relabel.get(&community[i]).unwrap()))
        .collect();

    // 5. Compute final modularity (for stats).
    let modularity = modularity(&adj, &community, &node_strength, total_weight);

    // 6. Persist.
    persist_assignments(kg, &assignments)?;

    Ok(CommunityStats {
        n_entities: n,
        n_edges,
        n_communities: relabel.len(),
        modularity,
        iterations,
    })
}

fn ensure_community_column(kg: &KnowledgeGraph) -> Result<()> {
    let conn = kg.connection();
    let has_col: bool = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('kg_entities') WHERE name = 'community_id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|_| true)
        .unwrap_or(false);
    if !has_col {
        conn.execute(
            "ALTER TABLE kg_entities ADD COLUMN community_id INTEGER",
            [],
        )
        .map_err(|e| TraceMindError::Storage(format!("alter community_id: {e}")))?;
    }
    Ok(())
}

fn persist_assignments(
    kg: &KnowledgeGraph,
    assignments: &[(i64, i64)],
) -> Result<()> {
    let conn = kg.connection();
    let tx_conn = conn;
    // No explicit transaction since KnowledgeGraph holds the conn.
    let mut stmt = tx_conn
        .prepare("UPDATE kg_entities SET community_id = ?1 WHERE id = ?2")
        .map_err(|e| TraceMindError::Storage(format!("community update prep: {e}")))?;
    for (eid, cid) in assignments {
        stmt.execute(params![cid, eid])
            .map_err(|e| TraceMindError::Storage(format!("community update: {e}")))?;
    }
    Ok(())
}

/// Summary stats from a community-labeling pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityLabelStats {
    pub n_communities: usize,
    pub n_labeled: usize,
    pub n_skipped: usize,
}

/// One labeled community, returned by [`community_label_map`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityLabel {
    pub community_id: i64,
    pub label: String,
    pub size: usize,
}

/// Recompute c-TF-IDF labels for every populated community. Reads
/// member entity names + intra-community relation types, runs them
/// through [`crate::labeler::label_clusters`], and persists each
/// community's top-K phrase into `kg_community_labels`.
///
/// Must run *after* [`recompute_communities`] — uses the
/// `kg_entities.community_id` it writes.
pub fn recompute_community_labels(kg: &KnowledgeGraph) -> Result<CommunityLabelStats> {
    ensure_label_table(kg)?;
    let conn = kg.connection();

    // 1. Group entity names by community.
    let mut texts_by_comm: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT community_id, name FROM kg_entities \
                 WHERE community_id IS NOT NULL",
            )
            .map_err(|e| TraceMindError::Storage(format!("comm-label ent prep: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let cid: i64 = row.get(0)?;
                let name: String = row.get(1)?;
                Ok((cid, name))
            })
            .map_err(|e| TraceMindError::Storage(format!("comm-label ent q: {e}")))?;
        for r in rows.flatten() {
            texts_by_comm.entry(r.0).or_default().push(r.1);
        }
    }
    if texts_by_comm.is_empty() {
        return Ok(CommunityLabelStats {
            n_communities: 0,
            n_labeled: 0,
            n_skipped: 0,
        });
    }

    // 2. Pull intra-community relation types — they carry semantic
    //    signal that bare entity names miss. We map endpoint -> community
    //    once, then for each relation where both endpoints share a
    //    community we add the rel_type to that community's text bag.
    let mut comm_by_entity: HashMap<i64, i64> = HashMap::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, community_id FROM kg_entities \
                 WHERE community_id IS NOT NULL",
            )
            .map_err(|e| TraceMindError::Storage(format!("comm-label ent2 prep: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let eid: i64 = row.get(0)?;
                let cid: i64 = row.get(1)?;
                Ok((eid, cid))
            })
            .map_err(|e| TraceMindError::Storage(format!("comm-label ent2 q: {e}")))?;
        for r in rows.flatten() {
            comm_by_entity.insert(r.0, r.1);
        }
    }
    {
        let mut stmt = conn
            .prepare("SELECT source_id, target_id, rel_type FROM kg_relations")
            .map_err(|e| TraceMindError::Storage(format!("comm-label rel prep: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| TraceMindError::Storage(format!("comm-label rel q: {e}")))?;
        for r in rows.flatten() {
            let (s, t, rel) = r;
            if let (Some(&cs), Some(&ct)) = (comm_by_entity.get(&s), comm_by_entity.get(&t)) {
                if cs == ct {
                    if let Some(word) = clean_rel_type_for_label(&rel) {
                        texts_by_comm.entry(cs).or_default().push(word);
                    }
                }
            }
        }
    }

    let total = texts_by_comm.len();
    let labels = crate::labeler::label_clusters(
        &texts_by_comm,
        crate::labeler::LabelerConfig::default(),
    );

    // 3. Persist. Wipe stale rows first so old labels for now-merged
    //    communities don't linger.
    conn.execute("DELETE FROM kg_community_labels", [])
        .map_err(|e| TraceMindError::Storage(format!("comm-label wipe: {e}")))?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut ins = conn
        .prepare(
            "INSERT INTO kg_community_labels (community_id, label, terms_json, updated_at) \
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(|e| TraceMindError::Storage(format!("comm-label ins prep: {e}")))?;
    let mut n_labeled = 0_usize;
    let mut n_skipped = 0_usize;
    for (cid, label) in &labels {
        if label.terms.is_empty() {
            n_skipped += 1;
            continue;
        }
        let terms_json = serde_json::to_string(&label.terms.iter()
            .map(|t| (t.term.clone(), t.score))
            .collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".to_string());
        ins.execute(params![cid, label.label, terms_json, now])
            .map_err(|e| TraceMindError::Storage(format!("comm-label ins: {e}")))?;
        n_labeled += 1;
    }

    Ok(CommunityLabelStats {
        n_communities: total,
        n_labeled,
        n_skipped,
    })
}

/// Extract a single semantic word from a stored `rel_type` value for use
/// in the c-TF-IDF labeler's text bag.
///
/// `kg_relations.rel_type` is stored as either a bare predicate string
/// (`"is_a"`, `"related_to"`) or a JSON envelope for custom predicates
/// (`{"custom":"indexes"}`). Both forms used to be pushed verbatim into
/// the labeler, which then tokenised the JSON braces/quotes into noise
/// tokens — and worst of all flooded every community label with
/// "indexes"/"custom" because the MOC `Predicate::Custom("indexes")`
/// backlink predicate dominates the relation table.
///
/// We:
/// 1. Parse the JSON envelope when present and keep just the inner word.
/// 2. Filter structural predicates that carry no semantic signal:
///    `indexes` (MOC backlinks) and `related_to` (the default fallback
///    predicate).
fn clean_rel_type_for_label(raw: &str) -> Option<String> {
    let word = if raw.starts_with('{') {
        serde_json::from_str::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| v.get("custom").and_then(|w| w.as_str()).map(str::to_owned))
            .unwrap_or_default()
    } else {
        raw.trim_matches('"').to_string()
    };
    let w = word.trim();
    if w.is_empty() {
        return None;
    }
    // Structural predicates — pure graph plumbing, not topic signal.
    if matches!(w, "indexes" | "related_to") {
        return None;
    }
    Some(w.to_string())
}

/// Read back the persisted community labels (after a label pass) for
/// surface code. Empty map when no labels exist yet.
pub fn community_label_map(kg: &KnowledgeGraph) -> Result<HashMap<i64, CommunityLabel>> {
    ensure_label_table(kg)?;
    let conn = kg.connection();
    let mut stmt = conn
        .prepare(
            "SELECT l.community_id, l.label, \
                    (SELECT COUNT(*) FROM kg_entities e \
                       WHERE e.community_id = l.community_id) AS size \
             FROM kg_community_labels l",
        )
        .map_err(|e| TraceMindError::Storage(format!("comm-label read prep: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CommunityLabel {
                community_id: row.get(0)?,
                label: row.get(1)?,
                size: row.get::<_, i64>(2)? as usize,
            })
        })
        .map_err(|e| TraceMindError::Storage(format!("comm-label read q: {e}")))?;
    let mut out = HashMap::new();
    for r in rows.flatten() {
        out.insert(r.community_id, r);
    }
    Ok(out)
}

/// One community's input bag for downstream labelers (c-TF-IDF or LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitySample {
    pub community_id: i64,
    pub size: usize,
    /// Up to `max_names` member entity names, newest first.
    pub names: Vec<String>,
    /// Up to `max_rels` distinct intra-community relation types.
    pub rel_types: Vec<String>,
}

/// Pull samples for the top-N largest populated communities. Cheap —
/// reads `kg_entities` once and `kg_relations` once. Used by the LLM
/// community labeler (`tm-tauri::cmd_consolidate` slow path) which
/// needs the actual member names to build a meaningful prompt.
pub fn top_community_samples(
    kg: &KnowledgeGraph,
    top_n: usize,
    max_names: usize,
    max_rels: usize,
) -> Result<Vec<CommunitySample>> {
    let conn = kg.connection();

    // Communities ranked by size.
    let mut size_stmt = conn
        .prepare(
            "SELECT community_id, COUNT(*) FROM kg_entities \
             WHERE community_id IS NOT NULL \
             GROUP BY community_id ORDER BY COUNT(*) DESC LIMIT ?1",
        )
        .map_err(|e| TraceMindError::Storage(format!("comm-sample prep ranks: {e}")))?;
    let ranked: Vec<(i64, usize)> = size_stmt
        .query_map(params![top_n as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(|e| TraceMindError::Storage(format!("comm-sample q ranks: {e}")))?
        .filter_map(|r| r.ok())
        .collect();
    drop(size_stmt);

    let mut out = Vec::with_capacity(ranked.len());
    for (cid, size) in ranked {
        let names: Vec<String> = {
            let mut s = conn
                .prepare(
                    "SELECT name FROM kg_entities WHERE community_id = ?1 \
                     ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| TraceMindError::Storage(format!("comm-sample prep n: {e}")))?;
            let collected: Vec<String> = s
                .query_map(params![cid, max_names as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| TraceMindError::Storage(format!("comm-sample q n: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        // Intra-community rel_types: any relation where both endpoints
        // sit in this community. Distinct so the prompt stays compact.
        let rel_types: Vec<String> = {
            let mut s = conn
                .prepare(
                    "SELECT DISTINCT r.rel_type \
                     FROM kg_relations r \
                     JOIN kg_entities es ON es.id = r.source_id \
                     JOIN kg_entities et ON et.id = r.target_id \
                     WHERE es.community_id = ?1 AND et.community_id = ?1 \
                     LIMIT ?2",
                )
                .map_err(|e| TraceMindError::Storage(format!("comm-sample prep r: {e}")))?;
            let collected: Vec<String> = s
                .query_map(params![cid, max_rels as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| TraceMindError::Storage(format!("comm-sample q r: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            collected
        };

        out.push(CommunitySample {
            community_id: cid,
            size,
            names,
            rel_types,
        });
    }
    Ok(out)
}

/// Upsert a single community label. Used by the LLM labeler so it can
/// override the c-TF-IDF fallback one community at a time without
/// touching the rest. `terms_json` is opaque to this layer — pass
/// `"[]"` if you only have a label string.
pub fn set_community_label(
    kg: &KnowledgeGraph,
    community_id: i64,
    label: &str,
    terms_json: &str,
) -> Result<()> {
    ensure_label_table(kg)?;
    let conn = kg.connection();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO kg_community_labels (community_id, label, terms_json, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(community_id) DO UPDATE SET \
            label = excluded.label, \
            terms_json = excluded.terms_json, \
            updated_at = excluded.updated_at",
        params![community_id, label, terms_json, now],
    )
    .map_err(|e| TraceMindError::Storage(format!("set community label: {e}")))?;
    Ok(())
}

fn ensure_label_table(kg: &KnowledgeGraph) -> Result<()> {
    let conn = kg.connection();
    conn.execute(
        "CREATE TABLE IF NOT EXISTS kg_community_labels (\
            community_id INTEGER PRIMARY KEY, \
            label TEXT NOT NULL, \
            terms_json TEXT NOT NULL, \
            updated_at TEXT NOT NULL\
         )",
        [],
    )
    .map_err(|e| TraceMindError::Storage(format!("create kg_community_labels: {e}")))?;
    Ok(())
}

fn modularity(
    adj: &[HashMap<usize, f64>],
    community: &[usize],
    node_strength: &[f64],
    total_weight: f64,
) -> f64 {
    if total_weight == 0.0 {
        return 0.0;
    }
    let two_m = 2.0 * total_weight;
    let n = adj.len();
    let mut q = 0.0_f64;
    for i in 0..n {
        for (&j, &w_ij) in adj[i].iter() {
            if community[i] == community[j] {
                let expected = node_strength[i] * node_strength[j] / two_m;
                q += w_ij - expected;
            }
        }
    }
    q / two_m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny in-memory graph using rusqlite directly and the
    /// `kg_entities` / `kg_relations` schema (subset). We don't go
    /// through `KnowledgeGraph` because that requires the
    /// `sqlite-knowledge-graph` setup; the algorithm itself is pure.
    ///
    /// The unit test below exercises the *algorithm only* — full
    /// `kg_entities` integration is covered by the higher-level
    /// `cargo test -p tm-graph` integration test in `community_test.rs`.
    #[test]
    fn clean_rel_type_parses_custom_envelope() {
        assert_eq!(
            clean_rel_type_for_label(r#"{"custom":"uses"}"#),
            Some("uses".to_string())
        );
        assert_eq!(
            clean_rel_type_for_label(r#"{"custom":"manages"}"#),
            Some("manages".to_string())
        );
    }

    #[test]
    fn clean_rel_type_keeps_bare_predicate() {
        assert_eq!(
            clean_rel_type_for_label("works_at"),
            Some("works_at".to_string())
        );
        assert_eq!(
            clean_rel_type_for_label("is_a"),
            Some("is_a".to_string())
        );
    }

    #[test]
    fn clean_rel_type_filters_structural_predicates() {
        // MOC backlinks dominate the relation table — must not pollute
        // every community label.
        assert_eq!(clean_rel_type_for_label(r#"{"custom":"indexes"}"#), None);
        // Default fallback predicate when extractor has no opinion.
        assert_eq!(clean_rel_type_for_label("related_to"), None);
    }

    #[test]
    fn modularity_zero_for_empty_graph() {
        let adj: Vec<HashMap<usize, f64>> = Vec::new();
        let comm: Vec<usize> = Vec::new();
        let strength: Vec<f64> = Vec::new();
        assert_eq!(modularity(&adj, &comm, &strength, 0.0), 0.0);
    }

    #[test]
    fn modularity_positive_for_two_well_separated_cliques() {
        // Two triangles connected by a single weak edge.
        let mut adj: Vec<HashMap<usize, f64>> = vec![HashMap::new(); 6];
        // Triangle A: 0-1, 1-2, 2-0 (weight 1)
        let edges_a = [(0, 1), (1, 2), (2, 0)];
        let edges_b = [(3, 4), (4, 5), (5, 3)];
        let mut total_weight = 0.0;
        for &(a, b) in edges_a.iter().chain(edges_b.iter()) {
            adj[a].insert(b, 1.0);
            adj[b].insert(a, 1.0);
            total_weight += 1.0;
        }
        // Bridge 2-3 with weight 0.1
        adj[2].insert(3, 0.1);
        adj[3].insert(2, 0.1);
        total_weight += 0.1;
        let strength: Vec<f64> = adj.iter().map(|m| m.values().sum::<f64>()).collect();
        // Assign A to community 0, B to community 1
        let comm: Vec<usize> = vec![0, 0, 0, 1, 1, 1];
        let q = modularity(&adj, &comm, &strength, total_weight);
        assert!(q > 0.3, "expected modularity > 0.3, got {q}");
    }
}
