//! Ambient browser-history capture (Safari + Chrome).
//!
//! This is the *real* ambient web-capture path — it reads the browser's
//! own history SQLite database directly, so it captures every page the
//! user actually visits with **no browser extension and no bookmarklet**.
//! (The bookmarklet endpoint in `browser_capture.rs` is a manual, opt-in
//! supplement; this source is the passive one people expect.)
//!
//! ## How it works
//! Browsers keep their history in a live SQLite DB that is usually locked
//! (WAL mode, held open by the running browser). We therefore **copy** the
//! DB (plus its `-wal`/`-shm` sidecars) to a private temp file and open the
//! copy read-only. We never touch the original — capture must never risk a
//! user's browser state.
//!
//! ## First run
//! On the very first poll (empty cursor) we only look back a bounded window
//! (24h) so we don't dump thousands of historical rows into memory. After
//! that we track a per-browser high-water mark on `visit_time` and only emit
//! genuinely new visits.
//!
//! ## Privacy / permissions
//! - Chrome's DB lives under `~/Library/Application Support` and is readable
//!   without special entitlement.
//! - Safari's `~/Library/Safari/History.db` is TCC-protected: reading it
//!   returns `Operation not permitted` until the user grants the binary
//!   **Full Disk Access**. We degrade gracefully — a failed copy just means
//!   that browser contributes nothing this tick (logged once at debug).
//! - Everything stays on-device; this only *reads* local files.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use tm_ingest::{Modality, MultimodalPayload};

use crate::modalities::ModalitySource;

/// Max visits pulled from a single browser in one poll. Keeps the daemon's
/// per-tick work bounded even if the user went on a browsing spree.
const MAX_VISITS_PER_POLL: usize = 50;
/// First-run look-back window, in seconds. We only backfill visits newer
/// than `now - this` so a fresh install doesn't ingest all of history, but
/// still seeds the graph with a useful slice of recent browsing (7 days).
/// Later polls paginate forward from the stored high-water mark.
const FIRST_RUN_LOOKBACK_SECS: i64 = 7 * 24 * 3600;

/// How long to wait before re-probing a browser after a TCC / permission
/// denial. Without a backoff, every daemon tick hammers the protected DB
/// and the user's Console fills with `Operation not permitted` noise —
/// AND, more importantly, if the user grants Full Disk Access we still
/// want to notice the flip within a few minutes. 5 min is the balance:
/// short enough that a granted permission takes effect on the next
/// natural tick, long enough that the retry cost stays negligible.
const PERMISSION_RETRY_COOLDOWN_SECS: u64 = 300;

/// Whether a browser's history DB is currently readable — used by the
/// `tracemind capture doctor` probe and by the daemon's per-browser
/// backoff. Ordering matters: `Reachable` is the happy path,
/// `PermissionDenied` is the actionable one (needs Full Disk Access),
/// `Missing` means the browser isn't installed / hasn't been used,
/// and `Unavailable` covers every other read error (locked, corrupt,
/// unknown schema).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryReachability {
    Reachable,
    PermissionDenied,
    Missing,
    Unavailable(String),
}

impl HistoryReachability {
    pub fn is_reachable(&self) -> bool {
        matches!(self, HistoryReachability::Reachable)
    }
    pub fn is_permission_denied(&self) -> bool {
        matches!(self, HistoryReachability::PermissionDenied)
    }
    /// One-line human message with the action to take (if any).
    pub fn describe(&self, b: Browser) -> String {
        match self {
            HistoryReachability::Reachable => format!("{}: history readable", b.key()),
            HistoryReachability::PermissionDenied => format!(
                "{}: needs Full Disk Access — grant to the daemon binary in \
                 System Settings → Privacy & Security → Full Disk Access, then \
                 restart `tracemind-capture`",
                b.key()
            ),
            HistoryReachability::Missing => {
                format!("{}: no history DB found (browser not installed or never used)", b.key())
            }
            HistoryReachability::Unavailable(why) => {
                format!("{}: unreadable ({})", b.key(), why)
            }
        }
    }
}

/// Classify a read error as either a TCC permission denial or a
/// generic other error. On macOS the kernel returns `EPERM (os error 1)`
/// with message `Operation not permitted` when Full Disk Access is
/// required and not granted; on Linux (WSL / dev containers) the same
/// error surface signals a mode issue.
fn classify_read_error(err: &(dyn std::error::Error + 'static)) -> HistoryReachability {
    let msg = err.to_string();
    let low = msg.to_lowercase();
    if low.contains("operation not permitted")
        || low.contains("permission denied")
        || low.contains("os error 1")
        || low.contains("os error 13")
    {
        return HistoryReachability::PermissionDenied;
    }
    if low.contains("no history db") || low.contains("no such file") {
        return HistoryReachability::Missing;
    }
    HistoryReachability::Unavailable(msg)
}

/// Public probe used by `tracemind capture doctor`. Attempts to copy
/// each browser's DB once and reports the classification without ever
/// emitting a payload. Pure read; safe to call at any time.
pub fn probe_browsers(home: &Path) -> Vec<(Browser, HistoryReachability)> {
    let mut out = Vec::with_capacity(2);
    for b in [Browser::Chrome, Browser::Safari] {
        let db = b.default_db(home);
        let r = if !db.exists() {
            HistoryReachability::Missing
        } else {
            match probe_copy(&db) {
                Ok(()) => HistoryReachability::Reachable,
                Err(e) => classify_read_error(e.as_ref()),
            }
        };
        out.push((b, r));
    }
    out
}

/// Attempt a single read-side operation against `db` to see whether
/// TCC lets us through. Copying is the exact operation the real
/// `read_visits` path performs first, so a successful copy here means
/// full reads should also succeed.
fn probe_copy(db: &Path) -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let staged = tmp.path().join("probe.db");
    std::fs::copy(db, &staged)?;
    Ok(())
}

/// Which browser a reader targets. Each variant knows its DB location, the
/// SQL to pull `(visit_time, url, title)`, and how to convert its native
/// timestamp epoch to/from Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Browser {
    Chrome,
    Safari,
}

impl Browser {
    pub fn key(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Safari => "safari",
        }
    }

    /// Default on-disk history DB for this browser under `$HOME`.
    fn default_db(self, home: &Path) -> PathBuf {
        match self {
            Browser::Chrome => home
                .join("Library")
                .join("Application Support")
                .join("Google")
                .join("Chrome")
                .join("Default")
                .join("History"),
            Browser::Safari => home.join("Library").join("Safari").join("History.db"),
        }
    }

    /// Convert this browser's native `visit_time` integer to Unix seconds.
    ///  - Chrome: microseconds since 1601-01-01 (WebKit/Windows epoch).
    ///  - Safari: seconds since 2001-01-01 (Cocoa epoch), stored as REAL but
    ///    read here as an integer number of seconds.
    fn to_unix_secs(self, raw: i64) -> i64 {
        match self {
            Browser::Chrome => raw / 1_000_000 - 11_644_473_600,
            Browser::Safari => raw + 978_307_200,
        }
    }

    /// Convert a Unix-seconds threshold into this browser's native units, so
    /// the `WHERE visit_time > ?` bound compares like-for-like.
    fn from_unix_secs(self, unix: i64) -> i64 {
        match self {
            Browser::Chrome => (unix + 11_644_473_600) * 1_000_000,
            Browser::Safari => unix - 978_307_200,
        }
    }

    /// SQL yielding `(visit_time, url, title)` for visits strictly newer than
    /// `:since`, oldest first, capped at `MAX_VISITS_PER_POLL`.
    fn query(self) -> &'static str {
        match self {
            Browser::Chrome => {
                "SELECT v.visit_time, u.url, u.title \
                 FROM visits v JOIN urls u ON u.id = v.url \
                 WHERE v.visit_time > :since \
                 ORDER BY v.visit_time ASC LIMIT :limit"
            }
            Browser::Safari => {
                "SELECT CAST(v.visit_time AS INTEGER), i.url, v.title \
                 FROM history_visits v JOIN history_items i ON i.id = v.history_item \
                 WHERE CAST(v.visit_time AS INTEGER) > :since \
                 ORDER BY v.visit_time ASC LIMIT :limit"
            }
        }
    }
}

/// A single captured page visit, normalized across browsers.
#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    pub raw_time: i64,
    pub url: String,
    pub title: String,
}

/// Per-browser high-water marks, persisted next to the other modality
/// cursors so daemon restarts don't re-emit visits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HistoryCursor {
    chrome_last: i64,
    safari_last: i64,
}

impl HistoryCursor {
    fn get(&self, b: Browser) -> i64 {
        match b {
            Browser::Chrome => self.chrome_last,
            Browser::Safari => self.safari_last,
        }
    }
    fn set(&mut self, b: Browser, v: i64) {
        match b {
            Browser::Chrome => self.chrome_last = v,
            Browser::Safari => self.safari_last = v,
        }
    }
}

/// Reads Chrome + Safari history and emits one page visit per `poll`.
pub struct BrowserHistorySource {
    home: PathBuf,
    data_dir: PathBuf,
    browsers: Vec<Browser>,
    cursor: HistoryCursor,
    /// Visits fetched from the DBs but not yet emitted (drained one per poll).
    queue: VecDeque<MultimodalPayload>,
    /// Per-browser "don't retry before this instant" table. Populated
    /// on TCC / permission denials so we don't hammer a protected DB
    /// every tick, and we don't spam the log with the same warning.
    /// Empty at construction; entries expire once
    /// [`PERMISSION_RETRY_COOLDOWN_SECS`] has elapsed.
    denied_until: HashMap<Browser, Instant>,
    /// Whether we've already emitted the one-time actionable warning
    /// for each browser. Reset when the browser goes from denied →
    /// reachable so a subsequent regression re-warns.
    warned: HashMap<Browser, bool>,
}

impl BrowserHistorySource {
    pub fn new(home: PathBuf, data_dir: PathBuf) -> Self {
        let cursor = load_history_cursor(&data_dir);
        Self {
            home,
            data_dir,
            browsers: vec![Browser::Chrome, Browser::Safari],
            cursor,
            queue: VecDeque::new(),
            denied_until: HashMap::new(),
            warned: HashMap::new(),
        }
    }

    /// Test hook — target explicit DB files instead of the `$HOME` defaults.
    #[cfg(test)]
    fn with_browsers(data_dir: PathBuf, browsers: Vec<Browser>, home: PathBuf) -> Self {
        Self {
            home,
            data_dir,
            browsers,
            cursor: HistoryCursor::default(),
            queue: VecDeque::new(),
            denied_until: HashMap::new(),
            warned: HashMap::new(),
        }
    }

    /// Refill `queue` by reading every configured browser once. Advances each
    /// browser's high-water mark and persists the cursor.
    fn refill(&mut self) {
        let mut changed = false;
        let now = Instant::now();
        for b in self.browsers.clone() {
            // Skip browsers we know are in permission-denial cooldown.
            if let Some(until) = self.denied_until.get(&b) {
                if now < *until {
                    continue;
                }
            }
            let db = b.default_db(&self.home);
            let since = self.effective_since(b);
            let visits = match read_visits(b, &db, since) {
                Ok(v) => {
                    // Recovered — clear the denial state and re-arm the
                    // one-shot warning so a future regression re-warns.
                    if self.denied_until.remove(&b).is_some() {
                        warn!(
                            "[history] {} recovered — history reads succeeded after \
                             previous permission denial",
                            b.key()
                        );
                    }
                    self.warned.insert(b, false);
                    v
                }
                Err(e) => {
                    let class = classify_read_error(e.as_ref());
                    match class {
                        HistoryReachability::PermissionDenied => {
                            self.denied_until.insert(
                                b,
                                now + Duration::from_secs(PERMISSION_RETRY_COOLDOWN_SECS),
                            );
                            // Warn exactly once per denial episode with
                            // the actionable remediation text — noisy
                            // enough to be noticed, quiet enough to not
                            // spam.
                            if !self.warned.get(&b).copied().unwrap_or(false) {
                                warn!(
                                    "[history] {}",
                                    HistoryReachability::PermissionDenied.describe(b),
                                );
                                self.warned.insert(b, true);
                            } else {
                                debug!(
                                    "[history] {} still permission-denied, retry in {}s",
                                    b.key(),
                                    PERMISSION_RETRY_COOLDOWN_SECS,
                                );
                            }
                        }
                        HistoryReachability::Missing => {
                            debug!("[history] {} db missing at {}", b.key(), db.display());
                        }
                        HistoryReachability::Unavailable(_) | HistoryReachability::Reachable => {
                            debug!("[history] {} read skipped: {e}", b.key());
                        }
                    }
                    continue;
                }
            };
            for v in visits {
                if v.raw_time > self.cursor.get(b) {
                    self.cursor.set(b, v.raw_time);
                    changed = true;
                }
                if let Some(p) = visit_to_payload(b, &v) {
                    self.queue.push_back(p);
                }
            }
        }
        if changed {
            save_history_cursor(&self.data_dir, &self.cursor);
        }
    }

    /// Read-only view of the source's current permission state. Used
    /// by `tracemind capture doctor`.
    pub fn permission_state(&self) -> Vec<(Browser, HistoryReachability)> {
        let now = Instant::now();
        self.browsers
            .iter()
            .map(|b| {
                let r = if self
                    .denied_until
                    .get(b)
                    .map(|t| now < *t)
                    .unwrap_or(false)
                {
                    HistoryReachability::PermissionDenied
                } else {
                    HistoryReachability::Reachable
                };
                (*b, r)
            })
            .collect()
    }

    /// The `visit_time` threshold to query from. First run (cursor 0) uses a
    /// bounded look-back; afterwards it's the stored high-water mark.
    fn effective_since(&self, b: Browser) -> i64 {
        let stored = self.cursor.get(b);
        if stored > 0 {
            return stored;
        }
        let unix_now = Utc::now().timestamp();
        b.from_unix_secs(unix_now - FIRST_RUN_LOOKBACK_SECS)
    }
}

#[async_trait]
impl ModalitySource for BrowserHistorySource {
    fn kind(&self) -> Modality {
        Modality::Web
    }
    fn name(&self) -> &'static str {
        "browser-history"
    }
    async fn poll(&mut self) -> Option<MultimodalPayload> {
        if self.queue.is_empty() {
            self.refill();
        }
        self.queue.pop_front()
    }
}

/// Render a visit as a `MultimodalPayload`. URL first so `ingest_fast`'s
/// URL gate classifies it Tier-1 and promotes immediately. Drops non-web
/// schemes (`chrome://`, `about:`, `file:`) which carry no memory value.
fn visit_to_payload(b: Browser, v: &Visit) -> Option<MultimodalPayload> {
    let url = v.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return None;
    }
    let title = v.title.trim();
    let text = if title.is_empty() {
        url.to_string()
    } else {
        format!("{url} {title}")
    };
    let unix = b.to_unix_secs(v.raw_time);
    let ts = DateTime::<Utc>::from_timestamp(unix, 0).unwrap_or_else(Utc::now);
    Some(MultimodalPayload {
        kind: Modality::Web,
        source: "browser-history".into(),
        ts,
        bytes: None,
        mime: Some("text/uri-list".into()),
        text: Some(text),
        uri: Some(url.to_string()),
        hint: (!title.is_empty()).then(|| title.to_string()),
    })
}

/// Copy `db` (+ `-wal`/`-shm` sidecars) to a private temp dir and read the
/// requested visits read-only. Copying avoids the live browser's lock and
/// guarantees we never mutate the original. Returns `Err` if the DB can't be
/// copied (e.g. Safari without Full Disk Access) or the schema is absent.
fn read_visits(b: Browser, db: &Path, since: i64) -> anyhow::Result<Vec<Visit>> {
    if !db.exists() {
        anyhow::bail!("no history db at {}", db.display());
    }
    let tmp = tempfile::tempdir()?;
    let staged = tmp.path().join("history.db");
    std::fs::copy(db, &staged)?;
    // Best-effort: bring the WAL/SHM sidecars so very recent visits are seen.
    for ext in ["-wal", "-shm"] {
        let side = with_suffix(db, ext);
        if side.exists() {
            let _ = std::fs::copy(&side, with_suffix(&staged, ext));
        }
    }

    let conn = Connection::open_with_flags(
        &staged,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let mut stmt = conn.prepare(b.query())?;
    let rows = stmt.query_map(
        rusqlite::named_params! { ":since": since, ":limit": MAX_VISITS_PER_POLL as i64 },
        |row| {
            Ok(Visit {
                raw_time: row.get(0)?,
                url: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                title: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        },
    )?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Append a raw suffix to a path's filename (e.g. `foo.db` + `-wal`).
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn history_cursor_path(data_dir: &Path) -> PathBuf {
    data_dir.join("modality-cursor.browser-history.json")
}

fn load_history_cursor(data_dir: &Path) -> HistoryCursor {
    std::fs::read_to_string(history_cursor_path(data_dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_history_cursor(data_dir: &Path, c: &HistoryCursor) {
    let path = history_cursor_path(data_dir);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string(c) {
        let _ = std::fs::write(&path, raw);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Chrome-schema history DB with the given rows.
    fn make_chrome_db(path: &Path, rows: &[(i64, &str, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER);",
        )
        .unwrap();
        for (i, (t, url, title)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            conn.execute(
                "INSERT INTO urls(id,url,title) VALUES(?,?,?)",
                rusqlite::params![id, url, title],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO visits(id,url,visit_time) VALUES(?,?,?)",
                rusqlite::params![id, id, t],
            )
            .unwrap();
        }
    }

    fn make_safari_db(path: &Path, rows: &[(i64, &str, &str)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE history_items(id INTEGER PRIMARY KEY, url TEXT);
             CREATE TABLE history_visits(id INTEGER PRIMARY KEY, history_item INTEGER, visit_time REAL, title TEXT);",
        )
        .unwrap();
        for (i, (t, url, title)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            conn.execute(
                "INSERT INTO history_items(id,url) VALUES(?,?)",
                rusqlite::params![id, url],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO history_visits(id,history_item,visit_time,title) VALUES(?,?,?,?)",
                rusqlite::params![id, id, t, title],
            )
            .unwrap();
        }
    }

    #[test]
    fn chrome_epoch_roundtrips() {
        // 2021-01-01T00:00:00Z = 1609459200 unix.
        let unix = 1_609_459_200;
        let raw = Browser::Chrome.from_unix_secs(unix);
        assert_eq!(Browser::Chrome.to_unix_secs(raw), unix);
    }

    #[test]
    fn safari_epoch_roundtrips() {
        let unix = 1_609_459_200;
        let raw = Browser::Safari.from_unix_secs(unix);
        assert_eq!(Browser::Safari.to_unix_secs(raw), unix);
    }

    #[test]
    fn reads_new_chrome_visits_and_skips_non_http() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History");
        // Chrome micros for a recent time + a chrome:// noise row.
        let t = Browser::Chrome.from_unix_secs(Utc::now().timestamp());
        make_chrome_db(
            &db,
            &[
                (t, "https://example.com/a", "Example A"),
                (t + 1, "chrome://settings", "Settings"),
                (t + 2, "https://rust-lang.org", "Rust"),
            ],
        );
        let visits = read_visits(Browser::Chrome, &db, 0).unwrap();
        assert_eq!(visits.len(), 3, "raw read returns all rows");
        let payloads: Vec<_> = visits
            .iter()
            .filter_map(|v| visit_to_payload(Browser::Chrome, v))
            .collect();
        assert_eq!(payloads.len(), 2, "chrome:// row dropped");
        assert_eq!(payloads[0].uri.as_deref(), Some("https://example.com/a"));
        assert_eq!(payloads[0].text.as_deref(), Some("https://example.com/a Example A"));
    }

    #[tokio::test]
    async fn poll_drains_queue_and_respects_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        // Lay down the Chrome DB at its default location under this fake HOME.
        let chrome_db = Browser::Chrome.default_db(&home);
        std::fs::create_dir_all(chrome_db.parent().unwrap()).unwrap();
        let base = Browser::Chrome.from_unix_secs(Utc::now().timestamp());
        make_chrome_db(
            &chrome_db,
            &[
                (base, "https://a.com", "A"),
                (base + 10, "https://b.com", "B"),
            ],
        );
        let mut src = BrowserHistorySource::with_browsers(
            data.path().to_path_buf(),
            vec![Browser::Chrome],
            home,
        );
        let p1 = src.poll().await.expect("first visit");
        let p2 = src.poll().await.expect("second visit");
        assert_eq!(p1.uri.as_deref(), Some("https://a.com"));
        assert_eq!(p2.uri.as_deref(), Some("https://b.com"));
        // Queue drained; nothing new since the cursor advanced past both.
        assert!(src.poll().await.is_none(), "no re-emission of seen visits");
    }

    #[test]
    fn safari_schema_reads() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History.db");
        let t = Browser::Safari.from_unix_secs(Utc::now().timestamp());
        make_safari_db(&db, &[(t, "https://apple.com", "Apple")]);
        let visits = read_visits(Browser::Safari, &db, 0).unwrap();
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://apple.com");
        assert_eq!(visits[0].title, "Apple");
    }

    #[test]
    fn classify_read_error_recognises_tcc_denial() {
        // The exact shape rusqlite / std::io emit on macOS when
        // Full Disk Access is missing.
        let e = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Operation not permitted (os error 1)",
        );
        assert!(classify_read_error(&e).is_permission_denied());

        let e2 = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Permission denied");
        assert!(classify_read_error(&e2).is_permission_denied());

        // A "file not found" is NOT a permission denial.
        let e3 = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory");
        assert!(!classify_read_error(&e3).is_permission_denied());
        assert!(matches!(
            classify_read_error(&e3),
            HistoryReachability::Missing
        ));
    }

    #[test]
    fn describe_permission_denied_carries_actionable_text() {
        let msg = HistoryReachability::PermissionDenied.describe(Browser::Safari);
        // The critical bits users must see.
        assert!(msg.to_lowercase().contains("full disk access"), "msg = {msg}");
        assert!(msg.to_lowercase().contains("system settings"), "msg = {msg}");
        assert!(msg.contains("safari"), "msg = {msg}");
    }

    #[test]
    fn probe_browsers_reports_missing_when_home_has_no_dbs() {
        let dir = tempfile::tempdir().unwrap();
        let rows = probe_browsers(dir.path());
        assert_eq!(rows.len(), 2);
        // Neither browser DB exists under this fake HOME.
        for (_, r) in &rows {
            assert_eq!(*r, HistoryReachability::Missing);
        }
    }

    #[test]
    fn probe_browsers_reports_reachable_when_db_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        // Create Chrome's DB under the fake HOME.
        let chrome_db = Browser::Chrome.default_db(dir.path());
        std::fs::create_dir_all(chrome_db.parent().unwrap()).unwrap();
        let t = Browser::Chrome.from_unix_secs(Utc::now().timestamp());
        make_chrome_db(&chrome_db, &[(t, "https://ex.com", "Ex")]);

        let rows = probe_browsers(dir.path());
        let chrome_row = rows.iter().find(|(b, _)| *b == Browser::Chrome).unwrap();
        assert!(chrome_row.1.is_reachable(), "chrome should probe OK");
        let safari_row = rows.iter().find(|(b, _)| *b == Browser::Safari).unwrap();
        assert_eq!(safari_row.1, HistoryReachability::Missing);
    }
}
