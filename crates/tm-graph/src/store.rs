//! Kuzu-backed graph store for TraceMind (Phase 2).
//!
//! Replaces the rusqlite implementation with Kuzu embedded graph DB.
//! Same public API: open, upsert_entity, get_entity, find_entity_by_name,
//! upsert_triple, get_triples_for_entity, k_hop_neighbors.

use kuzu::{Connection, Database, LogicalType, SystemConfig, Value};
use tm_types::{Entity, EntityType, Predicate, Result, TraceMindError, Triple};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use tracing::{debug, info};

pub struct GraphStore {
    db: Database,
}

impl GraphStore {
    /// Open or create a Kuzu graph store.
    /// Pass `:memory:` for an in-memory database (tests).
    pub fn open(path: &str) -> Result<Self> {
        info!("[graph] opening Kuzu store at {path}");

        let db = if path == ":memory:" {
            Database::in_memory(SystemConfig::default())
        } else {
            Database::new(path, SystemConfig::default())
        }
        .map_err(|e| TraceMindError::Storage(format!("kuzu open: {e}")))?;

        let store = Self { db };
        store.init_schema()?;
        Ok(store)
    }

    fn conn(&self) -> Result<Connection<'_>> {
        Connection::new(&self.db)
            .map_err(|e| TraceMindError::Storage(format!("kuzu connection: {e}")))
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn()?;

        conn.query(
            "CREATE NODE TABLE IF NOT EXISTS Entity(
                id STRING,
                name STRING,
                entity_type STRING,
                confidence DOUBLE,
                source_id STRING,
                created_at STRING,
                updated_at STRING,
                PRIMARY KEY(id)
            )"
        ).map_err(|e| TraceMindError::Storage(format!("create Entity table: {e}")))?;

        conn.query(
            "CREATE REL TABLE IF NOT EXISTS Triple(
                FROM Entity TO Entity,
                id STRING,
                predicate STRING,
                confidence DOUBLE,
                source_id STRING,
                created_at STRING,
                updated_at STRING
            )"
        ).map_err(|e| TraceMindError::Storage(format!("create Triple table: {e}")))?;

        debug!("[graph] schema initialized");
        Ok(())
    }

    pub fn upsert_entity(&self, entity: &Entity) -> Result<()> {
        let conn = self.conn()?;

        let entity_type_json = serde_json::to_string(&entity.entity_type)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let source_id_val = match &entity.source_id {
            Some(s) => Value::String(s.clone()),
            None => Value::Null(LogicalType::String),
        };

        let mut stmt = conn.prepare(
            "MERGE (e:Entity {id: $id})
             SET e.name = $name,
                 e.entity_type = $etype,
                 e.confidence = $conf,
                 e.source_id = $src,
                 e.created_at = $cat,
                 e.updated_at = $uat"
        ).map_err(|e| TraceMindError::Storage(format!("prepare upsert_entity: {e}")))?;

        conn.execute(
            &mut stmt,
            vec![
                ("id", Value::String(entity.id.to_string())),
                ("name", Value::String(entity.name.clone())),
                ("etype", Value::String(entity_type_json)),
                ("conf", Value::Double(entity.confidence)),
                ("src", source_id_val),
                ("cat", Value::String(entity.created_at.to_rfc3339())),
                ("uat", Value::String(entity.updated_at.to_rfc3339())),
            ],
        ).map_err(|e| TraceMindError::Storage(format!("execute upsert_entity: {e}")))?;

        debug!("[graph] upserted entity id={} name={}", entity.id, entity.name);
        Ok(())
    }

    pub fn get_entity(&self, id: Uuid) -> Result<Entity> {
        let conn = self.conn()?;

        let mut stmt = conn.prepare(
            "MATCH (e:Entity {id: $id})
             RETURN e.id, e.name, e.entity_type, e.confidence, e.source_id, e.created_at, e.updated_at"
        ).map_err(|e| TraceMindError::Storage(format!("prepare get_entity: {e}")))?;

        let result = conn.execute(
            &mut stmt,
            vec![("id", Value::String(id.to_string()))],
        ).map_err(|e| TraceMindError::Storage(format!("execute get_entity: {e}")))?;

        let row = result.into_iter().next()
            .ok_or_else(|| TraceMindError::Storage(format!("no rows for entity {id}")))?;

        parse_entity_row(&row)
    }

    pub fn find_entity_by_name(&self, name: &str) -> Result<Option<Entity>> {
        let conn = self.conn()?;

        let mut stmt = conn.prepare(
            "MATCH (e:Entity)
             WHERE e.name = $name
             RETURN e.id, e.name, e.entity_type, e.confidence, e.source_id, e.created_at, e.updated_at
             LIMIT 1"
        ).map_err(|e| TraceMindError::Storage(format!("prepare find_entity_by_name: {e}")))?;

        let result = conn.execute(
            &mut stmt,
            vec![("name", Value::String(name.to_string()))],
        ).map_err(|e| TraceMindError::Storage(format!("execute find_entity_by_name: {e}")))?;

        match result.into_iter().next() {
            Some(row) => Ok(Some(parse_entity_row(&row)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_triple(&self, triple: &Triple) -> Result<()> {
        let conn = self.conn()?;

        let predicate_json = serde_json::to_string(&triple.predicate)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let source_id_val = match &triple.source_id {
            Some(s) => Value::String(s.clone()),
            None => Value::Null(LogicalType::String),
        };

        // Delete existing triple with same id (if any)
        let mut del_stmt = conn.prepare(
            "MATCH (s:Entity)-[r:Triple]->(o:Entity) WHERE r.id = $id DELETE r"
        ).map_err(|e| TraceMindError::Storage(format!("prepare delete triple: {e}")))?;

        conn.execute(
            &mut del_stmt,
            vec![("id", Value::String(triple.id.to_string()))],
        ).map_err(|e| TraceMindError::Storage(format!("execute delete triple: {e}")))?;

        // Create new relationship
        let mut ins_stmt = conn.prepare(
            "MATCH (s:Entity {id: $sid}), (o:Entity {id: $oid})
             CREATE (s)-[r:Triple {
                 id: $id,
                 predicate: $pred,
                 confidence: $conf,
                 source_id: $src,
                 created_at: $cat,
                 updated_at: $uat
             }]->(o)"
        ).map_err(|e| TraceMindError::Storage(format!("prepare insert triple: {e}")))?;

        conn.execute(
            &mut ins_stmt,
            vec![
                ("sid", Value::String(triple.subject_id.to_string())),
                ("oid", Value::String(triple.object_id.to_string())),
                ("id", Value::String(triple.id.to_string())),
                ("pred", Value::String(predicate_json)),
                ("conf", Value::Double(triple.confidence)),
                ("src", source_id_val),
                ("cat", Value::String(triple.created_at.to_rfc3339())),
                ("uat", Value::String(triple.updated_at.to_rfc3339())),
            ],
        ).map_err(|e| TraceMindError::Storage(format!("execute insert triple: {e}")))?;

        debug!("[graph] upserted triple id={} ({} -> {})", triple.id, triple.subject_id, triple.object_id);
        Ok(())
    }

    pub fn get_triples_for_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let conn = self.conn()?;

        let eid = entity_id.to_string();

        let mut stmt = conn.prepare(
            "MATCH (s:Entity)-[r:Triple]->(o:Entity)
             WHERE s.id = $eid OR o.id = $eid
             RETURN r.id, s.id, r.predicate, o.id, r.confidence, r.source_id, r.created_at, r.updated_at"
        ).map_err(|e| TraceMindError::Storage(format!("prepare get_triples: {e}")))?;

        let result = conn.execute(
            &mut stmt,
            vec![("eid", Value::String(eid))],
        ).map_err(|e| TraceMindError::Storage(format!("execute get_triples: {e}")))?;

        let mut triples = Vec::new();
        for row in result {
            triples.push(parse_triple_row(&row)?);
        }

        debug!("[graph] found {} triples for entity {entity_id}", triples.len());
        Ok(triples)
    }

    /// Return entities reachable within `hops` hops via Cypher variable-length paths.
    pub fn k_hop_neighbors(&self, entity_id: Uuid, hops: u32) -> Result<Vec<Entity>> {
        if hops == 0 {
            return Ok(vec![]);
        }

        let conn = self.conn()?;

        // Kuzu variable-length path syntax; hops is embedded as literal (not parameterizable)
        let query = format!(
            "MATCH (e:Entity {{id: $id}})-[*1..{}]-(n:Entity)
             WHERE n.id <> $id
             RETURN DISTINCT n.id, n.name, n.entity_type, n.confidence, n.source_id, n.created_at, n.updated_at",
            hops
        );

        let mut stmt = conn.prepare(&query)
            .map_err(|e| TraceMindError::Storage(format!("prepare k_hop: {e}")))?;

        let result = conn.execute(
            &mut stmt,
            vec![("id", Value::String(entity_id.to_string()))],
        ).map_err(|e| TraceMindError::Storage(format!("execute k_hop: {e}")))?;

        let mut entities = Vec::new();
        let mut seen = HashSet::new();
        for row in result {
            let entity = parse_entity_row(&row)?;
            if seen.insert(entity.id) {
                entities.push(entity);
            }
        }

        info!("[graph] k_hop({hops}) from {entity_id}: {} neighbors", entities.len());
        Ok(entities)
    }
}

// ---------------------------------------------------------------------------
// Value extraction helpers
// ---------------------------------------------------------------------------

fn extract_string(val: &Value) -> Result<String> {
    match val {
        Value::String(s) => Ok(s.clone()),
        _ => Err(TraceMindError::Storage(format!("expected String, got {:?}", val))),
    }
}

fn extract_f64(val: &Value) -> Result<f64> {
    match val {
        Value::Double(d) => Ok(*d),
        Value::Float(f) => Ok(*f as f64),
        _ => Err(TraceMindError::Storage(format!("expected Double, got {:?}", val))),
    }
}

fn extract_opt_string(val: &Value) -> Result<Option<String>> {
    match val {
        Value::String(s) => Ok(Some(s.clone())),
        Value::Null(_) => Ok(None),
        _ => Err(TraceMindError::Storage(format!("expected String|Null, got {:?}", val))),
    }
}

fn parse_entity_row(row: &[Value]) -> Result<Entity> {
    let id: Uuid = extract_string(&row[0])?
        .parse()
        .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
    let name = extract_string(&row[1])?;
    let entity_type: EntityType = serde_json::from_str(&extract_string(&row[2])?)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let confidence = extract_f64(&row[3])?;
    let source_id = extract_opt_string(&row[4])?;
    let created_at: DateTime<Utc> = extract_string(&row[5])?
        .parse()
        .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;
    let updated_at: DateTime<Utc> = extract_string(&row[6])?
        .parse()
        .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;

    Ok(Entity {
        id,
        name,
        entity_type,
        confidence,
        source_id,
        created_at,
        updated_at,
    })
}

fn parse_triple_row(row: &[Value]) -> Result<Triple> {
    let id: Uuid = extract_string(&row[0])?
        .parse()
        .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
    let subject_id: Uuid = extract_string(&row[1])?
        .parse()
        .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
    let predicate: Predicate = serde_json::from_str(&extract_string(&row[2])?)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    let object_id: Uuid = extract_string(&row[3])?
        .parse()
        .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
    let confidence = extract_f64(&row[4])?;
    let source_id = extract_opt_string(&row[5])?;
    let created_at: DateTime<Utc> = extract_string(&row[6])?
        .parse()
        .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;
    let updated_at: DateTime<Utc> = extract_string(&row[7])?
        .parse()
        .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;

    Ok(Triple {
        id,
        subject_id,
        predicate,
        object_id,
        confidence,
        source_id,
        created_at,
        updated_at,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_entity(name: &str, entity_type: EntityType) -> Entity {
        let now = Utc::now();
        Entity {
            id: Uuid::new_v4(),
            name: name.to_string(),
            entity_type,
            confidence: 0.95,
            source_id: Some("test-source".to_string()),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_insert_and_get_entity() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let entity = make_entity("Alice", EntityType::Person);
        store.upsert_entity(&entity).expect("upsert entity");

        let fetched = store.get_entity(entity.id).expect("get entity");

        assert_eq!(fetched.id, entity.id);
        assert_eq!(fetched.name, entity.name);
        assert_eq!(fetched.confidence, entity.confidence);
        assert_eq!(fetched.source_id, entity.source_id);
    }

    #[test]
    fn test_find_entity_by_name() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let entity = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&entity).expect("upsert entity");

        let found = store.find_entity_by_name("Bob").expect("find by name");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, entity.id);

        let not_found = store.find_entity_by_name("Nobody").expect("find by name missing");
        assert!(not_found.is_none());
    }

    #[test]
    fn test_upsert_entity_updates_fields() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let mut entity = make_entity("Carol", EntityType::Person);
        store.upsert_entity(&entity).expect("upsert entity");

        entity.name = "Carol Updated".to_string();
        entity.confidence = 0.5;
        entity.updated_at = Utc::now();
        store.upsert_entity(&entity).expect("upsert entity again");

        let fetched = store.get_entity(entity.id).expect("get entity");
        assert_eq!(fetched.name, "Carol Updated");
        assert_eq!(fetched.confidence, 0.5);
    }

    #[test]
    fn test_upsert_and_get_triples() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).expect("upsert alice");
        store.upsert_entity(&bob).expect("upsert bob");

        let now = Utc::now();
        let triple = Triple {
            id: Uuid::new_v4(),
            subject_id: alice.id,
            predicate: Predicate::RelatedTo,
            object_id: bob.id,
            confidence: 0.9,
            source_id: Some("test".to_string()),
            created_at: now,
            updated_at: now,
        };
        store.upsert_triple(&triple).expect("upsert triple");

        let triples = store.get_triples_for_entity(alice.id).expect("get triples");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].id, triple.id);
        assert_eq!(triples[0].subject_id, alice.id);
        assert_eq!(triples[0].object_id, bob.id);
    }

    #[test]
    fn test_k_hop_neighbors() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        let carol = make_entity("Carol", EntityType::Person);
        store.upsert_entity(&alice).expect("upsert alice");
        store.upsert_entity(&bob).expect("upsert bob");
        store.upsert_entity(&carol).expect("upsert carol");

        let now = Utc::now();
        let t1 = Triple {
            id: Uuid::new_v4(),
            subject_id: alice.id,
            predicate: Predicate::RelatedTo,
            object_id: bob.id,
            confidence: 0.9,
            source_id: None,
            created_at: now,
            updated_at: now,
        };
        let t2 = Triple {
            id: Uuid::new_v4(),
            subject_id: bob.id,
            predicate: Predicate::RelatedTo,
            object_id: carol.id,
            confidence: 0.8,
            source_id: None,
            created_at: now,
            updated_at: now,
        };
        store.upsert_triple(&t1).expect("upsert t1");
        store.upsert_triple(&t2).expect("upsert t2");

        // 1-hop from alice should give bob only
        let one_hop = store.k_hop_neighbors(alice.id, 1).expect("1-hop");
        assert_eq!(one_hop.len(), 1);
        assert_eq!(one_hop[0].id, bob.id);

        // 2-hop from alice should give bob and carol
        let two_hop = store.k_hop_neighbors(alice.id, 2).expect("2-hop");
        assert_eq!(two_hop.len(), 2);
        let ids: HashSet<Uuid> = two_hop.iter().map(|e| e.id).collect();
        assert!(ids.contains(&bob.id));
        assert!(ids.contains(&carol.id));
    }
}
