import unittest
from unittest.mock import MagicMock, patch
import sys
import os

# Mock autogen
sys.modules["autogen"] = MagicMock()

from src.agent.workflow_autogen import AutoGenWorkflow
from src.agent.tools.registry import ToolRegistry
from src.memory.episodic_store import EpisodicStore

class TestDynamicAgents(unittest.TestCase):
    def setUp(self):
        self.mock_retriever = MagicMock()
        self.mock_llm = MagicMock()
        self.episodic = EpisodicStore(log_path="test_dynamic_trace.jsonl")
        self.registry = ToolRegistry(graph_store=MagicMock())
        
    def tearDown(self):
        if os.path.exists("test_dynamic_trace.jsonl"):
            os.remove("test_dynamic_trace.jsonl")

    @patch("src.agent.workflow_autogen.autogen.GroupChatManager")
    @patch("src.agent.workflow_autogen.autogen.GroupChat")
    @patch("src.agent.workflow_autogen.autogen.UserProxyAgent")
    @patch("src.agent.workflow_autogen.autogen.AssistantAgent")
    def test_hire_agent_logic(self, MockAssistant, MockUserProxy, MockGroupChat, MockManager):
        # Setup Workflow
        workflow = AutoGenWorkflow(self.mock_retriever, self.mock_llm, self.episodic, self.registry)
        
        # Initialize agents list in the mock groupchat
        workflow.groupchat.agents = []
        
        # Test hire_agent
        result = workflow.hire_agent("Python_Specialist", "You write python code.")
        
        # Verify
        self.assertIn("Agent Python_Specialist has been hired", result)
        self.assertEqual(len(workflow.groupchat.agents), 1)
        # self.assertEqual(workflow.groupchat.agents[0].try_to_set_attribute_or_mock_it, "mocked")
        
        # Verify AssistantAgent was created with correct name
        # We check the arguments passed to AssistantAgent constructor in the *latest* call
        # MockAssistant called multiple times during init (Planner, Memory, etc).
        # We want the LAST call.
        call_args = MockAssistant.call_args
        self.assertEqual(call_args[1]['name'], "Python_Specialist")
        self.assertEqual(call_args[1]['system_message'], "You write python code.")

if __name__ == "__main__":
    unittest.main()
