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
    /// Sprint C-0 — flag a retrieval result as not related to its query.
    /// Writes a row to `negative_signals`; subsequent bandit rewards on
    /// the same query_id are decomposed as
    /// `final_reward = relevance_reward - Σ weight(negatives)`.
    /// `result_id` is opaque (triple UUID, entity UUID, or signal row id).
    NotRelated {
        /// UUID of the query whose result you're flagging.
        query_id: String,
        /// Opaque id of the offending result.
        result_id: String,
        /// Negative weight in (0, ∞). Default 1.0.
        #[arg(long, default_value = "1.0")]
        weight: f32,
        /// Kind tag — defaults to `not_related`. Free-form, lets future
        /// surfaces split negatives by reason (e.g. `wrong_context`,
        /// `stale`, `private`).
        #[arg(long, default_value = "not_related")]
        kind: String,
        /// Optional source context UUID (the query's active context).
        #[arg(long)]
        context_a: Option<String>,
        /// Optional target context UUID (the offending result's context).
        #[arg(long)]
        context_b: Option<String>,
    },
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
    /// Manage *contexts* — named namespaces for the local memory store.
    /// Every captured signal + triple inherits the active context_id at
    /// ingest time; retrieval defaults to the active scope. See Sprint
    /// C-0 in `docs/DESIGN.md` §9.5. (`tracemind context use <name>`)
    Context {
        #[command(subcommand)]
        action: ContextAction,
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
    /// Insights surface — silence / unsilence per-commitment outlook
    /// divergences shown in the daily brief. Unit of silence is the
    /// commitment_id (the *open row*); silencing one row never
    /// removes the row itself, only the insight panel highlight.
    /// See TM-INTENT-006 + TM-INTENT-007.
    Insights {
        #[command(subcommand)]
        action: InsightsAction,
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
    /// Persistent outcome proposals from the implicit text matcher
    /// (`docs/INTENT_SYSTEM.md` §4.2). The brief surfaces active
    /// proposals; this surface lets you accept / dismiss them.
    Outcomes {
        #[command(subcommand)]
        action: OutcomesAction,
    },
    /// Record a Need — the 'why' behind commitments. Needs drive the
    /// intent arc: Need → Sentiment → Commitment → Action → Outcome.
    Need {
        /// What the user needs.
        statement: String,
        /// Urgency 0.0 (low) to 1.0 (critical). Default 0.5.
        #[arg(long)]
        urgency: Option<f32>,
        /// True for ongoing needs (health, learning).
        #[arg(long)]
        recurring: bool,
        /// Comma-separated tags.
        #[arg(long, default_value = "")]
        tags: String,
        /// Commitment UUID to link this need to.
        #[arg(long)]
        link: Option<String>,
    },
    /// Record sentiment toward a target (commitment, need, entity, topic).
    Sentiment {
        /// UUID of the target.
        target_id: String,
        /// Target type: commitment | need | entity | topic.
        #[arg(long)]
        target_type: String,
        /// Valence from -1.0 (negative) to +1.0 (positive).
        valence: f64,
        /// Evidence text that triggered this sentiment.
        #[arg(long)]
        evidence: Option<String>,
    },
    /// Record an action taken toward a commitment.
    Action {
        /// What was done.
        description: String,
        /// Commitment UUID this action relates to.
        #[arg(long)]
        commitment_id: Option<String>,
        /// Modality: digital | physical | communication | creation.
        #[arg(long, default_value = "digital")]
        modality: String,
    },
    /// Show the full intent arc for a commitment (Need → Sentiment →
    /// Commitment → Action → Outcome).
    Arc {
        /// Commitment UUID.
        commitment_id: String,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Demo helpers — restore a deterministic `~/.tracemind/` snapshot
    /// for recording the 3-minute walkthrough.
    Demo {
        #[command(subcommand)]
        action: DemoAction,
    },
    /// JTMS contradictions surface — list outstanding rows + resolve
    /// them. Mirrors the Tauri brief drawer (Shot 2 of the demo) so
    /// the same retraction beat works without the desktop app.
    Contradictions {
        #[command(subcommand)]
        action: ContradictionsAction,
    },
}

#[derive(clap::Subcommand)]
enum ContradictionsAction {
    /// List outstanding contradictions (resolved ones are filtered out).
    List {
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Resolve a contradiction by triple-pair. Use the triple UUIDs
    /// from `contradictions list` (or the brief). The choice maps to:
    ///
    /// - `keep-a`     → retract triple B
    /// - `keep-b`     → retract triple A
    /// - `keep-both`  → neither retracted; mark resolved
    Resolve {
        /// First triple UUID (the "A" side).
        triple_a: String,
        /// Second triple UUID (the "B" side).
        triple_b: String,
        /// One of: keep-a | keep-b | keep-both.
        choice: String,
    },
}

#[derive(clap::Subcommand)]
enum DemoAction {
    /// Wipe the data directory and rebuild a deterministic fixture
    /// (~15 entities, ~18 triples, 1 contradiction, 4 open + 5
    /// resolved commitments) so the brief and the retraction beat
    /// look the same on every run.
    Restore {
        /// Required when the data dir is non-empty. Wipes everything
        /// under `$TM_DATA_DIR` (default `~/.tracemind/`) before
        /// rebuilding. Without this, restore refuses to clobber an
        /// existing install.
        #[arg(long)]
        force: bool,
    },
    /// Run the ambient-capture daemon silently for N seconds. Used as
    /// the off-camera pre-roll for the demo recording so the brief
    /// already has fresh capture context when the camera rolls.
    ///
    /// Spawns `tracemind-capture` (or the path in `$TM_CAPTURE_BIN`)
    /// as a child process with stdout/stderr fully suppressed,
    /// waits, then sends SIGTERM and reaps it. No prompts, no
    /// terminal flicker.
    Preroll {
        /// Pre-roll duration in seconds. Demo script default is 30s.
        #[arg(long, default_value = "30")]
        seconds: u64,
        /// Print a one-line confirmation before/after instead of
        /// staying fully silent. Off by default for the recording.
        #[arg(long)]
        verbose: bool,
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
enum InsightsAction {
    /// List currently-surfacing insights (the same set the brief
    /// would show right now, after silences).
    List,
    /// List active insight silences. Useful before unsilencing.
    Silenced,
    /// Silence the insight surface on a specific commitment for some
    /// window. The commitment row itself is not affected — only the
    /// insight panel highlight is suppressed.
    Silence {
        /// Commitment UUID from the brief's "insights" or "open" sections.
        commitment_id: String,
        /// Silence window in days. Default 30 — shorter than the
        /// pattern silence default (90) because insights are
        /// per-commitment and self-resolve when the commitment moves
        /// to a terminal state.
        #[arg(long, default_value = "30")]
        days: i64,
        /// Optional reason — stored alongside the silence row.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Re-enable insight surfacing for a previously-silenced commitment.
    Unsilence { commitment_id: String },
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
        /// Architecture: `linear` (default) or `mlp` / `mlp:N` for a
        /// 1-hidden-layer ReLU MLP with hidden width N (default 8).
        #[arg(long, default_value = "linear")]
        arch: String,
        /// Fraction held out for validation. `0.0` disables the split.
        #[arg(long, default_value = "0.2")]
        validation_split: f32,
        /// Stop early after this many epochs without val-loss
        /// improvement. `0` disables early stopping.
        #[arg(long, default_value = "25")]
        patience: usize,
        /// Seed for the deterministic train/val shuffle.
        #[arg(long, default_value = "661466")]
        seed: u64,
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
    /// Score the world model's predictions against eventual resolution
    /// polarity. Out-of-sample by default — only commitments resolved
    /// after `model.trained_at` are scored — so the report says
    /// something honest about generalization. Pass `--all` to include
    /// in-sample rows too (useful for sanity-checking a fresh train).
    Calibration {
        /// Look-back window in days for completed commitments.
        #[arg(long, default_value = "365")]
        since_days: i64,
        /// Cap on rows fetched from the intent store.
        #[arg(long, default_value = "5000")]
        limit: usize,
        /// Skip the out-of-sample filter and score every completed row,
        /// even rows the model trained on. Off by default; this is a
        /// debug knob, not the trust-building surface.
        #[arg(long)]
        all: bool,
        /// Emit the CalibrationReport as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand)]
enum OutcomesAction {
    /// Show active (pending, unexpired) outcome proposals.
    List {
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Render as JSON for agent / external consumption.
        #[arg(long)]
        json: bool,
    },
    /// Accept a proposal — promotes it into a real `Outcome` row,
    /// transitions the underlying commitment, and links the proposal
    /// to the new outcome id.
    Accept {
        /// Proposal UUID from `outcomes list`.
        proposal_id: String,
        /// Optional free-text user note for the resulting Outcome.
        #[arg(long)]
        note: Option<String>,
    },
    /// Dismiss a proposal — marks it terminal so the brief stops
    /// surfacing it. Does *not* touch the underlying commitment.
    Dismiss {
        /// Proposal UUID from `outcomes list`.
        proposal_id: String,
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
enum ContextAction {
    /// Create a new context. Idempotent on name — re-running with the
    /// same name is a no-op (the existing row is preserved).
    Create {
        /// Short name (e.g. `rondo`, `tracemind`, `personal`).
        name: String,
        /// Optional comma-separated tags.
        #[arg(long, default_value = "")]
        tags: String,
    },
    /// List every context, newest first.
    List,
    /// Make a context the *active* one. Every subsequent ingest tags
    /// rows with this context_id; every retrieval is scoped to it
    /// unless `--cross-context` is passed.
    Use { name: String },
    /// Print the active context (if any).
    Current,
    /// Clear the active context (back to unscoped behaviour).
    Clear,
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
                        // TM-INTENT-009: persist polarity-hinted
                        // proposals so `tracemind brief` can carry
                        // them forward into the daily view, not just
                        // print them once at ingest-time.
                        match store.list_open(50) {
                            Ok(opens) if !opens.is_empty() => {
                                let cfg = tm_reflect::MatcherConfig::default();
                                let proposals =
                                    tm_reflect::propose_outcomes(&text, &opens, &cfg);
                                if !proposals.is_empty() {
                                    let now = chrono::Utc::now();
                                    let expiry = now + chrono::Duration::days(30);
                                    let mut persisted = 0usize;
                                    println!(
                                        "  + {} possible outcome match(es) — run `tracemind outcomes list`:",
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
                                        if let Some(polarity) = p.polarity_hint {
                                            let record = tm_intent::OutcomeProposal {
                                                id: uuid::Uuid::new_v4(),
                                                commitment_id: p.commitment_id,
                                                cell_key: tm_intent::OutcomeProposal::cell_key_for(
                                                    p.commitment_id,
                                                    polarity,
                                                ),
                                                proposed_polarity: polarity,
                                                description: p.reason.clone(),
                                                similarity: p.score,
                                                source_trace_id: None,
                                                proposed_at: now,
                                                expires_at: expiry,
                                                status: "pending".into(),
                                                resolved_at: None,
                                                resolved_outcome_id: None,
                                            };
                                            match store.insert_outcome_proposal(&record) {
                                                Ok(true) => persisted += 1,
                                                Ok(false) => {}
                                                Err(e) => eprintln!(
                                                    "      (persist proposal failed: {e})"
                                                ),
                                            }
                                        }
                                    }
                                    if persisted > 0 {
                                        println!("    ({} persisted into daily brief)", persisted);
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

        Commands::NotRelated {
            query_id,
            result_id,
            weight,
            kind,
            context_a,
            context_b,
        } => {
            let qid = match Uuid::parse_str(&query_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("error: invalid query_id (expect UUID): {e}");
                    std::process::exit(1);
                }
            };
            let ca = context_a
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .unwrap_or_else(|e| {
                    eprintln!("error: invalid --context-a: {e}");
                    std::process::exit(1);
                });
            let cb = context_b
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .unwrap_or_else(|e| {
                    eprintln!("error: invalid --context-b: {e}");
                    std::process::exit(1);
                });
            let graph = match GraphStore::open(&db_path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("error: failed to open graph store: {e}");
                    std::process::exit(1);
                }
            };
            let row_id = match graph.write_negative_signal(qid, &result_id, &kind, ca, cb, weight) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: failed to write negative signal: {e}");
                    std::process::exit(1);
                }
            };
            println!(
                "negative signal recorded (row {row_id}, query {qid}, result {result_id}, weight {weight}, kind {kind})"
            );
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
            let graph_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_brief(&intents_path, &world_path, &graph_path, json, resolved_days);
        }
        Commands::Patterns { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_patterns(&intents_path, action);
        }
        Commands::Insights { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            cmd_insights(&intents_path, &world_path, action);
        }
        Commands::World { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            let world_path = dir.join("world_model.json");
            cmd_world(&intents_path, &world_path, action);
        }
        Commands::Outcomes { action } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_outcomes(&intents_path, action);
        }
        Commands::Need {
            statement,
            urgency,
            recurring,
            tags,
            link,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_need(&intents_path, &statement, urgency, recurring, &tags, link.as_deref());
        }
        Commands::Sentiment {
            target_id,
            target_type,
            valence,
            evidence,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_sentiment(&intents_path, &target_id, &target_type, valence, evidence.as_deref());
        }
        Commands::Action {
            description,
            commitment_id,
            modality,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_action(&intents_path, &description, commitment_id.as_deref(), &modality);
        }
        Commands::Arc {
            commitment_id,
            json,
        } => {
            let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
            cmd_arc(&intents_path, &commitment_id, json);
        }
        Commands::Demo { action } => {
            cmd_demo(&dir, action);
        }
        Commands::Contradictions { action } => {
            let db_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_contradictions(&db_path, action);
        }
        Commands::Context { action } => {
            let db_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_context(&dir, &db_path, action);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `tracemind context …` handler — Sprint C-0 surface for the context
/// segmentation primitive. The active context is persisted in
/// `<data_dir>/active_context.json` so every subsequent CLI invocation
/// (and the MCP server, which reads the same file) picks up the scope.
fn cmd_context(data_dir: &PathBuf, db_path: &str, action: ContextAction) {
    use tm_graph::context::{ActiveContext, Context};

    let active_path = data_dir.join("active_context.json");

    match action {
        ContextAction::Create { name, tags } => {
            let graph = match GraphStore::open(db_path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("error: failed to open graph store: {e}");
                    std::process::exit(1);
                }
            };
            // Idempotent: if a row with that name exists, surface it.
            if let Ok(Some(existing)) = graph.get_context_by_name(&name) {
                println!("context already exists: {} ({})", existing.name, existing.id);
                return;
            }
            let ctx = Context::new(name.clone(), tags.clone());
            if let Err(e) = graph.create_context(&ctx) {
                eprintln!("error: failed to create context: {e}");
                std::process::exit(1);
            }
            println!("created context: {} ({})", ctx.name, ctx.id);
            if !tags.is_empty() {
                println!("  tags: {}", tags);
            }
        }
        ContextAction::List => {
            let graph = match GraphStore::open(db_path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("error: failed to open graph store: {e}");
                    std::process::exit(1);
                }
            };
            let contexts = match graph.list_contexts() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: failed to list contexts: {e}");
                    std::process::exit(1);
                }
            };
            let active = ActiveContext::load(&active_path).ok().flatten();
            if contexts.is_empty() {
                println!("no contexts yet — `tracemind context create <name>` to add one");
                return;
            }
            for c in contexts {
                let marker = match &active {
                    Some(a) if a.id == c.id => "*",
                    _ => " ",
                };
                let tags = if c.tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", c.tags)
                };
                println!("{marker} {:<24} {}{}", c.name, c.id, tags);
            }
        }
        ContextAction::Use { name } => {
            let graph = match GraphStore::open(db_path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("error: failed to open graph store: {e}");
                    std::process::exit(1);
                }
            };
            let ctx = match graph.get_context_by_name(&name) {
                Ok(Some(c)) => c,
                Ok(None) => {
                    eprintln!("error: no context named '{}' (create it first)", name);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
            let active = ActiveContext { id: ctx.id, name: ctx.name.clone() };
            if let Err(e) = active.save(&active_path) {
                eprintln!("error: failed to save active context: {e}");
                std::process::exit(1);
            }
            println!("active context → {} ({})", ctx.name, ctx.id);
        }
        ContextAction::Current => {
            match ActiveContext::load(&active_path) {
                Ok(Some(a)) => println!("active context: {} ({})", a.name, a.id),
                Ok(None) => println!("no active context (unscoped)"),
                Err(e) => {
                    eprintln!("error: failed to read active context: {e}");
                    std::process::exit(1);
                }
            }
        }
        ContextAction::Clear => {
            if let Err(e) = ActiveContext::clear(&active_path) {
                eprintln!("error: failed to clear active context: {e}");
                std::process::exit(1);
            }
            println!("active context cleared");
        }
    }
}

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
// `tracemind outcomes` — persistent outcome proposals from the implicit
// text matcher (`docs/INTENT_SYSTEM.md` §4.2 + TM-INTENT-009). Lets the
// user accept (promote into a real Outcome) or dismiss the proposals
// the brief is surfacing.
// ---------------------------------------------------------------------------

fn cmd_outcomes(intents_path: &str, action: OutcomesAction) {
    use tm_intent::{state::transition, IntentStore, Outcome, OutcomeSource, State};

    let store = match IntentStore::open(intents_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to open intent store at {intents_path}: {e}");
            std::process::exit(1);
        }
    };

    match action {
        OutcomesAction::List { limit, json } => {
            let now = chrono::Utc::now();
            // Best-effort: flip stale rows to expired so the list
            // matches reality even if no brief has run lately.
            let _ = store.expire_outcome_proposals(now);

            let rows = match store.list_active_outcome_proposals(now, limit) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("query failed: {e}");
                    std::process::exit(1);
                }
            };
            if json {
                let out: Vec<_> = rows
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id.to_string(),
                            "commitment_id": p.commitment_id.to_string(),
                            "proposed_polarity": format!("{:?}", p.proposed_polarity).to_lowercase(),
                            "description": p.description,
                            "similarity": p.similarity,
                            "proposed_at": p.proposed_at.to_rfc3339(),
                            "expires_at": p.expires_at.to_rfc3339(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
                return;
            }
            if rows.is_empty() {
                println!("(no active outcome proposals — clean slate)");
                return;
            }
            println!(
                "{} active outcome proposal(s) — `tracemind outcomes accept <id>` or `dismiss <id>`:",
                rows.len()
            );
            for p in rows {
                let pol = format!("{:?}", p.proposed_polarity).to_lowercase();
                println!(
                    "  {}  [{:<11}]  score={:.2}  → {}",
                    short_id(p.id),
                    pol,
                    p.similarity,
                    truncate_str(&p.description, 60),
                );
                println!(
                    "      ↳ commitment {}  proposed_at={}",
                    short_id(p.commitment_id),
                    p.proposed_at.to_rfc3339()
                );
            }
        }
        OutcomesAction::Accept { proposal_id, note } => {
            let pid = match Uuid::parse_str(&proposal_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("invalid proposal id '{proposal_id}': {e}");
                    std::process::exit(2);
                }
            };
            let proposal = match store.get_outcome_proposal(pid) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    eprintln!("no proposal with id {pid}");
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("lookup failed: {e}");
                    std::process::exit(1);
                }
            };
            if proposal.status != "pending" {
                eprintln!(
                    "proposal {pid} is already {} (no-op)",
                    proposal.status
                );
                std::process::exit(1);
            }
            let mut c = match store.get_commitment(proposal.commitment_id) {
                Ok(Some(c)) => c,
                Ok(None) => {
                    eprintln!("commitment {} not found", proposal.commitment_id);
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("commitment lookup failed: {e}");
                    std::process::exit(1);
                }
            };

            let mut outcome = Outcome::new(
                c.id,
                proposal.proposed_polarity,
                &proposal.description,
                OutcomeSource::ImplicitMatched,
            );
            if let Some(n) = note {
                outcome.user_note = Some(n);
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
            let now = chrono::Utc::now();
            if let Err(e) = store.mark_outcome_proposal_accepted(pid, outcome.id, now) {
                eprintln!(
                    "warning: outcome was created but proposal mark-accept failed: {e}"
                );
            }
            println!("proposal {pid} → accepted");
            println!("  commitment {} → completed", c.id);
            println!("  outcome    {}", outcome.id);
        }
        OutcomesAction::Dismiss { proposal_id } => {
            let pid = match Uuid::parse_str(&proposal_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("invalid proposal id '{proposal_id}': {e}");
                    std::process::exit(2);
                }
            };
            let now = chrono::Utc::now();
            match store.mark_outcome_proposal_dismissed(pid, now) {
                Ok(true) => println!("proposal {pid} → dismissed"),
                Ok(false) => {
                    eprintln!("proposal {pid} is not pending (already terminal or unknown)");
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
fn cmd_brief(
    intents_path: &str,
    world_path: &std::path::Path,
    graph_path: &str,
    json: bool,
    resolved_days: i64,
) {
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

    // Best-effort graph attach so the brief can surface JTMS-flagged
    // contradictions (Sprint C-2). A missing graph DB on first-run
    // installs is fine — the contradictions section just stays empty.
    let graph = GraphStore::open(graph_path).ok();

    let cfg = BriefConfig {
        resolved_window: Duration::days(resolved_days),
        ..Default::default()
    };
    let mut builder = BriefBuilder::new(&store).with_config(cfg);
    if let Some(ref m) = world_model {
        builder = builder.with_world_model(m);
    }
    if let Some(ref g) = graph {
        builder = builder.with_graph(g);
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

fn cmd_insights(intents_path: &str, world_path: &std::path::Path, action: InsightsAction) {
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
        InsightsAction::List => {
            // Mirror `patterns list` — reuse the brief builder so the
            // CLI surface matches the daily brief exactly. World model
            // load is best-effort: a missing model means insights
            // can't be computed, and we explain why.
            let model = tm_world_model::load(world_path).ok().flatten();
            let mut builder = BriefBuilder::new(&store).with_config(BriefConfig::default());
            if let Some(ref m) = model {
                builder = builder.with_world_model(m);
            }
            let brief = match builder.build(Utc::now()) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("brief failed: {e}");
                    std::process::exit(1);
                }
            };
            if model.is_none() {
                println!("(no world model trained yet — insights need `tracemind world train`)");
                return;
            }
            if brief.insights.is_empty() {
                println!("(no insights surfacing right now)");
                println!("  insights only fire when an open row's outlook diverges from your");
                println!("  completed-rate baseline by ≥20pp; also filters silenced commitments.");
                return;
            }
            println!("insights ({})", brief.insights.len());
            for i in &brief.insights {
                let glyph = match i.tone.as_str() {
                    "warning" => "⚠",
                    "tailwind" => "✓",
                    _ => "~",
                };
                println!("  {glyph} {}  {}", i.commitment_id, i.render);
            }
            println!("\n  → `tracemind insights silence <commitment_id>` to suppress");
        }
        InsightsAction::Silenced => {
            let now = Utc::now();
            let rows = match store.list_active_insight_silences(now) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("failed to read insight silences: {e}");
                    std::process::exit(1);
                }
            };
            if rows.is_empty() {
                println!("(no active insight silences)");
                return;
            }
            println!("active insight silences ({})", rows.len());
            for s in &rows {
                let until_local = s.silenced_until.with_timezone(&chrono::Local);
                let reason = s.reason.as_deref().unwrap_or("—");
                println!(
                    "  {}  until {}  reason={}",
                    s.commitment_id,
                    until_local.format("%b %-d %Y"),
                    reason,
                );
            }
        }
        InsightsAction::Silence {
            commitment_id,
            days,
            reason,
        } => {
            let cid = match uuid::Uuid::parse_str(&commitment_id) {
                Ok(u) => u,
                Err(_) => {
                    eprintln!("invalid commitment_id: {commitment_id}");
                    std::process::exit(2);
                }
            };
            let now = Utc::now();
            let until = now + Duration::days(days);
            match store.upsert_insight_silence(cid, now, until, reason.as_deref()) {
                Ok(()) => {
                    let until_local = until.with_timezone(&chrono::Local);
                    println!(
                        "silenced insights for {} until {} ({} days)",
                        cid,
                        until_local.format("%b %-d %Y"),
                        days,
                    );
                }
                Err(e) => {
                    eprintln!("failed to silence: {e}");
                    std::process::exit(1);
                }
            }
        }
        InsightsAction::Unsilence { commitment_id } => {
            let cid = match uuid::Uuid::parse_str(&commitment_id) {
                Ok(u) => u,
                Err(_) => {
                    eprintln!("invalid commitment_id: {commitment_id}");
                    std::process::exit(2);
                }
            };
            match store.remove_insight_silence(cid) {
                Ok(true) => println!("unsilenced insights for {cid}"),
                Ok(false) => println!("no insight silence found for {cid}"),
                Err(e) => {
                    eprintln!("failed to unsilence: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn print_brief_text(brief: &tm_reflect::DailyBrief) {
    let local_now = brief.generated_at.with_timezone(&chrono::Local);
    println!(
        "TRACEMIND BRIEF                  {}",
        local_now.format("%a %b %d, %-I:%M %p")
    );
    println!(
        "  overdue: {}    open: {}    resolved: {}    candidates: {}    patterns: {}    insights: {}    proposals: {}    contradictions: {}",
        brief.counts.overdue,
        brief.counts.open,
        brief.counts.resolved,
        brief.counts.candidates,
        brief.counts.patterns,
        brief.counts.insights,
        brief.counts.proposals,
        brief.counts.contradictions,
    );
    println!();

    // Contradictions panel — printed first because it's the highest-
    // signal attention item: the JTMS engine has flagged two triples
    // as logically inconsistent and the user needs to settle them.
    // Sprint C-2: only populated when `BriefBuilder::with_graph` is set.
    if !brief.contradictions.is_empty() {
        println!("▸ contradictions ({})", brief.contradictions.len());
        for c in &brief.contradictions {
            let detected_local = c
                .detected_at
                .with_timezone(&chrono::Local)
                .format("%b %-d");
            println!(
                "    ⚡ {} ↔ {}    cosine {:+.2}    detected {}",
                short_id(c.triple_a),
                short_id(c.triple_b),
                c.cosine_similarity,
                detected_local,
            );
        }
        println!("    (review with `tracemind brief --json | jq .contradictions`)");
        println!();
    }

    // Insights panel — printed first so the user sees the most
    // attention-worthy rows before drowning in the open list.
    // If the calibration gate suppressed the panel (TM-INTENT-010),
    // surface that fact directly so the user knows the model has
    // an opinion that's being held back.
    if let Some(reason) = &brief.model_quiet {
        let line = match reason {
            tm_reflect::ModelQuietReason::InsufficientEvaluations { n_evaluated, required } => {
                format!(
                    "▸ insights — model warming up ({}/{} completions evaluated)",
                    n_evaluated, required
                )
            }
            tm_reflect::ModelQuietReason::LowAccuracy { accuracy, floor, n_evaluated } => {
                format!(
                    "▸ insights — paused (accuracy {:.0}% < {:.0}% floor over {} completions)",
                    accuracy * 100.0,
                    floor * 100.0,
                    n_evaluated
                )
            }
            tm_reflect::ModelQuietReason::LowWarningPrecision {
                warning_precision,
                floor,
                n_evaluated,
            } => format!(
                "▸ insights — paused (warning precision {:.0}% < {:.0}% floor over {} completions)",
                warning_precision * 100.0,
                floor * 100.0,
                n_evaluated
            ),
        };
        println!("{line}");
        println!("    (run `tracemind world calibration` to see the score card)");
        println!();
    } else if !brief.insights.is_empty() {
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

    // Outcome proposals — TM-INTENT-009 / `INTENT_SYSTEM.md` §4.2.
    // Persistent suggestions from the implicit text matcher: a fresh
    // capture's text overlapped an open commitment, with a polarity
    // hint. Surfaced here so "what happened with X?" lands days
    // later when the user opens the brief, not only at the moment
    // the capture happened.
    if !brief.proposals.is_empty() {
        println!("▸ what happened with… ({})", brief.proposals.len());
        for p in &brief.proposals {
            let pol = match p.proposed_polarity {
                tm_intent::Polarity::Better => "better",
                tm_intent::Polarity::AsExpected => "as-expected",
                tm_intent::Polarity::Worse => "worse",
                tm_intent::Polarity::Mixed => "mixed",
                tm_intent::Polarity::NoOutcome => "no-outcome",
            };
            println!(
                "    {}  [{:>11}]  score={:.2}  ↦ {}",
                short_id(p.id),
                pol,
                p.similarity,
                truncate_str(&p.commitment_statement, 56),
            );
            println!("        ↳ \"{}\"", truncate_str(&p.description, 70));
        }
        println!("    → `tracemind outcomes accept|dismiss <id>` to triage");
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
        && brief.counts.proposals == 0
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
        explain_top_k, from_pairs, load, save, train, Architecture, OutcomeModel, TrainerConfig,
        DEFAULT_MLP_HIDDEN,
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
            arch,
            validation_split,
            patience,
            seed,
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

            let architecture = match parse_arch(&arch) {
                Ok(a) => a,
                Err(msg) => {
                    eprintln!("invalid --arch: {msg}");
                    std::process::exit(2);
                }
            };
            if !(0.0..1.0).contains(&validation_split) {
                eprintln!("--validation-split must be in [0.0, 1.0), got {validation_split}");
                std::process::exit(2);
            }

            let cfg = TrainerConfig {
                epochs,
                lr,
                l2,
                tag_vocab_size: vocab,
                min_examples,
                architecture,
                validation_split,
                early_stop_patience: patience,
                seed,
            };
            let (model, mut report) = train(&examples, &cfg);
            report.skipped_no_outcome = skipped;

            fn parse_arch(s: &str) -> Result<Architecture, String> {
                let trimmed = s.trim().to_lowercase();
                if trimmed == "linear" {
                    return Ok(Architecture::Linear);
                }
                if trimmed == "mlp" {
                    return Ok(Architecture::Mlp { hidden_dim: DEFAULT_MLP_HIDDEN });
                }
                if let Some(rest) = trimmed.strip_prefix("mlp:") {
                    let h: usize = rest.parse().map_err(|_| {
                        format!("could not parse hidden width from `mlp:{rest}`")
                    })?;
                    if h == 0 {
                        return Err("mlp hidden width must be > 0".into());
                    }
                    return Ok(Architecture::Mlp { hidden_dim: h });
                }
                Err(format!(
                    "{s} (expected `linear`, `mlp`, or `mlp:N` for hidden width N)"
                ))
            }

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
            println!("  architecture   : {}", report.architecture);
            println!(
                "  examples       : {} (from {} completed; {} skipped no-outcome)",
                report.n_examples, total_completed, report.skipped_no_outcome
            );
            if report.n_validation > 0 {
                println!("  held out       : {}", report.n_validation);
            }
            println!("  classes seen   : {}/4", report.n_classes_seen);
            println!("  feature dim    : {} ({} fixed + {} tags)",
                report.feature_dim,
                tm_world_model::FIXED_FEATURES,
                report.vocab_size,
            );
            println!("  epochs run     : {}{}",
                report.epochs_run,
                if report.early_stopped { " (early stopped)" } else { "" },
            );
            if report.epochs_run > 0 {
                println!("  final loss     : {:.4}", report.final_loss);
                println!("  final accuracy : {:.1}%", report.final_accuracy * 100.0);
                if let (Some(vl), Some(va)) = (report.val_loss, report.val_accuracy) {
                    println!("  val loss       : {:.4}", vl);
                    println!("  val accuracy   : {:.1}%", va * 100.0);
                }
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

        WorldAction::Calibration {
            since_days,
            limit,
            all,
            json,
        } => {
            let model = match load(world_path) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    eprintln!(
                        "no world model on disk at {} — run `tracemind world train` first.",
                        world_path.display()
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("failed to load world model: {e}");
                    std::process::exit(1);
                }
            };

            let store = match IntentStore::open(intents_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("failed to open intent store at {intents_path}: {e}");
                    std::process::exit(1);
                }
            };
            let since = chrono::Utc::now() - chrono::Duration::days(since_days);
            let rows = match store.list_completed_with_outcome_meta(since, limit) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("failed to read completed commitments: {e}");
                    std::process::exit(1);
                }
            };

            let (pairs, in_sample_skipped) = if all {
                // --all: keep everything, ignore trained_at cutoff.
                let kept = rows.into_iter().map(|(c, p, _)| (c, p)).collect();
                (kept, 0usize)
            } else {
                tm_world_model::split_out_of_sample(&model, rows)
            };

            let mut report = tm_world_model::evaluate(&model, &pairs);
            report.n_in_sample_skipped = in_sample_skipped;

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

            println!("world model calibration");
            println!("  trained_at     : {}", report.trained_at.as_deref().unwrap_or("—"));
            println!("  evaluated_at   : {}", report.evaluated_at);
            println!(
                "  n_evaluated    : {} ({} in-sample skipped, {} no-outcome skipped){}",
                report.n_evaluated,
                report.n_in_sample_skipped,
                report.n_no_outcome_skipped,
                if all { " [--all: in-sample included]" } else { "" }
            );
            if report.n_evaluated == 0 {
                println!(
                    "  (no out-of-sample completions to score — resolve more commitments \
                     after the last `world train` and try again.)"
                );
                return;
            }
            println!("  accuracy           : {:.1}%", report.accuracy * 100.0);
            println!("  positive recall    : {:.1}%", report.positive_recall * 100.0);
            println!("  warning precision  : {:.1}%", report.warning_precision * 100.0);
            println!("  brier score        : {:.4}  (lower is better; uniform=0.75)", report.brier_score);
            println!("  per-class:");
            for pc in &report.per_class {
                println!(
                    "    {:<12} actual={:>3}  predicted={:>3}  correct={:>3}",
                    pc.label, pc.n_actual, pc.n_predicted, pc.n_correct
                );
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
                    architecture: Option<String>,
                    hidden_dim: Option<usize>,
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
                        architecture: Some(m.architecture.label()),
                        hidden_dim: match m.architecture {
                            Architecture::Linear => None,
                            Architecture::Mlp { hidden_dim } => Some(hidden_dim),
                        },
                        n_train_examples: Some(m.n_train_examples),
                        trained_at: m.trained_at.clone(),
                        feature_dim: Some(m.feature_dim()),
                        vocab_size: Some(m.tag_vocab.len()),
                    },
                    None => Status {
                        path: world_path.display().to_string(),
                        present: false,
                        schema_version: None,
                        architecture: None,
                        hidden_dim: None,
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
                    println!("  architecture   : {}", m.architecture.label());
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

// ---------------------------------------------------------------------------
// Intent arc commands: need / sentiment / action / arc
// ---------------------------------------------------------------------------

fn cmd_need(
    intents_path: &str,
    statement: &str,
    urgency: Option<f32>,
    recurring: bool,
    tags: &str,
    link: Option<&str>,
) {
    use tm_intent::{IntentStore, Need, NeedSource};

    let mut need = Need::new(statement, NeedSource::Explicit);
    if let Some(u) = urgency {
        if !(0.0..=1.0).contains(&u) {
            eprintln!("urgency must be in [0,1], got {u}");
            std::process::exit(1);
        }
        need.urgency = u;
    }
    need.recurring = recurring;
    need.tags = tags
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let store = IntentStore::open(intents_path).expect("open intent store");
    store.insert_need(&need).expect("insert need");

    if let Some(cid_s) = link {
        let cid = Uuid::parse_str(cid_s).unwrap_or_else(|e| {
            eprintln!("invalid --link UUID '{cid_s}': {e}");
            std::process::exit(1);
        });
        store
            .link_need_to_commitment(need.id, cid)
            .unwrap_or_else(|e| {
                eprintln!("link failed: {e}");
                std::process::exit(1);
            });
        println!("Linked to commitment {cid}");
    }

    println!("Need recorded: {}", need.id);
    println!("  statement: {}", need.statement);
    println!("  urgency:   {:.1}", need.urgency);
    println!("  recurring: {}", need.recurring);
}

fn cmd_sentiment(
    intents_path: &str,
    target_id_s: &str,
    target_type_s: &str,
    valence: f64,
    evidence: Option<&str>,
) {
    use tm_intent::{IntentStore, Sentiment, SentimentSource, SentimentTarget};

    let target_id = Uuid::parse_str(target_id_s).unwrap_or_else(|e| {
        eprintln!("invalid target_id '{target_id_s}': {e}");
        std::process::exit(1);
    });
    let target_type = match target_type_s {
        "commitment" => SentimentTarget::Commitment,
        "need" => SentimentTarget::Need,
        "entity" => SentimentTarget::Entity,
        "topic" => SentimentTarget::Topic,
        other => {
            eprintln!("invalid --target-type: {other} (must be commitment|need|entity|topic)");
            std::process::exit(1);
        }
    };
    if !(-1.0..=1.0).contains(&valence) {
        eprintln!("valence must be in [-1,1], got {valence}");
        std::process::exit(1);
    }

    let mut s = Sentiment::new(target_id, target_type, valence as f32, SentimentSource::UserProvided);
    if let Some(ev) = evidence {
        s.evidence_text = Some(ev.to_string());
    }

    let store = IntentStore::open(intents_path).expect("open intent store");
    store.insert_sentiment(&s).expect("insert sentiment");

    let sign = if s.valence >= 0.0 { "+" } else { "" };
    println!("Sentiment recorded: {}", s.id);
    println!("  target:    {} ({})", target_id_s, target_type_s);
    println!("  valence:   {sign}{:.2}", s.valence);
    println!("  intensity: {:.2}", s.intensity);
}

fn cmd_action(
    intents_path: &str,
    description: &str,
    commitment_id: Option<&str>,
    modality_s: &str,
) {
    use tm_intent::{Action, ActionModality, ActionSource, IntentStore};

    let modality = match modality_s {
        "digital" => ActionModality::Digital,
        "physical" => ActionModality::Physical,
        "communication" => ActionModality::Communication,
        "creation" => ActionModality::Creation,
        other => {
            eprintln!("invalid --modality: {other} (must be digital|physical|communication|creation)");
            std::process::exit(1);
        }
    };

    let mut action = Action::new(description, ActionSource::UserReported);
    action.modality = modality;
    if let Some(cid_s) = commitment_id {
        action.commitment_id = Some(Uuid::parse_str(cid_s).unwrap_or_else(|e| {
            eprintln!("invalid --commitment-id '{cid_s}': {e}");
            std::process::exit(1);
        }));
    }

    let store = IntentStore::open(intents_path).expect("open intent store");
    store.insert_action(&action).expect("insert action");

    println!("Action recorded: {}", action.id);
    println!("  description: {}", action.description);
    println!("  modality:    {modality_s}");
    if let Some(cid) = action.commitment_id {
        println!("  commitment:  {cid}");
    }
}

fn cmd_arc(intents_path: &str, commitment_id_s: &str, json: bool) {
    use tm_intent::IntentStore;

    let cid = Uuid::parse_str(commitment_id_s).unwrap_or_else(|e| {
        eprintln!("invalid commitment_id '{commitment_id_s}': {e}");
        std::process::exit(1);
    });

    let store = IntentStore::open(intents_path).expect("open intent store");
    let commitment = store
        .get_commitment(cid)
        .expect("get_commitment")
        .unwrap_or_else(|| {
            eprintln!("commitment {cid} not found");
            std::process::exit(1);
        });

    // Gather linked needs
    let all_needs = store.list_needs(500).expect("list_needs");
    let linked_needs: Vec<_> = all_needs
        .into_iter()
        .filter(|n| n.linked_commitments.contains(&cid))
        .collect();

    let sentiments = store
        .list_sentiments_for(cid, 10)
        .expect("list_sentiments_for");
    let actions = store
        .list_actions_for_commitment(cid, 20)
        .expect("list_actions_for_commitment");
    let outcome = commitment
        .outcome_id
        .and_then(|oid| store.get_outcome(oid).ok().flatten());

    if json {
        let needs_json: Vec<serde_json::Value> = linked_needs
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id.to_string(),
                    "statement": n.statement,
                    "urgency": n.urgency,
                })
            })
            .collect();
        let sents_json: Vec<serde_json::Value> = sentiments
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id.to_string(),
                    "valence": s.valence,
                    "intensity": s.intensity,
                    "captured_at": s.captured_at.to_rfc3339(),
                })
            })
            .collect();
        let acts_json: Vec<serde_json::Value> = actions
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id.to_string(),
                    "description": a.description,
                    "taken_at": a.taken_at.to_rfc3339(),
                })
            })
            .collect();
        let out_json = outcome.as_ref().map(|o| {
            serde_json::json!({
                "id": o.id.to_string(),
                "polarity": format!("{:?}", o.polarity).to_lowercase(),
                "description": o.description,
            })
        });
        let arc = serde_json::json!({
            "commitment": {
                "id": commitment.id.to_string(),
                "kind": format!("{:?}", commitment.kind).to_lowercase(),
                "statement": commitment.statement,
                "state": format!("{:?}", commitment.state).to_lowercase(),
            },
            "needs": needs_json,
            "sentiments": sents_json,
            "actions": acts_json,
            "outcome": out_json,
        });
        println!("{}", serde_json::to_string_pretty(&arc).unwrap());
        return;
    }

    // Formatted text output
    println!("═══ Intent Arc ═══");
    println!(
        "Commitment: {} [{}] ({})",
        commitment.statement,
        format!("{:?}", commitment.kind).to_lowercase(),
        format!("{:?}", commitment.state).to_lowercase(),
    );
    println!("  id: {cid}");

    if !linked_needs.is_empty() {
        println!("\n── Needs ──");
        for n in &linked_needs {
            println!(
                "  • {} (urgency {:.1}{})",
                n.statement,
                n.urgency,
                if n.recurring { ", recurring" } else { "" },
            );
        }
    }

    if !sentiments.is_empty() {
        println!("\n── Sentiments ──");
        for s in &sentiments {
            let sign = if s.valence >= 0.0 { "+" } else { "" };
            println!(
                "  {} {sign}{:.2} (intensity {:.2}) — {}",
                s.captured_at.format("%Y-%m-%d"),
                s.valence,
                s.intensity,
                s.evidence_text.as_deref().unwrap_or("—"),
            );
        }
    }

    if !actions.is_empty() {
        println!("\n── Actions ──");
        for a in &actions {
            println!(
                "  {} {} [{}]",
                a.taken_at.format("%Y-%m-%d"),
                a.description,
                format!("{:?}", a.modality).to_lowercase(),
            );
        }
    }

    if let Some(o) = &outcome {
        println!("\n── Outcome ──");
        println!(
            "  {} — {} ({})",
            o.observed_at.format("%Y-%m-%d"),
            o.description,
            format!("{:?}", o.polarity).to_lowercase(),
        );
    }
}

// ---------------------------------------------------------------------------
// Demo fixture (P0 / D-2)
// ---------------------------------------------------------------------------

/// `tracemind demo …` — recordable-demo helpers. Currently exposes
/// `restore`, which wipes `$TM_DATA_DIR` and rebuilds a deterministic
/// fixture so the brief looks the same on every recording take.
///
/// Determinism strategy: every commitment / outcome / entity gets a
/// UUIDv5 derived from a fixed namespace + a stable label. Times are
/// expressed as offsets from `Utc::now()` so the brief's "due today"
/// / "stale" buckets stay correct without us having to re-shoot when
/// the calendar rolls over.
fn cmd_demo(dir: &PathBuf, action: DemoAction) {
    match action {
        DemoAction::Restore { force } => cmd_demo_restore(dir, force),
        DemoAction::Preroll { seconds, verbose } => cmd_demo_preroll(dir, seconds, verbose),
    }
}

/// `tracemind contradictions list | resolve` — terminal mirror of the
/// Tauri brief drawer. Same `BeliefStore::resolve_by_triples` path as
/// the desktop app, so a sidecar saved here replays correctly when
/// the user later opens the GUI.
fn cmd_contradictions(db_path: &str, action: ContradictionsAction) {
    use tm_graph::{GraphStore, ResolveChoice};

    let graph = match GraphStore::open(db_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to open graph at {db_path}: {e}");
            std::process::exit(1);
        }
    };

    match action {
        ContradictionsAction::List { json } => {
            let rows: Vec<_> = graph
                .contradictions()
                .into_iter()
                .filter(|r| r.resolution.is_none())
                .collect();
            if json {
                let out = serde_json::to_string_pretty(&rows)
                    .unwrap_or_else(|_| "[]".to_string());
                println!("{out}");
            } else if rows.is_empty() {
                println!("no outstanding contradictions");
            } else {
                println!("{} outstanding contradiction(s):\n", rows.len());
                for r in &rows {
                    let a = graph
                        .triple_detail(r.triple_a)
                        .ok()
                        .flatten()
                        .map(|d| format!("{} {} {}", d.subject_name, d.predicate, d.object_name))
                        .unwrap_or_else(|| r.triple_a.to_string());
                    let b = graph
                        .triple_detail(r.triple_b)
                        .ok()
                        .flatten()
                        .map(|d| format!("{} {} {}", d.subject_name, d.predicate, d.object_name))
                        .unwrap_or_else(|| r.triple_b.to_string());
                    println!(
                        "  {}  cosine {:+.2}",
                        r.detected_at.format("%Y-%m-%d %H:%M"),
                        r.cosine_similarity,
                    );
                    println!("    A  {}  ({})", a, &r.triple_a.to_string()[..8]);
                    println!("    B  {}  ({})", b, &r.triple_b.to_string()[..8]);
                    println!();
                }
                println!(
                    "resolve with: tracemind contradictions resolve <triple_a> <triple_b> <keep-a|keep-b|keep-both>",
                );
            }
        }
        ContradictionsAction::Resolve {
            triple_a,
            triple_b,
            choice,
        } => {
            let ta = match uuid::Uuid::parse_str(&triple_a) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("bad triple_a uuid: {e}");
                    std::process::exit(2);
                }
            };
            let tb = match uuid::Uuid::parse_str(&triple_b) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("bad triple_b uuid: {e}");
                    std::process::exit(2);
                }
            };
            let ch = match choice.to_lowercase().replace('_', "-").as_str() {
                "keep-a" => ResolveChoice::KeepA,
                "keep-b" => ResolveChoice::KeepB,
                "keep-both" => ResolveChoice::KeepBoth,
                other => {
                    eprintln!("unknown choice {other:?}; expected keep-a | keep-b | keep-both");
                    std::process::exit(2);
                }
            };
            match graph.resolve_contradiction_by_triples(ta, tb, ch) {
                Some((retracted, kept)) => {
                    println!("resolved: {} retracted, {} kept", retracted.len(), kept.len());
                    for u in retracted {
                        println!("  retracted {}", u);
                    }
                    for u in kept {
                        println!("  kept      {}", u);
                    }
                }
                None => {
                    eprintln!("no matching contradiction for that triple pair");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Run the ambient capture daemon silently for `seconds`, then stop it.
///
/// We resolve the binary in this order:
/// 1. `$TM_CAPTURE_BIN` if set (lets us point at a freshly-built
///    `target/release/tracemind-capture` from a workspace that isn't
///    on `PATH`).
/// 2. A sibling `tracemind-capture` next to the current executable.
/// 3. `tracemind-capture` on `PATH`.
///
/// stdin is closed; stdout + stderr are routed to /dev/null so the
/// recording stays free of terminal flicker. We propagate
/// `TM_DATA_DIR` to the child so the daemon writes to the same data
/// dir that the rest of the CLI uses.
fn cmd_demo_preroll(dir: &PathBuf, seconds: u64, verbose: bool) {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration as StdDuration;

    let bin = resolve_capture_bin();
    if !bin.exists() && std::env::var_os("TM_CAPTURE_BIN").is_none() {
        // Fall back to PATH lookup — let the OS resolve it.
        // (`Command::new("tracemind-capture")` will work if it's on PATH.)
    }

    if verbose {
        println!(
            "demo preroll: spawning capture daemon for {seconds}s ({})",
            bin.display()
        );
    }

    // Build the command. If `bin` doesn't exist on disk we still try
    // it by name so PATH resolution gets a shot.
    let mut cmd = if bin.exists() {
        Command::new(&bin)
    } else {
        Command::new("tracemind-capture")
    };
    cmd.env("TM_DATA_DIR", dir);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "demo preroll: failed to spawn capture daemon ({e}). \
                 Set TM_CAPTURE_BIN or add tracemind-capture to PATH."
            );
            std::process::exit(1);
        }
    };

    // Sleep for the pre-roll window. We could poll child.try_wait()
    // for early death, but the demo recording flow doesn't need that
    // sophistication — if the daemon crashes, the brief just won't
    // have fresh captures and the recording will fail naturally.
    thread::sleep(StdDuration::from_secs(seconds));

    // Send SIGTERM (kill = SIGKILL on Unix from std, but the daemon
    // is fine with abrupt termination — its writes are tx-committed).
    let _ = child.kill();
    let _ = child.wait();

    if verbose {
        println!("demo preroll: done");
    }
}

fn resolve_capture_bin() -> PathBuf {
    if let Some(env_path) = std::env::var_os("TM_CAPTURE_BIN") {
        return PathBuf::from(env_path);
    }
    if let Ok(self_exe) = std::env::current_exe() {
        if let Some(parent) = self_exe.parent() {
            let sibling = parent.join("tracemind-capture");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("tracemind-capture")
}

fn cmd_demo_restore(dir: &PathBuf, force: bool) {
    use chrono::{Duration, Utc};
    use tm_graph::GraphStore;
    use tm_intent::{
        Commitment, CommitmentKind, IntentStore, Outcome, OutcomeSource, Polarity, Source,
        Stakes, State, state::transition,
    };
    use tm_types::{Entity, EntityType, Predicate, Triple};

    // Refuse to clobber a non-empty data dir without --force. Errs on
    // the side of safety for users who run `tracemind demo restore`
    // by accident on a real install.
    let occupied = dir
        .read_dir()
        .map(|mut it| it.next().is_some())
        .unwrap_or(false);
    if occupied && !force {
        eprintln!(
            "refusing to overwrite non-empty data dir {} — pass --force to proceed",
            dir.display()
        );
        std::process::exit(2);
    }

    if occupied {
        // Wipe everything under dir but keep the dir itself so the
        // path stays valid for the next stores we open.
        if let Ok(entries) = dir.read_dir() {
            for e in entries.flatten() {
                let p = e.path();
                let _ = if p.is_dir() {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
            }
        }
    } else {
        ensure_data_dir(dir);
    }

    let now = Utc::now();
    let intents_path = dir.join("intents.db").to_str().unwrap().to_string();
    let graph_path = dir.join("memory.db").to_str().unwrap().to_string();
    let intents = IntentStore::open(&intents_path).expect("open intent store");
    let graph = GraphStore::open(&graph_path).expect("open graph store");

    // ── Entities (concept characters used by the script) ───────────
    let mk_entity = |name: &str, kind: EntityType| -> Entity {
        let id = Uuid::new_v5(&DEMO_NAMESPACE, format!("entity:{name}").as_bytes());
        let mut e = Entity::new(name, kind, 0.92);
        e.id = id;
        e
    };
    let alice = mk_entity("Alice", EntityType::Person);
    let bob = mk_entity("Bob", EntityType::Person);
    let carla = mk_entity("Carla", EntityType::Person);
    let priya = mk_entity("Priya", EntityType::Person);
    let mercury = mk_entity("Mercury Inc", EntityType::Organization);
    let q1memo = mk_entity("Q1 board memo", EntityType::File);
    let demo_proj = mk_entity("TraceMind demo", EntityType::Project);
    let postgres = mk_entity("Postgres", EntityType::Technology);
    let sqlite = mk_entity("SQLite", EntityType::Technology);
    let onboarding = mk_entity("Mercury onboarding", EntityType::Event);
    let pitch_deck = mk_entity("Investor pitch deck", EntityType::File);
    let calibration_plan = mk_entity("Calibration plan", EntityType::Concept);
    let research_doc = mk_entity("LoCoMo eval doc", EntityType::File);
    let sprint_plan = mk_entity("Sprint C-2 plan", EntityType::Concept);
    let oncall_runbook = mk_entity("Oncall runbook", EntityType::File);

    let entities = [
        &alice, &bob, &carla, &priya, &mercury, &q1memo, &demo_proj,
        &postgres, &sqlite, &onboarding, &pitch_deck, &calibration_plan,
        &research_doc, &sprint_plan, &oncall_runbook,
    ];
    for e in entities {
        graph.upsert_entity(e).expect("upsert entity");
    }

    // ── Triples ────────────────────────────────────────────────────
    let mk_triple = |label: &str, s: Uuid, p: Predicate, o: Uuid, conf: f64| -> Triple {
        let id = Uuid::new_v5(&DEMO_NAMESPACE, format!("triple:{label}").as_bytes());
        let mut t = Triple::new(s, p, o, conf);
        t.id = id;
        t
    };

    let triples = vec![
        mk_triple("alice-works-mercury", alice.id, Predicate::WorksAt, mercury.id, 0.95),
        mk_triple("bob-works-mercury", bob.id, Predicate::WorksAt, mercury.id, 0.92),
        mk_triple("carla-collab-alice", carla.id, Predicate::CollaboratesWith, alice.id, 0.88),
        mk_triple("priya-collab-bob", priya.id, Predicate::CollaboratesWith, bob.id, 0.84),
        mk_triple("alice-owns-q1memo", alice.id, Predicate::Owns, q1memo.id, 0.9),
        mk_triple("q1memo-references-mercury", q1memo.id, Predicate::References, mercury.id, 0.93),
        mk_triple("demo-depends-postgres", demo_proj.id, Predicate::DependsOn, postgres.id, 0.9),
        mk_triple("demo-depends-sqlite", demo_proj.id, Predicate::DependsOn, sqlite.id, 0.95),
        mk_triple("onboarding-partof-mercury", onboarding.id, Predicate::PartOf, mercury.id, 0.86),
        mk_triple("pitch-references-demo", pitch_deck.id, Predicate::References, demo_proj.id, 0.91),
        mk_triple("calibration-related", calibration_plan.id, Predicate::RelatedTo, demo_proj.id, 0.8),
        mk_triple("research-references-locomo", research_doc.id, Predicate::References, demo_proj.id, 0.87),
        mk_triple("sprint-related-demo", sprint_plan.id, Predicate::RelatedTo, demo_proj.id, 0.83),
        mk_triple("oncall-references-postgres", oncall_runbook.id, Predicate::References, postgres.id, 0.81),
        // The contradicting pair — same subject + object, opposing
        // predicates. Detected explicitly below at cosine = -0.94.
        mk_triple(
            "alice-loves-bob",
            alice.id,
            Predicate::Custom("loves".into()),
            bob.id,
            0.88,
        ),
        mk_triple(
            "alice-hates-bob",
            alice.id,
            Predicate::Custom("hates".into()),
            bob.id,
            0.85,
        ),
    ];
    for t in &triples {
        graph.upsert_triple(t).expect("upsert triple");
    }

    // Detect the contradiction so the brief surfaces it. The cosine
    // is hard-coded — the live pipeline computes it from embeddings,
    // but for the fixture we just want the JTMS to flip both beliefs
    // to Contradicted.
    let loves = triples[triples.len() - 2].id;
    let hates = triples[triples.len() - 1].id;
    let _ = graph
        .record_contradiction(loves, hates, -0.94)
        .expect("contradiction recorded");

    // ── Commitments + outcomes ─────────────────────────────────────
    let mk_commit = |label: &str,
                     statement: &str,
                     kind: CommitmentKind,
                     stakes: Stakes,
                     horizon: Option<chrono::DateTime<chrono::Utc>>|
     -> Commitment {
        let id = Uuid::new_v5(&DEMO_NAMESPACE, format!("commit:{label}").as_bytes());
        let mut c = Commitment::new(kind, statement, Source::Manual);
        c.id = id;
        c.stakes = stakes;
        c.horizon = horizon;
        // Anchor `made_at` slightly before horizon so the brief's
        // resolved-window math behaves predictably.
        if let Some(h) = horizon {
            c.made_at = h - Duration::days(7);
        }
        c
    };

    // Open + due
    let due_today = mk_commit(
        "memo-with-carla",
        "file Q1 board memo with Carla",
        CommitmentKind::Intent,
        Stakes::High,
        Some(now + Duration::hours(4)),
    );
    let overdue_3d = mk_commit(
        "mercury-followup",
        "follow up on Mercury contract",
        CommitmentKind::Intent,
        Stakes::High,
        Some(now - Duration::days(3)),
    );
    let stale_a = mk_commit(
        "rewrite-onboarding",
        "rewrite Mercury onboarding doc",
        CommitmentKind::Intent,
        Stakes::Medium,
        Some(now - Duration::days(21)),
    );
    let stale_b = mk_commit(
        "ship-pitch-v2",
        "ship investor pitch deck v2",
        CommitmentKind::Intent,
        Stakes::Medium,
        Some(now - Duration::days(28)),
    );

    intents.insert_commitment(&due_today).unwrap();
    intents.insert_commitment(&overdue_3d).unwrap();
    intents.insert_commitment(&stale_a).unwrap();
    intents.insert_commitment(&stale_b).unwrap();

    // Resolved (last 7 days) — provide priors for patterns + insights.
    let resolved_specs: Vec<(&str, &str, Polarity, i64)> = vec![
        ("ship-c1", "ship Sprint C-1 bitemporal substrate", Polarity::Better, 1),
        ("ship-c2", "ship Sprint C-2 contradictions", Polarity::AsExpected, 0),
        ("locomo-mini", "rerun LoCoMo mini eval after BGE swap", Polarity::Worse, 2),
        ("draft-investor", "draft investor narrative outline", Polarity::Better, 4),
        ("calibration-review", "review calibration plan with Priya", Polarity::Mixed, 5),
    ];
    for (label, statement, polarity, days_ago) in &resolved_specs {
        let mut c = mk_commit(
            label,
            statement,
            CommitmentKind::Intent,
            Stakes::Medium,
            Some(now - Duration::days(*days_ago)),
        );
        // The resolved bucket filters by `made_at >= now - resolved_window`
        // (default 7d). Without this override, mk_commit would push
        // `made_at` to `horizon - 7d`, which sits *just outside* the
        // window for `days_ago = 1`. Anchor `made_at` to `days_ago + 1`
        // so all five resolved rows surface in the brief.
        c.made_at = now - Duration::days(*days_ago + 1);
        intents.insert_commitment(&c).unwrap();

        let outcome = Outcome::new(
            c.id,
            *polarity,
            format!("resolved: {statement}"),
            OutcomeSource::Cli,
        );
        intents.insert_outcome(&outcome).unwrap();
        // Drive the state machine to keep the brief's resolved bucket
        // populated. transition() mutates the in-memory copy; we then
        // persist the updated state via update_state().
        transition(&mut c, State::Completed, Some(&outcome)).expect("transition Completed");
        intents
            .update_state(c.id, c.state, c.outcome_id)
            .expect("persist state");
    }

    println!("✓ demo fixture restored to {}", dir.display());
    println!("  entities:        {}", entities.len());
    println!("  triples:         {}", triples.len());
    println!("  contradictions:  1   (Alice loves Bob ↔ Alice hates Bob)");
    println!("  open commitments: 4  (1 due today, 1 overdue 3d, 2 stale)");
    println!("  resolved (7d):   {}", resolved_specs.len());
    println!();
    println!("  next: tracemind brief");
}

/// UUIDv5 namespace for the demo fixture. Picked once and frozen so
/// every restore produces byte-identical UUIDs, which makes recordings
/// re-shootable without re-editing the script's short-id callouts.
const DEMO_NAMESPACE: Uuid = Uuid::from_bytes([
    0x4d, 0x65, 0x6d, 0x6f, 0x52, 0x79, 0x44, 0x65, 0x6d, 0x6f, 0x46, 0x69, 0x78, 0x74, 0x75, 0x72,
]);
