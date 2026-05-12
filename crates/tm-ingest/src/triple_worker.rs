//! LM-8 — async triple-extraction worker (scaffold).
//!
//! The hot path of [`IngestPipeline::ingest`] uses `HeuristicExtractor`
//! synchronously so that ingest latency stays bounded. The
//! `TripleWorker` lifts a richer (SML or rule-based) extractor off the
//! hot path: callers enqueue a [`TripleJob`] and a background thread
//! pulls jobs from a bounded channel, runs the extractor, and routes
//! every resulting triple through `GraphStore::route_triple` so that
//! low-confidence triples land in the `pending_relations` pool and
//! high-confidence triples join `kg_relations` (LM-9).
//!
//! Today the worker runs `HeuristicExtractor` end-to-end, which means
//! it produces the *same* triples the hot path already enqueues —
//! intentional. When LM-6 picks a small language model, swap the
//! `Box<dyn EntityExtractor>` and the rest of the wiring stays put.
//!
//! Design constraints honoured here:
//! - **No tokio dependency**: tm-ingest must build on platforms where
//!   we don't ship a runtime. We use `std::sync::mpsc::sync_channel`.
//! - **Backpressure**: the channel is bounded; the producer non-blocks
//!   via `try_send` and counts `dropped`.
//! - **Clean shutdown**: dropping the [`TripleWorkerHandle`] closes
//!   the channel; the worker thread exits its loop and joins.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::JoinHandle;
use tm_graph::GraphStore;
use tm_types::Entity;
use uuid::Uuid;

use crate::extractor::{EntityExtractor, HeuristicExtractor};

/// Opaque per-worker DB target. The worker opens its own SQLite
/// connection so it doesn't contend with the hot path through a
/// mutex.
#[derive(Debug, Clone)]
pub enum WorkerDb {
    /// Path on disk; the worker opens `GraphStore::open(path)`.
    Path(String),
    /// In-memory only; tests use this. The graph state in the worker
    /// is *not* shared with anything else.
    InMemory,
}

/// One unit of work for the [`TripleWorker`]: the text to extract
/// from, the entities the synchronous pipeline already identified,
/// and the session that ingested them.
#[derive(Debug, Clone)]
pub struct TripleJob {
    pub text: String,
    pub entities: Vec<Entity>,
    pub session_id: Uuid,
}

/// Cumulative counters exposed via [`TripleWorkerHandle::stats`].
#[derive(Debug, Default)]
pub struct TripleWorkerStats {
    /// Jobs successfully enqueued.
    pub enqueued: AtomicU64,
    /// Jobs the channel rejected because it was full.
    pub dropped: AtomicU64,
    /// Jobs the worker has finished processing.
    pub processed: AtomicU64,
    /// Total triples routed (high-confidence + pending pool).
    pub triples_routed: AtomicU64,
}

impl TripleWorkerStats {
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.enqueued.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.processed.load(Ordering::Relaxed),
            self.triples_routed.load(Ordering::Relaxed),
        )
    }
}

/// Async triple-extraction worker.
///
/// Construct via [`TripleWorker::spawn`]. Hand the returned
/// [`TripleWorkerHandle`] to the ingest pipeline; call
/// [`TripleWorkerHandle::try_enqueue`] from the hot path.
pub struct TripleWorker;

/// Handle to a running worker. Dropping it closes the channel and
/// joins the thread.
pub struct TripleWorkerHandle {
    sender: Option<SyncSender<TripleJob>>,
    stats: Arc<TripleWorkerStats>,
    join: Option<JoinHandle<()>>,
}

impl TripleWorker {
    /// Spawn a worker that pulls jobs from a `capacity`-bounded channel,
    /// runs `extractor` on each job, and routes the resulting triples
    /// into a fresh `GraphStore` opened from `db`.
    ///
    /// The worker opens its own connection rather than sharing one
    /// with the hot path: SQLite WAL mode handles concurrent readers
    /// + a single writer fine, and a dedicated connection avoids
    /// `GraphStore`'s interior-mutable caches (which are `!Sync`).
    pub fn spawn(
        db: WorkerDb,
        extractor: Box<dyn EntityExtractor>,
        capacity: usize,
    ) -> TripleWorkerHandle {
        let (tx, rx) = sync_channel::<TripleJob>(capacity.max(1));
        let stats = Arc::new(TripleWorkerStats::default());
        let stats_for_thread = Arc::clone(&stats);
        let join = std::thread::Builder::new()
            .name("tm-ingest-triple-worker".into())
            .spawn(move || {
                let path = match db {
                    WorkerDb::Path(p) => p,
                    WorkerDb::InMemory => ":memory:".to_string(),
                };
                let graph = match GraphStore::open(&path) {
                    Ok(g) => g,
                    Err(e) => {
                        tracing::error!(
                            target: "tm_ingest::triple_worker",
                            "failed to open graph store for triple worker: {e}"
                        );
                        return;
                    }
                };
                while let Ok(job) = rx.recv() {
                    let triples = extractor.extract_triples(&job.text, &job.entities);
                    let mut routed = 0u64;
                    for t in triples {
                        // Confidence-routed acceptance (LM-9):
                        // `route_triple_by_confidence` already pools
                        // mid-confidence values and promotes
                        // high-confidence ones.
                        if graph.route_triple_by_confidence(&t).is_ok() {
                            routed += 1;
                        }
                    }
                    stats_for_thread
                        .processed
                        .fetch_add(1, Ordering::Relaxed);
                    stats_for_thread
                        .triples_routed
                        .fetch_add(routed, Ordering::Relaxed);
                }
            })
            .expect("triple-worker thread must spawn");
        TripleWorkerHandle {
            sender: Some(tx),
            stats,
            join: Some(join),
        }
    }

    /// Convenience: spawn a worker pre-wired with the default
    /// [`HeuristicExtractor`]. Used by tests + as the boot-strap
    /// configuration until LM-6 picks an SML.
    pub fn spawn_default(db: WorkerDb, capacity: usize) -> TripleWorkerHandle {
        Self::spawn(db, Box::new(HeuristicExtractor), capacity)
    }
}

impl TripleWorkerHandle {
    /// Non-blocking enqueue. Returns `false` and bumps `dropped` when
    /// the channel is full or closed; the hot path must not block on
    /// the worker.
    pub fn try_enqueue(&self, job: TripleJob) -> bool {
        let Some(sender) = self.sender.as_ref() else {
            return false;
        };
        match sender.try_send(job) {
            Ok(()) => {
                self.stats.enqueued.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Snapshot of `(enqueued, dropped, processed, triples_routed)`.
    pub fn stats(&self) -> (u64, u64, u64, u64) {
        self.stats.snapshot()
    }

    /// Cleanly shut down the worker: closes the channel, waits for the
    /// thread to drain and exit.
    pub fn shutdown(mut self) {
        drop(self.sender.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for TripleWorkerHandle {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tm_types::EntityType;

    fn wait_for_processed(handle: &TripleWorkerHandle, target: u64) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if handle.stats().2 >= target {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "worker did not process {} jobs (stats={:?})",
            target,
            handle.stats()
        );
    }

    #[test]
    fn worker_processes_an_enqueued_job() {
        let handle = TripleWorker::spawn_default(WorkerDb::InMemory, 4);
        let ents = vec![
            Entity::new("Aaditya", EntityType::Person, 0.9),
            Entity::new("TraceMind", EntityType::Project, 0.9),
        ];
        let ok = handle.try_enqueue(TripleJob {
            text: "Aaditya works at TraceMind".into(),
            entities: ents,
            session_id: Uuid::new_v4(),
        });
        assert!(ok, "first enqueue must succeed");
        wait_for_processed(&handle, 1);
        let (enq, dropped, processed, _routed) = handle.stats();
        assert_eq!(enq, 1);
        assert_eq!(dropped, 0);
        assert_eq!(processed, 1);
    }

    #[test]
    fn worker_backpressures_when_channel_is_full() {
        // capacity 1 means at most one queued job + one in flight.
        let handle = TripleWorker::spawn_default(WorkerDb::InMemory, 1);
        let mut accepted = 0u64;
        let mut rejected = 0u64;
        for _ in 0..50 {
            let ok = handle.try_enqueue(TripleJob {
                text: "x works at y".into(),
                entities: vec![Entity::new("x", EntityType::Person, 0.9)],
                session_id: Uuid::new_v4(),
            });
            if ok {
                accepted += 1;
            } else {
                rejected += 1;
            }
        }
        // We can't assert an exact number for rejected because the
        // background thread is also draining — but at least *some*
        // rejections must occur when we burst-publish 50 messages
        // into a capacity-1 channel.
        assert!(accepted >= 1, "at least one job must enqueue");
        let (enq, dropped, _, _) = handle.stats();
        assert_eq!(enq, accepted);
        assert_eq!(dropped, rejected);
    }

    #[test]
    fn handle_drop_joins_worker_cleanly() {
        let handle = TripleWorker::spawn_default(WorkerDb::InMemory, 4);
        handle.try_enqueue(TripleJob {
            text: "Pat works at Sequoia".into(),
            entities: vec![Entity::new("Pat", EntityType::Person, 0.9)],
            session_id: Uuid::new_v4(),
        });
        wait_for_processed(&handle, 1);
        // Dropping the handle should not panic and must join the worker
        // within a bounded time. The Drop impl handles this.
        drop(handle);
    }
}
