//! macOS Vision-framework OCR — X5 quality upgrade.
//!
//! When `feature = "macos-screencapture"` is on, `ocr_png_file(path)`
//! shells out to a small Swift shim that uses `VNRecognizeTextRequest`
//! and returns the extracted text. When the feature is off, or the OS
//! isn't macOS, or `swift` isn't on PATH, `ocr_png_file` returns
//! `None` — the daemon proceeds without OCR and the screenshot is
//! still stored (recoverable by content hash).
//!
//! **Why shell out instead of `objc2-vision`?** Vision needs an
//! Objective-C bridge; the mature Rust bindings pull in several
//! transitive crates and are moving targets. A ~40-line Swift script
//! embedded via `include_str!` is smaller, has zero new Cargo deps,
//! and works on any Mac with Xcode Command Line Tools (already a
//! developer-machine prerequisite for compiling this project).

use std::path::Path;

/// Recognized text from `png_path`, or `None` when OCR is unavailable,
/// the file is unreadable, or Vision returned no text. Never panics.
///
/// Feature-gated: without `macos-screencapture`, always returns `None`.
/// With the feature, still returns `None` on any of:
/// - not macOS
/// - `swift` binary missing (Xcode CLI Tools not installed)
/// - Vision returned an empty result (image contained no text)
/// - the Swift shim timed out or crashed
pub fn ocr_png_file(png_path: &Path) -> Option<String> {
    #[cfg(all(feature = "macos-screencapture", target_os = "macos"))]
    {
        real_ocr::ocr_png_file(png_path)
    }
    #[cfg(not(all(feature = "macos-screencapture", target_os = "macos")))]
    {
        let _ = png_path;
        None
    }
}

/// Whether Vision OCR is compiled in and the runtime prerequisites
/// (macOS + `swift` on PATH) are satisfied. Cheap to call; used by
/// `tracemind capture doctor` to report the OCR path's health.
pub fn ocr_available() -> bool {
    #[cfg(all(feature = "macos-screencapture", target_os = "macos"))]
    {
        real_ocr::swift_on_path()
    }
    #[cfg(not(all(feature = "macos-screencapture", target_os = "macos")))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ocr_returns_none_without_feature_or_off_macos() {
        // With `macos-screencapture` off (or not on macOS), the wrapper
        // must be a pure no-op: it's called from the hot capture path
        // and can't take the daemon down on a missing swift binary.
        let fake = PathBuf::from("/nonexistent/tm/does-not-exist.png");
        // Under `cfg(feature = "macos-screencapture", target_os =
        // "macos")` this actually invokes the shim, which will fail
        // gracefully (returning None). Either way — None is the only
        // acceptable answer for a missing path.
        let out = ocr_png_file(&fake);
        assert!(out.is_none(), "missing file must yield None, got {out:?}");
    }

    #[cfg(all(feature = "macos-screencapture", target_os = "macos"))]
    #[test]
    fn ocr_available_reports_true_when_swift_present() {
        // In this build config the wrapper delegates to the real
        // module. If `swift --version` succeeds on the test machine,
        // ocr_available() must be true. If it doesn't succeed, this
        // test is a no-op (skips) rather than a false failure.
        if real_ocr::swift_on_path() {
            assert!(ocr_available(), "swift is on PATH; ocr must be available");
        } else {
            eprintln!("swift missing — skipping availability assertion");
        }
    }
}

#[cfg(all(feature = "macos-screencapture", target_os = "macos"))]
mod real_ocr {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::time::Duration;

    /// Embed the Swift shim at compile time so the daemon binary is
    /// self-contained — no external file to ship or locate.
    const SWIFT_SRC: &str = include_str!("vision_ocr.swift");

    /// Write the Swift shim to the user's temp dir once per process
    /// and cache the path. Doing this on every call would race
    /// concurrent screenshot ticks and is wasteful.
    static SHIM_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

    fn shim_path() -> Option<&'static Path> {
        SHIM_PATH
            .get_or_init(|| {
                let dir = std::env::temp_dir();
                let path = dir.join(format!("tm_vision_ocr_{}.swift", std::process::id()));
                match std::fs::File::create(&path)
                    .and_then(|mut f| f.write_all(SWIFT_SRC.as_bytes()).map(|_| f))
                {
                    Ok(_) => Some(path),
                    Err(_) => None,
                }
            })
            .as_deref()
    }

    /// Cheap probe — `swift --version` is fast, we run it once per
    /// process and cache the result.
    static SWIFT_OK: OnceLock<bool> = OnceLock::new();
    pub fn swift_on_path() -> bool {
        *SWIFT_OK.get_or_init(|| {
            Command::new("swift")
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }

    /// Hard wall-clock cap so a runaway OCR call can't stall the
    /// capture daemon. Vision on a Retina screenshot usually returns
    /// in <500ms; giving 15s is generous for unusually large images.
    const OCR_TIMEOUT: Duration = Duration::from_secs(15);

    pub fn ocr_png_file(png_path: &Path) -> Option<String> {
        if !swift_on_path() {
            return None;
        }
        let shim = shim_path()?;

        let mut child = Command::new("swift")
            .arg(shim)
            .arg(png_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;

        // Best-effort timeout — poll `try_wait` on a short sleep loop.
        // std::process::Child doesn't expose a native timeout so we
        // roll one; the OCR call is single-shot and I/O-bound.
        let deadline = std::time::Instant::now() + OCR_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        tracing::debug!(
                            "[vision_ocr] timed out after {:?} on {}",
                            OCR_TIMEOUT,
                            png_path.display()
                        );
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
        let out = child.wait_with_output().ok()?;
        if !out.status.success() {
            tracing::debug!(
                "[vision_ocr] swift exited non-zero ({}) on {}: {}",
                out.status,
                png_path.display(),
                String::from_utf8_lossy(&out.stderr).trim(),
            );
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}
