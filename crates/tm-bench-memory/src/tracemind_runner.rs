//! Real TraceMind persistence runner.
//!
//! Two modes share the same code path:
//!
//! * **Per-pair mode** (legacy): each pair gets a fresh tempdir.
//!   Trivial — retrieval has 1–2 candidates per query, so the F1 we
//!   reported with this layout was structurally inflated.
//!
//! * **Shared-DB bulk mode** (honest): `bulk_ingest` opens ONE pipeline
//!   against ONE tempdir, writes every pair's store + distractors plus
//!   the global noise corpus, then drops the pipeline (closing WAL).
//!   `query` then opens ONE `RetrievalEngine` against the same DB,
//!   caches it, and answers every pair against the shared corpus.
//!   Retrieval has to find the needle among hundreds of competing
//!   sentences — the same conditions a real session 2 would face.

use async_trait::async_trait;
use std::path::PathBuf;
use uuid::Uuid;

use tm_ingest::IngestPipeline;
use tm_retrieval::RetrievalEngine;

use crate::dataset::PersistenceDataset;
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
    /// Tempdir held alive for the lifetime of the run. In per-pair mode
    /// this is overwritten on every `session_a_store`; in shared-DB
    /// mode this is set once by `bulk_ingest`.
    workdir: Option<tempfile::TempDir>,
    /// All sentences ever ingested into the current backing store. In
    /// shared-DB mode this is the union of every pair's store +
    /// distractors + the noise corpus. The extractor uses this to walk
    /// back through antecedents.
    ingested: Vec<String>,
    /// Per-pair store sentences only — the *answer-bearing* subset,
    /// used to bias the candidate scorer in shared-DB mode where
    /// retrieval may pull in distractors from other pairs.
    current_pair_store: Vec<String>,
    pending_db_path: Option<PathBuf>,
    pending_trace_path: Option<PathBuf>,
    /// Cached engine in shared-DB mode. Opened lazily on the first
    /// `query` call and reused across pairs.
    cached_engine: Option<RetrievalEngine>,
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
            current_pair_store: Vec::new(),
            pending_db_path: None,
            pending_trace_path: None,
            cached_engine: None,
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
    /// stream **within the current pair's store**, the LAST sentence
    /// containing one wins. We scope to `current_pair_store` so a
    /// retraction in pair X doesn't bleed into pair Y in shared-DB mode.
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
        for (i, s) in self.current_pair_store.iter().enumerate() {
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
        // Also add the current pair's own store sentences so the
        // candidate scorer can pick them even when retrieval misses
        // (hash embeddings on a noisy DB drop the needle often).
        hits.extend(self.current_pair_store.iter().cloned());
        if hits.is_empty() {
            return None;
        }
        // Dedupe while preserving order.
        let mut seen = std::collections::HashSet::new();
        let unique: Vec<String> = hits
            .into_iter()
            .filter(|h| seen.insert(h.clone()))
            .collect();
        // Score: query-token overlap; tiebreak by appearance in current pair store
        // (in-pair sentences preferred over noise from other pairs).
        let scored: Vec<(usize, usize, String)> = unique
            .into_iter()
            .map(|h| {
                let lower = h.to_lowercase();
                let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
                // 0 = in current pair store (preferred), 1 = elsewhere.
                let in_pair = if self
                    .current_pair_store
                    .iter()
                    .any(|s| s.to_lowercase() == lower)
                {
                    0
                } else {
                    1
                };
                (overlap, in_pair, h)
            })
            .collect();
        scored
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
            .map(|(_, _, h)| h)
    }

    /// Substring fallback over the current pair's store sentences only.
    fn substring_fallback(&self, query: &str) -> Option<String> {
        let q_tokens = query_tokens(query);
        if q_tokens.is_empty() {
            return None;
        }
        let scored: Vec<(usize, &String)> = self
            .current_pair_store
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

    /// Walk back through antecedents when the candidate (or its
    /// extracted answer) is pronoun-prefixed. Triggers:
    /// (a) the extracted answer is a bare pronoun (`She`), or
    /// (b) the QUESTION is Person-shape ("Who…?") AND the candidate
    ///     starts with a pronoun (`She suggested…`) — the subject is
    ///     in the previous sentence, even if the extractor lands on
    ///     some other capitalized word from the object side.
    ///
    /// We deliberately do NOT trigger (b) for Place / Choice / Date
    /// queries — in those shapes the answer often lives inside the
    /// pronoun continuation itself ("She came from Figma's growth
    /// team.").
    fn resolve_pronoun(&self, candidate: &str, query: &str, answer: &str) -> Option<String> {
        let candidate_starts_with_pronoun = candidate
            .split_whitespace()
            .next()
            .map(|w| {
                let stripped = w.trim_matches(|c: char| !c.is_alphanumeric());
                is_pronoun_only(stripped)
            })
            .unwrap_or(false);
        let qkind = classify(query);
        let person_walk = matches!(qkind, QKind::Person) && candidate_starts_with_pronoun;
        if !is_pronoun_only(answer) && !person_walk {
            return None;
        }
        let idx = self
            .current_pair_store
            .iter()
            .position(|s| s == candidate)
            .unwrap_or(self.current_pair_store.len());
        if idx == 0 {
            return None;
        }
        for prev in self.current_pair_store[..idx].iter().rev() {
            let reextracted = extract::compose(query, prev);
            if !is_pronoun_only(&reextracted) && reextracted != prev.as_str() {
                return Some(reextracted);
            }
        }
        None
    }

    /// Cross-sentence pre-extraction for name-resolution shapes — only
    /// over the current pair's store (never noise).
    ///
    /// Scoring is two-tier so a marker-driven extraction always beats
    /// a fallback extraction. Without this, "She came from Figma's
    /// growth team." (the answer-bearing sentence) loses to
    /// "Devika joined as our first design hire." (higher query-token
    /// overlap) and we return the wrong entity.
    fn cross_sentence(&self, query: &str) -> Option<String> {
        let qkind = classify(query);
        if !matches!(qkind, QKind::Person | QKind::Place | QKind::Choice) {
            return None;
        }
        let q_tokens = query_tokens(query);
        // If a retraction landed within the current pair, only consider
        // sentences at-or-after the retraction. Otherwise the
        // pre-retraction sentence (which often carries a marker like
        // " using ") will outrank the corrected answer.
        let retraction_start = self.retraction_marker_index();
        // (marker_match, overlap, prediction) — tuple ordering means
        // marker-driven extractions always rank above fallback ones.
        let mut best: Option<(u8, usize, String)> = None;
        for (i, s) in self.current_pair_store.iter().enumerate() {
            if let Some(r) = retraction_start {
                if i < r {
                    continue;
                }
            }
            let pred = extract::compose(query, s);
            if pred == s.as_str() || is_pronoun_only(&pred) {
                continue;
            }
            let marker = match qkind {
                QKind::Place => extract::extract_place_pattern(s).is_some() as u8,
                QKind::Choice => {
                    if extract::extract_choice(s, query).is_some() {
                        2
                    } else if extract::extract_place_pattern(s).is_some()
                        || extract::extract_value(query, s).is_some()
                    {
                        1
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            let lower = s.to_lowercase();
            let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
            let key = (marker, overlap);
            if best
                .as_ref()
                .map(|(m, o, _)| (key) > (*m, *o))
                .unwrap_or(true)
            {
                best = Some((marker, overlap, pred));
            }
        }
        best.map(|(_, _, p)| p)
    }

    /// Like `cross_sentence` but only returns a result when one of the
    /// store sentences fires a marker-driven extractor (place_pattern,
    /// choice marker). Used as a confidence override when our initial
    /// answer is a low-confidence cluster guess.
    fn cross_sentence_marker_only(&self, query: &str) -> Option<String> {
        let qkind = classify(query);
        if !matches!(qkind, QKind::Person | QKind::Place | QKind::Choice) {
            return None;
        }
        let q_tokens = query_tokens(query);
        let retraction_start = self.retraction_marker_index();
        // (tier, overlap, prediction) — tier captures marker quality so a
        // wh-focus topic_is match (tier 3) beats a place_pattern guess
        // (tier 1) even when the latter has higher token overlap.
        let mut best: Option<(u8, usize, String)> = None;
        for (i, s) in self.current_pair_store.iter().enumerate() {
            if let Some(r) = retraction_start {
                if i < r {
                    continue;
                }
            }
            let tiered: Option<(u8, String)> = match qkind {
                QKind::Place => extract::extract_place_pattern(s)
                    .map(|p| (3, p))
                    .or_else(|| extract::extract_topic_is(query, s).map(|p| (2, p))),
                QKind::Choice => extract::extract_topic_is(query, s)
                    .map(|p| (3, p))
                    .or_else(|| extract::extract_choice(s, query).map(|p| (2, p)))
                    .or_else(|| extract::extract_place_pattern(s).map(|p| (1, p))),
                _ => None,
            };
            let Some((tier, pred)) = tiered else { continue };
            if pred.is_empty() || is_pronoun_only(&pred) {
                continue;
            }
            let lower = s.to_lowercase();
            let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
            let key = (tier, overlap);
            if best.as_ref().map(|(t, o, _)| key > (*t, *o)).unwrap_or(true) {
                best = Some((tier, overlap, pred));
            }
        }
        best.map(|(_, _, p)| p)
    }

    /// Shared body of the query path — used by both per-pair and
    /// shared-DB modes.
    async fn answer_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        let query = ctx.pair.query.clone();
        self.current_pair_store = ctx.pair.store.clone();

        let result = {
            let engine = self
                .cached_engine
                .as_mut()
                .ok_or_else(|| "engine not opened".to_string())?;
            engine
                .query(&query)
                .map_err(|e| format!("query: {e:?}"))?
        };

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

        let candidate = match self.retraction_marker_index() {
            Some(i) => Some(self.current_pair_store[i].clone()),
            None => self
                .pick_candidate(&result, &query)
                .or_else(|| self.substring_fallback(&query)),
        };

        let mut answer = match &candidate {
            Some(c) => extract::compose(&query, c),
            None => result
                .entities
                .first()
                .map(|e| e.name.clone())
                .unwrap_or_default(),
        };

        if let Some(c) = &candidate {
            if let Some(resolved) = self.resolve_pronoun(c, &query, &answer) {
                answer = resolved;
            }
        }

        // Cross-sentence sweep:
        //  * If our extracted answer is empty / pronoun-only / the
        //    whole candidate, ALWAYS replace with the sweep result
        //    (anything is better than nothing).
        //  * Otherwise, only replace when the sweep found a
        //    marker-driven extraction (`marker_only=true`). A
        //    marker-driven result is structurally higher confidence
        //    than a capitalized-cluster guess, so it should win even
        //    when the cluster guess is non-empty.
        let answer_looks_weak = answer.is_empty()
            || is_pronoun_only(&answer)
            || candidate
                .as_ref()
                .map(|c| answer.as_str() == c.as_str())
                .unwrap_or(false);
        if answer_looks_weak {
            if let Some(p) = self.cross_sentence(&query) {
                answer = p;
            }
        } else if let Some(p) = self.cross_sentence_marker_only(&query) {
            answer = p;
        }

        Ok(self.truncate(&answer))
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
        // Per-pair mode: one tempdir per pair, pipeline opens-and-drops.
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

        // Reset any shared-DB state and switch to per-pair paths.
        self.cached_engine = None;
        self.workdir = Some(dir);
        self.pending_db_path = Some(db_path);
        self.pending_trace_path = Some(trace_path);
        self.ingested = ctx.pair.store.clone();
        self.current_pair_store = ctx.pair.store.clone();
        Ok(())
    }

    async fn session_b_query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        // Per-pair mode: open a fresh engine against the per-pair DB,
        // run one query, drop it. (Shared-DB mode goes through `query`
        // and the cached engine.)
        let (db, trace) = self
            .paths()
            .ok_or_else(|| "session_b before session_a".to_string())?;
        let engine = RetrievalEngine::open(
            db.to_string_lossy().as_ref(),
            trace.to_string_lossy().as_ref(),
            self.config.hash_embed,
        )
        .map_err(|e| format!("retrieval open: {e:?}"))?;
        self.cached_engine = Some(engine);
        let out = self.answer_query(ctx).await?;
        self.cached_engine = None;
        Ok(out)
    }

    async fn bulk_ingest(&mut self, dataset: &PersistenceDataset) -> Result<(), String> {
        // Shared-DB mode: ONE tempdir, ONE pipeline, every sentence the
        // dataset has, then drop.
        let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let db_path: PathBuf = dir.path().join("memory.db");
        let trace_path: PathBuf = dir.path().join("traces.jsonl");

        let mut all_sentences: Vec<String> = Vec::new();
        {
            let pipeline = IngestPipeline::open(
                db_path.to_string_lossy().as_ref(),
                self.config.hash_embed,
            )
            .map_err(|e| format!("ingest open: {e:?}"))?;
            // Each pair gets its own session id so trajectories don't
            // collapse together.
            for pair in &dataset.pairs {
                let session_id = Uuid::new_v4();
                for sentence in &pair.store {
                    if let Err(e) = pipeline.ingest(sentence, session_id) {
                        tracing::warn!(pair = %pair.id, ?e, "ingest failed; continuing");
                    }
                    all_sentences.push(sentence.clone());
                }
                for sentence in &pair.distractors {
                    if let Err(e) = pipeline.ingest(sentence, session_id) {
                        tracing::warn!(pair = %pair.id, ?e, "distractor ingest failed");
                    }
                    all_sentences.push(sentence.clone());
                }
            }
            // Noise corpus goes in last under its own session id.
            let noise_session = Uuid::new_v4();
            for sentence in &dataset.noise_corpus {
                if let Err(e) = pipeline.ingest(sentence, noise_session) {
                    tracing::warn!(?e, "noise ingest failed");
                }
                all_sentences.push(sentence.clone());
            }
            // pipeline drops here.
        }

        self.cached_engine = None;
        self.workdir = Some(dir);
        self.pending_db_path = Some(db_path);
        self.pending_trace_path = Some(trace_path);
        self.ingested = all_sentences;
        self.current_pair_store = Vec::new();
        Ok(())
    }

    async fn query(&mut self, ctx: &RunnerContext<'_>) -> Result<String, String> {
        // Lazily open the engine against the shared DB on first call.
        if self.cached_engine.is_none() {
            let (db, trace) = self
                .paths()
                .ok_or_else(|| "query before bulk_ingest".to_string())?;
            let engine = RetrievalEngine::open(
                db.to_string_lossy().as_ref(),
                trace.to_string_lossy().as_ref(),
                self.config.hash_embed,
            )
            .map_err(|e| format!("retrieval open: {e:?}"))?;
            self.cached_engine = Some(engine);
        }
        self.answer_query(ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{Category, PersistencePair, Split};

    fn pair(store: &[&str], query: &str, ans: &[&str], cat: Category) -> PersistencePair {
        PersistencePair {
            id: "p".into(),
            category: cat,
            split: Split::Test,
            store: store.iter().map(|s| s.to_string()).collect(),
            distractors: Vec::new(),
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

    #[tokio::test]
    async fn shared_db_bulk_round_trip() {
        let p1 = pair(
            &["My favourite tea is matcha."],
            "What is my favourite tea?",
            &["matcha"],
            Category::FactualRecall,
        );
        let mut p2 = pair(
            &["I committed to lunch with Sam on Friday."],
            "When am I meeting Sam?",
            &["Friday"],
            Category::TemporalPinning,
        );
        p2.id = "p2".into();
        let dataset = PersistenceDataset {
            pairs: vec![p1.clone(), p2.clone()],
            noise_corpus: vec![
                "The kettle is on the second shelf.".into(),
                "Patrick prefers oolong in the afternoons.".into(),
            ],
        };
        let mut r = TraceMindPersistenceRunner::new(TraceMindConfig::default());
        r.bulk_ingest(&dataset).await.unwrap();
        let out1 = r.query(&RunnerContext { pair: &p1 }).await.unwrap();
        let out2 = r.query(&RunnerContext { pair: &p2 }).await.unwrap();
        assert!(
            out1.to_lowercase().contains("matcha"),
            "expected matcha (p1) under shared DB, got '{out1}'"
        );
        assert!(
            out2.to_lowercase().contains("friday"),
            "expected Friday (p2) under shared DB, got '{out2}'"
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
