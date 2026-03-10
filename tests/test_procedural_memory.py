"""
Tests for Procedural Memory (Phase 10).
Verifies Procedure schema, GraphStore storage, VectorStore ingestion,
and Retriever integration — all via mocks.
"""
import unittest
from unittest.mock import MagicMock, patch, call
from src.types.schema import Procedure, ProcedureStep
from src.memory.graph_store import GraphStore
from src.memory.vector_store import VectorStore
from src.processing.ingest import Ingestor
from src.processing.retrieval import Retriever


class TestProcedureSchema(unittest.TestCase):
    def test_procedure_creation(self):
        proc = Procedure(
            name="Restart Nginx",
            description="How to restart Nginx on Linux.",
            steps=[
                ProcedureStep(step_number=1, action="Run `sudo systemctl stop nginx`", expected_outcome="Nginx stops"),
                ProcedureStep(step_number=2, action="Run `sudo systemctl start nginx`", expected_outcome="Nginx starts"),
            ],
            trigger_entities=["Nginx", "Server"]
        )
        self.assertEqual(proc.name, "Restart Nginx")
        self.assertEqual(len(proc.steps), 2)
        self.assertIn("Nginx", proc.trigger_entities)
        self.assertIsNotNone(proc.id)  # UUID auto-assigned

    def test_procedure_step_ordering(self):
        steps = [
            ProcedureStep(step_number=2, action="Step Two"),
            ProcedureStep(step_number=1, action="Step One"),
        ]
        sorted_steps = sorted(steps, key=lambda s: s.step_number)
        self.assertEqual(sorted_steps[0].action, "Step One")


class TestGraphStoreProcedures(unittest.TestCase):
    def setUp(self):
        self.store = GraphStore.__new__(GraphStore)
        self.mock_session = MagicMock()
        self.mock_driver = MagicMock()
        self.mock_driver.session.return_value.__enter__ = MagicMock(return_value=self.mock_session)
        self.mock_driver.session.return_value.__exit__ = MagicMock(return_value=False)
        self.store.driver = self.mock_driver

    def test_add_procedure_calls_session_run(self):
        proc = Procedure(
            name="Test Proc",
            description="A test procedure.",
            steps=[ProcedureStep(step_number=1, action="Do something")],
            trigger_entities=["Server"]
        )
        self.store.add_procedure(proc)
        # Should have called session.run multiple times: 1 for proc + 1 per step + 1 per entity
        self.assertGreaterEqual(self.mock_session.run.call_count, 3)

    def test_get_procedures_returns_empty_for_no_entities(self):
        result = self.store.get_procedures_for_entities([])
        self.assertEqual(result, [])

    def test_get_procedures_queries_graph(self):
        self.mock_session.run.return_value = []
        result = self.store.get_procedures_for_entities(["Server"])
        self.mock_session.run.assert_called()
        self.assertIsInstance(result, list)


class TestVectorStoreProcedures(unittest.TestCase):
    def setUp(self):
        self.store = VectorStore.__new__(VectorStore)
        self.mock_collection = MagicMock()
        self.mock_collection.add.return_value = None
        self.store.collection = self.mock_collection

    def test_add_procedure_stores_with_metadata(self):
        with patch.object(self.store, 'add_memory', return_value='vec-123') as mock_add:
            result = self.store.add_procedure("proc-id-1", "How to restart Nginx", "Restart Nginx")
            mock_add.assert_called_once_with(
                text="How to restart Nginx",
                metadata={"memory_type": "procedure", "procedure_id": "proc-id-1", "name": "Restart Nginx"}
            )
            self.assertEqual(result, 'vec-123')


class TestIngestorProcedure(unittest.TestCase):
    def setUp(self):
        self.graph_store = MagicMock(spec=GraphStore)
        self.vector_store = MagicMock(spec=VectorStore)
        self.llm_client = MagicMock()
        self.ingestor = Ingestor(self.graph_store, self.vector_store, self.llm_client)

    def test_ingest_procedure_calls_vector_and_graph(self):
        self.vector_store.add_procedure.return_value = "vec-456"
        self.llm_client.extract_procedure.return_value = {
            "name": "Restart Nginx",
            "steps": [
                {"action": "Stop service", "expected_outcome": "Service stopped"},
                {"action": "Start service", "expected_outcome": "Service running"},
            ]
        }
        procedure = self.ingestor.ingest_procedure(
            "To restart Nginx: stop then start the service.",
            trigger_entities=["Nginx"]
        )
        self.vector_store.add_procedure.assert_called_once()
        self.llm_client.extract_procedure.assert_called_once()
        self.graph_store.add_procedure.assert_called_once()
        self.assertEqual(procedure.name, "Restart Nginx")
        self.assertEqual(len(procedure.steps), 2)
        self.assertIn("Nginx", procedure.trigger_entities)


class TestRetrieverProcedures(unittest.TestCase):
    def test_retrieve_includes_procedural_context(self):
        graph_store = MagicMock(spec=GraphStore)
        vector_store = MagicMock(spec=VectorStore)
        llm_client = MagicMock()

        vector_store.search.return_value = []
        graph_store.search_nodes.return_value = ["Server"]
        graph_store.get_triplets_by_source.return_value = []
        graph_store.get_neighbors.return_value = []
        graph_store.get_procedures_for_entities.return_value = [{
            "name": "Restart Nginx",
            "steps": [{"step_number": 1, "action": "Stop service"}],
            "trigger_entities": ["Server"]
        }]

        retriever = Retriever(
            graph_store=graph_store,
            vector_store=vector_store,
            llm_client=llm_client,
            cluster_store=None
        )

        # Patch out cluster_ids ref
        with patch.object(retriever, 'retrieve', wraps=retriever.retrieve):
            result = retriever.retrieve("how to restart the server")

        self.assertIn("procedural_context", result)
        self.assertEqual(len(result["procedural_context"]), 1)
        self.assertEqual(result["procedural_context"][0]["name"], "Restart Nginx")


if __name__ == "__main__":
    unittest.main()
