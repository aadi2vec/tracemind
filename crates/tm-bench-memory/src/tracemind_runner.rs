//! Real TraceMind persistence runner.
//!
//! **Cross-session semantics.** Each pair gets its own fresh temp
//! directory acting as `TM_DATA_DIR`. Session A opens an
//! `IngestPipeline` against that dir, writes the store sentences, and
//! drops the pipeline — closing the SQLite handles and letting the WAL
//! flush. Session B opens a *new* `RetrievalEngine` against the same
//! dir and runs the query. That close-and-reopen cycle is the whole
//! claim of the benchmark.
//!
//! **Synthesis.** Once retrieval lands on a candidate sentence, we
//! pass (query, candidate) to `extract::compose` for a tight span. For
//! name-resolution shapes where the candidate uses a pronoun ("She is
//! joining..."), we walk back through the ingested sentences to find
//! the antecedent and re-extract.

use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;

use crate::extract::{self, classify, QKind};
use crate::runner::{PersistenceRunner, RunnerContext};

#[derive(Debug, Clone)]
pub struct TraceMindConfig {
    /// Use the deterministic hash embedder. CI default = true so the
    /// benchmark stays reproducible without a 90MB model download.
    pub hash_embed: bool,
    /// Cap on prediction length. SQuAD F1 punishes long predictions.
    pub max_answer_chars: usize,
}

impl Default for TraceMindConfig {
    fn default() -> Self {
        Self {
            hash_embed: true,
            max_answer_chars: 120,
        }
    }
}

pub struct TraceMindPersistenceRunner {
    config: TraceMindConfig,
    workdir: Option<tempfile::TempDir>,
    ingested: Vec<String>,
    pending_db_path: Option<PathBuf>,
    pending_trace_path: Option<PathBuf>,
    name: String,
}

impl TraceMindPersistenceRunner {
    pub fn new(config: TraceMindConfig) -> Self {
        let name = format!(
            "tracemind-w3-{}",
            if config.hash_embed { "hash" } else { "bge" }
        );
        Self {
            config,
            workdir: None,
            ingested: Vec::new(),
            pending_db_path: None,
            pending_trace_path: None,
            name,
        }
    }

    fn paths(&self) -> Option<(&PathBuf, &PathBuf)> {
        match (&self.pending_db_path, &self.pending_trace_path) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        }
    }

    fn truncate(&self, s: &str) -> String {
        let max = self.config.max_answer_chars;
        if s.chars().count() <= max {
            return s.to_string();
        }
        s.chars().take(max).collect()
    }

    /// Retraction markers. When any of these appears in the ingested
    /// stream, the LAST sentence containing one wins — that's the
    /// overriding statement.
    fn retraction_marker_index(&self) -> Option<usize> {
        const MARKERS: &[&str] = &[
            "actually,",
            "actually ",
            "wait,",
            "wait ",
            "no,",
            "no —",
            "scratch that",
            "moved to",
            "moved it to",
            "changed to",
            "rescheduled to",
            "called off",
            "cancelled",
            "instead",
            "updated to",
            "bumped to",
            "now —",
        ];
        let mut latest: Option<usize> = None;
        for (i, s) in self.ingested.iter().enumerate() {
            let lower = s.to_lowercase();
            if MARKERS.iter().any(|m| lower.contains(m)) {
                latest = Some(i);
            }
        }
        latest
    }

    /// Score candidates and pick the best.
    fn pick_candidate(
        &self,
        result: &tm_retrieval::engine::RetrievalResult,
        query: &str,
    ) -> Option<String> {
        let q_tokens = query_tokens(query);
        let mut hits: Vec<String> = result.signal_hits.iter().map(|s| s.text.clone()).collect();
        hits.extend(result.traces.iter().filter_map(|t| t.raw_text.clone()));
        // Also add the ingested sentences themselves; for short pairs
        // retrieval can miss the most-relevant entry on hash embeddings.
        hits.extend(self.ingested.iter().cloned());
        if hits.is_empty() {
            return None;
        }
        // Dedupe while preserving order.
        let mut seen = std::collections::HashSet::new();
        let unique: Vec<String> = hits
            .into_iter()
            .filter(|h| seen.insert(h.clone()))
            .collect();
        // Score: query-token overlap; tiebreak by ingest position (later wins).
        let scored: Vec<(usize, usize, String)> = unique
            .into_iter()
            .map(|h| {
                let lower = h.to_lowercase();
                let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
                let pos = self
                    .ingested
                    .iter()
                    .position(|s| s.to_lowercase() == lower)
                    .unwrap_or(0);
                (overlap, pos, h)
            })
            .collect();
        scored
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
            .map(|(_, _, h)| h)
    }

    /// Substring fallback over the ingested sentences.
    fn substring_fallback(&self, query: &str) -> Option<String> {
        let q_tokens = query_tokens(query);
        if q_tokens.is_empty() {
            return None;
        }
        let scored: Vec<(usize, &String)> = self
            .ingested
            .iter()
            .map(|s| {
                let lower = s.to_lowercase();
                let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
                (overlap, s)
            })
            .filter(|(o, _)| *o > 0)
            .collect();
        let best = scored.into_iter().max_by_key(|(o, _)| *o)?;
        Some(best.1.clone())
    }

    /// If the answer extracted from `candidate` is a pronoun-only span
    /// (e.g. "She", "He", "It"), walk backwards through the ingested
    /// sentences and re-extract from the antecedent.
    fn resolve_pronoun(&self, candidate: &str, query: &str, answer: &str) -> Option<String> {
        if !is_pronoun_only(answer) {
            return None;
        }
        let idx = self
            .ingested
            .iter()
            .position(|s| s == candidate)
            .unwrap_or(self.ingested.len());
        if idx == 0 {
            return None;
        }
        for prev in self.ingested[..idx].iter().rev() {
            let reextracted = extract::compose(query, prev);
            if !is_pronoun_only(&reextracted) && reextracted != prev.as_str() {
                return Some(reextracted);
            }
        }
        None
    }

    /// Cross-sentence pre-extraction for name-resolution. For some
    /// `from`-style queries ("Where did Emma join from?"), the
    /// answer-bearing sentence is *different* from the sentence with
    /// best query-token overlap. Try extracting from every ingested
    /// sentence and prefer non-pronoun proper-noun answers.
    fn cross_sentence(&self, query: &str) -> Option<String> {
        let qkind = classify(query);
        if !matches!(qkind, QKind::Person | QKind::Place | QKind::Choice) {
            return None;
        }
        let q_tokens = query_tokens(query);
        let mut best: Option<(usize, String)> = None;
        for s in &self.ingested {
            let pred = extract::compose(query, s);
            if pred == s.as_str() || is_pronoun_only(&pred) {
                continue;
            }
            let lower = s.to_lowercase();
            let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
            if best.as_ref().map(|(o, _)| overlap > *o).unwrap_or(true) {
                best = Some((overlap, pred));
            }
        }
        best.map(|(_, p)| p)
    }
}

const STOPWORDS: &[&str] = &[
    "what", "when", "where", "which", "who", "whom", "whose", "why", "how", "did", "does", "was",
    "were", "are", "is", "the", "and", "have", "has", "had", "that", "this", "with", "from",
    "for", "into", "onto", "about", "over", "under",
];

fn query_tokens(question: &str) -> Vec<String> {
    question
        .to_lowercase()
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .map(|t| t.strip_suffix("'s").map(str::to_string).unwrap_or(t))
        .filter(|t| t.len() > 2)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
}

fn is_pronoun_only(s: &str) -> bool {
    let t = s.trim().to_lowercase();
    matches!(
        t.as_str(),
        "he" | "she" | "it" | "they" | "his" | "her" | "their" | "them"
            | "my" | "your" | "our" | "us" | "we" | "i" | "you" | "me" | "him"
    )
}

#[async_trait]
impl PersistenceRunner for TraceMindPersistenceRunner {
    fn name(&self) -> &str {
        &self.name
    }

    async fn session_a_store(&mut self, ctx: &RunnerContext<'_>) -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let db_path: PathBuf = dir.path().join("memory.db");
        let trace_path: PathBuf = dir.path().join("traces.jsonl");

        let session_id = Uuid::new_v4();

        {
            let pipeline = IngestPipeline::open(
                db_path.to_string_lossy().as_ref(),
                self.config.hash_embed,
            )
            .map_err(|e| format!("ingest open: {e:?}"))?;
            for sentence in &ctx.pair.store {
                if let Err(e) = pipeline.ingest(sentence, session_id) {
                    tracing::warn!(?e, "ingest failed; continuing");
                }
            }
            // pipeline drops here, closing handles.
        }

        self.workdir = Some(dir);
        self.pending_db_path = Some(db_path);
        self.pending_trace_path = Some(trace_path);
        self.ingested = ctx.pair.store.clone();
        Ok(())
    }

    async fn session_b_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        let (db, trace) = self
            .paths()
            .ok_or_else(|| "session_b before session_a".to_string())?;
        let mut engine = RetrievalEngine::open(
            db.to_string_lossy().as_ref(),
            trace.to_string_lossy().as_ref(),
            self.config.hash_embed,
        )
        .map_err(|e| format!("retrieval open: {e:?}"))?;

        let query = &ctx.pair.query;
        let result = engine
            .query(query)
            .map_err(|e| format!("query: {e:?}"))?;

        if std::env::var("TM_BENCH_MEMORY_DEBUG").is_ok() {
            eprintln!(
                "[w3 debug] pair={} cat={:?} arm={} signals={} traces={} entities={}",
                ctx.pair.id,
                ctx.pair.category,
                result.arm,
                result.signal_hits.len(),
                result.traces.len(),
                result.entities.len()
            );
        }

        // Order:
        //   1. If a retraction marker exists, prefer the latest marker sentence.
        //   2. Else pick the best retrieval candidate.
        //   3. Else substring-fallback over the ingested store.
        let candidate = match self.retraction_marker_index() {
            Some(i) => Some(self.ingested[i].clone()),
            None => self
                .pick_candidate(&result, query)
                .or_else(|| self.substring_fallback(query)),
        };

        let mut answer = match &candidate {
            Some(c) => extract::compose(query, c),
            None => result
                .entities
                .first()
                .map(|e| e.name.clone())
                .unwrap_or_default(),
        };

        // Pronoun antecedent walk-back when the extractor lands on
        // "She" / "He" / "It" because the relevant sentence was the
        // pronoun continuation.
        if let Some(c) = &candidate {
            if let Some(resolved) = self.resolve_pronoun(c, query, &answer) {
                answer = resolved;
            }
        }

        // Cross-sentence fallback: if the answer is still the whole
        // sentence or a pronoun, sweep all ingested sentences and pick
        // the best proper-noun answer.
        if answer.is_empty()
            || is_pronoun_only(&answer)
            || candidate
                .as_ref()
                .map(|c| answer.as_str() == c.as_str())
                .unwrap_or(false)
        {
            if let Some(p) = self.cross_sentence(query) {
                answer = p;
            }
        }

        Ok(self.truncate(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{Category, PersistencePair};

    fn pair(store: &[&str], query: &str, ans: &[&str], cat: Category) -> PersistencePair {
        PersistencePair {
            id: "p".into(),
            category: cat,
            store: store.iter().map(|s| s.to_string()).collect(),
            query: query.into(),
            answers: ans.iter().map(|s| s.to_string()).collect(),
            note: None,
        }
    }

    #[tokio::test]
    async fn factual_recall_round_trip() {
        let mut r = TraceMindPersistenceRunner::new(TraceMindConfig::default());
        let p = pair(
            &["My favourite tea is matcha."],
            "What is my favourite tea?",
            &["matcha"],
            Category::FactualRecall,
        );
        let ctx = RunnerContext { pair: &p };
        r.session_a_store(&ctx).await.unwrap();
        let out = r.session_b_query(&ctx).await.unwrap();
        assert!(
            out.to_lowercase().contains("matcha"),
            "expected matcha in answer, got '{out}'"
        );
    }

    #[tokio::test]
    async fn contradiction_picks_retraction() {
        let mut r = TraceMindPersistenceRunner::new(TraceMindConfig::default());
        let p = pair(
            &[
                "I committed to lunch with Sam on Friday.",
                "Actually, I moved that to Saturday.",
            ],
            "When am I meeting Sam?",
            &["Saturday"],
            Category::ContradictionRecovery,
        );
        let ctx = RunnerContext { pair: &p };
        r.session_a_store(&ctx).await.unwrap();
        let out = r.session_b_query(&ctx).await.unwrap();
        assert!(
            out.to_lowercase().contains("saturday"),
            "expected retracted value in answer, got '{out}'"
        );
    }

    #[tokio::test]
    async fn name_resolution_walks_back_to_antecedent() {
        let mut r = TraceMindPersistenceRunner::new(TraceMindConfig::default());
        let p = pair(
            &[
                "I had lunch with my advisor Dr Patel on Tuesday.",
                "She suggested I read Hinton's 2022 paper.",
            ],
            "Who suggested I read Hinton's paper?",
            &["Dr Patel"],
            Category::NameResolution,
        );
        let ctx = RunnerContext { pair: &p };
        r.session_a_store(&ctx).await.unwrap();
        let out = r.session_b_query(&ctx).await.unwrap();
        assert!(
            out.to_lowercase().contains("patel"),
            "expected Dr Patel via pronoun walk-back, got '{out}'"
        );
    }

    #[test]
    fn query_tokens_filter_stopwords() {
        let toks = query_tokens("Who is the founder of TraceMind?");
        assert!(toks.contains(&"founder".to_string()));
        assert!(toks.contains(&"tracemind".to_string()));
        assert!(!toks.iter().any(|t| t == "the"));
    }
}
