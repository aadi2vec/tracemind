use std::fs;
use std::path::PathBuf;

use clap::Parser;
use tm_controller::UcbBandit;
use tm_episodic::TraceStore;
use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Bandit persistence struct
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct BanditState {
    counts: [u64; 4],
    rewards: [f64; 4],
    total_pulls: u64,
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(clap::Parser)]
#[command(name = "tracemind", about = "TraceMind local memory OS")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Ingest text into memory
    Ingest { text: String },
    /// Query memory with natural language
    Query { text: String },
    /// Register a reward for a bandit arm
    Feedback { arm: u8, reward: f64 },
    /// Show recent traces
    Trace {
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Show bandit arm statistics
    Status,
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
            let pipeline = IngestPipeline::open(&db_path)
                .expect("failed to open ingest pipeline");
            let session_id = Uuid::new_v4();
            let result = pipeline.ingest(&text, session_id)
                .expect("ingest failed");
            println!(
                "Stored {} entities, {} triples.",
                result.entities.len(),
                result.triples.len()
            );
        }

        Commands::Query { text } => {
            let mut engine = RetrievalEngine::open(&db_path, &trace_path)
                .expect("failed to open retrieval engine");
            let result = engine.query(&text).expect("query failed");
            for entity in &result.entities {
                println!("  [{}] {}", entity.entity_type, entity.name);
            }
            for triple in &result.triples {
                println!(
                    "  {} -> {} -> {}",
                    triple.subject_id, triple.predicate, triple.object_id
                );
            }
        }

        Commands::Feedback { arm, reward } => {
            // v1 limitation: bandit state is not persisted between CLI calls.
            // Load existing state if available, register reward, save back.
            let mut bandit = UcbBandit::new();

            // Attempt to load existing state and replay counts/rewards into a
            // fresh bandit so the running average is preserved.
            if bandit_path.exists() {
                if let Ok(raw) = fs::read_to_string(&bandit_path) {
                    if let Ok(state) = serde_json::from_str::<BanditState>(&raw) {
                        // Reconstruct by replaying synthetic single-pull events
                        // that reproduce the stored running averages.
                        // Since UcbBandit doesn't expose direct field mutation,
                        // we approximate by registering the stored average reward
                        // for each recorded pull.
                        let mut b = UcbBandit::new();
                        for arm_idx in 0u8..4 {
                            let pulls = state.counts[arm_idx as usize];
                            let avg = state.rewards[arm_idx as usize];
                            for _ in 0..pulls {
                                b.register_reward(arm_idx, avg);
                            }
                        }
                        bandit = b;
                    }
                }
            }

            bandit.register_reward(arm, reward);

            // Persist updated state.
            let stats = bandit.arm_stats();
            let total_pulls: u64 = stats.iter().map(|(c, _)| c).sum();
            let state = BanditState {
                counts: [stats[0].0, stats[1].0, stats[2].0, stats[3].0],
                rewards: [stats[0].1, stats[1].1, stats[2].1, stats[3].1],
                total_pulls,
            };
            let json = serde_json::to_string_pretty(&state)
                .expect("failed to serialize bandit state");
            fs::write(&bandit_path, json).expect("failed to write bandit state");

            println!("Reward registered.");
        }

        Commands::Trace { limit } => {
            let store = TraceStore::open(&trace_path)
                .expect("failed to open trace store");
            let traces = store.recent(limit).expect("failed to read traces");
            for trace in &traces {
                println!(
                    "  [{:?}] {} at {}",
                    trace.event_type, trace.id, trace.created_at
                );
            }
        }

        Commands::Status => {
            // Load bandit state from file if available; otherwise fresh bandit.
            let bandit = if bandit_path.exists() {
                if let Ok(raw) = fs::read_to_string(&bandit_path) {
                    if let Ok(state) = serde_json::from_str::<BanditState>(&raw) {
                        let mut b = UcbBandit::new();
                        for arm_idx in 0u8..4 {
                            let pulls = state.counts[arm_idx as usize];
                            let avg = state.rewards[arm_idx as usize];
                            for _ in 0..pulls {
                                b.register_reward(arm_idx, avg);
                            }
                        }
                        b
                    } else {
                        UcbBandit::new()
                    }
                } else {
                    UcbBandit::new()
                }
            } else {
                UcbBandit::new()
            };

            let stats = bandit.arm_stats();
            for (i, (pulls, avg_reward)) in stats.iter().enumerate() {
                println!("  Arm {}: pulls={}, avg_reward={:.2}", i, pulls, avg_reward);
            }
        }
    }
}
