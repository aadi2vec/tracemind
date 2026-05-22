//! LGS-2 — **ColBERT EntityIndex**.
//!
//! Per the investor "Designing a Graph-Based Memory System" doc and
//! `docs/TASKS.md` §P4d: ColBERT MaxSim isn't just a final-stage
//! reranker — it's the *entry-point selector* for graph traversal.
//!
//! Given a query, we want the top-k **entities** (graph nodes) whose
//! pre-computed ColBERT token grids have the highest MaxSim against
//! the query tokens. Those entities become the seed set for the
//! 4-action KG traversal in `tm-graph::store`.
//!
//! Architecture:
//!
//! 1. At ingest time, every entity's name + (optional) description is
//!    encoded once and the token grid is cached
//!    (`tm-graph::upsert_colbert_tokens`).
//! 2. At query time, we encode the query (~20-50 ms once per query),
//!    then loop over the cached document grids computing MaxSim.
//!    MaxSim is pure CPU vector ops — ≤ 1 ms per candidate at
//!    `COLBERT_DIM = 48`.
//! 3. The top-k entity ids are returned as `EntryPoint { id, score }`,
//!    sorted descending.
//!
//! Why an in-memory index rather than streaming over SQLite per query:
//! the typical TraceMind graph holds ≤ 10 k entities; their ColBERT
//! grids fit in ~50 MB at 32 tokens × 48 dims × f32. Loading once is
//! cheaper than 10 k SQLite roundtrips per query.

use std::sync::Arc;

use crate::reranker::{maxsim, ColbertReranker};
use tm_types::{Result, TraceMindError};

/// One entity in the index, with its pre-computed ColBERT token grid.
#[derive(Debug, Clone)]
pub struct IndexedEntity {
    pub id: String,
    /// Pre-computed ColBERT document tokens (rows = tokens, cols = `COLBERT_DIM`).
    pub doc_tokens: Vec<Vec<f32>>,
}

/// Top-k entry-point candidate returned by [`EntityIndex::top_k`].
#[derive(Debug, Clone, PartialEq)]
pub struct EntryPoint {
    pub id: String,
    pub score: f32,
}

/// In-memory ColBERT entity index. Optimised for the "encode query
/// once, score N entities" path that drives KG traversal entry-point
/// selection.
pub struct EntityIndex {
    reranker: Option<Arc<ColbertReranker>>,
    entries: Vec<IndexedEntity>,
}

impl EntityIndex {
    /// New empty index backed by a shared reranker. The reranker is
    /// `Option` so unit tests can build an index from pre-supplied
    /// token grids without instantiating ONNX Runtime.
    pub fn new(reranker: Option<Arc<ColbertReranker>>) -> Self {
        Self { reranker, entries: Vec::new() }
    }

    /// Insert an entity by encoding its text now. Requires a live
    /// reranker — fails if [`Self::new`] was called with `None`.
    pub fn upsert(&mut self, id: impl Into<String>, text: &str) -> Result<()> {
        let r = self
            .reranker
            .as_ref()
            .ok_or_else(|| TraceMindError::Embedding("EntityIndex has no reranker".into()))?;
        let toks = r.encode_document(text)?;
        self.upsert_with_tokens(id, toks);
        Ok(())
    }

    /// Insert an entity using a pre-computed token grid. Used by
    /// callers that pull the cached grid from
    /// `tm-graph::load_colbert_tokens` or by tests that want
    /// deterministic input.
    pub fn upsert_with_tokens(&mut self, id: impl Into<String>, tokens: Vec<Vec<f32>>) {
        let id = id.into();
        // Dedupe by id — last write wins.
        self.entries.retain(|e| e.id != id);
        self.entries.push(IndexedEntity { id, doc_tokens: tokens });
    }

    /// Number of entities currently in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Score every entity against the query and return the top-k by
    /// MaxSim, descending. Score ties are broken by insertion order.
    ///
    /// Returns an empty vec if the index is empty. Requires a live
    /// reranker (to encode the query); fails otherwise.
    pub fn top_k(&self, query: &str, k: usize) -> Result<Vec<EntryPoint>> {
        if self.entries.is_empty() || k == 0 {
            return Ok(Vec::new());
        }
        let r = self
            .reranker
            .as_ref()
            .ok_or_else(|| TraceMindError::Embedding("EntityIndex has no reranker".into()))?;
        let query_tokens = r.encode_query(query)?;
        Ok(self.top_k_with_query_tokens(&query_tokens, k))
    }

    /// As [`Self::top_k`] but takes a pre-computed query token grid.
    /// Used by callers that already encoded the query for some other
    /// purpose (e.g. arm 4 retrieval) so we don't pay the encode cost
    /// twice. Pure CPU — no I/O — and trivial to unit-test.
    pub fn top_k_with_query_tokens(
        &self,
        query_tokens: &[Vec<f32>],
        k: usize,
    ) -> Vec<EntryPoint> {
        if self.entries.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut scored: Vec<EntryPoint> = self
            .entries
            .iter()
            .map(|e| EntryPoint {
                id: e.id.clone(),
                score: maxsim(query_tokens, &e.doc_tokens),
            })
            .collect();
        // Sort descending by score; preserve insertion order for ties.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(k);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec3(a: f32, b: f32, c: f32) -> Vec<f32> {
        vec![a, b, c]
    }

    /// Construct a small deterministic index and verify ordering.
    /// The "winning" entity shares one token vector with the query,
    /// so its MaxSim contribution from that token is 1.0; the others
    /// share nothing.
    #[test]
    fn top_k_orders_by_maxsim() {
        let mut idx = EntityIndex::new(None);
        idx.upsert_with_tokens("losing", vec![vec3(0.0, 1.0, 0.0)]);
        idx.upsert_with_tokens(
            "winning",
            vec![vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0)],
        );
        idx.upsert_with_tokens("middle", vec![vec3(0.5, 0.5, 0.0)]);

        let query = vec![vec3(1.0, 0.0, 0.0)];
        let top = idx.top_k_with_query_tokens(&query, 3);
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].id, "winning");
        // "middle" has cosine ~ 0.707 against query, "losing" 0.0.
        assert_eq!(top[1].id, "middle");
        assert_eq!(top[2].id, "losing");
        assert!(top[0].score > top[1].score);
        assert!(top[1].score > top[2].score);
    }

    #[test]
    fn upsert_dedupes_by_id() {
        let mut idx = EntityIndex::new(None);
        idx.upsert_with_tokens("e1", vec![vec3(1.0, 0.0, 0.0)]);
        idx.upsert_with_tokens("e1", vec![vec3(0.0, 1.0, 0.0)]);
        assert_eq!(idx.len(), 1);

        let q = vec![vec3(0.0, 1.0, 0.0)];
        let top = idx.top_k_with_query_tokens(&q, 1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].id, "e1");
        // Updated tokens — should now score 1.0, not 0.0.
        assert!((top[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn empty_or_zero_k_returns_empty() {
        let idx = EntityIndex::new(None);
        let q = vec![vec3(1.0, 0.0, 0.0)];
        assert!(idx.top_k_with_query_tokens(&q, 5).is_empty());

        let mut idx2 = EntityIndex::new(None);
        idx2.upsert_with_tokens("a", vec![vec3(1.0, 0.0, 0.0)]);
        assert!(idx2.top_k_with_query_tokens(&q, 0).is_empty());
    }

    #[test]
    fn ties_preserve_insertion_order() {
        let mut idx = EntityIndex::new(None);
        idx.upsert_with_tokens("first", vec![vec3(1.0, 0.0, 0.0)]);
        idx.upsert_with_tokens("second", vec![vec3(1.0, 0.0, 0.0)]);
        let q = vec![vec3(1.0, 0.0, 0.0)];
        let top = idx.top_k_with_query_tokens(&q, 2);
        assert_eq!(top[0].id, "first");
        assert_eq!(top[1].id, "second");
    }
}
