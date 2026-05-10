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
use tm_tms::BeliefStatus;
use tm_types::{Entity, EntityType, Predicate, Result, TraceMindError, Triple};
use tracing::{debug, info};
use uuid::Uuid;

use crate::belief::{BeliefStore, ContradictionView};

/// A raw captured signal from the fast-path ingestion pipeline.
/// Signals are stored with their embedding but without NER/triple extraction.
/// The slow-path consolidation pass clusters signals and promotes clusters to entities.
#[derive(Debug, Clone)]
pub struct CapturedSignal {
    pub id: i64,
    pub source: String,
    pub raw_text: String,
    pub content_hash: u64,
    pub session_id: Option<Uuid>,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

/// Human-readable projection of a triple, for the brief contradiction
/// drawer. Combines the typed-triple row with both endpoint entity
/// names + the current belief status. Built for UI consumption — the
/// fields are flat and serializable so the Tauri/MCP layers can pass
/// it straight through without any further reshaping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TripleDetail {
    pub triple_id: Uuid,
    pub subject_id: Uuid,
    pub subject_name: String,
    pub subject_type: String,
    pub predicate: String,
    pub object_id: Uuid,
    pub object_name: String,
    pub object_type: String,
    pub confidence: f64,
    /// Capture / trace id this triple originated from. May be `None`
    /// for triples that were upserted without an attached source.
    pub source_id: Option<String>,
    pub ingested_at: DateTime<Utc>,
    /// Current JTMS status — `In` / `Out` / `Contradicted`. `None`
    /// means the belief engine hasn't been built yet (shouldn't
    /// happen for live triples but worth surfacing for diagnostics).
    pub status: Option<BeliefStatus>,
}

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
    /// Bitemporal revision history for entities and triples (Sprint C-1).
    /// Live state lives in `kg_entities` / `kg_relations`; this records
    /// every assertion + supersession so we can answer
    /// `entity_at(t)` / `triple_at(t)` and `*_history(id)`.
    /// Separate SQLite connection on the same DB file (or a separate
    /// `:memory:` instance for tests). See `temporal_facts` schema in
    /// `tm-temporal`.
    temporal: tm_temporal::TemporalStore,
    /// JTMS-backed belief store (Sprint C-2). Mirrors every triple
    /// upsert as a belief assertion; surfaces `BeliefStatus` to
    /// retrieval / brief without persisting itself — rebuilt at
    /// `open()` from the live triple set.
    beliefs: BeliefStore,
    /// Sidecar JSON path for persisted contradictions. The belief
    /// engine itself rebuilds deterministically from live triples,
    /// but contradictions need their cosine input to re-detect, which
    /// the graph doesn't keep. Saving the contradiction list here
    /// lets the brief surface them across CLI invocations.
    /// `None` when the graph is in-memory (`:memory:`).
    beliefs_path: Option<std::path::PathBuf>,
}

/// String tag stored in `temporal_facts.fact_type` for entity revisions.
const FACT_TYPE_ENTITY: &str = "graph_entity";
/// String tag stored in `temporal_facts.fact_type` for triple revisions.
const FACT_TYPE_TRIPLE: &str = "graph_triple";

/// Map a `tm_temporal::StoreError` into `TraceMindError::Storage` so the
/// graph-side `Result` chain stays homogeneous. Used everywhere the
/// bitemporal substrate is touched from `GraphStore`.
fn temporal_err(e: tm_temporal::StoreError) -> TraceMindError {
    TraceMindError::Storage(format!("temporal: {e}"))
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
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                session_id      TEXT,
                embedding       BLOB,
                cluster_id      INTEGER,
                promoted_entity TEXT,
                priority_tier   INTEGER DEFAULT 3
            );
            CREATE INDEX IF NOT EXISTS idx_signals_hash ON captured_signals(content_hash);
            CREATE INDEX IF NOT EXISTS idx_signals_cluster ON captured_signals(cluster_id);
            CREATE INDEX IF NOT EXISTS idx_signals_tier ON captured_signals(priority_tier, cluster_id);

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
            );

            CREATE TABLE IF NOT EXISTS colbert_tokens (
                entity_id   TEXT PRIMARY KEY,
                model_id    TEXT NOT NULL,
                token_count INTEGER NOT NULL,
                dim         INTEGER NOT NULL,
                embeddings  BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )
        .map_err(|e| TraceMindError::Storage(format!("create tables: {e}")))?;

        // Sprint C-0: context segmentation schema (contexts +
        // negative_signals tables, captured_signals.context_id column).
        // Idempotent — safe to call on every open.
        crate::context::init_schema(conn)?;

        // Migrate existing DBs that lack the two-speed pipeline columns.
        {
            let conn = kg.connection();
            for (col, ty) in &[
                ("session_id", "TEXT"),
                ("embedding", "BLOB"),
                ("cluster_id", "INTEGER"),
                ("promoted_entity", "TEXT"),
                ("priority_tier", "INTEGER DEFAULT 3"),
            ] {
                let sql = format!("ALTER TABLE captured_signals ADD COLUMN {col} {ty}");
                let _ = conn.execute(&sql, []); // Ignore error if column already exists
            }
            let _ = conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_signals_cluster ON captured_signals(cluster_id)",
                [],
            );
            let _ = conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_signals_tier ON captured_signals(priority_tier, cluster_id)",
                [],
            );
        }

        // Sprint C-1: open the bitemporal store. For on-disk graph DBs we
        // co-locate `temporal_facts` in a sibling file (`<path>.temporal`)
        // — two SQLite connections on the *same* file is technically safe
        // but stays in WAL contention; a sibling file is simpler and keeps
        // skg's schema completely untouched. For `:memory:` we get a fresh
        // ephemeral store, which is the correct test behaviour.
        let temporal_path = if path == ":memory:" {
            ":memory:".to_string()
        } else {
            format!("{path}.temporal")
        };
        let temporal = tm_temporal::TemporalStore::open(&temporal_path)
            .map_err(|e| TraceMindError::Storage(format!("temporal open: {e}")))?;

        // Sidecar where contradictions are persisted across restarts.
        // None for `:memory:` — ephemeral stores never persist anything.
        let beliefs_path = if path == ":memory:" {
            None
        } else {
            Some(std::path::PathBuf::from(format!("{path}.contradictions.json")))
        };

        let store = Self {
            kg,
            entity_map: RefCell::new(entity_map),
            triple_map: RefCell::new(triple_map),
            temporal,
            beliefs: BeliefStore::new(),
            beliefs_path,
        };

        // One-time backfill: emit a temporal fact for any entity / triple
        // that doesn't yet have one. Idempotent — `current_fact_id` skips
        // anything already tracked. Cheap (linear in #rows missing a fact).
        store.backfill_temporal()?;

        // Sprint C-2: rebuild the in-memory belief engine from live triples,
        // then replay any persisted contradictions so the brief surfaces
        // them across CLI invocations. Belief assertions are deterministic
        // from live triples; contradictions need a sidecar because the
        // cosine input that drove detection isn't recoverable from the graph.
        store.rebuild_beliefs()?;
        if let Some(ref p) = store.beliefs_path {
            if let Ok(n) = store.beliefs.replay_contradictions(p) {
                if n > 0 {
                    info!("[graph] belief engine: {n} contradictions replayed");
                }
            }
        }

        Ok(store)
    }

    /// Detect a contradiction between two triples and persist it so it
    /// survives across CLI restarts. Wraps `BeliefStore::detect_contradiction`
    /// + `save_contradictions`. Use this from any caller (ingest pipeline,
    /// demo fixture) that has the cosine on hand and wants the brief to
    /// remember the pair.
    pub fn record_contradiction(
        &self,
        triple_a: Uuid,
        triple_b: Uuid,
        cosine_sim: f32,
    ) -> Option<ContradictionView> {
        let view = self.beliefs.detect_contradiction(triple_a, triple_b, cosine_sim)?;
        if let Some(ref p) = self.beliefs_path {
            // Best-effort save — if the disk is full or read-only, the
            // in-memory contradiction is still surfaceable for the rest
            // of this process. We log instead of bubbling so callers
            // (ingest, demo restore) don't fail on a sidecar issue.
            if let Err(e) = self.beliefs.save_contradictions(p) {
                tracing::warn!("[graph] failed to persist contradictions: {e}");
            }
        }
        Some(view)
    }

    /// Resolve a previously detected contradiction. Drives the
    /// retraction beat in the brief / Tauri drawer:
    ///
    /// - `KeepA`     → retract `triple_b` in JTMS (status `Out`,
    ///                 `effective_confidence` returns 0 → never
    ///                 surfaces in retrieval).
    /// - `KeepB`     → symmetric.
    /// - `KeepBoth`  → both stay `In`, contradiction marked resolved
    ///                 so the brief stops surfacing it.
    ///
    /// Re-saves the sidecar after the resolution so the new statuses
    /// survive a restart. The triples themselves are *not* hard-
    /// deleted — `Out` is enough to filter them at retrieval and
    /// keeps the bitemporal substrate honest about what was once
    /// believed.
    pub fn resolve_contradiction(
        &self,
        contradiction_id: Uuid,
        choice: crate::belief::ResolveChoice,
    ) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
        let res = self.beliefs.resolve_contradiction(contradiction_id, choice)?;
        if let Some(ref p) = self.beliefs_path {
            if let Err(e) = self.beliefs.save_contradictions(p) {
                tracing::warn!("[graph] failed to persist contradictions after resolve: {e}");
            }
        }
        Some(res)
    }

    /// Resolve by stable `(triple_a, triple_b)` pair. Use this from
    /// IPC / CLI where the contradiction UUID seen by the user came
    /// from a previous `GraphStore::open` and may not match the id
    /// the freshly-opened engine assigned. Re-saves the sidecar so
    /// the resolution survives the next restart.
    pub fn resolve_contradiction_by_triples(
        &self,
        triple_a: Uuid,
        triple_b: Uuid,
        choice: crate::belief::ResolveChoice,
    ) -> Option<(Vec<Uuid>, Vec<Uuid>)> {
        let res = self.beliefs.resolve_by_triples(triple_a, triple_b, choice)?;
        if let Some(ref p) = self.beliefs_path {
            if let Err(e) = self.beliefs.save_contradictions(p) {
                tracing::warn!("[graph] failed to persist contradictions after resolve: {e}");
            }
        }
        Some(res)
    }

    /// Hydrate a triple into the human-readable form the brief drawer
    /// renders: subject + predicate + object as names (not UUIDs),
    /// the ingest timestamp, the originating capture / trace id, and
    /// the current belief status. Returns `None` if the triple was
    /// hard-deleted between sidecar save and load.
    pub fn triple_detail(&self, triple_id: Uuid) -> Result<Option<TripleDetail>> {
        let Some(triple) = self.find_triple_by_id(triple_id)? else {
            return Ok(None);
        };
        let subject = self.get_entity(triple.subject_id).ok();
        let object = self.get_entity(triple.object_id).ok();
        let status = self.beliefs.status_for(triple_id);
        Ok(Some(TripleDetail {
            triple_id,
            subject_id: triple.subject_id,
            subject_name: subject
                .as_ref()
                .map(|e| e.name.clone())
                .unwrap_or_else(|| triple.subject_id.to_string()),
            subject_type: subject
                .as_ref()
                .map(|e| format!("{:?}", e.entity_type))
                .unwrap_or_default(),
            predicate: triple.predicate.to_string(),
            object_id: triple.object_id,
            object_name: object
                .as_ref()
                .map(|e| e.name.clone())
                .unwrap_or_else(|| triple.object_id.to_string()),
            object_type: object
                .as_ref()
                .map(|e| format!("{:?}", e.entity_type))
                .unwrap_or_default(),
            confidence: triple.confidence,
            source_id: triple.source_id,
            ingested_at: triple.created_at,
            status,
        }))
    }

    /// Walk every live triple and assert it into the belief engine.
    /// Idempotent — `BeliefStore::assert_for_triple` returns the
    /// existing belief id if one already exists.
    fn rebuild_beliefs(&self) -> Result<()> {
        let triple_uuids: Vec<Uuid> = self.triple_map.borrow().keys().copied().collect();
        let mut count = 0usize;
        for uuid in triple_uuids {
            if let Some(triple) = self.find_triple_by_id(uuid)? {
                let pred_str = serde_json::to_string(&triple.predicate)
                    .unwrap_or_else(|_| "unknown".to_string());
                self.beliefs.assert_for_triple(
                    triple.id,
                    triple.subject_id,
                    &pred_str,
                    triple.object_id,
                    triple.confidence as f32,
                );
                count += 1;
            }
        }
        if count > 0 {
            info!("[graph] belief engine rebuilt: {count} triples asserted");
        }
        Ok(())
    }

    /// Walk live `kg_entities` and `kg_relations` and emit a temporal fact
    /// for any UUID that doesn't yet have one. Safe to call repeatedly:
    /// rows that already have an active temporal fact are skipped.
    fn backfill_temporal(&self) -> Result<()> {
        let entity_uuids: Vec<Uuid> = self.entity_map.borrow().keys().copied().collect();
        let mut filled_entities = 0usize;
        for uuid in entity_uuids {
            if self
                .temporal
                .current_fact_id(uuid, FACT_TYPE_ENTITY)
                .map_err(temporal_err)?
                .is_some()
            {
                continue;
            }
            // Read the live state and use its created_at as valid_from.
            if let Some(entity) = self.find_entity_by_id(uuid)? {
                let json = serde_json::to_string(&entity)
                    .map_err(|e| TraceMindError::Storage(e.to_string()))?;
                self.temporal
                    .insert_fact(uuid, FACT_TYPE_ENTITY, &json, entity.created_at, None)
                    .map_err(temporal_err)?;
                filled_entities += 1;
            }
        }

        let triple_uuids: Vec<Uuid> = self.triple_map.borrow().keys().copied().collect();
        let mut filled_triples = 0usize;
        for uuid in triple_uuids {
            if self
                .temporal
                .current_fact_id(uuid, FACT_TYPE_TRIPLE)
                .map_err(temporal_err)?
                .is_some()
            {
                continue;
            }
            if let Some(triple) = self.find_triple_by_id(uuid)? {
                let json = serde_json::to_string(&triple)
                    .map_err(|e| TraceMindError::Storage(e.to_string()))?;
                self.temporal
                    .insert_fact(uuid, FACT_TYPE_TRIPLE, &json, triple.created_at, None)
                    .map_err(temporal_err)?;
                filled_triples += 1;
            }
        }

        if filled_entities > 0 || filled_triples > 0 {
            info!(
                "[graph] temporal backfill: {filled_entities} entities, \
                 {filled_triples} triples"
            );
        }
        Ok(())
    }

    /// Re-scan the underlying SQLite store and rebuild the UUID ↔ skg-id
    /// caches from disk.
    ///
    /// Why: `GraphStore::open` snapshots the entity/triple maps once at open
    /// time. When two separate `GraphStore` instances point at the same DB
    /// (e.g. one inside `IngestPipeline`, one inside `RetrievalEngine` in a
    /// long-running MCP server), the retrieval side never sees writes made
    /// by the ingest side because `search_vectors` filters through the cache.
    /// Callers that need read-after-write across instances must invoke this
    /// before querying. TM-UX-001 Phase C relies on it for proactive
    /// `memory_store` context.
    pub fn reload_maps(&self) -> Result<()> {
        let entities = self
            .kg
            .list_entities(None, None)
            .map_err(|e| TraceMindError::Storage(format!("skg list_entities: {e}")))?;

        let mut entity_map = HashMap::new();
        for ent in &entities {
            if let Some(skg_id) = ent.id {
                if let Some(uuid) = prop_uuid(ent.get_property("uuid")) {
                    entity_map.insert(uuid, skg_id);
                }
            }
        }

        let mut triple_map = HashMap::new();
        {
            let conn = self.kg.connection();
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

        *self.entity_map.borrow_mut() = entity_map;
        *self.triple_map.borrow_mut() = triple_map;
        Ok(())
    }

    // ─── Sprint C-0: Context CRUD ──────────────────────────────────────
    //
    // Thin façades over `crate::context::*`. Held here (rather than as
    // free functions) so callers don't need to juggle the raw rusqlite
    // connection — the same pattern as `BeliefStore` wraps tm-tms.

    /// Create a new context (idempotent on name — duplicates are no-ops).
    pub fn create_context(&self, ctx: &crate::context::Context) -> Result<()> {
        crate::context::create_context(self.kg.connection(), ctx)
    }

    /// List every context, newest first.
    pub fn list_contexts(&self) -> Result<Vec<crate::context::Context>> {
        crate::context::list_contexts(self.kg.connection())
    }

    /// Look up a context by name.
    pub fn get_context_by_name(&self, name: &str) -> Result<Option<crate::context::Context>> {
        crate::context::get_context_by_name(self.kg.connection(), name)
    }

    /// Append a negative-feedback row. `result_id` is opaque — pass the
    /// triple UUID, entity UUID, or signal row id (stringified).
    pub fn write_negative_signal(
        &self,
        query_id: Uuid,
        result_id: &str,
        kind: &str,
        context_a: Option<Uuid>,
        context_b: Option<Uuid>,
        weight: f32,
    ) -> Result<i64> {
        crate::context::write_negative_signal(
            self.kg.connection(),
            query_id,
            result_id,
            kind,
            context_a,
            context_b,
            weight,
        )
    }

    /// Sum of negative-signal weights for a query — used by the bandit
    /// reward decomposition (`final = relevance - this`).
    pub fn negative_weight_for_query(&self, query_id: Uuid) -> Result<f32> {
        crate::context::negative_weight_for_query(self.kg.connection(), query_id)
    }

    // ─── Entity CRUD ────────────────────────────────────────────────────

    pub fn upsert_entity(&self, entity: &Entity) -> Result<()> {
        let etype_json = serde_json::to_string(&entity.entity_type)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut map = self.entity_map.borrow_mut();

        let is_new = !map.contains_key(&entity.id);
        if let Some(&skg_id) = map.get(&entity.id) {
            // Update existing.
            let mut skg_ent = self
                .kg
                .get_entity(skg_id)
                .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;

            skg_ent.name = entity.name.clone();
            skg_ent.entity_type = etype_json.clone();
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
        drop(map);

        // Sprint C-1: mirror this assertion into the bitemporal store so we
        // can answer `entity_at(t)` / `entity_history(id)`. We serialize the
        // *whole* live entity (including name, type, confidence) — JSON is
        // small and lets the temporal layer stay schema-agnostic.
        let json = serde_json::to_string(entity)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        if is_new {
            self.temporal
                .insert_fact(
                    entity.id,
                    FACT_TYPE_ENTITY,
                    &json,
                    entity.created_at,
                    None,
                )
                .map_err(temporal_err)?;
        } else if let Some(old_id) = self
            .temporal
            .current_fact_id(entity.id, FACT_TYPE_ENTITY)
            .map_err(temporal_err)?
        {
            // Update: closes the old fact and opens a new one valid from
            // `entity.updated_at`. If the caller never bumped updated_at,
            // we still record a new transaction-time row (so tx-time
            // history stays complete) but valid_from collapses to the
            // existing timestamp — the temporal store handles the
            // supersession chain.
            self.temporal
                .update_fact(old_id, &json, entity.updated_at, None)
                .map_err(temporal_err)?;
        } else {
            // Edge case: entity is in the live map but has no temporal
            // row (e.g. backfill missed it). Treat as a fresh insert.
            self.temporal
                .insert_fact(
                    entity.id,
                    FACT_TYPE_ENTITY,
                    &json,
                    entity.created_at,
                    None,
                )
                .map_err(temporal_err)?;
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

    /// Look up a triple by its TraceMind UUID. Returns `None` if the
    /// triple is not in the live `kg_relations` table (either never
    /// inserted, or pruned). Used by `backfill_temporal` and the
    /// bitemporal read APIs.
    pub fn find_triple_by_id(&self, id: Uuid) -> Result<Option<Triple>> {
        let tmap = self.triple_map.borrow();
        let skg_id = match tmap.get(&id) {
            Some(&sid) => sid,
            None => return Ok(None),
        };
        // Pull the row + reverse-lookup the subject/object UUIDs.
        let entity_map = self.entity_map.borrow();
        let reverse: HashMap<i64, Uuid> =
            entity_map.iter().map(|(&uuid, &sid)| (sid, uuid)).collect();
        drop(entity_map);
        drop(tmap);

        let conn = self.kg.connection();
        let result = conn.query_row(
            "SELECT source_id, target_id, rel_type, weight, properties \
             FROM kg_relations WHERE id = ?1",
            params![skg_id],
            |row| {
                let source_id: i64 = row.get(0)?;
                let target_id: i64 = row.get(1)?;
                let rel_type: String = row.get(2)?;
                let weight: f64 = row.get(3)?;
                let props_str: String = row.get(4)?;
                Ok((source_id, target_id, rel_type, weight, props_str))
            },
        );

        match result {
            Ok((src_skg, tgt_skg, rel_type, weight, props_str)) => {
                let props: HashMap<String, serde_json::Value> =
                    serde_json::from_str(&props_str).unwrap_or_default();
                let subject_id =
                    reverse.get(&src_skg).copied().unwrap_or_else(Uuid::new_v4);
                let object_id =
                    reverse.get(&tgt_skg).copied().unwrap_or_else(Uuid::new_v4);
                let predicate: Predicate =
                    serde_json::from_str(&rel_type).unwrap_or(Predicate::RelatedTo);
                let source_id = prop_string(props.get("source_id"));
                let created_at = prop_datetime(props.get("created_at"));
                let updated_at = prop_datetime(props.get("updated_at"));
                Ok(Some(Triple {
                    id,
                    subject_id,
                    predicate,
                    object_id,
                    confidence: weight,
                    source_id,
                    created_at,
                    updated_at,
                }))
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

    /// Enumerate entities whose name length is within `delta` of `len`.
    ///
    /// Intended for fuzzy-matching callers that need to score candidates
    /// with edit-distance in application code. We pre-filter on length to
    /// keep the candidate set small.
    pub fn entities_near_length(
        &self,
        len: usize,
        delta: usize,
    ) -> Result<Vec<Entity>> {
        let min_len = len.saturating_sub(delta) as i64;
        let max_len = (len + delta) as i64;
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT entity_type, name, properties FROM kg_entities \
                 WHERE length(name) BETWEEN ?1 AND ?2",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let rows = stmt
            .query_map(params![min_len, max_len], |row| {
                let etype: String = row.get(0)?;
                let name: String = row.get(1)?;
                let props_str: String = row.get(2)?;
                Ok((etype, name, props_str))
            })
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut out = Vec::new();
        for r in rows {
            let (etype_str, name, props_str) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let props: HashMap<String, serde_json::Value> =
                serde_json::from_str(&props_str).unwrap_or_default();
            let entity = props_to_tm_entity(&etype_str, &name, &props)?;
            out.push(entity);
        }
        Ok(out)
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

        let is_new = !tmap.contains_key(&triple.id);
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
        drop(tmap);

        // Sprint C-1: mirror into the bitemporal store. Same pattern as
        // entities — confidence becomes a derived view of belief revisions
        // once tm-tms is wired (Sprint C-2), but the substrate already
        // carries the data we'll need then.
        let json = serde_json::to_string(triple)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        if is_new {
            self.temporal
                .insert_fact(
                    triple.id,
                    FACT_TYPE_TRIPLE,
                    &json,
                    triple.created_at,
                    None,
                )
                .map_err(temporal_err)?;
        } else if let Some(old_id) = self
            .temporal
            .current_fact_id(triple.id, FACT_TYPE_TRIPLE)
            .map_err(temporal_err)?
        {
            self.temporal
                .update_fact(old_id, &json, triple.updated_at, None)
                .map_err(temporal_err)?;
        } else {
            self.temporal
                .insert_fact(
                    triple.id,
                    FACT_TYPE_TRIPLE,
                    &json,
                    triple.created_at,
                    None,
                )
                .map_err(temporal_err)?;
        }

        // Sprint C-2: mirror as a JTMS belief. Idempotent — re-asserting
        // the same triple just returns the existing belief id (and
        // revives it if it was previously `Out`).
        self.beliefs.assert_for_triple(
            triple.id,
            triple.subject_id,
            &predicate_str,
            triple.object_id,
            weight as f32,
        );

        debug!(
            "[graph] upserted triple id={} ({} -> {})",
            triple.id, triple.subject_id, triple.object_id
        );
        Ok(())
    }

    // ─── Belief APIs (Sprint C-2) ───────────────────────────────────────

    /// Borrow the JTMS-backed belief store. Use this to call
    /// `detect_contradiction` from the ingest pipeline (which has
    /// the embeddings needed to compute cosine similarity).
    pub fn beliefs(&self) -> &BeliefStore {
        &self.beliefs
    }

    /// Current `BeliefStatus` for `triple_id`. `None` means the triple
    /// has no belief recorded (shouldn't happen for triples that went
    /// through `upsert_triple` — only legacy data would).
    pub fn belief_status_for(&self, triple_id: Uuid) -> Option<BeliefStatus> {
        self.beliefs.status_for(triple_id)
    }

    /// All contradictions recorded so far, projected into triple-id
    /// space. Used by the daily brief.
    pub fn contradictions(&self) -> Vec<ContradictionView> {
        self.beliefs.contradictions()
    }

    // ─── Bitemporal reads (Sprint C-1) ──────────────────────────────────

    /// Reconstruct an entity as it appeared at valid-time `as_of`, given
    /// the full recorded history. Walks `history()` and picks the most
    /// recently *recorded* assertion whose valid interval contains
    /// `as_of`. Returns `None` if no version was valid then.
    ///
    /// Why not `temporal.query_at`: `update_fact` only marks the old row
    /// as superseded (transaction-time), it does *not* close its
    /// valid-time interval. So a strict `query_at(as_of, now)` filters
    /// the old row out as superseded and the new row out by valid_from,
    /// returning empty for an in-between `as_of`. Walking history with
    /// "most-recently-recorded among those whose valid_from ≤ as_of"
    /// gives the natural "what was true at t" answer.
    pub fn entity_at(&self, id: Uuid, as_of: DateTime<Utc>) -> Result<Option<Entity>> {
        let history = self
            .temporal
            .history(id, FACT_TYPE_ENTITY)
            .map_err(temporal_err)?;
        let mut chosen: Option<Entity> = None;
        for fact in history {
            if fact.valid_time.from > as_of {
                continue;
            }
            if let Some(to) = fact.valid_time.to {
                if to <= as_of {
                    continue;
                }
            }
            if let Ok(parsed) = serde_json::from_value::<Entity>(fact.fact) {
                // history() is ordered by recorded_at ASC, so each later
                // matching fact overrides earlier ones — last write wins.
                chosen = Some(parsed);
            }
        }
        Ok(chosen)
    }

    /// Reconstruct a triple as it appeared at valid-time `as_of`. See
    /// `entity_at` for the chosen-version semantics.
    pub fn triple_at(&self, id: Uuid, as_of: DateTime<Utc>) -> Result<Option<Triple>> {
        let history = self
            .temporal
            .history(id, FACT_TYPE_TRIPLE)
            .map_err(temporal_err)?;
        let mut chosen: Option<Triple> = None;
        for fact in history {
            if fact.valid_time.from > as_of {
                continue;
            }
            if let Some(to) = fact.valid_time.to {
                if to <= as_of {
                    continue;
                }
            }
            if let Ok(parsed) = serde_json::from_value::<Triple>(fact.fact) {
                chosen = Some(parsed);
            }
        }
        Ok(chosen)
    }

    /// Return every recorded version of an entity, oldest first. Each
    /// element is `(version, valid_from, recorded_at, superseded_at)` —
    /// callers that need the full bitemporal envelope should use
    /// `tm_temporal::TemporalStore::history` directly.
    pub fn entity_history(
        &self,
        id: Uuid,
    ) -> Result<Vec<(Entity, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>> {
        let facts = self
            .temporal
            .history(id, FACT_TYPE_ENTITY)
            .map_err(temporal_err)?;
        let mut out = Vec::with_capacity(facts.len());
        for fact in facts {
            if let Ok(parsed) = serde_json::from_value::<Entity>(fact.fact.clone()) {
                out.push((
                    parsed,
                    fact.valid_time.from,
                    fact.tx_time.recorded_at,
                    fact.tx_time.superseded_at,
                ));
            }
        }
        Ok(out)
    }

    /// Return every recorded version of a triple, oldest first. See
    /// `entity_history`.
    pub fn triple_history(
        &self,
        id: Uuid,
    ) -> Result<Vec<(Triple, DateTime<Utc>, DateTime<Utc>, Option<DateTime<Utc>>)>> {
        let facts = self
            .temporal
            .history(id, FACT_TYPE_TRIPLE)
            .map_err(temporal_err)?;
        let mut out = Vec::with_capacity(facts.len());
        for fact in facts {
            if let Ok(parsed) = serde_json::from_value::<Triple>(fact.fact.clone()) {
                out.push((
                    parsed,
                    fact.valid_time.from,
                    fact.tx_time.recorded_at,
                    fact.tx_time.superseded_at,
                ));
            }
        }
        Ok(out)
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

            // Sprint C-2: skip triples whose JTMS belief is `Out`
            // (explicitly retracted). `Contradicted` triples remain
            // visible — the brief surfaces them and downstream consumers
            // can apply `effective_confidence` to downrank.
            if matches!(
                self.beliefs.status_for(triple_uuid),
                Some(BeliefStatus::Out)
            ) {
                continue;
            }

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

    /// Insert a captured signal with its embedding and priority tier (two-speed fast path).
    /// Returns the row ID of the inserted signal.
    ///
    /// Tiers:
    /// - 1: InstantEntity (should not hit this method; promoted directly to graph)
    /// - 2: Priority (consolidated every 30s)
    /// - 3: Normal (consolidated every 5 min)
    /// - 4: Ephemeral (stays searchable but never promoted)
    pub fn insert_signal_with_embedding(
        &self,
        source: &str,
        raw_text: &str,
        content_hash: u64,
        session_id: Uuid,
        embedding: &[f32],
        relevance_score: Option<f64>,
        priority_tier: i64,
    ) -> Result<i64> {
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        let conn = self.kg.connection();
        conn.execute(
            "INSERT INTO captured_signals \
             (source, raw_text, content_hash, relevance_score, ingested, session_id, embedding, priority_tier) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
            params![
                source,
                raw_text,
                content_hash as i64,
                relevance_score,
                session_id.to_string(),
                blob,
                priority_tier
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("insert_signal_with_embedding: {e}")))?;
        Ok(conn.last_insert_rowid())
    }

    /// Load unconsolidated signals (those with no cluster_id and a stored embedding).
    /// Used by the slow-path consolidation pass. Optionally filter by priority tier.
    ///
    /// `tier_filter`: Some(tier) restricts to that tier; None loads all tiers except ephemeral (4).
    pub fn unconsolidated_signals(&self, limit: usize) -> Result<Vec<CapturedSignal>> {
        self.unconsolidated_signals_by_tier(None, limit)
    }

    /// Tier-scoped variant of `unconsolidated_signals`.
    /// If `tier_filter` is Some(t), returns only signals at that tier.
    /// If None, returns all non-ephemeral tiers (excludes tier 4).
    pub fn unconsolidated_signals_by_tier(
        &self,
        tier_filter: Option<i64>,
        limit: usize,
    ) -> Result<Vec<CapturedSignal>> {
        let conn = self.kg.connection();
        let (sql, use_filter) = match tier_filter {
            Some(_) => (
                "SELECT id, source, raw_text, content_hash, session_id, embedding, created_at \
                 FROM captured_signals \
                 WHERE cluster_id IS NULL AND embedding IS NOT NULL AND priority_tier = ?1 \
                 ORDER BY id ASC LIMIT ?2",
                true,
            ),
            None => (
                "SELECT id, source, raw_text, content_hash, session_id, embedding, created_at \
                 FROM captured_signals \
                 WHERE cluster_id IS NULL AND embedding IS NOT NULL AND priority_tier < 4 \
                 ORDER BY id ASC LIMIT ?1",
                false,
            ),
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| TraceMindError::Storage(format!("prep unconsolidated: {e}")))?;

        let mapper = |row: &rusqlite::Row| {
            let id: i64 = row.get(0)?;
            let source: String = row.get(1)?;
            let raw_text: String = row.get(2)?;
            let hash_i64: i64 = row.get(3)?;
            let session_str: Option<String> = row.get(4)?;
            let blob: Vec<u8> = row.get(5)?;
            let created_str: String = row.get(6)?;
            Ok((id, source, raw_text, hash_i64, session_str, blob, created_str))
        };

        let rows: Vec<_> = if use_filter {
            let tier = tier_filter.unwrap();
            stmt.query_map(params![tier, limit as i64], mapper)
                .map_err(|e| TraceMindError::Storage(format!("query unconsolidated: {e}")))?
                .collect()
        } else {
            stmt.query_map(params![limit as i64], mapper)
                .map_err(|e| TraceMindError::Storage(format!("query unconsolidated: {e}")))?
                .collect()
        };

        let mut signals = Vec::new();
        for row in rows {
            let (id, source, raw_text, hash_i64, session_str, blob, created_str) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;

            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();

            let session_id = session_str.and_then(|s| Uuid::parse_str(&s).ok());

            let created_at = created_str
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());

            signals.push(CapturedSignal {
                id,
                source,
                raw_text,
                content_hash: hash_i64 as u64,
                session_id,
                embedding,
                created_at,
            });
        }
        Ok(signals)
    }

    /// Search unpromoted signals by cosine similarity to a query embedding.
    /// Used by the hybrid retrieval path so fresh captures are findable even
    /// before consolidation has promoted them to entities.
    ///
    /// Returns at most `top_k` signals with similarity ≥ `min_sim`, sorted descending.
    /// Excludes ephemeral (tier 4) and already-promoted signals.
    pub fn search_signals(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        min_sim: f32,
    ) -> Result<Vec<(CapturedSignal, f32)>> {
        // Load candidates — in practice this is bounded because most signals get consolidated.
        // For large backlogs, we could use ANN; for now linear scan is fine (SQLite is already slow-ish).
        let signals = self.unconsolidated_signals_by_tier(None, 2000)?;
        if signals.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored: Vec<(CapturedSignal, f32)> = signals
            .into_iter()
            .filter_map(|s| {
                let sim = cosine_sim_slice(query_embedding, &s.embedding);
                if sim >= min_sim {
                    Some((s, sim))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Mark a signal as belonging to a cluster (and optionally promoted to an entity).
    /// cluster_id = -1 means the signal was too small to promote (noise).
    pub fn mark_signal_clustered(
        &self,
        signal_id: i64,
        cluster_id: i64,
        promoted_entity: Option<Uuid>,
    ) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "UPDATE captured_signals \
             SET cluster_id = ?1, promoted_entity = ?2, ingested = 1 \
             WHERE id = ?3",
            params![
                cluster_id,
                promoted_entity.map(|u| u.to_string()),
                signal_id
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("mark_signal_clustered: {e}")))?;
        Ok(())
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

    // ─── Temporal queries ─────────────────────────────────────────────────

    /// Return entities created or updated within a time range.
    ///
    /// Scans entity JSON properties for `created_at`/`updated_at` timestamps
    /// and returns those that fall within `[start, end)`.
    /// Results sorted by `updated_at` descending (most recently active first).
    pub fn get_entities_by_time_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<Entity>> {
        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();

        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id, entity_type, name, properties FROM kg_entities \
                 WHERE json_extract(properties, '$.updated_at') >= ?1 \
                   AND json_extract(properties, '$.updated_at') < ?2 \
                 ORDER BY json_extract(properties, '$.updated_at') DESC",
            )
            .map_err(|e| TraceMindError::Storage(format!("temporal query prepare: {e}")))?;

        let rows = stmt
            .query_map(params![start_str, end_str], |row| {
                let _id: i64 = row.get(0)?;
                let entity_type: String = row.get(1)?;
                let name: String = row.get(2)?;
                let props_str: String = row.get(3)?;
                Ok((entity_type, name, props_str))
            })
            .map_err(|e| TraceMindError::Storage(format!("temporal query: {e}")))?;

        let mut entities = Vec::new();
        for row in rows {
            let (etype_str, name, props_str) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let props: HashMap<String, serde_json::Value> =
                serde_json::from_str(&props_str).unwrap_or_default();
            if let Ok(entity) = props_to_tm_entity(&etype_str, &name, &props) {
                entities.push(entity);
            }
        }

        info!(
            "[graph] temporal query [{} → {}]: {} entities",
            start_str, end_str, entities.len()
        );
        Ok(entities)
    }

    /// Return entities accessed within a time range (from access_log).
    /// Complements `get_entities_by_time_range` by capturing entities that
    /// were *queried* (not just created/updated) in the period.
    pub fn get_accessed_entities_in_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<Uuid>> {
        let start_str = start.to_rfc3339();
        let end_str = end.to_rfc3339();

        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT entity_id FROM access_log \
                 WHERE created_at >= ?1 AND created_at < ?2 \
                 ORDER BY created_at DESC",
            )
            .map_err(|e| TraceMindError::Storage(format!("access log range: {e}")))?;

        let rows = stmt
            .query_map(params![start_str, end_str], |row| {
                let id_str: String = row.get(0)?;
                Ok(id_str)
            })
            .map_err(|e| TraceMindError::Storage(format!("access log range query: {e}")))?;

        let mut ids = Vec::new();
        for row in rows {
            let id_str = row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            if let Ok(uuid) = Uuid::parse_str(&id_str) {
                ids.push(uuid);
            }
        }

        info!(
            "[graph] accessed entities in range [{} → {}]: {} unique",
            start_str, end_str, ids.len()
        );
        Ok(ids)
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

// ─── ColBERT token cache (added to GraphStore) ─────────────────────────────

impl GraphStore {
    /// Store pre-computed ColBERT per-token embeddings for an entity.
    /// Embeddings stored as packed f32 LE bytes: token_count * dim * 4 bytes.
    pub fn upsert_colbert_tokens(
        &self,
        entity_id: Uuid,
        model_id: &str,
        token_count: usize,
        dim: usize,
        embeddings: &[f32],
    ) -> Result<()> {
        let id_str = entity_id.to_string();
        let bytes: Vec<u8> = embeddings.iter().flat_map(|f| f.to_le_bytes()).collect();
        let conn = self.kg.connection();
        conn.execute(
            "INSERT OR REPLACE INTO colbert_tokens (entity_id, model_id, token_count, dim, embeddings)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id_str, model_id, token_count as i64, dim as i64, bytes],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert colbert tokens: {e}")))?;
        Ok(())
    }

    /// Load pre-computed ColBERT token embeddings for an entity.
    /// Returns `(model_id, embeddings_flat, token_count, dim)`.
    pub fn get_colbert_tokens(&self, entity_id: Uuid) -> Result<Option<(String, Vec<f32>, usize, usize)>> {
        let id_str = entity_id.to_string();
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare("SELECT model_id, token_count, dim, embeddings FROM colbert_tokens WHERE entity_id = ?1")
            .map_err(|e| TraceMindError::Storage(format!("prepare colbert: {e}")))?;

        let result = stmt.query_row(params![id_str], |row| {
            let model_id: String = row.get(0)?;
            let token_count: i64 = row.get(1)?;
            let dim: i64 = row.get(2)?;
            let bytes: Vec<u8> = row.get(3)?;
            Ok((model_id, token_count as usize, dim as usize, bytes))
        });

        match result {
            Ok((model_id, token_count, dim, bytes)) => {
                let embeddings: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                Ok(Some((model_id, embeddings, token_count, dim)))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(TraceMindError::Storage(format!("get colbert tokens: {e}"))),
        }
    }

    /// Batch load ColBERT tokens for multiple entities.
    pub fn batch_colbert_tokens(&self, entity_ids: &[Uuid]) -> HashMap<Uuid, (Vec<f32>, usize, usize)> {
        let mut result = HashMap::new();
        for id in entity_ids {
            if let Ok(Some((_, embs, tc, dim))) = self.get_colbert_tokens(*id) {
                result.insert(*id, (embs, tc, dim));
            }
        }
        result
    }
}

/// Extract a DateTime<Utc> from a JSON property value, defaulting to now.
fn prop_datetime(val: Option<&serde_json::Value>) -> DateTime<Utc> {
    val.and_then(|v| v.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now)
}

/// Cosine similarity between two slices. Returns 0.0 for mismatched/empty vectors.
fn cosine_sim_slice(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
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

    #[test]
    fn test_get_entities_by_time_range() {
        let store = GraphStore::open(":memory:").unwrap();

        // Create entities at different times
        let now = Utc::now();
        let mut e1 = make_entity("Recent", EntityType::Concept);
        e1.updated_at = now;
        e1.created_at = now;
        store.upsert_entity(&e1).unwrap();

        let mut e2 = make_entity("Old", EntityType::Concept);
        e2.updated_at = now - chrono::Duration::days(30);
        e2.created_at = now - chrono::Duration::days(30);
        store.upsert_entity(&e2).unwrap();

        // Query for entities updated in the last 7 days
        let start = now - chrono::Duration::days(7);
        let end = now + chrono::Duration::hours(1);
        let results = store.get_entities_by_time_range(start, end).unwrap();

        assert_eq!(results.len(), 1, "should only find recent entity");
        assert_eq!(results[0].name, "Recent");

        // Query for entities updated in the last 60 days — should find both
        let start_wide = now - chrono::Duration::days(60);
        let results_wide = store.get_entities_by_time_range(start_wide, end).unwrap();
        assert_eq!(results_wide.len(), 2, "should find both entities");
    }

    #[test]
    fn test_get_accessed_entities_in_range() {
        let store = GraphStore::open(":memory:").unwrap();

        let e1 = make_entity("Accessed", EntityType::Concept);
        store.upsert_entity(&e1).unwrap();
        store.log_access(e1.id, "query_result", None).unwrap();

        let now = Utc::now();
        let start = now - chrono::Duration::hours(1);
        let end = now + chrono::Duration::hours(1);
        let accessed = store.get_accessed_entities_in_range(start, end).unwrap();

        assert!(accessed.contains(&e1.id), "should find the accessed entity");
    }

    #[test]
    fn test_colbert_token_cache() {
        let store = GraphStore::open(":memory:").unwrap();
        let entity_id = Uuid::new_v4();

        // Store 3 tokens of dim 4
        let tokens: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
        store.upsert_colbert_tokens(entity_id, "test-model", 3, 4, &tokens).unwrap();

        // Load back
        let loaded = store.get_colbert_tokens(entity_id).unwrap();
        assert!(loaded.is_some());
        let (model_id, embs, tc, dim) = loaded.unwrap();
        assert_eq!(model_id, "test-model");
        assert_eq!(tc, 3);
        assert_eq!(dim, 4);
        assert_eq!(embs.len(), 12);
        assert_eq!(embs[0], 1.0);
        assert_eq!(embs[11], 12.0);

        // Not found
        let missing = store.get_colbert_tokens(Uuid::new_v4()).unwrap();
        assert!(missing.is_none());

        // Batch
        let batch = store.batch_colbert_tokens(&[entity_id, Uuid::new_v4()]);
        assert_eq!(batch.len(), 1);
        assert!(batch.contains_key(&entity_id));
    }

    // ─── Sprint C-1: bitemporal substrate ───────────────────────────────

    #[test]
    fn bitemporal_entity_round_trip() {
        // Insert → entity_at(now) returns it; entity_at(before-creation)
        // returns None.
        let store = GraphStore::open(":memory:").unwrap();
        let mut e = make_entity("Dana", EntityType::Person);
        let t0 = Utc::now() - chrono::Duration::seconds(30);
        e.created_at = t0;
        e.updated_at = t0;
        store.upsert_entity(&e).unwrap();

        let now = Utc::now();
        let live = store.entity_at(e.id, now).unwrap();
        assert!(live.is_some(), "entity should be visible at now");
        assert_eq!(live.unwrap().name, "Dana");

        let before = store
            .entity_at(e.id, t0 - chrono::Duration::seconds(60))
            .unwrap();
        assert!(before.is_none(), "entity should not exist before t0");
    }

    #[test]
    fn bitemporal_entity_update_creates_supersedes_chain() {
        let store = GraphStore::open(":memory:").unwrap();
        let mut e = make_entity("Erin", EntityType::Person);
        let t0 = Utc::now() - chrono::Duration::seconds(60);
        e.created_at = t0;
        e.updated_at = t0;
        store.upsert_entity(&e).unwrap();

        // Mutate name + bump confidence at t1.
        let t1 = t0 + chrono::Duration::seconds(30);
        e.name = "Erin Updated".into();
        e.confidence = 0.42;
        e.updated_at = t1;
        store.upsert_entity(&e).unwrap();

        // History has both versions in chronological order.
        let history = store.entity_history(e.id).unwrap();
        assert_eq!(history.len(), 2, "expected two revisions in history");
        assert_eq!(history[0].0.name, "Erin");
        assert_eq!(history[1].0.name, "Erin Updated");
        // First revision must be superseded; second must still be live.
        assert!(history[0].3.is_some(), "first revision should be superseded");
        assert!(history[1].3.is_none(), "second revision should be live");

        // Querying at t0 should give old name; at t1 should give new.
        let at_t0 = store
            .entity_at(e.id, t0 + chrono::Duration::seconds(1))
            .unwrap()
            .expect("entity at t0");
        assert_eq!(at_t0.name, "Erin");

        let at_t1 = store
            .entity_at(e.id, t1 + chrono::Duration::seconds(1))
            .unwrap()
            .expect("entity at t1");
        assert_eq!(at_t1.name, "Erin Updated");
        assert!((at_t1.confidence - 0.42).abs() < 1e-9);
    }

    #[test]
    fn bitemporal_triple_round_trip() {
        let store = GraphStore::open(":memory:").unwrap();
        let a = make_entity("Frank", EntityType::Person);
        let b = make_entity("Gina", EntityType::Person);
        store.upsert_entity(&a).unwrap();
        store.upsert_entity(&b).unwrap();

        let t0 = Utc::now() - chrono::Duration::seconds(60);
        let mut t = Triple {
            id: Uuid::new_v4(),
            subject_id: a.id,
            predicate: Predicate::RelatedTo,
            object_id: b.id,
            confidence: 0.7,
            source_id: None,
            created_at: t0,
            updated_at: t0,
        };
        store.upsert_triple(&t).unwrap();

        // Bump confidence at t1.
        let t1 = t0 + chrono::Duration::seconds(30);
        t.confidence = 0.95;
        t.updated_at = t1;
        store.upsert_triple(&t).unwrap();

        let history = store.triple_history(t.id).unwrap();
        assert_eq!(history.len(), 2);
        assert!((history[0].0.confidence - 0.7).abs() < 1e-9);
        assert!((history[1].0.confidence - 0.95).abs() < 1e-9);

        let at_t0 = store
            .triple_at(t.id, t0 + chrono::Duration::seconds(1))
            .unwrap()
            .expect("triple at t0");
        assert!((at_t0.confidence - 0.7).abs() < 1e-9);

        let at_t1 = store
            .triple_at(t.id, t1 + chrono::Duration::seconds(1))
            .unwrap()
            .expect("triple at t1");
        assert!((at_t1.confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn bitemporal_history_missing_id_is_empty() {
        let store = GraphStore::open(":memory:").unwrap();
        let unknown = Uuid::new_v4();
        assert!(store.entity_history(unknown).unwrap().is_empty());
        assert!(store.triple_history(unknown).unwrap().is_empty());
        assert!(store.entity_at(unknown, Utc::now()).unwrap().is_none());
        assert!(store.triple_at(unknown, Utc::now()).unwrap().is_none());
    }
}
