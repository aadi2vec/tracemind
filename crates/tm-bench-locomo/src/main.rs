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

    /// Reasoning Quality benchmark — force every retrieval to use this
    /// bandit arm (0=narrow, 1=medium, 2=wide, 3=deep+episodic,
    /// 4=colbert). Bypasses LinUCB. Tracemind runner only. Mutually
    /// exclusive with `--compare-arms`.
    #[cfg(feature = "tracemind")]
    #[arg(long, value_name = "ARM")]
    force_arm: Option<u8>,

    /// Reasoning Quality benchmark — run twice with two forced arms
    /// and emit a delta report (`<shallow>,<deep>`, e.g. `0,3`). Output
    /// JSON contains per-category F1/EM lift. Mutually exclusive with
    /// `--force-arm`.
    #[cfg(feature = "tracemind")]
    #[arg(long, value_name = "SHALLOW,DEEP")]
    compare_arms: Option<String>,

    /// Restrict scoring to these LoCoMo categories (comma-separated:
    /// `single_hop,multi_hop,temporal,open_domain,adversarial`). The
    /// reasoning benchmark targets `multi_hop,adversarial` — categories
    /// where the typed graph + multi-hop expansion arm should beat
    /// shallow top-K. Question filtering happens after the run, so the
    /// full dataset is still ingested.
    #[arg(long, value_name = "CATS")]
    categories: Option<String>,
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

    // Compare mode short-circuits the normal single-run flow. Runs
    // the tracemind runner twice with two forced arms, then emits a
    // per-category delta report (deep − shallow). Output JSON is the
    // delta, NOT either individual report.
    #[cfg(feature = "tracemind")]
    if let Some(spec) = &args.compare_arms {
        if matches!(args.runner, RunnerKind::Tracemind) {
            return run_compare(&dataset, &args, spec).await;
        } else {
            eprintln!("--compare-arms requires --runner tracemind");
            std::process::exit(2);
        }
    }

    let report = match args.runner {
        RunnerKind::Echo => run(EchoRunner, &dataset).await?,
        RunnerKind::Null => run(NullRunner, &dataset).await?,
        #[cfg(feature = "tracemind")]
        RunnerKind::Tracemind => {
            let cfg = tm_bench_locomo::TraceMindConfig {
                hash_embed: !args.real_embeddings,
                force_arm: args.force_arm,
                ..Default::default()
            };
            run(tm_bench_locomo::TraceMindRunner::new(cfg), &dataset).await?
        }
    };

    let report = filter_categories(report, &dataset, args.categories.as_deref());
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

/// Restrict a report to only the categories named in `spec` (comma-
/// separated). Drops outcomes outside the set and rebuilds the report
/// so per-category breakdown and overall F1/EM reflect the filter.
/// Returns the original report when `spec` is None or empty.
fn filter_categories(
    report: BenchmarkReport,
    dataset: &LocomoDataset,
    spec: Option<&str>,
) -> BenchmarkReport {
    let Some(spec) = spec else { return report };
    let keep: std::collections::HashSet<String> = spec
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if keep.is_empty() {
        return report;
    }
    let outcomes: Vec<QuestionOutcome> = report
        .outcomes
        .into_iter()
        .filter(|o| keep.contains(o.category.as_str()))
        .collect();
    let runner = format!("{} (filtered: {})", report.runner, spec);
    BenchmarkReport::from_outcomes(runner, dataset, outcomes, report.wall_seconds)
}

#[cfg(feature = "tracemind")]
async fn run_compare(
    dataset: &LocomoDataset,
    args: &Args,
    spec: &str,
) -> anyhow::Result<()> {
    let parts: Vec<u8> = spec
        .split(',')
        .map(|s| s.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("--compare-arms expects <shallow>,<deep>: {e}"))?;
    if parts.len() != 2 {
        anyhow::bail!("--compare-arms expects exactly two arms, got {}", parts.len());
    }
    let (shallow, deep) = (parts[0], parts[1]);
    tracing::info!(shallow, deep, "compare-arms: starting");

    let make_runner = |arm: u8| {
        let cfg = tm_bench_locomo::TraceMindConfig {
            hash_embed: !args.real_embeddings,
            force_arm: Some(arm),
            ..Default::default()
        };
        tm_bench_locomo::TraceMindRunner::new(cfg)
    };

    let report_shallow = filter_categories(
        run(make_runner(shallow), dataset).await?,
        dataset,
        args.categories.as_deref(),
    );
    let report_deep = filter_categories(
        run(make_runner(deep), dataset).await?,
        dataset,
        args.categories.as_deref(),
    );

    // Per-category lift (deep − shallow).
    let mut by_category = serde_json::Map::new();
    for (name, deep_cat) in &report_deep.by_category {
        let shallow_cat = report_shallow.by_category.get(name);
        let shallow_f1 = shallow_cat.map(|c| c.f1).unwrap_or(0.0);
        let shallow_em = shallow_cat.map(|c| c.exact_match).unwrap_or(0.0);
        by_category.insert(
            name.clone(),
            serde_json::json!({
                "n": deep_cat.count,
                "shallow_f1": shallow_f1,
                "deep_f1": deep_cat.f1,
                "delta_f1": deep_cat.f1 - shallow_f1,
                "shallow_em": shallow_em,
                "deep_em": deep_cat.exact_match,
                "delta_em": deep_cat.exact_match - shallow_em,
            }),
        );
    }

    let summary = serde_json::json!({
        "shallow_arm": shallow,
        "deep_arm": deep,
        "categories_filter": args.categories,
        "overall": {
            "shallow_f1": report_shallow.overall_f1,
            "deep_f1": report_deep.overall_f1,
            "delta_f1": report_deep.overall_f1 - report_shallow.overall_f1,
            "shallow_em": report_shallow.overall_exact_match,
            "deep_em": report_deep.overall_exact_match,
            "delta_em": report_deep.overall_exact_match - report_shallow.overall_exact_match,
            "n_questions": report_deep.total_questions,
        },
        "by_category": by_category,
        "shallow_runner": report_shallow.runner,
        "deep_runner": report_deep.runner,
    });

    println!(
        "compare: shallow(arm{})={:.2} F1 / {:.2} EM  →  deep(arm{})={:.2} F1 / {:.2} EM  Δ={:+.2} F1",
        shallow,
        report_shallow.overall_f1,
        report_shallow.overall_exact_match,
        deep,
        report_deep.overall_f1,
        report_deep.overall_exact_match,
        report_deep.overall_f1 - report_shallow.overall_f1,
    );

    if let Some(path) = &args.output {
        let json = serde_json::to_string_pretty(&summary)?;
        std::fs::write(path, json)?;
        tracing::info!(output = %path.display(), "wrote compare report");
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
