//! LM-10 — `tm-bench-triples`: triple-extraction quality bench.
//!
//! Scores the active triple extractor against a hand-labeled fixture.
//! Two scores are reported so we can track progress on both the easy
//! and the strict goal:
//!
//! 1. **Entity-pair precision/recall** (predicate-agnostic) — does the
//!    extractor at least connect the right pair of entities? Direction-
//!    aware: (subject, object) must match the gold pair as an ordered
//!    tuple. This is the floor: anything that misses the pair can't
//!    have the predicate right either.
//! 2. **Typed precision** — same as above, plus the predicate string
//!    matches the gold predicate (case-insensitive, after `to_string`).
//!    This is the SML target: ≥ 0.70 Q3 / ≥ 0.80 Q4 per PROJECT_2026.
//!
//! ## CLI
//!
//! ```text
//! tm-bench-triples [--fixtures PATH] [--threshold F] [--strict]
//!                  [--min-confidence F] [--json]
//! ```
//!
//! - `--threshold` defaults to 0.0 (no gate). Pass `--threshold 0.70`
//!   to fail below the Q3 floor.
//! - `--strict` switches the gate from entity-pair precision to typed
//!   precision (the harder target).
//! - `--min-confidence` only counts extractor triples whose confidence
//!   is ≥ this value (default 0.0 = all). Pass 0.7 to mirror the
//!   "high-confidence auto-promote" path from LM-9.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use tm_ingest::{EntityExtractor, HeuristicExtractor};
use tm_types::{Entity, Triple};

#[derive(Debug, Deserialize)]
struct FixtureRecord {
    text: String,
    triples: Vec<GoldTriple>,
}

#[derive(Debug, Deserialize, Clone)]
struct GoldTriple {
    subject: String,
    predicate: String,
    object: String,
}

#[derive(Debug, Serialize, Default, Clone)]
struct Score {
    tp: usize,
    fp: usize,
    fn_: usize,
}

impl Score {
    fn precision(&self) -> f64 {
        let d = self.tp + self.fp;
        if d == 0 { 0.0 } else { self.tp as f64 / d as f64 }
    }
    fn recall(&self) -> f64 {
        let d = self.tp + self.fn_;
        if d == 0 { 0.0 } else { self.tp as f64 / d as f64 }
    }
    fn f1(&self) -> f64 {
        let p = self.precision();
        let r = self.recall();
        if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    fixture_path: String,
    extractor: String,
    sentences: usize,
    total_gold: usize,
    total_pred: usize,
    pair_score: Score,
    typed_score: Score,
    threshold: f64,
    strict: bool,
}

#[derive(Debug)]
struct Args {
    fixtures: PathBuf,
    threshold: f64,
    strict: bool,
    min_confidence: f64,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("triples_eval.jsonl");
    let mut threshold = 0.0_f64;
    let mut strict = false;
    let mut min_confidence = 0.0_f64;
    let mut json = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--fixtures" => {
                fixtures = PathBuf::from(
                    it.next().ok_or("missing value for --fixtures")?,
                );
            }
            "--threshold" => {
                threshold = it
                    .next()
                    .ok_or("missing value for --threshold")?
                    .parse()
                    .map_err(|e| format!("--threshold: {e}"))?;
            }
            "--strict" => strict = true,
            "--min-confidence" => {
                min_confidence = it
                    .next()
                    .ok_or("missing value for --min-confidence")?
                    .parse()
                    .map_err(|e| format!("--min-confidence: {e}"))?;
            }
            "--json" => json = true,
            "-h" | "--help" => {
                eprintln!(
                    "tm-bench-triples [--fixtures PATH] [--threshold F] [--strict] [--min-confidence F] [--json]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok(Args { fixtures, threshold, strict, min_confidence, json })
}

fn load_fixtures(path: &Path) -> Result<Vec<FixtureRecord>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read fixtures {}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rec: FixtureRecord = serde_json::from_str(line)
            .map_err(|e| format!("parse fixtures line {}: {e}", i + 1))?;
        out.push(rec);
    }
    Ok(out)
}

fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Tolerant containment match: either side may contain the other after
/// lowercasing. Mirrors `tm-bench-ner`'s match policy so partial-span
/// extractors aren't penalised for boundary differences.
fn name_eq(a: &str, b: &str) -> bool {
    let a = normalize(a);
    let b = normalize(b);
    if a == b { return true; }
    if a.len() < 3 || b.len() < 3 { return false; }
    a.contains(&b) || b.contains(&a)
}

fn run(args: &Args) -> Result<Report, String> {
    let records = load_fixtures(&args.fixtures)?;
    let ext = HeuristicExtractor;

    let mut pair_score = Score::default();
    let mut typed_score = Score::default();
    let mut total_pred = 0usize;
    let mut total_gold = 0usize;

    for rec in &records {
        let ents: Vec<Entity> = ext.extract_entities(&rec.text);
        let pred_triples: Vec<Triple> = ext.extract_triples(&rec.text, &ents);

        // Build (subject_name, predicate_string, object_name) for every
        // predicted triple, after confidence filter.
        let id_to_name: std::collections::HashMap<uuid::Uuid, String> =
            ents.iter().map(|e| (e.id, e.name.clone())).collect();
        let pred_norm: Vec<(String, String, String)> = pred_triples
            .iter()
            .filter(|t| t.confidence >= args.min_confidence)
            .filter_map(|t| {
                let s = id_to_name.get(&t.subject_id)?.clone();
                let o = id_to_name.get(&t.object_id)?.clone();
                Some((s, t.predicate.to_string(), o))
            })
            .collect();
        total_pred += pred_norm.len();
        total_gold += rec.triples.len();

        let mut matched_gold_pair: HashSet<usize> = HashSet::new();
        let mut matched_pred_pair: HashSet<usize> = HashSet::new();
        let mut matched_gold_typed: HashSet<usize> = HashSet::new();
        let mut matched_pred_typed: HashSet<usize> = HashSet::new();

        // Pair match (predicate-agnostic, direction-aware).
        for (pi, (ps, _pp, po)) in pred_norm.iter().enumerate() {
            for (gi, gt) in rec.triples.iter().enumerate() {
                if matched_gold_pair.contains(&gi) || matched_pred_pair.contains(&pi) {
                    continue;
                }
                if name_eq(ps, &gt.subject) && name_eq(po, &gt.object) {
                    matched_gold_pair.insert(gi);
                    matched_pred_pair.insert(pi);
                    pair_score.tp += 1;
                    break;
                }
            }
        }

        // Typed match — pair + predicate string equal (case-insensitive).
        for (pi, (ps, pp, po)) in pred_norm.iter().enumerate() {
            for (gi, gt) in rec.triples.iter().enumerate() {
                if matched_gold_typed.contains(&gi) || matched_pred_typed.contains(&pi) {
                    continue;
                }
                if name_eq(ps, &gt.subject)
                    && name_eq(po, &gt.object)
                    && normalize(pp) == normalize(&gt.predicate)
                {
                    matched_gold_typed.insert(gi);
                    matched_pred_typed.insert(pi);
                    typed_score.tp += 1;
                    break;
                }
            }
        }

        // FP/FN bookkeeping per metric.
        for pi in 0..pred_norm.len() {
            if !matched_pred_pair.contains(&pi) {
                pair_score.fp += 1;
            }
            if !matched_pred_typed.contains(&pi) {
                typed_score.fp += 1;
            }
        }
        for gi in 0..rec.triples.len() {
            if !matched_gold_pair.contains(&gi) {
                pair_score.fn_ += 1;
            }
            if !matched_gold_typed.contains(&gi) {
                typed_score.fn_ += 1;
            }
        }
    }

    Ok(Report {
        fixture_path: args.fixtures.display().to_string(),
        extractor: ext.name().to_string(),
        sentences: records.len(),
        total_gold,
        total_pred,
        pair_score,
        typed_score,
        threshold: args.threshold,
        strict: args.strict,
    })
}

fn print_text(report: &Report) {
    println!("LM-10 — Triple-extraction bench ({})", report.extractor);
    println!("  fixtures : {}", report.fixture_path);
    println!(
        "  corpus   : {} sentences, {} gold triples, {} predicted",
        report.sentences, report.total_gold, report.total_pred
    );
    println!(
        "  pair     : P={:.3}  R={:.3}  F1={:.3}   (tp={}, fp={}, fn={})",
        report.pair_score.precision(),
        report.pair_score.recall(),
        report.pair_score.f1(),
        report.pair_score.tp,
        report.pair_score.fp,
        report.pair_score.fn_,
    );
    println!(
        "  typed    : P={:.3}  R={:.3}  F1={:.3}   (tp={}, fp={}, fn={})",
        report.typed_score.precision(),
        report.typed_score.recall(),
        report.typed_score.f1(),
        report.typed_score.tp,
        report.typed_score.fp,
        report.typed_score.fn_,
    );
    if report.threshold > 0.0 {
        let active = if report.strict {
            report.typed_score.precision()
        } else {
            report.pair_score.precision()
        };
        let label = if report.strict { "typed" } else { "pair" };
        println!(
            "  gate     : {label} precision {:.3} vs threshold {:.3}",
            active, report.threshold
        );
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

    if args.threshold > 0.0 {
        let active = if args.strict {
            report.typed_score.precision()
        } else {
            report.pair_score.precision()
        };
        if active + f64::EPSILON < args.threshold {
            eprintln!(
                "FAIL: precision {:.3} < threshold {:.3}",
                active, args.threshold
            );
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("triples_eval.jsonl")
    }

    #[test]
    fn fixture_loads_and_has_thirty_rows() {
        let recs = load_fixtures(&default_fixture()).expect("load");
        assert_eq!(recs.len(), 30, "expected 30 hand-labeled sentences");
        for r in &recs {
            assert!(!r.text.is_empty());
            assert!(!r.triples.is_empty());
        }
    }

    #[test]
    fn bench_runs_and_produces_nonzero_pair_tp() {
        // Smoke-test only: HeuristicExtractor is pattern-based, so
        // a "WorksAt" sentence ought to produce at least one matching
        // pair. We don't assert a precision floor here — that's what
        // the gated CLI run is for.
        let args = Args {
            fixtures: default_fixture(),
            threshold: 0.0,
            strict: false,
            min_confidence: 0.0,
            json: false,
        };
        let r = run(&args).expect("run bench");
        assert_eq!(r.sentences, 30);
        assert!(r.pair_score.tp > 0, "heuristic should match some pairs");
    }

    #[test]
    fn name_eq_is_tolerant() {
        assert!(name_eq("OpenAI", "openai"));
        assert!(name_eq("Aaditya Srivathsan", "Aaditya"));
        assert!(!name_eq("OpenAI", "Anthropic"));
    }
}
