from datetime import datetime
from typing import List, Dict, Any, Optional, Literal
from pydantic import BaseModel, Field
from enum import Enum
import uuid


# ── Memory action space (AgentMem Controller) ────────────────────────────────

class MemoryAction(str, Enum):
    STORE    = "store"
    LINK     = "link"
    RETRIEVE = "retrieve"
    FORGET   = "forget"
    DEFER    = "defer"


# ── Core data types ───────────────────────────────────────────────────────────

class Entity(BaseModel):
    name: str
    type: str  # Person, Company, Policy, Asset, Event
    description: Optional[str] = None
    embedding: Optional[List[float]] = None


class Triplet(BaseModel):
    subject: str
    predicate: str
    object: str
    timestamp: Optional[str] = None
    confidence: float = 1.0
    source_id: Optional[str] = None


class GraphNode(BaseModel):
    id: str
    type: str
    properties: Dict[str, Any] = {}


# ── Soft clustering ──────────────────────────────────────────────────────────

class ClusterMembership(BaseModel):
    entity_id: str
    cluster_id: int          # -1 = noise (HDBSCAN convention)
    membership_score: float  # soft probability [0, 1]


# ── Human feedback ───────────────────────────────────────────────────────────

class Feedback(BaseModel):
    trace_id: str
    score: float  # -1.0 to 1.0
    correction: Optional[str] = None
    annotator_id: str = "user"
    timestamp: datetime = Field(default_factory=datetime.now)


# ── Context trace (full provenance) ──────────────────────────────────────────

class ContextTrace(BaseModel):
    trace_id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    task_id: str
    input_query: str

    # Retrieval provenance
    retrieved_memory_ids: List[str] = []
    retrieved_memories: List[Dict[str, Any]] = []
    graph_paths: List[List[str]] = []
    retrieval_arm: Optional[str] = None

    # Reasoning
    reasoning_steps: List[str] = []
    final_decision: str
    confidence: float

    # Outcome & reward (filled in after result is known)
    outcome: Literal["success", "failure", "unknown"] = "unknown"
    reward_signal: float = 0.0

    # Meta
    timestamp: datetime = Field(default_factory=datetime.now)
    feedback: Optional[Feedback] = None
