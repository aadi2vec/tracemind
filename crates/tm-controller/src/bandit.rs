use std::f64;

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

    fn params_for_arm(arm: u8) -> RetrievalParams {
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
}
