"""
AgentMem Controller (Phase C)
================================
A policy over memory operations. Rather than blindly reading/writing
the graph on every turn, the system passes through the controller which
decides:

  * Whether to STORE new entities/triplets
  * How deep and wide to RETRIEVE
  * Whether to FORGET low-confidence facts
  * Whether to LINK loosely related entities

Phase 1: Heuristic policy (confidence threshold + TTL-based decay).
Phase 2: Epsilon-greedy bandit updates to retrieval breadth / storage
         selectivity based on downstream decision quality signals.

The controller is injected into AutoGenWorkflow and called by the
Memory_Specialist agent's tool wrappers.
"""
from __future__ import annotations

import logging
import math
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from ..types.schema import MemoryAction

logger = logging.getLogger(__name__)


# ── Bandit arm definition ─────────────────────────────────────────────────────

@dataclass
class BanditArm:
    """One arm = one retrieval strategy (breadth level)."""
    name: str
    depth: int
    breadth: int          # max candidate entities
    total_reward: float = 0.0
    pulls: int = 0

    @property
    def q_value(self) -> float:
        """Average reward (Q)."""
        return self.total_reward / max(self.pulls, 1)

    @property
    def ucb(self) -> float:
        """Upper-Confidence Bound for arm selection."""
        if self.pulls == 0:
            return float("inf")
        return self.q_value + math.sqrt(2 * math.log(max(self.pulls, 1)) / self.pulls)


# ── AgentMem Controller ───────────────────────────────────────────────────────

class AgentMemController:
    """
    Central policy over all memory operations.

    Attributes
    ----------
    confidence_threshold : float
        Minimum confidence for a new triplet to be stored.
    ttl_hours : float
        Age (hours) after which a fact is eligible for forgetting.
    epsilon : float
        Exploration rate for bandit arm selection (Phase 2).
    """

    # Bandit arms — retrieval strategies ordered by breadth
    DEFAULT_ARMS = [
        BanditArm("narrow",  depth=1, breadth=3),
        BanditArm("medium",  depth=1, breadth=5),
        BanditArm("wide",    depth=2, breadth=10),
        BanditArm("deep",    depth=3, breadth=15),
    ]

    def __init__(
        self,
        confidence_threshold: float = 0.4,
        ttl_hours: float = 72.0,
        epsilon: float = 0.15,
    ):
        self.confidence_threshold = confidence_threshold
        self.ttl_hours = ttl_hours
        self.epsilon = epsilon

        self.arms: List[BanditArm] = [BanditArm(**vars(a)) for a in self.DEFAULT_ARMS]
        self._total_pulls: int = 0
        self._lock = threading.Lock()

        # Thread-local for simple fallback if arm name isn't passed back
        self._local = threading.local()

        logger.info(
            "AgentMemController ready | confidence_threshold=%.2f | ttl_hours=%.1f | epsilon=%.2f",
            confidence_threshold, ttl_hours, epsilon,
        )

    # ── Action: STORE ─────────────────────────────────────────────────────

    def should_store(self, confidence: float) -> bool:
        """
        Heuristic policy: store only if confidence exceeds threshold.
        Returns True (STORE) or False (DEFER).
        """
        action = MemoryAction.STORE if confidence >= self.confidence_threshold else MemoryAction.DEFER
        logger.debug("AgentMem.should_store(%.2f) → %s", confidence, action.value)
        return action == MemoryAction.STORE

    # ── Action: RETRIEVE ─────────────────────────────────────────────────

    def get_retrieval_params(self) -> Dict[str, Any]:
        """
        UCB bandit arm selection → returns {depth, breadth, arm} for this query.
        """
        import random
        with self._lock:
            if random.random() < self.epsilon:
                arm = random.choice(self.arms)
            else:
                arm = max(self.arms, key=lambda a: a.ucb)
            
            arm.pulls += 1
            self._total_pulls += 1
            self._local.last_arm_name = arm.name
            
        logger.debug("AgentMem retrieve arm: %s (depth=%d, breadth=%d)", arm.name, arm.depth, arm.breadth)
        return {"depth": arm.depth, "breadth": arm.breadth, "arm": arm.name}

    def register_reward(self, reward: float, arm_name: Optional[str] = None) -> None:
        """
        Called after a decision outcome is known.
        Updates the Q-value of the specified or last-used retrieval arm.
        """
        target_name = arm_name or getattr(self._local, "last_arm_name", None)
        if not target_name:
            return

        with self._lock:
            arm = next((a for a in self.arms if a.name == target_name), None)
            if arm:
                arm.total_reward += reward
                logger.debug("Bandit reward %.3f registered to arm '%s'", reward, target_name)

    # ── Action: FORGET ────────────────────────────────────────────────────

    def apply_decay(self, graph_store: Any) -> int:
        """
        Heuristic: reduce confidence on all nodes older than `ttl_hours`.
        The graph_store must expose `apply_confidence_decay(factor, min_conf)`.
        Returns the number of nodes decayed.
        """
        if not hasattr(graph_store, "apply_confidence_decay"):
            logger.warning("GraphStore does not support confidence decay; FORGET skipped.")
            return 0
        decay_factor = 0.9
        n = graph_store.apply_confidence_decay(
            factor=decay_factor,
            min_confidence=0.05,
            older_than_hours=self.ttl_hours,
        )
        logger.info("AgentMem.apply_decay → decayed %d nodes (factor=%.2f)", n, decay_factor)
        return n

    # ── Bandit state summary ──────────────────────────────────────────────

    def bandit_summary(self) -> List[Dict[str, Any]]:
        """Snapshot of all arm Q-values — useful for logging/debugging."""
        return [
            {
                "arm": a.name,
                "pulls": a.pulls,
                "q_value": round(a.q_value, 4),
                "ucb": round(a.ucb, 4) if a.pulls else None,
                "depth": a.depth,
                "breadth": a.breadth,
            }
            for a in self.arms
        ]

    # ── Procedure Learning ────────────────────────────────────────────────

    def register_procedure_reward(
        self,
        procedure_id: str,
        reward: float,
        graph_store: Any,
        decay_factor: float = 0.1,
    ) -> float:
        """
        Updates a procedure's confidence based on outcome reward.
        
        new_confidence = old_confidence + decay_factor * reward
        Clamped to [0.0, 1.0].
        
        Returns the new confidence value.
        """
        if not hasattr(graph_store, 'get_procedures_for_entities'):
            logger.warning("GraphStore missing procedural methods; reward skipped.")
            return 0.0

        # Read current confidence via raw Cypher
        if not graph_store.driver:
            return 0.0

        with graph_store.driver.session() as session:
            result = session.run(
                "MATCH (proc:Procedure {id: $pid}) RETURN proc.confidence AS conf",
                pid=procedure_id
            )
            record = result.single()
            if not record:
                logger.warning("Procedure %s not found for reward.", procedure_id)
                return 0.0
            old_conf = record['conf'] or 1.0

        new_conf = max(0.0, min(1.0, old_conf + decay_factor * reward))
        graph_store.update_procedure_confidence(procedure_id, new_conf)
        logger.info(
            "Procedure %s reward=%.2f: conf %.3f → %.3f",
            procedure_id, reward, old_conf, new_conf,
        )
        return new_conf
