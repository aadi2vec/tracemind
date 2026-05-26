//! CLI for the cross-session persistence benchmark.
//!
//! ```bash
//! # Honest mode (shared DB across all pairs, held-out test split):
//! tm-bench-memory \
//!   --dataset crates/tm-bench-memory/fixtures/persistence-real.json \
//!   --runner tracemind \
//!   --split test \
//!   --output crates/tm-bench-memory/baselines/w3-real.json
//!
//! # Legacy per-pair isolation (back-compat smoke):
//! tm-bench-memory --dataset ... --runner tracemind --per-pair-isolation
//! ```
//!
//! Adds the regression gate (same shape as `tm-bench-locomo`) so CI can
//! fail PRs that drop persistence by more than `--tolerance` points.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use tm_bench_memory::{
    dataset::{PersistenceDataset, Split},
    report::{regression_gate, BenchmarkReport, PairOutcome},
    runner::{EchoRunner, NullRunner, PersistenceRunner, RunnerContext},
    scoring::{best_exact_match, best_f1},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunnerKind {
    Echo,
    Null,
    #[cfg(feature = "tracemind")]
    Tracemind,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SplitArg {
    Tune,
    Test,
    All,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Cross-session persistence benchmark (W-3)")]
struct Args {
    #[arg(long)]
    dataset: PathBuf,

    #[arg(long, value_enum, default_value_t = RunnerKind::Echo)]
    runner: RunnerKind,

    /// Which split to score. Headline numbers should always be reported
    /// on `test` — `tune` is for extractor pattern engineering only.
    #[arg(long, value_enum, default_value_t = SplitArg::Test)]
    split: SplitArg,

    #[arg(long)]
    output: Option<PathBuf>,

    #[arg(long)]
    gate_against: Option<PathBuf>,

    #[arg(long, default_value_t = 1.0)]
    tolerance: f32,

    #[arg(long, default_value_t = false)]
    summary_only: bool,

    /// Run in the legacy per-pair-isolation mode (one fresh tempdir per
    /// pair, no cross-pair contamination, no distractors loaded into
    /// the DB). Use only for back-compat smoke tests; the headline
    /// number must come from shared-DB mode.
    #[arg(long, default_value_t = false)]
    per_pair_isolation: bool,

    /// Use real BGE embeddings (slower, requires model download).
    #[cfg(feature = "tracemind")]
    #[arg(long, default_value_t = false)]
    real_embeddings: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let full = PersistenceDataset::load_from_path(&args.dataset)?;
    tracing::info!(
        total_pairs = full.pairs.len(),
        noise = full.noise_corpus.len(),
        "loaded persistence dataset"
    );

    // We always ingest the FULL dataset (every pair × store+distractors
    // + noise) into the shared DB so retrieval sees the realistic
    // cross-pair contamination. We only *score* on the chosen split.
    let scoring_set = match args.split {
        SplitArg::Tune => full.filter_split(Split::Tune),
        SplitArg::Test => full.filter_split(Split::Test),
        SplitArg::All => full.clone(),
    };
    tracing::info!(
        scoring_pairs = scoring_set.pairs.len(),
        split = ?args.split,
        shared_db_size = full.shared_db_size(),
        "scoring split"
    );

    let report = match args.runner {
        RunnerKind::Echo => {
            run(EchoRunner, &full, &scoring_set, args.per_pair_isolation).await?
        }
        RunnerKind::Null => {
            run(NullRunner, &full, &scoring_set, args.per_pair_isolation).await?
        }
        #[cfg(feature = "tracemind")]
        RunnerKind::Tracemind => {
            let cfg = tm_bench_memory::TraceMindConfig {
                hash_embed: !args.real_embeddings,
                ..Default::default()
            };
            run(
                tm_bench_memory::TraceMindPersistenceRunner::new(cfg),
                &full,
                &scoring_set,
                args.per_pair_isolation,
            )
            .await?
        }
    };

    let report = if args.summary_only { report.summarized() } else { report };
    println!("{}", report.one_line());
    for (cat, br) in &report.by_category {
        println!("  {:<25} n={:<3} F1={:.2} EM={:.2}", cat, br.count, br.f1, br.exact_match);
    }

    if let Some(path) = &args.output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(path, json)?;
        tracing::info!(output = %path.display(), "wrote report");
    }

    if let Some(gate_path) = &args.gate_against {
        let raw = std::fs::read_to_string(gate_path)?;
        let prev: BenchmarkReport = serde_json::from_str(&raw)?;
        match regression_gate(&prev, &report, args.tolerance) {
            Ok(()) => {
                tracing::info!(
                    prev = prev.persistence_score,
                    current = report.persistence_score,
                    "gate: OK"
                );
            }
            Err(msg) => {
                eprintln!("gate FAILED: {msg}");
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

async fn run<R: PersistenceRunner>(
    mut runner: R,
    full: &PersistenceDataset,
    scoring: &PersistenceDataset,
    per_pair_isolation: bool,
) -> anyhow::Result<BenchmarkReport> {
    let started = Instant::now();
    let mut outcomes: Vec<PairOutcome> = Vec::new();

    if !per_pair_isolation {
        // Honest mode: bulk-ingest the WHOLE dataset once, then answer
        // each scoring-split pair against the shared DB.
        if let Err(err) = runner.bulk_ingest(full).await {
            anyhow::bail!("bulk_ingest failed: {err}");
        }
        for pair in &scoring.pairs {
            let ctx = RunnerContext { pair };
            let t0 = Instant::now();
            let (prediction, error) = match runner.query(&ctx).await {
                Ok(p) => (p, None),
                Err(e) => (String::new(), Some(format!("query: {e}"))),
            };
            let latency_ms = t0.elapsed().as_millis() as u64;
            let f1 = best_f1(&prediction, &pair.answers).0;
            let em = best_exact_match(&prediction, &pair.answers).0;
            outcomes.push(PairOutcome {
                pair_id: pair.id.clone(),
                category: pair.category,
                query: pair.query.clone(),
                prediction,
                references: pair.answers.clone(),
                f1,
                exact_match: em,
                latency_ms,
                error,
            });
        }
    } else {
        // Legacy mode: per-pair isolation.
        for pair in &scoring.pairs {
            let ctx = RunnerContext { pair };
            let t0 = Instant::now();
            if let Err(err) = runner.session_a_store(&ctx).await {
                tracing::error!(pair = %pair.id, %err, "session_a failed");
                outcomes.push(PairOutcome {
                    pair_id: pair.id.clone(),
                    category: pair.category,
                    query: pair.query.clone(),
                    prediction: String::new(),
                    references: pair.answers.clone(),
                    f1: 0.0,
                    exact_match: 0.0,
                    latency_ms: 0,
                    error: Some(format!("session_a: {err}")),
                });
                continue;
            }
            let (prediction, error) = match runner.session_b_query(&ctx).await {
                Ok(p) => (p, None),
                Err(e) => (String::new(), Some(format!("session_b: {e}"))),
            };
            let latency_ms = t0.elapsed().as_millis() as u64;
            let f1 = best_f1(&prediction, &pair.answers).0;
            let em = best_exact_match(&prediction, &pair.answers).0;
            outcomes.push(PairOutcome {
                pair_id: pair.id.clone(),
                category: pair.category,
                query: pair.query.clone(),
                prediction,
                references: pair.answers.clone(),
                f1,
                exact_match: em,
                latency_ms,
                error,
            });
        }
    }

    let wall = started.elapsed().as_secs_f64();
    Ok(BenchmarkReport::from_outcomes(
        runner.name(),
        scoring,
        outcomes,
        wall,
    ))
}
