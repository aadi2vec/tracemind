import unittest
from unittest.mock import MagicMock, patch
import os
import sys
import shutil
from src.processing.ingest import Ingestor
from src.processing.retrieval import Retriever
import autogen
# Mock autogen to avoid needing API keys for unit tests
sys.modules["autogen"] = MagicMock()
from src.agent.workflow_autogen import AutoGenWorkflow
from src.agent.tools.registry import ToolRegistry
from src.memory.episodic_store import EpisodicStore
from src.utils.llm import LLMClient
from src.types.schema import Triplet

class TestAgentSystem(unittest.TestCase):
    def setUp(self):
        # Mocks for Stores to avoid needing Docker
        self.mock_graph_store = MagicMock()
        self.mock_vector_store = MagicMock()
        
        # Real episodic store (uses local file, safe)
        self.log_path = "test_episodic.jsonl"
        self.episodic_store = EpisodicStore(log_path=self.log_path)
        
        # Mock LLM
        self.llm_client = MagicMock(spec=LLMClient)
        self.llm_client.model = "gpt-4o"
        self.llm_client.extract_graph_data.return_value = {
            "entities": [{"name": "Fed", "type": "Org"}, {"name": "Rates", "type": "Concept"}],
            "triplets": [{"subject": "Fed", "predicate": "raises", "object": "Rates"}]
        }
        self.llm_client.completion.return_value = "Mocked Decision: Rates are high."

    def tearDown(self):
        if os.path.exists(self.log_path):
            os.remove(self.log_path)

    def test_ingestion(self):
        # Setup Ingestor
        self.mock_vector_store.add_memory.return_value = "mem_123"
        ingestor = Ingestor(self.mock_graph_store, self.mock_vector_store, self.llm_client)
        ingestor.ingest("The Fed raises rates.")
        
        # Verify Calls
        self.mock_vector_store.add_memory.assert_called_once()
        self.mock_graph_store.add_entity.assert_called() # Should be called for Fed and Rates
        self.mock_graph_store.add_triplet.assert_called_once()
        
    def test_retrieval_and_workflow(self):
        # Setup Retriever Mocks
        self.mock_vector_store.search.return_value = [
            {"id": "mem1", "content": "The Fed raises rates.", "metadata": {}, "distance": 0.1}
        ]
        
        # Mock Search Nodes returning "Fed"
        self.mock_graph_store.search_nodes.return_value = ["Fed"]
        
        # Mock Triplets by Source 
        self.mock_graph_store.get_triplets_by_source.return_value = [
            Triplet(subject="Fed", predicate="raises", object="Rates").model_dump()
        ]
        
        # Mock Neighbors
        self.mock_graph_store.get_neighbors.return_value = []

        # Run Workflow
        self.llm_client.get_embedding.return_value = [0.0] * 1536
        retriever = Retriever(self.mock_graph_store, self.mock_vector_store, self.llm_client)
        registry = ToolRegistry(self.mock_graph_store) # Real registry is fine with mock store
        
        # Mock Autogen initiate_chat
        with patch("autogen.UserProxyAgent"), patch("autogen.AssistantAgent"), patch("autogen.GroupChat"), patch("autogen.GroupChatManager"):
             workflow = AutoGenWorkflow(retriever, self.llm_client, self.episodic_store, registry)
             
             # Mock initiate_chat return
             mock_chat_res = MagicMock()
             mock_chat_res.summary = "Mocked Decision: Rates are high."
             mock_chat_res.chat_history = []
             workflow.user_proxy.initiate_chat.return_value = mock_chat_res
             
             result = workflow.run("What did the Fed do?")
        
        # Verify
        self.assertIn("Mocked Decision", result["final_decision"])
        
        # Check Trace
        traces = self.episodic_store.get_recent_traces(1)
        self.assertEqual(len(traces), 1)
        self.assertEqual(traces[0].input_query, "What did the Fed do?")

if __name__ == "__main__":
    unittest.main()
