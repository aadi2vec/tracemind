//! LM-13 — `tm-bench-context`: cross-context false-bridge regression harness.
//!
//! Loads the hand-labeled fixture in `fixtures/false_bridges.json` (20
//! pairs at the time of writing) and verifies that, after `--strikes`
//! `wrong_context_suggestion` strikes per pair, the deny-list correctly
//! reports each pair as blocked.
//!
//! Score: **TN rate** = (correctly blocked pairs) / (total pairs).
//!
//! - Q2 target: ≥ 0.90 (deny-list catches user-flagged bridges)
//! - Q4 target: ≥ 0.98 (with ontological typing layered on top)
//!
//! The harness exits with code `1` if the TN rate is below `--threshold`
//! (defaults to 0.90), so it plugs straight into CI as a regression
//! gate, mirroring `tm-bench-locomo`'s F1 floor.
//!
//! CLI:
//!
//! ```text
//! tm-bench-context [--fixtures PATH] [--strikes N] [--threshold F] [--json]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use tm_graph::deny_list::DenyList;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct FixtureFile {
    #[allow(dead_code)]
    schema_version: u32,
    #[allow(dead_code)]
    description: Option<String>,
    strike_threshold: u32,
    pairs: Vec<FixturePair>,
}

#[derive(Debug, Deserialize)]
struct FixturePair {
    tag: String,
    domain_a: String,
    domain_b: String,
    ctx_a: Uuid,
    ctx_b: Uuid,
    entity_a: String,
    entity_b: String,
    #[allow(dead_code)]
    ent_a: Uuid,
    #[allow(dead_code)]
    ent_b: Uuid,
}

#[derive(Debug, Serialize)]
struct PairResult {
    tag: String,
    domain_a: String,
    domain_b: String,
    entity_a: String,
    entity_b: String,
    strikes_recorded: u32,
    blocked: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    fixture_path: String,
    strikes_per_pair: u32,
    threshold: f64,
    tn_rate: f64,
    blocked: usize,
    total: usize,
    pairs: Vec<PairResult>,
}

#[derive(Debug)]
struct Args {
    fixtures: PathBuf,
    strikes: u32,
    threshold: f64,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    // Default fixture path is relative to this crate so `cargo run -p
    // tm-bench-context` works from the workspace root.
    let mut fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("false_bridges.json");
    let mut strikes: u32 = 3;
    let mut threshold: f64 = 0.90;
    let mut json = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--fixtures" => {
                fixtures = PathBuf::from(
                    it.next().ok_or("missing value for --fixtures")?,
                );
            }
            "--strikes" => {
                strikes = it
                    .next()
                    .ok_or("missing value for --strikes")?
                    .parse()
                    .map_err(|e| format!("--strikes: {e}"))?;
            }
            "--threshold" => {
                threshold = it
                    .next()
                    .ok_or("missing value for --threshold")?
                    .parse()
                    .map_err(|e| format!("--threshold: {e}"))?;
            }
            "--json" => json = true,
            "-h" | "--help" => {
                eprintln!(
                    "tm-bench-context [--fixtures PATH] [--strikes N] [--threshold F] [--json]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok(Args { fixtures, strikes, threshold, json })
}

fn load_fixtures(path: &Path) -> Result<FixtureFile, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read fixtures {}: {e}", path.display()))?;
    serde_json::from_str::<FixtureFile>(&raw)
        .map_err(|e| format!("parse fixtures {}: {e}", path.display()))
}

fn run(args: &Args) -> Result<Report, String> {
    let fixtures = load_fixtures(&args.fixtures)?;

    // Use the threshold declared by the fixture so seeding stops at the
    // expected count, but let `--strikes` override (default 3 matches
    // PROJECT_2026 §1c "three-strike auto-deny").
    let strike_threshold = fixtures.strike_threshold;
    let mut deny_list = DenyList::default();
    deny_list.strike_threshold = strike_threshold;

    let mut pair_results = Vec::with_capacity(fixtures.pairs.len());
    let mut blocked = 0usize;

    for p in &fixtures.pairs {
        for i in 0..args.strikes {
            deny_list.record_context_strike(
                p.ctx_a,
                p.ctx_b,
                &format!("bench:{}:strike-{}", p.tag, i + 1),
            );
        }
        let is_blocked = deny_list.is_context_pair_blocked(p.ctx_a, p.ctx_b);
        if is_blocked {
            blocked += 1;
        }
        pair_results.push(PairResult {
            tag: p.tag.clone(),
            domain_a: p.domain_a.clone(),
            domain_b: p.domain_b.clone(),
            entity_a: p.entity_a.clone(),
            entity_b: p.entity_b.clone(),
            strikes_recorded: args.strikes,
            blocked: is_blocked,
        });
    }

    let total = fixtures.pairs.len();
    let tn_rate = if total == 0 { 0.0 } else { blocked as f64 / total as f64 };
    Ok(Report {
        fixture_path: args.fixtures.display().to_string(),
        strikes_per_pair: args.strikes,
        threshold: args.threshold,
        tn_rate,
        blocked,
        total,
        pairs: pair_results,
    })
}

fn print_text(report: &Report) {
    println!("LM-13 — Cross-context false-bridge regression bench");
    println!("  fixtures : {}", report.fixture_path);
    println!("  strikes  : {} per pair", report.strikes_per_pair);
    println!("  threshold: {:.2}", report.threshold);
    println!(
        "  result   : {} / {} blocked  (TN rate {:.3})",
        report.blocked, report.total, report.tn_rate,
    );
    println!();
    let mut misses = 0usize;
    for r in &report.pairs {
        let mark = if r.blocked { "✓" } else { "✗" };
        if !r.blocked {
            misses += 1;
        }
        println!(
            "  {} {:<32}  {} ({}) <-> {} ({})",
            mark, r.tag, r.entity_a, r.domain_a, r.entity_b, r.domain_b,
        );
    }
    if misses > 0 {
        println!();
        println!("  {} pair(s) NOT blocked — these are the regressions", misses);
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("argument error: {e}");
            return ExitCode::from(2);
        }
    };
    let report = match run(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bench error: {e}");
            return ExitCode::from(2);
        }
    };

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("serialize report: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        print_text(&report);
    }

    if report.tn_rate + f64::EPSILON < report.threshold {
        eprintln!(
            "FAIL: TN rate {:.3} < threshold {:.3}",
            report.tn_rate, report.threshold
        );
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("false_bridges.json")
    }

    #[test]
    fn fixture_loads_and_has_twenty_pairs() {
        let f = load_fixtures(&default_fixture()).expect("load fixtures");
        assert_eq!(f.pairs.len(), 20, "fixture must have 20 false-bridge pairs");
        assert_eq!(f.strike_threshold, 3);
    }

    #[test]
    fn bench_meets_q2_threshold_with_default_strikes() {
        // Default `--strikes 3` matches DEFAULT_STRIKE_THRESHOLD, so
        // every pair must be blocked → TN rate = 1.0 ≥ 0.90.
        let args = Args {
            fixtures: default_fixture(),
            strikes: 3,
            threshold: 0.90,
            json: false,
        };
        let report = run(&args).expect("run bench");
        assert_eq!(report.total, 20);
        assert_eq!(report.blocked, 20);
        assert!(
            (report.tn_rate - 1.0).abs() < 1e-9,
            "expected 1.0, got {}",
            report.tn_rate
        );
    }

    #[test]
    fn bench_fails_below_threshold_when_strikes_too_low() {
        // Two strikes < threshold(3) → nothing should be blocked.
        let args = Args {
            fixtures: default_fixture(),
            strikes: 2,
            threshold: 0.90,
            json: false,
        };
        let report = run(&args).expect("run bench");
        assert_eq!(report.blocked, 0, "no pair should be blocked with 2 strikes");
        assert!(report.tn_rate < args.threshold);
    }

    #[test]
    fn fixture_ctx_uuids_are_unique() {
        // Catch fixture-authoring mistakes: every ctx_a / ctx_b across
        // all 20 pairs must be a distinct UUID so the bench doesn't
        // accidentally test the same pair twice.
        let f = load_fixtures(&default_fixture()).expect("load fixtures");
        let mut seen = std::collections::HashSet::new();
        for p in &f.pairs {
            assert!(seen.insert(p.ctx_a), "duplicate ctx_a uuid for {}", p.tag);
            assert!(seen.insert(p.ctx_b), "duplicate ctx_b uuid for {}", p.tag);
        }
    }
}
