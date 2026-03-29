from typing import Any, Dict, List, Optional
from abc import ABC, abstractmethod
from ...memory.graph_store import GraphStore
from ...types.schema import Entity, Triplet

class BaseTool(ABC):
    name: str = "base_tool"
    description: str = "Base tool description"

    @abstractmethod
    def run(self, **kwargs) -> str:
        pass

class CalculatorTool(BaseTool):
    name: str = "calculator"
    description: str = "Perform math calculations. Input: expression (str)"

    def run(self, expression: str) -> str:
        try:
            # Dangerous in prod, safe for MVP if controlled
            return str(eval(expression))
        except Exception as e:
            return f"Error: {e}"

# Placeholder for real web search (could use Google Search API, DuckDuckGo etc.)
class WebSearchTool(BaseTool):
    name: str = "web_search"
    description: str = "Search the web for current information. Input: query (str)"

    def run(self, query: str) -> str:
        return f"[Mock Search Result] Results for '{query}': DeepSeek-R1 is a new reasoning model released in 2025..."

class GraphEditorTool(BaseTool):
    name: str = "graph_editor"
    description: str = "Manually add nodes/edges to the graph. Input: operation (add_node/add_edge), params (dict)"

    def __init__(self, graph_store: GraphStore):
        self.graph_store = graph_store

    def run(self, operation: str, params: Dict[str, Any]) -> str:
        try:
            if operation == "add_node":
                entity = Entity(**params)
                self.graph_store.add_entity(entity)
                return f"Added node: {entity.name}"
            elif operation == "add_edge":
                triplet = Triplet(**params)
                self.graph_store.add_triplet(triplet)
                return f"Added edge: {triplet.subject} -[{triplet.predicate}]-> {triplet.object}"
            else:
                return "Unknown operation"
        except Exception as e:
            return f"Error executing graph operation: {e}"
