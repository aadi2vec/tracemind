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

/// LM-1: one row of an entity's backlink panel. Carries the *source*
/// entity (the one linking *to* the target) plus the typed predicate
/// and confidence so the Tauri / Brief panels can render
/// "[[Acme Corp]] — **employs** — _conf 0.88_" without further lookups.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Backlink {
    pub triple_id: Uuid,
    pub source: Entity,
    pub predicate: Predicate,
    pub confidence: f64,
}

/// LM-9: result of routing a freshly-extracted triple through the
/// confidence gate. Returned by
/// [`GraphStore::route_triple_by_confidence`] so the ingest pipeline
/// can attribute the outcome in the trace log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRouteOutcome {
    /// Triple confidence ≥ `ACCEPT_THRESHOLD`. Written to `kg_relations`.
    Accepted,
    /// Triple confidence ∈ `[PENDING_FLOOR, ACCEPT_THRESHOLD)`. Stored
    /// in `pending_relations` with the wrapped row id.
    Pending(Uuid),
    /// Triple confidence < `PENDING_FLOOR`. Discarded silently.
    Dropped,
}

/// Map a predicate string (produced by `Predicate::Display`) back to a
/// [`Predicate`]. Used by `accept_pending` to re-hydrate the predicate
/// from the open-vocab text column. Unknown names fall through to
/// `Predicate::Custom(name)` so callers never silently lose information.
fn parse_predicate(name: &str) -> Predicate {
    match name {
        "RelatedTo" => Predicate::RelatedTo,
        "IsA" => Predicate::IsA,
        "PartOf" => Predicate::PartOf,
        "HasProperty" => Predicate::HasProperty,
        "WorksAt" => Predicate::WorksAt,
        "CollaboratesWith" => Predicate::CollaboratesWith,
        "Owns" => Predicate::Owns,
        "DependsOn" => Predicate::DependsOn,
        "Produces" => Predicate::Produces,
        "References" => Predicate::References,
        "HasProcedure" => Predicate::HasProcedure,
        other => Predicate::Custom(other.to_string()),
    }
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
    /// Sprint C-0: the context that every new entity / triple / signal
    /// will be tagged with on insert. `None` means "no active context"
    /// (= legacy / pre-Sprint-C-0 behaviour: rows are unscoped). Set
    /// via [`GraphStore::set_active_context`] — typically the caller
    /// reads `~/.tracemind/active_context.json` and forwards the UUID.
    active_context_id: RefCell<Option<Uuid>>,
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
            -- NOTE: idx_signals_cluster and idx_signals_tier reference cluster_id,
            -- which may not yet exist on older DBs (pre two-speed pipeline). They
            -- are created after the ALTER TABLE migration block below.

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
            );

            -- LM-5b: auto-tags. Each entity carries 0..N tags derived from
            -- hashtags / entity-type / future cluster-label sources. Tag
            -- source is tracked so the user can untag a heuristic without
            -- losing manual tags.
            CREATE TABLE IF NOT EXISTS kg_entity_tags (
                entity_id   TEXT NOT NULL,
                tag         TEXT NOT NULL,
                source      TEXT NOT NULL DEFAULT 'auto',
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (entity_id, tag)
            );
            CREATE INDEX IF NOT EXISTS idx_tags_entity ON kg_entity_tags(entity_id);
            CREATE INDEX IF NOT EXISTS idx_tags_tag ON kg_entity_tags(tag);"
        )
        .map_err(|e| TraceMindError::Storage(format!("create tables: {e}")))?;

        // Sprint C-0: context segmentation schema (contexts +
        // negative_signals tables, captured_signals.context_id column).
        // Idempotent — safe to call on every open.
        crate::context::init_schema(conn)?;

        // LM-11a: Memory Views — user-curated saved splices
        // (memory_views + memory_view_members). Idempotent.
        crate::memory_view::init_schema(conn)?;

        // LM-9: confidence-routed triple pending pool. Low-confidence
        // extracted relations land here instead of `kg_relations`;
        // the Tauri panel reads from this table. Idempotent.
        crate::pending_relations::init_schema(conn)?;

        // Migrate existing DBs that lack the two-speed pipeline columns.
        {
            let conn = kg.connection();
            for (col, ty) in &[
                ("session_id", "TEXT"),
                ("embedding", "BLOB"),
                ("cluster_id", "INTEGER"),
                ("promoted_entity", "TEXT"),
                ("priority_tier", "INTEGER DEFAULT 3"),
                // SimHash signature for the two-stage ANN search path. NULL
                // on rows from older builds; backfilled lazily at search time.
                ("simhash", "INTEGER"),
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
            active_context_id: RefCell::new(None),
        };

        // Sprint GRAPH: install threads / event graph / ontology / LGM /
        // bridge / view-expression schema. Idempotent — see graph_sprint.rs.
        {
            let conn = store.kg.connection();
            crate::graph_sprint::ensure_schema(conn)?;
        }

        // Q3.1: feedback signal fabric — explicit / implicit / behavioral signals
        // stored as first-class memories with UUID provenance.
        {
            let conn = store.kg.connection();
            crate::feedback_fabric::init_schema(conn)?;
        }

        // Q3.4: temporal KG columns (valid_from / valid_to on kg_relations).
        {
            let conn = store.kg.connection();
            crate::contradiction_rate::ensure_temporal_columns(conn)?;
        }

        // Q3.5: session_id / host_id scoping.
        {
            let conn = store.kg.connection();
            crate::session_scope::init_schema(conn)?;
        }

        // Q4.14: policy provenance tables (mutations + rollbacks).
        {
            let conn = store.kg.connection();
            crate::policy_provenance::init_schema(conn)?;
        }

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

    /// Borrow the underlying SQLite connection. Used by sibling
    /// modules in this crate (algebra, event_graph, thread_graph,
    /// ontology_types, portable_export) that operate on Sprint GRAPH
    /// tables co-located in `memory.db`.
    pub fn connection(&self) -> &rusqlite::Connection {
        self.kg.connection()
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

    /// Set (or clear) the context every new entity / triple / signal will
    /// be tagged with on insert. Pass `None` to revert to the legacy
    /// unscoped behaviour. Typical caller path:
    ///
    /// ```ignore
    /// let active = ActiveContext::load(&active_path)?;
    /// graph.set_active_context(active.map(|a| a.id));
    /// ```
    pub fn set_active_context(&self, ctx_id: Option<Uuid>) {
        *self.active_context_id.borrow_mut() = ctx_id;
    }

    /// Read the active context UUID (or `None` if unscoped).
    pub fn active_context_id(&self) -> Option<Uuid> {
        *self.active_context_id.borrow()
    }

    /// Look up the `context_id` stashed in an entity's properties at
    /// insert time. Returns `Ok(None)` if the entity is missing, was
    /// inserted before Sprint C-0 (no property), or was inserted with
    /// no active context. Used by the retrieval-scope filter.
    pub fn entity_context_id(&self, entity_id: Uuid) -> Result<Option<Uuid>> {
        let map = self.entity_map.borrow();
        let Some(&skg_id) = map.get(&entity_id) else { return Ok(None) };
        drop(map);
        let skg_ent = self
            .kg
            .get_entity(skg_id)
            .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;
        Ok(prop_uuid(skg_ent.get_property("context_id")))
    }

    /// Look up the `context_id` for a triple. See `entity_context_id`.
    pub fn triple_context_id(&self, triple_id: Uuid) -> Result<Option<Uuid>> {
        let map = self.triple_map.borrow();
        let Some(&skg_id) = map.get(&triple_id) else { return Ok(None) };
        drop(map);
        let conn = self.kg.connection();
        let props_str: std::result::Result<String, _> = conn.query_row(
            "SELECT properties FROM kg_relations WHERE id = ?1",
            params![skg_id],
            |row| row.get(0),
        );
        let props_str = match props_str {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let props: HashMap<String, serde_json::Value> =
            serde_json::from_str(&props_str).unwrap_or_default();
        Ok(prop_uuid(props.get("context_id")))
    }

    /// True iff `entity_id` is visible under the active scope. Unscoped
    /// rows (no `context_id` property) are *always* visible — legacy
    /// data from before Sprint C-0 stays reachable. When `cross_context`
    /// is true the function short-circuits to `true` (no filtering).
    pub fn entity_in_active_scope(&self, entity_id: Uuid, cross_context: bool) -> Result<bool> {
        if cross_context {
            return Ok(true);
        }
        let active = self.active_context_id();
        if active.is_none() {
            return Ok(true);
        }
        let row_ctx = self.entity_context_id(entity_id)?;
        Ok(match row_ctx {
            None => true, // unscoped row — always visible
            Some(c) => Some(c) == active,
        })
    }

    /// True iff `triple_id` is visible under the active scope. Same
    /// semantics as `entity_in_active_scope`.
    pub fn triple_in_active_scope(&self, triple_id: Uuid, cross_context: bool) -> Result<bool> {
        if cross_context {
            return Ok(true);
        }
        let active = self.active_context_id();
        if active.is_none() {
            return Ok(true);
        }
        let row_ctx = self.triple_context_id(triple_id)?;
        Ok(match row_ctx {
            None => true,
            Some(c) => Some(c) == active,
        })
    }

    /// True iff `signal_id` is visible under the active scope. Reads
    /// the dedicated `captured_signals.context_id` column.
    pub fn signal_in_active_scope(&self, signal_id: i64, cross_context: bool) -> Result<bool> {
        if cross_context {
            return Ok(true);
        }
        let active = self.active_context_id();
        if active.is_none() {
            return Ok(true);
        }
        let conn = self.kg.connection();
        let ctx_str: std::result::Result<Option<String>, _> = conn.query_row(
            "SELECT context_id FROM captured_signals WHERE id = ?1",
            params![signal_id],
            |row| row.get(0),
        );
        match ctx_str {
            Ok(None) => Ok(true),
            Ok(Some(s)) => Ok(Uuid::parse_str(&s).ok() == active),
            Err(_) => Ok(true), // unknown row — be permissive
        }
    }

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

    /// LM-14 — overwrite the `context_id` property stored on the skg
    /// entity row. `None` strips the property (returns the row to
    /// "unscoped" / always-visible). Used by `context merge` and
    /// `context split` to move whole entity sets between contexts
    /// without re-ingesting.
    pub fn set_entity_context(&self, entity_id: Uuid, ctx_id: Option<Uuid>) -> Result<()> {
        let map = self.entity_map.borrow();
        let &skg_id = map.get(&entity_id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {entity_id} not in id map"))
        })?;
        drop(map);
        let mut skg_ent = self
            .kg
            .get_entity(skg_id)
            .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;
        match ctx_id {
            Some(c) => skg_ent.set_property("context_id", json!(c.to_string())),
            None => skg_ent.set_property("context_id", json!(null)),
        }
        skg_ent.set_property("updated_at", json!(Utc::now().to_rfc3339()));
        self.kg
            .update_entity(&skg_ent)
            .map_err(|e| TraceMindError::Storage(format!("skg update_entity: {e}")))?;
        Ok(())
    }

    /// LM-14 — overwrite the `context_id` property on a triple's
    /// `kg_relations.properties` JSON. Same semantics as
    /// [`set_entity_context`].
    pub fn set_triple_context(&self, triple_id: Uuid, ctx_id: Option<Uuid>) -> Result<()> {
        let map = self.triple_map.borrow();
        let &skg_id = map.get(&triple_id).ok_or_else(|| {
            TraceMindError::Storage(format!("triple {triple_id} not in id map"))
        })?;
        drop(map);
        let conn = self.kg.connection();
        let props_str: std::result::Result<String, _> = conn.query_row(
            "SELECT properties FROM kg_relations WHERE id = ?1",
            params![skg_id],
            |row| row.get(0),
        );
        let props_str = props_str.map_err(|e| {
            TraceMindError::Storage(format!("read triple props for {triple_id}: {e}"))
        })?;
        let mut props: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str::<serde_json::Value>(&props_str)
                .ok()
                .and_then(|v| match v {
                    serde_json::Value::Object(m) => Some(m),
                    _ => None,
                })
                .unwrap_or_default();
        match ctx_id {
            Some(c) => {
                props.insert("context_id".into(), json!(c.to_string()));
            }
            None => {
                props.remove("context_id");
            }
        }
        let new_props = serde_json::Value::Object(props).to_string();
        conn.execute(
            "UPDATE kg_relations SET properties = ?1 WHERE id = ?2",
            params![new_props, skg_id],
        )
        .map_err(|e| {
            TraceMindError::Storage(format!("update triple props for {triple_id}: {e}"))
        })?;
        Ok(())
    }

    /// LM-14 — list every entity tagged with `ctx_id`. Linear scan —
    /// fine for `tracemind context merge/split/snapshot` which is
    /// human-paced.
    pub fn list_entities_in_context(&self, ctx_id: Uuid) -> Result<Vec<Entity>> {
        let mut out = Vec::new();
        for e in self.list_all_entities()? {
            if self.entity_context_id(e.id)? == Some(ctx_id) {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// LM-14 / LM-17 — assemble a self-contained snapshot of a single
    /// context as a `serde_json::Value`. Returned value matches the
    /// `.tmctx` on-disk shape (schema_version = 1, key = "tracemind.context_snapshot")
    /// so callers can either persist it directly or attach it to a
    /// transport payload (Tauri, IPC). Errors only on storage failures —
    /// resolving the context by name is the caller's responsibility.
    pub fn snapshot_context(
        &self,
        ctx: &crate::context::Context,
    ) -> Result<serde_json::Value> {
        let entities = self.list_entities_in_context(ctx.id)?;
        let triples = self.list_triples_in_context(ctx.id)?;
        Ok(json!({
            "schema_version": 1,
            "kind": "tracemind.context_snapshot",
            "exported_at": Utc::now().to_rfc3339(),
            "context": {
                "id": ctx.id.to_string(),
                "name": ctx.name,
                "tags": ctx.tags,
            },
            "counts": {
                "entities": entities.len(),
                "triples": triples.len(),
            },
            "entities": entities,
            "triples": triples,
        }))
    }

    /// LM-14 — list every triple tagged with `ctx_id`. Uses SQLite's
    /// `json_extract` so the filter is at the storage layer.
    pub fn list_triples_in_context(&self, ctx_id: Uuid) -> Result<Vec<Triple>> {
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id FROM kg_relations \
                 WHERE json_extract(properties, '$.context_id') = ?1",
            )
            .map_err(|e| TraceMindError::Storage(format!("prepare list_triples_in_context: {e}")))?;
        let skg_ids: Vec<i64> = stmt
            .query_map(params![ctx_id.to_string()], |row| row.get::<_, i64>(0))
            .map_err(|e| TraceMindError::Storage(format!("query list_triples_in_context: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        // Reverse-map skg_id -> our triple uuid via triple_map.
        let tmap = self.triple_map.borrow();
        let mut wanted: Vec<Uuid> = Vec::new();
        for (uuid, skg_id) in tmap.iter() {
            if skg_ids.contains(skg_id) {
                wanted.push(*uuid);
            }
        }
        drop(tmap);

        // Now fetch each triple. `get_triples_for_entity` is the only
        // exposed reader, and it's keyed by subject/object — instead we
        // hydrate via a dedicated single-triple read.
        let mut out = Vec::with_capacity(wanted.len());
        for id in wanted {
            if let Some(t) = self.find_triple_by_id(id)? {
                out.push(t);
            }
        }
        Ok(out)
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

    /// Append a positive-feedback row (F-1 "helpful" channel). Mirror of
    /// `write_negative_signal` — `result_id` is opaque, `context_id` is
    /// the active context at the time of feedback (may be `None` if no
    /// context is active).
    pub fn write_positive_signal(
        &self,
        query_id: Uuid,
        result_id: &str,
        kind: &str,
        context_id: Option<Uuid>,
        weight: f32,
    ) -> Result<i64> {
        crate::context::write_positive_signal(
            self.kg.connection(),
            query_id,
            result_id,
            kind,
            context_id,
            weight,
        )
    }

    /// Sum of positive-signal weights for a query — added to the bandit
    /// reward by `finalize_pending_reward`.
    pub fn positive_weight_for_query(&self, query_id: Uuid) -> Result<f32> {
        crate::context::positive_weight_for_query(self.kg.connection(), query_id)
    }

    // ─── Q3.1 Feedback signal fabric ─────────────────────────────────────

    /// Record a first-class feedback signal with full provenance.
    /// Use this for all three signal classes (Explicit / Implicit / Behavioral).
    pub fn record_feedback_signal(
        &self,
        signal: &tm_types::FeedbackSignal,
    ) -> Result<uuid::Uuid> {
        crate::feedback_fabric::record_signal(self.kg.connection(), signal)
    }

    /// Retrieve all feedback signals for a given feedback_hook_id.
    pub fn signals_for_hook(
        &self,
        hook_id: uuid::Uuid,
    ) -> Result<Vec<tm_types::FeedbackSignal>> {
        crate::feedback_fabric::signals_for_hook(self.kg.connection(), hook_id)
    }

    /// Most-recent N signals of the given class (Explicit/Implicit/Behavioral).
    pub fn recent_feedback_signals(
        &self,
        class: tm_types::FeedbackClass,
        limit: usize,
    ) -> Result<Vec<tm_types::FeedbackSignal>> {
        crate::feedback_fabric::recent_signals_by_class(self.kg.connection(), class, limit)
    }

    /// Verb affinity: (verb, weighted_count) pairs sorted by weight desc.
    /// Used by Q4.12 user-behavior model to drive L2 space weights.
    pub fn verb_affinity(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        crate::feedback_fabric::verb_affinity(self.kg.connection(), limit)
    }

    // ─── Memory Views (LM-11a) ──────────────────────────────────────────

    pub fn create_view(&self, view: &crate::memory_view::MemoryView) -> Result<()> {
        crate::memory_view::create_view(self.kg.connection(), view)
    }

    pub fn list_views(&self) -> Result<Vec<crate::memory_view::MemoryView>> {
        crate::memory_view::list_views(self.kg.connection())
    }

    pub fn get_view_by_name(
        &self,
        name: &str,
    ) -> Result<Option<crate::memory_view::MemoryView>> {
        crate::memory_view::get_view_by_name(self.kg.connection(), name)
    }

    pub fn get_view(&self, id: Uuid) -> Result<Option<crate::memory_view::MemoryView>> {
        crate::memory_view::get_view(self.kg.connection(), id)
    }

    pub fn update_view_metadata(
        &self,
        view_id: Uuid,
        description: Option<&str>,
        confidence_floor: Option<f32>,
        include_pending: Option<bool>,
    ) -> Result<()> {
        crate::memory_view::update_view_metadata(
            self.kg.connection(),
            view_id,
            description,
            confidence_floor,
            include_pending,
        )
    }

    pub fn delete_view(&self, view_id: Uuid) -> Result<bool> {
        crate::memory_view::delete_view(self.kg.connection(), view_id)
    }

    pub fn add_view_member(
        &self,
        view_id: Uuid,
        kind: crate::memory_view::MemberKind,
        member_type: crate::memory_view::MemberType,
        member_id: Uuid,
    ) -> Result<()> {
        crate::memory_view::add_member(
            self.kg.connection(),
            view_id,
            kind,
            member_type,
            member_id,
        )
    }

    pub fn remove_view_member(
        &self,
        view_id: Uuid,
        kind: crate::memory_view::MemberKind,
        member_type: crate::memory_view::MemberType,
        member_id: Uuid,
    ) -> Result<bool> {
        crate::memory_view::remove_member(
            self.kg.connection(),
            view_id,
            kind,
            member_type,
            member_id,
        )
    }

    pub fn list_view_members(
        &self,
        view_id: Uuid,
    ) -> Result<Vec<crate::memory_view::ViewMember>> {
        crate::memory_view::list_members(self.kg.connection(), view_id)
    }

    pub fn load_view_filter(&self, view_id: Uuid) -> Result<crate::memory_view::ViewFilter> {
        let view = self
            .get_view(view_id)?
            .ok_or_else(|| TraceMindError::Storage(format!("view {view_id} not found")))?;
        crate::memory_view::load_filter(self.kg.connection(), &view)
    }

    // ─── LM-9 Pending relations ─────────────────────────────────────────

    /// Insert one row into the pending pool. Callers should usually use
    /// [`Self::route_triple_by_confidence`] which respects the
    /// `ACCEPT_THRESHOLD` / `PENDING_FLOOR` constants.
    pub fn insert_pending(&self, row: &crate::pending_relations::PendingRelation) -> Result<()> {
        crate::pending_relations::insert(self.kg.connection(), row)
    }

    /// List pending pool rows. `None` returns every status.
    pub fn list_pending(
        &self,
        status_filter: Option<crate::pending_relations::PendingStatus>,
        limit: Option<usize>,
    ) -> Result<Vec<crate::pending_relations::PendingRelation>> {
        crate::pending_relations::list(self.kg.connection(), status_filter, limit)
    }

    /// Fetch one pending row by id.
    pub fn get_pending(
        &self,
        id: Uuid,
    ) -> Result<Option<crate::pending_relations::PendingRelation>> {
        crate::pending_relations::get(self.kg.connection(), id)
    }

    /// Accept a pending row: promote it to `kg_relations` and mark the
    /// row `accepted` for audit. The triple inherits the pending row's
    /// confidence (which the caller may have bumped before accepting).
    /// Returns the freshly created [`Triple`].
    pub fn accept_pending(&self, id: Uuid, note: &str) -> Result<Triple> {
        let pending = self
            .get_pending(id)?
            .ok_or_else(|| TraceMindError::Storage(format!("pending {id} not found")))?;
        if pending.status != crate::pending_relations::PendingStatus::Pending {
            return Err(TraceMindError::Storage(format!(
                "pending {id} is already {:?}",
                pending.status
            )));
        }
        let predicate = parse_predicate(&pending.predicate);
        let mut triple = Triple::new(
            pending.subject_id,
            predicate,
            pending.object_id,
            pending.confidence,
        );
        triple.source_id = pending.source_id.clone();
        self.upsert_triple(&triple)?;
        crate::pending_relations::set_status(
            self.kg.connection(),
            id,
            crate::pending_relations::PendingStatus::Accepted,
            note,
        )?;
        Ok(triple)
    }

    /// Reject a pending row. The row stays in the table for audit; the
    /// triple never enters `kg_relations`.
    pub fn reject_pending(&self, id: Uuid, note: &str) -> Result<bool> {
        crate::pending_relations::set_status(
            self.kg.connection(),
            id,
            crate::pending_relations::PendingStatus::Rejected,
            note,
        )
    }

    /// Purge terminal-state pending rows older than `days` days.
    pub fn purge_pending(&self, days: i64) -> Result<usize> {
        crate::pending_relations::purge_decided_older_than(self.kg.connection(), days)
    }

    /// LM-9 confidence routing.
    ///
    /// * `confidence >= ACCEPT_THRESHOLD`  → write directly to `kg_relations`
    /// * `PENDING_FLOOR <= confidence < ACCEPT_THRESHOLD` → land in `pending_relations`
    /// * `confidence < PENDING_FLOOR`  → drop entirely
    ///
    /// Returns [`PendingRouteOutcome`] describing what happened so the
    /// ingest pipeline can log it.
    pub fn route_triple_by_confidence(&self, triple: &Triple) -> Result<PendingRouteOutcome> {
        if triple.confidence >= crate::pending_relations::ACCEPT_THRESHOLD {
            self.upsert_triple(triple)?;
            return Ok(PendingRouteOutcome::Accepted);
        }
        if triple.confidence >= crate::pending_relations::PENDING_FLOOR {
            let row = crate::pending_relations::PendingRelation::new(
                triple.subject_id,
                triple.predicate.to_string(),
                triple.object_id,
                triple.confidence,
                triple.source_id.clone(),
            );
            let id = row.id;
            self.insert_pending(&row)?;
            return Ok(PendingRouteOutcome::Pending(id));
        }
        Ok(PendingRouteOutcome::Dropped)
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
            // Sprint C-0: tag with the active context. On update we
            // only *add* a context when one is now active — we never
            // clobber an existing tag, so re-ingesting a known entity
            // under "no active context" keeps it scoped to its
            // original context.
            set_entity_context(&mut skg_ent, *self.active_context_id.borrow());

            self.kg
                .update_entity(&skg_ent)
                .map_err(|e| TraceMindError::Storage(format!("skg update_entity: {e}")))?;
        } else {
            // Insert new.
            let mut skg_ent = SkgEntity::new(&etype_json, &entity.name);
            set_entity_props(&mut skg_ent, entity);
            set_entity_context(&mut skg_ent, *self.active_context_id.borrow());

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
                let predicate_confidence = props
                    .get("predicate_confidence")
                    .and_then(|v| v.as_f64());
                Ok(Some(Triple {
                    id,
                    subject_id,
                    predicate,
                    object_id,
                    confidence: weight,
                    source_id,
                    created_at,
                    updated_at,
                    predicate_confidence,
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
    /// Iterate every entity as `(uuid, name, entity_type)`. Used by the
    /// `tracemind reclassify` CLI backfill — small data layer that the
    /// caller can pair with the current heuristic to repair legacy rows
    /// the looser GLiNER + heuristic mislabeled.
    pub fn list_entity_types(&self) -> Result<Vec<(Uuid, String, EntityType)>> {
        let map = self.entity_map.borrow();
        let mut out = Vec::with_capacity(map.len());
        for (uuid, &skg_id) in map.iter() {
            let skg_ent = self
                .kg
                .get_entity(skg_id)
                .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;
            let etype: EntityType = serde_json::from_str(&skg_ent.entity_type)
                .map_err(|e| TraceMindError::Storage(format!("entity_type parse: {e}")))?;
            out.push((*uuid, skg_ent.name.clone(), etype));
        }
        Ok(out)
    }

    /// Overwrite an entity's `entity_type` in place. Touches nothing else
    /// (name, properties, confidence, context tag all preserved).
    pub fn set_entity_type(&self, id: Uuid, etype: EntityType) -> Result<()> {
        let map = self.entity_map.borrow();
        let &skg_id = map.get(&id).ok_or_else(|| {
            TraceMindError::Storage(format!("entity {id} not in id map"))
        })?;
        drop(map);
        let mut skg_ent = self
            .kg
            .get_entity(skg_id)
            .map_err(|e| TraceMindError::Storage(format!("skg get_entity: {e}")))?;
        let etype_json = serde_json::to_string(&etype)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        skg_ent.entity_type = etype_json;
        skg_ent.set_property("updated_at", json!(Utc::now().to_rfc3339()));
        self.kg
            .update_entity(&skg_ent)
            .map_err(|e| TraceMindError::Storage(format!("skg update_entity: {e}")))?;
        Ok(())
    }

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

        // ONT-1 gate: if both endpoints are bound to Object Types and the
        // predicate isn't an allowed Link Type between them, reject. Untyped
        // endpoints pass through unchanged (we don't force ontology adoption).
        {
            let conn = self.kg.connection();
            let predicate_label = triple.predicate.to_string();
            match crate::ontology_types::OntologyStore::check_triple(
                conn,
                triple.subject_id,
                triple.object_id,
                &predicate_label,
            )? {
                crate::ontology_types::TypeCheckOutcome::Allowed
                | crate::ontology_types::TypeCheckOutcome::Untyped => {}
                crate::ontology_types::TypeCheckOutcome::Rejected { reason } => {
                    return Err(TraceMindError::Storage(format!(
                        "ontology rejected triple: {reason}"
                    )));
                }
            }
        }

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
        let active_ctx = *self.active_context_id.borrow();
        if let Some(&skg_id) = tmap.get(&triple.id) {
            // Update via raw SQL (skg has no update_relation).
            let props = triple_props(triple, &predicate_str, active_ctx);
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
            if let Some(ctx_id) = active_ctx {
                rel.set_property("context_id", json!(ctx_id.to_string()));
            }
            // LM-7: SML-supplied predicate-label score (optional).
            if let Some(pc) = triple.predicate_confidence {
                rel.set_property("predicate_confidence", json!(pc));
            }

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

    /// Detect contradictions introduced by a set of freshly-extracted
    /// triples, by looking for an existing triple that shares the new
    /// triple's subject and a *functional* predicate but names a different
    /// object.
    ///
    /// Only functional predicates are checked — relations where a subject is
    /// expected to have a single object, so a second value is a genuine
    /// reversal ("works_at", "lives_in", "renamed_to") rather than a set
    /// membership ("collaborates_with", "references") where multiple objects
    /// are normal. This keeps the retraction beat from crying wolf on facts
    /// that legitimately accumulate.
    pub fn detect_store_contradictions(&self, new_triples: &[Triple]) -> Vec<StoreContradiction> {
        let mut out = Vec::new();
        for t in new_triples {
            if !is_functional_predicate(&t.predicate) {
                continue;
            }
            let existing = match self.get_triples_for_entity(t.subject_id) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for e in &existing {
                if e.id == t.id {
                    continue;
                }
                if e.subject_id == t.subject_id
                    && predicate_key(&e.predicate) == predicate_key(&t.predicate)
                    && e.object_id != t.object_id
                {
                    let subject = self.entity_name_or(t.subject_id, "something");
                    let old_object = self.entity_name_or(e.object_id, "something");
                    let new_object = self.entity_name_or(t.object_id, "something");
                    let pred = humanize_predicate(&t.predicate);
                    out.push(StoreContradiction {
                        message: format!(
                            "You told me {subject} {pred} {old_object}, but now it's {new_object}."
                        ),
                        subject,
                        predicate: pred,
                        old_object,
                        new_object,
                        old_triple_id: e.id,
                    });
                }
            }
        }
        out
    }

    /// Contradiction-rate statistics over the temporal facts (wires
    /// `contradiction_rate`, previously orphaned). Used by the nightly
    /// self-improvement run to report a real signal.
    pub fn contradiction_rate_stats(
        &self,
    ) -> Result<crate::contradiction_rate::ContradictionRateStats> {
        let conn = self.kg.connection();
        let _ = crate::contradiction_rate::ensure_temporal_columns(conn);
        crate::contradiction_rate::compute_contradiction_rate(conn)
            .map_err(|e| TraceMindError::Storage(format!("contradiction_rate: {e}")))
    }

    /// Close the valid-time interval of a triple's fact — it stopped being
    /// true when `at` occurred, because a newer fact reversed it. Called
    /// when the retraction beat fires so the bitemporal history is correct:
    /// an as-of query before `at` still returns the old value.
    pub fn supersede_triple(&self, old_triple_id: Uuid, at: DateTime<Utc>) -> Result<()> {
        self.temporal
            .close_validity(old_triple_id, FACT_TYPE_TRIPLE, at)
            .map_err(temporal_err)
    }

    /// Entity display name, or `fallback` if the entity is unknown.
    pub fn entity_name_or(&self, id: Uuid, fallback: &str) -> String {
        self.get_entity(id)
            .ok()
            .map(|e| e.name)
            .unwrap_or_else(|| fallback.to_string())
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

            let predicate_confidence = props
                .get("predicate_confidence")
                .and_then(|v| v.as_f64());
            triples.push(Triple {
                id: triple_uuid,
                subject_id: subject_uuid,
                predicate,
                object_id: object_uuid,
                confidence: weight,
                source_id,
                created_at,
                updated_at,
                predicate_confidence,
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
        // Write the timestamp explicitly in RFC3339 rather than relying on
        // the column's `datetime('now')` default. The default produces
        // "2026-07-22 15:32:16" (space separator, no offset) while every
        // range query binds `DateTime::to_rfc3339()`
        // ("2026-07-22T14:32:16+00:00"). Those are compared as *strings*,
        // and ' ' (0x20) sorts before 'T' (0x54), so a row written "now"
        // always compares as earlier than a lower bound written an hour ago
        // — making every access-log range query return nothing.
        conn.execute(
            "INSERT INTO access_log (entity_id, event_type, context, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                entity_id.to_string(),
                event_type,
                context,
                Utc::now().to_rfc3339()
            ],
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
        let ctx = self.active_context_id.borrow().map(|u| u.to_string());
        conn.execute(
            "INSERT INTO captured_signals (source, raw_text, content_hash, relevance_score, ingested, context_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![source, raw_text, content_hash as i64, relevance_score, ingested as i32, ctx],
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
        // SimHash signature for the two-stage ANN prefilter (stored as i64).
        let simhash = crate::simhash::signature(embedding) as i64;
        let conn = self.kg.connection();
        let ctx = self.active_context_id.borrow().map(|u| u.to_string());
        conn.execute(
            "INSERT INTO captured_signals \
             (source, raw_text, content_hash, relevance_score, ingested, session_id, embedding, priority_tier, context_id, simhash) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9)",
            params![
                source,
                raw_text,
                content_hash as i64,
                relevance_score,
                session_id.to_string(),
                blob,
                priority_tier,
                ctx,
                simhash
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

    /// CLU-2 — every captured signal that still carries an embedding,
    /// regardless of cluster assignment. Used by the periodic full
    /// HDBSCAN re-cluster so already-assigned events can move between
    /// clusters when the new partition is computed.
    ///
    /// Returned pairs are `(content_hash_hex, embedding)` so callers
    /// can feed [`tm_cluster::Clusterer::recluster`] directly. The hex
    /// format (16-char lowercase, zero-padded) matches what
    /// `IngestPipeline::ingest_fast` uses when it calls
    /// `Clusterer::assign`, so re-clustering an event updates the
    /// existing record instead of inserting a duplicate.
    pub fn all_signals_with_embeddings(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, Vec<f32>)>> {
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT content_hash, embedding FROM captured_signals \
                 WHERE embedding IS NOT NULL AND priority_tier < 4 \
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep all_signals: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let hash_i64: i64 = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((hash_i64, blob))
            })
            .map_err(|e| TraceMindError::Storage(format!("query all_signals: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (hash_i64, blob) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            if blob.len() < 4 || blob.len() % 4 != 0 {
                continue;
            }
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            out.push((format!("{:016x}", hash_i64 as u64), embedding));
        }
        Ok(out)
    }

    /// CLU-5 — Louvain community pass over `kg_entities` + `kg_relations`.
    /// Thin wrapper around [`crate::community::recompute_communities`] so
    /// callers (Tauri `cmd_consolidate`, CLI consolidate path) don't need
    /// a direct `KnowledgeGraph` handle.
    pub fn recompute_communities(&self) -> Result<crate::community::CommunityStats> {
        crate::community::recompute_communities(&self.kg)
    }

    /// CLU-2 / SALIENCE — recompute per-entity salience over the live
    /// graph. Thin wrapper around [`crate::salience::recompute`] for
    /// the same reasons as [`Self::recompute_communities`].
    pub fn recompute_salience(&self) -> Result<crate::salience::SalienceStats> {
        crate::salience::recompute(&self.kg)
    }

    /// CLU-5b — Recompute c-TF-IDF labels for every populated community.
    /// Must run after [`Self::recompute_communities`] in the same pass.
    pub fn recompute_community_labels(
        &self,
    ) -> Result<crate::community::CommunityLabelStats> {
        crate::community::recompute_community_labels(&self.kg)
    }

    /// Read-back of the persisted `community_id -> label` map. Empty
    /// until [`Self::recompute_community_labels`] has run at least once.
    pub fn community_label_map(
        &self,
    ) -> Result<std::collections::HashMap<i64, crate::community::CommunityLabel>> {
        crate::community::community_label_map(&self.kg)
    }

    /// CLU-5c — sample inputs for the slow-path LLM community labeler.
    /// Returns the top-`n` communities (by size) with their member names
    /// and intra-community relation types so a Tier-1 LLM can name them.
    pub fn top_community_samples(
        &self,
        top_n: usize,
        max_names: usize,
        max_rels: usize,
    ) -> Result<Vec<crate::community::CommunitySample>> {
        crate::community::top_community_samples(&self.kg, top_n, max_names, max_rels)
    }

    /// Upsert a single community label. Used by the LLM labeler to
    /// overwrite the c-TF-IDF fallback for the largest communities.
    pub fn set_community_label(
        &self,
        community_id: i64,
        label: &str,
        terms_json: &str,
    ) -> Result<()> {
        crate::community::set_community_label(&self.kg, community_id, label, terms_json)
    }

    /// Stage 1 of the two-stage signal search: return the ids of the best
    /// candidates by SimHash Hamming distance, unioned with a recency window.
    ///
    /// Loads only `(id, simhash, embedding-when-signature-missing)` so the
    /// hot path never materialises full embeddings for the whole corpus.
    /// The candidate budget scales with `top_k` but stays generous, because
    /// Hamming is nearly free and the exact rerank restores precision.
    fn signal_ann_candidates(&self, query_sig: u64, top_k: usize) -> Result<Vec<i64>> {
        use std::collections::HashSet;

        // How many candidates to hand to the exact reranker, and how many of
        // the most-recent signals to always include regardless of signature.
        let candidate_k = (top_k * 8).max(256);
        let recency_window = (top_k * 4).max(128);

        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id, simhash, embedding FROM captured_signals \
                 WHERE cluster_id IS NULL AND embedding IS NOT NULL AND priority_tier < 4",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep ann scan: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let sig: Option<i64> = row.get(1)?;
                // Only fetch the blob when the signature is missing (older row).
                let blob: Option<Vec<u8>> = if sig.is_none() { row.get(2)? } else { None };
                Ok((id, sig, blob))
            })
            .map_err(|e| TraceMindError::Storage(format!("ann scan: {e}")))?;

        // (hamming, id), plus a running record of the most-recent ids.
        let mut by_hamming: Vec<(u32, i64)> = Vec::new();
        for row in rows {
            let (id, sig, blob) = row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let signature = match sig {
                Some(s) => s as u64,
                None => {
                    // Backfill: compute from the embedding and persist so the
                    // next query is on the fast path.
                    let emb: Vec<f32> = blob
                        .unwrap_or_default()
                        .chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    let s = crate::simhash::signature(&emb);
                    let _ = conn.execute(
                        "UPDATE captured_signals SET simhash = ?1 WHERE id = ?2",
                        params![s as i64, id],
                    );
                    s
                }
            };
            by_hamming.push((crate::simhash::hamming(query_sig, signature), id));
        }

        if by_hamming.is_empty() {
            return Ok(Vec::new());
        }

        // Recency window: the highest ids (most recently inserted).
        let mut recent_ids: Vec<i64> = by_hamming.iter().map(|(_, id)| *id).collect();
        recent_ids.sort_unstable_by(|a, b| b.cmp(a));
        recent_ids.truncate(recency_window);

        // Top candidates by Hamming distance.
        by_hamming.sort_by_key(|(h, _)| *h);
        let mut chosen: HashSet<i64> = by_hamming
            .iter()
            .take(candidate_k)
            .map(|(_, id)| *id)
            .collect();
        chosen.extend(recent_ids);
        Ok(chosen.into_iter().collect())
    }

    /// Load full [`CapturedSignal`] rows for a specific set of ids.
    fn load_signals_by_ids(&self, ids: &[i64]) -> Result<Vec<CapturedSignal>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.kg.connection();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, source, raw_text, content_hash, session_id, embedding, created_at \
             FROM captured_signals WHERE id IN ({placeholders})"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| TraceMindError::Storage(format!("prep load_by_ids: {e}")))?;
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params_vec.as_slice(), |row| {
                let id: i64 = row.get(0)?;
                let source: String = row.get(1)?;
                let raw_text: String = row.get(2)?;
                let hash_i64: i64 = row.get(3)?;
                let session_str: Option<String> = row.get(4)?;
                let blob: Vec<u8> = row.get(5)?;
                let created_str: String = row.get(6)?;
                Ok((id, source, raw_text, hash_i64, session_str, blob, created_str))
            })
            .map_err(|e| TraceMindError::Storage(format!("load_by_ids: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            let (id, source, raw_text, hash_i64, session_str, blob, created_str) =
                row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            let embedding: Vec<f32> = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            out.push(CapturedSignal {
                id,
                source,
                raw_text,
                content_hash: hash_i64 as u64,
                session_id: session_str.and_then(|s| Uuid::parse_str(&s).ok()),
                embedding,
                created_at: created_str.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(out)
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
        // Two-stage ANN (holistic review §5 P2.6). The previous
        // implementation loaded the oldest 2,000 full embeddings and
        // cosine-scored them — a *silent* recall cliff, because past 2,000
        // unconsolidated captures the newest memories were dropped with no
        // error. Instead:
        //
        //   Stage 1 (cheap): load only (id, simhash) — 8 bytes each — for
        //   *every* unconsolidated non-ephemeral signal, and rank by Hamming
        //   distance to the query signature. No 384-dim work, no cap.
        //   Stage 2 (exact): load full embeddings for the top Hamming
        //   candidates plus a recency window (so a brand-new memory whose
        //   signature happens to differ is never dropped) and cosine-score
        //   only those.
        let query_sig = crate::simhash::signature(query_embedding);

        // Stage 1: signature scan. Rows missing a signature (older builds)
        // are backfilled from their embedding so they still participate.
        let candidate_ids = self.signal_ann_candidates(query_sig, top_k)?;
        if candidate_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Stage 2: exact cosine on the candidate set only.
        let signals = self.load_signals_by_ids(&candidate_ids)?;
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
                // `replace(created_at, ' ', 'T')` normalises rows written by
                // older builds (which used the `datetime('now')` default) so
                // they compare correctly against RFC3339 bounds.
                "SELECT DISTINCT entity_id FROM access_log \
                 WHERE replace(created_at, ' ', 'T') >= ?1 \
                   AND replace(created_at, ' ', 'T') < ?2 \
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

    /// LM-1: Backlinks panel data. For an entity `target`, return every
    /// *typed* (non-`RelatedTo`) triple that points at it, paired with
    /// the source entity. Sorted by confidence desc so the Tauri /
    /// Brief panels render the strongest links first.
    ///
    /// `limit = None` returns everything; `Some(n)` truncates.
    /// `include_related_to = true` opts back in to the noisy co-
    /// occurrence edges — disabled by default because they make the
    /// panel unreadable.
    pub fn backlinks(
        &self,
        target: Uuid,
        limit: Option<usize>,
        include_related_to: bool,
    ) -> Result<Vec<Backlink>> {
        let triples = self.get_triples_for_entity(target)?;
        let mut rows: Vec<Backlink> = Vec::new();
        for t in triples {
            // Only incoming edges where the target is the *object*.
            if t.object_id != target {
                continue;
            }
            // Skip self-loops in the panel — they read as noise.
            if t.subject_id == target {
                continue;
            }
            if !include_related_to && matches!(t.predicate, Predicate::RelatedTo) {
                continue;
            }
            let source = match self.get_entity(t.subject_id) {
                Ok(e) => e,
                Err(_) => continue, // dangling source, skip
            };
            rows.push(Backlink {
                triple_id: t.id,
                source,
                predicate: t.predicate,
                confidence: t.confidence,
            });
        }
        rows.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(n) = limit {
            rows.truncate(n);
        }
        Ok(rows)
    }

    /// List every triple currently in `kg_relations`. Used by the
    /// MOC generator (LM-5d) and by the cluster labeler (LM-21).
    pub fn list_all_triples(&self) -> Result<Vec<Triple>> {
        let ids: Vec<Uuid> = self.triple_map.borrow().keys().copied().collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(Some(t)) = self.find_triple_by_id(id) {
                out.push(t);
            }
        }
        Ok(out)
    }

    /// LM-5b — upsert one tag for an entity. Idempotent on
    /// `(entity_id, tag)`. `source` is informational
    /// ("auto" | "hashtag" | "type" | "cluster" | "manual").
    pub fn upsert_entity_tag(
        &self,
        entity_id: Uuid,
        tag: &str,
        source: &str,
    ) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "INSERT OR IGNORE INTO kg_entity_tags (entity_id, tag, source) \
             VALUES (?1, ?2, ?3)",
            params![entity_id.to_string(), tag, source],
        )
        .map_err(|e| TraceMindError::Storage(format!("upsert_entity_tag: {e}")))?;
        Ok(())
    }

    /// LM-5b — bulk upsert. Returns count of newly inserted rows.
    pub fn upsert_entity_tags(
        &self,
        entity_id: Uuid,
        tags: &[String],
        source: &str,
    ) -> Result<usize> {
        let mut new_rows = 0;
        for tag in tags {
            if tag.is_empty() {
                continue;
            }
            let conn = self.kg.connection();
            let changes = conn
                .execute(
                    "INSERT OR IGNORE INTO kg_entity_tags (entity_id, tag, source) \
                     VALUES (?1, ?2, ?3)",
                    params![entity_id.to_string(), tag, source],
                )
                .map_err(|e| TraceMindError::Storage(format!("upsert_entity_tags: {e}")))?;
            if changes > 0 {
                new_rows += 1;
            }
        }
        Ok(new_rows)
    }

    /// LM-5b — list tags for an entity, ordered alphabetically.
    pub fn tags_for_entity(&self, entity_id: Uuid) -> Result<Vec<String>> {
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT tag FROM kg_entity_tags WHERE entity_id = ?1 \
                 ORDER BY tag ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("tags_for_entity prep: {e}")))?;
        let rows: Vec<String> = stmt
            .query_map(params![entity_id.to_string()], |r| r.get::<_, String>(0))
            .map_err(|e| TraceMindError::Storage(format!("tags_for_entity q: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// LM-5b — drop one tag from an entity (used by user "untag").
    pub fn delete_entity_tag(&self, entity_id: Uuid, tag: &str) -> Result<bool> {
        let conn = self.kg.connection();
        let n = conn
            .execute(
                "DELETE FROM kg_entity_tags WHERE entity_id = ?1 AND tag = ?2",
                params![entity_id.to_string(), tag],
            )
            .map_err(|e| TraceMindError::Storage(format!("delete_entity_tag: {e}")))?;
        Ok(n > 0)
    }

    /// LM-20 — Memory Garden: cluster buckets over `captured_signals`.
    /// Returns `(cluster_id, count)` pairs sorted by count desc. A
    /// `None` cluster_id is the outlier bucket. `-1` is HDBSCAN's
    /// canonical "noise" assignment; `-2` is our manual "ignore"
    /// triage outcome (LM-22) — both fold into the unsorted tray.
    pub fn cluster_buckets(&self, limit: usize) -> Result<Vec<(Option<i64>, usize)>> {
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT cluster_id, COUNT(*) FROM captured_signals \
                 GROUP BY cluster_id ORDER BY COUNT(*) DESC LIMIT ?1",
            )
            .map_err(|e| TraceMindError::Storage(format!("cluster_buckets prepare: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let cid: Option<i64> = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((cid, count as usize))
            })
            .map_err(|e| TraceMindError::Storage(format!("cluster_buckets query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(
                r.map_err(|e| TraceMindError::Storage(format!("cluster_buckets row: {e}")))?,
            );
        }
        Ok(out)
    }

    /// LM-20 — sample raw_text rows for a specific cluster. When
    /// `cluster_id` is `None` or `-1`/`-2` the outlier bucket is used.
    pub fn cluster_samples(&self, cluster_id: Option<i64>, limit: usize) -> Result<Vec<String>> {
        let conn = self.kg.connection();
        let is_outlier = matches!(cluster_id, None | Some(-1) | Some(-2));
        let mut samples = Vec::new();
        if is_outlier {
            let mut stmt = conn
                .prepare(
                    "SELECT raw_text FROM captured_signals \
                     WHERE cluster_id IS NULL OR cluster_id = -1 OR cluster_id = -2 \
                     ORDER BY id DESC LIMIT ?1",
                )
                .map_err(|e| TraceMindError::Storage(format!("cluster_samples prep: {e}")))?;
            let rows = stmt
                .query_map(params![limit as i64], |row| row.get::<_, String>(0))
                .map_err(|e| TraceMindError::Storage(format!("cluster_samples q: {e}")))?;
            for r in rows {
                samples.push(
                    r.map_err(|e| TraceMindError::Storage(format!("cluster_samples row: {e}")))?,
                );
            }
        } else {
            let cid = cluster_id.unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT raw_text FROM captured_signals \
                     WHERE cluster_id = ?1 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| TraceMindError::Storage(format!("cluster_samples prep: {e}")))?;
            let rows = stmt
                .query_map(params![cid, limit as i64], |row| row.get::<_, String>(0))
                .map_err(|e| TraceMindError::Storage(format!("cluster_samples q: {e}")))?;
            for r in rows {
                samples.push(
                    r.map_err(|e| TraceMindError::Storage(format!("cluster_samples row: {e}")))?,
                );
            }
        }
        Ok(samples)
    }

    /// LM-21 — compute c-TF-IDF labels for every populated cluster.
    /// Walks `captured_signals`, groups by `cluster_id`, asks
    /// [`crate::labeler::label_clusters`] for a top-K topic phrase per
    /// cluster, and returns `cluster_id → label`. Outlier buckets
    /// (`NULL`, `-1`, `-2`) are skipped — the UI labels those as
    /// "unsorted" / "ignored" without TF-IDF.
    ///
    /// `sample_cap` bounds how many rows per cluster we pull when
    /// computing labels. 25 is a good default — labels stabilise at
    /// ~10 samples and we don't want this query to scan the full
    /// signal log on a huge memory.
    /// Like [`Self::cluster_labels`] but returns the raw
    /// `cluster_id → samples` map without running the labeler. Used by
    /// the ONT-2 proposer runner so callers outside this crate can feed
    /// the proposer without depending on rusqlite directly.
    pub fn cluster_sample_map(
        &self,
        sample_cap: usize,
    ) -> Result<std::collections::HashMap<i64, Vec<String>>> {
        let conn = self.kg.connection();
        let mut id_stmt = conn
            .prepare(
                "SELECT DISTINCT cluster_id FROM captured_signals \
                 WHERE cluster_id IS NOT NULL AND cluster_id >= 0",
            )
            .map_err(|e| TraceMindError::Storage(format!("cluster_sample_map prep ids: {e}")))?;
        let ids: Vec<i64> = id_stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| TraceMindError::Storage(format!("cluster_sample_map q ids: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        drop(id_stmt);

        let mut samples: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for cid in ids {
            let mut stmt = conn
                .prepare(
                    "SELECT raw_text FROM captured_signals \
                     WHERE cluster_id = ?1 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| TraceMindError::Storage(format!("cluster_sample_map prep: {e}")))?;
            let texts: Vec<String> = stmt
                .query_map(params![cid, sample_cap as i64], |r| r.get::<_, String>(0))
                .map_err(|e| TraceMindError::Storage(format!("cluster_sample_map q: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            if !texts.is_empty() {
                samples.insert(cid, texts);
            }
        }
        Ok(samples)
    }

    pub fn cluster_labels(
        &self,
        sample_cap: usize,
    ) -> Result<std::collections::HashMap<i64, crate::labeler::ClusterLabel>> {
        let conn = self.kg.connection();
        // Pull cluster ids first so we can issue one focused fetch per
        // bucket and keep memory bounded even on huge tables.
        let mut id_stmt = conn
            .prepare(
                "SELECT DISTINCT cluster_id FROM captured_signals \
                 WHERE cluster_id IS NOT NULL AND cluster_id >= 0",
            )
            .map_err(|e| TraceMindError::Storage(format!("cluster_labels prep ids: {e}")))?;
        let ids: Vec<i64> = id_stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| TraceMindError::Storage(format!("cluster_labels q ids: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        drop(id_stmt);

        let mut samples: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        for cid in ids {
            let mut stmt = conn
                .prepare(
                    "SELECT raw_text FROM captured_signals \
                     WHERE cluster_id = ?1 ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| TraceMindError::Storage(format!("cluster_labels prep s: {e}")))?;
            let texts: Vec<String> = stmt
                .query_map(params![cid, sample_cap as i64], |r| r.get::<_, String>(0))
                .map_err(|e| TraceMindError::Storage(format!("cluster_labels q s: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            samples.insert(cid, texts);
        }
        Ok(crate::labeler::label_clusters(
            &samples,
            crate::labeler::LabelerConfig::default(),
        ))
    }

    /// LM-22 — Outlier "Unsorted" tray rows. `(signal_id, raw_text,
    /// source, created_at)`. Includes `cluster_id IS NULL`, `-1`
    /// (HDBSCAN noise) and `-2` (manual ignore) so the user can
    /// re-triage anything previously dismissed.
    pub fn list_outlier_signals(
        &self,
        limit: usize,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let conn = self.kg.connection();
        let mut stmt = conn
            .prepare(
                "SELECT id, raw_text, source, created_at FROM captured_signals \
                 WHERE cluster_id IS NULL OR cluster_id = -1 OR cluster_id = -2 \
                 ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| TraceMindError::Storage(format!("list_outliers prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| TraceMindError::Storage(format!("list_outliers q: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| TraceMindError::Storage(format!("list_outliers row: {e}")))?);
        }
        Ok(out)
    }

    /// LM-22 — assign one signal to a cluster. Returns the new
    /// cluster_id so the caller can echo it back to the UI.
    pub fn assign_signal_to_cluster(&self, signal_id: i64, cluster_id: i64) -> Result<()> {
        let conn = self.kg.connection();
        conn.execute(
            "UPDATE captured_signals SET cluster_id = ?1 WHERE id = ?2",
            params![cluster_id, signal_id],
        )
        .map_err(|e| TraceMindError::Storage(format!("assign_signal_to_cluster: {e}")))?;
        Ok(())
    }

    /// LM-22 — next cluster id for a "new_cluster" triage action.
    /// Returns `MAX(cluster_id) + 1` over the captured_signals table,
    /// or `0` when the table has no clusters yet.
    pub fn next_cluster_id(&self) -> Result<i64> {
        let conn = self.kg.connection();
        let max: Option<i64> = conn
            .query_row("SELECT MAX(cluster_id) FROM captured_signals", [], |row| {
                row.get(0)
            })
            .ok();
        Ok(max.unwrap_or(0) + 1)
    }

    /// LM-23 — community-overlay buckets. Returns `(community_id,
    /// count, sample_names)` tuples. When the `community_id` column
    /// doesn't exist yet (CLU-* hasn't populated it) returns a single
    /// `(None, total, top3)` tuple so the frontend renders the empty
    /// state.
    pub fn community_buckets(
        &self,
        limit: usize,
    ) -> Result<Vec<(Option<i64>, usize, Vec<String>)>> {
        let conn = self.kg.connection();
        let has_col: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('kg_entities') WHERE name = 'community_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|_| true)
            .unwrap_or(false);
        if !has_col {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM kg_entities", [], |row| row.get(0))
                .unwrap_or(0);
            let mut names_stmt = conn
                .prepare(
                    "SELECT name FROM kg_entities \
                     WHERE entity_type != '\"map_of_content\"' \
                     ORDER BY id DESC LIMIT 3",
                )
                .map_err(|e| TraceMindError::Storage(format!("community names prep: {e}")))?;
            let samples: Vec<String> = names_stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| TraceMindError::Storage(format!("community names q: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            return Ok(vec![(None, count as usize, samples)]);
        }
        let mut stmt = conn
            .prepare(
                "SELECT community_id, COUNT(*) FROM kg_entities \
                 GROUP BY community_id ORDER BY COUNT(*) DESC LIMIT ?1",
            )
            .map_err(|e| TraceMindError::Storage(format!("community prep: {e}")))?;
        let rows: Vec<(Option<i64>, usize)> = stmt
            .query_map(params![limit as i64], |row| {
                let cid: Option<i64> = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((cid, count as usize))
            })
            .map_err(|e| TraceMindError::Storage(format!("community q: {e}")))?
            .filter_map(|r| r.ok())
            .collect();
        let mut out = Vec::new();
        for (cid, count) in rows {
            let names: Vec<String> = match cid {
                Some(c) => {
                    // Exclude MOC entities so the secondary text shows
                    // real concepts, not "indexes indexes ..." backlinks.
                    let mut s = conn
                        .prepare(
                            "SELECT name FROM kg_entities WHERE community_id = ?1 \
                             AND entity_type != '\"map_of_content\"' \
                             ORDER BY id DESC LIMIT 3",
                        )
                        .map_err(|e| TraceMindError::Storage(format!("c names prep: {e}")))?;
                    let collected: Vec<String> = s
                        .query_map(params![c], |row| row.get::<_, String>(0))
                        .map_err(|e| TraceMindError::Storage(format!("c names q: {e}")))?
                        .filter_map(|r| r.ok())
                        .collect();
                    collected
                }
                None => {
                    let mut s = conn
                        .prepare(
                            "SELECT name FROM kg_entities WHERE community_id IS NULL \
                             AND entity_type != '\"map_of_content\"' \
                             ORDER BY id DESC LIMIT 3",
                        )
                        .map_err(|e| TraceMindError::Storage(format!("c names prep: {e}")))?;
                    let collected: Vec<String> = s
                        .query_map([], |row| row.get::<_, String>(0))
                        .map_err(|e| TraceMindError::Storage(format!("c names q: {e}")))?
                        .filter_map(|r| r.ok())
                        .collect();
                    collected
                }
            };
            out.push((cid, count, names));
        }
        Ok(out)
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

/// Stash the Sprint C-0 active context_id on a skg entity if set. Pulled
/// out from `set_entity_props` so updates don't clobber a previously
/// written context_id when the live context is None (e.g. CLI flow with
/// no active context after some rows were already scoped).
fn set_entity_context(skg_ent: &mut SkgEntity, ctx_id: Option<Uuid>) {
    if let Some(c) = ctx_id {
        skg_ent.set_property("context_id", json!(c.to_string()));
    }
}

fn triple_props(triple: &Triple, predicate_str: &str, ctx_id: Option<Uuid>) -> serde_json::Value {
    let mut obj = json!({
        "uuid": triple.id.to_string(),
        "predicate": predicate_str,
        "source_id": triple.source_id,
        "created_at": triple.created_at.to_rfc3339(),
        "updated_at": triple.updated_at.to_rfc3339(),
    });
    if let Some(map) = obj.as_object_mut() {
        if let Some(c) = ctx_id {
            map.insert("context_id".into(), json!(c.to_string()));
        }
        // LM-7: independent predicate-label score (SML-supplied).
        // Skipped when `None` so legacy rows don't grow a null field.
        if let Some(pc) = triple.predicate_confidence {
            map.insert("predicate_confidence".into(), json!(pc));
        }
    }
    obj
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
/// A store-time contradiction: a new fact whose (subject, predicate) already
/// had a *different* object on record. The retraction beat in data form.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoreContradiction {
    pub subject: String,
    pub predicate: String,
    /// What was on record before.
    pub old_object: String,
    /// What the new statement asserts.
    pub new_object: String,
    /// A ready-to-surface sentence for the host.
    pub message: String,
    /// The triple whose fact is being reversed (its validity should be
    /// closed). Skipped in the host-facing JSON; used internally.
    #[serde(skip)]
    pub old_triple_id: Uuid,
}

/// Predicates where a subject is expected to have a single object, so a
/// second, different object is a genuine reversal rather than a set that
/// legitimately grows. The retraction beat only fires on these.
fn is_functional_predicate(p: &Predicate) -> bool {
    match p {
        Predicate::WorksAt | Predicate::DependsOn | Predicate::HasProperty => true,
        Predicate::Custom(s) => {
            let s = s.to_lowercase();
            [
                "works_at", "lives_in", "located_in", "renamed_to", "based_in", "reports_to",
                "married_to", "born_in", "is_a", "employed_by", "member_of", "assigned_to",
                "due_on", "scheduled_for", "priced_at", "costs",
            ]
            .contains(&s.as_str())
        }
        _ => false,
    }
}

/// A stable key for comparing predicates by name.
fn predicate_key(p: &Predicate) -> String {
    match p {
        Predicate::Custom(s) => s.to_lowercase(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// Human-readable predicate for the retraction message: `WorksAt` → "works
/// at", `renamed_to` → "renamed to".
fn humanize_predicate(p: &Predicate) -> String {
    match p {
        Predicate::Custom(s) => s.replace('_', " ").to_lowercase(),
        other => {
            let dbg = format!("{other:?}");
            let mut out = String::new();
            for (i, ch) in dbg.chars().enumerate() {
                if ch.is_uppercase() && i > 0 {
                    out.push(' ');
                }
                out.push(ch.to_ascii_lowercase());
            }
            out
        }
    }
}

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

    /// LM-1: backlinks returns every incoming typed edge, sorted by
    /// confidence desc, with the source entity hydrated.
    #[test]
    fn backlinks_returns_typed_incoming_edges_sorted() {
        let store = GraphStore::open(":memory:").expect("open in-memory db");
        let alice = make_entity("Alice", EntityType::Person);
        let acme = make_entity("Acme Corp", EntityType::Organization);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&acme).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t1 = Triple::new(alice.id, Predicate::WorksAt, acme.id, 0.7);
        let t2 = Triple::new(bob.id, Predicate::WorksAt, acme.id, 0.9);
        let t_noise = Triple::new(alice.id, Predicate::RelatedTo, acme.id, 0.5);
        store.upsert_triple(&t1).unwrap();
        store.upsert_triple(&t2).unwrap();
        store.upsert_triple(&t_noise).unwrap();

        let panel = store.backlinks(acme.id, None, false).expect("backlinks");
        assert_eq!(panel.len(), 2, "RelatedTo is excluded by default: {panel:?}");
        // Highest confidence first.
        assert_eq!(panel[0].source.id, bob.id);
        assert!((panel[0].confidence - 0.9).abs() < 1e-6);
        assert_eq!(panel[1].source.id, alice.id);

        // include_related_to=true brings the co-occurrence edge back.
        let panel_all = store
            .backlinks(acme.id, None, true)
            .expect("backlinks include_related_to");
        assert_eq!(panel_all.len(), 3);

        // limit truncates.
        let panel_top = store.backlinks(acme.id, Some(1), false).expect("limit");
        assert_eq!(panel_top.len(), 1);
        assert_eq!(panel_top[0].source.id, bob.id);

        // Entity with no incoming typed edges → empty.
        let empty = store.backlinks(alice.id, None, false).expect("empty");
        assert!(empty.is_empty());
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
            predicate_confidence: None,
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
            predicate_confidence: None,
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
            predicate_confidence: None,
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

    /// Regression: access-log range queries silently returned nothing
    /// because rows were stored as "YYYY-MM-DD HH:MM:SS" and compared as
    /// strings against RFC3339 bounds.
    #[test]
    fn accessed_range_matches_rows_written_by_older_builds() {
        let store = GraphStore::open(":memory:").unwrap();
        let e1 = make_entity("Legacy", EntityType::Concept);
        store.upsert_entity(&e1).unwrap();
        // Simulate an old row: space separator, no offset.
        let legacy_ts = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        store
            .kg
            .connection()
            .execute(
                "INSERT INTO access_log (entity_id, event_type, created_at) VALUES (?1, ?2, ?3)",
                params![e1.id.to_string(), "query_result", legacy_ts],
            )
            .unwrap();

        let now = Utc::now();
        let accessed = store
            .get_accessed_entities_in_range(now - chrono::Duration::hours(1), now + chrono::Duration::hours(1))
            .unwrap();
        assert!(accessed.contains(&e1.id), "legacy-format rows must still match");
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
            predicate_confidence: None,
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

    // ─── Sprint C-0.5: context tagging at ingest time ─────────────────

    #[test]
    fn entity_tagged_with_active_context() {
        let store = GraphStore::open(":memory:").unwrap();
        let ctx_id = Uuid::new_v4();
        store.set_active_context(Some(ctx_id));

        let e = make_entity("Alice", EntityType::Person);
        store.upsert_entity(&e).unwrap();

        let tagged = store.entity_context_id(e.id).unwrap();
        assert_eq!(tagged, Some(ctx_id), "entity should inherit active ctx");
    }

    #[test]
    fn entity_untagged_when_no_active_context() {
        let store = GraphStore::open(":memory:").unwrap();
        // Default: no active context.
        let e = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&e).unwrap();

        let tagged = store.entity_context_id(e.id).unwrap();
        assert_eq!(tagged, None, "entity should be unscoped");
    }

    #[test]
    fn triple_tagged_with_active_context() {
        let store = GraphStore::open(":memory:").unwrap();
        let ctx_id = Uuid::new_v4();
        store.set_active_context(Some(ctx_id));

        let a = make_entity("X", EntityType::Person);
        let b = make_entity("Y", EntityType::Person);
        store.upsert_entity(&a).unwrap();
        store.upsert_entity(&b).unwrap();
        let t = Triple {
            id: Uuid::new_v4(),
            subject_id: a.id,
            predicate: Predicate::RelatedTo,
            object_id: b.id,
            confidence: 0.7,
            source_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            predicate_confidence: None,
        };
        store.upsert_triple(&t).unwrap();

        assert_eq!(store.triple_context_id(t.id).unwrap(), Some(ctx_id));
    }

    #[test]
    fn entity_in_active_scope_includes_unscoped() {
        let store = GraphStore::open(":memory:").unwrap();
        let ctx_a = Uuid::new_v4();
        let ctx_b = Uuid::new_v4();

        // Insert an unscoped legacy entity.
        let legacy = make_entity("Legacy", EntityType::Person);
        store.upsert_entity(&legacy).unwrap();

        // Insert one scoped to ctx_a.
        store.set_active_context(Some(ctx_a));
        let scoped_a = make_entity("InA", EntityType::Person);
        store.upsert_entity(&scoped_a).unwrap();

        // Insert one scoped to ctx_b.
        store.set_active_context(Some(ctx_b));
        let scoped_b = make_entity("InB", EntityType::Person);
        store.upsert_entity(&scoped_b).unwrap();

        // Activate ctx_a — legacy + scoped_a should be visible; scoped_b should not.
        store.set_active_context(Some(ctx_a));
        assert!(store.entity_in_active_scope(legacy.id, false).unwrap());
        assert!(store.entity_in_active_scope(scoped_a.id, false).unwrap());
        assert!(!store.entity_in_active_scope(scoped_b.id, false).unwrap());

        // cross_context=true short-circuits to true for everyone.
        assert!(store.entity_in_active_scope(scoped_b.id, true).unwrap());
    }

    #[test]
    fn signal_tagged_with_active_context() {
        let store = GraphStore::open(":memory:").unwrap();
        let ctx_id = Uuid::new_v4();
        store.set_active_context(Some(ctx_id));

        // log_signal goes through the tagged path.
        store
            .log_signal("test", "hello world", 42u64, None, false)
            .unwrap();

        // Read the row back via SQL.
        let conn = store.kg.connection();
        let ctx_str: Option<String> = conn
            .query_row(
                "SELECT context_id FROM captured_signals WHERE content_hash = ?1",
                params![42i64],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ctx_str, Some(ctx_id.to_string()));
    }

    // ─── LM-9: pending_relations integration ───────────────────────────

    fn make_triple(subject: Uuid, object: Uuid, confidence: f64) -> Triple {
        let mut t = Triple::new(subject, Predicate::CollaboratesWith, object, confidence);
        t.source_id = Some("test-cap".to_string());
        t
    }

    fn functional_triple(subject: Uuid, object: Uuid) -> Triple {
        let mut t = Triple::new(subject, Predicate::WorksAt, object, 0.9);
        t.source_id = Some("test-cap".to_string());
        t
    }

    // ── The retraction beat (review §5 P1.4) ────────────────────────────

    #[test]
    fn retraction_beat_fires_on_a_reversed_functional_fact() {
        let store = GraphStore::open(":memory:").unwrap();
        let carol = make_entity("Carol", EntityType::Person);
        let stripe = make_entity("Stripe", EntityType::Organization);
        let datadog = make_entity("Datadog", EntityType::Organization);
        for e in [&carol, &stripe, &datadog] {
            store.upsert_entity(e).unwrap();
        }
        // First: Carol works_at Stripe.
        store.upsert_triple(&functional_triple(carol.id, stripe.id)).unwrap();

        // Now she says Datadog — the retraction beat must fire.
        let new = functional_triple(carol.id, datadog.id);
        let hits = store.detect_store_contradictions(&[new]);
        assert_eq!(hits.len(), 1, "expected one contradiction, got {hits:?}");
        assert_eq!(hits[0].old_object, "Stripe");
        assert_eq!(hits[0].new_object, "Datadog");
        assert!(hits[0].message.contains("Stripe") && hits[0].message.contains("Datadog"));
    }

    #[test]
    fn retraction_beat_stays_silent_when_the_object_is_unchanged() {
        let store = GraphStore::open(":memory:").unwrap();
        let carol = make_entity("Carol", EntityType::Person);
        let stripe = make_entity("Stripe", EntityType::Organization);
        store.upsert_entity(&carol).unwrap();
        store.upsert_entity(&stripe).unwrap();
        store.upsert_triple(&functional_triple(carol.id, stripe.id)).unwrap();

        // Restating the same fact is not a contradiction.
        let same = functional_triple(carol.id, stripe.id);
        assert!(store.detect_store_contradictions(&[same]).is_empty());
    }

    /// Supersession is correct in *valid time*, not just a message: after a
    /// reversal, an as-of query before the reversal still returns the old
    /// fact, and after it the old fact is no longer valid. This is P1.5 —
    /// what makes the retraction beat trustworthy.
    #[test]
    fn superseding_a_triple_closes_its_valid_time() {
        let store = GraphStore::open(":memory:").unwrap();
        let carol = make_entity("Carol", EntityType::Person);
        let stripe = make_entity("Stripe", EntityType::Organization);
        store.upsert_entity(&carol).unwrap();
        store.upsert_entity(&stripe).unwrap();

        let t0 = Utc::now() - chrono::Duration::minutes(10);
        let mut old = functional_triple(carol.id, stripe.id);
        old.created_at = t0;
        old.updated_at = t0;
        store.upsert_triple(&old).unwrap();

        // Valid at t0 + 1min (before reversal).
        let before = t0 + chrono::Duration::minutes(1);
        assert!(
            store.triple_at(old.id, before).unwrap().is_some(),
            "old fact should be valid before the reversal"
        );

        // Reverse it now.
        let at = Utc::now();
        store.supersede_triple(old.id, at).unwrap();

        // Still valid *before* the reversal instant.
        assert!(
            store.triple_at(old.id, before).unwrap().is_some(),
            "history before the reversal must be preserved"
        );
        // No longer valid after.
        let after = at + chrono::Duration::minutes(1);
        assert!(
            store.triple_at(old.id, after).unwrap().is_none(),
            "old fact must not be valid after it was superseded"
        );
    }

    #[test]
    fn retraction_beat_does_not_cry_wolf_on_set_predicates() {
        // collaborates_with is non-functional — a second collaborator is
        // normal accumulation, not a reversal.
        let store = GraphStore::open(":memory:").unwrap();
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        let carol = make_entity("Carol", EntityType::Person);
        for e in [&alice, &bob, &carol] {
            store.upsert_entity(e).unwrap();
        }
        store.upsert_triple(&make_triple(alice.id, bob.id, 0.9)).unwrap();
        let new = make_triple(alice.id, carol.id, 0.9); // collaborates_with
        assert!(
            store.detect_store_contradictions(&[new]).is_empty(),
            "set-membership predicate must not trigger the retraction beat"
        );
    }

    #[test]
    fn route_triple_accepts_high_confidence_directly() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t = make_triple(alice.id, bob.id, 0.85);
        let outcome = store.route_triple_by_confidence(&t).unwrap();
        assert_eq!(outcome, PendingRouteOutcome::Accepted);

        // Triple landed in kg_relations.
        let edges = store.get_triples_for_entity(alice.id).unwrap();
        assert!(edges.iter().any(|e| e.id == t.id));
        // Pending pool is empty.
        let pending = store.list_pending(None, None).unwrap();
        assert!(pending.is_empty());
    }

    #[test]
    fn route_triple_pools_mid_confidence() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t = make_triple(alice.id, bob.id, 0.5);
        let outcome = store.route_triple_by_confidence(&t).unwrap();
        match outcome {
            PendingRouteOutcome::Pending(_id) => {}
            other => panic!("expected Pending, got {other:?}"),
        }

        // Triple did NOT land in kg_relations.
        let edges = store.get_triples_for_entity(alice.id).unwrap();
        assert!(edges.iter().all(|e| e.id != t.id));

        // Pending pool has one row.
        let pending = store
            .list_pending(Some(crate::pending_relations::PendingStatus::Pending), None)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].subject_id, alice.id);
        assert_eq!(pending[0].object_id, bob.id);
        assert_eq!(pending[0].predicate, "CollaboratesWith");
        assert!((pending[0].confidence - 0.5).abs() < 1e-9);
    }

    #[test]
    fn route_triple_drops_below_floor() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t = make_triple(alice.id, bob.id, 0.15);
        let outcome = store.route_triple_by_confidence(&t).unwrap();
        assert_eq!(outcome, PendingRouteOutcome::Dropped);

        assert!(store.list_pending(None, None).unwrap().is_empty());
        let edges = store.get_triples_for_entity(alice.id).unwrap();
        assert!(edges.iter().all(|e| e.id != t.id));
    }

    #[test]
    fn accept_pending_promotes_to_kg_relations() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t = make_triple(alice.id, bob.id, 0.55);
        let outcome = store.route_triple_by_confidence(&t).unwrap();
        let pending_id = match outcome {
            PendingRouteOutcome::Pending(id) => id,
            other => panic!("expected Pending, got {other:?}"),
        };

        let promoted = store.accept_pending(pending_id, "user accepted").unwrap();
        // Predicate hydrates back from the open-vocab string.
        assert_eq!(promoted.predicate, Predicate::CollaboratesWith);
        assert_eq!(promoted.subject_id, alice.id);
        assert_eq!(promoted.object_id, bob.id);
        assert_eq!(promoted.source_id.as_deref(), Some("test-cap"));

        // It's now in kg_relations.
        let edges = store.get_triples_for_entity(alice.id).unwrap();
        assert!(edges.iter().any(|e| e.id == promoted.id));

        // Pending row stamped accepted with a decided_at.
        let row = store.get_pending(pending_id).unwrap().unwrap();
        assert_eq!(row.status, crate::pending_relations::PendingStatus::Accepted);
        assert!(row.decided_at.is_some());

        // Double-accept rejected.
        let err = store.accept_pending(pending_id, "again").unwrap_err();
        assert!(format!("{err}").contains("already"));
    }

    #[test]
    fn reject_pending_keeps_row_out_of_kg() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let t = make_triple(alice.id, bob.id, 0.45);
        let pending_id = match store.route_triple_by_confidence(&t).unwrap() {
            PendingRouteOutcome::Pending(id) => id,
            other => panic!("expected Pending, got {other:?}"),
        };

        assert!(store.reject_pending(pending_id, "wrong").unwrap());
        let row = store.get_pending(pending_id).unwrap().unwrap();
        assert_eq!(row.status, crate::pending_relations::PendingStatus::Rejected);

        let edges = store.get_triples_for_entity(alice.id).unwrap();
        assert!(edges.is_empty());
    }

    #[test]
    fn predicate_confidence_roundtrips_through_storage() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        let triple =
            Triple::new(alice.id, Predicate::WorksAt, bob.id, 0.85).with_predicate_confidence(0.62);
        store.upsert_triple(&triple).unwrap();

        let edges = store.get_triples_for_entity(alice.id).unwrap();
        let stored = edges.iter().find(|t| t.id == triple.id).expect("triple");
        assert_eq!(stored.predicate_confidence, Some(0.62));
        // Triple-existence confidence is unchanged.
        assert!((stored.confidence - 0.85).abs() < 1e-9);
    }

    #[test]
    fn predicate_confidence_defaults_to_none() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        // Legacy `Triple::new` path — no predicate confidence set.
        let triple = Triple::new(alice.id, Predicate::WorksAt, bob.id, 0.85);
        store.upsert_triple(&triple).unwrap();

        let edges = store.get_triples_for_entity(alice.id).unwrap();
        let stored = edges.iter().find(|t| t.id == triple.id).expect("triple");
        assert_eq!(stored.predicate_confidence, None);
    }

    #[test]
    fn accept_pending_preserves_custom_predicate_string() {
        let store = GraphStore::open(":memory:").expect("open db");
        let alice = make_entity("Alice", EntityType::Person);
        let bob = make_entity("Bob", EntityType::Person);
        store.upsert_entity(&alice).unwrap();
        store.upsert_entity(&bob).unwrap();

        // Insert directly with a Custom predicate string that wouldn't
        // round-trip through Triple → string → Predicate enum cleanly.
        let row = crate::pending_relations::PendingRelation::new(
            alice.id,
            "mentored",
            bob.id,
            0.55,
            Some("test-cap".to_string()),
        );
        let pending_id = row.id;
        store.insert_pending(&row).unwrap();

        let promoted = store.accept_pending(pending_id, "ok").unwrap();
        // Unknown name falls through to Custom — original text preserved.
        assert_eq!(promoted.predicate, Predicate::Custom("mentored".to_string()));
    }

    // ── Two-stage ANN signal search (review §5 P2.6) ────────────────────

    fn onehot(dim: usize, i: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[i % dim] = 1.0;
        v
    }

    #[test]
    fn signal_search_finds_the_exact_match() {
        let store = GraphStore::open(":memory:").unwrap();
        let dim = 16;
        for i in 0..50 {
            store
                .insert_signal_with_embedding(
                    "test",
                    &format!("signal {i}"),
                    i as u64,
                    Uuid::new_v4(),
                    &onehot(dim, i),
                    None,
                    3,
                )
                .unwrap();
        }
        // Query identical to signal 7's embedding.
        let hits = store.search_signals(&onehot(dim, 7), 3, 0.5).unwrap();
        assert!(!hits.is_empty(), "search returned nothing");
        assert_eq!(hits[0].0.raw_text, "signal 7", "top hit should be the exact match");
    }

    /// The whole point of the rewrite: a memory inserted *after* thousands of
    /// others must still be findable. The old `ORDER BY id ASC LIMIT 2000`
    /// dropped exactly these. The recency window guarantees it here even at
    /// scale, and the SimHash prefilter finds it by similarity regardless.
    #[test]
    fn newest_signal_is_never_silently_dropped() {
        let store = GraphStore::open(":memory:").unwrap();
        let dim = 32;
        // Fill with many unrelated signals.
        for i in 0..2500 {
            store
                .insert_signal_with_embedding(
                    "bulk",
                    &format!("bulk {i}"),
                    i as u64,
                    Uuid::new_v4(),
                    &onehot(dim, i),
                    None,
                    3,
                )
                .unwrap();
        }
        // Insert a distinctive newest signal.
        let needle = {
            let mut v = vec![0.0f32; dim];
            v[3] = 1.0;
            v[7] = 1.0; // distinctive pattern
            v
        };
        store
            .insert_signal_with_embedding("fresh", "the needle", 99999, Uuid::new_v4(), &needle, None, 3)
            .unwrap();

        let hits = store.search_signals(&needle, 5, 0.3).unwrap();
        assert!(
            hits.iter().any(|(s, _)| s.raw_text == "the needle"),
            "the newest signal was dropped — the silent-cliff regression is back"
        );
    }

    /// The two-stage result should agree with an exact brute-force scan on
    /// the top hit (the prefilter + rerank must not degrade the winner).
    #[test]
    fn two_stage_agrees_with_brute_force_on_top_hit() {
        let store = GraphStore::open(":memory:").unwrap();
        // dim >= n so every one-hot index is unique (no cosine ties that
        // would make "the" top hit ambiguous).
        let dim = 128;
        let n = 120;
        let mut embeddings = Vec::new();
        for i in 0..n {
            // Slightly perturbed one-hots so cosines differ but stay distinct.
            let mut v = onehot(dim, i);
            v[(i + 1) % dim] += 0.3;
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in v.iter_mut() {
                *x /= norm;
            }
            store
                .insert_signal_with_embedding("t", &format!("s{i}"), i as u64, Uuid::new_v4(), &v, None, 3)
                .unwrap();
            embeddings.push((format!("s{i}"), v));
        }
        let query = &embeddings[42].1;

        // Brute-force top hit.
        let brute = embeddings
            .iter()
            .map(|(name, e)| (name, cosine_sim_slice(query, e)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
            .0
            .clone();

        let hits = store.search_signals(query, 1, 0.0).unwrap();
        assert_eq!(hits[0].0.raw_text, brute, "two-stage disagreed with brute force");
    }
}
