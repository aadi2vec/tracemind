import unittest
import os
import shutil
from src.memory.cluster_store import ClusterStore

class TestClusterPersistence(unittest.TestCase):
    def setUp(self):
        self.db_path = "test_cluster_store.db"
        if os.path.exists(self.db_path):
            os.remove(self.db_path)

    def tearDown(self):
        if os.path.exists(self.db_path):
            os.remove(self.db_path)

    def test_persistence_flow(self):
        # 1. Create a store and add data
        cs1 = ClusterStore(embedding_dim=4, min_cluster_size=2, db_path=self.db_path)
        cs1.add_entity("E1", [1.0, 0.0, 0.0, 0.0])
        cs1.add_entity("E2", [1.0, 0.1, 0.0, 0.0])
        cs1.fit()
        
        c1 = cs1.get_cluster_for("E1")
        self.assertNotEqual(c1, -1)

        # 2. Create a second instance and verify data is there
        cs2 = ClusterStore(embedding_dim=4, min_cluster_size=2, db_path=self.db_path)
        self.assertEqual(len(cs2), 2)
        self.assertEqual(cs2.get_cluster_for("E1"), c1)
        
        # Verify expand_query works from loaded data
        results = cs2.expand_query([1.0, 0.0, 0.0, 0.0], top_k=5)
        self.assertIn("E1", results)
        self.assertIn("E2", results)

if __name__ == "__main__":
    unittest.main()
