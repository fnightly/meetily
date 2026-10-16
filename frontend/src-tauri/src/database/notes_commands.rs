//! Tauri commands for user-authored meeting notes, including note images.
//!
//! Note images live on disk rather than as data URLs inside the database: a few
//! photos would otherwise bloat the SQLite file and slow every note load. Image
//! bytes arrive from the frontend as a byte array, which avoids pulling in a
//! base64 dependency just to decode a data URL.

use log::{info as log_info, warn as log_warn};
use serde::Serialize;
use tauri::{AppHandle, Runtime};

use super::repositories::meeting_notes::{MeetingNote, MeetingNotesRepository, NoteInput};
use crate::state::AppState;

/// A note image that has been written to the notes folder.
#[derive(Debug, Serialize)]
pub struct SavedNoteImage {
    /// Absolute path to the stored file, used as the note block reference.
    pub path: String,
    pub file_name: String,
    pub bytes: usize,
}

/// Validate a meeting id the same way the rest of the data layer does.
fn require_meeting_id(meeting_id: &str) -> Result<(), String> {
    if meeting_id.trim().is_empty() {
        return Err("meeting_id cannot be empty".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn api_get_meeting_note<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingNote, String> {
    require_meeting_id(&meeting_id)?;
    MeetingNotesRepository::get(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting note: {e}"))
}

#[tauri::command]
pub async fn api_save_meeting_note<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    note: NoteInput,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingNote, String> {
    require_meeting_id(&meeting_id)?;
    MeetingNotesRepository::save(state.db_manager.pool(), &meeting_id, &note)
        .await
        .map_err(|e| format!("Failed to save meeting note: {e}"))
}

#[tauri::command]
pub async fn api_delete_meeting_note<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    require_meeting_id(&meeting_id)?;
    MeetingNotesRepository::delete(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to delete meeting note: {e}"))
}

/// Whether a meeting has a non-empty note (used to badge the sidebar).
#[tauri::command]
pub async fn api_meeting_has_note<R: Runtime>(
    _app: AppHandle<R>,
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    require_meeting_id(&meeting_id)?;
    MeetingNotesRepository::has_content(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to check meeting note: {e}"))
}

/// Where note images are stored for this app instance.
#[tauri::command]
pub async fn api_get_note_images_dir<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    Ok(note_images_dir(&app)?.to_string_lossy().to_string())
}

/// Resolve the directory that holds note images, creating it if needed.
///
/// Lives under the configured notes folder so moving the notes location moves
/// the images with it.
fn note_images_dir<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    let notes_root = crate::storage::load_storage_config(app).notes_folder(app)?;
    let dir = notes_root.join("images");

    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create note images directory {}: {e}", dir.display()))?;
    }

    Ok(dir)
}

/// Strip any directory components and unsafe characters from a file name.
fn sanitize_file_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("image")
        .trim();

    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Reject the "all dots" case, which would produce a hidden/odd file name and
    // provides no extension.
    if cleaned.trim_matches('.').is_empty() {
        "image.png".to_string()
    } else {
        cleaned
    }
}

/// Save pasted or dropped image bytes into the notes folder.
///
/// Returns the absolute path to reference from a note block.
#[tauri::command]
pub async fn api_save_note_image<R: Runtime>(
    app: AppHandle<R>,
    file_name: String,
    bytes: Vec<u8>,
) -> Result<SavedNoteImage, String> {
    if bytes.is_empty() {
        return Err("Image data is empty".to_string());
    }

    const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "Image is too large ({} bytes). The limit is {} bytes.",
            bytes.len(),
            MAX_IMAGE_BYTES
        ));
    }

    let dir = note_images_dir(&app)?;
    let safe_name = sanitize_file_name(&file_name);

    // Prefix with a timestamp so two pastes of "image.png" cannot collide and
    // silently overwrite an image already referenced by a note.
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let final_name = format!("{}-{}", stamp, safe_name);
    let path = dir.join(&final_name);

    std::fs::write(&path, &bytes)
        .map_err(|e| format!("Failed to write image to {}: {e}", path.display()))?;

    log_info!("Saved note image ({} bytes) to {:?}", bytes.len(), path);

    Ok(SavedNoteImage {
        path: path.to_string_lossy().to_string(),
        file_name: final_name,
        bytes: bytes.len(),
    })
}

/// Delete a note image, but only if it is inside the notes images directory.
///
/// The containment check matters: the path comes from note content, and without
/// it a crafted note could ask the backend to delete an arbitrary file.
#[tauri::command]
pub async fn api_delete_note_image<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<bool, String> {
    let dir = note_images_dir(&app)?;
    let target = std::path::Path::new(&path);

    let canonical_dir = dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve note images dir: {e}"))?;

    let canonical_target = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            log_warn!("Note image not found, nothing to delete: {}", path);
            return Ok(false);
        }
    };

    if !canonical_target.starts_with(&canonical_dir) {
        return Err("Refusing to delete a file outside the notes images directory".to_string());
    }

    std::fs::remove_file(&canonical_target)
        .map_err(|e| format!("Failed to delete image {}: {e}", canonical_target.display()))?;

    log_info!("Deleted note image {:?}", canonical_target);
    Ok(true)
}

/// Guess an image MIME type from its extension.
fn image_mime_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// Read a note image and return it as a data URL.
///
/// The editor cannot use Tauri's asset protocol for note images: the notes
/// folder is user-configurable, while `tauri.conf.json`'s `assetProtocol.scope`
/// only covers `$APPDATA/**`. Serving the bytes through a checked command keeps
/// images working wherever the user puts their notes, and the same containment
/// check as deletion applies so this cannot be used to read arbitrary files.
#[tauri::command]
pub async fn api_read_note_image<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<String, String> {
    let dir = note_images_dir(&app)?;
    let canonical_dir = dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve note images dir: {e}"))?;

    let canonical_target = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("Note image not found at {path}: {e}"))?;

    if !canonical_target.starts_with(&canonical_dir) {
        return Err("Refusing to read a file outside the notes images directory".to_string());
    }

    let bytes = std::fs::read(&canonical_target)
        .map_err(|e| format!("Failed to read image {}: {e}", canonical_target.display()))?;

    let ext = canonical_target
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    Ok(format!("data:{};base64,{}", image_mime_for(ext), base64_encode(&bytes)))
}

/// Minimal standard base64 encoder.
///
/// Hand-rolled to avoid adding a dependency for the single place that needs it
/// (embedding image bytes in a data URL for display).
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_all_byte_values() {
        let all: Vec<u8> = (0u8..=255).collect();
        let encoded = base64_encode(&all);
        // 256 bytes -> 344 chars including padding.
        assert_eq!(encoded.len(), 344);
        assert!(encoded.ends_with("=="));
    }

    #[test]
    fn sanitize_file_name_strips_traversal_and_separators() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("a/b/c.png"), "c.png");
        assert_eq!(sanitize_file_name(r"..\..\win.ini"), "win.ini");
        // Unsafe characters are replaced, extension preserved.
        assert_eq!(sanitize_file_name("my photo!.png"), "my_photo_.png");
    }

    #[test]
    fn sanitize_file_name_never_returns_empty_or_dotfile() {
        assert_eq!(sanitize_file_name(""), "image.png");
        assert_eq!(sanitize_file_name("..."), "image.png");
        assert_eq!(sanitize_file_name("///"), "image.png");
    }

    #[test]
    fn image_mime_mapping_is_case_insensitive() {
        assert_eq!(image_mime_for("PNG"), "image/png");
        assert_eq!(image_mime_for("Jpeg"), "image/jpeg");
        assert_eq!(image_mime_for("webp"), "image/webp");
        assert_eq!(image_mime_for("bin"), "application/octet-stream");
    }
}
