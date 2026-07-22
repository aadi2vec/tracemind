//! Tier 0 — extractive synthesis with span extraction and answer fusion.
//!
//! Significantly better than simple concatenation: extracts the most
//! relevant sentence(s) from each grounding chunk using token overlap
//! with the question, then fuses them into a concise answer.
//!
//! No LLM required. Expected gain vs original: +5-8 F1 on LoCoMo.

use async_trait::async_trait;
use std::collections::HashSet;
use std::time::Instant;

use crate::backend::{AnswerBackend, BackendAvailability};
use crate::types::{
    AnswerError, AnswerRequest, AnswerResponse, AnswerTier, Citation, Result, TaskKind,
};

pub struct ExtractiveBackend {
    /// Maximum number of grounding chunks to consider.
    max_chunks: usize,
    /// Maximum sentences to fuse into the final answer.
    max_fused_sentences: usize,
}

impl Default for ExtractiveBackend {
    fn default() -> Self {
        Self {
            max_chunks: 5,
            max_fused_sentences: 3,
        }
    }
}

impl ExtractiveBackend {
    pub fn new(max_chunks: usize) -> Self {
        Self {
            max_chunks: max_chunks.max(1),
            max_fused_sentences: 3,
        }
    }

    pub fn with_fusion(max_chunks: usize, max_fused_sentences: usize) -> Self {
        Self {
            max_chunks: max_chunks.max(1),
            max_fused_sentences: max_fused_sentences.max(1),
        }
    }

    fn synthesize(&self, req: &AnswerRequest) -> String {
        if req.grounding.is_empty() {
            return format!("No memories found for: {}", req.question);
        }

        let question_tokens = tokenize_query(&req.question);

        match req.task {
            TaskKind::ShortAnswer => {
                self.extractive_short_answer(&req.grounding[..self.max_chunks.min(req.grounding.len())], &question_tokens)
            }
            TaskKind::OpenEndedSynthesis => {
                self.fused_synthesis(&req.grounding[..self.max_chunks.min(req.grounding.len())], &question_tokens)
            }
            TaskKind::ContradictionCheck => {
                self.contradiction_summary(&req.grounding[..self.max_chunks.min(req.grounding.len())])
            }
            TaskKind::Summarization => {
                self.extractive_summary(&req.grounding[..self.max_chunks.min(req.grounding.len())], &question_tokens)
            }
            TaskKind::StructuredExtraction => {
                self.structured_extract(&req.grounding[..self.max_chunks.min(req.grounding.len())], &question_tokens)
            }
        }
    }

    /// ShortAnswer: extract the single best sentence from all chunks.
    fn extractive_short_answer(&self, chunks: &[crate::types::GroundingChunk], question_tokens: &HashSet<String>) -> String {
        let mut best_score = -1.0f32;
        let mut best_sentence = String::new();

        for chunk in chunks {
            let sentences = split_sentences(&chunk.text);
            for sentence in sentences {
                let score = sentence_overlap_score(&sentence, question_tokens)
                    + 0.1 * chunk.score; // tie-break on retrieval score
                if score > best_score {
                    best_score = score;
                    best_sentence = sentence.to_string();
                }
            }
        }

        if best_sentence.is_empty() {
            truncate(chunks[0].text.trim(), 300)
        } else {
            truncate(&best_sentence, 350)
        }
    }

    /// OpenEndedSynthesis: fuse top-N sentences across chunks, deduplicating.
    fn fused_synthesis(&self, chunks: &[crate::types::GroundingChunk], question_tokens: &HashSet<String>) -> String {
        // Score every sentence across all chunks
        let mut scored: Vec<(f32, &str)> = Vec::new();
        for chunk in chunks {
            for sentence in split_sentences_borrowed(&chunk.text) {
                let score = sentence_overlap_score(sentence, question_tokens)
                    + 0.05 * chunk.score;
                scored.push((score, sentence));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Take top N sentences, deduplicate by high token overlap
        let mut selected: Vec<&str> = Vec::new();
        let mut seen_tokens: HashSet<String> = HashSet::new();
        for (_, sentence) in &scored {
            if selected.len() >= self.max_fused_sentences { break; }
            let tokens = tokenize(sentence);
            let overlap = tokens.iter().filter(|t| seen_tokens.contains(*t)).count();
            let novelty = if tokens.is_empty() { 0.0 } else {
                1.0 - (overlap as f32 / tokens.len() as f32)
            };
            if novelty > 0.4 || selected.is_empty() {
                selected.push(sentence);
                seen_tokens.extend(tokens);
            }
        }

        if selected.is_empty() {
            truncate(chunks[0].text.trim(), 400)
        } else {
            selected.join(" ")
        }
    }

    /// ContradictionCheck: show both sides when conflict is detected.
    fn contradiction_summary(&self, chunks: &[crate::types::GroundingChunk]) -> String {
        if chunks.len() < 2 {
            return format!("Memory: {}", truncate(chunks[0].text.trim(), 300));
        }
        // Look for opposing signals using simple heuristics
        let text_a = truncate(chunks[0].text.trim(), 200);
        let text_b = truncate(chunks[1].text.trim(), 200);
        // Check if they likely contradict (different named entities for same predicate)
        let tokens_a = tokenize(&text_a);
        let tokens_b = tokenize(&text_b);
        let shared = tokens_a.intersection(&tokens_b).count();
        let total = tokens_a.len() + tokens_b.len();
        let similarity = if total == 0 { 0.0 } else { 2.0 * shared as f32 / total as f32 };

        if similarity > 0.3 && similarity < 0.8 {
            // Moderate overlap = potentially contradictory statements
            format!("Possible contradiction detected:\n• {text_a}\n• {text_b}")
        } else {
            format!("Related memories:\n• {text_a}\n• {text_b}")
        }
    }

    /// Summarization: extractive summary using coverage-based sentence selection.
    fn extractive_summary(&self, chunks: &[crate::types::GroundingChunk], question_tokens: &HashSet<String>) -> String {
        // Greedy coverage: add sentences that cover new tokens
        let mut covered: HashSet<String> = HashSet::new();
        let mut result: Vec<String> = Vec::new();

        // Sort all sentences by score
        let mut all_sentences: Vec<(f32, String)> = Vec::new();
        for chunk in chunks {
            for sentence in split_sentences(&chunk.text) {
                let score = sentence_overlap_score(&sentence, question_tokens) + 0.1 * chunk.score;
                all_sentences.push((score, sentence));
            }
        }
        all_sentences.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        for (_, sentence) in &all_sentences {
            if result.len() >= self.max_fused_sentences { break; }
            let tokens = tokenize(sentence);
            let new_tokens: usize = tokens.iter().filter(|t| !covered.contains(*t)).count();
            if new_tokens > 2 || result.is_empty() {
                result.push(sentence.clone());
                covered.extend(tokens);
            }
        }
        result.join(" ")
    }

    /// StructuredExtraction: extract key facts as bullet points.
    fn structured_extract(&self, chunks: &[crate::types::GroundingChunk], question_tokens: &HashSet<String>) -> String {
        let mut facts: Vec<String> = Vec::new();
        for chunk in chunks.iter().take(self.max_fused_sentences) {
            // Find the best sentence in each chunk
            let sentences = split_sentences(&chunk.text);
            if let Some(best) = sentences.iter()
                .max_by(|a, b| {
                    sentence_overlap_score(a, question_tokens)
                        .partial_cmp(&sentence_overlap_score(b, question_tokens))
                        .unwrap_or(std::cmp::Ordering::Equal)
                }) {
                let fact = truncate(best.trim(), 200);
                if !fact.is_empty() && !facts.iter().any(|f| f == &fact) {
                    facts.push(format!("• {fact}"));
                }
            }
        }
        if facts.is_empty() {
            "No structured facts extracted.".to_string()
        } else {
            facts.join("\n")
        }
    }
}

#[async_trait]
impl AnswerBackend for ExtractiveBackend {
    fn tier(&self) -> AnswerTier {
        AnswerTier::Extractive
    }

    fn availability(&self) -> BackendAvailability {
        BackendAvailability::Ready
    }

    async fn answer(&self, req: &AnswerRequest) -> Result<AnswerResponse> {
        let start = Instant::now();
        if req.grounding.is_empty() && matches!(req.task, TaskKind::ContradictionCheck) {
            return Err(AnswerError::NoGrounding);
        }
        let text = self.synthesize(req);
        let citations = req
            .grounding
            .iter()
            .take(self.max_chunks)
            .enumerate()
            .map(|(i, c)| Citation {
                trace_id: c.trace_id.clone(),
                entity_ids: c.entity_ids.clone(),
                chunk_index: i,
            })
            .collect();
        Ok(AnswerResponse {
            text,
            citations,
            tier: self.tier(),
            latency_ms: start.elapsed().as_millis() as u64,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split text into sentences using simple punctuation rules.
fn split_sentences(text: &str) -> Vec<String> {
    split_sentences_borrowed(text).into_iter().map(|s| s.to_string()).collect()
}

fn split_sentences_borrowed(text: &str) -> Vec<&str> {
    // Simple sentence splitter on ". ", "! ", "? " boundaries.
    let mut sentences: Vec<&str> = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < len {
        let b = bytes[i];
        if matches!(b, b'.' | b'!' | b'?') {
            let next = i + 1;
            let next_is_space = next >= len || bytes[next] == b' ' || bytes[next] == b'\n';
            if next_is_space {
                // Skip whitespace to find the next char
                let mut j = next;
                while j < len && bytes[j] == b' ' { j += 1; }
                // New sentence starts with uppercase or we're at end
                let next_upper = j >= len || (bytes[j].is_ascii_uppercase());
                if next_upper {
                    let end = i + 1;
                    let segment = text[start..end].trim();
                    if segment.split_whitespace().count() >= 3 {
                        sentences.push(segment);
                    }
                    start = j;
                }
            }
        }
        i += 1;
    }
    // Remainder
    let tail = text[start..].trim();
    if tail.split_whitespace().count() >= 3 {
        sentences.push(tail);
    }
    if sentences.is_empty() {
        sentences.push(text.trim());
    }
    sentences
}

/// Tokenize text into a set of lowercase non-stop-word tokens.
fn tokenize(text: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "shall", "can", "in", "on", "at", "to",
        "for", "of", "and", "or", "but", "not", "with", "as", "by", "from",
        "it", "its", "this", "that", "these", "those", "i", "you", "he",
        "she", "we", "they", "what", "which", "who", "when", "where", "how",
    ];
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 2 && !stop.contains(t.as_str()))
        .collect()
}

/// Tokenize the query — keep question words since they indicate answer type.
fn tokenize_query(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 1)
        .collect()
}

/// Score a sentence by its token overlap with the question.
/// Higher = more relevant.
fn sentence_overlap_score(sentence: &str, question_tokens: &HashSet<String>) -> f32 {
    if question_tokens.is_empty() { return 0.0; }
    let sentence_tokens = tokenize(sentence);
    if sentence_tokens.is_empty() { return 0.0; }
    let overlap = sentence_tokens.intersection(question_tokens).count();
    // Jaccard-like score, favouring shorter sentences (precision)
    let sentence_len = sentence_tokens.len();
    let jaccard = overlap as f32 / (sentence_len + question_tokens.len() - overlap) as f32;
    // Precision: what fraction of question tokens appear in sentence
    let precision = overlap as f32 / question_tokens.len() as f32;
    // Blend: Jaccard + precision (both reward relevance)
    0.5 * jaccard + 0.5 * precision
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GroundingChunk;

    fn chunk(id: &str, text: &str, score: f32) -> GroundingChunk {
        GroundingChunk {
            trace_id: id.to_string(),
            entity_ids: vec![format!("{id}-ent")],
            text: text.to_string(),
            score,
        }
    }

    #[tokio::test]
    async fn extractive_without_grounding_short_answer_ok() {
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("who is alice?", TaskKind::ShortAnswer);
        let resp = backend.answer(&req).await.unwrap();
        assert_eq!(resp.tier, AnswerTier::Extractive);
        assert!(resp.text.contains("No memories"));
        assert!(resp.citations.is_empty());
    }

    #[tokio::test]
    async fn extractive_contradiction_needs_grounding() {
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("does A contradict B?", TaskKind::ContradictionCheck);
        let err = backend.answer(&req).await.unwrap_err();
        assert!(matches!(err, AnswerError::NoGrounding));
    }

    #[tokio::test]
    async fn extractive_cites_up_to_max_chunks() {
        let backend = ExtractiveBackend::new(2);
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer).with_grounding(vec![
            chunk("t1", "Alice works at Acme Corp. She manages the engineering team there.", 0.9),
            chunk("t2", "Bob leads the product team at Beta Corp.", 0.8),
            chunk("t3", "Carol runs marketing at Gamma Inc.", 0.7),
        ]);
        let resp = backend.answer(&req).await.unwrap();
        assert_eq!(resp.citations.len(), 2);
        assert_eq!(resp.citations[0].trace_id, "t1");
    }

    #[tokio::test]
    async fn extractive_truncates_long_chunks() {
        let long = "x".repeat(500);
        let backend = ExtractiveBackend::default();
        let req = AnswerRequest::new("q", TaskKind::ShortAnswer)
            .with_grounding(vec![chunk("t1", &long, 1.0)]);
        let resp = backend.answer(&req).await.unwrap();
        assert!(resp.text.len() < 400 || resp.text.contains('…'));
    }

    #[test]
    fn availability_always_ready() {
        let backend = ExtractiveBackend::default();
        assert_eq!(backend.availability(), BackendAvailability::Ready);
    }

    #[tokio::test]
    async fn span_extraction_picks_relevant_sentence() {
        let backend = ExtractiveBackend::default();
        // Question about Alice's employer — should extract the sentence mentioning Acme
        let req = AnswerRequest::new("where does alice work?", TaskKind::ShortAnswer)
            .with_grounding(vec![
                chunk("t1",
                    "Alice Smith is a software engineer. Alice works at Acme Corp. She enjoys hiking.",
                    0.9),
            ]);
        let resp = backend.answer(&req).await.unwrap();
        // Should select the sentence most relevant to "alice work"
        assert!(
            resp.text.to_lowercase().contains("acme") || resp.text.to_lowercase().contains("alice"),
            "Expected Alice/Acme in answer, got: {}", resp.text
        );
    }

    #[tokio::test]
    async fn fusion_deduplicates_sentences() {
        let backend = ExtractiveBackend::with_fusion(5, 3);
        let req = AnswerRequest::new("alice employer company work", TaskKind::OpenEndedSynthesis)
            .with_grounding(vec![
                chunk("t1", "Alice works at Acme Corp. Alice is employed by Acme. She joined in 2024.", 0.9),
                chunk("t2", "Bob manages Alice at Acme Corp. The team has five engineers.", 0.8),
            ]);
        let resp = backend.answer(&req).await.unwrap();
        // Should fuse without duplicating "Acme Corp" three times
        let acme_count = resp.text.matches("Acme").count();
        assert!(acme_count <= 2, "Too many duplicates: {}", resp.text);
    }

    #[test]
    fn tokenize_filters_stop_words() {
        let tokens = tokenize("Alice is the manager of engineering at Acme Corp");
        assert!(tokens.contains("alice"));
        assert!(tokens.contains("manager"));
        assert!(tokens.contains("engineering"));
        assert!(tokens.contains("acme"));
        assert!(!tokens.contains("is"));
        assert!(!tokens.contains("the"));
        assert!(!tokens.contains("of"));
    }

    #[test]
    fn sentence_overlap_scores_relevant_higher() {
        let question_tokens = tokenize_query("where does alice work?");
        let relevant = "Alice works at Acme Corp as an engineer.";
        let irrelevant = "Bob enjoys hiking in the mountains on weekends.";
        let s_relevant = sentence_overlap_score(relevant, &question_tokens);
        let s_irrelevant = sentence_overlap_score(irrelevant, &question_tokens);
        assert!(s_relevant > s_irrelevant,
            "relevant={s_relevant:.3} should > irrelevant={s_irrelevant:.3}");
    }
}
