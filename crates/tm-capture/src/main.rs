//! TraceMind Capture Daemon
//!
//! Passive background process that monitors:
//! - Clipboard changes (macOS pbpaste polling)
//! - Shell history (watches ~/.zsh_history or ~/.bash_history)
//!
//! Automatically ingests captured content into TraceMind memory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tokio::time;
use tracing::{info, warn, debug};
use uuid::Uuid;

use tm_episodic::RecentStore;
use tm_ingest::{FastIngestResult, IngestPipeline, SignalPriority};
use tm_types::capture_permissions::{CapturePermissions, CaptureSource};
use tm_types::RecentCapture;

mod browser_capture;
mod browser_history;
mod modalities;
mod vision_ocr;

/// TM-NLP-004 helper: open an IngestPipeline and attach the real GLiNER
/// NER extractor when the model is available on disk / over the network.
/// The capture daemon benefits most from this — passive capture is where
/// high-quality NER moves the needle.
fn open_pipeline_with_ner(
    db_path: &str,
    hash_embed: bool,
) -> Result<IngestPipeline, tm_types::TraceMindError> {
    let mut pipeline = IngestPipeline::open(db_path, hash_embed)?;
    if let Some(gli) = tm_ingest::GlinerExtractor::auto_download_default() {
        pipeline = pipeline.with_extractor(Box::new(gli));
    }
    Ok(pipeline)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

struct CaptureConfig {
    db_path: String,
    /// Path to the intent store (`~/.tracemind/intents.db`). Mined
    /// commitment candidates are persisted here for the daily-brief
    /// to surface — see `docs/INTENT_SYSTEM.md` §3.1.
    intents_path: String,
    /// CAP-1 — path to per-source permissions file
    /// (`<data_dir>/capture_permissions.toml`). Resolved here so the
    /// daemon respects `TM_DATA_DIR` overrides used in tests.
    permissions_path: PathBuf,
    /// CAP-4 — path to the per-install browser bookmarklet token
    /// (`<data_dir>/capture_token`). Generated on first start;
    /// embedded in the bookmarklet snippet emitted by
    /// `tracemind capture bookmarklet`.
    capture_token_path: PathBuf,
    clipboard_interval: Duration,
    history_interval: Duration,
    /// How often to run the slow-path (Tier-3 Normal) consolidation pass.
    consolidation_interval: Duration,
    /// How often to run the priority-path (Tier-2) consolidation pass — novel or
    /// high-entropy captures get promoted on this tighter schedule so they don't
    /// have to wait for the normal pass.
    priority_interval: Duration,
    hash_embed: bool,
}

impl CaptureConfig {
    fn from_env() -> Self {
        let dir = if let Ok(val) = std::env::var("TM_DATA_DIR") {
            PathBuf::from(val)
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".tracemind")
        };
        std::fs::create_dir_all(&dir).expect("failed to create data dir");

        Self {
            db_path: dir.join("memory.db").to_str().unwrap().to_string(),
            intents_path: dir.join("intents.db").to_str().unwrap().to_string(),
            permissions_path: dir.join("capture_permissions.toml"),
            capture_token_path: dir.join("capture_token"),
            clipboard_interval: Duration::from_millis(
                std::env::var("TM_CLIP_INTERVAL_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1000),
            ),
            history_interval: Duration::from_secs(
                std::env::var("TM_HIST_INTERVAL_S")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(5),
            ),
            consolidation_interval: Duration::from_secs(
                std::env::var("TM_CONSOLIDATE_INTERVAL_S")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(300), // default: every 5 minutes (Tier-3)
            ),
            priority_interval: Duration::from_secs(
                std::env::var("TM_PRIORITY_INTERVAL_S")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(30), // default: every 30 seconds (Tier-2)
            ),
            // Honor BOTH the `--hash-embed` CLI flag (matching the
            // `tracemind` binary) and the `TM_HASH_EMBED=1` env var.
            // Mixing embedders between the daemon (ingest) and the CLI
            // (query) silently breaks retrieval — the vectors live in
            // different spaces — so the flag must be spelled the same way
            // everywhere.
            hash_embed: std::env::args().any(|a| a == "--hash-embed")
                || std::env::var("TM_HASH_EMBED").map(|v| v == "1").unwrap_or(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Recent ring-buffer helpers
// ---------------------------------------------------------------------------

fn tier_label(priority: SignalPriority) -> &'static str {
    match priority {
        SignalPriority::InstantEntity => "t1",
        SignalPriority::Priority => "t2",
        SignalPriority::Normal => "t3",
        SignalPriority::Ephemeral => "t4",
    }
}

fn recent_path(db_path: &str) -> PathBuf {
    let p = PathBuf::from(db_path);
    let dir = p.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    dir.join("recent.jsonl")
}

fn record_capture(
    store: &RecentStore,
    source: &str,
    text: &str,
    result: &FastIngestResult,
) {
    let promoted = result.skipped.is_none()
        && matches!(
            result.priority,
            SignalPriority::InstantEntity | SignalPriority::Priority | SignalPriority::Normal
        );
    let mut event = RecentCapture::new(source, &result.content_hash, text)
        .with_tier(tier_label(result.priority))
        .with_promoted(promoted);
    if let Some(reason) = &result.skipped {
        event = event.with_skipped(reason.clone());
    }
    if let Err(e) = store.append(&event) {
        debug!("[recent] failed to append capture: {e}");
    }
}

/// Run the [`tm_intent::miner`] over a freshly-captured text and persist
/// any hits as `pending` candidates in the intent store.
///
/// `mut_store` is taken `&mut` because the bulk insert path uses a
/// transaction. Callers hold one open `IntentStore` per loop and reuse
/// it across captures — opening per-capture would be wasteful.
///
/// All errors are logged at `debug!` and swallowed: the miner is a
/// soft-fail surface and a SQLite hiccup must never break ingest.
fn mine_capture_for_candidates(
    mut_store: &mut tm_intent::IntentStore,
    source: &str,
    text: &str,
) {
    let mined = tm_intent::mine(text);
    if mined.is_empty() {
        return;
    }
    let records: Vec<tm_intent::store::CandidateRecord> = mined
        .iter()
        .map(|m| tm_intent::store::CandidateRecord::from_mined(m, text.to_string()))
        .collect();
    match mut_store.insert_candidates(&records) {
        Ok(n) if n > 0 => {
            info!(
                "[{source}/miner] queued {n} commitment candidate(s) — phrases: {:?}",
                records.iter().map(|r| r.matched_phrase.as_str()).collect::<Vec<_>>()
            );
        }
        Ok(_) => {}
        Err(e) => debug!("[{source}/miner] persist failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Clipboard monitor
// ---------------------------------------------------------------------------

fn get_clipboard() -> Option<String> {
    let output = Command::new("pbpaste")
        .output()
        .ok()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() || text.len() < 10 {
            return None;
        }
        // Skip if it looks like a password (high entropy, short, no spaces)
        if text.len() < 50 && !text.contains(' ') && has_high_entropy(&text) {
            debug!("[clipboard] skipping high-entropy content (possible password)");
            return None;
        }
        Some(text)
    } else {
        None
    }
}

fn has_high_entropy(s: &str) -> bool {
    let unique: HashSet<char> = s.chars().collect();
    let ratio = unique.len() as f64 / s.len() as f64;
    ratio > 0.7 && s.len() > 8
}

fn content_hash(s: &str) -> u64 {
    seahash::hash(s.as_bytes())
}

async fn clipboard_loop(config: &CaptureConfig) {
    // CAP-1 — fail closed. If the source isn't explicitly enabled in
    // capture_permissions.toml, the loop never starts and the daemon
    // logs the skip so the user can audit.
    if !load_enabled(&config.permissions_path, CaptureSource::Clipboard) {
        info!("[clipboard] disabled in permissions — loop will not start");
        return;
    }

    info!("[clipboard] starting monitor (interval={}ms)", config.clipboard_interval.as_millis());

    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[clipboard] failed to open pipeline: {e}");
            return;
        }
    };

    let recent = match RecentStore::open(recent_path(&config.db_path)) {
        Ok(r) => Some(r),
        Err(e) => {
            warn!("[clipboard] failed to open recent store: {e}");
            None
        }
    };

    let mut intents = match tm_intent::IntentStore::open(&config.intents_path) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("[clipboard] failed to open intent store: {e}");
            None
        }
    };

    let mut seen_hashes: HashSet<u64> = HashSet::new();
    let mut last_hash: u64 = 0;

    loop {
        time::sleep(config.clipboard_interval).await;

        if let Some(text) = get_clipboard() {
            let hash = content_hash(&text);

            // Skip duplicates
            if hash == last_hash || seen_hashes.contains(&hash) {
                continue;
            }
            last_hash = hash;
            seen_hashes.insert(hash);

            // Keep seen set bounded
            if seen_hashes.len() > 1000 {
                seen_hashes.clear();
                seen_hashes.insert(hash);
            }

            info!("[clipboard] captured: \"{}\"", truncate(&text, 60));

            let session = Uuid::new_v4();
            match pipeline.ingest_fast(&text, "clipboard", session) {
                Ok(result) => {
                    if let Some(reason) = &result.skipped {
                        debug!("[clipboard] signal skipped: {reason}");
                    } else {
                        info!("[clipboard] signal stored (id={})", result.signal_id);
                    }
                    if let Some(store) = &recent {
                        record_capture(store, "clipboard", &text, &result);
                    }
                }
                Err(e) => {
                    debug!("[clipboard] fast ingest skipped: {e}");
                }
            }

            // Mine the captured text for commitment-shaped phrases. Runs
            // independently of fast-ingest success so even skipped
            // signals get a chance to surface as candidates.
            if let Some(store) = intents.as_mut() {
                mine_capture_for_candidates(store, "clipboard", &text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shell history monitor
// ---------------------------------------------------------------------------

fn history_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let zsh = PathBuf::from(&home).join(".zsh_history");
    if zsh.exists() {
        return Some(zsh);
    }
    let bash = PathBuf::from(&home).join(".bash_history");
    if bash.exists() {
        return Some(bash);
    }
    None
}

fn read_last_lines(path: &PathBuf, n: usize) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .rev()
        .take(n)
        .filter(|line| {
            // zsh history lines start with ": timestamp:0;" — strip that prefix
            let clean = if line.starts_with(": ") {
                line.splitn(3, ';').nth(1).unwrap_or(line)
            } else {
                line
            };
            let clean = clean.trim();
            // Skip short/empty, common noise
            clean.len() > 5
                && !clean.starts_with('#')
                && !matches!(
                    clean,
                    "ls" | "cd" | "pwd" | "clear" | "exit" | "history" | "ll" | "la"
                )
        })
        .map(|line| {
            if line.starts_with(": ") {
                line.splitn(3, ';')
                    .nth(1)
                    .unwrap_or(line)
                    .trim()
                    .to_string()
            } else {
                line.trim().to_string()
            }
        })
        .collect()
}

/// CAP-1 — load the permissions file (creating it with safe defaults
/// on first run) and report whether `source` is enabled. We swallow
/// load errors and fail *closed* (return false) so a malformed file
/// can never silently start a sensitive source.
fn load_enabled(path: &Path, source: CaptureSource) -> bool {
    match CapturePermissions::load_or_default(path) {
        Ok(perms) => perms.is_enabled(source),
        Err(e) => {
            warn!(
                "[capture] failed to load permissions ({}); failing closed for {source}: {e}",
                path.display()
            );
            false
        }
    }
}

async fn history_loop(config: &CaptureConfig) {
    // CAP-1 — fail closed on the shell source.
    if !load_enabled(&config.permissions_path, CaptureSource::Shell) {
        info!("[shell] disabled in permissions — loop will not start");
        return;
    }

    let hist_path = match history_path() {
        Some(p) => p,
        None => {
            warn!("[history] no shell history file found, skipping");
            return;
        }
    };

    info!("[history] monitoring {:?} (interval={}s)", hist_path, config.history_interval.as_secs());

    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[history] failed to open pipeline: {e}");
            return;
        }
    };

    let recent_store = match RecentStore::open(recent_path(&config.db_path)) {
        Ok(r) => Some(r),
        Err(e) => {
            warn!("[history] failed to open recent store: {e}");
            None
        }
    };

    let mut intents = match tm_intent::IntentStore::open(&config.intents_path) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("[history] failed to open intent store: {e}");
            None
        }
    };

    let mut seen_hashes: HashSet<u64> = HashSet::new();

    // Seed with existing history to avoid re-ingesting on startup.
    for line in read_last_lines(&hist_path, 100) {
        seen_hashes.insert(content_hash(&line));
    }
    info!("[history] seeded {} existing commands", seen_hashes.len());

    loop {
        time::sleep(config.history_interval).await;

        let recent = read_last_lines(&hist_path, 20);
        for line in recent {
            let hash = content_hash(&line);
            if seen_hashes.contains(&hash) {
                continue;
            }
            seen_hashes.insert(hash);

            // Keep bounded
            if seen_hashes.len() > 5000 {
                seen_hashes.clear();
                seen_hashes.insert(hash);
            }

            info!("[history] new command: \"{}\"", truncate(&line, 60));

            let prefixed = format!("shell command: {}", line);
            let session = Uuid::new_v4();
            match pipeline.ingest_fast(&prefixed, "shell", session) {
                Ok(result) => {
                    if let Some(reason) = &result.skipped {
                        debug!("[history] signal skipped: {reason}");
                    } else {
                        info!("[history] signal stored (id={})", result.signal_id);
                    }
                    if let Some(store) = &recent_store {
                        record_capture(store, "shell", &prefixed, &result);
                    }
                }
                Err(e) => {
                    debug!("[history] fast ingest skipped: {e}");
                }
            }

            // Mine the *unprefixed* line for commitment phrases — the
            // "shell command:" prefix would otherwise pollute every
            // mined statement.
            if let Some(store) = intents.as_mut() {
                mine_capture_for_candidates(store, "shell", &line);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Slow-path consolidation loop
// ---------------------------------------------------------------------------

async fn consolidation_loop(config: &CaptureConfig) {
    info!(
        "[consolidate/normal] starting (interval={}s, tier=3)",
        config.consolidation_interval.as_secs()
    );

    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[consolidate/normal] failed to open pipeline: {e}");
            return;
        }
    };

    loop {
        time::sleep(config.consolidation_interval).await;

        match pipeline.consolidate(200, 3, 0.75) {
            Ok(stats) => {
                if stats.signals_scanned > 0 {
                    info!(
                        "[consolidate/normal] scanned={} clusters={} entities_promoted={} triples={} noise={}",
                        stats.signals_scanned,
                        stats.clusters_formed,
                        stats.entities_promoted,
                        stats.triples_created,
                        stats.noise_signals,
                    );
                }
            }
            Err(e) => {
                warn!("[consolidate/normal] error: {e}");
            }
        }
    }
}

/// Aggressive priority consolidation — promotes Tier-2 (novel/high-entropy)
/// captures quickly so the graph reflects fresh insights without waiting for
/// the normal 5-minute pass.
async fn priority_consolidation_loop(config: &CaptureConfig) {
    info!(
        "[consolidate/priority] starting (interval={}s, tier=2)",
        config.priority_interval.as_secs()
    );

    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[consolidate/priority] failed to open pipeline: {e}");
            return;
        }
    };

    loop {
        time::sleep(config.priority_interval).await;

        match pipeline.consolidate_priority(100, 0.75) {
            Ok(stats) => {
                if stats.signals_scanned > 0 {
                    info!(
                        "[consolidate/priority] scanned={} clusters={} entities_promoted={} triples={}",
                        stats.signals_scanned,
                        stats.clusters_formed,
                        stats.entities_promoted,
                        stats.triples_created,
                    );
                }
            }
            Err(e) => {
                warn!("[consolidate/priority] error: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        format!("{}...", &s[..max])
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // TM-NLP-005: resolve bundled model directory before logging so that any
    // later env-var reads see the bundle path.
    let bundled = tm_types::bundled::init();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("tracemind_capture=info".parse().unwrap()),
        )
        .init();

    if let Some(r) = &bundled {
        tracing::info!(
            "[tm-capture] bundled models resolved from {} ({})",
            r.hf_cache.display(),
            r.source
        );
    }

    let config = CaptureConfig::from_env();

    println!("TraceMind Capture Daemon (two-speed pipeline, 4-tier promotion)");
    println!("  Data dir: {}", config.db_path.rsplit('/').nth(1).unwrap_or("?"));
    println!("  Clipboard polling:     {}ms", config.clipboard_interval.as_millis());
    println!("  History polling:       {}s", config.history_interval.as_secs());
    println!("  Priority consolidate:  every {}s (Tier-2)", config.priority_interval.as_secs());
    println!("  Normal consolidate:    every {}s (Tier-3)", config.consolidation_interval.as_secs());
    println!("  Press Ctrl+C to stop\n");

    // Run fast-path capture + slow-path (normal + priority) consolidation concurrently.
    tokio::join!(
        clipboard_loop(&config),
        history_loop(&config),
        browser_capture::browser_capture_loop(
            config.permissions_path.clone(),
            config.db_path.clone(),
            config.capture_token_path.clone(),
        ),
        priority_consolidation_loop(&config),
        consolidation_loop(&config),
        multimodal_loop(&config),
    );
}

/// Product Plan X1/X5/X11/X12/X15/X18/X19 — poll every registered
/// modality source and route new payloads through `ingest_multimodal`.
/// Interval is deliberately slow (default 30s) — most modalities are
/// low-frequency (screenshots, saved PDFs) and file-watching is cheap
/// enough that missing a beat is fine.
async fn multimodal_loop(config: &CaptureConfig) {
    let interval = Duration::from_secs(
        std::env::var("TM_MODALITY_INTERVAL_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
    );
    // Data dir is the parent of memory.db.
    let data_dir = PathBuf::from(&config.db_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[multimodal] cannot open ingest pipeline: {e}; loop off");
            return;
        }
    };
    // CAP-1 — fail closed. Only sources the user explicitly enabled in
    // capture_permissions.toml are polled; every modality here (Notes,
    // Calendar, Mail, Photos, PDFs, …) is high-sensitivity and starts
    // disabled. A malformed/unreadable permissions file yields zero
    // sources, matching clipboard/shell/browser behaviour.
    let perms = match CapturePermissions::load_or_default(&config.permissions_path) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "[multimodal] failed to load permissions ({}); failing closed — no sources will start: {e}",
                config.permissions_path.display()
            );
            return;
        }
    };
    let mut registry = modalities::ModalityRegistry::with_enabled(data_dir.clone(), &perms);
    if registry.is_empty() {
        info!("[multimodal] no modality sources enabled in permissions — loop will not start");
        return;
    }
    info!(
        "[multimodal] {} source(s) enabled ({:?}), tick={}s",
        registry.len(),
        registry.names(),
        interval.as_secs()
    );
    // Mirror accepted captures into the `recent.jsonl` ring buffer so
    // `tracemind recent` surfaces ambient modality captures alongside
    // clipboard/shell — otherwise the daemon's most product-visible work
    // (Notes, Mail, Calendar) would be invisible in the audit surface.
    let recent = match RecentStore::open(recent_path(&config.db_path)) {
        Ok(r) => Some(r),
        Err(e) => {
            warn!("[multimodal] failed to open recent store: {e}");
            None
        }
    };
    loop {
        time::sleep(interval).await;
        let payloads = registry.tick().await;
        for payload in payloads {
            let session = Uuid::new_v4();
            match pipeline.ingest_multimodal(&payload, session) {
                Ok(res) => {
                    debug!(
                        target: "tm_modality",
                        kind = payload.kind.as_str(),
                        source = %payload.source,
                        skipped = ?res.skipped,
                        content_hash = %res.content_hash,
                        "ingested"
                    );
                    if let Some(store) = &recent {
                        // Prefer the canonical/extracted text; fall back to
                        // the caller hint (note title, email subject) and
                        // finally a modality stub so the ring buffer row is
                        // never blank.
                        let display = payload
                            .text
                            .clone()
                            .or_else(|| payload.hint.clone())
                            .unwrap_or_else(|| format!("[{} capture]", payload.kind.as_str()));
                        record_capture(store, &payload.source, &display, &res);
                    }
                }
                Err(e) => {
                    warn!(
                        "[multimodal] ingest_multimodal({}, {}) failed: {e}",
                        payload.kind.as_str(),
                        payload.source
                    );
                }
            }
        }
    }
}
