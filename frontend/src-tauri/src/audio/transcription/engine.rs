// audio/transcription/engine.rs
//
// TranscriptionEngine enum and model initialization/validation logic.

use super::provider::TranscriptionProvider;
use log::{info, warn};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime};

// ============================================================================
// TRANSCRIPTION ENGINE ENUM
// ============================================================================

// Transcription engine abstraction to support multiple providers
pub enum TranscriptionEngine {
    Whisper(Arc<crate::whisper_engine::WhisperEngine>),  // Direct access (backward compat)
    Parakeet(Arc<crate::parakeet_engine::ParakeetEngine>), // Direct access (backward compat)
    Provider(Arc<dyn TranscriptionProvider>),  // Trait-based (preferred for new code)
}

impl TranscriptionEngine {
    /// Check if the engine has a model loaded
    pub async fn is_model_loaded(&self) -> bool {
        match self {
            Self::Whisper(engine) => engine.is_model_loaded().await,
            Self::Parakeet(engine) => engine.is_model_loaded().await,
            Self::Provider(provider) => provider.is_model_loaded().await,
        }
    }

    /// Get the current model name
    pub async fn get_current_model(&self) -> Option<String> {
        match self {
            Self::Whisper(engine) => engine.get_current_model().await,
            Self::Parakeet(engine) => engine.get_current_model().await,
            Self::Provider(provider) => provider.get_current_model().await,
        }
    }

    /// Get the provider name for logging
    pub fn provider_name(&self) -> &str {
        match self {
            Self::Whisper(_) => "Whisper (direct)",
            Self::Parakeet(_) => "Parakeet (direct)",
            Self::Provider(provider) => provider.provider_name(),
        }
    }
}

// ============================================================================
// MODEL VALIDATION AND INITIALIZATION
// ============================================================================

/// Validate that transcription models (Whisper or Parakeet) are ready before starting recording
pub async fn validate_transcription_model_ready<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // Check transcript configuration to determine which engine to validate
    let config = match crate::api::api::api_get_transcript_config(
        app.clone(),
        app.clone().state(),
        None,
    )
    .await
    {
        Ok(Some(config)) => {
            info!(
                "📝 Found transcript config - provider: {}, model: {}",
                config.provider, config.model
            );
            config
        }
        Ok(None) => {
            info!("📝 No transcript config found, using the catalog default provider");
            crate::api::api::TranscriptConfig {
                provider: "parakeet".to_string(),
                model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
                api_key: None,
            }
        }
        Err(e) => {
            warn!("⚠️ Failed to get transcript config: {}, using the catalog default provider", e);
            crate::api::api::TranscriptConfig {
                provider: "parakeet".to_string(),
                model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
                api_key: None,
            }
        }
    };

    // Reserved providers (listed in settings but not wired up) must be refused
    // here, before any recording starts. This is the chokepoint that keeps cloud
    // audio upload impossible while those providers are unimplemented.
    super::catalog::ensure_selectable(&config.provider)?;

    // Language capability is checked per provider: recommending Parakeet for a
    // Chinese meeting produced unusable output with no explanation.
    let requested_language = crate::get_language_preference_internal();
    if let Err(e) = super::catalog::validate_language(&config.provider, requested_language.as_deref())
    {
        warn!("❌ Provider/language mismatch: {}", e);
        return Err(e);
    }

    // Keep a single local model resident: switching engine (or moving to a cloud
    // provider) releases the other one instead of holding both in memory.
    // Batch jobs may still be using it, so only when nothing else is running.
    if super::residency::no_active_jobs() {
        let keep = match config.provider.as_str() {
            "localWhisper" => Some("localWhisper"),
            "parakeet" => Some("parakeet"),
            _ => None,
        };
        super::residency::release_engines_except(keep).await;
    }

    // Validate based on provider
    match config.provider.as_str() {
        "localWhisper" => {
            info!("🔍 Validating Whisper model...");
            // Ensure whisper engine is initialized first
            if let Err(init_error) = crate::whisper_engine::commands::whisper_init().await {
                warn!("❌ Failed to initialize Whisper engine: {}", init_error);
                return Err(format!(
                    "Failed to initialize speech recognition: {}",
                    init_error
                ));
            }

            // Call the whisper validation command with config support
            match crate::whisper_engine::commands::whisper_validate_model_ready_with_config(app).await {
                Ok(model_name) => {
                    info!("✅ Whisper model validation successful: {} is ready", model_name);
                    Ok(())
                }
                Err(e) => {
                    warn!("❌ Whisper model validation failed: {}", e);
                    Err(e)
                }
            }
        }
        "parakeet" => {
            info!("🔍 Validating Parakeet model...");
            // Ensure parakeet engine is initialized first
            if let Err(init_error) = crate::parakeet_engine::commands::parakeet_init().await {
                warn!("❌ Failed to initialize Parakeet engine: {}", init_error);
                return Err(format!(
                    "Failed to initialize Parakeet speech recognition: {}",
                    init_error
                ));
            }

            // Use the validation command that includes auto-discovery and loading
            // This matches the Whisper behavior for consistency
            match crate::parakeet_engine::commands::parakeet_validate_model_ready_with_config(app).await {
                Ok(model_name) => {
                    info!("✅ Parakeet model validation successful: {} is ready", model_name);
                    Ok(())
                }
                Err(e) => {
                    warn!("❌ Parakeet model validation failed: {}", e);
                    Err(e)
                }
            }
        }
        id if super::cloud_provider::is_cloud_transcription_provider(id) => {
            // Cloud: nothing to load, but the key and model must be present so
            // the recording does not start only to fail on the first segment.
            info!("☁️ Validating cloud transcription provider '{}'", id);
            super::cloud_provider::CloudSpeechProvider::from_settings(app, id, None, None)
                .await
                .map(|provider| {
                    info!("✅ Cloud transcription ready: {} / {}", id, provider.model());
                })
        }
        other => {
            // Reserved providers (cloud) land here: they are listed in settings
            // but must not be used to record yet.
            if !super::catalog::is_implemented(other) {
                return Err(super::catalog::ensure_selectable(other)
                    .err()
                    .unwrap_or_else(|| {
                        format!("Transcription provider '{}' is not available", other)
                    }));
            }

            warn!("❌ Unsupported transcription provider for local recording: {}", other);
            Err(format!(
                "Provider '{}' is not supported for local transcription. Please select 'localWhisper' or 'parakeet'.",
                other
            ))
        }
    }
}

/// Get or initialize the transcription engine for the configured provider.
///
/// `provider_override` forces a specific provider for this session, which is how
/// a language-compatible provider gets used for one recording without rewriting
/// the user's saved settings.
pub async fn get_or_init_transcription_engine<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<TranscriptionEngine, String> {
    get_or_init_transcription_engine_for(app, None).await
}

/// Read the provider id from the stored transcript config, falling back to the
/// catalog default when nothing is configured.
///
/// Callers that need to reason about provider choice *before* initializing an
/// engine (e.g. language-compatibility switching) use this instead of
/// duplicating the config lookup and its default.
pub async fn configured_provider_id<R: Runtime>(app: &AppHandle<R>) -> String {
    match crate::api::api::api_get_transcript_config(app.clone(), app.clone().state(), None).await {
        Ok(Some(config)) => config.provider,
        _ => super::catalog::default_provider_id().to_string(),
    }
}

/// Get or initialize the transcription engine, optionally overriding the provider.
pub async fn get_or_init_transcription_engine_for<R: Runtime>(
    app: &AppHandle<R>,
    provider_override: Option<&str>,
) -> Result<TranscriptionEngine, String> {
    // Get provider configuration from API
    let mut config = match crate::api::api::api_get_transcript_config(
        app.clone(),
        app.clone().state(),
        None,
    )
    .await
    {
        Ok(Some(config)) => {
            info!(
                "📝 Transcript config - provider: {}, model: {}",
                config.provider, config.model
            );
            config
        }
        Ok(None) => {
            info!("📝 No transcript config found, using the catalog default provider");
            crate::api::api::TranscriptConfig {
                provider: super::catalog::default_provider_id().to_string(),
                model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
                api_key: None,
            }
        }
        Err(e) => {
            warn!("⚠️ Failed to get transcript config: {}, using the catalog default provider", e);
            crate::api::api::TranscriptConfig {
                provider: super::catalog::default_provider_id().to_string(),
                model: crate::config::DEFAULT_PARAKEET_MODEL.to_string(),
                api_key: None,
            }
        }
    };

    if let Some(forced) = provider_override {
        if forced != config.provider {
            info!(
                "🔀 Overriding transcript provider for this session: {} -> {} (language compatibility)",
                config.provider, forced
            );
            config.provider = forced.to_string();
        }
    }

    // Initialize the appropriate engine based on provider
    match config.provider.as_str() {
        "parakeet" => {
            info!("🦜 Initializing Parakeet transcription engine");

            // Get Parakeet engine
            let engine = {
                let guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
                    .lock()
                    .unwrap();
                guard.as_ref().cloned()
            };

            match engine {
                Some(engine) => {
                    // Check if model is loaded
                    if engine.is_model_loaded().await {
                        let model_name = engine.get_current_model().await
                            .unwrap_or_else(|| "unknown".to_string());
                        info!("✅ Parakeet model '{}' already loaded", model_name);
                        Ok(TranscriptionEngine::Parakeet(engine))
                    } else {
                        Err("Parakeet engine initialized but no model loaded. This should not happen after validation.".to_string())
                    }
                }
                None => {
                    Err("Parakeet engine not initialized. This should not happen after validation.".to_string())
                }
            }
        }
        id if super::cloud_provider::is_cloud_transcription_provider(id) => {
            info!("☁️ Initializing cloud transcription provider '{}'", id);
            let provider =
                super::cloud_provider::CloudSpeechProvider::from_settings(app, id, None, None).await?;
            Ok(TranscriptionEngine::Provider(Arc::new(provider)))
        }
        "localWhisper" | _ => {
            // `localWhisper` is the fallback arm, so an unrecognised provider id
            // would silently be treated as Whisper. Refuse reserved ids instead.
            if config.provider != "localWhisper" {
                super::catalog::ensure_selectable(&config.provider)?;
            }

            info!("🎤 Initializing Whisper transcription engine");
            let whisper_engine = get_or_init_whisper(app).await?;
            Ok(TranscriptionEngine::Whisper(whisper_engine))
        }
    }
}

/// Get or initialize transcription engine using API configuration
/// Returns Whisper engine if provider is localWhisper, otherwise returns error for non-Whisper providers
pub async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Arc<crate::whisper_engine::WhisperEngine>, String> {
    // Check if engine already exists and has a model loaded
    let existing_engine = {
        let engine_guard = crate::whisper_engine::commands::WHISPER_ENGINE
            .lock()
            .unwrap();
        engine_guard.as_ref().cloned()
    };

    if let Some(engine) = existing_engine {
        // Check if a model is already loaded
        if engine.is_model_loaded().await {
            let current_model = engine
                .get_current_model()
                .await
                .unwrap_or_else(|| "unknown".to_string());

            // NEW: Check if loaded model matches saved config
            let configured_model = match crate::api::api::api_get_transcript_config(
                app.clone(),
                app.clone().state(),
                None,
            )
            .await
            {
                Ok(Some(config)) => {
                    info!(
                        "📝 Saved transcript config - provider: {}, model: {}",
                        config.provider, config.model
                    );
                    if config.provider == "localWhisper" && !config.model.is_empty() {
                        Some(config.model)
                    } else {
                        None
                    }
                }
                Ok(None) => {
                    info!("📝 No transcript config found in database");
                    None
                }
                Err(e) => {
                    warn!("⚠️ Failed to get transcript config: {}", e);
                    None
                }
            };

            // If loaded model matches config, reuse it
            if let Some(ref expected_model) = configured_model {
                if current_model == *expected_model {
                    info!(
                        "✅ Loaded model '{}' matches saved config, reusing",
                        current_model
                    );
                    return Ok(engine);
                } else {
                    info!(
                        "🔄 Loaded model '{}' doesn't match saved config '{}', reloading correct model...",
                        current_model, expected_model
                    );
                    // Unload the incorrect model
                    engine.unload_model().await;
                    info!("📉 Unloaded incorrect model '{}'", current_model);
                    // Continue to model loading logic below
                }
            } else {
                // No specific config saved, accept currently loaded model
                info!(
                    "✅ No specific model configured, using currently loaded model: '{}'",
                    current_model
                );
                return Ok(engine);
            }
        } else {
            info!("🔄 Whisper engine exists but no model loaded, will load model from config");
        }
    }

    // Initialize new engine if needed
    info!("Initializing Whisper engine");

    // First ensure the engine is initialized
    if let Err(e) = crate::whisper_engine::commands::whisper_init().await {
        return Err(format!("Failed to initialize Whisper engine: {}", e));
    }

    // Get the engine reference
    let engine = {
        let engine_guard = crate::whisper_engine::commands::WHISPER_ENGINE
            .lock()
            .unwrap();
        engine_guard
            .as_ref()
            .cloned()
            .ok_or("Failed to get initialized engine")?
    };

    // Get model configuration from API
    let model_to_load =
        match crate::api::api::api_get_transcript_config(app.clone(), app.clone().state(), None)
            .await
        {
            Ok(Some(config)) => {
                info!(
                    "Got transcript config from API - provider: {}, model: {}",
                    config.provider, config.model
                );
                if config.provider == "localWhisper" {
                    info!("Using model from API config: {}", config.model);
                    config.model
                } else {
                    // Non-Whisper provider (e.g., parakeet) - this function shouldn't be called
                    return Err(format!(
                        "Cannot initialize Whisper engine: Config uses '{}' provider. This is a bug in the transcription task initialization.",
                        config.provider
                    ));
                }
            }
            Ok(None) => {
                info!("No transcript config found in API, falling back to 'small'");
                "small".to_string()
            }
            Err(e) => {
                warn!(
                    "Failed to get transcript config from API: {}, falling back to 'small'",
                    e
                );
                "small".to_string()
            }
        };

    info!("Selected model to load: {}", model_to_load);

    // Discover available models to check if the desired model is downloaded
    let models = engine
        .discover_models()
        .await
        .map_err(|e| format!("Failed to discover models: {}", e))?;

    info!("Discovered {} models", models.len());
    for model in &models {
        info!(
            "Model: {} - Status: {:?} - Path: {}",
            model.name,
            model.status,
            model.path.display()
        );
    }

    // Check if the desired model is available
    let model_info = models.iter().find(|model| model.name == model_to_load);

    if model_info.is_none() {
        info!(
            "Model '{}' not found in discovered models. Available models: {:?}",
            model_to_load,
            models.iter().map(|m| &m.name).collect::<Vec<_>>()
        );
    }

    match model_info {
        Some(model) => {
            match model.status {
                crate::whisper_engine::ModelStatus::Available => {
                    info!("Loading model: {}", model_to_load);
                    engine
                        .load_model(&model_to_load)
                        .await
                        .map_err(|e| format!("Failed to load model '{}': {}", model_to_load, e))?;
                    info!("✅ Model '{}' loaded successfully", model_to_load);
                }
                crate::whisper_engine::ModelStatus::Missing => {
                    return Err(format!(
                        "Model '{}' is not downloaded. Please download it first from the settings.",
                        model_to_load
                    ));
                }
                crate::whisper_engine::ModelStatus::Downloading { progress } => {
                    return Err(format!("Model '{}' is currently downloading ({}%). Please wait for it to complete.", model_to_load, progress));
                }
                crate::whisper_engine::ModelStatus::Error(ref err) => {
                    return Err(format!("Model '{}' has an error: {}. Please check the model or try downloading it again.", model_to_load, err));
                }
                crate::whisper_engine::ModelStatus::Corrupted { .. } => {
                    return Err(format!("Model '{}' is corrupted. Please delete it and download again from the settings.", model_to_load));
                }
            }
        }
        None => {
            // Check if we have any available models and try to load the first one
            let available_models: Vec<_> = models
                .iter()
                .filter(|m| matches!(m.status, crate::whisper_engine::ModelStatus::Available))
                .collect();

            if let Some(fallback_model) = available_models.first() {
                warn!(
                    "Model '{}' not found, falling back to available model: '{}'",
                    model_to_load, fallback_model.name
                );
                engine.load_model(&fallback_model.name).await.map_err(|e| {
                    format!(
                        "Failed to load fallback model '{}': {}",
                        fallback_model.name, e
                    )
                })?;
                info!(
                    "✅ Fallback model '{}' loaded successfully",
                    fallback_model.name
                );
            } else {
                return Err(format!("Model '{}' is not supported and no other models are available. Please download a model from the settings.", model_to_load));
            }
        }
    }

    Ok(engine)
}
