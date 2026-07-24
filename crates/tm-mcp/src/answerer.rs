//! Glue between `tm-retrieval` and `tm-answer` for the MCP server.
//!
//! Mirrors `tm-cli/src/answerer.rs` — kept in-binary so neither crate has to
//! pull a transitive dep on the other's internals.

use std::sync::Arc;

use tm_answer::{
    AnswerBackend, AnswerRequest, ExtractiveBackend, GroundingChunk, TaskKind, TieredAnswerer,
};
use tm_retrieval::RetrievalResult;

/// Construct the default answerer for the MCP server: Tier 0 always; Tier 1
/// when the `local-llm` feature is compiled in.
pub fn build_answerer() -> TieredAnswerer {
    let extractive: Arc<dyn AnswerBackend> = Arc::new(ExtractiveBackend::default());
    #[allow(unused_mut)]
    let mut t = TieredAnswerer::new(extractive);

    #[cfg(feature = "local-llm")]
    {
        let cfg = tm_answer::LocalLlmConfig::primary(tm_answer::default_model_path());
        let backend: Arc<dyn AnswerBackend> = Arc::new(tm_answer::LocalLlmBackend::new(cfg));
        t = t.with_local_llm(backend);
    }

    t
}

/// Map a [`RetrievalResult`] into a grounding bundle. See
/// `tm-cli/src/answerer.rs` for the rationale.
pub fn grounding_from(result: &RetrievalResult, max_chunks: usize) -> Vec<GroundingChunk> {
    let cap = max_chunks.max(1);
    let mut out: Vec<GroundingChunk> = Vec::new();

    for hit in result.signal_hits.iter().take(cap) {
        out.push(GroundingChunk {
            trace_id: format!("signal:{}", hit.signal_id),
            entity_ids: Vec::new(),
            text: hit.text.clone(),
            score: hit.score,
        });
    }

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

pub fn short_answer_request(question: &str, grounding: Vec<GroundingChunk>) -> AnswerRequest {
    AnswerRequest::new(question.to_string(), TaskKind::ShortAnswer)
        .with_grounding(grounding)
        .with_max_tokens(256)
}

/// Build the answer request, attaching the topics the system *does* know
/// about so an empty result becomes a useful abstention rather than an
/// empty string. See `tm_answer::extractive::abstain`.
pub fn short_answer_request_with_context(
    question: &str,
    result: &tm_retrieval::RetrievalResult,
    grounding: Vec<GroundingChunk>,
) -> AnswerRequest {
    let mut req = short_answer_request(question, grounding);
    req.nearby_topics = result
        .related_entities
        .iter()
        .map(|r| r.name.clone())
        .chain(result.entities.iter().map(|e| e.name.clone()))
        .take(5)
        .collect();
    req
}
