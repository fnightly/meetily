// audio/transcription/provider_commands.rs
//
// Tauri surface for the speech-provider catalog.
//
// The frontend needs to know, per provider: what it supports, whether it is
// usable yet, and whether its API key is configured. Previously that knowledge
// lived only in a hardcoded `modelOptions` map in the UI, which is how the UI
// ended up offering options the backend rejected.

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use super::catalog::{self, ProviderKind, ProviderStatus, SpeechProviderDescriptor, PROVIDERS};
use crate::api::api::{api_get_transcript_api_key, TranscriptConfig};
use crate::state::AppState;

/// A provider as presented to the settings UI.
#[derive(Debug, Serialize)]
pub struct SpeechProviderView {
    pub id: String,
    pub display_name: String,
    pub kind: ProviderKind,
    pub status: ProviderStatus,
    /// `None` = any language. `Some(list)` = restricted to these codes.
    pub supported_languages: Option<Vec<String>>,
    pub requires_api_key: bool,
    /// Whether a key is actually stored for this provider.
    pub has_api_key: bool,
    pub models: Vec<String>,
    pub notes: String,
    pub api_key_url: Option<String>,
    /// Whether the provider can be selected for a new recording right now.
    pub selectable: bool,
}

/// Read a stored API key for a provider, treating "absent" as `None`.
///
/// `api_get_transcript_api_key` returns an empty string when nothing is stored,
/// so blank must be normalised to `None` here.
async fn stored_api_key<R: Runtime>(app: &AppHandle<R>, provider: &str) -> Option<String> {
    match api_get_transcript_api_key(
        app.clone(),
        app.state::<AppState>(),
        provider.to_string(),
        None::<String>,
    )
    .await
    {
        Ok(key) if !key.trim().is_empty() => Some(key),
        _ => None,
    }
}

/// Provider metadata plus this install's readiness, for the whole catalog.
#[tauri::command]
pub async fn api_list_speech_providers<R: Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<SpeechProviderView>, String> {
    let mut views = Vec::with_capacity(PROVIDERS.len());

    for descriptor in PROVIDERS {
        views.push(build_view(&app, descriptor).await);
    }

    Ok(views)
}

/// Current provider + language as the backend will interpret them.
///
/// Useful for the UI to explain a mismatch before the user starts recording.
#[derive(Debug, Serialize)]
pub struct ActiveSpeechSettings {
    pub provider: String,
    pub model: String,
    /// Raw language preference (`auto`, `auto-translate`, or a code).
    pub language: Option<String>,
    pub has_api_key: bool,
}

#[tauri::command]
pub async fn api_get_active_speech_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ActiveSpeechSettings, String> {
    let config = load_or_default_config(&app).await;

    let has_api_key = if catalog::find_provider(&config.provider)
        .map(|p| p.requires_api_key)
        .unwrap_or(false)
    {
        stored_api_key(&app, &config.provider).await.is_some()
    } else {
        false
    };

    Ok(ActiveSpeechSettings {
        provider: config.provider,
        model: config.model,
        language: crate::get_language_preference_internal(),
        has_api_key,
    })
}

/// Validate a provider/language combination without changing any state.
///
/// Returns a human-readable explanation on failure, which the UI can show
/// inline before the user commits to a configuration.
///
/// No `R: Runtime` parameter: this touches neither the app handle nor managed
/// state, and an unconstrained runtime generic cannot be inferred by
/// `generate_handler!`.
#[tauri::command]
pub async fn api_validate_speech_selection(
    provider: String,
    language: Option<String>,
) -> Result<(), String> {
    catalog::ensure_selectable(&provider)?;
    catalog::validate_language(&provider, language.as_deref())
}

/// Language codes usable with the given provider.
///
/// `None` means unrestricted (every language the engine knows). Used to drive
/// the language picker so it cannot offer a language the provider will reject.
#[tauri::command]
pub async fn api_get_provider_languages(
    provider: String,
) -> Result<Option<Vec<String>>, String> {
    let descriptor = catalog::find_provider(&provider)
        .ok_or_else(|| format!("Unknown speech provider '{}'", provider))?;

    Ok(descriptor
        .supported_languages
        .map(|langs| langs.iter().map(|l| l.to_string()).collect()))
}

async fn build_view<R: Runtime>(
    app: &AppHandle<R>,
    descriptor: &SpeechProviderDescriptor,
) -> SpeechProviderView {
    let has_api_key = if descriptor.requires_api_key {
        stored_api_key(app, descriptor.id).await.is_some()
    } else {
        false
    };

    SpeechProviderView {
        id: descriptor.id.to_string(),
        display_name: descriptor.display_name.to_string(),
        kind: descriptor.kind,
        status: descriptor.status,
        supported_languages: descriptor
            .supported_languages
            .map(|langs| langs.iter().map(|l| l.to_string()).collect()),
        requires_api_key: descriptor.requires_api_key,
        has_api_key,
        models: descriptor.models.iter().map(|m| m.to_string()).collect(),
        notes: descriptor.notes.to_string(),
        api_key_url: descriptor.api_key_url.map(|u| u.to_string()),
        selectable: catalog::is_implemented(descriptor.id),
    }
}

/// Read the stored transcript config, falling back to the catalog default.
///
/// Shared by the commands above so they cannot disagree about the default.
async fn load_or_default_config<R: Runtime>(app: &AppHandle<R>) -> TranscriptConfig {
    match crate::api::api::api_get_transcript_config(app.clone(), app.state::<AppState>(), None).await
    {
        Ok(Some(config)) => config,
        _ => TranscriptConfig {
            provider: catalog::default_provider_id().to_string(),
            model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
            api_key: None,
        },
    }
}
