//! `PropagateDelete` implementation for the vector layer.
//!
//! The actual embedding rows live in `kg_vectors` inside the shared
//! `memory.db` (owned by `tm-graph`). This module exposes a thin
//! [`VectorStore`] wrapper that opens the same file with `rusqlite` and
//! implements the trait so the Propagator can call it uniformly.
//!
//! Rows are keyed by the skg entity id. We resolve the uuid → skg-id
//! mapping by inspecting `kg_entities.properties` for the `uuid` field
//! (matching what `tm-graph::store` writes on upsert). If the entity is
//! not present, the report simply comes back with `rows_removed: 0`.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use tm_types::{PropagateDelete, PropagateReport, Result, TraceMindError};
use uuid::Uuid;

/// Standard database file name (shared with `tm-graph`).
pub const MEMORY_DB_FILE: &str = "memory.db";

/// Thin owner of a rusqlite connection scoped to vector rows.
///
/// This exists purely so the vector layer can implement
/// [`PropagateDelete`]; day-to-day embedding I/O lives in
/// [`crate::Embedder`] and in `tm-graph`.
pub struct VectorStore {
    conn: Connection,
    path: PathBuf,
}

impl VectorStore {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(MEMORY_DB_FILE);
        Self::open_path(path)
    }

    pub fn open_path(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        }
        let conn = Connection::open(&path)
            .map_err(|e| TraceMindError::Storage(format!("open {}: {e}", path.display())))?;
        Ok(Self { conn, path })
    }

    /// Wrap an already-open connection (used by tests and by callers that
    /// share the connection with tm-graph).
    pub fn open_with_connection(conn: Connection) -> Result<Self> {
        Ok(Self {
            conn,
            path: PathBuf::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Rusqlite handle (borrowed). Callers may issue read-only queries;
    /// mutations should go through the trait method or through `tm-graph`.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Resolve a TraceMind uuid to the underlying skg entity id by
    /// inspecting the `properties` JSON column of `kg_entities`.
    ///
    /// Returns `None` when the entity does not exist or the table is
    /// missing (fresh DB before `tm-graph` has initialised it).
    fn resolve_skg_id(&self, memory_id: Uuid) -> Result<Option<i64>> {
        let uuid_needle = format!("\"{}\"", memory_id);
        let mut stmt = match self.conn.prepare(
            "SELECT id FROM kg_entities WHERE properties LIKE '%' || ?1 || '%' LIMIT 1",
        ) {
            Ok(s) => s,
            // Table absent — nothing to delete.
            Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
                if msg.contains("no such table") =>
            {
                return Ok(None);
            }
            Err(e) => {
                return Err(TraceMindError::Storage(format!(
                    "kg_entities lookup prepare: {e}"
                )))
            }
        };
        let row = stmt
            .query_row(params![uuid_needle], |r| r.get::<_, i64>(0))
            .ok();
        Ok(row)
    }
}

impl PropagateDelete for VectorStore {
    fn propagate_delete(&self, memory_id: Uuid) -> Result<PropagateReport> {
        let skg_id = match self.resolve_skg_id(memory_id)? {
            Some(id) => id,
            None => return Ok(PropagateReport::new("tm-vector", 0)),
        };

        // The `kg_vectors` table may or may not exist depending on the
        // stage of tm-graph initialisation. Treat "missing table" as zero
        // rows.
        let removed = match self.conn.execute(
            "DELETE FROM kg_vectors WHERE entity_id = ?1",
            params![skg_id],
        ) {
            Ok(n) => n,
            Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
                if msg.contains("no such table") =>
            {
                0
            }
            Err(e) => return Err(TraceMindError::Storage(format!("kg_vectors: {e}"))),
        };

        Ok(PropagateReport::new("tm-vector", removed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Connection, i64, Uuid) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE kg_entities (id INTEGER PRIMARY KEY, properties TEXT);
            CREATE TABLE kg_vectors (entity_id INTEGER, vector BLOB);
            "#,
        )
        .unwrap();

        let uuid = Uuid::new_v4();
        let props = format!(r#"{{"uuid":"{}","confidence":0.9}}"#, uuid);
        conn.execute(
            "INSERT INTO kg_entities (id, properties) VALUES (7, ?1)",
            params![props],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO kg_vectors (entity_id, vector) VALUES (7, X'00')",
            [],
        )
        .unwrap();
        (conn, 7, uuid)
    }

    #[test]
    fn deletes_row_and_returns_count() {
        let (conn, _skg, uuid) = setup();
        let store = VectorStore::open_with_connection(conn).unwrap();
        let report = store.propagate_delete(uuid).unwrap();
        assert_eq!(report.crate_name, "tm-vector");
        assert_eq!(report.rows_removed, 1);

        // Idempotent.
        let again = store.propagate_delete(uuid).unwrap();
        assert_eq!(again.rows_removed, 0);
    }

    #[test]
    fn unknown_id_reports_zero() {
        let (conn, _skg, _uuid) = setup();
        let store = VectorStore::open_with_connection(conn).unwrap();
        let report = store.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.rows_removed, 0);
    }

    #[test]
    fn missing_tables_are_treated_as_empty() {
        let conn = Connection::open_in_memory().unwrap();
        let store = VectorStore::open_with_connection(conn).unwrap();
        let report = store.propagate_delete(Uuid::new_v4()).unwrap();
        assert_eq!(report.rows_removed, 0);
    }
}
