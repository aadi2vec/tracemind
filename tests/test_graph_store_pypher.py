import unittest
from unittest.mock import MagicMock, patch
from src.memory.graph_store import GraphStore
from src.types.schema import Entity, Triplet
from pypher.builder import Pypher

class TestGraphStorePypher(unittest.TestCase):
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
        
        self.assertIn("MERGE (n:`Entity` {`name`: $", query)
        self.assertIn("SET n.`type` = $", query)
        self.assertIn("n.`description` = $", query)
        
        # Verify params are bound correctly
        param_values = list(kwargs.values())
        self.assertIn("Apple", param_values)
        self.assertIn("Company", param_values)
        self.assertIn("Tech giant", param_values)

    def test_add_triplet_query(self):
        triplet = Triplet(subject="Apple", predicate="headquartered_in", object="Cupertino", confidence=0.9, timestamp="2024-01-01", source_id="v1")
        self.store.add_triplet(triplet)
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MERGE (s:`Entity` {`name`: $", query)
        self.assertIn("MERGE (o:`Entity` {`name`: $", query)
        self.assertIn("RELATED_TO", query)
        self.assertIn("SET r.`timestamp` = $", query)
        
        param_values = list(kwargs.values())
        self.assertIn("Apple", param_values)
        self.assertIn("headquartered_in", param_values)
        self.assertIn("Cupertino", param_values)
        self.assertIn(0.9, param_values)

    def test_get_neighbors_query(self):
        self.store.get_neighbors("Apple")
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MATCH (s:`Entity` {`name`: $", query)
        self.assertIn("-[r:`RELATED_TO`]->", query)
        self.assertIn("AS subject", query) # Pypher doesn't backtick aliases by default if done via .alias()
        
        param_values = list(kwargs.values())
        self.assertIn("Apple", param_values)

    def test_search_nodes_query(self):
        self.store.search_nodes("App")
        
        args, kwargs = self.mock_session.run.call_args
        query = args[0]
        
        self.assertIn("MATCH (n:`Entity`)", query)
        self.assertIn("WHERE", query)
        self.assertIn("CONTAINS", query)
        self.assertIn("toLower", query)
        
        param_values = list(kwargs.values())
        self.assertIn("App", param_values)

if __name__ == "__main__":
    unittest.main()
