from typing import List, Dict, Any
from ..memory.graph_store import GraphStore
from ..memory.vector_store import VectorStore
from ..utils.llm import LLMClient
from ..types.schema import Entity, Triplet, Procedure, ProcedureStep

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

    def ingest_procedure(self, text: str, trigger_entities: List[str], source: str = "user_input"):
        """
        Ingests a procedural "how-to" text into both the Vector Store and Graph Store.

        The Cognitive Pipeline (Kinetic Path):
        1. Store the raw description in the Vector Store for semantic discovery.
        2. Use LLM to extract structured steps.
        3. Store the structured Procedure + ProcedureStep nodes in the Graph Store.
        4. Link procedure to trigger entities via HAS_PROCEDURE edges.
        """
        # 1. Store description in Vector Store (for fuzzy semantic discovery)
        procedure_name = text.split(".")[0][:80]  # Use first sentence as name hint
        vector_id = self.vector_store.add_procedure(
            procedure_id="temp",  # will be replaced below after real id is known
            description=text,
            name=procedure_name
        )
        print(f"Stored Procedure in Vector Memory: {vector_id}")

        # 2. Extract steps via LLM
        data = self.llm_client.extract_procedure(text)
        steps = [
            ProcedureStep(
                step_number=i + 1,
                action=step.get("action", str(step)),
                expected_outcome=step.get("expected_outcome")
            )
            for i, step in enumerate(data.get("steps", []))
        ]

        # 3. Build and store Procedure in Graph Store
        procedure = Procedure(
            name=data.get("name", procedure_name),
            description=text,
            steps=steps,
            trigger_entities=trigger_entities,
            confidence=1.0,
            source_id=vector_id
        )
        self.graph_store.add_procedure(procedure)
        print(f"Procedure '{procedure.name}' stored with {len(steps)} steps linked to {trigger_entities}.")
        return procedure
