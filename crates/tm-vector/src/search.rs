//! SQLite-backed brute-force vector store for TraceMind MVP.
//!
//! Embeddings are stored as raw little-endian `f32` BLOBs.  Search is O(n)
//! cosine similarity over all rows — acceptable up to ~100 K vectors / 200 MB.
//! Phase 2 will introduce an HNSW index once that boundary approaches.

use rusqlite::{params, Connection};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// A persistent vector store backed by a single SQLite database file.
pub struct VectorStore {
    conn: Connection,
}

impl VectorStore {
    /// Open (or create) the vector store at `path`.
    ///
    /// Runs `CREATE TABLE IF NOT EXISTS` so callers can use any fresh path
    /// without pre-initialisation.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                id        TEXT PRIMARY KEY,
                embedding BLOB NOT NULL
            );",
        )
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        Ok(Self { conn })
    }

    /// Insert or replace the embedding for `id`.
    ///
    /// The embedding is serialised as a sequence of little-endian 4-byte
    /// chunks (one per `f32`).
    pub fn upsert(&self, id: Uuid, embedding: &[f32]) -> Result<()> {
        let blob: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        self.conn
            .execute(
                "INSERT OR REPLACE INTO vectors (id, embedding) VALUES (?1, ?2)",
                params![id.to_string(), blob],
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        Ok(())
    }

    /// Return the `top_k` most similar vectors to `query`, ranked by
    /// descending cosine similarity.
    ///
    /// This is a full-table scan — fine for the MVP target of <100 K rows.
    /// Pairs where either norm is zero are silently skipped.
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(Uuid, f32)>> {
        let query_norm = l2_norm(query);
        if query_norm == 0.0 {
            return Ok(vec![]);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT id, embedding FROM vectors")
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;

        let mut scores: Vec<(Uuid, f32)> = stmt
            .query_map([], |row| {
                let id_str: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id_str, blob))
            })
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .filter_map(|res| {
                let (id_str, blob) = res.ok()?;
                let id = Uuid::parse_str(&id_str).ok()?;
                let vec = deserialize_embedding(&blob);

                let row_norm = l2_norm(&vec);
                if row_norm == 0.0 {
                    return None;
                }

                let dot: f32 = query.iter().zip(vec.iter()).map(|(a, b)| a * b).sum();
                let similarity = dot / (query_norm * row_norm);
                Some((id, similarity))
            })
            .collect();

        // Sort descending by score; NaN values (shouldn't arise) go last.
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Less));
        scores.truncate(top_k);

        Ok(scores)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn deserialize_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny normalised vector of `dim` dimensions where only slot 0
    /// is non-zero, making cosine similarity trivial to reason about.
    fn unit_vec(dim: usize, hot_index: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; dim];
        v[hot_index] = 1.0;
        v
    }

    #[test]
    fn search_returns_exact_match_as_top1() {
        // Use an in-memory SQLite database for test isolation.
        let store = VectorStore::open(":memory:").expect("open in-memory store");

        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let id_c = Uuid::new_v4();

        let vec_a = unit_vec(4, 0); // [1, 0, 0, 0]
        let vec_b = unit_vec(4, 1); // [0, 1, 0, 0]
        let vec_c = unit_vec(4, 2); // [0, 0, 1, 0]

        store.upsert(id_a, &vec_a).unwrap();
        store.upsert(id_b, &vec_b).unwrap();
        store.upsert(id_c, &vec_c).unwrap();

        // Query with vec_b — should come back as top-1 with score ≈ 1.0.
        let results = store.search(&vec_b, 3).unwrap();

        assert_eq!(results.len(), 3, "should return 3 results");

        let (top_id, top_score) = results[0];
        assert_eq!(top_id, id_b, "top result must be the queried vector");
        assert!(
            (top_score - 1.0).abs() < 1e-5,
            "cosine similarity of a vector with itself must be ≈ 1.0, got {top_score}"
        );

        // The other two vectors are orthogonal to vec_b, so score = 0.0.
        assert!(
            results[1].1.abs() < 1e-5,
            "orthogonal vectors should have similarity ≈ 0"
        );
    }

    #[test]
    fn upsert_replaces_existing_entry() {
        let store = VectorStore::open(":memory:").unwrap();
        let id = Uuid::new_v4();

        store.upsert(id, &unit_vec(4, 0)).unwrap();
        store.upsert(id, &unit_vec(4, 1)).unwrap(); // overwrite

        // After overwrite the stored vector should match [0,1,0,0].
        let results = store.search(&unit_vec(4, 1), 1).unwrap();
        assert_eq!(results[0].0, id);
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn search_respects_top_k_limit() {
        let store = VectorStore::open(":memory:").unwrap();

        for i in 0..10 {
            store.upsert(Uuid::new_v4(), &unit_vec(10, i)).unwrap();
        }

        let results = store.search(&unit_vec(10, 0), 3).unwrap();
        assert_eq!(results.len(), 3, "top_k=3 must return at most 3 results");
    }
}
