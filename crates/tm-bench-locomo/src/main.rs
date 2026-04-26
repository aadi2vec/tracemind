//! CLI entry point for the LoCoMo harness.
//!
//! Wires dataset load + runner + scoring + report emit. Intended for CI use:
//!
//! ```bash
//! tm-bench-locomo \
//!   --dataset fixtures/locomo-smoke.json \
//!   --runner echo \
//!   --output target/locomo-report.json \
//!   --gate-against baselines/locomo-last.json \
//!   --tolerance 0.5
//! ```
//!
//! Exits non-zero when the regression gate fails. Real TraceMind runner
//! (ingest + retrieval + answer) is wired in the follow-up PR.

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use tm_bench_locomo::{
    dataset::LocomoDataset,
    report::{regression_gate, BenchmarkReport, QuestionOutcome},
    runner::{EchoRunner, LocomoRunner, NullRunner, RunnerContext},
    scoring::{best_exact_match, best_f1},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunnerKind {
    /// Echoes the first reference answer — F1=1.0, smoke-tests the harness.
    Echo,
    /// Returns empty strings — F1=0.0, smoke-tests the gate failure path.
    Null,
    /// Real TraceMind stack: ingest + retrieval + extractive synthesis.
    /// Requires the `tracemind` feature. Uses hash embeddings by default
    /// for CI determinism — pass --real-embeddings to flip to BGE.
    #[cfg(feature = "tracemind")]
    Tracemind,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "LoCoMo benchmark harness for TraceMind")]
struct Args {
    /// Path to the LoCoMo dataset JSON.
    #[arg(long)]
    dataset: PathBuf,

    /// Which runner to benchmark.
    #[arg(long, value_enum, default_value_t = RunnerKind::Echo)]
    runner: RunnerKind,

    /// Where to write the JSON report.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Optional previous report to gate against.
    #[arg(long)]
    gate_against: Option<PathBuf>,

    /// Max tolerated F1 drop in points (default 0.5).
    #[arg(long, default_value_t = 0.5)]
    tolerance: f32,

    /// Omit per-question outcomes from the emitted report.
    #[arg(long, default_value_t = false)]
    summary_only: bool,

    /// For the tracemind runner: use real BGE embeddings instead of the
    /// deterministic hash embedder. Slower, requires model download.
    #[cfg(feature = "tracemind")]
    #[arg(long, default_value_t = false)]
    real_embeddings: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let dataset = LocomoDataset::load_from_path(&args.dataset)?;
    tracing::info!(
        samples = dataset.samples.len(),
        questions = dataset.total_questions(),
        turns = dataset.total_turns(),
        "loaded LoCoMo dataset"
    );

    let report = match args.runner {
        RunnerKind::Echo => run(EchoRunner, &dataset).await?,
        RunnerKind::Null => run(NullRunner, &dataset).await?,
        #[cfg(feature = "tracemind")]
        RunnerKind::Tracemind => {
            let cfg = tm_bench_locomo::TraceMindConfig {
                hash_embed: !args.real_embeddings,
                ..Default::default()
            };
            run(tm_bench_locomo::TraceMindRunner::new(cfg), &dataset).await?
        }
    };

    let report = if args.summary_only { report.summarized() } else { report };
    println!("{}", report.one_line());

    if let Some(path) = &args.output {
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
                    prev = prev.overall_f1,
                    current = report.overall_f1,
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

async fn run<R: LocomoRunner>(
    mut runner: R,
    dataset: &LocomoDataset,
) -> anyhow::Result<BenchmarkReport> {
    let started = Instant::now();
    let mut outcomes: Vec<QuestionOutcome> = Vec::new();

    for sample in &dataset.samples {
        let ctx = RunnerContext { sample };
        if let Err(err) = runner.ingest_sample(&ctx).await {
            tracing::error!(sample = %sample.sample_id, %err, "ingest failed");
            for q in &sample.questions {
                outcomes.push(QuestionOutcome {
                    sample_id: sample.sample_id.clone(),
                    question_id: q.id.clone(),
                    category: q.category,
                    question: q.question.clone(),
                    prediction: String::new(),
                    references: q.answers.clone(),
                    f1: 0.0,
                    exact_match: 0.0,
                    latency_ms: 0,
                    error: Some(format!("ingest: {err}")),
                });
            }
            continue;
        }

        for q in &sample.questions {
            let t0 = Instant::now();
            let (prediction, error) = match runner.answer(&ctx, q).await {
                Ok(p) => (p, None),
                Err(e) => (String::new(), Some(e)),
            };
            let latency_ms = t0.elapsed().as_millis() as u64;
            let f1 = best_f1(&prediction, &q.answers).0;
            let em = best_exact_match(&prediction, &q.answers).0;
            outcomes.push(QuestionOutcome {
                sample_id: sample.sample_id.clone(),
                question_id: q.id.clone(),
                category: q.category,
                question: q.question.clone(),
                prediction,
                references: q.answers.clone(),
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
        dataset,
        outcomes,
        wall,
    ))
}
