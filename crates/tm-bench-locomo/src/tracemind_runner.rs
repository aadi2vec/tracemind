//! Real TraceMind runner — ingests LoCoMo conversation turns into a fresh
//! per-sample SQLite store, then queries each question through the
//! [`RetrievalEngine`] and synthesizes an answer string from the top
//! retrieved trace text + entity names (Tier-0 extractive — what ships
//! out of the box, no LLM required).
//!
//! Compiled only with `--features tracemind` because pulling in `tm-ingest`
//! and `tm-retrieval` drags fastembed and its transitive deps. The default
//! build keeps `tm-bench-locomo` lean.

use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;

use crate::dataset::LocomoQuestion;
use crate::runner::{LocomoRunner, RunnerContext};

/// Configuration for the TraceMind runner.
#[derive(Debug, Clone)]
pub struct TraceMindConfig {
    /// When true, use the deterministic hash embedder (no model download).
    /// Required for CI reproducibility; flip off for the publishable
    /// real-embeddings run.
    pub hash_embed: bool,
    /// Cap on retrieved-trace text included in the answer string. SQuAD F1
    /// punishes long predictions on precision, so we keep this tight.
    pub max_answer_chars: usize,
    /// Number of top traces to splice into the answer.
    pub top_k_traces: usize,
}

impl Default for TraceMindConfig {
    fn default() -> Self {
        Self {
            hash_embed: true,
            max_answer_chars: 200,
            top_k_traces: 1,
        }
    }
}

pub struct TraceMindRunner {
    config: TraceMindConfig,
    /// Per-sample temp directory; replaced on every ingest_sample.
    workdir: Option<tempfile::TempDir>,
    pipeline: Option<IngestPipeline>,
    engine: Option<RetrievalEngine>,
    /// Track ingested texts so the extractive synthesizer can fall back to
    /// substring scan when retrieval misses (rare but happens with hash
    /// embeddings on short conversations).
    ingested_turns: Vec<String>,
    name: String,
}

impl TraceMindRunner {
    pub fn new(config: TraceMindConfig) -> Self {
        let name = format!(
            "tracemind-v0.1-{}",
            if config.hash_embed { "hash" } else { "bge" }
        );
        Self {
            config,
            workdir: None,
            pipeline: None,
            engine: None,
            ingested_turns: Vec::new(),
            name,
        }
    }

    /// Open a fresh ingest+retrieval pair backed by a new temp directory.
    fn reset_stores(&mut self) -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let db_path: PathBuf = dir.path().join("graph.db");
        let trace_path: PathBuf = dir.path().join("traces.jsonl");

        let pipeline = IngestPipeline::open(
            db_path.to_string_lossy().as_ref(),
            self.config.hash_embed,
        )
        .map_err(|e| format!("ingest open: {e:?}"))?;

        let engine = RetrievalEngine::open(
            db_path.to_string_lossy().as_ref(),
            trace_path.to_string_lossy().as_ref(),
            self.config.hash_embed,
        )
        .map_err(|e| format!("retrieval open: {e:?}"))?;

        self.workdir = Some(dir);
        self.pipeline = Some(pipeline);
        self.engine = Some(engine);
        self.ingested_turns.clear();
        Ok(())
    }

    /// Synthesize a Tier-0 extractive answer from a retrieval result.
    ///
    /// Strategy (in priority order):
    /// 1. Top-`k` retrieved trace `raw_text` joined and truncated.
    /// 2. Top entity names joined.
    /// 3. Substring scan of ingested turns for any word from the question
    ///    (cheap fallback — catches short factual lookups that hash
    ///    embeddings often miss).
    /// 4. Empty string (the SQuAD F1 will be 0).
    fn synthesize(
        &self,
        result: &tm_retrieval::engine::RetrievalResult,
        question: &str,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        for trace in result.traces.iter().take(self.config.top_k_traces) {
            if let Some(raw) = &trace.raw_text {
                parts.push(strip_skipped_marker(raw));
            }
        }

        if parts.is_empty() {
            for ent in result.entities.iter().take(3) {
                parts.push(ent.name.clone());
            }
        }

        if parts.is_empty() {
            if let Some(hit) = self.substring_fallback(question) {
                parts.push(hit);
            }
        }

        let joined = parts.join(" ");
        truncate(&joined, self.config.max_answer_chars)
    }

    /// Cheap last-resort fallback: scan the ingested turns for a turn that
    /// shares a content word with the question. Picks the longest match by
    /// shared-token count.
    fn substring_fallback(&self, question: &str) -> Option<String> {
        let q_tokens: Vec<String> = question
            .to_lowercase()
            .split_whitespace()
            .filter(|t| t.len() > 3)
            .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if q_tokens.is_empty() {
            return None;
        }
        self.ingested_turns
            .iter()
            .map(|turn| {
                let lower = turn.to_lowercase();
                let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
                (overlap, turn.clone())
            })
            .filter(|(o, _)| *o > 0)
            .max_by_key(|(o, _)| *o)
            .map(|(_, turn)| turn)
    }
}

fn strip_skipped_marker(raw: &str) -> String {
    // The ingest pipeline prefixes skipped texts with `[SKIPPED: <reason>] `.
    // Strip it so the synthesizer doesn't include the marker in the answer.
    if raw.starts_with("[SKIPPED:") {
        if let Some(idx) = raw.find("] ") {
            return raw[idx + 2..].to_string();
        }
    }
    raw.to_string()
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.chars().take(max_chars).collect()
}

#[async_trait]
impl LocomoRunner for TraceMindRunner {
    fn name(&self) -> &str {
        &self.name
    }

    async fn ingest_sample(&mut self, ctx: &RunnerContext<'_>) -> Result<(), String> {
        self.reset_stores()?;
        let pipeline = self.pipeline.as_ref().expect("pipeline open");
        let session_id = Uuid::new_v4();

        for session in &ctx.sample.sessions {
            for turn in &session.turns {
                // Format: "Speaker (date): text" — gives the embedder some
                // structure and preserves attribution for the synthesizer.
                let formatted = match &session.date {
                    Some(d) => format!("{} ({}): {}", turn.speaker, d, turn.text),
                    None => format!("{}: {}", turn.speaker, turn.text),
                };
                if let Err(e) = pipeline.ingest(&formatted, session_id) {
                    tracing::warn!(?e, "ingest turn failed; continuing");
                    continue;
                }
                self.ingested_turns.push(formatted);
            }
        }
        Ok(())
    }

    async fn answer(
        &mut self,
        _ctx: &RunnerContext<'_>,
        question: &LocomoQuestion,
    ) -> Result<String, String> {
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| "engine not initialised".to_string())?;
        let result = engine
            .query(&question.question)
            .map_err(|e| format!("query: {e:?}"))?;
        Ok(self.synthesize(&result, &question.question))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_skipped_marker_removes_prefix() {
        let raw = "[SKIPPED: too short] hi there";
        assert_eq!(strip_skipped_marker(raw), "hi there");
    }

    #[test]
    fn strip_skipped_marker_passes_through_normal_text() {
        let raw = "Alice: hello there";
        assert_eq!(strip_skipped_marker(raw), "Alice: hello there");
    }

    #[test]
    fn truncate_caps_chars() {
        let s = "x".repeat(500);
        assert_eq!(truncate(&s, 100).chars().count(), 100);
    }
}
