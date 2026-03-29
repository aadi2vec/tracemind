import unittest
from unittest.mock import MagicMock, patch
import os
import sys

# Mock autogen to avoid needing API keys or real execution logic for unit tests
sys.modules["autogen"] = MagicMock()

from src.agent.workflow_autogen import AutoGenWorkflow
from src.agent.tools.registry import ToolRegistry
from src.memory.episodic_store import EpisodicStore
from src.processing.retrieval import Retriever
from src.utils.llm import LLMClient

class TestAutoGenSystem(unittest.TestCase):
    def setUp(self):
        # Mocks
        self.mock_retriever = MagicMock()
        self.mock_llm = MagicMock()
        self.episodic = EpisodicStore(log_path="test_autogen_trace.jsonl")
        self.registry = ToolRegistry(graph_store=MagicMock())
        
        # Setup AutoGen Mock returns
        self.mock_user_proxy = MagicMock()
        self.mock_analyst = MagicMock()
        
        # When workflow inits, it calls autogen.UserProxyAgent and AssistantAgent
        # We need to ensure the workflow instance gets our mocks
        
    def tearDown(self):
        if os.path.exists("test_autogen_trace.jsonl"):
            os.remove("test_autogen_trace.jsonl")

    @patch("src.agent.workflow_autogen.autogen.UserProxyAgent")
    @patch("src.agent.workflow_autogen.autogen.AssistantAgent")
    def test_workflow_init_and_run(self, MockAssistant, MockUserProxy):
        # Setup specific mock instances
        user_proxy_instance = MockUserProxy.return_value
        assistant_instance = MockAssistant.return_value
        
        # Mock chat result
        mock_chat_res = MagicMock()
        mock_chat_res.summary = "Final Answer via AutoGen"
        mock_chat_res.chat_history = [{"role": "user", "content": "hi"}, {"role": "assistant", "content": "Final Answer"}]
        
        user_proxy_instance.initiate_chat.return_value = mock_chat_res

        # Initialize
        workflow = AutoGenWorkflow(self.mock_retriever, self.mock_llm, self.episodic, self.registry)
        
        # Run
        result = workflow.run("Hello AutoGen")
        
        # Verify
        self.assertEqual(result["final_decision"], "Final Answer via AutoGen")
        user_proxy_instance.initiate_chat.assert_called_once()
        
        # Verify Tool Registration was attempted
        # We can perform a check on autogen.register_function calls if we mock the module level function 
        # But this basic test confirms the class structure works.

if __name__ == "__main__":
    unittest.main()
