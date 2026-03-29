import json
from typing import List, Dict, Any, Optional
from litellm import completion, embedding
import os

class LLMClient:
    def __init__(self, model: Optional[str] = None, embedding_model: Optional[str] = None):
        self.model = model or os.getenv("LLM_MODEL", "gpt-4o")
        self.embedding_model = embedding_model or os.getenv("EMBEDDING_MODEL", "text-embedding-3-small")
        self._embedding_dim = None

    def get_embedding(self, text: str) -> List[float]:
        """Generates an embedding vector for the given text."""
        try:
            response = embedding(
                model=self.embedding_model,
                input=[text]
            )
            vec = response.data[0].embedding
            self._embedding_dim = len(vec)
            return vec
        except Exception as e:
            print(f"Embedding generation failed: {e}")
            # Return dummy zero vector of expected dimension (default 1536)
            return [0.0] * (self._embedding_dim or 1536)

    def extract_graph_data(self, text: str) -> Dict[str, Any]:
        """
        Extracts entities and triplets from text using a structured prompt.
        Returns a dictionary with 'entities' and 'triplets'.
        """
        prompt = f"""
        You are a knowledge graph extractor. specific for financial and policy domains.
        Extract relevant entities and their relationships from the text below.
        
        Output JSON format:
        {{
            "entities": [
                {{"name": "Fed", "type": "Organization", "description": "US Central Bank"}},
                ...
            ],
            "triplets": [
                {{"subject": "Fed", "predicate": "raises_rates", "object": "Interest_Rates", "confidence": 0.9}},
                ...
            ]
        }}
        
        Text: {text}
        """
        
        try:
            response = completion(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                response_format={"type": "json_object"}
            )
            content = response.choices[0].message.content
            return json.loads(content)
        except Exception as e:
            print(f"LLM Extraction failed: {e}")
            # Return empty structure on failure for MVP robustness
            return {"entities": [], "triplets": []}

    def extract_procedure(self, text: str) -> Dict[str, Any]:
        """
        Extracts a structured procedure (name + ordered steps) from a 'how-to' text.
        Returns: {"name": str, "steps": [{"action": str, "expected_outcome": str}]}
        """
        prompt = f"""
        You are a procedure extractor. Given the how-to text below, extract a structured procedure.

        Output JSON format:
        {{
            "name": "Brief procedure name (e.g. Restart Nginx)",
            "steps": [
                {{"action": "Step description", "expected_outcome": "What should happen"}},
                ...
            ]
        }}

        Text: {text}
        """
        try:
            response = completion(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                response_format={"type": "json_object"}
            )
            return json.loads(response.choices[0].message.content)
        except Exception as e:
            print(f"Procedure extraction failed: {e}")
            return {"name": text[:50], "steps": []}

    def generate_reasoning(self, query: str, context: str) -> str:
        """Generates a reasoning trace based on the query and retrieved context."""
        prompt = f"""
        Answer the query using the provided context.
        Provide a step-by-step reasoning trace.
        
        Context:
        {context}
        
        Query: {query}
        """
    def completion(self, prompt: str) -> str:
        """Generic completion for reasoning tasks."""
        try:
            response = completion(
                model=self.model,
                messages=[{"role": "user", "content": prompt}]
            )
            return response.choices[0].message.content
        except Exception as e:
            print(f"LLM Completion failed: {e}")
            return "Error generating response."

