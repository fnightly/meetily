//! Tauri command surface for the unified storage-location settings.
//!
//! The one non-obvious behaviour lives in [`set_storage_config`]: the SQLite
//! database is opened once during app setup and its pool is held in managed
//! state, so a database relocation cannot be applied to the running process.
//! Instead we snapshot the database to the new folder now (so nothing is lost
//! if the user never restarts) and tell the caller that a restart is required
//! for the change to take effect.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::DialogExt;

use super::config::{
    self, ensure_dir, load_storage_config, save_storage_config, ResolvedStoragePaths, StorageConfig,
    DEFAULT_MODELS_SUBDIR, DEFAULT_NOTES_SUBDIR, DEFAULT_RECORDINGS_SUBDIR, DATABASE_FILE_NAME,
};

/// Result of a database relocation attempt.
#[derive(Debug, Serialize)]
pub struct DatabaseMigrationOutcome {
    /// Where the database should live after the next start.
    pub destination: String,
    /// Whether a copy was actually performed.
    pub copied: bool,
    /// True when the running app is still using the old database location, so
    /// the UI must tell the user to restart.
    pub restart_required: bool,
    /// Human-readable detail for the UI / logs.
    pub detail: String,
}

/// What the UI needs to render the storage panel.
#[derive(Debug, Serialize)]
pub struct StorageSettingsView {
    pub config: StorageConfig,
    pub resolved: ResolvedStoragePaths,
    /// Where the database is actually open right now (equals
    /// `resolved.database_file` unless the user moved it and has not restarted).
    pub active_database_file: String,
    pub restart_required: bool,
    /// The models folder actually in use. Differs from
    /// `config.models_folder` when the configured folder is empty but the
    /// default one holds downloaded models.
    pub effective_models_folder: String,
    /// True when the configured models folder exists but has no models and the
    /// default folder is being used instead.
    pub models_folder_fallback: bool,
}

#[tauri::command]
pub async fn get_storage_config<R: Runtime>(app: AppHandle<R>) -> Result<StorageSettingsView, String> {
    let cfg = load_storage_config(&app);
    let resolved = cfg.resolve(&app)?;

    // `DatabaseManager` records the file its pool is actually open against, so
    // the UI can tell "configured" apart from "in effect".
    let active_database_file = app
        .try_state::<crate::state::AppState>()
        .map(|state| state.db_manager.path().to_string_lossy().to_string())
        .unwrap_or_else(|| resolved.database_file.clone());

    // Same idea for models: `models_folder()` may fall back to the default
    // location, and the UI must show where models really are so the user is not
    // left wondering why an empty folder appears to have models.
    let effective_models_folder = cfg.models_folder(&app)?;

    // Compare against the *stored* value: `resolved.models_folder` deliberately
    // reports what was configured, while `effective_models_folder` reports what
    // is in use, and those differ exactly when the fallback fired.
    let configured_models = non_empty_opt(&cfg.models_folder)
        .map(PathBuf::from)
        .unwrap_or(config::default_models_dir(&app)?);
    let models_folder_fallback = effective_models_folder != configured_models;

    Ok(StorageSettingsView {
        restart_required: active_database_file != resolved.database_file,
        config: cfg,
        resolved,
        active_database_file,
        effective_models_folder: effective_models_folder.to_string_lossy().to_string(),
        models_folder_fallback,
    })
}

/// `Some(value)` only when the option holds a non-blank string.
fn non_empty_opt(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Persist new storage locations.
///
/// If the database folder changes, the existing database is snapshotted into
/// the new folder so the change survives even if the app is killed before a
/// clean restart.
#[tauri::command]
pub async fn set_storage_config<R: Runtime>(
    app: AppHandle<R>,
    config: StorageConfig,
) -> Result<DatabaseMigrationOutcome, String> {
    let previous = load_storage_config(&app);

    // Validate / create the requested folders before persisting anything, so a
    // bad path cannot leave the app pointing at a non-existent location.
    let mut validated = config.clone();

    if let Some(folder) = non_empty(&config.database_folder) {
        ensure_dir(Path::new(folder))?;
        validated.database_folder = Some(folder.to_string());
    }
    if let Some(folder) = non_empty(&config.models_folder) {
        ensure_dir(Path::new(folder))?;
        validated.models_folder = Some(folder.to_string());
    }
    if let Some(folder) = non_empty(&config.notes_folder) {
        ensure_dir(Path::new(folder))?;
        validated.notes_folder = Some(folder.to_string());
    }
    if let Some(folder) = non_empty(&config.recordings_folder) {
        ensure_dir(Path::new(folder))?;
        validated.recordings_folder = Some(folder.to_string());
    }

    let old_db = previous.database_file(&app)?;
    let new_db = validated.database_file(&app)?;

    let mut outcome = DatabaseMigrationOutcome {
        destination: new_db.to_string_lossy().to_string(),
        copied: false,
        restart_required: false,
        detail: String::new(),
    };

    if old_db != new_db {
        // Snapshot the live database into the new location. The running pool
        // keeps its own file; the snapshot is what the next start will open.
        match snapshot_database(&app, &new_db).await {
            Ok(detail) => {
                outcome.copied = true;
                outcome.restart_required = true;
                outcome.detail = detail;
            }
            Err(e) => {
                // Do not persist a database move we could not honour: the app
                // would start against an empty database.
                return Err(format!(
                    "Refusing to move the database: could not copy it to {}: {e}",
                    new_db.display()
                ));
            }
        }
    }

    save_storage_config(&app, &validated)?;

    if outcome.detail.is_empty() {
        outcome.detail = "Storage locations saved.".to_string();
    }

    Ok(outcome)
}

/// Open a native folder picker and return the chosen path.
///
/// `kind` lets the UI reuse one command for every storage row and only affects
/// the dialog title / default location.
#[tauri::command]
pub async fn pick_storage_directory<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
) -> Result<Option<String>, String> {
    let (title, default_dir) = match kind.as_str() {
        "database" => ("Select database folder", config::app_data_dir(&app).ok()),
        "models" => ("Select models folder", config::default_models_dir(&app).ok()),
        "notes" => ("Select notes folder", config::default_notes_dir(&app).ok()),
        "recordings" => (
            "Select recordings folder",
            Some(crate::audio::recording_preferences::get_default_recordings_folder()),
        ),
        other => return Err(format!("Unknown storage kind: {other}")),
    };

    let app_for_dialog = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut builder = app_for_dialog.dialog().file().set_title(title);
        if let Some(dir) = default_dir.filter(|d| d.exists()) {
            builder = builder.set_directory(dir);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("Folder dialog task failed: {e}"))?;

    Ok(picked.map(|p| p.to_string()))
}

/// `true` when the persisted database location differs from the open one.
#[tauri::command]
pub async fn storage_restart_required<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let view = get_storage_config(app).await?;
    Ok(view.restart_required)
}

/// Reveal one of the configured folders in the OS file manager.
#[tauri::command]
pub async fn open_storage_folder<R: Runtime>(app: AppHandle<R>, kind: String) -> Result<(), String> {
    let cfg = load_storage_config(&app);
    let path = match kind.as_str() {
        "database" => cfg.database_folder(&app)?,
        "models" => cfg.models_folder(&app)?,
        "notes" => cfg.notes_folder(&app)?,
        "recordings" => cfg.recordings_folder(&app)?,
        "app_data" => config::app_data_dir(&app)?,
        other => return Err(format!("Unknown storage kind: {other}")),
    };

    ensure_dir(&path)?;
    open_in_file_manager(&path)
}

/// Reset one location back to its default (by clearing the stored value).
#[tauri::command]
pub async fn reset_storage_location<R: Runtime>(
    app: AppHandle<R>,
    kind: String,
) -> Result<StorageSettingsView, String> {
    let mut cfg = load_storage_config(&app);
    match kind.as_str() {
        "database" => cfg.database_folder = None,
        "models" => cfg.models_folder = None,
        "notes" => cfg.notes_folder = None,
        "recordings" => cfg.recordings_folder = None,
        other => return Err(format!("Unknown storage kind: {other}")),
    }
    save_storage_config(&app, &cfg)?;
    get_storage_config(app).await
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Produce a consistent copy of the live database at `destination`.
///
/// Preferred path is SQLite's `VACUUM INTO`, which writes a single consistent
/// file with the WAL already folded in. If the destination already holds a
/// database we treat the migration as done rather than overwriting user data.
async fn snapshot_database<R: Runtime>(
    app: &AppHandle<R>,
    destination: &Path,
) -> Result<String, String> {
    let cfg = load_storage_config(app);
    let source = cfg.database_file(app)?;

    if !source.exists() {
        return Err(format!("source database {} does not exist", source.display()));
    }

    if let Some(parent) = destination.parent() {
        ensure_dir(parent)?;
    }

    if destination.exists() {
        return Ok(format!(
            "A database already exists at {}; kept it instead of overwriting.",
            destination.display()
        ));
    }

    // `VACUUM INTO` is a consistent snapshot and needs no WAL handling.
    let state = app.try_state::<crate::state::AppState>();
    if let Some(state) = &state {
        let escaped = destination.to_string_lossy().replace('\'', "''");
        let sql = format!("VACUUM INTO '{escaped}'");
        match sqlx::query(&sql).execute(state.db_manager.pool()).await {
            Ok(_) => {
                return Ok(format!(
                    "Database copied to {} via VACUUM INTO. Restart to use it.",
                    destination.display()
                ))
            }
            Err(e) => {
                log::warn!("VACUUM INTO failed ({e}); falling back to checkpoint+copy");
            }
        }
    }

    // Fallback: fold the WAL back into the main file, then copy it plus any
    // remaining sidecar files.
    if let Some(state) = &state {
        if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(state.db_manager.pool())
            .await
        {
            log::warn!("wal_checkpoint before copy failed: {e}");
        }
    }

    std::fs::copy(&source, destination)
        .map_err(|e| format!("copy {} -> {} failed: {e}", source.display(), destination.display()))?;

    for suffix in ["-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{}", source.display(), suffix));
        if from.exists() {
            let to = PathBuf::from(format!("{}{}", destination.display(), suffix));
            if let Err(e) = std::fs::copy(&from, &to) {
                log::warn!("Failed to copy sidecar {}: {e}", from.display());
            }
        }
    }

    Ok(format!(
        "Database copied to {} via file copy. Restart to use it.",
        destination.display()
    ))
}

/// Open a directory with the platform's file manager.
fn open_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(path).spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(path).spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(path).spawn();

    result
        .map(|_| ())
        .map_err(|e| format!("Failed to open {}: {e}", path.display()))
}

/// Expose the default sub directory names so the UI can describe defaults.
#[tauri::command]
pub fn storage_default_subdirs() -> Vec<String> {
    vec![
        DEFAULT_MODELS_SUBDIR.to_string(),
        DEFAULT_NOTES_SUBDIR.to_string(),
        DEFAULT_RECORDINGS_SUBDIR.to_string(),
        DATABASE_FILE_NAME.to_string(),
    ]
}
