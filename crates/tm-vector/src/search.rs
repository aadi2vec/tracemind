//! LanceDB-backed vector store for TraceMind (Phase 2).
//!
//! A dedicated multi-thread tokio Runtime is embedded so callers can use the
//! synchronous interface regardless of the calling context.

use std::sync::Arc;

use arrow_array::{
    types::Float32Type, Array, ArrayRef, FixedSizeListArray, RecordBatch, RecordBatchIterator,
    StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Connection;
use tokio::runtime::Runtime;
use tm_types::{Result, TraceMindError};
use tracing::{debug, info};
use uuid::Uuid;

const TABLE_NAME: &str = "vectors";
const EMBED_DIM: i32 = 384;

/// Persistent vector store backed by LanceDB (cosine distance, 384-dim).
pub struct VectorStore {
    rt: Runtime,
    conn: Connection,
}

impl VectorStore {
    /// Open or create a LanceDB store at `path` (a directory on disk).
    pub fn open(path: &str) -> Result<Self> {
        info!("[vector] opening LanceDB store at {path}");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| TraceMindError::Storage(format!("tokio runtime: {e}")))?;

        let conn = rt
            .block_on(lancedb::connect(path).execute())
            .map_err(|e| TraceMindError::Storage(format!("lancedb connect: {e}")))?;

        // Create table if it doesn't exist.
        let exists = rt
            .block_on(conn.table_names().execute())
            .map_err(|e| TraceMindError::Storage(format!("table_names: {e}")))?
            .contains(&TABLE_NAME.to_string());

        if !exists {
            info!("[vector] creating table '{TABLE_NAME}' (first run)");
            rt.block_on(conn.create_empty_table(TABLE_NAME, table_schema()).execute())
                .map_err(|e| TraceMindError::Storage(format!("create table: {e}")))?;
        } else {
            debug!("[vector] table '{TABLE_NAME}' already exists");
        }

        Ok(Self { rt, conn })
    }

    /// Insert or replace the embedding for `id`.
    pub fn upsert(&self, id: Uuid, embedding: &[f32]) -> Result<()> {
        debug!("[vector] upsert id={id}");

        let tbl = self
            .rt
            .block_on(self.conn.open_table(TABLE_NAME).execute())
            .map_err(|e| TraceMindError::Storage(format!("open table: {e}")))?;

        let batch = single_row_batch(id, embedding)?;
        let schema = batch.schema();
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);

        let mut mi = tbl.merge_insert(&["id"]);
        mi.when_matched_update_all(None).when_not_matched_insert_all();

        self.rt
            .block_on(mi.execute(Box::new(reader)))
            .map_err(|e| TraceMindError::Storage(format!("merge_insert: {e}")))?;

        debug!("[vector] upsert complete for id={id}");
        Ok(())
    }

    /// Return the `top_k` most similar vectors to `query` (descending similarity).
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(Uuid, f32)>> {
        debug!("[vector] searching top_k={top_k}");

        let tbl = self
            .rt
            .block_on(self.conn.open_table(TABLE_NAME).execute())
            .map_err(|e| TraceMindError::Storage(format!("open table: {e}")))?;

        let row_count = self
            .rt
            .block_on(tbl.count_rows(None))
            .map_err(|e| TraceMindError::Storage(format!("count_rows: {e}")))?;

        if row_count == 0 {
            debug!("[vector] table empty, returning []");
            return Ok(vec![]);
        }
        debug!("[vector] table has {row_count} rows");

        let stream = self
            .rt
            .block_on(
                tbl.vector_search(query.to_vec())
                    .map_err(|e| TraceMindError::Storage(format!("vector_search: {e}")))?
                    .distance_type(lancedb::DistanceType::Cosine)
                    .limit(top_k)
                    .execute(),
            )
            .map_err(|e| TraceMindError::Storage(format!("execute search: {e}")))?;

        use futures::TryStreamExt;
        let batches: Vec<RecordBatch> = self
            .rt
            .block_on(stream.try_collect())
            .map_err(|e| TraceMindError::Storage(format!("collect results: {e}")))?;

        let mut results: Vec<(Uuid, f32)> = Vec::new();

        for batch in &batches {
            let id_col = batch
                .column_by_name("id")
                .ok_or_else(|| TraceMindError::Storage("missing id column".into()))?;
            let dist_col = batch
                .column_by_name("_distance")
                .ok_or_else(|| TraceMindError::Storage("missing _distance column".into()))?;

            let ids = id_col
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| TraceMindError::Storage("id not StringArray".into()))?;
            let dists = dist_col
                .as_any()
                .downcast_ref::<arrow_array::Float32Array>()
                .ok_or_else(|| TraceMindError::Storage("_distance not Float32Array".into()))?;

            for i in 0..batch.num_rows() {
                if ids.is_null(i) || dists.is_null(i) {
                    continue;
                }
                let uid = Uuid::parse_str(ids.value(i))
                    .map_err(|e| TraceMindError::Storage(format!("bad uuid: {e}")))?;
                let similarity = 1.0 - dists.value(i);
                results.push((uid, similarity));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Less));
        results.truncate(top_k);
        info!("[vector] search returned {} results", results.len());
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn table_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBED_DIM,
            ),
            false,
        ),
    ]))
}

fn single_row_batch(id: Uuid, embedding: &[f32]) -> Result<RecordBatch> {
    let schema = table_schema();

    let id_array = Arc::new(StringArray::from(vec![id.to_string()])) as ArrayRef;

    let embed_values: Vec<Option<Vec<Option<f32>>>> =
        vec![Some(embedding.iter().map(|&f| Some(f)).collect())];
    let embed_array = Arc::new(FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        embed_values,
        EMBED_DIM,
    )) as ArrayRef;

    RecordBatch::try_new(schema, vec![id_array, embed_array])
        .map_err(|e| TraceMindError::Storage(format!("record batch: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vec(dim: usize, hot: usize) -> Vec<f32> {
        let mut v = vec![0.0_f32; dim];
        v[hot] = 1.0;
        v
    }

    fn tmp_store() -> (VectorStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("tm_vec_{}", Uuid::new_v4()));
        let store = VectorStore::open(dir.to_str().unwrap()).expect("open temp store");
        (store, dir)
    }

    #[test]
    fn search_returns_exact_match_as_top1() {
        let (store, dir) = tmp_store();
        let dim = EMBED_DIM as usize;
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let id_c = Uuid::new_v4();

        store.upsert(id_a, &unit_vec(dim, 0)).unwrap();
        store.upsert(id_b, &unit_vec(dim, 1)).unwrap();
        store.upsert(id_c, &unit_vec(dim, 2)).unwrap();

        let results = store.search(&unit_vec(dim, 1), 3).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].0, id_b);
        assert!((results[0].1 - 1.0).abs() < 1e-4, "score={}", results[0].1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upsert_replaces_existing_entry() {
        let (store, dir) = tmp_store();
        let dim = EMBED_DIM as usize;
        let id = Uuid::new_v4();

        store.upsert(id, &unit_vec(dim, 0)).unwrap();
        store.upsert(id, &unit_vec(dim, 1)).unwrap();

        let results = store.search(&unit_vec(dim, 1), 1).unwrap();
        assert_eq!(results[0].0, id);
        assert!((results[0].1 - 1.0).abs() < 1e-4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_respects_top_k_limit() {
        let (store, dir) = tmp_store();
        let dim = EMBED_DIM as usize;
        for i in 0..10 {
            store.upsert(Uuid::new_v4(), &unit_vec(dim, i)).unwrap();
        }
        let results = store.search(&unit_vec(dim, 0), 3).unwrap();
        assert_eq!(results.len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_on_empty_store_returns_empty() {
        let (store, dir) = tmp_store();
        let dim = EMBED_DIM as usize;
        let results = store.search(&unit_vec(dim, 0), 5).unwrap();
        assert!(results.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
