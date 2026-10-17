// audio/recordings_commands.rs
//
// Tauri command surface for the saved-recordings list and the per-recording
// operations: rename, delete, move, open, transcribe and summarise.
//
// Destructive operations are deliberately narrow: every one of them takes the
// recording's own folder path and verifies it is a recording before acting, so a
// stale or crafted path cannot remove an unrelated directory.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Runtime};

use super::recordings::{
    find_audio_file, is_recording_folder, list_recordings, renamed_folder_name, RecordingEntry,
    METADATA_FILE,
};
use crate::database::repositories::transcript::TranscriptsRepository;
use crate::state::AppState;

/// What the recordings list UI needs.
#[derive(Debug, Serialize)]
pub struct RecordingsListView {
    /// The folder that was scanned, so the UI can offer to open it.
    pub folder: String,
    /// Recordings, newest first.
    pub recordings: Vec<RecordingEntry>,
    /// Total size of every audio file found, in bytes.
    pub total_bytes: u64,
    /// True when the configured recordings folder does not exist yet.
    pub folder_missing: bool,
}

/// Result of an operation that changes where a recording lives.
#[derive(Debug, Serialize)]
pub struct RecordingMutationResult {
    /// Path of the recording after the operation.
    pub folder_path: String,
    /// Display name after the operation.
    pub title: String,
    /// Extra detail for the user, e.g. what was freed or moved.
    pub detail: String,
}

/// Refuse to act on a path that is not a recording folder.
///
/// This is the guard that keeps a delete or move from touching something that
/// merely happens to be at the given path.
fn require_recording_folder(folder_path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(folder_path);

    if !path.exists() {
        return Err(format!("Recording folder not found: {folder_path}"));
    }

    if !is_recording_folder(&path) {
        return Err(format!(
            "Not a recording folder (no {METADATA_FILE}): {folder_path}"
        ));
    }

    Ok(path)
}

/// Total size of every file inside a folder, for the "freed" figure.
pub fn folder_size_bytes(folder: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };

    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                folder_size_bytes(&path)
            } else {
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

/// List the recordings saved on disk.
///
/// Reads the filesystem rather than the database: a recording whose meeting row
/// was never written would otherwise be invisible even though the audio exists.
#[tauri::command]
pub async fn list_saved_recordings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RecordingsListView, String> {
    let prefs = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))?;

    let folder = prefs.save_folder.clone();

    let (folder, recordings, total_bytes, folder_missing) = tokio::task::spawn_blocking(move || {
        let folder_missing = !folder.is_dir();
        let recordings = list_recordings(&folder);
        let total_bytes = recordings.iter().filter_map(|r| r.audio_bytes).sum();
        (folder, recordings, total_bytes, folder_missing)
    })
    .await
    .map_err(|e| format!("Recordings list task failed: {}", e))?;

    Ok(RecordingsListView {
        folder: folder.to_string_lossy().to_string(),
        recordings,
        total_bytes,
        folder_missing,
    })
}

/// Find the meeting row for a recording folder, creating one when absent.
///
/// Streaming transcription can be retried — by the user, or by the UI after a
/// failure — and each attempt would otherwise insert a fresh meeting row for the
/// same folder, leaving duplicates in the meeting list. Looking the folder up
/// first keeps one recording mapped to one meeting.
///
/// The created row has no transcript segments: the transcription that follows
/// writes them through the usual transcript commands.
#[tauri::command]
pub async fn ensure_meeting_for_recording(
    state: tauri::State<'_, AppState>,
    folder_path: String,
    meeting_title: String,
) -> Result<String, String> {
    let pool = state.db_manager.pool();

    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM meetings WHERE folder_path = ? LIMIT 1")
            .bind(&folder_path)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("Failed to look up the meeting for this recording: {}", e))?;

    if let Some(meeting_id) = existing {
        return Ok(meeting_id);
    }

    TranscriptsRepository::save_transcript(pool, &meeting_title, &[], Some(folder_path))
        .await
        .map_err(|e| format!("Failed to create a meeting for this recording: {}", e))
}

/// Open a recording folder in the OS file manager.
///
/// Takes a path rather than a meeting id because the interesting case is a
/// recording that has no database row, which an id-based lookup cannot serve.
#[tauri::command]
pub async fn open_recording_folder(folder_path: String) -> Result<(), String> {
    let path = require_recording_folder(&folder_path)?;

    let target = path.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer").arg(&target).spawn();

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&target).spawn();

    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(&target).spawn();

    result.map_err(|e| format!("Failed to open folder: {}", e))?;

    Ok(())
}

/// Rename a recording.
///
/// Writes the new title into `metadata.json` and renames the folder, preserving
/// its time stamp so ordering survives the rename and two recordings cannot end
/// up with the same folder name.
#[tauri::command]
pub async fn rename_saved_recording(
    folder_path: String,
    new_title: String,
) -> Result<RecordingMutationResult, String> {
    let trimmed = new_title.trim().to_string();
    if trimmed.is_empty() {
        return Err("A recording name cannot be empty".to_string());
    }

    tokio::task::spawn_blocking(move || {
        let path = require_recording_folder(&folder_path)?;

        let folder_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| "Recording folder has no name".to_string())?;

        let new_folder_name = renamed_folder_name(&folder_name, &trimmed);
        let parent = path
            .parent()
            .ok_or_else(|| "Recording folder has no parent".to_string())?;
        let new_path = parent.join(&new_folder_name);

        // Update the metadata first: if the rename then fails, the title is still
        // correct and the folder simply keeps its old name.
        if let Some(mut metadata) = super::retention_commands::read_meeting_metadata(&path) {
            metadata.meeting_name = Some(trimmed.clone());
            let serialized = serde_json::to_string_pretty(&metadata)
                .map_err(|e| format!("Failed to serialise metadata: {}", e))?;
            std::fs::write(path.join(METADATA_FILE), serialized)
                .map_err(|e| format!("Failed to update metadata: {}", e))?;
        }

        let mut final_path = path.clone();
        if new_path != path {
            if new_path.exists() {
                return Err(format!(
                    "A recording named '{}' already exists",
                    new_folder_name
                ));
            }
            std::fs::rename(&path, &new_path)
                .map_err(|e| format!("Failed to rename recording folder: {}", e))?;
            final_path = new_path;
        }

        Ok(RecordingMutationResult {
            folder_path: final_path.to_string_lossy().to_string(),
            title: trimmed,
            detail: format!("Renamed to '{new_folder_name}'"),
        })
    })
    .await
    .map_err(|e| format!("Rename task failed: {}", e))?
}

/// Delete a recording from disk.
#[tauri::command]
pub async fn delete_saved_recording(
    folder_path: String,
) -> Result<RecordingMutationResult, String> {
    tokio::task::spawn_blocking(move || {
        let path = require_recording_folder(&folder_path)?;

        let freed = folder_size_bytes(&path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        std::fs::remove_dir_all(&path)
            .map_err(|e| format!("Failed to delete recording: {}", e))?;

        Ok(RecordingMutationResult {
            folder_path: String::new(),
            title: name,
            detail: format!("Deleted, freeing {}", super::recordings::format_bytes(freed)),
        })
    })
    .await
    .map_err(|e| format!("Delete task failed: {}", e))?
}

/// Copy a recording to another folder, then remove the original.
///
/// Copies before deleting so a failure part-way through leaves the recording
/// intact at its original location rather than losing it. The destination is
/// chosen with a folder picker when `destination` is not supplied.
#[tauri::command]
pub async fn move_saved_recording<R: Runtime>(
    app: AppHandle<R>,
    folder_path: String,
    destination: Option<String>,
) -> Result<RecordingMutationResult, String> {
    use tauri_plugin_dialog::DialogExt;

    let source = require_recording_folder(&folder_path)?;

    let destination = match destination {
        Some(value) if !value.trim().is_empty() => PathBuf::from(value.trim()),
        _ => {
            let picked = app
                .dialog()
                .file()
                .set_title("Choose where to move the recording")
                .blocking_pick_folder()
                .ok_or_else(|| "No destination was chosen".to_string())?;

            picked
                .into_path()
                .map_err(|e| format!("Invalid destination folder: {}", e))?
        }
    };

    tokio::task::spawn_blocking(move || {
        if !destination.is_dir() {
            return Err(format!(
                "Destination folder does not exist: {}",
                destination.display()
            ));
        }

        let folder_name = source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| "Recording folder has no name".to_string())?;

        let target = destination.join(&folder_name);

        // Refuse to move a folder into itself or into its own subtree, which
        // would recurse until the disk fills.
        if target.starts_with(&source) {
            return Err("A recording cannot be moved inside itself".to_string());
        }

        if target.exists() {
            return Err(format!(
                "A recording named '{folder_name}' already exists in the destination"
            ));
        }

        copy_dir_recursive(&source, &target)
            .map_err(|e| format!("Failed to copy the recording: {}", e))?;

        // Only remove the original once the copy is complete.
        if let Err(e) = std::fs::remove_dir_all(&source) {
            return Err(format!(
                "The recording was copied to {} but the original could not be removed: {}",
                target.display(),
                e
            ));
        }

        let title = super::retention_commands::read_meeting_metadata(&target)
            .and_then(|m| m.meeting_name)
            .unwrap_or_else(|| folder_name.clone());

        Ok(RecordingMutationResult {
            folder_path: target.to_string_lossy().to_string(),
            title,
            detail: format!(
                "Moved to {}. It now lives outside your recordings folder, so it no longer appears in this list.",
                destination.display()
            ),
        })
    })
    .await
    .map_err(|e| format!("Move task failed: {}", e))?
}

/// Copy a directory tree.
fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;

    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = target.join(entry.file_name());

        if path.is_dir() {
            copy_dir_recursive(&path, &destination)?;
        } else {
            std::fs::copy(&path, &destination)?;
        }
    }

    Ok(())
}

/// Remove a generated summary, keeping the recording and its transcript.
#[tauri::command]
pub async fn delete_saved_recording_summary(
    folder_path: String,
) -> Result<RecordingMutationResult, String> {
    tokio::task::spawn_blocking(move || {
        let path = require_recording_folder(&folder_path)?;
        let summary = path.join(super::recordings::SUMMARY_FILE);

        if !summary.is_file() {
            return Err("This recording has no summary to remove".to_string());
        }

        std::fs::remove_file(&summary)
            .map_err(|e| format!("Failed to remove the summary: {}", e))?;

        let title = super::retention_commands::read_meeting_metadata(&path)
            .and_then(|m| m.meeting_name)
            .unwrap_or_default();

        Ok(RecordingMutationResult {
            folder_path: path.to_string_lossy().to_string(),
            title,
            detail: "Summary removed. The transcript and audio are untouched.".to_string(),
        })
    })
    .await
    .map_err(|e| format!("Remove summary task failed: {}", e))?
}

/// Whether a recording has audio that can be transcribed.
#[tauri::command]
pub async fn saved_recording_has_audio(folder_path: String) -> Result<bool, String> {
    let path = require_recording_folder(&folder_path)?;
    Ok(find_audio_file(&path).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_recording(root: &Path, name: &str) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).expect("create");
        std::fs::write(
            folder.join(METADATA_FILE),
            serde_json::json!({
                "version": "1.0",
                "meeting_id": null,
                "meeting_name": "Original",
                "created_at": "2026-05-01T10:00:00Z",
                "completed_at": "2026-05-01T10:30:00Z",
                "duration_seconds": 1800.0,
                "devices": { "microphone": null, "system_audio": null },
                "audio_file": "audio.m4a",
                "transcript_file": "transcripts.json",
                "sample_rate": 48000,
                "status": "completed"
            })
            .to_string(),
        )
        .expect("write metadata");
        std::fs::write(folder.join("audio.m4a"), vec![0u8; 2048]).expect("write audio");
        folder
    }

    #[test]
    fn require_recording_folder_rejects_a_plain_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).expect("create");

        // The guard is what stops a delete or move from touching arbitrary paths.
        let error = require_recording_folder(&plain.to_string_lossy()).unwrap_err();
        assert!(error.contains("Not a recording folder"), "got: {error}");
    }

    #[test]
    fn require_recording_folder_rejects_a_missing_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("nope");
        assert!(require_recording_folder(&missing.to_string_lossy()).is_err());
    }

    #[test]
    fn require_recording_folder_accepts_a_recording() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = make_recording(tmp.path(), "r_2026-05-01_10-00");
        assert!(require_recording_folder(&folder.to_string_lossy()).is_ok());
    }

    #[test]
    fn folder_size_counts_nested_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = make_recording(tmp.path(), "r_2026-05-01_10-00");
        let nested = folder.join(".checkpoints");
        std::fs::create_dir_all(&nested).expect("create");
        std::fs::write(nested.join("chunk.mp4"), vec![0u8; 1024]).expect("write");

        let audio = std::fs::metadata(folder.join("audio.m4a")).expect("stat").len();

        // The point is that nested files count too, so compare against the
        // measured audio size rather than hardcoding a total that would break
        // whenever the metadata schema changes.
        assert!(
            folder_size_bytes(&folder) >= audio + 1024,
            "nested files must contribute to the total"
        );
    }

    #[test]
    fn folder_size_is_zero_for_a_missing_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(folder_size_bytes(&tmp.path().join("nope")), 0);
    }

    #[test]
    fn copy_dir_recursive_reproduces_the_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = make_recording(tmp.path(), "src_2026-05-01_10-00");
        std::fs::create_dir_all(source.join("sub")).expect("create");
        std::fs::write(source.join("sub").join("f.txt"), b"hi").expect("write");

        let target = tmp.path().join("target");
        copy_dir_recursive(&source, &target).expect("copy");

        assert!(target.join(METADATA_FILE).is_file());
        assert!(target.join("audio.m4a").is_file());
        assert_eq!(
            std::fs::read_to_string(target.join("sub").join("f.txt")).expect("read"),
            "hi"
        );
    }

    #[test]
    fn a_recording_can_be_moved_out_and_deleted_from_the_original() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("recordings");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&root).expect("create");
        std::fs::create_dir_all(&elsewhere).expect("create");
        let folder = make_recording(&root, "r_2026-05-01_10-00");

        let target = elsewhere.join("r_2026-05-01_10-00");
        copy_dir_recursive(&folder, &target).expect("copy");
        std::fs::remove_dir_all(&folder).expect("remove original");

        assert!(!folder.exists(), "the original must be gone");
        assert!(target.join("audio.m4a").is_file(), "the copy must be intact");
        // Moving out of the root removes it from the list, which is the documented
        // behaviour rather than a bug.
        assert!(list_recordings(&root).is_empty());
    }
}
