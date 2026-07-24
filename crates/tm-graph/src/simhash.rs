//! SimHash signatures for two-stage approximate nearest-neighbour search.
//!
//! The signal search path used to load every unconsolidated embedding and
//! cosine-score it, capped at the oldest 2,000 rows. That cap was a *silent*
//! recall cliff: past 2,000 unconsolidated captures the newest memories were
//! dropped with no error, so recall decayed as a function of how long the
//! user had the product installed — the worst failure shape a memory system
//! can have (holistic review §5 P2.6).
//!
//! This module replaces the O(n) full-embedding scan with a two-stage index:
//!
//! 1. **Cheap prefilter.** Each signal carries a 64-bit SimHash signature
//!    (one `i64` column, 8 bytes) computed from its embedding at insert time.
//!    A query's signature is compared to every stored signature by Hamming
//!    distance — a single `xor` + `count_ones`, ~1 ns each — so millions of
//!    signatures can be ranked per query without touching a 384-dim vector.
//! 2. **Exact rerank.** Only the top candidates by Hamming distance (plus a
//!    recency window, so a brand-new memory is never dropped for a stale
//!    signature) have their full embeddings loaded and cosine-scored.
//!
//! SimHash's guarantee is that the probability two vectors' signature bits
//! agree is proportional to their angular similarity, so Hamming distance is
//! a monotone-in-expectation proxy for cosine distance. The exact rerank
//! then restores precision on the small candidate set. The hyperplanes are
//! generated from a fixed seed so a signature is stable across runs and
//! processes.

/// Number of random hyperplanes / signature bits.
pub const SIMHASH_BITS: usize = 64;

/// Deterministic seed for the hyperplane generator. Changing it invalidates
/// every stored signature, so it is a constant, not a config.
const SIMHASH_SEED: u64 = 0x5eed_51a4_5c00_0001;

/// Compute the 64-bit SimHash signature of `embedding`.
///
/// Bit `i` is set when the embedding is on the positive side of the `i`-th
/// random hyperplane. Deterministic for a given embedding.
pub fn signature(embedding: &[f32]) -> u64 {
    if embedding.is_empty() {
        return 0;
    }
    let mut sig: u64 = 0;
    for bit in 0..SIMHASH_BITS {
        let mut dot = 0.0f32;
        let mut rng = SplitMix64::new(SIMHASH_SEED ^ (bit as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for &x in embedding {
            // Gaussian-ish hyperplane component in [-1, 1); sign is all that
            // matters, so a cheap uniform is sufficient for SimHash.
            let h = rng.next_unit();
            dot += x * h;
        }
        if dot >= 0.0 {
            sig |= 1 << bit;
        }
    }
    sig
}

/// Hamming distance between two signatures (0..=64). Lower = more similar.
#[inline]
pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// A minimal SplitMix64 PRNG — deterministic, no dependency, good enough for
/// generating fixed random hyperplanes.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [-1, 1).
    fn next_unit(&mut self) -> f32 {
        let u = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32; // [0,1)
        u * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(v: &mut [f32]) {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            for x in v.iter_mut() {
                *x /= n;
            }
        }
    }

    #[test]
    fn signature_is_deterministic() {
        let v = vec![0.1, -0.3, 0.5, 0.2, -0.9, 0.4];
        assert_eq!(signature(&v), signature(&v));
    }

    #[test]
    fn identical_vectors_have_zero_hamming() {
        let v = vec![0.2, 0.4, -0.6, 0.1];
        assert_eq!(hamming(signature(&v), signature(&v)), 0);
    }

    #[test]
    fn similar_vectors_are_closer_than_dissimilar_ones() {
        let mut base = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut near = vec![0.98, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut far = vec![-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        normalize(&mut base);
        normalize(&mut near);
        normalize(&mut far);
        let d_near = hamming(signature(&base), signature(&near));
        let d_far = hamming(signature(&base), signature(&far));
        assert!(d_near < d_far, "near {d_near} should be < far {d_far}");
    }

    #[test]
    fn empty_embedding_is_safe() {
        assert_eq!(signature(&[]), 0);
    }

    /// Hamming ranking must recover most true cosine neighbours — the
    /// property the two-stage search relies on. We generate random unit
    /// vectors, pick the true top-k by cosine, and check the SimHash
    /// prefilter's larger candidate set contains most of them.
    #[test]
    fn hamming_prefilter_has_high_recall_of_cosine_neighbours() {
        let dim = 64;
        let n = 400;
        let mut rng = SplitMix64::new(42);
        let vecs: Vec<Vec<f32>> = (0..n)
            .map(|_| {
                let mut v: Vec<f32> = (0..dim).map(|_| rng.next_unit()).collect();
                normalize(&mut v);
                v
            })
            .collect();
        let query = &vecs[0];

        let cosine = |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| x * y).sum() };

        // True top-10 by cosine.
        let mut by_cos: Vec<(usize, f32)> =
            (0..n).map(|i| (i, cosine(query, &vecs[i]))).collect();
        by_cos.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let truth: std::collections::HashSet<usize> =
            by_cos.iter().take(10).map(|(i, _)| *i).collect();

        // SimHash candidate set. Hamming distance is ~free (one xor +
        // popcount), so the two-stage search overfetches generously — the
        // exact cosine rerank runs on the candidates, so a wide prefilter
        // costs little and buys recall. Here: top-100 of 400 (the real
        // config caps candidates at max(top_k*8, a few hundred)).
        let qsig = signature(query);
        let mut by_ham: Vec<(usize, u32)> =
            (0..n).map(|i| (i, hamming(qsig, signature(&vecs[i])))).collect();
        by_ham.sort_by_key(|(_, d)| *d);
        let candidates: std::collections::HashSet<usize> =
            by_ham.iter().take(100).map(|(i, _)| *i).collect();

        let recovered = truth.iter().filter(|i| candidates.contains(i)).count();
        // With generous overfetch the prefilter recovers ~all true
        // neighbours even on adversarial near-orthogonal random vectors.
        assert!(
            recovered >= 9,
            "SimHash prefilter recall too low: {recovered}/10 recovered"
        );
    }
}
