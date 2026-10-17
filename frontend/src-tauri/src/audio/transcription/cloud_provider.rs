// audio/transcription/cloud_provider.rs
//
// Cloud speech-to-text over the OpenAI-compatible `/audio/transcriptions` API.
//
// One implementation covers every provider that speaks that protocol (OpenAI,
// Groq, SiliconFlow/SenseVoice). Audio only leaves the machine when the user has
// explicitly selected one of these providers *and* stored an API key: the
// catalog marks them `Cloud`, the language-fallback never picks a cloud
// provider on its own, and construction fails without a key.

use super::catalog::ProviderKind;
use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use log::{info, warn};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// Sample rate of every buffer handed to a transcription provider.
pub const PIPELINE_SAMPLE_RATE: u32 = 16_000;

/// Per-request timeout. Segments are at most ~25 s of audio, so a minute is
/// generous even on slow links.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Attempts per segment for transient failures (429 / 5xx / network).
const MAX_ATTEMPTS: u32 = 3;

/// Static wire details for a cloud provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudEndpoint {
    pub provider_id: &'static str,
    pub display_name: &'static str,
    /// Base URL ending before `/audio/transcriptions`.
    pub base_url: &'static str,
    /// Whether the API accepts an ISO-639-1 `language` field.
    pub accepts_language: bool,
    /// Whether the API accepts `response_format=json`.
    pub accepts_response_format: bool,
    /// Whether the API accepts a `prompt` (vocabulary hint) field.
    pub accepts_prompt: bool,
}

pub const CLOUD_ENDPOINTS: &[CloudEndpoint] = &[
    CloudEndpoint {
        provider_id: "openai",
        display_name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        accepts_language: true,
        accepts_response_format: true,
        accepts_prompt: true,
    },
    CloudEndpoint {
        provider_id: "groq",
        display_name: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        accepts_language: true,
        accepts_response_format: true,
        accepts_prompt: true,
    },
    CloudEndpoint {
        provider_id: "siliconflow",
        display_name: "SiliconFlow",
        base_url: "https://api.siliconflow.cn/v1",
        // SenseVoice auto-detects the language and documents only `file`+`model`.
        accepts_language: false,
        accepts_response_format: false,
        accepts_prompt: false,
    },
];

/// Wire details for a provider id, or `None` when it is not a cloud provider.
pub fn endpoint_for(provider_id: &str) -> Option<&'static CloudEndpoint> {
    CLOUD_ENDPOINTS.iter().find(|e| e.provider_id == provider_id)
}

/// Whether this id is a cloud provider this module can actually drive.
pub fn is_cloud_transcription_provider(provider_id: &str) -> bool {
    endpoint_for(provider_id).is_some()
        && super::catalog::is_cloud(provider_id)
        && super::catalog::is_implemented(provider_id)
}

/// Encode mono f32 samples as a 16-bit PCM WAV file in memory.
pub fn encode_wav_pcm16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Reduce the app's language preference to what the API expects.
///
/// `auto`, `auto-translate` and blank mean "detect"; region suffixes are
/// dropped (`zh-CN` -> `zh`) because the APIs take ISO-639-1 codes.
pub fn api_language(language: Option<&str>) -> Option<String> {
    let lang = language?.trim();
    if lang.is_empty()
        || lang.eq_ignore_ascii_case("auto")
        || lang.eq_ignore_ascii_case("auto-translate")
    {
        return None;
    }
    let primary = lang.split(['-', '_']).next().unwrap_or(lang).to_ascii_lowercase();
    if primary.len() < 2 || primary.len() > 3 {
        return None;
    }
    Some(primary)
}

/// Remove SenseVoice-style control tokens such as `<|zh|><|NEUTRAL|>`.
fn strip_control_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        match rest[start..].find("|>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = &rest[start..];
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Extract the transcript from a response body (JSON `{"text": ...}` or plain text).
pub fn parse_transcription_response(body: &str) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("Unreadable transcription response: {}", e))?;
        if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
            return Ok(strip_control_tokens(text));
        }
        if let Some(message) = value
            .get("error")
            .and_then(|e| e.get("message").or(Some(e)))
            .and_then(|m| m.as_str())
        {
            return Err(message.to_string());
        }
        return Err("Transcription response had no `text` field".to_string());
    }
    Ok(strip_control_tokens(trimmed))
}

/// Turn an HTTP failure into a message the user can act on.
fn describe_http_error(display_name: &str, status: u16, body: &str) -> String {
    let detail = parse_transcription_response(body)
        .err()
        .unwrap_or_else(|| body.chars().take(200).collect());
    let hint = match status {
        401 | 403 => "the API key was rejected — check it in Settings → Transcription",
        404 => "the model name was not found for this provider",
        413 => "the audio segment is too large for this provider",
        429 => "rate limit or quota exceeded",
        500..=599 => "the provider is having problems; try again later",
        _ => "request failed",
    };
    format!("{} ({}): {}. {}", display_name, status, hint, detail.trim())
}

fn is_retryable(status: u16) -> bool {
    status == 429 || status == 408 || (500..=599).contains(&status)
}

/// Custom-dictionary terms as a vocabulary hint (Whisper-style `prompt`).
///
/// Whisper weighs the prompt as preceding context, so a short list of the
/// user's proper nouns measurably improves how they are spelled. Kept short:
/// the API truncates prompts beyond ~224 tokens.
pub fn dictionary_prompt<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    let dictionary = crate::dictionary::load_dictionary(app);
    terms_prompt(dictionary.entries.iter().map(|e| e.term.as_str()))
}

fn terms_prompt<'a>(terms: impl Iterator<Item = &'a str>) -> Option<String> {
    const MAX_PROMPT_CHARS: usize = 400;
    let mut prompt = String::new();
    for term in terms.map(str::trim).filter(|t| !t.is_empty()) {
        let extra = if prompt.is_empty() { term.chars().count() } else { term.chars().count() + 2 };
        if prompt.chars().count() + extra > MAX_PROMPT_CHARS {
            break;
        }
        if !prompt.is_empty() {
            prompt.push_str(", ");
        }
        prompt.push_str(term);
    }
    (!prompt.is_empty()).then_some(prompt)
}

/// A configured cloud transcription client.
pub struct CloudSpeechProvider {
    endpoint: &'static CloudEndpoint,
    model: String,
    api_key: String,
    /// Optional vocabulary hint (custom dictionary terms).
    prompt: Option<String>,
    client: reqwest::Client,
}

impl CloudSpeechProvider {
    pub fn new(provider_id: &str, model: &str, api_key: &str) -> Result<Self, String> {
        let endpoint = endpoint_for(provider_id)
            .ok_or_else(|| format!("'{}' is not a cloud transcription provider", provider_id))?;
        if api_key.trim().is_empty() {
            return Err(format!(
                "{} needs an API key. Add it in Settings → Transcription.",
                endpoint.display_name
            ));
        }
        let model = model.trim();
        if model.is_empty() {
            return Err(format!("No {} transcription model selected", endpoint.display_name));
        }
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;
        Ok(Self {
            endpoint,
            model: model.to_string(),
            api_key: api_key.trim().to_string(),
            prompt: None,
            client,
        })
    }

    /// Attach a vocabulary hint, used only by providers that accept one.
    pub fn with_prompt(mut self, prompt: Option<String>) -> Self {
        self.prompt = prompt.filter(|p| !p.trim().is_empty());
        self
    }

    /// Build a client from the stored settings.
    ///
    /// `model_override` wins; otherwise the saved transcript model is used when
    /// it belongs to this provider, else the catalog's first suggestion.
    pub async fn from_settings<R: Runtime>(
        app: &AppHandle<R>,
        provider_id: &str,
        model_override: Option<&str>,
        api_key_override: Option<&str>,
    ) -> Result<Self, String> {
        if !is_cloud_transcription_provider(provider_id) {
            return Err(format!("'{}' is not an available cloud transcription provider", provider_id));
        }
        let state = app.state::<crate::state::AppState>();
        let pool = state.db_manager.pool();

        let api_key = match api_key_override.filter(|k| !k.trim().is_empty()) {
            Some(k) => k.to_string(),
            None => crate::database::repositories::setting::SettingsRepository::get_transcript_api_key(
                pool,
                provider_id,
            )
            .await
            .map_err(|e| format!("Failed to read API key: {}", e))?
            .unwrap_or_default(),
        };

        let model = match model_override.filter(|m| !m.trim().is_empty()) {
            Some(m) => m.to_string(),
            None => {
                let saved = crate::database::repositories::setting::SettingsRepository::get_transcript_config(pool)
                    .await
                    .ok()
                    .flatten()
                    .filter(|c| c.provider == provider_id)
                    .map(|c| c.model)
                    .filter(|m| !m.trim().is_empty());
                saved
                    .or_else(|| {
                        super::catalog::find_provider(provider_id)
                            .and_then(|p| p.models.first())
                            .map(|m| m.to_string())
                    })
                    .unwrap_or_default()
            }
        };

        Ok(Self::new(provider_id, &model, &api_key)?.with_prompt(dictionary_prompt(app)))
    }

    pub fn provider_id(&self) -> &'static str {
        self.endpoint.provider_id
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn url(&self) -> String {
        format!("{}/audio/transcriptions", self.endpoint.base_url.trim_end_matches('/'))
    }

    fn build_form(&self, wav: Vec<u8>, language: Option<&str>) -> Result<reqwest::multipart::Form, String> {
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("segment.wav")
            .mime_str("audio/wav")
            .map_err(|e| e.to_string())?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());
        if self.endpoint.accepts_response_format {
            form = form.text("response_format", "json");
        }
        if self.endpoint.accepts_language {
            if let Some(lang) = api_language(language) {
                form = form.text("language", lang);
            }
        }
        if self.endpoint.accepts_prompt {
            if let Some(prompt) = &self.prompt {
                form = form.text("prompt", prompt.clone());
            }
        }
        Ok(form)
    }

    /// Transcribe one 16 kHz mono buffer. Retries transient failures.
    pub async fn transcribe_samples(&self, samples: &[f32], language: Option<&str>) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let wav = encode_wav_pcm16(samples, PIPELINE_SAMPLE_RATE);
        let mut last_error = String::new();

        for attempt in 1..=MAX_ATTEMPTS {
            let form = self.build_form(wav.clone(), language)?;
            let result = self
                .client
                .post(self.url())
                .bearer_auth(&self.api_key)
                .multipart(form)
                .send()
                .await;

            match result {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    if (200..300).contains(&status) {
                        return parse_transcription_response(&body);
                    }
                    last_error = describe_http_error(self.endpoint.display_name, status, &body);
                    if !is_retryable(status) {
                        return Err(last_error);
                    }
                }
                Err(e) => {
                    last_error = if e.is_timeout() {
                        format!("{}: request timed out", self.endpoint.display_name)
                    } else {
                        format!("{}: network error: {}", self.endpoint.display_name, e)
                    };
                }
            }

            if attempt < MAX_ATTEMPTS {
                let delay = Duration::from_millis(800 * 2u64.pow(attempt - 1));
                warn!("☁️ {} attempt {} failed ({}); retrying in {:?}", self.endpoint.display_name, attempt, last_error, delay);
                tokio::time::sleep(delay).await;
            }
        }

        Err(last_error)
    }

    /// Send one second of near-silence to verify key, model and connectivity.
    pub async fn test_connection(&self) -> Result<(), String> {
        // Faint noise rather than exact zeros: some APIs reject all-zero audio.
        let samples: Vec<f32> = (0..PIPELINE_SAMPLE_RATE as usize)
            .map(|i| ((i as f32 * 0.37).sin()) * 0.001)
            .collect();
        self.transcribe_samples(&samples, None).await.map(|_| ())
    }
}

#[async_trait]
impl TranscriptionProvider for CloudSpeechProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        self.transcribe_samples(&audio, language.as_deref())
            .await
            .map(|text| TranscriptResult {
                text,
                confidence: None,
                is_partial: false,
            })
            .map_err(TranscriptionError::EngineFailed)
    }

    async fn is_model_loaded(&self) -> bool {
        true
    }

    async fn get_current_model(&self) -> Option<String> {
        Some(self.model.clone())
    }

    fn provider_name(&self) -> &'static str {
        self.endpoint.display_name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Cloud
    }
}

/// Test a provider/key/model combination from the settings UI.
///
/// `api_key` may be omitted to test the stored key.
#[tauri::command]
pub async fn api_test_speech_provider<R: Runtime>(
    app: AppHandle<R>,
    provider: String,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<String, String> {
    let client =
        CloudSpeechProvider::from_settings(&app, &provider, model.as_deref(), api_key.as_deref()).await?;
    info!("☁️ Testing cloud transcription: {} / {}", client.provider_id(), client.model());
    client.test_connection().await?;
    Ok(format!("{} / {}", client.endpoint.display_name, client.model()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let wav = encode_wav_pcm16(&[0.0, 1.0, -1.0, 2.0], 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        assert_eq!(wav.len(), 44 + 8);
        // clipping: 2.0 clamps to i16::MAX
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([wav[48], wav[49]]), -i16::MAX);
    }

    #[test]
    fn language_is_reduced_to_iso_code() {
        assert_eq!(api_language(Some("zh-CN")), Some("zh".into()));
        assert_eq!(api_language(Some("EN_us")), Some("en".into()));
        assert_eq!(api_language(Some("auto")), None);
        assert_eq!(api_language(Some("auto-translate")), None);
        assert_eq!(api_language(Some(" ")), None);
        assert_eq!(api_language(None), None);
    }

    #[test]
    fn response_parsing_handles_json_text_and_errors() {
        assert_eq!(parse_transcription_response(r#"{"text":" 你好 "}"#).unwrap(), "你好");
        assert_eq!(parse_transcription_response("plain words\n").unwrap(), "plain words");
        assert_eq!(
            parse_transcription_response(r#"{"text":"<|zh|><|NEUTRAL|><|Speech|>开会了"}"#).unwrap(),
            "开会了"
        );
        let err = parse_transcription_response(r#"{"error":{"message":"bad key"}}"#).unwrap_err();
        assert_eq!(err, "bad key");
    }

    #[test]
    fn construction_requires_key_and_model() {
        assert!(CloudSpeechProvider::new("openai", "whisper-1", "").is_err());
        assert!(CloudSpeechProvider::new("openai", "", "sk-x").is_err());
        assert!(CloudSpeechProvider::new("parakeet", "x", "sk-x").is_err());
        assert!(CloudSpeechProvider::new("groq", "whisper-large-v3-turbo", "gsk").is_ok());
    }

    #[test]
    fn every_endpoint_is_a_cloud_catalog_entry() {
        for e in CLOUD_ENDPOINTS {
            let p = super::super::catalog::find_provider(e.provider_id)
                .unwrap_or_else(|| panic!("{} missing from catalog", e.provider_id));
            assert_eq!(p.kind, ProviderKind::Cloud);
            assert!(p.requires_api_key);
            assert!(e.base_url.starts_with("https://"));
        }
    }

    #[test]
    fn form_omits_fields_the_provider_rejects() {
        // SiliconFlow documents only file + model; extra fields must not be sent.
        let sf = CloudSpeechProvider::new("siliconflow", "FunAudioLLM/SenseVoiceSmall", "k").unwrap();
        assert!(!sf.endpoint.accepts_language && !sf.endpoint.accepts_response_format);
        assert!(sf.url().ends_with("/v1/audio/transcriptions"));
    }

    #[test]
    fn dictionary_terms_become_a_bounded_prompt() {
        assert_eq!(terms_prompt(["微见智能", " ", "TFLN"].into_iter()), Some("微见智能, TFLN".into()));
        assert_eq!(terms_prompt(std::iter::empty()), None);
        let long: Vec<String> = (0..500).map(|i| format!("term{i}")).collect();
        let p = terms_prompt(long.iter().map(|s| s.as_str())).unwrap();
        assert!(p.chars().count() <= 400);
    }

    #[test]
    fn http_errors_are_actionable() {
        let msg = describe_http_error("OpenAI", 401, r#"{"error":{"message":"Incorrect API key"}}"#);
        assert!(msg.contains("API key"), "{msg}");
        assert!(msg.contains("Incorrect API key"), "{msg}");
        assert!(is_retryable(429) && is_retryable(503) && !is_retryable(400));
    }
}
