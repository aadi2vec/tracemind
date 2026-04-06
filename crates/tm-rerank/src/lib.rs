//! Optional ColBERT late-interaction reranker for TraceMind.
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
//! # Usage
//!
//! ```ignore
//! use tm_rerank::ColbertReranker;
//!
//! let reranker = ColbertReranker::new("model.onnx", "tokenizer.json", 0.7)?;
//!
//! let doc_tokens = reranker.encode_document("Alice works at Acme Corp")?;
//! let query_tokens = reranker.encode_query("who works at Acme")?;
//! let score = tm_rerank::maxsim(&query_tokens, &doc_tokens);
//! ```
//!
//! # Feature flag
//!
//! Enable with `--features colbert`. Without the flag, this crate compiles
//! to no-op stubs that always return `Err(Unavailable)`.

pub mod reranker;

pub use reranker::{maxsim, ColbertReranker, RerankCandidate, RerankResult, COLBERT_DIM};
