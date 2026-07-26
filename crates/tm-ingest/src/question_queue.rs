//! Question queue — I10 / N1.2.
//!
//! Users pin questions they can't answer yet. Every subsequent ingestion is
//! scanned for keyword overlap; a match becomes a Brief card ("you asked X
//! last Thursday, this document seems relevant").
//!
//! The matching is deliberately simple — case-insensitive keyword
//! intersection with a minimum overlap threshold — so it stays honest and
//! auditable. A future patch can swap the scorer for a dense embedding
//! comparison without changing the storage schema.
//!
//! Persistence uses JSON lines in `~/.tracemind/questions.jsonl`, mirroring
//! the pattern used by receipts and traces. Deletes rewrite the file (the
//! queue is tiny — dozens of items at most).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

pub const QUESTIONS_FILE_NAME: &str = "questions.jsonl";

/// A pinned open question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PinnedQuestion {
    pub id: Uuid,
    pub text: String,
    pub pinned_at: DateTime<Utc>,
    /// Set when the user marks the question as answered (or auto-closed
    /// after N days). While `resolved_at` is `None`, the question is open
    /// and continues to match new captures.
    #[serde(default)]
    pub resolved_at: Option<DateTime<Utc>>,
    /// Capture ids that already matched this question — used to avoid
    /// repeat-Brief-ing the same document.
    #[serde(default)]
    pub matched_capture_ids: Vec<Uuid>,
}

impl PinnedQuestion {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            text: text.into(),
            pinned_at: Utc::now(),
            resolved_at: None,
            matched_capture_ids: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.resolved_at.is_none()
    }

    /// Simple keyword extractor: alpha-only tokens ≥ 4 chars, lowercased.
    /// Filters common English stopwords so a question like "what is the
    /// current status" reduces to `{"current", "status"}` instead of
    /// `{"what", "is", "the", "current", "status"}`.
    pub fn keywords(&self) -> Vec<String> {
        keywordise(&self.text)
    }
}

/// Result of testing a capture against every open question in the queue.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestionMatch {
    pub question_id: Uuid,
    pub question_text: String,
    pub overlap: Vec<String>,
    pub score: f32,
}

/// Persistent, thread-safe question queue.
pub struct QuestionQueue {
    path: PathBuf,
    inner: Mutex<Vec<PinnedQuestion>>,
}

impl QuestionQueue {
    /// Open (or create) the queue in `dir`. Loads all existing questions.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let path = dir.join(QUESTIONS_FILE_NAME);
        let items = load_all(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(items),
        })
    }

    /// Pin a new question. Deduplicates by exact `text` (case-insensitive) —
    /// the second pin returns the existing entry rather than duplicating.
    pub fn pin(&self, text: impl Into<String>) -> Result<PinnedQuestion> {
        let text = text.into();
        let mut items = self.inner.lock().expect("question queue lock");
        if let Some(existing) = items
            .iter()
            .find(|q| q.text.eq_ignore_ascii_case(&text) && q.is_open())
        {
            return Ok(existing.clone());
        }
        let q = PinnedQuestion::new(text);
        items.push(q.clone());
        rewrite_all(&self.path, &items)?;
        Ok(q)
    }

    /// Return all questions (open + resolved), newest first.
    pub fn list(&self) -> Vec<PinnedQuestion> {
        let items = self.inner.lock().expect("question queue lock");
        let mut v = items.clone();
        v.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at));
        v
    }

    pub fn open_questions(&self) -> Vec<PinnedQuestion> {
        self.list().into_iter().filter(|q| q.is_open()).collect()
    }

    /// Mark the given question as resolved.
    pub fn resolve(&self, id: Uuid) -> Result<Option<PinnedQuestion>> {
        let mut items = self.inner.lock().expect("question queue lock");
        let found = items.iter_mut().find(|q| q.id == id);
        let out = if let Some(q) = found {
            q.resolved_at = Some(Utc::now());
            Some(q.clone())
        } else {
            None
        };
        rewrite_all(&self.path, &items)?;
        Ok(out)
    }

    /// Score a captured document against every open question. Any match
    /// with overlap ≥ `min_overlap` keywords is returned, sorted by score
    /// descending. Captures previously matched to a question are skipped.
    pub fn scan(
        &self,
        capture_id: Uuid,
        content: &str,
        min_overlap: usize,
    ) -> Result<Vec<QuestionMatch>> {
        let mut items = self.inner.lock().expect("question queue lock");
        let capture_keys = keywordise(content);
        if capture_keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut matches: Vec<QuestionMatch> = Vec::new();
        let mut dirty = false;
        for q in items.iter_mut() {
            if !q.is_open() {
                continue;
            }
            if q.matched_capture_ids.contains(&capture_id) {
                continue;
            }
            let qk = q.keywords();
            if qk.is_empty() {
                continue;
            }
            let overlap: Vec<String> = qk
                .iter()
                .filter(|k| capture_keys.contains(k))
                .cloned()
                .collect();
            if overlap.len() >= min_overlap.max(1) {
                let score = overlap.len() as f32 / qk.len() as f32;
                matches.push(QuestionMatch {
                    question_id: q.id,
                    question_text: q.text.clone(),
                    overlap,
                    score,
                });
                q.matched_capture_ids.push(capture_id);
                dirty = true;
            }
        }
        if dirty {
            rewrite_all(&self.path, &items)?;
        }
        matches.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(matches)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn load_all(path: &Path) -> Result<Vec<PinnedQuestion>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TraceMindError::Storage(e.to_string())),
    };
    Ok(raw
        .split('\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

fn rewrite_all(path: &Path, items: &[PinnedQuestion]) -> Result<()> {
    let mut buf = String::new();
    for q in items {
        let line = serde_json::to_string(q)?;
        buf.push_str(&line);
        buf.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    file.write_all(buf.as_bytes())
        .map_err(|e| TraceMindError::Storage(e.to_string()))?;
    Ok(())
}

const STOPWORDS: &[&str] = &[
    "what", "when", "where", "which", "whom", "whose", "that", "this", "these", "those",
    "with", "from", "have", "will", "would", "should", "could", "there", "their", "about",
    "into", "over", "under", "been", "being", "than", "then", "here", "some", "such",
    "does", "doing", "done", "just", "like", "make", "made", "your", "yours", "them",
];

fn keywordise(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Vec<String>| {
        if word.len() >= 4 && !STOPWORDS.contains(&word.as_str()) && !out.contains(word) {
            out.push(word.clone());
        }
        word.clear();
    };
    for ch in lower.chars() {
        if ch.is_ascii_alphabetic() {
            word.push(ch);
        } else {
            flush(&mut word, &mut out);
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_and_list_returns_question() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        let p = q.pin("what is our Q4 pricing strategy").unwrap();
        let list = q.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].text, p.text);
        assert!(list[0].is_open());
    }

    #[test]
    fn pin_deduplicates_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        let a = q.pin("Where did I put the tax docs?").unwrap();
        let b = q.pin("where did I PUT the tax docs?").unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(q.list().len(), 1);
    }

    #[test]
    fn resolve_marks_question_closed() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        let p = q.pin("open one").unwrap();
        let r = q.resolve(p.id).unwrap().unwrap();
        assert!(r.resolved_at.is_some());
        assert_eq!(q.open_questions().len(), 0);
    }

    #[test]
    fn scan_matches_open_questions_by_keyword_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        q.pin("what is our Q4 pricing strategy for enterprise").unwrap();
        q.pin("who owns the security review process").unwrap();

        let cid = Uuid::new_v4();
        let hits = q
            .scan(cid, "Draft: enterprise pricing tiers for Q4", 2)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].question_text.contains("Q4 pricing"));
        assert!(hits[0].overlap.contains(&"pricing".to_string()));
    }

    #[test]
    fn scan_ignores_closed_questions() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        let p = q.pin("resolved question about pricing tiers").unwrap();
        q.resolve(p.id).unwrap();
        let hits = q
            .scan(Uuid::new_v4(), "pricing tiers document", 1)
            .unwrap();
        assert_eq!(hits.len(), 0);
    }

    #[test]
    fn scan_skips_capture_already_matched() {
        let dir = tempfile::tempdir().unwrap();
        let q = QuestionQueue::open(dir.path()).unwrap();
        q.pin("plan the launch schedule").unwrap();
        let cid = Uuid::new_v4();
        let first = q.scan(cid, "launch schedule dates", 1).unwrap();
        assert_eq!(first.len(), 1);
        let second = q.scan(cid, "launch schedule dates", 1).unwrap();
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn state_persists_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let p = {
            let q = QuestionQueue::open(dir.path()).unwrap();
            q.pin("what did I decide about caching").unwrap()
        };
        let q = QuestionQueue::open(dir.path()).unwrap();
        let list = q.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, p.id);
    }

    #[test]
    fn keywordise_drops_stopwords_and_short_tokens() {
        let kws = keywordise("what is our Q4 pricing strategy for enterprise");
        assert!(kws.contains(&"pricing".to_string()));
        assert!(kws.contains(&"strategy".to_string()));
        assert!(kws.contains(&"enterprise".to_string()));
        assert!(!kws.contains(&"what".to_string()));
        assert!(!kws.contains(&"our".to_string()));
    }
}
