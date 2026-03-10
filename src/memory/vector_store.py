import chromadb
from chromadb.config import Settings
from typing import List, Dict, Any, Optional
import uuid

class VectorStore:
    def __init__(self, collection_name: str = "agent_memory", host: str = "localhost", port: int = 8000):
        # Use HttpClient for remote/docker connection
        try:
            self.client = chromadb.HttpClient(host=host, port=str(port))
            self.collection = self.client.get_or_create_collection(name=collection_name)
        except Exception as e:
            print(f"Failed to connect to ChromaDB Server: {e}")
            self.client = None
            self.collection = None

    def add_memory(self, text: str, metadata: Dict[str, Any], embedding: Optional[List[float]] = None):
        """Adds a text memory to the vector store."""
        if not self.collection: return None
        
        memory_id = str(uuid.uuid4())
        self.collection.add(
            documents=[text],
            metadatas=[metadata],
            ids=[memory_id],
            embeddings=[embedding] if embedding else None
        )
        return memory_id

    def search(self, query: str, n_results: int = 5) -> List[Dict[str, Any]]:
        """Searches for similar memories."""
        if not self.collection: return []
        
        results = self.collection.query(
            query_texts=[query],
            n_results=n_results
        )
        
        formatted_results = []
        if results['documents'] and results['documents'][0]:
            for i in range(len(results['documents'][0])):
                formatted_results.append({
                    "id": results['ids'][0][i],
                    "content": results['documents'][0][i],
                    "metadata": results['metadatas'][0][i] if results['metadatas'] else {},
                    "distance": results['distances'][0][i] if results['distances'] else None
                })
        return formatted_results

    def add_procedure(self, procedure_id: str, description: str, name: str) -> Optional[str]:
        """Stores a procedure's description in the vector store for semantic discovery."""
        return self.add_memory(
            text=description,
            metadata={"memory_type": "procedure", "procedure_id": procedure_id, "name": name}
        )
