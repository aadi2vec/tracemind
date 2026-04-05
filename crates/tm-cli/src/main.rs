use std::fs;
use std::io::Read as _;
use std::path::PathBuf;

use clap::Parser;
use tm_controller::UcbBandit;
use tm_episodic::{ProcedureStore, TraceStore, dry_run};
use tm_graph::GraphStore;
use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;
use tm_types::{Procedure, ProcedureStep};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(clap::Parser)]
#[command(name = "tracemind", about = "TraceMind local memory OS")]
struct Cli {
    /// Use deterministic hash embedder instead of ONNX model (offline/test mode)
    #[arg(long, global = true)]
    hash_embed: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Ingest text into memory (use "-" to read from stdin)
    Ingest { text: String },
    /// Query memory with natural language
    Query { text: String },
    /// Register a reward for a bandit arm
    Feedback { arm: u8, reward: f64 },
    /// Show recent traces with full audit detail
    Trace {
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Show a specific trace by ID (full detail)
        #[arg(long)]
        id: Option<String>,
    },
    /// Decay all entity/triple confidence by a factor
    Decay {
        #[arg(long, default_value = "0.95")]
        factor: f64,
        #[arg(long, default_value = "0.05")]
        threshold: f64,
    },
    /// Manage procedures (learnable action sequences)
    Proc {
        #[command(subcommand)]
        action: ProcAction,
    },
    /// Show bandit arm statistics
    Status,
}

#[derive(clap::Subcommand)]
enum ProcAction {
    /// Add a new procedure (steps as "action1;action2;action3")
    Add {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
        /// Semicolon-separated steps
        steps: String,
    },
    /// List all active procedures
    List,
    /// Dry-run a procedure by name
    Run { name: String },
    /// Record success/failure for a procedure
    Feedback {
        name: String,
        #[arg(long)]
        success: bool,
    },
}

// ---------------------------------------------------------------------------
// Data directory resolution
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    if let Ok(val) = std::env::var("TM_DATA_DIR") {
        PathBuf::from(val)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".tracemind")
    }
}

fn ensure_data_dir(dir: &PathBuf) {
    if !dir.exists() {
        fs::create_dir_all(dir).expect("failed to create data directory");
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let dir = data_dir();
    ensure_data_dir(&dir);

    let db_path = dir.join("memory.db").to_str().unwrap().to_string();
    let trace_path = dir.join("traces.jsonl").to_str().unwrap().to_string();
    let bandit_path = dir.join("bandit.json");

    match cli.command {
        Commands::Ingest { text } => {
            // Support stdin: `tracemind ingest -` reads from pipe/stdin
            let text = if text == "-" {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).expect("failed to read stdin");
                buf.trim().to_string()
            } else {
                text
            };

            if text.is_empty() {
                eprintln!("Nothing to ingest (empty input).");
                std::process::exit(1);
            }

            let pipeline = IngestPipeline::open(&db_path, cli.hash_embed)
                .expect("failed to open ingest pipeline");
            let session_id = Uuid::new_v4();
            let result = pipeline.ingest(&text, session_id)
                .expect("ingest failed");

            // Persist the trace.
            let trace_store = TraceStore::open(&trace_path)
                .expect("failed to open trace store");
            trace_store.append(&result.trace).expect("failed to write trace");

            // Show what was extracted
            println!("Ingested: \"{}\"", truncate_str(&text, 80));
            println!("  Trace:    {}", result.trace.id);
            println!("  Entities: {}", result.entities.len());
            for e in &result.entities {
                println!("    [{:>12}] {}", e.entity_type, e.name);
            }
            let typed: Vec<_> = result.triples.iter()
                .filter(|t| !matches!(t.predicate, tm_types::Predicate::RelatedTo))
                .collect();
            if !typed.is_empty() {
                println!("  Typed triples:");
                let name_of: std::collections::HashMap<Uuid, &str> = result.entities.iter()
                    .map(|e| (e.id, e.name.as_str())).collect();
                for t in &typed {
                    let s = name_of.get(&t.subject_id).unwrap_or(&"?");
                    let o = name_of.get(&t.object_id).unwrap_or(&"?");
                    println!("    {} -> {} -> {}", s, t.predicate, o);
                }
            }
            println!("  + {} co-occurrence triples", result.triples.len() - typed.len());
        }

        Commands::Query { text } => {
            let mut engine = RetrievalEngine::open(&db_path, &trace_path, cli.hash_embed)
                .expect("failed to open retrieval engine");
            let result = engine.query(&text).expect("query failed");

            // Build a name lookup from the returned entities.
            let name_of: std::collections::HashMap<uuid::Uuid, String> = result
                .entities
                .iter()
                .map(|e| (e.id, e.name.clone()))
                .collect();

            for entity in &result.entities {
                println!("  [{}] {}", entity.entity_type, entity.name);
            }
            for triple in &result.triples {
                // Only print if both endpoints are in the result set.
                if let (Some(subj), Some(obj)) = (
                    name_of.get(&triple.subject_id),
                    name_of.get(&triple.object_id),
                ) {
                    println!("  {} -> {} -> {}", subj, triple.predicate, obj);
                }
            }

            // Bandit stats are auto-saved by RetrievalEngine after each query.
        }

        Commands::Feedback { arm, reward } => {
            let mut bandit = UcbBandit::load(&bandit_path);
            bandit.register_reward(arm, reward);
            bandit.save(&bandit_path);
            println!("Reward registered.");
        }

        Commands::Decay { factor, threshold } => {
            let graph = GraphStore::open(&db_path)
                .expect("failed to open graph store");
            let below = graph.decay_all(factor, threshold)
                .expect("decay failed");
            println!("Decay applied (factor={factor}). {below} entities below {threshold} threshold.");
        }

        Commands::Trace { limit, id } => {
            let store = TraceStore::open(&trace_path)
                .expect("failed to open trace store");

            if let Some(target_id) = id {
                // Full audit detail for a single trace
                let traces = store.recent(1000).expect("failed to read traces");
                let trace = traces.iter().find(|t| t.id.to_string().starts_with(&target_id));
                match trace {
                    Some(t) => print_trace_detail(t, &db_path),
                    None => eprintln!("Trace '{}' not found.", target_id),
                }
            } else {
                // Summary list
                let traces = store.recent(limit).expect("failed to read traces");
                if traces.is_empty() {
                    println!("No traces recorded yet.");
                    return;
                }
                for trace in &traces {
                    let text_preview = trace.raw_text.as_deref()
                        .map(|t| truncate_str(t, 60))
                        .unwrap_or_else(|| "(no text)".to_string());
                    println!(
                        "  {:>8} {} | {} entities, {} triples | {}",
                        format!("{:?}", trace.event_type),
                        &trace.id.to_string()[..8],
                        trace.entities_extracted.len(),
                        trace.triples_extracted.len(),
                        text_preview,
                    );
                    if let Some(arm) = trace.retrieval_arm {
                        println!(
                            "           arm={}, latency={}ms",
                            arm,
                            trace.retrieval_latency_ms.unwrap_or(0)
                        );
                    }
                }
                println!("\nUse --id <prefix> to see full detail for a trace.");
            }
        }

        Commands::Proc { action } => {
            let proc_path = dir.join("procedures.jsonl").to_str().unwrap().to_string();
            let store = ProcedureStore::open(&proc_path)
                .expect("failed to open procedure store");

            match action {
                ProcAction::Add { name, desc, steps } => {
                    let steps: Vec<ProcedureStep> = steps
                        .split(';')
                        .enumerate()
                        .map(|(i, s)| ProcedureStep::new(i as u32 + 1, s.trim()))
                        .collect();
                    let proc = Procedure::new(&name, &desc, steps);
                    store.save(&proc).expect("failed to save procedure");
                    println!("Procedure '{}' created ({} steps, id={}).", name, proc.steps.len(), proc.id);
                }
                ProcAction::List => {
                    let procs = store.list_active().expect("failed to list procedures");
                    if procs.is_empty() {
                        println!("No active procedures.");
                    }
                    for p in &procs {
                        println!(
                            "  [{}] {} v{} ({:?}, {:.0}% confidence, {} steps)",
                            p.id, p.name, p.version, p.status,
                            p.confidence * 100.0, p.steps.len()
                        );
                    }
                }
                ProcAction::Run { name } => {
                    let proc = store
                        .get_by_name(&name)
                        .expect("failed to read procedures")
                        .unwrap_or_else(|| {
                            eprintln!("Procedure '{}' not found.", name);
                            std::process::exit(1);
                        });
                    for line in dry_run(&proc) {
                        println!("{line}");
                    }
                }
                ProcAction::Feedback { name, success } => {
                    let mut proc = store
                        .get_by_name(&name)
                        .expect("failed to read procedures")
                        .unwrap_or_else(|| {
                            eprintln!("Procedure '{}' not found.", name);
                            std::process::exit(1);
                        });
                    if success {
                        proc.record_success();
                    } else {
                        proc.record_failure();
                    }
                    store.save(&proc).expect("failed to save procedure");
                    println!(
                        "Procedure '{}': {:?} (confidence={:.0}%, {} ok / {} fail)",
                        proc.name, proc.status,
                        proc.confidence * 100.0,
                        proc.success_count, proc.failure_count
                    );
                }
            }
        }

        Commands::Status => {
            let bandit = UcbBandit::load(&bandit_path);
            let stats = bandit.arm_stats();
            let arm_names = ["vector-only", "graph-heavy", "hybrid", "episodic"];
            for (i, (pulls, avg_reward)) in stats.iter().enumerate() {
                let name = arm_names.get(i).unwrap_or(&"unknown");
                println!("  Arm {} ({}): pulls={}, avg_reward={:.2}", i, name, pulls, avg_reward);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_str(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        format!("{}...", &s[..max])
    }
}

fn print_trace_detail(trace: &tm_types::Trace, db_path: &str) {
    println!("Trace {}", trace.id);
    println!("  Event:      {:?}", trace.event_type);
    println!("  Session:    {}", trace.session_id);
    println!("  Created:    {}", trace.created_at);
    println!("  Hash:       {}", trace.content_hash);

    if let Some(ref text) = trace.raw_text {
        println!("  Raw text:   \"{}\"", text);
    }

    if !trace.entities_extracted.is_empty() {
        // Try to resolve entity names from the graph
        let graph = GraphStore::open(db_path).ok();
        println!("  Entities extracted ({}):", trace.entities_extracted.len());
        for eid in &trace.entities_extracted {
            let label = graph.as_ref()
                .and_then(|g| g.get_entity(*eid).ok())
                .map(|e| format!("[{}] {}", e.entity_type, e.name))
                .unwrap_or_else(|| eid.to_string());
            println!("    - {}", label);
        }
    }

    if !trace.triples_extracted.is_empty() {
        println!("  Triples extracted: {}", trace.triples_extracted.len());
    }

    if let Some(arm) = trace.retrieval_arm {
        let arm_names = ["vector-only", "graph-heavy", "hybrid", "episodic"];
        let name = arm_names.get(arm as usize).unwrap_or(&"unknown");
        println!("  Retrieval strategy: arm {} ({})", arm, name);
        if let Some(ms) = trace.retrieval_latency_ms {
            println!("  Latency: {}ms", ms);
        }
    }
}
