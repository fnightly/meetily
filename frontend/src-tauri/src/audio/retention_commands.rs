// audio/retention_commands.rs
//
// Filesystem side of the retention policy, plus the free-space watch used while
// a recording is running.
//
// The decision rules live in `retention.rs` and are unit tested; this module
// only reads metadata, applies the decision, and removes what was selected.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tauri::{AppHandle, Runtime};

use serde::Serialize;

use super::retention::{is_meeting_folder, select_expired, RecordingCandidate, KeepReason};
use super::recording_saver::MeetingMetadata;

/// Outcome of a retention sweep, for the settings UI and the log.
#[derive(Debug, Serialize)]
pub struct RetentionSweepResult {
    /// Folders removed.
    pub deleted: Vec<String>,
    /// Number of folders examined.
    pub examined: usize,
    /// Folders kept despite being old, with the reason, so a user wondering why
    /// their recordings survived can see the rule that applied.
    pub kept: Vec<KeptRecording>,
    /// Human-readable summary.
    pub detail: String,
    /// True when retention is switched off, in which case nothing was examined.
    pub disabled: bool,
}

#[derive(Debug, Serialize)]
pub struct KeptRecording {
    pub path: String,
    pub reason: String,
}

/// Parse the RFC 3339 `completed_at` from a meeting's metadata.
///
/// Returns `None` when the field is absent or unparseable. A recording whose age
/// cannot be established is kept — deleting on a parse failure would be data loss
/// caused by a metadata bug.
pub fn completed_at_from_metadata(metadata: &MeetingMetadata) -> Option<SystemTime> {
    // A recording still in progress is never a candidate.
    if metadata.status == "recording" {
        return None;
    }

    let raw = metadata.completed_at.as_deref()?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| SystemTime::from(dt.with_timezone(&chrono::Utc)))
}

/// Read a meeting folder's `metadata.json`, or `None` if it cannot be read.
pub fn read_meeting_metadata(folder: &Path) -> Option<MeetingMetadata> {
    let contents = std::fs::read_to_string(folder.join("metadata.json")).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Collect the recording folders directly under `root`.
///
/// Also reports folders that exist but are not recordings, so the caller can
/// leave them alone rather than counting them as examined recordings.
pub fn collect_candidates(root: &Path) -> Vec<RecordingCandidate> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_meeting_folder(&path) {
            continue;
        }

        let completed_at = read_meeting_metadata(&path).and_then(|m| completed_at_from_metadata(&m));
        candidates.push(RecordingCandidate::new(path, completed_at));
    }

    candidates
}

/// Remove `folders`, returning the ones successfully deleted.
///
/// Failures are logged and skipped rather than aborting the sweep: one locked
/// folder should not stop the others from being cleaned up.
pub fn delete_recordings(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut deleted = Vec::new();

    for folder in folders {
        match std::fs::remove_dir_all(folder) {
            Ok(()) => {
                log::info!("Retention removed recording: {}", folder.display());
                deleted.push(folder.clone());
            }
            Err(e) => {
                log::warn!(
                    "Retention could not remove {}: {}",
                    folder.display(),
                    e
                );
            }
        }
    }

    deleted
}

/// Remove directories left empty after a sweep.
///
/// Only touches empty directories, so a folder still holding anything — a
/// `.checkpoints` remnant, a stray file — is left alone.
pub fn remove_empty_dirs(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };

    let mut removed = 0;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // `remove_dir` fails on a non-empty directory, which is exactly the
        // guard wanted here.
        if std::fs::remove_dir(&path).is_ok() {
            log::info!("Retention removed empty folder: {}", path.display());
            removed += 1;
        }
    }

    removed
}

fn describe_reason(reason: KeepReason) -> &'static str {
    match reason {
        KeepReason::RetentionDisabled => "retention is disabled",
        KeepReason::WithinLifetime => "still within the retention period",
        KeepReason::UnknownAge => "completion time unknown",
        KeepReason::NotYetComplete => "not older than the retention period",
        KeepReason::Missing => "no longer present",
    }
}

/// Run one retention sweep over `recordings_root`.
pub fn sweep(recordings_root: &Path, lifetime_days: u32) -> RetentionSweepResult {
    if lifetime_days == 0 {
        return RetentionSweepResult {
            deleted: Vec::new(),
            examined: 0,
            kept: Vec::new(),
            detail: "Retention is disabled; no recordings were examined.".to_string(),
            disabled: true,
        };
    }

    let candidates = collect_candidates(recordings_root);
    let examined = candidates.len();

    let (expired, kept) = select_expired(&candidates, lifetime_days, SystemTime::now());

    let deleted_paths = delete_recordings(&expired);
    let empty_removed = remove_empty_dirs(recordings_root);

    let detail = format!(
        "Examined {} recording(s): removed {}, kept {}. Empty folders removed: {}.",
        examined,
        deleted_paths.len(),
        kept.len(),
        empty_removed
    );

    log::info!("Retention sweep: {}", detail);

    RetentionSweepResult {
        deleted: deleted_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        examined,
        kept: kept
            .into_iter()
            .map(|(path, reason)| KeptRecording {
                path: path.to_string_lossy().to_string(),
                reason: describe_reason(reason).to_string(),
            })
            .collect(),
        detail,
        disabled: false,
    }
}

/// Free-space status of the recordings volume.
#[derive(Debug, Serialize)]
pub struct DiskSpaceStatus {
    pub folder: String,
    pub free_bytes: u64,
    pub free_mb: u64,
    /// Configured warning threshold in MB (zero means no warning).
    pub threshold_mb: u32,
    /// True when free space is below the threshold.
    pub is_low: bool,
    /// False when free space could not be measured.
    pub measured: bool,
}

/// Whether the recordings volume is running low.
///
/// Recording writes for as long as the meeting lasts, so a disk that is fine at
/// the start can fill up mid-recording. Checking during capture turns "the
/// recording failed and the audio is gone" into an early warning.
#[tauri::command]
pub async fn get_recording_disk_space<R: Runtime>(
    app: AppHandle<R>,
) -> Result<DiskSpaceStatus, String> {
    use super::retention::DiskWatch;

    let prefs = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))?;
    let folder = prefs.save_folder.clone();
    let watch = DiskWatch::new(prefs.low_disk_warning_mb);

    let probe = crate::disk_space::nearest_existing_ancestor(&folder);

    match crate::disk_space::available_space_bytes(&probe) {
        Some(free) => Ok(DiskSpaceStatus {
            folder: probe.to_string_lossy().to_string(),
            free_bytes: free,
            free_mb: crate::disk_space::human_mb(free),
            threshold_mb: watch.warn_below_mb,
            is_low: watch.is_low(free),
            measured: true,
        }),
        None => Ok(DiskSpaceStatus {
            folder: probe.to_string_lossy().to_string(),
            free_bytes: 0,
            free_mb: 0,
            threshold_mb: watch.warn_below_mb,
            // Unknown space must not be reported as "low", or every recording
            // would show a warning on a platform where the query fails.
            is_low: false,
            measured: false,
        }),
    }
}

/// Run a retention sweep now, using the configured lifetime.
#[tauri::command]
pub async fn purge_expired_recordings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RetentionSweepResult, String> {
    let prefs = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))?;
    let root = prefs.save_folder.clone();
    let lifetime = prefs.recordings_retention_days;

    // Deletion touches the filesystem for a while; keep it off the async runtime.
    tokio::task::spawn_blocking(move || sweep(&root, lifetime))
        .await
        .map_err(|e| format!("Retention sweep task failed: {}", e))
}

/// Report what a sweep *would* remove, without deleting anything.
#[tauri::command]
pub async fn preview_retention_sweep<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RetentionSweepResult, String> {
    let prefs = super::recording_preferences::load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))?;
    let root = prefs.save_folder.clone();
    let lifetime = prefs.recordings_retention_days;

    tokio::task::spawn_blocking(move || {
        if lifetime == 0 {
            return RetentionSweepResult {
                deleted: Vec::new(),
                examined: 0,
                kept: Vec::new(),
                detail: "Retention is disabled; nothing would be removed.".to_string(),
                disabled: true,
            };
        }

        let candidates = collect_candidates(&root);
        let examined = candidates.len();
        let (expired, kept) = select_expired(&candidates, lifetime, SystemTime::now());

        RetentionSweepResult {
            // `deleted` lists what *would* go; the preview never removes anything.
            detail: format!(
                "Examined {} recording(s): {} would be removed, {} kept.",
                examined,
                expired.len(),
                kept.len()
            ),
            deleted: expired
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            examined,
            kept: kept
                .into_iter()
                .map(|(path, reason)| KeptRecording {
                    path: path.to_string_lossy().to_string(),
                    reason: describe_reason(reason).to_string(),
                })
                .collect(),
            disabled: false,
        }
    })
    .await
    .map_err(|e| format!("Retention preview task failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata_json(status: &str, completed_at: Option<&str>) -> String {
        serde_json::json!({
            "version": "1.0",
            "meeting_id": null,
            "meeting_name": "Test",
            "created_at": "2026-01-01T00:00:00Z",
            "completed_at": completed_at,
            "duration_seconds": 60.0,
            "devices": { "microphone": null, "system_audio": null },
            "audio_file": "audio.m4a",
            "transcript_file": "transcripts.json",
            "sample_rate": 48000,
            "status": status
        })
        .to_string()
    }

    fn make_meeting(root: &Path, name: &str, status: &str, completed_at: Option<&str>) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).expect("create meeting dir");
        std::fs::write(folder.join("metadata.json"), metadata_json(status, completed_at))
            .expect("write metadata");
        folder
    }

    #[test]
    fn parses_a_completed_timestamp() {
        let metadata: MeetingMetadata =
            serde_json::from_str(&metadata_json("completed", Some("2026-01-02T03:04:05Z")))
                .expect("parse");
        assert!(completed_at_from_metadata(&metadata).is_some());
    }

    #[test]
    fn an_in_progress_recording_is_never_a_candidate() {
        // Even with a timestamp present, status takes precedence: deleting the
        // recording currently being written would be the worst possible outcome.
        let metadata: MeetingMetadata =
            serde_json::from_str(&metadata_json("recording", Some("2020-01-01T00:00:00Z")))
                .expect("parse");
        assert!(completed_at_from_metadata(&metadata).is_none());
    }

    #[test]
    fn a_missing_timestamp_is_unknown_age() {
        let metadata: MeetingMetadata =
            serde_json::from_str(&metadata_json("completed", None)).expect("parse");
        assert!(completed_at_from_metadata(&metadata).is_none());
    }

    #[test]
    fn an_unparseable_timestamp_is_unknown_age() {
        let metadata: MeetingMetadata =
            serde_json::from_str(&metadata_json("completed", Some("not-a-date"))).expect("parse");
        assert!(completed_at_from_metadata(&metadata).is_none());
    }

    #[test]
    fn collect_candidates_ignores_non_recording_folders() {
        let root = tempfile::tempdir().expect("tempdir");
        make_meeting(root.path(), "Team Standup_2026-01-01_10-00", "completed", Some("2026-01-01T10:30:00Z"));
        std::fs::create_dir_all(root.path().join("not-a-recording")).expect("create");
        std::fs::write(root.path().join("stray.txt"), b"x").expect("write");

        let candidates = collect_candidates(root.path());
        assert_eq!(candidates.len(), 1, "only metadata-bearing folders count");
    }

    #[test]
    fn sweep_is_a_no_op_when_disabled() {
        let root = tempfile::tempdir().expect("tempdir");
        make_meeting(root.path(), "old", "completed", Some("2020-01-01T00:00:00Z"));

        let result = sweep(root.path(), 0);

        assert!(result.disabled);
        assert!(result.deleted.is_empty());
        assert!(root.path().join("old").exists(), "nothing may be removed");
    }

    #[test]
    fn sweep_removes_only_dated_recordings_past_the_lifetime() {
        let root = tempfile::tempdir().expect("tempdir");
        let old = make_meeting(root.path(), "old", "completed", Some("2020-01-01T00:00:00Z"));
        let undated = make_meeting(root.path(), "undated", "completed", None);

        // One day retention: the 2020 recording is far past it.
        let result = sweep(root.path(), 1);

        assert_eq!(result.deleted.len(), 1);
        assert!(!old.exists(), "the dated recording should be gone");
        assert!(undated.exists(), "an undated recording must survive");
        assert!(result.kept.iter().any(|k| k.reason.contains("unknown")));
    }

    #[test]
    fn sweep_never_deletes_a_recording_still_in_progress() {
        let root = tempfile::tempdir().expect("tempdir");
        let active = make_meeting(
            root.path(),
            "in-progress",
            "recording",
            Some("2020-01-01T00:00:00Z"),
        );

        let result = sweep(root.path(), 1);

        assert!(result.deleted.is_empty());
        assert!(active.exists());
    }

    #[test]
    fn remove_empty_dirs_leaves_populated_folders() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("empty")).expect("create");
        let populated = root.path().join("populated");
        std::fs::create_dir_all(&populated).expect("create");
        std::fs::write(populated.join("keep.txt"), b"x").expect("write");

        let removed = remove_empty_dirs(root.path());

        assert_eq!(removed, 1);
        assert!(!root.path().join("empty").exists());
        assert!(populated.exists());
    }

    #[test]
    fn read_meeting_metadata_returns_none_for_a_broken_file() {
        let root = tempfile::tempdir().expect("tempdir");
        let folder = root.path().join("broken");
        std::fs::create_dir_all(&folder).expect("create");
        std::fs::write(folder.join("metadata.json"), b"{ not json").expect("write");

        assert!(read_meeting_metadata(&folder).is_none());
    }

    #[test]
    fn collect_candidates_on_a_missing_root_is_empty_not_an_error() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(collect_candidates(&root.path().join("nope")).is_empty());
    }
}
