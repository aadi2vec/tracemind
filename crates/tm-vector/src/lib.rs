pub mod embed;
pub mod bge_m3;
pub mod space;
pub mod composed_index;

pub use embed::{Embedder, EmbedModel};
pub use bge_m3::BgeM3DenseModel;
pub use space::{
    Space, MemoryMeta, TextSpace, RecencySpace, ConfidenceSpace,
    EntityTypeSpace, HostSessionSpace,
};
pub use composed_index::{ComposedIndex, VerbWeights, default_verb_weights};

// ---------------------------------------------------------------------------
// Dimension constants
// ---------------------------------------------------------------------------

/// Embedding dimension for all fastembed-backed models (BGE-small, MiniLM, Arctic).
///
/// The SQLite `vec_items` table is created with this width by default.
/// Changing the active model's dimension requires a schema migration —
/// see `migrate_dim()` (not yet implemented; tracked as a future task).
pub const EMBED_DIM_384: usize = 384;

/// Embedding dimension for BGE-M3 dense path.
///
/// A database created with [`EMBED_DIM_384`] is **not** compatible with this
/// dimension. Operators switching to BGE-M3 must either:
///  1. Create a fresh data directory, or
///  2. Wait for `migrate_dim()` (planned for the Q4 BGE-M3 full wire-up).
pub const EMBED_DIM_768: usize = 768;
