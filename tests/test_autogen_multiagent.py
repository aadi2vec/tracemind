import unittest
from unittest.mock import MagicMock, patch
import os
import sys

# Mock autogen
sys.modules["autogen"] = MagicMock()

from src.agent.workflow_autogen import AutoGenWorkflow
from src.agent.tools.registry import ToolRegistry
from src.memory.episodic_store import EpisodicStore
from src.processing.retrieval import Retriever
from src.utils.llm import LLMClient

class TestAutoGenMultiAgent(unittest.TestCase):
    def setUp(self):
        # Mocks
        self.mock_retriever = MagicMock()
        self.mock_llm = MagicMock()
        self.episodic = EpisodicStore(log_path="test_multi_agent_trace.jsonl")
        self.registry = ToolRegistry(graph_store=MagicMock())
        
    def tearDown(self):
        if os.path.exists("test_multi_agent_trace.jsonl"):
            os.remove("test_multi_agent_trace.jsonl")

    @patch("src.agent.workflow_autogen.autogen.GroupChatManager")
    @patch("src.agent.workflow_autogen.autogen.GroupChat")
    @patch("src.agent.workflow_autogen.autogen.UserProxyAgent")
    @patch("src.agent.workflow_autogen.autogen.AssistantAgent")
    def test_multi_agent_workflow(self, MockAssistant, MockUserProxy, MockGroupChat, MockManager):
        # Setup specific mock instances
        user_proxy_instance = MockUserProxy.return_value
        
        # Mock chat result
        mock_chat_res = MagicMock()
        mock_chat_res.summary = "Global Recession Probability: High"
        # Simulate Multi-Agent History
        mock_chat_res.chat_history = [
            {"name": "User_Proxy", "content": "Analyze risk."},
            {"name": "Planner", "content": "1. Check news."},
            {"name": "Researcher", "content": "Inflation is 5%."},
            {"name": "Analyst", "content": "Risk is high. TERMINATE"}
        ]
        
        user_proxy_instance.initiate_chat.return_value = mock_chat_res

        # Initialize
        workflow = AutoGenWorkflow(self.mock_retriever, self.mock_llm, self.episodic, self.registry)
        
        # Verification: Check if multiple agents were created
        # We expect 4 AssistantAgents (Planner, Memory, Researcher, Analyst)
        self.assertTrue(MockAssistant.call_count >= 4)
        
        # Verify GroupChat was initialized
        MockGroupChat.assert_called_once()
        
        # Verify Manager was initialized
        MockManager.assert_called_once()

        # Run
        result = workflow.run("Analyze market")
        
        # Verify
        self.assertEqual(result["final_decision"], "Global Recession Probability: High")
        # Verify user proxy talked to Manager (not Analyst directly used in previous version)
        user_proxy_instance.initiate_chat.assert_called_once()
        args, _ = user_proxy_instance.initiate_chat.call_args
        self.assertEqual(args[0], MockManager.return_value)

if __name__ == "__main__":
    unittest.main()
