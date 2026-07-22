//! CLI entry point for the LongMemEval harness.
//!
//! ```bash
//! tm-bench-longmem \
//!   --dataset crates/tm-bench-longmem/fixtures/longmem-mini.json \
//!   --runner mock \
//!   --output target/longmem-report.json
//! ```
//!
//! Exits 0 on success. Use `--runner mock` for CI smoke-tests (no real
//! embeddings). Real TraceMind runner requires `--features tracemind`.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use tm_bench_longmem::{
    compute_em, compute_token_f1,
    longmem::{LongMemDataset, LongMemResult},
    metrics::EvalMetrics,
    runner::{LongMemRunner, MockRunner},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunnerKind {
    /// Returns the last ingested turn — smoke-tests harness plumbing.
    Mock,
}

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "LongMemEval benchmark harness for TraceMind"
)]
struct Args {
    /// Path to the LongMemEval dataset JSON (array of LongMemCase).
    /// If omitted, the built-in 10-case mini fixture is used.
    #[arg(long)]
    dataset: Option<PathBuf>,

    /// Runner to benchmark.
    #[arg(long, value_enum, default_value_t = RunnerKind::Mock)]
    runner: RunnerKind,

    /// Where to write the JSON report. Omit to skip file output.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Suppress per-case rows; print summary table only.
    #[arg(long, default_value_t = false)]
    summary_only: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Load dataset.
    let dataset = match &args.dataset {
        Some(path) => LongMemDataset::from_json(path)?,
        None => {
            tracing::info!("no --dataset provided; using built-in mini fixture");
            LongMemDataset::mini_fixture()
        }
    };
    tracing::info!(cases = dataset.cases.len(), "dataset loaded");

    // Build runner.
    let mut runner: Box<dyn LongMemRunner> = match args.runner {
        RunnerKind::Mock => Box::new(MockRunner::default()),
    };
    tracing::info!(runner = runner.name(), "runner selected");

    // Run evaluation.
    let mut results: Vec<LongMemResult> = Vec::with_capacity(dataset.cases.len());
    for case in &dataset.cases {
        let session_id = format!("lme-{}", case.id);

        // Ingest all context turns.
        for turn in &case.context_turns {
            runner.ingest(turn, &session_id).await?;
        }

        // Query and time it.
        let t0 = Instant::now();
        let predicted = runner.query(&case.question).await?;
        let latency_ms = t0.elapsed().as_millis() as u64;

        // Score.
        let token_f1 = case
            .gold_spans
            .iter()
            .map(|g| compute_token_f1(&predicted, g))
            .fold(0.0f64, f64::max);
        let exact_match = compute_em(&predicted, &case.gold_answer);

        results.push(LongMemResult {
            case_id: case.id.clone(),
            category: case.category.clone(),
            predicted_answer: predicted,
            token_f1,
            exact_match,
            latency_ms,
        });
    }

    // Aggregate.
    let raw: Vec<(String, f64, bool)> = results
        .iter()
        .map(|r| (r.category.to_string(), r.token_f1, r.exact_match))
        .collect();
    let metrics = EvalMetrics::from_results(&raw);

    // Print per-case table.
    if !args.summary_only {
        println!("\n{:<12} {:<12} {:<8} {:<8} {}", "id", "category", "F1", "EM", "latency_ms");
        println!("{}", "-".repeat(60));
        for r in &results {
            println!(
                "{:<12} {:<12} {:<8.3} {:<8} {}",
                r.case_id,
                r.category.to_string(),
                r.token_f1,
                if r.exact_match { "✓" } else { "✗" },
                r.latency_ms,
            );
        }
    }

    // Summary table.
    println!("\n=== LongMemEval Summary ===");
    println!("Total cases : {}", metrics.count);
    println!("Token F1    : {:.2}%", metrics.token_f1 * 100.0);
    println!("Exact Match : {:.2}%", metrics.exact_match_rate * 100.0);
    println!("\nPer-category breakdown:");
    let mut cats: Vec<_> = metrics.by_category.iter().collect();
    cats.sort_by_key(|(k, _)| k.as_str());
    for (cat, cm) in &cats {
        println!(
            "  {:<14} F1={:.2}%  EM={:.2}%  n={}",
            cat,
            cm.token_f1 * 100.0,
            cm.exact_match_rate * 100.0,
            cm.count
        );
    }

    // Write JSON report.
    if let Some(out_path) = &args.output {
        #[derive(serde::Serialize)]
        struct Report<'a> {
            runner: &'a str,
            metrics: &'a EvalMetrics,
            results: &'a [LongMemResult],
        }
        let report = Report {
            runner: runner.name(),
            metrics: &metrics,
            results: &results,
        };
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out_path, &json)?;
        tracing::info!(?out_path, "report written");
    }

    Ok(())
}
