from typing import List, Dict, Any, Optional, Set
from ..memory.graph_store import GraphStore
from ..memory.vector_store import VectorStore
from ..memory.cluster_store import ClusterStore


class Retriever:
    def __init__(
        self,
        graph_store: GraphStore,
        vector_store: VectorStore,
        llm_client: Any,
        cluster_store: Optional[ClusterStore] = None,
    ):
        self.graph_store = graph_store
        self.vector_store = vector_store
        self.llm_client = llm_client
        self.cluster_store = cluster_store

    def retrieve(self, query: str) -> Dict[str, Any]:
        """
        Performs hybrid retrieval.
        1. Vector search to find relevant text chunks.
        2. Graph search: 
            a. Entities directly mentioned in the query (Fuzzy Search).
            b. Triplets linked to the found Vector memories (Source ID matching).
            c. 1-hop expansion of found entities.
        Returns combined context.
        """
        # 1. Vector Search
        vector_results = self.vector_store.search(query, n_results=3)
        vector_ids = [res['id'] for res in vector_results]
        
        graph_context = []
        candidate_entities: Set[str] = set()

        # 2a. Graph: Fuzzy search for entities in query
        # Split query into words/bigrams? Or just pass full query? 
        # Passing full query to 'CONTAINS' might be too strict if query is long.
        # MVP: split by space and search for Keywords
        keywords = [w for w in query.split() if len(w) > 3] # simple filter
        for kw in keywords:
            found = self.graph_store.search_nodes(kw)
            candidate_entities.update(found)

        # 2b. Graph: Fetch triplets linked to vector results
        linked_triplets = self.graph_store.get_triplets_by_source(vector_ids)
        graph_context.extend(linked_triplets)
        
        # Add entities from these triplets to candidates for expansion
        for t in linked_triplets:
            candidate_entities.add(t['subject'])
            candidate_entities.add(t['object'])

        # 2c. Expand Graph Neighborhood (1 hop) for candidate entities
        # Cap candidates to avoid explosion
        for entity in list(candidate_entities)[:5]:
            neighbors = self.graph_store.get_neighbors(entity, depth=1)
            for triple in neighbors:
                graph_context.append(triple.model_dump())

        # Deduplicate graph context
        unique_context = []
        seen = set()
        for item in graph_context:
            # Create unique key
            key = f"{item['subject']}-{item['predicate']}-{item['object']}"
            if key not in seen:
                seen.add(key)
                unique_context.append(item)

        # 3. Cluster expansion (probabilistic) — only if cluster_store available
        if self.cluster_store is not None:
            # Phase 2.2: Use real query embedding for expansion
            query_embedding = self.llm_client.get_embedding(query)
            cluster_ids = self.cluster_store.expand_query(
                query_embedding=query_embedding,
                top_k=10,
            )
            for eid in cluster_ids[:5]:  # budget cap
                if eid not in candidate_entities:
                    neighbors = self.graph_store.get_neighbors(eid, depth=1)
                    for triple in neighbors:
                        t_dict = triple.model_dump()
                        key = f"{t_dict['subject']}-{t_dict['predicate']}-{t_dict['object']}"
                        if key not in seen:
                            seen.add(key)
                            unique_context.append(t_dict)

        return {
            "vector_context": vector_results,
            "graph_context": unique_context,
            "entities_found": list(candidate_entities),
            "cluster_expanded": cluster_ids,
        }
