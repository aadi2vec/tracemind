//! Multimodal payload + preprocessor router (Product Plan §3, X1).
//!
//! `MultimodalPayload` is the single input type the router accepts. Each
//! variant carries the modality-native bytes plus an optional pre-extracted
//! text. Preprocessors are pure Rust and never depend on OS frameworks —
//! OCR/ASR/VLM outputs are supplied by the caller (menu-bar app, capture
//! daemon, MCP client) as `extracted_text` so this crate stays portable and
//! testable offline.
//!
//! Storage model:
//! - Blob bytes → `~/.tracemind/blobs/<sha256[:2]>/<sha256>` (content addressed)
//! - Row → `attachments` SQLite table (see [`AttachmentStore`])
//! - Canonical text → `IngestPipeline::ingest_fast` (existing text path)
//!
//! Extension points (deliberately empty stubs until wired):
//! - macOS Vision framework OCR for `Image`
//! - Whisper.cpp ASR for `Audio`
//! - `lopdf` / `pdfium` for `Pdf`
//! - EventKit for `Calendar`
//! - IMAP client for `Email` (headers/body already parsed by caller)

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use tm_types::{Result, TraceMindError};

// ---------------------------------------------------------------------------
// Modality
// ---------------------------------------------------------------------------

/// The modalities `tm-ingest` recognises. Kept small and closed — new
/// modalities require a preprocessor + a `ComposedIndex` space, so adding one
/// is a deliberate design act, not an enum extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Pdf,
    Web,
    Email,
    Photo,
    Calendar,
}

impl Modality {
    pub fn as_str(self) -> &'static str {
        match self {
            Modality::Text => "text",
            Modality::Image => "image",
            Modality::Audio => "audio",
            Modality::Pdf => "pdf",
            Modality::Web => "web",
            Modality::Email => "email",
            Modality::Photo => "photo",
            Modality::Calendar => "calendar",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "text" => Modality::Text,
            "image" => Modality::Image,
            "audio" => Modality::Audio,
            "pdf" => Modality::Pdf,
            "web" => Modality::Web,
            "email" => Modality::Email,
            "photo" => Modality::Photo,
            "calendar" => Modality::Calendar,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// A single multimodal ingest event. `source` is a free-form label
/// ("clipboard", "screenshot", "safari-tab", "downloads-watcher") so
/// consumers of `feedback_signals.host_id` can attribute retention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalPayload {
    pub kind: Modality,
    pub source: String,
    #[serde(default = "Utc::now")]
    pub ts: DateTime<Utc>,
    /// Native bytes for non-text modalities. `None` for `Modality::Text`
    /// (which stores its content in `text`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    /// MIME type ("image/png", "application/pdf", …) — used to pick the
    /// preprocessor when the modality is ambiguous. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// Pre-extracted canonical text. When present, preprocessors skip
    /// their own extraction and use this. This is how OS-specific paths
    /// (macOS Vision OCR, Whisper.cpp) feed results in without dragging
    /// their dependencies into this crate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// URL for `Modality::Web`, `Modality::Email` (message-id), or free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Optional caller hint (e.g. voice-note title, screenshot app name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl MultimodalPayload {
    pub fn text(body: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            kind: Modality::Text,
            source: source.into(),
            ts: Utc::now(),
            bytes: None,
            mime: Some("text/plain".into()),
            text: Some(body.into()),
            uri: None,
            hint: None,
        }
    }

    pub fn image(bytes: Vec<u8>, mime: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            kind: Modality::Image,
            source: source.into(),
            ts: Utc::now(),
            bytes: Some(bytes),
            mime: Some(mime.into()),
            text: None,
            uri: None,
            hint: None,
        }
    }

    pub fn pdf(bytes: Vec<u8>, source: impl Into<String>) -> Self {
        Self {
            kind: Modality::Pdf,
            source: source.into(),
            ts: Utc::now(),
            bytes: Some(bytes),
            mime: Some("application/pdf".into()),
            text: None,
            uri: None,
            hint: None,
        }
    }

    pub fn web(url: impl Into<String>, html: impl Into<String>, source: impl Into<String>) -> Self {
        let html_string: String = html.into();
        Self {
            kind: Modality::Web,
            source: source.into(),
            ts: Utc::now(),
            bytes: Some(html_string.clone().into_bytes()),
            mime: Some("text/html".into()),
            text: None,
            uri: Some(url.into()),
            hint: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Preprocessing result
// ---------------------------------------------------------------------------

/// Output of running a payload through its preprocessor. Feeds the text into
/// `IngestPipeline::ingest_fast` and writes an `attachments` row keyed by the
/// blob's content hash.
#[derive(Debug, Clone)]
pub struct Preprocessed {
    /// Canonical text for the text pipeline. Empty string is legal (an image
    /// with no OCR yet) — the caller may choose not to run the text pipeline
    /// in that case.
    pub canonical_text: String,
    /// SHA-256 of the source bytes, hex-encoded. `None` for text-only
    /// payloads with no bytes.
    pub blob_sha256: Option<String>,
    /// MIME type as recorded (for the attachments row).
    pub mime: Option<String>,
    /// Length of the source bytes in bytes (0 for text-only).
    pub bytes_len: usize,
    /// Whether the text field was supplied by the caller (skip re-processing).
    pub used_caller_text: bool,
}

// ---------------------------------------------------------------------------
// Blob storage
// ---------------------------------------------------------------------------

/// Content-addressed blob store rooted at `<data_dir>/blobs/`.
///
/// Layout: `<root>/blobs/<sha256[:2]>/<sha256>` — the two-char prefix keeps
/// any single directory from exceeding a few thousand entries even at
/// millions of blobs.
///
/// The store is idempotent: writing the same bytes twice is a no-op. Dedup
/// happens on write, so callers don't need to check `exists()` first.
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Open (or create) a blob store rooted at `<data_dir>/blobs/`.
    pub fn open<P: AsRef<Path>>(data_dir: P) -> Result<Self> {
        let root = data_dir.as_ref().join("blobs");
        fs::create_dir_all(&root)
            .map_err(|e| TraceMindError::Storage(format!("blob root: {e}")))?;
        Ok(Self { root })
    }

    /// Compute the sha256 of `bytes` as a lowercase hex string.
    pub fn sha256(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        format!("{:x}", h.finalize())
    }

    /// Reject any string that isn't a canonical sha256 hex digest —
    /// exactly 64 lowercase-hex characters. Everything downstream
    /// derives the on-disk path from this value; without validation a
    /// caller can walk out of the blob root by passing `"../../…"`.
    fn validate_sha(sha: &str) -> Result<()> {
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(TraceMindError::Storage(format!(
                "invalid sha256 (must be 64 lowercase hex chars): {sha:?}"
            )));
        }
        Ok(())
    }

    /// Absolute path a blob with `sha` would live at. Returns `None`
    /// when `sha` is not a canonical 64-char lowercase-hex string —
    /// path traversal guard.
    pub fn path_for(&self, sha: &str) -> Option<PathBuf> {
        Self::validate_sha(sha).ok()?;
        // `sha` is validated 64 hex chars, so [..2] is always safe.
        let prefix = &sha[..2];
        Some(self.root.join(prefix).join(sha))
    }

    /// Maximum blob size accepted by [`Self::put`]. 512 MB. Rationale:
    /// a Retina screenshot is ~10 MB, a large PDF ~200 MB, an .eml
    /// with attachments up to ~50 MB — 512 MB is a comfortable
    /// ceiling that still rejects a runaway ingest before it fills
    /// the user's disk. Callers that need larger payloads should
    /// chunk before calling.
    pub const MAX_BLOB_BYTES: usize = 512 * 1024 * 1024;

    /// Write `bytes` if they don't already exist. Returns the sha256.
    /// Rejects payloads larger than [`Self::MAX_BLOB_BYTES`].
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        if bytes.len() > Self::MAX_BLOB_BYTES {
            return Err(TraceMindError::Storage(format!(
                "blob rejected: {} bytes exceeds cap ({})",
                bytes.len(),
                Self::MAX_BLOB_BYTES
            )));
        }
        let sha = Self::sha256(bytes);
        let path = self
            .path_for(&sha)
            .expect("sha256() output is always 64 lowercase hex chars");
        if path.exists() {
            return Ok(sha);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| TraceMindError::Storage(format!("blob dir: {e}")))?;
        }
        // Write to a per-process temp file next to the target then
        // rename — avoids half-written files if the process is killed
        // mid-write, and the unique suffix prevents cross-process
        // races on the same tmp path.
        let tmp = path.with_extension(format!(
            "part.{}.{}",
            std::process::id(),
            Uuid::new_v4().simple(),
        ));
        {
            let mut f = fs::File::create(&tmp)
                .map_err(|e| TraceMindError::Storage(format!("blob create: {e}")))?;
            f.write_all(bytes)
                .map_err(|e| TraceMindError::Storage(format!("blob write: {e}")))?;
            f.sync_all().ok();
        }
        // On POSIX rename is atomic and overwriting a rare identical-
        // content race is safe (same sha). On error, clean up tmp so
        // we don't leak `.part.*` files.
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(TraceMindError::Storage(format!("blob rename: {e}")));
        }
        Ok(sha)
    }

    pub fn exists(&self, sha: &str) -> bool {
        self.path_for(sha).map(|p| p.exists()).unwrap_or(false)
    }

    pub fn read(&self, sha: &str) -> Result<Vec<u8>> {
        let path = self
            .path_for(sha)
            .ok_or_else(|| TraceMindError::Storage(format!("blob read: invalid sha {sha:?}")))?;
        fs::read(&path)
            .map_err(|e| TraceMindError::Storage(format!("blob read: {e}")))
    }

    /// Root directory (for tests / inspection).
    pub fn root(&self) -> &Path {
        &self.root
    }
}

// ---------------------------------------------------------------------------
// Attachments table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub memory_content_hash: String,
    pub modality: Modality,
    pub sha256: String,
    pub mime: Option<String>,
    pub bytes_len: i64,
    pub extracted_text_ref: Option<String>,
    pub source: String,
    pub uri: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct AttachmentStore;

impl AttachmentStore {
    /// Create the `attachments` table. Idempotent.
    pub fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS attachments (
                id TEXT NOT NULL PRIMARY KEY,
                memory_content_hash TEXT NOT NULL,
                modality TEXT NOT NULL,
                sha256 TEXT NOT NULL,
                mime TEXT,
                bytes_len INTEGER NOT NULL,
                extracted_text_ref TEXT,
                source TEXT NOT NULL,
                uri TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_attach_hash
                ON attachments(memory_content_hash);
            CREATE INDEX IF NOT EXISTS idx_attach_sha
                ON attachments(sha256);
            CREATE INDEX IF NOT EXISTS idx_attach_modality
                ON attachments(modality, created_at);",
        )
        .map_err(|e| TraceMindError::Storage(format!("attachments schema: {e}")))?;
        Ok(())
    }

    pub fn insert(conn: &Connection, row: &AttachmentRow) -> Result<()> {
        conn.execute(
            "INSERT INTO attachments
                (id, memory_content_hash, modality, sha256, mime, bytes_len,
                 extracted_text_ref, source, uri, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                row.id.to_string(),
                row.memory_content_hash,
                row.modality.as_str(),
                row.sha256,
                row.mime,
                row.bytes_len,
                row.extracted_text_ref,
                row.source,
                row.uri,
                row.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| TraceMindError::Storage(format!("attachment insert: {e}")))?;
        Ok(())
    }

    pub fn for_memory(conn: &Connection, memory_content_hash: &str) -> Result<Vec<AttachmentRow>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, memory_content_hash, modality, sha256, mime,
                        bytes_len, extracted_text_ref, source, uri, created_at
                 FROM attachments WHERE memory_content_hash = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![memory_content_hash], row_from)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(rows)
    }

    pub fn recent(conn: &Connection, limit: usize) -> Result<Vec<AttachmentRow>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, memory_content_hash, modality, sha256, mime,
                        bytes_len, extracted_text_ref, source, uri, created_at
                 FROM attachments ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit as i64], row_from)
            .map_err(|e| TraceMindError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| TraceMindError::Storage(e.to_string()))?;
        Ok(rows)
    }
}

fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRow> {
    let id_s: String = row.get(0)?;
    let memory_content_hash: String = row.get(1)?;
    let modality_s: String = row.get(2)?;
    let sha256: String = row.get(3)?;
    let mime: Option<String> = row.get(4)?;
    let bytes_len: i64 = row.get(5)?;
    let extracted_text_ref: Option<String> = row.get(6)?;
    let source: String = row.get(7)?;
    let uri: Option<String> = row.get(8)?;
    let created_at_s: String = row.get(9)?;
    Ok(AttachmentRow {
        id: Uuid::parse_str(&id_s).unwrap_or_else(|_| Uuid::nil()),
        memory_content_hash,
        modality: Modality::from_str(&modality_s).unwrap_or(Modality::Text),
        sha256,
        mime,
        bytes_len,
        extracted_text_ref,
        source,
        uri,
        created_at: created_at_s
            .parse::<DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now()),
    })
}

// ---------------------------------------------------------------------------
// Preprocessors (pure Rust)
// ---------------------------------------------------------------------------

/// Trait implemented by each modality's preprocessor. Given a payload
/// (and the blob store to persist bytes to) return a `Preprocessed` result.
pub trait Preprocessor {
    fn kind(&self) -> Modality;
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed>;
}

// ---- text ----

pub struct TextPreprocessor;
impl Preprocessor for TextPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Text
    }
    fn preprocess(&self, payload: &MultimodalPayload, _blobs: &BlobStore) -> Result<Preprocessed> {
        let text = payload.text.clone().unwrap_or_default();
        Ok(Preprocessed {
            canonical_text: text,
            blob_sha256: None,
            mime: payload.mime.clone(),
            bytes_len: 0,
            used_caller_text: true,
        })
    }
}

// ---- web ----

pub struct WebPreprocessor;
impl Preprocessor for WebPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Web
    }
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed> {
        // Persist original HTML/bytes for provenance.
        let (sha, bytes_len) = if let Some(b) = &payload.bytes {
            (Some(blobs.put(b)?), b.len())
        } else {
            (None, 0)
        };
        let canonical = if let Some(text) = &payload.text {
            text.clone()
        } else if let Some(b) = &payload.bytes {
            let html = String::from_utf8_lossy(b);
            strip_html(&html)
        } else {
            String::new()
        };
        // Only prepend the URI when it's an actual web address — a real
        // web pin's URL is useful retrieval context. A local file path
        // (e.g. a notes-vault `.md` reused through this preprocessor) is
        // noise: it dilutes the embedding and leaks into extractive
        // answers as a spurious first line. Provenance for local files is
        // already carried by the stored blob.
        let canonical = match &payload.uri {
            Some(uri) if uri.starts_with("http://") || uri.starts_with("https://") => {
                format!("{}\n\n{}", uri, canonical)
            }
            _ => canonical,
        };
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: sha,
            mime: payload.mime.clone().or_else(|| Some("text/html".into())),
            bytes_len,
            used_caller_text: payload.text.is_some(),
        })
    }
}

/// A conservative HTML → text stripper. Removes `<script>` and `<style>`
/// bodies, then drops every tag and collapses whitespace. Not a full
/// readability implementation — the plan (X6) upgrades this to
/// `readability-rs` in a later sprint. Good enough to make retrieval work
/// against ambient web captures today.
///
/// **Byte-safety note:** HTML tag names are ASCII by spec (RFC HTML5
/// §8.2.4). We compare tags using `to_ascii_lowercase()` on the *bytes*
/// so offsets in the case-folded view align 1:1 with the original —
/// crucial when the surrounding page body contains Unicode text where
/// `char::to_lowercase` can change byte length (e.g. Turkish `İ` → `i̇`).
pub fn strip_html(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript", "svg"] {
        loop {
            let open = format!("<{tag}");
            // Build an ASCII-lowered projection with identical byte
            // length so search offsets are valid in the original.
            let lower: Vec<u8> = s.bytes().map(|b| b.to_ascii_lowercase()).collect();
            let Some(start) = window_find(&lower, open.as_bytes()) else {
                break;
            };
            let close = format!("</{tag}>");
            let Some(end_rel) = window_find(&lower[start..], close.as_bytes()) else {
                // Unclosed tag — drop from `start` to end (still a
                // char boundary in `s` because we search ASCII bytes).
                s.replace_range(start.., " ");
                break;
            };
            let end = start + end_rel + close.len();
            s.replace_range(start..end, " ");
        }
    }
    // Drop remaining tags. Walk the original char-by-char so we never
    // slice mid-codepoint.
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // Collapse whitespace.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Byte-level substring search. Small, obvious, no dependency.
fn window_find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

// ---- pdf ----

/// Minimal PDF preprocessor. Callers who want real text extraction should
/// supply `payload.text` (via `pdf-extract`, `pdfium`, or Preview on macOS).
/// When bytes-only PDF arrives, we store the blob and emit a canonical
/// stub `[[PDF <name>: <n> bytes]]` so retrieval still surfaces the
/// attachment. Real extraction lives behind the `pdf-extract` feature (X11).
pub struct PdfPreprocessor;
impl Preprocessor for PdfPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Pdf
    }
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed> {
        let (sha, bytes_len) = if let Some(b) = &payload.bytes {
            (Some(blobs.put(b)?), b.len())
        } else {
            (None, 0)
        };
        let canonical = if let Some(t) = &payload.text {
            t.clone()
        } else {
            let name = payload.hint.as_deref().unwrap_or("document.pdf");
            format!("[[PDF {name}: {bytes_len} bytes]]")
        };
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: sha,
            mime: Some("application/pdf".into()),
            bytes_len,
            used_caller_text: payload.text.is_some(),
        })
    }
}

// ---- image / photo ----

/// Image preprocessor. OCR/VLM description is supplied by the caller (macOS
/// Vision framework on the menu-bar app side). If neither is present we
/// still store the blob and emit a stub — the image is recoverable by
/// content hash later.
pub struct ImagePreprocessor {
    pub kind: Modality,
}
impl Preprocessor for ImagePreprocessor {
    fn kind(&self) -> Modality {
        self.kind
    }
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed> {
        let (sha, bytes_len) = if let Some(b) = &payload.bytes {
            (Some(blobs.put(b)?), b.len())
        } else {
            (None, 0)
        };
        let canonical = if let Some(t) = &payload.text {
            t.clone()
        } else {
            let hint = payload.hint.as_deref().unwrap_or("image");
            format!("[[IMAGE {hint}: {bytes_len} bytes]]")
        };
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: sha,
            mime: payload.mime.clone().or_else(|| Some("image/*".into())),
            bytes_len,
            used_caller_text: payload.text.is_some(),
        })
    }
}

// ---- audio ----

/// Audio preprocessor. Whisper transcript is supplied by the caller.
pub struct AudioPreprocessor;
impl Preprocessor for AudioPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Audio
    }
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed> {
        let (sha, bytes_len) = if let Some(b) = &payload.bytes {
            (Some(blobs.put(b)?), b.len())
        } else {
            (None, 0)
        };
        let canonical = if let Some(t) = &payload.text {
            t.clone()
        } else {
            format!("[[AUDIO: {bytes_len} bytes — transcript pending]]")
        };
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: sha,
            mime: payload.mime.clone().or_else(|| Some("audio/*".into())),
            bytes_len,
            used_caller_text: payload.text.is_some(),
        })
    }
}

// ---- email ----

/// Email preprocessor. Parsed headers/body come from the caller (IMAP
/// client). Canonical text is `Subject: …\n\nBody`; the raw MIME message
/// is persisted as the blob for exact recall.
pub struct EmailPreprocessor;
impl Preprocessor for EmailPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Email
    }
    fn preprocess(&self, payload: &MultimodalPayload, blobs: &BlobStore) -> Result<Preprocessed> {
        let (sha, bytes_len) = if let Some(b) = &payload.bytes {
            (Some(blobs.put(b)?), b.len())
        } else {
            (None, 0)
        };
        let canonical = payload.text.clone().unwrap_or_default();
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: sha,
            mime: payload.mime.clone().or_else(|| Some("message/rfc822".into())),
            bytes_len,
            used_caller_text: payload.text.is_some(),
        })
    }
}

// ---- calendar ----

/// Calendar-event preprocessor. Called with `text` = "Title @ start — end
/// with A, B; location L; notes …" pre-composed by the EventKit adapter.
pub struct CalendarPreprocessor;
impl Preprocessor for CalendarPreprocessor {
    fn kind(&self) -> Modality {
        Modality::Calendar
    }
    fn preprocess(&self, payload: &MultimodalPayload, _blobs: &BlobStore) -> Result<Preprocessed> {
        let canonical = payload.text.clone().unwrap_or_default();
        Ok(Preprocessed {
            canonical_text: canonical,
            blob_sha256: None,
            mime: Some("application/x-calendar-event".into()),
            bytes_len: 0,
            used_caller_text: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Single decision point (Product Plan §3.3): given a `MultimodalPayload`,
/// return the `Preprocessed` output.
pub struct ModalityRouter {
    blobs: BlobStore,
}

impl ModalityRouter {
    pub fn open<P: AsRef<Path>>(data_dir: P) -> Result<Self> {
        Ok(Self {
            blobs: BlobStore::open(data_dir)?,
        })
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn route(&self, payload: &MultimodalPayload) -> Result<Preprocessed> {
        match payload.kind {
            Modality::Text => TextPreprocessor.preprocess(payload, &self.blobs),
            Modality::Image => ImagePreprocessor {
                kind: Modality::Image,
            }
            .preprocess(payload, &self.blobs),
            Modality::Photo => ImagePreprocessor {
                kind: Modality::Photo,
            }
            .preprocess(payload, &self.blobs),
            Modality::Audio => AudioPreprocessor.preprocess(payload, &self.blobs),
            Modality::Pdf => PdfPreprocessor.preprocess(payload, &self.blobs),
            Modality::Web => WebPreprocessor.preprocess(payload, &self.blobs),
            Modality::Email => EmailPreprocessor.preprocess(payload, &self.blobs),
            Modality::Calendar => CalendarPreprocessor.preprocess(payload, &self.blobs),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn tmp_router() -> (TempDir, ModalityRouter) {
        let td = TempDir::new().unwrap();
        let r = ModalityRouter::open(td.path()).unwrap();
        (td, r)
    }

    #[test]
    fn modality_roundtrip() {
        for m in [
            Modality::Text,
            Modality::Image,
            Modality::Audio,
            Modality::Pdf,
            Modality::Web,
            Modality::Email,
            Modality::Photo,
            Modality::Calendar,
        ] {
            assert_eq!(Modality::from_str(m.as_str()), Some(m));
        }
    }

    #[test]
    fn blob_store_dedups_on_write() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::open(td.path()).unwrap();
        let sha1 = bs.put(b"hello world").unwrap();
        let sha2 = bs.put(b"hello world").unwrap();
        assert_eq!(sha1, sha2);
        assert!(bs.exists(&sha1));
        assert_eq!(bs.read(&sha1).unwrap(), b"hello world");
    }

    #[test]
    fn text_preprocessor_returns_input() {
        let (_td, r) = tmp_router();
        let p = MultimodalPayload::text("hello there", "clipboard");
        let out = r.route(&p).unwrap();
        assert_eq!(out.canonical_text, "hello there");
        assert!(out.blob_sha256.is_none());
        assert_eq!(out.bytes_len, 0);
        assert!(out.used_caller_text);
    }

    #[test]
    fn web_preprocessor_strips_html() {
        let (_td, r) = tmp_router();
        let html = "<html><head><title>t</title><script>alert(1)</script></head>\
                    <body><h1>Hello</h1><p>world <b>bold</b></p></body></html>";
        let p = MultimodalPayload::web("https://ex.com/a", html, "safari");
        let out = r.route(&p).unwrap();
        assert!(out.canonical_text.contains("Hello"));
        assert!(out.canonical_text.contains("world"));
        assert!(out.canonical_text.contains("bold"));
        assert!(!out.canonical_text.contains("alert(1)"));
        assert!(out.canonical_text.contains("https://ex.com/a"));
        assert!(out.blob_sha256.is_some());
        assert_eq!(out.bytes_len, html.len());
    }

    #[test]
    fn pdf_preprocessor_stores_blob_and_stubs_text_when_no_caller_text() {
        let (_td, r) = tmp_router();
        let bytes = b"%PDF-1.4\n%\xC7\xEC\x8F\xA2\n1 0 obj\n<<>>\nendobj\n".to_vec();
        let p = MultimodalPayload::pdf(bytes.clone(), "downloads");
        let out = r.route(&p).unwrap();
        assert!(out.canonical_text.starts_with("[[PDF"));
        assert!(out.blob_sha256.is_some());
        assert_eq!(out.bytes_len, bytes.len());
    }

    #[test]
    fn pdf_preprocessor_uses_caller_text_when_supplied() {
        let (_td, r) = tmp_router();
        let mut p = MultimodalPayload::pdf(b"pdf-bytes".to_vec(), "downloads");
        p.text = Some("Extracted body text".into());
        let out = r.route(&p).unwrap();
        assert_eq!(out.canonical_text, "Extracted body text");
        assert!(out.used_caller_text);
    }

    #[test]
    fn image_preprocessor_stubs_when_no_ocr() {
        let (_td, r) = tmp_router();
        let mut p = MultimodalPayload::image(b"PNG BYTES".to_vec(), "image/png", "screenshot");
        p.hint = Some("whiteboard".into());
        let out = r.route(&p).unwrap();
        assert!(out.canonical_text.contains("[[IMAGE whiteboard"));
        assert!(out.blob_sha256.is_some());
    }

    #[test]
    fn image_preprocessor_uses_ocr_when_supplied() {
        let (_td, r) = tmp_router();
        let mut p = MultimodalPayload::image(b"PNG".to_vec(), "image/png", "screenshot");
        p.text = Some("meet at 4pm bring the sketch".into());
        let out = r.route(&p).unwrap();
        assert_eq!(out.canonical_text, "meet at 4pm bring the sketch");
    }

    #[test]
    fn attachment_store_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        AttachmentStore::init_schema(&conn).unwrap();
        let row = AttachmentRow {
            id: Uuid::new_v4(),
            memory_content_hash: "abc123".into(),
            modality: Modality::Pdf,
            sha256: "sha".into(),
            mime: Some("application/pdf".into()),
            bytes_len: 4096,
            extracted_text_ref: Some("hello".into()),
            source: "downloads".into(),
            uri: None,
            created_at: Utc::now(),
        };
        AttachmentStore::insert(&conn, &row).unwrap();
        let rows = AttachmentStore::for_memory(&conn, "abc123").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].modality, Modality::Pdf);
        assert_eq!(rows[0].bytes_len, 4096);
    }

    #[test]
    fn attachment_store_recent_orders_desc() {
        let conn = Connection::open_in_memory().unwrap();
        AttachmentStore::init_schema(&conn).unwrap();
        for i in 0..3 {
            let row = AttachmentRow {
                id: Uuid::new_v4(),
                memory_content_hash: format!("h{i}"),
                modality: Modality::Text,
                sha256: format!("s{i}"),
                mime: None,
                bytes_len: i as i64,
                extracted_text_ref: None,
                source: "test".into(),
                uri: None,
                created_at: Utc::now() + chrono::Duration::seconds(i),
            };
            AttachmentStore::insert(&conn, &row).unwrap();
        }
        let rows = AttachmentStore::recent(&conn, 10).unwrap();
        assert_eq!(rows.len(), 3);
        // Most recent (i=2) first.
        assert_eq!(rows[0].bytes_len, 2);
        assert_eq!(rows[2].bytes_len, 0);
    }

    #[test]
    fn strip_html_handles_nested_script() {
        let html = "<div>keep<script>drop() && drop()</script>keep2</div>";
        let text = strip_html(html);
        assert!(text.contains("keep"));
        assert!(text.contains("keep2"));
        assert!(!text.contains("drop"));
    }

    #[test]
    fn strip_html_does_not_panic_on_unicode_case_shift() {
        // Regression: pre-fix, `to_lowercase()` byte offsets were
        // applied to the original string. Characters where lower/upper
        // case have different UTF-8 lengths (Turkish `İ` → `i̇` is
        // 2 → 3 bytes) caused mid-codepoint slicing → panic.
        let html = "İSTANBUL <SCRIPT>alert(1)</SCRIPT> AFTER İSTANBUL";
        let out = strip_html(html);
        assert!(out.contains("İSTANBUL"));
        assert!(out.contains("AFTER"));
        assert!(!out.contains("alert(1)"));
    }

    #[test]
    fn strip_html_handles_unclosed_script_tag() {
        // Malformed HTML: script tag never closes. Must not panic and
        // must drop the script region so extractive answers don't
        // leak JS.
        let html = "<p>keep me</p><script>malicious()";
        let out = strip_html(html);
        assert!(out.contains("keep me"));
        assert!(!out.contains("malicious"));
    }

    #[test]
    fn blob_store_rejects_path_traversal_shas() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::open(td.path()).unwrap();
        // Path traversal attempts.
        assert!(bs.path_for("../../../etc/passwd").is_none());
        assert!(bs.path_for("../foo").is_none());
        // Missing chars, wrong length, uppercase, non-hex.
        assert!(bs.path_for("abc").is_none());
        assert!(bs.path_for(&"a".repeat(63)).is_none());
        assert!(bs.path_for(&"a".repeat(65)).is_none());
        assert!(bs.path_for(&"A".repeat(64)).is_none()); // uppercase
        assert!(bs.path_for(&"z".repeat(64)).is_none()); // z is non-hex
        // Canonical 64-char lowercase-hex passes.
        assert!(bs.path_for(&"0".repeat(64)).is_some());
        assert!(bs.path_for(&"deadbeef".repeat(8)).is_some());
    }

    #[test]
    fn blob_read_rejects_path_traversal() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::open(td.path()).unwrap();
        // Plant a real file OUTSIDE the blob root.
        let victim = td.path().parent().unwrap().join("victim.txt");
        std::fs::write(&victim, b"secret").ok();
        // A read with a traversal-shaped `sha` must fail — not return
        // the victim's contents.
        let err = bs.read("../../victim.txt");
        assert!(err.is_err(), "traversal must be refused");
        // exists() also refuses.
        assert!(!bs.exists("../../victim.txt"));
    }

    #[test]
    fn blob_store_enforces_size_cap() {
        let td = TempDir::new().unwrap();
        let bs = BlobStore::open(td.path()).unwrap();
        // A payload one byte over the cap is refused.
        let oversize = vec![0u8; BlobStore::MAX_BLOB_BYTES + 1];
        let err = bs.put(&oversize);
        assert!(err.is_err(), "oversized blob must be refused");
    }

    #[test]
    fn calendar_preprocessor_passes_text_through() {
        let (_td, r) = tmp_router();
        let mut p = MultimodalPayload {
            kind: Modality::Calendar,
            source: "eventkit".into(),
            ts: Utc::now(),
            bytes: None,
            mime: None,
            text: Some("Standup @ 09:00 with Alice, Bob".into()),
            uri: None,
            hint: None,
        };
        p.mime = None;
        let out = r.route(&p).unwrap();
        assert!(out.canonical_text.contains("Standup"));
        assert!(out.blob_sha256.is_none());
    }
}
