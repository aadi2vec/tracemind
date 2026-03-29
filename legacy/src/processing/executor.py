"""
Procedure Executor (Phase 11C)
==============================
Executes procedures step-by-step, supporting dry-run and live modes.

The executor is the "Kinetic Action" engine — it doesn't just recall
what to do, it *does* it. Results feed back into the learning loop
via register_procedure_reward().

Mirrors Palantir's "Action" ontology primitives.
"""
from __future__ import annotations

import logging
import subprocess
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional

from ..types.schema import Procedure, ProcedureStep

logger = logging.getLogger(__name__)


class StepStatus(str, Enum):
    PENDING   = "pending"
    SUCCESS   = "success"
    FAILED    = "failed"
    SKIPPED   = "skipped"
    DRY_RUN   = "dry_run"


@dataclass
class StepResult:
    """Result of executing a single procedure step."""
    step_number: int
    action: str
    status: StepStatus
    output: str = ""
    error: str = ""
    duration_ms: float = 0.0


@dataclass
class ExecutionResult:
    """Aggregate result of a full procedure execution."""
    procedure_id: str
    procedure_name: str
    version: int
    dry_run: bool
    steps: List[StepResult] = field(default_factory=list)
    overall_status: StepStatus = StepStatus.PENDING
    started_at: str = ""
    finished_at: str = ""

    @property
    def success_rate(self) -> float:
        if not self.steps:
            return 0.0
        ok = sum(1 for s in self.steps if s.status in (StepStatus.SUCCESS, StepStatus.DRY_RUN))
        return ok / len(self.steps)

    @property
    def reward(self) -> float:
        """Compute a reward signal from execution outcome.
        +1.0 if all steps succeed, -1.0 if all fail, proportional otherwise.
        """
        return 2.0 * self.success_rate - 1.0

    def to_dict(self) -> Dict[str, Any]:
        return {
            "procedure_id": self.procedure_id,
            "procedure_name": self.procedure_name,
            "version": self.version,
            "dry_run": self.dry_run,
            "overall_status": self.overall_status.value,
            "success_rate": round(self.success_rate, 3),
            "reward": round(self.reward, 3),
            "steps": [
                {
                    "step": s.step_number,
                    "action": s.action,
                    "status": s.status.value,
                    "output": s.output[:200],
                    "error": s.error[:200],
                }
                for s in self.steps
            ],
        }


class ProcedureExecutor:
    """
    Executes procedures, collecting step-level results.

    dry_run=True (default): generates an execution plan without side effects.
    dry_run=False: actually runs shell commands (security-gated).

    Example:
        executor = ProcedureExecutor()
        result = executor.execute(procedure, dry_run=True)
        print(result.to_dict())
    """

    def __init__(self, allowed_commands: Optional[List[str]] = None):
        """
        Args:
            allowed_commands: if set, only shell commands containing one
                              of these prefixes will be allowed in live mode.
                              E.g. ["kubectl", "docker", "systemctl"]
        """
        self.allowed_commands = allowed_commands or []

    def execute(self, procedure: Procedure, dry_run: bool = True) -> ExecutionResult:
        """Execute all steps in a procedure."""
        result = ExecutionResult(
            procedure_id=procedure.id,
            procedure_name=procedure.name,
            version=procedure.version,
            dry_run=dry_run,
            started_at=datetime.now().isoformat(),
        )

        for step in sorted(procedure.steps, key=lambda s: s.step_number):
            step_result = self._execute_step(step, dry_run)
            result.steps.append(step_result)

            # Abort on failure in live mode
            if not dry_run and step_result.status == StepStatus.FAILED:
                logger.warning(
                    "Procedure '%s' step %d failed, aborting remaining steps.",
                    procedure.name, step.step_number
                )
                # Mark remaining steps as skipped
                for remaining in procedure.steps:
                    if remaining.step_number > step.step_number:
                        result.steps.append(StepResult(
                            step_number=remaining.step_number,
                            action=remaining.action,
                            status=StepStatus.SKIPPED,
                        ))
                break

        result.finished_at = datetime.now().isoformat()
        result.overall_status = (
            StepStatus.SUCCESS
            if all(s.status in (StepStatus.SUCCESS, StepStatus.DRY_RUN) for s in result.steps)
            else StepStatus.FAILED
        )

        logger.info(
            "Procedure '%s' v%d %s → %s (reward=%.2f)",
            procedure.name, procedure.version,
            "DRY RUN" if dry_run else "LIVE",
            result.overall_status.value, result.reward,
        )
        return result

    def _execute_step(self, step: ProcedureStep, dry_run: bool) -> StepResult:
        """Execute or simulate a single step."""
        import time
        start = time.time()

        if dry_run:
            return StepResult(
                step_number=step.step_number,
                action=step.action,
                status=StepStatus.DRY_RUN,
                output=f"[DRY RUN] Would execute: {step.action}",
                duration_ms=0.0,
            )

        # Live execution — only shell commands for now
        if not self._is_allowed(step.action):
            return StepResult(
                step_number=step.step_number,
                action=step.action,
                status=StepStatus.FAILED,
                error=f"Command not in allowed list: {step.action}",
                duration_ms=(time.time() - start) * 1000,
            )

        try:
            proc = subprocess.run(
                step.action,
                shell=True,
                capture_output=True,
                text=True,
                timeout=30,
            )
            elapsed = (time.time() - start) * 1000
            if proc.returncode == 0:
                return StepResult(
                    step_number=step.step_number,
                    action=step.action,
                    status=StepStatus.SUCCESS,
                    output=proc.stdout[:500],
                    duration_ms=elapsed,
                )
            else:
                return StepResult(
                    step_number=step.step_number,
                    action=step.action,
                    status=StepStatus.FAILED,
                    output=proc.stdout[:200],
                    error=proc.stderr[:200],
                    duration_ms=elapsed,
                )
        except subprocess.TimeoutExpired:
            return StepResult(
                step_number=step.step_number,
                action=step.action,
                status=StepStatus.FAILED,
                error="Timeout after 30s",
                duration_ms=30000.0,
            )
        except Exception as e:
            return StepResult(
                step_number=step.step_number,
                action=step.action,
                status=StepStatus.FAILED,
                error=str(e),
                duration_ms=(time.time() - start) * 1000,
            )

    def _is_allowed(self, action: str) -> bool:
        """Check if an action is whitelisted for live execution."""
        if not self.allowed_commands:
            return False  # No whitelist = deny all live execution
        return any(action.strip().startswith(prefix) for prefix in self.allowed_commands)
