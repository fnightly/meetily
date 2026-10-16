//! Centralized, user-configurable storage locations.
//!
//! Historically every storage path in this app was derived directly from
//! `app_data_dir()` at the point of use (database, models, notes, recordings),
//! which made any relocation impossible and scattered the resolution logic.
//! This module is the single place that answers "where does X live?".
//!
//! Resolution rules:
//! - An empty/unset folder field means "use the default under `app_data_dir()`".
//! - Paths are resolved on demand from the app handle, so a change takes effect
//!   for whatever is opened next rather than being cached at startup.
//! - The **database** location cannot move while the pool is open; the caller
//!   must migrate (see [`commands::set_storage_config`]) and the app must reload
//!   the database, which is why the UI surfaces that as an explicit action.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};
// Brings `AppHandle::store()` into scope for the JSON-backed settings file.
use tauri_plugin_store::StoreExt;

/// Name of the JSON store that persists these settings.
pub const STORAGE_CONFIG_STORE: &str = "storage_config.json";
/// Key inside that store.
pub const STORAGE_CONFIG_KEY: &str = "storage";

/// The database file name. Kept identical to the historical value so that an
/// unconfigured install finds its existing database.
pub const DATABASE_FILE_NAME: &str = "meeting_minutes.sqlite";

/// Relative sub directory names used under the app data dir by default.
pub const DEFAULT_MODELS_SUBDIR: &str = "models";
pub const DEFAULT_NOTES_SUBDIR: &str = "notes";
pub const DEFAULT_RECORDINGS_SUBDIR: &str = "recordings";

/// User-configurable storage roots.
///
/// Every field is optional on disk: `None` or an empty string means
/// "use the default". That keeps the stored file forward/backward compatible
/// when new locations are added.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Folder that holds `meeting_minutes.sqlite`.
    #[serde(default)]
    pub database_folder: Option<String>,
    /// Folder that holds downloaded ASR / summary models.
    #[serde(default)]
    pub models_folder: Option<String>,
    /// Folder that holds user meeting notes.
    #[serde(default)]
    pub notes_folder: Option<String>,
    /// Folder that holds recorded audio.
    #[serde(default)]
    pub recordings_folder: Option<String>,
}

/// Fully resolved absolute paths. Never contains an unset entry: each field
/// falls back to the corresponding default under `app_data_dir()`.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedStoragePaths {
    pub database_folder: String,
    pub database_file: String,
    pub models_folder: String,
    pub notes_folder: String,
    pub recordings_folder: String,
    /// The app data dir itself, useful for the UI to show "default".
    pub app_data_dir: String,
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The default app data directory, or an error if the platform cannot supply it.
pub fn app_data_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))
}

/// Default models directory (matches the pre-existing `app_data_dir()/models`).
pub fn default_models_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(DEFAULT_MODELS_SUBDIR))
}

/// Default notes directory.
pub fn default_notes_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join(DEFAULT_NOTES_SUBDIR))
}

/// Read the persisted config, falling back to all-defaults on any problem.
///
/// This is intentionally infallible from the caller's perspective: a missing or
/// corrupt store must never prevent the app from starting, because the default
/// behaviour is exactly what an unconfigured install wants.
pub fn load_storage_config<R: Runtime>(app: &AppHandle<R>) -> StorageConfig {
    let store = match app.store(STORAGE_CONFIG_STORE) {
        Ok(store) => store,
        Err(e) => {
            log::warn!("Failed to access storage config store: {e}; using defaults");
            return StorageConfig::default();
        }
    };

    match store.get(STORAGE_CONFIG_KEY) {
        Some(value) => match serde_json::from_value::<StorageConfig>(value.clone()) {
            Ok(config) => config,
            Err(e) => {
                log::warn!("Failed to deserialize storage config: {e}; using defaults");
                StorageConfig::default()
            }
        },
        None => StorageConfig::default(),
    }
}

/// Persist the config to disk.
pub fn save_storage_config<R: Runtime>(
    app: &AppHandle<R>,
    config: &StorageConfig,
) -> Result<(), String> {
    let store = app
        .store(STORAGE_CONFIG_STORE)
        .map_err(|e| format!("Failed to access storage config store: {e}"))?;

    let value = serde_json::to_value(config)
        .map_err(|e| format!("Failed to serialize storage config: {e}"))?;

    store.set(STORAGE_CONFIG_KEY, value);
    store
        .save()
        .map_err(|e| format!("Failed to persist storage config: {e}"))?;

    log::info!("Persisted storage config: {config:?}");
    Ok(())
}

impl StorageConfig {
    /// Resolve the configured (or default) database folder.
    pub fn database_folder<R: Runtime>(&self, app: &AppHandle<R>) -> Result<PathBuf, String> {
        match non_empty(&self.database_folder) {
            Some(p) => Ok(PathBuf::from(p)),
            None => app_data_dir(app),
        }
    }

    /// Resolve the configured (or default) models folder.
    ///
    /// Changing the models location does **not** move models that were already
    /// downloaded, so a freshly-chosen (and still empty) folder would otherwise
    /// look like "no transcription model installed" and block recording. When the
    /// configured folder holds no models but the default location does, prefer
    /// the location that actually has them and say so in the log.
    ///
    /// This is a safety net, not a substitute for moving the files: once the user
    /// downloads a model into the configured folder, that folder wins.
    pub fn models_folder<R: Runtime>(&self, app: &AppHandle<R>) -> Result<PathBuf, String> {
        let Some(configured) = non_empty(&self.models_folder) else {
            return default_models_dir(app);
        };

        let configured_path = PathBuf::from(configured);
        if dir_contains_models(&configured_path) {
            return Ok(configured_path);
        }

        let fallback = default_models_dir(app)?;
        if configured_path != fallback && dir_contains_models(&fallback) {
            log::warn!(
                "Configured models folder {} has no models, but {} does; using the latter. \
                 Move your models to the configured folder to silence this.",
                configured_path.display(),
                fallback.display()
            );
            return Ok(fallback);
        }

        // Nothing downloaded anywhere yet: honour the user's choice so a new
        // download lands where they asked.
        Ok(configured_path)
    }

    /// Resolve the configured (or default) notes folder.
    pub fn notes_folder<R: Runtime>(&self, app: &AppHandle<R>) -> Result<PathBuf, String> {
        match non_empty(&self.notes_folder) {
            Some(p) => Ok(PathBuf::from(p)),
            None => default_notes_dir(app),
        }
    }

    /// Full path to the SQLite database file.
    pub fn database_file<R: Runtime>(&self, app: &AppHandle<R>) -> Result<PathBuf, String> {
        Ok(self.database_folder(app)?.join(DATABASE_FILE_NAME))
    }

    /// Resolve everything at once, for the settings UI.
    pub fn resolve<R: Runtime>(&self, app: &AppHandle<R>) -> Result<ResolvedStoragePaths, String> {
        Ok(ResolvedStoragePaths {
            database_folder: self.database_folder(app)?.to_string_lossy().to_string(),
            database_file: self.database_file(app)?.to_string_lossy().to_string(),
            models_folder: self.models_folder(app)?.to_string_lossy().to_string(),
            notes_folder: self.notes_folder(app)?.to_string_lossy().to_string(),
            recordings_folder: self.recordings_folder(app)?.to_string_lossy().to_string(),
            app_data_dir: app_data_dir(app)?.to_string_lossy().to_string(),
        })
    }

    /// Resolve the configured (or default) recordings folder.
    ///
    /// Falls back to the recording-preferences default when no explicit storage
    /// value is set, so this unified panel and the existing "recordings folder"
    /// setting cannot disagree about the default.
    ///
    /// `app` is unused here but kept so every resolver shares one signature and
    /// a future per-app default does not change the call sites.
    pub fn recordings_folder<R: Runtime>(&self, _app: &AppHandle<R>) -> Result<PathBuf, String> {
        if let Some(p) = non_empty(&self.recordings_folder) {
            return Ok(PathBuf::from(p));
        }

        Ok(crate::audio::recording_preferences::get_default_recordings_folder())
    }
}

/// File extensions used by the model formats this app downloads.
const MODEL_EXTENSIONS: &[&str] = &["bin", "gguf", "onnx"];

/// Whether a directory contains at least one model file, directly or nested.
///
/// Model layouts differ per engine — Whisper stores `ggml-*.bin` at the root,
/// Parakeet uses a per-model subdirectory, and the built-in summary model is a
/// `.gguf` under `summary/` — so this walks the tree instead of checking one
/// expected shape.
///
/// Depth is bounded to avoid walking a huge unrelated tree if a user points the
/// setting at something like a drive root.
pub fn dir_contains_models(dir: &Path) -> bool {
    const MAX_DEPTH: usize = 4;

    fn walk(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                if depth < MAX_DEPTH && walk(&path, depth + 1) {
                    return true;
                }
                continue;
            }

            let matches_extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| MODEL_EXTENSIONS.iter().any(|known| e.eq_ignore_ascii_case(known)))
                .unwrap_or(false);

            if matches_extension {
                return true;
            }
        }

        false
    }

    if !dir.is_dir() {
        return false;
    }

    walk(dir, 0)
}

/// Create a directory if it does not exist yet.
pub fn ensure_dir(path: &Path) -> Result<(), String> {    if path.exists() {
        if !path.is_dir() {
            return Err(format!("{} exists but is not a directory", path.display()));
        }
        return Ok(());
    }

    std::fs::create_dir_all(path)
        .map_err(|e| format!("Failed to create directory {}: {e}", path.display()))
}

/// Convenience: the models directory, for callers that only need the default
/// when no storage config is available (e.g. very early startup).
pub fn resolve_models_dir<R: Runtime>(app: &AppHandle<R>) -> PathBuf {
    load_storage_config(app)
        .models_folder(app)
        .unwrap_or_else(|e| {
            log::error!("Falling back to app data models dir: {e}");
            app.path()
                .app_data_dir()
                .map(|d| d.join(DEFAULT_MODELS_SUBDIR))
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_MODELS_SUBDIR))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    #[test]
    fn dir_contains_models_detects_whisper_flat_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(&tmp.path().join("ggml-base.bin"), b"x");
        assert!(dir_contains_models(tmp.path()));
    }

    #[test]
    fn dir_contains_models_detects_summary_gguf_in_subdir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(&tmp.path().join("summary").join("Qwen3.5-4B-Q4_K_M.gguf"), b"x");
        assert!(dir_contains_models(tmp.path()));
    }

    #[test]
    fn dir_contains_models_detects_parakeet_onnx_in_subdir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(&tmp.path().join("parakeet").join("encoder.onnx"), b"x");
        assert!(dir_contains_models(tmp.path()));
    }

    #[test]
    fn dir_contains_models_is_false_for_empty_or_model_free_dirs() {
        let empty = tempfile::tempdir().expect("tempdir");
        assert!(!dir_contains_models(empty.path()));

        // The realistic failure mode: the folder exists (so it is "configured")
        // but nothing was ever moved into it.
        let with_unrelated = tempfile::tempdir().expect("tempdir");
        write(&with_unrelated.path().join("readme.txt"), b"x");
        std::fs::create_dir_all(with_unrelated.path().join("parakeet")).expect("create");
        assert!(!dir_contains_models(with_unrelated.path()));
    }

    #[test]
    fn dir_contains_models_is_false_for_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(!dir_contains_models(&tmp.path().join("does-not-exist")));
    }

    #[test]
    fn dir_contains_models_matches_extension_case_insensitively() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(&tmp.path().join("model.BIN"), b"x");
        assert!(dir_contains_models(tmp.path()));
    }

    #[test]
    fn dir_contains_models_stops_at_depth_limit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Deeper than MAX_DEPTH (4): must not be discovered, so pointing the
        // setting at a huge tree cannot turn into an unbounded walk.
        let deep = tmp.path().join("a").join("b").join("c").join("d").join("e");
        write(&deep.join("model.bin"), b"x");
        assert!(!dir_contains_models(tmp.path()));
    }
}

