//! Integration test for the I-P1 PropagateDelete pipeline.
//!
//! Inserts a memory (entity + embedding + recent-capture row + trace),
//! runs `Propagator::forget`, and asserts every derived-data store drops
//! the memory to zero rows.
//!
//! The trace log is deliberately excluded from the zero-rows assertion —
//! it is an immutable audit log that reports `rows_removed: 0` with an
//! explanatory note (S3 vs S1 tension in the plan).

use std::sync::Arc;

use rusqlite::params;
use tm_controller::LinUcbBandit;
use tm_episodic::{RecentStore, TraceStore};
use tm_graph::GraphStore;
use tm_ingest::{Propagator, PropagatorConfig};
use tm_reflect::ReflexionStore;
use tm_types::{Entity, EntityType, RecentCapture};
use tm_vector::VectorStore;

fn count_rows(conn: &rusqlite::Connection, sql: &str, args: &[&dyn rusqlite::ToSql]) -> i64 {
    conn.query_row(sql, args, |r| r.get::<_, i64>(0)).unwrap_or(0)
}

#[test]
fn forget_removes_all_derived_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("memory.db");

    // ─── Set up the graph store ─────────────────────────────────────
    let graph = Arc::new(GraphStore::open(db_path.to_str().unwrap()).expect("open graph"));

    let entity = Entity::new("integration-test", EntityType::Concept, 0.9);
    let memory_id = entity.id;
    graph.upsert_entity(&entity).expect("upsert entity");

    // Add a hand-written vector row so the vector-side delete has
    // something to remove. Use the graph's connection so both layers see
    // the same DB file.
    let skg_id = graph.skg_id_for(memory_id).expect("skg id resolved");
    graph
        .raw_connection()
        .execute(
            "INSERT OR REPLACE INTO kg_vectors (entity_id, vector, dimension) VALUES (?1, X'00', 384)",
            params![skg_id],
        )
        .expect("seed kg_vectors");

    // Access log row keyed by the uuid.
    graph
        .raw_connection()
        .execute(
            "INSERT INTO access_log (entity_id, event_type) VALUES (?1, 'read')",
            params![memory_id.to_string()],
        )
        .expect("seed access_log");

    // Retrieval feedback row keyed by the uuid.
    graph
        .raw_connection()
        .execute(
            "INSERT INTO retrieval_feedback (entity_id, retrieved, succeeded) VALUES (?1, 1, 1)",
            params![memory_id.to_string()],
        )
        .expect("seed retrieval_feedback");

    // ─── Set up the recent-ring, trace, reflection, vector stores ───
    let recent =
        Arc::new(RecentStore::open_with_capacity(dir.path().join("recent.jsonl"), 10).unwrap());
    // The recent store keys by content hash; use the memory id string so
    // the propagate can find and drop it.
    recent
        .append(&RecentCapture::new("clipboard", memory_id.to_string(), "hello"))
        .unwrap();
    recent
        .append(&RecentCapture::new("clipboard", "unrelated", "world"))
        .unwrap();

    let trace = Arc::new(TraceStore::open(dir.path().join("traces.jsonl")).unwrap());
    let reflection =
        Arc::new(ReflexionStore::open(dir.path().join("reflexions.jsonl")).unwrap());
    let vector = Arc::new(
        VectorStore::open_path(db_path.clone()).expect("open vector store on shared db"),
    );
    let linucb = Arc::new(LinUcbBandit::new());

    // ─── Sanity: everything is present pre-forget ───────────────────
    let conn = graph.raw_connection();
    assert!(count_rows(conn, "SELECT COUNT(*) FROM kg_vectors WHERE entity_id = ?1", &[&skg_id]) >= 1);
    assert!(
        count_rows(conn, "SELECT COUNT(*) FROM access_log WHERE entity_id = ?1", &[&memory_id.to_string()])
            >= 1
    );
    assert_eq!(recent.read_all().unwrap().len(), 2);

    // ─── Forget ──────────────────────────────────────────────────────
    let propagator = Propagator::new()
        .with_graph(graph.clone())
        .with_vector(vector)
        .with_recent(recent.clone())
        .with_trace(trace)
        .with_reflection(reflection)
        .with_linucb(linucb)
        .with_config(PropagatorConfig::default());

    let reports = propagator.forget(memory_id).expect("forget");

    // ─── Assertions: no orphans remain ──────────────────────────────
    // The graph should have zero rows for this memory across every table.
    let vec_rows = count_rows(
        conn,
        "SELECT COUNT(*) FROM kg_vectors WHERE entity_id = ?1",
        &[&skg_id],
    );
    assert_eq!(vec_rows, 0, "kg_vectors row survived forget");

    let access_rows = count_rows(
        conn,
        "SELECT COUNT(*) FROM access_log WHERE entity_id = ?1",
        &[&memory_id.to_string()],
    );
    assert_eq!(access_rows, 0, "access_log row survived forget");

    let feedback_rows = count_rows(
        conn,
        "SELECT COUNT(*) FROM retrieval_feedback WHERE entity_id = ?1",
        &[&memory_id.to_string()],
    );
    assert_eq!(feedback_rows, 0, "retrieval_feedback row survived forget");

    let entity_rows = count_rows(
        conn,
        "SELECT COUNT(*) FROM kg_entities WHERE id = ?1",
        &[&skg_id],
    );
    assert_eq!(entity_rows, 0, "kg_entities row survived forget");

    // Recent ring keeps the unrelated row and drops the target one.
    let after_recent = recent.read_all().unwrap();
    assert_eq!(after_recent.len(), 1);
    assert_eq!(after_recent[0].content_hash, "unrelated");

    // Reports include every layer we wired.
    let crates: Vec<&str> = reports.iter().map(|r| r.crate_name.as_str()).collect();
    assert!(crates.contains(&"tm-episodic:recent"));
    assert!(crates.contains(&"tm-episodic:trace"));
    assert!(crates.contains(&"tm-reflect"));
    assert!(crates.contains(&"tm-vector"));
    assert!(crates.contains(&"tm-controller:linucb"));
    assert!(crates.contains(&"tm-graph"));

    // The trace layer intentionally preserved its rows.
    let trace_report = reports
        .iter()
        .find(|r| r.crate_name == "tm-episodic:trace")
        .unwrap();
    assert_eq!(trace_report.rows_removed, 0);
    assert!(trace_report.note.is_some());

    // The graph layer removed at least entity + access + feedback rows.
    // kg_vectors is cleaned by tm-vector first (the propagator runs vector
    // before graph), so graph's own delete count excludes that row.
    let graph_report = reports
        .iter()
        .find(|r| r.crate_name == "tm-graph")
        .unwrap();
    assert!(
        graph_report.rows_removed >= 3,
        "graph removed only {} rows",
        graph_report.rows_removed
    );

    // Cross-crate total should include the vector row too.
    let vector_report = reports
        .iter()
        .find(|r| r.crate_name == "tm-vector")
        .unwrap();
    assert!(vector_report.rows_removed >= 1, "vector removed 0 rows");

    // ─── Idempotency: a second forget returns zero everywhere ───────
    let second = propagator.forget(memory_id).expect("second forget");
    for report in &second {
        assert_eq!(
            report.rows_removed, 0,
            "second forget for {} removed {} rows",
            report.crate_name, report.rows_removed
        );
    }
}
