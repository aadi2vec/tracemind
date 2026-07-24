//! Memory tier classification: hot / warm / cold.

use serde::{Deserialize, Serialize};

/// A three-level freshness classification for memory entries.
///
/// Ordering: `Cold < Warm < Hot` (derived `Ord` reflects this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
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
        }
    }

    /// Classify a memory entry by how long ago (in seconds) it was last accessed.
    ///
    /// * `< 86_400 s` (24 h) → Hot
    /// * `< 604_800 s` (7 d) → Warm
    /// * otherwise          → Cold
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
    fn round_trip_str() {
        for tier in [MemoryTier::Hot, MemoryTier::Warm, MemoryTier::Cold] {
            assert_eq!(MemoryTier::from_str(tier.as_str()), Some(tier));
        }
    }

    #[test]
    fn serde_round_trip() {
        let encoded = serde_json::to_string(&MemoryTier::Hot).unwrap();
        assert_eq!(encoded, "\"hot\"");
        let decoded: MemoryTier = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, MemoryTier::Hot);
    }
}
