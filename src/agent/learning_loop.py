"""
Self-Improvement Learning Loop (Phase D)
==========================================
Runs periodically in a background thread.

Phase 1 (active now): TTL-based confidence decay via AgentMemController.
Phase 2 (active now): Bandit arm update from episodic trace outcomes.
Phase 3 (future):     Offline GRPO / RLVR-style batch optimization.

Usage::
    loop = LearningLoop(controller, episodic_store, graph_store)
    loop.start(interval_s=300)   # runs every 5 min
    ...
    loop.stop()
"""
from __future__ import annotations

import logging
import threading
import time
from typing import Any, Optional

from .agentmem_controller import AgentMemController
from ..memory.episodic_store import EpisodicStore

logger = logging.getLogger(__name__)


class LearningLoop:
    def __init__(
        self,
        controller: AgentMemController,
        episodic_store: EpisodicStore,
        graph_store: Any,
        interval_s: float = 300.0,
    ):
        self.controller = controller
        self.episodic = episodic_store
        self.graph_store = graph_store
        self.interval_s = interval_s
        self._thread: Optional[threading.Thread] = None
        self._stop_event = threading.Event()

    # ── Public API ────────────────────────────────────────────────────────

    def start(self, interval_s: Optional[float] = None) -> None:
        if interval_s:
            self.interval_s = interval_s
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._loop, daemon=True, name="LearningLoop")
        self._thread.start()
        logger.info("LearningLoop started (interval=%.0fs)", self.interval_s)

    def stop(self) -> None:
        self._stop_event.set()
        if self._thread:
            self._thread.join(timeout=5)
        logger.info("LearningLoop stopped.")

    def run_offline_update(self) -> dict:
        """Single synchronous update — useful for testing or manual triggering."""
        return self._update_cycle()

    # ── Internal loop ─────────────────────────────────────────────────────

    def _loop(self) -> None:
        while not self._stop_event.is_set():
            try:
                stats = self._update_cycle()
                logger.info("LearningLoop cycle complete: %s", stats)
            except Exception as exc:
                logger.error("LearningLoop error: %s", exc, exc_info=True)
            self._stop_event.wait(self.interval_s)

    def _update_cycle(self) -> dict:
        stats: dict = {}

        # ─ Phase 1: TTL decay ────────────────────────────────────────────
        decayed = self.controller.apply_decay(self.graph_store)
        stats["nodes_decayed"] = decayed

        # ─ Phase 2: Bandit updates from episodic outcomes ────────────────
        traces = self.episodic.get_traces_for_learning(limit=50)
        rewards_applied = 0
        for trace in traces:
            if trace.reward_signal != 0.0:
                self.controller.register_reward(trace.reward_signal, arm_name=trace.retrieval_arm)
                rewards_applied += 1
        stats["rewards_applied"] = rewards_applied

        # ─ Summary ───────────────────────────────────────────────────────
        stats["bandit_arms"] = self.controller.bandit_summary()
        stats["episodic_size"] = len(self.episodic)
        return stats
