//! Sprint GRAPH — shared schema migrations for the unified graph sprint.
//!
//! This module owns the **schema only** for every new table introduced in
//! Sprint GRAPH:
//!
//! - `threads`                                — first-class AI-conversation primitive
//! - `event_nodes` / `event_edges`            — Glean-style event/trajectory graph
//! - `bridge_edges`                           — sparse cross-context bridge table
//! - `ontology_object_types`                  — Palantir-style Object Types
//! - `ontology_link_types`                    — Palantir-style Link Types
//! - `memory_views.expression`                — graph-algebra expression column
//!
//! The migration is **idempotent and additive** — it runs at every
//! `GraphStore::open()` call. No data is rewritten; old views without an
//! `expression` column gain one with an empty default. Reads always tolerate
//! both shapes.
//!
//! See `docs/SPRINT_GRAPH.md` for the full design.

use chrono::Utc;
use rusqlite::Connection;
use tm_types::{Result, TraceMindError};

/// Bumped each time the migration touches a new table or column. The store
/// stores this in a `schema_meta` row so a future migrator can detect skew.
pub const GRAPH_SPRINT_SCHEMA_VERSION: u32 = 1;

/// Run every Sprint GRAPH migration. Safe to call repeatedly.
pub fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(MIGRATION_SQL).map_err(|e| {
        TraceMindError::Storage(format!("sprint-graph migration: {e}"))
    })?;

    // Add `expression` column to memory_views if missing (LM-11a was shipped
    // without it). SQLite's ALTER TABLE rejects existing-column adds, so we
    // probe via PRAGMA first.
    if !column_exists(conn, "memory_views", "expression")? {
        conn.execute(
            "ALTER TABLE memory_views ADD COLUMN expression TEXT NOT NULL DEFAULT ''",
            [],
        )
        .map_err(|e| {
            TraceMindError::Storage(format!("alter memory_views.expression: {e}"))
        })?;
    }

    // Sprint EVG-Followup: LGM random variables now reference an Object
    // Type from the ontology (optional — Custom variables can stay free-form,
    // but Activity/Topic/etc. nail down to a typed Object Type).
    if !column_exists(conn, "lgm_variables", "object_type")? {
        conn.execute(
            "ALTER TABLE lgm_variables ADD COLUMN object_type TEXT",
            [],
        )
        .map_err(|e| {
            TraceMindError::Storage(format!("alter lgm_variables.object_type: {e}"))
        })?;
    }

    seed_builtin_ontology(conn)?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| TraceMindError::Storage(format!("pragma: {e}")))?;
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            Ok(name)
        })
        .map_err(|e| TraceMindError::Storage(format!("pragma query: {e}")))?;
    for r in rows {
        if r.map_err(|e| TraceMindError::Storage(e.to_string()))? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Seed the 12 default Object Types and ~30 default Link Types if the
/// ontology is empty. The user's evolved ontology takes precedence; this
/// only fires on a fresh store.
fn seed_builtin_ontology(conn: &Connection) -> Result<()> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ontology_object_types", [], |r| r.get(0))
        .map_err(|e| TraceMindError::Storage(format!("count ot: {e}")))?;
    if count > 0 {
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    let object_types: &[(&str, Option<&str>)] = &[
        ("Person", None),
        ("Organization", None),
        ("Project", None),
        ("Concept", None),
        ("Technology", None),
        ("Decision", None),
        ("Event", None),
        ("Location", None),
        ("Artifact", None),
        ("Thread", None),
        ("Topic", None),
        ("Commitment", None),
    ];

    for (name, parent) in object_types {
        conn.execute(
            "INSERT INTO ontology_object_types
               (id, name, version, parent_id, property_schema, source, created_at, updated_at)
             VALUES (lower(hex(randomblob(16))), ?, 1, ?, '{}', 'builtin', ?, ?)",
            rusqlite::params![name, parent, now, now],
        )
        .map_err(|e| TraceMindError::Storage(format!("insert ot {name}: {e}")))?;
    }

    let link_types: &[(&str, &str, &str)] = &[
        // (name, from_object_type, to_object_type)
        ("WorksAt", "Person", "Organization"),
        ("CollaboratesWith", "Person", "Person"),
        ("Mentions", "Thread", "Topic"),
        ("MentionsEntity", "Thread", "Person"),
        ("Discusses", "Thread", "Project"),
        ("PartOfProject", "Concept", "Project"),
        ("PartOfOrganization", "Person", "Organization"),
        ("OwnsProject", "Person", "Project"),
        ("FoundedBy", "Organization", "Person"),
        ("CommittedBy", "Commitment", "Person"),
        ("ResolvesCommitment", "Decision", "Commitment"),
        ("AboutTopic", "Commitment", "Topic"),
        ("UsesTech", "Project", "Technology"),
        ("UsesTech_Person", "Person", "Technology"),
        ("DependsOn", "Project", "Project"),
        ("Produces", "Person", "Artifact"),
        ("Authored", "Person", "Artifact"),
        ("LocatedAt", "Event", "Location"),
        ("ParticipatedIn", "Person", "Event"),
        ("RelatedTo", "Concept", "Concept"),
        ("References", "Artifact", "Concept"),
        ("AnalogousTo", "Concept", "Concept"),
        ("ContradictsWith", "Decision", "Decision"),
        ("HasProperty", "Person", "Concept"),
        ("HasProperty_Org", "Organization", "Concept"),
        ("OccurredAt", "Event", "Location"),
        ("LinksTo", "Artifact", "Artifact"),
        ("FollowsUp", "Commitment", "Commitment"),
        ("InContext", "Thread", "Project"),
        ("DerivedFrom", "Concept", "Artifact"),
    ];

    for (name, from_ot, to_ot) in link_types {
        conn.execute(
            "INSERT INTO ontology_link_types
               (id, name, version, from_object_type_id, to_object_type_id,
                cardinality, source, created_at, updated_at)
             VALUES (
               lower(hex(randomblob(16))), ?, 1,
               (SELECT id FROM ontology_object_types WHERE name = ?),
               (SELECT id FROM ontology_object_types WHERE name = ?),
               'many_to_many', 'builtin', ?, ?
             )",
            rusqlite::params![name, from_ot, to_ot, now, now],
        )
        .map_err(|e| TraceMindError::Storage(format!("insert lt {name}: {e}")))?;
    }

    Ok(())
}

const MIGRATION_SQL: &str = r#"
-- ─── threads ─────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS threads (
  id          TEXT PRIMARY KEY,
  context_id  TEXT,
  title       TEXT NOT NULL DEFAULT '',
  source      TEXT NOT NULL DEFAULT 'other',
  started_at  TEXT NOT NULL,
  ended_at    TEXT,
  metadata    TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_threads_context ON threads(context_id);
CREATE INDEX IF NOT EXISTS idx_threads_started ON threads(started_at);

-- ─── event_nodes + event_edges (Glean) ───────────────────────────────
CREATE TABLE IF NOT EXISTS event_nodes (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,
  ts          INTEGER NOT NULL,
  payload_ref TEXT NOT NULL,
  cluster_id  INTEGER,
  context_id  TEXT,
  thread_id   TEXT,
  salience    REAL NOT NULL DEFAULT 0.0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_event_nodes_ts ON event_nodes(ts);
CREATE INDEX IF NOT EXISTS idx_event_nodes_thread ON event_nodes(thread_id);
CREATE INDEX IF NOT EXISTS idx_event_nodes_context ON event_nodes(context_id);
CREATE INDEX IF NOT EXISTS idx_event_nodes_kind ON event_nodes(kind);

CREATE TABLE IF NOT EXISTS event_edges (
  id            TEXT PRIMARY KEY,
  from_id       TEXT NOT NULL,
  to_id         TEXT NOT NULL,
  kind          TEXT NOT NULL,
  strength      REAL NOT NULL DEFAULT 0.0,
  support_count INTEGER NOT NULL DEFAULT 1,
  context_id    TEXT,
  first_seen    TEXT NOT NULL,
  last_seen     TEXT NOT NULL,
  UNIQUE (from_id, to_id, kind, context_id)
);
CREATE INDEX IF NOT EXISTS idx_event_edges_from ON event_edges(from_id);
CREATE INDEX IF NOT EXISTS idx_event_edges_kind ON event_edges(kind, context_id);

-- ─── cross-context bridge edges (sparse, gated) ──────────────────────
CREATE TABLE IF NOT EXISTS bridge_edges (
  id            TEXT PRIMARY KEY,
  context_a     TEXT NOT NULL,
  context_b     TEXT NOT NULL,
  object_type_a TEXT,
  object_type_b TEXT,
  evidence_kind TEXT NOT NULL,
  support_count INTEGER NOT NULL DEFAULT 1,
  status        TEXT NOT NULL DEFAULT 'proposed',
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL,
  UNIQUE (context_a, context_b, object_type_a, object_type_b)
);
CREATE INDEX IF NOT EXISTS idx_bridge_status ON bridge_edges(status);

-- ─── Palantir-style Ontology ─────────────────────────────────────────
CREATE TABLE IF NOT EXISTS ontology_object_types (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  version         INTEGER NOT NULL DEFAULT 1,
  parent_id       TEXT,
  property_schema TEXT NOT NULL DEFAULT '{}',
  source          TEXT NOT NULL DEFAULT 'builtin',
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS ontology_link_types (
  id                  TEXT PRIMARY KEY,
  name                TEXT NOT NULL,
  version             INTEGER NOT NULL DEFAULT 1,
  from_object_type_id TEXT NOT NULL,
  to_object_type_id   TEXT NOT NULL,
  cardinality         TEXT NOT NULL DEFAULT 'many_to_many',
  source              TEXT NOT NULL DEFAULT 'builtin',
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL,
  UNIQUE (name, from_object_type_id, to_object_type_id)
);
CREATE INDEX IF NOT EXISTS idx_lt_from ON ontology_link_types(from_object_type_id);
CREATE INDEX IF NOT EXISTS idx_lt_to ON ontology_link_types(to_object_type_id);

-- Optional per-entity ontology binding (no-op until callers set it).
CREATE TABLE IF NOT EXISTS entity_object_type (
  entity_id      TEXT PRIMARY KEY,
  object_type_id TEXT NOT NULL,
  assigned_at    TEXT NOT NULL
);

-- ─── LGM (lives in tm-pgm but the tables are co-located in memory.db) ─
CREATE TABLE IF NOT EXISTS lgm_variables (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  kind        TEXT NOT NULL,
  domain      TEXT NOT NULL DEFAULT '{}',
  observed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS lgm_dependencies (
  child_id      TEXT NOT NULL,
  parent_id     TEXT NOT NULL,
  cpd           TEXT NOT NULL,
  log_score     REAL NOT NULL DEFAULT 0.0,
  support_count INTEGER NOT NULL DEFAULT 1,
  updated_at    TEXT NOT NULL,
  PRIMARY KEY (child_id, parent_id)
);
CREATE INDEX IF NOT EXISTS idx_lgm_dep_child ON lgm_dependencies(child_id);

-- ─── thread → memory-view default attachment (MCP attach_view) ───────
CREATE TABLE IF NOT EXISTS thread_attached_view (
  thread_id  TEXT PRIMARY KEY,
  view_id    TEXT NOT NULL,
  attached_at TEXT NOT NULL
);

-- ─── ONT-2 — Statistical ontology proposals (accept / reject) ────────
-- `tm-reflect` writes rows here when a recurring cluster looks like a new
-- Object Type. The user accepts or rejects in Settings; accepted rows
-- become real `ontology_object_types` entries with source='statistical'.
CREATE TABLE IF NOT EXISTS ontology_proposals (
  id            TEXT PRIMARY KEY,
  proposal_kind TEXT NOT NULL,           -- 'object_type' | 'link_type'
  name          TEXT NOT NULL,
  evidence      TEXT NOT NULL DEFAULT '{}',  -- JSON: cluster_id, sample event_ids, top terms
  support_count INTEGER NOT NULL DEFAULT 0,
  status        TEXT NOT NULL DEFAULT 'pending',  -- pending | accepted | rejected
  created_at    TEXT NOT NULL,
  decided_at    TEXT
);
CREATE INDEX IF NOT EXISTS idx_ont_prop_status ON ontology_proposals(status);
CREATE INDEX IF NOT EXISTS idx_ont_prop_kind ON ontology_proposals(proposal_kind);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn open() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn migration_is_idempotent() {
        let conn = open();
        // Pre-create memory_views without `expression` to exercise the ALTER path.
        conn.execute_batch(
            "CREATE TABLE memory_views (
               id TEXT PRIMARY KEY, name TEXT, description TEXT,
               created_at TEXT, updated_at TEXT,
               confidence_floor REAL DEFAULT 0,
               include_pending INTEGER DEFAULT 0
             );",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        ensure_schema(&conn).unwrap(); // idempotent
        // expression column was added
        assert!(column_exists(&conn, "memory_views", "expression").unwrap());
    }

    #[test]
    fn builtin_ontology_seeds_once() {
        let conn = open();
        conn.execute_batch(
            "CREATE TABLE memory_views (
               id TEXT PRIMARY KEY, name TEXT, description TEXT,
               created_at TEXT, updated_at TEXT,
               confidence_floor REAL DEFAULT 0,
               include_pending INTEGER DEFAULT 0
             );",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();

        let ot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ontology_object_types", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ot_count, 12);

        let lt_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ontology_link_types", [], |r| r.get(0))
            .unwrap();
        assert!(lt_count >= 25);

        // Re-running does not double-seed.
        ensure_schema(&conn).unwrap();
        let ot_count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM ontology_object_types", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ot_count, ot_count2);
    }

    #[test]
    fn new_tables_exist_after_migration() {
        let conn = open();
        conn.execute_batch(
            "CREATE TABLE memory_views (
               id TEXT PRIMARY KEY, name TEXT
             );",
        )
        .unwrap();
        ensure_schema(&conn).unwrap();
        for table in [
            "threads",
            "event_nodes",
            "event_edges",
            "bridge_edges",
            "ontology_object_types",
            "ontology_link_types",
            "entity_object_type",
            "lgm_variables",
            "lgm_dependencies",
            "thread_attached_view",
        ] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} missing");
        }
    }
}
