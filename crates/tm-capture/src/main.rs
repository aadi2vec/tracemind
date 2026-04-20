//! TraceMind Capture Daemon
//!
//! Passive background process that monitors:
//! - Clipboard changes (macOS pbpaste polling)
//! - Shell history (watches ~/.zsh_history or ~/.bash_history)
//!
//! Automatically ingests captured content into TraceMind memory.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tokio::time;
use tracing::{info, warn, debug};
use uuid::Uuid;

use tm_ingest::IngestPipeline;

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
            hash_embed: std::env::var("TM_HASH_EMBED")
                .map(|v| v == "1")
                .unwrap_or(false),
        }
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
    info!("[clipboard] starting monitor (interval={}ms)", config.clipboard_interval.as_millis());

    let pipeline = match open_pipeline_with_ner(&config.db_path, config.hash_embed) {
        Ok(p) => p,
        Err(e) => {
            warn!("[clipboard] failed to open pipeline: {e}");
            return;
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
                    if let Some(reason) = result.skipped {
                        debug!("[clipboard] signal skipped: {reason}");
                    } else {
                        info!("[clipboard] signal stored (id={})", result.signal_id);
                    }
                }
                Err(e) => {
                    debug!("[clipboard] fast ingest skipped: {e}");
                }
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

async fn history_loop(config: &CaptureConfig) {
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
                    if let Some(reason) = result.skipped {
                        debug!("[history] signal skipped: {reason}");
                    } else {
                        info!("[history] signal stored (id={})", result.signal_id);
                    }
                }
                Err(e) => {
                    debug!("[history] fast ingest skipped: {e}");
                }
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("tm_capture=info".parse().unwrap()),
        )
        .init();

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
        priority_consolidation_loop(&config),
        consolidation_loop(&config),
    );
}
