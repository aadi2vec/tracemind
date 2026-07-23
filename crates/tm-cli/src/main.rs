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
use tm_types::{Entity, EntityType, Procedure, ProcedureStep};
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
    Query {
        text: String,
        /// Sprint C-0.6 — search every context instead of just the
        /// active one. Off by default: decoupled-by-default is the
        /// wedge.
        #[arg(long)]
        cross_context: bool,
        /// LM-11c — apply a saved Memory View (by name) before
        /// returning results. Overrides the active view, if any.
        /// Use `--view ''` to force-disable the active view for this
        /// one query.
        #[arg(long)]
        view: Option<String>,
        /// LM-11c — ad-hoc include this entity UUID in the splice for
        /// this query only (not persisted to any view). Repeatable.
        #[arg(long = "include-entity", value_name = "UUID")]
        include_entity: Vec<String>,
        /// LM-11c — ad-hoc exclude this entity UUID. Repeatable.
        #[arg(long = "exclude-entity", value_name = "UUID")]
        exclude_entity: Vec<String>,
    },
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
        /// Sprint C-0.6 — bridge contexts when gathering grounding.
        #[arg(long)]
        cross_context: bool,
        /// LM-11c — apply a saved Memory View by name before grounding.
        #[arg(long)]
        view: Option<String>,
        /// LM-11c — ad-hoc include entity UUID for this question only.
        #[arg(long = "include-entity", value_name = "UUID")]
        include_entity: Vec<String>,
        /// LM-11c — ad-hoc exclude entity UUID for this question only.
        #[arg(long = "exclude-entity", value_name = "UUID")]
        exclude_entity: Vec<String>,
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
    /// Sprint D / F-1 — mark a retrieval result as *helpful* for its
    /// query. Symmetric counterpart to `not-related`: writes a row to
    /// `positive_signals`; the next bandit reward on the same query_id
    /// is composed as
    /// `final = (relevance + Σ positives - Σ negatives).clamp(0, 1)`.
    Helpful {
        /// UUID of the query whose result you're rewarding.
        query_id: String,
        /// Opaque id of the useful result (triple / entity / signal id).
        result_id: String,
        /// Positive weight in (0, ∞). Default 0.3 — a soft nudge so a
        /// single thumbs-up doesn't saturate the reward.
        #[arg(long, default_value = "0.3")]
        weight: f32,
        /// Kind tag — defaults to `helpful`. Free-form (`bookmark`,
        /// `cited`, `kept`) so future surfaces can split positives.
        #[arg(long, default_value = "helpful")]
        kind: String,
        /// Optional active context UUID (snapshot of `active_context`
        /// at the time of feedback).
        #[arg(long)]
        context_id: Option<String>,
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
    /// Re-run the current heuristic classifier over every stored
    /// entity name and update `entity_type` when the verdict differs.
    /// Fixes legacy rows the old, looser heuristic and GLiNER mislabeled
    /// (e.g. `UcbBandit`/`Anticipate` stored as Organization).
    Reclassify {
        /// Print the changes that *would* be made without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Recompute Louvain communities and c-TF-IDF community labels.
    /// Equivalent to the Tauri `cmd_consolidate` slow path without the
    /// LLM relabeler — useful for verifying the labeler pipeline from
    /// the CLI after changing the labeling code.
    Relabel,
    /// Purge single-token entities whose name is a heuristic stopword
    /// (e.g. `entirely`, `plane`, `during`, `we'll`) — leftover noise
    /// from earlier ingest runs before the classifier was tightened.
    /// Deletes the entity, its relations, and its vector.
    PurgeStopwords {
        /// Print the entities that *would* be purged without writing.
        #[arg(long)]
        dry_run: bool,
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
    /// LM-11 — Memory Views: user-curated, saved splices of memory.
    /// A view is an include/exclude list of entities, triples, and
    /// contexts that gives the user surgical control over which
    /// memories enter a session. See PROJECT_2026 §1c and
    /// `docs/TASKS.md` LM-11.
    View {
        #[command(subcommand)]
        action: ViewAction,
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
    /// LM-16 — export the local graph as a portable Obsidian-compatible
    /// markdown bundle. Produces one file per entity with `[[wikilinks]]`
    /// to related entities plus an `index.md` overview. Used as a trust
    /// artifact ("your memory is yours, here's the bundle") and as the
    /// data path for Karpathy-style PKM workflows (see PROJECT_2026 §1c).
    Export {
        /// Filter to a single context. Accepts a context name (e.g.
        /// "TraceMind dev") or a context UUID.
        #[arg(long)]
        context: Option<String>,
        /// Output format. Only `markdown` is supported today; `json` is
        /// reserved for a future structured-export pass.
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Destination directory. Created if it does not exist. Existing
        /// files inside are overwritten without prompting.
        #[arg(long)]
        output: PathBuf,
        /// LM-11f — filter to a saved Memory View (by name or UUID).
        /// Combines with `--context` (intersection).
        #[arg(long)]
        view: Option<String>,
        /// LM-18 — filter to a single entity and its 1-hop neighbours.
        /// Accepts a UUID or an exact entity name. Useful for sharing
        /// a focused slice of memory ("here's everything I have on X").
        #[arg(long)]
        entity: Option<String>,
        /// LM-19 — run a PII scrub on every name / triple / source_id
        /// before writing. Uses `tm-governance::Governor`'s redactor.
        /// Off by default.
        #[arg(long, default_value_t = false)]
        redact: bool,
        /// Cap the number of entities exported (most-recently-updated
        /// first). `None` means "export all".
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show bandit arm statistics
    Status,
    /// Inspect or promote the GEPA-tuned retrieval policy.
    ///
    /// The optimisation loop runs in `tm-bench-locomo --gepa` against an
    /// anchor set; this is how its output reaches a running instance.
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
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
    /// Scan local storage and run on-demand cleanups (vacuum, prune
    /// ephemeral signals, truncate the trace log). All actions are
    /// local-only and never touch the network.
    Storage {
        #[command(subcommand)]
        action: StorageAction,
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
    /// CTX-EVG-C — Commitment Ledger reader. Prints the score
    /// (kept / broken / pending) over a sliding window and lists the
    /// commitments themselves. Reads the EVG event-graph directly; does
    /// not conflict with the higher-level Intent-System `commit` /
    /// `resolve` / `commitments` subcommands.
    Ledger {
        /// Window length in days. `0` = all time.
        #[arg(long, default_value = "7")]
        window: u32,
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
    /// CAP-1 — manage per-source ambient capture permissions. Every
    /// source (clipboard, shell, screenshot, browser, audio, calendar)
    /// is opt-in and revocable. Default first-run state has clipboard
    /// + shell enabled; everything else is off until you flip it on.
    /// State persists to `~/.tracemind/capture_permissions.toml`.
    Capture {
        #[command(subcommand)]
        action: CaptureAction,
    },
    /// LM-1 — show the backlinks panel for an entity: every typed
    /// incoming edge with the source entity and confidence. Use to
    /// answer "what links to this entity?" from the terminal, mirrors
    /// the panel rendered in Brief / Dashboard / entity drawer.
    Backlinks {
        /// Entity UUID or exact name (case-insensitive).
        entity: String,
        /// Truncate to N rows. Default: 25.
        #[arg(long, default_value = "25")]
        limit: usize,
        /// Include noisy `RelatedTo` co-occurrence edges. Off by default.
        #[arg(long = "include-related-to")]
        include_related_to: bool,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// LM-9 — inspect and decide on triples in the pending pool.
    /// Mid-confidence relations (extracted, not yet trusted) land in
    /// `pending_relations`; this subcommand is the headless equivalent
    /// of the Tauri "pending" panel.
    Pending {
        #[command(subcommand)]
        action: PendingAction,
    },
    /// LM-5c — Karpathy-style daily note. Upserts a `DailyNote`
    /// entity for today's local date (idempotent — re-running upserts),
    /// auto-creates `RelatedTo` backlinks from every memory created
    /// today, and prints the day's brief. No typing required.
    Today {
        /// Override the local date (YYYY-MM-DD). Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Q4.4 — Run nightly self-improvement tasks on-device.
    /// Triggers GEPA spike, verb affinity update, tier cycle, and
    /// contradiction rate computation. Results are persisted to
    /// `~/.tracemind/nightly_runs.jsonl`.
    Nightly,
}

#[derive(clap::Subcommand)]
enum PendingAction {
    /// List rows in the pending pool, sorted by confidence desc.
    List {
        /// Restrict to one status. Default: `pending`.
        #[arg(long, default_value = "pending")]
        status: String,
        /// Truncate to N rows. Default: 25.
        #[arg(long, default_value = "25")]
        limit: usize,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Accept a pending row — promotes it into `kg_relations` and
    /// stamps the row `accepted`. The pending entry stays for audit.
    Accept {
        /// Pending row UUID (from `pending list`).
        id: String,
        /// Optional human-readable note recorded with the decision.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Reject a pending row — keeps the row out of the graph and
    /// stamps it `rejected`. Stored for audit.
    Reject {
        /// Pending row UUID (from `pending list`).
        id: String,
        /// Optional human-readable note recorded with the decision.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// Purge terminal-state (accepted/rejected) rows older than N days.
    Purge {
        /// Age in days. Rows decided more than `days` days ago are
        /// removed. Default: 30.
        #[arg(long, default_value = "30")]
        days: i64,
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
    /// CTX-EVG Slice A — seed a deterministic thread with a handful of
    /// captures and queries so the EVG view in the desktop app has
    /// non-zero data on a clean install. Starts a thread, writes ~5
    /// `Capture` event nodes + ~2 `Query` event nodes bound to it,
    /// ends the thread, then prints the thread id.
    EvgThread {
        /// Thread title. Defaults to "EVG demo thread".
        #[arg(long, default_value = "EVG demo thread")]
        title: String,
        /// Leave the thread open instead of ending it. Useful when
        /// you want to keep capturing into it from the desktop app.
        #[arg(long)]
        keep_open: bool,
    },
    /// CTX-EVG-C — seed a deterministic Commitment Ledger fixture so
    /// the homepage card has non-empty score on a clean install. Writes
    /// 6 commitments (3 kept, 2 broken, 1 pending) with Resolves edges
    /// for the resolved ones, all in a single demo thread.
    EvgLedger {
        /// Thread title for the fixture.
        #[arg(long, default_value = "Ledger demo thread")]
        title: String,
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
enum StorageAction {
    /// Show on-disk footprint of `$TM_DATA_DIR` (default
    /// `~/.tracemind/`) plus row counts for the graph + signal tables.
    Status {
        /// Emit raw JSON instead of a formatted table.
        #[arg(long)]
        json: bool,
    },
    /// Run `VACUUM` on every SQLite database in the data dir (graph,
    /// temporal index, intents). Reclaims pages freed by deletes.
    Vacuum,
    /// Delete tier-4 (Ephemeral) captured signals and any consolidated
    /// signal above tier 1. Entities + triples are untouched.
    CleanEphemeral,
    /// Truncate `traces.jsonl` to the most recent N lines.
    TruncateTraces {
        /// Keep the last N lines (default 5000).
        #[arg(long, default_value = "5000")]
        keep: usize,
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
    /// LM-14 — merge two contexts. Every entity and triple tagged
    /// `--from-a` or `--from-b` is re-tagged to the new (or existing)
    /// `--into` context. Idempotent: re-running with the same triple
    /// produces no extra rows.
    Merge {
        /// First source context name.
        a: String,
        /// Second source context name.
        b: String,
        /// Target context name. Created if it doesn't exist.
        #[arg(long)]
        into: String,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// LM-14 — split a context. Moves every entity (and its outbound
    /// triples) whose `EntityType` matches `--by-entity-type` to a new
    /// context `--into`. The rest stay in the source context.
    Split {
        /// Source context name.
        name: String,
        /// Match `EntityType` (case-insensitive). Examples: `person`,
        /// `project`, `daily_note`.
        #[arg(long = "by-entity-type", value_name = "TYPE")]
        by_entity_type: String,
        /// Target context name. Created if it doesn't exist.
        #[arg(long)]
        into: String,
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// LM-14 — write a self-contained `.tmctx` snapshot of one context
    /// to disk (entities + triples + metadata). Format: pretty JSON,
    /// schema_version = 1. Use this for archive / share / "snapshot
    /// before merging" workflows.
    Snapshot {
        /// Source context name.
        name: String,
        /// Output path. Default: `<context-name>.tmctx`.
        #[arg(long)]
        output: Option<String>,
    },
}

/// LM-11b — Memory Views CLI subcommands.
#[derive(clap::Subcommand)]
enum ViewAction {
    /// Create a new view. Name must be unique.
    Create {
        /// Short name (e.g. `rondo-only`, `high-trust`, `today`).
        name: String,
        /// Optional human-readable description.
        #[arg(long, default_value = "")]
        description: String,
        /// Drop members whose triple confidence is below this floor
        /// (0.0 disables). Default: 0.0.
        #[arg(long, default_value = "0.0")]
        confidence_floor: f32,
        /// Include rows from `pending_relations` (LM-9 mid-confidence
        /// pool). Default: off.
        #[arg(long, default_value_t = false)]
        include_pending: bool,
    },
    /// List every view, newest-updated first.
    List,
    /// Show one view's metadata + members.
    Show {
        /// View name or UUID.
        name: String,
    },
    /// Add a member to a view.
    Add {
        /// View name or UUID.
        name: String,
        /// Member type: entity | triple | context.
        #[arg(long, default_value = "entity")]
        kind: String,
        /// `include` or `exclude`. Default: include.
        #[arg(long, default_value = "include")]
        mode: String,
        /// UUID of the entity / triple / context.
        id: String,
    },
    /// Remove a member from a view.
    Remove {
        /// View name or UUID.
        name: String,
        /// Member type: entity | triple | context.
        #[arg(long, default_value = "entity")]
        kind: String,
        /// `include` or `exclude`. Default: include.
        #[arg(long, default_value = "include")]
        mode: String,
        /// UUID of the entity / triple / context.
        id: String,
    },
    /// Delete a view and all of its members.
    Delete {
        /// View name or UUID.
        name: String,
    },
    /// Edit a view's metadata (description / confidence floor /
    /// include-pending flag). Pass any flag you want to change.
    Edit {
        /// View name or UUID.
        name: String,
        /// New description.
        #[arg(long)]
        description: Option<String>,
        /// New confidence floor.
        #[arg(long)]
        confidence_floor: Option<f32>,
        /// New include-pending flag.
        #[arg(long)]
        include_pending: Option<bool>,
    },
    /// Mark a view as the *active* one. Subsequent queries default to
    /// this view's splice (overridable per-query). State persists to
    /// `<data_dir>/active_view.json`.
    Use {
        /// View name.
        name: String,
    },
    /// Print the active view (if any).
    Current,
    /// Clear the active view (back to no-view behaviour).
    Clear,
}

#[derive(clap::Subcommand)]
enum CaptureAction {
    /// Show every source with its current enabled/disabled state,
    /// grant timestamp, last event, and lifetime event count. This is
    /// the audit surface — what the daemon would do *if started right
    /// now*.
    Status {
        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Alias for `status` that prints only one source per line, with
    /// short descriptions. Handy for piping into other tools.
    List,
    /// Turn a source ON. Sources: clipboard, shell, screenshot,
    /// browser, audio, calendar. Re-enabling preserves the original
    /// grant timestamp (reaffirmation, not new consent).
    Enable {
        /// Source name (case-insensitive).
        source: String,
    },
    /// Turn a source OFF. The daemon will skip its loop on next
    /// permission check. Existing captures are NOT deleted — use
    /// `tracemind decay` or remove `recent.jsonl` to scrub data.
    Disable {
        /// Source name (case-insensitive).
        source: String,
    },
    /// CAP-2 — ingest the last N days of shell history so the *first*
    /// query post-install is non-empty (UX-8 "60-second meaningful
    /// brief"). Skips entries older than the cutoff, blanks, and
    /// trivial commands (ls/cd/pwd…). Honors capture permissions:
    /// shell must be enabled. Idempotent — re-running deduplicates
    /// against the existing memory.
    Backfill {
        /// How many days of history to scan (default: 7). Applies to
        /// shell and notes; clipboard is a one-shot snapshot.
        #[arg(long, default_value = "7")]
        days: u32,
        /// Maximum number of distinct shell commands to ingest in
        /// this run (safety cap so a 50k-line history file doesn't
        /// stall the daemon).
        #[arg(long, default_value = "500")]
        max: usize,
        /// Skip shell-history backfill.
        #[arg(long, default_value_t = false)]
        no_shell: bool,
        /// Skip Apple Notes backfill (macOS only).
        #[arg(long, default_value_t = false)]
        no_notes: bool,
        /// Skip clipboard snapshot.
        #[arg(long, default_value_t = false)]
        no_clipboard: bool,
    },
    /// CAP-4 — print a one-line JavaScript bookmarklet snippet the
    /// user can drag to their bookmarks bar. Tapping the bookmark on
    /// any page POSTs `{url, title, selection}` to the capture
    /// daemon's localhost endpoint, gated by the per-install
    /// `capture_token`. The daemon must be running and `browser`
    /// must be enabled.
    Bookmarklet {
        /// Print install instructions in addition to the snippet.
        #[arg(long)]
        full: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
enum PolicyAction {
    /// Print the policy currently in force.
    Show,
    /// Promote a policy produced by a GEPA run (its `best_policy` field,
    /// or a bare policy object) into `~/.tracemind/policy.json`.
    Set {
        /// Path to the GEPA run report, or a JSON file holding a policy.
        path: PathBuf,
    },
    /// Delete `policy.json`, reverting to the compiled-in defaults.
    Reset,
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

            // The retraction beat (holistic review §5 P1.4). Surface any
            // fact this store reversed — the wedge behaviour.
            if !result.contradictions.is_empty() {
                println!("  ⚠ Retraction:");
                for c in &result.contradictions {
                    println!("    {}", c.message);
                }
            }

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

        Commands::Query {
            text,
            cross_context,
            view,
            include_entity,
            exclude_entity,
        } => {
            let reranker = ColbertReranker::auto_download_or_none(0.7);
            let mut engine = RetrievalEngine::open(&db_path, &trace_path, cli.hash_embed)
                .expect("failed to open retrieval engine")
                .with_reranker_instance(reranker);
            engine.set_cross_context(cross_context);
            // LM-11c: resolve view (CLI override > active view) and
            // attach ad-hoc include/exclude entity flags. An empty
            // `--view ''` string force-disables the active view.
            if let Some(filter) =
                resolve_view_filter(&dir, &db_path, view.as_deref(), &include_entity, &exclude_entity)
            {
                engine.set_view_filter(Some(filter));
            }
            let result = engine.query(&text).expect("query failed");

            // Sprint C-0.7 — surface the query_id so users can wire
            // retraction feedback via `tracemind not-related <query_id> <result_id>`.
            println!("Query: {}", result.query_id);

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

        Commands::Ask {
            text,
            tier,
            task,
            max_tokens,
            grounding,
            json,
            cross_context,
            view,
            include_entity,
            exclude_entity,
        } => {
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
                cross_context,
                view.as_deref(),
                &include_entity,
                &exclude_entity,
                &dir,
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

        Commands::Helpful {
            query_id,
            result_id,
            weight,
            kind,
            context_id,
        } => {
            let qid = match Uuid::parse_str(&query_id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("error: invalid query_id (expect UUID): {e}");
                    std::process::exit(1);
                }
            };
            let ctx = context_id
                .as_deref()
                .map(Uuid::parse_str)
                .transpose()
                .unwrap_or_else(|e| {
                    eprintln!("error: invalid --context-id: {e}");
                    std::process::exit(1);
                });
            let graph = match GraphStore::open(&db_path) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("error: failed to open graph store: {e}");
                    std::process::exit(1);
                }
            };
            let row_id = match graph.write_positive_signal(qid, &result_id, &kind, ctx, weight) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: failed to write positive signal: {e}");
                    std::process::exit(1);
                }
            };
            println!(
                "positive signal recorded (row {row_id}, query {qid}, result {result_id}, weight {weight}, kind {kind})"
            );
        }

        Commands::Decay { factor, threshold } => {
            let graph = GraphStore::open(&db_path)
                .expect("failed to open graph store");
            let below = graph.decay_all(factor, threshold)
                .expect("decay failed");
            println!("Decay applied (factor={factor}). {below} entities below {threshold} threshold.");
        }

        Commands::Relabel => {
            let graph = GraphStore::open(&db_path)
                .expect("failed to open graph store");
            let cstats = graph.recompute_communities()
                .expect("recompute_communities failed");
            println!(
                "communities: {} entities → {} communities (modularity {:.3})",
                cstats.n_entities, cstats.n_communities, cstats.modularity
            );
            let lstats = graph.recompute_community_labels()
                .expect("recompute_community_labels failed");
            println!(
                "labels: {} populated communities, {} labeled, {} skipped",
                lstats.n_communities, lstats.n_labeled, lstats.n_skipped
            );
        }

        Commands::Reclassify { dry_run } => {
            use tm_ingest::classify_token;
            use tm_types::EntityType;
            let graph = GraphStore::open(&db_path)
                .expect("failed to open graph store");
            let rows = graph.list_entity_types()
                .expect("failed to list entities");
            let total = rows.len();
            let mut changed = 0usize;
            let mut unchanged = 0usize;
            let mut preserved_custom = 0usize;
            for (id, name, current) in rows {
                // Only touch entities currently typed as one of the three
                // labels the heuristic actually produces. `MapOfContent`,
                // `Custom(_)`, `Event`, `Url`, `File`, `Technology` are
                // either graph-system or LLM-assigned and outside the
                // heuristic's competence — leave them alone.
                if !matches!(
                    current,
                    EntityType::Person | EntityType::Organization | EntityType::Concept
                ) {
                    preserved_custom += 1;
                    continue;
                }
                // Restrict to single-token entities — multi-word Person
                // classifications include real full names ("Aaditya
                // Srivathsan") that the heuristic can't distinguish from
                // noun phrases. Only the cheap single-token downgrades
                // are safe to apply automatically.
                if name.split_whitespace().count() > 1 {
                    unchanged += 1;
                    continue;
                }
                let proposed = classify_token(&name);
                let Some(proposed) = proposed else {
                    unchanged += 1;
                    continue;
                };
                if proposed == current {
                    unchanged += 1;
                    continue;
                }
                // Don't *upgrade* a Concept to Person/Org — too risky
                // ("Anthropic" looks like a Person to the heuristic).
                // Only the *downgrade* direction (Person/Org → Concept)
                // is safe enough to apply automatically.
                let is_safe_downgrade = matches!(
                    (&current, &proposed),
                    (EntityType::Person, EntityType::Concept)
                        | (EntityType::Organization, EntityType::Concept)
                        | (EntityType::Person, EntityType::Organization)
                );
                if !is_safe_downgrade {
                    unchanged += 1;
                    continue;
                }
                println!(
                    "  {:<32} {:>14?} → {:?}",
                    truncate_str(&name, 32),
                    current,
                    proposed,
                );
                if !dry_run {
                    if let Err(e) = graph.set_entity_type(id, proposed.clone()) {
                        eprintln!("    ! update failed: {e}");
                    } else {
                        changed += 1;
                    }
                } else {
                    changed += 1;
                }
            }
            println!(
                "\n{}: {} reviewed | {} reclassified | {} unchanged | {} non-heuristic preserved",
                if dry_run { "DRY RUN" } else { "Reclassify done" },
                total, changed, unchanged, preserved_custom
            );
        }

        Commands::PurgeStopwords { dry_run } => {
            use tm_ingest::classify_token;
            use tm_types::EntityType;
            let graph = GraphStore::open(&db_path)
                .expect("failed to open graph store");
            let rows = graph.list_entity_types()
                .expect("failed to list entities");
            let total = rows.len();
            let mut purged = 0usize;
            let mut skipped_multiword = 0usize;
            let mut skipped_non_heuristic = 0usize;
            let mut skipped_valid = 0usize;
            for (id, name, current) in rows {
                // Restrict to the three types the heuristic actually produces.
                if !matches!(
                    current,
                    EntityType::Person | EntityType::Organization | EntityType::Concept
                ) {
                    skipped_non_heuristic += 1;
                    continue;
                }
                // Single-token only — multi-word entities ("Aaditya Srivathsan",
                // "Google Gemini") aren't candidates for stopword purge.
                if name.split_whitespace().count() > 1 {
                    skipped_multiword += 1;
                    continue;
                }
                // classify_token returns None for STOPWORDS / SKIP_WORDS /
                // pure-punctuation / too-short tokens — exactly the noise
                // we want to delete.
                if classify_token(&name).is_some() {
                    skipped_valid += 1;
                    continue;
                }
                println!("  purge {:<32} ({:?})", truncate_str(&name, 32), current);
                if !dry_run {
                    if let Err(e) = graph.delete_entity(id) {
                        eprintln!("    ! delete failed: {e}");
                    } else {
                        purged += 1;
                    }
                } else {
                    purged += 1;
                }
            }
            println!(
                "\n{}: {} reviewed | {} purged | {} valid kept | {} multi-word kept | {} non-heuristic kept",
                if dry_run { "DRY RUN" } else { "Purge done" },
                total, purged, skipped_valid, skipped_multiword, skipped_non_heuristic
            );
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

        Commands::Export {
            context,
            format,
            output,
            view,
            entity,
            redact,
            limit,
        } => {
            if let Err(e) = cmd_export(
                &db_path,
                context.as_deref(),
                &format,
                &output,
                view.as_deref(),
                entity.as_deref(),
                redact,
                limit,
            ) {
                eprintln!("export failed: {e}");
                std::process::exit(1);
            }
        }

        Commands::Policy { action } => {
            let policy_path = data_dir().join("policy.json");
            match action {
                PolicyAction::Show => {
                    let (policy, source) = match std::fs::read_to_string(&policy_path) {
                        Ok(raw) => match serde_json::from_str::<tm_gepa::RetrievalPolicy>(&raw) {
                            Ok(p) => (p, format!("{}", policy_path.display())),
                            Err(e) => {
                                eprintln!("policy.json is unparseable ({e}); showing defaults");
                                (tm_gepa::RetrievalPolicy::default(), "compiled-in defaults".into())
                            }
                        },
                        Err(_) => (
                            tm_gepa::RetrievalPolicy::default(),
                            "compiled-in defaults".into(),
                        ),
                    };
                    println!("source: {source}");
                    for (space, w) in policy.normalised_weights() {
                        println!("  {space:<12} {w:.3}");
                    }
                    println!("  min_score      {:.3}", policy.min_score);
                    println!("  cand_mult      {}", policy.candidate_multiplier);
                    println!("  coverage_wt    {:.3}", policy.coverage_weight);
                    println!("  fit_wt         {:.3}", policy.fit_weight);
                    println!("  generic_boost  {:.3}", policy.generic_coverage_boost);
                }
                PolicyAction::Set { path } => {
                    let raw = match std::fs::read_to_string(&path) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("cannot read {}: {e}", path.display());
                            std::process::exit(1);
                        }
                    };
                    let value: serde_json::Value = match serde_json::from_str(&raw) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("{} is not valid JSON: {e}", path.display());
                            std::process::exit(1);
                        }
                    };
                    // Accept either a GEPA run report or a bare policy.
                    let policy_value = value
                        .pointer("/result/best_policy")
                        .or_else(|| value.pointer("/best_policy"))
                        .cloned()
                        .unwrap_or(value);
                    let policy: tm_gepa::RetrievalPolicy =
                        match serde_json::from_value(policy_value) {
                            Ok(p) => p,
                            Err(e) => {
                                eprintln!("no retrieval policy found in {}: {e}", path.display());
                                std::process::exit(1);
                            }
                        };
                    let encoded = serde_json::to_string_pretty(&policy)
                        .expect("policy serialises");
                    if let Err(e) = std::fs::write(&policy_path, encoded) {
                        eprintln!("cannot write {}: {e}", policy_path.display());
                        std::process::exit(1);
                    }
                    println!("promoted policy to {}", policy_path.display());
                    println!("it takes effect on the next query (CLI, MCP, or desktop app)");
                }
                PolicyAction::Reset => {
                    match std::fs::remove_file(&policy_path) {
                        Ok(()) => println!("removed {}", policy_path.display()),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            println!("no policy.json; already on compiled-in defaults")
                        }
                        Err(e) => {
                            eprintln!("cannot remove {}: {e}", policy_path.display());
                            std::process::exit(1);
                        }
                    }
                }
            }
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
        Commands::Storage { action } => {
            cmd_storage(&dir, action);
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
        Commands::Ledger { window, json } => {
            cmd_evg_ledger(&dir, window, json);
        }
        Commands::Contradictions { action } => {
            let db_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_contradictions(&db_path, action);
        }
        Commands::Context { action } => {
            let db_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_context(&dir, &db_path, action);
        }
        Commands::View { action } => {
            let db_path = dir.join("memory.db").to_str().unwrap().to_string();
            cmd_view(&dir, &db_path, action);
        }
        Commands::Capture { action } => {
            cmd_capture(&dir, action);
        }
        Commands::Backlinks {
            entity,
            limit,
            include_related_to,
            json,
        } => {
            if let Err(e) = cmd_backlinks(&db_path, &entity, limit, include_related_to, json) {
                eprintln!("backlinks failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::Pending { action } => {
            if let Err(e) = cmd_pending(&db_path, action) {
                eprintln!("pending failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::Today { date, json } => {
            if let Err(e) = cmd_today(&db_path, date.as_deref(), json) {
                eprintln!("today failed: {e}");
                std::process::exit(1);
            }
        }
        Commands::Nightly => {
            let scheduler = tm_controller::NightlyScheduler::new(dir.to_path_buf());
            let mut record = scheduler.run();

            // Compute the real contradiction rate from the local graph —
            // wiring the previously-orphaned tm-graph::contradiction_rate
            // (holistic review §6a: report real signals, not fabricated
            // ones). Best-effort: a failure here must not fail the run.
            let db_path = dir.join("memory.db");
            if db_path.exists() {
                if let Ok(store) = tm_graph::GraphStore::open(db_path.to_str().unwrap_or_default()) {
                    if let Ok(stats) = store.contradiction_rate_stats() {
                        record.contradiction_rate = Some(stats.rate);
                    }
                }
            }
            scheduler.record(&record).ok();

            println!("Nightly self-improvement run:");
            println!("  retraction beats fired : {}", record.retractions_fired.map(|n| n.to_string()).unwrap_or_else(|| "0 (never)".into()));
            match record.contradiction_rate {
                Some(r) => println!("  contradiction rate     : {r:.3}"),
                None => println!("  contradiction rate     : (not computed)"),
            }
            println!("  history                : {}", dir.join("nightly_runs.jsonl").display());
        }
    }
}

/// LM-1: `tracemind backlinks <entity>` — print every typed incoming
/// edge for the resolved entity. Mirrors `GraphStore::backlinks` and
/// reuses its sort/limit semantics. Accepts a UUID or an exact name
/// (case-insensitive).
fn cmd_backlinks(
    db_path: &str,
    entity_arg: &str,
    limit: usize,
    include_related_to: bool,
    as_json: bool,
) -> Result<(), String> {
    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;

    let target = if let Ok(id) = Uuid::parse_str(entity_arg) {
        graph
            .get_entity(id)
            .map_err(|_| format!("no entity with id '{entity_arg}'"))?
    } else {
        graph
            .find_entity_by_name_icase(entity_arg)
            .map_err(|e| format!("lookup entity '{entity_arg}': {e}"))?
            .ok_or_else(|| format!("no entity named '{entity_arg}'"))?
    };

    let rows = graph
        .backlinks(target.id, Some(limit), include_related_to)
        .map_err(|e| format!("backlinks: {e}"))?;

    if as_json {
        let payload = serde_json::json!({
            "entity": {
                "id": target.id.to_string(),
                "name": target.name,
                "type": target.entity_type.to_string(),
            },
            "backlinks": rows.iter().map(|b| serde_json::json!({
                "triple_id": b.triple_id.to_string(),
                "source_id": b.source.id.to_string(),
                "source_name": b.source.name,
                "source_type": b.source.entity_type.to_string(),
                "predicate": b.predicate.to_string(),
                "confidence": b.confidence,
            })).collect::<Vec<_>>(),
            "count": rows.len(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload)
                .map_err(|e| format!("serialize: {e}"))?
        );
        return Ok(());
    }

    println!(
        "Backlinks for [{}] {} ({})",
        target.entity_type, target.name, target.id
    );
    if rows.is_empty() {
        println!("  (none)");
        return Ok(());
    }
    for b in &rows {
        println!(
            "  ← [{}] {} — {} _(conf {:.2})_",
            b.source.entity_type, b.source.name, b.predicate, b.confidence
        );
    }
    println!("  ({} total)", rows.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// `tracemind pending …` — LM-9
// ---------------------------------------------------------------------------

/// Top-level dispatcher for the `tracemind pending` family of
/// subcommands. The pending pool is the user-facing queue of
/// mid-confidence triples that the ingest pipeline declined to write
/// directly to `kg_relations` — `accept` promotes a row, `reject`
/// keeps it out for good, and `purge` cleans up decided rows.
fn cmd_pending(db_path: &str, action: PendingAction) -> Result<(), String> {
    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    match action {
        PendingAction::List { status, limit, json } => {
            let filter = match status.as_str() {
                "any" | "all" => None,
                other => Some(
                    tm_graph::PendingStatus::parse(other)
                        .map_err(|e| format!("status filter: {e}"))?,
                ),
            };
            let rows = graph
                .list_pending(filter, Some(limit))
                .map_err(|e| format!("list pending: {e}"))?;
            if json {
                let payload = serde_json::json!({
                    "count": rows.len(),
                    "rows": rows.iter().map(|r| serde_json::json!({
                        "id": r.id.to_string(),
                        "subject_id": r.subject_id.to_string(),
                        "predicate": r.predicate,
                        "object_id": r.object_id.to_string(),
                        "confidence": r.confidence,
                        "source_id": r.source_id,
                        "status": r.status,
                        "created_at": r.created_at.to_rfc3339(),
                        "decided_at": r.decided_at.map(|d| d.to_rfc3339()),
                        "note": r.note,
                    })).collect::<Vec<_>>(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload)
                        .map_err(|e| format!("serialize: {e}"))?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!("(no pending rows)");
                return Ok(());
            }
            for r in &rows {
                let subj = graph
                    .get_entity(r.subject_id)
                    .map(|e| e.name)
                    .unwrap_or_else(|_| r.subject_id.to_string());
                let obj = graph
                    .get_entity(r.object_id)
                    .map(|e| e.name)
                    .unwrap_or_else(|_| r.object_id.to_string());
                println!(
                    "  [{}] {} — {} → {}  _(conf {:.2}, id {})_",
                    r.status.as_str(),
                    subj,
                    r.predicate,
                    obj,
                    r.confidence,
                    r.id
                );
            }
            println!("  ({} total)", rows.len());
            Ok(())
        }
        PendingAction::Accept { id, note } => {
            let uuid =
                Uuid::parse_str(&id).map_err(|_| format!("invalid uuid '{id}'"))?;
            let triple = graph
                .accept_pending(uuid, &note)
                .map_err(|e| format!("accept: {e}"))?;
            println!(
                "accepted: triple {} ({} → {} → {}) conf={:.2}",
                triple.id,
                triple.subject_id,
                triple.predicate,
                triple.object_id,
                triple.confidence
            );
            Ok(())
        }
        PendingAction::Reject { id, note } => {
            let uuid =
                Uuid::parse_str(&id).map_err(|_| format!("invalid uuid '{id}'"))?;
            let ok = graph
                .reject_pending(uuid, &note)
                .map_err(|e| format!("reject: {e}"))?;
            if ok {
                println!("rejected: {uuid}");
            } else {
                eprintln!("no pending row with id {uuid}");
                std::process::exit(1);
            }
            Ok(())
        }
        PendingAction::Purge { days } => {
            let n = graph
                .purge_pending(days)
                .map_err(|e| format!("purge: {e}"))?;
            println!("purged {n} terminal-state rows older than {days} day(s)");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// `tracemind today` — LM-5c
// ---------------------------------------------------------------------------

/// LM-5c — upsert a first-class `DailyNote` entity for today's local
/// date (or `--date` override), then auto-create `RelatedTo` backlinks
/// from every entity created on that day → the daily note. Idempotent:
/// re-running on the same date upserts the entity and only adds
/// missing backlinks. No typing required — this is the Karpathy daily
/// note workflow.
fn cmd_today(db_path: &str, date_override: Option<&str>, as_json: bool) -> Result<(), String> {
    use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

    let target_date: NaiveDate = match date_override {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| format!("invalid --date '{s}': {e} (want YYYY-MM-DD)"))?,
        None => Local::now().date_naive(),
    };
    let note_name = target_date.format("%Y-%m-%d").to_string();

    // UTC day window matching the local target date — the bitemporal
    // store records `created_at` in UTC, so convert the local-day
    // boundaries to UTC for the range check.
    let day_start_local = Local
        .from_local_datetime(&target_date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or_else(|| "ambiguous local midnight (DST)".to_string())?;
    let day_end_local = day_start_local + chrono::Duration::days(1);
    let day_start_utc: DateTime<Utc> = day_start_local.with_timezone(&Utc);
    let day_end_utc: DateTime<Utc> = day_end_local.with_timezone(&Utc);

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;

    // Find-or-create the DailyNote entity (idempotent by name).
    let daily = match graph
        .find_entity_by_name_icase(&note_name)
        .map_err(|e| format!("lookup daily note: {e}"))?
    {
        Some(e) if e.entity_type == EntityType::DailyNote => e,
        Some(other) => {
            return Err(format!(
                "name '{note_name}' is already taken by a non-DailyNote entity ({})",
                other.entity_type
            ));
        }
        None => {
            let mut new_note = Entity::new(&note_name, EntityType::DailyNote, 1.0);
            new_note.source_id = Some("tracemind::today".to_string());
            graph
                .upsert_entity(&new_note)
                .map_err(|e| format!("create daily note: {e}"))?;
            new_note
        }
    };

    // List entities created on the target day (excluding the daily
    // note itself).
    let all = graph
        .list_all_entities()
        .map_err(|e| format!("list entities: {e}"))?;
    let same_day: Vec<Entity> = all
        .into_iter()
        .filter(|e| e.id != daily.id)
        .filter(|e| e.created_at >= day_start_utc && e.created_at < day_end_utc)
        .collect();

    // Existing backlinks: triples where object is the daily note. We
    // dedup by subject so re-running doesn't create another edge.
    let existing = graph
        .get_triples_for_entity(daily.id)
        .map_err(|e| format!("read daily-note triples: {e}"))?;
    let already_linked: std::collections::HashSet<Uuid> = existing
        .iter()
        .filter(|t| t.object_id == daily.id)
        .map(|t| t.subject_id)
        .collect();

    let mut created = 0usize;
    let mut skipped = 0usize;
    for e in &same_day {
        if already_linked.contains(&e.id) {
            skipped += 1;
            continue;
        }
        let mut t = tm_types::Triple::new(e.id, tm_types::Predicate::RelatedTo, daily.id, 1.0);
        t.source_id = Some("tracemind::today".to_string());
        graph
            .upsert_triple(&t)
            .map_err(|err| format!("link {} → daily note: {err}", e.id))?;
        created += 1;
    }

    if as_json {
        let payload = serde_json::json!({
            "date": note_name,
            "daily_note_id": daily.id.to_string(),
            "memories_today": same_day.len(),
            "backlinks_created": created,
            "backlinks_existing": skipped,
            "memories": same_day.iter().map(|e| serde_json::json!({
                "id": e.id.to_string(),
                "name": e.name,
                "entity_type": e.entity_type.to_string(),
                "confidence": e.confidence,
                "created_at": e.created_at.to_rfc3339(),
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {e}"))?
        );
        return Ok(());
    }

    println!("# Daily note · {note_name}");
    println!("  id: {}", daily.id);
    println!(
        "  {} memor{} created on this day ({} new backlink{}, {} already linked)",
        same_day.len(),
        if same_day.len() == 1 { "y" } else { "ies" },
        created,
        if created == 1 { "" } else { "s" },
        skipped,
    );
    if same_day.is_empty() {
        println!("  (no other memories created on this day yet)");
        return Ok(());
    }
    println!();
    for e in &same_day {
        println!(
            "  - [{}] {}  _(conf {:.2}, id {})_",
            e.entity_type, e.name, e.confidence, e.id
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `tracemind view …` — LM-11b
// ---------------------------------------------------------------------------

/// Active view pointer, persisted to `<data_dir>/active_view.json`. We
/// keep this in its own file (instead of folding it into
/// `active_context.json`) so that "context" and "view" stay
/// independent surfaces — a user can have an active view but no
/// active context, or vice versa.
#[derive(serde::Serialize, serde::Deserialize)]
struct ActiveView {
    id: Uuid,
    name: String,
}

impl ActiveView {
    fn load(path: &std::path::Path) -> std::io::Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Self>(&s) {
                Ok(v) => Ok(Some(v)),
                Err(_) => Ok(None),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let s = serde_json::to_string_pretty(self).unwrap();
        fs::write(path, s)
    }

    fn clear(path: &std::path::Path) -> std::io::Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// LM-11c — build a [`ViewFilter`] from CLI flags. Precedence:
///
/// 1. `--view '<name>'` (CLI override). Empty string explicitly disables
///    the active view for this one call.
/// 2. The active view stored at `<data_dir>/active_view.json`.
/// 3. No view → returns `None` unless ad-hoc include/exclude flags are
///    present, in which case a minimal filter with just the ad-hoc sets
///    is returned.
///
/// Invalid UUIDs in `--include-entity` / `--exclude-entity` are
/// reported via stderr and skipped (we don't exit — a partial filter
/// is still useful and matches the spirit of the rest of the CLI).
fn resolve_view_filter(
    data_dir: &PathBuf,
    db_path: &str,
    view_name: Option<&str>,
    include_entity: &[String],
    exclude_entity: &[String],
) -> Option<tm_graph::ViewFilter> {
    // Resolve which view (if any) to load.
    let view = match view_name {
        Some("") => None, // explicit disable
        Some(name) => match GraphStore::open(db_path) {
            Ok(graph) => match resolve_view(&graph, name) {
                Some(v) => Some(v),
                None => {
                    eprintln!("warning: no view named '{name}' — skipping view filter");
                    None
                }
            },
            Err(_) => None,
        },
        None => {
            // Fall back to active view, if any.
            let active_path = data_dir.join("active_view.json");
            match ActiveView::load(&active_path).ok().flatten() {
                Some(active) => match GraphStore::open(db_path) {
                    Ok(graph) => graph.get_view(active.id).ok().flatten(),
                    Err(_) => None,
                },
                None => None,
            }
        }
    };

    let mut filter = match view {
        Some(v) => match GraphStore::open(db_path) {
            Ok(graph) => match graph.load_view_filter(v.id) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("warning: failed to load view '{}': {e}", v.name);
                    return None;
                }
            },
            Err(_) => return None,
        },
        None => tm_graph::ViewFilter::default(),
    };

    // Layer in ad-hoc include/exclude UUIDs (not persisted).
    for s in include_entity {
        match Uuid::parse_str(s) {
            Ok(u) => {
                filter.adhoc_include_entities.insert(u);
            }
            Err(_) => eprintln!("warning: --include-entity '{s}' is not a valid UUID"),
        }
    }
    for s in exclude_entity {
        match Uuid::parse_str(s) {
            Ok(u) => {
                filter.adhoc_exclude_entities.insert(u);
            }
            Err(_) => eprintln!("warning: --exclude-entity '{s}' is not a valid UUID"),
        }
    }

    if filter.is_empty() {
        None
    } else {
        Some(filter)
    }
}

/// Resolve a `<name-or-uuid>` argument to a `MemoryView`.
fn resolve_view(graph: &GraphStore, name_or_id: &str) -> Option<tm_graph::MemoryView> {
    if let Ok(id) = Uuid::parse_str(name_or_id) {
        if let Ok(Some(v)) = graph.get_view(id) {
            return Some(v);
        }
    }
    if let Ok(Some(v)) = graph.get_view_by_name(name_or_id) {
        return Some(v);
    }
    None
}

fn parse_member_type(s: &str) -> Result<tm_graph::MemberType, String> {
    match s {
        "entity" => Ok(tm_graph::MemberType::Entity),
        "triple" => Ok(tm_graph::MemberType::Triple),
        "context" => Ok(tm_graph::MemberType::Context),
        other => Err(format!(
            "unknown kind '{other}' — expected entity | triple | context"
        )),
    }
}

fn parse_member_mode(s: &str) -> Result<tm_graph::MemberKind, String> {
    match s {
        "include" => Ok(tm_graph::MemberKind::Include),
        "exclude" => Ok(tm_graph::MemberKind::Exclude),
        other => Err(format!(
            "unknown mode '{other}' — expected include | exclude"
        )),
    }
}

fn cmd_view(data_dir: &PathBuf, db_path: &str, action: ViewAction) {
    let active_path = data_dir.join("active_view.json");
    let graph = match GraphStore::open(db_path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: failed to open graph store: {e}");
            std::process::exit(1);
        }
    };

    match action {
        ViewAction::Create {
            name,
            description,
            confidence_floor,
            include_pending,
        } => {
            let mut view = tm_graph::MemoryView::new(name.clone(), description);
            view.confidence_floor = confidence_floor;
            view.include_pending = include_pending;
            if let Err(e) = graph.create_view(&view) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!("created view: {} ({})", view.name, view.id);
        }
        ViewAction::List => {
            let views = match graph.list_views() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: failed to list views: {e}");
                    std::process::exit(1);
                }
            };
            if views.is_empty() {
                println!("no views yet — `tracemind view create <name>` to add one");
                return;
            }
            let active = ActiveView::load(&active_path).ok().flatten();
            for v in views {
                let marker = match &active {
                    Some(a) if a.id == v.id => "*",
                    _ => " ",
                };
                let floor = if v.confidence_floor > 0.0 {
                    format!(" floor={:.2}", v.confidence_floor)
                } else {
                    String::new()
                };
                let pending = if v.include_pending { " pending" } else { "" };
                println!("{marker} {:<24} {}{}{}", v.name, v.id, floor, pending);
                if !v.description.is_empty() {
                    println!("     {}", v.description);
                }
            }
        }
        ViewAction::Show { name } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            println!("view {} ({})", view.name, view.id);
            if !view.description.is_empty() {
                println!("description: {}", view.description);
            }
            println!("confidence_floor: {:.2}", view.confidence_floor);
            println!("include_pending: {}", view.include_pending);
            println!("created_at: {}", view.created_at.to_rfc3339());
            println!("updated_at: {}", view.updated_at.to_rfc3339());
            let members = match graph.list_view_members(view.id) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("error: failed to list members: {e}");
                    std::process::exit(1);
                }
            };
            if members.is_empty() {
                println!("(no members yet)");
                return;
            }
            println!("members ({}):", members.len());
            for m in members {
                println!(
                    "  {:>7}  {:<7}  {}",
                    m.kind.as_str(),
                    m.member_type.as_str(),
                    m.member_id
                );
            }
        }
        ViewAction::Add { name, kind, mode, id } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            let member_type = match parse_member_type(&kind) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            };
            let member_kind = match parse_member_mode(&mode) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            };
            let member_id = match Uuid::parse_str(&id) {
                Ok(u) => u,
                Err(_) => {
                    eprintln!("error: '{id}' is not a valid UUID");
                    std::process::exit(2);
                }
            };
            if let Err(e) =
                graph.add_view_member(view.id, member_kind, member_type, member_id)
            {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!(
                "added {} {} {} to view {}",
                member_kind.as_str(),
                member_type.as_str(),
                member_id,
                view.name
            );
        }
        ViewAction::Remove { name, kind, mode, id } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            let member_type = match parse_member_type(&kind) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            };
            let member_kind = match parse_member_mode(&mode) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            };
            let member_id = match Uuid::parse_str(&id) {
                Ok(u) => u,
                Err(_) => {
                    eprintln!("error: '{id}' is not a valid UUID");
                    std::process::exit(2);
                }
            };
            match graph.remove_view_member(view.id, member_kind, member_type, member_id) {
                Ok(true) => println!(
                    "removed {} {} {} from view {}",
                    member_kind.as_str(),
                    member_type.as_str(),
                    member_id,
                    view.name
                ),
                Ok(false) => println!("no such member in view {}", view.name),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        ViewAction::Delete { name } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            match graph.delete_view(view.id) {
                Ok(true) => {
                    // If the active view was this one, clear the pointer.
                    if let Ok(Some(active)) = ActiveView::load(&active_path) {
                        if active.id == view.id {
                            let _ = ActiveView::clear(&active_path);
                        }
                    }
                    println!("deleted view: {}", view.name);
                }
                Ok(false) => println!("no such view: {}", view.name),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        ViewAction::Edit {
            name,
            description,
            confidence_floor,
            include_pending,
        } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            if let Err(e) = graph.update_view_metadata(
                view.id,
                description.as_deref(),
                confidence_floor,
                include_pending,
            ) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            println!("updated view: {}", view.name);
        }
        ViewAction::Use { name } => {
            let view = match resolve_view(&graph, &name) {
                Some(v) => v,
                None => {
                    eprintln!("error: no view named '{name}'");
                    std::process::exit(1);
                }
            };
            let active = ActiveView { id: view.id, name: view.name.clone() };
            if let Err(e) = active.save(&active_path) {
                eprintln!("error: failed to save active view: {e}");
                std::process::exit(1);
            }
            println!("active view → {} ({})", view.name, view.id);
        }
        ViewAction::Current => match ActiveView::load(&active_path) {
            Ok(Some(a)) => println!("active view: {} ({})", a.name, a.id),
            Ok(None) => println!("no active view"),
            Err(e) => {
                eprintln!("error: failed to read active view: {e}");
                std::process::exit(1);
            }
        },
        ViewAction::Clear => {
            if let Err(e) = ActiveView::clear(&active_path) {
                eprintln!("error: failed to clear active view: {e}");
                std::process::exit(1);
            }
            println!("active view cleared");
        }
    }
}

/// `tracemind capture …` handler (CAP-1). Reads/writes
/// `<data_dir>/capture_permissions.toml` via the schema in
/// `tm_types::capture_permissions`. The daemon
/// (`tracemind-capture`) reads the same file at startup and skips
/// loops whose source is disabled.
fn cmd_capture(data_dir: &PathBuf, action: CaptureAction) {
    use tm_types::capture_permissions::{CapturePermissions, CaptureSource};

    let path = data_dir.join("capture_permissions.toml");
    let mut perms = match CapturePermissions::load_or_default(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to load capture permissions: {e}");
            std::process::exit(1);
        }
    };

    match action {
        CaptureAction::Status { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&perms).unwrap());
                return;
            }
            println!("capture permissions ({}):", path.display());
            for source in CaptureSource::all() {
                let p = perms.get(source);
                let state = if p.enabled { "ON " } else { "off" };
                let granted = p
                    .granted_at
                    .map(|t| t.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".to_string());
                let last = p
                    .last_event_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "—".to_string());
                println!(
                    "  [{state}] {:<10}  granted {granted}  last {last}  events {}",
                    source.as_str(),
                    p.event_count
                );
            }
        }
        CaptureAction::List => {
            for source in CaptureSource::all() {
                let p = perms.get(source);
                let state = if p.enabled { "on " } else { "off" };
                println!("{state}  {:<10}  {}", source.as_str(), source.description());
            }
        }
        CaptureAction::Enable { source } => {
            let Some(src) = CaptureSource::parse(&source) else {
                eprintln!("unknown source: {source}");
                eprintln!("valid: clipboard, shell, notes, screenshot, browser, audio, calendar");
                std::process::exit(2);
            };
            perms.enable(src);
            if let Err(e) = perms.save(&path) {
                eprintln!("failed to save: {e}");
                std::process::exit(1);
            }
            println!("enabled: {src}");
        }
        CaptureAction::Disable { source } => {
            let Some(src) = CaptureSource::parse(&source) else {
                eprintln!("unknown source: {source}");
                eprintln!("valid: clipboard, shell, notes, screenshot, browser, audio, calendar");
                std::process::exit(2);
            };
            perms.disable(src);
            if let Err(e) = perms.save(&path) {
                eprintln!("failed to save: {e}");
                std::process::exit(1);
            }
            println!("disabled: {src}");
        }
        CaptureAction::Backfill {
            days,
            max,
            no_shell,
            no_notes,
            no_clipboard,
        } => {
            // CAP-2 — three-source startup backfill (shell + notes +
            // clipboard) so the seed user query post-install is
            // non-empty. Each source is fail-closed against its own
            // CaptureSource permission, skippable via --no-*.
            let db_path = data_dir.join("memory.db").to_str().unwrap().to_string();

            // Shell history.
            if no_shell {
                println!("skipped shell-history backfill (--no-shell)");
            } else if !perms.is_enabled(CaptureSource::Shell) {
                eprintln!("shell capture disabled — skipping (run `tracemind capture enable shell` to opt in)");
            } else {
                let ingested = cmd_capture_backfill_shell(&db_path, days, max);
                println!("backfilled {ingested} shell history entries (last {days} days)");
            }

            // Apple Notes (macOS only).
            if no_notes {
                println!("skipped Apple Notes backfill (--no-notes)");
            } else if !perms.is_enabled(CaptureSource::Notes) {
                eprintln!("notes capture disabled — skipping (run `tracemind capture enable notes` to opt in)");
            } else {
                let ingested = cmd_capture_backfill_notes(&db_path, days);
                println!("backfilled {ingested} Apple Notes entries (last {days} days)");
            }

            // Clipboard snapshot (one-shot).
            if no_clipboard {
                println!("skipped clipboard snapshot (--no-clipboard)");
            } else if !perms.is_enabled(CaptureSource::Clipboard) {
                eprintln!("clipboard capture disabled — skipping (run `tracemind capture enable clipboard` to opt in)");
            } else {
                let ingested = cmd_capture_backfill_clipboard(&db_path);
                if ingested > 0 {
                    println!("backfilled clipboard snapshot ({ingested} entry)");
                } else {
                    println!("clipboard was empty or skipped (too short / high-entropy)");
                }
            }
        }
        CaptureAction::Bookmarklet { full } => {
            cmd_capture_bookmarklet(data_dir, full);
        }
    }
}

/// CAP-4 — emit the bookmarklet snippet. We read the per-install
/// token from `<data_dir>/capture_token` (the capture daemon writes
/// it on first start). If the token doesn't exist yet, we generate
/// one here so the user can install the bookmarklet *before* booting
/// the daemon. The same file is read by the daemon's HTTP loop.
fn cmd_capture_bookmarklet(data_dir: &PathBuf, full: bool) {
    let token_path = data_dir.join("capture_token");
    let token = match std::fs::read_to_string(&token_path) {
        Ok(t) => t.trim().to_string(),
        Err(_) => {
            // Generate + persist a token so the bookmarklet works on
            // first run, before the daemon has booted.
            if let Err(e) = std::fs::create_dir_all(data_dir) {
                eprintln!("failed to create {}: {e}", data_dir.display());
                std::process::exit(1);
            }
            let t = Uuid::new_v4().simple().to_string();
            if let Err(e) = std::fs::write(&token_path, &t) {
                eprintln!("failed to write {}: {e}", token_path.display());
                std::process::exit(1);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &token_path,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
            eprintln!("[tracemind] new capture token written to {}", token_path.display());
            t
        }
    };

    // Compose the JS payload. Keep it minimal: grab the URL, title,
    // and selection; POST as JSON; show a tiny toast on success.
    let js = format!(
        "javascript:(function(){{var s=window.getSelection?String(window.getSelection()):'';fetch('http://127.0.0.1:7710/capture',{{method:'POST',headers:{{'Content-Type':'application/json','Authorization':'Bearer {token}'}},body:JSON.stringify({{url:location.href,title:document.title,selection:s}})}}).then(r=>{{var t=document.createElement('div');t.textContent=r.ok?'ok captured':'fail '+r.status;t.style.cssText='position:fixed;top:12px;right:12px;padding:8px 12px;background:#222;color:#fff;border-radius:6px;font:13px sans-serif;z-index:2147483647;opacity:0.9';document.body.appendChild(t);setTimeout(()=>t.remove(),1500);}}).catch(e=>alert('TraceMind capture failed: '+e));}})();"
    );

    if full {
        println!("# TraceMind browser bookmarklet (CAP-4)");
        println!();
        println!("1. Make sure the capture daemon is running:");
        println!("     tracemind-capture");
        println!();
        println!("2. Make sure browser capture is enabled:");
        println!("     tracemind capture enable browser");
        println!();
        println!("3. Drag this JS snippet to your bookmarks bar (or create a new bookmark with the URL field set to it):");
        println!();
        println!("{js}");
        println!();
        println!("Token persisted at: {}", token_path.display());
    } else {
        println!("{js}");
    }
}

/// CAP-2 — read shell history, filter by age + triviality, and
/// ingest the surviving entries through `IngestPipeline::ingest_fast`.
/// Mirrors the daemon's parser (`tm_capture::read_last_lines`) but
/// is age-aware: zsh's `: <epoch>:0;<command>` format gives us the
/// timestamp, so we can drop anything older than `days` cleanly.
/// Bash history has no timestamps by default — we take the last
/// `max` non-trivial lines and ingest them all.
fn cmd_capture_backfill_shell(db_path: &str, days: u32, max: usize) -> usize {
    use std::path::PathBuf;
    use tm_ingest::IngestPipeline;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let zsh = PathBuf::from(&home).join(".zsh_history");
    let bash = PathBuf::from(&home).join(".bash_history");
    let (path, is_zsh) = if zsh.exists() {
        (zsh, true)
    } else if bash.exists() {
        (bash, false)
    } else {
        eprintln!("no shell history file found at ~/.zsh_history or ~/.bash_history");
        return 0;
    };

    let cutoff = chrono::Utc::now().timestamp() - i64::from(days) * 86_400;
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("read {}: {e}", path.display());
            return 0;
        }
    };

    let mut entries: Vec<String> = Vec::new();
    // Walk newest-first so the safety cap keeps the *most recent* commands.
    for line in raw.lines().rev() {
        if entries.len() >= max {
            break;
        }
        // zsh: ": 1715450000:0;cargo build --release"
        let (ts, cmd) = if is_zsh && line.starts_with(": ") {
            let rest = &line[2..];
            let (ts_str, rest) = match rest.split_once(':') {
                Some(p) => p,
                None => continue,
            };
            let cmd = match rest.split_once(';') {
                Some((_, c)) => c,
                None => continue,
            };
            let ts = ts_str.parse::<i64>().unwrap_or(0);
            (Some(ts), cmd)
        } else {
            (None, line)
        };
        if let Some(ts) = ts {
            if ts < cutoff {
                continue;
            }
        }
        let cmd = cmd.trim();
        if cmd.len() <= 5 || cmd.starts_with('#') {
            continue;
        }
        if matches!(
            cmd,
            "ls" | "cd" | "pwd" | "clear" | "exit" | "history" | "ll" | "la"
        ) {
            continue;
        }
        entries.push(cmd.to_string());
    }

    if entries.is_empty() {
        return 0;
    }

    // Reverse so we ingest oldest→newest (preserving causal order).
    entries.reverse();

    // Backfill is a bulk one-shot bypass the per-source token bucket
    // (CAP-5) so a 7-day history doesn't get throttled to 2/min.
    let pipeline = match IngestPipeline::open(db_path, /*hash_embed=*/ false) {
        Ok(p) => p.with_rate_limiter(tm_ingest::RateLimiter::unlimited()),
        Err(e) => {
            eprintln!("open pipeline: {e}");
            return 0;
        }
    };

    let session = Uuid::new_v4();
    let mut ingested = 0usize;
    for cmd in entries {
        let text = format!("shell command: {cmd}");
        match pipeline.ingest_fast(&text, "shell-backfill", session) {
            Ok(r) if r.skipped.is_none() => ingested += 1,
            Ok(_) => {}
            Err(e) => {
                eprintln!("ingest_fast failed: {e}");
            }
        }
    }
    ingested
}

/// CAP-2 — Apple Notes backfill (macOS only). We shell out to
/// `osascript` and ask the Notes app to enumerate every note's
/// `folder`, `name`, `body`, and `modificationDate`. Notes has no
/// public change-event API, so we treat this as a startup-time + on-
/// demand dump; the daemon does *not* poll continuously. Re-running
/// `tracemind capture backfill` picks up edits (`ingest_fast` is
/// idempotent against the existing memory).
///
/// On non-macOS platforms this is a no-op.
fn cmd_capture_backfill_notes(db_path: &str, days: u32) -> usize {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (db_path, days);
        eprintln!("Apple Notes backfill is macOS-only — skipping");
        return 0;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        use tm_ingest::IngestPipeline;

        // Field separator: pick something extremely unlikely to occur
        // inside a note body, but still printable so we can split in
        // Rust. Record separator (\u{1E}) and unit separator (\u{1F})
        // are the canonical ASCII choices. AppleScript can emit them
        // via `character id N`.
        //
        // Record  := folder \u{1F} title \u{1F} epoch_seconds \u{1F} body
        // Records joined by \u{1E}.
        let script = r#"
set _rs to character id 30
set _us to character id 31
set _out to ""
tell application "Notes"
    set _notes to every note
    repeat with _n in _notes
        try
            set _folder to name of container of _n
        on error
            set _folder to ""
        end try
        set _title to name of _n
        set _modDate to modification date of _n
        -- AppleScript epoch trick: subtract the Unix epoch.
        set _epoch to (_modDate - (date "Thursday, January 1, 1970 at 12:00:00 AM")) as integer
        set _body to plaintext of _n
        if _out is not "" then
            set _out to _out & _rs
        end if
        set _out to _out & _folder & _us & _title & _us & _epoch & _us & _body
    end repeat
end tell
return _out
"#;

        let output = match Command::new("osascript").arg("-e").arg(script).output() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("osascript failed (is Apple Notes available?): {e}");
                return 0;
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("osascript exited non-zero: {}", stderr.trim());
            // Common case: user hasn't granted Automation access yet.
            // Surface a hint and move on without crashing the rest of
            // the backfill.
            if stderr.contains("-1743") || stderr.to_lowercase().contains("not authorized") {
                eprintln!("hint: System Settings → Privacy & Security → Automation → allow Terminal/iTerm to control Notes");
            }
            return 0;
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        let raw = raw.trim();
        if raw.is_empty() {
            return 0;
        }

        let cutoff = chrono::Utc::now().timestamp() - i64::from(days) * 86_400;
        let pipeline = match IngestPipeline::open(db_path, /*hash_embed=*/ false) {
            Ok(p) => p.with_rate_limiter(tm_ingest::RateLimiter::unlimited()),
            Err(e) => {
                eprintln!("open pipeline: {e}");
                return 0;
            }
        };
        let session = Uuid::new_v4();

        let rs = char::from_u32(0x1E).unwrap();
        let us = char::from_u32(0x1F).unwrap();

        let mut ingested = 0usize;
        for record in raw.split(rs) {
            let mut parts = record.splitn(4, us);
            let folder = parts.next().unwrap_or("").trim();
            let title = parts.next().unwrap_or("").trim();
            let epoch = parts.next().unwrap_or("0").trim().parse::<i64>().unwrap_or(0);
            let body = parts.next().unwrap_or("").trim();

            if epoch != 0 && epoch < cutoff {
                continue;
            }
            // Drop empties; a Note with no title + no body is just a
            // stale shell.
            if title.is_empty() && body.is_empty() {
                continue;
            }

            // Compose a single document. The folder + title give the
            // memory a queryable header; the body is the payload.
            let header = if folder.is_empty() {
                format!("note: {title}")
            } else {
                format!("note ({folder}): {title}")
            };
            let text = if body.is_empty() {
                header
            } else {
                format!("{header}\n\n{body}")
            };

            match pipeline.ingest_fast(&text, "notes-backfill", session) {
                Ok(r) if r.skipped.is_none() => ingested += 1,
                Ok(_) => {}
                Err(e) => {
                    eprintln!("ingest_fast failed for note '{title}': {e}");
                }
            }
        }
        ingested
    }
}

/// CAP-2 — clipboard one-shot snapshot. We don't poll; the daemon's
/// clipboard watcher handles that. This grabs `pbpaste` once at
/// startup so the very first user query has something to land on
/// even before they copy anything new. Same skip rules as the
/// daemon: blank, <10 chars, or short high-entropy strings (looks
/// like a token / API key) are dropped.
///
/// On non-macOS this is a no-op (no portable `pbpaste` equivalent
/// ships by default; xclip / wl-clipboard support deferred).
fn cmd_capture_backfill_clipboard(db_path: &str) -> usize {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = db_path;
        eprintln!("clipboard snapshot is macOS-only — skipping");
        return 0;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        use tm_ingest::IngestPipeline;

        let output = match Command::new("pbpaste").output() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("pbpaste failed: {e}");
                return 0;
            }
        };
        if !output.status.success() {
            eprintln!("pbpaste exited non-zero");
            return 0;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Same skip rules as the daemon's `get_clipboard()`.
        if text.len() < 10 {
            return 0;
        }
        if looks_like_secret(&text) {
            return 0;
        }

        let pipeline = match IngestPipeline::open(db_path, /*hash_embed=*/ false) {
            Ok(p) => p.with_rate_limiter(tm_ingest::RateLimiter::unlimited()),
            Err(e) => {
                eprintln!("open pipeline: {e}");
                return 0;
            }
        };
        let session = Uuid::new_v4();
        let payload = format!("clipboard contents: {text}");
        match pipeline.ingest_fast(&payload, "clipboard-backfill", session) {
            Ok(r) if r.skipped.is_none() => 1,
            Ok(_) => 0,
            Err(e) => {
                eprintln!("ingest_fast failed: {e}");
                0
            }
        }
    }
}

/// Heuristic: short, no-spaces, high alphanum density → likely a
/// token / API key / hash. Mirrors the daemon's filter so the one-
/// shot snapshot doesn't sneak secrets into memory.
#[cfg(target_os = "macos")]
fn looks_like_secret(s: &str) -> bool {
    if s.len() > 200 {
        return false; // long pastes are almost certainly real text
    }
    if s.contains(char::is_whitespace) {
        return false;
    }
    let alnum = s.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    let ratio = alnum as f64 / s.len() as f64;
    ratio > 0.9
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
        ContextAction::Merge { a, b, into, json } => {
            if let Err(e) = cmd_context_merge(db_path, &a, &b, &into, json) {
                eprintln!("merge failed: {e}");
                std::process::exit(1);
            }
        }
        ContextAction::Split {
            name,
            by_entity_type,
            into,
            json,
        } => {
            if let Err(e) = cmd_context_split(db_path, &name, &by_entity_type, &into, json) {
                eprintln!("split failed: {e}");
                std::process::exit(1);
            }
        }
        ContextAction::Snapshot { name, output } => {
            if let Err(e) = cmd_context_snapshot(db_path, &name, output.as_deref()) {
                eprintln!("snapshot failed: {e}");
                std::process::exit(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LM-14 — `tracemind context merge/split/snapshot`
// ---------------------------------------------------------------------------

/// Resolve a context name to a Context row, creating it if absent. Used
/// by `merge --into` and `split --into` so the user doesn't have to
/// pre-create the target.
fn resolve_or_create_context(
    graph: &GraphStore,
    name: &str,
    tags: &str,
) -> Result<tm_graph::context::Context, String> {
    use tm_graph::context::Context;
    if let Some(existing) = graph
        .get_context_by_name(name)
        .map_err(|e| format!("lookup context '{name}': {e}"))?
    {
        return Ok(existing);
    }
    let ctx = Context::new(name.to_string(), tags.to_string());
    graph
        .create_context(&ctx)
        .map_err(|e| format!("create context '{name}': {e}"))?;
    Ok(ctx)
}

fn cmd_context_merge(
    db_path: &str,
    a: &str,
    b: &str,
    into: &str,
    as_json: bool,
) -> Result<(), String> {
    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let ctx_a = graph
        .get_context_by_name(a)
        .map_err(|e| format!("lookup '{a}': {e}"))?
        .ok_or_else(|| format!("no context named '{a}'"))?;
    let ctx_b = graph
        .get_context_by_name(b)
        .map_err(|e| format!("lookup '{b}': {e}"))?
        .ok_or_else(|| format!("no context named '{b}'"))?;
    let ctx_into = resolve_or_create_context(&graph, into, "")?;

    let mut entities_moved = 0usize;
    for src in [&ctx_a, &ctx_b] {
        let rows = graph
            .list_entities_in_context(src.id)
            .map_err(|e| format!("list entities in '{}': {e}", src.name))?;
        for e in rows {
            graph
                .set_entity_context(e.id, Some(ctx_into.id))
                .map_err(|err| format!("move entity {}: {err}", e.id))?;
            entities_moved += 1;
        }
    }
    let mut triples_moved = 0usize;
    for src in [&ctx_a, &ctx_b] {
        let rows = graph
            .list_triples_in_context(src.id)
            .map_err(|e| format!("list triples in '{}': {e}", src.name))?;
        for t in rows {
            graph
                .set_triple_context(t.id, Some(ctx_into.id))
                .map_err(|err| format!("move triple {}: {err}", t.id))?;
            triples_moved += 1;
        }
    }

    if as_json {
        let payload = serde_json::json!({
            "merged_from": [
                { "name": a, "id": ctx_a.id.to_string() },
                { "name": b, "id": ctx_b.id.to_string() }
            ],
            "merged_into": { "name": into, "id": ctx_into.id.to_string() },
            "entities_moved": entities_moved,
            "triples_moved": triples_moved,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {e}"))?
        );
    } else {
        println!(
            "merged '{a}' ({}) + '{b}' ({}) → '{into}' ({})",
            ctx_a.id, ctx_b.id, ctx_into.id
        );
        println!("  {entities_moved} entit{}, {triples_moved} triple{}",
            if entities_moved == 1 { "y" } else { "ies" },
            if triples_moved == 1 { "" } else { "s" });
    }
    Ok(())
}

fn cmd_context_split(
    db_path: &str,
    name: &str,
    by_entity_type: &str,
    into: &str,
    as_json: bool,
) -> Result<(), String> {
    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let ctx_src = graph
        .get_context_by_name(name)
        .map_err(|e| format!("lookup '{name}': {e}"))?
        .ok_or_else(|| format!("no context named '{name}'"))?;
    let ctx_into = resolve_or_create_context(&graph, into, "")?;
    if ctx_into.id == ctx_src.id {
        return Err(format!("--into '{into}' must differ from source '{name}'"));
    }

    let needle = by_entity_type.trim().to_ascii_lowercase();

    let rows = graph
        .list_entities_in_context(ctx_src.id)
        .map_err(|e| format!("list entities: {e}"))?;
    let mut moved_ids: Vec<Uuid> = Vec::new();
    for e in &rows {
        if e.entity_type.to_string().to_ascii_lowercase() == needle {
            graph
                .set_entity_context(e.id, Some(ctx_into.id))
                .map_err(|err| format!("move entity {}: {err}", e.id))?;
            moved_ids.push(e.id);
        }
    }

    // Move every triple whose subject moved with the entity; this keeps
    // each entity's outgoing edges co-located with the entity itself.
    let mut triples_moved = 0usize;
    let src_triples = graph
        .list_triples_in_context(ctx_src.id)
        .map_err(|e| format!("list triples: {e}"))?;
    let moved_set: std::collections::HashSet<Uuid> = moved_ids.iter().copied().collect();
    for t in &src_triples {
        if moved_set.contains(&t.subject_id) {
            graph
                .set_triple_context(t.id, Some(ctx_into.id))
                .map_err(|err| format!("move triple {}: {err}", t.id))?;
            triples_moved += 1;
        }
    }

    if as_json {
        let payload = serde_json::json!({
            "split_from": { "name": name, "id": ctx_src.id.to_string() },
            "split_into": { "name": into, "id": ctx_into.id.to_string() },
            "by_entity_type": by_entity_type,
            "entities_moved": moved_ids.len(),
            "triples_moved": triples_moved,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {e}"))?
        );
    } else {
        println!(
            "split '{name}' ({}) by entity_type='{by_entity_type}' → '{into}' ({})",
            ctx_src.id, ctx_into.id
        );
        println!(
            "  {} entit{} moved, {} triple{} moved",
            moved_ids.len(),
            if moved_ids.len() == 1 { "y" } else { "ies" },
            triples_moved,
            if triples_moved == 1 { "" } else { "s" },
        );
    }
    Ok(())
}

fn cmd_context_snapshot(
    db_path: &str,
    name: &str,
    output: Option<&str>,
) -> Result<(), String> {
    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;
    let ctx = graph
        .get_context_by_name(name)
        .map_err(|e| format!("lookup '{name}': {e}"))?
        .ok_or_else(|| format!("no context named '{name}'"))?;
    let snapshot = graph
        .snapshot_context(&ctx)
        .map_err(|e| format!("snapshot '{name}': {e}"))?;

    let default_path = format!("{name}.tmctx");
    let out_path = output.unwrap_or(&default_path);
    let pretty = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| format!("serialize snapshot: {e}"))?;
    fs::write(out_path, pretty).map_err(|e| format!("write {out_path}: {e}"))?;

    let n_ent = snapshot["counts"]["entities"].as_u64().unwrap_or(0);
    let n_tri = snapshot["counts"]["triples"].as_u64().unwrap_or(0);
    println!(
        "wrote snapshot: {out_path}  ({n_ent} entities, {n_tri} triples, context {})",
        ctx.id
    );
    Ok(())
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

/// `tracemind storage` — scan footprint + on-demand cleanup.
///
/// Surfaces `tm_graph::maintenance` to humans. No network, no model
/// downloads, no graph rewrites — every action is bounded and listed in
/// the help text.
fn cmd_storage(dir: &PathBuf, action: StorageAction) {
    match action {
        StorageAction::Status { json } => match tm_graph::storage_stats(dir) {
            Ok(stats) => {
                if json {
                    match serde_json::to_string_pretty(&stats) {
                        Ok(s) => println!("{}", s),
                        Err(e) => {
                            eprintln!("serialize failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    println!("Storage @ {}", stats.data_dir);
                    println!("  Total on disk:       {}", fmt_bytes(stats.total_bytes));
                    println!("  Entities:            {}", stats.entity_count);
                    println!("  Triples:             {}", stats.triple_count);
                    println!(
                        "  Signals (total/ephem/consol): {}/{}/{}",
                        stats.signal_count,
                        stats.ephemeral_signal_count,
                        stats.consolidated_signal_count,
                    );
                    println!("  Trace lines:         {}", stats.trace_line_count);
                    if !stats.files.is_empty() {
                        println!("  Files:");
                        for f in &stats.files {
                            println!("    {:<32} {}", f.name, fmt_bytes(f.bytes));
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("storage status failed: {}", e);
                std::process::exit(1);
            }
        },
        StorageAction::Vacuum => match tm_graph::vacuum_all(dir) {
            Ok(r) => print_cleanup(&r),
            Err(e) => {
                eprintln!("vacuum failed: {}", e);
                std::process::exit(1);
            }
        },
        StorageAction::CleanEphemeral => match tm_graph::clean_ephemeral(dir) {
            Ok(r) => print_cleanup(&r),
            Err(e) => {
                eprintln!("clean-ephemeral failed: {}", e);
                std::process::exit(1);
            }
        },
        StorageAction::TruncateTraces { keep } => match tm_graph::truncate_traces(dir, keep) {
            Ok(r) => print_cleanup(&r),
            Err(e) => {
                eprintln!("truncate-traces failed: {}", e);
                std::process::exit(1);
            }
        },
    }
}

fn print_cleanup(r: &tm_graph::CleanupReport) {
    let delta = if r.bytes_freed >= 0 {
        format!("freed {}", fmt_bytes(r.bytes_freed as u64))
    } else {
        format!("grew {}", fmt_bytes((-r.bytes_freed) as u64))
    };
    println!(
        "{}: {} → {} ({}){}",
        r.action,
        fmt_bytes(r.bytes_before),
        fmt_bytes(r.bytes_after),
        delta,
        if r.rows_deleted > 0 {
            format!(" · removed {} rows", r.rows_deleted)
        } else {
            String::new()
        },
    );
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.2} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.2} KB", n as f64 / KB as f64)
    } else {
        format!("{} B", n)
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
    cross_context: bool,
    view_name: Option<&str>,
    include_entity: &[String],
    exclude_entity: &[String],
    data_dir: &PathBuf,
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
    engine.set_cross_context(cross_context);
    if let Some(filter) = resolve_view_filter(
        data_dir,
        db_path,
        view_name,
        include_entity,
        exclude_entity,
    ) {
        engine.set_view_filter(Some(filter));
    }
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

// ---------------------------------------------------------------------------
// LM-16 — `tracemind export` markdown bundle.
//
// Emits an Obsidian-compatible markdown vault:
//
//   <output>/
//     index.md            — overview, counts, time range, context filter
//     entities/<slug>.md  — one file per entity with YAML frontmatter, a
//                           Relations section using [[wikilinks]], and a
//                           Backlinks section.
//
// The format is deliberately plain so users can open the bundle in
// Obsidian / Logseq / a plain editor without any TraceMind binary. This
// is the "your memory is yours" trust artifact promised in PROJECT_2026
// §1c (Legibility), and the data path for Karpathy-style PKM workflows.
// ---------------------------------------------------------------------------

/// Convert an entity name into a filename-safe slug. Strategy: lowercase,
/// keep `[a-z0-9]`, replace every other run of characters with a single
/// `-`, trim leading/trailing dashes. Empty / all-symbol names fall back
/// to `entity` so we never emit a zero-length filename.
fn export_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = true; // suppress leading dashes
    for c in name.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "entity".to_string()
    } else {
        out
    }
}

/// Escape a YAML scalar so it's safe to drop into our frontmatter. We
/// quote with single quotes and double any embedded single quotes
/// (standard YAML 1.2 single-quoted-scalar rules). Newlines collapse to
/// spaces — entity names with newlines are pathological enough that
/// the round-trip is best-effort.
fn export_yaml_scalar(s: &str) -> String {
    let escaped: String = s.replace('\'', "''").replace('\n', " ");
    format!("'{}'", escaped)
}

/// LM-19: scrub PII from a free-form string before it lands in the
/// export bundle. We mirror the patterns that `tm-governance` *detects*
/// (email, US phone, SSN, 16-digit credit card) and replace each match
/// with a stable placeholder. This is best-effort: governance already
/// prevents PII from entering the graph in the first place, but we
/// re-scrub here because:
///   1. Older databases predate the governance gate.
///   2. Entity *names* (e.g. "Alice <alice@acme.com>") can carry an
///      email that the gate may have admitted as a contact handle.
///   3. Users sharing a bundle deserve a "no surprises" guarantee.
///
/// When `redact` is false this is a zero-cost passthrough.
fn export_redact(s: &str, redact: bool) -> String {
    if !redact || s.is_empty() {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());

    // Walk char-by-char. At each position we try (in order):
    //   - email starting at this position
    //   - US phone starting at this position
    //   - SSN (DDD-DD-DDDD)
    //   - 16-digit credit card (digits + spaces/dashes allowed)
    // First match wins; otherwise emit the char and advance by 1.
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(end) = match_email(s, i) {
            out.push_str("[REDACTED-EMAIL]");
            i = end;
            continue;
        }
        if let Some(end) = match_ssn(s, i) {
            out.push_str("[REDACTED-SSN]");
            i = end;
            continue;
        }
        if let Some(end) = match_credit_card(s, i) {
            out.push_str("[REDACTED-CC]");
            i = end;
            continue;
        }
        if let Some(end) = match_phone(s, i) {
            out.push_str("[REDACTED-PHONE]");
            i = end;
            continue;
        }
        // Push one UTF-8 char.
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Try to match an email starting at byte offset `pos`. Returns the
/// end-byte-index on success.
fn match_email(s: &str, pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    // Local part: at least one of [A-Za-z0-9._%+-].
    let mut end = pos;
    while end < bytes.len() {
        let c = bytes[end];
        let ok = c.is_ascii_alphanumeric()
            || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-');
        if !ok { break; }
        end += 1;
    }
    if end == pos { return None; }
    if end >= bytes.len() || bytes[end] != b'@' { return None; }
    let at = end;
    end += 1; // past '@'
    // Domain: [A-Za-z0-9.-]+
    let dom_start = end;
    while end < bytes.len() {
        let c = bytes[end];
        if c.is_ascii_alphanumeric() || c == b'.' || c == b'-' { end += 1; } else { break; }
    }
    let domain = &s[dom_start..end];
    if let Some(dot) = domain.rfind('.') {
        if dot > 0 && domain.len() - dot - 1 >= 2 {
            // Need at least one char before '@'.
            if at > pos {
                return Some(end);
            }
        }
    }
    None
}

/// SSN: DDD-DD-DDDD with no adjacent digits.
fn match_ssn(s: &str, pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if pos + 11 > bytes.len() { return None; }
    let slice = &bytes[pos..pos + 11];
    let pat = [
        slice[0].is_ascii_digit(), slice[1].is_ascii_digit(), slice[2].is_ascii_digit(),
        slice[3] == b'-',
        slice[4].is_ascii_digit(), slice[5].is_ascii_digit(),
        slice[6] == b'-',
        slice[7].is_ascii_digit(), slice[8].is_ascii_digit(),
        slice[9].is_ascii_digit(), slice[10].is_ascii_digit(),
    ];
    if pat.iter().all(|b| *b) {
        // Word boundary check on either side.
        let before_ok = pos == 0 || !bytes[pos - 1].is_ascii_digit();
        let after_ok = pos + 11 == bytes.len() || !bytes[pos + 11].is_ascii_digit();
        if before_ok && after_ok {
            return Some(pos + 11);
        }
    }
    None
}

/// Credit card: 16 consecutive digits allowing spaces/dashes as
/// separators. Returns the end position of the match (including
/// separators).
fn match_credit_card(s: &str, pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if pos >= bytes.len() || !bytes[pos].is_ascii_digit() {
        return None;
    }
    let mut digits = 0usize;
    let mut end = pos;
    while end < bytes.len() && digits < 16 {
        let c = bytes[end];
        if c.is_ascii_digit() {
            digits += 1;
            end += 1;
        } else if c == b' ' || c == b'-' {
            end += 1;
        } else {
            break;
        }
    }
    if digits == 16 {
        // Word boundary: next char (if any) must not be a digit.
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        if after_ok {
            return Some(end);
        }
    }
    None
}

/// US phone: optional `+1`, then 10 digits with `-`, `.`, space, or
/// `()` separators tolerated.
fn match_phone(s: &str, pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut end = pos;
    // Optional +1 prefix.
    if end + 1 < bytes.len() && bytes[end] == b'+' && bytes[end + 1] == b'1' {
        end += 2;
        while end < bytes.len() && matches!(bytes[end], b' ' | b'-' | b'.') {
            end += 1;
        }
    }
    let mut digits = 0usize;
    let mut k = end;
    while k < bytes.len() && digits < 10 {
        let c = bytes[k];
        if c.is_ascii_digit() {
            digits += 1;
            k += 1;
        } else if matches!(c, b' ' | b'-' | b'.' | b'(' | b')') {
            k += 1;
        } else {
            break;
        }
    }
    if digits == 10 {
        let after_ok = k == bytes.len() || !bytes[k].is_ascii_digit();
        if after_ok {
            return Some(k);
        }
    }
    None
}

/// Resolve a `--context` argument (name *or* UUID string) to a stored
/// Context. Returns `Ok(None)` if the argument is `None` and the caller
/// did not pass a filter. Returns `Err` if the argument was provided but
/// no matching context exists — we want a hard error, not a silent
/// "exported everything" surprise.
fn export_resolve_context(
    graph: &GraphStore,
    arg: Option<&str>,
) -> Result<Option<tm_graph::context::Context>, String> {
    let Some(arg) = arg else { return Ok(None) };
    // Try UUID first — cheaper than scanning the table.
    if let Ok(uuid) = Uuid::parse_str(arg) {
        let all = graph
            .list_contexts()
            .map_err(|e| format!("list contexts: {e}"))?;
        if let Some(c) = all.into_iter().find(|c| c.id == uuid) {
            return Ok(Some(c));
        }
        return Err(format!("no context with id {uuid}"));
    }
    match graph
        .get_context_by_name(arg)
        .map_err(|e| format!("lookup context '{arg}': {e}"))?
    {
        Some(c) => Ok(Some(c)),
        None => Err(format!("no context named '{arg}'")),
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_export(
    db_path: &str,
    context_arg: Option<&str>,
    format: &str,
    output: &std::path::Path,
    view: Option<&str>,
    entity_arg: Option<&str>,
    redact: bool,
    limit: Option<usize>,
) -> Result<(), String> {
    if format != "markdown" {
        return Err(format!(
            "unsupported --format '{format}' (only 'markdown' is implemented today)"
        ));
    }

    let graph = GraphStore::open(db_path).map_err(|e| format!("open graph: {e}"))?;

    // LM-11f Memory Views — resolve a named view to its ViewFilter so
    // include/exclude entity & context lists apply during export. The
    // bundle is the durable artefact users hand off, so the filter has
    // to be honoured here just like at query time.
    let view_filter: Option<tm_graph::ViewFilter> = match view {
        None => None,
        Some(name) => {
            let v = graph
                .get_view_by_name(name)
                .map_err(|e| format!("lookup view '{name}': {e}"))?
                .ok_or_else(|| format!("no view named '{name}'"))?;
            let f = graph
                .load_view_filter(v.id)
                .map_err(|e| format!("load view '{name}': {e}"))?;
            Some(f)
        }
    };

    let ctx_filter = export_resolve_context(&graph, context_arg)?;

    let mut entities = graph
        .list_all_entities()
        .map_err(|e| format!("list entities: {e}"))?;

    // Apply context filter.
    if let Some(ref ctx) = ctx_filter {
        entities.retain(|e| {
            graph
                .entity_context_id(e.id)
                .ok()
                .flatten()
                .map(|c| c == ctx.id)
                .unwrap_or(false)
        });
    }

    // LM-11f apply view filter on entities.
    if let Some(ref vf) = view_filter {
        entities.retain(|e| {
            let ctx = graph.entity_context_id(e.id).ok().flatten();
            !vf.rejects_entity(e.id, ctx)
        });
    }

    // LM-18 --entity: keep only the named entity + its 1-hop neighbours.
    // Accepts either a UUID or an exact name (case-insensitive). We
    // expand the neighbour set *before* the limit so the slice always
    // includes the focal entity even if it has many hops.
    if let Some(arg) = entity_arg {
        let focal_id: Uuid = if let Ok(id) = Uuid::parse_str(arg) {
            // Make sure the UUID actually exists.
            graph
                .get_entity(id)
                .map_err(|_| format!("no entity with id '{arg}'"))?;
            id
        } else {
            let ent = graph
                .find_entity_by_name_icase(arg)
                .map_err(|e| format!("lookup entity '{arg}': {e}"))?
                .ok_or_else(|| format!("no entity named '{arg}'"))?;
            ent.id
        };

        let triples = graph
            .get_triples_for_entity(focal_id)
            .map_err(|e| format!("neighbours for {focal_id}: {e}"))?;
        let mut keep: std::collections::HashSet<Uuid> =
            std::collections::HashSet::new();
        keep.insert(focal_id);
        for t in &triples {
            keep.insert(t.subject_id);
            keep.insert(t.object_id);
        }
        entities.retain(|e| keep.contains(&e.id));
        if entities.is_empty() {
            return Err(format!(
                "--entity '{arg}' resolved to {focal_id} but no entities remain after other filters"
            ));
        }
    }

    // Sort by updated_at desc so `--limit` keeps the most recent slice.
    entities.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    if let Some(n) = limit {
        entities.truncate(n);
    }

    // Build a slug map up-front, disambiguating collisions with a
    // short-id suffix. We need stable slugs *before* writing any file so
    // that `[[wikilinks]]` between entities resolve correctly.
    //
    // LM-19: when `--redact` is set we slug from the *scrubbed* display
    // name so an email like "alice@acme.com" doesn't leak through the
    // filename. Disambiguation by UUID suffix is what keeps slugs unique
    // when many names redact to the same placeholder.
    let mut slug_for: std::collections::HashMap<Uuid, String> =
        std::collections::HashMap::with_capacity(entities.len());
    let mut display_name: std::collections::HashMap<Uuid, String> =
        std::collections::HashMap::with_capacity(entities.len());
    let mut used: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(entities.len());
    for e in &entities {
        let shown = export_redact(&e.name, redact);
        let base = export_slug(&shown);
        let mut slug = base.clone();
        if used.contains(&slug) {
            // Disambiguate with the first 8 chars of the UUID — enough
            // to be unique in any realistic graph.
            let short = e.id.simple().to_string();
            slug = format!("{}-{}", base, &short[..8]);
        }
        used.insert(slug.clone());
        slug_for.insert(e.id, slug);
        display_name.insert(e.id, shown);
    }

    // Compute backlinks: for each entity, the set of (other_entity_id,
    // predicate) pairs whose `object_id` is this entity. We only walk
    // *typed* (non-RelatedTo) triples because co-occurrence noise makes
    // backlinks unreadable otherwise.
    let mut backlinks: std::collections::HashMap<Uuid, Vec<(Uuid, tm_types::Predicate)>> =
        std::collections::HashMap::new();

    // Prepare output dir.
    fs::create_dir_all(output).map_err(|e| format!("mkdir {}: {e}", output.display()))?;
    let entities_dir = output.join("entities");
    fs::create_dir_all(&entities_dir)
        .map_err(|e| format!("mkdir {}: {e}", entities_dir.display()))?;

    // First pass: build backlink map. Triples that the view filter
    // rejects must not appear as backlinks either, otherwise the
    // bundle would expose the excluded edge indirectly.
    for e in &entities {
        let triples = graph
            .get_triples_for_entity(e.id)
            .map_err(|err| format!("triples for {}: {err}", e.id))?;
        for t in &triples {
            // We only care about typed outgoing edges where subject == e.id.
            if t.subject_id != e.id {
                continue;
            }
            if matches!(t.predicate, tm_types::Predicate::RelatedTo) {
                continue;
            }
            // Skip self-loops in the backlinks panel.
            if t.object_id == e.id {
                continue;
            }
            if let Some(ref vf) = view_filter {
                if vf.rejects_triple(t.id, t.confidence) {
                    continue;
                }
            }
            backlinks
                .entry(t.object_id)
                .or_default()
                .push((e.id, t.predicate.clone()));
        }
    }

    // Second pass: write per-entity markdown.
    let mut written = 0usize;
    let mut earliest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;
    for e in &entities {
        earliest = Some(earliest.map_or(e.created_at, |x| x.min(e.created_at)));
        latest = Some(latest.map_or(e.updated_at, |x| x.max(e.updated_at)));

        let slug = slug_for.get(&e.id).cloned().unwrap_or_else(|| "entity".into());
        let file_path = entities_dir.join(format!("{slug}.md"));

        let shown_name = display_name
            .get(&e.id)
            .cloned()
            .unwrap_or_else(|| export_redact(&e.name, redact));

        let mut body = String::new();
        body.push_str("---\n");
        body.push_str(&format!("id: '{}'\n", e.id));
        body.push_str(&format!("name: {}\n", export_yaml_scalar(&shown_name)));
        body.push_str(&format!("type: {}\n", export_yaml_scalar(&e.entity_type.to_string())));
        body.push_str(&format!("confidence: {:.4}\n", e.confidence));
        body.push_str(&format!("created_at: '{}'\n", e.created_at.to_rfc3339()));
        body.push_str(&format!("updated_at: '{}'\n", e.updated_at.to_rfc3339()));
        if let Some(src) = &e.source_id {
            body.push_str(&format!(
                "source_id: {}\n",
                export_yaml_scalar(&export_redact(src, redact))
            ));
        }
        if let Ok(Some(ctx_id)) = graph.entity_context_id(e.id) {
            body.push_str(&format!("context_id: '{}'\n", ctx_id));
        }
        body.push_str(&format!(
            "tags: ['tracemind', 'entity/{}']\n",
            e.entity_type.to_string().to_lowercase()
        ));
        if redact {
            body.push_str("redacted: true\n");
        }
        body.push_str("---\n\n");

        body.push_str(&format!("# {}\n\n", shown_name));
        body.push_str(&format!(
            "Type: **{}**  ·  Confidence: **{:.2}**\n\n",
            e.entity_type, e.confidence
        ));

        // Outgoing typed relations.
        let triples = graph
            .get_triples_for_entity(e.id)
            .map_err(|err| format!("triples for {}: {err}", e.id))?;
        let mut outgoing: Vec<&tm_types::Triple> = triples
            .iter()
            .filter(|t| {
                if t.subject_id != e.id {
                    return false;
                }
                if matches!(t.predicate, tm_types::Predicate::RelatedTo) {
                    return false;
                }
                if let Some(ref vf) = view_filter {
                    if vf.rejects_triple(t.id, t.confidence) {
                        return false;
                    }
                }
                true
            })
            .collect();
        outgoing.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if !outgoing.is_empty() {
            body.push_str("## Relations\n\n");
            for t in &outgoing {
                let target_slug = slug_for.get(&t.object_id).cloned();
                let target_display = match target_slug {
                    Some(slug) => {
                        let target_human = display_name
                            .get(&t.object_id)
                            .cloned()
                            .unwrap_or_else(|| {
                                graph
                                    .get_entity(t.object_id)
                                    .map(|x| export_redact(&x.name, redact))
                                    .unwrap_or_else(|_| slug.clone())
                            });
                        format!("[[{slug}|{target_human}]]")
                    }
                    None => {
                        // Target entity wasn't in our exported set (e.g.
                        // filtered out by context). Fall back to fetching
                        // its name; render as plain text rather than a
                        // dangling wikilink.
                        match graph.get_entity(t.object_id) {
                            Ok(ent) => format!(
                                "`{}` _(not exported)_",
                                export_redact(&ent.name, redact)
                            ),
                            Err(_) => format!("`<unknown {}>`", t.object_id),
                        }
                    }
                };
                body.push_str(&format!(
                    "- **{}** → {} _(conf {:.2})_\n",
                    t.predicate, target_display, t.confidence
                ));
            }
            body.push('\n');
        }

        // Backlinks.
        if let Some(incoming) = backlinks.get(&e.id) {
            let mut incoming = incoming.clone();
            incoming.sort_by(|(a_id, _), (b_id, _)| a_id.cmp(b_id));
            body.push_str("## Backlinks\n\n");
            for (src_id, pred) in &incoming {
                let src_slug = slug_for
                    .get(src_id)
                    .cloned()
                    .unwrap_or_else(|| src_id.to_string());
                let src_human = display_name
                    .get(src_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        graph
                            .get_entity(*src_id)
                            .map(|x| export_redact(&x.name, redact))
                            .unwrap_or_else(|_| src_slug.clone())
                    });
                body.push_str(&format!("- [[{src_slug}|{src_human}]] — **{pred}**\n"));
            }
            body.push('\n');
        }

        fs::write(&file_path, body)
            .map_err(|err| format!("write {}: {err}", file_path.display()))?;
        written += 1;
    }

    // index.md
    let mut index = String::new();
    index.push_str("---\n");
    index.push_str("title: 'TraceMind export'\n");
    index.push_str(&format!("exported_at: '{}'\n", chrono::Utc::now().to_rfc3339()));
    if let Some(ref c) = ctx_filter {
        index.push_str(&format!("context: {}\n", export_yaml_scalar(&c.name)));
        index.push_str(&format!("context_id: '{}'\n", c.id));
    }
    if let Some(v) = view {
        index.push_str(&format!("view: {}\n", export_yaml_scalar(v)));
    }
    if let Some(arg) = entity_arg {
        index.push_str(&format!("entity_filter: {}\n", export_yaml_scalar(arg)));
    }
    if redact {
        index.push_str("redacted: true\n");
    }
    index.push_str(&format!("entity_count: {}\n", written));
    index.push_str("---\n\n");
    index.push_str("# TraceMind export\n\n");
    match &ctx_filter {
        Some(c) => index.push_str(&format!("Filtered to context **{}**.\n\n", c.name)),
        None => index.push_str("All contexts included.\n\n"),
    }
    if let Some(v) = view {
        index.push_str(&format!("View: **{v}**.\n\n"));
    }
    if let Some(arg) = entity_arg {
        index.push_str(&format!("Entity filter: **{arg}** (+ 1-hop neighbours).\n\n"));
    }
    if redact {
        index.push_str("PII redacted: emails, phone numbers, SSNs, credit-card numbers replaced with placeholders.\n\n");
    }
    index.push_str(&format!("- Entities: **{}**\n", written));
    if let (Some(e), Some(l)) = (earliest, latest) {
        index.push_str(&format!("- Earliest: {}\n", e.to_rfc3339()));
        index.push_str(&format!("- Latest:   {}\n", l.to_rfc3339()));
    }
    index.push_str("\n## Entities\n\n");
    // Stable alpha-sort by the displayed (possibly redacted) name so
    // diffs between exports stay small *and* don't leak ordering by
    // PII.
    let mut sorted: Vec<&tm_types::Entity> = entities.iter().collect();
    sorted.sort_by(|a, b| {
        let an = display_name.get(&a.id).map(|s| s.as_str()).unwrap_or(&a.name);
        let bn = display_name.get(&b.id).map(|s| s.as_str()).unwrap_or(&b.name);
        an.to_lowercase().cmp(&bn.to_lowercase())
    });
    for e in &sorted {
        let slug = slug_for.get(&e.id).cloned().unwrap_or_default();
        let shown = display_name
            .get(&e.id)
            .cloned()
            .unwrap_or_else(|| export_redact(&e.name, redact));
        index.push_str(&format!("- [[{slug}|{shown}]] — {}\n", e.entity_type));
    }
    let index_path = output.join("index.md");
    fs::write(&index_path, index)
        .map_err(|err| format!("write {}: {err}", index_path.display()))?;

    println!("Exported {written} entit{} to {}", if written == 1 { "y" } else { "ies" }, output.display());
    if let Some(c) = ctx_filter {
        println!("  Context filter: {} ({})", c.name, c.id);
    } else {
        println!("  Context filter: (none — all contexts)");
    }
    println!("  Index:          {}", index_path.display());
    println!("  Entities dir:   {}", entities_dir.display());
    Ok(())
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
    use super::{cmd_export, export_redact, export_slug, export_yaml_scalar, strip_md_frontmatter};
    use tm_graph::GraphStore;
    use tm_types::{Entity, EntityType, Predicate, Triple};

    #[test]
    fn slug_basic_alphanumeric() {
        assert_eq!(export_slug("TraceMind"), "tracemind");
        assert_eq!(export_slug("Hello World"), "hello-world");
    }

    #[test]
    fn slug_collapses_separators() {
        assert_eq!(export_slug("foo / bar / baz"), "foo-bar-baz");
        assert_eq!(export_slug("--leading--and--trailing--"), "leading-and-trailing");
    }

    #[test]
    fn slug_empty_falls_back() {
        assert_eq!(export_slug(""), "entity");
        assert_eq!(export_slug("///"), "entity");
    }

    #[test]
    fn yaml_scalar_quotes_and_escapes() {
        assert_eq!(export_yaml_scalar("plain"), "'plain'");
        assert_eq!(export_yaml_scalar("it's mine"), "'it''s mine'");
        // newlines collapsed to spaces — best-effort round-trip
        assert_eq!(export_yaml_scalar("a\nb"), "'a b'");
    }

    #[test]
    fn export_writes_index_and_entity_files_with_wikilinks() {
        // Build a throwaway store with two typed-linked entities, then
        // run the export and inspect the bundle.
        let tmp = tempdir_for_test("tm-cli-export-test");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let graph = GraphStore::open(&db).expect("open graph");
            let alice = Entity::new("Alice", EntityType::Person, 0.95);
            let acme = Entity::new("Acme Corp", EntityType::Organization, 0.9);
            graph.upsert_entity(&alice).expect("upsert alice");
            graph.upsert_entity(&acme).expect("upsert acme");
            let t = Triple::new(alice.id, Predicate::WorksAt, acme.id, 0.88);
            graph.upsert_triple(&t).expect("upsert triple");
        }

        let out_dir = tmp.join("bundle");
        cmd_export(&db, None, "markdown", &out_dir, None, None, false, None)
            .expect("export ok");

        let index = std::fs::read_to_string(out_dir.join("index.md")).expect("index.md");
        assert!(index.contains("entity_count: 2"), "index frontmatter has count: {index}");
        assert!(index.contains("[[alice|Alice]]"), "alice wikilink missing: {index}");
        assert!(index.contains("[[acme-corp|Acme Corp]]"), "acme wikilink missing: {index}");

        let alice_md = std::fs::read_to_string(out_dir.join("entities/alice.md"))
            .expect("alice.md");
        assert!(alice_md.starts_with("---\n"), "frontmatter open");
        assert!(alice_md.contains("type: 'Person'"));
        assert!(alice_md.contains("## Relations"), "relations section");
        assert!(alice_md.contains("**WorksAt** → [[acme-corp|Acme Corp]]"), "wikilink in body: {alice_md}");

        let acme_md = std::fs::read_to_string(out_dir.join("entities/acme-corp.md"))
            .expect("acme-corp.md");
        assert!(acme_md.contains("## Backlinks"), "backlinks section");
        assert!(acme_md.contains("[[alice|Alice]]"), "backlink wikilink: {acme_md}");
    }

    #[test]
    fn export_rejects_unsupported_format() {
        let tmp = tempdir_for_test("tm-cli-export-fmt");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        // Touch the DB so open() doesn't error before format check.
        {
            let _ = GraphStore::open(&db).expect("open graph");
        }
        let out_dir = tmp.join("bundle");
        let err = cmd_export(&db, None, "json", &out_dir, None, None, false, None)
            .expect_err("must reject json");
        assert!(err.contains("unsupported --format"), "got: {err}");
    }

    #[test]
    fn export_rejects_unknown_context() {
        let tmp = tempdir_for_test("tm-cli-export-ctx");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let _ = GraphStore::open(&db).expect("open graph");
        }
        let out_dir = tmp.join("bundle");
        let err = cmd_export(
            &db,
            Some("no-such-context"),
            "markdown",
            &out_dir,
            None,
            None,
            false,
            None,
        )
        .expect_err("must reject unknown context");
        assert!(err.contains("no context named"), "got: {err}");
    }

    #[test]
    fn export_view_unknown_rejected() {
        let tmp = tempdir_for_test("tm-cli-export-view");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let _ = GraphStore::open(&db).expect("open graph");
        }
        let out_dir = tmp.join("bundle");
        let err = cmd_export(
            &db,
            None,
            "markdown",
            &out_dir,
            Some("focus-1-2-3"),
            None,
            false,
            None,
        )
        .expect_err("must reject unknown view");
        assert!(err.contains("no view named"), "got: {err}");
    }

    /// LM-11f: when a Memory View excludes an entity, the export
    /// bundle must not contain that entity's file or any backlink
    /// referencing it.
    #[test]
    fn export_with_view_excludes_entities() {
        use tm_graph::memory_view::{MemberKind, MemberType, MemoryView};
        let tmp = tempdir_for_test("tm-cli-export-view-filter");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        let (alice_id, _acme_id) = {
            let graph = GraphStore::open(&db).expect("open graph");
            let alice = Entity::new("Alice", EntityType::Person, 0.95);
            let acme = Entity::new("Acme Corp", EntityType::Organization, 0.9);
            graph.upsert_entity(&alice).expect("alice");
            graph.upsert_entity(&acme).expect("acme");
            let t = Triple::new(alice.id, Predicate::WorksAt, acme.id, 0.88);
            graph.upsert_triple(&t).expect("triple");
            // Create a view that excludes Alice.
            let view = MemoryView::new("acme-only", "exclude alice");
            graph.create_view(&view).expect("create view");
            graph
                .add_view_member(
                    view.id,
                    MemberKind::Exclude,
                    MemberType::Entity,
                    alice.id,
                )
                .expect("add member");
            (alice.id, acme.id)
        };

        let out_dir = tmp.join("bundle");
        cmd_export(
            &db,
            None,
            "markdown",
            &out_dir,
            Some("acme-only"),
            None,
            false,
            None,
        )
        .expect("export ok");

        let index = std::fs::read_to_string(out_dir.join("index.md")).expect("index");
        assert!(index.contains("view: 'acme-only'"), "view tag in frontmatter: {index}");
        assert!(index.contains("entity_count: 1"), "count is 1: {index}");
        assert!(!out_dir.join("entities/alice.md").exists(), "alice.md must not exist");
        assert!(out_dir.join("entities/acme-corp.md").exists(), "acme-corp.md exists");
        let acme_md = std::fs::read_to_string(out_dir.join("entities/acme-corp.md"))
            .expect("acme-corp.md");
        assert!(
            !acme_md.contains("Alice"),
            "no Alice backlink should be present: {acme_md}"
        );
        // alice_id is unused beyond ensuring the entity exists; reference
        // it so the binding doesn't trigger an unused-variable warning.
        let _ = alice_id;
    }

    /// LM-18: `--entity` keeps only the named entity and its 1-hop
    /// neighbours. Other entities are dropped from the bundle.
    #[test]
    fn export_entity_keeps_only_1hop_neighbours() {
        let tmp = tempdir_for_test("tm-cli-export-entity");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let graph = GraphStore::open(&db).expect("open graph");
            let alice = Entity::new("Alice", EntityType::Person, 0.95);
            let acme = Entity::new("Acme Corp", EntityType::Organization, 0.9);
            let bob = Entity::new("Bob", EntityType::Person, 0.9);
            graph.upsert_entity(&alice).expect("alice");
            graph.upsert_entity(&acme).expect("acme");
            graph.upsert_entity(&bob).expect("bob");
            let t = Triple::new(alice.id, Predicate::WorksAt, acme.id, 0.88);
            graph.upsert_triple(&t).expect("triple");
            // Bob has no edges → must be dropped under --entity Alice.
        }

        let out_dir = tmp.join("bundle");
        cmd_export(
            &db,
            None,
            "markdown",
            &out_dir,
            None,
            Some("Alice"),
            false,
            None,
        )
        .expect("export ok");

        let index = std::fs::read_to_string(out_dir.join("index.md")).expect("index");
        assert!(index.contains("entity_filter: 'Alice'"), "filter tag in fm: {index}");
        assert!(index.contains("entity_count: 2"), "count is 2 (alice + acme): {index}");
        assert!(out_dir.join("entities/alice.md").exists(), "alice.md kept");
        assert!(out_dir.join("entities/acme-corp.md").exists(), "acme-corp.md kept");
        assert!(!out_dir.join("entities/bob.md").exists(), "bob.md dropped");
    }

    /// LM-18: unknown `--entity` argument is a hard error.
    #[test]
    fn export_entity_unknown_rejected() {
        let tmp = tempdir_for_test("tm-cli-export-entity-unknown");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let _ = GraphStore::open(&db).expect("open graph");
        }
        let out_dir = tmp.join("bundle");
        let err = cmd_export(
            &db,
            None,
            "markdown",
            &out_dir,
            None,
            Some("Nobody"),
            false,
            None,
        )
        .expect_err("must reject unknown entity");
        assert!(err.contains("no entity"), "got: {err}");
    }

    /// LM-19: redact pure pattern coverage.
    #[test]
    fn redact_scrubs_pii_patterns() {
        assert_eq!(
            export_redact("contact alice@acme.com please", true),
            "contact [REDACTED-EMAIL] please"
        );
        assert_eq!(
            export_redact("ssn 123-45-6789 is private", true),
            "ssn [REDACTED-SSN] is private"
        );
        assert_eq!(
            export_redact("call +1 415-555-0123 now", true),
            "call [REDACTED-PHONE] now"
        );
        assert_eq!(
            export_redact("card 4111 1111 1111 1111 expired", true),
            "card [REDACTED-CC] expired"
        );
        // Passthrough.
        assert_eq!(export_redact("nothing to scrub here", true), "nothing to scrub here");
        // redact=false is a no-op.
        assert_eq!(
            export_redact("alice@acme.com", false),
            "alice@acme.com"
        );
    }

    /// LM-19: when `--redact` is set the bundle must not leak the
    /// original email out of an entity name.
    #[test]
    fn export_with_redact_scrubs_entity_names() {
        let tmp = tempdir_for_test("tm-cli-export-redact");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let graph = GraphStore::open(&db).expect("open graph");
            let alice = Entity::new(
                "Alice <alice@acme.com>",
                EntityType::Person,
                0.95,
            );
            graph.upsert_entity(&alice).expect("alice");
        }
        let out_dir = tmp.join("bundle");
        cmd_export(
            &db,
            None,
            "markdown",
            &out_dir,
            None,
            None,
            true,
            None,
        )
        .expect("export ok");
        let index = std::fs::read_to_string(out_dir.join("index.md")).expect("index");
        assert!(index.contains("redacted: true"), "redacted flag: {index}");
        assert!(!index.contains("alice@acme.com"), "raw email leaked: {index}");
        assert!(index.contains("[REDACTED-EMAIL]"), "placeholder shown: {index}");
        // Read the only entity file (slug derives from the redacted
        // name, which collapses to something containing "redacted").
        let entities_dir = out_dir.join("entities");
        let mut files = std::fs::read_dir(&entities_dir)
            .expect("read entities dir")
            .map(|e| e.expect("dirent").path())
            .collect::<Vec<_>>();
        files.sort();
        assert_eq!(files.len(), 1, "exactly one entity file");
        let body = std::fs::read_to_string(&files[0]).expect("body");
        assert!(!body.contains("alice@acme.com"), "raw email leaked in body: {body}");
        assert!(body.contains("[REDACTED-EMAIL]"), "placeholder in body: {body}");
    }

    /// Build a uniquely-named directory under the OS temp dir. Avoids
    /// pulling in a new dev-dep just for tests.
    fn tempdir_for_test(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{label}-{pid}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

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

    // -----------------------------------------------------------------
    // LM-5c — DailyNote / `tracemind today`
    // -----------------------------------------------------------------

    #[test]
    fn today_creates_daily_note_idempotently() {
        use super::cmd_today;
        use chrono::Local;
        let tmp = tempdir_for_test("tm-cli-today-idem");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let _ = GraphStore::open(&db).expect("open graph");
        }
        let date = Local::now().date_naive().format("%Y-%m-%d").to_string();

        // First run creates the entity.
        cmd_today(&db, Some(&date), false).expect("first run");
        let graph = GraphStore::open(&db).expect("reopen");
        let first = graph
            .find_entity_by_name_icase(&date)
            .expect("lookup")
            .expect("daily note exists");
        assert_eq!(first.entity_type, EntityType::DailyNote);

        // Second run should upsert — same id, still a DailyNote.
        drop(graph);
        cmd_today(&db, Some(&date), false).expect("second run");
        let graph = GraphStore::open(&db).expect("reopen 2");
        let second = graph
            .find_entity_by_name_icase(&date)
            .expect("lookup 2")
            .expect("daily note still exists");
        assert_eq!(second.id, first.id, "daily note must be idempotent");
    }

    // -----------------------------------------------------------------
    // LM-14 — context merge / split / snapshot
    // -----------------------------------------------------------------

    #[test]
    fn context_merge_relabels_entities_and_triples() {
        use super::cmd_context_merge;
        use tm_graph::context::Context;
        let tmp = tempdir_for_test("tm-cli-ctx-merge");
        let db = tmp.join("memory.db").to_string_lossy().to_string();

        // Two source contexts plus a target. Each source has one entity.
        let (ent_a, ent_b, ctx_a, ctx_b, ctx_into) = {
            let graph = GraphStore::open(&db).expect("open");
            let a = Context::new("a".to_string(), String::new());
            let b = Context::new("b".to_string(), String::new());
            let into = Context::new("merged".to_string(), String::new());
            graph.create_context(&a).unwrap();
            graph.create_context(&b).unwrap();
            graph.create_context(&into).unwrap();

            graph.set_active_context(Some(a.id));
            let ea = Entity::new("Alpha", EntityType::Person, 0.9);
            graph.upsert_entity(&ea).unwrap();

            graph.set_active_context(Some(b.id));
            let eb = Entity::new("Beta", EntityType::Person, 0.9);
            graph.upsert_entity(&eb).unwrap();

            // Triple under context b: Alpha->RelatedTo->Beta. Tag it b.
            let t = Triple::new(ea.id, Predicate::RelatedTo, eb.id, 0.8);
            graph.upsert_triple(&t).unwrap();

            (ea.id, eb.id, a.id, b.id, into.id)
        };

        cmd_context_merge(&db, "a", "b", "merged", false).expect("merge");

        let graph = GraphStore::open(&db).expect("reopen");
        // After merge, both entities point to ctx_into.
        assert_eq!(graph.entity_context_id(ent_a).unwrap(), Some(ctx_into));
        assert_eq!(graph.entity_context_id(ent_b).unwrap(), Some(ctx_into));
        // Original ctx_a / ctx_b are now empty.
        assert!(graph.list_entities_in_context(ctx_a).unwrap().is_empty());
        assert!(graph.list_entities_in_context(ctx_b).unwrap().is_empty());
        // ctx_into now owns both.
        let merged = graph.list_entities_in_context(ctx_into).unwrap();
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn context_snapshot_writes_self_contained_bundle() {
        use super::cmd_context_snapshot;
        use tm_graph::context::Context;
        let tmp = tempdir_for_test("tm-cli-ctx-snap");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        {
            let graph = GraphStore::open(&db).expect("open");
            let ctx = Context::new("rondo".to_string(), String::new());
            graph.create_context(&ctx).unwrap();
            graph.set_active_context(Some(ctx.id));
            let e1 = Entity::new("Player1", EntityType::Person, 0.9);
            let e2 = Entity::new("Team1", EntityType::Organization, 0.9);
            graph.upsert_entity(&e1).unwrap();
            graph.upsert_entity(&e2).unwrap();
            let t = Triple::new(e1.id, Predicate::WorksAt, e2.id, 0.85);
            graph.upsert_triple(&t).unwrap();
        }
        let out = tmp.join("rondo.tmctx");
        cmd_context_snapshot(&db, "rondo", Some(out.to_str().unwrap())).expect("snapshot");

        let raw = std::fs::read_to_string(&out).expect("read snapshot");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse snapshot");
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["kind"], "tracemind.context_snapshot");
        assert_eq!(v["counts"]["entities"], 2);
        assert_eq!(v["counts"]["triples"], 1);
        assert!(v["entities"].as_array().unwrap().len() == 2);
        assert!(v["triples"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn context_split_moves_only_matching_entity_type() {
        use super::cmd_context_split;
        use tm_graph::context::Context;
        let tmp = tempdir_for_test("tm-cli-ctx-split");
        let db = tmp.join("memory.db").to_string_lossy().to_string();
        let (person_id, project_id, ctx_src_id, ctx_into_id) = {
            let graph = GraphStore::open(&db).expect("open");
            let src = Context::new("src".to_string(), String::new());
            let into = Context::new("people".to_string(), String::new());
            graph.create_context(&src).unwrap();
            graph.create_context(&into).unwrap();
            graph.set_active_context(Some(src.id));
            let p = Entity::new("Alice", EntityType::Person, 0.9);
            let pr = Entity::new("ProjectX", EntityType::Project, 0.9);
            graph.upsert_entity(&p).unwrap();
            graph.upsert_entity(&pr).unwrap();
            (p.id, pr.id, src.id, into.id)
        };
        cmd_context_split(&db, "src", "person", "people", false).expect("split");

        let graph = GraphStore::open(&db).expect("reopen");
        assert_eq!(graph.entity_context_id(person_id).unwrap(), Some(ctx_into_id));
        assert_eq!(graph.entity_context_id(project_id).unwrap(), Some(ctx_src_id));
    }

    #[test]
    fn today_backlinks_memories_created_today() {
        use super::cmd_today;
        use chrono::Local;
        let tmp = tempdir_for_test("tm-cli-today-backlinks");
        let db = tmp.join("memory.db").to_string_lossy().to_string();

        // Seed two entities created "today" (now).
        let (alice_id, bob_id) = {
            let graph = GraphStore::open(&db).expect("open graph");
            let alice = Entity::new("Alice", EntityType::Person, 0.9);
            let bob = Entity::new("Bob", EntityType::Person, 0.9);
            graph.upsert_entity(&alice).expect("upsert alice");
            graph.upsert_entity(&bob).expect("upsert bob");
            (alice.id, bob.id)
        };

        let date = Local::now().date_naive().format("%Y-%m-%d").to_string();
        cmd_today(&db, Some(&date), false).expect("today run");

        let graph = GraphStore::open(&db).expect("reopen");
        let daily = graph
            .find_entity_by_name_icase(&date)
            .expect("lookup")
            .expect("daily note");

        // Each of alice/bob should have a triple with object = daily note.
        let alice_tr = graph
            .get_triples_for_entity(alice_id)
            .expect("alice triples");
        assert!(
            alice_tr.iter().any(|t| t.subject_id == alice_id && t.object_id == daily.id),
            "alice → daily-note backlink missing"
        );
        let bob_tr = graph.get_triples_for_entity(bob_id).expect("bob triples");
        assert!(
            bob_tr.iter().any(|t| t.subject_id == bob_id && t.object_id == daily.id),
            "bob → daily-note backlink missing"
        );

        // Running again must not duplicate edges.
        drop(graph);
        cmd_today(&db, Some(&date), false).expect("today rerun");
        let graph = GraphStore::open(&db).expect("reopen 2");
        let alice_tr2 = graph
            .get_triples_for_entity(alice_id)
            .expect("alice triples rerun");
        let n_alice_links = alice_tr2
            .iter()
            .filter(|t| t.subject_id == alice_id && t.object_id == daily.id)
            .count();
        assert_eq!(n_alice_links, 1, "rerun must not duplicate alice backlink");
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
        DemoAction::EvgThread { title, keep_open } => cmd_demo_evg_thread(dir, &title, keep_open),
        DemoAction::EvgLedger { title } => cmd_demo_evg_ledger(dir, &title),
    }
}

/// CTX-EVG-C — seed a deterministic Commitment Ledger fixture.
/// Writes 6 commitments (3 kept, 2 broken, 1 pending) attached to a
/// single demo thread, plus Resolves edges for the resolved ones, so
/// the homepage Ledger card has non-empty data on a clean install.
fn cmd_demo_evg_ledger(dir: &PathBuf, title: &str) {
    use tm_graph::event_graph::{
        CommitmentState, EventGraphStore, EventNode, EventNodeKind,
    };
    use tm_graph::thread_graph::{Thread, ThreadGraphStore, ThreadSource};
    use tm_graph::GraphStore;

    let db_path = dir.join("memory.db");
    let db_path_str = db_path.to_string_lossy().to_string();

    let graph = match GraphStore::open(&db_path_str) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to open graph at {db_path_str}: {e}");
            std::process::exit(1);
        }
    };

    let thread = Thread::new(title.to_string(), ThreadSource::Tracemind);
    if let Err(e) = ThreadGraphStore::upsert(graph.connection(), &thread) {
        eprintln!("upsert thread: {e}");
        std::process::exit(1);
    }

    let now = chrono::Utc::now().timestamp_millis();
    let day = 86_400_000i64;
    // Six commitments distributed across the week: 3 kept, 2 broken,
    // 1 pending. `ts` is when the commitment was created; `due_at` is
    // when it was promised. Salience is the heuristic confidence.
    let fixture = [
        // text, due_offset_days, state
        ("I'll ship the EVG ledger by Friday", -1, CommitmentState::Kept),
        ("I'll write the spec doc by tomorrow", -2, CommitmentState::Kept),
        ("Todo: review the Foundry parity notes", 0, CommitmentState::Kept),
        ("I will email the investor update by Monday", -3, CommitmentState::Broken),
        ("I'm going to draft the demo script by Thursday", -2, CommitmentState::Broken),
        ("I'll record the 3-min walkthrough by Sunday", 2, CommitmentState::Pending),
    ];

    let mut commitment_ids = Vec::new();
    for (i, (text, due_delta, state)) in fixture.iter().enumerate() {
        let due_at = Some(now + (*due_delta as i64) * day);
        let mut node = EventNode::commitment((*text).to_string(), due_at);
        node.ts = now - ((6 - i) as i64) * (day / 4); // staggered creation
        node.thread_id = Some(thread.id);
        node.state = *state;
        node.salience = 0.9;
        if let Err(e) = EventGraphStore::insert_node(graph.connection(), &node) {
            eprintln!("insert commitment {i}: {e}");
            std::process::exit(1);
        }
        commitment_ids.push((node.id, *state));
    }

    // For the kept + broken commitments, write a paired Outcome node
    // and Resolves edge so the EVG view shows a complete loop.
    for (cid, state) in &commitment_ids {
        let polarity = match state {
            CommitmentState::Kept => 1.0,
            CommitmentState::Broken => -1.0,
            _ => continue,
        };
        let payload = format!("outcome-for:{cid}");
        let mut o = EventNode::new(EventNodeKind::Outcome, payload);
        o.thread_id = Some(thread.id);
        o.ts = now;
        if let Err(e) = EventGraphStore::insert_node(graph.connection(), &o) {
            eprintln!("insert outcome: {e}");
            std::process::exit(1);
        }
        if let Err(e) =
            EventGraphStore::resolve_commitment(graph.connection(), o.id, *cid, polarity)
        {
            eprintln!("resolve commitment: {e}");
            std::process::exit(1);
        }
        // Re-upsert with the same polarity so the Resolves edge clears
        // the MIN_SUPPORT floor (>=2) and renders in the EVG view.
        let _ = EventGraphStore::resolve_commitment(
            graph.connection(),
            o.id,
            *cid,
            polarity,
        );
    }

    println!(
        "Seeded Commitment Ledger fixture: thread={} commitments={} (kept=3, broken=2, pending=1)",
        thread.id,
        commitment_ids.len()
    );
}

/// CTX-EVG-C — read the Commitment Ledger.
fn cmd_evg_ledger(dir: &PathBuf, window_days: u32, json: bool) {
    use tm_graph::event_graph::EventGraphStore;
    use tm_graph::GraphStore;

    let db_path = dir.join("memory.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    let graph = match GraphStore::open(&db_path_str) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to open graph at {db_path_str}: {e}");
            std::process::exit(1);
        }
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    // Opportunistic sweep so output reflects current broken state.
    let _ = EventGraphStore::sweep_broken(graph.connection(), now_ms);

    let (since_ms, until_ms) = if window_days == 0 {
        (0, i64::MAX / 2)
    } else {
        let dur = (window_days as i64) * 86_400_000;
        (now_ms - dur, now_ms + 86_400_000)
    };

    let summary = match EventGraphStore::commitment_ledger(
        graph.connection(),
        since_ms,
        until_ms,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ledger: {e}");
            std::process::exit(1);
        }
    };

    if json {
        let obj = serde_json::json!({
            "window_days": window_days,
            "kept": summary.kept,
            "broken": summary.broken,
            "pending": summary.pending,
            "abandoned": summary.abandoned,
            "commitment_ids": summary.commitment_ids
                .iter().map(|u| u.to_string()).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&obj).unwrap());
        return;
    }

    println!("─── Commitment Ledger (last {} days) ───", window_days);
    println!("  kept      {}", summary.kept);
    println!("  broken    {}", summary.broken);
    println!("  pending   {}", summary.pending);
    println!("  abandoned {}", summary.abandoned);
    println!();
    for cid in &summary.commitment_ids {
        if let Ok(Some(n)) = EventGraphStore::get_node(graph.connection(), *cid) {
            let due = n
                .due_at
                .map(|d| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(d)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "?".into())
                })
                .unwrap_or_else(|| "—".into());
            println!(
                "  [{:>9}] due={} {} {}",
                n.state.as_str(),
                due,
                &n.id.to_string()[..8],
                n.payload_ref,
            );
        }
    }
}

/// CTX-EVG Slice A — seed a deterministic thread + a handful of
/// capture/query event nodes so the desktop EventGraphView and the
/// ThreadsView panel both have non-zero data on a clean install. We
/// reuse the live ingest pipeline + retrieval engine so this fixture
/// also exercises the same writers the IPC handlers use — proving
/// the wiring end-to-end, not just the schema.
fn cmd_demo_evg_thread(dir: &PathBuf, title: &str, keep_open: bool) {
    use tm_graph::event_graph::{EventGraphStore, EventNode, EventNodeKind};
    use tm_graph::thread_graph::{Thread, ThreadGraphStore, ThreadSource};
    use tm_graph::GraphStore;
    use uuid::Uuid;

    let db_path = dir.join("memory.db");
    let db_path_str = db_path.to_string_lossy().to_string();

    let graph = match GraphStore::open(&db_path_str) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to open graph at {db_path_str}: {e}");
            std::process::exit(1);
        }
    };

    // 1. Open a thread.
    let mut thread = Thread::new(title.to_string(), ThreadSource::Tracemind);
    if let Err(e) = ThreadGraphStore::upsert(graph.connection(), &thread) {
        eprintln!("thread upsert failed: {e}");
        std::process::exit(1);
    }
    let thread_id = thread.id;

    // 2. Seed 5 Capture events + 2 Query events. We use synthetic
    //    payload_refs (uuid strings) so the fixture works without
    //    having to also touch the trace store — the EVG view only
    //    cares about the event-node row.
    let captures = [
        ("Aaditya kicked off the EVG sprint", 0.82),
        ("Decided Slice A = hot-path wiring", 0.78),
        ("Threaded ingest writes Capture nodes", 0.71),
        ("Threaded query writes Query nodes", 0.69),
        ("Demo fixture seeds a thread", 0.74),
    ];
    let mut node_ids: Vec<Uuid> = Vec::with_capacity(captures.len() + 2);
    for (_label, salience) in captures.iter() {
        let mut n = EventNode::new(EventNodeKind::Capture, Uuid::new_v4().to_string());
        n.thread_id = Some(thread_id);
        n.salience = *salience;
        if let Err(e) = EventGraphStore::insert_node(graph.connection(), &n) {
            eprintln!("insert capture node failed: {e}");
            std::process::exit(1);
        }
        node_ids.push(n.id);
    }
    for salience in [0.20, 0.40] {
        let mut n = EventNode::new(EventNodeKind::Query, Uuid::new_v4().to_string());
        n.thread_id = Some(thread_id);
        n.salience = salience;
        if let Err(e) = EventGraphStore::insert_node(graph.connection(), &n) {
            eprintln!("insert query node failed: {e}");
            std::process::exit(1);
        }
        node_ids.push(n.id);
    }

    // 3. Optionally end the thread so it shows up in the "ended"
    //    section. The materialize view still works on closed threads.
    //    Also promote `Precedes` edges between consecutive nodes so
    //    the EventGraphView has real visible edges. Two passes lift
    //    them above MIN_SUPPORT = 2 so they render via
    //    `visible_edges()` instead of staying invisible-below-floor.
    let mut promoted = 0usize;
    if !keep_open {
        if let Err(e) = ThreadGraphStore::end_thread(graph.connection(), thread_id) {
            eprintln!("end thread failed: {e}");
            std::process::exit(1);
        }
        thread.ended_at = Some(chrono::Utc::now());
        for _ in 0..2 {
            match EventGraphStore::promote_thread_sequence_edges(graph.connection(), thread_id) {
                Ok(n) => promoted = n,
                Err(e) => {
                    eprintln!("promote thread edges failed: {e}");
                    break;
                }
            }
        }
    }

    println!("seeded EVG thread");
    println!("  thread_id     : {thread_id}");
    println!("  source        : tracemind");
    println!("  state         : {}", if keep_open { "open" } else { "ended" });
    println!("  capture events: {}", captures.len());
    println!("  query events  : 2");
    println!("  total nodes   : {}", node_ids.len());
    if !keep_open {
        println!("  precedes edges: {} (visible: support>=2)", promoted);
    }
    println!();
    println!("open the desktop app → Threads view to inspect.");
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
    use tm_graph::{GraphStore, context::Context};
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

    // ── Contexts (UI-6) ────────────────────────────────────────────
    // Two deterministic contexts so the demo recording can showcase
    // context switching + scoped retrieval. UUIDs are UUIDv5 from the
    // frozen demo namespace so the short-id callouts in the demo
    // script never drift between recordings.
    let mk_ctx = |name: &str, tags: &str| -> Context {
        let id = Uuid::new_v5(&DEMO_NAMESPACE, format!("ctx:{name}").as_bytes());
        Context {
            id,
            name: name.to_string(),
            tags: tags.to_string(),
            created_at: now - Duration::days(30),
        }
    };
    let ctx_mercury = mk_ctx("Mercury work", "client,mercury,b2b");
    let ctx_dev = mk_ctx("TraceMind dev", "internal,engineering,product");
    graph.create_context(&ctx_mercury).expect("create ctx mercury");
    graph.create_context(&ctx_dev).expect("create ctx dev");

    // Cluster 1 — Mercury work. Customer-facing.
    let mercury_entities = [
        &alice, &bob, &carla, &priya, &mercury, &q1memo, &onboarding, &oncall_runbook,
    ];
    graph.set_active_context(Some(ctx_mercury.id));
    for e in mercury_entities {
        graph.upsert_entity(e).expect("upsert mercury entity");
    }

    // Cluster 2 — TraceMind dev. Internal product/engineering.
    let dev_entities = [
        &demo_proj, &postgres, &sqlite, &pitch_deck,
        &calibration_plan, &research_doc, &sprint_plan,
    ];
    graph.set_active_context(Some(ctx_dev.id));
    for e in dev_entities {
        graph.upsert_entity(e).expect("upsert dev entity");
    }
    // Reset to unscoped so subsequent triples can decide their own ctx.
    graph.set_active_context(None);
    let entities = mercury_entities
        .iter()
        .chain(dev_entities.iter())
        .copied()
        .collect::<Vec<_>>();

    // ── Triples ────────────────────────────────────────────────────
    let mk_triple = |label: &str, s: Uuid, p: Predicate, o: Uuid, conf: f64| -> Triple {
        let id = Uuid::new_v5(&DEMO_NAMESPACE, format!("triple:{label}").as_bytes());
        let mut t = Triple::new(s, p, o, conf);
        t.id = id;
        t
    };

    // Mercury-context triples. Includes the loves/hates contradiction
    // since both subjects (Alice, Bob) live in the Mercury cluster.
    let mercury_triples = vec![
        mk_triple("alice-works-mercury", alice.id, Predicate::WorksAt, mercury.id, 0.95),
        mk_triple("bob-works-mercury", bob.id, Predicate::WorksAt, mercury.id, 0.92),
        mk_triple("carla-collab-alice", carla.id, Predicate::CollaboratesWith, alice.id, 0.88),
        mk_triple("priya-collab-bob", priya.id, Predicate::CollaboratesWith, bob.id, 0.84),
        mk_triple("alice-owns-q1memo", alice.id, Predicate::Owns, q1memo.id, 0.9),
        mk_triple("q1memo-references-mercury", q1memo.id, Predicate::References, mercury.id, 0.93),
        mk_triple("onboarding-partof-mercury", onboarding.id, Predicate::PartOf, mercury.id, 0.86),
        // The contradicting pair — same subject + object, opposing
        // predicates. Detected explicitly below at cosine = -0.94.
        mk_triple("alice-loves-bob", alice.id, Predicate::Custom("loves".into()), bob.id, 0.88),
        mk_triple("alice-hates-bob", alice.id, Predicate::Custom("hates".into()), bob.id, 0.85),
    ];
    // Dev-context triples.
    let dev_triples = vec![
        mk_triple("demo-depends-postgres", demo_proj.id, Predicate::DependsOn, postgres.id, 0.9),
        mk_triple("demo-depends-sqlite", demo_proj.id, Predicate::DependsOn, sqlite.id, 0.95),
        mk_triple("pitch-references-demo", pitch_deck.id, Predicate::References, demo_proj.id, 0.91),
        mk_triple("calibration-related", calibration_plan.id, Predicate::RelatedTo, demo_proj.id, 0.8),
        mk_triple("research-references-locomo", research_doc.id, Predicate::References, demo_proj.id, 0.87),
        mk_triple("sprint-related-demo", sprint_plan.id, Predicate::RelatedTo, demo_proj.id, 0.83),
        mk_triple("oncall-references-postgres", oncall_runbook.id, Predicate::References, postgres.id, 0.81),
    ];

    graph.set_active_context(Some(ctx_mercury.id));
    for t in &mercury_triples {
        graph.upsert_triple(t).expect("upsert mercury triple");
    }
    graph.set_active_context(Some(ctx_dev.id));
    for t in &dev_triples {
        graph.upsert_triple(t).expect("upsert dev triple");
    }
    graph.set_active_context(None);
    let triples: Vec<Triple> = mercury_triples
        .iter()
        .chain(dev_triples.iter())
        .cloned()
        .collect();

    // Detect the contradiction so the brief surfaces it. The cosine
    // is hard-coded — the live pipeline computes it from embeddings,
    // but for the fixture we just want the JTMS to flip both beliefs
    // to Contradicted. The pair lives at the tail of the Mercury
    // cluster (alice-loves-bob, alice-hates-bob).
    let loves = mercury_triples[mercury_triples.len() - 2].id;
    let hates = mercury_triples[mercury_triples.len() - 1].id;
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
    println!("  contexts:        2   (Mercury work, TraceMind dev)");
    println!("  entities:        {}  ({} Mercury / {} TraceMind dev)",
        entities.len(), mercury_entities.len(), dev_entities.len());
    println!("  triples:         {}  ({} Mercury / {} TraceMind dev)",
        triples.len(), mercury_triples.len(), dev_triples.len());
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
