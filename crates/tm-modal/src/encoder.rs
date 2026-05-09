//! Encoder trait and registry for cross-modal embedding.
//!
//! Each modality gets its own [`ModalEncoder`] that produces a raw embedding
//! and can project it into a shared 384-dim space for cross-modal comparison.

use std::collections::HashMap;
use thiserror::Error;

use crate::types::Modality;

/// Errors produced by modal encoders.
#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("unsupported format for this encoder")]
    UnsupportedFormat,

    #[error("model not loaded — call load() first or enable auto-download")]
    ModelNotLoaded,

    #[error("inference failed: {0}")]
    InferenceFailed(String),
}

/// Trait for encoding raw data from a single modality into embeddings.
pub trait ModalEncoder: Send + Sync {
    /// Which modality this encoder handles.
    fn modality(&self) -> Modality;

    /// Encode raw bytes into a modality-native embedding vector.
    fn encode(&self, data: &[u8]) -> Result<Vec<f32>, EncoderError>;

    /// Project a raw embedding into the shared cross-modal space.
    fn project(&self, raw_embedding: &[f32]) -> Result<Vec<f32>, EncoderError>;

    /// Dimensionality of the raw (modality-native) embedding.
    fn embedding_dim(&self) -> usize;

    /// Dimensionality of the shared projected space (default 384).
    fn projected_dim(&self) -> usize {
        384
    }
}

/// Stub text encoder — returns [`EncoderError::ModelNotLoaded`].
///
/// The real implementation will delegate to BGE-small via fastembed.
pub struct TextEncoder;

impl ModalEncoder for TextEncoder {
    fn modality(&self) -> Modality {
        Modality::Text
    }

    fn encode(&self, _data: &[u8]) -> Result<Vec<f32>, EncoderError> {
        Err(EncoderError::ModelNotLoaded)
    }

    fn project(&self, _raw_embedding: &[f32]) -> Result<Vec<f32>, EncoderError> {
        Err(EncoderError::ModelNotLoaded)
    }

    fn embedding_dim(&self) -> usize {
        384
    }
}

/// Registry mapping modalities to their encoders.
pub struct EncoderRegistry {
    encoders: HashMap<Modality, Box<dyn ModalEncoder>>,
}

impl EncoderRegistry {
    pub fn new() -> Self {
        Self {
            encoders: HashMap::new(),
        }
    }

    /// Register an encoder for the given modality (replaces any existing one).
    pub fn register(&mut self, encoder: Box<dyn ModalEncoder>) {
        let m = encoder.modality();
        self.encoders.insert(m, encoder);
    }

    /// Encode raw data using the encoder registered for `modality`.
    pub fn encode(&self, modality: Modality, data: &[u8]) -> Result<Vec<f32>, EncoderError> {
        let enc = self
            .encoders
            .get(&modality)
            .ok_or(EncoderError::UnsupportedFormat)?;
        enc.encode(data)
    }

    /// Encode raw data and project into the shared embedding space.
    pub fn encode_and_project(
        &self,
        modality: Modality,
        data: &[u8],
    ) -> Result<(Vec<f32>, Vec<f32>), EncoderError> {
        let enc = self
            .encoders
            .get(&modality)
            .ok_or(EncoderError::UnsupportedFormat)?;
        let raw = enc.encode(data)?;
        let proj = enc.project(&raw)?;
        Ok((raw, proj))
    }
}

impl Default for EncoderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_encoder_returns_model_not_loaded() {
        let enc = TextEncoder;
        assert_eq!(enc.modality(), Modality::Text);
        assert_eq!(enc.embedding_dim(), 384);
        assert_eq!(enc.projected_dim(), 384);

        let err = enc.encode(b"hello world").unwrap_err();
        assert!(matches!(err, EncoderError::ModelNotLoaded));

        let err = enc.project(&[0.1, 0.2]).unwrap_err();
        assert!(matches!(err, EncoderError::ModelNotLoaded));
    }

    #[test]
    fn registry_missing_modality_returns_unsupported() {
        let reg = EncoderRegistry::new();
        let err = reg.encode(Modality::Image, b"png-bytes").unwrap_err();
        assert!(matches!(err, EncoderError::UnsupportedFormat));
    }

    #[test]
    fn registry_register_and_encode() {
        let mut reg = EncoderRegistry::new();
        reg.register(Box::new(TextEncoder));

        // TextEncoder is a stub so it will return ModelNotLoaded
        let err = reg.encode(Modality::Text, b"hello").unwrap_err();
        assert!(matches!(err, EncoderError::ModelNotLoaded));

        // Image is still unregistered
        let err = reg.encode(Modality::Image, b"png").unwrap_err();
        assert!(matches!(err, EncoderError::UnsupportedFormat));
    }

    /// A fake encoder that actually returns embeddings, for testing the registry plumbing.
    struct FakeEncoder;

    impl ModalEncoder for FakeEncoder {
        fn modality(&self) -> Modality {
            Modality::Code
        }

        fn encode(&self, data: &[u8]) -> Result<Vec<f32>, EncoderError> {
            Ok(vec![data.len() as f32; 64])
        }

        fn project(&self, raw_embedding: &[f32]) -> Result<Vec<f32>, EncoderError> {
            // Simple identity truncation to projected_dim
            let dim = self.projected_dim();
            let mut proj = raw_embedding.to_vec();
            proj.resize(dim, 0.0);
            Ok(proj)
        }

        fn embedding_dim(&self) -> usize {
            64
        }
    }

    #[test]
    fn registry_encode_and_project_with_fake() {
        let mut reg = EncoderRegistry::new();
        reg.register(Box::new(FakeEncoder));

        let (raw, proj) = reg
            .encode_and_project(Modality::Code, b"fn main() {}")
            .unwrap();
        assert_eq!(raw.len(), 64);
        assert_eq!(proj.len(), 384);
        // Each element should be the byte-length of the input
        assert_eq!(raw[0], 12.0);
    }
}
