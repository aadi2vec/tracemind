//! Glue between `tm-retrieval` and `tm-answer`.
//!
//! Builds a [`TieredAnswerer`] (Tier 0 always; Tier 1 when the `local-llm`
//! feature is on) and converts a [`RetrievalResult`] into the
//! [`Vec<GroundingChunk>`] the answer layer expects.
//!
//! Kept tiny on purpose — neither `tm-cli` nor `tm-answer` should grow a
//! dependency on the other's internals.

use std::sync::Arc;

use tm_answer::{
    AnswerBackend, AnswerRequest, AnswerResponse, ExtractiveBackend, GroundingChunk, TaskKind,
    TieredAnswerer,
};
use tm_retrieval::RetrievalResult;

/// Construct the default answerer for the CLI: Tier 0 always, Tier 1 when
/// the `local-llm` feature is compiled in (and weights are available — the
/// dispatcher downgrades transparently otherwise).
pub fn build_answerer() -> TieredAnswerer {
    let extractive: Arc<dyn AnswerBackend> = Arc::new(ExtractiveBackend::default());
    #[allow(unused_mut)]
    let mut t = TieredAnswerer::new(extractive);

    #[cfg(feature = "local-llm")]
    {
        // Default config points at the laptop-tier weights under the
        // active TM_DATA_DIR. Availability resolves to `NeedsDownload`
        // until `tracemind models pull` (or the first call) drops the
        // GGUF in place.
        let cfg = tm_answer::LocalLlmConfig::primary(tm_answer::default_model_path());
        let backend: Arc<dyn AnswerBackend> = Arc::new(tm_answer::LocalLlmBackend::new(cfg));
        t = t.with_local_llm(backend);
    }

    t
}

/// Map a [`RetrievalResult`] into the grounding bundle the answer layer
/// consumes. Strategy:
///
/// 1. Prefer raw signal hits when present (these are the actual captured
///    sentences — much higher information density than entity names).
/// 2. Fall back to entities + triples summarised as compact factual lines
///    when no signals match.
///
/// `max_chunks` caps the bundle size; the LLM prompt budget is what really
/// constrains us downstream.
pub fn grounding_from(result: &RetrievalResult, max_chunks: usize) -> Vec<GroundingChunk> {
    let cap = max_chunks.max(1);
    let mut out: Vec<GroundingChunk> = Vec::new();

    // Signal hits carry verbatim text + provenance — best grounding.
    for hit in result.signal_hits.iter().take(cap) {
        out.push(GroundingChunk {
            trace_id: format!("signal:{}", hit.signal_id),
            entity_ids: Vec::new(),
            text: hit.text.clone(),
            score: hit.score,
        });
    }

    // If we still have budget, attach entity / triple summaries.
    if out.len() < cap {
        let name_of: std::collections::HashMap<uuid::Uuid, String> = result
            .entities
            .iter()
            .map(|e| (e.id, e.name.clone()))
            .collect();

        for triple in result.triples.iter() {
            if out.len() >= cap {
                break;
            }
            let subj = match name_of.get(&triple.subject_id) {
                Some(n) => n,
                None => continue,
            };
            let obj = match name_of.get(&triple.object_id) {
                Some(n) => n,
                None => continue,
            };
            out.push(GroundingChunk {
                trace_id: format!("triple:{}", triple.id),
                entity_ids: vec![
                    triple.subject_id.to_string(),
                    triple.object_id.to_string(),
                ],
                text: format!("{} {} {}.", subj, triple.predicate, obj),
                score: triple.confidence as f32,
            });
        }

        // Last resort: bare entities so the LLM has anchors to cite.
        for ent in result.entities.iter() {
            if out.len() >= cap {
                break;
            }
            out.push(GroundingChunk {
                trace_id: format!("entity:{}", ent.id),
                entity_ids: vec![ent.id.to_string()],
                text: format!("{} ({}).", ent.name, ent.entity_type),
                score: ent.confidence as f32,
            });
        }
    }

    out
}

/// Build a default [`AnswerRequest`] for free-text queries from the CLI /
/// MCP `memory_query` paths. `ShortAnswer` is the closest fit — these are
/// grounded factual lookups, not open-ended synthesis.
pub fn short_answer_request(question: &str, grounding: Vec<GroundingChunk>) -> AnswerRequest {
    AnswerRequest::new(question.to_string(), TaskKind::ShortAnswer)
        .with_grounding(grounding)
        .with_max_tokens(256)
}

/// Run an [`AnswerRequest`] from sync code. tm-cli is `fn main()` (sync), so
/// we spin up a small current-thread runtime per call. Cheap relative to the
/// inference itself and keeps the binary out of `#[tokio::main]`.
pub fn answer_blocking(
    answerer: &TieredAnswerer,
    req: &AnswerRequest,
) -> Result<AnswerResponse, tm_answer::AnswerError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current-thread tokio runtime");
    rt.block_on(answerer.answer(req))
}
