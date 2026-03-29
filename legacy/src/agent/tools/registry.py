from typing import Dict, List, Type
from .standard import BaseTool, CalculatorTool, WebSearchTool, GraphEditorTool
from ...memory.graph_store import GraphStore

class ToolRegistry:
    def __init__(self, graph_store: GraphStore):
        self.tools: Dict[str, BaseTool] = {}
        
        # Register Standard Tools
        self.register(CalculatorTool())
        self.register(WebSearchTool())
        self.register(GraphEditorTool(graph_store))

    def register(self, tool: BaseTool):
        self.tools[tool.name] = tool

    def get_tool(self, name: str) -> BaseTool:
        return self.tools.get(name)

    def get_tool_descriptions(self) -> str:
        return "\n".join([f"- {t.name}: {t.description}" for t in self.tools.values()])
    
    def list_tools(self) -> List[str]:
        return list(self.tools.keys())
