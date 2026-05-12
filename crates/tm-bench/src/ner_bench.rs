//! TM-NLP-004 NER evaluation harness.
//!
//! Loads `fixtures/ner_eval.jsonl` (30 sentences with gold entities), runs
//! HeuristicExtractor + GlinerExtractor over each, and reports P/R/F1 both
//! overall and broken down by entity type.
//!
//! Usage:
//!   tm-bench-ner            # run both extractors
//!   tm-bench-ner --heuristic-only
//!   tm-bench-ner --gliner-only
//!
//! Exit code: 0 if both extractors finish; 2 if GLiNER model could not be
//! loaded (offline). Does NOT enforce a quality gate — that belongs in
//! a CI threshold file once we've settled on numbers.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use tm_ingest::{EntityExtractor, GlinerExtractor, HeuristicExtractor};
use tm_types::{Entity, EntityType};

const FIXTURE_PATH: &str = "crates/tm-bench/fixtures/ner_eval.jsonl";

#[derive(Debug, Deserialize)]
struct GoldRecord {
    text: String,
    entities: Vec<GoldEntity>,
}

#[derive(Debug, Deserialize, Clone)]
struct GoldEntity {
    name: String,
    #[serde(rename = "type")]
    etype: String,
}

#[derive(Debug, Default, Clone)]
struct Metrics {
    tp: usize,
    fp: usize,
    fn_: usize,
}

impl Metrics {
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let run_heuristic = !args.iter().any(|a| a == "--gliner-only");
    let run_gliner = !args.iter().any(|a| a == "--heuristic-only");

    let path = resolve_fixture();
    let records = load_fixture(&path);
    println!("Loaded {} labeled sentences from {}", records.len(), path.display());

    if run_heuristic {
        let ext = HeuristicExtractor;
        evaluate(&ext, &records, "HeuristicExtractor");
    }

    if run_gliner {
        match GlinerExtractor::auto_download_default() {
            Some(gli) => {
                evaluate(&gli, &records, "GlinerExtractor");
            }
            None => {
                eprintln!("\n[ner-bench] GLiNER model not available (offline?) — skipping.");
                std::process::exit(2);
            }
        }
    }
}

fn resolve_fixture() -> std::path::PathBuf {
    // Allow running from either workspace root or crate dir.
    let candidates = [
        FIXTURE_PATH.to_string(),
        format!("../../{FIXTURE_PATH}"),
        format!("../{FIXTURE_PATH}"),
        "fixtures/ner_eval.jsonl".to_string(),
    ];
    for c in &candidates {
        if Path::new(c).exists() {
            return std::path::PathBuf::from(c);
        }
    }
    panic!("could not find ner_eval.jsonl; searched: {candidates:?}");
}

fn load_fixture(path: &Path) -> Vec<GoldRecord> {
    let text = std::fs::read_to_string(path).expect("read fixture");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse fixture line"))
        .collect()
}

fn evaluate<E: EntityExtractor>(ext: &E, records: &[GoldRecord], name: &str) {
    println!("\n══════════════════════════════════════════════════════════════");
    println!("  {name}");
    println!("══════════════════════════════════════════════════════════════");

    let mut overall = Metrics::default();
    let mut per_type: HashMap<String, Metrics> = HashMap::new();

    let start = std::time::Instant::now();
    let mut total_preds = 0;

    for (i, rec) in records.iter().enumerate() {
        let preds = ext.extract_entities(&rec.text);
        total_preds += preds.len();
        let gold_norm: Vec<(String, String)> = rec
            .entities
            .iter()
            .map(|g| (normalize(&g.name), g.etype.clone()))
            .collect();
        let pred_norm: Vec<(String, String)> = preds
            .iter()
            .map(|p| (normalize(&p.name), entity_type_to_label(&p.entity_type)))
            .collect();

        // Name-match (type-agnostic): tolerant TP — boundaries can differ
        // slightly between extractors. Gold "GPT-5" vs pred "GPT 5" should
        // count once matched. We use containment either direction.
        let mut matched_gold: HashSet<usize> = HashSet::new();
        let mut matched_pred: HashSet<usize> = HashSet::new();

        for (pi, (pname, _ptype)) in pred_norm.iter().enumerate() {
            for (gi, (gname, _gtype)) in gold_norm.iter().enumerate() {
                if matched_gold.contains(&gi) || matched_pred.contains(&pi) {
                    continue;
                }
                if name_matches(pname, gname) {
                    matched_gold.insert(gi);
                    matched_pred.insert(pi);
                    overall.tp += 1;
                    let m = per_type.entry(gold_norm[gi].1.clone()).or_default();
                    m.tp += 1;
                    break;
                }
            }
        }

        // FP: predictions that didn't match any gold
        for (pi, (_, ptype)) in pred_norm.iter().enumerate() {
            if !matched_pred.contains(&pi) {
                overall.fp += 1;
                let m = per_type.entry(ptype.clone()).or_default();
                m.fp += 1;
            }
        }
        // FN: gold that didn't match any pred
        for (gi, (_, gtype)) in gold_norm.iter().enumerate() {
            if !matched_gold.contains(&gi) {
                overall.fn_ += 1;
                let m = per_type.entry(gtype.clone()).or_default();
                m.fn_ += 1;
            }
        }

        if std::env::var("VERBOSE").is_ok() {
            println!("\n[{i}] {}", rec.text);
            println!("   gold: {gold_norm:?}");
            println!("   pred: {pred_norm:?}");
        }
    }

    let dur = start.elapsed();
    println!(
        "\n  sentences={}, total_preds={}, wall={:.2}s ({:.1}ms/doc)",
        records.len(),
        total_preds,
        dur.as_secs_f64(),
        dur.as_secs_f64() * 1000.0 / records.len() as f64
    );
    println!(
        "\n  Overall:   P={:.3}  R={:.3}  F1={:.3}   (tp={}, fp={}, fn={})",
        overall.precision(),
        overall.recall(),
        overall.f1(),
        overall.tp,
        overall.fp,
        overall.fn_,
    );

    println!("\n  Per entity-type:");
    let mut keys: Vec<_> = per_type.keys().cloned().collect();
    keys.sort();
    for k in &keys {
        let m = &per_type[k];
        println!(
            "    {:14}  P={:.3}  R={:.3}  F1={:.3}   (tp={}, fp={}, fn={})",
            k,
            m.precision(),
            m.recall(),
            m.f1(),
            m.tp,
            m.fp,
            m.fn_
        );
    }
}

fn normalize(s: &str) -> String {
    s.trim()
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim_start_matches(|c: char| c.is_ascii_punctuation())
        .to_lowercase()
}

/// Containment-tolerant name match. Either direction counts.
fn name_matches(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    a.contains(b) || b.contains(a)
}

fn entity_type_to_label(t: &EntityType) -> String {
    match t {
        EntityType::Person => "Person".into(),
        EntityType::Organization => "Organization".into(),
        EntityType::Project => "Project".into(),
        EntityType::File => "File".into(),
        EntityType::Url => "Url".into(),
        EntityType::Concept => "Concept".into(),
        EntityType::Technology => "Technology".into(),
        EntityType::Decision => "Decision".into(),
        EntityType::Event => "Event".into(),
        EntityType::DailyNote => "DailyNote".into(),
        EntityType::MapOfContent => "MapOfContent".into(),
        EntityType::Custom(s) => s.clone(),
    }
}

// silence dead-code warning for Entity import in case we extend later
#[allow(dead_code)]
fn _unused_entity(_: Entity) {}
