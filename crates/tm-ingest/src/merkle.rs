//! Attestable export — Merkle-proof of completeness (I18 / S10).
//!
//! The user can export their entire memory in a way that an auditor can
//! verify was complete: every included item hashes into a Merkle root, and
//! the export includes the root so any tampering (or silent omission) can
//! be detected after the fact.
//!
//! The tree is binary, unbalanced only at the last leaf (duplicated to
//! stay a power of two), and uses SHA-256 as the leaf + interior hash.
//! Kept in-crate to avoid a heavy `merkle-tree` dep — the workspace
//! already pulls `sha2` transitively.
//!
//! `ExportManifest` is the audit-facing JSON envelope: the root, the ordered
//! leaves, and a schema version so an offline auditor knows how to
//! reconstruct the root from just the leaves.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The current export schema version. Increment whenever the leaf-shape
/// changes so auditors can pick the right verifier.
pub const EXPORT_SCHEMA_VERSION: u32 = 1;

/// One leaf entry in the export.
///
/// `id` is opaque (usually a UUID); `hash` is the SHA-256 of the leaf
/// payload as a hex string. Callers hash their own payload so this module
/// stays payload-agnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExportLeaf {
    pub id: String,
    pub hash: String,
}

impl ExportLeaf {
    pub fn from_bytes(id: impl Into<String>, payload: &[u8]) -> Self {
        Self {
            id: id.into(),
            hash: sha256_hex(payload),
        }
    }
}

/// Full export manifest — root + schema + leaves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExportManifest {
    pub schema_version: u32,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub root: String,
    pub leaves: Vec<ExportLeaf>,
}

impl ExportManifest {
    pub fn build(leaves: Vec<ExportLeaf>) -> Self {
        let root = merkle_root(&leaves);
        Self {
            schema_version: EXPORT_SCHEMA_VERSION,
            generated_at: chrono::Utc::now(),
            root,
            leaves,
        }
    }

    /// Recompute the root from `leaves` and compare against `root`.
    /// Returns `true` when they match — the manifest has not been tampered
    /// with. Returns `false` for any mismatch (added, removed, reordered,
    /// or edited leaf).
    pub fn verify(&self) -> bool {
        merkle_root(&self.leaves) == self.root
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(&mut out, "{:02x}", b).expect("string write");
    }
    out
}

fn hex_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = hex_nibble(bytes[i]);
        let lo = hex_nibble(bytes[i + 1]);
        out.push((hi << 4) | lo);
        i += 2;
    }
    out
}

fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

/// Compute the Merkle root over an ordered list of leaves.
///
/// Returns the empty-hash sentinel (SHA-256 of the empty string) when no
/// leaves are provided — this keeps the manifest well-defined for a fresh
/// user with zero memories.
pub fn merkle_root(leaves: &[ExportLeaf]) -> String {
    if leaves.is_empty() {
        return sha256_hex(b"");
    }
    let mut layer: Vec<Vec<u8>> = leaves.iter().map(|l| hex_decode(&l.hash)).collect();
    while layer.len() > 1 {
        if layer.len() % 2 != 0 {
            layer.push(layer.last().cloned().expect("non-empty"));
        }
        layer = layer
            .chunks(2)
            .map(|pair| {
                let mut hasher = Sha256::new();
                hasher.update(&pair[0]);
                hasher.update(&pair[1]);
                hasher.finalize().to_vec()
            })
            .collect();
    }
    hex_encode(&layer[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_manifest_has_stable_root() {
        let m = ExportManifest::build(vec![]);
        assert_eq!(m.root, sha256_hex(b""));
        assert!(m.verify());
    }

    #[test]
    fn single_leaf_root_equals_leaf_hash() {
        // With exactly one leaf there is no pairing step — the root is the
        // leaf hash itself. This matches Bitcoin's Merkle-root-of-one
        // convention and keeps `verify` consistent.
        let leaves = vec![ExportLeaf::from_bytes("m1", b"hello")];
        let m = ExportManifest::build(leaves.clone());
        assert_eq!(m.root, leaves[0].hash);
        assert!(m.verify());
    }

    #[test]
    fn tampered_leaf_fails_verify() {
        let mut m = ExportManifest::build(vec![
            ExportLeaf::from_bytes("m1", b"a"),
            ExportLeaf::from_bytes("m2", b"b"),
        ]);
        m.leaves[0].hash = sha256_hex(b"different");
        assert!(!m.verify());
    }

    #[test]
    fn added_leaf_fails_verify() {
        let mut m = ExportManifest::build(vec![ExportLeaf::from_bytes("m1", b"a")]);
        m.leaves.push(ExportLeaf::from_bytes("m2", b"b"));
        assert!(!m.verify());
    }

    #[test]
    fn removed_leaf_fails_verify() {
        let mut m = ExportManifest::build(vec![
            ExportLeaf::from_bytes("m1", b"a"),
            ExportLeaf::from_bytes("m2", b"b"),
        ]);
        m.leaves.pop();
        assert!(!m.verify());
    }

    #[test]
    fn reordered_leaves_fails_verify() {
        let leaves = vec![
            ExportLeaf::from_bytes("m1", b"a"),
            ExportLeaf::from_bytes("m2", b"b"),
            ExportLeaf::from_bytes("m3", b"c"),
        ];
        let mut m = ExportManifest::build(leaves.clone());
        m.leaves.reverse();
        assert!(!m.verify());
    }

    #[test]
    fn manifest_json_round_trip_verifies() {
        let m = ExportManifest::build(vec![
            ExportLeaf::from_bytes("m1", b"one"),
            ExportLeaf::from_bytes("m2", b"two"),
            ExportLeaf::from_bytes("m3", b"three"),
        ]);
        let encoded = serde_json::to_string(&m).unwrap();
        let decoded: ExportManifest = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.verify());
        assert_eq!(decoded.root, m.root);
    }
}
