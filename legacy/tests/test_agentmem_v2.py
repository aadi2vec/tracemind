"""Tests for Phase B (ClusterStore), Phase C (AgentMemController), Phase D (LearningLoop)."""
import unittest
from unittest.mock import MagicMock, patch
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))


# ── Phase B: ClusterStore ──────────────────────────────────────────────────

class TestClusterStore(unittest.TestCase):
    def setUp(self):
        from src.memory.cluster_store import ClusterStore
        self.db_path = "test_v2_cluster_store.db"
        if os.path.exists(self.db_path):
            os.remove(self.db_path)
        self.cs = ClusterStore(embedding_dim=4, min_cluster_size=2, db_path=self.db_path)

    def tearDown(self):
        if hasattr(self, "db_path") and os.path.exists(self.db_path):
            os.remove(self.db_path)

    def test_add_and_expand(self):
        self.cs.add_entity("TSLA", [1.0, 0.0, 0.0, 0.0])
        self.cs.add_entity("AMZN", [1.0, 0.1, 0.0, 0.0])
        self.cs.add_entity("AAPL", [0.0, 0.0, 1.0, 0.0])
        self.cs.add_entity("GOOG", [0.0, 0.0, 1.1, 0.0])
        n = self.cs.fit()
        self.assertGreaterEqual(n, 0)  # At least no error
        results = self.cs.expand_query([1.0, 0.0, 0.0, 0.0], top_k=5)
        self.assertIsInstance(results, list)

    def test_len(self):
        self.cs.add_entity("X", [1.0, 0.0, 0.0, 0.0])
        self.assertEqual(len(self.cs), 1)

    def test_update_entity(self):
        self.cs.add_entity("X", [1.0, 0.0, 0.0, 0.0])
        self.cs.add_entity("X", [0.5, 0.5, 0.0, 0.0])  # update
        self.assertEqual(len(self.cs), 1)


# ── Phase C: AgentMemController ───────────────────────────────────────────

class TestAgentMemController(unittest.TestCase):
    def setUp(self):
        from src.agent.agentmem_controller import AgentMemController
        self.ctrl = AgentMemController(confidence_threshold=0.5, epsilon=0.0)

    def test_should_store_high_confidence(self):
        self.assertTrue(self.ctrl.should_store(0.9))

    def test_should_defer_low_confidence(self):
        self.assertFalse(self.ctrl.should_store(0.1))

    def test_retrieval_params_returns_depth_breadth(self):
        params = self.ctrl.get_retrieval_params()
        self.assertIn("depth", params)
        self.assertIn("breadth", params)

    def test_bandit_ucb_selects_unvisited_arm(self):
        # With epsilon=0, UCB should always prefer unpulled arms first
        for _ in range(4):
            self.ctrl.get_retrieval_params()
        # After pulling all arms once, Q-values exist for all
        summary = self.ctrl.bandit_summary()
        self.assertEqual(len(summary), 4)

    def test_register_reward(self):
        self.ctrl.get_retrieval_params()
        self.ctrl.register_reward(1.0)
        summary = self.ctrl.bandit_summary()
        pulled = [a for a in summary if a["pulls"] > 0]
        self.assertTrue(any(a["q_value"] > 0 for a in pulled))


# ── Phase D: LearningLoop ─────────────────────────────────────────────────

class TestLearningLoop(unittest.TestCase):
    def setUp(self):
        from src.agent.agentmem_controller import AgentMemController
        from src.memory.episodic_store import EpisodicStore
        from src.agent.learning_loop import LearningLoop

        self.ctrl    = AgentMemController()
        self.episodic = MagicMock(spec=EpisodicStore)
        self.episodic.get_traces_for_learning.return_value = []
        self.episodic.__len__ = MagicMock(return_value=0)
        self.graph   = MagicMock()
        self.loop    = LearningLoop(self.ctrl, self.episodic, self.graph)

    def test_run_offline_update_no_traces(self):
        stats = self.loop.run_offline_update()
        self.assertIn("rewards_applied", stats)
        self.assertEqual(stats["rewards_applied"], 0)

    def test_run_offline_update_with_reward(self):
        from src.types.schema import ContextTrace
        trace = ContextTrace(
            task_id="t1",
            input_query="test",
            final_decision="sell",
            confidence=0.8,
            outcome="success",
            reward_signal=1.0,
        )
        self.episodic.get_traces_for_learning.return_value = [trace]
        stats = self.loop.run_offline_update()
        self.assertEqual(stats["rewards_applied"], 1)


if __name__ == "__main__":
    unittest.main()
