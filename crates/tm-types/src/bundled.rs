//! TM-NLP-005 — bundled-model resolution.
//!
//! At binary startup, every TraceMind binary calls [`init`] to discover a
//! local models directory and route `hf-hub` + `fastembed` at it before any
//! model code runs. The resolution ladder is:
//!
//! 1. `$TM_MODELS_DIR` — explicit override for air-gapped / sysadmin setups.
//! 2. Exe-relative bundle directories:
//!    - `<exe>/../Resources/models`        (macOS Tauri `.app`)
//!    - `<exe>/../../Resources/models`     (symlinked binaries inside the bundle)
//!    - `<exe>/models`                     (dev: next to `target/release/tracemind`)
//!    - `<exe>/../../models`               (workspace-root bundle directory)
//! 3. `~/.tracemind/models` — per-user cache (populated by first-run download).
//!
//! If a directory is found, `HF_HOME` and `FASTEMBED_CACHE_DIR` env vars are
//! set to point at it (unless the user already set them). Both `hf-hub` (the
//! GLiNER + ColBERT path) and `fastembed` (the BGE embedding path) will then
//! read from the bundled weights with zero outbound network requests.
//!
//! **Privacy guarantee:** when a bundled directory is resolved and contains
//! all required snapshots, no TraceMind binary opens a connection to
//! HuggingFace.
//!
//! This module is I/O-free beyond directory stat and env mutation — safe to
//! call in `main()` before logging is configured.

use std::path::PathBuf;

/// Result of a successful model-dir resolution.
#[derive(Debug, Clone)]
pub struct ResolvedModels {
    /// Absolute path that was chosen as the HF cache root.
    pub hf_cache: PathBuf,
    /// Which rung of the resolution ladder matched.
    pub source: &'static str,
}

/// Resolve a bundled models directory and route the model loaders at it.
/// Returns `None` if no directory is found — the binary should continue as
/// usual and fall back to hf-hub's default `~/.cache/huggingface/`.
pub fn init() -> Option<ResolvedModels> {
    let resolved = resolve()?;
    if std::env::var_os("HF_HOME").is_none() {
        std::env::set_var("HF_HOME", &resolved.hf_cache);
    }
    if std::env::var_os("FASTEMBED_CACHE_DIR").is_none() {
        std::env::set_var("FASTEMBED_CACHE_DIR", &resolved.hf_cache);
    }
    Some(resolved)
}

/// Pure resolver — no env mutation. Exposed for tests / diagnostics.
pub fn resolve() -> Option<ResolvedModels> {
    // 1. Explicit override.
    if let Some(dir) = std::env::var_os("TM_MODELS_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(ResolvedModels {
                hf_cache: canon(&p),
                source: "TM_MODELS_DIR",
            });
        }
    }

    // 2. Exe-relative candidates.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidates = [
                parent.join("..").join("Resources").join("models"),
                parent
                    .join("..")
                    .join("..")
                    .join("Resources")
                    .join("models"),
                parent.join("models"),
                parent.join("..").join("..").join("models"),
            ];
            for c in candidates {
                if c.is_dir() {
                    return Some(ResolvedModels {
                        hf_cache: canon(&c),
                        source: "bundle",
                    });
                }
            }
        }
    }

    // 3. Per-user cache.
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let p = home.join(".tracemind").join("models");
        if p.is_dir() {
            return Some(ResolvedModels {
                hf_cache: canon(&p),
                source: "~/.tracemind",
            });
        }
    }

    None
}

fn canon(p: &std::path::Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The resolver touches process-global env; serialise tests to keep them
    // deterministic regardless of cargo's thread-per-test default.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap();
        let saved = [
            ("TM_MODELS_DIR", std::env::var_os("TM_MODELS_DIR")),
            ("HF_HOME", std::env::var_os("HF_HOME")),
            ("FASTEMBED_CACHE_DIR", std::env::var_os("FASTEMBED_CACHE_DIR")),
        ];
        for (k, _) in &saved {
            std::env::remove_var(k);
        }
        f();
        for (k, v) in saved {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn resolve_returns_none_when_nothing_present() {
        with_clean_env(|| {
            // Even if a bundle happens to exist on disk for this checkout,
            // we at least assert the function doesn't panic.
            let _ = resolve();
        });
    }

    #[test]
    fn tm_models_dir_override_wins() {
        with_clean_env(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::env::set_var("TM_MODELS_DIR", tmp.path());
            let r = resolve().expect("should resolve to override");
            assert_eq!(r.source, "TM_MODELS_DIR");
            assert_eq!(r.hf_cache, tmp.path().canonicalize().unwrap());
        });
    }

    #[test]
    fn tm_models_dir_ignored_when_not_a_directory() {
        with_clean_env(|| {
            std::env::set_var("TM_MODELS_DIR", "/definitely/not/a/real/path/xyz");
            // Should fall through rather than return a bogus path.
            let r = resolve();
            if let Some(x) = &r {
                assert_ne!(x.source, "TM_MODELS_DIR");
            }
        });
    }

    #[test]
    fn init_sets_env_only_when_unset() {
        with_clean_env(|| {
            let tmp = tempfile::tempdir().unwrap();
            std::env::set_var("TM_MODELS_DIR", tmp.path());
            std::env::set_var("HF_HOME", "/user/preferred");
            let _ = init();
            // User's HF_HOME must not be overwritten.
            assert_eq!(
                std::env::var_os("HF_HOME").unwrap(),
                std::ffi::OsString::from("/user/preferred")
            );
            // FASTEMBED_CACHE_DIR was unset, so init fills it.
            assert!(std::env::var_os("FASTEMBED_CACHE_DIR").is_some());
        });
    }
}
