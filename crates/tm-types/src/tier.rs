//! Memory tier classification: quarantine / cold / warm / hot.

use serde::{Deserialize, Serialize};

/// A freshness / trust classification for memory entries.
///
/// Ordering: `Quarantine < Cold < Warm < Hot` (derived `Ord` reflects this).
/// Quarantine is the ingestion holding pen introduced by
/// `docs/INGESTION_EXPERIENCE_PLAN-2026-07-22.md` §3.2 — items sit here
/// for 48h and are re-gated before being promoted to `Cold`/`Warm`/`Hot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    Quarantine,
    Cold,
    Warm,
    Hot,
}

impl MemoryTier {
    /// Static string label used for storage.
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryTier::Hot => "hot",
            MemoryTier::Warm => "warm",
            MemoryTier::Cold => "cold",
            MemoryTier::Quarantine => "quarantine",
        }
    }

    /// Classify a memory entry by how long ago (in seconds) it was last accessed.
    ///
    /// * `< 86_400 s` (24 h) → Hot
    /// * `< 604_800 s` (7 d) → Warm
    /// * otherwise          → Cold
    ///
    /// Quarantine is *never* returned from this function — it is applied
    /// explicitly at ingest time.
    pub fn from_access_recency(last_accessed_secs_ago: u64) -> Self {
        if last_accessed_secs_ago < 86_400 {
            MemoryTier::Hot
        } else if last_accessed_secs_ago < 604_800 {
            MemoryTier::Warm
        } else {
            MemoryTier::Cold
        }
    }

    /// Parse from the string form stored in SQLite.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "hot" => Some(MemoryTier::Hot),
            "warm" => Some(MemoryTier::Warm),
            "cold" => Some(MemoryTier::Cold),
            "quarantine" => Some(MemoryTier::Quarantine),
            _ => None,
        }
    }

    /// How long (in seconds) an item in this tier should sit before being
    /// automatically promoted / re-evaluated. Currently only `Quarantine`
    /// has a promotion clock (48h); all other tiers age via access recency
    /// rather than a wall-clock timer.
    pub fn promote_after_secs(self) -> Option<u64> {
        match self {
            MemoryTier::Quarantine => Some(172_800),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_boundaries() {
        assert_eq!(MemoryTier::from_access_recency(0), MemoryTier::Hot);
        assert_eq!(MemoryTier::from_access_recency(86_399), MemoryTier::Hot);
        assert_eq!(MemoryTier::from_access_recency(86_400), MemoryTier::Warm);
        assert_eq!(MemoryTier::from_access_recency(604_799), MemoryTier::Warm);
        assert_eq!(MemoryTier::from_access_recency(604_800), MemoryTier::Cold);
        assert_eq!(MemoryTier::from_access_recency(u64::MAX), MemoryTier::Cold);
    }

    #[test]
    fn ordering_cold_lt_warm_lt_hot() {
        assert!(MemoryTier::Cold < MemoryTier::Warm);
        assert!(MemoryTier::Warm < MemoryTier::Hot);
    }

    #[test]
    fn quarantine_is_lowest() {
        assert!(MemoryTier::Quarantine < MemoryTier::Cold);
        assert!(MemoryTier::Quarantine < MemoryTier::Warm);
        assert!(MemoryTier::Quarantine < MemoryTier::Hot);
    }

    #[test]
    fn round_trip_str() {
        for tier in [
            MemoryTier::Hot,
            MemoryTier::Warm,
            MemoryTier::Cold,
            MemoryTier::Quarantine,
        ] {
            assert_eq!(MemoryTier::from_str(tier.as_str()), Some(tier));
        }
    }

    #[test]
    fn serde_round_trip() {
        let encoded = serde_json::to_string(&MemoryTier::Hot).unwrap();
        assert_eq!(encoded, "\"hot\"");
        let decoded: MemoryTier = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, MemoryTier::Hot);

        let q = serde_json::to_string(&MemoryTier::Quarantine).unwrap();
        assert_eq!(q, "\"quarantine\"");
        let decoded_q: MemoryTier = serde_json::from_str(&q).unwrap();
        assert_eq!(decoded_q, MemoryTier::Quarantine);
    }

    #[test]
    fn promote_after_only_for_quarantine() {
        assert_eq!(MemoryTier::Quarantine.promote_after_secs(), Some(172_800));
        assert_eq!(MemoryTier::Cold.promote_after_secs(), None);
        assert_eq!(MemoryTier::Warm.promote_after_secs(), None);
        assert_eq!(MemoryTier::Hot.promote_after_secs(), None);
    }
}
