from neo4j import GraphDatabase
from typing import List, Dict, Any, Optional
import os
from pypher.builder import Pypher, __, Param
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
        """Adds or updates an entity node in the graph using Pypher."""
        if not self.driver: return
        
        p = Pypher()
        p.MERGE.node('n', labels='Entity', name=entity.name)
        p.SET(__.n.property('type') == entity.type)
        p.SET(__.n.property('description') == entity.description)
        
        with self.driver.session() as session:
            session.run(str(p), **p.bound_params)

    def add_triplet(self, triplet: Triplet):
        """Adds a relationship between two entities using Pypher."""
        if not self.driver: return
        
        p = Pypher()
        p.MERGE.node('s', labels='Entity', name=triplet.subject)
        p.MERGE.node('o', labels='Entity', name=triplet.object)
        p.MERGE.node('s').relationship('r', labels='RELATED_TO', direction='out', type=triplet.predicate).node('o')
        p.SET(__.r.property('timestamp') == triplet.timestamp)
        p.SET(__.r.property('confidence') == triplet.confidence)
        p.SET(__.r.property('source_id') == triplet.source_id)
        
        with self.driver.session() as session:
            session.run(str(p), **p.bound_params)

    def get_neighbors(self, node_id: str, depth: int = 1) -> List[Triplet]:
        """Retrieves outgoing edges/triplets from a node using Pypher."""
        if not self.driver: return []
        
        p = Pypher()
        p.MATCH.node('s', labels='Entity', name=node_id).relationship('r', labels='RELATED_TO', direction='out').node('o', labels='Entity')
        p.RETURN(__.s.property('name').alias('subject'), 
                 __.r.property('type').alias('predicate'), 
                 __.o.property('name').alias('object'), 
                 __.r.property('timestamp').alias('timestamp'), 
                 __.r.property('confidence').alias('confidence'), 
                 __.r.property('source_id').alias('source_id'))
        
        results = []
        with self.driver.session() as session:
            record_list = session.run(str(p), **p.bound_params)
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
        """Fuzzy searches for nodes by name using Pypher."""
        if not self.driver: return []
        
        p = Pypher()
        p.MATCH.node('n', labels='Entity')
        p.WHERE(__.toLower(__.n.property('name')).CONTAINS(__.toLower(Param('q', query))))
        p.RETURN(__.n.property('name').alias('name'))
        p.LIMIT(5)
        
        results = []
        with self.driver.session() as session:
            records = session.run(str(p), **p.bound_params)
            for r in records:
                results.append(r["name"])
        return results

    def get_triplets_by_source(self, source_ids: List[str]) -> List[Dict[str, Any]]:
        """Retrieves triplets linked to specific vector memory IDs using Pypher."""
        if not self.driver or not source_ids: return []
        
        p = Pypher()
        p.MATCH.node('s').relationship('r', labels='RELATED_TO').node('o')
        p.WHERE(__.r.property('source_id').IN(Param('sid', source_ids)))
        p.RETURN(__.s.property('name').alias('subject'), 
                 __.r.property('type').alias('predicate'), 
                 __.o.property('name').alias('object'), 
                 __.r.property('timestamp').alias('timestamp'), 
                 __.r.property('confidence').alias('confidence'))
        
        results = []
        with self.driver.session() as session:
            records = session.run(str(p), **p.bound_params)
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
