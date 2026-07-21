pub mod algebra;
pub mod belief;
pub mod community;
pub mod context;
pub mod deny_list;
pub mod entity_resolve;
pub mod event_graph;
pub mod feedback_fabric;
pub mod graph_sprint;
pub mod labeler;
pub mod maintenance;
pub mod memory_view;
pub mod moc;
pub mod ontology;
pub mod ontology_proposals;
pub mod ontology_types;
pub mod pending_relations;
pub mod portable_export;
pub mod salience;
pub mod store;
pub mod thread_graph;

pub use community::{
    community_label_map, recompute_communities, recompute_community_labels,
    set_community_label, top_community_samples, CommunityAssignment, CommunityLabel,
    CommunityLabelStats, CommunitySample, CommunityStats,
};
pub use salience::{
    ensure_schema as ensure_salience_schema, recency_decay_at_age, recompute as recompute_salience,
    score_for as salience_for, top_k as salience_top_k, SalienceRow, SalienceStats,
    RECENCY_HALF_LIFE_DAYS, SATURATION_DEGREE,
};

pub use belief::{effective_confidence, BeliefStore, ContradictionView, ResolveChoice};
// Re-export so downstream crates (tm-tauri, tm-mcp) can pattern-match
// the result of `GraphStore::belief_status_for` without taking a direct
// dependency on `tm-tms`. The status type itself lives in `tm-tms` —
// this is a thin pass-through.
pub use tm_tms::BeliefStatus;
pub use context::{
    ActiveContext, Context, NegativeSignal, PositiveSignal,
};
pub use deny_list::{
    ContextPairRule, DenyList, EntityPairRule, StrikeOutcome,
    DEFAULT_STRIKE_THRESHOLD, SCHEMA_VERSION as DENY_LIST_SCHEMA_VERSION,
};
pub use entity_resolve::{resolve_in_text, EntityMention};
pub use labeler::{
    label_clusters, ClusterLabel, LabelTerm, LabelerConfig, BIGRAM_BOOST, DEFAULT_MIN_TOKEN_LEN,
    DEFAULT_TOP_K,
};
pub use maintenance::{
    clean_ephemeral, reset_all, storage_stats, truncate_traces, vacuum_all, CleanupReport,
    FileStat, StorageStats,
};
pub use memory_view::{
    MemberKind, MemberType, MemoryView, ViewFilter, ViewMember, VIEW_SCHEMA_VERSION,
};
pub use moc::{generate_moc_for_cluster, heuristic_moc_groups, upsert_moc, MocUpsert};
pub use ontology::classify as classify_ontology;
pub use pending_relations::{
    PendingRelation, PendingStatus, ACCEPT_THRESHOLD as PENDING_ACCEPT_THRESHOLD,
    PENDING_FLOOR, SCHEMA_VERSION as PENDING_RELATIONS_SCHEMA_VERSION,
};
pub use store::{Backlink, CapturedSignal, GraphStore, PendingRouteOutcome, TripleDetail};

pub use algebra::{Algebra, GraphExpr, SetOp};
pub use event_graph::{EventEdge, EventEdgeKind, EventGraphStore, EventNode, EventNodeKind};
pub use graph_sprint::{ensure_schema as ensure_sprint_graph_schema, GRAPH_SPRINT_SCHEMA_VERSION};
pub use ontology_proposals::{
    OntologyProposal, ProposalKind, ProposalStatus, ProposalStore,
};
pub use ontology_types::{
    LinkType, ObjectType, OntologyStore, ObjectTypeAssignment, TypeCheckOutcome,
};
pub use portable_export::{export_portable, PortableEdge, PortableGraph, PortableNode};
pub use thread_graph::{Thread, ThreadGraph, ThreadGraphStore, ThreadSource};
pub use feedback_fabric::{
    init_schema as init_feedback_schema,
    record_signal as record_feedback_signal,
    signals_for_hook,
    recent_signals_by_class,
    verb_affinity,
};

use tm_types::Predicate;
use uuid::Uuid;

/// KG-R1 inspired schema-agnostic graph action space.
///
/// These 4 actions are provably sufficient to traverse any reasoning path
/// in a directed knowledge graph (KG-R1, arXiv:2509.26383). The constrained
/// action space makes future learned traversal policies tractable — even a
/// 3B model can learn effective graph navigation with just these operations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GraphAction {
    /// What predicates leave this entity? → Vec<(Predicate, target_id)>
    GetOutgoingPredicates(Uuid),
    /// What predicates point to this entity? → Vec<(Predicate, source_id)>
    GetIncomingPredicates(Uuid),
    /// Follow a specific predicate forward → Vec<Entity>
    FollowPredicate(Uuid, Predicate),
    /// Reverse-follow a predicate → Vec<Entity>
    ReverseFollow(Uuid, Predicate),
}

impl GraphAction {
    /// Execute this action against a GraphStore.
    pub fn execute(&self, graph: &GraphStore) -> tm_types::Result<GraphActionResult> {
        match self {
            GraphAction::GetOutgoingPredicates(id) => {
                let pairs = graph.outgoing_predicates(*id)?;
                Ok(GraphActionResult::PredicatePairs(pairs))
            }
            GraphAction::GetIncomingPredicates(id) => {
                let pairs = graph.incoming_predicates(*id)?;
                Ok(GraphActionResult::PredicatePairs(pairs))
            }
            GraphAction::FollowPredicate(id, pred) => {
                let entities = graph.follow_predicate(*id, pred)?;
                Ok(GraphActionResult::Entities(entities))
            }
            GraphAction::ReverseFollow(id, pred) => {
                let entities = graph.reverse_follow(*id, pred)?;
                Ok(GraphActionResult::Entities(entities))
            }
        }
    }
}

/// Result of executing a GraphAction.
#[derive(Debug, Clone)]
pub enum GraphActionResult {
    PredicatePairs(Vec<(Predicate, Uuid)>),
    Entities(Vec<tm_types::Entity>),
}
