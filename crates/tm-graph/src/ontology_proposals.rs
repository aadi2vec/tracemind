//! ONT-2 — Statistical ontology proposals.
//!
//! `tm-reflect` scans recurring event clusters and writes proposed new
//! Object Types here. The user accepts or rejects them from Settings.
//! Accepted proposals become real `ontology_object_types` rows with
//! `source = 'statistical'`. Rejected proposals are kept (status =
//! 'rejected') so the proposer can avoid re-suggesting them.
//!
//! Schema is owned by `graph_sprint::MIGRATION_SQL`.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tm_types::{Result, TraceMindError};
use uuid::Uuid;

use crate::ontology_types::OntologyStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    ObjectType,
    LinkType,
}

impl ProposalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalKind::ObjectType => "object_type",
            ProposalKind::LinkType => "link_type",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "object_type" => Some(ProposalKind::ObjectType),
            "link_type" => Some(ProposalKind::LinkType),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Accepted,
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Pending => "pending",
            ProposalStatus::Accepted => "accepted",
            ProposalStatus::Rejected => "rejected",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "accepted" => ProposalStatus::Accepted,
            "rejected" => ProposalStatus::Rejected,
            _ => ProposalStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyProposal {
    pub id: Uuid,
    pub kind: ProposalKind,
    pub name: String,
    /// JSON blob: cluster_id, sample event_ids, top terms, suggested
    /// property schema. Free-form so the proposer can evolve.
    pub evidence: serde_json::Value,
    pub support_count: u32,
    pub status: ProposalStatus,
    pub created_at: String,
    pub decided_at: Option<String>,
}

pub struct ProposalStore;

impl ProposalStore {
    /// Insert a new proposal. If a pending proposal with the same name
    /// already exists, bump its `support_count` instead — this is how
    /// the proposer earns its way past the user's noise threshold.
    pub fn upsert(conn: &Connection, p: &OntologyProposal) -> Result<()> {
        // Look up existing pending row by (kind, name).
        let existing: Option<(String, u32)> = conn
            .query_row(
                "SELECT id, support_count FROM ontology_proposals
                 WHERE proposal_kind = ? AND name = ? AND status = 'pending'",
                params![p.kind.as_str(), p.name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32)),
            )
            .ok();

        if let Some((id, support)) = existing {
            conn.execute(
                "UPDATE ontology_proposals SET
                   support_count = ?, evidence = ?
                 WHERE id = ?",
                params![
                    (support + p.support_count.max(1)) as i64,
                    p.evidence.to_string(),
                    id,
                ],
            )
            .map_err(|e| TraceMindError::Storage(format!("update prop: {e}")))?;
        } else {
            conn.execute(
                "INSERT INTO ontology_proposals
                   (id, proposal_kind, name, evidence, support_count, status,
                    created_at, decided_at)
                 VALUES (?,?,?,?,?,?,?,?)",
                params![
                    p.id.to_string(),
                    p.kind.as_str(),
                    p.name,
                    p.evidence.to_string(),
                    p.support_count as i64,
                    p.status.as_str(),
                    p.created_at,
                    p.decided_at,
                ],
            )
            .map_err(|e| TraceMindError::Storage(format!("insert prop: {e}")))?;
        }
        Ok(())
    }

    pub fn list_pending(conn: &Connection) -> Result<Vec<OntologyProposal>> {
        Self::list_with_status(conn, "pending")
    }

    pub fn list_with_status(conn: &Connection, status: &str) -> Result<Vec<OntologyProposal>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, proposal_kind, name, evidence, support_count, status,
                        created_at, decided_at
                 FROM ontology_proposals
                 WHERE status = ?
                 ORDER BY support_count DESC, created_at ASC",
            )
            .map_err(|e| TraceMindError::Storage(format!("prep prop list: {e}")))?;
        let rows = stmt
            .query_map(params![status], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|e| TraceMindError::Storage(format!("prop list: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            let (id, kind, name, ev, support, st, created, decided) =
                r.map_err(|e| TraceMindError::Storage(e.to_string()))?;
            out.push(OntologyProposal {
                id: Uuid::parse_str(&id)
                    .map_err(|e| TraceMindError::Storage(format!("uuid: {e}")))?,
                kind: ProposalKind::parse(&kind).unwrap_or(ProposalKind::ObjectType),
                name,
                evidence: serde_json::from_str(&ev)
                    .unwrap_or_else(|_| serde_json::json!({})),
                support_count: support as u32,
                status: ProposalStatus::parse(&st),
                created_at: created,
                decided_at: decided,
            });
        }
        Ok(out)
    }

    /// Accept a pending proposal: mark accepted + materialize it.
    /// For `ObjectType` proposals, creates the row in
    /// `ontology_object_types` with `source = 'statistical'`.
    pub fn accept(conn: &Connection, id: Uuid) -> Result<()> {
        let row: (String, String, String) = conn
            .query_row(
                "SELECT proposal_kind, name, evidence FROM ontology_proposals
                 WHERE id = ? AND status = 'pending'",
                params![id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| TraceMindError::Storage(format!("accept lookup: {e}")))?;

        let (kind, name, ev_json) = row;
        let evidence: serde_json::Value =
            serde_json::from_str(&ev_json).unwrap_or_else(|_| serde_json::json!({}));

        match ProposalKind::parse(&kind) {
            Some(ProposalKind::ObjectType) => {
                let schema = evidence
                    .get("property_schema")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                let parent = evidence
                    .get("parent")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                OntologyStore::create_object_type_with_source(
                    conn,
                    &name,
                    parent.as_deref(),
                    schema,
                    "statistical",
                )?;
            }
            Some(ProposalKind::LinkType) => {
                let from = evidence
                    .get("from")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TraceMindError::Storage(
                            "link-type proposal missing 'from' object type".into(),
                        )
                    })?;
                let to = evidence
                    .get("to")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        TraceMindError::Storage(
                            "link-type proposal missing 'to' object type".into(),
                        )
                    })?;
                let card = evidence
                    .get("cardinality")
                    .and_then(|v| v.as_str())
                    .unwrap_or("many_to_many");
                OntologyStore::create_link_type(conn, &name, from, to, card)?;
            }
            None => {
                return Err(TraceMindError::Storage(format!(
                    "unknown proposal kind: {kind}"
                )));
            }
        }

        conn.execute(
            "UPDATE ontology_proposals SET status = 'accepted', decided_at = ?
             WHERE id = ?",
            params![Utc::now().to_rfc3339(), id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("accept update: {e}")))?;
        Ok(())
    }

    /// Reject a pending proposal — keeps the row around so the proposer
    /// can dedupe future suggestions of the same name.
    pub fn reject(conn: &Connection, id: Uuid) -> Result<()> {
        conn.execute(
            "UPDATE ontology_proposals SET status = 'rejected', decided_at = ?
             WHERE id = ? AND status = 'pending'",
            params![Utc::now().to_rfc3339(), id.to_string()],
        )
        .map_err(|e| TraceMindError::Storage(format!("reject: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_sprint::ensure_schema;

    fn fresh() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE memory_views (id TEXT PRIMARY KEY, name TEXT);")
            .unwrap();
        ensure_schema(&c).unwrap();
        c
    }

    fn proposal(name: &str) -> OntologyProposal {
        OntologyProposal {
            id: Uuid::new_v4(),
            kind: ProposalKind::ObjectType,
            name: name.into(),
            evidence: serde_json::json!({"cluster_id": 7, "top_terms": ["rondo", "scout"]}),
            support_count: 1,
            status: ProposalStatus::Pending,
            created_at: Utc::now().to_rfc3339(),
            decided_at: None,
        }
    }

    #[test]
    fn upsert_then_list_pending() {
        let c = fresh();
        ProposalStore::upsert(&c, &proposal("Scout")).unwrap();
        let pending = ProposalStore::list_pending(&c).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "Scout");
    }

    #[test]
    fn duplicate_pending_bumps_support() {
        let c = fresh();
        ProposalStore::upsert(&c, &proposal("Scout")).unwrap();
        ProposalStore::upsert(&c, &proposal("Scout")).unwrap();
        ProposalStore::upsert(&c, &proposal("Scout")).unwrap();
        let pending = ProposalStore::list_pending(&c).unwrap();
        assert_eq!(pending.len(), 1, "should dedupe by name+kind");
        assert!(pending[0].support_count >= 3);
    }

    #[test]
    fn accept_materializes_object_type() {
        let c = fresh();
        let p = proposal("Scout");
        let id = p.id;
        ProposalStore::upsert(&c, &p).unwrap();
        ProposalStore::accept(&c, id).unwrap();

        let ots = OntologyStore::list_object_types(&c).unwrap();
        assert!(ots.iter().any(|o| o.name == "Scout" && o.source == "statistical"));

        // Status moved to accepted.
        let pending = ProposalStore::list_pending(&c).unwrap();
        assert!(pending.is_empty());
        let accepted = ProposalStore::list_with_status(&c, "accepted").unwrap();
        assert_eq!(accepted.len(), 1);
    }

    #[test]
    fn reject_keeps_row_out_of_pending() {
        let c = fresh();
        let p = proposal("Scout");
        let id = p.id;
        ProposalStore::upsert(&c, &p).unwrap();
        ProposalStore::reject(&c, id).unwrap();
        assert!(ProposalStore::list_pending(&c).unwrap().is_empty());
        assert_eq!(
            ProposalStore::list_with_status(&c, "rejected").unwrap().len(),
            1
        );
    }
}
