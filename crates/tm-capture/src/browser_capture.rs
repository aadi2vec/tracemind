//! CAP-4 — browser bookmarklet capture endpoint.
//!
//! Listens on `127.0.0.1:7710` for `POST /capture` from a tiny
//! bookmarklet the user installs in their browser. The request body
//! is the JSON `{url, title, selection?, ts?}` payload and must carry
//! a `Authorization: Bearer <token>` header whose value matches the
//! per-install token stored at `<data_dir>/capture_token`.
//!
//! The server is hand-rolled (no axum / hyper-server dep) because the
//! surface is intentionally tiny: one route, one method, loopback
//! only. Refuses anything that isn't `POST /capture` or `GET /health`.
//! Requests larger than 1 MiB are dropped to avoid OOM by an
//! adversarial local process.
//!
//! Privacy posture:
//! - bind address is `127.0.0.1` only — never `0.0.0.0`
//! - token is generated locally on first start; the bookmarklet
//!   snippet (emitted by `tracemind capture bookmarklet`) embeds it
//! - body is rate-limited via the standard `IngestPipeline`
//!   `RateLimiter` (CAP-5), so a runaway bookmarklet loop can't
//!   stall queries

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};
use uuid::Uuid;

use tm_ingest::IngestPipeline;
use tm_types::capture_permissions::{CapturePermissions, CaptureSource};

const BIND_ADDR: &str = "127.0.0.1:7710";
const MAX_BODY_BYTES: usize = 1 << 20; // 1 MiB
const MAX_HEADER_BYTES: usize = 8 * 1024; // 8 KiB

/// Payload shape the bookmarklet POSTs to `/capture`. Only `url` is
/// required; the rest are best-effort. `ts` is ignored on the server
/// (we trust the local clock and stamp the ingest time ourselves).
#[derive(Debug, Deserialize)]
struct BrowserCapturePayload {
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    selection: String,
}

/// Public entrypoint — call from the capture daemon's `tokio::join!`
/// block. Returns when the listener fails to bind; otherwise loops
/// forever serving requests.
pub async fn browser_capture_loop(
    permissions_path: PathBuf,
    db_path: String,
    token_path: PathBuf,
) {
    // CAP-1 — fail closed on the browser source.
    if !load_enabled(&permissions_path, CaptureSource::Browser) {
        info!("[browser] disabled in permissions — endpoint will not start");
        return;
    }

    let token = match load_or_create_token(&token_path) {
        Ok(t) => t,
        Err(e) => {
            warn!("[browser] failed to load/create token at {}: {e}", token_path.display());
            return;
        }
    };

    let listener = match TcpListener::bind(BIND_ADDR).await {
        Ok(l) => l,
        Err(e) => {
            warn!("[browser] failed to bind {BIND_ADDR}: {e}");
            return;
        }
    };
    info!("[browser] capture endpoint listening on http://{BIND_ADDR}");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                debug!("[browser] accept failed: {e}");
                continue;
            }
        };
        // Reject non-loopback peers defensively (TcpListener bound to
        // 127.0.0.1 already rejects, but if someone sets BIND_ADDR via
        // a future env override, this is the second line of defence).
        if !peer.ip().is_loopback() {
            debug!("[browser] non-loopback peer rejected: {peer}");
            continue;
        }
        let token_clone = token.clone();
        let db = db_path.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_one(stream, &token_clone, &db).await {
                debug!("[browser] connection error: {e}");
            }
        });
    }
}

/// Read one HTTP/1.1 request, dispatch, write one response, close.
async fn handle_one(
    mut stream: TcpStream,
    expected_token: &str,
    db_path: &str,
) -> std::io::Result<()> {
    // Read until "\r\n\r\n" or MAX_HEADER_BYTES.
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 1024];
    let mut header_end = None;
    while buf.len() < MAX_HEADER_BYTES {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(idx) = find_double_crlf(&buf) {
            header_end = Some(idx);
            break;
        }
    }
    let Some(header_end) = header_end else {
        return write_response(&mut stream, 400, "bad request", "missing headers\n").await;
    };

    let headers_bytes = &buf[..header_end];
    let body_already_read = &buf[header_end + 4..];

    let headers_str = match std::str::from_utf8(headers_bytes) {
        Ok(s) => s,
        Err(_) => {
            return write_response(&mut stream, 400, "bad request", "non-utf8 headers\n").await;
        }
    };

    // Parse request line + headers.
    let mut lines = headers_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let mut content_length: usize = 0;
    let mut auth: Option<&str> = None;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        match k.as_str() {
            "content-length" => content_length = v.parse().unwrap_or(0),
            "authorization" => auth = Some(v),
            _ => {}
        }
    }

    // Health probe used by the CLI bookmarklet installer to verify
    // the daemon is up. No auth required; returns 200 with the
    // server name so the user knows what answered.
    if method == "GET" && path == "/health" {
        return write_response(
            &mut stream,
            200,
            "ok",
            "{\"server\":\"tracemind-capture\",\"endpoint\":\"/capture\"}\n",
        )
        .await;
    }

    if method != "POST" || path != "/capture" {
        return write_response(&mut stream, 404, "not found", "use POST /capture\n").await;
    }

    // Auth: constant-time-ish prefix check.
    let supplied = auth.and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    if !ct_eq(supplied.as_bytes(), expected_token.as_bytes()) {
        return write_response(&mut stream, 401, "unauthorized", "bad or missing token\n").await;
    }

    if content_length > MAX_BODY_BYTES {
        return write_response(&mut stream, 413, "too large", "body too large\n").await;
    }

    // Read remainder of body.
    let mut body = Vec::with_capacity(content_length.min(MAX_BODY_BYTES));
    body.extend_from_slice(body_already_read);
    while body.len() < content_length {
        let need = content_length - body.len();
        let mut chunk = vec![0u8; need.min(4096)];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    if body.len() != content_length {
        return write_response(&mut stream, 400, "bad request", "truncated body\n").await;
    }

    let payload: BrowserCapturePayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            return write_response(
                &mut stream,
                400,
                "bad request",
                &format!("invalid json: {e}\n"),
            )
            .await;
        }
    };

    if payload.url.is_empty() {
        return write_response(&mut stream, 400, "bad request", "url is required\n").await;
    }

    // Render the captured payload as a single line. The url goes
    // first so the URL/file gate in `ingest_fast` classifies this
    // as Tier-1 (InstantEntity) and promotes it immediately.
    let mut text = payload.url.clone();
    if !payload.title.is_empty() {
        text.push(' ');
        text.push_str(&payload.title);
    }
    if !payload.selection.is_empty() {
        text.push_str(" — ");
        text.push_str(payload.selection.trim());
    }

    // Move the heavy lifting (pipeline open + ingest) off the IO
    // task; this also lets us cap concurrent ingests via the
    // pipeline's CAP-5 rate limiter automatically.
    let db_path_owned = db_path.to_string();
    let ingest_result = tokio::task::spawn_blocking(move || {
        let pipeline = IngestPipeline::open(&db_path_owned, /*hash_embed=*/ false)?;
        pipeline.ingest_fast(&text, "browser", Uuid::new_v4())
    })
    .await;

    match ingest_result {
        Ok(Ok(result)) => {
            if let Some(reason) = &result.skipped {
                info!("[browser] captured but skipped: {reason}");
                write_response(
                    &mut stream,
                    202,
                    "accepted",
                    &format!("{{\"status\":\"skipped\",\"reason\":\"{reason}\"}}\n"),
                )
                .await
            } else {
                info!("[browser] captured signal_id={}", result.signal_id);
                write_response(&mut stream, 200, "ok", "{\"status\":\"stored\"}\n").await
            }
        }
        Ok(Err(e)) => {
            warn!("[browser] ingest failed: {e}");
            write_response(&mut stream, 500, "error", "ingest failed\n").await
        }
        Err(e) => {
            warn!("[browser] join error: {e}");
            write_response(&mut stream, 500, "error", "internal error\n").await
        }
    }
}

async fn write_response(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Constant-time-ish byte comparison. Not cryptographically rigorous
/// (we're not dealing with timing-adversaries on localhost) but
/// avoids the early-exit of `==`.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Read the existing token, or generate + persist a new one (32 hex
/// chars from a v4 UUID — 128 bits of entropy is plenty for a
/// localhost gate).
fn load_or_create_token(path: &Path) -> std::io::Result<String> {
    if path.exists() {
        return std::fs::read_to_string(path).map(|s| s.trim().to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let token = Uuid::new_v4().simple().to_string();
    std::fs::write(path, &token)?;
    // Best-effort chmod 600 on Unix so other users can't read it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    info!("[browser] new capture token written to {}", path.display());
    Ok(token)
}

/// Same fail-closed permission load used by the other loops.
fn load_enabled(path: &Path, source: CaptureSource) -> bool {
    match CapturePermissions::load_or_default(path) {
        Ok(perms) => perms.is_enabled(source),
        Err(e) => {
            warn!(
                "[browser] failed to load permissions ({}); failing closed: {e}",
                path.display()
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_double_crlf_finds_separator() {
        assert_eq!(find_double_crlf(b"GET /\r\n\r\nbody"), Some(5));
        assert_eq!(find_double_crlf(b"no separator"), None);
    }

    #[test]
    fn ct_eq_correct() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }

    #[test]
    fn load_or_create_token_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture_token");
        let t1 = load_or_create_token(&path).unwrap();
        let t2 = load_or_create_token(&path).unwrap();
        assert_eq!(t1, t2);
        assert_eq!(t1.len(), 32, "uuid simple = 32 hex chars");
    }
}
