from typing import List, Dict, Any
from ..memory.graph_store import GraphStore
from ..memory.vector_store import VectorStore
from ..utils.llm import LLMClient
from ..types.schema import Entity, Triplet

class Ingestor:
    def __init__(self, graph_store: GraphStore, vector_store: VectorStore, llm_client: LLMClient):
        self.graph_store = graph_store
        self.vector_store = vector_store
        self.llm_client = llm_client

    def ingest(self, text: str, source: str = "user_input", **kwargs):
        """
        Canonicalizes text into the memory system.
        1. store raw text in vector store
        2. extract graph data
        3. update graph store
        """
        # 1. Vector Store
        metadata = {"source": source}
        metadata.update(kwargs) # Add extra metadata like 'app'
        memory_id = self.vector_store.add_memory(text, metadata)
        print(f"Stored in Vector Memory: {memory_id}")

        # 2. Extract Graph Data
        data = self.llm_client.extract_graph_data(text)
        
        # 3. Update Graph Store
        # Entities
        for e in data.get("entities", []):
            try:
                entity = Entity(
                    name=e["name"],
                    type=e.get("type", "Unknown"),
                    description=e.get("description")
                )
                self.graph_store.add_entity(entity)
            except Exception as e:
                print(f"Entity error: {e}")

        # Triplets
        for t in data.get("triplets", []):
            try:
                triplet = Triplet(
                    subject=t["subject"],
                    predicate=t["predicate"],
                    object=t["object"],
                    confidence=t.get("confidence", 1.0),
                    source_id=memory_id
                )
                self.graph_store.add_triplet(triplet)
            except Exception as e:
                print(f"Triplet error: {e}")
        
        print(f"Graph Updated: {len(data.get('entities', []))} entities, {len(data.get('triplets', []))} triplets.")
