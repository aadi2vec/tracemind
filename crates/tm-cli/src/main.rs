use std::fs;
use std::io::Read as _;
use std::path::PathBuf;

use clap::Parser;
use tm_controller::UcbBandit;
use tm_episodic::{ProcedureStore, TraceStore, dry_run};
use tm_graph::GraphStore;
use tm_ingest::IngestPipeline;
use tm_rerank::ColbertReranker;
use tm_retrieval::RetrievalEngine;
use tm_types::{Procedure, ProcedureStep};
use uuid::Uuid;

mod answerer;

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
    /// Ask a question — clean prose answer with citations, dispatched
    /// through the tiered answer layer (Tier 0 always; Tier 1 / Tier 2
    /// when available). Hides the entity-rank dump that `query` shows.
    Ask {
        /// The question to ask.
        text: String,
        /// Force a specific tier: extractive | local-llm | apple-fm.
        /// Default: auto (dispatcher picks based on task + availability).
        #[arg(long)]
        tier: Option<String>,
        /// Task kind: short | open | summarize | extract | contradict.
        #[arg(long, default_value = "short")]
        task: String,
        /// Max output tokens.
        #[arg(long, default_value = "256")]
        max_tokens: u32,
        /// How many grounding chunks to feed the answer layer.
        #[arg(long, default_value = "6")]
        grounding: usize,
        /// Render the full AnswerResponse as JSON.
        #[arg(long)]
        json: bool,
    },
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
    /// Import files or directories into memory
    Import {
        /// Path to a file or directory to import
        path: String,
        /// File extensions to include (comma-separated, e.g. "md,txt,rs")
        #[arg(long, default_value = "md,txt,rs,py,js,ts,toml,yaml,yml,json")]
        ext: String,
        /// Maximum file size in KB (skip larger files)
        #[arg(long, default_value = "100")]
        max_kb: u64,
        /// Dry run — show what would be imported without actually importing
        #[arg(long)]
        dry_run: bool,
    },
    /// Show bandit arm statistics
    Status,
    /// Show the most recent capture events from the ring buffer
    /// (what the capture daemon / MCP server just ingested).
    Recent {
        /// Maximum number of events to show (newest first).
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Emit raw JSON lines instead of a formatted table.
        #[arg(long)]
        json: bool,
    },
    /// Manage local model weights (Tier-1 LLM, embedders, rerankers).
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
    /// Record a Commitment (intent / decision / hypothesis) — the wedge
    /// primitive of the system of intents (see `docs/INTENT_SYSTEM.md` §1.1).
    Commit {
        /// One of: intent | decision | hypothesis
        #[arg(long)]
        kind: String,
        /// Free-text statement of the commitment.
        statement: String,
        /// Optional RFC3339 deadline / horizon (e.g. 2026-05-01T17:00:00Z).
        #[arg(long)]
        horizon: Option<String>,
        /// Stakes: low | medium | high | reversible (default medium).
        #[arg(long)]
        stakes: Option<String>,
        /// Confidence in [0,1].
        #[arg(long)]
        confidence: Option<f32>,
        /// Comma-separated tags.
        #[arg(long, default_value = "")]
        tags: String,
        /// Comma-separated options considered.
        #[arg(long, default_value = "")]
        options: String,
        /// The chosen option (only meaningful for kind=decision).
        #[arg(long, default_value = "")]
        chosen: String,
        /// Expected outcome (free text).
        #[arg(long)]
        expected: Option<String>,
        /// Skip the world-model preflight line. Default: preflight on
        /// when a trained model + ≥6 priors are available. See
        /// `docs/INTENT_SYSTEM.md` §6.2.
        #[arg(long)]
        no_preflight: bool,
    },
    /// Attach an Outcome to a Commitment, walking the state machine to Completed.
    Resolve {
        /// Commitment UUID returned by `tracemind commit`.
        commitment_id: String,
        /// Polarity: better | as_expected | worse | mixed | no_outcome
        #[arg(long)]
        polarity: String,
        /// Free-text description of what actually happened.
        description: String,
        /// Optional free-text user note.
        #[arg(long)]
        note: Option<String>,
    },
    /// List open + acted (non-terminal) commitments.
    Commitments {
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Mined commitment candidates awaiting confirmation
    /// (`docs/INTENT_SYSTEM.md` §3.1).
    Candidates {
        #[command(subcommand)]
        action: CandidatesAction,
    },
    /// Daily brief — overdue + open + recently resolved + pending
    /// candidates. (`docs/INTENT_SYSTEM.md` §9.1)
    Brief {
        /// Render as JSON instead of formatted text. JSON is the
        /// stable format consumed by agents / external tooling.
        #[arg(long)]
        json: bool,
        /// Look-back window for the "resolved" section, in days.
        #[arg(long, default_value = "7")]
        resolved_days: i64,
    },
    /// Pattern detector surface — show / silence / unsilence the
    /// detector's findings. (`docs/INTENT_SYSTEM.md` §5)
    Patterns {
        #[command(subcommand)]
        action: PatternsAction,
    },
    /// World model v0 — `f_outcome` predictor over your completed
    /// commitments. Trains a small multinomial logistic regression on
    /// metadata features (stakes / time-band / horizon / kind / tags)
    /// and persists weights to `~/.tracemind/world_model.json`.
    /// See `docs/INTENT_SYSTEM.md` §7.
    World {
        #[command(subcommand)]
        action: WorldAction,
    },
}

#[derive(clap::Subcommand)]
enum CandidatesAction {
    /// Show pending candidates (newest first).
    List {
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Promote a candidate into a real Open commitment.
    Accept { candidate_id: String },
    /// Mark a candidate dismissed (no commitment created).
    Dismiss { candidate_id: String },
}

#[derive(clap::Subcommand)]
enum PatternsAction {
    /// List currently-surfacing patterns (the same set the brief
    /// would show right now, after silences).
    List,
    /// List silenced cells. Useful before unsilencing.
    Silenced,
    /// Silence a cell so the detector stops surfacing it. Default
    /// window: 90 days (`INTENT_SYSTEM.md` §5.1.3).
    Silence {
        /// 16-hex cell hash from the brief's "patterns spotted" section.
        cell_hash: String,
        /// Silence window in days. Default 90.
        #[arg(long, default_value = "90")]
        days: i64,
        /// Optional reason — stored alongside the silence row.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Re-enable surfacing for a previously-silenced cell.
    Unsilence { cell_hash: String },
}

#[derive(clap::Subcommand)]
enum WorldAction {
    /// Train (or refresh) the world model from completed commitments.
    Train {
        /// Look-back window in days for completed commitments.
        #[arg(long, default_value = "365")]
        since_days: i64,
        /// SGD epochs. Default 400 — converges fast on tiny N.
        #[arg(long, default_value = "400")]
        epochs: usize,
        /// Learning rate.
        #[arg(long, default_value = "0.1")]
        lr: f32,
        /// L2 weight-decay coefficient.
        #[arg(long, default_value = "0.001")]
        l2: f32,
        /// Tag-vocab cap.
        #[arg(long, default_value = "16")]
        vocab: usize,
        /// Skip training below this many completed commitments.
        #[arg(long, default_value = "6")]
        min_examples: usize,
        /// Cap on rows fetched from the intent store.
        #[arg(long, default_value = "5000")]
        limit: usize,
        /// Emit the TrainReport as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Predict the polarity distribution for a hypothetical commitment.
    /// Mirrors the fields of `tracemind commit` so you can dry-run
    /// "if I made this commitment now, what would my own track record
    /// say about it?" before actually recording it.
    Predict {
        /// One of: intent | decision | hypothesis
        #[arg(long, default_value = "intent")]
        kind: String,
        /// Free-text statement of the commitment (used for hashing only).
        statement: String,
        /// Stakes: low | medium | high | reversible (default medium).
        #[arg(long)]
        stakes: Option<String>,
        /// Confidence in [0,1].
        #[arg(long)]
        confidence: Option<f32>,
        /// Comma-separated tags.
        #[arg(long, default_value = "")]
        tags: String,
        /// Optional RFC3339 horizon (presence flips the has-horizon
        /// feature on; the actual time is unused for v0).
        #[arg(long)]
        horizon: Option<String>,
        /// Comma-separated options considered (count > 1 trips the
        /// has-options feature).
        #[arg(long, default_value = "")]
        options: String,
        /// Show top-K feature contributions for the predicted class.
        #[arg(long)]
        explain: bool,
        /// Number of features to show with --explain.
        #[arg(long, default_value = "5")]
        top_k: usize,
        /// Emit the OutcomePrediction + explanation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show whether a trained model is on disk and summarize it.
    Status {
        /// Emit JSON instead of a formatted summary.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand)]
enum ModelsAction {
    /// Show whether Tier-1 weights are present and where they live.
    Status,
    /// Download Tier-1 LLM weights (Qwen 2.5 1.5B Q4_K_M, ~900 MB) from
    /// HuggingFace into `~/.tracemind/models/`. Idempotent — no-op when
    /// weights are already present.
    Pull {
        /// Use the mobile-class model (Qwen 2.5 0.5B Q4_K_M, ~350 MB)
        /// instead of the laptop default.
        #[arg(long)]
        mobile: bool,
    },
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
    // TM-NLP-005: route model loaders at any bundled weights before first load.
    if let Some(r) = tm_types::bundled::init() {
        eprintln!(
            "[tracemind] using bundled models from {} ({})",
            r.hf_cache.display(),
            r.source
        );
    }

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

            let mut pipeline = IngestPipeline::open(&db_path, cli.hash_embed)
                .expect("failed to open ingest pipeline");
            // TM-NLP-004: opt into real GLiNER NER when the model is available.
            // Falls back silently to the heuristic extractor if offline.
            if let Some(gli) = tm_ingest::GlinerExtractor::auto_download_default() {
                pipeline = pipeline.with_extractor(Box::new(gli));
            }
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

            // Sprint C: mine commitment candidates from the ingested text.
            // Soft-fail: any miner / store error is logged via eprintln! and
            // never blocks the ingest path.
            //
            // Sprint D: also score the new text against open commitments
            // (`INTENT_SYSTEM.md` §4.2) and surface proposed outcomes —
            // the user can then run `tracemind resolve <id>` to confirm.
            {
                let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
                match tm_intent::IntentStore::open(&intents_path) {
                    Ok(mut store) => {
                        // -- mining --
                        let mined = tm_intent::mine(&text);
                        if !mined.is_empty() {
                            let records: Vec<_> = mined
                                .into_iter()
                                .map(|c| tm_intent::CandidateRecord::from_mined(&c, text.clone()))
                                .collect();
                            match store.insert_candidates(&records) {
                                Ok(n) => {
                                    if n > 0 {
                                        println!("  + {} commitment candidate(s) mined (run `tracemind candidates list`)", n);
                                    }
                                }
                                Err(e) => eprintln!("  (candidate mining: insert failed: {e})"),
                            }
                        }

                        // -- outcome matching --
                        match store.list_open(50) {
                            Ok(opens) if !opens.is_empty() => {
                                let cfg = tm_reflect::MatcherConfig::default();
                                let proposals =
                                    tm_reflect::propose_outcomes(&text, &opens, &cfg);
                                if !proposals.is_empty() {
                                    println!(
                                        "  + {} possible outcome match(es) — run `tracemind resolve <id>`:",
                                        proposals.len()
                                    );
                                    for p in &proposals {
                                        let hint = match p.polarity_hint {
                                            Some(tm_intent::Polarity::Better) => " [hint: better]",
                                            Some(tm_intent::Polarity::Worse) => " [hint: worse]",
                                            Some(tm_intent::Polarity::AsExpected) => " [hint: as_expected]",
                                            Some(tm_intent::Polarity::Mixed) => " [hint: mixed]",
                                            Some(tm_intent::Polarity::NoOutcome) => " [hint: no_outcome]",
                                            None => "",
                                        };
                                        println!(
                                            "      {}  score={:.2}{}  → {}",
                                            short_id(p.commitment_id),
                                            p.score,
                                            hint,
                                            truncate_str(&p.commitment_statement, 50),
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(e) => eprintln!("  (outcome matching: list_open failed: {e})"),
                        }
                    }
                    Err(e) => eprintln!("  (intent store: open failed: {e})"),
                }
            }
        }

        Commands::Query { text } => {
            let reranker = ColbertReranker::auto_download_or_none(0.7);
            let mut engine = RetrievalEngine::open(&db_path, &trace_path, cli.hash_embed)
                .expect("failed to open retrieval engine")
                .with_reranker_instance(reranker);
            let result = engine.query(&text).expect("query failed");

            // Sprint A: dispatch through the tiered answerer (Tier 0 always;
            // Tier 1 when `local-llm` feature is on and weights are present).
            let answerer = answerer::build_answerer();
            let grounding = answerer::grounding_from(&result, 6);
            let req = answerer::short_answer_request(&text, grounding);
            match answerer::answer_blocking(&answerer, &req) {
                Ok(resp) => {
                    println!("Answer ({:?}, {}ms):", resp.tier, resp.latency_ms);
                    println!("{}", resp.text.trim());
                    if !resp.citations.is_empty() {
                        let cites: Vec<String> = resp
                            .citations
                            .iter()
                            .map(|c| format!("[{}] {}", c.chunk_index + 1, c.trace_id))
                            .collect();
                        println!("\nCitations: {}", cites.join(", "));
                    }
                    println!();
                }
                Err(e) => {
                    eprintln!("[answer] dispatch failed: {e}");
                }
            }

            // Build a name lookup from the returned entities (used by the
            // entity / triple / related listing below).
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

            // TM-UX-001: surface 1-hop graph neighbours as recommendations.
            if !result.related_entities.is_empty() {
                println!("\nRelated:");
                for r in &result.related_entities {
                    println!("  [{}] {} ({})", r.entity_type, r.name, r.reason);
                }
            }

            // Bandit stats are auto-saved by RetrievalEngine after each query.
        }

        Commands::Ask { text, tier, task, max_tokens, grounding, json } => {
            cmd_ask(
                &text,
                tier.as_deref(),
                &task,
                max_tokens,
                grounding,
                json,
                &db_path,
                &trace_path,
                cli.hash_embed,
            );
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

        Commands::Import { path, ext, max_kb, dry_run } => {
            cmd_import(&path, &ext, max_kb, dry_run, cli.hash_embed, &db_path);
        }

        Commands::Status => {
            let bandit = UcbBandit::load(&bandit_path);
            let stats = bandit.arm_stats();
            for (i, (pulls, avg_reward)) in stats.iter().enumerate() {
                let name = UcbBandit::arm_name(i as u8);
                println!("  Arm {} ({}): pulls={}, avg_reward={:.2}", i, name, pulls, avg_reward);
            }
        }

        Commands::Recent { limit, json } => {
            let recent_path = dir.join("recent.jsonl");
            let store = match tm_episodic::RecentStore::open(&recent_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open recent store at {}: {}", recent_path.display(), e);
                    std::process::exit(1);
                }
            };
            let events = store.recent(limit).unwrap_or_default();

            if json {
                for ev in &events {
                    match serde_json::to_string(ev) {
                        Ok(line) => println!("{}", line),
                        Err(e) => eprintln!("serialize error: {}", e),
                    }
                }
                return;
            }

            if events.is_empty() {
                println!("No recent captures. Start the capture daemon or ingest via MCP.");
                return;
            }

            println!("Recent captures (newest first, limit={}):", limit);
            for ev in &events {
                let status = if let Some(reason) = &ev.skipped_reason {
                    format!("skipped[{}]", reason)
                } else if ev.promoted {
                    "promoted".to_string()
                } else {
                    "stored".to_string()
                };
                let tier = ev.tier.as_deref().unwrap_or("--");
                println!(
                    "  {} [{:>8}] {:>10} {:>10}  {}",
                    ev.timestamp.format("%H:%M:%S"),
                    ev.source,
                    tier,
                    status,
                    ev.text_preview,
                );
            }
        }

        Commands::Models { action } => {
            cmd_models(action);
        }
        Commands::Commit {
            kind,
            statement,
            horizon,
            stakes,
            confidence,
            tags,
            options,
            chosen,
            expected,
            no_preflight,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            // Preflight is suppressed by either --no-preflight or
            // TM_NO_PREFLIGHT=1, so agents driving the CLI in scripted
            // contexts can opt out without touching argv.
            let preflight_off =
                no_preflight || std::env::var_os("TM_NO_PREFLIGHT").is_some();
            cmd_commit(
                &intents_path,
                &world_path,
                &kind,
                statement,
                horizon.as_deref(),
                stakes.as_deref(),
                confidence,
                &tags,
                &options,
                &chosen,
                expected.as_deref(),
                !preflight_off,
            );
        }
        Commands::Resolve {
            commitment_id,
            polarity,
            description,
            note,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            cmd_resolve(
                &intents_path,
                &world_path,
                &commitment_id,
                &polarity,
                &description,
                note.as_deref(),
            );
        }
        Commands::Commitments { limit } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_commitments(&intents_path, limit);
        }
        Commands::Candidates { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_candidates(&intents_path, action);
        }
        Commands::Brief { json, resolved_days } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            cmd_brief(&intents_path, &world_path, json, resolved_days);
        }
        Commands::Patterns { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_patterns(&intents_path, action);
        }
        Commands::World { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            cmd_world(&intents_path, &world_path, action);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `tracemind models …` handler. Reports tier-1 weight status and (when the
/// `local-llm` feature is on) downloads the GGUF on demand.
fn cmd_models(action: ModelsAction) {
    use tm_answer::{LocalLlmBackend, LocalLlmConfig, default_model_path};

    match action {
        ModelsAction::Status => {
            let path = default_model_path();
            let cfg = LocalLlmConfig::primary(path.clone());
            let backend = LocalLlmBackend::new(cfg);
            let approx_mb = tm_answer::QWEN_1_5B_Q4_APPROX_BYTES / (1024 * 1024);
            println!("Tier-1 LLM (Qwen 2.5 1.5B Q4_K_M)");
            println!("  Path:     {}", path.display());
            if backend.weights_present() {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!("  Status:   READY ({} MB on disk)", size / (1024 * 1024));
            } else {
                println!("  Status:   needs download (~{} MB)", approx_mb);
                #[cfg(feature = "local-llm")]
                println!("  Hint:     run `tracemind models pull` to fetch it.");
                #[cfg(not(feature = "local-llm"))]
                println!(
                    "  Hint:     rebuild with `--features local-llm` (or local-llm-metal on macOS)\n            to enable Tier-1 inference + auto-download."
                );
            }
            println!("  Source:   {}/{}", tm_answer::HF_REPO_PRIMARY, tm_answer::HF_FILE_PRIMARY);
        }

        ModelsAction::Pull { mobile } => {
            let path = default_model_path();
            // Mobile config swaps repo/file but keeps the same `models/` dir.
            let mobile_path = path
                .parent()
                .map(|p| p.join(tm_answer::HF_FILE_MOBILE))
                .unwrap_or_else(|| std::path::PathBuf::from(tm_answer::HF_FILE_MOBILE));
            let (cfg, target) = if mobile {
                (LocalLlmConfig::mobile(mobile_path.clone()), mobile_path)
            } else {
                (LocalLlmConfig::primary(path.clone()), path)
            };
            let backend = LocalLlmBackend::new(cfg);

            if backend.weights_present() {
                println!("Weights already present at {}", target.display());
                return;
            }

            #[cfg(not(feature = "local-llm"))]
            {
                eprintln!(
                    "tracemind was built without the `local-llm` feature, so it cannot download \
                    Tier-1 weights.\nRebuild with: cargo build --release -p tm-cli --features local-llm \
                    (add `local-llm-metal` on Apple Silicon for Metal acceleration)."
                );
                std::process::exit(2);
            }

            #[cfg(feature = "local-llm")]
            {
                let approx_mb = if mobile {
                    350
                } else {
                    tm_answer::QWEN_1_5B_Q4_APPROX_BYTES / (1024 * 1024)
                };
                println!(
                    "Downloading {}/{} (~{} MB) → {}",
                    if mobile {
                        tm_answer::HF_REPO_MOBILE
                    } else {
                        tm_answer::HF_REPO_PRIMARY
                    },
                    if mobile {
                        tm_answer::HF_FILE_MOBILE
                    } else {
                        tm_answer::HF_FILE_PRIMARY
                    },
                    approx_mb,
                    target.display(),
                );
                match backend.ensure_weights() {
                    Ok(p) => {
                        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                        println!("Done. Weights at {} ({} MB).", p.display(), size / (1024 * 1024));
                    }
                    Err(e) => {
                        eprintln!("Download failed: {}", e);
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}

/// `tracemind ask <q>` — clean prose answer through the tiered answerer.
///
/// Distinct from `query`: skips the entity / triple / related dump and
/// surfaces just the synthesized text + citations + tier badge. Designed
/// for humans (and IDE/agent integrations) who want one answer, not a
/// retrieval audit.
fn cmd_ask(
    text: &str,
    tier_arg: Option<&str>,
    task_arg: &str,
    max_tokens: u32,
    grounding_n: usize,
    json: bool,
    db_path: &str,
    trace_path: &str,
    hash_embed: bool,
) {
    use tm_answer::{AnswerRequest, AnswerTier, TaskKind};

    let task = match task_arg.to_lowercase().as_str() {
        "short" | "short_answer" | "short-answer" => TaskKind::ShortAnswer,
        "open" | "synth" | "synthesis" | "open_ended" | "open-ended" => {
            TaskKind::OpenEndedSynthesis
        }
        "summarize" | "summary" | "summarization" => TaskKind::Summarization,
        "extract" | "structured" | "structured_extraction" => TaskKind::StructuredExtraction,
        "contradict" | "contradiction" | "contradiction_check" => TaskKind::ContradictionCheck,
        other => {
            eprintln!(
                "unknown --task '{}': use short | open | summarize | extract | contradict",
                other
            );
            std::process::exit(2);
        }
    };

    let preferred_tier: Option<AnswerTier> = match tier_arg {
        None | Some("auto") => None,
        Some(t) => match t.to_lowercase().as_str() {
            "extractive" | "tier0" | "tier-0" | "0" => Some(AnswerTier::Extractive),
            "local-llm" | "local_llm" | "localllm" | "tier1" | "tier-1" | "1" => {
                Some(AnswerTier::LocalLlm)
            }
            "apple-fm" | "apple_fm" | "applefm" | "tier2" | "tier-2" | "2" => {
                Some(AnswerTier::AppleFm)
            }
            other => {
                eprintln!(
                    "unknown --tier '{}': use auto | extractive | local-llm | apple-fm",
                    other
                );
                std::process::exit(2);
            }
        },
    };

    let reranker = ColbertReranker::auto_download_or_none(0.7);
    let mut engine = RetrievalEngine::open(db_path, trace_path, hash_embed)
        .expect("failed to open retrieval engine")
        .with_reranker_instance(reranker);
    let result = engine.query(text).expect("query failed");

    let answerer = answerer::build_answerer();
    let grounding = answerer::grounding_from(&result, grounding_n.max(1));
    let mut req = AnswerRequest::new(text.to_string(), task)
        .with_grounding(grounding)
        .with_max_tokens(max_tokens.max(8));
    if let Some(t) = preferred_tier {
        req = req.with_preferred_tier(t);
    }

    match answerer::answer_blocking(&answerer, &req) {
        Ok(resp) => {
            if json {
                match serde_json::to_string_pretty(&resp) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("json encode failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                println!("{}", resp.text.trim());
                println!();
                let label = match resp.tier {
                    AnswerTier::Extractive => "tier-0 extractive",
                    AnswerTier::LocalLlm => "tier-1 local-llm",
                    AnswerTier::AppleFm => "tier-2 apple-fm",
                };
                println!(
                    "— {} • {} ms • {} citation(s)",
                    label,
                    resp.latency_ms,
                    resp.citations.len()
                );
                for c in &resp.citations {
                    println!("    [{}] {}", c.chunk_index + 1, c.trace_id);
                }

                // If we fell back to extractive while Tier-1 weights are
                // missing, point the user at the one-command unlock.
                if matches!(resp.tier, AnswerTier::Extractive) {
                    let backend = tm_answer::LocalLlmBackend::new(
                        tm_answer::LocalLlmConfig::primary(tm_answer::default_model_path()),
                    );
                    if !backend.weights_present() {
                        let mb = tm_answer::QWEN_1_5B_Q4_APPROX_BYTES / (1024 * 1024);
                        eprintln!(
                            "\n(hint: Tier-1 weights not present. \
                             `tracemind models pull` (~{mb} MB) unlocks prose answers.)"
                        );
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("ask failed: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_import(path: &str, extensions: &str, max_kb: u64, dry_run: bool, hash_embed: bool, db_path: &str) {
    let ext_set: std::collections::HashSet<String> = extensions
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .collect();

    let skip_dirs: std::collections::HashSet<&str> = [
        "node_modules", "target", ".git", "__pycache__", "dist",
        "build", ".next", "vendor", ".venv", "venv",
    ].into_iter().collect();

    let root = std::path::Path::new(path);
    if !root.exists() {
        eprintln!("Error: path '{}' does not exist", path);
        std::process::exit(1);
    }

    let files = collect_files(root, &ext_set, &skip_dirs, max_kb * 1024);

    if dry_run {
        println!("Dry run — would import {} files:", files.len());
        for f in &files {
            let size_kb = f.metadata().map(|m| m.len() / 1024).unwrap_or(0);
            println!("  {} ({} KB)", f.display(), size_kb);
        }
        return;
    }

    println!("Importing {} files from {}", files.len(), path);

    let mut pipeline = IngestPipeline::open(db_path, hash_embed)
        .expect("failed to open ingest pipeline");
    // TM-NLP-004: real GLiNER NER for file imports too (bulk path benefits most).
    if let Some(gli) = tm_ingest::GlinerExtractor::auto_download_default() {
        pipeline = pipeline.with_extractor(Box::new(gli));
    }
    let session_id = Uuid::new_v4();

    let mut imported = 0u32;
    let mut skipped = 0u32;
    let mut errors = 0u32;
    let mut total_bytes = 0u64;

    for file_path in &files {
        let rel_path = file_path.strip_prefix(root).unwrap_or(file_path);

        match std::fs::read_to_string(file_path) {
            Ok(raw) => {
                // Obsidian vaults store YAML frontmatter at the top of every
                // note (`---\n...\n---`). Stripping it before ingest keeps the
                // body's prose / wikilinks / tags intact while preventing
                // metadata keys (created, tags:, aliases:) from polluting the
                // entity extractor.
                let contents = strip_md_frontmatter(&raw);
                if contents.trim().is_empty() {
                    skipped += 1;
                    continue;
                }
                let text = format!("[File: {}]\n\n{}", rel_path.display(), contents);
                total_bytes += text.len() as u64;

                match pipeline.ingest(&text, session_id) {
                    Ok(result) => {
                        if result.skip_gate {
                            skipped += 1;
                        } else {
                            imported += 1;
                            println!("  {} ({} entities, {} triples)",
                                rel_path.display(), result.entities.len(), result.triples.len());
                        }
                    }
                    Err(e) => {
                        eprintln!("  x {}: {}", rel_path.display(), e);
                        errors += 1;
                    }
                }
            }
            Err(_) => {
                skipped += 1; // not UTF-8
            }
        }
    }

    println!("\nImport complete:");
    println!("  Imported: {}", imported);
    println!("  Skipped:  {} (empty, non-UTF-8, or duplicate)", skipped);
    println!("  Errors:   {}", errors);
    println!("  Total:    {} KB processed", total_bytes / 1024);
}

/// Strip a YAML frontmatter block (`---\n…\n---`) from the start of a
/// markdown document. Supports both `---` and `+++` (TOML) fences. If no
/// fence is present at the very start, returns the input unchanged.
///
/// This makes `tracemind import <obsidian-vault>` ingest the *body* of each
/// note without leaking metadata fields (created:, tags:, aliases:, …) into
/// the entity / triple extractor.
fn strip_md_frontmatter(s: &str) -> &str {
    let trimmed_leading = s.trim_start_matches(|c: char| c == '\u{feff}');
    let fence: &str = if trimmed_leading.starts_with("---\n") || trimmed_leading.starts_with("---\r\n") {
        "---"
    } else if trimmed_leading.starts_with("+++\n") || trimmed_leading.starts_with("+++\r\n") {
        "+++"
    } else {
        return s;
    };

    // Skip the opening fence line, then look for the matching closing fence
    // on its own line.
    let body_start = match trimmed_leading.find('\n') {
        Some(n) => n + 1,
        None => return s,
    };
    let after_open = &trimmed_leading[body_start..];

    // Look for "\n---\n" (or with \r) — the closing fence at line start.
    let mut search_from = 0usize;
    while let Some(idx) = after_open[search_from..].find(fence) {
        let abs = search_from + idx;
        let starts_at_line = abs == 0 || after_open.as_bytes()[abs - 1] == b'\n';
        let after_fence = abs + fence.len();
        let ends_line = after_fence == after_open.len()
            || matches!(after_open.as_bytes().get(after_fence), Some(b'\n') | Some(b'\r'));
        if starts_at_line && ends_line {
            // Skip past the fence + trailing newline if any.
            let mut tail = after_fence;
            if after_open.as_bytes().get(tail) == Some(&b'\r') {
                tail += 1;
            }
            if after_open.as_bytes().get(tail) == Some(&b'\n') {
                tail += 1;
            }
            return &after_open[tail..];
        }
        search_from = abs + fence.len();
    }
    // Unterminated frontmatter — leave it alone rather than swallow content.
    s
}

fn collect_files(
    root: &std::path::Path,
    extensions: &std::collections::HashSet<String>,
    skip_dirs: &std::collections::HashSet<&str>,
    max_bytes: u64,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_recursive(root, extensions, skip_dirs, max_bytes, &mut files);
    files.sort();
    files
}

fn collect_files_recursive(
    dir: &std::path::Path,
    extensions: &std::collections::HashSet<String>,
    skip_dirs: &std::collections::HashSet<&str>,
    max_bytes: u64,
    out: &mut Vec<PathBuf>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            if !skip_dirs.contains(name.as_str()) {
                collect_files_recursive(&path, extensions, skip_dirs, max_bytes, out);
            }
            continue;
        }

        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if !extensions.contains(&ext.to_lowercase()) {
                continue;
            }
        } else {
            continue;
        }

        if let Ok(meta) = entry.metadata() {
            if meta.len() > max_bytes {
                continue;
            }
        }

        out.push(path);
    }
}

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
        let name = UcbBandit::arm_name(arm);
        println!("  Retrieval strategy: arm {} ({})", arm, name);
        if let Some(ms) = trace.retrieval_latency_ms {
            println!("  Latency: {}ms", ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::strip_md_frontmatter;

    #[test]
    fn no_frontmatter_passthrough() {
        let s = "# Hello\n\nbody";
        assert_eq!(strip_md_frontmatter(s), s);
    }

    #[test]
    fn yaml_frontmatter_stripped() {
        let s = "---\ntitle: Note\ntags: [a, b]\n---\n# Body\n\ntext";
        assert_eq!(strip_md_frontmatter(s), "# Body\n\ntext");
    }

    #[test]
    fn toml_frontmatter_stripped() {
        let s = "+++\ntitle = \"Note\"\n+++\nbody";
        assert_eq!(strip_md_frontmatter(s), "body");
    }

    #[test]
    fn unterminated_frontmatter_left_alone() {
        let s = "---\ntitle: oops\nno close fence";
        assert_eq!(strip_md_frontmatter(s), s);
    }

    #[test]
    fn crlf_frontmatter_stripped() {
        let s = "---\r\ntitle: Note\r\n---\r\nbody";
        assert_eq!(strip_md_frontmatter(s), "body");
    }

    #[test]
    fn fence_inside_body_not_consumed() {
        let s = "no frontmatter\n---\nseparator\n---\nmore";
        assert_eq!(strip_md_frontmatter(s), s);
    }
}

// ---------------------------------------------------------------------------
// `tracemind commit` / `resolve` / `commitments` — system of intents wedge.
// Mirrors the MCP tools `memory_commit` / `memory_resolve` so the same
// primitive works from the terminal. See `docs/INTENT_SYSTEM.md`.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn cmd_commit(
    intents_path: &str,
    world_path: &std::path::Path,
    kind_s: &str,
    statement: String,
    horizon: Option<&str>,
    stakes: Option<&str>,
    confidence: Option<f32>,
    tags_csv: &str,
    options_csv: &str,
    chosen: &str,
    expected: Option<&str>,
    preflight: bool,
) {
    use tm_intent::{Commitment, CommitmentKind, IntentStore, Source, Stakes};

    let kind = match kind_s {
        "intent" => CommitmentKind::Intent,
        "decision" => CommitmentKind::Decision,
        "hypothesis" => CommitmentKind::Hypothesis,
        other => {
            eprintln!("invalid --kind: {other} (expected intent|decision|hypothesis)");
            std::process::exit(2);
        }
    };
    let statement = statement.trim().to_string();
    if statement.is_empty() {
        eprintln!("statement must be non-empty");
        std::process::exit(2);
    }

    let mut c = Commitment::new(kind, statement, Source::Cli);

    if let Some(h) = horizon {
        match chrono::DateTime::parse_from_rfc3339(h) {
            Ok(t) => c.horizon = Some(t.with_timezone(&chrono::Utc)),
            Err(e) => {
                eprintln!("invalid --horizon (need RFC3339, e.g. 2026-05-01T17:00:00Z): {e}");
                std::process::exit(2);
            }
        }
    }
    if let Some(s) = stakes {
        c.stakes = match s {
            "low" => Stakes::Low,
            "medium" => Stakes::Medium,
            "high" => Stakes::High,
            "reversible" => Stakes::Reversible,
            other => {
                eprintln!("invalid --stakes: {other}");
                std::process::exit(2);
            }
        };
    }
    if let Some(f) = confidence {
        if !(0.0..=1.0).contains(&f) {
            eprintln!("--confidence must be in [0,1], got {f}");
            std::process::exit(2);
        }
        c.confidence = f;
    }
    if !tags_csv.is_empty() {
        c.tags = tags_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if !options_csv.is_empty() {
        c.options_considered = options_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if !chosen.is_empty() {
        c.chosen = chosen.to_string();
    }
    if let Some(e) = expected {
        c.expected_outcome = Some(e.to_string());
    }

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = store.insert_commitment(&c) {
        eprintln!("failed to persist commitment: {e}");
        std::process::exit(1);
    }
    println!("commitment {} ({})", c.id, kind_s);
    println!("  state:     open");
    println!("  statement: {}", c.statement);
    if let Some(h) = c.horizon {
        println!("  horizon:   {}", h.to_rfc3339());
    }

    // World-model preflight: best-effort, side-effect-free. Failure to
    // load → silent skip. We deliberately run *after* persist so a slow
    // model load can never block a commit; the user sees the prediction
    // as a separate line.
    if preflight {
        emit_preflight(world_path, &c);
    }
}

/// Emit a one-line "your track record" summary if the world model is
/// trained and has enough priors. Quiet when there's nothing to say —
/// silence is the safe failure mode here. See INTENT_SYSTEM.md §6.2.
fn emit_preflight(world_path: &std::path::Path, c: &tm_intent::Commitment) {
    use tm_world_model::{load, PolarityClass};

    // Threshold below which we don't speak — INTENT_SYSTEM.md §5/§7
    // pin support gates around N≥6 to avoid pareidolia.
    const MIN_PRIORS: usize = 6;

    let model = match load(world_path) {
        Ok(Some(m)) if m.is_trained() && m.n_train_examples >= MIN_PRIORS => m,
        _ => return, // missing / dormant / under-supported → silent
    };
    let pred = model.predict(c);

    // Decide tone:
    // - argmax = Worse AND positive_prob < 0.40 → loud warning.
    // - argmax = Better/AsExpected AND positive_prob > 0.65 → quiet
    //   tailwind line.
    // - otherwise → mixed signal line, neutral.
    let positive = pred.positive_prob;
    let pct_better = (pred.dist.0[PolarityClass::Better.index()] * 100.0).round() as i32;
    let pct_as_exp = (pred.dist.0[PolarityClass::AsExpected.index()] * 100.0).round() as i32;
    let pct_worse = (pred.dist.0[PolarityClass::Worse.index()] * 100.0).round() as i32;

    println!();
    if matches!(pred.argmax, PolarityClass::Worse) && positive < 0.40 {
        println!(
            "  ⚠ track record (n={}): {}% worse, {}% better, {}% as-expected.",
            pred.n_priors, pct_worse, pct_better, pct_as_exp,
        );
        println!("    your call — but similar-shaped commitments tend to land worse.");
    } else if positive > 0.65 {
        println!(
            "  ✓ track record (n={}): {}% better/as-expected ({}% better, {}% as-expected).",
            pred.n_priors,
            pct_better + pct_as_exp,
            pct_better,
            pct_as_exp,
        );
    } else {
        println!(
            "  ~ track record (n={}): mixed signal — {}% better, {}% worse, {}% as-expected.",
            pred.n_priors, pct_better, pct_worse, pct_as_exp,
        );
    }
    println!("    (`tracemind world predict` for full breakdown · `--no-preflight` to silence)");
}

fn cmd_resolve(
    intents_path: &str,
    world_path: &std::path::Path,
    commitment_id: &str,
    polarity_s: &str,
    description: &str,
    note: Option<&str>,
) {
    use tm_intent::{state::transition, IntentStore, Outcome, OutcomeSource, Polarity, State};

    let cid = match Uuid::parse_str(commitment_id) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("invalid commitment id '{commitment_id}': {e}");
            std::process::exit(2);
        }
    };
    let polarity = match polarity_s {
        "better" => Polarity::Better,
        "as_expected" => Polarity::AsExpected,
        "worse" => Polarity::Worse,
        "mixed" => Polarity::Mixed,
        "no_outcome" => Polarity::NoOutcome,
        other => {
            eprintln!(
                "invalid --polarity: {other} (expected better|as_expected|worse|mixed|no_outcome)"
            );
            std::process::exit(2);
        }
    };

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };

    let mut c = match store.get_commitment(cid) {
        Ok(Some(c)) => c,
        Ok(None) => {
            eprintln!("no commitment with id {cid}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("lookup failed: {e}");
            std::process::exit(1);
        }
    };

    let mut outcome = Outcome::new(c.id, polarity, description, OutcomeSource::Cli);
    if let Some(n) = note {
        outcome.user_note = Some(n.to_string());
    }

    if let Err(e) = transition(&mut c, State::Completed, Some(&outcome)) {
        eprintln!("state transition rejected: {e}");
        std::process::exit(1);
    }

    if let Err(e) = store.insert_outcome(&outcome) {
        eprintln!("failed to persist outcome: {e}");
        std::process::exit(1);
    }
    if let Err(e) = store.update_state(c.id, c.state, c.outcome_id) {
        eprintln!("failed to update commitment state: {e}");
        std::process::exit(1);
    }

    println!("commitment {cid} → completed ({polarity_s})");
    println!("  outcome:   {}", outcome.id);
    println!("  observed:  {}", outcome.observed_at.to_rfc3339());

    // Ambient retrain — refresh the world model so tomorrow's brief
    // surfaces on this new outcome. Best-effort: failures here never
    // block the resolve. Silent below `min_examples` so the user
    // doesn't see noise on day one.
    if let Some(msg) = auto_retrain_world_model(&store, world_path) {
        println!("  world:     {msg}");
    }
}

/// Re-train the world model after a resolve. Returns a one-line
/// status string when training actually ran (so the CLI/MCP caller
/// can surface a single line of feedback), or `None` when we silently
/// skipped (no examples yet, or persistence failed). Soft-fail
/// throughout — never propagates errors back to the resolve path.
fn auto_retrain_world_model(
    store: &tm_intent::IntentStore,
    world_path: &std::path::Path,
) -> Option<String> {
    use tm_world_model::{from_pairs, save, train, TrainerConfig};
    let cfg = TrainerConfig::default();
    // Pull a year of completed-with-polarity outcomes — same window
    // the explicit `tracemind world train` uses by default.
    let since = chrono::Utc::now() - chrono::Duration::days(365);
    let rows = store.list_completed_with_polarity(since, 5000).ok()?;
    let (examples, _skipped) = from_pairs(rows);
    if examples.len() < cfg.min_examples {
        return None;
    }
    let (model, report) = train(&examples, &cfg);
    save(&model, world_path).ok()?;
    Some(format!(
        "retrained on {} priors (acc {:.0}%)",
        report.n_examples,
        report.final_accuracy * 100.0
    ))
}

fn cmd_commitments(intents_path: &str, limit: usize) {
    use tm_intent::IntentStore;

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };
    let rows = match store.list_open(limit) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("query failed: {e}");
            std::process::exit(1);
        }
    };
    if rows.is_empty() {
        println!("(no open or acted commitments)");
        return;
    }
    for c in rows {
        let horizon = c
            .horizon
            .map(|h| h.to_rfc3339())
            .unwrap_or_else(|| "—".to_string());
        let kind = format!("{:?}", c.kind).to_lowercase();
        let state = format!("{:?}", c.state).to_lowercase();
        println!(
            "{}  [{:<10}]  {:<5}  horizon={}  {}",
            c.id, kind, state, horizon, c.statement
        );
    }
}

// ---------------------------------------------------------------------------
// `tracemind candidates` — mined commitment candidates awaiting review.
// Mirrors the brief's confirm/dismiss surface for terminal users.
// ---------------------------------------------------------------------------

fn cmd_candidates(intents_path: &str, action: CandidatesAction) {
    use tm_intent::IntentStore;

    match action {
        CandidatesAction::List { limit } => {
            let store = match IntentStore::open(intents_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open intent store at {intents_path}: {e}");
                    std::process::exit(1);
                }
            };
            let pending = match store.list_pending_candidates(limit) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("query failed: {e}");
                    std::process::exit(1);
                }
            };
            if pending.is_empty() {
                println!("(no pending candidates)");
                return;
            }
            for c in pending {
                let kind = format!("{:?}", c.kind).to_lowercase();
                println!(
                    "{}  [{:<10}]  conf={:.2}  via=\"{}\"  {}",
                    c.id, kind, c.confidence, c.matched_phrase, c.statement
                );
            }
        }
        CandidatesAction::Accept { candidate_id } => {
            let cid = match Uuid::parse_str(&candidate_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("invalid candidate id '{candidate_id}': {e}");
                    std::process::exit(2);
                }
            };
            let mut store = match IntentStore::open(intents_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open intent store at {intents_path}: {e}");
                    std::process::exit(1);
                }
            };
            match store.accept_candidate(cid) {
                Ok(new_id) => {
                    println!("candidate {cid} → commitment {new_id} (open)");
                }
                Err(e) => {
                    eprintln!("accept failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        CandidatesAction::Dismiss { candidate_id } => {
            let cid = match Uuid::parse_str(&candidate_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("invalid candidate id '{candidate_id}': {e}");
                    std::process::exit(2);
                }
            };
            let store = match IntentStore::open(intents_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open intent store at {intents_path}: {e}");
                    std::process::exit(1);
                }
            };
            match store.dismiss_candidate(cid) {
                Ok(true) => println!("candidate {cid} dismissed"),
                Ok(false) => {
                    eprintln!(
                        "candidate {cid} was not pending (already accepted or dismissed)"
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("dismiss failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// `tracemind brief` — render the daily brief to stdout.
///
/// Two output modes:
/// - text (default) — formatted for terminal reading per
///   `INTENT_SYSTEM.md` §9.1
/// - JSON (`--json`) — the [`tm_reflect::DailyBrief`] structure
///   verbatim; this is the stable contract for agents and external
///   tooling.
fn cmd_brief(intents_path: &str, world_path: &std::path::Path, json: bool, resolved_days: i64) {
    use chrono::{Duration, Utc};
    use tm_intent::IntentStore;
    use tm_reflect::{BriefBuilder, BriefConfig};

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };

    // Best-effort world-model load. Missing file / corrupt schema /
    // dormant model all fall through to "no outlook" — the brief
    // renders fine without it. Surfacing the load error would be noise
    // on first-run installs.
    let world_model = tm_world_model::load(world_path).ok().flatten();

    let cfg = BriefConfig {
        resolved_window: Duration::days(resolved_days),
        ..Default::default()
    };
    let mut builder = BriefBuilder::new(&store).with_config(cfg);
    if let Some(ref m) = world_model {
        builder = builder.with_world_model(m);
    }
    let brief = match builder.build(Utc::now()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("brief failed: {e}");
            std::process::exit(1);
        }
    };

    if json {
        match serde_json::to_string_pretty(&brief) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("brief json failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    print_brief_text(&brief);
}

/// `tracemind patterns` — pattern-detector surface
/// (`INTENT_SYSTEM.md` §5). All actions key off the 16-hex
/// `cell_hash` printed in the brief's "patterns spotted" section.
fn cmd_patterns(intents_path: &str, action: PatternsAction) {
    use chrono::{Duration, Utc};
    use tm_intent::IntentStore;
    use tm_reflect::{BriefBuilder, BriefConfig};

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };

    match action {
        PatternsAction::List => {
            // Reuse the brief builder so `patterns list` is exactly
            // what `brief` would surface — no schema drift.
            let brief = match BriefBuilder::new(&store)
                .with_config(BriefConfig::default())
                .build(Utc::now())
            {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("brief failed: {e}");
                    std::process::exit(1);
                }
            };
            if brief.patterns.is_empty() {
                println!("(no patterns surfacing right now)");
                println!("  needs ≥{} completed commitments globally + ≥{} per cell;",
                    brief.counts.resolved.max(0), tm_reflect::PatternConfig::default().min_n);
                println!("  also filters cells you've silenced.");
                return;
            }
            println!("patterns spotted ({})", brief.patterns.len());
            for p in &brief.patterns {
                println!(
                    "  {}  n={:>3}  lift_worse={:+.2}  support_lb={:.2}",
                    &p.cell_hash, p.n, p.lift_worse, p.support_lb,
                );
                println!("    {}", p.render);
            }
            println!("\n  → `tracemind patterns silence <cell_hash>` to suppress");
        }
        PatternsAction::Silenced => {
            let now = Utc::now();
            let rows = match store.list_active_pattern_silences(now) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("failed to read silences: {e}");
                    std::process::exit(1);
                }
            };
            if rows.is_empty() {
                println!("(no active silences)");
                return;
            }
            println!("active silences ({})", rows.len());
            for s in &rows {
                let until_local = s.silenced_until.with_timezone(&chrono::Local);
                let reason = s.reason.as_deref().unwrap_or("—");
                println!(
                    "  {}  until {}  reason={}\n    cell: {}",
                    s.cell_hash,
                    until_local.format("%b %-d %Y"),
                    reason,
                    s.cell_label,
                );
            }
        }
        PatternsAction::Silence {
            cell_hash,
            days,
            reason,
        } => {
            // Try to look up a current cell label so the silence row
            // is self-describing in `patterns silenced`. Fall back to
            // empty if no current pattern matches the hash (the user
            // may pre-silence a hash they expect to see).
            let label = match BriefBuilder::new(&store)
                .with_config(BriefConfig::default())
                .build(Utc::now())
            {
                Ok(b) => b
                    .patterns
                    .iter()
                    .find(|p| p.cell_hash == cell_hash)
                    .map(|p| p.cell.label())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            };
            let now = Utc::now();
            let until = now + Duration::days(days);
            match store.upsert_pattern_silence(
                &cell_hash,
                &label,
                now,
                until,
                reason.as_deref(),
            ) {
                Ok(()) => {
                    let until_local = until.with_timezone(&chrono::Local);
                    println!(
                        "silenced cell {} until {} ({} days)",
                        cell_hash,
                        until_local.format("%b %-d %Y"),
                        days,
                    );
                    if !label.is_empty() {
                        println!("  cell: {label}");
                    }
                }
                Err(e) => {
                    eprintln!("failed to silence: {e}");
                    std::process::exit(1);
                }
            }
        }
        PatternsAction::Unsilence { cell_hash } => match store.remove_pattern_silence(&cell_hash) {
            Ok(true) => println!("unsilenced {cell_hash}"),
            Ok(false) => println!("no silence found for {cell_hash}"),
            Err(e) => {
                eprintln!("failed to unsilence: {e}");
                std::process::exit(1);
            }
        },
    }
}

fn print_brief_text(brief: &tm_reflect::DailyBrief) {
    let local_now = brief.generated_at.with_timezone(&chrono::Local);
    println!(
        "TRACEMIND BRIEF                  {}",
        local_now.format("%a %b %d, %-I:%M %p")
    );
    println!(
        "  overdue: {}    open: {}    resolved: {}    candidates: {}    patterns: {}    insights: {}",
        brief.counts.overdue,
        brief.counts.open,
        brief.counts.resolved,
        brief.counts.candidates,
        brief.counts.patterns,
        brief.counts.insights,
    );
    println!();

    // Insights panel — printed first so the user sees the most
    // attention-worthy rows before drowning in the open list.
    if !brief.insights.is_empty() {
        println!("▸ insights ({})", brief.insights.len());
        for ins in &brief.insights {
            let glyph = match ins.tone.as_str() {
                "warning" => "⚠",
                "tailwind" => "✓",
                _ => "~",
            };
            println!("    {} {}", glyph, ins.render);
        }
        println!("    (model deviation from your completed-rate baseline; n={} priors)",
            brief.insights.first().map(|i| i.n_priors).unwrap_or(0));
        println!();
    }

    if !brief.overdue.is_empty() {
        println!("▸ overdue ({})", brief.overdue.len());
        for row in &brief.overdue {
            let bucket = match row.overdue_class {
                Some(tm_reflect::OverdueClass::DueToday) => "due today",
                Some(tm_reflect::OverdueClass::OverdueRecent) => "overdue",
                Some(tm_reflect::OverdueClass::OverdueStale) => "stale",
                None => "—",
            };
            let horizon = row
                .horizon
                .map(|h| h.with_timezone(&chrono::Local).format("%b %-d").to_string())
                .unwrap_or_else(|| "—".into());
            println!(
                "    {}  [{:>9}]  by {:>6}  {}",
                short_id(row.id),
                bucket,
                horizon,
                truncate_str(&row.statement, 60)
            );
            print_outlook_line(row.outlook.as_ref());
        }
        println!();
    }

    if !brief.open.is_empty() {
        println!("▸ open intents ({})", brief.open.len());
        for row in &brief.open {
            let kind = match row.kind {
                tm_intent::CommitmentKind::Intent => "intent",
                tm_intent::CommitmentKind::Decision => "decision",
                tm_intent::CommitmentKind::Hypothesis => "hypothesis",
            };
            let horizon = row
                .horizon
                .map(|h| h.with_timezone(&chrono::Local).format("%b %-d").to_string())
                .unwrap_or_else(|| "—".into());
            println!(
                "    {}  [{:>10}]  by {:>6}  {}",
                short_id(row.id),
                kind,
                horizon,
                truncate_str(&row.statement, 60)
            );
            print_outlook_line(row.outlook.as_ref());
        }
        println!();
    }

    if !brief.resolved.is_empty() {
        println!("▸ resolved (last 7d) ({})", brief.resolved.len());
        for row in &brief.resolved {
            let polarity = row
                .polarity
                .map(|p| match p {
                    tm_intent::Polarity::Better => "better",
                    tm_intent::Polarity::AsExpected => "as-expected",
                    tm_intent::Polarity::Worse => "worse",
                    tm_intent::Polarity::Mixed => "mixed",
                    tm_intent::Polarity::NoOutcome => "no-outcome",
                })
                .unwrap_or("—");
            let state = match row.state {
                tm_intent::State::Completed => "completed",
                tm_intent::State::Abandoned => "abandoned",
                tm_intent::State::Superseded => "superseded",
                _ => "?", // not expected — list_recent_resolved filters terminal states only
            };
            println!(
                "    {}  [{:>10} / {:>11}]  {}",
                short_id(row.id),
                state,
                polarity,
                truncate_str(&row.statement, 60)
            );
        }
        println!();
    }

    if !brief.candidates.is_empty() {
        println!("▸ pending candidates ({})", brief.candidates.len());
        for row in &brief.candidates {
            let kind = match row.kind {
                tm_intent::CommitmentKind::Intent => "intent",
                tm_intent::CommitmentKind::Decision => "decision",
                tm_intent::CommitmentKind::Hypothesis => "hypothesis",
            };
            println!(
                "    {}  [{:>10}]  conf={:.2}  via=\"{}\"  {}",
                short_id(row.id),
                kind,
                row.confidence,
                row.matched_phrase,
                truncate_str(&row.statement, 60)
            );
        }
        println!("\n  → `tracemind candidates accept|dismiss <id>` to triage");
        println!();
    }

    // Patterns — `INTENT_SYSTEM.md` §5 / §9.1. Statistical, factual,
    // citation-friendly tone. The detector pre-renders the headline
    // template per §5.1.4; we just frame it. Each row includes the
    // 16-hex `cell_hash` the user can pass to `tracemind patterns
    // silence <hash>` to suppress.
    if !brief.patterns.is_empty() {
        println!("▸ patterns spotted ({})", brief.patterns.len());
        for p in &brief.patterns {
            println!(
                "    {}  n={:>3}  lift_worse={:+.2}  support_lb={:.2}",
                &p.cell_hash, p.n, p.lift_worse, p.support_lb,
            );
            println!("      {}", p.render);
        }
        println!("    (correlation only — your call on what to do)");
        println!("    → `tracemind patterns silence <cell_hash>` to suppress");
        println!();
    }

    if brief.counts.overdue == 0
        && brief.counts.open == 0
        && brief.counts.resolved == 0
        && brief.counts.candidates == 0
        && brief.counts.patterns == 0
    {
        println!("(empty — no commitments yet. try `tracemind commit` or capture some text.)");
    }
}

fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Render the world-model outlook one-liner under a brief row. The
/// glyph mirrors the tone classification done in `tm-reflect::brief`
/// (warning / tailwind / mixed) so the visual cue matches the JSON
/// `tone` field. No-op when `outlook` is None — the brief stays
/// honest about cold starts and dormant models.
fn print_outlook_line(outlook: Option<&tm_reflect::CommitmentOutlook>) {
    let Some(o) = outlook else { return };
    let glyph = match o.tone.as_str() {
        "warning" => "⚠",
        "tailwind" => "✓",
        _ => "~",
    };
    println!(
        "        {} outlook: {} {:.0}%  (positive {:.0}%, n={})",
        glyph,
        o.argmax,
        o.confidence * 100.0,
        o.positive_prob * 100.0,
        o.n_priors,
    );
}

// ---------------------------------------------------------------------------
// `tracemind world` — f_outcome predictor (`docs/INTENT_SYSTEM.md` §7).
// v0: multinomial logistic regression on metadata features. The training
// corpus is the user's *own* completed commitments — no aggregate priors,
// no remote calls, no embeddings (yet — that's v1).
// ---------------------------------------------------------------------------

fn cmd_world(intents_path: &str, world_path: &std::path::Path, action: WorldAction) {
    use tm_intent::IntentStore;
    use tm_world_model::{
        explain_top_k, from_pairs, load, save, train, OutcomeModel, TrainerConfig,
    };

    match action {
        WorldAction::Train {
            since_days,
            epochs,
            lr,
            l2,
            vocab,
            min_examples,
            limit,
            json,
        } => {
            let store = match IntentStore::open(intents_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open intent store at {intents_path}: {e}");
                    std::process::exit(1);
                }
            };
            let since = chrono::Utc::now() - chrono::Duration::days(since_days);
            let rows = match store.list_completed_with_polarity(since, limit) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("failed to read completed commitments: {e}");
                    std::process::exit(1);
                }
            };
            let total_completed = rows.len();
            let (examples, skipped) = from_pairs(rows);

            let cfg = TrainerConfig {
                epochs,
                lr,
                l2,
                tag_vocab_size: vocab,
                min_examples,
            };
            let (model, mut report) = train(&examples, &cfg);
            report.skipped_no_outcome = skipped;

            if let Err(e) = save(&model, world_path) {
                eprintln!("failed to persist world model to {}: {e}", world_path.display());
                std::process::exit(1);
            }

            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("json serialize failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            println!("world model trained → {}", world_path.display());
            println!(
                "  examples       : {} (from {} completed; {} skipped no-outcome)",
                report.n_examples, total_completed, report.skipped_no_outcome
            );
            println!("  classes seen   : {}/4", report.n_classes_seen);
            println!("  feature dim    : {} ({} fixed + {} tags)",
                report.feature_dim,
                tm_world_model::FIXED_FEATURES,
                report.vocab_size,
            );
            println!("  epochs run     : {}", report.epochs_run);
            if report.epochs_run > 0 {
                println!("  final loss     : {:.4}", report.final_loss);
                println!("  final accuracy : {:.1}%", report.final_accuracy * 100.0);
            } else {
                println!(
                    "  (dormant — need ≥{} usable examples; have {}.)",
                    cfg.min_examples, report.n_examples
                );
            }
        }

        WorldAction::Predict {
            kind,
            statement,
            stakes,
            confidence,
            tags,
            horizon,
            options,
            explain,
            top_k,
            json,
        } => {
            use tm_intent::{Commitment, CommitmentKind, Source, Stakes};

            let kind_enum = match kind.as_str() {
                "intent" => CommitmentKind::Intent,
                "decision" => CommitmentKind::Decision,
                "hypothesis" => CommitmentKind::Hypothesis,
                other => {
                    eprintln!("invalid --kind: {other} (expected intent|decision|hypothesis)");
                    std::process::exit(2);
                }
            };
            let statement = statement.trim().to_string();
            if statement.is_empty() {
                eprintln!("statement must be non-empty");
                std::process::exit(2);
            }

            let mut probe = Commitment::new(kind_enum, statement, Source::Cli);
            if let Some(s) = stakes.as_deref() {
                probe.stakes = match s {
                    "low" => Stakes::Low,
                    "medium" => Stakes::Medium,
                    "high" => Stakes::High,
                    "reversible" => Stakes::Reversible,
                    other => {
                        eprintln!("invalid --stakes: {other}");
                        std::process::exit(2);
                    }
                };
            }
            if let Some(f) = confidence {
                if !(0.0..=1.0).contains(&f) {
                    eprintln!("--confidence must be in [0,1], got {f}");
                    std::process::exit(2);
                }
                probe.confidence = f;
            }
            if !tags.is_empty() {
                probe.tags = tags
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            if let Some(h) = horizon.as_deref() {
                match chrono::DateTime::parse_from_rfc3339(h) {
                    Ok(t) => probe.horizon = Some(t.with_timezone(&chrono::Utc)),
                    Err(e) => {
                        eprintln!("invalid --horizon (need RFC3339): {e}");
                        std::process::exit(2);
                    }
                }
            }
            if !options.is_empty() {
                probe.options_considered = options
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }

            let model: OutcomeModel = match load(world_path) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    eprintln!(
                        "no world model on disk yet — run `tracemind world train` first.\n\
                         (looked at {})",
                        world_path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("failed to load world model: {e}");
                    std::process::exit(1);
                }
            };

            let pred = model.predict(&probe);
            let explanation = if explain && model.is_trained() {
                Some(explain_top_k(&model, &probe, pred.argmax, top_k))
            } else {
                None
            };

            if json {
                #[derive(serde::Serialize)]
                struct Out<'a> {
                    prediction: &'a tm_world_model::OutcomePrediction,
                    explanation: &'a Option<Vec<tm_world_model::FeatureContribution>>,
                }
                match serde_json::to_string_pretty(&Out {
                    prediction: &pred,
                    explanation: &explanation,
                }) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("json serialize failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            if !model.is_trained() {
                println!("world model is dormant (untrained) — predictions are uniform.");
                println!("  → run `tracemind world train` once you have ≥6 completed commitments.");
                return;
            }

            let labels = ["better", "as_expected", "worse", "mixed"];
            println!("prediction (n_priors={}):", pred.n_priors);
            for (i, &l) in labels.iter().enumerate() {
                let bar_len = (pred.dist.0[i] * 40.0).round() as usize;
                let bar = "█".repeat(bar_len);
                println!("  {:<12} {:>5.1}%  {}", l, pred.dist.0[i] * 100.0, bar);
            }
            println!(
                "  argmax       : {:?}  (positive_prob={:.1}%, confidence={:.2})",
                pred.argmax,
                pred.positive_prob * 100.0,
                pred.confidence,
            );

            if let Some(ex) = explanation {
                println!("\ntop-{} contributors to {:?}:", ex.len(), pred.argmax);
                for f in ex {
                    let sign = if f.contribution >= 0.0 { "+" } else { "−" };
                    println!(
                        "  {} {:<22} φ={:.2}  w={:+.3}  Δlogit={:+.3}",
                        sign,
                        f.label,
                        f.value,
                        f.weight,
                        f.contribution.abs() * f.contribution.signum(),
                    );
                }
            }
        }

        WorldAction::Status { json } => {
            let model_opt: Option<OutcomeModel> = match load(world_path) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("failed to load world model: {e}");
                    std::process::exit(1);
                }
            };

            if json {
                #[derive(serde::Serialize)]
                struct Status {
                    path: String,
                    present: bool,
                    schema_version: Option<u32>,
                    n_train_examples: Option<usize>,
                    trained_at: Option<String>,
                    feature_dim: Option<usize>,
                    vocab_size: Option<usize>,
                }
                let s = match &model_opt {
                    Some(m) => Status {
                        path: world_path.display().to_string(),
                        present: true,
                        schema_version: Some(m.schema_version),
                        n_train_examples: Some(m.n_train_examples),
                        trained_at: m.trained_at.clone(),
                        feature_dim: Some(m.feature_dim()),
                        vocab_size: Some(m.tag_vocab.len()),
                    },
                    None => Status {
                        path: world_path.display().to_string(),
                        present: false,
                        schema_version: None,
                        n_train_examples: None,
                        trained_at: None,
                        feature_dim: None,
                        vocab_size: None,
                    },
                };
                match serde_json::to_string_pretty(&s) {
                    Ok(s) => println!("{}", s),
                    Err(e) => {
                        eprintln!("json serialize failed: {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }

            match model_opt {
                None => {
                    println!("world model: (none on disk)");
                    println!("  expected at: {}", world_path.display());
                    println!("  → run `tracemind world train` to bootstrap.");
                }
                Some(m) => {
                    println!("world model:");
                    println!("  path           : {}", world_path.display());
                    println!("  schema version : v{}", m.schema_version);
                    println!("  trained        : {}", m.is_trained());
                    println!("  n_train        : {}", m.n_train_examples);
                    println!(
                        "  trained_at     : {}",
                        m.trained_at.as_deref().unwrap_or("—")
                    );
                    println!("  feature dim    : {}", m.feature_dim());
                    println!("  tag vocab      : {} entries", m.tag_vocab.len());
                    if !m.tag_vocab.is_empty() {
                        let preview: Vec<&str> = m
                            .tag_vocab
                            .entries()
                            .iter()
                            .take(5)
                            .map(|s| s.as_str())
                            .collect();
                        println!("    top tags     : {}", preview.join(", "));
                    }
                }
            }
        }
    }
}
