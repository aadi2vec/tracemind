//! Sprint GRAPH — Palantir-style Object + Link Type ontology.
//!
//! Sits *alongside* `crate::ontology` (the existing LM-15 heuristic
//! fiction/real classifier). This module owns the **versioned schema
//! layer**: Object Types (Person, Org, Project, …) + Link Types
//! (WorksAt, Mentions, …) that map types → allowed edges. Every entity
//! can optionally be bound to one Object Type; type-checking on write
//! enforces "this edge is allowed between these types".

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectType {
    pub id: Uuid,
    pub name: String,
    pub version: u32,
    pub parent_id: Option<Uuid>,
    pub property_schema: serde_json::Value,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkType {
    pub id: Uuid,
    pub name: String,
    pub version: u32,
    pub from_object_type_id: Uuid,
    pub to_object_type_id: Uuid,
    pub cardinality: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectTypeAssignment {
    pub entity_id: Uuid,
    pub object_type_id: Uuid,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TypeCheckOutcome {
    /// Edge allowed by an existing link type rule.
    Allowed,
    /// No matching link type — write rejected.
    Rejected { reason: String },
    /// One or both endpoints are not type-bound — pass through.
    Untyped,
}

pub struct OntologyStore;

impl OntologyStore {
    /// List object types (newest first).
    pub fn list_object_types(conn: &Connection) -> Result<Vec<ObjectType>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, version, parent_id, property_schema, source
                 FROM ontology_object_types ORDER BY name ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep ot list: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let version: i64 = row.get(2)?;
                let parent: Option<String> = row.get(3)?;
                let schema: String = row.get(4)?;
                let source: String = row.get(5)?;
                Ok((id, name, version, parent, schema, source))
            })
            .map_err(|e| TraceMindError::Storage(format!("ot query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, version, parent, schema, source) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(ObjectType {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                version: version as u32,
                parent_id: parent.and_then(|s| Uuid::parse_str(&s).ok()),
                property_schema: serde_json::from_str(&schema)
                    .unwrap_or_else(|_| serde_json::json!({})),
                source,
            });
        }
        Ok(out)
    }

    pub fn list_link_types(conn: &Connection) -> Result<Vec<LinkType>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, version, from_object_type_id, to_object_type_id,
                        cardinality, source
                 FROM ontology_link_types ORDER BY name ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep lt list: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let name: String = row.get(1)?;
                let version: i64 = row.get(2)?;
                let from: String = row.get(3)?;
                let to: String = row.get(4)?;
                let card: String = row.get(5)?;
                let src: String = row.get(6)?;
                Ok((id, name, version, from, to, card, src))
            })
            .map_err(|e| TraceMindError::Storage(format!("lt query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, version, from, to, card, src) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(LinkType {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                name,
                version: version as u32,
                from_object_type_id: Uuid::parse_str(&from)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                to_object_type_id: Uuid::parse_str(&to)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                cardinality: card,
                source: src,
            });
        }
        Ok(out)
    }

    /// Bind an entity to an object type (idempotent).
    pub fn assign_object_type(
        conn: &Connection,
        entity_id: Uuid,
        object_type_name: &str,
    ) -> Result<()> {
        let ot_id: std::result::Result<String, _> = conn.query_row(
            "SELECT id FROM ontology_object_types WHERE name = ?",
            params![object_type_name],
            |r| r.get(0),
        );
        let ot_id = match ot_id {
            Ok(s) => s,
            Err(_) => {
                return Err(TraceMindError::Storage(format!(
                    "no such object type: {object_type_name}"
                )));
            }
        };

        conn.execute(
            "INSERT INTO entity_object_type (entity_id, object_type_id, assigned_at)
             VALUES (?,?,?)
             ON CONFLICT(entity_id) DO UPDATE SET
               object_type_id = excluded.object_type_id,
               assigned_at    = excluded.assigned_at",
            params![entity_id.to_string(), ot_id, Utc::now().to_rfc3339()],
        )
        .map_err(|e| TraceMindError::Storage(format!("assign ot: {e}")))?;
        Ok(())
    }

    /// Look up the object-type name for an entity, if any.
    pub fn object_type_for_entity(
        conn: &Connection,
        entity_id: Uuid,
    ) -> Result<Option<String>> {
        let r: std::result::Result<String, _> = conn.query_row(
            "SELECT ot.name FROM entity_object_type e
             JOIN ontology_object_types ot ON ot.id = e.object_type_id
             WHERE e.entity_id = ?",
            params![entity_id.to_string()],
            |r| r.get(0),
        );
        match r {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(format!("ot for ent: {e}"))),
        }
    }

    /// Type-check a proposed edge. Untyped endpoints are *not* errors —
    /// we don't force ontology adoption.
    pub fn check_edge(
        conn: &Connection,
        from_entity: Uuid,
        to_entity: Uuid,
        link_type_name: &str,
    ) -> Result<TypeCheckOutcome> {
        let from_ot = Self::object_type_for_entity(conn, from_entity)?;
        let to_ot = Self::object_type_for_entity(conn, to_entity)?;
        let (Some(from_ot), Some(to_ot)) = (from_ot, to_ot) else {
            return Ok(TypeCheckOutcome::Untyped);
        };

        let allowed: std::result::Result<i64, _> = conn.query_row(
            "SELECT 1 FROM ontology_link_types lt
             JOIN ontology_object_types ot_from ON ot_from.id = lt.from_object_type_id
             JOIN ontology_object_types ot_to   ON ot_to.id   = lt.to_object_type_id
             WHERE lt.name = ? AND ot_from.name = ? AND ot_to.name = ?",
            params![link_type_name, from_ot, to_ot],
            |r| r.get(0),
        );
        match allowed {
            Ok(_) => Ok(TypeCheckOutcome::Allowed),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(TypeCheckOutcome::Rejected {
                reason: format!(
                    "no link type '{link_type_name}' between {from_ot} → {to_ot}"
                ),
            }),
            Err(e) => Err(TraceMindError::Storage(format!("check edge: {e}"))),
        }
    }

    /// Create a *user-defined* object type. Bumps version on conflict.
    pub fn create_object_type(
        conn: &Connection,
        name: &str,
        parent_name: Option<&str>,
        property_schema: serde_json::Value,
    ) -> Result<Uuid> {
        let parent_id: Option<String> = if let Some(pn) = parent_name {
            let r: std::result::Result<String, _> = conn.query_row(
                "SELECT id FROM ontology_object_types WHERE name = ?",
                params![pn],
                |r| r.get(0),
            );
            r.ok()
        } else {
            None
        };
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO ontology_object_types
              (id, name, version, parent_id, property_schema, source, created_at, updated_at)
             VALUES (?, ?, 1, ?, ?, 'user', ?, ?)
             ON CONFLICT(name) DO UPDATE SET
               version = ontology_object_types.version + 1,
               property_schema = excluded.property_schema,
               updated_at = excluded.updated_at",
            params![
                id.to_string(),
                name,
                parent_id,
                property_schema.to_string(),
                now,
                now
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("create ot: {e}")))?;
        Ok(id)
    }

    /// Create a user-defined link type.
    pub fn create_link_type(
        conn: &Connection,
        name: &str,
        from_ot_name: &str,
        to_ot_name: &str,
        cardinality: &str,
    ) -> Result<Uuid> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4();
        conn.execute(
            "INSERT INTO ontology_link_types
              (id, name, version, from_object_type_id, to_object_type_id,
               cardinality, source, created_at, updated_at)
             VALUES (
               ?, ?, 1,
               (SELECT id FROM ontology_object_types WHERE name = ?),
               (SELECT id FROM ontology_object_types WHERE name = ?),
               ?, 'user', ?, ?
             )",
            params![id.to_string(), name, from_ot_name, to_ot_name, cardinality, now, now],
        )
        .map_err(|e| TraceMindError::Storage(format!("create lt: {e}")))?;
        Ok(id)
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

    #[test]
    fn builtin_types_populated() {
        let c = fresh();
        let ots = OntologyStore::list_object_types(&c).unwrap();
        assert!(ots.iter().any(|o| o.name == "Person"));
        assert!(ots.iter().any(|o| o.name == "Project"));
        let lts = OntologyStore::list_link_types(&c).unwrap();
        assert!(lts.iter().any(|l| l.name == "WorksAt"));
    }

    #[test]
    fn type_check_allows_known_edge_rejects_unknown() {
        let c = fresh();
        let pat = Uuid::new_v4();
        let acme = Uuid::new_v4();
        OntologyStore::assign_object_type(&c, pat, "Person").unwrap();
        OntologyStore::assign_object_type(&c, acme, "Organization").unwrap();
        assert_eq!(
            OntologyStore::check_edge(&c, pat, acme, "WorksAt").unwrap(),
            TypeCheckOutcome::Allowed
        );
        let outcome = OntologyStore::check_edge(&c, pat, acme, "NoSuchLink").unwrap();
        match outcome {
            TypeCheckOutcome::Rejected { .. } => {}
            other => panic!("expected Rejected got {other:?}"),
        }
    }

    #[test]
    fn untyped_edge_passes_through() {
        let c = fresh();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert_eq!(
            OntologyStore::check_edge(&c, a, b, "WorksAt").unwrap(),
            TypeCheckOutcome::Untyped
        );
    }
}
