//! CLI entry point for the MCP-server benchmark.
//!
//! ```bash
//! # selection quality only (no server needed):
//! tm-bench-mcp --scenarios fixtures/core-scenarios.json --selection-only
//!
//! # full run against the real server (spawns tm-mcp):
//! tm-bench-mcp \
//!   --scenarios crates/tm-bench-mcp/fixtures/core-scenarios.json \
//!   --server target/release/tm-mcp \
//!   --output /tmp/mcp-report.json
//! ```

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::json;

use tm_bench_mcp::{
    percentile_ms, score_selection, McpServer, ScenarioSet, ToolDef, ToolSelector,
};

#[derive(Parser, Debug)]
#[command(about = "Benchmark the TraceMind MCP server surface")]
struct Args {
    /// Scenario JSON file.
    #[arg(long)]
    scenarios: PathBuf,

    /// Path to the `tm-mcp` binary. If omitted, the run is selection-only
    /// and the tool descriptions come from `--descriptions` or a built-in
    /// core set.
    #[arg(long)]
    server: Option<PathBuf>,

    /// Score tool selection only; do not spawn the server.
    #[arg(long, default_value_t = false)]
    selection_only: bool,

    /// Advertise the full advanced surface when spawning the server.
    #[arg(long, default_value_t = false)]
    advanced: bool,

    /// Minimum selection F1 to pass (CI gate). Exits non-zero below this.
    #[arg(long, default_value_t = 0.0)]
    min_f1: f32,

    /// Maximum tolerated p95 latency in ms (0 = no gate).
    #[arg(long, default_value_t = 0.0)]
    max_p95_ms: f64,

    /// Where to write the JSON report.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Optimise the tool descriptions against a training scenario set (GEPA
    /// over descriptions). `--scenarios` is then treated as the held-out
    /// set, scored before and after. Writes the tuned descriptions to
    /// `--descriptions-out` for `tm-mcp` to load.
    #[arg(long)]
    optimize_on: Option<PathBuf>,

    /// Where to write the optimised description override map.
    #[arg(long)]
    descriptions_out: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let raw = std::fs::read_to_string(&args.scenarios)
        .with_context(|| format!("reading {}", args.scenarios.display()))?;
    let set: ScenarioSet = serde_json::from_str(&raw).context("parsing scenarios")?;

    // 1. Obtain the tool descriptions the host would see. Prefer the live
    //    server (the real advertised surface); fall back to a built-in core
    //    set for selection-only runs without a binary.
    let mut server: Option<McpServer> = None;
    let tools: Vec<ToolDef> = if args.selection_only || args.server.is_none() {
        builtin_core_tools()
    } else {
        let bin = args.server.as_ref().unwrap().to_string_lossy().to_string();
        let tmp = std::env::temp_dir().join(format!("tm-bench-mcp-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).ok();
        let mut srv = McpServer::spawn(&bin, tmp.to_string_lossy().as_ref(), args.advanced)
            .context("spawning tm-mcp")?;
        srv.initialize().context("initialize handshake")?;
        let advertised = srv.list_tools().context("tools/list")?;
        server = Some(srv);
        advertised
            .into_iter()
            .map(|(name, description)| ToolDef { name, description })
            .collect()
    };

    // 1b. Optional: GEPA over descriptions. Tune on `--optimize-on`, then
    //     score `--scenarios` (held out) before and after to report the
    //     honest generalisation delta.
    let mut tools = tools;
    if let Some(train_path) = &args.optimize_on {
        let train_raw = std::fs::read_to_string(train_path)
            .with_context(|| format!("reading {}", train_path.display()))?;
        let train: ScenarioSet = serde_json::from_str(&train_raw).context("parsing train scenarios")?;

        let before = score_selection(&set, &ToolSelector::new(tools.clone())).f1;
        let opt = tm_bench_mcp::optimize_descriptions(&tools, &train, 8);
        // Apply the tuned descriptions.
        tools = tools
            .into_iter()
            .map(|t| ToolDef {
                description: opt.descriptions.get(&t.name).cloned().unwrap_or(t.description),
                name: t.name,
            })
            .collect();
        let after = score_selection(&set, &ToolSelector::new(tools.clone())).f1;

        println!("── description optimisation ──");
        println!("  train F1     {:.3} -> {:.3}  ({} edits)", opt.baseline_f1, opt.best_f1, opt.edits);
        println!("  held-out F1  {before:.3} -> {after:.3}");
        for line in &opt.history {
            println!("    {line}");
        }
        if let Some(out) = &args.descriptions_out {
            std::fs::write(out, serde_json::to_string_pretty(&opt.descriptions)?)?;
            println!("  descriptions → {}", out.display());
        }
        println!();
    }

    // 2. Selection quality (the GEPA-optimisable metric).
    let selector = ToolSelector::new(tools.clone());
    let selection = score_selection(&set, &selector);

    // 3. Latency + contract, against the live server, for scenarios that
    //    carry executable arguments.
    let mut latencies: Vec<Duration> = Vec::new();
    let mut contract_ok = 0usize;
    let mut contract_total = 0usize;
    let mut contract_failures: Vec<String> = Vec::new();
    if let Some(srv) = server.as_mut() {
        for sc in &set.scenarios {
            let (Some(tool), Some(args_val)) = (sc.gold(), sc.arguments.as_ref()) else {
                continue;
            };
            contract_total += 1;
            match srv.call_tool(tool, args_val.clone()) {
                Ok((_result, dur)) => {
                    latencies.push(dur);
                    contract_ok += 1;
                }
                Err(e) => contract_failures.push(format!("{}: {e}", sc.id)),
            }
        }
    }

    let advertised_count = tools.len();
    let p50 = percentile_ms(&latencies, 50.0);
    let p95 = percentile_ms(&latencies, 95.0);

    // 4. Report.
    println!("── MCP surface benchmark ──");
    println!("  advertised tools    {advertised_count}");
    println!(
        "  selection accuracy  {:.1}%  ({}/{})",
        selection.accuracy * 100.0,
        selection.correct,
        selection.total
    );
    println!("  selection P/R/F1    {:.2} / {:.2} / {:.2}", selection.precision, selection.recall, selection.f1);
    println!("  false triggers      {}", selection.false_triggers);
    if contract_total > 0 {
        println!("  contract ok         {contract_ok}/{contract_total}");
        println!("  latency p50 / p95   {p50:.1}ms / {p95:.1}ms");
    }
    if !selection.outcomes.iter().all(|o| o.correct) {
        println!("  misses:");
        for o in selection.outcomes.iter().filter(|o| !o.correct) {
            println!(
                "    {:<12} want={:<20} got={:<20} · {}",
                o.id,
                o.expected.as_deref().unwrap_or("none"),
                o.predicted.as_deref().unwrap_or("none"),
                o.utterance
            );
        }
    }
    for f in &contract_failures {
        println!("  contract FAIL: {f}");
    }

    if let Some(path) = &args.output {
        let report = json!({
            "advertised_tools": advertised_count,
            "selection": &selection,
            "latency_p50_ms": p50,
            "latency_p95_ms": p95,
            "contract_ok": contract_ok,
            "contract_total": contract_total,
            "contract_failures": contract_failures,
        });
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("  report → {}", path.display());
    }

    // 5. Gates.
    let mut failed = false;
    if selection.f1 < args.min_f1 {
        eprintln!("GATE FAIL: selection F1 {:.2} < {:.2}", selection.f1, args.min_f1);
        failed = true;
    }
    if args.max_p95_ms > 0.0 && p95 > args.max_p95_ms {
        eprintln!("GATE FAIL: p95 {p95:.1}ms > {:.1}ms", args.max_p95_ms);
        failed = true;
    }
    if !contract_failures.is_empty() {
        eprintln!("GATE FAIL: {} contract failures", contract_failures.len());
        failed = true;
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

/// The core-6 descriptions, used for selection-only runs without a server.
/// Kept in sync with `tm-mcp`'s CORE_TOOLS by the selection scenarios.
fn builtin_core_tools() -> Vec<ToolDef> {
    vec![
        ToolDef { name: "memory_store".into(), description: "Ingest text into memory, remembering durable facts, decisions, and notes; extracts entities and triples so they are recallable later.".into() },
        ToolDef { name: "memory_query".into(), description: "Answer a question by recalling and retrieving relevant stored memories from the past.".into() },
        ToolDef { name: "memory_contradict".into(), description: "Check whether a new statement clashes with, conflicts with, or is inconsistent with a previously stored belief — the retraction beat.".into() },
        ToolDef { name: "memory_forget".into(), description: "Delete and forget a stored memory or entity permanently, along with everything derived from it.".into() },
        ToolDef { name: "memory_compose".into(), description: "Compose, combine, and pull together context from past conversation threads into the next conversation window.".into() },
        ToolDef { name: "memory_feedback".into(), description: "Record whether a surfaced memory was helpful or an unrelated miss, to improve future recall.".into() },
    ]
}
