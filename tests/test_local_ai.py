import unittest
import os
from unittest.mock import patch, MagicMock
from src.utils.llm import LLMClient
from src.memory.cluster_store import ClusterStore

class TestLocalAIIntegration(unittest.TestCase):
    def setUp(self):
        # Clear env vars before tests
        if "LLM_MODEL" in os.environ: del os.environ["LLM_MODEL"]
        if "EMBEDDING_MODEL" in os.environ: del os.environ["EMBEDDING_MODEL"]
        self.test_db = "test_local_ai.db"
        if os.path.exists(self.test_db):
            os.remove(self.test_db)

    def tearDown(self):
        if os.path.exists(self.test_db):
            os.remove(self.test_db)

    def test_client_defaults(self):
        client = LLMClient()
        self.assertEqual(client.model, "gpt-4o")
        self.assertEqual(client.embedding_model, "text-embedding-3-small")

    def test_client_env_override(self):
        os.environ["LLM_MODEL"] = "ollama/llama3.2"
        os.environ["EMBEDDING_MODEL"] = "ollama/nomic-embed-text"
        client = LLMClient()
        self.assertEqual(client.model, "ollama/llama3.2")
        self.assertEqual(client.embedding_model, "ollama/nomic-embed-text")

    @patch("src.utils.llm.embedding")
    def test_adaptive_dimensions(self, mock_embedding):
        # Simulate a 768-dim local model (like nomic-embed-text)
        mock_embedding.return_value.data = [MagicMock(embedding=[0.1]*768)]
        
        client = LLMClient(embedding_model="ollama/nomic-embed-text")
        vec = client.get_embedding("test")
        self.assertEqual(len(vec), 768)
        
        store = ClusterStore(db_path=self.test_db)
        store.add_entity("test_entity", vec)
        self.assertEqual(store.embedding_dim, 768)

    @patch("src.utils.llm.completion")
    def test_ollama_completion_call(self, mock_completion):
        client = LLMClient(model="ollama/llama3.2")
        client.completion("hello")
        
        # Verify litellm was called with the correct model string
        args, kwargs = mock_completion.call_args
        self.assertEqual(kwargs["model"], "ollama/llama3.2")

if __name__ == "__main__":
    unittest.main()
