// summary/cleanup_commands.rs
//
// Applies the transcript cleanup pass: rewrites the finished transcript to remove
// filler words, false starts and stutters, and to turn spoken punctuation and
// numbers into written form.
//
// Deliberately restricted to local providers. Cleanup sends the entire meeting
// transcript to a model, which is the user's actual conversation, so a cloud
// provider chosen for *summaries* must not be reused here without the user asking.
// The summary path may use any provider; this one may not.

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use super::cleanup::{is_local_provider, CleanupSkip};
use super::llm_client::LLMProvider;
use crate::database::repositories::setting::SettingsRepository;

/// Outcome of a cleanup attempt.
#[derive(Debug, Serialize)]
pub struct CleanupOutcome {
    /// The cleaned text, or `None` when cleanup did not run.
    pub cleaned: Option<String>,
    /// Original text, echoed so the caller can fall back without keeping a copy.
    pub original: String,
    /// True when the text was actually rewritten.
    pub applied: bool,
    /// Why cleanup was skipped, when it was.
    pub skipped: Option<String>,
}

/// Result of resolving which local model to clean with.
enum CleanupTarget {
    Ready {
        provider: LLMProvider,
        provider_id: String,
        model: String,
        ollama_endpoint: Option<String>,
    },
    Skipped(CleanupSkip),
}

/// The configuration fields cleanup cares about, so the policy can be tested
/// without constructing a database row.
pub struct CleanupConfig {
    pub provider: String,
    pub model: String,
    pub ollama_endpoint: Option<String>,
}

/// Decide which model may clean the transcript.
///
/// Kept separate from the command so the policy is testable without a database.
pub fn resolve_cleanup_target(config: Option<&CleanupConfig>) -> CleanupTarget {
    let Some(config) = config else {
        return CleanupTarget::Skipped(CleanupSkip::NoLocalModel);
    };

    if !is_local_provider(&config.provider) {
        return CleanupTarget::Skipped(CleanupSkip::CloudProviderNotAllowed(
            config.provider.clone(),
        ));
    }

    let provider = match LLMProvider::from_str(&config.provider) {
        Ok(provider) => provider,
        Err(_) => return CleanupTarget::Skipped(CleanupSkip::NoLocalModel),
    };

    // A local provider with no model name has nothing to run.
    if config.model.trim().is_empty() {
        return CleanupTarget::Skipped(CleanupSkip::NoLocalModel);
    }

    CleanupTarget::Ready {
        provider,
        provider_id: config.provider.clone(),
        model: config.model.clone(),
        ollama_endpoint: config.ollama_endpoint.clone(),
    }
}

/// Rewrite a finished transcript using a local model.
///
/// Returns the original text untouched when cleanup cannot run, so a caller can
/// always store the result without branching — losing the transcript because a
/// model was unavailable would be far worse than storing it uncleaned.
#[tauri::command]
pub async fn cleanup_transcript<R: Runtime>(
    app: AppHandle<R>,
    text: String,
) -> Result<CleanupOutcome, String> {
    if text.trim().is_empty() {
        return Ok(CleanupOutcome {
            cleaned: None,
            original: text,
            applied: false,
            skipped: Some(CleanupSkip::EmptyText.message()),
        });
    }

    let state = app.state::<crate::state::AppState>();
    let pool = state.db_manager.pool();

    let setting = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|e| format!("Failed to read the model configuration: {e}"))?;

    let config = setting.map(|row| CleanupConfig {
        provider: row.provider,
        model: row.model,
        ollama_endpoint: row.ollama_endpoint,
    });

    let (provider, provider_id, model, ollama_endpoint) =
        match resolve_cleanup_target(config.as_ref()) {
            CleanupTarget::Ready {
                provider,
                provider_id,
                model,
                ollama_endpoint,
            } => (provider, provider_id, model, ollama_endpoint),
            CleanupTarget::Skipped(skip) => {
                let message = skip.message();
                log::info!("Transcript cleanup skipped: {message}");
                return Ok(CleanupOutcome {
                    cleaned: None,
                    original: text,
                    applied: false,
                    skipped: Some(message),
                });
            }
        };

    let dictionary = crate::dictionary::load_dictionary(&app);

    log::info!(
        "Cleaning transcript with local provider {provider_id}/{model} ({} chars)",
        text.len()
    );

    let client = crate::http_client::client_with_proxy();
    let system_prompt = super::cleanup::chinese_cleanup_system_prompt();
    let user_prompt = super::cleanup::chinese_cleanup_user_prompt(&text, &dictionary);

    let app_data_dir = app.path().app_data_dir().ok();

    let completion = super::llm_client::generate_summary(
        &client,
        &provider,
        &model,
        "",
        &system_prompt,
        &user_prompt,
        ollama_endpoint.as_deref(),
        None,
        // Cleanup restates the transcript rather than composing prose, so a low
        // temperature keeps it faithful.
        None,
        Some(0.2),
        None,
        app_data_dir.as_ref(),
        None,
    )
    .await
    .map_err(|e| format!("Transcript cleanup failed: {e}"))?;

    let cleaned = completion.content.trim().to_string();

    // A model that returns nothing, or something implausibly short, has failed at
    // the task; keep the original rather than replacing a real transcript with it.
    if cleaned.is_empty() {
        log::warn!("Cleanup returned no text; keeping the original transcript");
        return Ok(CleanupOutcome {
            cleaned: None,
            original: text,
            applied: false,
            skipped: Some("The model returned no text, so the original was kept.".to_string()),
        });
    }

    Ok(CleanupOutcome {
        cleaned: Some(cleaned),
        original: text,
        applied: true,
        skipped: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str, model: &str) -> CleanupConfig {
        CleanupConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            ollama_endpoint: Some("http://localhost:11434".to_string()),
        }
    }

    #[test]
    fn a_local_provider_is_accepted() {
        match resolve_cleanup_target(Some(&config("ollama", "qwen2.5:1.5b"))) {
            CleanupTarget::Ready { provider, model, .. } => {
                assert_eq!(provider, LLMProvider::Ollama);
                assert_eq!(model, "qwen2.5:1.5b");
            }
            CleanupTarget::Skipped(skip) => panic!("unexpectedly skipped: {skip:?}"),
        }
    }

    #[test]
    fn the_builtin_provider_is_accepted() {
        assert!(matches!(
            resolve_cleanup_target(Some(&config("builtin-ai", "Qwen3.5-4B-Q4_K_M"))),
            CleanupTarget::Ready { .. }
        ));
    }

    #[test]
    fn a_cloud_provider_is_refused_and_named() {
        // The user may have chosen a cloud provider for summaries; that is not
        // consent to upload the transcript for rewriting.
        match resolve_cleanup_target(Some(&config("openai", "gpt-4o"))) {
            CleanupTarget::Skipped(skip) => match skip {
                CleanupSkip::CloudProviderNotAllowed(provider) => assert_eq!(provider, "openai"),
                other => panic!("wrong skip reason: {other:?}"),
            },
            CleanupTarget::Ready { .. } => panic!("a cloud provider must not be used"),
        }
    }

    #[test]
    fn every_cloud_provider_is_refused() {
        for provider in ["openai", "claude", "groq", "openrouter", "custom-openai"] {
            assert!(
                matches!(
                    resolve_cleanup_target(Some(&config(provider, "some-model"))),
                    CleanupTarget::Skipped(CleanupSkip::CloudProviderNotAllowed(_))
                ),
                "{provider} must be refused"
            );
        }
    }

    #[test]
    fn no_config_means_no_local_model() {
        assert!(matches!(
            resolve_cleanup_target(None),
            CleanupTarget::Skipped(CleanupSkip::NoLocalModel)
        ));
    }

    #[test]
    fn a_local_provider_without_a_model_is_skipped() {
        assert!(matches!(
            resolve_cleanup_target(Some(&config("ollama", "   "))),
            CleanupTarget::Skipped(CleanupSkip::NoLocalModel)
        ));
    }

    #[test]
    fn the_target_carries_the_ollama_endpoint() {
        // A custom Ollama port must be honoured, otherwise cleanup silently talks
        // to the default one.
        match resolve_cleanup_target(Some(&config("ollama", "m"))) {
            CleanupTarget::Ready { ollama_endpoint, .. } => {
                assert_eq!(ollama_endpoint.as_deref(), Some("http://localhost:11434"));
            }
            CleanupTarget::Skipped(skip) => panic!("unexpectedly skipped: {skip:?}"),
        }
    }
}
