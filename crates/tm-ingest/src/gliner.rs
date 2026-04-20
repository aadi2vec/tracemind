//! TM-NLP-004 — real GLiNER NER extractor.
//!
//! Runs `onnx-community/gliner_small-v2.1` (int8 quantized, ~183 MB) via
//! ONNX Runtime. Zero-shot span-based NER: given a list of candidate entity
//! labels and raw text, it returns typed spans with confidence scores.
//!
//! Architecture reminders (verified empirically against the real ONNX export
//! on 2026-04-19):
//!
//! **Inputs (6 tensors):**
//! - `input_ids` `[B, L]` i64 — `[CLS] <<ENT>> l1 <<ENT>> l2 ... <<SEP>> w1 w2 ... [SEP]`
//! - `attention_mask` `[B, L]` i64 — 1 on real positions, 0 on pad
//! - `words_mask` `[B, L]` i64 — 0 everywhere except the first subtoken of each
//!   input word, which carries `word_index + 1`
//! - `text_lengths` `[B, 1]` i64 — number of words in the text region
//! - `span_idx` `[B, num_words * max_width, 2]` i64 — full (start_word, end_word)
//!   grid (inclusive). Not a compacted list; the model indexes into it as a
//!   dense `num_words × max_width` reshape
//! - `span_mask` `[B, num_words * max_width]` bool — true where `end_word < num_words`
//!
//! **Output:** `logits` `[B, num_words, max_width, num_classes]` f32. Each
//! `(word_i, width_j, class_k)` entry is a pre-sigmoid score for the span
//! `(word_i ..= word_i + width_j)` belonging to class `labels[class_k]`.
//!
//! **Decode:** sigmoid → threshold (0.5 default) → greedy overlap resolution
//! (highest-prob span wins, suppress any span that overlaps it).
//!
//! The extractor auto-downloads on first use and gracefully falls back to
//! `HeuristicExtractor` if the download fails (`auto_download_or_none`).

use std::collections::HashSet;
use std::sync::Mutex;

use tm_types::{Entity, EntityType, Result, Triple, TraceMindError};
use tracing::{debug, info, warn};

use crate::extractor::EntityExtractor;
use crate::pipeline::extract_triples as heuristic_triples;

// ---------------------------------------------------------------------------
// Model config (matches onnx-community/gliner_small-v2.1/gliner_config.json)
// ---------------------------------------------------------------------------

/// DeBERTa-v3 small's [CLS] token id.
const CLS_ID: i64 = 1;
/// DeBERTa-v3 small's [SEP] / [EOS] token id.
const EOS_ID: i64 = 2;
/// GLiNER's `<<ENT>>` marker (added token, id = 128002).
const ENT_MARKER_ID: i64 = 128002;
/// GLiNER's `<<SEP>>` marker between prompt and text (id = 128003).
const SEP_MARKER_ID: i64 = 128003;

/// Max span width in words. Matches `max_width` in `gliner_config.json`.
const MAX_WIDTH: usize = 12;

/// HuggingFace repo hosting the transformers.js-compatible ONNX export.
const HF_REPO: &str = "onnx-community/gliner_small-v2.1";
/// Quantized int8 export — ~183 MB, acceptable bundle cost.
const HF_MODEL_FILE: &str = "onnx/model_quantized.onnx";
const HF_TOKENIZER_FILE: &str = "tokenizer.json";

/// Default candidate labels. Zero-shot, so this is user-configurable at
/// construction time; this list covers the common TraceMind capture cases.
pub const DEFAULT_LABELS: &[&str] = &[
    "person",
    "organization",
    "location",
    "technology",
    "product",
    "project",
    "concept",
    "event",
];

/// Probability threshold below which spans are dropped.
/// Tuned on `fixtures/ner_eval.jsonl`: sweep over {0.3, 0.4, 0.5} gave
/// F1 = {0.910, 0.844, 0.795}. 0.3 picks up the Technology long-tail
/// (CoreML, CUDA, NVIDIA, …) that 0.5 misses without significantly
/// hurting precision (0.929 vs 0.986).
pub const DEFAULT_THRESHOLD: f32 = 0.3;

// ---------------------------------------------------------------------------
// GlinerExtractor
// ---------------------------------------------------------------------------

/// Real GLiNER extractor. Owns an ORT session + tokenizer + label list.
pub struct GlinerExtractor {
    session: Mutex<ort::session::Session>,
    tokenizer: tokenizers::Tokenizer,
    labels: Vec<String>,
    /// Pre-tokenized label prompt segment: `<<ENT>> l1 <<ENT>> l2 ... <<SEP>>`.
    /// Cached because it's the same for every call.
    prompt_ids: Vec<i64>,
    threshold: f32,
}

impl GlinerExtractor {
    /// Construct from explicit paths with a custom label set + threshold.
    pub fn new(
        model_path: &str,
        tokenizer_path: &str,
        labels: Vec<String>,
        threshold: f32,
    ) -> Result<Self> {
        info!("[gliner] loading model from {model_path}");

        let session = ort::session::Session::builder()
            .map_err(|e| TraceMindError::Embedding(format!("ort builder: {e}")))?
            .with_intra_threads(2)
            .map_err(|e| TraceMindError::Embedding(format!("ort threads: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| TraceMindError::Embedding(format!("ort load: {e}")))?;

        let tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
            .map_err(|e| TraceMindError::Embedding(format!("load tokenizer: {e}")))?;

        let prompt_ids = build_prompt_ids(&tokenizer, &labels)?;
        info!(
            "[gliner] ready — {} labels, prompt_len={}, threshold={}",
            labels.len(),
            prompt_ids.len(),
            threshold
        );

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
            labels,
            prompt_ids,
            threshold: threshold.clamp(0.0, 1.0),
        })
    }

    /// Auto-download from HuggingFace Hub. Returns `Err` on network failure.
    pub fn auto_download(labels: Vec<String>, threshold: f32) -> Result<Self> {
        info!("[gliner] resolving model via hf-hub ({HF_REPO})");
        let api = hf_hub::api::sync::Api::new()
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub init: {e}")))?;
        let repo = api.model(HF_REPO.to_string());

        let model_path = repo
            .get(HF_MODEL_FILE)
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub fetch model: {e}")))?;
        let tokenizer_path = repo
            .get(HF_TOKENIZER_FILE)
            .map_err(|e| TraceMindError::Embedding(format!("hf-hub fetch tokenizer: {e}")))?;

        info!(
            "[gliner] cached at model={} tokenizer={}",
            model_path.display(),
            tokenizer_path.display()
        );

        Self::new(
            model_path.to_string_lossy().as_ref(),
            tokenizer_path.to_string_lossy().as_ref(),
            labels,
            threshold,
        )
    }

    /// Best-effort constructor: on any failure, log and return `None` so the
    /// pipeline falls back to `HeuristicExtractor`.
    pub fn auto_download_or_none(labels: Vec<String>, threshold: f32) -> Option<Self> {
        match Self::auto_download(labels, threshold) {
            Ok(x) => Some(x),
            Err(e) => {
                warn!("[gliner] unavailable — falling back to heuristic: {e}");
                None
            }
        }
    }

    /// Default-labels constructor for the common path.
    pub fn auto_download_default() -> Option<Self> {
        Self::auto_download_or_none(
            DEFAULT_LABELS.iter().map(|s| s.to_string()).collect(),
            DEFAULT_THRESHOLD,
        )
    }

    /// Raw span-level inference. Returns `(start_word, end_word, label_idx, score)`.
    /// Exposed for the eval harness; callers usually want
    /// [`EntityExtractor::extract_entities`].
    pub fn predict_spans(&self, text: &str) -> Result<Vec<SpanHit>> {
        let words = split_words(text);
        if words.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenize each word (no special tokens) and remember the first-subtoken
        // position inside the concatenated word-token region.
        let mut word_tok_ids: Vec<i64> = Vec::with_capacity(words.len() * 2);
        let mut word_first_subtok: Vec<usize> = Vec::with_capacity(words.len());
        for w in &words {
            let enc = self
                .tokenizer
                .encode(w.as_str(), false)
                .map_err(|e| TraceMindError::Embedding(format!("tokenize word: {e}")))?;
            word_first_subtok.push(word_tok_ids.len());
            for id in enc.get_ids() {
                word_tok_ids.push(*id as i64);
            }
        }

        // Assemble full sequence: [CLS] + prompt + word_tok + [EOS]
        let mut input_ids: Vec<i64> = Vec::with_capacity(
            1 + self.prompt_ids.len() + word_tok_ids.len() + 1,
        );
        input_ids.push(CLS_ID);
        input_ids.extend_from_slice(&self.prompt_ids);
        let prompt_offset = input_ids.len(); // absolute index where word tokens start
        input_ids.extend_from_slice(&word_tok_ids);
        input_ids.push(EOS_ID);

        let seq_len = input_ids.len();
        let attention_mask = vec![1_i64; seq_len];

        // words_mask: 0 everywhere except first subtok of each word, which gets (wi+1)
        let mut words_mask = vec![0_i64; seq_len];
        for (wi, &rel) in word_first_subtok.iter().enumerate() {
            let abs = prompt_offset + rel;
            if abs < seq_len {
                words_mask[abs] = (wi + 1) as i64;
            }
        }

        let num_words = words.len();

        // Dense span grid: (start_word, end_word) for start in 0..num_words, width in 0..MAX_WIDTH.
        let grid_size = num_words * MAX_WIDTH;
        let mut span_idx_flat: Vec<i64> = Vec::with_capacity(grid_size * 2);
        let mut span_mask_flat: Vec<bool> = Vec::with_capacity(grid_size);
        for s in 0..num_words {
            for w in 0..MAX_WIDTH {
                let e = s + w;
                span_idx_flat.push(s as i64);
                span_idx_flat.push(e as i64);
                span_mask_flat.push(e < num_words);
            }
        }

        // Build ORT tensors.
        use ndarray::{Array2, Array3};
        use ort::value::Value;

        let ids_array = Array2::from_shape_vec((1, seq_len), input_ids)
            .map_err(|e| TraceMindError::Embedding(format!("shape ids: {e}")))?;
        let attn_array = Array2::from_shape_vec((1, seq_len), attention_mask.clone())
            .map_err(|e| TraceMindError::Embedding(format!("shape attn: {e}")))?;
        let wm_array = Array2::from_shape_vec((1, seq_len), words_mask)
            .map_err(|e| TraceMindError::Embedding(format!("shape words_mask: {e}")))?;
        let tl_array = Array2::from_shape_vec((1, 1), vec![num_words as i64])
            .map_err(|e| TraceMindError::Embedding(format!("shape text_lengths: {e}")))?;
        let span_array = Array3::from_shape_vec((1, grid_size, 2), span_idx_flat)
            .map_err(|e| TraceMindError::Embedding(format!("shape span_idx: {e}")))?;
        let span_mask_array = Array2::from_shape_vec((1, grid_size), span_mask_flat)
            .map_err(|e| TraceMindError::Embedding(format!("shape span_mask: {e}")))?;

        let ids_v = Value::from_array(ids_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort ids: {e}")))?;
        let attn_v = Value::from_array(attn_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort attn: {e}")))?;
        let wm_v = Value::from_array(wm_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort wm: {e}")))?;
        let tl_v = Value::from_array(tl_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort tl: {e}")))?;
        let span_v = Value::from_array(span_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort span: {e}")))?;
        let span_mask_v = Value::from_array(span_mask_array)
            .map_err(|e| TraceMindError::Embedding(format!("ort span_mask: {e}")))?;

        let inputs = ort::inputs![
            "input_ids" => ids_v,
            "attention_mask" => attn_v,
            "words_mask" => wm_v,
            "text_lengths" => tl_v,
            "span_idx" => span_v,
            "span_mask" => span_mask_v,
        ];

        let mut session = self.session.lock().expect("gliner session mutex poisoned");
        let outputs = session
            .run(inputs)
            .map_err(|e| TraceMindError::Embedding(format!("ort run: {e}")))?;

        // First output is logits [1, num_words, max_width, num_classes]
        let logits_out = outputs
            .get("logits")
            .ok_or_else(|| TraceMindError::Embedding("missing logits".into()))?;
        let (shape, data) = logits_out
            .try_extract_tensor::<f32>()
            .map_err(|e| TraceMindError::Embedding(format!("extract logits: {e}")))?;

        if shape.len() != 4 {
            return Err(TraceMindError::Embedding(format!(
                "unexpected logits rank {}",
                shape.len()
            )));
        }
        let w_dim = shape[1] as usize;
        let ww_dim = shape[2] as usize;
        let c_dim = shape[3] as usize;
        debug!(
            "[gliner] logits shape = [1, {}, {}, {}]",
            w_dim, ww_dim, c_dim
        );

        let mut hits: Vec<SpanHit> = Vec::new();
        for wi in 0..w_dim.min(num_words) {
            for wj in 0..ww_dim.min(MAX_WIDTH) {
                let end_word = wi + wj;
                if end_word >= num_words {
                    continue;
                }
                for ci in 0..c_dim.min(self.labels.len()) {
                    let idx = ((wi * ww_dim) + wj) * c_dim + ci;
                    if idx >= data.len() {
                        continue;
                    }
                    let logit = data[idx];
                    let score = sigmoid(logit);
                    if score >= self.threshold {
                        hits.push(SpanHit {
                            start_word: wi,
                            end_word,
                            label_idx: ci,
                            score,
                        });
                    }
                }
            }
        }

        // Greedy overlap resolution: sort by score desc, keep span if it doesn't
        // overlap any already-accepted span.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut kept: Vec<SpanHit> = Vec::new();
        for h in hits {
            let overlaps = kept.iter().any(|k| {
                h.start_word <= k.end_word && k.start_word <= h.end_word
            });
            if !overlaps {
                kept.push(h);
            }
        }
        kept.sort_by_key(|h| h.start_word);

        Ok(kept)
    }

    /// Convenience: raw spans materialised as `Entity` values using the
    /// label→`EntityType` mapping. Used by the eval harness.
    pub fn extract_entities_with_words(&self, text: &str) -> Result<Vec<Entity>> {
        let words = split_words(text);
        let hits = self.predict_spans(text)?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let name = words[h.start_word..=h.end_word].join(" ");
            let name = trim_trailing_punct(&name).to_string();
            if name.is_empty() {
                continue;
            }
            let key = name.to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let et = map_label_to_entity_type(&self.labels[h.label_idx]);
            out.push(Entity::new(name, et, h.score.min(0.99) as f64));
        }
        Ok(out)
    }
}

impl EntityExtractor for GlinerExtractor {
    fn extract_entities(&self, text: &str) -> Vec<Entity> {
        match self.extract_entities_with_words(text) {
            Ok(v) => v,
            Err(e) => {
                warn!("[gliner] extract failed, returning empty: {e}");
                Vec::new()
            }
        }
    }

    fn extract_triples(&self, text: &str, entities: &[Entity]) -> Vec<Triple> {
        // GLiNER is NER-only; relation extraction stays heuristic for now
        // (TM-NLP-004 explicitly scopes to NER; relation upgrade is future work).
        heuristic_triples(text, entities)
    }

    fn name(&self) -> &'static str {
        "gliner"
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Raw span hit as returned by [`GlinerExtractor::predict_spans`].
#[derive(Debug, Clone)]
pub struct SpanHit {
    pub start_word: usize,
    pub end_word: usize,
    pub label_idx: usize,
    pub score: f32,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// GLiNER's `words_splitter_type` is `"whitespace"`. Match that exactly so our
/// word indices line up with what the model was trained on.
fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

/// Strip trailing punctuation from a multi-word span so the emitted entity
/// name is `"TraceMind"` not `"TraceMind,"`.
fn trim_trailing_punct(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_ascii_punctuation())
}

/// Construct the cached label-prompt id sequence:
/// `<<ENT>> l1_toks <<ENT>> l2_toks ... <<ENT>> lN_toks <<SEP>>`
fn build_prompt_ids(
    tokenizer: &tokenizers::Tokenizer,
    labels: &[String],
) -> Result<Vec<i64>> {
    let mut out = Vec::new();
    for l in labels {
        out.push(ENT_MARKER_ID);
        let enc = tokenizer
            .encode(l.as_str(), false)
            .map_err(|e| TraceMindError::Embedding(format!("tokenize label: {e}")))?;
        for id in enc.get_ids() {
            out.push(*id as i64);
        }
    }
    out.push(SEP_MARKER_ID);
    Ok(out)
}

/// Map a GLiNER label (lowercase English noun) to the closest `EntityType`.
fn map_label_to_entity_type(label: &str) -> EntityType {
    match label.to_lowercase().as_str() {
        "person" | "people" | "human" => EntityType::Person,
        "organization" | "org" | "company" => EntityType::Organization,
        "technology" | "tech" | "tool" | "framework" | "language" => EntityType::Technology,
        "project" => EntityType::Project,
        "concept" | "topic" | "idea" => EntityType::Concept,
        "event" | "meeting" | "conference" => EntityType::Event,
        "file" | "path" => EntityType::File,
        "url" | "link" => EntityType::Url,
        other => EntityType::Custom(
            other
                .chars()
                .enumerate()
                .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests — pure helpers only. Real-model tests live in the eval harness
// (tm-bench ner_eval) because they require the 183 MB ONNX download.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigmoid_midpoint() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn sigmoid_saturates() {
        assert!(sigmoid(20.0) > 0.99);
        assert!(sigmoid(-20.0) < 0.01);
    }

    #[test]
    fn split_words_handles_whitespace() {
        let w = split_words("  hello   world  \n foo");
        assert_eq!(w, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn trim_trailing_punct_basic() {
        assert_eq!(trim_trailing_punct("TraceMind,"), "TraceMind");
        assert_eq!(trim_trailing_punct("hello!!!"), "hello");
        assert_eq!(trim_trailing_punct("plain"), "plain");
    }

    #[test]
    fn label_mapping_covers_canonical() {
        assert_eq!(map_label_to_entity_type("person"), EntityType::Person);
        assert_eq!(
            map_label_to_entity_type("organization"),
            EntityType::Organization
        );
        assert_eq!(
            map_label_to_entity_type("technology"),
            EntityType::Technology
        );
        assert_eq!(map_label_to_entity_type("concept"), EntityType::Concept);
        assert!(matches!(
            map_label_to_entity_type("location"),
            EntityType::Custom(s) if s == "Location"
        ));
    }

    /// `auto_download_or_none` must never panic, even if network fails.
    /// Contract: returns `Option<_>` — CI may or may not have network.
    #[test]
    fn auto_download_never_panics() {
        let _ = GlinerExtractor::auto_download_default();
    }

    /// Missing file path returns `Err`, not a panic.
    #[test]
    fn new_errors_on_missing_files() {
        let res = GlinerExtractor::new(
            "/nonexistent/model.onnx",
            "/nonexistent/tokenizer.json",
            DEFAULT_LABELS.iter().map(|s| s.to_string()).collect(),
            0.5,
        );
        assert!(res.is_err());
    }
}
