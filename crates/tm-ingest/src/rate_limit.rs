//! Per-source ingestion rate limiter (CAP-5).
//!
//! The ambient capture layer can fire many events per second when the
//! user is typing fast, scrolling, or screenshotting in a loop. Without
//! a throttle, embedding + indexing throughput becomes the bottleneck
//! and queries stall behind the backlog. CAP-5's promise is: *"capture
//! never breaks query latency."*
//!
//! Design: a classic token bucket, one bucket per source string.
//! Each successful `try_acquire(source)` costs one token; tokens
//! refill at `per_minute / 60` tokens per second up to `burst`.
//! Unknown sources fall back to a conservative default bucket
//! (`other`), so an attacker pumping an unknown source can't bypass
//! the limit.
//!
//! Defaults (per minute / burst) — tuned to "user is busy but
//! reasonable":
//!
//! | source     | rate/min | burst | rationale                           |
//! |------------|---------:|------:|-------------------------------------|
//! | clipboard  |       60 |    30 | one paste/sec is already noisy      |
//! | shell      |      120 |    60 | command bursts during build/test    |
//! | notes      |       30 |    15 | backfill is bursty, no continuous   |
//! | screenshot |       12 |     6 | OCR is expensive; 1 every 5s        |
//! | browser    |       60 |    30 | tab switches + selections           |
//! | audio      |       30 |    15 | Whisper-tiny is ~1s on M-series     |
//! | calendar   |        6 |     3 | refresh-driven; should not burst    |
//! | other      |       30 |    15 | unknown source ⇒ conservative       |
//!
//! Overridable per source via env:
//! - `TM_RATE_<SOURCE>_PER_MIN` — integer per-minute refill rate
//! - `TM_RATE_<SOURCE>_BURST`   — integer burst capacity
//!
//! Set either to `0` to disable the limit for that source (used by
//! tests + the CLI's interactive `ingest` subcommand, which is
//! human-paced and shouldn't be rate-limited).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Per-source limit configuration. Lives at construction time; the
/// runtime state (current token count) is on `RateLimiter`.
#[derive(Debug, Clone)]
pub struct SourceLimit {
    /// Steady-state refill rate, tokens per minute.
    pub per_minute: u32,
    /// Max tokens the bucket can hold (controls burst tolerance).
    pub burst: u32,
}

impl SourceLimit {
    /// Disabled — every call to `try_acquire` succeeds.
    pub const fn disabled() -> Self {
        Self {
            per_minute: 0,
            burst: 0,
        }
    }

    /// True if this source bypasses rate limiting.
    pub fn is_disabled(&self) -> bool {
        self.per_minute == 0 && self.burst == 0
    }

    /// Refill rate in tokens per second.
    pub fn tokens_per_sec(&self) -> f64 {
        f64::from(self.per_minute) / 60.0
    }
}

/// One bucket of tokens, refilled lazily on each `try_acquire`.
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
    limit: SourceLimit,
}

impl Bucket {
    fn new(limit: SourceLimit, now: Instant) -> Self {
        Self {
            tokens: f64::from(limit.burst),
            last_refill: now,
            limit,
        }
    }

    /// Refill the bucket up to its burst cap based on elapsed time,
    /// then try to consume one token. Returns true on success.
    fn try_consume(&mut self, now: Instant) -> bool {
        if self.limit.is_disabled() {
            return true;
        }
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.limit.tokens_per_sec())
            .min(f64::from(self.limit.burst));
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Shared rate limiter. Cheap to clone (it's a `Mutex` behind an
/// `Arc`-friendly handle pattern — but we keep it `Send + Sync` via
/// the `Mutex` and don't expose a clone).
#[derive(Debug)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    defaults: HashMap<String, SourceLimit>,
    /// Fallback bucket for sources we've never seen.
    fallback: SourceLimit,
}

impl RateLimiter {
    /// Construct a limiter with the CAP-5 default table, allowing
    /// env-variable overrides for ops + tests.
    pub fn from_env() -> Self {
        let table: &[(&str, u32, u32)] = &[
            ("clipboard", 60, 30),
            ("shell", 120, 60),
            ("notes", 30, 15),
            ("screenshot", 12, 6),
            ("browser", 60, 30),
            ("audio", 30, 15),
            ("calendar", 6, 3),
        ];
        let mut defaults = HashMap::new();
        for (name, per_min, burst) in table {
            defaults.insert((*name).to_string(), resolve_limit(name, *per_min, *burst));
        }
        Self {
            buckets: Mutex::new(HashMap::new()),
            defaults,
            fallback: resolve_limit("other", 30, 15),
        }
    }

    /// Build a limiter with every source disabled. Used by tests and
    /// the human-paced `ingest` CLI command.
    pub fn unlimited() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            defaults: HashMap::new(),
            fallback: SourceLimit::disabled(),
        }
    }

    /// Attempt to acquire one token for `source`. Returns `Ok(())`
    /// when the call should proceed, or `Err(reason)` with a
    /// human-readable explanation when the bucket is empty.
    pub fn try_acquire(&self, source: &str) -> Result<(), String> {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(source.to_string()).or_insert_with(|| {
            let limit = self
                .defaults
                .get(source)
                .cloned()
                .unwrap_or_else(|| self.fallback.clone());
            Bucket::new(limit, now)
        });
        if bucket.try_consume(now) {
            Ok(())
        } else {
            let limit = &bucket.limit;
            Err(format!(
                "rate-limited: {source} exceeded {}/min (burst {})",
                limit.per_minute, limit.burst
            ))
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Resolve a (per_minute, burst) pair against the env. Env vars
/// override the compile-time defaults; setting both to `0` disables
/// the limit for that source.
fn resolve_limit(source: &str, default_per_min: u32, default_burst: u32) -> SourceLimit {
    let upper = source.to_uppercase();
    let per_minute = std::env::var(format!("TM_RATE_{upper}_PER_MIN"))
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default_per_min);
    let burst = std::env::var(format!("TM_RATE_{upper}_BURST"))
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default_burst);
    SourceLimit { per_minute, burst }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn disabled_limit_always_passes() {
        let l = RateLimiter::unlimited();
        for _ in 0..1000 {
            assert!(l.try_acquire("clipboard").is_ok());
        }
    }

    #[test]
    fn burst_then_refusal() {
        // Force a tight bucket via env override.
        std::env::set_var("TM_RATE_TEST_BURST_PER_MIN", "60");
        std::env::set_var("TM_RATE_TEST_BURST_BURST", "3");
        let l = RateLimiter::from_env();
        // Unknown to defaults → fallback, but we'll exercise the
        // env-resolved path by calling `try_acquire` on a custom
        // source through the public API. Since `defaults` is built at
        // construction, `test_burst` won't be in the table → uses
        // fallback. Construct a focused limiter instead:
        let mut buckets = HashMap::new();
        buckets.insert(
            "shell".to_string(),
            Bucket::new(
                SourceLimit {
                    per_minute: 60,
                    burst: 3,
                },
                Instant::now(),
            ),
        );
        let direct = RateLimiter {
            buckets: Mutex::new(buckets),
            defaults: HashMap::new(),
            fallback: SourceLimit::disabled(),
        };
        // 3 succeed (burst), 4th refused.
        assert!(direct.try_acquire("shell").is_ok());
        assert!(direct.try_acquire("shell").is_ok());
        assert!(direct.try_acquire("shell").is_ok());
        let err = direct.try_acquire("shell").unwrap_err();
        assert!(err.contains("rate-limited"));
        // Cleanup env so other tests aren't affected.
        let _ = l; // keep `from_env` used so the compiler doesn't drop the test
        std::env::remove_var("TM_RATE_TEST_BURST_PER_MIN");
        std::env::remove_var("TM_RATE_TEST_BURST_BURST");
    }

    #[test]
    fn refills_over_time() {
        // 600/min = 10/sec; burst 1 — after one token consumed, wait
        // a hair over 100ms, refill should give us one more. We use a
        // faster refill rate than production so the test doesn't add
        // a noticeable delay to the parallel test runner.
        let mut buckets = HashMap::new();
        buckets.insert(
            "shell".to_string(),
            Bucket::new(
                SourceLimit {
                    per_minute: 600,
                    burst: 1,
                },
                Instant::now(),
            ),
        );
        let l = RateLimiter {
            buckets: Mutex::new(buckets),
            defaults: HashMap::new(),
            fallback: SourceLimit::disabled(),
        };
        assert!(l.try_acquire("shell").is_ok());
        assert!(l.try_acquire("shell").is_err());
        sleep(Duration::from_millis(150));
        assert!(
            l.try_acquire("shell").is_ok(),
            "bucket should have refilled after 150ms at 10 tok/s"
        );
    }

    #[test]
    fn unknown_source_uses_fallback() {
        let l = RateLimiter::from_env();
        // Default fallback is 30/min, burst 15 — exhaust it.
        let mut last_err = None;
        for i in 0..50 {
            if let Err(e) = l.try_acquire("custom_unknown_source") {
                last_err = Some((i, e));
                break;
            }
        }
        let (i, e) = last_err.expect("fallback bucket must refuse eventually");
        assert!(i <= 16, "should refuse within burst (got at {i})");
        assert!(e.contains("rate-limited"));
    }

    #[test]
    fn env_override_changes_per_minute() {
        std::env::set_var("TM_RATE_CALENDAR_PER_MIN", "0");
        std::env::set_var("TM_RATE_CALENDAR_BURST", "0");
        let l = RateLimiter::from_env();
        // Disabled — 1000 acquires must all pass.
        for _ in 0..1000 {
            assert!(l.try_acquire("calendar").is_ok());
        }
        std::env::remove_var("TM_RATE_CALENDAR_PER_MIN");
        std::env::remove_var("TM_RATE_CALENDAR_BURST");
    }
}
