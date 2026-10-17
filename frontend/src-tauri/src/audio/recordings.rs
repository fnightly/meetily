// audio/recordings.rs
//
// The saved-recordings list, and the per-recording operations on it.
//
// The recordings folder is treated as the source of truth rather than the
// database: a recording whose meeting row was never written — an interrupted
// session, or a deferred-transcription recording — exists only on disk, and a
// database-driven list would hide exactly the audio that needs attention.
//
// Operations that change a recording inside its folder (rename, delete,
// transcribe, summarise) act on the folder. `move` is the exception: it relocates
// the folder out of the scanned root, so the recording deliberately leaves the
// list and lives wherever the user put it.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// One saved recording folder.
#[derive(Debug, Clone, Serialize)]
pub struct RecordingEntry {
    /// Folder name, which encodes the meeting name and start time.
    pub folder_name: String,
    /// Absolute path to the recording folder.
    pub folder_path: String,
    /// Display name: the metadata title, falling back to the folder name.
    pub meeting_name: String,
    /// RFC 3339 creation time, when known.
    pub created_at: Option<String>,
    /// Recording length in seconds, when known.
    pub duration_seconds: Option<f64>,
    /// File name of the audio inside the folder, when one was found.
    pub audio_file: Option<String>,
    /// Absolute path of the audio file, when one was found.
    ///
    /// The UI needs the path rather than the name to transcribe a recording that
    /// has no database row, and to play it without re-deriving the folder layout.
    pub audio_path: Option<String>,
    /// Size of the audio file in bytes.
    pub audio_bytes: Option<u64>,
    /// Whether the recording holds transcribed speech.
    ///
    /// The recorder writes `transcripts.json` on every stop, even when deferred
    /// transcription captured nothing, so the file's presence is not evidence of a
    /// transcript. See `transcript_has_segments`.
    pub has_transcript: bool,
    /// Whether a summary file exists.
    pub has_summary: bool,
    /// `recording`, `completed` or `error` when metadata says so.
    pub status: Option<String>,
    /// Seconds since the Unix epoch, for ordering. 0 when undeterminable.
    pub sort_key: i64,
}

/// Audio extensions the app can produce, most current first.
///
/// Includes the historical `audio.mp4` name so recordings made before the format
/// setting worked are still found.
pub const AUDIO_FILE_CANDIDATES: &[&str] = &[
    "audio.m4a",
    "audio.mp4",
    "audio.mp3",
    "audio.wav",
    "audio.flac",
    "audio.ogg",
];

/// Frames written by the recorder that describe the meeting.
pub const METADATA_FILE: &str = "metadata.json";
pub const TRANSCRIPT_FILE: &str = "transcripts.json";
pub const SUMMARY_FILE: &str = "summary.json";

/// The app's own sub-directories, excluded from the list by name.
pub fn is_internal_folder_name(name: &str) -> bool {
    name.starts_with('.')
}

/// Whether a directory looks like a recording folder.
pub fn is_recording_folder(path: &Path) -> bool {
    path.is_dir() && path.join(METADATA_FILE).is_file()
}

/// Find the audio file inside a recording folder.
pub fn find_audio_file(folder: &Path) -> Option<PathBuf> {
    for candidate in AUDIO_FILE_CANDIDATES {
        let path = folder.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Whether a recording folder holds at least one transcribed segment.
///
/// A missing, unreadable, malformed or empty transcript all mean "nothing was
/// transcribed". The recorder creates `transcripts.json` on every stop, so a
/// file that merely exists would claim every recording is transcribed and hide
/// the Transcribe button for audio that still needs it.
fn transcript_has_segments(folder: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(folder.join(TRANSCRIPT_FILE)) else {
        return false;
    };

    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };

    value
        .get("segments")
        .and_then(|segments| segments.as_array())
        .is_some_and(|segments| !segments.is_empty())
}

fn modified_time(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn epoch_seconds(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse an RFC 3339 timestamp to epoch seconds.
pub fn parse_rfc3339_seconds(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp())
}

/// Read one recording folder.
///
/// Returns `None` only when the folder is not a recording folder. A folder whose
/// metadata cannot be read is still returned with those fields empty: an
/// unreadable metadata file must not hide the recording, which is when the audio
/// matters most.
pub fn read_recording_entry(folder: &Path) -> Option<RecordingEntry> {
    if !is_recording_folder(folder) {
        return None;
    }

    let folder_name = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let metadata = super::retention_commands::read_meeting_metadata(folder);

    let created_at = metadata
        .as_ref()
        .map(|m| m.created_at.clone())
        .filter(|s| !s.is_empty());
    let duration_seconds = metadata.as_ref().and_then(|m| m.duration_seconds);
    let status = metadata.as_ref().map(|m| m.status.clone());
    let meeting_name = metadata
        .as_ref()
        .and_then(|m| m.meeting_name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| folder_name.clone());

    let audio_path = find_audio_file(folder);
    let audio_file = audio_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string());
    let audio_bytes = audio_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());

    let sort_key = created_at
        .as_deref()
        .and_then(parse_rfc3339_seconds)
        .or_else(|| audio_path.as_deref().and_then(modified_time).map(epoch_seconds))
        .or_else(|| modified_time(folder).map(epoch_seconds))
        .unwrap_or(0);

    Some(RecordingEntry {
        folder_name,
        folder_path: folder.to_string_lossy().to_string(),
        meeting_name,
        created_at,
        duration_seconds,
        audio_file,
        audio_path: audio_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        audio_bytes,
        has_transcript: transcript_has_segments(folder),
        has_summary: folder.join(SUMMARY_FILE).is_file(),
        status,
        sort_key,
    })
}

/// List every recording under `root`, newest first.
///
/// Only immediate children are considered, and the app's own dot-directories are
/// skipped: `.checkpoints` holds audio chunks and must never be listed as a
/// meeting.
pub fn list_recordings(root: &Path) -> Vec<RecordingEntry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut recordings: Vec<RecordingEntry> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            !is_internal_folder_name(&name)
        })
        .filter_map(|entry| read_recording_entry(&entry.path()))
        .collect();

    recordings.sort_by(|a, b| {
        b.sort_key
            .cmp(&a.sort_key)
            .then_with(|| a.folder_name.cmp(&b.folder_name))
    });

    recordings
}

/// The trailing `_YYYY-MM-DD_HH-MM` stamp in a recording folder name.
///
/// Returns `None` when the folder does not carry one, in which case a rename
/// keeps the whole name as the title.
///
/// Works on characters rather than bytes: meeting names are frequently Chinese,
/// and slicing a `&str` at a byte offset derived from `len()` would panic on a
/// multi-byte boundary.
pub fn folder_timestamp_suffix(folder_name: &str) -> Option<&str> {
    const STAMP_LEN: usize = 16; // 2026-10-14_09-30

    let char_count = folder_name.chars().count();
    if char_count <= STAMP_LEN + 1 {
        return None;
    }

    // The stamp must be a whole trailing segment, so the character before it has
    // to be the separator. Without this, `name_2026-10-14_09-30extra` style names
    // would be mis-split.
    let separator_index = char_count - STAMP_LEN - 1;
    if folder_name.chars().nth(separator_index) != Some('_') {
        return None;
    }

    let stamp: String = folder_name.chars().skip(char_count - STAMP_LEN).collect();
    if stamp.len() != STAMP_LEN {
        return None;
    }

    // Separator positions in `YYYY-MM-DD_HH-MM`; every other position is a digit.
    const SEPARATORS: &[(usize, char)] = &[(4, '-'), (7, '-'), (10, '_'), (13, '-')];

    for (index, byte) in stamp.bytes().enumerate() {
        match SEPARATORS.iter().find(|(i, _)| *i == index) {
            Some((_, expected)) if *expected as u8 != byte => return None,
            Some(_) => {}
            None => {
                if !byte.is_ascii_digit() {
                    return None;
                }
            }
        }
    }

    // Safe to slice because the stamp is ASCII, so its byte length equals its
    // character count.
    let start = folder_name
        .char_indices()
        .nth(char_count - STAMP_LEN)
        .map(|(index, _)| index)?;

    folder_name.get(start..)
}

/// Build the folder name for a renamed recording.
///
/// Keeps the original time stamp so ordering survives a rename, and so two
/// recordings of the same name cannot collide.
pub fn renamed_folder_name(folder_name: &str, new_title: &str) -> String {
    let sanitized = crate::audio::audio_processing::sanitize_filename(new_title);
    let sanitized = sanitized.trim();

    match folder_timestamp_suffix(folder_name) {
        Some(stamp) if !sanitized.is_empty() => format!("{sanitized}_{stamp}"),
        // Without a stamp there is nothing to preserve; use the title alone.
        _ if !sanitized.is_empty() => sanitized.to_string(),
        // An empty title must not produce an empty folder name.
        _ => folder_name.to_string(),
    }
}

/// Human-readable size, for the UI.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format a duration as `m:ss` or `h:mm:ss`.
pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "—".to_string();
    }

    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_metadata(folder: &Path, created_at: &str, name: &str) {
        std::fs::create_dir_all(folder).expect("create folder");
        let json = serde_json::json!({
            "version": "1.0",
            "meeting_id": null,
            "meeting_name": name,
            "created_at": created_at,
            "completed_at": null,
            "duration_seconds": 1800.0,
            "devices": { "microphone": null, "system_audio": null },
            "audio_file": "audio.m4a",
            "transcript_file": "transcripts.json",
            "sample_rate": 48000,
            "status": "completed"
        });
        std::fs::write(folder.join(METADATA_FILE), json.to_string()).expect("write metadata");
    }

    #[test]
    fn a_folder_without_metadata_is_not_a_recording() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("random")).expect("create");
        assert!(read_recording_entry(&tmp.path().join("random")).is_none());
    }

    /// A uniquely named scratch folder under the OS temp directory.
    ///
    /// Removed when it drops, including when an assertion fails, so the tests
    /// leave nothing behind and need no fixture checked into the repository.
    struct ScratchFolder {
        path: PathBuf,
    }

    impl ScratchFolder {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};

            static COUNTER: AtomicU64 = AtomicU64::new(0);

            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let unique = format!(
                "meetily-recordings-test-{}-{}-{}",
                std::process::id(),
                nanos,
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );

            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("create scratch folder");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ScratchFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_transcript(folder: &Path, segments: serde_json::Value) {
        let json = serde_json::json!({
            "version": "1.0",
            "total_segments": segments.as_array().map(|a| a.len()).unwrap_or(0),
            "segments": segments,
        });
        std::fs::write(folder.join(TRANSCRIPT_FILE), json.to_string()).expect("write transcripts");
    }

    #[test]
    fn reads_the_basic_fields() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("Team Standup_2026-10-14_09-30");
        write_metadata(&folder, "2026-10-14T09:30:00Z", "Team Standup");
        std::fs::write(folder.join("audio.m4a"), b"audio-bytes").expect("write");

        let entry = read_recording_entry(&folder).expect("entry");

        assert_eq!(entry.meeting_name, "Team Standup");
        assert_eq!(entry.duration_seconds, Some(1800.0));
        assert_eq!(entry.audio_file.as_deref(), Some("audio.m4a"));
        assert_eq!(entry.audio_bytes, Some(11));
        assert!(!entry.has_transcript);
    }

    #[test]
    fn an_empty_transcript_does_not_count_as_transcribed() {
        // The recorder writes transcripts.json on every stop, so a file with an
        // empty segment list is the normal state of deferred transcription. If
        // that counted, the UI would hide the Transcribe button for audio that
        // was never transcribed — which is exactly the bug this fixes.
        let scratch = ScratchFolder::new();
        let folder = scratch.path().join("Deferred_2026-10-14_09-30");
        write_metadata(&folder, "2026-10-14T09:30:00Z", "Deferred");
        std::fs::write(folder.join("audio.mp3"), b"audio-bytes").expect("write audio");
        write_transcript(&folder, serde_json::json!([]));

        let entry = read_recording_entry(&folder).expect("entry");

        assert!(!entry.has_transcript, "an empty segment list is not a transcript");

        let expected_audio = folder.join("audio.mp3").to_string_lossy().to_string();
        assert_eq!(entry.audio_path.as_deref(), Some(expected_audio.as_str()));
    }

    #[test]
    fn a_transcript_with_a_segment_counts() {
        let scratch = ScratchFolder::new();
        let folder = scratch.path().join("Transcribed_2026-10-14_09-30");
        write_metadata(&folder, "2026-10-14T09:30:00Z", "Transcribed");
        std::fs::write(folder.join("audio.mp3"), b"audio-bytes").expect("write audio");
        write_transcript(
            &folder,
            serde_json::json!([{ "text": "Hello world", "sequence_id": 0 }]),
        );

        let entry = read_recording_entry(&folder).expect("entry");

        assert!(entry.has_transcript, "a stored segment must count as transcribed");
    }

    #[test]
    fn a_malformed_transcript_does_not_count() {
        // A half-written file must not claim the recording was transcribed; the
        // audio still needs transcribing and the UI must offer it.
        let scratch = ScratchFolder::new();
        let folder = scratch.path().join("Broken_2026-10-14_09-30");
        write_metadata(&folder, "2026-10-14T09:30:00Z", "Broken");
        std::fs::write(folder.join("audio.mp3"), b"audio-bytes").expect("write audio");
        std::fs::write(folder.join(TRANSCRIPT_FILE), b"{ not json").expect("write transcripts");

        let entry = read_recording_entry(&folder).expect("entry");

        assert!(!entry.has_transcript);

        // Valid JSON without a segments array is equally not a transcript.
        write_transcript(&folder, serde_json::Value::Null);
        let entry = read_recording_entry(&folder).expect("entry still listed");
        assert!(!entry.has_transcript);
    }

    #[test]
    fn finds_every_audio_extension() {
        for candidate in AUDIO_FILE_CANDIDATES {
            let tmp = tempfile::tempdir().expect("tempdir");
            let folder = tmp.path().join("m");
            write_metadata(&folder, "2026-01-01T00:00:00Z", "m");
            std::fs::write(folder.join(candidate), b"x").expect("write");

            assert_eq!(
                find_audio_file(&folder)
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())),
                Some((*candidate).to_string())
            );
        }
    }

    #[test]
    fn prefers_current_format_over_legacy_mp4() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("m");
        write_metadata(&folder, "2026-01-01T00:00:00Z", "m");
        std::fs::write(folder.join("audio.mp4"), b"old").expect("write");
        std::fs::write(folder.join("audio.m4a"), b"new").expect("write");

        assert_eq!(find_audio_file(&folder).unwrap().file_name().unwrap(), "audio.m4a");
    }

    #[test]
    fn unreadable_metadata_does_not_hide_the_recording() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let folder = tmp.path().join("Broken_2026-01-01_00-00");
        std::fs::create_dir_all(&folder).expect("create");
        std::fs::write(folder.join(METADATA_FILE), b"{ not json").expect("write");
        std::fs::write(folder.join("audio.m4a"), b"x").expect("write");

        let entry = read_recording_entry(&folder).expect("still listed");
        assert_eq!(entry.meeting_name, "Broken_2026-01-01_00-00");
    }

    #[test]
    fn list_returns_newest_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_metadata(&tmp.path().join("old"), "2026-01-01T00:00:00Z", "Old");
        write_metadata(&tmp.path().join("new"), "2026-06-01T00:00:00Z", "New");
        write_metadata(&tmp.path().join("mid"), "2026-03-01T00:00:00Z", "Mid");

        let names: Vec<String> = list_recordings(tmp.path())
            .into_iter()
            .map(|e| e.meeting_name)
            .collect();

        assert_eq!(names, vec!["New", "Mid", "Old"]);
    }

    #[test]
    fn list_ignores_files_and_unrelated_folders() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_metadata(&tmp.path().join("real"), "2026-01-01T00:00:00Z", "Real");
        std::fs::write(tmp.path().join("stray.txt"), b"x").expect("write");
        std::fs::create_dir_all(tmp.path().join("not-ours")).expect("create");

        assert_eq!(list_recordings(tmp.path()).len(), 1);
    }

    #[test]
    fn checkpoint_directories_are_never_listed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let checkpoints = tmp.path().join(".checkpoints");
        std::fs::create_dir_all(&checkpoints).expect("create");
        std::fs::write(checkpoints.join(METADATA_FILE), b"{}").expect("write");

        assert!(list_recordings(tmp.path()).is_empty());
    }

    #[test]
    fn list_on_a_missing_root_is_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(list_recordings(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn detects_the_folder_timestamp() {
        assert_eq!(
            folder_timestamp_suffix("Team Standup_2026-10-14_09-30"),
            Some("2026-10-14_09-30")
        );
        // A name containing underscores still splits on the stamp.
        assert_eq!(
            folder_timestamp_suffix("a_b_c_2026-01-02_03-04"),
            Some("2026-01-02_03-04")
        );
    }

    #[test]
    fn rejects_names_without_a_timestamp() {
        assert_eq!(folder_timestamp_suffix("no-stamp"), None);
        assert_eq!(folder_timestamp_suffix("2026-10-14_09-30"), None);
        assert_eq!(folder_timestamp_suffix("name_2026-10-14"), None);
        assert_eq!(folder_timestamp_suffix("name_2026-10-14_09-3x"), None);
        assert_eq!(folder_timestamp_suffix(""), None);
    }

    #[test]
    fn rename_preserves_the_timestamp() {
        // Keeping the stamp is what stops a rename from breaking ordering or
        // colliding with another recording of the same name.
        assert_eq!(
            renamed_folder_name("Old Name_2026-10-14_09-30", "New Name"),
            "New Name_2026-10-14_09-30"
        );
    }

    #[test]
    fn rename_sanitizes_illegal_characters() {
        assert_eq!(
            renamed_folder_name("x_2026-10-14_09-30", "a/b:c*d"),
            "a_b_c_d_2026-10-14_09-30"
        );
    }

    #[test]
    fn rename_keeps_the_original_name_when_the_title_is_blank() {
        // An empty folder name would be unusable.
        assert_eq!(
            renamed_folder_name("Keep Me_2026-10-14_09-30", "   "),
            "Keep Me_2026-10-14_09-30"
        );
    }

    #[test]
    fn rename_works_without_a_timestamp() {
        assert_eq!(renamed_folder_name("plain", "Better"), "Better");
    }

    #[test]
    fn formats_sizes_readably() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn formats_durations_readably() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(65.0), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }

    #[test]
    fn format_duration_handles_unknown_values() {
        assert_eq!(format_duration(f64::NAN), "—");
        assert_eq!(format_duration(-1.0), "—");
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(parse_rfc3339_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert!(parse_rfc3339_seconds("nope").is_none());
    }

    #[test]
    fn timestamp_detection_survives_multibyte_names() {
        // Chinese meeting names are common; slicing by byte offset would panic.
        let name = "团队周会_2026-10-14_09-30";
        assert_eq!(folder_timestamp_suffix(name), Some("2026-10-14_09-30"));
        assert_eq!(renamed_folder_name(name, "新名字"), "新名字_2026-10-14_09-30");
    }

    #[test]
    fn a_stamp_must_be_a_whole_trailing_segment() {
        // A name that merely ends with digits must not be treated as stamped.
        assert_eq!(folder_timestamp_suffix("name_2026-10-14_09-30extra"), None);
        assert_eq!(folder_timestamp_suffix("name-2026-10-14_09-30"), None);
    }
}
