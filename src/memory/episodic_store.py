import json
import os
from typing import List, Optional
from ..types.schema import ContextTrace, Feedback


class EpisodicStore:
    """Append-only episodic memory log with reward & outcome support."""

    def __init__(self, log_path: str = "episodic_memory.jsonl"):
        self.log_path = log_path
        if not os.path.exists(log_path):
            open(log_path, "w").close()

    # ── Write ────────────────────────────────────────────────────────────────

    def log_trace(self, trace: ContextTrace) -> None:
        """Append a completed context trace."""
        with open(self.log_path, "a") as f:
            f.write(trace.model_dump_json() + "\n")

    def update_outcome(
        self,
        trace_id: str,
        outcome: str,
        reward: float,
        feedback: Optional[Feedback] = None,
    ) -> bool:
        """
        Rewrite the matching trace in-place with outcome & reward.
        Returns True if the trace was found and updated.
        """
        if not os.path.exists(self.log_path):
            return False

        lines = open(self.log_path).readlines()
        updated = False
        new_lines = []
        for line in lines:
            try:
                t = ContextTrace.model_validate_json(line)
                if t.trace_id == trace_id:
                    t.outcome = outcome  # type: ignore[assignment]
                    t.reward_signal = reward
                    if feedback:
                        t.feedback = feedback
                    new_lines.append(t.model_dump_json() + "\n")
                    updated = True
                else:
                    new_lines.append(line)
            except Exception:
                new_lines.append(line)

        with open(self.log_path, "w") as f:
            f.writelines(new_lines)
        return updated

    # ── Read ─────────────────────────────────────────────────────────────────

    def get_recent_traces(self, limit: int = 10) -> List[ContextTrace]:
        """Return the last N traces."""
        try:
            lines = open(self.log_path).readlines()
            return [ContextTrace.model_validate_json(l) for l in lines[-limit:]]
        except FileNotFoundError:
            return []

    def get_traces_for_learning(self, limit: int = 100) -> List[ContextTrace]:
        """
        Return traces that have a known outcome — used by the Learning Loop
        to compute bandit rewards.
        """
        try:
            lines = open(self.log_path).readlines()
            results = []
            for line in reversed(lines):
                try:
                    t = ContextTrace.model_validate_json(line)
                    if t.outcome != "unknown":
                        results.append(t)
                    if len(results) >= limit:
                        break
                except Exception:
                    continue
            return list(reversed(results))
        except FileNotFoundError:
            return []

    def __len__(self) -> int:
        try:
            return sum(1 for _ in open(self.log_path))
        except FileNotFoundError:
            return 0
