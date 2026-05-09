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
    /// Strategy (v0.2, in priority order):
    /// 1. Walk top `signal_hits` (the unpromoted-signal hybrid retrieval
    ///    path; populated regardless of bandit arm and always carries the
    ///    raw ingested turn text in `.text`). Then fall back to
    ///    `result.traces` (only arm 3 populates it but include for
    ///    completeness).
    /// 2. For each candidate:
    ///    - If turn is a question (`?`-ending), substitute the next
    ///      ingested turn (the answer in a Q→A dialogue), else skip.
    ///    - Strip `Speaker (date): ` prefix to keep prediction tokens
    ///      tight (the speaker prefix dilutes SQuAD F1 precision).
    /// 3. If no candidate survives, fall back to top entity names.
    /// 4. Substring scan of ingested turns as last-resort fallback.
    /// 5. Empty string (SQuAD F1 will be 0).
    fn synthesize(
        &self,
        result: &tm_retrieval::engine::RetrievalResult,
        question: &str,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        // Primary candidate stack: signal hits (raw ingested turns,
        // populated for every arm). Secondary: episodic traces (arm 3).
        let mut candidates: Vec<String> =
            result.signal_hits.iter().map(|s| s.text.clone()).collect();
        candidates.extend(
            result
                .traces
                .iter()
                .filter_map(|t| t.raw_text.clone()),
        );

        for raw in candidates.iter().take(8) {
            let cleaned = strip_skipped_marker(raw);
            // If the retrieved turn is a question, swap in the next turn
            // (the answer). If no successor, skip and keep walking.
            let candidate = if is_question_turn(&cleaned) {
                match self.next_turn_after(&cleaned) {
                    Some(next) if !is_question_turn(&next) => next,
                    _ => continue,
                }
            } else {
                cleaned
            };
            let stripped = strip_speaker_prefix(&candidate).to_string();
            parts.push(stripped);
            break;
        }

        if parts.is_empty() {
            for ent in result.entities.iter().take(3) {
                parts.push(ent.name.clone());
            }
        }

        if parts.is_empty() {
            if let Some(hit) = self.substring_fallback(question) {
                parts.push(strip_speaker_prefix(&hit).to_string());
            }
        }

        let joined = parts.join(" ");
        truncate(&joined, self.config.max_answer_chars)
    }

    /// Find the next ingested turn after the one that matches `text`.
    ///
    /// The retrieval layer may return signal/trace text with the speaker
    /// prefix already stripped, while `ingested_turns` stores the
    /// `Speaker (date): text` format. We match by suffix (case-insensitive)
    /// so either form resolves to the right position.
    fn next_turn_after(&self, text: &str) -> Option<String> {
        let needle = strip_speaker_prefix(text).trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        let pos = self.ingested_turns.iter().position(|t| {
            let lower = strip_speaker_prefix(t).trim().to_lowercase();
            lower == needle || lower.ends_with(&needle) || needle.ends_with(&lower)
        })?;
        // Window the Q→A adjacency to next 1–3 turns (not just next turn).
        // Fixes cases like "What was Ethan's finish time?" where the answer
        // came 2 turns later, not 1.
        for offset in 1..=3 {
            if let Some(turn) = self.ingested_turns.get(pos + offset) {
                if !is_question_turn(turn) {
                    return Some(turn.clone());
                }
            }
        }
        None
    }

    /// Token-overlap retrieval over the ingested turns.
    ///
    /// In v0.2 this is the primary retrieval path — the underlying
    /// `RetrievalEngine` returns empty signal/trace/entity sets for the
    /// LoCoMo mini fixtures because the cosine threshold (lowered to 0.2
    /// in v0.3) and confidence gate are high for 15-turn conversational
    /// input against a fresh DB. Token overlap on the small per-sample buffer
    /// is fast and reliable; the engine path will start contributing
    /// once warm-up data accumulates.
    ///
    /// Algorithm:
    /// 1. Stem-light token bag from question (drop articles + stopwords,
    ///    keep tokens with >2 alphanumerics).
    /// 2. Score every ingested turn by overlap, ties broken by ingest
    ///    recency (later turns = later position).
    /// 3. If the winner is a question turn, return the next ingested
    ///    turn (the answering turn in a Q→A exchange).
    /// 4. None if no turn shares any content token.
    fn substring_fallback(&self, question: &str) -> Option<String> {
        let q_tokens = question_tokens(question);
        if q_tokens.is_empty() {
            return None;
        }
        let scored: Vec<(usize, usize, &String)> = self
            .ingested_turns
            .iter()
            .enumerate()
            .map(|(idx, turn)| {
                // Strip speaker prefix before scoring — otherwise every
                // turn by the question's subject scores +1 spuriously.
                let body = strip_speaker_prefix(turn).to_lowercase();
                // Substring containment lets "rename" match "renamed",
                // "fly" match "flying" — important for verb morphology
                // when there's no stemmer.
                let overlap = q_tokens
                    .iter()
                    .filter(|t| body.contains(t.as_str()))
                    .count();
                (overlap, idx, turn)
            })
            .filter(|(o, _, _)| *o > 0)
            .collect();
        // Tie-break: at equal overlap, *prefer question turns* — they
        // signal a Q→A adjacency where the next turn is the answer.
        // Then prefer later ingest position (more recent state).
        let best = scored.into_iter().max_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| {
                    let aq = is_question_turn(a.2);
                    let bq = is_question_turn(b.2);
                    aq.cmp(&bq) // true > false → question wins
                })
                .then_with(|| a.1.cmp(&b.1))
        })?;
        let (_, idx, turn) = best;
        // If the winner is a question turn, return the next turn — the
        // answer in a Q→A dialogue. Skip further questions.
        if is_question_turn(turn) {
            for next in self.ingested_turns.iter().skip(idx + 1).take(3) {
                if !is_question_turn(next) {
                    return Some(next.clone());
                }
            }
            return None;
        }
        Some(turn.clone())
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

/// Strip a leading `Speaker (date): ` or `Speaker: ` prefix from a turn.
///
/// Heuristic: if the first `: ` separator appears within the first 40
/// characters and the candidate prefix has no terminal punctuation
/// (`.?!`), treat it as a speaker label and drop it. Otherwise return
/// the input untouched. This is intentionally conservative — we only
/// strip what we ingested ourselves.
fn strip_speaker_prefix(s: &str) -> &str {
    let head = match s.char_indices().take(40).last() {
        Some((i, c)) => i + c.len_utf8(),
        None => return s,
    };
    let head = &s[..head.min(s.len())];
    if let Some(idx) = head.find(": ") {
        let prefix = &s[..idx];
        if !prefix.contains(['.', '?', '!']) {
            return &s[idx + 2..];
        }
    }
    s
}

/// Whether a turn (with or without speaker prefix) ends with a question
/// mark — the signal we use to detect Q-turns and substitute the
/// answering turn instead.
fn is_question_turn(s: &str) -> bool {
    strip_speaker_prefix(s).trim_end().ends_with('?')
}

/// Interrogatives + auxiliaries we strip from the question token bag.
/// Content-words like "about", "from", "with", "into" stay — they
/// often anchor the answer turn.
const STOPWORDS: &[&str] = &[
    "what", "when", "where", "which", "who", "whom", "whose", "why", "how",
    "did", "does", "was", "were", "are", "is", "the", "and", "have", "has",
    "had", "that", "this",
];

/// Build a content-token bag from a question for overlap scoring.
///
/// We split on whitespace, drop punctuation at edges and apostrophes
/// inside the token (so `"alice's"` → `"alices"` won't help, but
/// stripping the trailing `'s` exposes the bare noun for substring
/// matching). Stopwords/auxiliaries are filtered.
fn question_tokens(question: &str) -> Vec<String> {
    question
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .map(|t| {
            // Drop possessive 's so "alice's" → "alice".
            t.strip_suffix("'s").map(str::to_string).unwrap_or(t)
        })
        .filter(|t| t.len() > 2)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
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
        if std::env::var("TM_BENCH_DEBUG").is_ok() {
            eprintln!(
                "[debug] Q: {} | arm={} signals={} traces={} entities={}",
                question.question,
                result.arm,
                result.signal_hits.len(),
                result.traces.len(),
                result.entities.len()
            );
        }
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

    #[test]
    fn strip_speaker_prefix_removes_dated_prefix() {
        assert_eq!(
            strip_speaker_prefix("Alice (2026-04-01): I'm flying to Tokyo on May 3rd."),
            "I'm flying to Tokyo on May 3rd."
        );
    }

    #[test]
    fn strip_speaker_prefix_removes_simple_prefix() {
        assert_eq!(strip_speaker_prefix("Bob: hello there"), "hello there");
    }

    #[test]
    fn strip_speaker_prefix_passes_through_no_colon() {
        assert_eq!(strip_speaker_prefix("just some text"), "just some text");
    }

    #[test]
    fn strip_speaker_prefix_keeps_sentence_with_terminal_punct_in_head() {
        // If the head before the first `: ` contains terminal punctuation
        // we should *not* treat it as a speaker label.
        let s = "I asked. Then: what's next?";
        assert_eq!(strip_speaker_prefix(s), s);
    }

    #[test]
    fn is_question_turn_detects_question() {
        assert!(is_question_turn("Bob (2026-04-15): What's the talk about?"));
        assert!(is_question_turn("First hire?"));
    }

    #[test]
    fn is_question_turn_rejects_statement() {
        assert!(!is_question_turn(
            "Alice: Local memory systems for consumer apps."
        ));
        assert!(!is_question_turn("Carol: I'm leaving Stripe."));
    }
}
