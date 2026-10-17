use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use anyhow::Result;
#[cfg(target_os = "macos")]
use log::error;

#[cfg(target_os = "macos")]
use crate::audio::capture::AudioCaptureBackend;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordingPreferences {
    pub save_folder: PathBuf,
    pub auto_save: bool,
    /// Output format identifier (`m4a`, `mp3`, `wav`, `flac`, `ogg`).
    ///
    /// Legacy values are accepted: `mp4`/`aac` are read as `m4a`, which is what
    /// they always actually produced.
    pub file_format: String,
    /// Encoding quality for lossy formats (`low`, `medium`, `high`, `best`).
    /// Lossless formats ignore it.
    #[serde(default = "default_file_quality")]
    pub file_quality: String,
    /// When true, recording does not run realtime transcription: only audio is
    /// captured, and transcription is expected to be run afterwards on the
    /// saved file (via the retranscription path). This trades live captions for
    /// a much lighter recording session (no ASR inference competing with
    /// capture, no realtime VAD pass).
    #[serde(default)]
    pub defer_transcription: bool,

    /// When true, the whole transcript is rewritten by a local model once the
    /// recording stops, to drop filler words and fix spoken punctuation.
    ///
    /// Off by default: cleanup rewrites the user's own words, so it is a lossy
    /// transformation that has to be opted into.
    #[serde(default)]
    pub cleanup_after_recording: bool,

    /// Stop automatically after this many seconds with no audible input.
    ///
    /// Zero disables the feature, which is the default: stopping on silence can
    /// cut off a quiet ending, so it must be opted into.
    #[serde(default)]
    pub silence_stop_enabled_seconds: u32,

    /// Hard cap on recording length in seconds. Zero means no cap.
    ///
    /// A safety net for the common "left it recording" case; it also bounds the
    /// damage when a meeting runs long unattended.
    #[serde(default)]
    pub max_recording_seconds: u32,

    /// Delete recordings older than this many days. Zero keeps them forever.
    ///
    /// Off by default: silently deleting a user's recordings is not something to
    /// do without an explicit choice.
    #[serde(default)]
    pub recordings_retention_days: u32,

    /// Warn when free space on the recordings volume drops below this many MB.
    /// Zero disables the warning.
    #[serde(default = "default_low_disk_warning_mb")]
    pub low_disk_warning_mb: u32,

    /// How long the local speech model stays loaded after use:
    /// `auto` (default), `onDemand`, `keepWarm`, `always`.
    /// See `audio::transcription::residency`.
    #[serde(default = "default_model_residency")]
    pub model_residency: String,
    /// Idle minutes before a warm model is released (`keepWarm` / `auto`).
    #[serde(default = "default_keep_warm_minutes")]
    pub model_keep_warm_minutes: u32,

    #[serde(default)]
    pub preferred_mic_device: Option<String>,
    #[serde(default)]
    pub preferred_system_device: Option<String>,
    #[cfg(target_os = "macos")]
    #[serde(default)]
    pub system_audio_backend: Option<String>,
}

/// Serde default for `file_quality`.
fn default_file_quality() -> String {
    super::formats::RecordingQuality::default().id().to_string()
}

/// Warn below 1 GB free by default.
///
/// A one-hour lossy recording is tens of MB, but a WAV recording is ~330 MB, so
/// 1 GB leaves room for a long session before the disk is actually a problem.
fn default_model_residency() -> String {
    "auto".to_string()
}

fn default_keep_warm_minutes() -> u32 {
    super::transcription::residency::DEFAULT_KEEP_WARM_MINUTES
}

fn default_low_disk_warning_mb() -> u32 {
    1024
}

impl RecordingPreferences {
    /// Resolve the stored strings into validated encoding settings.
    ///
    /// Unknown or corrupt values fall back to the defaults instead of failing, so
    /// a bad preferences file cannot block recording.
    pub fn encoding_settings(&self) -> super::formats::AudioEncodingSettings {
        super::formats::AudioEncodingSettings::from_stored(&self.file_format, &self.file_quality)
    }
}

impl Default for RecordingPreferences {
    fn default() -> Self {
        Self {
            save_folder: get_default_recordings_folder(),
            auto_save: true,
            file_format: super::formats::RecordingFormat::default().id().to_string(),
            file_quality: default_file_quality(),
            // Transcribe after the recording, not during it.
            //
            // Live transcription runs the speech model on the same machine that is
            // capturing audio, and on a CPU-only setup the two compete: a slow decode
            // can starve the capture callback and show up as glitches in the saved
            // track. Deferring keeps the recording itself clean, which is the part that
            // cannot be redone. The audio lands in the library and can be transcribed
            // from there, so nothing is lost — only reordered.
            defer_transcription: true,
            cleanup_after_recording: false,
            silence_stop_enabled_seconds: 0,
            max_recording_seconds: 0,
            recordings_retention_days: 0,
            low_disk_warning_mb: default_low_disk_warning_mb(),
            model_residency: default_model_residency(),
            model_keep_warm_minutes: default_keep_warm_minutes(),
            preferred_mic_device: None,
            preferred_system_device: None,
            #[cfg(target_os = "macos")]
            system_audio_backend: Some("coreaudio".to_string()),
        }
    }
}

/// Get the default recordings folder based on platform
pub fn get_default_recordings_folder() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // Windows: %USERPROFILE%\Music\meetily-recordings
        if let Some(music_dir) = dirs::audio_dir() {
            music_dir.join("meetily-recordings")
        } else {
            // Fallback to Documents if Music folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meetily-recordings")
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Movies/meetily-recordings
        if let Some(movies_dir) = dirs::video_dir() {
            movies_dir.join("meetily-recordings")
        } else {
            // Fallback to Documents if Movies folder is not available
            dirs::document_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meetily-recordings")
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Linux/Others: ~/Documents/meetily-recordings
        dirs::document_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("meetily-recordings")
    }
}

/// Ensure the recordings directory exists
pub fn ensure_recordings_directory(path: &PathBuf) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        info!("Created recordings directory: {:?}", path);
    }
    Ok(())
}

/// Generate a unique filename for a recording
pub fn generate_recording_filename(format: &str) -> String {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S");
    format!("recording_{}.{}", timestamp, format)
}

/// Load recording preferences from store
pub async fn load_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<RecordingPreferences> {
    // Try to load from Tauri store
    let store = match app.store("recording_preferences.json") {
        Ok(store) => store,
        Err(e) => {
            warn!("Failed to access store: {}, using defaults", e);
            return Ok(RecordingPreferences::default());
        }
    };

    // Try to get the preferences from store
    let prefs = if let Some(value) = store.get("preferences") {
        match serde_json::from_value::<RecordingPreferences>(value.clone()) {
            // `mut` is only needed on macOS, where the backend field is refreshed
            // from the current capture backend below.
            #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
            Ok(mut p) => {
                info!("Loaded recording preferences from store");
                // Update macOS backend to current value if needed
                #[cfg(target_os = "macos")]
                {
                    let backend = crate::audio::capture::get_current_backend();
                    p.system_audio_backend = Some(backend.to_string());
                }
                p
            }
            Err(e) => {
                warn!("Failed to deserialize preferences: {}, using defaults", e);
                RecordingPreferences::default()
            }
        }
    } else {
        info!("No stored preferences found, using defaults");
        RecordingPreferences::default()
    };

    // Logs the switches that change what gets written, so the effective settings
    // can be confirmed from a log rather than inferred from behaviour.
    info!("Loaded recording preferences: save_folder={:?}, auto_save={}, format={}, quality={}, defer_transcription={}, cleanup_after_recording={}, mic={:?}, system={:?}",
          prefs.save_folder, prefs.auto_save, prefs.file_format, prefs.file_quality,
          prefs.defer_transcription, prefs.cleanup_after_recording,
          prefs.preferred_mic_device, prefs.preferred_system_device);
    Ok(prefs)
}

/// Save recording preferences to store
pub async fn save_recording_preferences<R: Runtime>(
    app: &AppHandle<R>,
    preferences: &RecordingPreferences,
) -> Result<()> {
    info!("Saving recording preferences: save_folder={:?}, auto_save={}, format={}, mic={:?}, system={:?}",
          preferences.save_folder, preferences.auto_save, preferences.file_format,
          preferences.preferred_mic_device, preferences.preferred_system_device);

    // Get or create store
    let store = app
        .store("recording_preferences.json")
        .map_err(|e| anyhow::anyhow!("Failed to access store: {}", e))?;

    // Serialize preferences to JSON value
    let prefs_value = serde_json::to_value(preferences)
        .map_err(|e| anyhow::anyhow!("Failed to serialize preferences: {}", e))?;

    // Save to store
    store.set("preferences", prefs_value);

    // Persist to disk
    store
        .save()
        .map_err(|e| anyhow::anyhow!("Failed to save store to disk: {}", e))?;

    info!("Successfully persisted recording preferences to disk");

    // Save backend preference to global config
    #[cfg(target_os = "macos")]
    if let Some(backend_str) = &preferences.system_audio_backend {
        if let Some(backend) = AudioCaptureBackend::from_string(backend_str) {
            info!("Setting audio capture backend to: {:?}", backend);
            crate::audio::capture::set_current_backend(backend);
        }
    }

    // Ensure the directory exists
    ensure_recordings_directory(&preferences.save_folder)?;

    Ok(())
}

/// Tauri commands for recording preferences
#[tauri::command]
pub async fn get_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
) -> Result<RecordingPreferences, String> {
    load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {}", e))
}

/// One selectable recording format/quality, for the settings UI.
#[derive(Debug, Serialize)]
pub struct RecordingFormatOption {
    pub id: String,
    pub label: String,
    /// True when the format ignores the quality setting.
    pub lossless: bool,
}

#[derive(Debug, Serialize)]
pub struct RecordingEncodingOptions {
    pub formats: Vec<RecordingFormatOption>,
    pub qualities: Vec<RecordingFormatOption>,
    /// Format id to select when nothing is configured.
    pub default_format: String,
    pub default_quality: String,
}

/// Supported recording formats and quality levels.
///
/// Exposed from Rust so the UI cannot offer a format the encoder does not
/// support — the previous hardcoded UI list had no connection to the encoder at
/// all, which is how it ended up advertising a format that was never produced.
#[tauri::command]
pub async fn get_recording_encoding_options() -> Result<RecordingEncodingOptions, String> {
    use super::formats::{RecordingFormat, RecordingQuality};

    Ok(RecordingEncodingOptions {
        formats: RecordingFormat::ALL
            .iter()
            .map(|f| RecordingFormatOption {
                id: f.id().to_string(),
                label: f.label().to_string(),
                lossless: f.is_lossless(),
            })
            .collect(),
        qualities: RecordingQuality::ALL
            .iter()
            .map(|q| RecordingFormatOption {
                id: q.id().to_string(),
                label: q.label().to_string(),
                // Quality is meaningless for lossless formats; the UI hides it.
                lossless: false,
            })
            .collect(),
        default_format: RecordingFormat::default().id().to_string(),
        default_quality: RecordingQuality::default().id().to_string(),
    })
}

#[tauri::command]
pub async fn set_recording_preferences<R: Runtime>(
    app: AppHandle<R>,
    preferences: RecordingPreferences,
) -> Result<(), String> {
    save_recording_preferences(&app, &preferences)
        .await
        .map_err(|e| format!("Failed to save recording preferences: {}", e))?;

    // Keep the unified storage panel truthful. That panel displays its own configured
    // recordings folder, which nothing else reads — so without this it would keep showing a
    // folder chosen earlier while recordings go wherever this call just put them.
    //
    // `save_recording_preferences` is the low-level writer used by the storage command, so
    // writing the config here rather than in that function keeps the two from recursing.
    let mut config = crate::storage::load_storage_config(&app);
    let folder = preferences.save_folder.to_string_lossy().to_string();

    if config.recordings_folder.as_deref() != Some(folder.as_str()) {
        config.recordings_folder = Some(folder);
        // Logged rather than raised: the preferences themselves are saved, so failing the
        // call would tell the user nothing was stored when the recording setting did apply.
        if let Err(error) = crate::storage::save_storage_config(&app, &config) {
            warn!("Recording preferences saved, but the storage settings could not be updated: {error}");
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_default_recordings_folder_path() -> Result<String, String> {
    let path = get_default_recordings_folder();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_recordings_folder<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let preferences = load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load preferences: {}", e))?;

    // Ensure directory exists before trying to open it
    ensure_recordings_directory(&preferences.save_folder)
        .map_err(|e| format!("Failed to create directory: {}", e))?;

    let folder_path = preferences.save_folder.to_string_lossy().to_string();

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(&folder_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    info!("Opened recordings folder: {}", folder_path);
    Ok(())
}

#[tauri::command]
pub async fn select_recording_folder<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    // Default the dialog to the currently configured folder so the user can see
    // where recordings go today.
    let current = load_recording_preferences(&app)
        .await
        .map(|p| p.save_folder)
        .ok();

    let app_for_dialog = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut builder = app_for_dialog.dialog().file().set_title("Select recordings folder");
        if let Some(dir) = current.filter(|d| d.exists()) {
            builder = builder.set_directory(dir);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("Folder dialog task failed: {e}"))?;

    let Some(folder) = picked else {
        info!("User cancelled recordings folder selection");
        return Ok(None);
    };

    let path = PathBuf::from(folder.to_string());
    ensure_recordings_directory(&path).map_err(|e| format!("Failed to prepare recordings folder: {e}"))?;

    // Persist immediately: the caller expects the choice to take effect, and the
    // settings UI reads the same preference back.
    let mut prefs = load_recording_preferences(&app)
        .await
        .map_err(|e| format!("Failed to load recording preferences: {e}"))?;
    prefs.save_folder = path.clone();
    save_recording_preferences(&app, &prefs)
        .await
        .map_err(|e| format!("Failed to save recording preferences: {e}"))?;

    let as_string = path.to_string_lossy().to_string();
    info!("Recordings folder set to: {}", as_string);
    Ok(Some(as_string))
}

// Backend selection commands

/// Get available audio capture backends for the current platform
#[tauri::command]
pub async fn get_available_audio_backends() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let backends = crate::audio::capture::get_available_backends();
        Ok(backends.iter().map(|b| b.to_string()).collect())
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Only ScreenCaptureKit available on non-macOS
        Ok(vec!["screencapturekit".to_string()])
    }
}

/// Get current audio capture backend
#[tauri::command]
pub async fn get_current_audio_backend() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let backend = crate::audio::capture::get_current_backend();
        Ok(backend.to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok("screencapturekit".to_string())
    }
}

/// Set audio capture backend
#[tauri::command]
pub async fn set_audio_backend(backend: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;
        use crate::audio::permissions::{
            check_screen_recording_permission, request_screen_recording_permission,
        };

        let backend_enum = AudioCaptureBackend::from_string(&backend)
            .ok_or_else(|| format!("Invalid backend: {}", backend))?;

        // If switching to Core Audio, log information about Audio Capture permission
        if backend_enum == AudioCaptureBackend::CoreAudio {
            info!("🔐 Core Audio backend requires Audio Capture permission (macOS 14.4+)");
            info!("📍 Permission dialog will appear automatically when recording starts");

            // Check if permission is already granted (this is informational only)
            if !check_screen_recording_permission() {
                warn!("⚠️  Audio Capture permission may not be granted");

                // Attempt to open System Settings (opens System Settings)
                if let Err(e) = request_screen_recording_permission() {
                    error!("Failed to open System Settings: {}", e);
                }

                return Err(
                    "Core Audio requires Audio Capture permission. \
                    The permission dialog will appear when you start recording. \
                    If already denied, enable it in System Settings → Privacy & Security → Audio Capture, \
                    then restart the app.".to_string()
                );
            }

            info!(
                "✅ Core Audio backend selected - permission check will occur at recording start"
            );
        }

        info!("Setting audio backend to: {:?}", backend_enum);
        crate::audio::capture::set_current_backend(backend_enum);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        if backend != "screencapturekit" {
            return Err(format!(
                "Backend {} not available on this platform",
                backend
            ));
        }
        Ok(())
    }
}

/// Get backend information (name and description)
#[derive(Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[tauri::command]
pub async fn get_audio_backend_info() -> Result<Vec<BackendInfo>, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio::capture::AudioCaptureBackend;

        let backends = vec![
            BackendInfo {
                id: AudioCaptureBackend::ScreenCaptureKit.to_string(),
                name: AudioCaptureBackend::ScreenCaptureKit.name().to_string(),
                description: AudioCaptureBackend::ScreenCaptureKit
                    .description()
                    .to_string(),
            },
            BackendInfo {
                id: AudioCaptureBackend::CoreAudio.to_string(),
                name: AudioCaptureBackend::CoreAudio.name().to_string(),
                description: AudioCaptureBackend::CoreAudio.description().to_string(),
            },
        ];
        Ok(backends)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(vec![BackendInfo {
            id: "screencapturekit".to_string(),
            name: "ScreenCaptureKit".to_string(),
            description: "Default system audio capture".to_string(),
        }])
    }
}

