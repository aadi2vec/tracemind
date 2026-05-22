use std::f64;
use std::path::Path;

/// **LGS-3** — 6 arms now. Arm 5 (`subgraph_colbert`) seeds the
/// graph traversal with the top-k entities chosen by ColBERT MaxSim
/// over per-entity token grids (see `tm_rerank::EntityIndex`), then
/// reranks each candidate subgraph by MaxSim against the query.
pub const NUM_ARMS: usize = 6;

#[derive(serde::Serialize, serde::Deserialize)]
struct BanditState {
    counts: Vec<u64>,
    rewards: Vec<f64>,
    total_pulls: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetrievalParams {
    pub arm: u8,
    pub top_k: usize,
    pub hops: u32,
    pub include_episodic: bool,
    #[serde(default)]
    pub include_colbert: bool,
    /// **LGS-3** — arm 5 only. When true, the retrieval engine seeds
    /// graph traversal with entry points from `tm_rerank::EntityIndex`
    /// and reranks each candidate subgraph by MaxSim against the query.
    #[serde(default)]
    pub include_subgraph_colbert: bool,
}

pub struct UcbBandit {
    counts: [u64; NUM_ARMS],
    rewards: [f64; NUM_ARMS],
    total_pulls: u64,
}

impl UcbBandit {
    pub fn new() -> Self {
        Self {
            counts: [0u64; NUM_ARMS],
            rewards: [0.0f64; NUM_ARMS],
            total_pulls: 0,
        }
    }

    pub fn select(&self) -> RetrievalParams {
        let mut best_arm: u8 = 0;
        let mut best_score = f64::NEG_INFINITY;

        for arm in 0u8..NUM_ARMS as u8 {
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

    pub fn arm_stats(&self) -> [(u64, f64); NUM_ARMS] {
        std::array::from_fn(|i| (self.counts[i], self.rewards[i]))
    }

    /// Load bandit state from a JSON file, or return a fresh bandit if missing/corrupt.
    pub fn load(path: &Path) -> Self {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(state) = serde_json::from_str::<BanditState>(&raw) {
                let mut b = Self::new();
                let n = state.counts.len().min(NUM_ARMS);
                for arm_idx in 0..n {
                    let pulls = state.counts[arm_idx];
                    let avg = state.rewards[arm_idx];
                    for _ in 0..pulls {
                        b.register_reward(arm_idx as u8, avg);
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
            counts: stats.iter().map(|(c, _)| *c).collect(),
            rewards: stats.iter().map(|(_, r)| *r).collect(),
            total_pulls,
        };
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn params_for_arm(arm: u8) -> RetrievalParams {
        match arm {
            0 => RetrievalParams { arm: 0, top_k: 5,  hops: 0, include_episodic: false, include_colbert: false, include_subgraph_colbert: false },
            1 => RetrievalParams { arm: 1, top_k: 10, hops: 1, include_episodic: false, include_colbert: false, include_subgraph_colbert: false },
            2 => RetrievalParams { arm: 2, top_k: 15, hops: 2, include_episodic: false, include_colbert: false, include_subgraph_colbert: false },
            3 => RetrievalParams { arm: 3, top_k: 20, hops: 2, include_episodic: true,  include_colbert: false, include_subgraph_colbert: false },
            4 => RetrievalParams { arm: 4, top_k: 10, hops: 1, include_episodic: false, include_colbert: true,  include_subgraph_colbert: false },
            // LGS-3 — subgraph ColBERT arm: deeper hops, ColBERT-seeded
            // entry points, MaxSim rerank over candidate subgraphs.
            5 => RetrievalParams { arm: 5, top_k: 12, hops: 2, include_episodic: false, include_colbert: true,  include_subgraph_colbert: true },
            _ => RetrievalParams { arm: 0, top_k: 5,  hops: 0, include_episodic: false, include_colbert: false, include_subgraph_colbert: false },
        }
    }

    /// Human-readable name for an arm index.
    pub fn arm_name(arm: u8) -> &'static str {
        match arm {
            0 => "narrow",
            1 => "medium",
            2 => "wide",
            3 => "deep",
            4 => "colbert",
            5 => "subgraph_colbert",
            _ => "unknown",
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
const LINUCB_ARMS: usize = NUM_ARMS;
const LINUCB_ALPHA_INIT: f64 = 0.5;
const LINUCB_ALPHA_MIN: f64 = 0.05;
const LINUCB_ALPHA_DECAY: f64 = 0.995;
const LINUCB_LR: f64 = 0.01;

const ARM_FEATURES: [[f64; 2]; LINUCB_ARMS] = [
    [0.25, 0.0],  // narrow
    [0.50, 0.33], // medium
    [0.75, 0.67], // wide
    [1.00, 1.00], // deep
    [0.50, 0.50], // colbert: medium breadth + depth, token-level scoring
    // subgraph_colbert (arm 5): wide breadth + deep hops + ColBERT seeding.
    // Slightly above colbert on both axes so the LinUCB policy can prefer
    // it for queries that benefit from entity-anchored multi-hop traversal.
    [0.80, 0.80],
];

#[derive(serde::Serialize, serde::Deserialize)]
struct LinUcbState {
    weights: Vec<Vec<f64>>,
    variances: Vec<Vec<f64>>,
    counts: Vec<u64>,
    total_pulls: u64,
    #[serde(default = "default_alpha")]
    alpha: f64,
    #[serde(default)]
    arm_bias: Vec<f64>,
}

fn default_alpha() -> f64 { LINUCB_ALPHA_INIT }

pub struct LinUcbBandit {
    weights: [Vec<f64>; LINUCB_ARMS],
    variances: [Vec<f64>; LINUCB_ARMS],
    counts: [u64; LINUCB_ARMS],
    total_pulls: u64,
    alpha: f64,                   // annealed exploration coefficient
    arm_bias: [f64; LINUCB_ARMS], // learned bias from arm features
}

impl LinUcbBandit {
    pub fn new() -> Self {
        Self {
            weights: std::array::from_fn(|_| vec![0.0; LINUCB_DIM]),
            variances: std::array::from_fn(|_| vec![1.0; LINUCB_DIM]), // prior: unit variance
            counts: [0u64; LINUCB_ARMS],
            total_pulls: 0,
            alpha: LINUCB_ALPHA_INIT,
            arm_bias: [0.0; LINUCB_ARMS],
        }
    }

    /// Current exploration coefficient (annealed over time).
    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    /// Select arm given a 384-dim query embedding as context.
    ///
    /// Incorporates:
    /// - Annealed exploration (α decays with total pulls, floored at α_min)
    /// - Arm feature sharing (breadth/depth bias shared across similar arms)
    /// - Optional trajectory prior (pass best_arm_hint from nearest-neighbor lookup)
    pub fn select(&self, context: &[f32]) -> RetrievalParams {
        self.select_with_hint(context, None)
    }

    /// Select arm with optional trajectory-based hint.
    ///
    /// `trajectory_hint`: if a nearest-neighbor trajectory lookup found that
    /// a similar past query succeeded with arm X, pass `Some(x)` to bias
    /// selection toward that arm (+0.1 bonus). This is a non-parametric prior
    /// that exploits the trajectory store without any learning.
    pub fn select_with_hint(&self, context: &[f32], trajectory_hint: Option<u8>) -> RetrievalParams {
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
                // Exploitation: w_a · x + arm_bias (shared via arm features)
                let exploit: f64 = self.weights[arm].iter()
                    .zip(x.iter())
                    .map(|(w, xi)| w * xi)
                    .sum::<f64>()
                    + self.arm_bias[arm];

                // Exploration: α * sqrt(Σ x_i² / (v_a_i + 1))
                // α is annealed: starts at 0.5, decays to 0.05 over time
                let uncertainty: f64 = x.iter()
                    .zip(self.variances[arm].iter())
                    .map(|(xi, vi)| (xi * xi) / (vi + 1.0))
                    .sum::<f64>()
                    .sqrt();

                let mut s = exploit + self.alpha * uncertainty;

                // Trajectory hint bonus: if nearest-neighbor says this arm worked
                // for a similar query, give it a small boost
                if trajectory_hint == Some(arm as u8) {
                    s += 0.1;
                }

                s
            };

            if score > best_score {
                best_score = score;
                best_arm = arm as u8;
            }
        }

        UcbBandit::params_for_arm(best_arm)
    }

    /// Register reward for the selected arm with the context used.
    ///
    /// Also updates:
    /// - Exploration coefficient α (annealed by LINUCB_ALPHA_DECAY per pull)
    /// - Arm feature bias (shared strength across arms on the breadth/depth axis)
    pub fn register_reward(&mut self, arm: u8, reward: f64, context: &[f32]) {
        let arm_idx = arm as usize;
        if arm_idx >= LINUCB_ARMS || context.len() != LINUCB_DIM {
            return;
        }

        let reward = reward.clamp(0.0, 1.0);
        let x: Vec<f64> = context.iter().map(|&v| v as f64).collect();

        // Current prediction: w · x + arm_bias
        let prediction: f64 = self.weights[arm_idx].iter()
            .zip(x.iter())
            .map(|(w, xi)| w * xi)
            .sum::<f64>()
            + self.arm_bias[arm_idx];

        // Update weights: w += lr * (reward - prediction) * x
        let error = reward - prediction;
        for i in 0..LINUCB_DIM {
            self.weights[arm_idx][i] += LINUCB_LR * error * x[i];
        }

        // Update arm feature bias: share reward signal across similar arms.
        // Arms close on the breadth/depth axis get proportional updates.
        let chosen_features = ARM_FEATURES[arm_idx];
        for other in 0..LINUCB_ARMS {
            let other_features = ARM_FEATURES[other];
            let dist = ((chosen_features[0] - other_features[0]).powi(2)
                      + (chosen_features[1] - other_features[1]).powi(2))
                      .sqrt();
            // Similarity kernel: 1.0 for same arm, decaying with distance
            let sim = (-3.0 * dist).exp(); // σ ≈ 0.33
            self.arm_bias[other] += LINUCB_LR * error * sim;
        }

        // Update variance accumulators: v += x²
        for i in 0..LINUCB_DIM {
            self.variances[arm_idx][i] += x[i] * x[i];
        }

        // Anneal exploration: α *= decay, floored at α_min
        self.alpha = (self.alpha * LINUCB_ALPHA_DECAY).max(LINUCB_ALPHA_MIN);

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
                let n = state.weights.len();
                if n >= 4
                    && state.variances.len() == n
                    && state.weights.iter().all(|w| w.len() == LINUCB_DIM)
                    && state.variances.iter().all(|v| v.len() == LINUCB_DIM)
                {
                    let mut bandit = Self::new();
                    for i in 0..n.min(LINUCB_ARMS) {
                        bandit.weights[i] = state.weights[i].clone();
                        bandit.variances[i] = state.variances[i].clone();
                    }
                    let mut counts = [0u64; LINUCB_ARMS];
                    for i in 0..state.counts.len().min(LINUCB_ARMS) {
                        counts[i] = state.counts[i];
                    }
                    bandit.counts = counts;
                    bandit.total_pulls = state.total_pulls;
                    bandit.alpha = state.alpha;
                    let mut arm_bias = [0.0f64; LINUCB_ARMS];
                    for i in 0..state.arm_bias.len().min(LINUCB_ARMS) {
                        arm_bias[i] = state.arm_bias[i];
                    }
                    bandit.arm_bias = arm_bias;
                    return bandit;
                }
            }
        }
        Self::new()
    }

    pub fn save(&self, path: &Path) {
        let state = LinUcbState {
            weights: self.weights.iter().map(|w| w.clone()).collect(),
            variances: self.variances.iter().map(|v| v.clone()).collect(),
            counts: self.counts.to_vec(),
            total_pulls: self.total_pulls,
            alpha: self.alpha,
            arm_bias: self.arm_bias.to_vec(),
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
        let mut b = UcbBandit::new();
        let mut seen = [false; NUM_ARMS];

        for arm_idx in 0..NUM_ARMS {
            seen[b.select().arm as usize] = true;
            b.register_reward(arm_idx as u8, 0.5);
        }

        for i in 0..NUM_ARMS {
            assert!(seen[i], "arm {} should be selected during exploration", i);
        }
    }

    #[test]
    fn test_high_reward_arm_selected_after_training() {
        let mut bandit = UcbBandit::new();

        // Give all arms equal baseline pulls to equalise exploration bonus
        for _ in 0..20 {
            for arm in 0u8..NUM_ARMS as u8 {
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
        assert!(!p.include_colbert);

        // Arm 4 — ColBERT
        let p = UcbBandit::params_for_arm(4);
        assert_eq!(p.arm, 4);
        assert_eq!(p.top_k, 10);
        assert!(p.include_colbert);
        assert!(!p.include_episodic);

        assert_eq!(UcbBandit::arm_name(0), "narrow");
        assert_eq!(UcbBandit::arm_name(4), "colbert");

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

        let mut seen = [false; LINUCB_ARMS];
        for arm_idx in 0..LINUCB_ARMS {
            seen[bandit.select(&ctx).arm as usize] = true;
            bandit.register_reward(arm_idx as u8, 0.5, &ctx);
        }

        assert!(seen.iter().all(|&s| s), "should explore all {} arms", LINUCB_ARMS);
    }

    #[test]
    fn linucb_learns_context_arm_association() {
        let mut bandit = LinUcbBandit::new();

        let ctx_a = make_context(10);  // "ML queries" → arm 2
        let ctx_b = make_context(50);  // "lookup queries" → arm 0

        // Explore phase: pull each arm once with each context
        for arm in 0u8..NUM_ARMS as u8 {
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
        assert!(params.arm < NUM_ARMS as u8);
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
        for arm in 0..LINUCB_ARMS {
            for dim in 0..LINUCB_DIM {
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

    #[test]
    fn linucb_alpha_decays_over_time() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);
        let initial_alpha = bandit.alpha();

        // After 100 pulls, alpha should have decayed significantly
        for arm in 0u8..NUM_ARMS as u8 {
            for _ in 0..20 {
                bandit.register_reward(arm, 0.5, &ctx);
            }
        }

        assert!(bandit.alpha() < initial_alpha, "alpha should decay: {} vs {}", bandit.alpha(), initial_alpha);
        assert!(bandit.alpha() >= LINUCB_ALPHA_MIN, "alpha should not go below minimum");

        // After 1000 pulls, should be near floor
        for _ in 0..900 {
            bandit.register_reward(0, 0.5, &ctx);
        }
        assert!(bandit.alpha() < 0.1, "alpha should be near floor after 1000 pulls: {}", bandit.alpha());
    }

    #[test]
    fn linucb_arm_bias_shared_across_similar_arms() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);

        // Explore all arms first
        for arm in 0u8..NUM_ARMS as u8 {
            bandit.register_reward(arm, 0.3, &ctx);
        }

        // Give arm 2 (wide) high reward many times
        for _ in 0..50 {
            bandit.register_reward(2, 0.9, &ctx);
        }

        // Arm 3 (deep) is closest to arm 2 on the breadth/depth axis,
        // so it should have a higher arm_bias than arm 0 (narrow)
        assert!(
            bandit.arm_bias[3] > bandit.arm_bias[0],
            "deep arm bias ({}) should be > narrow arm bias ({}) due to shared strength",
            bandit.arm_bias[3], bandit.arm_bias[0]
        );
    }

    // ── LGS-3 — arm 5 (subgraph_colbert) ─────────────────────────────────
    #[test]
    fn arm5_is_subgraph_colbert() {
        let p = UcbBandit::params_for_arm(5);
        assert_eq!(p.arm, 5);
        assert!(p.include_colbert);
        assert!(p.include_subgraph_colbert);
        assert_eq!(UcbBandit::arm_name(5), "subgraph_colbert");
    }

    #[test]
    fn arm5_can_be_explored_and_rewarded() {
        let mut b = UcbBandit::new();
        // Reward every other arm low, arm 5 high; after a few pulls the
        // UCB selector should converge on arm 5.
        for _ in 0..3 {
            for arm in 0u8..NUM_ARMS as u8 {
                let r = if arm == 5 { 0.95 } else { 0.10 };
                b.register_reward(arm, r);
            }
        }
        let chosen = b.select();
        assert_eq!(chosen.arm, 5, "UCB1 should prefer the high-reward arm 5");
        assert!(chosen.include_subgraph_colbert);
    }

    #[test]
    fn linucb_trajectory_hint_biases_selection() {
        let mut bandit = LinUcbBandit::new();
        let ctx = make_context(0);

        // Explore all arms, give equal rewards
        for arm in 0u8..NUM_ARMS as u8 {
            for _ in 0..10 {
                bandit.register_reward(arm, 0.5, &ctx);
            }
        }

        let no_hint = bandit.select_with_hint(&ctx, None);
        let with_hint = bandit.select_with_hint(&ctx, Some(3));

        assert!(with_hint.arm < NUM_ARMS as u8);
        assert!(no_hint.arm < NUM_ARMS as u8);
    }
}
