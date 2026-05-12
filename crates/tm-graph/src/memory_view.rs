//! LM-11a — Memory Views (user-controlled splicing).
//!
//! A **Memory View** is a named, saved splice of memory: an include-list and
//! an exclude-list of memory IDs that the user curates. Views give the user
//! surgical control over which memories enter a session — "focus on links
//! 1, 2, 3, not 4" — without touching the underlying graph. They are the
//! primary user-power feature of PROJECT_2026 §1c (Legibility pillar),
//! sibling to the auto-corrective deny-list and the Q4 ontological pass.
//!
//! ## Data model
//!
//! ```text
//! memory_views(id, name, description, created_at, updated_at)
//!   ─── memory_view_members(view_id, member_kind, member_type, member_id,
//!                           added_at)
//! ```
//!
//! `member_kind` ∈ {`include`, `exclude`}. `member_type` ∈ {`entity`,
//! `triple`, `context`} — kept as TEXT so we can add `cluster`,
//! `community` later without a migration.
//!
//! ## SLM-ready slots (no-op until LM-7/LM-8/LM-15 ship)
//!
//! The schema also carries:
//! - `confidence_floor REAL` — drop rows whose backing triple confidence
//!   is below this. Lets a view say "high-trust only" without depending
//!   on the SLM extractor being implemented yet.
//! - `include_pending INTEGER` — include rows from the
//!   `pending_relations` pool (mid-confidence SLM output). Default off.
//!
//! These slots exist so the schema is forward-compatible with the SML
//! extractor (LM-8) and ontological typing (LM-15) without a migration.
//! See PROJECT_2026 §1c.
//!
//! ## Query-time semantics
//!
//! A view evaluates as:
//!
//! ```text
//! result_set = retrieved
//!   ∪  include.entity_ids                  ── always-bring-along
//!   ∩  (include.context_ids   ∪ unscoped)  ── if non-empty
//!   ∖  exclude.entity_ids                  ── never-show
//!   ∖  exclude.context_ids' entities       ── never-show
//!   ∖  deny_list                           ── auto-correct floor
//!   ∧  confidence ≥ confidence_floor       ── high-trust gate
//! ```
//!
//! See `RetrievalFilter::apply` in `tm-retrieval` for the live consumer.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// On-disk schema version for the `memory_views` tables. Bump on
/// non-additive changes; reads gracefully tolerate older versions.
pub const VIEW_SCHEMA_VERSION: u32 = 1;

/// A user-curated splice of memory. Names are unique within a store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryView {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// SLM-ready: floor on triple confidence (0.0 = off).
    #[serde(default)]
    pub confidence_floor: f32,
    /// SLM-ready: include `pending_relations` (LM-9 mid-confidence pool).
    #[serde(default)]
    pub include_pending: bool,
}

impl MemoryView {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: description.into(),
            created_at: now,
            updated_at: now,
            confidence_floor: 0.0,
            include_pending: false,
        }
    }
}

/// What kind of row a [`ViewMember`] points at. Stored as TEXT in the
/// DB so future types (`cluster`, `community`, `tag`) can slot in
/// without a migration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemberType {
    Entity,
    Triple,
    Context,
}

impl MemberType {
    pub fn as_str(self) -> &'static str {
        match self {
            MemberType::Entity => "entity",
            MemberType::Triple => "triple",
            MemberType::Context => "context",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "entity" => Ok(MemberType::Entity),
            "triple" => Ok(MemberType::Triple),
            "context" => Ok(MemberType::Context),
            other => Err(TraceMindError::Storage(format!(
                "unknown view member_type '{other}'"
            ))),
        }
    }
}

/// Whether a member adds or removes from the view's set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    Include,
    Exclude,
}

impl MemberKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MemberKind::Include => "include",
            MemberKind::Exclude => "exclude",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "include" => Ok(MemberKind::Include),
            "exclude" => Ok(MemberKind::Exclude),
            other => Err(TraceMindError::Storage(format!(
                "unknown view member_kind '{other}'"
            ))),
        }
    }
}

/// A single row in `memory_view_members`. The `(view_id, member_kind,
/// member_type, member_id)` tuple is unique — re-adding the same row is
/// idempotent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewMember {
    pub view_id: Uuid,
    pub kind: MemberKind,
    pub member_type: MemberType,
    pub member_id: Uuid,
    pub added_at: DateTime<Utc>,
}

/// In-memory snapshot of a view's predicates, ready to apply at query
/// time. Built once by [`load_filter`] and consumed by retrieval.
///
/// Use the helper accessors (`is_excluded`, `is_included`, ...) instead
/// of poking at the sets directly — that way the semantics evolve in
/// one place.
#[derive(Debug, Clone, Default)]
pub struct ViewFilter {
    pub view_id: Option<Uuid>,
    pub include_entities: HashSet<Uuid>,
    pub include_triples: HashSet<Uuid>,
    pub include_contexts: HashSet<Uuid>,
    pub exclude_entities: HashSet<Uuid>,
    pub exclude_triples: HashSet<Uuid>,
    pub exclude_contexts: HashSet<Uuid>,
    pub confidence_floor: f32,
    pub include_pending: bool,
    /// Ad-hoc additions from CLI flags (`--include-entity`,
    /// `--exclude-entity`) that aren't persisted to the view.
    pub adhoc_include_entities: HashSet<Uuid>,
    pub adhoc_exclude_entities: HashSet<Uuid>,
}

impl ViewFilter {
    /// True if no predicate is active — the caller should fast-path
    /// past view evaluation entirely.
    pub fn is_empty(&self) -> bool {
        self.include_entities.is_empty()
            && self.include_triples.is_empty()
            && self.include_contexts.is_empty()
            && self.exclude_entities.is_empty()
            && self.exclude_triples.is_empty()
            && self.exclude_contexts.is_empty()
            && self.adhoc_include_entities.is_empty()
            && self.adhoc_exclude_entities.is_empty()
            && self.confidence_floor <= 0.0
            && !self.include_pending
    }

    /// Should this entity be filtered out by the view?
    ///
    /// Returns `true` (drop) when:
    /// - it appears in any exclude list (persisted or ad-hoc), OR
    /// - an include-entity list is set and the entity isn't in it AND
    ///   isn't reachable via an include-context.
    ///
    /// `entity_context` is the entity's context_id if known. When the
    /// view's include-contexts is empty, context membership is ignored.
    pub fn rejects_entity(&self, entity_id: Uuid, entity_context: Option<Uuid>) -> bool {
        if self.exclude_entities.contains(&entity_id)
            || self.adhoc_exclude_entities.contains(&entity_id)
        {
            return true;
        }
        if let Some(ctx) = entity_context {
            if self.exclude_contexts.contains(&ctx) {
                return true;
            }
        }
        let has_include = !self.include_entities.is_empty()
            || !self.include_contexts.is_empty()
            || !self.adhoc_include_entities.is_empty();
        if !has_include {
            return false;
        }
        if self.include_entities.contains(&entity_id)
            || self.adhoc_include_entities.contains(&entity_id)
        {
            return false;
        }
        if let Some(ctx) = entity_context {
            if self.include_contexts.contains(&ctx) {
                return false;
            }
        }
        true
    }

    /// Should this triple be filtered out? Mirrors `rejects_entity`
    /// but at the triple level.
    pub fn rejects_triple(&self, triple_id: Uuid, triple_confidence: f64) -> bool {
        if self.exclude_triples.contains(&triple_id) {
            return true;
        }
        if triple_confidence < self.confidence_floor as f64 {
            return true;
        }
        let has_include = !self.include_triples.is_empty();
        if !has_include {
            return false;
        }
        !self.include_triples.contains(&triple_id)
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Initialize the Memory Views tables. Idempotent.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_views (
            id                TEXT PRIMARY KEY,
            name              TEXT NOT NULL UNIQUE,
            description       TEXT NOT NULL DEFAULT '',
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
            confidence_floor  REAL NOT NULL DEFAULT 0.0,
            include_pending   INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_memory_views_name ON memory_views(name);

        CREATE TABLE IF NOT EXISTS memory_view_members (
            view_id      TEXT NOT NULL,
            kind         TEXT NOT NULL CHECK(kind IN ('include','exclude')),
            member_type  TEXT NOT NULL,
            member_id    TEXT NOT NULL,
            added_at     TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (view_id, kind, member_type, member_id),
            FOREIGN KEY (view_id) REFERENCES memory_views(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_view_members_view
          ON memory_view_members(view_id, kind);",
    )
    .map_err(|e| TraceMindError::Storage(format!("init memory_views schema: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

/// Create a new view. Returns an error if the name is taken.
pub fn create_view(conn: &Connection, view: &MemoryView) -> Result<()> {
    conn.execute(
        "INSERT INTO memory_views
            (id, name, description, created_at, updated_at, confidence_floor, include_pending)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            view.id.to_string(),
            view.name,
            view.description,
            view.created_at.to_rfc3339(),
            view.updated_at.to_rfc3339(),
            view.confidence_floor as f64,
            view.include_pending as i64,
        ],
    )
    .map_err(|e| {
        // Detect uniqueness violation and return a friendlier error.
        let msg = e.to_string();
        if msg.contains("UNIQUE constraint failed") {
            TraceMindError::Storage(format!("view '{}' already exists", view.name))
        } else {
            TraceMindError::Storage(format!("create_view: {msg}"))
        }
    })?;
    Ok(())
}

/// List every view, newest-updated first.
pub fn list_views(conn: &Connection) -> Result<Vec<MemoryView>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, created_at, updated_at,
                    confidence_floor, include_pending
             FROM memory_views
             ORDER BY updated_at DESC",
        )
        .map_err(|e| TraceMindError::Storage(format!("list_views prepare: {e}")))?;
    let rows = stmt
        .query_map([], row_to_view)
        .map_err(|e| TraceMindError::Storage(format!("list_views query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| TraceMindError::Storage(e.to_string()))?);
    }
    Ok(out)
}

/// Look up a view by name. `Ok(None)` if it doesn't exist.
pub fn get_view_by_name(conn: &Connection, name: &str) -> Result<Option<MemoryView>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, created_at, updated_at,
                    confidence_floor, include_pending
             FROM memory_views WHERE name = ?1",
        )
        .map_err(|e| TraceMindError::Storage(format!("get_view prepare: {e}")))?;
    let mut rows = stmt
        .query_map(params![name], row_to_view)
        .map_err(|e| TraceMindError::Storage(format!("get_view query: {e}")))?;
    if let Some(first) = rows.next() {
        Ok(Some(first.map_err(|e| TraceMindError::Storage(e.to_string()))?))
    } else {
        Ok(None)
    }
}

/// Look up a view by UUID. `Ok(None)` if it doesn't exist.
pub fn get_view(conn: &Connection, id: Uuid) -> Result<Option<MemoryView>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, created_at, updated_at,
                    confidence_floor, include_pending
             FROM memory_views WHERE id = ?1",
        )
        .map_err(|e| TraceMindError::Storage(format!("get_view prepare: {e}")))?;
    let mut rows = stmt
        .query_map(params![id.to_string()], row_to_view)
        .map_err(|e| TraceMindError::Storage(format!("get_view query: {e}")))?;
    if let Some(first) = rows.next() {
        Ok(Some(first.map_err(|e| TraceMindError::Storage(e.to_string()))?))
    } else {
        Ok(None)
    }
}

/// Update a view's metadata (description, confidence floor, pending
/// flag). Bumps `updated_at`. Does not touch members — use the member
/// helpers for that.
pub fn update_view_metadata(
    conn: &Connection,
    view_id: Uuid,
    description: Option<&str>,
    confidence_floor: Option<f32>,
    include_pending: Option<bool>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    if let Some(d) = description {
        conn.execute(
            "UPDATE memory_views SET description = ?1, updated_at = ?2 WHERE id = ?3",
            params![d, now, view_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("update view description: {e}")))?;
    }
    if let Some(c) = confidence_floor {
        conn.execute(
            "UPDATE memory_views SET confidence_floor = ?1, updated_at = ?2 WHERE id = ?3",
            params![c as f64, now, view_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("update view conf_floor: {e}")))?;
    }
    if let Some(p) = include_pending {
        conn.execute(
            "UPDATE memory_views SET include_pending = ?1, updated_at = ?2 WHERE id = ?3",
            params![p as i64, now, view_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("update view inc_pending: {e}")))?;
    }
    Ok(())
}

/// Delete a view and all its members.
pub fn delete_view(conn: &Connection, view_id: Uuid) -> Result<bool> {
    let n = conn
        .execute(
            "DELETE FROM memory_views WHERE id = ?1",
            params![view_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("delete_view: {e}")))?;
    Ok(n > 0)
}

/// Add a member to a view. Idempotent: re-adding the same tuple is a
/// no-op. Bumps the parent view's `updated_at`.
pub fn add_member(
    conn: &Connection,
    view_id: Uuid,
    kind: MemberKind,
    member_type: MemberType,
    member_id: Uuid,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO memory_view_members
            (view_id, kind, member_type, member_id, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(view_id, kind, member_type, member_id) DO NOTHING",
        params![
            view_id.to_string(),
            kind.as_str(),
            member_type.as_str(),
            member_id.to_string(),
            now,
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("add_member: {e}")))?;
    conn.execute(
        "UPDATE memory_views SET updated_at = ?1 WHERE id = ?2",
        params![now, view_id.to_string()],
    )
    .map_err(|e| TraceMindError::Storage(format!("bump updated_at: {e}")))?;
    Ok(())
}

/// Remove a member. Returns whether a row was deleted.
pub fn remove_member(
    conn: &Connection,
    view_id: Uuid,
    kind: MemberKind,
    member_type: MemberType,
    member_id: Uuid,
) -> Result<bool> {
    let n = conn
        .execute(
            "DELETE FROM memory_view_members
              WHERE view_id = ?1 AND kind = ?2 AND member_type = ?3 AND member_id = ?4",
            params![
                view_id.to_string(),
                kind.as_str(),
                member_type.as_str(),
                member_id.to_string(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("remove_member: {e}")))?;
    if n > 0 {
        conn.execute(
            "UPDATE memory_views SET updated_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), view_id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("bump updated_at: {e}")))?;
    }
    Ok(n > 0)
}

/// List every member of a view.
pub fn list_members(conn: &Connection, view_id: Uuid) -> Result<Vec<ViewMember>> {
    let mut stmt = conn
        .prepare(
            "SELECT view_id, kind, member_type, member_id, added_at
             FROM memory_view_members
             WHERE view_id = ?1
             ORDER BY kind, member_type, added_at",
        )
        .map_err(|e| TraceMindError::Storage(format!("list_members prepare: {e}")))?;
    let rows = stmt
        .query_map(params![view_id.to_string()], |r| {
            let view_id: String = r.get(0)?;
            let kind: String = r.get(1)?;
            let member_type: String = r.get(2)?;
            let member_id: String = r.get(3)?;
            let added_at: String = r.get(4)?;
            Ok((view_id, kind, member_type, member_id, added_at))
        })
        .map_err(|e| TraceMindError::Storage(format!("list_members query: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        let (vid, k, mt, mid, at) = r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
        out.push(ViewMember {
            view_id: Uuid::parse_str(&vid)
                .map_err(|e| TraceMindError::Storage(format!("bad view uuid: {e}")))?,
            kind: MemberKind::parse(&k)?,
            member_type: MemberType::parse(&mt)?,
            member_id: Uuid::parse_str(&mid)
                .map_err(|e| TraceMindError::Storage(format!("bad member uuid: {e}")))?,
            added_at: DateTime::parse_from_rfc3339(&at)
                .map_err(|e| TraceMindError::Storage(format!("bad added_at: {e}")))?
                .with_timezone(&Utc),
        });
    }
    Ok(out)
}

/// Build the in-memory [`ViewFilter`] for a view. This is the hot path
/// called once per query that runs with `--view <name>`.
pub fn load_filter(conn: &Connection, view: &MemoryView) -> Result<ViewFilter> {
    let mut f = ViewFilter {
        view_id: Some(view.id),
        confidence_floor: view.confidence_floor,
        include_pending: view.include_pending,
        ..Default::default()
    };
    for m in list_members(conn, view.id)? {
        let set: &mut HashSet<Uuid> = match (m.kind, m.member_type) {
            (MemberKind::Include, MemberType::Entity) => &mut f.include_entities,
            (MemberKind::Include, MemberType::Triple) => &mut f.include_triples,
            (MemberKind::Include, MemberType::Context) => &mut f.include_contexts,
            (MemberKind::Exclude, MemberType::Entity) => &mut f.exclude_entities,
            (MemberKind::Exclude, MemberType::Triple) => &mut f.exclude_triples,
            (MemberKind::Exclude, MemberType::Context) => &mut f.exclude_contexts,
        };
        set.insert(m.member_id);
    }
    Ok(f)
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

fn row_to_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryView> {
    let id: String = r.get(0)?;
    let name: String = r.get(1)?;
    let description: String = r.get(2)?;
    let created_at: String = r.get(3)?;
    let updated_at: String = r.get(4)?;
    let confidence_floor: f64 = r.get(5)?;
    let include_pending: i64 = r.get(6)?;
    Ok(MemoryView {
        id: Uuid::parse_str(&id).unwrap_or_default(),
        name,
        description,
        created_at: DateTime::parse_from_rfc3339(&created_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        confidence_floor: confidence_floor as f32,
        include_pending: include_pending != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn schema_is_idempotent() {
        let conn = mem_conn();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap();
    }

    #[test]
    fn create_list_get_delete_view() {
        let conn = mem_conn();
        let v = MemoryView::new("rondo-only", "Just Rondo memories");
        create_view(&conn, &v).unwrap();

        let listed = list_views(&conn).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "rondo-only");

        let by_name = get_view_by_name(&conn, "rondo-only").unwrap().unwrap();
        assert_eq!(by_name.id, v.id);

        let by_id = get_view(&conn, v.id).unwrap().unwrap();
        assert_eq!(by_id.name, "rondo-only");

        assert!(delete_view(&conn, v.id).unwrap());
        assert!(list_views(&conn).unwrap().is_empty());
        // second delete = noop
        assert!(!delete_view(&conn, v.id).unwrap());
    }

    #[test]
    fn duplicate_name_is_rejected_with_friendly_error() {
        let conn = mem_conn();
        create_view(&conn, &MemoryView::new("dup", "")).unwrap();
        let err = create_view(&conn, &MemoryView::new("dup", "")).unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");
    }

    #[test]
    fn add_remove_members_idempotent() {
        let conn = mem_conn();
        let v = MemoryView::new("x", "");
        create_view(&conn, &v).unwrap();

        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        add_member(&conn, v.id, MemberKind::Include, MemberType::Entity, e1).unwrap();
        add_member(&conn, v.id, MemberKind::Include, MemberType::Entity, e1).unwrap(); // dup ok
        add_member(&conn, v.id, MemberKind::Exclude, MemberType::Entity, e2).unwrap();

        let m = list_members(&conn, v.id).unwrap();
        assert_eq!(m.len(), 2);

        assert!(remove_member(&conn, v.id, MemberKind::Include, MemberType::Entity, e1).unwrap());
        assert!(!remove_member(&conn, v.id, MemberKind::Include, MemberType::Entity, e1).unwrap());
        let m = list_members(&conn, v.id).unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn load_filter_populates_sets() {
        let conn = mem_conn();
        let v = MemoryView::new("y", "");
        create_view(&conn, &v).unwrap();
        let e_inc = Uuid::new_v4();
        let t_exc = Uuid::new_v4();
        let c_inc = Uuid::new_v4();
        add_member(&conn, v.id, MemberKind::Include, MemberType::Entity, e_inc).unwrap();
        add_member(&conn, v.id, MemberKind::Exclude, MemberType::Triple, t_exc).unwrap();
        add_member(&conn, v.id, MemberKind::Include, MemberType::Context, c_inc).unwrap();

        let f = load_filter(&conn, &v).unwrap();
        assert!(f.include_entities.contains(&e_inc));
        assert!(f.exclude_triples.contains(&t_exc));
        assert!(f.include_contexts.contains(&c_inc));
        assert!(!f.is_empty());
    }

    #[test]
    fn filter_rejects_entity_logic() {
        let mut f = ViewFilter::default();
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        let c_in = Uuid::new_v4();
        let c_out = Uuid::new_v4();

        // No predicates → nothing is rejected.
        assert!(!f.rejects_entity(e1, None));

        // Exclude wins.
        f.exclude_entities.insert(e1);
        assert!(f.rejects_entity(e1, None));

        // With include set, only included entities pass.
        let mut g = ViewFilter::default();
        g.include_entities.insert(e1);
        assert!(!g.rejects_entity(e1, None));
        assert!(g.rejects_entity(e2, None));

        // Include-context rescues an entity in that context.
        let mut h = ViewFilter::default();
        h.include_contexts.insert(c_in);
        assert!(!h.rejects_entity(e1, Some(c_in)));
        assert!(h.rejects_entity(e1, Some(c_out)));
        assert!(h.rejects_entity(e1, None)); // no context info → rejected

        // Exclude-context wins over include-entity.
        let mut i = ViewFilter::default();
        i.include_entities.insert(e1);
        i.exclude_contexts.insert(c_out);
        assert!(i.rejects_entity(e1, Some(c_out)));
    }

    #[test]
    fn filter_rejects_triple_logic() {
        let mut f = ViewFilter::default();
        let t = Uuid::new_v4();

        assert!(!f.rejects_triple(t, 0.9));

        f.exclude_triples.insert(t);
        assert!(f.rejects_triple(t, 0.9));

        // Confidence floor.
        let mut g = ViewFilter::default();
        g.confidence_floor = 0.5;
        assert!(g.rejects_triple(Uuid::new_v4(), 0.4));
        assert!(!g.rejects_triple(Uuid::new_v4(), 0.6));
    }

    #[test]
    fn adhoc_include_exclude_work() {
        let mut f = ViewFilter::default();
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        f.adhoc_include_entities.insert(e1);
        f.adhoc_exclude_entities.insert(e2);
        // include-set is active, e1 is the only allowed entity
        assert!(!f.rejects_entity(e1, None));
        assert!(f.rejects_entity(e2, None));
        // unrelated entity also rejected
        assert!(f.rejects_entity(Uuid::new_v4(), None));
    }

    #[test]
    fn update_metadata_bumps_updated_at() {
        let conn = mem_conn();
        let v = MemoryView::new("z", "");
        create_view(&conn, &v).unwrap();
        let before = get_view(&conn, v.id).unwrap().unwrap().updated_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        update_view_metadata(&conn, v.id, Some("new desc"), Some(0.6), Some(true)).unwrap();
        let after = get_view(&conn, v.id).unwrap().unwrap();
        assert_eq!(after.description, "new desc");
        assert_eq!(after.confidence_floor, 0.6);
        assert!(after.include_pending);
        assert!(after.updated_at >= before);
    }
}
