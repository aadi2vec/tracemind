//! Tier 1 — bundled local LLM (Qwen 2.5 1.5B Q4_K_M primary).
//!
//! Wires `llama-cpp-2` behind the [`AnswerBackend`] trait. Three things make
//! this safe to ship as the default Tier:
//!
//! 1. **Lazy load.** The model is *not* loaded at [`LocalLlmBackend::new`].
//!    First call to [`AnswerBackend::answer`] mmaps the weights and builds a
//!    fresh context.
//! 2. **Idle unload.** A model that hasn't served a request for
//!    [`LocalLlmConfig::idle_unload_secs`] is dropped on the next call,
//!    reclaiming ~1.2 GB of resident memory.
//! 3. **Prompt templates per [`TaskKind`].** Structured tasks
//!    (extraction / contradiction / summarization) use tight Qwen
//!    chat-template prompts; open-ended tasks get a longer instruction
//!    block. Output token budget is taken from
//!    [`AnswerRequest::max_output_tokens`].
//!
//! Lifecycle:
//! - weights live under `~/.tracemind/models/<name>-<hash>.gguf`
//!   (or wherever [`LocalLlmConfig::model_path`] points)
//! - mmap'd via llama.cpp; the OS pages out clean pages under memory pressure
//! - ~1.2 GB active RAM with KV cache, ~700 MB idle (mmap only after unload)
//!
//! See `docs/PHASE3.md §2` for the full strategy.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
#[cfg(feature = "local-llm")]
use std::time::Duration;
#[cfg(feature = "local-llm")]
use tracing::info;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{
    AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, GroundingChunk, Result,
    TaskKind,
};

/// Approximate download size for the primary Qwen 2.5 1.5B Q4_K_M weights.
pub const QWEN_1_5B_Q4_APPROX_BYTES: u64 = 940 * 1024 * 1024;

/// HuggingFace repo + filename for the primary GGUF. Matches the repo we
/// point first-run users at. The 0.5B variant lives at the same repo prefix
/// with a different filename — see [`LocalLlmConfig::mobile`].
pub const HF_REPO_PRIMARY: &str = "Qwen/Qwen2.5-1.5B-Instruct-GGUF";
pub const HF_FILE_PRIMARY: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";

pub const HF_REPO_MOBILE: &str = "Qwen/Qwen2.5-0.5B-Instruct-GGUF";
pub const HF_FILE_MOBILE: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";

/// Configuration for the local LLM backend.
#[derive(Debug, Clone)]
pub struct LocalLlmConfig {
    /// Absolute path to the GGUF weights file.
    pub model_path: PathBuf,
    /// HuggingFace repo to download from on first use (when the file at
    /// `model_path` is missing). `None` disables auto-download.
    pub hf_repo: Option<String>,
    /// Filename within `hf_repo` to fetch.
    pub hf_file: Option<String>,
    /// Unload the model from RAM after this many seconds of inactivity.
    pub idle_unload_secs: u64,
    /// Hard cap on concurrent inferences (1 is correct for a single-user Mac).
    pub max_concurrency: u32,
    /// Context window in tokens. Qwen 2.5 1.5B supports 32k native; we cap at
    /// 4k to keep KV cache bounded.
    pub n_ctx: u32,
    /// Threads for CPU inference. 0 → llama.cpp picks `num_cpus / 2`.
    pub n_threads: i32,
    /// GPU layers to offload (Metal on Mac). 0 → CPU-only. -1 → all.
    pub n_gpu_layers: i32,
}

impl LocalLlmConfig {
    /// Default desktop configuration: Qwen 2.5 1.5B Q4_K_M at the canonical
    /// path under `~/.tracemind/models/`.
    pub fn primary(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            hf_repo: Some(HF_REPO_PRIMARY.to_string()),
            hf_file: Some(HF_FILE_PRIMARY.to_string()),
            idle_unload_secs: 300,
            max_concurrency: 1,
            n_ctx: 4096,
            n_threads: 0,
            // -1 (all-Metal) on Apple Silicon when the metal feature is on,
            // else 0 (CPU). The runtime guards against unsupported requests.
            n_gpu_layers: default_gpu_layers(),
        }
    }

    /// Mobile-class configuration: Qwen 2.5 0.5B Q4_K_M, smaller context.
    pub fn mobile(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            hf_repo: Some(HF_REPO_MOBILE.to_string()),
            hf_file: Some(HF_FILE_MOBILE.to_string()),
            idle_unload_secs: 120,
            max_concurrency: 1,
            n_ctx: 2048,
            n_threads: 0,
            n_gpu_layers: default_gpu_layers(),
        }
    }

    /// Construct from an explicit path with all other fields default-primary.
    /// Kept for backward compatibility with the original scaffold.
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self::primary(model_path)
    }
}

const fn default_gpu_layers() -> i32 {
    // Build flag chosen at compile time; runtime verifies metal is actually
    // initialized via `LlamaBackend`.
    if cfg!(all(target_os = "macos", feature = "local-llm-metal")) {
        -1
    } else {
        0
    }
}

/// Resolve the canonical model path under `~/.tracemind/models/`. Useful for
/// callers that don't want to hard-code the path (CLI, MCP, Tauri).
pub fn default_model_path() -> PathBuf {
    let base = std::env::var_os("TM_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs_home_dir().map(|h| h.join(".tracemind")))
        .unwrap_or_else(|| PathBuf::from(".tracemind"));
    base.join("models").join(HF_FILE_PRIMARY)
}

fn dirs_home_dir() -> Option<PathBuf> {
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
// Backend
// ---------------------------------------------------------------------------

pub struct LocalLlmBackend {
    config: LocalLlmConfig,
    #[cfg_attr(not(feature = "local-llm"), allow(dead_code))]
    inner: Arc<Mutex<Inner>>,
}

/// Held under the mutex. Loaded lazily; dropped on idle.
#[cfg_attr(not(feature = "local-llm"), allow(dead_code))]
struct Inner {
    /// `None` until first inference (or after idle-unload).
    model: Option<LoadedModel>,
    /// Wall-clock of the last successful inference. Drives idle-unload.
    last_used: Option<Instant>,
}

#[cfg(feature = "local-llm")]
struct LoadedModel {
    /// Owned model handle. The backing GGUF stays mmap'd while this lives.
    /// We deliberately do NOT keep `LlamaBackend` here — that's a process-
    /// wide singleton (see [`shared_backend`]) because llama.cpp errors on
    /// double-init.
    model: llama_cpp_2::model::LlamaModel,
}

// Stub variant when feature is off — keeps the type system happy if anyone
// flips the feature off mid-build. `inner` only ever holds None in that case.
#[cfg(not(feature = "local-llm"))]
struct LoadedModel;

/// Process-wide [`LlamaBackend`] handle. llama.cpp's backend init is a
/// once-only operation guarded by an atomic — calling `init` twice returns
/// `BackendAlreadyInitialized`. We work around that by initializing exactly
/// once via a [`std::sync::OnceLock`] and sharing the resulting handle
/// across all backend instances and across idle-unload / lazy-reload cycles.
///
/// The init result (success or failure) is cached: a host that fails init
/// once will keep returning the same error rather than thrashing on retries.
#[cfg(feature = "local-llm")]
fn shared_backend() -> Result<&'static llama_cpp_2::llama_backend::LlamaBackend> {
    use std::sync::OnceLock;
    static BACKEND: OnceLock<std::result::Result<llama_cpp_2::llama_backend::LlamaBackend, String>> =
        OnceLock::new();
    let entry = BACKEND.get_or_init(|| {
        llama_cpp_2::llama_backend::LlamaBackend::init()
            .map_err(|e| format!("LlamaBackend::init: {e}"))
    });
    match entry {
        Ok(b) => Ok(b),
        Err(s) => Err(AnswerError::ModelLoad(s.clone())),
    }
}

impl LocalLlmBackend {
    /// Construct the backend. Does **not** load the model — load happens on
    /// first [`AnswerBackend::answer`] call.
    pub fn new(config: LocalLlmConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner {
                model: None,
                last_used: None,
            })),
        }
    }

    /// True if the weights file exists on disk. Used by [`Self::availability`]
    /// and by the first-run download UX.
    pub fn weights_present(&self) -> bool {
        self.config.model_path.exists()
    }

    /// Download the configured weights from HuggingFace Hub if not already
    /// present at `model_path`. Returns the resolved on-disk path.
    ///
    /// No-op when `hf_repo` / `hf_file` are unset.
    #[cfg(feature = "local-llm")]
    pub fn ensure_weights(&self) -> Result<PathBuf> {
        if self.weights_present() {
            return Ok(self.config.model_path.clone());
        }
        let (repo, file) = match (&self.config.hf_repo, &self.config.hf_file) {
            (Some(r), Some(f)) => (r.clone(), f.clone()),
            _ => {
                return Err(AnswerError::ModelLoad(format!(
                    "weights missing at {} and no hf_repo configured",
                    self.config.model_path.display()
                )));
            }
        };

        info!(
            "[answer/local-llm] downloading {}/{} → {}",
            repo,
            file,
            self.config.model_path.display()
        );

        let api = hf_hub::api::sync::Api::new()
            .map_err(|e| AnswerError::ModelLoad(format!("hf-hub init: {e}")))?;
        let cached = api
            .model(repo.clone())
            .get(&file)
            .map_err(|e| AnswerError::ModelLoad(format!("hf-hub fetch {repo}/{file}: {e}")))?;

        if let Some(parent) = self.config.model_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // hf-hub caches under ~/.cache/huggingface; we hard-link or copy into
        // the configured path so users see the model under ~/.tracemind/models.
        if cached != self.config.model_path {
            if std::fs::hard_link(&cached, &self.config.model_path).is_err() {
                std::fs::copy(&cached, &self.config.model_path)?;
            }
        }
        info!(
            "[answer/local-llm] weights ready at {}",
            self.config.model_path.display()
        );
        Ok(self.config.model_path.clone())
    }

    #[cfg(not(feature = "local-llm"))]
    pub fn ensure_weights(&self) -> Result<PathBuf> {
        Err(AnswerError::Unavailable(
            "tm-answer built without `local-llm` feature".into(),
        ))
    }
}

#[async_trait]
impl AnswerBackend for LocalLlmBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::LocalLlm
    }

    fn availability(&self) -> BackendAvailability {
        if self.weights_present() {
            BackendAvailability::Ready
        } else {
            BackendAvailability::NeedsDownload {
                approx_bytes: QWEN_1_5B_Q4_APPROX_BYTES,
            }
        }
    }

    async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse> {
        run_inference(self, req).await
    }
}

// ---------------------------------------------------------------------------
// Inference
// ---------------------------------------------------------------------------

#[cfg(feature = "local-llm")]
async fn run_inference(
    backend: &LocalLlmBackend,
    req: &AnswerRequest,
) -> Result<AnswerResponse> {
    use llama_cpp_2::{
        context::params::LlamaContextParams,
        model::{params::LlamaModelParams, AddBos, LlamaModel, Special},
        sampling::LlamaSampler,
    };

    if !backend.weights_present() {
        return Err(AnswerError::Unavailable(format!(
            "weights not present at {}; call ensure_weights() first",
            backend.config.model_path.display()
        )));
    }

    let prompt = build_prompt(req);
    let max_new = req.max_output_tokens.max(8) as i32;
    let cfg = backend.config.clone();
    let inner = backend.inner.clone();
    let citations = citations_for(&req.grounding);
    let started = Instant::now();

    // llama-cpp-2 inference is fully blocking; hop to a blocking thread.
    let text = tokio::task::spawn_blocking(move || -> Result<String> {
        let mut guard = inner.blocking_lock();

        // Idle-unload: drop the model if it's gone cold.
        if let Some(last) = guard.last_used {
            if last.elapsed() > Duration::from_secs(cfg.idle_unload_secs) {
                info!(
                    "[answer/local-llm] idle > {}s → unloading",
                    cfg.idle_unload_secs
                );
                guard.model = None;
            }
        }

        // Process-wide backend (idempotent across idle-unload).
        let lb = shared_backend()?;

        // Lazy load.
        if guard.model.is_none() {
            let gpu_layers = cfg.n_gpu_layers.max(0) as u32;
            let mp = LlamaModelParams::default().with_n_gpu_layers(gpu_layers);
            let model = LlamaModel::load_from_file(lb, &cfg.model_path, &mp)
                .map_err(|e| AnswerError::ModelLoad(format!("load gguf: {e}")))?;
            guard.model = Some(LoadedModel { model });
            info!(
                "[answer/local-llm] loaded {} (n_ctx={}, n_gpu_layers={})",
                cfg.model_path.display(),
                cfg.n_ctx,
                cfg.n_gpu_layers
            );
        }
        let loaded = guard.model.as_ref().expect("model just loaded");

        // Build a fresh context per request — KV cache lives only for one answer.
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(cfg.n_ctx))
            .with_n_threads(if cfg.n_threads > 0 { cfg.n_threads } else { num_threads() });
        let mut ctx = loaded
            .model
            .new_context(lb, ctx_params)
            .map_err(|e| AnswerError::ModelLoad(format!("new_context: {e}")))?;

        // Tokenize prompt.
        let tokens = loaded
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| AnswerError::Inference(format!("tokenize: {e}")))?;

        let mut batch = llama_cpp_2::llama_batch::LlamaBatch::new(tokens.len().max(8), 1);
        let last_index = tokens.len().saturating_sub(1) as i32;
        for (i, tok) in tokens.iter().enumerate() {
            batch
                .add(*tok, i as i32, &[0], i as i32 == last_index)
                .map_err(|e| AnswerError::Inference(format!("batch.add: {e}")))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| AnswerError::Inference(format!("prompt decode: {e}")))?;

        // Sampler chain: temperature 0.2 for grounded answers + greedy tail.
        let mut sampler = LlamaSampler::chain_simple([
            LlamaSampler::temp(0.2),
            LlamaSampler::greedy(),
        ]);

        let mut out = String::new();
        let mut n_cur = batch.n_tokens();
        let eos = loaded.model.token_eos();

        for _ in 0..max_new {
            let new_token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(new_token);
            if new_token == eos {
                break;
            }
            let piece = loaded
                .model
                .token_to_str(new_token, Special::Tokenize)
                .unwrap_or_default();
            out.push_str(&piece);
            // Bail early on common stop strings. Cheaper than a full grammar.
            if out.ends_with("<|im_end|>") {
                out.truncate(out.len() - "<|im_end|>".len());
                break;
            }

            batch.clear();
            batch
                .add(new_token, n_cur, &[0], true)
                .map_err(|e| AnswerError::Inference(format!("batch.add (gen): {e}")))?;
            n_cur += 1;
            ctx.decode(&mut batch)
                .map_err(|e| AnswerError::Inference(format!("gen decode: {e}")))?;
        }

        guard.last_used = Some(Instant::now());
        Ok(out.trim().to_string())
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

#[cfg(not(feature = "local-llm"))]
async fn run_inference(
    _backend: &LocalLlmBackend,
    _req: &AnswerRequest,
) -> Result<AnswerResponse> {
    Err(AnswerError::Unavailable(
        "tm-answer built without `local-llm` feature".into(),
    ))
}

#[cfg(feature = "local-llm")]
fn num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() as i32 / 2).max(1))
        .unwrap_or(2)
}

// ---------------------------------------------------------------------------
// Prompt templates
// ---------------------------------------------------------------------------

/// Build a Qwen-formatted chat prompt for the given request. Public so tests
/// (and downstream tooling) can golden-test the wire format without booting
/// llama.cpp.
pub fn build_prompt(req: &AnswerRequest) -> String {
    let system = system_prompt_for(req.task);
    let user = user_prompt_for(req);
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"
    )
}

fn system_prompt_for(task: TaskKind) -> &'static str {
    match task {
        TaskKind::ShortAnswer => {
            "You are TraceMind, a local-only memory assistant. Answer the user's \
             question using ONLY the supplied memories. Be concise (1–3 \
             sentences). Cite memories inline as [1], [2] matching their order. \
             If the memories don't answer the question, say so."
        }
        TaskKind::OpenEndedSynthesis => {
            "You are TraceMind, a local-only memory assistant. Synthesize a \
             grounded answer from the supplied memories. Use prose, cite \
             inline as [1], [2]. Stay within the evidence — never invent \
             facts that aren't in the memories."
        }
        TaskKind::Summarization => {
            "You are TraceMind. Produce a tight extractive summary of the \
             supplied memory. Preserve named entities verbatim. No prose, no \
             commentary — just the summary."
        }
        TaskKind::StructuredExtraction => {
            "You are TraceMind. Extract entities and relations from the input \
             as compact JSON: {\"entities\":[{\"name\":..., \"type\":...}], \
             \"triples\":[[\"subj\",\"pred\",\"obj\"]]}. Output JSON only — \
             no prose, no markdown fences."
        }
        TaskKind::ContradictionCheck => {
            "You are TraceMind. Decide whether the two statements contradict. \
             Respond with one line: `contradiction: yes|no|unclear` followed \
             by a one-sentence reason. No other text."
        }
    }
}

fn user_prompt_for(req: &AnswerRequest) -> String {
    let mut s = String::new();
    if !req.grounding.is_empty() {
        s.push_str("Memories:\n");
        for (i, c) in req.grounding.iter().enumerate() {
            // Truncate per chunk to keep total prompt < n_ctx-1024 for typical
            // grounding sets. 480 chars × 12 chunks ≈ 5.7 KB ≈ 1.4k tokens.
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

#[cfg_attr(not(feature = "local-llm"), allow(dead_code))]
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskKind;

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
        let cfg = LocalLlmConfig::new("/nonexistent/model.gguf");
        let backend = LocalLlmBackend::new(cfg);
        match backend.availability() {
            BackendAvailability::NeedsDownload { approx_bytes } => {
                assert_eq!(approx_bytes, QWEN_1_5B_Q4_APPROX_BYTES);
            }
            other => panic!("expected NeedsDownload, got {other:?}"),
        }
    }

    #[test]
    fn default_model_path_under_data_dir() {
        let p = default_model_path();
        let s = p.to_string_lossy();
        assert!(s.ends_with(HF_FILE_PRIMARY), "got {}", s);
        assert!(s.contains("models"), "got {}", s);
    }

    #[test]
    fn primary_config_picks_primary_repo() {
        let cfg = LocalLlmConfig::primary("/x/y.gguf");
        assert_eq!(cfg.hf_repo.as_deref(), Some(HF_REPO_PRIMARY));
        assert_eq!(cfg.hf_file.as_deref(), Some(HF_FILE_PRIMARY));
        assert_eq!(cfg.n_ctx, 4096);
    }

    #[test]
    fn mobile_config_picks_smaller_model() {
        let cfg = LocalLlmConfig::mobile("/x/y.gguf");
        assert_eq!(cfg.hf_repo.as_deref(), Some(HF_REPO_MOBILE));
        assert_eq!(cfg.hf_file.as_deref(), Some(HF_FILE_MOBILE));
        assert!(cfg.n_ctx <= 2048);
    }

    #[test]
    fn build_prompt_short_answer_includes_grounding_and_question() {
        let req = AnswerRequest::new("who is alice?", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", "Alice is a software engineer.")]);
        let p = build_prompt(&req);
        assert!(p.contains("<|im_start|>system"));
        assert!(p.contains("<|im_start|>user"));
        assert!(p.contains("<|im_start|>assistant"));
        assert!(p.contains("[1] Alice is a software engineer."));
        assert!(p.contains("Question: who is alice?"));
        // Cite-as-[N] instruction should be there.
        assert!(p.to_lowercase().contains("cite"));
    }

    #[test]
    fn build_prompt_open_ended_uses_synthesis_system() {
        let req = AnswerRequest::new("tell me about alice", TaskKind::OpenEndedSynthesis)
            .with_grounding(vec![chunk("t1", "Alice ships Rust.")]);
        let p = build_prompt(&req);
        assert!(p.to_lowercase().contains("synthesize"));
        assert!(p.contains("[1] Alice ships Rust."));
    }

    #[test]
    fn build_prompt_structured_extraction_demands_json_only() {
        let req = AnswerRequest::new("extract", TaskKind::StructuredExtraction)
            .with_grounding(vec![chunk("t1", "Alice met Bob in Paris.")]);
        let p = build_prompt(&req);
        let lower = p.to_lowercase();
        assert!(lower.contains("json"));
        assert!(lower.contains("entities"));
        assert!(lower.contains("triples"));
        // Must explicitly forbid prose/markdown wrapping.
        assert!(lower.contains("no prose") || lower.contains("json only"));
    }

    #[test]
    fn build_prompt_summarization_requests_tight_summary() {
        let req = AnswerRequest::new("summarize", TaskKind::Summarization)
            .with_grounding(vec![chunk("t1", "x".repeat(100).as_str())]);
        let p = build_prompt(&req);
        assert!(p.to_lowercase().contains("summary"));
        assert!(p.contains("[1]"));
    }

    #[test]
    fn build_prompt_contradiction_check_constrains_format() {
        let req = AnswerRequest::new(
            "do these contradict?",
            TaskKind::ContradictionCheck,
        )
        .with_grounding(vec![
            chunk("t1", "Alice lives in Paris."),
            chunk("t2", "Alice lives in Tokyo."),
        ]);
        let p = build_prompt(&req);
        assert!(p.to_lowercase().contains("contradiction:"));
        assert!(p.contains("[1]"));
        assert!(p.contains("[2]"));
    }

    #[test]
    fn build_prompt_with_no_grounding_says_none() {
        let req = AnswerRequest::new("hello?", TaskKind::ShortAnswer);
        let p = build_prompt(&req);
        assert!(p.contains("(none)"));
    }

    #[test]
    fn build_prompt_truncates_oversized_chunks() {
        let big = "x".repeat(2_000);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", &big)]);
        let p = build_prompt(&req);
        // Truncation marker should appear; full 2k chars must not.
        assert!(p.contains('…'));
        assert!(!p.contains(&"x".repeat(1_000)));
    }

    #[test]
    fn citations_match_grounding_order() {
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("a", "A"), chunk("b", "B"), chunk("c", "C")]);
        let cites = citations_for(&req.grounding);
        assert_eq!(cites.len(), 3);
        assert_eq!(cites[0].trace_id, "a");
        assert_eq!(cites[0].chunk_index, 0);
        assert_eq!(cites[2].trace_id, "c");
        assert_eq!(cites[2].chunk_index, 2);
    }

    #[tokio::test]
    async fn answer_returns_unavailable_until_weights_present() {
        let cfg = LocalLlmConfig::new("/nonexistent/model.gguf");
        let backend = LocalLlmBackend::new(cfg);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::Unavailable(_)));
    }

    /// Live smoke test: requires the actual Qwen 2.5 1.5B Q4 weights at the
    /// default path. Skipped in CI; run locally with:
    ///
    /// ```text
    /// cargo test -p tm-answer --features local-llm \
    ///     --release -- --ignored live_inference_short_answer
    /// ```
    #[cfg(feature = "local-llm")]
    #[tokio::test]
    #[ignore]
    async fn live_inference_short_answer() {
        let path = default_model_path();
        if !path.exists() {
            eprintln!(
                "skipping: model not at {} — run `tracemind models pull` first",
                path.display()
            );
            return;
        }
        let backend = LocalLlmBackend::new(LocalLlmConfig::primary(&path));
        let req = AnswerRequest::new(
            "Who built the Eiffel Tower?",
            TaskKind::ShortAnswer,
        )
        .with_grounding(vec![chunk(
            "t1",
            "The Eiffel Tower was built by Gustave Eiffel's company in 1889.",
        )])
        .with_max_tokens(64);
        let resp = backend.answer(&req).await.expect("inference ok");
        assert_eq!(resp.tier, AnswerTier::LocalLlm);
        assert!(!resp.text.is_empty());
        assert!(
            resp.text.to_lowercase().contains("eiffel"),
            "expected the answer to mention Eiffel, got: {}",
            resp.text
        );
        assert_eq!(resp.citations.len(), 1);
    }
}

