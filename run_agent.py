import os
import argparse
from dotenv import load_dotenv
from src.memory.graph_store import GraphStore
from src.memory.cluster_store import ClusterStore
from src.memory.vector_store import VectorStore
from src.memory.episodic_store import EpisodicStore
from src.processing.ingest import Ingestor
from src.processing.retrieval import Retriever
from src.utils.llm import LLMClient

load_dotenv()
if not os.getenv("OPENAI_API_KEY"):
    print("WARNING: OPENAI_API_KEY not found in env.")

def main():
    parser = argparse.ArgumentParser(description="AgentMem v2 — CLI")
    subparsers = parser.add_subparsers(dest="command")

    # ingest
    ingest_p = subparsers.add_parser("ingest", help="Ingest text into memory")
    ingest_p.add_argument("--text", type=str, required=True)

    # query
    query_p = subparsers.add_parser("query", help="Ask the agent a question")
    query_p.add_argument("question", type=str)

    # feedback  ← new
    fb_p = subparsers.add_parser("feedback", help="Submit human feedback for a past decision")
    fb_p.add_argument("--trace-id", required=True, help="Trace ID from a previous query")
    fb_p.add_argument("--score", type=float, required=True, help="Score -1.0 to 1.0")
    fb_p.add_argument("--correction", type=str, default=None, help="Optional correction text")

    args = parser.parse_args()

    # ── Init ──────────────────────────────────────────────────────────────
    neo4j_uri  = os.getenv("NEO4J_URI",      "bolt://localhost:7687")
    neo4j_user = os.getenv("NEO4J_USER",     "neo4j")
    neo4j_pass = os.getenv("NEO4J_PASSWORD", "password")
    chroma_host= os.getenv("CHROMA_HOST",    "localhost")
    chroma_port= int(os.getenv("CHROMA_PORT","8000"))

    graph_store    = GraphStore(uri=neo4j_uri, auth=(neo4j_user, neo4j_pass))
    cluster_store  = ClusterStore(embedding_dim=1536)
    vector_store   = VectorStore(host=chroma_host, port=chroma_port)
    episodic_store = EpisodicStore()
    llm_client     = LLMClient()

    from src.agent.tools.registry import ToolRegistry
    from src.agent.agentmem_controller import AgentMemController
    tool_registry = ToolRegistry(graph_store)
    controller    = AgentMemController()

    ingestor  = Ingestor(graph_store, vector_store, llm_client)
    retriever = Retriever(graph_store, vector_store, llm_client, cluster_store)

    # ── Commands ──────────────────────────────────────────────────────────
    if args.command == "ingest":
        print(f"Ingesting: {args.text[:60]}...")
        ingestor.ingest(args.text)
        print("Done.")

    elif args.command == "query":
        from src.agent.workflow_autogen import AutoGenWorkflow
        workflow = AutoGenWorkflow(retriever, llm_client, episodic_store, tool_registry, controller)
        print(f"Querying: {args.question}")
        result = workflow.run(args.question)
        print("\n=== FINAL DECISION ===")
        print(result.get("final_decision"))
        print(f"\nTrace ID: {result.get('trace_log')}")

    elif args.command == "feedback":
        from src.agent.feedback import FeedbackManager
        mgr = FeedbackManager(episodic_store, ingestor, controller)
        result = mgr.ingest_feedback(
            trace_id=args.trace_id,
            score=args.score,
            correction=args.correction,
        )
        print(f"Feedback recorded: {result}")

    else:
        parser.print_help()

if __name__ == "__main__":
    main()
