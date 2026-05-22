//! CLU-5c — LLM-powered community labels.
//!
//! Sits on top of the c-TF-IDF labeler in `tm-graph::community`. After
//! the cheap pass writes a fallback like "custom indexes / indexes /
//! custom" for every community, we ask Tier-1 (Qwen 2.5 1.5B Q4 via
//! `tm-answer::LocalLlmBackend`) to name the largest few communities
//! with a short human phrase ("Database internals", "Anthropic
//! relationships", "Bug-hunt repro steps", …).
//!
//! Why only top-N: each call is ~150 ms on Apple Silicon, and the user
//! mostly only sees the top labels in the status line / Memory Garden.
//! Smaller communities keep the TF-IDF fallback, which is free.
//!
//! Compiled out entirely when the `local-llm` feature is off — the
//! `cmd_consolidate` caller checks `cfg!(feature = "local-llm")` and
//! skips invocation.

#![cfg(feature = "local-llm")]

use std::path::PathBuf;

use tm_answer::{
    AnswerBackend, AnswerRequest, BackendAvailability, LocalLlmBackend, LocalLlmConfig,
    TaskKind, default_model_path,
};
use tm_graph::{CommunitySample, GraphStore};

/// Summary of an LLM labeling pass.
#[derive(Debug, Default, Clone)]
pub struct LlmLabelStats {
    pub n_attempted: usize,
    pub n_succeeded: usize,
    pub n_skipped: usize,
    pub skipped_reason: Option<String>,
}

/// Run an LLM relabel pass over the top-N largest communities and
/// upsert the resulting names. Returns immediately with a skip reason
/// when weights are missing or there's nothing to label — never blocks
/// the consolidate flow on LLM errors.
pub async fn relabel_top_communities(
    db_path: &str,
    top_n: usize,
) -> LlmLabelStats {
    let mut stats = LlmLabelStats::default();

    // Pull samples first so we don't load the model just to discover
    // there's nothing to do. Scope the (`!Send`) `GraphStore` to a
    // synchronous block so it never crosses an `.await`.
    let samples = {
        let graph = match GraphStore::open(db_path) {
            Ok(g) => g,
            Err(e) => {
                stats.skipped_reason = Some(format!("open graph: {e}"));
                return stats;
            }
        };
        match graph.top_community_samples(top_n, 12, 8) {
            Ok(v) => v,
            Err(e) => {
                stats.skipped_reason = Some(format!("top_community_samples: {e}"));
                return stats;
            }
        }
    };
    // Filter to communities meaningful enough to label. Singleton /
    // doubleton communities are noise — TF-IDF handles them fine.
    let candidates: Vec<CommunitySample> =
        samples.into_iter().filter(|s| s.size >= 3 && !s.names.is_empty()).collect();
    if candidates.is_empty() {
        stats.skipped_reason = Some("no communities above size threshold".into());
        return stats;
    }
    stats.n_attempted = candidates.len();

    // Construct backend once; idle-unload handles the cleanup.
    let model_path: PathBuf = default_model_path();
    let backend = LocalLlmBackend::new(LocalLlmConfig::primary(&model_path));
    match backend.availability() {
        BackendAvailability::Ready => {}
        BackendAvailability::NeedsDownload { .. } => {
            // Best-effort first-run download from HF Hub. Same flow the
            // ingest-side Qwen extractor uses. If this fails (offline,
            // disk full, etc.), we fall back to the TF-IDF labels —
            // never block the consolidate cycle on a network error.
            tracing::info!(
                target = %model_path.display(),
                "LLM community labeler: downloading Qwen 1.5B weights (first run, ~900MB)"
            );
            match backend.ensure_weights() {
                Ok(_) => {}
                Err(e) => {
                    stats.skipped_reason = Some(format!("ensure_weights: {e}"));
                    stats.n_skipped = candidates.len();
                    return stats;
                }
            }
        }
        BackendAvailability::Unsupported(s) => {
            stats.skipped_reason = Some(format!("unsupported: {s}"));
            stats.n_skipped = candidates.len();
            return stats;
        }
    }

    for sample in candidates {
        let prompt = build_community_label_prompt(&sample);
        let req = AnswerRequest::new(prompt, TaskKind::Summarization).with_max_tokens(32);
        match backend.answer(&req).await {
            Ok(resp) => {
                let label = sanitize_label(&resp.text);
                if label.is_empty() {
                    stats.n_skipped += 1;
                    continue;
                }
                // Reopen `GraphStore` inside a sync scope so the
                // `!Send` connection never spans the next await.
                let write_res = {
                    let graph = match GraphStore::open(db_path) {
                        Ok(g) => g,
                        Err(e) => {
                            tracing::warn!(error = %e, community_id = sample.community_id, "open graph for set_community_label");
                            stats.n_skipped += 1;
                            continue;
                        }
                    };
                    graph.set_community_label(sample.community_id, &label, "[]")
                };
                if let Err(e) = write_res {
                    tracing::warn!(error = %e, community_id = sample.community_id, "set_community_label");
                    stats.n_skipped += 1;
                    continue;
                }
                stats.n_succeeded += 1;
            }
            Err(e) => {
                tracing::warn!(error = %e, community_id = sample.community_id, "LLM label call");
                stats.n_skipped += 1;
            }
        }
    }
    stats
}

/// Build the question we hand to Qwen. Compact, structured, with a
/// clear stop signal — Summarization task uses the
/// "no prose / no commentary" system prompt.
fn build_community_label_prompt(sample: &CommunitySample) -> String {
    let names = sample
        .names
        .iter()
        .take(12)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let rels = if sample.rel_types.is_empty() {
        String::from("(none)")
    } else {
        sample
            .rel_types
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "Name the topic that ties these entities together. Return ONLY a \
         short 2-5 word phrase, Title Case, no punctuation, no quotes.\n\
         \n\
         Entities: {names}\n\
         Relationships used inside the cluster: {rels}\n\
         \n\
         Topic phrase:"
    )
}

/// Clean up the model's reply: strip whitespace, drop quotes/braces, cap
/// at 60 chars so a stray paragraph doesn't blow out the UI.
fn sanitize_label(raw: &str) -> String {
    let line = raw.lines().next().unwrap_or("");
    let trimmed = line
        .trim()
        .trim_matches(|c: char| {
            c == '"' || c == '\'' || c == '`' || c == '{' || c == '}' || c == '.'
        })
        .trim();
    let mut out = trimmed.to_string();
    if out.chars().count() > 60 {
        out = out.chars().take(57).collect::<String>() + "…";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_caps_length() {
        assert_eq!(sanitize_label("\"Hello World\"\n\n"), "Hello World");
        let long = "a".repeat(120);
        let out = sanitize_label(&long);
        assert!(out.chars().count() <= 60);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn sanitize_uses_first_line_only() {
        assert_eq!(
            sanitize_label("Database Internals\nExplanation: lots of SQL"),
            "Database Internals"
        );
    }

    #[test]
    fn prompt_includes_names_and_rels() {
        let sample = CommunitySample {
            community_id: 7,
            size: 5,
            names: vec!["Alice".into(), "Bob".into(), "Carol".into()],
            rel_types: vec!["WORKS_AT".into(), "MANAGES".into()],
        };
        let p = build_community_label_prompt(&sample);
        assert!(p.contains("Alice, Bob, Carol"));
        assert!(p.contains("WORKS_AT, MANAGES"));
        assert!(p.to_lowercase().contains("topic phrase"));
    }
}
