from neo4j import GraphDatabase
from typing import List, Dict, Any, Optional
import os
from cymple import QueryBuilder as Query
from ..types.schema import Entity, Triplet

class GraphStore:
    def __init__(self, uri: str = "bolt://localhost:7687", auth: tuple = ("neo4j", "password")):
        try:
            self.driver = GraphDatabase.driver(uri, auth=auth)
            self.verify_connectivity()
        except Exception as e:
            print(f"Failed to connect to Neo4j: {e}")
            self.driver = None

    def verify_connectivity(self):
        try:
            self.driver.verify_connectivity()
            print("Connected to Neo4j.")
        except Exception as e:
            print(f"Neo4j connectivity check failed: {e}")

    def close(self):
        if self.driver:
            self.driver.close()

    def add_entity(self, entity: Entity):
        """Adds or updates an entity node in the graph."""
        if not self.driver: return
        
        # We use .cypher() for the node patterns to ensure parameters like $name are not quoted
        query = str(Query()
                 .merge().cypher('(n:Entity {name: $name})')
                 .set('n.type = $type, n.description = $description'))
        
        with self.driver.session() as session:
            session.run(query, name=entity.name, type=entity.type, description=entity.description)

    def add_triplet(self, triplet: Triplet):
        """Adds a relationship between two entities."""
        if not self.driver: return
        
        query = str(Query()
                 .merge().cypher('(s:Entity {name: $subject})')
                 .merge().cypher('(o:Entity {name: $object})')
                 .merge().cypher('(s)-[r:RELATED_TO {type: $predicate}]->(o)')
                 .set('r.timestamp = $timestamp, r.confidence = $confidence, r.source_id = $source_id'))
        
        with self.driver.session() as session:
            session.run(query, 
                subject=triplet.subject, 
                object=triplet.object, 
                predicate=triplet.predicate,
                timestamp=triplet.timestamp,
                confidence=triplet.confidence,
                source_id=triplet.source_id
            )

    def get_neighbors(self, node_id: str, depth: int = 1) -> List[Triplet]:
        """Retrieves outgoing edges/triplets from a node."""
        if not self.driver: return []
        
        query = str(Query()
                 .match().cypher('(s:Entity {name: $name})-[r:RELATED_TO]->(o:Entity)')
                 .return_literal('s.name as subject, r.type as predicate, o.name as object, r.timestamp as timestamp, r.confidence as confidence, r.source_id as source_id'))
        
        results = []
        with self.driver.session() as session:
            record_list = session.run(query, name=node_id)
            for record in record_list:
                results.append(Triplet(
                    subject=record["subject"],
                    predicate=record["predicate"],
                    object=record["object"],
                    timestamp=record["timestamp"],
                    confidence=record.get("confidence", 1.0),
                    source_id=record.get("source_id")
                ))
        return results

    def search_nodes(self, query: str) -> List[str]:
        """Fuzzy searches for nodes by name."""
        if not self.driver: return []
        
        cypher = str(Query()
                  .match().node(labels=['Entity'], ref_name='n')
                  .where_literal('toLower(n.name) CONTAINS toLower($query)')
                  .return_literal('n.name as name')
                  .limit(5))
        
        results = []
        with self.driver.session() as session:
            records = session.run(cypher, query=query)
            for r in records:
                results.append(r["name"])
        return results

    def get_triplets_by_source(self, source_ids: List[str]) -> List[Dict[str, Any]]:
        """Retrieves triplets linked to specific vector memory IDs."""
        if not self.driver or not source_ids: return []
        
        cypher = str(Query()
                  .match().cypher('(s)-[r:RELATED_TO]->(o)')
                  .where_literal('r.source_id IN $source_ids')
                  .return_literal('s.name as subject, r.type as predicate, o.name as object, r.timestamp as timestamp, r.confidence as confidence'))
        
        results = []
        with self.driver.session() as session:
            records = session.run(cypher, source_ids=source_ids)
            for r in records:
                results.append(Triplet(
                    subject=r["subject"],
                    predicate=r["predicate"],
                    object=r["object"],
                    timestamp=r["timestamp"],
                    confidence=r["confidence"]
                ).model_dump())
        return results

    def save(self):
        pass

    def load(self):
        pass
