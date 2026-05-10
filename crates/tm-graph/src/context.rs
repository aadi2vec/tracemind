//! Sprint C-0 — Context segmentation primitives.
//!
//! A [`Context`] is a named namespace within a TraceMind store: a project,
//! a venture, a personal sub-life. Every captured signal and every triple
//! inherits the *active* context_id at ingest time. By default retrieval
//! is scoped to the active context — cross-context bridging must be
//! opted into explicitly, and bad bridges are correctable by writing a
//! row to `negative_signals` (see [`NegativeSignal`]).
//!
//! The wedge-critical insight (2026-05-10 investor review): local
//! machines have MORE context blur than cloud — one laptop hosts every
//! venture and every personal thread. If TraceMind doesn't segment
//! context cleanly, users prefer separate cloud accounts. Decoupled-by-
//! default + negative-feedback-driven coupling is the right shape.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// A named namespace inside the memory store. New contexts are created
/// explicitly via the CLI / API; the user picks one as *active* and all
/// subsequent ingest tags rows with this `id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Context {
    pub id: Uuid,
    pub name: String,
    /// Comma-separated free-form tags. Used by the brief surface so the
    /// user can see at a glance what each context is *about* without
    /// opening a settings drawer.
    pub tags: String,
    pub created_at: DateTime<Utc>,
}

impl Context {
    pub fn new(name: impl Into<String>, tags: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            tags: tags.into(),
            created_at: Utc::now(),
        }
    }
}

/// A negative-feedback row written when the user marks a result as not
/// related to a query. The bandit reward is decomposed at training time:
///
/// ```text
/// final_reward = relevance_reward - Σ weight(negative_signals matching this query)
/// ```
///
/// `context_a` and `context_b` are filled when the negative signal
/// crosses a context boundary — they let the rerank learn a per-pair
/// penalty over time. For same-context negatives both are equal (or
/// `None` if the row is unscoped, e.g. legacy data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegativeSignal {
    pub id: i64,
    pub query_id: Uuid,
    pub result_id: String,
    pub kind: String,
    pub context_a: Option<Uuid>,
    pub context_b: Option<Uuid>,
    pub weight: f32,
    pub created_at: DateTime<Utc>,
}

/// What's currently active. Persisted to `~/.tracemind/active_context.json`
/// so every CLI invocation (and every MCP request) sees the same scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveContext {
    pub id: Uuid,
    pub name: String,
}

impl ActiveContext {
    /// Read the active context from disk. Returns `Ok(None)` if the file
    /// doesn't exist (= unscoped / pre-Sprint-C-0 behaviour).
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| TraceMindError::Storage(format!("read active_context: {e}")))?;
        let active: Self = serde_json::from_str(&raw)
            .map_err(|e| TraceMindError::Storage(format!("parse active_context: {e}")))?;
        Ok(Some(active))
    }

    /// Atomically write the active context. Best-effort tempfile +
    /// rename so a crash mid-write doesn't leave a half-flushed file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(format!("active_context mkdir: {e}")))?;
        }
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)
            .map_err(|e| TraceMindError::Storage(format!("write tmp: {e}")))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| TraceMindError::Storage(format!("rename: {e}")))?;
        Ok(())
    }

    /// Clear the active context (delete the file).
    pub fn clear(path: &Path) -> Result<()> {
        if path.exists() {
            std::fs::remove_file(path)
                .map_err(|e| TraceMindError::Storage(format!("clear active_context: {e}")))?;
        }
        Ok(())
    }
}

/// Schema bootstrap. Idempotent — safe to run on every open. Creates
/// the `contexts` and `negative_signals` tables plus the additive
/// `context_id` columns on `captured_signals`. The `kg_relations` and
/// `entities` rows (managed by skg) carry `context_id` inside their
/// JSON `properties` blob instead, so the skg schema stays untouched.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS contexts (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            tags        TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_contexts_name ON contexts(name);

        CREATE TABLE IF NOT EXISTS negative_signals (
            id          INTEGER PRIMARY KEY,
            query_id    TEXT NOT NULL,
            result_id   TEXT NOT NULL,
            kind        TEXT NOT NULL DEFAULT 'not_related',
            context_a   TEXT,
            context_b   TEXT,
            weight      REAL NOT NULL DEFAULT 1.0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_neg_query   ON negative_signals(query_id);
        CREATE INDEX IF NOT EXISTS idx_neg_pair    ON negative_signals(context_a, context_b);",
    )
    .map_err(|e| TraceMindError::Storage(format!("init context schema: {e}")))?;

    // Additive column on captured_signals. We ignore the error so re-runs
    // are idempotent (SQLite has no `ADD COLUMN IF NOT EXISTS`).
    let _ = conn.execute(
        "ALTER TABLE captured_signals ADD COLUMN context_id TEXT",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_signals_context ON captured_signals(context_id)",
        [],
    );

    Ok(())
}

/// Insert (or upsert by name) a context row.
pub fn create_context(conn: &Connection, ctx: &Context) -> Result<()> {
    conn.execute(
        "INSERT INTO contexts (id, name, tags, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO NOTHING",
        params![
            ctx.id.to_string(),
            ctx.name,
            ctx.tags,
            ctx.created_at.to_rfc3339(),
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("create_context: {e}")))?;
    Ok(())
}

/// List every context, newest first.
pub fn list_contexts(conn: &Connection) -> Result<Vec<Context>> {
    let mut stmt = conn
        .prepare("SELECT id, name, tags, created_at FROM contexts ORDER BY created_at DESC")
        .map_err(|e| TraceMindError::Storage(format!("list_contexts prepare: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let tags: String = r.get(2)?;
            let created_at: String = r.get(3)?;
            Ok((id, name, tags, created_at))
        })
        .map_err(|e| TraceMindError::Storage(format!("list_contexts query: {e}")))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, tags, created_at) =
            row.map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let id = Uuid::parse_str(&id)
            .map_err(|e| TraceMindError::Storage(format!("bad context uuid: {e}")))?;
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map_err(|e| TraceMindError::Storage(format!("bad created_at: {e}")))?
            .with_timezone(&Utc);
        out.push(Context { id, name, tags, created_at });
    }
    Ok(out)
}

/// Lookup by name. Returns `Ok(None)` if no row matches.
pub fn get_context_by_name(conn: &Connection, name: &str) -> Result<Option<Context>> {
    let mut stmt = conn
        .prepare("SELECT id, name, tags, created_at FROM contexts WHERE name = ?1")
        .map_err(|e| TraceMindError::Storage(format!("get_context prepare: {e}")))?;
    let mut rows = stmt
        .query(params![name])
        .map_err(|e| TraceMindError::Storage(format!("get_context query: {e}")))?;
    if let Some(row) = rows
        .next()
        .map_err(|e| TraceMindError::Storage(e.to_string()))?
    {
        let id: String = row.get(0).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let name: String = row.get(1).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let tags: String = row.get(2).map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let created_at: String = row
            .get(3)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let id = Uuid::parse_str(&id)
            .map_err(|e| TraceMindError::Storage(format!("bad context uuid: {e}")))?;
        let created_at = DateTime::parse_from_rfc3339(&created_at)
            .map_err(|e| TraceMindError::Storage(format!("bad created_at: {e}")))?
            .with_timezone(&Utc);
        Ok(Some(Context { id, name, tags, created_at }))
    } else {
        Ok(None)
    }
}

/// Write a negative-feedback signal. `result_id` is opaque (an entity
/// UUID, a triple UUID, or a signal row id stringified) so the caller
/// can target the appropriate granularity.
pub fn write_negative_signal(
    conn: &Connection,
    query_id: Uuid,
    result_id: &str,
    kind: &str,
    context_a: Option<Uuid>,
    context_b: Option<Uuid>,
    weight: f32,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO negative_signals (query_id, result_id, kind, context_a, context_b, weight)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            query_id.to_string(),
            result_id,
            kind,
            context_a.map(|u| u.to_string()),
            context_b.map(|u| u.to_string()),
            weight,
        ],
    )
    .map_err(|e| TraceMindError::Storage(format!("write_negative_signal: {e}")))?;
    Ok(conn.last_insert_rowid())
}

/// Sum negative-signal weights for a given query. Used by the bandit
/// reward decomposition: `final_reward = relevance_reward - this_sum`.
pub fn negative_weight_for_query(conn: &Connection, query_id: Uuid) -> Result<f32> {
    let mut stmt = conn
        .prepare("SELECT COALESCE(SUM(weight), 0.0) FROM negative_signals WHERE query_id = ?1")
        .map_err(|e| TraceMindError::Storage(format!("neg_weight prepare: {e}")))?;
    let weight: f64 = stmt
        .query_row(params![query_id.to_string()], |r| r.get(0))
        .map_err(|e| TraceMindError::Storage(format!("neg_weight query: {e}")))?;
    Ok(weight as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // Needed because init_schema's ALTER TABLE assumes captured_signals exists.
        conn.execute_batch(
            "CREATE TABLE captured_signals (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                raw_text TEXT NOT NULL,
                content_hash INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn init_schema_is_idempotent() {
        let conn = fresh_conn();
        // Run twice — second call must not error.
        init_schema(&conn).unwrap();
    }

    #[test]
    fn create_and_list_contexts_round_trip() {
        let conn = fresh_conn();
        let c1 = Context::new("rondo", "venture,sports");
        let c2 = Context::new("tracemind", "venture,memory");
        create_context(&conn, &c1).unwrap();
        create_context(&conn, &c2).unwrap();

        let all = list_contexts(&conn).unwrap();
        assert_eq!(all.len(), 2);
        // ORDER BY created_at DESC — c2 came last.
        assert_eq!(all[0].name, "tracemind");
        assert_eq!(all[1].name, "rondo");

        let r = get_context_by_name(&conn, "rondo").unwrap().unwrap();
        assert_eq!(r.id, c1.id);
        assert_eq!(r.tags, "venture,sports");

        let missing = get_context_by_name(&conn, "nope").unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn duplicate_context_name_is_a_noop() {
        let conn = fresh_conn();
        let c1 = Context::new("rondo", "venture");
        create_context(&conn, &c1).unwrap();
        let c2 = Context::new("rondo", "different,tags");
        // Same name — ON CONFLICT DO NOTHING. We keep the first.
        create_context(&conn, &c2).unwrap();
        let r = get_context_by_name(&conn, "rondo").unwrap().unwrap();
        assert_eq!(r.id, c1.id);
        assert_eq!(r.tags, "venture");
    }

    #[test]
    fn active_context_save_load_clear() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("active_context.json");
        assert!(ActiveContext::load(&p).unwrap().is_none());

        let a = ActiveContext { id: Uuid::new_v4(), name: "rondo".into() };
        a.save(&p).unwrap();

        let loaded = ActiveContext::load(&p).unwrap().unwrap();
        assert_eq!(loaded.id, a.id);
        assert_eq!(loaded.name, "rondo");

        ActiveContext::clear(&p).unwrap();
        assert!(ActiveContext::load(&p).unwrap().is_none());
    }

    #[test]
    fn negative_signals_sum_correctly() {
        let conn = fresh_conn();
        let q = Uuid::new_v4();
        let ca = Some(Uuid::new_v4());
        let cb = Some(Uuid::new_v4());

        write_negative_signal(&conn, q, "result-1", "not_related", ca, cb, 1.0).unwrap();
        write_negative_signal(&conn, q, "result-2", "not_related", ca, cb, 0.5).unwrap();
        // Different query — must not count.
        let q2 = Uuid::new_v4();
        write_negative_signal(&conn, q2, "result-3", "not_related", ca, cb, 9.9).unwrap();

        let s = negative_weight_for_query(&conn, q).unwrap();
        assert!((s - 1.5).abs() < 1e-6, "expected 1.5, got {s}");

        let s2 = negative_weight_for_query(&conn, q2).unwrap();
        assert!((s2 - 9.9).abs() < 1e-6);

        let s3 = negative_weight_for_query(&conn, Uuid::new_v4()).unwrap();
        assert_eq!(s3, 0.0);
    }
}
