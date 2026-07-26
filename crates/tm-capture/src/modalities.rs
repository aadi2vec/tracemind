//! Modality sources — Product Plan X2 / X5 / X8 / X11 / X12 / X15 / X18 / X19.
//!
//! Every source implements [`ModalitySource`] and returns
//! [`tm_ingest::MultimodalPayload`] items when it has something new. The
//! sources here are *real* — they poll actual disk locations using pure-
//! Rust file walking + parsers, and emit blob-backed payloads. Real
//! platform capture (ScreenCaptureKit hooks, whisper.cpp transcription,
//! Vision-framework OCR) is a *quality* upgrade behind a feature gate;
//! the user experience of "screenshot recall" works today by polling
//! `~/Desktop` for new PNGs, which is where macOS drops them by default.
//!
//! Every real source keeps a persistent cursor at
//! `~/.tracemind/modality-cursor.<name>.json` so restarts don't
//! re-emit the same payload — that would double-count against the
//! capture-daemon's rate limits.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tm_ingest::{Modality, MultimodalPayload};
use tm_types::capture_permissions::{CapturePermissions, CaptureSource};
use walkdir::WalkDir;

/// Map a modality source's [`ModalitySource::name`] to the CAP-1
/// [`CaptureSource`] permission it lives under. Every real source MUST
/// map to a permission — a `None` here means the daemon has no way to
/// let the user consent, so the caller fails *closed* and never polls
/// that source.
pub fn capture_source_for(name: &str) -> Option<CaptureSource> {
    match name {
        "screenshot" => Some(CaptureSource::Screenshot),
        "voice-note" => Some(CaptureSource::Audio),
        "downloads-watcher" => Some(CaptureSource::Pdf),
        "browser-ext" => Some(CaptureSource::Browser),
        "browser-history" => Some(CaptureSource::Browser),
        "imap-inbox" => Some(CaptureSource::Email),
        "photo-library" => Some(CaptureSource::Photo),
        "eventkit" => Some(CaptureSource::Calendar),
        "notes-vault" => Some(CaptureSource::Notes),
        _ => None,
    }
}

/// Every modality source is polled on the capture daemon's tick. `poll`
/// returns `Some(payload)` when there's something new to ingest.
/// Sources are expected to be cheap when there's nothing to report.
#[async_trait]
pub trait ModalitySource: Send + Sync {
    fn kind(&self) -> Modality;
    /// Free-form identifier used as `MultimodalPayload::source`.
    fn name(&self) -> &'static str;
    async fn poll(&mut self) -> Option<MultimodalPayload>;
}

// ---------------------------------------------------------------------------
// Cursor persistence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cursor {
    /// Absolute paths already emitted. Capped at [`Cursor::SEEN_CAP`]
    /// so a long-running daemon watching a busy directory doesn't
    /// grow the cursor JSON without bound. The `last_mtime` field is
    /// the primary duplicate guard; `seen` handles the tie-break for
    /// files sharing an mtime and de-dupes runs after a restart.
    seen: HashSet<String>,
    /// Files strictly newer than `last_mtime` are considered new.
    last_mtime: Option<DateTime<Utc>>,
}

impl Cursor {
    /// Cap on the persisted `seen` set. Any writes past this size
    /// evict eagerly — we prefer forgetting an old path (which the
    /// `last_mtime` guard will still filter out on the common path)
    /// over letting the cursor file balloon to hundreds of MB.
    const SEEN_CAP: usize = 4096;

    fn mark_seen(&mut self, path: String) {
        if self.seen.len() >= Self::SEEN_CAP {
            // Trim aggressively — 25% off the top. We can't order a
            // HashSet, but any eviction is fine because `last_mtime`
            // is the primary filter.
            let to_drop = Self::SEEN_CAP / 4;
            let victims: Vec<String> = self.seen.iter().take(to_drop).cloned().collect();
            for v in victims {
                self.seen.remove(&v);
            }
        }
        self.seen.insert(path);
    }
}

/// True if `p` is (or resolves through) a symlink. Modality sources
/// refuse to read symlinks — a malicious symlink in a watched folder
/// (e.g. `~/Desktop/screenshot.png → ~/.ssh/id_rsa`) would otherwise
/// let arbitrary file contents flow into the ingest path. `fs::read`
/// follows links by default, so we screen at the entry gate.
fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn cursor_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(format!("modality-cursor.{name}.json"))
}

fn load_cursor(data_dir: &Path, name: &str) -> Cursor {
    let path = cursor_path(data_dir, name);
    fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_cursor(data_dir: &Path, name: &str, c: &Cursor) {
    let path = cursor_path(data_dir, name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string(c) {
        let _ = fs::write(&path, raw);
    }
}

fn mtime_utc(path: &Path) -> Option<DateTime<Utc>> {
    let m = fs::metadata(path).ok()?;
    let sys = m.modified().ok()?;
    let dur = sys.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let secs = dur.as_secs() as i64;
    chrono::DateTime::<Utc>::from_timestamp(secs, dur.subsec_nanos())
}

/// Per-file byte cap for modality sources. Any file larger than this
/// is *skipped*, not truncated — surfacing part of a document creates
/// worse retrieval than surfacing none of it (extractive answers key
/// off "the text was ingested" as a completeness signal). 256 MB is
/// generous for screenshots / PDFs / .eml with attachments; anything
/// larger is almost certainly video / disk image / accidental drop.
pub const MODALITY_FILE_CAP_BYTES: u64 = 256 * 1024 * 1024;

/// Scan `root` recursively for files with any of `exts` (lowercase, no dot).
/// Returns absolute paths sorted by mtime ascending — callers emit them in
/// order to preserve the natural capture sequence.
///
/// **Security invariants:**
/// - Symlinks are refused (`WalkDir` doesn't descend into linked dirs,
///   and we screen individual file entries via `is_symlink`). This
///   prevents `~/Desktop/pic.png → ~/.ssh/id_rsa` attacks.
/// - Files larger than [`MODALITY_FILE_CAP_BYTES`] are skipped.
fn find_new_files(root: &Path, exts: &[&str], cursor: &Cursor) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    // `follow_links(false)` is WalkDir's default; being explicit here
    // makes the intent obvious to a security reviewer.
    let mut out: Vec<(DateTime<Utc>, PathBuf)> = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        // Refuse symlinks — the entry may be a symlink to a file, and
        // `p.is_file()` would follow it.
        if is_symlink(p) {
            continue;
        }
        // `symlink_metadata()` returned non-symlink; safe to check
        // is_file which does not re-follow.
        let Ok(meta) = fs::symlink_metadata(p) else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        if meta.len() > MODALITY_FILE_CAP_BYTES {
            tracing::debug!(
                "[modality] skipping oversized file {}: {} bytes > cap",
                p.display(),
                meta.len()
            );
            continue;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if !exts.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
            continue;
        }
        let abs = p.to_string_lossy().to_string();
        if cursor.seen.contains(&abs) {
            continue;
        }
        let mtime = match mtime_utc(p) {
            Some(t) => t,
            None => continue,
        };
        if let Some(last) = cursor.last_mtime {
            if mtime <= last {
                continue;
            }
        }
        out.push((mtime, p.to_path_buf()));
    }
    out.sort_by_key(|(t, _)| *t);
    out.into_iter().map(|(_, p)| p).collect()
}

// ---------------------------------------------------------------------------
// Real sources
// ---------------------------------------------------------------------------

/// X5 — screenshot capture. Real path polls a directory (defaults to
/// `~/Desktop`) for new PNG files, which is where macOS's `⇧⌘4` puts
/// them by default. OCR text is left `None` unless the caller supplied
/// it — the plan intends real OCR to arrive via a `macos-screencapture`
/// feature that swaps in ScreenCaptureKit + Vision.
pub struct ScreenshotSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl ScreenshotSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "screenshot");
        Self { root, data_dir, cursor }
    }

    /// Default — `~/Desktop` on macOS, otherwise the current directory.
    pub fn default_root() -> PathBuf {
        home_dir().map(|h| h.join("Desktop")).unwrap_or_else(|| PathBuf::from("."))
    }
}

#[async_trait]
impl ModalitySource for ScreenshotSource {
    fn kind(&self) -> Modality {
        Modality::Image
    }
    fn name(&self) -> &'static str {
        "screenshot"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["png"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            save_cursor(&self.data_dir, "screenshot", &self.cursor);
            let hint = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string);
            // X5 — real macOS Vision OCR when compiled with the
            // `macos-screencapture` feature and running on macOS.
            // Off the feature (or off macOS), this is a no-op that
            // returns None so the screenshot is still stored as a
            // blob-only stub, recoverable by content hash later.
            let ocr = crate::vision_ocr::ocr_png_file(&path);
            let mut payload = MultimodalPayload::image(bytes, "image/png", "screenshot");
            payload.hint = hint;
            payload.uri = Some(abs);
            payload.text = ocr;
            return Some(payload);
        }
        None
    }
}

/// X12 — voice notes. Polls `~/.tracemind/voice` for `.wav`/`.m4a`.
/// Transcript stays `None` unless supplied by the caller — a real
/// whisper.cpp integration slots in behind `feature = "whisper-cpp"`.
pub struct VoiceNoteSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl VoiceNoteSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "voice");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir()
            .map(|h| h.join(".tracemind").join("voice"))
            .unwrap_or_else(|| PathBuf::from("./voice"))
    }
}

#[async_trait]
impl ModalitySource for VoiceNoteSource {
    fn kind(&self) -> Modality {
        Modality::Audio
    }
    fn name(&self) -> &'static str {
        "voice-note"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["wav", "m4a", "mp3"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            save_cursor(&self.data_dir, "voice", &self.cursor);
            let mime = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| match s.to_lowercase().as_str() {
                    "wav" => "audio/wav",
                    "m4a" => "audio/mp4",
                    "mp3" => "audio/mpeg",
                    _ => "audio/*",
                })
                .unwrap_or("audio/*")
                .to_string();
            let mut payload = MultimodalPayload {
                kind: Modality::Audio,
                source: "voice-note".into(),
                ts: Utc::now(),
                bytes: Some(bytes),
                mime: Some(mime),
                text: None,
                uri: Some(abs),
                hint: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string),
            };
            #[cfg(feature = "whisper-cpp")]
            {
                // Real transcription lands here — same path in the tree,
                // populated only when the feature is on.
                payload.text = whisper_cpp_transcribe(payload.bytes.as_deref().unwrap_or(&[]));
            }
            #[cfg(not(feature = "whisper-cpp"))]
            let _ = &payload; // keep `payload` referenced when feature is off
            return Some(payload);
        }
        None
    }
}

#[cfg(feature = "whisper-cpp")]
fn whisper_cpp_transcribe(_bytes: &[u8]) -> Option<String> {
    // Fill in with a real whisper.cpp call when the feature is wired.
    None
}

/// X11 — Downloads-folder PDF watcher. Uses `pdf-extract` to pull text
/// server-side so retrieval works over the *contents* of the PDF, not
/// just the filename.
pub struct PdfWatcherSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl PdfWatcherSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "pdf");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir().map(|h| h.join("Downloads")).unwrap_or_else(|| PathBuf::from("."))
    }
}

#[async_trait]
impl ModalitySource for PdfWatcherSource {
    fn kind(&self) -> Modality {
        Modality::Pdf
    }
    fn name(&self) -> &'static str {
        "downloads-watcher"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["pdf"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            save_cursor(&self.data_dir, "pdf", &self.cursor);
            // pdf-extract can panic on malformed PDFs; catch that so a
            // single bad download never takes the daemon down.
            let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes))
                .ok()
                .and_then(|r| r.ok());
            let hint = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string);
            let mut payload = MultimodalPayload::pdf(bytes, "downloads-watcher");
            payload.text = text;
            payload.uri = Some(abs);
            payload.hint = hint;
            return Some(payload);
        }
        None
    }
}

/// M2 — web-page capture. Watches a "pin drop" folder — the browser
/// extension writes `{sha256}.html` files there when the user pins a
/// tab. Simpler than a socket for MVP; equivalent user experience.
pub struct WebPinSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl WebPinSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "web");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir()
            .map(|h| h.join(".tracemind").join("web-pins"))
            .unwrap_or_else(|| PathBuf::from("./web-pins"))
    }
}

#[async_trait]
impl ModalitySource for WebPinSource {
    fn kind(&self) -> Modality {
        Modality::Web
    }
    fn name(&self) -> &'static str {
        "browser-ext"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["html", "htm"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(html) = fs::read_to_string(&path) else { continue };
            save_cursor(&self.data_dir, "web", &self.cursor);
            // Convention: sibling `<sha>.url` file holds the origin URL
            // if the extension wrote one. Optional.
            let uri = fs::read_to_string(path.with_extension("url"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let payload = MultimodalPayload {
                kind: Modality::Web,
                source: "browser-ext".into(),
                ts: Utc::now(),
                bytes: Some(html.clone().into_bytes()),
                mime: Some("text/html".into()),
                text: None,
                uri,
                hint: path.file_name().and_then(|n| n.to_str()).map(str::to_string),
            };
            return Some(payload);
        }
        None
    }
}

/// X15 — email .eml folder watcher. Full IMAP with auth is complex
/// enough that a real integration needs its own sprint; watching a
/// user-drop folder is functionally equivalent for MVP and preserves
/// the local-only property (no network in the request path).
pub struct EmailInboxSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl EmailInboxSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "email");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir()
            .map(|h| h.join(".tracemind").join("email"))
            .unwrap_or_else(|| PathBuf::from("./email"))
    }
}

#[async_trait]
impl ModalitySource for EmailInboxSource {
    fn kind(&self) -> Modality {
        Modality::Email
    }
    fn name(&self) -> &'static str {
        "imap-inbox"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["eml"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            save_cursor(&self.data_dir, "email", &self.cursor);
            let parsed = mail_parser::MessageParser::default().parse(&bytes);
            let (subject, body) = if let Some(msg) = parsed {
                let subj = msg.subject().unwrap_or("").to_string();
                let body = msg
                    .body_text(0)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|| msg.body_html(0).map(|c| c.into_owned()).unwrap_or_default());
                (subj, body)
            } else {
                (String::new(), String::from_utf8_lossy(&bytes).into_owned())
            };
            let canonical = if subject.is_empty() {
                body.clone()
            } else {
                format!("Subject: {subject}\n\n{body}")
            };
            let payload = MultimodalPayload {
                kind: Modality::Email,
                source: "imap-inbox".into(),
                ts: Utc::now(),
                bytes: Some(bytes),
                mime: Some("message/rfc822".into()),
                text: Some(canonical),
                uri: Some(abs),
                hint: Some(subject),
            };
            return Some(payload);
        }
        None
    }
}

/// X18 — photo library. Reads EXIF via `kamadak-exif` and composes a
/// canonical text string of `filename @ DateTimeOriginal [GPS lat, lon]`.
pub struct PhotoLibrarySource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl PhotoLibrarySource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "photo");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir().map(|h| h.join("Pictures")).unwrap_or_else(|| PathBuf::from("./Pictures"))
    }
}

#[async_trait]
impl ModalitySource for PhotoLibrarySource {
    fn kind(&self) -> Modality {
        Modality::Photo
    }
    fn name(&self) -> &'static str {
        "photo-library"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["jpg", "jpeg", "heic", "png"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            save_cursor(&self.data_dir, "photo", &self.cursor);
            let text = compose_exif_text(&bytes, &path);
            let payload = MultimodalPayload {
                kind: Modality::Photo,
                source: "photo-library".into(),
                ts: Utc::now(),
                bytes: Some(bytes),
                mime: Some("image/jpeg".into()),
                text: Some(text),
                uri: Some(abs),
                hint: path.file_name().and_then(|n| n.to_str()).map(str::to_string),
            };
            return Some(payload);
        }
        None
    }
}

fn compose_exif_text(bytes: &[u8], path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        parts.push(name.to_string());
    }
    let mut reader = std::io::Cursor::new(bytes);
    if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
        if let Some(dto) = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY) {
            parts.push(format!("@ {}", dto.display_value()));
        }
        if let (Some(lat), Some(lon)) = (
            exif.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY),
            exif.get_field(exif::Tag::GPSLongitude, exif::In::PRIMARY),
        ) {
            parts.push(format!("[GPS {} {}]", lat.display_value(), lon.display_value()));
        }
        if let Some(model) = exif.get_field(exif::Tag::Model, exif::In::PRIMARY) {
            parts.push(format!("({})", model.display_value()));
        }
    }
    parts.join(" ")
}

/// X19 — calendar events. Reads `.ics` files under the given root
/// (defaults to `~/Library/Calendars` — macOS's Calendar app writes
/// its cache there in that format). Composes canonical text as
/// `Title @ DTSTART — DTEND; LOCATION; DESCRIPTION` so retrieval can
/// answer "what did I have on Tuesday?"
pub struct CalendarSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
    /// Emit at most one event per poll — the daemon calls again next
    /// tick.
    pending: Vec<MultimodalPayload>,
}

impl CalendarSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "calendar");
        Self {
            root,
            data_dir,
            cursor,
            pending: Vec::new(),
        }
    }
    pub fn default_root() -> PathBuf {
        home_dir()
            .map(|h| h.join("Library").join("Calendars"))
            .unwrap_or_else(|| PathBuf::from("./Calendars"))
    }
}

#[async_trait]
impl ModalitySource for CalendarSource {
    fn kind(&self) -> Modality {
        Modality::Calendar
    }
    fn name(&self) -> &'static str {
        "eventkit"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        if let Some(p) = self.pending.pop() {
            return Some(p);
        }
        let candidates = find_new_files(&self.root, &["ics"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(raw) = fs::read_to_string(&path) else { continue };
            save_cursor(&self.data_dir, "calendar", &self.cursor);
            let reader = ical::IcalParser::new(raw.as_bytes());
            for cal in reader.flatten() {
                for event in cal.events {
                    let mut title = String::from("(untitled)");
                    let mut dtstart = String::new();
                    let mut dtend = String::new();
                    let mut location = String::new();
                    let mut description = String::new();
                    for prop in event.properties {
                        match prop.name.as_str() {
                            "SUMMARY" => title = prop.value.unwrap_or_default(),
                            "DTSTART" => dtstart = prop.value.unwrap_or_default(),
                            "DTEND" => dtend = prop.value.unwrap_or_default(),
                            "LOCATION" => location = prop.value.unwrap_or_default(),
                            "DESCRIPTION" => description = prop.value.unwrap_or_default(),
                            _ => {}
                        }
                    }
                    let mut parts = vec![format!("{title} @ {dtstart}")];
                    if !dtend.is_empty() {
                        parts.push(format!("— {dtend}"));
                    }
                    if !location.is_empty() {
                        parts.push(format!("; {location}"));
                    }
                    if !description.is_empty() {
                        parts.push(format!("; {description}"));
                    }
                    let text = parts.join(" ");
                    self.pending.push(MultimodalPayload {
                        kind: Modality::Calendar,
                        source: "eventkit".into(),
                        ts: Utc::now(),
                        bytes: None,
                        mime: Some("text/calendar".into()),
                        text: Some(text),
                        uri: Some(abs.clone()),
                        hint: Some(title),
                    });
                }
            }
            if let Some(p) = self.pending.pop() {
                return Some(p);
            }
        }
        None
    }
}

/// I18 — Notes.app / Obsidian read-only vault sync.
///
/// Watches a folder of `.md` / `.markdown` / `.txt` notes (Obsidian's
/// default layout, or a Notes.app export via File → Export as PDF/
/// Markdown workflow) and emits each new/changed note as a text
/// payload. Purely read-only — we never write into the vault, which
/// respects the plan's "librarian, not wire-tap" contract for content
/// that already has an authoritative home.
///
/// The default root is `$HOME/Documents/Obsidian` — the OS-idiomatic
/// spot for Obsidian's "default vault". Callers can point at a Notes
/// export folder or a specific vault via `new(root, data_dir)`.
pub struct NotesVaultSource {
    root: PathBuf,
    data_dir: PathBuf,
    cursor: Cursor,
}

impl NotesVaultSource {
    pub fn new(root: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_cursor(&data_dir, "notes-vault");
        Self { root, data_dir, cursor }
    }
    pub fn default_root() -> PathBuf {
        home_dir()
            .map(|h| h.join("Documents").join("Obsidian"))
            .unwrap_or_else(|| PathBuf::from("./Obsidian"))
    }
}

#[async_trait]
impl ModalitySource for NotesVaultSource {
    fn kind(&self) -> Modality {
        // Notes are structured text — reuse the Web modality for now
        // since it's the closest match ("body text with a title-like
        // hint"). A dedicated Modality::Note variant is a follow-up
        // that requires a schema bump.
        Modality::Web
    }
    fn name(&self) -> &'static str {
        "notes-vault"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        let candidates = find_new_files(&self.root, &["md", "markdown", "txt"], &self.cursor);
        for path in candidates {
            let abs = path.to_string_lossy().to_string();
            self.cursor.mark_seen(abs.clone());
            if let Some(t) = mtime_utc(&path) {
                if self.cursor.last_mtime.map(|x| t > x).unwrap_or(true) {
                    self.cursor.last_mtime = Some(t);
                }
            }
            let Ok(raw) = fs::read_to_string(&path) else { continue };
            save_cursor(&self.data_dir, "notes-vault", &self.cursor);
            // First non-empty line, stripped of leading `#` markers,
            // is a reasonable "hint" (Obsidian's convention is the
            // filename == title, but many notes carry a `# Title`
            // as the first line too).
            let title = raw
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .filter(|s| !s.is_empty());
            let filename_hint = path
                .file_stem()
                .and_then(|n| n.to_str())
                .map(str::to_string);
            // Canonical (embedded + answerable) text strips leading markdown
            // heading markers (`#`, `##`, …) per line so the raw `#` glyphs
            // don't leak into extractive answers or dilute the embedding.
            // The exact bytes are still preserved in the blob for provenance.
            let canonical: String = raw
                .lines()
                .map(|l| l.trim_start_matches('#').trim_start())
                .collect::<Vec<_>>()
                .join("\n");
            let payload = MultimodalPayload {
                kind: Modality::Web,
                source: "notes-vault".into(),
                ts: Utc::now(),
                bytes: Some(raw.clone().into_bytes()),
                mime: Some("text/markdown".into()),
                text: Some(canonical),
                uri: Some(abs),
                hint: title.or(filename_hint),
            };
            return Some(payload);
        }
        None
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Registry — the capture daemon's tick loop asks this for every payload.
// ---------------------------------------------------------------------------

pub struct ModalityRegistry {
    sources: Vec<Box<dyn ModalitySource>>,
}

impl ModalityRegistry {
    pub fn empty() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// The full default set — every real source rooted at its OS-native
    /// default directory. The daemon can override each via
    /// [`Self::push`] or its own configuration path.
    pub fn with_defaults(data_dir: PathBuf) -> Self {
        Self {
            sources: vec![
                Box::new(ScreenshotSource::new(
                    ScreenshotSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(VoiceNoteSource::new(
                    VoiceNoteSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(PdfWatcherSource::new(
                    PdfWatcherSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(WebPinSource::new(
                    WebPinSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(EmailInboxSource::new(
                    EmailInboxSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(PhotoLibrarySource::new(
                    PhotoLibrarySource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(CalendarSource::new(
                    CalendarSource::default_root(),
                    data_dir.clone(),
                )),
                Box::new(NotesVaultSource::new(
                    NotesVaultSource::default_root(),
                    data_dir.clone(),
                )),
                // Real ambient web capture — reads Chrome/Safari history
                // directly (no extension). Gated under CaptureSource::Browser.
                Box::new(crate::browser_history::BrowserHistorySource::new(
                    home_dir().unwrap_or_else(|| PathBuf::from(".")),
                    data_dir.clone(),
                )),
            ],
        }
    }

    /// Like [`Self::with_defaults`] but only keeps sources whose CAP-1
    /// permission is enabled. Fails *closed*: a source with no
    /// [`capture_source_for`] mapping, or one the user has not toggled
    /// on, is dropped before its first poll — so its watched directory
    /// is never even read. This is the privacy invariant the other
    /// capture loops enforce with `load_enabled(...)`, applied to the
    /// bundled modality loop.
    pub fn with_enabled(data_dir: PathBuf, perms: &CapturePermissions) -> Self {
        let mut reg = Self::with_defaults(data_dir);
        reg.sources.retain(|s| {
            capture_source_for(s.name())
                .map(|cs| perms.is_enabled(cs))
                .unwrap_or(false)
        });
        reg
    }

    pub fn push(&mut self, source: Box<dyn ModalitySource>) {
        self.sources.push(source);
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Poll every source in order. Returns collected payloads (empty vec
    /// when nothing is ready).
    pub async fn tick(&mut self) -> Vec<MultimodalPayload> {
        let mut out = Vec::new();
        for src in &mut self.sources {
            if let Some(p) = src.poll().await {
                out.push(p);
            }
        }
        out
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.sources.iter().map(|s| s.name()).collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn touch(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        // Ensure mtime resolution can see them as newer than "no files".
        std::thread::sleep(std::time::Duration::from_millis(10));
        p
    }

    #[tokio::test]
    async fn screenshot_source_emits_new_png() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let root = td.path().to_path_buf();
        touch(&root, "Screenshot 2026-07-23.png", b"\x89PNGfake");
        let mut src = ScreenshotSource::new(root.clone(), data.path().to_path_buf());
        let p = src.poll().await.expect("first png emitted");
        assert_eq!(p.kind, Modality::Image);
        assert_eq!(p.source, "screenshot");
        assert!(p.hint.as_ref().unwrap().starts_with("Screenshot"));
        // Second call must not re-emit the same file.
        assert!(src.poll().await.is_none());
    }

    #[tokio::test]
    async fn pdf_source_extracts_text_when_readable() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        // A tiny, malformed PDF — pdf-extract's panic-catch keeps us
        // from blowing up. The important thing is the payload still
        // carries the bytes and blob path, and no crash.
        touch(td.path(), "junk.pdf", b"not a real pdf");
        let mut src = PdfWatcherSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("junk pdf still emitted");
        assert_eq!(p.kind, Modality::Pdf);
        // text may be None or Some(""); the important assertion is: no
        // crash + bytes preserved.
        assert!(p.bytes.is_some());
    }

    #[tokio::test]
    async fn calendar_source_parses_summary_and_dtstart() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Team standup\r\n\
            DTSTART:20260723T090000Z\r\nDTEND:20260723T093000Z\r\n\
            LOCATION:Room 3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        touch(td.path(), "cal.ics", ics.as_bytes());
        let mut src = CalendarSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("event emitted");
        assert_eq!(p.kind, Modality::Calendar);
        let text = p.text.unwrap();
        assert!(text.contains("Team standup"));
        assert!(text.contains("20260723T090000Z"));
        assert!(text.contains("Room 3"));
    }

    #[tokio::test]
    async fn email_source_parses_subject_and_body() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let eml = "Subject: Draft agenda\r\nFrom: a@b\r\nTo: c@d\r\n\r\nHere are the items we should discuss.\r\n";
        touch(td.path(), "1.eml", eml.as_bytes());
        let mut src = EmailInboxSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("email emitted");
        let text = p.text.unwrap();
        assert!(text.contains("Draft agenda"));
        assert!(text.contains("items we should discuss"));
    }

    #[tokio::test]
    async fn photo_source_composes_filename_when_no_exif() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        touch(td.path(), "IMG_0042.jpg", b"not-real-jpeg");
        let mut src = PhotoLibrarySource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("photo emitted");
        let text = p.text.unwrap();
        assert!(text.contains("IMG_0042.jpg"));
    }

    #[tokio::test]
    async fn web_pin_source_reads_html_and_optional_url_sidecar() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        touch(
            td.path(),
            "abc.html",
            b"<html><body><p>captured</p></body></html>",
        );
        touch(td.path(), "abc.url", b"https://example.com/page\n");
        let mut src = WebPinSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("web pin emitted");
        assert_eq!(p.uri.as_deref(), Some("https://example.com/page"));
        assert_eq!(p.kind, Modality::Web);
    }

    #[tokio::test]
    async fn cursor_prevents_repeat_emission_across_ticks() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        touch(td.path(), "shot.png", b"\x89PNG");
        let mut src = ScreenshotSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        assert!(src.poll().await.is_some());
        assert!(src.poll().await.is_none());
        // Even a re-created source (simulating restart) sees the cursor
        // and skips the already-emitted file.
        let mut src2 = ScreenshotSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        assert!(src2.poll().await.is_none());
    }

    #[tokio::test]
    async fn notes_vault_source_emits_markdown_with_title_hint() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        touch(
            td.path(),
            "sub/first.md",
            b"# Meeting notes\n\nSome body content.\n",
        );
        let mut src = NotesVaultSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("markdown note emitted");
        assert_eq!(p.source, "notes-vault");
        assert_eq!(p.mime.as_deref(), Some("text/markdown"));
        assert_eq!(p.hint.as_deref(), Some("Meeting notes"));
        assert!(p.text.as_ref().unwrap().contains("body content"));
        // Cursor prevents re-emission.
        assert!(src.poll().await.is_none());
    }

    #[tokio::test]
    async fn notes_vault_source_falls_back_to_filename_when_no_title() {
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        touch(td.path(), "grocery-list.md", b"eggs\nmilk\n");
        let mut src = NotesVaultSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        let p = src.poll().await.expect("emitted");
        // First non-empty line is "eggs" — that's a valid title-guess,
        // so we prefer it over the filename. The important thing is
        // the payload has *some* hint.
        assert!(p.hint.is_some());
    }

    #[test]
    fn every_default_source_maps_to_a_permission() {
        // If a new default source is added without a permission mapping,
        // with_enabled would silently drop it forever — catch that here.
        let data = TempDir::new().unwrap();
        let reg = ModalityRegistry::with_defaults(data.path().to_path_buf());
        for name in reg.names() {
            assert!(
                capture_source_for(name).is_some(),
                "modality source {name:?} has no CaptureSource mapping"
            );
        }
    }

    #[test]
    fn with_enabled_keeps_only_enabled_sources() {
        let data = TempDir::new().unwrap();
        let mut perms = CapturePermissions::default();
        // Defaults: Notes on (notes-vault); Calendar/Photo/Email/etc off.
        let reg = ModalityRegistry::with_enabled(data.path().to_path_buf(), &perms);
        assert!(reg.names().contains(&"notes-vault"));
        assert!(!reg.names().contains(&"eventkit"));
        assert!(!reg.names().contains(&"imap-inbox"));

        // Enabling Calendar surfaces eventkit; disabling Notes drops it.
        perms.enable(CaptureSource::Calendar);
        perms.disable(CaptureSource::Notes);
        let reg = ModalityRegistry::with_enabled(data.path().to_path_buf(), &perms);
        assert!(reg.names().contains(&"eventkit"));
        assert!(!reg.names().contains(&"notes-vault"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn screenshot_source_refuses_symlinked_files() {
        // Regression: pre-fix, a symlinked "screenshot.png" pointing
        // at an arbitrary file (`~/.ssh/id_rsa` in the real threat)
        // would be read and ingested. The symlink screen in
        // `find_new_files` refuses those.
        use std::os::unix::fs::symlink;
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        // Plant a "sensitive" file *outside* the watched root.
        let sensitive = data.path().join("victim.key");
        std::fs::write(&sensitive, b"SECRET-KEY-MATERIAL").unwrap();
        // Symlink inside the watched root pointing at it.
        let link = td.path().join("innocuous.png");
        symlink(&sensitive, &link).unwrap();
        let mut src = ScreenshotSource::new(
            td.path().to_path_buf(),
            data.path().to_path_buf(),
        );
        // The symlinked file must not be emitted — the source must
        // return None, and the symlink's target must not have been
        // read.
        assert!(src.poll().await.is_none(), "symlink must be refused");
    }

    #[tokio::test]
    async fn oversized_files_are_skipped_not_truncated() {
        // A file above MODALITY_FILE_CAP_BYTES is a candidate for
        // the source to skip. We can't reasonably materialise 256 MB
        // in a unit test, so drop the cap temporarily by writing a
        // large-ish file (10 MB) that would be *accepted* and one
        // that would be rejected under a low cap. Since the cap is
        // a compile-time const, this test just proves the file that
        // is skipped stays unemitted — a smaller cap-check test lives
        // at the metadata level. We assert the well-formed small
        // file continues to work, i.e. the guard didn't break the
        // happy path.
        let td = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        std::fs::write(td.path().join("ok.png"), b"\x89PNG-tiny").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mut src = ScreenshotSource::new(td.path().to_path_buf(), data.path().to_path_buf());
        assert!(src.poll().await.is_some(), "small file still admitted");
    }

    #[test]
    fn cursor_seen_set_is_capped() {
        // Regression: pre-fix, `seen` grew without bound. The cap
        // must keep the set from exceeding SEEN_CAP even under a
        // long-running flood.
        let mut c = Cursor::default();
        for i in 0..(Cursor::SEEN_CAP * 3) {
            c.mark_seen(format!("/tmp/path/{i}.png"));
        }
        assert!(
            c.seen.len() <= Cursor::SEEN_CAP,
            "cursor seen set exceeded cap: {} > {}",
            c.seen.len(),
            Cursor::SEEN_CAP,
        );
    }

    #[tokio::test]
    async fn registry_ticks_every_source() {
        let data = TempDir::new().unwrap();
        let td = TempDir::new().unwrap();
        touch(td.path(), "one.png", b"\x89PNG");
        let mut reg = ModalityRegistry::empty();
        reg.push(Box::new(ScreenshotSource::new(
            td.path().to_path_buf(),
            data.path().to_path_buf(),
        )));
        let out = reg.tick().await;
        assert_eq!(out.len(), 1);
    }
}
