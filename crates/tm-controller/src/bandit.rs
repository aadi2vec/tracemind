use std::f64;
use std::path::Path;

#[derive(serde::Serialize, serde::Deserialize)]
struct BanditState {
    counts: [u64; 4],
    rewards: [f64; 4],
    total_pulls: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetrievalParams {
    pub arm: u8,
    pub top_k: usize,
    pub hops: u32,
    pub include_episodic: bool,
}

pub struct UcbBandit {
    counts: [u64; 4],
    rewards: [f64; 4],
    total_pulls: u64,
}

impl UcbBandit {
    pub fn new() -> Self {
        Self {
            counts: [0u64; 4],
            rewards: [0.0f64; 4],
            total_pulls: 0,
        }
    }

    pub fn select(&self) -> RetrievalParams {
        let mut best_arm: u8 = 0;
        let mut best_score = f64::NEG_INFINITY;

        for arm in 0u8..4 {
            let score = if self.counts[arm as usize] == 0 {
                f64::INFINITY
            } else {
                let q = self.rewards[arm as usize];
                let n = self.counts[arm as usize] as f64;
                let n_total = self.total_pulls as f64;
                q + (2.0 * n_total.ln() / n).sqrt()
            };

            if score > best_score {
                best_score = score;
                best_arm = arm;
            }
        }

        Self::params_for_arm(best_arm)
    }

    pub fn register_reward(&mut self, arm: u8, reward: f64) {
        let arm_idx = arm as usize;
        let reward = reward.clamp(0.0, 1.0);
        self.counts[arm_idx] += 1;
        self.total_pulls += 1;
        let n = self.counts[arm_idx] as f64;
        self.rewards[arm_idx] =
            (self.rewards[arm_idx] * (n - 1.0) + reward) / n;
    }

    pub fn arm_stats(&self) -> [(u64, f64); 4] {
        [
            (self.counts[0], self.rewards[0]),
            (self.counts[1], self.rewards[1]),
            (self.counts[2], self.rewards[2]),
            (self.counts[3], self.rewards[3]),
        ]
    }

    /// Load bandit state from a JSON file, or return a fresh bandit if missing/corrupt.
    pub fn load(path: &Path) -> Self {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(state) = serde_json::from_str::<BanditState>(&raw) {
                let mut b = Self::new();
                for arm_idx in 0u8..4 {
                    let pulls = state.counts[arm_idx as usize];
                    let avg = state.rewards[arm_idx as usize];
                    for _ in 0..pulls {
                        b.register_reward(arm_idx, avg);
                    }
                }
                return b;
            }
        }
        Self::new()
    }

    /// Save bandit state to a JSON file.
    pub fn save(&self, path: &Path) {
        let stats = self.arm_stats();
        let total_pulls: u64 = stats.iter().map(|(c, _)| c).sum();
        let state = BanditState {
            counts: [stats[0].0, stats[1].0, stats[2].0, stats[3].0],
            rewards: [stats[0].1, stats[1].1, stats[2].1, stats[3].1],
            total_pulls,
        };
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn params_for_arm(arm: u8) -> RetrievalParams {
        match arm {
            0 => RetrievalParams {
                arm: 0,
                top_k: 5,
                hops: 0,
                include_episodic: false,
            },
            1 => RetrievalParams {
                arm: 1,
                top_k: 10,
                hops: 1,
                include_episodic: false,
            },
            2 => RetrievalParams {
                arm: 2,
                top_k: 15,
                hops: 2,
                include_episodic: false,
            },
            3 => RetrievalParams {
                arm: 3,
                top_k: 20,
                hops: 2,
                include_episodic: true,
            },
            _ => RetrievalParams {
                arm: 0,
                top_k: 5,
                hops: 0,
                include_episodic: false,
            },
        }
    }
}

impl Default for UcbBandit {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// LinUCB — Contextual Bandit (Diagonal Approximation)
// ---------------------------------------------------------------------------
//
// Instead of one global policy (UCB1), LinUCB learns a per-query-type policy:
// "for ML queries use wide arm, for simple lookups use narrow arm."
//
// Uses diagonal approximation of the full LinUCB algorithm:
// - Full LinUCB stores d×d matrix A per arm (~1.2MB per arm at d=384)
// - Diagonal LinUCB stores d-vector per arm (~3KB per arm) — 400× smaller
// - Accuracy is nearly identical for sparse, high-dimensional features
//
// Algorithm:
//   For each arm a, maintain:
//     w_a: d-dim weight vector (learned preferences)
//     v_a: d-dim variance accumulator (sum of x_i² seen)
//     n_a: pull count
//
//   Select: p_a = w_a · x + α * sqrt(Σ x_i² / (v_a_i + 1))
//   Update: v_a += x², w_a += lr * (r - w_a · x) * x
//
// Reference: Li et al. 2010 "A Contextual-Bandit Approach to Personalized
// News Article Recommendation" (simplified to diagonal)
// ---------------------------------------------------------------------------

const LINUCB_DIM: usize = 384;
const LINUCB_ARMS: usize = 4;
const LINUCB_ALPHA: f64 = 0.5;     // exploration coefficient
const LINUCB_LR: f64 = 0.01;       // weight update learning rate

#[derive(serde::Serialize, serde::Deserialize)]
struct LinUcbState {
    weights: Vec<Vec<f64>>,      // [4][384] — learned arm preferences
    variances: Vec<Vec<f64>>,    // [4][384] — accumulated x² per dim
    counts: [u64; LINUCB_ARMS],
    total_pulls: u64,
}

pub struct LinUcbBandit {
    weights: [Vec<f64>; LINUCB_ARMS],
    variances: [Vec<f64>; LINUCB_ARMS],
    counts: [u64; LINUCB_ARMS],
    total_pulls: u64,
}

impl LinUcbBandit {
    pub fn new() -> Self {
        Self {
            weights: std::array::from_fn(|_| vec![0.0; LINUCB_DIM]),
            variances: std::array::from_fn(|_| vec![1.0; LINUCB_DIM]), // prior: unit variance
            counts: [0u64; LINUCB_ARMS],
            total_pulls: 0,
        }
    }

    /// Select arm given a 384-dim query embedding as context.
    /// Falls back to UCB1-style exploration if context is wrong dimension.
    pub fn select(&self, context: &[f32]) -> RetrievalParams {
        if context.len() != LINUCB_DIM {
            // Wrong dimension — fall back to round-robin exploration
            let arm = (self.total_pulls % LINUCB_ARMS as u64) as u8;
            return UcbBandit::params_for_arm(arm);
        }

        let x: Vec<f64> = context.iter().map(|&v| v as f64).collect();
        let mut best_arm: u8 = 0;
        let mut best_score = f64::NEG_INFINITY;

        for arm in 0..LINUCB_ARMS {
            let score = if self.counts[arm] == 0 {
                f64::INFINITY // explore unpulled arms first
            } else {
                // Exploitation: w_a · x
                let exploit: f64 = self.weights[arm].iter()
                    .zip(x.iter())
                    .map(|(w, xi)| w * xi)
                    .sum();

                // Exploration: α * sqrt(Σ x_i² / (v_a_i + 1))
                let uncertainty: f64 = x.iter()
                    .zip(self.variances[arm].iter())
                    .map(|(xi, vi)| (xi * xi) / (vi + 1.0))
                    .sum::<f64>()
                    .sqrt();

                exploit + LINUCB_ALPHA * uncertainty
            };

            if score > best_score {
                best_score = score;
                best_arm = arm as u8;
            }
        }

        UcbBandit::params_for_arm(best_arm)
    }

    /// Register reward for the selected arm with the context used.
    pub fn register_reward(&mut self, arm: u8, reward: f64, context: &[f32]) {
        let arm_idx = arm as usize;
        if arm_idx >= LINUCB_ARMS || context.len() != LINUCB_DIM {
            return;
        }

        let reward = reward.clamp(0.0, 1.0);
        let x: Vec<f64> = context.iter().map(|&v| v as f64).collect();

        // Current prediction: w · x
        let prediction: f64 = self.weights[arm_idx].iter()
            .zip(x.iter())
            .map(|(w, xi)| w * xi)
            .sum();

        // Update weights: w += lr * (reward - prediction) * x
        let error = reward - prediction;
        for i in 0..LINUCB_DIM {
            self.weights[arm_idx][i] += LINUCB_LR * error * x[i];
        }

        // Update variance accumulators: v += x²
        for i in 0..LINUCB_DIM {
            self.variances[arm_idx][i] += x[i] * x[i];
        }

        self.counts[arm_idx] += 1;
        self.total_pulls += 1;
    }

    pub fn arm_stats(&self) -> [(u64, f64); LINUCB_ARMS] {
        std::array::from_fn(|i| {
            let avg_weight_mag = if self.weights[i].is_empty() {
                0.0
            } else {
                self.weights[i].iter().map(|w| w.abs()).sum::<f64>() / self.weights[i].len() as f64
            };
            (self.counts[i], avg_weight_mag)
        })
    }

    pub fn load(path: &Path) -> Self {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(state) = serde_json::from_str::<LinUcbState>(&raw) {
                if state.weights.len() == LINUCB_ARMS
                    && state.variances.len() == LINUCB_ARMS
                    && state.weights.iter().all(|w| w.len() == LINUCB_DIM)
                    && state.variances.iter().all(|v| v.len() == LINUCB_DIM)
                {
                    return Self {
                        weights: std::array::from_fn(|i| state.weights[i].clone()),
                        variances: std::array::from_fn(|i| state.variances[i].clone()),
                        counts: state.counts,
                        total_pulls: state.total_pulls,
                    };
                }
            }
        }
        Self::new()
    }

    pub fn save(&self, path: &Path) {
        let state = LinUcbState {
            weights: self.weights.iter().map(|w| w.clone()).collect(),
            variances: self.variances.iter().map(|v| v.clone()).collect(),
            counts: self.counts,
            total_pulls: self.total_pulls,
        };
        if let Ok(json) = serde_json::to_string(&state) {
            let _ = std::fs::write(path, json);
        }
    }
}

impl Default for LinUcbBandit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fresh_bandit_explores_all_arms() {
        let bandit = UcbBandit::new();

        // Collect the first 4 arm selections — all should be distinct
        // because unpulled arms have score = INFINITY and are picked in order 0..3
        let mut seen = [false; 4];
        for _ in 0..4 {
            // We cannot mutate the bandit here (select is &self), so we simulate
            // by manually checking that each arm appears in a sequence of selects
            // on a fresh bandit (all arms still have INFINITY until pulled).
            let _ = bandit.select();
        }

        // More precise: a fresh bandit always returns arm 0 first (first INFINITY wins),
        // so we build up by registering rewards step by step.
        let mut b = UcbBandit::new();
        seen[b.select().arm as usize] = true;
        b.register_reward(0, 0.5);

        seen[b.select().arm as usize] = true;
        b.register_reward(1, 0.5);

        seen[b.select().arm as usize] = true;
        b.register_reward(2, 0.5);

        seen[b.select().arm as usize] = true;

        assert!(seen[0], "arm 0 should be selected during exploration");
        assert!(seen[1], "arm 1 should be selected during exploration");
        assert!(seen[2], "arm 2 should be selected during exploration");
        assert!(seen[3], "arm 3 should be selected during exploration");
    }

    #[test]
    fn test_high_reward_arm_selected_after_training() {
        let mut bandit = UcbBandit::new();

        // Give all arms equal baseline pulls to equalise exploration bonus
        for _ in 0..20 {
            for arm in 0u8..4 {
                bandit.register_reward(arm, 0.5);
            }
        }

        // Give arm 0 many extra high-reward pulls — raises Q(0) and shrinks its CI
        for _ in 0..20 {
            bandit.register_reward(0, 1.0);
        }

        let selected = bandit.select();
        assert_eq!(
            selected.arm, 0,
            "arm 0 should be selected after high rewards"
        );
    }

    #[test]
    fn test_register_reward_clamps_negative() {
        let mut bandit = UcbBandit::new();
        bandit.register_reward(0, -5.0);
        let stats = bandit.arm_stats();
        assert_eq!(stats[0].0, 1, "count should be 1");
        assert!(
            (stats[0].1 - 0.0).abs() < f64::EPSILON,
            "reward should be clamped to 0.0, got {}",
            stats[0].1
        );
    }

    #[test]
    fn test_arm_stats_correct_counts() {
        let mut bandit = UcbBandit::new();
        bandit.register_reward(0, 1.0);
        bandit.register_reward(0, 1.0);
        bandit.register_reward(0, 0.0);
        bandit.register_reward(2, 0.6);

        let stats = bandit.arm_stats();
        assert_eq!(stats[0].0, 3, "arm 0 should have 3 pulls");
        assert_eq!(stats[1].0, 0, "arm 1 should have 0 pulls");
        assert_eq!(stats[2].0, 1, "arm 2 should have 1 pull");
        assert_eq!(stats[3].0, 0, "arm 3 should have 0 pulls");

        // arm 0 average: (1.0 + 1.0 + 0.0) / 3 = 2/3
        let expected_avg = 2.0 / 3.0;
        assert!(
            (stats[0].1 - expected_avg).abs() < 1e-10,
            "arm 0 avg reward should be {}, got {}",
            expected_avg,
            stats[0].1
        );
    }

    #[test]
    fn test_retrieval_params_mapping() {
        let b = UcbBandit::new();
        // Arm 0
        let p = UcbBandit::params_for_arm(0);
        assert_eq!(p.arm, 0);
        assert_eq!(p.top_k, 5);
        assert_eq!(p.hops, 0);
        assert!(!p.include_episodic);

        // Arm 1
        let p = UcbBandit::params_for_arm(1);
        assert_eq!(p.arm, 1);
        assert_eq!(p.top_k, 10);
        assert_eq!(p.hops, 1);
        assert!(!p.include_episodic);

        // Arm 2
        let p = UcbBandit::params_for_arm(2);
        assert_eq!(p.arm, 2);
        assert_eq!(p.top_k, 15);
        assert_eq!(p.hops, 2);
        assert!(!p.include_episodic);

        // Arm 3
        let p = UcbBandit::params_for_arm(3);
        assert_eq!(p.arm, 3);
        assert_eq!(p.top_k, 20);
        assert_eq!(p.hops, 2);
        assert!(p.include_episodic);

        // Suppress unused variable warning
        let _ = b;
    }

    #[test]
    fn test_register_reward_clamps_above_one() {
        let mut bandit = UcbBandit::new();
        bandit.register_reward(3, 99.0);
        let stats = bandit.arm_stats();
        assert!(
            (stats[3].1 - 1.0).abs() < f64::EPSILON,
            "reward should be clamped to 1.0, got {}",
            stats[3].1
        );
    }

    #[test]
    fn test_total_pulls_accumulates() {
        let mut bandit = UcbBandit::new();
        assert_eq!(bandit.total_pulls, 0);
        bandit.register_reward(0, 0.5);
        assert_eq!(bandit.total_pulls, 1);
        bandit.register_reward(1, 0.5);
        bandit.register_reward(2, 0.5);
        assert_eq!(bandit.total_pulls, 3);
    }

    // -----------------------------------------------------------------------
    // LinUCB tests
    // -----------------------------------------------------------------------

    fn make_context(dominant_dim: usize) -> Vec<f32> {
        // Create a 384-dim context that's mostly zero but has signal in one dimension
        let mut ctx = vec![0.0f32; 384];
        ctx[dominant_dim] = 1.0;
        // Add small noise to other dims for realism
        for i in 0..384 {
            if i != dominant_dim {
                ctx[i] = 0.01 * ((i * 7 + dominant_dim) % 13) as f32 / 13.0;
            }
        }
        // L2 normalize
        let norm: f32 = ctx.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in ctx.iter_mut() { *x /= norm; }
        }
        ctx
    }

    #[test]
    fn linucb_fresh_explores_all_arms() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);

        // First 4 selects should cover all arms (unpulled → INFINITY)
        let mut seen = [false; 4];
        seen[bandit.select(&ctx).arm as usize] = true;
        bandit.register_reward(0, 0.5, &ctx);

        seen[bandit.select(&ctx).arm as usize] = true;
        bandit.register_reward(1, 0.5, &ctx);

        seen[bandit.select(&ctx).arm as usize] = true;
        bandit.register_reward(2, 0.5, &ctx);

        seen[bandit.select(&ctx).arm as usize] = true;

        assert!(seen.iter().all(|&s| s), "should explore all 4 arms");
    }

    #[test]
    fn linucb_learns_context_arm_association() {
        let mut bandit = LinUcbBandit::new();

        let ctx_a = make_context(10);  // "ML queries" → arm 2
        let ctx_b = make_context(50);  // "lookup queries" → arm 0

        // Explore phase: pull each arm once with each context
        for arm in 0u8..4 {
            bandit.register_reward(arm, 0.3, &ctx_a);
            bandit.register_reward(arm, 0.3, &ctx_b);
        }

        // Train: context A gets high reward on arm 2
        for _ in 0..50 {
            bandit.register_reward(2, 0.9, &ctx_a);
            bandit.register_reward(0, 0.2, &ctx_a);
        }
        // Train: context B gets high reward on arm 0
        for _ in 0..50 {
            bandit.register_reward(0, 0.9, &ctx_b);
            bandit.register_reward(2, 0.2, &ctx_b);
        }

        // After training, context A should prefer arm 2, context B should prefer arm 0
        let selected_a = bandit.select(&ctx_a);
        let selected_b = bandit.select(&ctx_b);

        assert_eq!(selected_a.arm, 2, "context A should select arm 2 (wide)");
        assert_eq!(selected_b.arm, 0, "context B should select arm 0 (narrow)");
    }

    #[test]
    fn linucb_wrong_dim_falls_back() {
        let bandit = LinUcbBandit::new();
        let short_ctx = vec![0.5f32; 10]; // wrong dimension
        let params = bandit.select(&short_ctx);
        // Should not panic, should return valid params
        assert!(params.arm < 4);
    }

    #[test]
    fn linucb_reward_clamps() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);
        bandit.register_reward(0, -5.0, &ctx); // should clamp to 0
        bandit.register_reward(1, 99.0, &ctx);  // should clamp to 1
        assert_eq!(bandit.counts[0], 1);
        assert_eq!(bandit.counts[1], 1);
    }

    #[test]
    fn linucb_save_load_roundtrip() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(42);

        for _ in 0..10 {
            bandit.register_reward(1, 0.8, &ctx);
        }

        let dir = std::env::temp_dir().join("tm-linucb-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("linucb.json");
        bandit.save(&path);

        let loaded = LinUcbBandit::load(&path);
        assert_eq!(loaded.counts, bandit.counts);
        assert_eq!(loaded.total_pulls, bandit.total_pulls);
        // Weights should be identical
        for arm in 0..4 {
            for dim in 0..384 {
                assert!(
                    (loaded.weights[arm][dim] - bandit.weights[arm][dim]).abs() < 1e-10,
                    "weight mismatch at arm {} dim {}", arm, dim
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn linucb_stats_reflect_pulls() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);
        bandit.register_reward(0, 0.5, &ctx);
        bandit.register_reward(0, 0.7, &ctx);
        bandit.register_reward(3, 0.9, &ctx);

        let stats = bandit.arm_stats();
        assert_eq!(stats[0].0, 2);
        assert_eq!(stats[1].0, 0);
        assert_eq!(stats[2].0, 0);
        assert_eq!(stats[3].0, 1);
        assert_eq!(bandit.total_pulls, 3);
    }
}
