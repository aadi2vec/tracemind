"""
Human-in-the-Loop Feedback Manager (Phase E)
==============================================
Restored and upgraded from the original MVP.

When a human submits a correction:
1. The correction text is canonicalised → new entities / triplets.
2. Those facts are written to Neo4j (memory write-back).
3. The original ContextTrace is updated with outcome + reward signal.
4. The AgentMemController is notified of the reward.
"""
from __future__ import annotations

import logging
from typing import Optional

from ..memory.episodic_store import EpisodicStore
from ..types.schema import Feedback
from ..processing.ingest import Ingestor
from .agentmem_controller import AgentMemController

logger = logging.getLogger(__name__)


class FeedbackManager:
    def __init__(
        self,
        episodic_store: EpisodicStore,
        ingestor: Ingestor,
        controller: Optional[AgentMemController] = None,
    ):
        self.episodic = episodic_store
        self.ingestor = ingestor
        self.controller = controller

    def ingest_feedback(
        self,
        trace_id: str,
        score: float,
        correction: Optional[str] = None,
        annotator_id: str = "user",
    ) -> dict:
        """
        Process human feedback for a past decision.

        Parameters
        ----------
        trace_id    : ID of the ContextTrace being reviewed.
        score       : Float in [-1.0, 1.0].  >0 = good decision, <0 = bad.
        correction  : Optional free-text correction to ingest as new memory.
        annotator_id: Who provided the feedback.

        Returns
        -------
        Dict with summary of what changed.
        """
        feedback = Feedback(
            trace_id=trace_id,
            score=score,
            correction=correction,
            annotator_id=annotator_id,
        )
        outcome = "success" if score >= 0 else "failure"

        # 1. Update trace in episodic store
        updated = self.episodic.update_outcome(
            trace_id=trace_id,
            outcome=outcome,
            reward=score,
            feedback=feedback,
        )

        # 2. Write correction to memory (if provided)
        new_entities = []
        if correction:
            try:
                result = self.ingestor.ingest(correction)
                new_entities = result.get("entities", [])
                logger.info(
                    "Feedback correction ingested: %d new entities / triplets",
                    len(new_entities),
                )
            except Exception as exc:
                logger.error("Failed to ingest correction text: %s", exc)

        # 3. Notify bandit controller
        if self.controller:
            self.controller.register_reward(score)

        return {
            "trace_updated": updated,
            "outcome": outcome,
            "new_entities": new_entities,
            "reward": score,
        }
