import unittest
from unittest.mock import MagicMock, patch
from src.memory.graph_store import GraphStore
from src.types.schema import Entity, Triplet

class TestGraphStoreCymple(unittest.TestCase):
    @patch('src.memory.graph_store.GraphDatabase.driver')
    def setUp(self, mock_driver):
        self.store = GraphStore(uri="bolt://localhost:7687", auth=("neo4j", "password"))
        self.mock_session = MagicMock()
        self.store.driver.session.return_value.__enter__.return_value = self.mock_session

    def test_add_entity_query(self):
        entity = Entity(name="Apple", type="Company", description="Tech giant")
        self.store.add_entity(entity)
        
        # Capture the query sent to session.run
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MERGE (n:Entity {name: $name})", query)
        self.assertIn("SET n.type = $type", query)
        self.assertIn("n.description = $description", query)
        self.assertEqual(kwargs['name'], "Apple")

    def test_add_triplet_query(self):
        triplet = Triplet(subject="Apple", predicate="headquartered_in", object="Cupertino", confidence=0.9)
        self.store.add_triplet(triplet)
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MERGE (s:Entity {name: $subject})", query)
        self.assertIn("MERGE (o:Entity {name: $object})", query)
        self.assertIn("MERGE (s)-[r:RELATED_TO {type: $predicate}]->(o)", query)
        self.assertIn("SET r.timestamp = $timestamp", query)
        self.assertEqual(kwargs['subject'], "Apple")

    def test_get_neighbors_query(self):
        self.store.get_neighbors("Apple")
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MATCH (s:Entity {name: $name})-[r:RELATED_TO]->(o:Entity)", query)
        self.assertIn("RETURN s.name as subject", query)
        self.assertEqual(kwargs['name'], "Apple")

    def test_search_nodes_query(self):
        self.store.search_nodes("App")
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        # Use more flexible assertions if cymple adds spaces
        self.assertIn("Entity", query)
        self.assertIn("CONTAINS toLower($query)", query)
        self.assertIn("RETURN n.name as name", query)
        self.assertEqual(kwargs['query'], "App")

if __name__ == "__main__":
    unittest.main()
