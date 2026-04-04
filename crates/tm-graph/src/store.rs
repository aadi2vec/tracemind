use rusqlite::{Connection, params};
use tm_types::{Entity, EntityType, Triple, Predicate, TraceMindError, Result};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use std::collections::{HashSet, VecDeque};
use tracing::{debug, info};

pub struct GraphStore {
    conn: Connection,
}

impl GraphStore {
    pub fn open(path: &str) -> Result<Self> {
        info!("[graph] opening SQLite store at {path}");

        let conn = Connection::open(path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                confidence REAL NOT NULL,
                source_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);

            CREATE TABLE IF NOT EXISTS triples (
                id TEXT PRIMARY KEY,
                subject_id TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object_id TEXT NOT NULL,
                confidence REAL NOT NULL,
                source_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject_id);
            CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object_id);
        ").map_err(|e| TraceMindError::Storage(e.to_string()))?;

        debug!("[graph] schema initialized");
        Ok(Self { conn })
    }

    pub fn upsert_entity(&self, entity: &Entity) -> Result<()> {
        let entity_type_json = serde_json::to_string(&entity.entity_type)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        self.conn.execute(
            "INSERT INTO entities (id, name, entity_type, confidence, source_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                entity_type = excluded.entity_type,
                confidence = excluded.confidence,
                source_id = excluded.source_id,
                updated_at = excluded.updated_at",
            params![
                entity.id.to_string(),
                entity.name,
                entity_type_json,
                entity.confidence,
                entity.source_id,
                entity.created_at.to_rfc3339(),
                entity.updated_at.to_rfc3339(),
            ],
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        debug!("[graph] upserted entity id={} name={}", entity.id, entity.name);
        Ok(())
    }

    pub fn get_entity(&self, id: Uuid) -> Result<Entity> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, entity_type, confidence, source_id, created_at, updated_at
             FROM entities WHERE id = ?1"
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let entity = stmt.query_row(params![id.to_string()], |row| {
            let id_str: String = row.get(0)?;
            let name: String = row.get(1)?;
            let entity_type_str: String = row.get(2)?;
            let confidence: f64 = row.get(3)?;
            let source_id: Option<String> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;

            Ok((id_str, name, entity_type_str, confidence, source_id, created_at_str, updated_at_str))
        }).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let (id_str, name, entity_type_str, confidence, source_id, created_at_str, updated_at_str) = entity;

        let id: Uuid = id_str.parse()
            .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
        let entity_type: EntityType = serde_json::from_str(&entity_type_str)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let created_at: DateTime<Utc> = created_at_str.parse()
            .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;
        let updated_at: DateTime<Utc> = updated_at_str.parse()
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

    pub fn find_entity_by_name(&self, name: &str) -> Result<Option<Entity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, entity_type, confidence, source_id, created_at, updated_at
             FROM entities WHERE name = ?1 LIMIT 1"
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let result = stmt.query_row(params![name], |row| {
            let id_str: String = row.get(0)?;
            let name: String = row.get(1)?;
            let entity_type_str: String = row.get(2)?;
            let confidence: f64 = row.get(3)?;
            let source_id: Option<String> = row.get(4)?;
            let created_at_str: String = row.get(5)?;
            let updated_at_str: String = row.get(6)?;

            Ok((id_str, name, entity_type_str, confidence, source_id, created_at_str, updated_at_str))
        });

        match result {
            Ok((id_str, name, entity_type_str, confidence, source_id, created_at_str, updated_at_str)) => {
                let id: Uuid = id_str.parse()
                    .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
                let entity_type: EntityType = serde_json::from_str(&entity_type_str)
                    .map_err(|e| TraceMindError::Storage(e.to_string()))?;
                let created_at: DateTime<Utc> = created_at_str.parse()
                    .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;
                let updated_at: DateTime<Utc> = updated_at_str.parse()
                    .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;

                Ok(Some(Entity {
                    id,
                    name,
                    entity_type,
                    confidence,
                    source_id,
                    created_at,
                    updated_at,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(e.to_string())),
        }
    }

    pub fn upsert_triple(&self, triple: &Triple) -> Result<()> {
        let predicate_json = serde_json::to_string(&triple.predicate)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        self.conn.execute(
            "INSERT INTO triples (id, subject_id, predicate, object_id, confidence, source_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                subject_id = excluded.subject_id,
                predicate = excluded.predicate,
                object_id = excluded.object_id,
                confidence = excluded.confidence,
                source_id = excluded.source_id,
                updated_at = excluded.updated_at",
            params![
                triple.id.to_string(),
                triple.subject_id.to_string(),
                predicate_json,
                triple.object_id.to_string(),
                triple.confidence,
                triple.source_id,
                triple.created_at.to_rfc3339(),
                triple.updated_at.to_rfc3339(),
            ],
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        debug!("[graph] upserted triple id={} ({} -> {})", triple.id, triple.subject_id, triple.object_id);
        Ok(())
    }

    pub fn get_triples_for_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let entity_id_str = entity_id.to_string();

        let mut stmt = self.conn.prepare(
            "SELECT id, subject_id, predicate, object_id, confidence, source_id, created_at, updated_at
             FROM triples WHERE subject_id = ?1 OR object_id = ?1"
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let rows = stmt.query_map(params![entity_id_str], |row| {
            let id_str: String = row.get(0)?;
            let subject_id_str: String = row.get(1)?;
            let predicate_str: String = row.get(2)?;
            let object_id_str: String = row.get(3)?;
            let confidence: f64 = row.get(4)?;
            let source_id: Option<String> = row.get(5)?;
            let created_at_str: String = row.get(6)?;
            let updated_at_str: String = row.get(7)?;

            Ok((id_str, subject_id_str, predicate_str, object_id_str, confidence, source_id, created_at_str, updated_at_str))
        }).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut triples = Vec::new();
        for row in rows {
            let (id_str, subject_id_str, predicate_str, object_id_str, confidence, source_id, created_at_str, updated_at_str) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;

            let id: Uuid = id_str.parse()
                .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
            let subject_id: Uuid = subject_id_str.parse()
                .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
            let predicate: Predicate = serde_json::from_str(&predicate_str)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let object_id: Uuid = object_id_str.parse()
                .map_err(|e: uuid::Error| TraceMindError::Storage(e.to_string()))?;
            let created_at: DateTime<Utc> = created_at_str.parse()
                .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;
            let updated_at: DateTime<Utc> = updated_at_str.parse()
                .map_err(|e: chrono::ParseError| TraceMindError::Storage(e.to_string()))?;

            triples.push(Triple {
                id,
                subject_id,
                predicate,
                object_id,
                confidence,
                source_id,
                created_at,
                updated_at,
            });
        }

        debug!("[graph] found {} triples for entity {entity_id}", triples.len());
        Ok(triples)
    }

    /// Multiply all entity and triple confidence values by `factor` (e.g. 0.95).
    /// Returns the number of entities whose confidence dropped below `threshold`.
    pub fn decay_all(&self, factor: f64, threshold: f64) -> Result<usize> {
        info!("[graph] decaying all confidence by {factor}, threshold={threshold}");

        self.conn.execute(
            "UPDATE entities SET confidence = confidence * ?1, updated_at = ?2",
            params![factor, Utc::now().to_rfc3339()],
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        self.conn.execute(
            "UPDATE triples SET confidence = confidence * ?1, updated_at = ?2",
            params![factor, Utc::now().to_rfc3339()],
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM entities WHERE confidence < ?1",
            params![threshold],
            |row| row.get(0),
        ).map_err(|e| TraceMindError::Storage(e.to_string()))?;

        info!("[graph] decay complete: {count} entities below threshold");
        Ok(count)
    }

    pub fn k_hop_neighbors(&self, entity_id: Uuid, hops: u32) -> Result<Vec<Entity>> {
        if hops == 0 {
            return Ok(vec![]);
        }

        let mut visited: HashSet<Uuid> = HashSet::new();
        let mut queue: VecDeque<(Uuid, u32)> = VecDeque::new();
        let mut neighbor_ids: Vec<Uuid> = Vec::new();

        visited.insert(entity_id);
        queue.push_back((entity_id, 0));

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= hops {
                continue;
            }

            let triples = self.get_triples_for_entity(current_id)?;

            for triple in triples {
                let next_id = if triple.subject_id == current_id {
                    triple.object_id
                } else {
                    triple.subject_id
                };

                if !visited.contains(&next_id) {
                    visited.insert(next_id);
                    neighbor_ids.push(next_id);
                    queue.push_back((next_id, depth + 1));
                }
            }
        }

        let mut entities = Vec::new();
        for id in neighbor_ids {
            match self.get_entity(id) {
                Ok(entity) => entities.push(entity),
                Err(TraceMindError::Storage(ref msg)) if msg.contains("no rows") => {
                    // Entity referenced in a triple but not in entities table; skip
                }
                Err(e) => return Err(e),
            }
        }

        info!("[graph] k_hop({hops}) from {entity_id}: {} neighbors", entities.len());
        Ok(entities)
    }
}

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

    #[test]
    fn test_decay_all_reduces_confidence() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        for name in &["A", "B", "C", "D", "E"] {
            let mut e = make_entity(name, EntityType::Concept);
            e.confidence = 0.5;
            store.upsert_entity(&e).expect("upsert");
        }

        // Decay by 0.1× so all go from 0.5 to 0.05 (at threshold)
        let below = store.decay_all(0.1, 0.05).expect("decay");
        assert_eq!(below, 0); // 0.5 * 0.1 = 0.05, not below

        // Decay again → 0.005, all below 0.05
        let below = store.decay_all(0.1, 0.05).expect("decay");
        assert_eq!(below, 5);

        // Verify actual values
        let e = store.find_entity_by_name("A").expect("find").expect("exists");
        assert!(e.confidence < 0.05, "confidence={}", e.confidence);
    }
}
