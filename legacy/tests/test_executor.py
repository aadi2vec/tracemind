"""Tests for Procedure Executor (Phase 11C)."""
import unittest
from src.types.schema import Procedure, ProcedureStep
from src.processing.executor import ProcedureExecutor, StepStatus


class TestProcedureExecutorDryRun(unittest.TestCase):
    def setUp(self):
        self.executor = ProcedureExecutor()
        self.procedure = Procedure(
            name="Restart Nginx",
            description="How to restart Nginx on Linux.",
            steps=[
                ProcedureStep(step_number=1, action="sudo systemctl stop nginx",
                              expected_outcome="Service stops"),
                ProcedureStep(step_number=2, action="sudo systemctl start nginx",
                              expected_outcome="Service starts"),
            ],
            trigger_entities=["Nginx", "Server"],
            version=1,
        )

    def test_dry_run_returns_plan(self):
        result = self.executor.execute(self.procedure, dry_run=True)
        self.assertTrue(result.dry_run)
        self.assertEqual(result.overall_status, StepStatus.SUCCESS)
        self.assertEqual(len(result.steps), 2)
        for step in result.steps:
            self.assertEqual(step.status, StepStatus.DRY_RUN)
            self.assertIn("DRY RUN", step.output)

    def test_dry_run_reward_is_positive(self):
        result = self.executor.execute(self.procedure, dry_run=True)
        self.assertEqual(result.reward, 1.0)  # All steps "succeed" in dry run

    def test_to_dict_format(self):
        result = self.executor.execute(self.procedure, dry_run=True)
        d = result.to_dict()
        self.assertIn("procedure_id", d)
        self.assertIn("steps", d)
        self.assertEqual(d["version"], 1)


class TestProcedureExecutorLive(unittest.TestCase):
    def test_no_whitelist_denies_all(self):
        executor = ProcedureExecutor(allowed_commands=[])
        proc = Procedure(
            name="Test",
            description="Test live execution.",
            steps=[ProcedureStep(step_number=1, action="echo hello")],
            trigger_entities=["Test"],
        )
        result = executor.execute(proc, dry_run=False)
        self.assertEqual(result.overall_status, StepStatus.FAILED)
        self.assertIn("not in allowed list", result.steps[0].error)

    def test_whitelisted_command_succeeds(self):
        executor = ProcedureExecutor(allowed_commands=["echo"])
        proc = Procedure(
            name="Echo Test",
            description="Test echo execution.",
            steps=[ProcedureStep(step_number=1, action="echo hello world")],
            trigger_entities=["Test"],
        )
        result = executor.execute(proc, dry_run=False)
        self.assertEqual(result.overall_status, StepStatus.SUCCESS)
        self.assertIn("hello world", result.steps[0].output)

    def test_failed_step_aborts(self):
        executor = ProcedureExecutor(allowed_commands=["echo", "false"])
        proc = Procedure(
            name="Abort Test",
            description="Test abort on failure.",
            steps=[
                ProcedureStep(step_number=1, action="false"),  # exits with code 1
                ProcedureStep(step_number=2, action="echo should not run"),
            ],
            trigger_entities=["Test"],
        )
        result = executor.execute(proc, dry_run=False)
        self.assertEqual(result.overall_status, StepStatus.FAILED)
        self.assertEqual(result.steps[1].status, StepStatus.SKIPPED)

    def test_reward_for_mixed_outcome(self):
        executor = ProcedureExecutor(allowed_commands=["echo", "false"])
        proc = Procedure(
            name="Mixed",
            description="Mixed outcome.",
            steps=[
                ProcedureStep(step_number=1, action="false"),
                ProcedureStep(step_number=2, action="echo ok"),
            ],
            trigger_entities=["Test"],
        )
        result = executor.execute(proc, dry_run=False)
        self.assertLess(result.reward, 1.0)  # Not perfect


if __name__ == "__main__":
    unittest.main()
