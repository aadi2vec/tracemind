pub mod belief;
pub mod store;

pub use belief::{effective_confidence, BeliefStore, ContradictionView, ResolveChoice};
pub use store::{CapturedSignal, GraphStore, TripleDetail};

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
