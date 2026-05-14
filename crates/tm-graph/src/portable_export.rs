//! Sprint GRAPH — portable graph export.
//!
//! Serializes a `ThreadGraph` (or any composed graph expression) into a
//! self-contained JSON document so the user can route it as MCP context
//! into another AI host (Claude / Cursor / Goose / web). Object Types +
//! Link Types are inlined so the receiver can interpret the structure
//! without round-tripping back to TraceMind.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};

use crate::ontology_types::OntologyStore;
use crate::thread_graph::ThreadGraph;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableNode {
    pub id: String,
    pub kind: String, // "entity" | "event" | "commitment" | "capture"
    pub label: Option<String>,
    pub object_type: Option<String>,
    pub properties: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableEdge {
    pub from: String,
    pub to: String,
    pub kind: String, // event-edge kind or "mentions"
    pub strength: f64,
    pub support_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableGraph {
    pub schema_version: u32,
    pub origin: String,
    pub thread_id: String,
    pub nodes: Vec<PortableNode>,
    pub edges: Vec<PortableEdge>,
    pub object_types: Vec<String>,
    pub link_types: Vec<String>,
    pub topic_clusters: Vec<i64>,
}

/// Serialize a materialized ThreadGraph to a portable JSON document.
pub fn export_portable(conn: &Connection, g: &ThreadGraph) -> Result<PortableGraph> {
    let mut nodes = Vec::new();

    // Entities — pull label + object_type
    for ent in &g.entity_ids {
        let label: std::result::Result<Option<String>, _> = conn.query_row(
            "SELECT label FROM kg_entities WHERE properties LIKE '%' || ? || '%' LIMIT 1",
            params![ent.to_string()],
            |r| r.get(0),
        );
        let ot = OntologyStore::object_type_for_entity(conn, *ent)?;
        nodes.push(PortableNode {
            id: ent.to_string(),
            kind: "entity".into(),
            label: label.ok().flatten(),
            object_type: ot,
            properties: serde_json::json!({}),
        });
    }

    // Commitments — best-effort label
    for cid in &g.commitment_ids {
        nodes.push(PortableNode {
            id: cid.to_string(),
            kind: "commitment".into(),
            label: None,
            object_type: Some("Commitment".into()),
            properties: serde_json::json!({}),
        });
    }

    // Captures
    for sid in &g.capture_signal_ids {
        let raw: std::result::Result<String, _> = conn.query_row(
            "SELECT raw_text FROM captured_signals WHERE id = ?",
            params![sid],
            |r| r.get(0),
        );
        nodes.push(PortableNode {
            id: sid.to_string(),
            kind: "capture".into(),
            label: raw.ok().map(|s| s.chars().take(80).collect()),
            object_type: None,
            properties: serde_json::json!({}),
        });
    }

    // Edges — event_edges restricted to nodes in this graph and above
    // the frequency floor (rely on supporting count column).
    let mut edges = Vec::new();
    if !g.event_node_ids.is_empty() {
        let placeholders = (0..g.event_node_ids.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT from_id, to_id, kind, strength, support_count
             FROM event_edges
             WHERE support_count >= 2
               AND from_id IN ({placeholders})
               AND to_id IN ({placeholders})"
        );
        let params_vec: Vec<rusqlite::types::Value> = g
            .event_node_ids
            .iter()
            .map(|u| rusqlite::types::Value::Text(u.to_string()))
            .chain(
                g.event_node_ids
                    .iter()
                    .map(|u| rusqlite::types::Value::Text(u.to_string())),
            )
            .collect();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| TraceMindError::Storage(format!("prep export: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                let from: String = row.get(0)?;
                let to: String = row.get(1)?;
                let kind: String = row.get(2)?;
                let strength: f64 = row.get(3)?;
                let support: i64 = row.get(4)?;
                Ok(PortableEdge {
                    from,
                    to,
                    kind,
                    strength,
                    support_count: support as u32,
                })
            })
            .map_err(|e| TraceMindError::Storage(format!("export q: {e}")))?;
        for r in rows {
            edges.push(r.map_err(|e| TraceMindError::Storage(e.to_string()))?);
        }
    }

    let object_types: Vec<String> = OntologyStore::list_object_types(conn)?
        .into_iter()
        .map(|o| o.name)
        .collect();
    let link_types: Vec<String> = OntologyStore::list_link_types(conn)?
        .into_iter()
        .map(|l| l.name)
        .collect();

    Ok(PortableGraph {
        schema_version: 1,
        origin: "tracemind".into(),
        thread_id: g.thread_id.to_string(),
        nodes,
        edges,
        object_types,
        link_types,
        topic_clusters: g.topic_clusters.clone(),
    })
}

/// Write a portable graph to a path (UTF-8 JSON).
pub fn write_to_path(graph: &PortableGraph, path: &std::path::Path) -> Result<()> {
    let s = serde_json::to_string_pretty(graph)
        .map_err(|e| TraceMindError::Storage(format!("serialize: {e}")))?;
    std::fs::write(path, s)
        .map_err(|e| TraceMindError::Storage(format!("write {path:?}: {e}")))?;
    Ok(())
}

/// Embedding-free token estimate so callers can warn before pasting
/// huge contexts into a target LLM. Conservative ~4 chars/token.
pub fn approx_token_count(graph: &PortableGraph) -> usize {
    let s = serde_json::to_string(graph).unwrap_or_default();
    s.len() / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_sprint::ensure_schema;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE memory_views (id TEXT PRIMARY KEY, name TEXT);
             CREATE TABLE kg_entities (id INTEGER PRIMARY KEY, label TEXT, properties TEXT);
             CREATE TABLE captured_signals (
               id INTEGER PRIMARY KEY, source TEXT, raw_text TEXT,
               content_hash INTEGER, promoted_entity TEXT
             );",
        )
        .unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    #[test]
    fn portable_round_trip() {
        let c = fresh();
        let g = ThreadGraph {
            thread_id: uuid::Uuid::new_v4(),
            event_node_ids: vec![],
            entity_ids: vec![uuid::Uuid::new_v4()],
            topic_clusters: vec![1, 2],
            commitment_ids: vec![],
            capture_signal_ids: vec![],
        };
        let portable = export_portable(&c, &g).unwrap();
        assert_eq!(portable.schema_version, 1);
        assert_eq!(portable.origin, "tracemind");
        assert!(approx_token_count(&portable) > 0);
        let s = serde_json::to_string(&portable).unwrap();
        let back: PortableGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(back.thread_id, portable.thread_id);
    }
}
