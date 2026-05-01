//! TraceMind world model — v0.
//!
//! `f_outcome`: given a fresh [`tm_intent::Commitment`], predict the
//! distribution over outcome polarities (better / as_expected / worse /
//! mixed) implied by the user's *own* prior commitment+outcome history.
//!
//! Honest scope (see `docs/INTENT_SYSTEM.md` §7):
//! - **v0** (this crate): multinomial logistic regression over
//!   engineered metadata features (stakes / time-band / horizon / kind /
//!   tag presence). No text embedding. Trains in milliseconds on N=10
//!   to N=1000 commitments. Lives in JSON at
//!   `~/.tracemind/world_model.json`.
//! - **v1** (next): add a BGE 384-dim topic embedding as a feature
//!   block; same linear head.
//! - **v2**: small MLP layer on the topic embedding + contrastive
//!   training across commitments that share an outcome polarity.
//!
//! Why ship v0 first: the §7 contract says the world model must
//! (a) be local-only, (b) be trained on the user's data only, and
//! (c) carry no aggregate priors. A linear classifier on metadata
//! satisfies all three immediately and is honest about what it can
//! and cannot do — it isn't pretending to be a brain. The §6 surfaces
//! (L1 prefetch, L2 pattern surfacing, L3 recommendation) all consume
//! the same [`OutcomePrediction`] shape, so v0 unlocks them today.

pub mod features;
pub mod predictor;
pub mod store;
pub mod trainer;
pub mod types;

pub use features::{extract, feature_dim, normalize_tag, TagVocab, FIXED_FEATURES};
pub use predictor::{
    explain_top_k, softmax, FeatureContribution, OutcomeModel, N_CLASSES, SCHEMA_VERSION,
};
pub use store::{default_path, load, save, StoreError, DEFAULT_FILENAME};
pub use trainer::{from_pairs, train, Example, TrainReport, TrainerConfig};
pub use types::{OutcomePrediction, PolarityClass, PolarityDist};
