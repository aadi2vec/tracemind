//! Graph store backed by sqlite-knowledge-graph.
//!
//! Wraps [`sqlite_knowledge_graph::KnowledgeGraph`] while preserving TraceMind's
//! UUID-based public API. Entities and relations store TraceMind metadata (UUID,
//! confidence, timestamps) as skg JSON properties.
//!
//! **Vector search** is unified into the same SQLite file — callers no longer
//! need a separate LanceDB store.

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde_json::json;
use sqlite_knowledge_graph::{
    Entity as SkgEntity, KnowledgeGraph, Relation as SkgRelation,
};
use tm_types::{Entity, EntityType, Predicate, Result, TraceMindError, Triple};
use tracing::{debug, info};
use uuid::Uuid;

/// Knowledge-graph store backed by a single SQLite file (via sqlite-knowledge-graph).
///
/// Provides entity/triple CRUD, k-hop traversal, vector search, PageRank, and
/// decay — all in one file, no external dependencies.
pub struct GraphStore {
    kg: KnowledgeGraph,
    /// TraceMind UUID → skg i64 entity ID.
    entity_map: RefCell<HashMap<Uuid, i64>>,
    /// TraceMind Triple UUID → skg i64 relation ID.
    triple_map: RefCell<HashMap<Uuid, i64>>,
}

impl GraphStore {
    /// Open (or create) a graph store at `path`.
    ///
    /// Pass `":memory:"` for an ephemeral in-memory store (useful for tests).
    pub fn open(path: &str) -> Result<Self> {
        info!("[graph] opening sqlite-knowledge-graph store at {path}");

        let kg = if path == ":memory:" {
            KnowledgeGraph::open_in_memory()
        } else {
            KnowledgeGraph::open(path)
        }
        .map_err(|e| TraceMindError::Storage(format!("skg open: {e}")))?;

        // Rebuild UUID ↔ skg-id maps from existing data.
        let mut entity_map = HashMap::new();
        let mut triple_map = HashMap::new();

        let entities = kg
            .list_entities(None, None)
            .map_err(|e| TraceMindError::Storage(format!("skg list_entities: {e}")))?;

        for ent in &entities {
            if let Some(skg_id) = ent.id {
                if let Some(uuid) = prop_uuid(ent.get_property("uuid")) {
                    entity_map.insert(uuid, skg_id);
                }
            }
        }

        // Rebuild triple map from relations (skg has no list_all_relations).
        {
            let conn = kg.connection();
            let mut stmt = conn
                .prepare("SELECT id, properties FROM kg_relations")
                .map_err(|e| TraceMindError::Storage(format!("load relations: {e}")))?;

            let rows = stmt
                .query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let props_str: String = row.get(1)?;
                    Ok((id, props_str))
                })
                .map_err(|e| TraceMindError::Storage(format!("query relations: {e}")))?;

            for row in rows {
                let (id, props_str) =
                    row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
                if let Ok(props) =
                    serde_json::from_str::<HashMap<String, serde_json::Value>>(&props_str)
                {
                    if let Some(uuid) = prop_uuid(props.get("uuid")) {
                        triple_map.insert(uuid, id);
                    }
                }
            }
        }

        info!(
            "[graph] loaded {} entities, {} relations from existing store",
            entity_map.len(),
            triple_map.len()
        );

        // Create access_log and captured_signals tables for dynamic ingestion/recommendations.
        let conn = kg.connection();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS access_log (
                id         INTEGER PRIMARY KEY,
                entity_id  TEXT NOT NULL,
                event_type TEXT NOT NULL,
                context    TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_access_entity ON access_log(entity_id, created_at);

            CREATE TABLE IF NOT EXISTS captured_signals (
                id              INTEGER PRIMARY KEY,
                source          TEXT NOT NULL,
                raw_text        TEXT NOT NULL,
                content_hash    INTEGER NOT NULL,
                relevance_score REAL,
                ingested        INTEGER DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_signals_hash ON captured_signals(content_hash);

            CREATE TABLE IF NOT EXISTS retrieval_feedback (
                id           INTEGER PRIMARY KEY,
                entity_id    TEXT NOT NULL,
                retrieved    INTEGER NOT NULL DEFAULT 0,
                succeeded    INTEGER NOT NULL DEFAULT 0,
                UNIQUE(entity_id)
            );
            CREATE INDEX IF NOT EXISTS idx_feedback_entity ON retrieval_feedback(entity_id);

            CREATE TABLE IF NOT EXISTS recommendations (
                id          INTEGER PRIMARY KEY,
                entity_id   TEXT NOT NULL,
                score       REAL NOT NULL,
                reason      TEXT NOT NULL,
                clicked     INTEGER DEFAULT 0,
                query_id    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )
        .map_err(|e| TraceMindError::Storage(format!("create tables: {e}")))?;

        Ok(Self {
            kg,
            entity_map: RefCell::new(entity_map),
            triple_map: RefCell::new(triple_map),
        })
    }

    // ─── Entity CRUD ────────────────────────────────────────────────────

    pub fn upsert_entity(&self, entity: &Entity) -> Result<()> {
        let etype_json = serde_json::to_string(&entity.entity_type)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut map = self.entity_map.borrow_mut();

        if let Some(&skg_id) = map.get(&entity.id) {
            // Update existing.
            let mut skg_ent = self
                .kg
                .get_entity(skg_id)
                .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;

            skg_ent.name = entity.name.clone();
            skg_ent.entity_type = etype_json;
            set_entity_props(&mut skg_ent, entity);

            self.kg
                .update_entity(&skg_ent)
                .map_err(|e| TraceMindError::Storage(format!("skg update_entity: {e}")))?;
        } else {
            // Insert new.
            let mut skg_ent = SkgEntity::new(&etype_json, &entity.name);
            set_entity_props(&mut skg_ent, entity);

            let skg_id = self
                .kg
                .insert_entity(&skg_ent)
                .map_err(|e| TraceMindError::Storage(format!("skg insert_entity: {e}")))?;

            map.insert(entity.id, skg_id);
        }

        debug!("[graph] upserted entity id={} name={}", entity.id, entity.name);
        Ok(())
    }

    pub fn get_entity(&self, id: Uuid) -> Result<Entity> {
        let map = self.entity_map.borrow();
        let &skg_id = map.get(&id).ok_or_else(|| {
            TraceMindError::Storage(format!(
                "Query returned no rows: entity {id} not in id map"
            ))
        })?;

        let skg_ent = self
            .kg
            .get_entity(skg_id)
            .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;

        skg_entity_to_tm(&skg_ent)
    }

    pub fn find_entity_by_id(&self, id: Uuid) -> Result<Option<Entity>> {
        let map = self.entity_map.borrow();
        let skg_id = match map.get(&id) {
            Some(&sid) => sid,
            None => return Ok(None),
        };
        drop(map);

        let conn = self.kg.connection();
        let result = conn.query_row(
            "SELECT entity_type, name, properties FROM kg_entities WHERE id = ?1",
            params![skg_id],
            |row| {
                let entity_type: String = row.get(0)?;
                let name: String = row.get(1)?;
                let props_str: String = row.get(2)?;
                Ok((entity_type, name, props_str))
            },
        );

        match result {
            Ok((etype_str, name, props_str)) => {
                let props: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&props_str).unwrap_or_default();
                let entity = props_to_tm_entity(&etype_str, &name, &props)?;
                Ok(Some(entity))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(e.to_string())),
        }
    }

    pub fn find_entity_by_name(&self, name: &str) -> Result<Option<Entity>> {
        let conn = self.kg.connection();
        let result = conn.query_row(
            "SELECT id, entity_type, name, properties FROM kg_entities WHERE name = ?1 LIMIT 1",
            params![name],
            |row| {
                let _id: i64 = row.get(0)?;
                let entity_type: String = row.get(1)?;
                let name: String = row.get(2)?;
                let props_str: String = row.get(3)?;
                Ok((entity_type, name, props_str))
            },
        );

        match result {
            Ok((etype_str, name, props_str)) => {
                let props: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&props_str).unwrap_or_default();
                let entity = props_to_tm_entity(&etype_str, &name, &props)?;
                Ok(Some(entity))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(e.to_string())),
        }
    }

    /// Case-insensitive entity lookup. Returns the first match.
    pub fn find_entity_by_name_icase(&self, name: &str) -> Result<Option<Entity>> {
        let conn = self.kg.connection();
        let result = conn.query_row(
            "SELECT id, entity_type, name, properties FROM kg_entities WHERE name = ?1 COLLATE NOCASE LIMIT 1",
            params![name],
            |row| {
                let _id: i64 = row.get(0)?;
                let entity_type: String = row.get(1)?;
                let name: String = row.get(2)?;
                let props_str: String = row.get(3)?;
                Ok((entity_type, name, props_str))
            },
        );

        match result {
            Ok((etype_str, name, props_str)) => {
                let props: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&props_str).unwrap_or_default();
                let entity = props_to_tm_entity(&etype_str, &name, &props)?;
                Ok(Some(entity))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(e.to_string())),
        }
    }

    /// Reinforce an existing entity's confidence and update its timestamp.
    pub fn reinforce_entity(&self, id: Uuid, amount: f64) -> Result<()> {
        let map = self.entity_map.borrow();
        let &skg_id = map.get(&id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {id} not in id map"))
        })?;
        drop(map);

        let mut skg_ent = self
            .kg
            .get_entity(skg_id)
            .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;

        let cur = skg_ent
            .get_property("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5);
        let new_conf = (cur + amount).min(1.0);
        skg_ent.set_property("confidence", json!(new_conf));
        skg_ent.set_property("updated_at", json!(Utc::now().to_rfc3339()));

        self.kg
            .update_entity(&skg_ent)
            .map_err(|e| TraceMindError::Storage(format!("skg update_entity: {e}")))?;

        debug!("[graph] reinforced entity {id}: {cur:.3} → {new_conf:.3}");
        Ok(())
    }

    // ─── Triple (Relation) CRUD ─────────────────────────────────────────

    pub fn upsert_triple(&self, triple: &Triple) -> Result<()> {
        let predicate_str = serde_json::to_string(&triple.predicate)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let entity_map = self.entity_map.borrow();
        let &subj_skg = entity_map.get(&triple.subject_id).ok_or_else(|| {
            TraceMindError::Storage(format!("subject {} not in entity map", triple.subject_id))
        })?;
        let &obj_skg = entity_map.get(&triple.object_id).ok_or_else(|| {
            TraceMindError::Storage(format!("object {} not in entity map", triple.object_id))
        })?;
        drop(entity_map);

        // Clamp confidence into [0.0, 1.0] for skg weight validation.
        let weight = triple.confidence.clamp(0.0, 1.0);

        let mut tmap = self.triple_map.borrow_mut();

        if let Some(&skg_id) = tmap.get(&triple.id) {
            // Update via raw SQL (skg has no update_relation).
            let props = triple_props(triple, &predicate_str);
            let conn = self.kg.connection();
            conn.execute(
                "UPDATE kg_relations SET source_id=?1, target_id=?2, rel_type=?3, weight=?4, properties=?5 WHERE id=?6",
                params![subj_skg, obj_skg, predicate_str, weight, props.to_string(), skg_id],
            )
            .map_err(|e| TraceMindError::Storage(format!("update relation: {e}")))?;
        } else {
            // Insert new.
            let mut rel = SkgRelation::new(subj_skg, obj_skg, &predicate_str, weight)
                .map_err(|e| TraceMindError::Storage(format!("skg Relation::new: {e}")))?;

            rel.set_property("uuid", json!(triple.id.to_string()));
            rel.set_property("predicate", json!(predicate_str));
            rel.set_property("source_id", json!(triple.source_id));
            rel.set_property("created_at", json!(triple.created_at.to_rfc3339()));
            rel.set_property("updated_at", json!(triple.updated_at.to_rfc3339()));

            let skg_id = self
                .kg
                .insert_relation(&rel)
                .map_err(|e| TraceMindError::Storage(format!("skg insert_relation: {e}")))?;
            tmap.insert(triple.id, skg_id);
        }

        debug!(
            "[graph] upserted triple id={} ({} -> {})",
            triple.id, triple.subject_id, triple.object_id
        );
        Ok(())
    }

    pub fn get_triples_for_entity(&self, entity_id: Uuid) -> Result<Vec<Triple>> {
        let entity_map = self.entity_map.borrow();
        let &skg_id = entity_map.get(&entity_id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {entity_id} not in id map"))
        })?;

        // Build reverse map: skg i64 → UUID.
        let reverse: HashMap<i64, Uuid> =
            entity_map.iter().map(|(&uuid, &sid)| (sid, uuid)).collect();
        drop(entity_map);

        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id, source_id, target_id, rel_type, weight, properties \
                 FROM kg_relations WHERE source_id = ?1 OR target_id = ?1",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![skg_id], |row| {
                let id: i64 = row.get(0)?;
                let source_id: i64 = row.get(1)?;
                let target_id: i64 = row.get(2)?;
                let rel_type: String = row.get(3)?;
                let weight: f64 = row.get(4)?;
                let props_str: String = row.get(5)?;
                Ok((id, source_id, target_id, rel_type, weight, props_str))
            })
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut triples = Vec::new();
        for row in rows {
            let (_skg_id, src_skg, tgt_skg, rel_type, weight, props_str) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;

            let props: HashMap<String, serde_json::Value> =
                serde_json::from_str(&props_str).unwrap_or_default();

            let triple_uuid = prop_uuid(props.get("uuid")).unwrap_or_else(Uuid::new_v4);
            let subject_uuid = reverse.get(&src_skg).copied().unwrap_or_else(Uuid::new_v4);
            let object_uuid = reverse.get(&tgt_skg).copied().unwrap_or_else(Uuid::new_v4);
            let predicate: Predicate =
                serde_json::from_str(&rel_type).unwrap_or(Predicate::RelatedTo);
            let source_id = prop_string(props.get("source_id"));
            let created_at = prop_datetime(props.get("created_at"));
            let updated_at = prop_datetime(props.get("updated_at"));

            triples.push(Triple {
                id: triple_uuid,
                subject_id: subject_uuid,
                predicate,
                object_id: object_uuid,
                confidence: weight,
                source_id,
                created_at,
                updated_at,
            });
        }

        debug!(
            "[graph] found {} triples for entity {entity_id}",
            triples.len()
        );
        Ok(triples)
    }

    // ─── Counts ─────────────────────────────────────────────────────────

    pub fn entity_count(&self) -> Result<usize> {
        self.kg
            .connection()
            .query_row("SELECT COUNT(*) FROM kg_entities", [], |row| row.get(0))
            .map_err(|e| TraceMindError::Storage(e.to_string()))
    }

    pub fn triple_count(&self) -> Result<usize> {
        self.kg
            .connection()
            .query_row("SELECT COUNT(*) FROM kg_relations", [], |row| row.get(0))
            .map_err(|e| TraceMindError::Storage(e.to_string()))
    }

    // ─── Decay ──────────────────────────────────────────────────────────

    /// Multiply all entity and relation confidence/weight by `factor`.
    /// Returns the number of entities whose confidence dropped below `threshold`.
    pub fn decay_all(&self, factor: f64, threshold: f64) -> Result<usize> {
        info!("[graph] decaying all confidence by {factor}, threshold={threshold}");

        // Decay entity confidence (stored in JSON properties).
        let entities = self
            .kg
            .list_entities(None, None)
            .map_err(|e| TraceMindError::Storage(format!("list entities for decay: {e}")))?;

        for ent in &entities {
            if ent.id.is_some() {
                let mut updated = ent.clone();
                let cur = ent
                    .get_property("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                updated.set_property("confidence", json!(cur * factor));
                updated.set_property("updated_at", json!(Utc::now().to_rfc3339()));
                let _ = self.kg.update_entity(&updated);
            }
        }

        // Decay relation weights directly.
        self.kg
            .connection()
            .execute(
                "UPDATE kg_relations SET weight = weight * ?1",
                params![factor],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        // Count entities below threshold.
        let count = self
            .kg
            .list_entities(None, None)
            .map_err(|e| TraceMindError::Storage(format!("list entities: {e}")))?
            .iter()
            .filter(|e| {
                e.get_property("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0)
                    < threshold
            })
            .count();

        info!("[graph] decay complete: {count} entities below threshold");
        Ok(count)
    }

    // ─── Graph traversal ────────────────────────────────────────────────

    pub fn k_hop_neighbors(&self, entity_id: Uuid, hops: u32) -> Result<Vec<Entity>> {
        if hops == 0 {
            return Ok(vec![]);
        }

        let entity_map = self.entity_map.borrow();
        let &skg_id = entity_map.get(&entity_id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {entity_id} not in id map"))
        })?;
        drop(entity_map);

        let neighbors = self
            .kg
            .get_neighbors(skg_id, hops)
            .map_err(|e| TraceMindError::Storage(format!("skg get_neighbors: {e}")))?;

        let mut entities = Vec::new();
        for neighbor in &neighbors {
            if let Ok(entity) = skg_entity_to_tm(&neighbor.entity) {
                entities.push(entity);
            }
        }

        info!(
            "[graph] k_hop({hops}) from {entity_id}: {} neighbors",
            entities.len()
        );
        Ok(entities)
    }

    // ─── Vector search (replaces LanceDB) ───────────────────────────────

    /// Insert or replace the embedding vector for an entity.
    pub fn upsert_vector(&self, entity_id: Uuid, embedding: &[f32]) -> Result<()> {
        let entity_map = self.entity_map.borrow();
        let &skg_id = entity_map.get(&entity_id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {entity_id} not in id map for vector"))
        })?;
        drop(entity_map);

        self.kg
            .insert_vector(skg_id, embedding.to_vec())
            .map_err(|e| TraceMindError::Storage(format!("skg insert_vector: {e}")))?;

        debug!("[graph] upserted vector for entity {entity_id}");
        Ok(())
    }

    /// Return the `top_k` most similar entities by cosine similarity.
    ///
    /// Returns `(entity_uuid, similarity)` pairs sorted descending.
    pub fn search_vectors(&self, query: &[f32], top_k: usize) -> Result<Vec<(Uuid, f32)>> {
        let results = self
            .kg
            .search_vectors(query.to_vec(), top_k)
            .map_err(|e| TraceMindError::Storage(format!("skg search_vectors: {e}")))?;

        let entity_map = self.entity_map.borrow();
        let reverse: HashMap<i64, Uuid> =
            entity_map.iter().map(|(&uuid, &sid)| (sid, uuid)).collect();
        drop(entity_map);

        let mut out = Vec::new();
        for r in results {
            if let Some(&uuid) = reverse.get(&r.entity_id) {
                out.push((uuid, r.similarity));
            }
        }

        info!("[graph] vector search returned {} results", out.len());
        Ok(out)
    }

    // ─── Get vector for an entity ───────────────────────────────────────

    /// Retrieve the embedding vector for a single entity, if it exists.
    pub fn get_vector(&self, entity_id: Uuid) -> Result<Option<Vec<f32>>> {
        let entity_map = self.entity_map.borrow();
        let &skg_id = match entity_map.get(&entity_id) {
            Some(id) => id,
            None => return Ok(None),
        };
        drop(entity_map);

        let conn = self.kg.connection();
        let result: std::result::Result<Vec<u8>, _> = conn.query_row(
            "SELECT vector FROM kg_vectors WHERE entity_id = ?1",
            params![skg_id],
            |row| row.get(0),
        );

        match result {
            Ok(blob) => {
                // skg stores vectors as f32 little-endian bytes
                let floats: Vec<f32> = blob
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                Ok(Some(floats))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(format!("get_vector: {e}"))),
        }
    }

    // ─── Access log (for recommendations) ────────────────────────────────

    /// Log an entity access event (query_result, clicked, recommended, ingested).
    pub fn log_access(&self, entity_id: Uuid, event_type: &str, context: Option<&str>) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "INSERT INTO access_log (entity_id, event_type, context) VALUES (?1, ?2, ?3)",
            params![entity_id.to_string(), event_type, context],
        )
        .map_err(|e| TraceMindError::Storage(format!("log_access: {e}")))?;
        Ok(())
    }

    // ─── Retrieval feedback (MIA-style value scoring) ──────────────────

    /// Record that an entity was returned in a retrieval result set.
    pub fn record_retrieval(&self, entity_id: Uuid) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "INSERT INTO retrieval_feedback (entity_id, retrieved, succeeded) VALUES (?1, 1, 0)
             ON CONFLICT(entity_id) DO UPDATE SET retrieved = retrieved + 1",
            params![entity_id.to_string()],
        ).map_err(|e| TraceMindError::Storage(format!("record_retrieval: {e}")))?;
        Ok(())
    }

    /// Record positive feedback for entities in a result set.
    pub fn record_success(&self, entity_ids: &[Uuid]) -> Result<()> {
        let conn = self.kg.connection();
        for id in entity_ids {
            conn.execute(
                "UPDATE retrieval_feedback SET succeeded = succeeded + 1 WHERE entity_id = ?1",
                params![id.to_string()],
            ).map_err(|e| TraceMindError::Storage(format!("record_success: {e}")))?;
        }
        Ok(())
    }

    /// Batch fetch value scores: succeeded / (retrieved + 1) for each entity.
    pub fn batch_value_scores(&self, entity_ids: &[Uuid]) -> HashMap<Uuid, f64> {
        let mut result = HashMap::new();
        if entity_ids.is_empty() {
            return result;
        }

        let conn = self.kg.connection();
        let id_strs: Vec<String> = entity_ids.iter().map(|id| format!("'{}'", id)).collect();
        let sql = format!(
            "SELECT entity_id, CAST(succeeded AS REAL) / (retrieved + 1) AS value_score \
             FROM retrieval_feedback WHERE entity_id IN ({})",
            id_strs.join(",")
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map([], |row| {
                let id_str: String = row.get(0)?;
                let score: f64 = row.get(1)?;
                Ok((id_str, score))
            }) {
                for row in rows.flatten() {
                    if let Ok(id) = Uuid::parse_str(&row.0) {
                        result.insert(id, row.1);
                    }
                }
            }
        }
        result
    }

    /// Batch fetch frequency scores: 1.0 / (retrieved + 1) for each entity.
    pub fn batch_frequency_scores(&self, entity_ids: &[Uuid]) -> HashMap<Uuid, f64> {
        let mut result = HashMap::new();
        if entity_ids.is_empty() {
            return result;
        }

        let conn = self.kg.connection();
        let id_strs: Vec<String> = entity_ids.iter().map(|id| format!("'{}'", id)).collect();
        let sql = format!(
            "SELECT entity_id, 1.0 / (retrieved + 1) AS freq_score \
             FROM retrieval_feedback WHERE entity_id IN ({})",
            id_strs.join(",")
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map([], |row| {
                let id_str: String = row.get(0)?;
                let score: f64 = row.get(1)?;
                Ok((id_str, score))
            }) {
                for row in rows.flatten() {
                    if let Ok(id) = Uuid::parse_str(&row.0) {
                        result.insert(id, row.1);
                    }
                }
            }
        }
        result
    }

    /// Recency score: exp(-lambda * hours_since_last_access). Half-life ~14h.
    pub fn recency_score(&self, entity_id: Uuid) -> f64 {
        let conn = self.kg.connection();
        let result: std::result::Result<String, _> = conn.query_row(
            "SELECT MAX(created_at) FROM access_log WHERE entity_id = ?1",
            params![entity_id.to_string()],
            |row| row.get(0),
        );
        match result {
            Ok(ts) => {
                if let Ok(dt) = ts.parse::<DateTime<Utc>>() {
                    let hours = (Utc::now() - dt).num_minutes() as f64 / 60.0;
                    (-0.05 * hours).exp()
                } else {
                    0.0
                }
            }
            Err(_) => 0.0,
        }
    }

    /// Novelty score: 1.0 / (1.0 + ln(1 + access_count)). Less seen = more novel.
    pub fn novelty_score(&self, entity_id: Uuid) -> f64 {
        let conn = self.kg.connection();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM access_log WHERE entity_id = ?1",
                params![entity_id.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(0);
        1.0 / (1.0 + (1.0 + count as f64).ln())
    }

    // ─── Captured signals ────────────────────────────────────────────────

    /// Log a captured signal (clipboard, query, shell, etc.) with its ingestion status.
    pub fn log_signal(
        &self,
        source: &str,
        raw_text: &str,
        content_hash: u64,
        relevance_score: Option<f64>,
        ingested: bool,
    ) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "INSERT INTO captured_signals (source, raw_text, content_hash, relevance_score, ingested) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![source, raw_text, content_hash as i64, relevance_score, ingested as i32],
        )
        .map_err(|e| TraceMindError::Storage(format!("log_signal: {e}")))?;
        Ok(())
    }

    /// Check if a signal with the given content hash was already seen.
    pub fn signal_exists(&self, content_hash: u64) -> bool {
        let conn = self.kg.connection();
        conn.query_row(
            "SELECT 1 FROM captured_signals WHERE content_hash = ?1 LIMIT 1",
            params![content_hash as i64],
            |_| Ok(()),
        )
        .is_ok()
    }

    // ─── Graph algorithms (new capabilities) ────────────────────────────

    /// Run PageRank on the knowledge graph. Returns entity UUID → score.
    pub fn pagerank(&self) -> Result<HashMap<Uuid, f64>> {
        let config = sqlite_knowledge_graph::PageRankConfig::default();
        let scores = sqlite_knowledge_graph::pagerank(self.kg.connection(), config)
            .map_err(|e| TraceMindError::Storage(format!("pagerank: {e}")))?;

        let entity_map = self.entity_map.borrow();
        let reverse: HashMap<i64, Uuid> =
            entity_map.iter().map(|(&uuid, &sid)| (sid, uuid)).collect();

        let mut result = HashMap::new();
        for (skg_id, score) in scores {
            if let Some(&uuid) = reverse.get(&skg_id) {
                result.insert(uuid, score);
            }
        }
        Ok(result)
    }

    /// Run Louvain community detection. Returns entity UUID → community ID.
    pub fn louvain(&self) -> Result<HashMap<Uuid, i32>> {
        let result = self
            .kg
            .kg_louvain()
            .map_err(|e| TraceMindError::Storage(format!("louvain: {e}")))?;

        let entity_map = self.entity_map.borrow();
        let reverse: HashMap<i64, Uuid> =
            entity_map.iter().map(|(&uuid, &sid)| (sid, uuid)).collect();

        let mut communities = HashMap::new();
        for (skg_id, community_id) in result.memberships {
            if let Some(&uuid) = reverse.get(&skg_id) {
                communities.insert(uuid, community_id);
            }
        }
        info!(
            "[graph] louvain: {} communities, modularity={:.4}",
            result.num_communities, result.modularity
        );
        Ok(communities)
    }

    /// Batch recency scores for multiple entities in a single query.
    pub fn batch_recency_scores(&self, entity_ids: &[Uuid]) -> HashMap<Uuid, f64> {
        let mut scores = HashMap::new();
        if entity_ids.is_empty() {
            return scores;
        }
        let conn = self.kg.connection();
        // Use a single query to get max access time for all entities
        let placeholders: Vec<String> = entity_ids.iter().map(|id| format!("'{}'", id)).collect();
        let sql = format!(
            "SELECT entity_id, MAX(created_at) FROM access_log WHERE entity_id IN ({}) GROUP BY entity_id",
            placeholders.join(",")
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map([], |row| {
                let id_str: String = row.get(0)?;
                let ts: String = row.get(1)?;
                Ok((id_str, ts))
            }) {
                for row in rows.flatten() {
                    if let Ok(uuid) = Uuid::parse_str(&row.0) {
                        if let Ok(dt) = row.1.parse::<DateTime<Utc>>() {
                            let hours = (Utc::now() - dt).num_minutes() as f64 / 60.0;
                            scores.insert(uuid, (-0.05 * hours).exp());
                        }
                    }
                }
            }
        }
        // Fill missing with 0.0
        for id in entity_ids {
            scores.entry(*id).or_insert(0.0);
        }
        scores
    }

    /// Batch novelty scores for multiple entities in a single query.
    pub fn batch_novelty_scores(&self, entity_ids: &[Uuid]) -> HashMap<Uuid, f64> {
        let mut scores = HashMap::new();
        if entity_ids.is_empty() {
            return scores;
        }
        let conn = self.kg.connection();
        let placeholders: Vec<String> = entity_ids.iter().map(|id| format!("'{}'", id)).collect();
        let sql = format!(
            "SELECT entity_id, COUNT(*) FROM access_log WHERE entity_id IN ({}) GROUP BY entity_id",
            placeholders.join(",")
        );
        if let Ok(mut stmt) = conn.prepare(&sql) {
            if let Ok(rows) = stmt.query_map([], |row| {
                let id_str: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((id_str, count))
            }) {
                for row in rows.flatten() {
                    if let Ok(uuid) = Uuid::parse_str(&row.0) {
                        scores.insert(uuid, 1.0 / (1.0 + (1.0 + row.1 as f64).ln()));
                    }
                }
            }
        }
        // Fill missing with max novelty (1.0 / (1.0 + ln(1)) = 1.0)
        for id in entity_ids {
            scores.entry(*id).or_insert(1.0 / (1.0 + 1.0f64.ln()));
        }
        scores
    }

    /// Delete an entity and all its associated relations and vectors.
    pub fn delete_entity(&self, entity_id: Uuid) -> Result<()> {
        let mut map = self.entity_map.borrow_mut();
        let &skg_id = map.get(&entity_id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {entity_id} not in id map"))
        })?;

        let conn = self.kg.connection();

        // Delete relations involving this entity
        conn.execute(
            "DELETE FROM kg_relations WHERE source_id = ?1 OR target_id = ?1",
            params![skg_id],
        )
        .map_err(|e| TraceMindError::Storage(format!("delete relations: {e}")))?;

        // Delete vectors for this entity
        conn.execute(
            "DELETE FROM kg_vectors WHERE entity_id = ?1",
            params![skg_id],
        )
        .map_err(|e| TraceMindError::Storage(format!("delete vectors: {e}")))?;

        // Delete the entity itself
        conn.execute(
            "DELETE FROM kg_entities WHERE id = ?1",
            params![skg_id],
        )
        .map_err(|e| TraceMindError::Storage(format!("delete entity: {e}")))?;

        // Clean up access log
        conn.execute(
            "DELETE FROM access_log WHERE entity_id = ?1",
            params![entity_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("delete access_log: {e}")))?;

        // Remove from maps
        map.remove(&entity_id);

        // Remove triple map entries that reference this entity's relations
        let mut tmap = self.triple_map.borrow_mut();
        tmap.retain(|_, _| true); // We can't easily filter by entity, but the DB rows are gone

        info!("[graph] deleted entity {entity_id} and all associated data");
        Ok(())
    }

    /// List all entities (for graph visualization, export, etc.).
    pub fn list_all_entities(&self) -> Result<Vec<Entity>> {
        let entities = self
            .kg
            .list_entities(None, None)
            .map_err(|e| TraceMindError::Storage(format!("list entities: {e}")))?;

        let mut result = Vec::new();
        for ent in &entities {
            if let Ok(entity) = skg_entity_to_tm(ent) {
                result.push(entity);
            }
        }
        Ok(result)
    }

    /// Access the underlying `KnowledgeGraph` for advanced operations
    /// (BFS/DFS traversal, export, etc.).
    pub fn inner(&self) -> &KnowledgeGraph {
        &self.kg
    }

    // ─── KG-R1 Schema-Agnostic Graph Actions ───────────────────────────
    //
    // Minimal 4-action API inspired by KG-R1 (arXiv:2509.26383).
    // These 4 operations are provably sufficient to traverse any path in
    // a directed knowledge graph, and constrain the action space for
    // future learned traversal policies.

    /// KG-R1 Action 1: Get all outgoing predicates from an entity.
    /// Returns `Vec<(Predicate, Uuid)>` — the predicate and target entity ID.
    pub fn outgoing_predicates(&self, entity_id: Uuid) -> Result<Vec<(Predicate, Uuid)>> {
        let triples = self.get_triples_for_entity(entity_id)?;
        Ok(triples
            .into_iter()
            .filter(|t| t.subject_id == entity_id)
            .map(|t| (t.predicate, t.object_id))
            .collect())
    }

    /// KG-R1 Action 2: Get all incoming predicates pointing to an entity.
    /// Returns `Vec<(Predicate, Uuid)>` — the predicate and source entity ID.
    pub fn incoming_predicates(&self, entity_id: Uuid) -> Result<Vec<(Predicate, Uuid)>> {
        let triples = self.get_triples_for_entity(entity_id)?;
        Ok(triples
            .into_iter()
            .filter(|t| t.object_id == entity_id)
            .map(|t| (t.predicate, t.subject_id))
            .collect())
    }

    /// KG-R1 Action 3: Follow a specific predicate forward from an entity.
    /// Returns all target entities reachable via `predicate` from `entity_id`.
    pub fn follow_predicate(&self, entity_id: Uuid, predicate: &Predicate) -> Result<Vec<Entity>> {
        let outgoing = self.outgoing_predicates(entity_id)?;
        let mut results = Vec::new();
        for (pred, target_id) in outgoing {
            if &pred == predicate {
                if let Ok(entity) = self.get_entity(target_id) {
                    results.push(entity);
                }
            }
        }
        Ok(results)
    }

    /// KG-R1 Action 4: Reverse-follow a predicate — find entities that point
    /// to `entity_id` via `predicate`.
    pub fn reverse_follow(&self, entity_id: Uuid, predicate: &Predicate) -> Result<Vec<Entity>> {
        let incoming = self.incoming_predicates(entity_id)?;
        let mut results = Vec::new();
        for (pred, source_id) in incoming {
            if &pred == predicate {
                if let Ok(entity) = self.get_entity(source_id) {
                    results.push(entity);
                }
            }
        }
        Ok(results)
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────

fn set_entity_props(skg_ent: &mut SkgEntity, entity: &Entity) {
    skg_ent.set_property("uuid", json!(entity.id.to_string()));
    skg_ent.set_property("confidence", json!(entity.confidence));
    skg_ent.set_property("source_id", json!(entity.source_id));
    skg_ent.set_property("created_at", json!(entity.created_at.to_rfc3339()));
    skg_ent.set_property("updated_at", json!(entity.updated_at.to_rfc3339()));
}

fn triple_props(triple: &Triple, predicate_str: &str) -> serde_json::Value {
    json!({
        "uuid": triple.id.to_string(),
        "predicate": predicate_str,
        "source_id": triple.source_id,
        "created_at": triple.created_at.to_rfc3339(),
        "updated_at": triple.updated_at.to_rfc3339(),
    })
}

fn skg_entity_to_tm(skg_ent: &SkgEntity) -> Result<Entity> {
    let uuid = prop_uuid(skg_ent.get_property("uuid"))
        .ok_or_else(|| TraceMindError::Storage("entity missing uuid property".into()))?;

    let entity_type: EntityType =
        serde_json::from_str(&skg_ent.entity_type).unwrap_or(EntityType::Concept);

    let confidence = skg_ent
        .get_property("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);

    let source_id = prop_string(skg_ent.get_property("source_id"));
    let created_at = prop_datetime(skg_ent.get_property("created_at"));
    let updated_at = prop_datetime(skg_ent.get_property("updated_at"));

    Ok(Entity {
        id: uuid,
        name: skg_ent.name.clone(),
        entity_type,
        confidence,
        source_id,
        created_at,
        updated_at,
    })
}

fn props_to_tm_entity(
    entity_type_str: &str,
    name: &str,
    props: &HashMap<String, serde_json::Value>,
) -> Result<Entity> {
    let uuid = prop_uuid(props.get("uuid"))
        .ok_or_else(|| TraceMindError::Storage("entity missing uuid property".into()))?;

    let entity_type: EntityType =
        serde_json::from_str(entity_type_str).unwrap_or(EntityType::Concept);

    let confidence = props
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);

    let source_id = prop_string(props.get("source_id"));
    let created_at = prop_datetime(props.get("created_at"));
    let updated_at = prop_datetime(props.get("updated_at"));

    Ok(Entity {
        id: uuid,
        name: name.to_string(),
        entity_type,
        confidence,
        source_id,
        created_at,
        updated_at,
    })
}

/// Extract a UUID from a JSON property value.
fn prop_uuid(val: Option<&serde_json::Value>) -> Option<Uuid> {
    val.and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// Extract an optional string from a JSON property value.
fn prop_string(val: Option<&serde_json::Value>) -> Option<String> {
    val.and_then(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().map(|s| s.to_string())
        }
    })
}

/// Extract a DateTime<Utc> from a JSON property value, defaulting to now.
fn prop_datetime(val: Option<&serde_json::Value>) -> DateTime<Utc> {
    val.and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now)
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashSet;
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

        let not_found = store
            .find_entity_by_name("Nobody")
            .expect("find by name missing");
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

        let triples = store
            .get_triples_for_entity(alice.id)
            .expect("get triples");
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
        let e = store
            .find_entity_by_name("A")
            .expect("find")
            .expect("exists");
        assert!(e.confidence < 0.05, "confidence={}", e.confidence);
    }

    #[test]
    fn test_entity_and_triple_counts() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        assert_eq!(store.entity_count().unwrap(), 0);
        assert_eq!(store.triple_count().unwrap(), 0);

        let a = make_entity("A", EntityType::Person);
        let b = make_entity("B", EntityType::Person);
        store.upsert_entity(&a).unwrap();
        store.upsert_entity(&b).unwrap();

        assert_eq!(store.entity_count().unwrap(), 2);

        let t = Triple::new(a.id, Predicate::RelatedTo, b.id, 0.8);
        store.upsert_triple(&t).unwrap();
        assert_eq!(store.triple_count().unwrap(), 1);
    }

    #[test]
    fn test_vector_upsert_and_search() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");

        let a = make_entity("Alpha", EntityType::Concept);
        let b = make_entity("Beta", EntityType::Concept);
        let c = make_entity("Gamma", EntityType::Concept);
        store.upsert_entity(&a).unwrap();
        store.upsert_entity(&b).unwrap();
        store.upsert_entity(&c).unwrap();

        // Insert vectors
        store.upsert_vector(a.id, &[1.0, 0.0, 0.0]).unwrap();
        store.upsert_vector(b.id, &[0.0, 1.0, 0.0]).unwrap();
        store.upsert_vector(c.id, &[0.9, 0.1, 0.0]).unwrap();

        // Search should return Alpha (exact) then Gamma (close)
        let results = store.search_vectors(&[1.0, 0.0, 0.0], 3).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, a.id);
        assert_eq!(results[1].0, c.id);
    }
}
