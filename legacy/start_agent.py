import os
import time
import sys
import threading
from dotenv import load_dotenv
from src.memory.graph_store import GraphStore
from src.memory.cluster_store import ClusterStore
from src.memory.vector_store import VectorStore
from src.memory.episodic_store import EpisodicStore
from src.processing.ingest import Ingestor
from src.processing.retrieval import Retriever
from src.processing.monitor import NewsMonitor
from src.processing.macos_monitor import MacOSInteractionMonitor
from src.agent.agentmem_controller import AgentMemController
from src.agent.learning_loop import LearningLoop
from src.utils.llm import LLMClient

def main():
    load_dotenv()
    print("=== AgentMem v2 — Self-Improving Multi-Agent System ===")

    # 1. Initialize Memory (from env — works locally AND inside Docker)
    print("Connecting to Memory Core (Neo4j + Chroma)...")
    neo4j_uri  = os.getenv("NEO4J_URI",     "bolt://localhost:7687")
    neo4j_user = os.getenv("NEO4J_USER",    "neo4j")
    neo4j_pass = os.getenv("NEO4J_PASSWORD","password")
    chroma_host= os.getenv("CHROMA_HOST",   "localhost")
    chroma_port= int(os.getenv("CHROMA_PORT","8000"))
    
    # AI Models
    llm_model = os.getenv("LLM_MODEL", "gpt-4o")
    emb_model = os.getenv("EMBEDDING_MODEL", "text-embedding-3-small")
    
    if "ollama" in llm_model.lower():
        import requests
        try:
            requests.get("http://localhost:11434/api/tags", timeout=2)
            print(f"Ollama detected! Using local model: {llm_model}")
        except Exception:
            print("WARNING: Ollama model selected but service not detected at :11434. Local inference will fail.")

    try:
        graph_store    = GraphStore(uri=neo4j_uri, auth=(neo4j_user, neo4j_pass))
        cluster_store  = ClusterStore() # dimension will be inferred on first add
        vector_store   = VectorStore(host=chroma_host, port=chroma_port)
        episodic_store = EpisodicStore()
    except Exception as e:
        print(f"CRITICAL: Failed to connect to memory servers. Ensure 'docker-compose up' is running. Error: {e}")
        sys.exit(1)

    # 2. Initialize AgentMem controller + logic
    controller  = AgentMemController()
    llm_client  = LLMClient(model=llm_model, embedding_model=emb_model)

    from src.agent.tools.registry import ToolRegistry
    tool_registry = ToolRegistry(graph_store)

    ingestor  = Ingestor(graph_store, vector_store, llm_client)
    retriever = Retriever(graph_store, vector_store, cluster_store)

    from src.agent.workflow_autogen import AutoGenWorkflow
    workflow = AutoGenWorkflow(retriever, llm_client, episodic_store, tool_registry, controller)

    # 3. Start background services
    monitor = NewsMonitor(ingestor, interval=60)
    monitor.start()

    macos_monitor = None
    if os.getenv("ENABLE_MACOS_MONITOR", "false").lower() == "true":
        macos_monitor = MacOSInteractionMonitor(ingestor, interval=15)
        macos_monitor.start()

    learning_loop = LearningLoop(controller, episodic_store, graph_store)
    learning_loop.start(interval_s=300)  # update every 5 minutes

    print("\n[System] Ready. Type your query (or 'exit' to quit, 'bandit' to see arm stats).")
    if macos_monitor:
        print("[System] News monitor + Learning loop + MacOS monitor running in background...")
    else:
        print("[System] News monitor + Learning loop running in background...")

    try:
        while True:
            try:
                user_input = input("\n> ")
                if user_input.lower() in ["exit", "quit", "q"]:
                    break

                if not user_input.strip():
                    continue

                # Debug command: show bandit arm Q-values
                if user_input.lower() == "bandit":
                    import json
                    print(json.dumps(controller.bandit_summary(), indent=2))
                    continue

                print("[AgentMem] Routing query through GroupChat...")
                result = workflow.run(user_input)

                print(f"\n>> DECISION: {result.get('final_decision')}")
                if result.get("trace_log"):
                    print(f">> TRACE ID: {result.get('trace_log')}")

            except KeyboardInterrupt:
                break
            except Exception as e:
                print(f"[Error] {e}")

    finally:
        print("\n[System] Shutting down...")
        monitor.stop()
        if macos_monitor:
            macos_monitor.stop()
        learning_loop.stop()
        if graph_store:
            graph_store.close()
        print("[System] Bye.")

if __name__ == "__main__":
    main()
