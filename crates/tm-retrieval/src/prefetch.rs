//! L1 prefetch cache — short-circuits the SQLite kNN lookup when the
//! world model has anticipated a query.
//!
//! ## Why this exists
//!
//! `tm-intent` declares `AnticipationKind::PrefetchQuery` for the
//! "silent pre-warm" predictive layer (L1, see
//! `docs/INTENT_SYSTEM.md` §6). When the world model emits a
//! prefetch anticipation, an outer orchestration layer is supposed
//! to ask the retrieval engine to *warm* a query in advance, so that
//! when the user actually types it (or something close), the
//! candidate list is ready.
//!
//! This module is the place that ready-list lives. It is deliberately
//! decoupled from `tm-intent` itself — the cache doesn't know about
//! Anticipations. Whoever has access to both the `IntentStore` and
//! the `RetrievalEngine` is responsible for translating
//! `PrefetchQuery → engine.prime_prefetch(query)`.
//!
//! Keeping the cache anticipation-agnostic also means the same
//! mechanism powers other warm-up paths later: explicit user
//! "preload these queries" hints, scheduled brief warms, etc.
//!
//! ## What the cache stores
//!
//! For each primed query string, we keep:
//! - the embedding we produced (so we don't pay BGE twice if the
//!   user's query is identical), and
//! - the top-k `(entity_id, score)` candidates from the graph kNN.
//!
//! Entries expire after a configurable TTL. The cache is bounded;
//! oldest entries are evicted FIFO when full.
//!
//! ## What the cache does NOT do
//!
//! - It does **not** rewrite or fuzzy-match queries. Lookup is by
//!   exact normalized string. Anticipations should specify the
//!   query the user is likely to type, verbatim.
//! - It does not skip rerank, MMR, graph expansion, or any later
//!   phase. The cache only short-circuits the vector kNN.
//! - It is in-RAM only — drained on process restart. Anticipations
//!   in `IntentStore` are persistent; the *cache* is the hot copy.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// One cached prefetch entry. Stores the precomputed query embedding
/// and the kNN result so a follow-up `query(text)` matching this key
/// can skip both the embedder call and the SQLite kNN.
#[derive(Debug, Clone)]
pub struct PrefetchEntry {
    pub query: String,
    pub embedding: Vec<f32>,
    pub candidates: Vec<(Uuid, f32)>,
    inserted_at: Instant,
}

impl PrefetchEntry {
    /// `true` when this entry has lived past its TTL and should be
    /// dropped on next eviction sweep.
    pub fn is_expired(&self, ttl: Duration, now: Instant) -> bool {
        now.duration_since(self.inserted_at) > ttl
    }
}

/// Telemetry surfaced via [`PrefetchCache::stats`]. The retrieval
/// engine bubbles these out so callers can size the TTL / capacity
/// against measured hit rate, and so the brief / status surface can
/// say something like "L1 hit-rate: 38% over 142 queries".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrefetchStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub primes: u64,
}

impl PrefetchStats {
    /// Hit rate over total `lookup` calls. Returns `0.0` when no
    /// lookups have happened yet — by design, not a panic.
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f32 / total as f32
    }
}

/// In-RAM, bounded, TTL-evicting prefetch cache.
///
/// Cloning is cheap (the inner map is `HashMap`) but you usually
/// want one cache *per* `RetrievalEngine` and access it via
/// `&mut`. Two engines sharing one cache would race on hits.
#[derive(Debug)]
pub struct PrefetchCache {
    entries: HashMap<String, PrefetchEntry>,
    ttl: Duration,
    capacity: usize,
    insertion_order: Vec<String>,
    stats: PrefetchStats,
}

impl PrefetchCache {
    /// Default TTL — 5 minutes. Anticipations carry their own
    /// `expires_at`; the cache TTL is the *engine's* upper bound on
    /// how long a warmed result is considered fresh, independent of
    /// the predicting source.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);
    /// Default capacity — 32 prefetched queries. Comfortably above
    /// the working-memory width (8) so a few queries can be held in
    /// reserve per WM slot.
    pub const DEFAULT_CAPACITY: usize = 32;

    pub fn new() -> Self {
        Self::with_config(Self::DEFAULT_TTL, Self::DEFAULT_CAPACITY)
    }

    pub fn with_config(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            ttl,
            capacity,
            insertion_order: Vec::with_capacity(capacity),
            stats: PrefetchStats::default(),
        }
    }

    /// Number of live (not-yet-evicted) entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn stats(&self) -> PrefetchStats {
        self.stats
    }

    /// Drop everything. Useful for "new session" boundaries and
    /// tests.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.insertion_order.clear();
    }

    /// Normalize a query string the same way for inserts and lookups
    /// so trivial whitespace/casing differences don't miss the cache.
    /// Kept conservative — Unicode normalization is overkill here.
    fn key(query: &str) -> String {
        query.trim().to_lowercase()
    }

    /// Insert a primed entry. Replaces any existing entry for the
    /// same normalized key. Evicts the oldest entry FIFO when the
    /// cache is at capacity. Capacity-0 caches drop the prime on the
    /// floor.
    pub fn prime(&mut self, query: &str, embedding: Vec<f32>, candidates: Vec<(Uuid, f32)>) {
        if self.capacity == 0 {
            return;
        }
        let key = Self::key(query);
        let entry = PrefetchEntry {
            query: key.clone(),
            embedding,
            candidates,
            inserted_at: Instant::now(),
        };

        // If this key already exists, update in place — don't double-count
        // it in insertion_order.
        if self.entries.insert(key.clone(), entry).is_some() {
            // existing — re-anchor in insertion_order to mark it freshest
            self.insertion_order.retain(|k| k != &key);
            self.insertion_order.push(key);
        } else {
            self.insertion_order.push(key);
            // Cap the size by evicting the oldest live key.
            while self.insertion_order.len() > self.capacity {
                let oldest = self.insertion_order.remove(0);
                if self.entries.remove(&oldest).is_some() {
                    self.stats.evictions += 1;
                }
            }
        }
        self.stats.primes += 1;
    }

    /// Look up a primed entry. Updates hit/miss stats. Expired
    /// entries are treated as a miss and dropped on the spot.
    pub fn lookup(&mut self, query: &str) -> Option<PrefetchEntry> {
        let key = Self::key(query);
        let now = Instant::now();
        let expired = match self.entries.get(&key) {
            Some(e) => e.is_expired(self.ttl, now),
            None => {
                self.stats.misses += 1;
                return None;
            }
        };
        if expired {
            self.entries.remove(&key);
            self.insertion_order.retain(|k| k != &key);
            self.stats.evictions += 1;
            self.stats.misses += 1;
            return None;
        }
        self.stats.hits += 1;
        self.entries.get(&key).cloned()
    }

    /// Sweep all expired entries. Cheap to call periodically (e.g.
    /// from a background brief generator).
    pub fn evict_expired(&mut self) -> usize {
        let now = Instant::now();
        let ttl = self.ttl;
        let before = self.entries.len();
        let dead: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_expired(ttl, now))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &dead {
            self.entries.remove(k);
        }
        if !dead.is_empty() {
            self.insertion_order.retain(|k| !dead.contains(k));
            self.stats.evictions += dead.len() as u64;
        }
        before - self.entries.len()
    }

    /// Iterate primed query keys oldest-first. Mostly for debug /
    /// status surfaces.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.insertion_order.iter().map(|s| s.as_str())
    }
}

impl Default for PrefetchCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn prime_then_lookup_hits() {
        let mut c = PrefetchCache::new();
        c.prime("hello world", vec![0.1, 0.2], vec![(id(1), 0.9)]);
        let hit = c.lookup("hello world").expect("hit");
        assert_eq!(hit.candidates, vec![(id(1), 0.9)]);
        assert_eq!(c.stats().hits, 1);
        assert_eq!(c.stats().misses, 0);
    }

    #[test]
    fn lookup_misses_when_unprimed() {
        let mut c = PrefetchCache::new();
        assert!(c.lookup("not seen").is_none());
        assert_eq!(c.stats().misses, 1);
    }

    #[test]
    fn key_normalizes_case_and_whitespace() {
        let mut c = PrefetchCache::new();
        c.prime("  Hello World  ", vec![], vec![(id(7), 1.0)]);
        let hit = c.lookup("hello world").unwrap();
        assert_eq!(hit.candidates, vec![(id(7), 1.0)]);
    }

    #[test]
    fn priming_same_key_overwrites_in_place() {
        let mut c = PrefetchCache::with_config(Duration::from_secs(60), 4);
        c.prime("q", vec![], vec![(id(1), 0.5)]);
        c.prime("q", vec![], vec![(id(2), 0.9)]);
        let hit = c.lookup("q").unwrap();
        assert_eq!(hit.candidates, vec![(id(2), 0.9)]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn capacity_evicts_oldest_fifo() {
        let mut c = PrefetchCache::with_config(Duration::from_secs(60), 2);
        c.prime("a", vec![], vec![(id(1), 0.1)]);
        c.prime("b", vec![], vec![(id(2), 0.2)]);
        c.prime("c", vec![], vec![(id(3), 0.3)]);
        assert_eq!(c.len(), 2);
        assert!(c.lookup("a").is_none()); // evicted
        assert!(c.lookup("b").is_some());
        assert!(c.lookup("c").is_some());
        assert_eq!(c.stats().evictions, 1);
    }

    #[test]
    fn capacity_zero_drops_primes() {
        let mut c = PrefetchCache::with_config(Duration::from_secs(60), 0);
        c.prime("q", vec![], vec![(id(1), 0.5)]);
        assert!(c.is_empty());
        assert!(c.lookup("q").is_none());
    }

    #[test]
    fn ttl_expires_entries_on_lookup() {
        let mut c = PrefetchCache::with_config(Duration::from_millis(20), 4);
        c.prime("q", vec![], vec![(id(1), 0.5)]);
        sleep(Duration::from_millis(40));
        assert!(c.lookup("q").is_none());
        assert_eq!(c.stats().misses, 1);
        assert_eq!(c.stats().evictions, 1);
        assert!(c.is_empty());
    }

    #[test]
    fn evict_expired_sweeps_dead_entries() {
        let mut c = PrefetchCache::with_config(Duration::from_millis(20), 4);
        c.prime("a", vec![], vec![(id(1), 0.1)]);
        c.prime("b", vec![], vec![(id(2), 0.2)]);
        sleep(Duration::from_millis(40));
        c.prime("c", vec![], vec![(id(3), 0.3)]); // fresh
        let dropped = c.evict_expired();
        assert_eq!(dropped, 2);
        assert_eq!(c.len(), 1);
        assert!(c.lookup("c").is_some());
    }

    #[test]
    fn hit_rate_is_zero_with_no_lookups() {
        let s = PrefetchStats::default();
        assert_eq!(s.hit_rate(), 0.0);
    }

    #[test]
    fn hit_rate_reflects_observed_traffic() {
        let mut c = PrefetchCache::new();
        c.prime("a", vec![], vec![(id(1), 0.1)]);
        let _ = c.lookup("a"); // hit
        let _ = c.lookup("a"); // hit
        let _ = c.lookup("b"); // miss
        let s = c.stats();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert!((s.hit_rate() - (2.0 / 3.0)).abs() < 1e-6);
    }

    #[test]
    fn clear_drops_everything_but_keeps_stats() {
        let mut c = PrefetchCache::new();
        c.prime("a", vec![], vec![(id(1), 0.1)]);
        let _ = c.lookup("a");
        c.clear();
        assert!(c.is_empty());
        // stats are observability — clearing the cache should NOT
        // reset them (they describe the cache's lifetime behavior).
        assert_eq!(c.stats().hits, 1);
        assert_eq!(c.stats().primes, 1);
    }

    #[test]
    fn keys_returns_insertion_order() {
        let mut c = PrefetchCache::with_config(Duration::from_secs(60), 4);
        c.prime("a", vec![], vec![]);
        c.prime("b", vec![], vec![]);
        c.prime("c", vec![], vec![]);
        let keys: Vec<&str> = c.keys().collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }
}
