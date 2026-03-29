from neo4j import GraphDatabase
from typing import List, Dict, Any, Optional
import os
from pypher.builder import Pypher, __, Param
from ..types.schema import Entity, Triplet, Procedure

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

    def add_procedure(self, procedure: Procedure):
        """Stores a Procedure node and its steps in the graph using Pypher.
        
        Creates:
          (:Procedure {id, name, description, confidence, source_id})
          (:ProcedureStep {step_number, action, expected_outcome}) connected via [:HAS_STEP]
          Entity nodes connected via [:HAS_PROCEDURE]
        """
        if not self.driver: return

        # 1. MERGE the Procedure node
        p = Pypher()
        p.MERGE.node('proc', labels='Procedure', id=procedure.id)
        p.SET(__.proc.property('name') == procedure.name)
        p.SET(__.proc.property('description') == procedure.description)
        p.SET(__.proc.property('confidence') == procedure.confidence)
        p.SET(__.proc.property('source_id') == procedure.source_id)
        p.SET(__.proc.property('version') == procedure.version)
        p.SET(__.proc.property('deprecated') == procedure.deprecated)
        p.SET(__.proc.property('created_at') == str(procedure.created_at))
        with self.driver.session() as session:
            session.run(str(p), **p.bound_params)

        # 2. MERGE each ProcedureStep and link to Procedure
        for step in procedure.steps:
            ps = Pypher()
            ps.MATCH.node('proc', labels='Procedure', id=procedure.id)
            ps.MERGE.node('s', labels='ProcedureStep',
                          procedure_id=procedure.id,
                          step_number=step.step_number)
            ps.SET(__.s.property('action') == step.action)
            ps.SET(__.s.property('expected_outcome') == step.expected_outcome)
            ps.MERGE.node('proc').relationship('r', labels='HAS_STEP', direction='out').node('s')
            with self.driver.session() as session:
                session.run(str(ps), **ps.bound_params)

        # 3. Link Procedure to trigger entities
        for entity_name in procedure.trigger_entities:
            pe = Pypher()
            pe.MERGE.node('e', labels='Entity', name=entity_name)
            pe.MERGE.node('proc', labels='Procedure', id=procedure.id)
            pe.MERGE.node('e').relationship('r', labels='HAS_PROCEDURE', direction='out').node('proc')
            with self.driver.session() as session:
                session.run(str(pe), **pe.bound_params)

    def get_procedures_for_entities(self, entity_names: List[str]) -> List[Dict[str, Any]]:
        """Retrieves all Procedures and their steps linked to the given entity names."""
        if not self.driver or not entity_names: return []

        # Use raw Cypher param passing for lists since Pypher cannot hash list values
        cypher = (
            "MATCH (e:Entity)-[:HAS_PROCEDURE]->(proc:Procedure) "
            "WHERE e.name IN $names AND proc.deprecated = false "
            "RETURN e.name AS entity, proc.id AS proc_id, proc.name AS proc_name, "
            "proc.description AS description, proc.confidence AS confidence, "
            "proc.version AS version"
        )

        procedures: Dict[str, Dict] = {}
        with self.driver.session() as session:
            records = session.run(cypher, names=entity_names)
            for r in records:
                pid = r['proc_id']
                if pid not in procedures:
                    procedures[pid] = {
                        'id': pid,
                        'name': r['proc_name'],
                        'description': r['description'],
                        'confidence': r['confidence'],
                        'version': r['version'],
                        'trigger_entities': [],
                        'steps': []
                    }
                procedures[pid]['trigger_entities'].append(r['entity'])

        # Fetch steps for each procedure
        steps_cypher = (
            "MATCH (proc:Procedure {id: $proc_id})-[:HAS_STEP]->(s:ProcedureStep) "
            "RETURN s.step_number AS step_number, s.action AS action, "
            "s.expected_outcome AS expected_outcome "
            "ORDER BY s.step_number"
        )
        for pid, proc in procedures.items():
            with self.driver.session() as session:
                step_records = session.run(steps_cypher, proc_id=pid)
                for sr in step_records:
                    proc['steps'].append({
                        'step_number': sr['step_number'],
                        'action': sr['action'],
                        'expected_outcome': sr['expected_outcome']
                    })

        return list(procedures.values())

    def deprecate_procedure(self, procedure_id: str):
        """Marks a procedure as deprecated (superseded by a newer version)."""
        if not self.driver: return
        cypher = "MATCH (proc:Procedure {id: $pid}) SET proc.deprecated = true"
        with self.driver.session() as session:
            session.run(cypher, pid=procedure_id)

    def update_procedure_confidence(self, procedure_id: str, new_confidence: float):
        """Updates the confidence of a procedure (reward-based learning)."""
        if not self.driver: return
        cypher = "MATCH (proc:Procedure {id: $pid}) SET proc.confidence = $conf"
        with self.driver.session() as session:
            session.run(cypher, pid=procedure_id, conf=max(0.0, min(1.0, new_confidence)))

    def save(self):
        pass

    def load(self):
        pass
