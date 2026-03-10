import unittest
from unittest.mock import MagicMock
from src.processing.retrieval import Retriever

class TestRetrievalV2(unittest.TestCase):
    def setUp(self):
        self.graph = MagicMock()
        self.vector = MagicMock()
        self.llm = MagicMock()
        self.cluster = MagicMock()
        
        # Default mocks
        self.vector.search.return_value = []
        self.graph.search_nodes.return_value = []
        self.graph.get_triplets_by_source.return_value = []
        self.graph.get_neighbors.return_value = []
        self.llm.get_embedding.return_value = [0.1] * 1536
        self.cluster.expand_query.return_value = ["E1"]
        self.cluster.embedding_dim = 1536

    def test_retrieve_calls_get_embedding(self):
        retriever = Retriever(self.graph, self.vector, self.llm, self.cluster)
        retriever.retrieve("test query")
        
        self.llm.get_embedding.assert_called_once_with("test query")
        self.cluster.expand_query.assert_called_once_with(
            query_embedding=[0.1] * 1536,
            top_k=10
        )

if __name__ == "__main__":
    unittest.main()
