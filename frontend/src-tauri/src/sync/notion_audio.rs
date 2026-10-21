//! Audio attachment for Notion sync.
//!
//! Notion's File Upload API accepts at most 20 MiB in a single-part upload, and
//! the workspace may impose a lower limit. Meeting audio is usually larger, so it
//! is re-encoded as mono AAC at the bitrate that fits its duration into the limit
//! with headroom. Everything after the page exists is best effort: a failed
//! upload is reported as a skipped reason instead of losing the whole note.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::database::repositories::transcript::TranscriptsRepository;

use super::notion;

/// Workspace upload limit assumed when Notion does not report one.
pub(crate) const DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES: u64 = 5 * 1024 * 1024;
/// Notion's single-part upload ceiling.
pub(crate) const MAX_SINGLE_PART_UPLOAD_BYTES: u64 = 20 * 1024 * 1024;
/// Fraction of the limit the compressed audio is allowed to use.
const SIZE_HEADROOM: f64 = 0.85;
const MIN_AUDIO_BITRATE_KBPS: u32 = 6;
const MAX_AUDIO_BITRATE_KBPS: u32 = 48;
const USERS_ME_URL: &str = "https://api.notion.com/v1/users/me";
const FILE_UPLOADS_URL: &str = "https://api.notion.com/v1/file_uploads";

/// Audio that is ready to upload, or a reason none can be attached.
pub(crate) enum PreparedAudio {
    /// Nothing to upload; the note itself is still synced.
    Skip(String),
    /// Bytes to upload, already compressed when the original was too large.
    Upload {
        bytes: Vec<u8>,
        filename: String,
        content_type: &'static str,
    },
}

/// Outcome of attaching prepared audio once the page exists.
pub(crate) struct AudioAttachment {
    pub uploaded: bool,
    pub skipped_reason: Option<String>,
}

impl AudioAttachment {
    fn uploaded() -> Self {
        Self {
            uploaded: true,
            skipped_reason: None,
        }
    }

    fn skipped(reason: String) -> Self {
        Self {
            uploaded: false,
            skipped_reason: Some(reason),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FileUploadResponse {
    id: String,
    upload_url: String,
    #[serde(default)]
    status: Option<String>,
}

/// Content type Notion should record for a recording file.
pub(crate) fn audio_content_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();

    match extension.as_str() {
        "m4a" | "mp4" => Some("audio/mp4"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "ogg" | "opus" => Some("audio/ogg"),
        "webm" => Some("audio/webm"),
        "flac" => Some("audio/flac"),
        _ => None,
    }
}

/// The workspace upload limit from the GET /v1/users/me response.
///
/// A missing, zero or malformed value falls back to the documented 5 MiB default
/// rather than refusing to sync.
pub(crate) fn workspace_upload_limit_bytes(profile: &Value) -> u64 {
    profile
        .get("bot")
        .and_then(|bot| bot.get("workspace_limits"))
        .and_then(|limits| limits.get("max_file_upload_size_in_bytes"))
        .and_then(Value::as_u64)
        .filter(|limit| *limit > 0)
        .unwrap_or(DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES)
}

/// Bitrate (kbps) that fits the duration into the byte limit with headroom.
///
/// Returns None when the duration is unusable, and clamps into the range that is
/// intelligible for mono speech but small enough for tight limits.
pub(crate) fn target_bitrate_kbps(limit_bytes: u64, duration_seconds: f64) -> Option<u32> {
    if limit_bytes == 0 || !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return None;
    }

    let usable_bits = limit_bytes as f64 * SIZE_HEADROOM * 8.0;
    let kbps = (usable_bits / duration_seconds / 1000.0).floor();

    if !kbps.is_finite() || kbps <= 0.0 {
        return Some(MIN_AUDIO_BITRATE_KBPS);
    }

    Some(
        (kbps as i64).clamp(
            MIN_AUDIO_BITRATE_KBPS as i64,
            MAX_AUDIO_BITRATE_KBPS as i64,
        ) as u32,
    )
}

/// The block that references an uploaded file.
pub(crate) fn audio_block(file_upload_id: &str) -> Value {
    json!({
        "object": "block",
        "type": "audio",
        "audio": {
            "type": "file_upload",
            "file_upload": { "id": file_upload_id },
        },
    })
}

/// A temporary compressed file that is removed on every exit path.
struct TemporaryAudio(PathBuf);

impl Drop for TemporaryAudio {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Could not remove temporary audio file {}: {}",
                    self.0.display(),
                    error
                );
            }
        }
    }
}

/// Fallback display name for a compressed upload.
fn compressed_upload_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                '-'
            } else {
                character
            }
        })
        .collect();

    let cleaned: String = cleaned
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(80)
        .collect();

    if cleaned.is_empty() {
        "meeting-audio.m4a".to_string()
    } else {
        format!("{cleaned}.m4a")
    }
}

/// Resolve the meeting's audio and make it fit Notion's upload limit.
///
/// Runs before the page is created: a compression or size failure must not leave
/// a page without audio. A meeting that simply has no recording is a Skip, not an
/// error, because the note itself is still worth syncing.
pub(crate) async fn prepare_audio(
    pool: &sqlx::SqlitePool,
    client: &reqwest::Client,
    token: &str,
    meeting_id: &str,
    title: &str,
) -> Result<PreparedAudio, String> {
    let folder_path: Option<String> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT folder_path FROM meetings WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| format!("Could not look up the meeting's recording folder: {error}"))?
    .flatten();

    let Some(folder_path) = folder_path else {
        return Ok(PreparedAudio::Skip(
            "This meeting has no recording folder, so there is no audio to upload.".to_string(),
        ));
    };

    let folder = PathBuf::from(folder_path);
    let Some(audio_path) = crate::audio::recordings::find_audio_file(&folder) else {
        return Ok(PreparedAudio::Skip(
            "The meeting folder does not contain an audio file.".to_string(),
        ));
    };

    let Some(content_type) = audio_content_type(&audio_path) else {
        return Ok(PreparedAudio::Skip(
            "The recording is in a format Notion cannot play.".to_string(),
        ));
    };

    let limit = effective_upload_limit(client, token).await?;
    let source_size = std::fs::metadata(&audio_path)
        .map_err(|error| format!("Could not read the recording's size: {error}"))?
        .len();

    if source_size <= limit {
        let bytes = tokio::fs::read(&audio_path)
            .await
            .map_err(|error| format!("Could not read the recording: {error}"))?;
        let filename = audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("meeting-audio")
            .to_string();

        return Ok(PreparedAudio::Upload {
            bytes,
            filename,
            content_type,
        });
    }

    let duration = TranscriptsRepository::meeting_duration_seconds(pool, meeting_id)
        .await
        .map_err(|error| format!("Could not read the recording's duration: {error}"))?
        .filter(|duration| duration.is_finite() && *duration > 0.0)
        .ok_or_else(|| {
            "The recording is larger than Notion's upload limit and its duration is unknown, so it cannot be compressed to fit."
                .to_string()
        })?;

    let bitrate = target_bitrate_kbps(limit, duration).ok_or_else(|| {
        "The recording is larger than Notion's upload limit and cannot be compressed to fit."
            .to_string()
    })?;

    let temp_path =
        std::env::temp_dir().join(format!("meetily-notion-audio-{}.m4a", uuid::Uuid::new_v4()));
    let temporary = TemporaryAudio(temp_path.clone());

    let input = audio_path.clone();
    let output = temp_path.clone();
    tokio::task::spawn_blocking(move || {
        crate::audio::encode::transcode_audio_to_mono_aac(&input, &output, bitrate)
    })
    .await
    .map_err(|error| format!("The audio compression task failed: {error}"))?
    .map_err(|error| format!("The recording could not be compressed for Notion: {error}"))?;

    let compressed_size = std::fs::metadata(temporary.0.as_path())
        .map_err(|error| format!("Could not read the compressed recording's size: {error}"))?
        .len();

    if compressed_size > limit {
        return Err(format!(
            "The recording could not be compressed to fit Notion's upload limit of {} bytes (compressed size: {} bytes).",
            limit, compressed_size
        ));
    }

    let bytes = tokio::fs::read(temporary.0.as_path())
        .await
        .map_err(|error| format!("Could not read the compressed recording: {error}"))?;

    Ok(PreparedAudio::Upload {
        bytes,
        filename: compressed_upload_name(title),
        content_type: "audio/mp4",
    })
}

/// The smallest of the workspace limit and Notion's single-part maximum.
async fn effective_upload_limit(client: &reqwest::Client, token: &str) -> Result<u64, String> {
    let profile = notion::execute(
        || notion::authorized_request(client, reqwest::Method::GET, USERS_ME_URL, token),
        None,
    )
    .await
    .map_err(|error| format!("Could not read Notion's upload limit: {error}"))?;

    Ok(workspace_upload_limit_bytes(&profile).min(MAX_SINGLE_PART_UPLOAD_BYTES))
}

/// Upload prepared audio through the File Upload API and attach it to the page.
///
/// Never returns Err: once the page exists the caller must keep it, so failures
/// become a skipped reason on the result.
pub(crate) async fn attach_prepared_audio(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    prepared: PreparedAudio,
) -> AudioAttachment {
    let (bytes, filename, content_type) = match prepared {
        PreparedAudio::Skip(reason) => return AudioAttachment::skipped(reason),
        PreparedAudio::Upload {
            bytes,
            filename,
            content_type,
        } => (bytes, filename, content_type),
    };

    match upload_and_attach(client, token, page_id, bytes, filename, content_type).await {
        Ok(()) => AudioAttachment::uploaded(),
        Err(reason) => {
            log::warn!(
                "Notion audio upload failed for page {}: {}",
                page_id,
                reason
            );
            AudioAttachment::skipped(reason)
        }
    }
}

async fn upload_and_attach(
    client: &reqwest::Client,
    token: &str,
    page_id: &str,
    bytes: Vec<u8>,
    filename: String,
    content_type: &'static str,
) -> Result<(), String> {
    let content_length = bytes.len() as u64;

    let create_body = json!({
        "mode": "single_part",
        "filename": filename,
        "content_type": content_type,
        "content_length": content_length,
    });

    let created = notion::execute(
        || {
            notion::authorized_request(client, reqwest::Method::POST, FILE_UPLOADS_URL, token)
                .json(&create_body)
        },
        None,
    )
    .await
    .map_err(|error| format!("Notion refused the audio upload request: {error}"))?;

    let upload: FileUploadResponse = serde_json::from_value(created)
        .map_err(|error| format!("Could not read Notion's upload response: {error}"))?;

    if let Some(status) = upload.status.as_deref() {
        if status != "pending" && status != "uploaded" {
            return Err(format!(
                "Notion reported the upload as '{}' instead of accepting it.",
                status
            ));
        }
    }

    let bytes = Arc::new(bytes);
    let upload_url = upload.upload_url.clone();
    let filename_for_part = filename.clone();

    let uploaded = notion::execute(
        || {
            let part = reqwest::multipart::Part::bytes((*bytes).clone())
                .file_name(filename_for_part.clone())
                .mime_str(content_type)
                .expect("static content type is a valid mime type");
            let form = reqwest::multipart::Form::new().part("file", part);

            notion::authorized_request(client, reqwest::Method::POST, &upload_url, token)
                .multipart(form)
        },
        None,
    )
    .await
    .map_err(|error| format!("Notion could not receive the audio file: {error}"))?;

    let status = uploaded.get("status").and_then(Value::as_str).unwrap_or("");

    if status != "uploaded" {
        return Err(format!(
            "Notion did not confirm the audio upload (reported status: '{}').",
            if status.is_empty() { "unknown" } else { status }
        ));
    }

    notion::append_children(client, token, page_id, vec![audio_block(&upload.id)])
        .await
        .map_err(|error| {
            format!("Notion stored the audio but could not attach it to the page: {error}")
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_match_notion_upload_types() {
        let cases = [
            ("audio.m4a", Some("audio/mp4")),
            ("audio.mp4", Some("audio/mp4")),
            ("audio.mp3", Some("audio/mpeg")),
            ("audio.wav", Some("audio/wav")),
            ("audio.ogg", Some("audio/ogg")),
            ("audio.opus", Some("audio/ogg")),
            ("audio.webm", Some("audio/webm")),
            ("audio.flac", Some("audio/flac")),
        ];

        for (name, expected) in cases {
            assert_eq!(audio_content_type(Path::new(name)), expected, "{name}");
        }

        assert_eq!(audio_content_type(Path::new("notes.txt")), None);
        assert_eq!(audio_content_type(Path::new("no-extension")), None);
        assert_eq!(audio_content_type(Path::new("UPPER.M4A")), Some("audio/mp4"));
    }

    #[test]
    fn workspace_limit_defaults_and_parses_the_notion_profile() {
        let reported = json!({
            "object": "user",
            "bot": {
                "workspace_limits": { "max_file_upload_size_in_bytes": 7_340_032u64 }
            }
        });
        assert_eq!(workspace_upload_limit_bytes(&reported), 7_340_032);

        // Missing, zero and malformed values all fall back to the 5 MiB default.
        assert_eq!(
            workspace_upload_limit_bytes(&json!({ "object": "user" })),
            DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES
        );
        assert_eq!(
            workspace_upload_limit_bytes(&json!({ "bot": { "workspace_limits": {} } })),
            DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES
        );
        assert_eq!(
            workspace_upload_limit_bytes(
                &json!({ "bot": { "workspace_limits": { "max_file_upload_size_in_bytes": 0 } } })
            ),
            DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES
        );
        assert_eq!(
            workspace_upload_limit_bytes(
                &json!({ "bot": { "workspace_limits": { "max_file_upload_size_in_bytes": "lots" } } })
            ),
            DEFAULT_WORKSPACE_UPLOAD_LIMIT_BYTES
        );
    }

    #[test]
    fn target_bitrate_uses_headroom_and_clamps() {
        let five_mib = 5 * 1024 * 1024;

        // 10 minutes at 5 MiB is well above the 48 kbps ceiling.
        assert_eq!(target_bitrate_kbps(five_mib, 600.0), Some(48));
        // An hour at 5 MiB fits comfortably below the ceiling and follows the formula.
        let hour = target_bitrate_kbps(five_mib, 3600.0).expect("bitrate");
        assert!((6..=48).contains(&hour), "hour bitrate out of range: {hour}");
        assert_eq!(
            hour,
            ((five_mib as f64 * 0.85 * 8.0) / 3600.0 / 1000.0).floor() as u32
        );
        // A tiny limit for an hour clamps up to the intelligible minimum.
        assert_eq!(target_bitrate_kbps(256 * 1024, 3600.0), Some(6));
        // Unusable input has no answer.
        assert_eq!(target_bitrate_kbps(0, 60.0), None);
        assert_eq!(target_bitrate_kbps(five_mib, 0.0), None);
        assert_eq!(target_bitrate_kbps(five_mib, f64::NAN), None);
        assert_eq!(target_bitrate_kbps(five_mib, -5.0), None);
    }

    #[test]
    fn the_audio_block_references_a_file_upload() {
        let block = audio_block("file-upload-id");

        assert_eq!(block["object"], "block");
        assert_eq!(block["type"], "audio");
        assert_eq!(block["audio"]["type"], "file_upload");
        assert_eq!(block["audio"]["file_upload"]["id"], "file-upload-id");
    }
}
