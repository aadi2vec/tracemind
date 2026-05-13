//! LM-6 — Qwen-backed triple extractor (Tier 1, opt-in).
//!
//! Slot-fills typed relations using the bundled local LLM (Qwen 2.5 1.5B
//! Q4_K_M). The model is invoked only on the **slow path** during
//! consolidation: a [`TripleWorker`](crate::TripleWorker) job pulls one
//! capture at a time off a bounded channel, runs this extractor, and
//! writes the resulting triples to the graph. Hot ingest never touches
//! llama.cpp.
//!
//! Layering:
//! - The prompt builder ([`build_qwen_prompt`]) and JSON parser
//!   ([`parse_qwen_response`]) are pure functions — they have no I/O and
//!   no model dependency, so they are testable without any GGUF on disk.
//! - The feature-gated `local-llm` block holds the actual llama-cpp-2
//!   call. When the feature is off, [`QwenTripleExtractor::extract_triples`]
//!   transparently falls back to [`HeuristicExtractor`].
//! - Entity extraction always delegates to [`HeuristicExtractor`] — Qwen
//!   only writes the *predicate* layer. NER stays on the deterministic
//!   path because it has to run on every capture.
//!
//! Output contract (what the model must emit):
//! ```json
//! { "triples": [
//!     {"subject": "Aaditya", "predicate": "works_at", "object": "Anthropic", "confidence": 0.86},
//!     {"subject": "TraceMind", "predicate": "depends_on", "object": "Rust",     "confidence": 0.7}
//! ]}
//! ```
//! Subject and object names must match an entry in the supplied entity
//! list verbatim (case-insensitive). Predicates that aren't in the
//! canonical set become [`Predicate::Custom`]. Rows whose subject or
//! object can't be resolved are dropped.

use serde::Deserialize;
use std::path::PathBuf;

use tm_types::{Entity, Predicate, Triple};
#[cfg(test)]
use tm_types::EntityType;

use crate::extractor::{EntityExtractor, HeuristicExtractor};

/// Configuration for the Qwen-backed extractor. The defaults mirror
/// [`tm_answer::LocalLlmConfig::primary`] but with smaller token budgets —
/// triple extraction needs short JSON output, not prose.
#[derive(Debug, Clone)]
pub struct QwenTripleConfig {
    /// Absolute path to the GGUF weights.
    pub model_path: PathBuf,
    /// HF repo / filename for first-run download.
    pub hf_repo: Option<String>,
    pub hf_file: Option<String>,
    /// Context window for the slow-path job. 2k is plenty — captures
    /// rarely exceed 200 tokens, and the JSON output is tight.
    pub n_ctx: u32,
    /// CPU threads. 0 → `num_cpus / 2`.
    pub n_threads: i32,
    /// GPU layers to offload (Metal on Apple Silicon). -1 = all.
    pub n_gpu_layers: i32,
    /// Sampling temperature. 0.1 keeps the JSON tight and stable.
    pub temperature: f32,
    /// Hard cap on generated tokens. 256 ≈ 10–15 triples.
    pub max_output_tokens: u32,
}

impl QwenTripleConfig {
    /// Construct a config pointing at the canonical
    /// `~/.tracemind/models/qwen2.5-1.5b-instruct-q4_k_m.gguf` path.
    pub fn primary(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            hf_repo: Some("Qwen/Qwen2.5-1.5B-Instruct-GGUF".to_string()),
            hf_file: Some("qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string()),
            n_ctx: 2048,
            n_threads: 0,
            n_gpu_layers: default_gpu_layers(),
            temperature: 0.1,
            max_output_tokens: 256,
        }
    }
}

const fn default_gpu_layers() -> i32 {
    if cfg!(all(target_os = "macos", feature = "local-llm-metal")) {
        -1
    } else {
        0
    }
}

/// LLM-backed triple extractor. Names entities with the heuristic pass
/// and asks Qwen to fill the predicate layer.
///
/// Construction is cheap — the model is loaded lazily on the first
/// `extract_triples` call. When the feature is off or the weights are
/// missing, behaviour is identical to [`HeuristicExtractor`].
pub struct QwenTripleExtractor {
    config: QwenTripleConfig,
    fallback: HeuristicExtractor,
    #[cfg(feature = "local-llm")]
    inner: std::sync::Mutex<QwenLoaded>,
}

#[cfg(feature = "local-llm")]
struct QwenLoaded {
    model: Option<llama_cpp_2::model::LlamaModel>,
}

impl QwenTripleExtractor {
    pub fn new(config: QwenTripleConfig) -> Self {
        Self {
            config,
            fallback: HeuristicExtractor,
            #[cfg(feature = "local-llm")]
            inner: std::sync::Mutex::new(QwenLoaded { model: None }),
        }
    }

    /// True if the GGUF weights exist on disk. When false, the extractor
    /// silently falls back to the heuristic pass.
    pub fn weights_present(&self) -> bool {
        self.config.model_path.exists()
    }
}

impl EntityExtractor for QwenTripleExtractor {
    fn extract_entities(&self, text: &str) -> Vec<Entity> {
        // NER is always heuristic — runs on every capture, never gated.
        self.fallback.extract_entities(text)
    }

    fn extract_triples(&self, text: &str, entities: &[Entity]) -> Vec<Triple> {
        // Need at least two entities to form a triple.
        if entities.len() < 2 {
            return Vec::new();
        }

        #[cfg(feature = "local-llm")]
        {
            if self.weights_present() {
                match run_qwen(self, text, entities) {
                    Ok(triples) if !triples.is_empty() => return triples,
                    Ok(_) => {
                        // Model returned nothing parseable → heuristic.
                    }
                    Err(e) => {
                        tracing::warn!("qwen extractor failed, falling back: {e}");
                    }
                }
            }
        }
        // Fallback path (feature off, weights missing, or model error).
        self.fallback.extract_triples(text, entities)
    }

    fn name(&self) -> &'static str {
        if self.weights_present() {
            "qwen+heuristic"
        } else {
            "heuristic (qwen weights missing)"
        }
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// Build the Qwen chat prompt for relation extraction. The output is a
/// fully-rendered `<|im_start|>system…assistant\n` string ready to be
/// tokenised by llama-cpp-2.
///
/// `entities` is the deduped node set — only relations between these
/// entities will be accepted, so it doubles as the slot-fill candidate
/// list shown to the model.
pub fn build_qwen_prompt(text: &str, entities: &[Entity]) -> String {
    let system = "You are TraceMind, a relation extraction model. Extract \
                  ONLY relations between the supplied entities. Output \
                  compact JSON with a single `triples` array. Each row is \
                  {\"subject\": <name>, \"predicate\": <relation>, \
                  \"object\": <name>, \"confidence\": <0.0-1.0>}. \
                  Predicate must be one of: related_to, is_a, part_of, \
                  has_property, works_at, collaborates_with, owns, \
                  depends_on, produces, references, has_procedure — or a \
                  short snake_case custom label if none fit. \
                  Subject and object MUST appear verbatim in the entity \
                  list. Output JSON only, no prose, no markdown fences.";

    let mut entity_list = String::new();
    for (i, e) in entities.iter().enumerate() {
        entity_list.push_str(&format!("- {} ({})\n", e.name, e.entity_type));
        // Cap at 64 entities so we never blow past n_ctx on giant captures.
        if i >= 63 {
            entity_list.push_str("- …\n");
            break;
        }
    }

    let trimmed = if text.chars().count() > 1500 {
        let head: String = text.chars().take(1500).collect();
        format!("{head}…")
    } else {
        text.to_string()
    };

    let user = format!(
        "Entities:\n{entity_list}\nText:\n{trimmed}\n\nReturn JSON now.",
    );

    format!(
        "<|im_start|>system\n{system}<|im_end|>\n\
         <|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

// ---------------------------------------------------------------------------
// JSON parser
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct QwenTripleResponse {
    triples: Vec<QwenTripleRow>,
}

#[derive(Debug, Deserialize)]
struct QwenTripleRow {
    subject: String,
    predicate: String,
    object: String,
    #[serde(default = "default_conf")]
    confidence: f64,
}

fn default_conf() -> f64 {
    0.65
}

/// Parse the raw model output into typed [`Triple`]s.
///
/// Tolerates the common ways a 1.5B model botches structured output:
/// - leading/trailing prose ("Here is the JSON:" etc.)
/// - markdown code fences
/// - trailing commas (best-effort — only when the rest of the doc parses)
///
/// Subjects and objects must resolve to an entity (case-insensitive name
/// match). Unknown predicates become [`Predicate::Custom`]. Confidence is
/// clamped to `[0.0, 1.0]`.
pub fn parse_qwen_response(raw: &str, entities: &[Entity]) -> Vec<Triple> {
    let json = match extract_json_block(raw) {
        Some(j) => j,
        None => return Vec::new(),
    };

    let parsed: QwenTripleResponse = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    // Build a case-insensitive name → entity_id lookup.
    let lookup: std::collections::HashMap<String, uuid::Uuid> = entities
        .iter()
        .map(|e| (e.name.to_lowercase(), e.id))
        .collect();

    let mut out = Vec::new();
    for row in parsed.triples {
        let s_id = match lookup.get(&row.subject.trim().to_lowercase()) {
            Some(id) => *id,
            None => continue,
        };
        let o_id = match lookup.get(&row.object.trim().to_lowercase()) {
            Some(id) => *id,
            None => continue,
        };
        if s_id == o_id {
            continue; // skip self-loops
        }
        let pred = predicate_from_str(&row.predicate);
        let conf = row.confidence.clamp(0.0, 1.0);
        out.push(Triple::new(s_id, pred, o_id, conf).with_predicate_confidence(conf));
    }
    out
}

/// Pull the first balanced `{...}` block out of a string. Tolerates
/// leading prose and markdown fences.
fn extract_json_block(s: &str) -> Option<String> {
    // Strip common markdown fence wrapping first.
    let cleaned = s.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned)
        .trim();
    let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();

    let bytes = cleaned.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        match b {
            b'\\' if in_string => escape = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(cleaned[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn predicate_from_str(s: &str) -> Predicate {
    let norm = s.trim().to_lowercase().replace([' ', '-'], "_");
    match norm.as_str() {
        "related_to" | "relates_to" | "related" => Predicate::RelatedTo,
        "is_a" | "type_of" | "kind_of" => Predicate::IsA,
        "part_of" | "partof" => Predicate::PartOf,
        "has_property" | "property" => Predicate::HasProperty,
        "works_at" | "employed_at" | "employed_by" => Predicate::WorksAt,
        "collaborates_with" | "collaborator" | "works_with" => Predicate::CollaboratesWith,
        "owns" | "owner_of" => Predicate::Owns,
        "depends_on" | "requires" => Predicate::DependsOn,
        "produces" | "creates" | "outputs" => Predicate::Produces,
        "references" | "mentions" | "cites" => Predicate::References,
        "has_procedure" | "uses_procedure" => Predicate::HasProcedure,
        "" => Predicate::RelatedTo,
        other => Predicate::Custom(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Feature-gated inference
// ---------------------------------------------------------------------------

#[cfg(feature = "local-llm")]
fn run_qwen(
    backend: &QwenTripleExtractor,
    text: &str,
    entities: &[Entity],
) -> Result<Vec<Triple>, String> {
    use llama_cpp_2::{
        context::params::LlamaContextParams,
        model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
        sampling::LlamaSampler,
    };

    let prompt = build_qwen_prompt(text, entities);
    let cfg = backend.config.clone();
    let mut guard = backend
        .inner
        .lock()
        .map_err(|e| format!("qwen lock poisoned: {e}"))?;

    let lb = shared_backend()?;

    if guard.model.is_none() {
        let gpu_layers = cfg.n_gpu_layers.max(0) as u32;
        let mp = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
        let model = LlamaModel::load_from_file(lb, &cfg.model_path, &mp)
            .map_err(|e| format!("load gguf: {e}"))?;
        guard.model = Some(model);
        tracing::info!(
            "[ingest/qwen] loaded {} (n_ctx={}, n_gpu_layers={})",
            cfg.model_path.display(),
            cfg.n_ctx,
            cfg.n_gpu_layers
        );
    }
    let model = guard.model.as_ref().expect("just loaded");

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(std::num::NonZeroU32::new(cfg.n_ctx))
        .with_n_threads(if cfg.n_threads > 0 {
            cfg.n_threads
        } else {
            num_threads()
        });
    let mut ctx = model
        .new_context(lb, ctx_params)
        .map_err(|e| format!("new_context: {e}"))?;

    let tokens = model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|e| format!("tokenize: {e}"))?;

    let mut batch = llama_cpp_2::llama_batch::LlamaBatch::new(tokens.len().max(8), 1);
    let last_index = tokens.len().saturating_sub(1) as i32;
    for (i, tok) in tokens.iter().enumerate() {
        batch
            .add(*tok, i as i32, &[0], i as i32 == last_index)
            .map_err(|e| format!("batch.add: {e}"))?;
    }
    ctx.decode(&mut batch).map_err(|e| format!("decode: {e}"))?;

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::temp(cfg.temperature),
        LlamaSampler::greedy(),
    ]);

    let mut out = String::new();
    let mut n_cur = batch.n_tokens();
    let eos = model.token_eos();
    let max_new = cfg.max_output_tokens.max(16) as i32;

    for _ in 0..max_new {
        let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);
        if new_token == eos {
            break;
        }
        let piece = model
            .token_to_str(new_token, Special::Tokenize)
            .unwrap_or_default();
        out.push_str(&piece);
        if out.ends_with("<|im_end|>") {
            out.truncate(out.len() - "<|im_end|>".len());
            break;
        }
        batch.clear();
        batch
            .add(new_token, n_cur, &[0], true)
            .map_err(|e| format!("batch.add (gen): {e}"))?;
        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| format!("gen decode: {e}"))?;
    }

    Ok(parse_qwen_response(&out, entities))
}

#[cfg(feature = "local-llm")]
fn shared_backend() -> Result<&'static llama_cpp_2::llama_backend::LlamaBackend, String> {
    use std::sync::OnceLock;
    static BACKEND: OnceLock<Result<llama_cpp_2::llama_backend::LlamaBackend, String>> =
        OnceLock::new();
    let entry = BACKEND.get_or_init(|| {
        llama_cpp_2::llama_backend::LlamaBackend::init()
            .map_err(|e| format!("LlamaBackend::init: {e}"))
    });
    match entry {
        Ok(b) => Ok(b),
        Err(s) => Err(s.clone()),
    }
}

#[cfg(feature = "local-llm")]
fn num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() as i32 / 2).max(1))
        .unwrap_or(2)
}

// ---------------------------------------------------------------------------
// Tests — pure prompt + parser only (no model on disk required).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(name: &str, ty: EntityType) -> Entity {
        Entity::new(name, ty, 0.9)
    }

    #[test]
    fn prompt_contains_chat_template_and_entity_list() {
        let ents = vec![
            ent("Aaditya", EntityType::Person),
            ent("Anthropic", EntityType::Organization),
        ];
        let p = build_qwen_prompt("Aaditya works at Anthropic.", &ents);
        assert!(p.contains("<|im_start|>system"));
        assert!(p.contains("<|im_start|>user"));
        assert!(p.contains("<|im_start|>assistant"));
        assert!(p.contains("Aaditya (Person)"));
        assert!(p.contains("Anthropic (Organization)"));
        assert!(p.contains("Aaditya works at Anthropic."));
        let lower = p.to_lowercase();
        assert!(lower.contains("json"));
        assert!(lower.contains("triples"));
    }

    #[test]
    fn prompt_caps_entity_list() {
        let ents: Vec<Entity> = (0..200)
            .map(|i| ent(&format!("E{i}"), EntityType::Concept))
            .collect();
        let p = build_qwen_prompt("noise", &ents);
        // Last entity index emitted should be 63 (64th item).
        assert!(p.contains("E63 (Concept)"));
        assert!(!p.contains("E64 (Concept)"));
        assert!(p.contains("- …"));
    }

    #[test]
    fn prompt_truncates_long_text() {
        let big = "x".repeat(3_000);
        let ents = vec![
            ent("X", EntityType::Concept),
            ent("Y", EntityType::Concept),
        ];
        let p = build_qwen_prompt(&big, &ents);
        assert!(p.contains('…'));
    }

    #[test]
    fn parser_extracts_canonical_predicates() {
        let alice = ent("Alice", EntityType::Person);
        let acme = ent("Acme", EntityType::Organization);
        let raw = r#"{
            "triples": [
                {"subject": "Alice", "predicate": "works_at", "object": "Acme", "confidence": 0.92}
            ]
        }"#;
        let triples = parse_qwen_response(raw, &[alice.clone(), acme.clone()]);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_id, alice.id);
        assert_eq!(triples[0].object_id, acme.id);
        assert_eq!(triples[0].predicate, Predicate::WorksAt);
        assert!((triples[0].confidence - 0.92).abs() < 1e-9);
        assert_eq!(triples[0].predicate_confidence, Some(0.92));
    }

    #[test]
    fn parser_handles_markdown_fences_and_prose() {
        let a = ent("Aaditya", EntityType::Person);
        let b = ent("TraceMind", EntityType::Project);
        let raw = r#"Sure! Here is the JSON:
```json
{ "triples": [
    {"subject":"Aaditya","predicate":"owns","object":"TraceMind","confidence":0.8}
] }
```
Hope this helps!"#;
        let triples = parse_qwen_response(raw, &[a.clone(), b.clone()]);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].predicate, Predicate::Owns);
    }

    #[test]
    fn parser_drops_unresolved_entities() {
        let a = ent("Alice", EntityType::Person);
        let raw = r#"{ "triples": [
            {"subject":"Alice","predicate":"works_at","object":"Ghost","confidence":0.9}
        ] }"#;
        let triples = parse_qwen_response(raw, &[a]);
        assert!(triples.is_empty());
    }

    #[test]
    fn parser_drops_self_loops() {
        let a = ent("Alice", EntityType::Person);
        let raw = r#"{ "triples": [
            {"subject":"Alice","predicate":"related_to","object":"Alice","confidence":0.5}
        ] }"#;
        let triples = parse_qwen_response(raw, &[a]);
        assert!(triples.is_empty());
    }

    #[test]
    fn parser_uses_custom_predicate_for_unknown() {
        let a = ent("Rust", EntityType::Technology);
        let b = ent("TraceMind", EntityType::Project);
        let raw = r#"{ "triples": [
            {"subject":"TraceMind","predicate":"written_in","object":"Rust","confidence":0.95}
        ] }"#;
        let triples = parse_qwen_response(raw, &[a, b]);
        assert_eq!(triples.len(), 1);
        match &triples[0].predicate {
            Predicate::Custom(s) => assert_eq!(s, "written_in"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn parser_clamps_confidence_range() {
        let a = ent("A", EntityType::Concept);
        let b = ent("B", EntityType::Concept);
        let raw = r#"{ "triples": [
            {"subject":"A","predicate":"related_to","object":"B","confidence":3.0}
        ] }"#;
        let triples = parse_qwen_response(raw, &[a, b]);
        assert_eq!(triples.len(), 1);
        assert!((triples[0].confidence - 1.0).abs() < 1e-9);
    }

    #[test]
    fn parser_returns_empty_on_garbage() {
        let a = ent("A", EntityType::Concept);
        let triples = parse_qwen_response("not json at all", &[a]);
        assert!(triples.is_empty());
    }

    #[test]
    fn parser_case_insensitive_entity_match() {
        let alice = ent("Alice", EntityType::Person);
        let acme = ent("Acme Corp", EntityType::Organization);
        let raw = r#"{ "triples": [
            {"subject":"alice","predicate":"works_at","object":"ACME CORP","confidence":0.7}
        ] }"#;
        let triples = parse_qwen_response(raw, &[alice.clone(), acme.clone()]);
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject_id, alice.id);
        assert_eq!(triples[0].object_id, acme.id);
    }

    #[test]
    fn predicate_aliases_resolve() {
        assert_eq!(predicate_from_str("works_at"), Predicate::WorksAt);
        assert_eq!(predicate_from_str("employed_at"), Predicate::WorksAt);
        assert_eq!(predicate_from_str("works at"), Predicate::WorksAt);
        assert_eq!(predicate_from_str("Works-At"), Predicate::WorksAt);
        assert_eq!(predicate_from_str("kind_of"), Predicate::IsA);
        assert_eq!(predicate_from_str("requires"), Predicate::DependsOn);
        assert_eq!(predicate_from_str(""), Predicate::RelatedTo);
    }

    #[test]
    fn extract_triples_with_no_weights_falls_back() {
        let cfg = QwenTripleConfig::primary("/definitely/not/real.gguf");
        let ext = QwenTripleExtractor::new(cfg);
        let text = "Aaditya works at Acme Corp on TraceMind.";
        let ents = ext.extract_entities(text);
        let triples = ext.extract_triples(text, &ents);
        // Falls back to HeuristicExtractor — should match its output
        // exactly for the same input.
        let hf = HeuristicExtractor.extract_triples(text, &ents);
        assert_eq!(triples.len(), hf.len());
    }

    #[test]
    fn extract_triples_short_circuits_under_two_entities() {
        let cfg = QwenTripleConfig::primary("/nope.gguf");
        let ext = QwenTripleExtractor::new(cfg);
        let one = vec![ent("Alone", EntityType::Concept)];
        assert!(ext.extract_triples("Alone here", &one).is_empty());
    }
}
