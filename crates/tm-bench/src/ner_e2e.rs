//! TM-NLP-004 end-to-end ingest + query test.
//!
//! Wires the whole pipeline together against a fresh temp DB:
//!
//!   fixture (30 sentences) → IngestPipeline (+ GlinerExtractor)
//!       → GraphStore (SQLite) → VectorStore (SQLite)
//!       → RetrievalEngine → natural-language queries
//!
//! For each query we assert that the expected entity surfaces in the top-k
//! returned entities. Passes iff ≥ 80% of the query probes hit.
//!
//! This is the test the user asked for: "run a comprehensive e2e test to
//! test this ability". Exit code: 0 on pass, 1 on fail, 2 on GLiNER offline.

use std::path::PathBuf;

use tempfile::tempdir;
use tm_episodic::TraceStore;
use tm_ingest::{GlinerExtractor, IngestPipeline};
use tm_retrieval::RetrievalEngine;
use tm_types::Entity;
use uuid::Uuid;

#[derive(Debug)]
struct Probe {
    query: &'static str,
    /// Any of these substrings appearing in a returned entity name counts as a hit.
    expected_any: &'static [&'static str],
}

const PROBES: &[Probe] = &[
    Probe {
        query: "who works at Anthropic",
        expected_any: &["Aaditya", "Dario", "Daniela"],
    },
    Probe {
        query: "what is TraceMind built with",
        expected_any: &["TraceMind", "Rust"],
    },
    Probe {
        query: "tell me about San Francisco events",
        expected_any: &["San Francisco"],
    },
    Probe {
        query: "conferences and keynotes",
        expected_any: &["WWDC", "Google I/O", "NeurIPS", "DevDay"],
    },
    Probe {
        query: "who founded Anthropic",
        expected_any: &["Dario", "Daniela", "Anthropic"],
    },
    Probe {
        query: "tools for RAG pipelines",
        expected_any: &["Pinecone", "Weaviate", "LangChain", "RAG"],
    },
    Probe {
        query: "Rust systems programming",
        expected_any: &["Rust", "Mozilla", "TraceMind"],
    },
    Probe {
        query: "who is Sam Altman",
        expected_any: &["Sam Altman", "OpenAI"],
    },
    Probe {
        query: "Apple hardware announcements",
        expected_any: &["Apple", "M4", "MacBook", "WWDC", "Cupertino"],
    },
    Probe {
        query: "Tesla and Cybertruck",
        expected_any: &["Tesla", "Cybertruck", "Austin", "Texas"],
    },
];

fn main() {
    println!("\n═══════════════════════════════════════════════════════════════");
    println!("  TM-NLP-004 — E2E ingest + query test");
    println!("═══════════════════════════════════════════════════════════════");

    // Spin up fresh temp DB — this test must not touch the user's data dir.
    let tmp = tempdir().expect("tempdir");
    let db_path = tmp.path().join("memory.db").to_string_lossy().to_string();
    let trace_path = tmp
        .path()
        .join("traces.jsonl")
        .to_string_lossy()
        .to_string();

    println!("  temp db: {db_path}");

    // Resolve + load fixture.
    let fixture_path = resolve_fixture();
    println!("  fixture: {}", fixture_path.display());
    let lines = std::fs::read_to_string(&fixture_path).expect("read fixture");
    let texts: Vec<String> = lines
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("parse fixture");
            v["text"].as_str().unwrap().to_string()
        })
        .collect();
    println!("  sentences: {}\n", texts.len());

    // Build the pipeline with GLiNER if available.
    let mut pipeline =
        IngestPipeline::open(&db_path, false).expect("open ingest pipeline");
    let gliner_on = match GlinerExtractor::auto_download_default() {
        Some(gli) => {
            pipeline = pipeline.with_extractor(Box::new(gli));
            true
        }
        None => {
            eprintln!("[e2e] GLiNER unavailable — this test needs the real model.");
            std::process::exit(2);
        }
    };
    println!("  extractor: {}", if gliner_on { "gliner" } else { "heuristic" });

    let trace_store = TraceStore::open(&trace_path).expect("trace store");
    let session = Uuid::new_v4();

    // ─── Ingest every fixture sentence ───────────────────────────────────
    println!("\n  [1/3] Ingesting {} sentences…", texts.len());
    let start = std::time::Instant::now();
    let mut total_entities = 0usize;
    for (i, t) in texts.iter().enumerate() {
        let res = pipeline.ingest(t, session).expect("ingest");
        total_entities += res.entities.len();
        let _ = trace_store.append(&res.trace);
        if i < 3 {
            let names: Vec<&str> = res.entities.iter().map(|e| e.name.as_str()).collect();
            println!("      [{i}] → {} entities: {names:?}", res.entities.len());
        }
    }
    let ingest_ms = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "      ingested {} sentences, {} entities total ({:.0}ms, {:.1}ms/doc)",
        texts.len(),
        total_entities,
        ingest_ms,
        ingest_ms / texts.len() as f64
    );

    // ─── Open retrieval engine on the same DB ────────────────────────────
    println!("\n  [2/3] Opening retrieval engine…");
    let mut engine = RetrievalEngine::open(&db_path, &trace_path, false)
        .expect("open retrieval engine");

    // ─── Probe queries ───────────────────────────────────────────────────
    println!("\n  [3/3] Running {} probe queries…\n", PROBES.len());
    let mut hits = 0usize;
    let mut misses: Vec<String> = Vec::new();

    for p in PROBES {
        let res = match engine.query(p.query) {
            Ok(r) => r,
            Err(e) => {
                println!("    [MISS] \"{}\" → query error: {e}", p.query);
                misses.push(p.query.to_string());
                continue;
            }
        };

        let hit_any = p.expected_any.iter().any(|needle| {
            res.entities
                .iter()
                .any(|e| contains_ci(&e.name, needle))
        });

        if hit_any {
            hits += 1;
            let matched = p
                .expected_any
                .iter()
                .find(|n| res.entities.iter().any(|e| contains_ci(&e.name, n)))
                .unwrap_or(&"");
            println!(
                "    [HIT ] \"{}\" → found \"{matched}\" in top-{}",
                p.query,
                res.entities.len()
            );
        } else {
            let top: Vec<&str> = res
                .entities
                .iter()
                .take(5)
                .map(|e: &Entity| e.name.as_str())
                .collect();
            println!(
                "    [MISS] \"{}\" → expected any of {:?}, got {:?}",
                p.query, p.expected_any, top
            );
            misses.push(p.query.to_string());
        }
    }

    let rate = hits as f64 / PROBES.len() as f64;
    println!("\n═══════════════════════════════════════════════════════════════");
    println!(
        "  Result: {}/{} probes hit ({:.0}%)",
        hits,
        PROBES.len(),
        rate * 100.0
    );
    println!("═══════════════════════════════════════════════════════════════");

    if rate >= 0.8 {
        println!("  PASS — ≥ 80% of probes surfaced an expected entity.");
        std::process::exit(0);
    } else {
        println!("  FAIL — hit rate below 80%.");
        for m in &misses {
            println!("    missed: {m}");
        }
        std::process::exit(1);
    }
}

fn resolve_fixture() -> PathBuf {
    let candidates = [
        "crates/tm-bench/fixtures/ner_eval.jsonl",
        "../../crates/tm-bench/fixtures/ner_eval.jsonl",
        "fixtures/ner_eval.jsonl",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return PathBuf::from(c);
        }
    }
    panic!("ner_eval.jsonl not found");
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}
