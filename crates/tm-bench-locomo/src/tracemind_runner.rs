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

use crate::dataset::{LocomoQuestion, LocomoSample};
use crate::extract;
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
    /// When true, synthesize the final answer with the Tier-1 candle LLM
    /// (Qwen2.5-0.5B) over the retrieved grounding, instead of the Tier-0
    /// extractive span picker. Requires the `tier1` feature.
    pub use_tier1: bool,
}

impl Default for TraceMindConfig {
    fn default() -> Self {
        Self {
            hash_embed: true,
            max_answer_chars: 200,
            top_k_traces: 1,
            use_tier1: false,
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
    /// Tier-1 candle LLM backend (lazily constructed when `use_tier1`).
    #[cfg(feature = "tier1")]
    candle: Option<tm_answer::candle_backend::CandleBackend>,
    /// Answer-selection weights, supplied by the active GEPA policy.
    policy: tm_gepa::RetrievalPolicy,
}

impl TraceMindRunner {
    pub fn new(config: TraceMindConfig) -> Self {
        let tier_tag = if config.use_tier1 { "tier1" }
            else if config.hash_embed { "hash" } else { "bge" };
        let name = format!("tracemind-v0.1-{tier_tag}");
        Self {
            #[cfg(feature = "tier1")]
            candle: if config.use_tier1 {
                Some(tm_answer::candle_backend::CandleBackend::default_desktop())
            } else {
                None
            },
            config,
            workdir: None,
            pipeline: None,
            engine: None,
            ingested_turns: Vec::new(),
            name,
            policy: tm_gepa::RetrievalPolicy::default(),
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
        // Carry the retrieval score alongside the text. Selection needs the
        // actual score, not the rank: the gap between rank 0 and rank 6 is
        // often only 0.83 vs 0.65, whereas a rank-derived prior of
        // 1/(1+rank) implies an 7x difference and makes it impossible for
        // better evidence to ever overtake a marginally-better cosine.
        let mut candidates: Vec<(String, f32)> = result
            .signal_hits
            .iter()
            .map(|s| (s.text.clone(), s.score))
            .collect();
        candidates.extend(
            result
                .traces
                .iter()
                .filter_map(|t| t.raw_text.clone().map(|txt| (txt, 0.3))),
        );

        // Answer-aware candidate selection.
        //
        // Retrieval rank alone is not a good answer selector: the turn most
        // similar to "Which airport did Alice fly into?" is the one about
        // flying, while the turn that *answers* it mentions Haneda and
        // little else. So walk the ranked candidates and prefer the
        // highest-ranked one that actually carries an extractable answer of
        // the kind the question demands, falling back to rank order when
        // none does.
        let qkind = extract::classify_question(question);
        let mut fallback: Option<String> = None;
        // (score, rank, text) for every candidate that can answer.
        let mut answerable: Vec<(f32, String)> = Vec::new();

        // Inverse document frequency over the candidate set, so that rare,
        // discriminating question words outweigh common ones. "Why did
        // Carol rename the company?" matches "…leaving Stripe to start a
        // company" and "Renamed Loom to Memex…" on exactly one token each,
        // so unweighted coverage ties and retrieval rank — which is wrong
        // here — decides. "rename" occurs in one candidate and "company" in
        // several, so weighting by IDF breaks the tie correctly.
        let idf = TokenIdf::build(candidates.iter().map(|(t, _)| t.as_str()));

        for (raw, retrieval_score) in candidates.iter().take(12) {
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
            if stripped.trim().is_empty() {
                continue;
            }
            if qkind.is_satisfied_by(&stripped, question) {
                let recency = self.turn_recency(&stripped);
                let score = evidence_score(
                    &stripped,
                    question,
                    *retrieval_score,
                    idf.coverage(question, &stripped),
                    // Causal questions get no help from the answer-type
                    // filter and their answer is a specific clause, so
                    // lexical evidence carries more of the decision there.
                    if matches!(qkind, extract::QKind::Reason) {
                        self.policy.coverage_weight * self.policy.generic_coverage_boost
                    } else {
                        self.policy.coverage_weight
                    },
                    self.policy.fit_weight,
                    recency,
                    self.policy.recency_weight,
                );
                answerable.push((score, stripped));
            } else if fallback.is_none() {
                fallback = Some(stripped);
            }
        }

        // Rank the answerable candidates rather than taking the first.
        //
        // Several turns can satisfy the answer type — Carol's history holds
        // three money amounts and two company names — so "first one that
        // type-checks" picks by retrieval rank alone and is frequently
        // wrong. Scoring each candidate on how much of the *question* its
        // text accounts for is what separates "Renamed Loom to Memex" from
        // "I'm leaving Stripe to start a company" for a question about
        // renaming.
        // Supersession applies only when the question asks for the current
        // state of a changing fact.
        let recency_pref = if extract::asks_for_latest(question) {
            self.policy.recency_weight
        } else {
            0.0
        };
        if let Some(best) = pick_answerable(answerable, recency_pref, |t| self.turn_recency(t)) {
            parts.push(best);
        }

        // No candidate carried a typed answer — widen to the full ingested
        // transcript before giving up, then fall back to rank order.
        if parts.is_empty() {
            if let Some(hit) = self.answer_bearing_turn(question, qkind) {
                parts.push(strip_speaker_prefix(&hit).to_string());
            } else if let Some(f) = fallback {
                parts.push(f);
            }
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
        // Tier-0 span composition: classify the question and pull the
        // tightest matching span (date / money / time) from the candidate.
        // Falls back to the full candidate when no extractor fires —
        // SQuAD F1 punishes long predictions on precision but rewards
        // recall, so the fallback is still better than empty.
        // Resolve named answers against the graph's own NER rather than
        // orthography — see `extract::resolve_named`.
        let composed = if matches!(qkind, extract::QKind::Named) {
            let lookup = |name: &str| -> Option<String> {
                self.engine
                    .as_ref()
                    .and_then(|e| e.graph().find_entity_by_name_icase(name).ok().flatten())
                    .map(|ent| format!("{:?}", ent.entity_type))
            };
            extract::resolve_named(&joined, question, &lookup)
                .unwrap_or_else(|| joined.clone())
        } else {
            extract::compose_short_answer(question, &joined)
        };
        truncate(&composed, self.config.max_answer_chars)
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
    /// Where a turn sits in the conversation, as a 0..1 fraction.
    ///
    /// Supersession proxy: 1.0 is the most recent statement. When two turns
    /// both answer the question ("June 4th" and "July 9th"), the later one
    /// is the current fact.
    fn turn_recency(&self, text: &str) -> f32 {
        if self.ingested_turns.len() < 2 {
            return 1.0;
        }
        let needle = text.trim().to_lowercase();
        let pos = self.ingested_turns.iter().position(|t| {
            strip_speaker_prefix(t).trim().to_lowercase() == needle
        });
        match pos {
            Some(i) => i as f32 / (self.ingested_turns.len() - 1) as f32,
            None => 0.5,
        }
    }

    /// Apply a GEPA retrieval policy to the underlying engine.
    ///
    /// Called between candidate evaluations so each policy is measured
    /// against the same corpus with only the policy differing.
    pub fn apply_policy(&mut self, policy: &tm_gepa::RetrievalPolicy) {
        self.policy = policy.clone();
        if let Some(engine) = self.engine.as_mut() {
            engine.apply_policy(policy);
        }
    }

    /// Synchronous ingest, for the GEPA scorer.
    ///
    /// Mirrors [`LocomoRunner::ingest_sample`] without the async wrapper —
    /// the underlying pipeline is entirely synchronous; only the trait is
    /// async (for backends that call out over the network).
    pub fn ingest_sample_sync(&mut self, sample: &LocomoSample) -> Result<(), String> {
        self.reset_stores()?;
        let pipeline = self.pipeline.as_ref().expect("pipeline open");
        let session_id = Uuid::new_v4();

        for session in &sample.sessions {
            for turn in &session.turns {
                let formatted = match &session.date {
                    Some(d) => format!("{} ({}): {}", turn.speaker, d, turn.text),
                    None => format!("{}: {}", turn.speaker, turn.text),
                };
                if pipeline.ingest(&formatted, session_id).is_err() {
                    continue;
                }
                self.ingested_turns.push(formatted);
            }
        }
        if let Some(engine) = self.engine.as_mut() {
            engine
                .refresh_graph()
                .map_err(|e| format!("refresh_graph: {e:?}"))?;
        }
        Ok(())
    }

    /// Synchronous answer, returning the prediction and how many grounding
    /// candidates retrieval supplied (the scorer needs the latter to tell a
    /// recall failure apart from a ranking failure).
    pub fn answer_sync(&mut self, question: &LocomoQuestion) -> (String, usize) {
        let engine = match self.engine.as_mut() {
            Some(e) => e,
            None => return (String::new(), 0),
        };
        let result = match engine.query(&question.question) {
            Ok(r) => r,
            Err(_) => return (String::new(), 0),
        };
        let grounding = result.signal_hits.len() + result.entities.len();
        (self.synthesize(&result, &question.question), grounding)
    }

    /// Scan the full ingested transcript for the turn that both overlaps the
    /// question lexically and carries an extractable answer of `qkind`.
    ///
    /// This is the reader's last resort when retrieval's ranked candidates
    /// all fail the answer-type check. It is deliberately answer-type-aware
    /// rather than a pure keyword scan: for "Which airport did Alice fly
    /// into?" the highest-overlap turn is the one about flying to Tokyo,
    /// but the only turn bearing a novel place name is the Haneda one.
    fn answer_bearing_turn(&self, question: &str, qkind: extract::QKind) -> Option<String> {
        if matches!(qkind, extract::QKind::Generic | extract::QKind::YesNo) {
            return self.substring_fallback(question);
        }
        let q_tokens = question_tokens(question);
        if q_tokens.is_empty() {
            return None;
        }

        let mut scored: Vec<(usize, usize, &String)> = Vec::new();
        for (idx, turn) in self.ingested_turns.iter().enumerate() {
            if is_question_turn(turn) {
                continue;
            }
            let body = strip_speaker_prefix(turn);
            if !qkind.is_satisfied_by(body, question) {
                continue;
            }
            let lower = body.to_lowercase();
            let overlap = q_tokens.iter().filter(|t| lower.contains(t.as_str())).count();
            scored.push((overlap, idx, turn));
        }

        // Highest lexical overlap wins; ties break toward the later turn,
        // which is the more recent statement of a changing fact.
        scored
            .into_iter()
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
            .map(|(_, _, turn)| turn.clone())
    }

    fn substring_fallback(&self, question: &str) -> Option<String> {
        let q_tokens = question_tokens(question);
        if q_tokens.is_empty() {
            return None;
        }
        // Detect "finish/final/actual/result" recency-cued questions —
        // when the question asks for an outcome value, the LATEST turn
        // that carries the right typed value is almost always the answer.
        let qkind = extract::classify_question(question);
        let lower_q = question.to_lowercase();
        let recency_cue = lower_q.contains("finish")
            || lower_q.contains("final")
            || lower_q.contains("actual")
            || lower_q.contains("result")
            || lower_q.contains("end up");
        let scored: Vec<(usize, bool, usize, &String)> = self
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
                // Typed-value match: turn contains the specific kind of
                // value the question is asking for. With a recency cue,
                // this is the dominant signal — bumps the score above
                // raw overlap.
                let typed_hit = match qkind {
                    extract::QKind::Time => extract::extract_time(turn).is_some(),
                    extract::QKind::Date => extract::extract_date(turn).is_some(),
                    extract::QKind::Money => extract::extract_money(turn).is_some(),
                    _ => false,
                };
                (overlap, typed_hit, idx, turn)
            })
            .filter(|(o, t, _, _)| *o > 0 || (recency_cue && *t))
            .collect();
        // Tie-break order:
        // 1. With a recency cue, typed-value-bearing turns win first.
        //    (For "finish time", a turn with "2:58:42" beats a turn
        //    that just shares the word "time".)
        // 2. Otherwise raw overlap.
        // 3. Question-turn (Q→A adjacency for next-turn answer).
        // 4. Later ingest position.
        let best = scored.into_iter().max_by(|a, b| {
            if recency_cue {
                a.1.cmp(&b.1)
                    .then_with(|| a.0.cmp(&b.0))
                    .then_with(|| {
                        let aq = is_question_turn(a.3);
                        let bq = is_question_turn(b.3);
                        aq.cmp(&bq)
                    })
                    .then_with(|| a.2.cmp(&b.2))
            } else {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.cmp(&b.1))
                    .then_with(|| {
                        let aq = is_question_turn(a.3);
                        let bq = is_question_turn(b.3);
                        aq.cmp(&bq)
                    })
                    .then_with(|| a.2.cmp(&b.2))
            }
        })?;
        let (_, _, idx, turn) = best;
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

/// Score how well a candidate turn answers `question`.
///
/// Combines two signals:
/// - **Content-token coverage**: the share of the question's content words
///   the turn accounts for. This is what distinguishes turns that merely
///   mention the subject from the turn actually about the asked-for event.
/// - **Retrieval rank**: a mild prior, so that when coverage ties, the
///   ranking the engine produced still decides.
///
/// Coverage dominates deliberately — retrieval rank is the signal that was
/// already shown to put the wrong turn on top.
fn evidence_score(
    turn: &str,
    question: &str,
    retrieval_score: f32,
    lexical_score: f32,
    coverage_weight: f32,
    fit_weight: f32,
    recency: f32,
    recency_weight: f32,
) -> f32 {
    let _ = (turn, question);
    let rank_prior = retrieval_score.max(0.01);
    let coverage = lexical_score;
    // Coverage multiplies the rank prior rather than being summed with it.
    // Summing let a long, loosely-related turn outscore the correct short
    // one purely by containing more question tokens; as a multiplier,
    // coverage can only promote a candidate that retrieval already rated
    // plausible, which is the intended "break ties with evidence" effect.
    // Syntactic fit: does this turn's structure match what the question
    // asks for, or does it merely contain a value of the right type?
    // Both boosts are GEPA-tunable: how much question-token coverage and
    // how much syntactic fit should be allowed to override the retriever's
    // own ranking is exactly the kind of trade-off the loop can search
    // better than it can be guessed.
    let fit = extract::answer_confidence(turn, question);
    rank_prior
        * (1.0 + coverage_weight * coverage)
        * (1.0 + fit_weight * (fit - 0.6))
        * (1.0 + recency_weight * recency)
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
        // The engine was opened before this ingest ran, so its in-memory
        // entity_map (skg_id -> Uuid) is stale — every vector hit would fail
        // to map back to a Uuid and `search_vectors` would silently return
        // empty. Reload the maps now that the graph is populated.
        if let Some(engine) = self.engine.as_mut() {
            engine
                .refresh_graph()
                .map_err(|e| format!("refresh_graph: {e:?}"))?;
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
            for (i, h) in result.signal_hits.iter().enumerate().take(8) {
                eprintln!("        [{i}] {:.3} {}", h.score, h.text);
            }
        }

        // Tier-1 path: synthesize with the candle LLM over retrieved grounding.
        // We build the owned request via a *synchronous* helper so that no
        // reference to `self` (which holds non-Sync RefCells via
        // RetrievalEngine) is held across the await below — keeping the outer
        // future `Send` as the LocomoRunner trait requires.
        #[cfg(feature = "tier1")]
        if self.config.use_tier1 {
            if let Some((candle, req)) = self.build_tier1_request(&result, &question.question) {
                use tm_answer::backend::AnswerBackend;
                match candle.answer(&req).await {
                    Ok(resp) => {
                        let text = resp.text.trim().to_string();
                        if !text.is_empty() {
                            return Ok(text);
                        }
                    }
                    Err(e) => {
                        if std::env::var("TM_BENCH_DEBUG").is_ok() {
                            eprintln!("[tier1] candle failed: {e}");
                        }
                    }
                }
                // Fall through to extractive on any Tier-1 failure.
            }
        }

        Ok(self.synthesize(&result, &question.question))
    }
}

impl TraceMindRunner {
    /// Build an owned Tier-1 request (candle handle + AnswerRequest) from the
    /// retrieval result. Synchronous — all `&self` access happens here so the
    /// caller can await on the returned owned values without capturing `self`.
    #[cfg(feature = "tier1")]
    fn build_tier1_request(
        &self,
        result: &tm_retrieval::engine::RetrievalResult,
        question: &str,
    ) -> Option<(tm_answer::candle_backend::CandleBackend, tm_answer::types::AnswerRequest)> {
        use tm_answer::types::{AnswerRequest, GroundingChunk, TaskKind};

        let candle = self.candle.as_ref()?.clone();

        // Grounding strategy: retrieval-ranked hits first (they carry the
        // strongest signal), then backfill with the rest of the conversation
        // so the LLM never has to answer "unknown" purely because retrieval
        // under-surfaced the answer turn. LoCoMo conversations are short
        // enough that the full transcript fits the model's context; for
        // production-scale transcripts the retrieval prefix is what bounds
        // the context and this backfill is capped.
        let mut grounding: Vec<GroundingChunk> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for s in result.signal_hits.iter().take(8) {
            let text = strip_speaker_prefix(&s.text).to_string();
            if seen.insert(text.clone()) {
                grounding.push(GroundingChunk {
                    trace_id: String::new(),
                    entity_ids: vec![],
                    text,
                    score: s.score,
                });
            }
        }

        // Backfill with remaining conversation turns (retrieval-independent),
        // capped so we stay well inside the context budget.
        for turn in self.ingested_turns.iter() {
            if grounding.len() >= 24 { break; }
            if is_question_turn(turn) { continue; }
            let text = strip_speaker_prefix(turn).to_string();
            if text.trim().is_empty() { continue; }
            if seen.insert(text.clone()) {
                grounding.push(GroundingChunk {
                    trace_id: String::new(),
                    entity_ids: vec![],
                    text,
                    score: 0.1,
                });
            }
        }

        if grounding.is_empty() {
            return None;
        }

        let req = AnswerRequest {
            nearby_topics: Vec::new(),
            question: question.to_string(),
            grounding,
            task: TaskKind::ShortAnswer,
            max_output_tokens: 48,
            preferred_tier: None,
        };
        Some((candle, req))
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

/// Inverse document frequency over a small candidate set.
///
/// Used to weight question-token overlap so that a rare, topic-defining
/// word ("rename", "finish", "employer") counts for more than a word that
/// half the candidates share ("company", the subject's name).
struct TokenIdf {
    df: std::collections::HashMap<String, usize>,
    n_docs: usize,
}

impl TokenIdf {
    fn build<'a>(docs: impl Iterator<Item = &'a str>) -> Self {
        let mut df: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut n_docs = 0usize;
        for doc in docs {
            n_docs += 1;
            let mut seen: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for t in question_tokens(doc) {
                if seen.insert(t.clone()) {
                    *df.entry(t).or_insert(0) += 1;
                }
            }
        }
        Self { df, n_docs }
    }

    /// IDF-weighted share of the question's content tokens that `turn`
    /// accounts for, in [0,1].
    fn coverage(&self, question: &str, turn: &str) -> f32 {
        let q_tokens = question_tokens(question);
        if q_tokens.is_empty() {
            return 0.0;
        }
        let body = turn.to_lowercase();
        let mut matched = 0.0f32;
        let mut total = 0.0f32;
        for t in &q_tokens {
            let w = self.idf(t);
            total += w;
            // Substring containment, so "rename" matches "Renamed" without
            // needing a stemmer.
            if body.contains(t.as_str()) {
                matched += w;
            }
        }
        if total <= 0.0 {
            0.0
        } else {
            matched / total
        }
    }

    fn idf(&self, term: &str) -> f32 {
        let n = self.n_docs.max(1) as f32;
        let df = *self.df.get(term).unwrap_or(&0) as f32;
        // Standard smoothed IDF; a term in no candidate still carries the
        // maximum weight, which is correct — it is maximally discriminating
        // if some candidate does contain it.
        (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
    }
}

/// Choose among candidates that all satisfy the answer type.
///
/// Evidence score decides, unless the caller passes a non-zero
/// `recency_weight` — then, among candidates within [`SUPERSESSION_BAND`]
/// of the best, the *later* memory wins.
///
/// This is fact supersession, a defining property of a memory system rather
/// than a retrieval trick: an offer that rose from $40M to $65M leaves both
/// statements in the history and both type-check as answers.
///
/// It is deliberately **not** applied by default. Measured on the training
/// split, both a blanket recency multiplier and a tie-break at any band
/// width made things worse — many candidates score exactly equal, and
/// flipping those to the later turn is wrong more often than right, because
/// most facts are stated once and never revised. Callers enable it only
/// when the question asks for the current state
/// (see [`extract::asks_for_latest`]).
fn pick_answerable(
    answerable: Vec<(f32, String)>,
    recency_weight: f32,
    recency_of: impl Fn(&str) -> f32,
) -> Option<String> {
    if answerable.is_empty() {
        return None;
    }
    let best = answerable
        .iter()
        .map(|(s, _)| *s)
        .fold(f32::MIN, f32::max);
    if best <= 0.0 {
        return answerable.into_iter().next().map(|(_, t)| t);
    }

    let floor = best * (1.0 - SUPERSESSION_BAND);
    let mut contenders: Vec<&(f32, String)> =
        answerable.iter().filter(|(s, _)| *s >= floor).collect();
    if contenders.len() > 1 && recency_weight > 0.0 {
        contenders.sort_by(|a, b| {
            recency_of(&a.1)
                .partial_cmp(&recency_of(&b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return contenders.last().map(|(_, t)| t.clone());
    }
    answerable
        .into_iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, t)| t)
}

/// How close two candidates must be for recency to break the tie.
const SUPERSESSION_BAND: f32 = 0.35;

#[cfg(test)]
mod supersession_tests {
    use super::*;

    #[test]
    fn clear_winner_is_not_overridden_by_recency() {
        let cands = vec![(1.0, "strong early".to_string()), (0.2, "weak late".to_string())];
        let pick = pick_answerable(cands, 1.0, |t| if t.contains("late") { 1.0 } else { 0.0 });
        assert_eq!(pick.as_deref(), Some("strong early"));
    }

    #[test]
    fn near_tie_resolves_to_the_later_memory() {
        let cands = vec![(1.0, "June 4th".to_string()), (0.95, "July 9th".to_string())];
        let pick = pick_answerable(cands, 1.0, |t| if t.contains("July") { 1.0 } else { 0.0 });
        assert_eq!(pick.as_deref(), Some("July 9th"), "later fact must supersede");
    }

    #[test]
    fn recency_disabled_falls_back_to_score() {
        let cands = vec![(1.0, "June 4th".to_string()), (0.95, "July 9th".to_string())];
        let pick = pick_answerable(cands, 0.0, |t| if t.contains("July") { 1.0 } else { 0.0 });
        assert_eq!(pick.as_deref(), Some("June 4th"));
    }

    #[test]
    fn empty_input_yields_none() {
        assert!(pick_answerable(vec![], 1.0, |_| 1.0).is_none());
    }
}
