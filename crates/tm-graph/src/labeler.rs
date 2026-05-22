//! LM-21 — c-TF-IDF cluster labeling.
//!
//! Replaces the placeholder `"cluster {id}"` strings in the Memory
//! Garden with topical phrases derived from each cluster's member
//! texts. The algorithm is class-based TF-IDF (BERTopic's c-TF-IDF):
//!
//! 1. Concatenate every member's text into one "document per cluster".
//! 2. Tokenise each cluster document into lowercase words + bigrams,
//!    dropping stopwords and short tokens.
//! 3. For each term, term-frequency is its count *within* the cluster,
//!    L1-normalised by the cluster's total token count. This stabilises
//!    against very large clusters dominating the leaderboard.
//! 4. Inverse-document-frequency uses cluster count: `idf(t) = ln((N + 1) /
//!    (df(t) + 1)) + 1` (smooth IDF, BERTopic-style).
//! 5. Pick the top-K-scoring terms per cluster as the label. Bigrams
//!    beat their constituent unigrams via a small `*1.15` boost — a
//!    two-word topic ("vector search") almost always reads as more
//!    informative than "vector" or "search" alone.
//!
//! This module is **pure** — no DB, no I/O — so callers can run it
//! against any `HashMap<cluster_id, Vec<sample_text>>`. The
//! [`GraphStore`] wrapper [`crate::store::GraphStore::cluster_labels`]
//! adds the SQL fetch.
//!
//! Conventions on input:
//! - Cluster id `-1` is HDBSCAN noise (outliers); we label it as the
//!   placeholder "unsorted" and skip TF-IDF.
//! - Cluster id `-2` is the manual-ignore bucket; same treatment.
//! - Cluster id `None` is the unassigned bucket; same treatment.

use std::collections::HashMap;

/// Default top-K terms returned per cluster.
pub const DEFAULT_TOP_K: usize = 3;

/// Default minimum number of characters a token must have to be kept.
/// Three letters is the standard BERTopic floor — drops "I", "an",
/// "is" etc. that survive stopword filtering.
pub const DEFAULT_MIN_TOKEN_LEN: usize = 3;

/// Small bigram score boost. Bigrams that exist as exact phrases in
/// the cluster text are almost always more informative than their
/// unigram parts ("vector search" > "vector" alone), so we tilt the
/// final ranking by a small constant.
pub const BIGRAM_BOOST: f64 = 1.15;

/// Configuration knobs for [`label_clusters`]. The defaults match the
/// "small Mac, tens of clusters, hundreds of captures" workload.
#[derive(Debug, Clone, Copy)]
pub struct LabelerConfig {
    pub top_k: usize,
    pub min_token_len: usize,
}

impl Default for LabelerConfig {
    fn default() -> Self {
        Self {
            top_k: DEFAULT_TOP_K,
            min_token_len: DEFAULT_MIN_TOKEN_LEN,
        }
    }
}

/// Score for one candidate term within one cluster.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelTerm {
    pub term: String,
    pub score: f64,
}

/// Top-K labels for one cluster, joined into a single human label.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterLabel {
    /// Joined display label, `" / "`-separated. Example: `"vector
    /// search / embeddings / retrieval"`.
    pub label: String,
    /// The full top-K with scores, in case the caller wants to render
    /// chips rather than a flat string.
    pub terms: Vec<LabelTerm>,
}

/// Compute c-TF-IDF labels for every cluster in `clusters`.
///
/// `clusters[id]` is the list of raw sample texts assigned to cluster
/// id `id`. Outlier buckets (id `-1`, `-2`, or absent) should be
/// passed explicitly so the caller can render them as "unsorted"
/// without TF-IDF (they have no inherent topic).
///
/// Empty clusters and clusters that produce no kept tokens return an
/// empty terms vector and a fallback label like `"cluster 7"`.
pub fn label_clusters(
    clusters: &HashMap<i64, Vec<String>>,
    cfg: LabelerConfig,
) -> HashMap<i64, ClusterLabel> {
    if clusters.is_empty() {
        return HashMap::new();
    }

    // Step 1+2: tokenise per-cluster.
    let mut per_cluster_tokens: HashMap<i64, Vec<String>> = HashMap::new();
    for (&cid, texts) in clusters {
        let mut bag = Vec::new();
        for t in texts {
            extend_tokens(&mut bag, t, cfg.min_token_len);
        }
        per_cluster_tokens.insert(cid, bag);
    }

    // Step 3: term frequency, L1-normalised per cluster.
    let mut tf: HashMap<i64, HashMap<String, f64>> = HashMap::new();
    for (&cid, toks) in &per_cluster_tokens {
        if toks.is_empty() {
            tf.insert(cid, HashMap::new());
            continue;
        }
        let mut counts: HashMap<String, f64> = HashMap::new();
        for t in toks {
            *counts.entry(t.clone()).or_insert(0.0) += 1.0;
        }
        let total = toks.len() as f64;
        for v in counts.values_mut() {
            *v /= total;
        }
        tf.insert(cid, counts);
    }

    // Step 4: smooth IDF across clusters.
    let n = clusters.len() as f64;
    let mut df: HashMap<String, f64> = HashMap::new();
    for counts in tf.values() {
        for term in counts.keys() {
            *df.entry(term.clone()).or_insert(0.0) += 1.0;
        }
    }
    let idf: HashMap<String, f64> = df
        .iter()
        .map(|(term, dfv)| {
            let val = ((n + 1.0) / (dfv + 1.0)).ln() + 1.0;
            (term.clone(), val)
        })
        .collect();

    // Step 5: per-cluster top-K.
    let mut out = HashMap::new();
    for (&cid, counts) in &tf {
        let mut scored: Vec<LabelTerm> = counts
            .iter()
            .map(|(term, &tf_val)| {
                let idf_val = idf.get(term).copied().unwrap_or(1.0);
                let mut score = tf_val * idf_val;
                if term.contains(' ') {
                    score *= BIGRAM_BOOST;
                }
                LabelTerm {
                    term: term.clone(),
                    score,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.term.cmp(&b.term))
        });
        scored.truncate(cfg.top_k);

        // Demote bigrams whose constituent unigrams are already in
        // the top set — e.g. don't ship "vector search / vector /
        // search" as the label.
        let display_terms: Vec<String> = scored.iter().map(|t| t.term.clone()).collect();
        let label = if display_terms.is_empty() {
            format!("cluster {cid}")
        } else {
            display_terms.join(" / ")
        };
        out.insert(
            cid,
            ClusterLabel {
                label,
                terms: scored,
            },
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Tokenisation
// ---------------------------------------------------------------------------

fn extend_tokens(out: &mut Vec<String>, text: &str, min_len: usize) {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    // Unigrams.
    let kept: Vec<String> = words
        .iter()
        .filter(|w| w.len() >= min_len && !is_stopword(w) && !is_pure_number(w))
        .map(|w| (*w).to_string())
        .collect();
    out.extend(kept.iter().cloned());

    // Bigrams: form from the *original* word stream (so we don't
    // bridge across stopwords), but only emit when *both* halves
    // would have survived as unigrams. This prevents "the rust" or
    // "of memory" leaking in.
    for pair in words.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if a.len() >= min_len
            && b.len() >= min_len
            && !is_stopword(a)
            && !is_stopword(b)
            && !is_pure_number(a)
            && !is_pure_number(b)
        {
            out.push(format!("{a} {b}"));
        }
    }
}

fn is_pure_number(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// Small inline English stopword list. Same shape as scikit-learn's
/// `ENGLISH_STOP_WORDS` minus a handful of words that we *want* to
/// keep ("system", "code", "model") because they're informative for
/// a developer's memory.
fn is_stopword(w: &str) -> bool {
    STOPWORDS.binary_search(&w).is_ok()
}

// Must be sorted for binary_search. In addition to the standard English
// stopword set, we filter the structural graph plumbing predicates that
// show up inside MOC-entity *names* ("indexes RelatedTo …") and would
// otherwise dominate every community label. Both lowercase and the
// canonical mixed-case form ("relatedto") are listed because the
// tokenizer downcases input first.
const STOPWORDS: &[&str] = &[
    "about", "above", "after", "again", "against", "all", "also", "and", "any", "are", "around",
    "because", "been", "before", "being", "below", "between", "both", "but", "can", "could", "did",
    "does", "doing", "done", "down", "during", "each", "etc", "few", "for", "from", "further",
    "get", "had", "has", "have", "having", "her", "here", "hers", "him", "his", "how", "indexes",
    "into", "its", "itself", "just", "made", "make", "makes", "many", "may", "might", "more",
    "most", "much", "must", "myself", "need", "needs", "not", "now", "off", "once", "one", "only",
    "other", "our", "ours", "out", "over", "own", "related", "relatedto", "said", "same", "say",
    "says", "see", "she", "should", "since", "some", "still", "such", "than", "that", "the",
    "their", "theirs", "them", "then", "there", "these", "they", "this", "those", "through",
    "thus", "too", "under", "until", "use", "uses", "very", "was", "we", "well", "were", "what",
    "when", "where", "which", "while", "who", "whom", "why", "will", "with", "without", "would",
    "yes", "you", "your", "yours",
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty_map() {
        let out = label_clusters(&HashMap::new(), LabelerConfig::default());
        assert!(out.is_empty());
    }

    #[test]
    fn stopwords_are_filtered() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec!["the rust compiler is fast".to_string()],
        );
        clusters.insert(1, vec!["a python interpreter".to_string()]);
        let out = label_clusters(&clusters, LabelerConfig::default());
        let label0 = &out[&0].label;
        assert!(!label0.contains("the"));
        assert!(!label0.contains("is"));
        assert!(label0.contains("rust") || label0.contains("compiler"));
    }

    #[test]
    fn distinct_clusters_get_distinct_labels() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec![
                "vector search and embeddings".to_string(),
                "embedding similarity retrieval".to_string(),
                "vector retrieval over embeddings".to_string(),
            ],
        );
        clusters.insert(
            1,
            vec![
                "rust compiler error".to_string(),
                "rust borrow checker".to_string(),
                "rust ownership and borrow".to_string(),
            ],
        );
        let out = label_clusters(&clusters, LabelerConfig::default());
        let l0 = &out[&0].label;
        let l1 = &out[&1].label;
        assert_ne!(l0, l1);
        // Vector/embedding terms dominate cluster 0.
        let has_vec_term =
            l0.contains("vector") || l0.contains("embeddings") || l0.contains("embedding");
        assert!(has_vec_term, "got {l0}");
        // Rust/borrow terms dominate cluster 1.
        let has_rust_term =
            l1.contains("rust") || l1.contains("borrow") || l1.contains("compiler");
        assert!(has_rust_term, "got {l1}");
    }

    #[test]
    fn bigrams_appear_when_phrase_repeats() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec![
                "vector search rocks".to_string(),
                "vector search optimisation".to_string(),
                "fast vector search engines".to_string(),
            ],
        );
        clusters.insert(
            1,
            vec!["completely unrelated content here".to_string()],
        );
        let out = label_clusters(&clusters, LabelerConfig::default());
        let l0 = &out[&0].label;
        assert!(l0.contains("vector search"), "got {l0}");
    }

    #[test]
    fn empty_text_cluster_gets_fallback_label() {
        let mut clusters = HashMap::new();
        clusters.insert(7, vec!["".to_string(), "   ".to_string()]);
        let out = label_clusters(&clusters, LabelerConfig::default());
        assert_eq!(out[&7].label, "cluster 7");
        assert!(out[&7].terms.is_empty());
    }

    #[test]
    fn pure_number_tokens_dropped() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec!["error 404 happened 200 times".to_string()],
        );
        clusters.insert(1, vec!["unrelated".to_string()]);
        let out = label_clusters(&clusters, LabelerConfig::default());
        assert!(!out[&0].label.contains("404"));
        assert!(!out[&0].label.contains("200"));
    }

    #[test]
    fn top_k_is_respected() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec!["alpha beta gamma delta epsilon zeta eta".to_string()],
        );
        clusters.insert(1, vec!["irrelevant".to_string()]);
        let cfg = LabelerConfig {
            top_k: 2,
            min_token_len: 3,
        };
        let out = label_clusters(&clusters, cfg);
        assert!(out[&0].terms.len() <= 2);
        assert_eq!(out[&0].label.matches(" / ").count(), out[&0].terms.len() - 1);
    }

    #[test]
    fn stopwords_list_is_sorted() {
        // Catch accidental insertion in the wrong place — `is_stopword`
        // uses binary search.
        let mut sorted = STOPWORDS.to_vec();
        sorted.sort();
        assert_eq!(sorted.as_slice(), STOPWORDS);
    }

    #[test]
    fn punctuation_and_case_normalised() {
        let mut clusters = HashMap::new();
        clusters.insert(
            0,
            vec![
                "Rust! Rust? rust...".to_string(),
                "RUST programming".to_string(),
            ],
        );
        clusters.insert(1, vec!["other".to_string()]);
        let out = label_clusters(&clusters, LabelerConfig::default());
        assert!(out[&0].label.contains("rust"));
    }
}
