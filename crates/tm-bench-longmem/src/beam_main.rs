//! CLI entry point for the BEAM contradiction stress-test harness.
//!
//! ```bash
//! tm-bench-beam \
//!   --dataset crates/tm-bench-longmem/fixtures/beam-mini.json \
//!   --runner mock \
//!   --output target/beam-report.json
//! ```
//!
//! Exits 0 on success. Use `--runner mock` for CI smoke-tests.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use tm_bench_longmem::{
    beam::{BeamDataset, BeamResult},
    compute_em, compute_token_f1,
    runner::{BeamRunner, MockRunner},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunnerKind {
    /// Returns the last ingested text — smoke-tests harness plumbing.
    Mock,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "BEAM contradiction harness for TraceMind")]
struct Args {
    /// Path to the BEAM dataset JSON (array of BeamCase).
    /// If omitted, the built-in 5-case mini fixture is used.
    #[arg(long)]
    dataset: Option<PathBuf>,

    /// Runner to benchmark.
    #[arg(long, value_enum, default_value_t = RunnerKind::Mock)]
    runner: RunnerKind,

    /// Where to write the JSON report. Omit to skip file output.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Suppress per-case rows; print summary only.
    #[arg(long, default_value_t = false)]
    summary_only: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Load dataset.
    let dataset = match &args.dataset {
        Some(path) => BeamDataset::from_json(path)?,
        None => {
            tracing::info!("no --dataset provided; using built-in mini fixture");
            BeamDataset::mini_fixture()
        }
    };
    tracing::info!(cases = dataset.cases.len(), "BEAM dataset loaded");

    // Build runner.
    let mut runner: Box<dyn BeamRunner> = match args.runner {
        RunnerKind::Mock => Box::new(MockRunner::default()),
    };
    tracing::info!(runner = runner.name(), "runner selected");

    // Run evaluation.
    let mut results: Vec<BeamResult> = Vec::with_capacity(dataset.cases.len());
    for case in &dataset.cases {
        // Step 1: ingest initial claim.
        runner.ingest(&case.initial_claim).await?;

        // Step 2: ingest contradicting claim.
        runner.ingest(&case.contradicting_claim).await?;

        // Step 3: query and time.
        let t0 = Instant::now();
        let (predicted, contradiction_surfaced) = runner.query(&case.query).await?;
        let latency_ms = t0.elapsed().as_millis() as u64;

        let token_f1 = compute_token_f1(&predicted, &case.expected_answer);
        let exact_match = compute_em(&predicted, &case.expected_answer);

        results.push(BeamResult {
            case_id: case.id.clone(),
            predicted_answer: predicted,
            contradiction_surfaced,
            exact_match,
            token_f1,
        });

        let _ = latency_ms; // available for future report field
    }

    // Aggregate.
    let total = results.len();
    let avg_f1: f64 = results.iter().map(|r| r.token_f1).sum::<f64>() / total as f64;
    let em_rate: f64 = results.iter().filter(|r| r.exact_match).count() as f64 / total as f64;
    let contradiction_rate: f64 =
        results.iter().filter(|r| r.contradiction_surfaced).count() as f64 / total as f64;

    // Print per-case table.
    if !args.summary_only {
        println!("\n{:<12} {:<8} {:<8} {:<14} {}",
            "id", "F1", "EM", "contradiction", "predicted_answer");
        println!("{}", "-".repeat(72));
        for r in &results {
            println!(
                "{:<12} {:<8.3} {:<8} {:<14} {}",
                r.case_id,
                r.token_f1,
                if r.exact_match { "✓" } else { "✗" },
                if r.contradiction_surfaced { "surfaced" } else { "not surfaced" },
                &r.predicted_answer[..r.predicted_answer.len().min(40)],
            );
        }
    }

    // Summary.
    println!("\n=== BEAM Summary ===");
    println!("Total cases          : {}", total);
    println!("Token F1             : {:.2}%", avg_f1 * 100.0);
    println!("Exact Match          : {:.2}%", em_rate * 100.0);
    println!("Contradiction rate   : {:.2}%", contradiction_rate * 100.0);

    // Write JSON report.
    if let Some(out_path) = &args.output {
        #[derive(serde::Serialize)]
        struct Report<'a> {
            runner: &'a str,
            token_f1: f64,
            exact_match_rate: f64,
            contradiction_rate: f64,
            count: usize,
            results: &'a [BeamResult],
        }
        let report = Report {
            runner: runner.name(),
            token_f1: avg_f1,
            exact_match_rate: em_rate,
            contradiction_rate,
            count: total,
            results: &results,
        };
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(out_path, &json)?;
        tracing::info!(?out_path, "BEAM report written");
    }

    Ok(())
}
