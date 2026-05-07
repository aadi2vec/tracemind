//! Persistence for [`OutcomeModel`] — JSON on disk.
//!
//! The model is small (a few KB) and the format needs to be readable
//! for debugging + manual inspection. JSON is the right call here.
//! When a future version introduces a real neural net, switch to a
//! binary tensor format (safetensors) and bump `SCHEMA_VERSION`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::predictor::{OutcomeModel, SCHEMA_VERSION};

/// Default canonical filename inside the data directory.
pub const DEFAULT_FILENAME: &str = "world_model.json";

/// Resolve `<TM_DATA_DIR or ~/.tracemind>/world_model.json`. Mirrors
/// the resolution logic used by `tm-answer::default_model_path()` so
/// the world model lives next to the LLM weights.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("TM_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join(".tracemind")))
        .unwrap_or_else(|| PathBuf::from(".tracemind"));
    base.join(DEFAULT_FILENAME)
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "world-model schema mismatch: file has v{found}, this build expects v{expected} \
         (run `tracemind world train` to retrain into the new format)"
    )]
    SchemaMismatch { found: u32, expected: u32 },
}

/// Save the model atomically: write to `<path>.tmp` then rename.
/// Prevents a partial write from leaving a corrupt JSON file behind.
pub fn save(model: &OutcomeModel, path: &Path) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(model)?;
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Load a model. Returns `Ok(None)` if the file doesn't exist —
/// caller decides whether to bootstrap a fresh untrained model.
pub fn load(path: &Path) -> Result<Option<OutcomeModel>, StoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let model: OutcomeModel = serde_json::from_slice(&bytes)?;
    if model.schema_version != SCHEMA_VERSION {
        return Err(StoreError::SchemaMismatch {
            found: model.schema_version,
            expected: SCHEMA_VERSION,
        });
    }
    Ok(Some(model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::TagVocab;

    #[test]
    fn save_load_round_trip_mlp() {
        use crate::predictor::Architecture;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wm_mlp.json");

        let mut model = OutcomeModel::fresh_with(
            TagVocab::default(),
            Architecture::Mlp { hidden_dim: 6 },
        );
        model.n_train_examples = 3;
        model.b1[2] = 0.7;
        if let Some(ref mut w2) = model.w2 {
            w2[5] = -0.25;
        }

        save(&model, &path).unwrap();
        let loaded = load(&path).unwrap().expect("file should exist");
        assert_eq!(loaded.architecture, Architecture::Mlp { hidden_dim: 6 });
        assert!((loaded.b1[2] - 0.7).abs() < 1e-6);
        assert_eq!(loaded.w2.as_ref().unwrap()[5], -0.25);
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wm.json");

        let mut model = OutcomeModel::fresh(TagVocab::default());
        model.n_train_examples = 7;
        model.b1[0] = 0.42;

        save(&model, &path).unwrap();
        let loaded = load(&path).unwrap().expect("file should exist");
        assert_eq!(loaded.n_train_examples, 7);
        assert!((loaded.b1[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let loaded = load(&path).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn schema_mismatch_errors_loudly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");

        // Forge a future-version file.
        let mut model = OutcomeModel::fresh(TagVocab::default());
        model.schema_version = SCHEMA_VERSION + 1;
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        fs::write(&path, bytes).unwrap();

        match load(&path) {
            Err(StoreError::SchemaMismatch { found, expected }) => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(expected, SCHEMA_VERSION);
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }
}
