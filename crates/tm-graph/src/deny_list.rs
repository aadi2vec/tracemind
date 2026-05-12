//! LM-11 — Cross-context deny list (scaffold).
//!
//! Persistent, file-backed allow/block list for cross-context bridges.
//! This is the *cheap path* of PROJECT_2026 §1c primitive 3b (context
//! splicing): when the user repeatedly marks a bridged result as
//! `wrong_context_suggestion`, we auto-promote that context pair (or
//! entity pair) into a deny rule so future retrievals stop suggesting
//! the same bad bridge.
//!
//! **Scope of this commit:** schema + loader/saver + strike accounting
//! only. Wiring into the retrieval pipeline and the
//! `wrong_context_suggestion` feedback path lands in later LM-11 tasks.
//! Keeping the file format independent of the call sites means we can
//! ship the data layer today and let the retrieval team consume it
//! when the rest of the deny-list flow is ready.
//!
//! File location (default): `~/.tracemind/cross_ctx_block_list.json`.
//! The format is plain JSON — users can inspect / hand-edit / version-
//! control their own block list, which is itself a legibility win.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

/// Strike count at which a pair is promoted from "watching" to
/// "blocked". Three matches the "3-strike auto-add" rule documented in
/// PROJECT_2026 §1c. Exposed so tests can override it without going
/// through environment variables.
pub const DEFAULT_STRIKE_THRESHOLD: u32 = 3;

/// Current on-disk schema version. Bump when the format changes in a
/// non-additive way; the loader will refuse to read a newer version.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level on-disk shape. Two parallel rule tables (context pairs and
/// entity pairs) because the user-felt complaint operates at both
/// granularities: "stop bridging *fictional* into *real-life* contexts"
/// (context pair) and "don't link Hogwarts to Alcatraz specifically"
/// (entity pair).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyList {
    pub version: u32,
    pub updated_at: DateTime<Utc>,
    /// Threshold at which a watching rule auto-blocks. Persisted so an
    /// org / user can tune sensitivity per-install.
    #[serde(default = "default_threshold")]
    pub strike_threshold: u32,
    pub context_pairs: Vec<ContextPairRule>,
    pub entity_pairs: Vec<EntityPairRule>,
}

fn default_threshold() -> u32 {
    DEFAULT_STRIKE_THRESHOLD
}

impl Default for DenyList {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            updated_at: Utc::now(),
            strike_threshold: DEFAULT_STRIKE_THRESHOLD,
            context_pairs: Vec::new(),
            entity_pairs: Vec::new(),
        }
    }
}

/// A directional ban between two contexts. The pair is stored
/// canonically (sorted lexically by UUID) so `(a, b)` and `(b, a)`
/// collapse to a single rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPairRule {
    pub context_a: Uuid,
    pub context_b: Uuid,
    pub strikes: u32,
    pub blocked: bool,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub notes: String,
}

/// Same shape as [`ContextPairRule`] but at the entity level. Used when
/// the user wants to ban specific bridges (e.g. "Hogwarts ↔ Alcatraz")
/// without banning the contexts they live in wholesale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityPairRule {
    pub entity_a: Uuid,
    pub entity_b: Uuid,
    pub strikes: u32,
    pub blocked: bool,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub notes: String,
}

/// Outcome of [`DenyList::record_context_strike`] / `record_entity_strike`.
/// `JustBlocked` is the signal callers need to surface "we've stopped
/// bridging X ↔ Y" toast UX — it fires exactly once, on the strike that
/// crossed the threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrikeOutcome {
    /// Strike recorded; pair is still under the threshold.
    Watching { strikes: u32 },
    /// Strike recorded; pair just crossed the threshold and is now blocked.
    JustBlocked { strikes: u32 },
    /// Strike recorded against an already-blocked pair (idempotent).
    AlreadyBlocked { strikes: u32 },
}

impl DenyList {
    /// Canonical (sorted) UUID pair so directionality doesn't double-store.
    fn canon(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
        if a <= b { (a, b) } else { (b, a) }
    }

    /// Default deny-list file path under the user's TraceMind data dir.
    pub fn default_path(data_dir: &Path) -> PathBuf {
        data_dir.join("cross_ctx_block_list.json")
    }

    /// Load the deny-list from disk. Missing file → empty default;
    /// unreadable / malformed file → loud error (we don't want to
    /// silently lose user-curated rules).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|e| {
            TraceMindError::Storage(format!("read deny-list {}: {e}", path.display()))
        })?;
        let parsed: Self = serde_json::from_str(&raw).map_err(|e| {
            TraceMindError::Storage(format!("parse deny-list {}: {e}", path.display()))
        })?;
        if parsed.version > SCHEMA_VERSION {
            return Err(TraceMindError::Storage(format!(
                "deny-list schema v{} newer than supported v{SCHEMA_VERSION}",
                parsed.version
            )));
        }
        Ok(parsed)
    }

    /// Persist atomically: write to a temp file and rename. This avoids
    /// truncated reads if the process is killed mid-write.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.updated_at = Utc::now();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                TraceMindError::Storage(format!("mkdir {}: {e}", parent.display()))
            })?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            TraceMindError::Storage(format!("serialize deny-list: {e}"))
        })?;
        std::fs::write(&tmp, json).map_err(|e| {
            TraceMindError::Storage(format!("write {}: {e}", tmp.display()))
        })?;
        std::fs::rename(&tmp, path).map_err(|e| {
            TraceMindError::Storage(format!(
                "rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            ))
        })?;
        Ok(())
    }

    /// True iff bridging from `a` to `b` (or `b` to `a`) is denied
    /// either at the context or entity level. The retrieval pipeline
    /// will eventually call this on every candidate cross-context
    /// bridge.
    pub fn is_context_pair_blocked(&self, a: Uuid, b: Uuid) -> bool {
        let (lo, hi) = Self::canon(a, b);
        self.context_pairs
            .iter()
            .any(|r| r.blocked && r.context_a == lo && r.context_b == hi)
    }

    /// True iff a specific entity pair has been denied.
    pub fn is_entity_pair_blocked(&self, a: Uuid, b: Uuid) -> bool {
        let (lo, hi) = Self::canon(a, b);
        self.entity_pairs
            .iter()
            .any(|r| r.blocked && r.entity_a == lo && r.entity_b == hi)
    }

    /// LM-15 — true iff a bridge between two entities should be denied
    /// purely on **ontological-domain** grounds (e.g. real person vs
    /// fictional person). This is the **type-level** Harry Potter fix
    /// that sits alongside the strike-based block list.
    ///
    /// Returns `false` for any pair where either side is
    /// [`tm_types::OntologicalDomain::Unknown`] so the deny path stays
    /// conservative when classification abstains.
    pub fn is_bridge_blocked_by_ontology(
        domain_a: tm_types::OntologicalDomain,
        domain_b: tm_types::OntologicalDomain,
    ) -> bool {
        !domain_a.is_compatible_with(domain_b)
    }

    /// Record a `wrong_context_suggestion` strike against a context
    /// pair. Auto-promotes to `blocked` when `strikes >= strike_threshold`.
    ///
    /// Callers should `save()` after one or more strikes; we don't
    /// fsync per-strike to keep the hot path cheap.
    pub fn record_context_strike(
        &mut self,
        a: Uuid,
        b: Uuid,
        note: &str,
    ) -> StrikeOutcome {
        let (lo, hi) = Self::canon(a, b);
        let threshold = self.strike_threshold;
        let now = Utc::now();
        if let Some(rule) = self
            .context_pairs
            .iter_mut()
            .find(|r| r.context_a == lo && r.context_b == hi)
        {
            rule.strikes += 1;
            rule.last_seen = now;
            if !note.is_empty() {
                rule.notes = note.to_string();
            }
            if rule.blocked {
                return StrikeOutcome::AlreadyBlocked { strikes: rule.strikes };
            }
            if rule.strikes >= threshold {
                rule.blocked = true;
                return StrikeOutcome::JustBlocked { strikes: rule.strikes };
            }
            return StrikeOutcome::Watching { strikes: rule.strikes };
        }
        // New rule.
        let rule = ContextPairRule {
            context_a: lo,
            context_b: hi,
            strikes: 1,
            blocked: 1 >= threshold,
            first_seen: now,
            last_seen: now,
            notes: note.to_string(),
        };
        let blocked = rule.blocked;
        self.context_pairs.push(rule);
        if blocked {
            StrikeOutcome::JustBlocked { strikes: 1 }
        } else {
            StrikeOutcome::Watching { strikes: 1 }
        }
    }

    /// Same as `record_context_strike` but at entity-pair granularity.
    pub fn record_entity_strike(
        &mut self,
        a: Uuid,
        b: Uuid,
        note: &str,
    ) -> StrikeOutcome {
        let (lo, hi) = Self::canon(a, b);
        let threshold = self.strike_threshold;
        let now = Utc::now();
        if let Some(rule) = self
            .entity_pairs
            .iter_mut()
            .find(|r| r.entity_a == lo && r.entity_b == hi)
        {
            rule.strikes += 1;
            rule.last_seen = now;
            if !note.is_empty() {
                rule.notes = note.to_string();
            }
            if rule.blocked {
                return StrikeOutcome::AlreadyBlocked { strikes: rule.strikes };
            }
            if rule.strikes >= threshold {
                rule.blocked = true;
                return StrikeOutcome::JustBlocked { strikes: rule.strikes };
            }
            return StrikeOutcome::Watching { strikes: rule.strikes };
        }
        let rule = EntityPairRule {
            entity_a: lo,
            entity_b: hi,
            strikes: 1,
            blocked: 1 >= threshold,
            first_seen: now,
            last_seen: now,
            notes: note.to_string(),
        };
        let blocked = rule.blocked;
        self.entity_pairs.push(rule);
        if blocked {
            StrikeOutcome::JustBlocked { strikes: 1 }
        } else {
            StrikeOutcome::Watching { strikes: 1 }
        }
    }

    /// Manually pin a context pair as blocked regardless of strike
    /// count. Used by the eventual `tracemind context block <A> <B>`
    /// CLI; exposed here so the file format and entry point are
    /// shippable now.
    pub fn block_context_pair(&mut self, a: Uuid, b: Uuid, note: &str) {
        let (lo, hi) = Self::canon(a, b);
        let now = Utc::now();
        if let Some(rule) = self
            .context_pairs
            .iter_mut()
            .find(|r| r.context_a == lo && r.context_b == hi)
        {
            rule.blocked = true;
            rule.last_seen = now;
            if !note.is_empty() {
                rule.notes = note.to_string();
            }
            return;
        }
        self.context_pairs.push(ContextPairRule {
            context_a: lo,
            context_b: hi,
            strikes: self.strike_threshold, // make it look like it earned the block
            blocked: true,
            first_seen: now,
            last_seen: now,
            notes: note.to_string(),
        });
    }

    /// Drop a context-pair rule entirely. Returns `true` iff a row was
    /// removed. Used by the eventual `tracemind context unblock` CLI.
    pub fn unblock_context_pair(&mut self, a: Uuid, b: Uuid) -> bool {
        let (lo, hi) = Self::canon(a, b);
        let before = self.context_pairs.len();
        self.context_pairs
            .retain(|r| !(r.context_a == lo && r.context_b == hi));
        before != self.context_pairs.len()
    }

    /// Cheap in-memory summary for status surfaces / metrics. Returns
    /// `(blocked_ctx, watching_ctx, blocked_ent, watching_ent)`.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let (mut bc, mut wc, mut be, mut we) = (0, 0, 0, 0);
        for r in &self.context_pairs {
            if r.blocked { bc += 1 } else { wc += 1 }
        }
        for r in &self.entity_pairs {
            if r.blocked { be += 1 } else { we += 1 }
        }
        (bc, wc, be, we)
    }

    /// Build a fast-lookup index from canonical pair → rule index, used
    /// by retrieval-time hot paths that need to filter many candidate
    /// bridges per query. The map is intentionally Vec-index-based so
    /// the live list stays the source of truth.
    pub fn context_pair_index(&self) -> HashMap<(Uuid, Uuid), usize> {
        self.context_pairs
            .iter()
            .enumerate()
            .map(|(i, r)| ((r.context_a, r.context_b), i))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_orders_pair() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        assert_eq!(DenyList::canon(a, b), (a, b));
        assert_eq!(DenyList::canon(b, a), (a, b));
    }

    #[test]
    fn three_strikes_promotes_to_blocked() {
        let mut dl = DenyList::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert!(!dl.is_context_pair_blocked(a, b));
        assert_eq!(
            dl.record_context_strike(a, b, "first"),
            StrikeOutcome::Watching { strikes: 1 }
        );
        assert_eq!(
            dl.record_context_strike(a, b, "second"),
            StrikeOutcome::Watching { strikes: 2 }
        );
        assert_eq!(
            dl.record_context_strike(a, b, "third"),
            StrikeOutcome::JustBlocked { strikes: 3 }
        );
        assert!(dl.is_context_pair_blocked(a, b));
        // Direction-insensitive.
        assert!(dl.is_context_pair_blocked(b, a));
        // Fourth strike is idempotent.
        assert_eq!(
            dl.record_context_strike(a, b, "fourth"),
            StrikeOutcome::AlreadyBlocked { strikes: 4 }
        );
    }

    #[test]
    fn manual_block_then_unblock() {
        let mut dl = DenyList::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        dl.block_context_pair(a, b, "operator-set");
        assert!(dl.is_context_pair_blocked(a, b));
        assert!(dl.unblock_context_pair(a, b));
        assert!(!dl.is_context_pair_blocked(a, b));
        assert!(!dl.unblock_context_pair(a, b)); // already gone
    }

    #[test]
    fn entity_pair_strikes_work_independently() {
        let mut dl = DenyList::default();
        let ctx_a = Uuid::new_v4();
        let ctx_b = Uuid::new_v4();
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        // Strikes on the context pair must not affect entity pair state.
        dl.record_context_strike(ctx_a, ctx_b, "ctx strike");
        assert!(!dl.is_entity_pair_blocked(e1, e2));
        dl.record_entity_strike(e1, e2, "");
        dl.record_entity_strike(e1, e2, "");
        dl.record_entity_strike(e1, e2, "");
        assert!(dl.is_entity_pair_blocked(e1, e2));
        assert!(!dl.is_context_pair_blocked(ctx_a, ctx_b));
    }

    #[test]
    fn load_missing_returns_default() {
        let path = std::env::temp_dir().join(format!(
            "tm-deny-missing-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let dl = DenyList::load(&path).expect("load missing -> default");
        assert_eq!(dl.version, SCHEMA_VERSION);
        assert!(dl.context_pairs.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "tm-deny-rt-{}-{}.json",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_file(&path);
        let mut dl = DenyList::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        dl.record_context_strike(a, b, "first");
        dl.record_context_strike(a, b, "second");
        dl.save(&path).expect("save");

        let loaded = DenyList::load(&path).expect("load");
        assert_eq!(loaded.context_pairs.len(), 1);
        assert_eq!(loaded.context_pairs[0].strikes, 2);
        assert!(!loaded.context_pairs[0].blocked);
        assert_eq!(loaded.strike_threshold, DEFAULT_STRIKE_THRESHOLD);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reject_newer_schema_version() {
        let path = std::env::temp_dir().join(format!(
            "tm-deny-future-{}.json",
            std::process::id()
        ));
        let future = serde_json::json!({
            "version": SCHEMA_VERSION + 1,
            "updated_at": Utc::now().to_rfc3339(),
            "strike_threshold": 3,
            "context_pairs": [],
            "entity_pairs": []
        });
        std::fs::write(&path, future.to_string()).unwrap();
        let err = DenyList::load(&path).expect_err("must reject newer schema");
        assert!(matches!(err, TraceMindError::Storage(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn counts_split_blocked_vs_watching() {
        let mut dl = DenyList::default();
        // One blocked context pair.
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        dl.block_context_pair(a, b, "");
        // One watching context pair.
        dl.record_context_strike(Uuid::new_v4(), Uuid::new_v4(), "");
        // One blocked entity pair (three strikes).
        let e1 = Uuid::new_v4();
        let e2 = Uuid::new_v4();
        dl.record_entity_strike(e1, e2, "");
        dl.record_entity_strike(e1, e2, "");
        dl.record_entity_strike(e1, e2, "");
        let (bc, wc, be, we) = dl.counts();
        assert_eq!((bc, wc, be, we), (1, 1, 1, 0));
    }
}
