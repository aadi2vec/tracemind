//! Q4.9 — Matryoshka slicing for hierarchical retrieval.
//! BGE-M3 supports Matryoshka representation learning — dense embeddings
//! where the first N dims are a valid lower-dimensional embedding.
//! This gives "free" hierarchical retrieval tiers:
//! - 128d: fast coarse search (mobile, background)
//! - 256d: medium quality
//! - 512d: high quality
//! - 768d: full BGE-M3 dense

pub const MATRYOSHKA_DIMS: &[usize] = &[128, 256, 512, 768];

/// Slice the first `target_dim` dimensions from a 768-dim BGE-M3 embedding
/// and L2-normalize the result.
pub fn slice_and_normalize(embedding: &[f32], target_dim: usize) -> Vec<f32> {
    assert!(
        target_dim <= embedding.len(),
        "target_dim exceeds embedding length"
    );
    let sliced = &embedding[..target_dim];
    let norm: f32 = sliced.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm < 1e-8 {
        sliced.to_vec()
    } else {
        sliced.iter().map(|x| x / norm).collect()
    }
}

/// Select the appropriate Matryoshka tier based on retrieval budget.
/// budget: "fast" → 128d, "medium" → 256d, "high" → 512d, "full" → 768d
pub fn tier_dim(budget: &str) -> usize {
    match budget {
        "fast" => 128,
        "medium" => 256,
        "high" => 512,
        _ => 768,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_is_normalized() {
        // Create a 768-dim embedding with non-unit values
        let embedding: Vec<f32> = (0..768).map(|i| (i as f32) * 0.01 + 0.1).collect();
        let result = slice_and_normalize(&embedding, 256);
        assert_eq!(result.len(), 256);

        // Check L2 norm is ~1.0
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn tier_dim_mapping() {
        assert_eq!(tier_dim("fast"), 128);
        assert_eq!(tier_dim("medium"), 256);
        assert_eq!(tier_dim("high"), 512);
        assert_eq!(tier_dim("full"), 768);
        // Unknown budget falls back to full
        assert_eq!(tier_dim("unknown"), 768);
        assert_eq!(tier_dim(""), 768);
    }

    #[test]
    fn correct_dims_in_matryoshka_dims() {
        assert_eq!(MATRYOSHKA_DIMS, &[128, 256, 512, 768]);
        assert_eq!(MATRYOSHKA_DIMS.len(), 4);
        // Each tier is a valid Matryoshka sub-dim
        for &dim in MATRYOSHKA_DIMS {
            assert!(dim <= 768, "dim {dim} exceeds BGE-M3 max");
        }
        // Tiers are strictly increasing
        for w in MATRYOSHKA_DIMS.windows(2) {
            assert!(w[0] < w[1], "dims not strictly increasing");
        }
    }
}
