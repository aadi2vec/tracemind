//! BM25 lexical scoring — the sparse half of hybrid retrieval.
//!
//! Dense embeddings miss exact-token queries: a question like "Who led
//! Carol's pre-seed round?" needs the turn containing the literal token
//! "pre-seed", and a 384-dim cosine over short conversational turns will
//! happily rank three other turns above it. Conversely, lexical search
//! misses paraphrase ("Which airport did Alice fly into?" vs "Just landed
//! at Haneda"). Neither is sufficient alone, which is why production
//! retrieval is hybrid.
//!
//! This is the sparse path that H2 Q3.9 deferred out of the BGE-M3
//! integration ("sparse and multi-vector outputs deferred to Q4.9"). It is
//! implemented directly rather than via BGE-M3's learned sparse head so it
//! works with any embedder, including the offline hash embedder.
//!
//! Okapi BM25 with the standard parameterisation:
//!
//! ```text
//! score(q, d) = Σ_t IDF(t) · (f(t,d) · (k1 + 1)) / (f(t,d) + k1 · (1 - b + b · |d|/avgdl))
//! IDF(t)      = ln(1 + (N - n(t) + 0.5) / (n(t) + 0.5))
//! ```
//!
//! Scores are normalised to [0,1] against the corpus-best score for the
//! query so they compose with cosine similarity in a `ComposedIndex`.

use std::collections::HashMap;

/// Term-frequency saturation. 1.2 is the standard default.
const K1: f32 = 1.2;
/// Length-normalisation strength. 0.75 is the standard default.
const B: f32 = 0.75;

/// Tokenise into lowercase alphanumeric terms, splitting on anything else.
///
/// Possessives are stripped (`alice's` → `alice`) because conversational
/// questions are full of them and they otherwise fragment the term space.
/// Tokens containing digits or `:` are preserved whole so times (`2:58:42`),
/// money (`$2.5m`), and dates survive as single high-IDF terms — those are
/// precisely the tokens that make a turn the right answer.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == ':' || ch == '.' || ch == '$' || ch == '-' {
            cur.push(ch.to_ascii_lowercase());
        } else if ch == '\'' {
            // Keep building; the possessive is trimmed below.
            cur.push('\'');
        } else if !cur.is_empty() {
            push_token(&mut out, std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        push_token(&mut out, cur);
    }
    out
}

fn push_token(out: &mut Vec<String>, mut tok: String) {
    if let Some(stripped) = tok.strip_suffix("'s") {
        tok = stripped.to_string();
    }
    // Trim punctuation that survived the character filter (trailing periods
    // from sentence ends, stray hyphens).
    let trimmed = tok.trim_matches(|c: char| c == '.' || c == '-' || c == '\'');
    if trimmed.is_empty() {
        return;
    }
    out.push(trimmed.to_string());
}

/// A BM25 index over an in-memory document corpus.
///
/// Built per-query-batch rather than persisted: the signal corpus that
/// reaches ranking is already bounded (`search_signals` caps candidates), so
/// rebuilding is cheaper than maintaining an on-disk inverted index, and it
/// can never go stale relative to the graph.
pub struct Bm25Index {
    /// Per-document term frequencies.
    doc_terms: Vec<HashMap<String, f32>>,
    /// Per-document length in tokens.
    doc_len: Vec<f32>,
    /// Document frequency per term.
    doc_freq: HashMap<String, usize>,
    avg_len: f32,
    n_docs: usize,
}

impl Bm25Index {
    /// Build an index over `docs`. Document `i` keeps index `i` in `score`.
    pub fn build(docs: &[String]) -> Self {
        let mut doc_terms = Vec::with_capacity(docs.len());
        let mut doc_len = Vec::with_capacity(docs.len());
        let mut doc_freq: HashMap<String, usize> = HashMap::new();

        for doc in docs {
            let toks = tokenize(doc);
            let mut tf: HashMap<String, f32> = HashMap::new();
            for t in &toks {
                *tf.entry(t.clone()).or_insert(0.0) += 1.0;
            }
            for t in tf.keys() {
                *doc_freq.entry(t.clone()).or_insert(0) += 1;
            }
            doc_len.push(toks.len() as f32);
            doc_terms.push(tf);
        }

        let n_docs = docs.len();
        let avg_len = if n_docs == 0 {
            0.0
        } else {
            doc_len.iter().sum::<f32>() / n_docs as f32
        };

        Self {
            doc_terms,
            doc_len,
            doc_freq,
            avg_len,
            n_docs,
        }
    }

    /// Raw (unnormalised) BM25 score of `query` against document `idx`.
    pub fn raw_score(&self, query: &str, idx: usize) -> f32 {
        if idx >= self.n_docs || self.avg_len <= 0.0 {
            return 0.0;
        }
        let tf = &self.doc_terms[idx];
        let dl = self.doc_len[idx];
        let mut score = 0.0f32;

        for term in tokenize(query) {
            let f = match tf.get(&term) {
                Some(f) => *f,
                None => continue,
            };
            let n_t = *self.doc_freq.get(&term).unwrap_or(&0) as f32;
            let idf =
                (1.0 + (self.n_docs as f32 - n_t + 0.5) / (n_t + 0.5)).ln();
            let denom = f + K1 * (1.0 - B + B * dl / self.avg_len);
            if denom > 0.0 {
                score += idf * (f * (K1 + 1.0)) / denom;
            }
        }
        score
    }

    /// Scores for every document, normalised to [0,1] by the corpus max.
    ///
    /// Normalising per-query (rather than by a global constant) is what lets
    /// the result compose with cosine similarity: both sides then mean
    /// "how good is this relative to the best available candidate".
    pub fn normalised_scores(&self, query: &str) -> Vec<f32> {
        let raw: Vec<f32> = (0..self.n_docs).map(|i| self.raw_score(query, i)).collect();
        let max = raw.iter().cloned().fold(0.0f32, f32::max);
        if max <= 0.0 {
            return vec![0.0; self.n_docs];
        }
        raw.into_iter().map(|s| s / max).collect()
    }

    pub fn len(&self) -> usize {
        self.n_docs
    }

    pub fn is_empty(&self) -> bool {
        self.n_docs == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<String> {
        vec![
            "Alice: I'm flying to Tokyo on May 3rd for a conference.".to_string(),
            "Alice: Just landed at Haneda, exhausted.".to_string(),
            "Carol: Pre-seed round led by Sequoia, $2.5M raised.".to_string(),
            "Ethan: Finished in 2:58:42, beat my sub-3:00 goal.".to_string(),
        ]
    }

    #[test]
    fn tokenize_strips_possessive() {
        assert_eq!(tokenize("Alice's talk"), vec!["alice", "talk"]);
    }

    #[test]
    fn tokenize_preserves_times_and_money() {
        let toks = tokenize("Finished in 2:58:42 after raising $2.5M");
        assert!(toks.contains(&"2:58:42".to_string()), "got {toks:?}");
        assert!(toks.contains(&"$2.5m".to_string()), "got {toks:?}");
    }

    #[test]
    fn rare_term_outranks_common_term() {
        let docs = corpus();
        let idx = Bm25Index::build(&docs);
        let scores = idx.normalised_scores("Who led the pre-seed round?");
        // Doc 2 carries "pre-seed" and "led"; it must win.
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(best, 2, "scores = {scores:?}");
    }

    #[test]
    fn exact_time_token_matches() {
        let docs = corpus();
        let idx = Bm25Index::build(&docs);
        let scores = idx.normalised_scores("What was the 2:58:42 finish?");
        assert!(scores[3] > 0.0);
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(best, 3, "scores = {scores:?}");
    }

    #[test]
    fn normalised_scores_are_bounded() {
        let docs = corpus();
        let idx = Bm25Index::build(&docs);
        for s in idx.normalised_scores("Tokyo conference May") {
            assert!((0.0..=1.0).contains(&s), "out of range: {s}");
        }
    }

    #[test]
    fn empty_corpus_is_safe() {
        let idx = Bm25Index::build(&[]);
        assert!(idx.is_empty());
        assert!(idx.normalised_scores("anything").is_empty());
    }

    #[test]
    fn no_match_returns_zeros() {
        let docs = corpus();
        let idx = Bm25Index::build(&docs);
        let scores = idx.normalised_scores("quantum chromodynamics");
        assert!(scores.iter().all(|s| *s == 0.0), "{scores:?}");
    }
}
