//! ColBERT late-interaction reranker for TraceMind.
//!
//! # Architecture
//!
//! ColBERT (Contextualised Late Interaction over BERT) produces per-token
//! embeddings for both queries and documents. Relevance is computed via
//! **MaxSim**: for each query token, find the max cosine similarity against
//! all document tokens, then sum across query tokens.
//!
//! This gives cross-encoder quality at bi-encoder speed because document
//! embeddings can be pre-computed and cached.
//!
//! # Model
//!
//! `mxbai-edge-colbert-v0-17m` — 16.8M params, ~35MB fp16, BEIR 0.490
//! (beats MiniLM's ~0.42). Output dim: 48. Runs in ~20-50ms on M1 Mac CPU.
//!
//! Auto-downloaded from HuggingFace Hub on first use — no feature flag, no
//! manual model-path configuration. Falls back gracefully if the network is
//! unreachable (caller gets `Err`, retrieval continues without reranking).
//!
//! # Usage
//!
//! ```ignore
//! use tm_rerank::ColbertReranker;
//!
//! // Auto-download on first call, cached on disk afterwards.
//! let reranker = ColbertReranker::auto_download(0.7)?;
//!
//! let doc_tokens = reranker.encode_document("Alice works at Acme Corp")?;
//! let query_tokens = reranker.encode_query("who works at Acme")?;
//! let score = tm_rerank::maxsim(&query_tokens, &doc_tokens);
//! ```

pub mod entity_index;
pub mod reranker;

pub use entity_index::{EntityIndex, EntryPoint, IndexedEntity};
pub use reranker::{maxsim, ColbertReranker, RerankCandidate, RerankResult, COLBERT_DIM};
