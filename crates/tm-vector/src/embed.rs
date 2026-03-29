//! Hash-based fake embedder for TraceMind MVP.
//!
//! Phase 2 will swap this out for a real ONNX model (e.g. all-MiniLM-L6-v2).
//! The current implementation is deterministic and fast: it seeds a pseudo-random
//! walk from a seahash of the input text and L2-normalises the result.

use seahash;

/// A deterministic text embedder producing 384-dimensional unit vectors.
///
/// In the MVP this uses a hash-seeded arithmetic walk rather than a neural
/// network, so the geometry is fake — but the interface is identical to what
/// Phase 2 will expose, making the swap-in trivial.
pub struct Embedder {
    dim: usize,
}

impl Embedder {
    /// Create a new embedder with the default dimension (384).
    pub fn new() -> Self {
        Self { dim: 384 }
    }

    /// Embed `text` into a unit vector of length `self.dim`.
    ///
    /// Algorithm:
    /// 1. Hash the UTF-8 bytes with seahash.
    /// 2. Fill a `Vec<f32>` using a simple LCG-style step per index so that
    ///    every dimension depends on both the hash and the index.
    /// 3. L2-normalise the result in-place.
    pub fn embed(&self, text: &str) -> Vec<f32> {
        let h = seahash::hash(text.as_bytes()) as u64;

        let mut v: Vec<f32> = (0..self.dim)
            .map(|i| {
                let mixed = h
                    .wrapping_mul(i as u64 + 1)
                    .wrapping_add((i as u64).wrapping_mul(6_364_136_223_846_793_005));
                // Map the full u64 range to [-1.0, 1.0].
                (mixed as f32) / (u64::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        // L2-normalise.
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }

        v
    }
}

impl Default for Embedder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_hello_has_correct_length_and_unit_norm() {
        let embedder = Embedder::new();
        let v = embedder.embed("hello");

        assert_eq!(v.len(), 384, "embedding length must be 384");

        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "L2 norm should be ≈ 1.0, got {norm}"
        );
    }

    #[test]
    fn embed_is_deterministic() {
        let embedder = Embedder::new();
        assert_eq!(embedder.embed("hello"), embedder.embed("hello"));
    }

    #[test]
    fn embed_different_texts_differ() {
        let embedder = Embedder::new();
        assert_ne!(embedder.embed("hello"), embedder.embed("world"));
    }
}
