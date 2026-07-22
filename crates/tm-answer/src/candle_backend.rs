//! Tier 1 — candle-based local LLM synthesis (pure Rust, no cmake).
//!
//! Uses Qwen2.5-0.5B-Instruct Q4_K_M via candle-transformers GGUF loader.
//! Automatically downloads the model from HuggingFace on first use.
//!
//! # Why candle instead of llama-cpp-2
//!
//! `llama-cpp-2` requires cmake at build time, which is not always available.
//! `candle` is a pure-Rust ML framework with zero C/C++ build dependencies.
//!
//! # Feature gate
//!
//! Compile with `--features candle-llm`. The default build keeps this module
//! type-present but inert so downstream crates can reference [`CandleBackend`]
//! without pulling in the ML stack.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "candle-llm")]
use std::time::Instant;
use tokio::sync::Mutex;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{
    AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, GroundingChunk, Result,
    TaskKind,
};

// ---------------------------------------------------------------------------
// Public constants — useful to callers regardless of feature gate
// ---------------------------------------------------------------------------

/// HuggingFace repo for the default Qwen2.5-0.5B Q4_K_M model.
pub const CANDLE_HF_REPO: &str = "Qwen/Qwen2.5-0.5B-Instruct-GGUF";
/// GGUF filename within the repo.
pub const CANDLE_HF_FILE: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";
/// Approximate download size in bytes (~400 MB).
pub const QWEN_0_5B_Q4_APPROX_BYTES: u64 = 400 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the candle Tier-1 backend.
#[derive(Debug, Clone)]
pub struct CandleConfig {
    /// Absolute path to the GGUF weights file.
    pub model_path: PathBuf,
    /// HuggingFace repo to download from on first use.
    pub hf_repo: String,
    /// Filename within `hf_repo` to fetch.
    pub hf_file: String,
    /// Maximum tokens to generate per answer.
    pub max_new_tokens: usize,
    /// Sampling temperature (lower = more deterministic).
    pub temperature: f64,
    /// Top-p nucleus sampling cutoff.
    pub top_p: f64,
}

impl Default for CandleConfig {
    fn default() -> Self {
        let base = std::env::var_os("TM_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".tracemind")))
            .unwrap_or_else(|| PathBuf::from(".tracemind"));
        Self {
            model_path: base.join("models").join(CANDLE_HF_FILE),
            hf_repo: CANDLE_HF_REPO.to_string(),
            hf_file: CANDLE_HF_FILE.to_string(),
            max_new_tokens: 256,
            temperature: 0.1,
            top_p: 0.95,
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Backend struct — always present so the type is usable without the feature
// ---------------------------------------------------------------------------

pub struct CandleBackend {
    config: CandleConfig,
    #[allow(dead_code)]
    inner: Arc<Mutex<InnerState>>,
}

struct InnerState {
    /// Loaded model state; `None` until first inference.
    #[cfg_attr(not(feature = "candle-llm"), allow(dead_code))]
    loaded: Option<LoadedCandle>,
}

// ---------------------------------------------------------------------------
// Loaded model — only compiled when the feature is active
// ---------------------------------------------------------------------------

#[cfg(feature = "candle-llm")]
struct LoadedCandle {
    model: candle_transformers::models::quantized_llama::ModelWeights,
    tokenizer: tokenizers::Tokenizer,
    device: candle_core::Device,
}

/// Stub when feature is off.
#[cfg(not(feature = "candle-llm"))]
struct LoadedCandle;

// ---------------------------------------------------------------------------
// Constructor + availability
// ---------------------------------------------------------------------------

impl CandleBackend {
    pub fn new(config: CandleConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(InnerState { loaded: None })),
        }
    }

    /// Convenience constructor with default model path under `~/.tracemind/`.
    pub fn default_desktop() -> Self {
        Self::new(CandleConfig::default())
    }

    pub fn weights_present(&self) -> bool {
        self.config.model_path.exists()
    }
}

#[async_trait]
impl AnswerBackend for CandleBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::LocalLlm
    }

    fn availability(&self) -> BackendAvailability {
        if self.weights_present() {
            BackendAvailability::Ready
        } else {
            BackendAvailability::NeedsDownload {
                approx_bytes: QWEN_0_5B_Q4_APPROX_BYTES,
            }
        }
    }

    async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse> {
        run_candle_inference(self, req).await
    }
}

// ---------------------------------------------------------------------------
// Inference implementation (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "candle-llm")]
async fn run_candle_inference(
    backend: &CandleBackend,
    req: &AnswerRequest,
) -> Result<AnswerResponse> {
    use candle_core::{quantized::gguf_file, Device, Tensor};
    use candle_transformers::generation::LogitsProcessor;
    use candle_transformers::models::quantized_llama::ModelWeights;

    if !backend.weights_present() {
        // Try to download the model on first use.
        if let Err(e) = download_model(&backend.config) {
            return Err(AnswerError::Unavailable(format!(
                "candle model not found at {} and download failed: {e}",
                backend.config.model_path.display()
            )));
        }
    }

    let prompt = build_candle_prompt(req);
    let max_new = backend.config.max_new_tokens;
    let temperature = backend.config.temperature;
    let top_p = backend.config.top_p;
    let cfg = backend.config.clone();
    let inner = backend.inner.clone();
    let citations = citations_for(&req.grounding);
    let started = Instant::now();

    // candle inference is blocking (synchronous tensor ops); move to a
    // blocking thread to avoid stalling the async executor.
    let text = tokio::task::spawn_blocking(move || -> Result<String> {
        let mut guard = inner.blocking_lock();

        // Lazy load: initialize model on first call.
        if guard.loaded.is_none() {
            let device = Device::Cpu;

            // Load GGUF weights.
            let mut file = std::fs::File::open(&cfg.model_path)
                .map_err(|e| AnswerError::ModelLoad(format!("open gguf: {e}")))?;
            let model_content = gguf_file::Content::read(&mut file)
                .map_err(|e| AnswerError::ModelLoad(format!("read gguf: {e}")))?;
            let model = ModelWeights::from_gguf(model_content, &mut file, &device)
                .map_err(|e| AnswerError::ModelLoad(format!("init model: {e}")))?;

            // Load tokenizer — download from HF Hub alongside the GGUF.
            let tok_path = cfg
                .model_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("qwen2.5-0.5b-tokenizer.json");
            let tokenizer = if tok_path.exists() {
                tokenizers::Tokenizer::from_file(&tok_path)
                    .map_err(|e| AnswerError::ModelLoad(format!("load tokenizer: {e}")))?
            } else {
                // Download from HuggingFace.
                let api = hf_hub::api::sync::Api::new()
                    .map_err(|e| AnswerError::ModelLoad(format!("hf-hub init: {e}")))?;
                let repo_id = "Qwen/Qwen2.5-0.5B-Instruct";
                let repo = api.model(repo_id.to_string());
                let tok_src = repo
                    .get("tokenizer.json")
                    .map_err(|e| AnswerError::ModelLoad(format!("fetch tokenizer: {e}")))?;
                // Copy into our models dir for future offline use.
                if let Some(parent) = tok_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::copy(&tok_src, &tok_path);
                tokenizers::Tokenizer::from_file(&tok_src)
                    .map_err(|e| AnswerError::ModelLoad(format!("load tokenizer: {e}")))?
            };

            tracing::info!(
                "[answer/candle] loaded Qwen2.5-0.5B from {:?}",
                cfg.model_path
            );
            guard.loaded = Some(LoadedCandle {
                model,
                tokenizer,
                device,
            });
        }

        let loaded = guard.loaded.as_mut().expect("just initialized");
        let encoding = loaded
            .tokenizer
            .encode(prompt.as_str(), true)
            .map_err(|e| AnswerError::Inference(format!("tokenize: {e}")))?;
        let input_ids: Vec<u32> = encoding.get_ids().to_vec();

        let input_tensor = Tensor::new(input_ids.as_slice(), &loaded.device)
            .map_err(|e| AnswerError::Inference(format!("tensor: {e}")))?
            .unsqueeze(0)
            .map_err(|e| AnswerError::Inference(format!("unsqueeze: {e}")))?;

        // Prefill prompt tokens.
        let logits = loaded
            .model
            .forward(&input_tensor, 0)
            .map_err(|e| AnswerError::Inference(format!("prefill forward: {e}")))?;

        let mut logits_processor =
            LogitsProcessor::new(42, Some(temperature), Some(top_p));

        // Get first generated token from prefill.
        let logits_last = logits
            .squeeze(0)
            .map_err(|e| AnswerError::Inference(format!("squeeze: {e}")))?;
        let seq_len = logits_last.dim(0)
            .map_err(|e| AnswerError::Inference(format!("dim: {e}")))?;
        let logits_last = logits_last
            .narrow(0, seq_len - 1, 1)
            .map_err(|e| AnswerError::Inference(format!("narrow: {e}")))?;

        let mut next_token = logits_processor
            .sample(&logits_last)
            .map_err(|e| AnswerError::Inference(format!("sample: {e}")))?;

        let eos_token_id = loaded
            .tokenizer
            .token_to_id("<|im_end|>")
            .or_else(|| loaded.tokenizer.token_to_id("</s>"))
            .unwrap_or(151645); // Qwen2 default EOS id

        let mut generated_ids: Vec<u32> = Vec::with_capacity(max_new);
        let prompt_len = input_ids.len();

        for _ in 0..max_new {
            if next_token == eos_token_id {
                break;
            }
            generated_ids.push(next_token);

            let next_input = Tensor::new(&[next_token], &loaded.device)
                .map_err(|e| AnswerError::Inference(format!("next tensor: {e}")))?
                .unsqueeze(0)
                .map_err(|e| AnswerError::Inference(format!("unsqueeze: {e}")))?;

            let step = prompt_len + generated_ids.len() - 1;
            let step_logits = loaded
                .model
                .forward(&next_input, step)
                .map_err(|e| AnswerError::Inference(format!("step forward: {e}")))?
                .squeeze(0)
                .map_err(|e| AnswerError::Inference(format!("step squeeze: {e}")))?;

            next_token = logits_processor
                .sample(&step_logits)
                .map_err(|e| AnswerError::Inference(format!("step sample: {e}")))?;
        }

        let text = loaded
            .tokenizer
            .decode(&generated_ids, true)
            .map_err(|e| AnswerError::Inference(format!("decode: {e}")))?;

        // Strip trailing stop strings that the model may echo.
        let text = text
            .trim_end_matches("<|im_end|>")
            .trim_end_matches("</s>")
            .trim()
            .to_string();

        Ok(text)
    })
    .await
    .map_err(|e| AnswerError::Inference(format!("blocking task panicked: {e}")))??;

    Ok(AnswerResponse {
        text,
        citations,
        tier: AnswerTier::LocalLlm,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// Stub when `candle-llm` feature is off.
#[cfg(not(feature = "candle-llm"))]
async fn run_candle_inference(
    _backend: &CandleBackend,
    _req: &AnswerRequest,
) -> Result<AnswerResponse> {
    Err(AnswerError::Unavailable(
        "tm-answer built without `candle-llm` feature".into(),
    ))
}

// ---------------------------------------------------------------------------
// Model download helper (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "candle-llm")]
fn download_model(config: &CandleConfig) -> anyhow::Result<()> {
    use hf_hub::api::sync::Api;

    if config.model_path.exists() {
        return Ok(());
    }
    if let Some(parent) = config.model_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    tracing::info!(
        "[answer/candle] downloading {}/{} from HuggingFace…",
        config.hf_repo,
        config.hf_file
    );
    let api = Api::new()?;
    let repo = api.model(config.hf_repo.clone());
    let src = repo.get(&config.hf_file)?;

    // hf-hub caches in ~/.cache/huggingface; symlink/copy into our models dir.
    if src != config.model_path {
        let target = std::fs::canonicalize(&src).unwrap_or(src.clone());
        let _ = std::fs::remove_file(&config.model_path);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, &config.model_path).is_ok();
        #[cfg(not(unix))]
        let linked = false;
        if !linked && std::fs::hard_link(&target, &config.model_path).is_err() {
            std::fs::copy(&target, &config.model_path)?;
        }
    }
    tracing::info!(
        "[answer/candle] model ready at {:?}",
        config.model_path
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Prompt builder — public for golden tests
// ---------------------------------------------------------------------------

/// Build a Qwen2.5 chat-template prompt for the given request.
///
/// The format follows `<|im_start|>role\ncontent<|im_end|>` which matches
/// Qwen2.5-Instruct's native chat template.
pub fn build_candle_prompt(req: &AnswerRequest) -> String {
    let system = candle_system_prompt(req.task);
    let user = candle_user_message(req);
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"
    )
}

fn candle_system_prompt(task: TaskKind) -> &'static str {
    match task {
        TaskKind::ShortAnswer => {
            "You are TraceMind, a local-only memory assistant. Answer the user's \
             question using ONLY the supplied memories. Be concise (1–3 sentences). \
             Cite memories inline as [1], [2]. If the memories don't answer the \
             question, say so."
        }
        TaskKind::OpenEndedSynthesis => {
            "You are TraceMind, a local-only memory assistant. Synthesize a grounded \
             answer from the supplied memories. Use prose, cite inline as [1], [2]. \
             Stay within the evidence — never invent facts."
        }
        TaskKind::Summarization => {
            "You are TraceMind. Produce a tight extractive summary. \
             Preserve named entities verbatim. No prose, no commentary."
        }
        TaskKind::StructuredExtraction => {
            "You are TraceMind. Extract entities and relations from the input as \
             compact JSON: {\"entities\":[{\"name\":...,\"type\":...}],\
             \"triples\":[[\"subj\",\"pred\",\"obj\"]]}. \
             Output JSON only — no prose, no markdown fences."
        }
        TaskKind::ContradictionCheck => {
            "You are TraceMind. Decide whether the statements contradict. \
             Respond with one line: `contradiction: yes|no|unclear` followed \
             by a one-sentence reason. No other text."
        }
    }
}

fn candle_user_message(req: &AnswerRequest) -> String {
    let mut s = String::new();
    if !req.grounding.is_empty() {
        s.push_str("Memories:\n");
        for (i, c) in req.grounding.iter().enumerate() {
            let snippet = truncate_chars(&c.text, 480);
            s.push_str(&format!("[{}] {}\n", i + 1, snippet));
        }
        s.push('\n');
    } else {
        s.push_str("Memories: (none)\n\n");
    }
    s.push_str("Question: ");
    s.push_str(req.question.trim());
    s
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg_attr(not(feature = "candle-llm"), allow(dead_code))]
fn citations_for(grounding: &[GroundingChunk]) -> Vec<Citation> {
    grounding
        .iter()
        .enumerate()
        .map(|(i, c)| Citation {
            trace_id: c.trace_id.clone(),
            entity_ids: c.entity_ids.clone(),
            chunk_index: i,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Public constructor helper
// ---------------------------------------------------------------------------

/// Create a [`CandleBackend`] with default desktop settings.
pub fn default_candle_backend() -> CandleBackend {
    CandleBackend::default_desktop()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GroundingChunk, TaskKind};

    fn chunk(id: &str, text: &str) -> GroundingChunk {
        GroundingChunk {
            trace_id: id.to_string(),
            entity_ids: vec![format!("{id}-ent")],
            text: text.to_string(),
            score: 0.9,
        }
    }

    #[test]
    fn missing_weights_reports_needs_download() {
        let cfg = CandleConfig {
            model_path: PathBuf::from("/nonexistent/model.gguf"),
            ..CandleConfig::default()
        };
        let backend = CandleBackend::new(cfg);
        match backend.availability() {
            BackendAvailability::NeedsDownload { approx_bytes } => {
                assert_eq!(approx_bytes, QWEN_0_5B_Q4_APPROX_BYTES);
            }
            other => panic!("expected NeedsDownload, got {other:?}"),
        }
    }

    #[test]
    fn default_model_path_contains_gguf_filename() {
        let backend = CandleBackend::default_desktop();
        let p = backend.config.model_path.to_string_lossy().to_string();
        assert!(p.ends_with(CANDLE_HF_FILE), "got {p}");
        assert!(p.contains("models"), "got {p}");
    }

    #[test]
    fn build_prompt_includes_system_and_grounding() {
        let req = AnswerRequest::new("who is alice?", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", "Alice is a software engineer.")]);
        let p = build_candle_prompt(&req);
        assert!(p.contains("<|im_start|>system"));
        assert!(p.contains("<|im_start|>user"));
        assert!(p.contains("<|im_start|>assistant"));
        assert!(p.contains("[1] Alice is a software engineer."));
        assert!(p.contains("Question: who is alice?"));
        assert!(p.to_lowercase().contains("cite"));
    }

    #[test]
    fn build_prompt_extraction_demands_json_only() {
        let req = AnswerRequest::new("extract", TaskKind::StructuredExtraction)
            .with_grounding(vec![chunk("t1", "Alice met Bob in Paris.")]);
        let p = build_candle_prompt(&req);
        let lower = p.to_lowercase();
        assert!(lower.contains("json"));
        assert!(lower.contains("no prose") || lower.contains("json only"));
    }

    #[test]
    fn build_prompt_contradiction_check_constrains_format() {
        let req = AnswerRequest::new("do these contradict?", TaskKind::ContradictionCheck)
            .with_grounding(vec![
                chunk("t1", "Alice lives in Paris."),
                chunk("t2", "Alice lives in Tokyo."),
            ]);
        let p = build_candle_prompt(&req);
        assert!(p.to_lowercase().contains("contradiction:"));
    }

    #[test]
    fn build_prompt_no_grounding_says_none() {
        let req = AnswerRequest::new("hello?", TaskKind::ShortAnswer);
        let p = build_candle_prompt(&req);
        assert!(p.contains("(none)"));
    }

    #[test]
    fn build_prompt_truncates_long_chunks() {
        let big = "x".repeat(2_000);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", &big)]);
        let p = build_candle_prompt(&req);
        assert!(p.contains('…'));
    }

    #[tokio::test]
    async fn answer_returns_unavailable_without_weights() {
        let cfg = CandleConfig {
            model_path: PathBuf::from("/nonexistent/model.gguf"),
            // Disable HF download by pointing to a bogus repo that won't resolve
            hf_repo: "nonexistent/repo-zzz".to_string(),
            ..CandleConfig::default()
        };
        let backend = CandleBackend::new(cfg);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::Unavailable(_)));
    }
}
