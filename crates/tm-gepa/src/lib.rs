
// The executable GEPA loop: a policy the retrieval path actually reads, a
// scorer that runs it, a gate that can fail, and a per-instance frontier.
pub mod verifier;
pub mod retrieval_policy;
pub mod scorer;
pub mod instance_pareto;
pub mod reflect;
pub mod optimize;
pub mod curator;

pub use verifier::{AnchorEvalResult, VerifierGate, VerifierConfig};

pub use retrieval_policy::{RetrievalPolicy, SPACE_NAMES};
pub use scorer::{InstanceScore, PolicyScorer, ScoreReport};
pub use instance_pareto::{ArchivedCandidate, InstanceParetoArchive};
pub use reflect::{diagnose, propose, Diagnosis, Proposal};
pub use curator::{curate, CuratorPrior};
pub use optimize::{
    dominant_diagnosis, OptimizeConfig, OptimizeResult, Optimizer, RoundRecord,
};
