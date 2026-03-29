"""Tests for Governance Layer (Phase 11B)."""
import os
import unittest
import tempfile
from src.agent.governance import GovernancePolicy, ResourceACL, Visibility


class TestGovernancePolicy(unittest.TestCase):
    def setUp(self):
        self.audit_file = tempfile.NamedTemporaryFile(suffix=".jsonl", delete=False)
        self.audit_file.close()
        self.gov = GovernancePolicy(audit_path=self.audit_file.name)

    def tearDown(self):
        os.unlink(self.audit_file.name)

    def test_public_access_granted(self):
        acl = ResourceACL(
            resource_id="ent-1", resource_type="entity",
            owner_id="agent_1", visibility=Visibility.PUBLIC
        )
        self.gov.register_resource(acl)
        self.assertTrue(self.gov.can_access("anyone", "ent-1"))

    def test_private_access_owner(self):
        acl = ResourceACL(
            resource_id="proc-1", resource_type="procedure",
            owner_id="agent_1", visibility=Visibility.PRIVATE
        )
        self.gov.register_resource(acl)
        self.assertTrue(self.gov.can_access("agent_1", "proc-1"))
        self.assertFalse(self.gov.can_access("agent_2", "proc-1"))

    def test_private_access_allowed_list(self):
        acl = ResourceACL(
            resource_id="proc-2", resource_type="procedure",
            owner_id="agent_1", visibility=Visibility.PRIVATE,
            allowed_ids=["agent_3"]
        )
        self.gov.register_resource(acl)
        self.assertTrue(self.gov.can_access("agent_3", "proc-2"))
        self.assertFalse(self.gov.can_access("agent_4", "proc-2"))

    def test_can_execute_logs_action(self):
        acl = ResourceACL(
            resource_id="proc-3", resource_type="procedure",
            owner_id="agent_1", visibility=Visibility.PUBLIC
        )
        self.gov.register_resource(acl)
        self.assertTrue(self.gov.can_execute("agent_2", "proc-3"))
        logs = self.gov.get_audit_log()
        self.assertTrue(any(e["action"] == "execute" for e in logs))

    def test_unregistered_resource_defaults_open(self):
        self.assertTrue(self.gov.can_access("anyone", "unknown-id"))

    def test_validate_entity_type(self):
        self.assertTrue(self.gov.validate_entity_type("Person"))
        self.assertTrue(self.gov.validate_entity_type("Asset"))
        self.assertFalse(self.gov.validate_entity_type("Banana"))

    def test_audit_log_written(self):
        acl = ResourceACL(
            resource_id="e1", resource_type="entity",
            owner_id="agent_1", visibility=Visibility.PRIVATE
        )
        self.gov.register_resource(acl)
        self.gov.can_access("agent_2", "e1")  # Should be denied
        logs = self.gov.get_audit_log()
        self.assertGreaterEqual(len(logs), 2)  # create + deny
        self.assertTrue(any(e["action"] == "deny" for e in logs))


if __name__ == "__main__":
    unittest.main()
