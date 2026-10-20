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
use tauri::{AppHandle, Manager, Runtime};

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
    state: tauri::State<'_, AppState>,
) -> Result<RecordingsListView, String> {
    let prefs = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))?;

    let folder = prefs.save_folder.clone();

    // The recordings root is not the whole library: changing the save folder
    // leaves older recordings where they were, and those meetings are still in
    // the sidebar. Read the meeting folders too so both lists describe the same
    // set of recordings, and a meeting whose audio was moved or deleted still
    // appears (with the missing-audio state) instead of silently vanishing.
    let meeting_rows: Vec<(String, Option<String>, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT m.title, m.folder_path, m.created_at,
                EXISTS(SELECT 1 FROM transcripts t WHERE t.meeting_id = m.id) AS has_transcript,
                EXISTS(SELECT 1 FROM summary_processes s
                       WHERE s.meeting_id = m.id AND s.status = 'completed') AS has_summary
         FROM meetings m",
    )
    .fetch_all(state.db_manager.pool())
    .await
    .map_err(|e| format!("Failed to load meetings for the recordings list: {e}"))?;

    let (folder, recordings, total_bytes, folder_missing) = tokio::task::spawn_blocking(move || {
        let folder_missing = !folder.is_dir();
        let mut recordings = list_recordings(&folder);

        for (title, meeting_folder, created_at, has_transcript, has_summary) in meeting_rows {
            let Some(path) = meeting_folder else {
                continue;
            };

            let normalized = normalize_folder(&path);

            if let Some(existing) = recordings
                .iter_mut()
                .find(|entry| normalize_folder(&entry.folder_path) == normalized)
            {
                // The meeting's title is the one the details page shows, and the
                // database knows about transcripts/summaries even when the
                // recording folder was written before those files existed.
                existing.meeting_name = title;
                existing.has_transcript |= has_transcript != 0;
                existing.has_summary |= has_summary != 0;
                continue;
            }

            let path_buf = PathBuf::from(&path);
            if path_buf.is_dir() {
                if let Some(mut entry) = super::recordings::read_recording_entry(&path_buf) {
                    entry.meeting_name = title;
                    entry.has_transcript |= has_transcript != 0;
                    entry.has_summary |= has_summary != 0;
                    recordings.push(entry);
                    continue;
                }
            }

            // The meeting is still listed but its audio is gone. It remains
            // actionable (open, rename, delete the empty record) and the library
            // can explain why there is nothing to play.
            let folder_name = path_buf
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| title.clone());
            let sort_key = created_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp())
                .unwrap_or(0);
            recordings.push(RecordingEntry {
                folder_name,
                folder_path: path,
                meeting_name: title,
                created_at,
                duration_seconds: None,
                audio_file: None,
                audio_path: None,
                audio_bytes: None,
                has_transcript: has_transcript != 0,
                has_summary: has_summary != 0,
                status: None,
                sort_key,
            });
        }

        recordings.sort_by(|a, b| {
            b.sort_key
                .cmp(&a.sort_key)
                .then_with(|| a.folder_name.cmp(&b.folder_name))
        });

        let total_bytes = recordings.iter().filter_map(|r| r.audio_bytes).sum();
        (folder, recordings, total_bytes, folder_missing)
    })
    .await
    .map_err(|e| format!("Recordings list task failed: {}", e))?;

    // Individual audio files can be outside $APPDATA, so allow each one for the
    // asset protocol before the library renders players. Only the exact files are
    // granted: allowing the recordings root recursively would expose every other
    // file under a user-chosen folder to the webview.
    for entry in &recordings {
        if let Some(audio_path) = entry.audio_path.as_deref() {
            if let Err(e) = app.asset_protocol_scope().allow_file(audio_path) {
                log::warn!("Failed to allow reading audio asset {}: {}", audio_path, e);
            }
        }
    }

    Ok(RecordingsListView {
        folder: folder.to_string_lossy().to_string(),
        recordings,
        total_bytes,
        folder_missing,
    })
}

/// Compare two folder paths without caring about separators or trailing slashes.
fn normalize_folder(path: &str) -> String {
    path.trim().replace('\\', "/").trim_end_matches('/').to_string()
}

/// Resolve the audio file inside a saved recording folder.
///
/// Meeting details already knows the folder but not the encoder's file name
/// (`audio.mp3`, `.m4a`, ...), and the recordings root may have changed since the
/// meeting was saved. Resolving it from the folder keeps old meetings playable.
#[tauri::command]
pub async fn get_recording_audio_path<R: Runtime>(
    app: AppHandle<R>,
    folder_path: String,
) -> Result<Option<String>, String> {
    let folder = require_recording_folder(&folder_path)?;

    let audio = find_audio_file(&folder);
    // The recordings folder may live outside $APPDATA, which the static asset
    // scope does not cover. Allow the resolved file so the frontend can stream it
    // through the asset protocol instead of reading the whole file over IPC.
    if let Some(path) = audio.as_ref() {
        if let Err(e) = app.asset_protocol_scope().allow_file(path) {
            log::warn!("Failed to allow reading audio asset {:?}: {}", path, e);
        }
    }

    Ok(audio.map(|path| path.to_string_lossy().to_string()))
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
    find_or_create_meeting(state.db_manager.pool(), &folder_path, &meeting_title).await
}

/// Look up a folder's meeting, creating an empty one on first use.
///
/// Kept out of the command so the reuse rule can be tested against an in-memory
/// database: a second transcribe attempt must not add a duplicate meeting row.
async fn find_or_create_meeting(
    pool: &sqlx::SqlitePool,
    folder_path: &str,
    meeting_title: &str,
) -> Result<String, String> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM meetings WHERE folder_path = ? LIMIT 1")
            .bind(folder_path)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("Failed to look up the meeting for this recording: {}", e))?;

    if let Some(meeting_id) = existing {
        return Ok(meeting_id);
    }

    TranscriptsRepository::save_transcript(pool, meeting_title, &[], Some(folder_path.to_string()))
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
/// Rename a recording's audio file and folder on disk and update its metadata.
///
/// Shared by the library's rename command and by meeting-title saves, so renaming
/// a meeting from the sidebar or the meeting header keeps the audio file name in
/// step instead of leaving the old file name behind.
pub async fn rename_recording_on_disk(
    folder_path: String,
    new_title: String,
    rename_folder: bool,
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

        // A meeting-title save only renames the audio file: the folder path is
        // referenced by the open meeting page and by retranscription, so keeping
        // it stable avoids leaving those pointing at a path that no longer exists.
        let new_folder_name = if rename_folder {
            renamed_folder_name(&folder_name, &trimmed)
        } else {
            folder_name.clone()
        };
        let parent = path
            .parent()
            .ok_or_else(|| "Recording folder has no parent".to_string())?;
        let new_path = parent.join(&new_folder_name);

        // Rename the audio file with the recording. Keep the extension and fall
        // back to a numbered name on collision; a failure here must not stop the
        // folder/metadata rename, and find_audio_file can still recover the file.
        let audio_rename = find_audio_file(&path).and_then(|audio_path| {
            let extension = audio_path.extension().and_then(|value| value.to_str())?;
            let stem = super::audio_processing::sanitize_filename(&trimmed);
            let stem = stem.trim();
            if stem.is_empty() {
                return None;
            }

            let mut target = path.join(format!("{stem}.{extension}"));
            let mut counter = 2;
            while target.exists() && target != audio_path {
                target = path.join(format!("{stem} ({counter}).{extension}"));
                counter += 1;
            }

            if target == audio_path {
                return Some(audio_path);
            }

            std::fs::rename(&audio_path, &target).ok()?;
            Some(target)
        });

        // Update the metadata first: if the rename then fails, the title is still
        // correct and the folder simply keeps its old name.
        if let Some(mut metadata) = super::retention_commands::read_meeting_metadata(&path) {
            metadata.meeting_name = Some(trimmed.clone());
            if let Some(audio_path) = audio_rename.as_ref() {
                if let Some(name) = audio_path.file_name().and_then(|value| value.to_str()) {
                    metadata.audio_file = name.to_string();
                }
            }
            let serialized = serde_json::to_string_pretty(&metadata)
                .map_err(|e| format!("Failed to serialise metadata: {}", e))?;
            std::fs::write(path.join(METADATA_FILE), serialized)
                .map_err(|e| format!("Failed to update metadata: {}", e))?;
        }

        let mut final_path = path.clone();
        if rename_folder && new_path != path {
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

/// Writes the new title into `metadata.json` and renames the folder and audio
/// file, preserving the folder's time stamp so ordering survives the rename.
#[tauri::command]
pub async fn rename_saved_recording(
    state: tauri::State<'_, AppState>,
    folder_path: String,
    new_title: String,
) -> Result<RecordingMutationResult, String> {
    let original_path = folder_path.clone();
    let original_folder = PathBuf::from(&folder_path);
    // Snapshot what exists today so a failed database update can be undone: the
    // folder name changes on disk before the row is updated, and leaving the two
    // out of step would strand the meeting at a path that no longer exists.
    let original_metadata = std::fs::read_to_string(original_folder.join(METADATA_FILE)).ok();
    let original_audio_file = super::retention_commands::read_meeting_metadata(&original_folder)
        .map(|metadata| metadata.audio_file)
        .filter(|name| !name.is_empty());

    let result = rename_recording_on_disk(folder_path, new_title, true).await?;

    // The sidebar reads meetings from the database. Without this update a rename
    // changed the folder on disk but left the sidebar pointing at the old path, so
    // the two lists disagreed.
    if let Err(e) = sqlx::query("UPDATE meetings SET folder_path = ?, title = ? WHERE folder_path = ?")
        .bind(&result.folder_path)
        .bind(&result.title)
        .bind(&original_path)
        .execute(state.db_manager.pool())
        .await
    {
        let renamed_folder = PathBuf::from(&result.folder_path);
        let rollback = rollback_rename(
            &original_folder,
            &renamed_folder,
            original_metadata.as_deref(),
            original_audio_file.as_deref(),
        );

        return Err(match rollback {
            Ok(()) => format!(
                "The recording name was not saved because its meeting could not be updated: {e}"
            ),
            Err(rollback_error) => format!(
                "The recording was renamed but its meeting could not be updated ({e}), and restoring the old name failed ({rollback_error}). The recording is at {}.",
                result.folder_path
            ),
        });
    }

    Ok(result)
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
    state: tauri::State<'_, AppState>,
    folder_path: String,
    destination: Option<String>,
) -> Result<RecordingMutationResult, String> {
    use tauri_plugin_dialog::DialogExt;

    let original_path = folder_path.clone();
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

    let result = tokio::task::spawn_blocking(move || {
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
            detail: format!("Moved to {}", destination.display()),
        })
    })
    .await
    .map_err(|e| format!("Move task failed: {}", e))??;

    // Keep the sidebar and the library pointing at the same folder after a move.
    if let Err(e) = sqlx::query("UPDATE meetings SET folder_path = ? WHERE folder_path = ?")
        .bind(&result.folder_path)
        .bind(&original_path)
        .execute(state.db_manager.pool())
        .await
    {
        let original = PathBuf::from(&original_path);
        let moved = PathBuf::from(&result.folder_path);

        return Err(match restore_moved_folder(&original, &moved) {
            Ok(()) => format!(
                "The recording could not be moved because its meeting could not be updated: {e}"
            ),
            Err(restore_error) => format!(
                "The recording was moved to '{}' but its meeting could not be updated ({e}), and restoring the original location failed: {restore_error}",
                result.folder_path
            ),
        });
    }

    Ok(result)
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

/// Undo a recording rename after the database update failed.
///
/// Restores the original folder name, the audio file name recorded in the
/// original metadata and the original metadata.json itself. The caller logs a
/// failure here: the recording is still on disk, but possibly under the new name.
fn rollback_rename(
    original_folder: &Path,
    renamed_folder: &Path,
    original_metadata: Option<&str>,
    original_audio_file: Option<&str>,
) -> Result<(), String> {
    if renamed_folder != original_folder && renamed_folder.is_dir() {
        if original_folder.exists() {
            return Err(format!(
                "Cannot restore the original folder name because '{}' already exists",
                original_folder.display()
            ));
        }
        std::fs::rename(renamed_folder, original_folder).map_err(|e| {
            format!(
                "Failed to rename '{}' back to '{}': {}",
                renamed_folder.display(),
                original_folder.display(),
                e
            )
        })?;
    }

    let folder = if original_folder.is_dir() {
        original_folder
    } else {
        renamed_folder
    };

    if let Some(original_name) = original_audio_file {
        if let Some(current) = find_audio_file(folder) {
            if current.file_name().and_then(|name| name.to_str()) != Some(original_name) {
                let target = folder.join(original_name);
                if !target.exists() {
                    std::fs::rename(&current, &target).map_err(|e| {
                        format!(
                            "Failed to restore the audio file name '{}': {}",
                            original_name, e
                        )
                    })?;
                }
            }
        }
    }

    if let Some(metadata) = original_metadata {
        std::fs::write(folder.join(METADATA_FILE), metadata)
            .map_err(|e| format!("Failed to restore {}: {}", METADATA_FILE, e))?;
    }

    Ok(())
}

/// Undo a move when the database update failed: put the recording back at its
/// original path. A rename is preferred; when the two paths are on different
/// volumes the copy-then-remove fallback is used.
fn restore_moved_folder(original_folder: &Path, moved_folder: &Path) -> Result<(), String> {
    if !moved_folder.is_dir() {
        return Err(format!(
            "The moved recording is missing: {}",
            moved_folder.display()
        ));
    }
    if original_folder.exists() {
        return Err(format!(
            "The original recording path is already in use: {}",
            original_folder.display()
        ));
    }
    if let Some(parent) = original_folder.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to recreate '{}': {}", parent.display(), e))?;
    }

    match std::fs::rename(moved_folder, original_folder) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            // Cross-volume moves cannot be renamed; copy back and drop the copy.
            copy_dir_recursive(moved_folder, original_folder).map_err(|copy_error| {
                format!(
                    "Failed to restore '{}': {} (copying it back also failed: {})",
                    original_folder.display(),
                    rename_error,
                    copy_error
                )
            })?;
            std::fs::remove_dir_all(moved_folder).map_err(|remove_error| {
                format!(
                    "Restored '{}' but could not remove the moved copy '{}': {}",
                    original_folder.display(),
                    moved_folder.display(),
                    remove_error
                )
            })
        }
    }
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

    /// The meetings table as the migration creates it, reduced to the columns the
    /// lookup and insert touch.
    async fn test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        sqlx::query(
            "CREATE TABLE meetings (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                updated_at DATETIME NOT NULL,
                folder_path TEXT
            )",
        )
        .execute(&pool)
        .await
        .expect("create meetings table");
        pool
    }

    #[tokio::test]
    async fn meeting_for_a_folder_is_created_once_and_then_reused() {
        let pool = test_pool().await;

        let first = find_or_create_meeting(&pool, "D:\\recordings\\a", "First title")
            .await
            .expect("create");
        // A retry, possibly after a rename, must not add a second meeting row.
        let again = find_or_create_meeting(&pool, "D:\\recordings\\a", "Renamed")
            .await
            .expect("reuse");
        assert_eq!(first, again);

        let other = find_or_create_meeting(&pool, "D:\\recordings\\b", "Second")
            .await
            .expect("create second");
        assert_ne!(first, other);

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 2, "one meeting row per recording folder");
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

    #[test]
    fn rollback_rename_restores_folder_metadata_and_audio_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = make_recording(tmp.path(), "Old_2026-05-01_10-00");
        let original_metadata =
            std::fs::read_to_string(original.join(METADATA_FILE)).expect("read metadata");

        // Simulate the completed rename that the database update failed after.
        let renamed = tmp.path().join("New_2026-05-01_10-00");
        std::fs::rename(&original, &renamed).expect("rename folder");
        std::fs::rename(renamed.join("audio.m4a"), renamed.join("New.m4a"))
            .expect("rename audio file");
        std::fs::write(
            renamed.join(METADATA_FILE),
            serde_json::json!({
                "version": "1.0",
                "meeting_name": "New",
                "audio_file": "New.m4a",
                "created_at": "2026-05-01T10:00:00Z",
                "transcript_file": "transcripts.json",
                "sample_rate": 48000,
                "status": "completed"
            })
            .to_string(),
        )
        .expect("write updated metadata");

        rollback_rename(&original, &renamed, Some(&original_metadata), Some("audio.m4a"))
            .expect("rollback");

        assert!(!renamed.exists(), "the renamed folder must be gone");
        assert!(original.is_dir(), "the original folder name must be back");
        assert!(
            original.join("audio.m4a").is_file(),
            "the original audio file name must be restored"
        );
        assert!(!original.join("New.m4a").exists());
        assert_eq!(
            std::fs::read_to_string(original.join(METADATA_FILE)).expect("read metadata"),
            original_metadata,
            "metadata must be restored byte for byte"
        );
    }

    #[test]
    fn rollback_rename_refuses_to_overwrite_an_existing_original() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = make_recording(tmp.path(), "Old_2026-05-01_10-00");
        let renamed = make_recording(tmp.path(), "New_2026-05-01_10-00");

        assert!(rollback_rename(&original, &renamed, None, None).is_err());
        // Nothing was destroyed: both recordings still exist.
        assert!(original.join("audio.m4a").is_file());
        assert!(renamed.join("audio.m4a").is_file());
    }

    #[test]
    fn restore_moved_folder_puts_a_recording_back_at_its_original_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("recordings");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::create_dir_all(&elsewhere).expect("create elsewhere");
        let original = make_recording(&root, "r_2026-05-01_10-00");
        let moved = elsewhere.join("r_2026-05-01_10-00");

        copy_dir_recursive(&original, &moved).expect("copy");
        std::fs::remove_dir_all(&original).expect("remove source");

        restore_moved_folder(&original, &moved).expect("restore");

        assert!(
            original.is_dir(),
            "the recording must be back at its original path"
        );
        assert!(original.join(METADATA_FILE).is_file());
        assert!(!moved.exists(), "the moved copy must be gone");
    }

    #[test]
    fn restore_moved_folder_refuses_when_the_original_path_is_occupied() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = make_recording(tmp.path(), "r_2026-05-01_10-00");
        let moved = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&moved).expect("create moved folder");

        assert!(restore_moved_folder(&original, &moved).is_err());
        assert!(
            original.join("audio.m4a").is_file(),
            "an existing folder must never be overwritten"
        );
    }
}
