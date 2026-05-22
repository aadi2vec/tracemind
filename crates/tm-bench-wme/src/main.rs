//! WME-8..10 — Working Memory Engine guardrail bench.
//!
//! Three gates, each emit JSON + non-zero exit on failure:
//!
//! * **useful-rate**   ≥ 0.60 on a deterministic acceptance policy
//!   (cards where `relevance > 0.5` are "accepted"). This is the
//!   falsifiability test for the P4 anticipatory claim.
//! * **anti-spam cap** ≤ 8 cards/hour even under a 1000-event flood
//!   (WME-9 regression).
//! * **per-tick p95**  < 50 ms across 1000 ticks (WME-10 latency).
//!
//! Run:
//!
//! ```bash
//! cargo run --release -p tm-bench-wme
//! ```

use std::time::Instant;

use chrono::Duration;
use serde::Serialize;
use tempfile::TempDir;
use tm_reflect::{
    proposals_from_sources, ActivityContext, CandidateSources, CommitmentInput, FeedbackKind,
    OutlierInput, SimilarMemoryInput, WmeConfig, WorkingMemoryEngine, MAX_CARDS_PER_HOUR,
};

const N_EVENTS: usize = 1000;
const USEFUL_RATE_GATE: f32 = 0.60;
const LATENCY_P95_GATE_MS: f32 = 50.0;

#[derive(Debug, Serialize)]
struct BenchReport {
    n_ticks: usize,
    cards_pushed: usize,
    cards_useful: usize,
    useful_rate: f32,
    cards_in_first_hour: usize,
    cards_in_first_hour_cap: usize,
    p50_ms: f32,
    p95_ms: f32,
    p99_ms: f32,
    gate_useful_rate: bool,
    gate_anti_spam: bool,
    gate_latency_p95: bool,
    all_gates_pass: bool,
}

fn synthetic_commitments(n: usize) -> Vec<CommitmentInput> {
    (0..n)
        .map(|i| CommitmentInput {
            commitment_id: format!("commit-{i}"),
            statement: format!("Commitment number {i}"),
            overdue: i % 5 == 0,
            age: Duration::hours((i % 48) as i64),
            statement_embedding: None,
        })
        .collect()
}

fn synthetic_memories(n: usize, relevance_high_ratio: f32) -> Vec<SimilarMemoryInput> {
    (0..n)
        .map(|i| {
            // Half the memories are "highly relevant" (>0.5), the rest are noise.
            let relevance = if (i as f32 / n as f32) < relevance_high_ratio {
                0.85
            } else {
                0.25
            };
            SimilarMemoryInput {
                memory_id: format!("mem-{i}"),
                statement: format!("Memory chunk {i}"),
                relevance,
                age: Duration::hours((i % 72) as i64),
                statement_embedding: None,
                seen_this_session: false,
            }
        })
        .collect()
}

fn synthetic_outliers(n: usize) -> Vec<OutlierInput> {
    (0..n)
        .map(|i| OutlierInput {
            cluster_id: format!("outlier-{i}"),
            statement: format!("Novel topic {i}"),
            age: Duration::minutes((i % 30) as i64),
        })
        .collect()
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f32 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn main() {
    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("wme-bench.db");

    // High max_cards so the latency loop isn't artificially throttled
    // — the hourly cap below is the *real* anti-spam test.
    let cfg = WmeConfig {
        max_cards: 32,
        ..WmeConfig::default()
    };
    let engine = WorkingMemoryEngine::open_with(&db, cfg).expect("engine open");

    // Drive 1000 ticks, each one carrying a fresh batch of candidate
    // proposals. Half of them are designed to be "useful" (relevance > 0.5).
    let mut latencies_ns: Vec<u128> = Vec::with_capacity(N_EVENTS);
    let mut total_pushed: usize = 0;
    let mut useful_pushed: usize = 0;

    let topic: Vec<f32> = Vec::new(); // empty topic → uses proposal.relevance directly
    let _ = WorkingMemoryEngine::topic_vector(&ActivityContext {
        centroids: Vec::new(),
        active_entities: Vec::new(),
        recent_outliers: Vec::new(),
    });

    for tick in 0..N_EVENTS {
        let commits = synthetic_commitments(1);
        let mems = synthetic_memories(4, 0.5);
        let outs = synthetic_outliers(1);

        // Stamp uniqueness per-tick so cooldowns don't drop everything
        // (the cooldown is a real product feature; here we cycle target ids
        // so the bench exercises scoring + spam-cap, not cooldown).
        let commits: Vec<_> = commits
            .into_iter()
            .map(|mut c| {
                c.commitment_id = format!("{}-t{tick}", c.commitment_id);
                c
            })
            .collect();
        let mems: Vec<_> = mems
            .into_iter()
            .map(|mut m| {
                m.memory_id = format!("{}-t{tick}", m.memory_id);
                m
            })
            .collect();
        let outs: Vec<_> = outs
            .into_iter()
            .map(|mut o| {
                o.cluster_id = format!("{}-t{tick}", o.cluster_id);
                o
            })
            .collect();

        let sources = CandidateSources {
            open_commitments: &commits,
            contradictions: &[],
            similar_memories: &mems,
            bridges: &[],
            outliers: &outs,
        };
        let proposals = proposals_from_sources(sources);

        let start = Instant::now();
        let cards = engine
            .rank_and_push(proposals, &topic)
            .expect("rank_and_push");
        latencies_ns.push(start.elapsed().as_nanos());

        for card in &cards {
            total_pushed += 1;
            if card.signals.relevance > 0.5 {
                useful_pushed += 1;
                let _ = engine.record_feedback(card.id, card.kind, FeedbackKind::UsefulNow);
            } else {
                let _ = engine.record_feedback(card.id, card.kind, FeedbackKind::NotUsefulNow);
            }
        }
    }

    // ── Gates ───────────────────────────────────────────────────────
    let useful_rate = if total_pushed == 0 {
        0.0
    } else {
        useful_pushed as f32 / total_pushed as f32
    };
    let cards_in_first_hour = engine.cards_in_last_hour().unwrap_or(0);
    let mut lat_ms: Vec<f32> = latencies_ns
        .iter()
        .map(|n| *n as f32 / 1_000_000.0)
        .collect();
    lat_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&lat_ms, 0.50);
    let p95 = percentile(&lat_ms, 0.95);
    let p99 = percentile(&lat_ms, 0.99);

    let gate_useful_rate = useful_rate >= USEFUL_RATE_GATE;
    let gate_anti_spam = cards_in_first_hour <= MAX_CARDS_PER_HOUR;
    let gate_latency_p95 = p95 < LATENCY_P95_GATE_MS;
    let all_gates_pass = gate_useful_rate && gate_anti_spam && gate_latency_p95;

    let report = BenchReport {
        n_ticks: N_EVENTS,
        cards_pushed: total_pushed,
        cards_useful: useful_pushed,
        useful_rate,
        cards_in_first_hour,
        cards_in_first_hour_cap: MAX_CARDS_PER_HOUR,
        p50_ms: p50,
        p95_ms: p95,
        p99_ms: p99,
        gate_useful_rate,
        gate_anti_spam,
        gate_latency_p95,
        all_gates_pass,
    };

    println!("{}", serde_json::to_string_pretty(&report).unwrap());

    if !all_gates_pass {
        eprintln!("WME bench gates failed:");
        if !gate_useful_rate {
            eprintln!(
                "  useful_rate {:.3} < {:.3}",
                useful_rate, USEFUL_RATE_GATE
            );
        }
        if !gate_anti_spam {
            eprintln!(
                "  cards_in_first_hour {} > {}",
                cards_in_first_hour, MAX_CARDS_PER_HOUR
            );
        }
        if !gate_latency_p95 {
            eprintln!("  p95 {:.2} ms >= {} ms", p95, LATENCY_P95_GATE_MS);
        }
        std::process::exit(1);
    }
}
