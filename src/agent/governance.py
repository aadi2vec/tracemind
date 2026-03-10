"""
Governance Layer (Phase 11B)
============================
Access control and audit logging for memory resources.

Provides:
  - GovernancePolicy: controls who can read/execute memories and procedures.
  - AuditLog: records all governance-related events for compliance.

This mirrors Palantir's granular security policies, adapted for
agent-native operation where the "users" are agents, humans, or tools.
"""
from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional

logger = logging.getLogger(__name__)


class Visibility(str, Enum):
    PRIVATE = "private"   # Only the owner can access
    TEAM    = "team"      # Any agent in the same group can access
    PUBLIC  = "public"    # Everyone can access


@dataclass
class ResourceACL:
    """Access control entry for a memory resource."""
    resource_id: str
    resource_type: str     # "entity", "triplet", "procedure", "memory"
    owner_id: str          # Agent or user who created this
    visibility: Visibility = Visibility.PUBLIC
    allowed_ids: List[str] = field(default_factory=list)  # Extra IDs with access


@dataclass
class AuditEntry:
    """A single logged governance event."""
    timestamp: str
    action: str            # "access", "execute", "deny", "create", "deprecate"
    user_id: str
    resource_id: str
    resource_type: str
    detail: str = ""


class GovernancePolicy:
    """
    Controls access to memory resources and logs governance events.

    Usage:
        governance = GovernancePolicy(audit_path="./governance_audit.jsonl")
        if governance.can_access("agent_1", resource_acl):
            ... # proceed with retrieval
    """

    # Schema enforcement: allowed entity types
    ALLOWED_ENTITY_TYPES = {
        "Person", "Organization", "Asset", "Event", "Policy",
        "Location", "Process", "Tool", "Unknown"
    }

    def __init__(self, audit_path: str = "governance_audit.jsonl"):
        self.audit_path = audit_path
        self._acl_store: Dict[str, ResourceACL] = {}

    # ── ACL Management ────────────────────────────────────────────────────

    def register_resource(self, acl: ResourceACL):
        """Registers or updates an ACL for a resource."""
        self._acl_store[acl.resource_id] = acl
        self._log("create", acl.owner_id, acl.resource_id, acl.resource_type,
                  f"visibility={acl.visibility.value}")

    def can_access(self, user_id: str, resource_id: str) -> bool:
        """Checks if a user/agent can read a resource."""
        acl = self._acl_store.get(resource_id)
        if not acl:
            return True  # No ACL registered → open access (default)

        granted = self._check_permission(user_id, acl)
        if not granted:
            self._log("deny", user_id, resource_id, acl.resource_type, "access denied")
        return granted

    def can_execute(self, user_id: str, procedure_id: str) -> bool:
        """Checks if a user/agent can execute a procedure."""
        acl = self._acl_store.get(procedure_id)
        if not acl:
            return True

        granted = self._check_permission(user_id, acl)
        if not granted:
            self._log("deny", user_id, procedure_id, "procedure", "execution denied")
        else:
            self._log("execute", user_id, procedure_id, "procedure", "execution authorized")
        return granted

    def validate_entity_type(self, entity_type: str) -> bool:
        """Validates that an entity type conforms to the schema registry."""
        return entity_type in self.ALLOWED_ENTITY_TYPES

    # ── Internal ──────────────────────────────────────────────────────────

    def _check_permission(self, user_id: str, acl: ResourceACL) -> bool:
        if acl.visibility == Visibility.PUBLIC:
            return True
        if acl.owner_id == user_id:
            return True
        if user_id in acl.allowed_ids:
            return True
        if acl.visibility == Visibility.TEAM:
            return True  # In future: check team membership
        return False

    def _log(self, action: str, user_id: str, resource_id: str,
             resource_type: str, detail: str = ""):
        entry = AuditEntry(
            timestamp=datetime.now().isoformat(),
            action=action,
            user_id=user_id,
            resource_id=resource_id,
            resource_type=resource_type,
            detail=detail,
        )
        try:
            with open(self.audit_path, "a") as f:
                f.write(json.dumps(entry.__dict__) + "\n")
        except Exception as e:
            logger.warning("Governance audit write failed: %s", e)

    # ── Audit Query ───────────────────────────────────────────────────────

    def get_audit_log(self, limit: int = 50) -> List[Dict[str, Any]]:
        """Returns the last N audit entries."""
        if not os.path.exists(self.audit_path):
            return []
        with open(self.audit_path, "r") as f:
            lines = f.readlines()
        entries = [json.loads(line) for line in lines[-limit:]]
        return entries
