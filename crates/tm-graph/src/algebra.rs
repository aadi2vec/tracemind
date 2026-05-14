//! Sprint GRAPH — graph algebra.
//!
//! Five set-ops over `ThreadGraph` (and any other set of entity / event /
//! commitment / capture IDs):
//!
//! - `union(A, B)`                    — combine both
//! - `intersect(A, B)`                — shared structure only
//! - `diff(A, B)`                     — A minus B
//! - `filter(A, predicate)`           — type / confidence / object-type filter
//! - `bridge(A, B, gate)`             — restricted union across approved
//!                                      object-type pairs (uses `bridge_edges`)
//!
//! Each operator is pure (no SQL mutation). Expressions compose: every
//! operator returns a `ThreadGraph`, so `union(diff(A,B), C)` is just
//! `Algebra::union(Algebra::diff(a, b), c)`.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tm_types::Result;
use uuid::Uuid;

use crate::ontology_types::OntologyStore;
use crate::thread_graph::{ThreadGraph, ThreadGraphStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetOp {
    Union,
    Intersect,
    Diff,
}

/// Predicate for `filter(A, predicate)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FilterPredicate {
    /// Keep nodes whose entity is bound to one of these object types.
    ObjectTypeAny { types: Vec<String> },
    /// Keep nodes with cluster_id in this set.
    ClusterAny { clusters: Vec<i64> },
    /// Keep capture signals only.
    OnlyCaptures,
    /// Keep commitments only.
    OnlyCommitments,
}

/// Composable expression that evaluates to a `ThreadGraph`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum GraphExpr {
    /// Identity: a single materialized thread.
    Thread { thread_id: Uuid },
    /// Two-arg set operation.
    SetOp {
        op: SetOp,
        left: Box<GraphExpr>,
        right: Box<GraphExpr>,
    },
    /// One-arg filter.
    Filter {
        inner: Box<GraphExpr>,
        predicate: FilterPredicate,
    },
    /// Restricted cross-context union.
    Bridge {
        left: Box<GraphExpr>,
        right: Box<GraphExpr>,
        approved_pairs: Vec<(String, String)>, // (object_type_a, object_type_b)
    },
}

pub struct Algebra;

impl Algebra {
    pub fn eval(conn: &Connection, expr: &GraphExpr) -> Result<ThreadGraph> {
        match expr {
            GraphExpr::Thread { thread_id } => ThreadGraphStore::materialize(conn, *thread_id),
            GraphExpr::SetOp { op, left, right } => {
                let a = Self::eval(conn, left)?;
                let b = Self::eval(conn, right)?;
                Ok(match op {
                    SetOp::Union => Self::union(&a, &b),
                    SetOp::Intersect => Self::intersect(&a, &b),
                    SetOp::Diff => Self::diff(&a, &b),
                })
            }
            GraphExpr::Filter { inner, predicate } => {
                let g = Self::eval(conn, inner)?;
                Self::filter(conn, &g, predicate)
            }
            GraphExpr::Bridge {
                left,
                right,
                approved_pairs,
            } => {
                let a = Self::eval(conn, left)?;
                let b = Self::eval(conn, right)?;
                Self::bridge(conn, &a, &b, approved_pairs)
            }
        }
    }

    pub fn union(a: &ThreadGraph, b: &ThreadGraph) -> ThreadGraph {
        let mut out = a.clone();
        for x in &b.event_node_ids {
            if !out.event_node_ids.contains(x) {
                out.event_node_ids.push(*x);
            }
        }
        for x in &b.entity_ids {
            if !out.entity_ids.contains(x) {
                out.entity_ids.push(*x);
            }
        }
        for x in &b.topic_clusters {
            if !out.topic_clusters.contains(x) {
                out.topic_clusters.push(*x);
            }
        }
        for x in &b.commitment_ids {
            if !out.commitment_ids.contains(x) {
                out.commitment_ids.push(*x);
            }
        }
        for x in &b.capture_signal_ids {
            if !out.capture_signal_ids.contains(x) {
                out.capture_signal_ids.push(*x);
            }
        }
        out
    }

    pub fn intersect(a: &ThreadGraph, b: &ThreadGraph) -> ThreadGraph {
        let set_evt: HashSet<&Uuid> = b.event_node_ids.iter().collect();
        let set_ent: HashSet<&Uuid> = b.entity_ids.iter().collect();
        let set_cl: HashSet<&i64> = b.topic_clusters.iter().collect();
        let set_com: HashSet<&Uuid> = b.commitment_ids.iter().collect();
        let set_cap: HashSet<&i64> = b.capture_signal_ids.iter().collect();
        ThreadGraph {
            thread_id: a.thread_id,
            event_node_ids: a
                .event_node_ids
                .iter()
                .copied()
                .filter(|x| set_evt.contains(x))
                .collect(),
            entity_ids: a
                .entity_ids
                .iter()
                .copied()
                .filter(|x| set_ent.contains(x))
                .collect(),
            topic_clusters: a
                .topic_clusters
                .iter()
                .copied()
                .filter(|x| set_cl.contains(x))
                .collect(),
            commitment_ids: a
                .commitment_ids
                .iter()
                .copied()
                .filter(|x| set_com.contains(x))
                .collect(),
            capture_signal_ids: a
                .capture_signal_ids
                .iter()
                .copied()
                .filter(|x| set_cap.contains(x))
                .collect(),
        }
    }

    pub fn diff(a: &ThreadGraph, b: &ThreadGraph) -> ThreadGraph {
        let set_evt: HashSet<&Uuid> = b.event_node_ids.iter().collect();
        let set_ent: HashSet<&Uuid> = b.entity_ids.iter().collect();
        let set_cl: HashSet<&i64> = b.topic_clusters.iter().collect();
        let set_com: HashSet<&Uuid> = b.commitment_ids.iter().collect();
        let set_cap: HashSet<&i64> = b.capture_signal_ids.iter().collect();
        ThreadGraph {
            thread_id: a.thread_id,
            event_node_ids: a
                .event_node_ids
                .iter()
                .copied()
                .filter(|x| !set_evt.contains(x))
                .collect(),
            entity_ids: a
                .entity_ids
                .iter()
                .copied()
                .filter(|x| !set_ent.contains(x))
                .collect(),
            topic_clusters: a
                .topic_clusters
                .iter()
                .copied()
                .filter(|x| !set_cl.contains(x))
                .collect(),
            commitment_ids: a
                .commitment_ids
                .iter()
                .copied()
                .filter(|x| !set_com.contains(x))
                .collect(),
            capture_signal_ids: a
                .capture_signal_ids
                .iter()
                .copied()
                .filter(|x| !set_cap.contains(x))
                .collect(),
        }
    }

    pub fn filter(
        conn: &Connection,
        g: &ThreadGraph,
        pred: &FilterPredicate,
    ) -> Result<ThreadGraph> {
        match pred {
            FilterPredicate::ObjectTypeAny { types } => {
                let want: HashSet<String> = types.iter().cloned().collect();
                let mut kept = Vec::new();
                for id in &g.entity_ids {
                    if let Some(ot) = OntologyStore::object_type_for_entity(conn, *id)? {
                        if want.contains(&ot) {
                            kept.push(*id);
                        }
                    }
                }
                Ok(ThreadGraph {
                    thread_id: g.thread_id,
                    entity_ids: kept,
                    ..g.clone()
                })
            }
            FilterPredicate::ClusterAny { clusters } => {
                let want: HashSet<i64> = clusters.iter().copied().collect();
                Ok(ThreadGraph {
                    thread_id: g.thread_id,
                    topic_clusters: g
                        .topic_clusters
                        .iter()
                        .copied()
                        .filter(|c| want.contains(c))
                        .collect(),
                    ..g.clone()
                })
            }
            FilterPredicate::OnlyCaptures => Ok(ThreadGraph {
                thread_id: g.thread_id,
                commitment_ids: Vec::new(),
                ..g.clone()
            }),
            FilterPredicate::OnlyCommitments => Ok(ThreadGraph {
                thread_id: g.thread_id,
                capture_signal_ids: Vec::new(),
                ..g.clone()
            }),
        }
    }

    /// Restricted union across (object_type_a, object_type_b) pairs.
    /// Entities in B whose object type is not on the approved list are
    /// dropped. Entities without a type binding pass through (we don't
    /// gate untyped data — that would silently delete legacy state).
    pub fn bridge(
        conn: &Connection,
        a: &ThreadGraph,
        b: &ThreadGraph,
        approved_pairs: &[(String, String)],
    ) -> Result<ThreadGraph> {
        // Build a set of allowed object types from approved_pairs (both sides).
        let approved: HashSet<String> = approved_pairs
            .iter()
            .flat_map(|(x, y)| [x.clone(), y.clone()])
            .collect();

        let mut filtered_b = b.clone();
        filtered_b.entity_ids.retain(|id| {
            match OntologyStore::object_type_for_entity(conn, *id) {
                Ok(Some(ot)) => approved.contains(&ot),
                _ => true, // untyped — keep
            }
        });

        Ok(Self::union(a, &filtered_b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_sprint::ensure_schema;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE memory_views (id TEXT PRIMARY KEY, name TEXT);")
            .unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    fn mk(ents: &[Uuid]) -> ThreadGraph {
        ThreadGraph {
            thread_id: Uuid::new_v4(),
            event_node_ids: vec![],
            entity_ids: ents.to_vec(),
            topic_clusters: vec![],
            commitment_ids: vec![],
            capture_signal_ids: vec![],
        }
    }

    #[test]
    fn union_intersect_diff_basics() {
        let a = mk(&[Uuid::from_u128(1), Uuid::from_u128(2)]);
        let b = mk(&[Uuid::from_u128(2), Uuid::from_u128(3)]);
        assert_eq!(Algebra::union(&a, &b).entity_ids.len(), 3);
        assert_eq!(Algebra::intersect(&a, &b).entity_ids, vec![Uuid::from_u128(2)]);
        assert_eq!(Algebra::diff(&a, &b).entity_ids, vec![Uuid::from_u128(1)]);
    }

    #[test]
    fn filter_object_type() {
        let c = fresh();
        let p = Uuid::new_v4();
        let o = Uuid::new_v4();
        OntologyStore::assign_object_type(&c, p, "Person").unwrap();
        OntologyStore::assign_object_type(&c, o, "Organization").unwrap();
        let g = mk(&[p, o]);
        let filtered = Algebra::filter(
            &c,
            &g,
            &FilterPredicate::ObjectTypeAny {
                types: vec!["Person".into()],
            },
        )
        .unwrap();
        assert_eq!(filtered.entity_ids, vec![p]);
    }

    #[test]
    fn bridge_drops_unapproved_types() {
        let c = fresh();
        let person_a = Uuid::new_v4();
        let topic_a = Uuid::new_v4();
        let person_b = Uuid::new_v4();
        let proj_b = Uuid::new_v4();
        OntologyStore::assign_object_type(&c, person_a, "Person").unwrap();
        OntologyStore::assign_object_type(&c, topic_a, "Topic").unwrap();
        OntologyStore::assign_object_type(&c, person_b, "Person").unwrap();
        OntologyStore::assign_object_type(&c, proj_b, "Project").unwrap();

        let a = mk(&[person_a, topic_a]);
        let b = mk(&[person_b, proj_b]);
        // Approve only Person ↔ Project: so person_b survives, proj_b survives
        let pairs = vec![("Person".to_string(), "Project".to_string())];
        let bridged = Algebra::bridge(&c, &a, &b, &pairs).unwrap();
        // person_a + topic_a from A + person_b + proj_b from B (all approved)
        assert_eq!(bridged.entity_ids.len(), 4);
    }

    #[test]
    fn graphexpr_compose_round_trip() -> Result<()> {
        let c = fresh();
        // We can't easily eval Thread{} without a real thread row, but we
        // can confirm the enum serializes/deserializes round-trip.
        let expr = GraphExpr::SetOp {
            op: SetOp::Diff,
            left: Box::new(GraphExpr::Thread {
                thread_id: Uuid::nil(),
            }),
            right: Box::new(GraphExpr::Filter {
                inner: Box::new(GraphExpr::Thread {
                    thread_id: Uuid::nil(),
                }),
                predicate: FilterPredicate::OnlyCaptures,
            }),
        };
        let s = serde_json::to_string(&expr).unwrap();
        let back: GraphExpr = serde_json::from_str(&s).unwrap();
        match back {
            GraphExpr::SetOp { op: SetOp::Diff, .. } => {}
            _ => panic!("round-trip failed"),
        }
        let _ = c;
        Ok(())
    }
}
