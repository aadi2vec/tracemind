//! Smoke test for the candle Tier-1 backend.
//!
//! Run: cargo run --release -p tm-answer --features candle-llm --example smoke_candle
//!
//! Requires the model at ~/.tracemind/models/qwen2.5-0.5b-instruct-q4_k_m.gguf
//! (download via scripts/fetch-models.sh or it auto-downloads on first call).

use tm_answer::backend::AnswerBackend;
use tm_answer::candle_backend::CandleBackend;
use tm_answer::types::{AnswerRequest, GroundingChunk, TaskKind};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let backend = CandleBackend::default_desktop();
    println!("weights present: {}", backend.weights_present());

    let grounding = vec![
        GroundingChunk {
            trace_id: "t1".into(),
            entity_ids: vec![],
            text: "Carol is leaving Stripe to start a company. Working title: Loom.".into(),
            score: 0.9,
        },
        GroundingChunk {
            trace_id: "t2".into(),
            entity_ids: vec![],
            text: "Carol got the term sheet from Sequoia. They wanted to lead at $2.5M.".into(),
            score: 0.85,
        },
        GroundingChunk {
            trace_id: "t3".into(),
            entity_ids: vec![],
            text: "Carol renamed Loom to Memex — the original name was trademarked.".into(),
            score: 0.8,
        },
    ];

    let questions = [
        "What was Carol's previous employer?",
        "How much did Carol raise?",
        "What did Carol rename her company to?",
    ];

    for q in questions {
        let req = AnswerRequest {
            question: q.to_string(),
            grounding: grounding.clone(),
            task: TaskKind::ShortAnswer,
            max_output_tokens: 64,
            preferred_tier: None,
        };
        let start = std::time::Instant::now();
        match backend.answer(&req).await {
            Ok(resp) => println!(
                "Q: {q}\n  A: {}\n  (tier={:?}, {}ms)\n",
                resp.text.trim(),
                resp.tier,
                start.elapsed().as_millis()
            ),
            Err(e) => println!("Q: {q}\n  ERROR: {e}\n"),
        }
    }

    Ok(())
}
