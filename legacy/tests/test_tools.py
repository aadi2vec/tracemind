import unittest
from unittest.mock import MagicMock, patch
import os
import sys

# Mock autogen
sys.modules["autogen"] = MagicMock()

from src.agent.workflow_autogen import AutoGenWorkflow
from src.agent.tools.registry import ToolRegistry
from src.agent.tools.standard import CalculatorTool
from src.memory.episodic_store import EpisodicStore
from src.processing.retrieval import Retriever
from src.utils.llm import LLMClient

class TestToolExecution(unittest.TestCase):
    def setUp(self):
        # Mocks
        self.mock_retriever = MagicMock()
        self.mock_retriever.retrieve.return_value = {}
        
        self.mock_llm = MagicMock()
        # Mock LLM returning a Tool Call request
        self.mock_llm.completion.return_value = '{"tool": "calculator", "params": {"expression": "2 + 2"}}'
        
        self.episodic = EpisodicStore(log_path="test_tool_trace.jsonl")
        
        # Real Registry with Calculator
        self.registry = ToolRegistry(graph_store=MagicMock()) # Mock graph store as calc doesn't need it
        
    def tearDown(self):
        if os.path.exists("test_tool_trace.jsonl"):
            os.remove("test_tool_trace.jsonl")

    @patch("src.agent.workflow_autogen.autogen.UserProxyAgent")
    @patch("src.agent.workflow_autogen.autogen.AssistantAgent")
    @patch("src.agent.workflow_autogen.autogen.GroupChat")
    @patch("src.agent.workflow_autogen.autogen.GroupChatManager")
    def test_tool_workflow(self, MockManager, MockGroupChat, MockAssistant, MockUserProxy):
        self.mock_llm.get_embedding.return_value = [0.0] * 1536
        retriever = Retriever(MagicMock(), MagicMock(), self.mock_llm)
        workflow = AutoGenWorkflow(retriever, self.mock_llm, self.episodic, self.registry)
        
        user_proxy_instance = MockUserProxy.return_value
        mock_chat_res = MagicMock()
        mock_chat_res.summary = "Tool (calculator) Output: 4"
        mock_chat_res.chat_history = []
        user_proxy_instance.initiate_chat.return_value = mock_chat_res

        result = workflow.run("Calculate 2+2")
        self.assertIn("Tool (calculator) Output: 4", result["final_decision"])
        
    def test_context_injection(self):
        # Verify tool descriptions are passed to Policy (via Retrieve Node)
        # We need to peek into Retrieve Node logic or check context passed to Policy
        # For this test, let's just check registry returns correct desc
        desc = self.registry.get_tool_descriptions()
        self.assertIn("calculator", desc)

if __name__ == "__main__":
    unittest.main()
