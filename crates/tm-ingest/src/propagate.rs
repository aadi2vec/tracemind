//! Coordinator for [`tm_types::PropagateDelete`].
//!
//! When a user forgets a memory, every crate that stored anything derived
//! from that memory must drop its rows too (plan §5 S3). The `Propagator`
//! holds shared references to every derived-data store and calls
//! `propagate_delete` on each in sequence. The returned
//! `Vec<PropagateReport>` is what the caller (the future `memory_forget`
//! MCP verb + `tracemind forget` CLI + integration tests) uses to prove
//! no orphans remain.
//!
//! Ordering matters: the graph goes last so that any downstream store
//! that might do a "does this uuid still exist?" check runs against a
//! graph that still knows about the entity. Every impl is idempotent so
//! retries are safe.

use std::sync::Arc;

use tm_controller::{LinUcbBandit, UcbBandit};
use tm_episodic::{RecentStore, TraceStore};
use tm_graph::GraphStore;
use tm_reflect::ReflexionStore;
use tm_types::{PropagateDelete, PropagateReport, Result};
use tm_vector::VectorStore;
use uuid::Uuid;

/// Which subsystems the coordinator should call. Everything defaults on
/// so the safe path is the default path — callers explicitly opt *out*
/// (e.g. tests that only care about the graph).
#[derive(Debug, Clone)]
pub struct PropagatorConfig {
    pub include_graph: bool,
    pub include_vector: bool,
    pub include_recent: bool,
    pub include_trace: bool,
    pub include_reflection: bool,
    pub include_bandit: bool,
}

impl Default for PropagatorConfig {
    fn default() -> Self {
        Self {
            include_graph: true,
            include_vector: true,
            include_recent: true,
            include_trace: true,
            include_reflection: true,
            include_bandit: true,
        }
    }
}

/// Coordinates propagate-delete across every crate that stores derived data.
pub struct Propagator {
    pub graph: Option<Arc<GraphStore>>,
    pub vector: Option<Arc<VectorStore>>,
    pub recent: Option<Arc<RecentStore>>,
    pub trace: Option<Arc<TraceStore>>,
    pub reflection: Option<Arc<ReflexionStore>>,
    pub ucb: Option<Arc<UcbBandit>>,
    pub linucb: Option<Arc<LinUcbBandit>>,
    pub config: PropagatorConfig,
}

impl Propagator {
    pub fn new() -> Self {
        Self {
            graph: None,
            vector: None,
            recent: None,
            trace: None,
            reflection: None,
            ucb: None,
            linucb: None,
            config: PropagatorConfig::default(),
        }
    }

    pub fn with_graph(mut self, graph: Arc<GraphStore>) -> Self {
        self.graph = Some(graph);
        self
    }

    pub fn with_vector(mut self, vector: Arc<VectorStore>) -> Self {
        self.vector = Some(vector);
        self
    }

    pub fn with_recent(mut self, recent: Arc<RecentStore>) -> Self {
        self.recent = Some(recent);
        self
    }

    pub fn with_trace(mut self, trace: Arc<TraceStore>) -> Self {
        self.trace = Some(trace);
        self
    }

    pub fn with_reflection(mut self, refl: Arc<ReflexionStore>) -> Self {
        self.reflection = Some(refl);
        self
    }

    pub fn with_ucb(mut self, ucb: Arc<UcbBandit>) -> Self {
        self.ucb = Some(ucb);
        self
    }

    pub fn with_linucb(mut self, linucb: Arc<LinUcbBandit>) -> Self {
        self.linucb = Some(linucb);
        self
    }

    pub fn with_config(mut self, config: PropagatorConfig) -> Self {
        self.config = config;
        self
    }

    /// Ask every configured store to drop rows for `memory_id`.
    ///
    /// Reports are returned in call order. On the first hard error the
    /// propagation stops — callers should surface the partial vector to
    /// the audit log so a retry can catch the tail.
    pub fn forget(&self, memory_id: Uuid) -> Result<Vec<PropagateReport>> {
        let mut reports = Vec::new();

        if self.config.include_recent {
            if let Some(store) = &self.recent {
                reports.push(store.propagate_delete(memory_id)?);
            }
        }
        if self.config.include_trace {
            if let Some(store) = &self.trace {
                reports.push(store.propagate_delete(memory_id)?);
            }
        }
        if self.config.include_reflection {
            if let Some(store) = &self.reflection {
                reports.push(store.propagate_delete(memory_id)?);
            }
        }
        if self.config.include_vector {
            if let Some(store) = &self.vector {
                reports.push(store.propagate_delete(memory_id)?);
            }
        }
        if self.config.include_bandit {
            if let Some(b) = &self.ucb {
                reports.push(b.propagate_delete(memory_id)?);
            }
            if let Some(b) = &self.linucb {
                reports.push(b.propagate_delete(memory_id)?);
            }
        }
        // Graph last — see module doc.
        if self.config.include_graph {
            if let Some(store) = &self.graph {
                reports.push(store.propagate_delete(memory_id)?);
            }
        }

        Ok(reports)
    }
}

impl Default for Propagator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_propagator_returns_empty_vec() {
        let p = Propagator::new();
        let reports = p.forget(Uuid::new_v4()).unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn config_gates_subsystems() {
        let dir = tempfile::tempdir().unwrap();
        let recent = Arc::new(
            RecentStore::open_with_capacity(dir.path().join("recent.jsonl"), 10).unwrap(),
        );
        // The recent store is empty, but with the gate off it should never
        // even be consulted → zero reports.
        let cfg = PropagatorConfig {
            include_recent: false,
            ..PropagatorConfig::default()
        };
        let p = Propagator::new().with_recent(recent).with_config(cfg);
        let reports = p.forget(Uuid::new_v4()).unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn recent_store_only_reports_from_that_layer() {
        let dir = tempfile::tempdir().unwrap();
        let recent = Arc::new(
            RecentStore::open_with_capacity(dir.path().join("recent.jsonl"), 10).unwrap(),
        );
        let p = Propagator::new().with_recent(recent);
        let reports = p.forget(Uuid::new_v4()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].crate_name, "tm-episodic:recent");
        assert_eq!(reports[0].rows_removed, 0);
    }
}
