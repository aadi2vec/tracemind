import os
from datetime import datetime
import threading
import autogen
from typing import Dict, Any, List, Optional
from ..memory.episodic_store import EpisodicStore
from ..processing.retrieval import Retriever
from ..utils.llm import LLMClient
from .tools.registry import ToolRegistry
from .agentmem_controller import AgentMemController
from ..types.schema import ContextTrace

# Thread-local storage for capturing retrieval provenance during a chat session
_retrieval_storage = threading.local()

class AutoGenWorkflow:
    def __init__(
        self,
        retriever: Retriever,
        llm_client: LLMClient,
        episodic_store: EpisodicStore,
        tool_registry: ToolRegistry,
        controller: Optional[AgentMemController] = None,
    ):
        self.retriever = retriever
        self.llm_client = llm_client
        self.episodic_store = episodic_store
        self.tool_registry = tool_registry
        self.controller = controller or AgentMemController()
        
        # Configuration
        self.config_list = [{
            "model": "gpt-4o", 
            "api_key": os.environ.get("OPENAI_API_KEY", "sk-mock-key")
        }]
        
        self.llm_config = {
            "config_list": self.config_list,
            "temperature": 0.1,
        }

        # --- Agents ---

        # 1. User Proxy (Admin/Executor)
        self.user_proxy = autogen.UserProxyAgent(
            name="User_Proxy",
            system_message="A human admin. Execute suggestions and tools from agents. Terminate the chat when the Analyst provides the final answer.",
            code_execution_config=False,
            human_input_mode="NEVER",
            max_consecutive_auto_reply=10,
        )

        # 2. Planner
        self.planner = autogen.AssistantAgent(
            name="Planner",
            system_message="""You are a Planner.
            Goal: Breakdown the user's complex query into a step-by-step plan.
            Output: A numbered list of steps instructing which agent (Memory_Specialist or Researcher) should act.
            Do NOT call tools yourself. Just Plan.""",
            llm_config=self.llm_config,
        )

        # 3. Memory Specialist
        self.memory_agent = autogen.AssistantAgent(
            name="Memory_Specialist",
            system_message="""You are the Memory Specialist.
            Goal: Retrieve relevant information from the internal knowledge graph and vector store.
            Tools: 'retrieve_memory', 'graph_editor'.
            Action: When asked, use 'retrieve_memory' to find facts. Report the findings back to the Group.""",
            llm_config=self.llm_config,
        )

        # 4. Researcher
        self.researcher = autogen.AssistantAgent(
            name="Researcher",
            system_message="""You are the External Researcher.
            Goal: Find information from the internet that is NOT in internal memory.
            Tools: 'web_search'.
            Action: Use 'web_search' to get real-time info. Report findings back.""",
            llm_config=self.llm_config,
        )

        # 5. Analyst
        self.analyst = autogen.AssistantAgent(
            name="Analyst",
            system_message="""You are the Lead Analyst.
            Goal: Synthesize all information provided by Memory_Specialist and Researcher to answer the User's query.
            Tools: 'calculator'.
            Action:
            1. Listen to reports.
            2. Perform calculations if needed.
            3. Provide the FINAL ANSWER to the User_Proxy.
            4. End the message with "TERMINATE".""",
            llm_config=self.llm_config,
        )

        # --- Group Chat ---
        self.groupchat = autogen.GroupChat(
            agents=[self.user_proxy, self.planner, self.memory_agent, self.researcher, self.analyst],
            messages=[],
            max_round=20,
            speaker_selection_method="auto"
        )
        
        self.manager = autogen.GroupChatManager(
            groupchat=self.groupchat,
            llm_config=self.llm_config
        )

        # --- Tool Registration ---
        self._register_tools()

    def hire_agent(self, role: str, description: str) -> str:
        """
        Dynamically creates a new agent for the group.
        Args:
            role (str): The name of the agent (e.g., 'Python_Expert').
            description (str): The system prompt/instructions for the agent.
        """
        print(f"[Tool] Hiring new agent: {role}")
        
        # Create new agent
        new_agent = autogen.AssistantAgent(
            name=role,
            system_message=description,
            llm_config=self.llm_config
        )
        
        # Add to group chat
        self.groupchat.agents.append(new_agent)
        
        return f"Agent {role} has been hired and added to the group. You can now assign tasks to {role}."

    def _register_tools(self):
        # 1. Retrieve Memory -> Memory Agent (policy-guided depth/breadth)
        def retrieve_memory_wrapper(query: str) -> str:
            params = self.controller.get_retrieval_params()
            arm_name = params.get("arm")
            print(f"[AgentMem] Retrieving — depth={params['depth']}, breadth={params['breadth']}, arm={arm_name}")
            res = self.retriever.retrieve(query)
            
            # Capture provenance in thread-local storage
            if not hasattr(_retrieval_storage, "memories"):
                _retrieval_storage.memories = []
            _retrieval_storage.memories.append({"res": res, "arm": arm_name})
            
            return str(res)

        autogen.agentchat.register_function(
            retrieve_memory_wrapper,
            caller=self.memory_agent,
            executor=self.user_proxy,
            name="retrieve_memory",
            description="Search internal graph/vector memory."
        )

        # 2. Graph Editor -> Memory Agent
        graph_tool = self.tool_registry.get_tool("graph_editor")
        if graph_tool:
            autogen.agentchat.register_function(
                graph_tool.run,
                caller=self.memory_agent,
                executor=self.user_proxy,
                name="graph_editor",
                description="Modify the internal graph."
            )

        # 3. Web Search -> Researcher
        search_tool = self.tool_registry.get_tool("web_search")
        if search_tool:
            autogen.agentchat.register_function(
                search_tool.run,
                caller=self.researcher,
                executor=self.user_proxy,
                name="web_search",
                description=search_tool.description
            )
            
        # 4. Calculator -> Analyst
        calc_tool = self.tool_registry.get_tool("calculator")
        if calc_tool:
            autogen.agentchat.register_function(
                calc_tool.run,
                caller=self.analyst,
                executor=self.user_proxy,
                name="calculator",
                description=calc_tool.description
            )

        # 5. Hire Agent -> Planner
        autogen.agentchat.register_function(
            self.hire_agent,
            caller=self.planner,
            executor=self.user_proxy,
            name="hire_agent",
            description="Create a new specialized agent. Args: role (str), description (str)."
        )

    def run(self, query: str) -> Dict[str, Any]:
        """Entry point for the Multi-Agent workflow."""
        
        # Initiate Chat: UserProxy -> Manager
        # Using the Manager as the recipient for GroupChat
        chat_res = self.user_proxy.initiate_chat(
            self.manager,
            message=query,
            summary_method="reflection_with_llm"
        )
        
        final_msg = chat_res.summary

        # 2. Extract provenance from storage
        all_retrieved = getattr(_retrieval_storage, "memories", [])
        memory_ids = []
        full_memories = []
        used_arms = set()
        for entry in all_retrieved:
            r = entry["res"]
            arm = entry["arm"]
            if arm: used_arms.add(arm)
            
            # Vector IDs
            for v in r.get("vector_context", []):
                memory_ids.append(v.get("id"))
                full_memories.append({"type": "vector", "content": v})
            # Graph Triplets
            for g in r.get("graph_context", []):
                tid = f"{g['subject']}-{g['predicate']}-{g['object']}"
                memory_ids.append(tid)
                full_memories.append({"type": "graph", "content": g})

        # 3. Log trace
        trace = ContextTrace(
            trace_id=str(os.urandom(4).hex()),
            task_id="autogen_multi_agent",
            timestamp=datetime.now(),
            input_query=query,
            retrieved_memory_ids=list(set(memory_ids)),
            retrieved_memories=full_memories,
            retrieval_arm=list(used_arms)[0] if used_arms else None,
            reasoning_steps=[f"{msg.get('name', 'unknown')}: {msg.get('content')}" for msg in chat_res.chat_history],
            final_decision=str(final_msg),
            confidence=1.0,
            feedback=None
        )
        self.episodic_store.log_trace(trace)
        
        return {
            "final_decision": final_msg,
            "history": chat_res.chat_history,
            "trace_log": trace.trace_id
        }
