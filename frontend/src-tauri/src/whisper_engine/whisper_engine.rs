// Commit name to recover the serial whisper engine processing for smaller meetings [Slower processing but dooes not fail] - "before parallel processing implementation"

use std::path::{PathBuf};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use whisper_rs::{WhisperContext, WhisperContextParameters, FullParams, SamplingStrategy};
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use crate::config::WHISPER_MODEL_CATALOG;
use super::acceleration::{whisper_context_acceleration_for, WhisperCompiledBackend};
use crate::audio::transcription::text_cleanup;

/// Whisper's initial-prompt budget is roughly 224 tokens. CJK terms cost about one
/// token per character, so the prompt is capped by characters to stay inside it.
const MAX_PROMPT_CHARS: usize = 224;

/// Combine the custom-dictionary glossary with the tail of the previous transcript.
///
/// The glossary comes first and recent speech second because the tokens closest to
/// the audio influence the decode most. Context is skipped for `auto-translate`: the
/// output is English while the previous transcript is in the source language, and
/// mixing the two biases the translation.
pub(crate) fn build_initial_prompt(
    vocabulary: Option<&str>,
    previous_context: Option<&str>,
    language: Option<&str>,
) -> Option<String> {
    let vocabulary = vocabulary.map(str::trim).filter(|text| !text.is_empty());
    let context = previous_context
        .map(str::trim)
        .filter(|text| !text.is_empty() && language != Some("auto-translate"));

    let vocabulary_chars = vocabulary.map(|text| text.chars().count()).unwrap_or(0);
    let separator = if vocabulary.is_some() && context.is_some() { 1 } else { 0 };
    let context_budget = MAX_PROMPT_CHARS
        .saturating_sub(vocabulary_chars + separator)
        .min(text_cleanup::MAX_CONTEXT_CHARS);
    let context = context.and_then(|text| text_cleanup::context_tail(text, context_budget));

    match (vocabulary, context) {
        (Some(vocabulary), Some(context)) => Some(format!("{vocabulary} {context}")),
        (Some(vocabulary), None) => Some(vocabulary.to_string()),
        (None, Some(context)) => Some(context),
        (None, None) => None,
    }
}

/// Build the decoder parameters shared by both transcription entry points.
///
/// Kept in one place so the two paths cannot drift apart: the text-only path used to
/// override the hardware profile's temperature with a hard-coded value.
fn transcription_params<'a>(
    adaptive_config: &crate::audio::AdaptiveWhisperConfig,
    language: Option<&'a str>,
    vocabulary_prompt: Option<&str>,
    previous_context: Option<&str>,
) -> FullParams<'a, 'static> {
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: adaptive_config.beam_size as i32,
        patience: 1.0,
    });

    // None/"auto" -> language detection; "auto-translate" -> translate to English;
    // anything else is a language code.
    let (language_code, should_translate) = match language {
        Some("auto") | None => (None, false),
        Some("auto-translate") => (None, true),
        Some(lang) => (Some(lang), false),
    };
    params.set_language(language_code);
    params.set_translate(should_translate);

    // Custom-dictionary terms plus the previous transcript, bounded by the helper.
    if let Some(prompt) = build_initial_prompt(vocabulary_prompt, previous_context, language) {
        params.set_initial_prompt(&prompt);
    }

    // CRITICAL: timestamp tokens trigger whisper.cpp's "single timestamp ending - skip
    // the whole chunk" heuristic, which discards complete valid text.
    params.set_no_timestamps(true);
    params.set_token_timestamps(true);

    // Keep the C library quiet; its logging dominates the transcription hot path.
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_temperature(adaptive_config.temperature);
    params.set_max_initial_ts(1.0);
    params.set_entropy_thold(2.4);
    params.set_logprob_thold(-1.0);
    // 0.55 keeps quiet speech while still rejecting silence hallucinations.
    params.set_no_speech_thold(0.55);
    params.set_max_len(200);
    params.set_single_segment(false);

    params
}

/// Run CPU-heavy whisper work without parking a tokio worker thread.
///
/// `whisper.cpp` decode and model load are synchronous and can take seconds. Running
/// them inline in an async fn starves the capture pipeline (and every other async
/// command) on low-core machines. `block_in_place` requires a multi-thread runtime,
/// which is what Tauri installs; tests or embedders on a current-thread runtime fall
/// back to running inline instead of panicking.
fn run_blocking<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread =>
        {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelStatus {
    Available,
    Missing,
    Downloading { progress: u8 },
    Error(String),
    Corrupted { file_size: u64, expected_min_size: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_mb: u32,
    pub accuracy: String,
    pub speed: String,
    pub status: ModelStatus,
    pub description: String,
}

struct ActiveDownload {
    cancellation: CancellationToken,
    completion: watch::Sender<bool>,
}

#[cfg(test)]
struct DownloadStateTestHook {
    cancellation_first_lock_acquired: tokio::sync::Notify,
    continue_cancellation: tokio::sync::Notify,
    discovery_active_lock_acquired: tokio::sync::Notify,
    continue_discovery: tokio::sync::Notify,
}

#[derive(Debug, thiserror::Error)]
#[error("Download cancelled by user")]
pub(crate) struct DownloadCancelled;

pub(crate) fn is_download_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DownloadCancelled>().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CancelDownloadOutcome {
    Cancelled,
    Pending,
}

const CANCEL_DOWNLOAD_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct WhisperEngine {
    models_dir: PathBuf,
    current_context: Arc<RwLock<Option<WhisperContext>>>,
    current_model: Arc<RwLock<Option<String>>>,
    available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,
    // State tracking for smart logging
    last_transcription_was_short: Arc<RwLock<bool>>,
    short_audio_warning_logged: Arc<RwLock<bool>>,
    // Performance optimization: reduce logging frequency
    transcription_count: Arc<RwLock<u64>>,
    // A model remains active until its owning worker has completed cleanup.
    active_downloads: Arc<Mutex<HashMap<String, Arc<ActiveDownload>>>>,
    #[cfg(test)]
    download_state_test_hook: std::sync::Mutex<Option<Arc<DownloadStateTestHook>>>,
}

impl WhisperEngine {
    /// Detect available GPU acceleration capabilities
    fn detect_gpu_acceleration() -> bool {
        match WhisperCompiledBackend::current() {
            WhisperCompiledBackend::Metal => {
                log::info!("macOS detected - attempting to enable Metal GPU acceleration");
                true
            }
            WhisperCompiledBackend::Cuda => {
                log::info!("CUDA feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::Vulkan => {
                log::info!("Vulkan feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::HipBlas => {
                log::info!("HIP BLAS feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::Cpu => {
                log::info!("No GPU acceleration features detected - using CPU processing");
                false
            }
        }
    }

    pub fn new() -> Result<Self> {
        Self::new_with_models_dir(None)
    }

    /// Create a new WhisperEngine with optional custom models directory
    /// If models_dir is None, uses default location (app data dir for production, local for dev)
    pub fn new_with_models_dir(models_dir: Option<PathBuf>) -> Result<Self> {
        // PERFORMANCE: Suppress verbose whisper.cpp and Metal logs
        // These C library logs bypass Rust logging and clutter output
        // Set environment variables to reduce C library verbosity
        std::env::set_var("GGML_METAL_LOG_LEVEL", "1"); // 0=off, 1=error, 2=warn, 3=info
        std::env::set_var("WHISPER_LOG_LEVEL", "1");    // Reduce whisper.cpp verbosity

        let models_dir = if let Some(dir) = models_dir {
            // Use provided directory (for production with app_data_dir)
            dir
        } else {
            // Fallback: determine based on debug/release mode
            let current_dir = std::env::current_dir()
                .map_err(|e| anyhow!("Failed to get current directory: {}", e))?;

            // Development: Use frontend/models or backend directories
            // Production: Use system directories (should be overridden by caller)
            if cfg!(debug_assertions) {
                // Development mode - try frontend and backend directories
                if current_dir.join("models").exists() {
                    current_dir.join("models")
                } else if current_dir.join("../models").exists() {
                    current_dir.join("../models")
                } else if current_dir.join("backend/whisper-server-package/models").exists() {
                    current_dir.join("backend/whisper-server-package/models")
                } else if current_dir.join("../backend/whisper-server-package/models").exists() {
                    current_dir.join("../backend/whisper-server-package/models")
                } else {
                    // Create models directory in current directory for development
                    current_dir.join("models")
                }
            } else {
                // Production mode fallback (shouldn't reach here, caller should provide path)
                log::warn!("WhisperEngine: No models directory provided, using fallback path");
                dirs::data_dir()
                    .or_else(|| dirs::home_dir())
                    .ok_or_else(|| anyhow!("Could not find system data directory"))?
                    .join("Meetily")
                    .join("models")
            }
        };
        
        log::info!("WhisperEngine using models directory: {}", models_dir.display());
        log::info!("Debug mode: {}", cfg!(debug_assertions));

        // Log acceleration capabilities
        let gpu_support = Self::detect_gpu_acceleration();
        log::info!("Hardware acceleration support: {}", if gpu_support { "enabled" } else { "disabled" });

        #[cfg(feature = "metal")]
        log::info!("Apple Metal GPU support: enabled");

        #[cfg(feature = "openblas")]
        log::info!("OpenBLAS CPU optimization: enabled");

        #[cfg(feature = "coreml")]
        log::info!("Apple CoreML support: enabled");

        #[cfg(feature = "cuda")]
        log::info!("NVIDIA CUDA support: enabled");

        #[cfg(feature = "vulkan")]
        log::info!("Vulkan GPU support: enabled");

        #[cfg(feature = "openmp")]
        log::info!("OpenMP parallel processing: enabled");
        
        let engine = Self {
            models_dir,
            current_context: Arc::new(RwLock::new(None)),
            current_model: Arc::new(RwLock::new(None)),
            available_models: Arc::new(RwLock::new(HashMap::new())),
            // Initialize state tracking
            last_transcription_was_short: Arc::new(RwLock::new(false)),
            short_audio_warning_logged: Arc::new(RwLock::new(false)),
            // Performance optimization: reduce logging frequency
            transcription_count: Arc::new(RwLock::new(0)),
            active_downloads: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            download_state_test_hook: std::sync::Mutex::new(None),
        };
        
        Ok(engine)
    }

    #[cfg(test)]
    fn test_hook(&self) -> Option<Arc<DownloadStateTestHook>> {
        self.download_state_test_hook
            .lock()
            .expect("download state test hook mutex should not be poisoned")
            .clone()
    }
    
    pub async fn discover_models(&self) -> Result<Vec<ModelInfo>> {
        let models_dir = &self.models_dir;
        let mut models = Vec::new();
        // Use centralized model catalog from config.rs
        let model_configs = WHISPER_MODEL_CATALOG;

        for &(name, filename, size_mb, accuracy, speed, description) in model_configs {
            let model_path = models_dir.join(filename);
            let status = if model_path.exists() {
                // Check if file size is reasonable (at least 1MB for a valid model)
                match std::fs::metadata(&model_path) {
                    Ok(metadata) => {
                        let file_size_bytes = metadata.len();
                        let file_size_mb = file_size_bytes / (1024 * 1024);
                        let expected_min_size_mb = (size_mb as f64 * 0.9) as u64; // Allow 90% of expected size as minimum for more accurate corruption detection

                        if file_size_mb >= expected_min_size_mb && file_size_mb > 1 {
                            // File size looks good, but let's also check if it's a valid GGML file
                            match self.validate_model_file(&model_path).await {
                                Ok(_) => ModelStatus::Available,
                                Err(_) => {
                                    log::warn!("Model file {} has correct size but appears corrupted (failed validation)",
                                             filename);
                                    ModelStatus::Corrupted {
                                        file_size: file_size_bytes,
                                        expected_min_size: (expected_min_size_mb * 1024 * 1024) as u64
                                    }
                                }
                            }
                        } else if file_size_mb > 0 {
                            log::warn!("Model file {} exists but is corrupted ({} MB, expected ~{} MB)",
                                     filename, file_size_mb, size_mb);
                            ModelStatus::Corrupted {
                                file_size: file_size_bytes,
                                expected_min_size: (expected_min_size_mb * 1024 * 1024) as u64
                            }
                        } else {
                            ModelStatus::Missing
                        }
                    }
                    Err(_) => ModelStatus::Missing
                }
            } else {
                ModelStatus::Missing
            };
            
            let model_info = ModelInfo {
                name: name.to_string(),
                path: model_path,
                size_mb: size_mb as u32,
                accuracy: accuracy.to_string(),
                speed: speed.to_string(),
                status,
                description: description.to_string(),
            };
            
            models.push(model_info);
        }
        
        let active_downloads = self.active_downloads.lock().await;
        #[cfg(test)]
        if let Some(hook) = self.test_hook() {
            hook.discovery_active_lock_acquired.notify_one();
            hook.continue_discovery.notified().await;
        }
        let mut available_models = self.available_models.write().await;
        for model in &mut models {
            if active_downloads.contains_key(&model.name) {
                let progress = available_models
                    .get(&model.name)
                    .and_then(|cached| match cached.status {
                        ModelStatus::Downloading { progress } => Some(progress),
                        _ => None,
                    })
                    .unwrap_or(0);
                model.status = ModelStatus::Downloading { progress };
            }
        }
        available_models.clear();
        for model in &models {
            available_models.insert(model.name.clone(), model.clone());
        }
        
        Ok(models)
    }
    
    pub async fn load_model(&self, model_name: &str) -> Result<()> {
        // Copy what is needed and release the list lock: loading a model takes
        // seconds and holding it here would stall discover_models (and download
        // progress updates) for the whole load.
        let (model_status, model_path) = {
            let models = self.available_models.read().await;
            let model_info = models.get(model_name)
                .ok_or_else(|| anyhow!("Model {} not found", model_name))?;
            (model_info.status.clone(), model_info.path.clone())
        };

        match model_status {
            ModelStatus::Available => {
                // FIX 5: Check if this model is already loaded
                if let Some(current_model) = self.current_model.read().await.as_ref() {
                    if current_model == model_name {
                        log::info!("Model {} is already loaded, skipping reload", model_name);
                        return Ok(());
                    }

                    // FIX 5: Unload current model before loading new one
                    log::info!("Unloading current model '{}' before loading '{}'", current_model, model_name);
                    self.unload_model().await;
                }

                log::info!("Loading model: {}", model_name);

                // PERFORMANCE OPTIMIZATION: Use comprehensive hardware profile for optimal GPU configuration
                let hardware_profile = crate::audio::HardwareProfile::detect();
                let adaptive_config = hardware_profile.get_whisper_config();
                let acceleration = whisper_context_acceleration_for(
                    WhisperCompiledBackend::current(),
                    hardware_profile.gpu_type,
                    hardware_profile.performance_tier,
                );

                let context_param = WhisperContextParameters {
                    use_gpu: acceleration.use_gpu,
                    gpu_device: acceleration.gpu_device,
                    flash_attn: acceleration.flash_attn,
                    ..Default::default()
                };

                log::info!(
                    "Whisper acceleration decision: compiled_backend={} runtime_detected_gpu={:?} use_gpu={} flash_attn={} gpu_device={}",
                    acceleration.compiled_backend.as_str(),
                    acceleration.runtime_detected_gpu,
                    acceleration.use_gpu,
                    acceleration.flash_attn,
                    acceleration.gpu_device,
                );

                // PERFORMANCE: Suppress verbose C library logs during model loading
                // This hides the excessive Metal/GGML initialization logs in release builds
                let ctx = {
                    // let _suppressor = crate::whisper_engine::StderrSuppressor::new();

                    // Load whisper context with hardware-optimized parameters.
                    // `path` takes `AsRef<Path>`, so pass the path directly rather
                    // than a `Cow<str>` (which is not `AsRef<Path>`). Model loading is
                    // blocking for seconds, so keep it off the async runtime.
                    run_blocking(|| WhisperContext::new_with_params(&model_path, context_param))
                        .map_err(|e| anyhow!("Failed to load model {}: {}", model_name, e))?
                    // Suppressor dropped here, stderr restored
                };

                // Update current context and model
                *self.current_context.write().await = Some(ctx);
                *self.current_model.write().await = Some(model_name.to_string());

                // Enhanced acceleration status reporting
                let acceleration_status = acceleration.status_label();

                log::info!("Successfully loaded model: {} with {} (Performance Tier: {:?}, Beam Size: {}, Threads: {:?})",
                          model_name, acceleration_status, hardware_profile.performance_tier,
                          adaptive_config.beam_size, adaptive_config.max_threads);
                Ok(())
            },
            ModelStatus::Missing => {
                Err(anyhow!("Model {} is not downloaded", model_name))
            },
            ModelStatus::Downloading { .. } => {
                Err(anyhow!("Model {} is currently downloading", model_name))
            },
            ModelStatus::Error(ref err) => {
                Err(anyhow!("Model {} has error: {}", model_name, err))
            },
            ModelStatus::Corrupted { .. } => {
                Err(anyhow!("Model {} is corrupted and cannot be loaded", model_name))
            }
        }
    }

    pub async fn unload_model(&self) -> bool  {
        let mut ctx_guard = self.current_context.write().await;
        let unloaded = ctx_guard.take().is_some();
        if unloaded {
            log::info!("📉Whisper model unloaded");
        }

        let mut model_name_guard = self.current_model.write().await;
        model_name_guard.take();

        unloaded
    }

    pub async fn get_current_model(&self) -> Option<String> {
        self.current_model.read().await.clone()
    }
    
    pub async fn is_model_loaded(&self) -> bool {
        self.current_context.read().await.is_some()
    }
    
    // Enhanced function to clean repetitive text patterns and meaningless outputs
    fn clean_repetitive_text(text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }

        // Check for obviously meaningless patterns first
        if Self::is_meaningless_output(text) {
            // Performance optimization: reduce meaningless output logging to debug level
            perf_debug!("Detected meaningless output, returning empty: '{}'", text);
            return String::new();
        }

        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 3 {
            return text.to_string();
        }

        // Enhanced repetition detection with sliding window
        let cleaned_words = Self::remove_word_repetitions(&words);

        // Remove phrase repetitions with more sophisticated detection
        let cleaned_words = Self::remove_phrase_repetitions(&cleaned_words);

        // Check for overall repetition ratio
        let final_text = cleaned_words.join(" ");
        if Self::calculate_repetition_ratio(&final_text) > 0.7 {
            // Performance optimization: reduce repetition ratio logging to debug level
            perf_debug!("High repetition ratio detected, filtering out: '{}'", final_text);
            return String::new();
        }

        final_text
    }

    // Check for obviously meaningless patterns
    fn is_meaningless_output(text: &str) -> bool {
        let text_lower = text.to_lowercase();

        // Check for common meaningless patterns
        let meaningless_patterns = [
            "thank you for watching",
            "thanks for watching",
            "like and subscribe",
            "music playing",
            "applause",
            "laughter",
            "um um um",
            "uh uh uh",
            "ah ah ah",
        ];

        for pattern in &meaningless_patterns {
            if text_lower.contains(pattern) {
                return true;
            }
        }

        // Check if text is mostly the same character or very short repetitive patterns
        let unique_chars: HashSet<char> = text.chars().collect();
        if unique_chars.len() <= 3 && text.len() > 10 {
            return true;
        }

        false
    }

    // Enhanced word repetition removal
    fn remove_word_repetitions<'a>(words: &'a [&'a str]) -> Vec<&'a str> {
        let mut cleaned_words = Vec::new();
        let mut i = 0;

        while i < words.len() {
            let current_word = words[i];
            let mut repeat_count = 1;

            // Count consecutive repetitions of the same word
            while i + repeat_count < words.len() && words[i + repeat_count] == current_word {
                repeat_count += 1;
            }

            // Be more aggressive: if word is repeated 2+ times, only keep one instance
            if repeat_count >= 2 {
                cleaned_words.push(current_word);
                i += repeat_count;
            } else {
                cleaned_words.push(current_word);
                i += 1;
            }
        }

        cleaned_words
    }

    // Enhanced phrase repetition removal with variable length detection
    fn remove_phrase_repetitions<'a>(words: &'a [&'a str]) -> Vec<&'a str> {
        if words.len() < 4 {
            return words.to_vec();
        }

        let mut final_words = Vec::new();
        let mut i = 0;

        while i < words.len() {
            let mut phrase_found = false;

            // Check for 2-word to 5-word phrase repetitions
            for phrase_len in 2..=std::cmp::min(5, (words.len() - i) / 2) {
                if i + phrase_len * 2 <= words.len() {
                    let phrase1 = &words[i..i + phrase_len];
                    let phrase2 = &words[i + phrase_len..i + phrase_len * 2];

                    if phrase1 == phrase2 {
                        // Add the phrase once and skip the repetition
                        final_words.extend_from_slice(phrase1);
                        i += phrase_len * 2;
                        phrase_found = true;
                        break;
                    }
                }
            }

            if !phrase_found {
                final_words.push(words[i]);
                i += 1;
            }
        }

        final_words
    }

    // Calculate repetition ratio in text
    fn calculate_repetition_ratio(text: &str) -> f32 {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() < 4 {
            return 0.0;
        }

        let mut word_counts = HashMap::new();
        for word in &words {
            *word_counts.entry(word.to_lowercase()).or_insert(0) += 1;
        }

        let total_words = words.len() as f32;
        let repeated_words: usize = word_counts.values().map(|&count| if count > 1 { count - 1 } else { 0 }).sum();

        repeated_words as f32 / total_words
    }
    
    /// Transcribe audio with streaming support for partial results and adaptive quality
    pub async fn transcribe_audio_with_confidence(&self, audio_data: Vec<f32>, language: Option<String>) -> Result<(String, f32, bool)> {
        self.transcribe_audio_with_confidence_and_context(audio_data, language, None).await
    }

    /// Transcribe with confidence, optionally primed with the tail of the previous
    /// transcript from the same session.
    ///
    /// `previous_context` is bounded by the prompt builder and is only ever supplied
    /// by the caller's own session (the live worker owns it). It exists so names and
    /// jargon stay stable across consecutive VAD segments without any global state.
    pub async fn transcribe_audio_with_confidence_and_context(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        previous_context: Option<String>,
    ) -> Result<(String, f32, bool)> {
        let ctx_lock = self.current_context.read().await;
        let ctx = ctx_lock.as_ref()
            .ok_or_else(|| anyhow!("No model loaded. Please load a model first."))?;

        let hardware_profile = crate::audio::HardwareProfile::detect();
        let adaptive_config = hardware_profile.get_whisper_config();
        let vocabulary_prompt = crate::dictionary::active_vocabulary_prompt();
        let params = transcription_params(
            &adaptive_config,
            language.as_deref(),
            vocabulary_prompt.as_deref(),
            previous_context.as_deref(),
        );

        let duration_seconds = audio_data.len() as f64 / 16000.0;
        let is_partial = duration_seconds < 15.0; // Consider chunks under 15s as partial

        // The decode plus segment extraction is CPU-bound for seconds; keep it off the
        // async worker threads (see `run_blocking`).
        let segment_texts = run_blocking(|| -> Result<Vec<String>> {
            let mut state = ctx.create_state()?;
            state.full(params, &audio_data)?;
            let num_segments = state.full_n_segments() as usize;

            let mut segment_texts: Vec<String> = Vec::new();
            for i in 0..num_segments {
                let Some(segment) = state.get_segment(i as i32) else { continue; };
                let Ok(segment_text) = segment.to_str_lossy() else { continue; };
                segment_texts.push(segment_text.into_owned());
            }

            Ok(segment_texts)
        })?;

        let mut total_confidence = 0.0;
        let mut segment_count = 0;
        for segment_text in &segment_texts {
            // Segment length stands in for decode confidence here; keep the same
            // measure so the reported confidence number does not change.
            let segment_length = segment_text.len() as f32;
            let segment_confidence = if segment_length > 0.0 {
                (segment_length / 100.0).min(0.9) + 0.1
            } else {
                0.1
            };
            total_confidence += segment_confidence;
            segment_count += 1;
        }

        let final_result =
            text_cleanup::join_segments(segment_texts.iter().map(|text| text.as_str()));
        let cleaned_result = Self::clean_repetitive_text(&final_result);

        let avg_confidence = if segment_count > 0 {
            total_confidence / segment_count as f32
        } else {
            0.0
        };

        Ok((cleaned_result, avg_confidence, is_partial))
    }

    pub async fn transcribe_audio(&self, audio_data: Vec<f32>, language: Option<String>) -> Result<String> {
        let ctx_lock = self.current_context.read().await;
        let ctx = ctx_lock.as_ref()
            .ok_or_else(|| anyhow!("No model loaded. Please load a model first."))?;

        let hardware_profile = crate::audio::HardwareProfile::detect();
        let adaptive_config = hardware_profile.get_whisper_config();
        let vocabulary_prompt = crate::dictionary::active_vocabulary_prompt();
        // No previous-transcript context here: batch callers transcribe independent
        // chunks, so a preceding transcript would come from unrelated audio.
        let params = transcription_params(
            &adaptive_config,
            language.as_deref(),
            vocabulary_prompt.as_deref(),
            None,
        );
        let duration_seconds = audio_data.len() as f64 / 16000.0; // Assuming 16kHz
        let is_short_audio = duration_seconds < 1.0;

        // Smart logging based on audio duration and previous states
        let mut should_log_transcription = true;
        let mut should_log_short_warning = false;

        if is_short_audio {
            let last_was_short = *self.last_transcription_was_short.read().await;
            let warning_logged = *self.short_audio_warning_logged.read().await;

            if !warning_logged {
                should_log_short_warning = true;
                *self.short_audio_warning_logged.write().await = true;
            }

            // Only log transcription start if it's the first short audio or previous wasn't short
            should_log_transcription = !last_was_short;

            *self.last_transcription_was_short.write().await = true;
        } else {
            let last_was_short = *self.last_transcription_was_short.read().await;

            // Always log when transitioning from short to normal audio
            if last_was_short {
                log::info!("Audio duration normalized, resuming transcription");
                *self.short_audio_warning_logged.write().await = false;
            }

            *self.last_transcription_was_short.write().await = false;
        }

        if should_log_short_warning {
            log::warn!("Audio duration is short ({:.1}s < 1.0s). Consider padding the input audio with silence. Further short audio warnings will be suppressed.", duration_seconds);
        }

        // Performance optimization: reduce transcription start logging frequency
        let transcription_count = {
            let mut count = self.transcription_count.write().await;
            *count += 1;
            *count
        };

        // Only log every 10th transcription or significant audio (>10s) to reduce I/O overhead
        if should_log_transcription && (transcription_count % 10 == 0 || duration_seconds > 10.0) {
            log::info!("Starting transcription #{} of {} samples ({:.1}s duration)",
                      transcription_count, audio_data.len(), duration_seconds);
        }
        // The decode plus segment extraction is CPU-bound for seconds; keep it off the
        // async worker threads (see `run_blocking`).
        let (num_segments, segment_texts) = run_blocking(|| -> Result<(i32, Vec<String>)> {
            let mut state = ctx.create_state()?;
            state.full(params, &audio_data)?;

            // `full_n_segments` returns a plain c_int in whisper-rs 0.16.
            let num_segments = state.full_n_segments();
            let mut segment_texts: Vec<String> = Vec::new();

            for i in 0..num_segments {
                // Fetch each segment once: text and timestamps both come from the same
                // handle instead of three separate C calls per segment.
                let Some(segment) = state.get_segment(i) else { continue; };
                let Ok(segment_text) = segment.to_str_lossy() else { continue; };

                // Only long recordings log segments; keep timestamps out of the hot
                // path unless they are about to be printed.
                if duration_seconds > 30.0 {
                    perf_trace!("Segment {} ({:.2}s-{:.2}s): '{}'",
                               i, segment.start_timestamp() as f64 / 100.0,
                               segment.end_timestamp() as f64 / 100.0, segment_text);
                }

                if !segment_text.trim().is_empty() {
                    segment_texts.push(segment_text.into_owned());
                }
            }

            Ok((num_segments, segment_texts))
        })?;

        // Performance optimization: reduce segment completion logging
        // Only log for significant transcriptions to avoid I/O overhead
        if (should_log_transcription || num_segments > 0) && (num_segments > 3 || duration_seconds > 5.0) {
            perf_debug!("Transcription #{} completed with {} segments ({:.1}s)", transcription_count, num_segments, duration_seconds);
        }

        let final_result =
            text_cleanup::join_segments(segment_texts.iter().map(|text| text.as_str()));

        // Check for repetition loops and clean them up
        let cleaned_result = Self::clean_repetitive_text(&final_result);

        // Performance optimization: smart logging for transcription results
        if cleaned_result.is_empty() {
            // Only log empty results occasionally to reduce spam
            if should_log_transcription && transcription_count % 20 == 0 {
                perf_debug!("Transcription #{} result is empty - no speech detected", transcription_count);
            }
        } else {
            if cleaned_result != final_result {
                log::info!("Cleaned repetitive transcription #{}: '{}' -> '{}'", transcription_count, final_result, cleaned_result);
            }
            // Reduce successful transcription logging frequency
            // Only log every 5th result or significant results (>50 chars) to reduce I/O overhead
            if transcription_count % 5 == 0 || cleaned_result.len() > 50 || duration_seconds > 10.0 {
                log::info!("Transcription #{} result: '{}'", transcription_count, cleaned_result);
            } else {
                perf_debug!("Transcription #{} result: '{}'", transcription_count, cleaned_result);
            }
        }

        Ok(cleaned_result)
    }
    
    pub async fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }

    /// Validate if a model file is a valid GGML file by checking its header
    async fn validate_model_file(&self, model_path: &PathBuf) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut file = fs::File::open(model_path).await
            .map_err(|e| anyhow!("Failed to open model file: {}", e))?;

        // Read the first 8 bytes to check for GGML magic number
        let mut buffer = [0u8; 8];
        file.read_exact(&mut buffer).await
            .map_err(|e| anyhow!("Failed to read model file header: {}", e))?;

        // Check for GGML magic number (various versions and endianness)
        if buffer.starts_with(b"ggml") || buffer.starts_with(b"GGUF") || buffer.starts_with(b"ggmf") ||
           buffer.starts_with(b"lmgg") || buffer.starts_with(b"FUGU") || buffer.starts_with(b"fmgg") {
            Ok(())
        } else {
            Err(anyhow!("Invalid model file: missing GGML/GGUF magic number. Found: {:?}",
                       String::from_utf8_lossy(&buffer[..4])))
        }
    }

    pub async fn delete_model(&self, model_name: &str) -> Result<String> {
        log::info!("Attempting to delete model: {}", model_name);

        // Get model info to find the file path
        let model_info = {
            let models = self.available_models.read().await;
            models.get(model_name).cloned()
        };

        let model_info = model_info.ok_or_else(|| anyhow!("Model '{}' not found", model_name))?;

        // Check if model is corrupted before allowing deletion
        log::info!("Model '{}' has status: {:?}", model_name, model_info.status);
        match &model_info.status {
            ModelStatus::Corrupted { file_size, expected_min_size } => {
                log::info!("Deleting corrupted model '{}' (file size: {} bytes, expected min: {} bytes)",
                          model_name, file_size, expected_min_size);

                // Delete the file
                if model_info.path.exists() {
                    fs::remove_file(&model_info.path).await
                        .map_err(|e| anyhow!("Failed to delete file '{}': {}", model_info.path.display(), e))?;
                    log::info!("Successfully deleted corrupted file: {}", model_info.path.display());
                } else {
                    log::warn!("File '{}' does not exist, nothing to delete", model_info.path.display());
                }

                // Update model status to Missing
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model) = models.get_mut(model_name) {
                        model.status = ModelStatus::Missing;
                    }
                }

                Ok(format!("Successfully deleted corrupted model '{}'", model_name))
            }
            ModelStatus::Available => {
                // Allow deletion of available models for testing/cleanup
                log::info!("Deleting available model '{}' (for cleanup)", model_name);

                if model_info.path.exists() {
                    fs::remove_file(&model_info.path).await
                        .map_err(|e| anyhow!("Failed to delete file '{}': {}", model_info.path.display(), e))?;
                    log::info!("Successfully deleted available model file: {}", model_info.path.display());
                } else {
                    log::warn!("File '{}' does not exist, nothing to delete", model_info.path.display());
                }

                // Update model status to Missing
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model) = models.get_mut(model_name) {
                        model.status = ModelStatus::Missing;
                    }
                }

                Ok(format!("Successfully deleted model '{}'", model_name))
            }
            _ => {
                Err(anyhow!("Can only delete corrupted or available models. Model '{}' has status: {:?}", model_name, model_info.status))
            }
        }
    }
    
    async fn reserve_active_download(&self, model_name: &str) -> Result<Arc<ActiveDownload>> {
        let mut active_downloads = self.active_downloads.lock().await;
        if active_downloads.contains_key(model_name) {
            return Err(anyhow!("Download already in progress for model: {}", model_name));
        }

        let (completion, _) = watch::channel(false);
        let active_download = Arc::new(ActiveDownload {
            cancellation: CancellationToken::new(),
            completion,
        });

        active_downloads.insert(model_name.to_string(), Arc::clone(&active_download));
        Ok(active_download)
    }

    async fn finish_download(
        &self,
        model_name: &str,
        active_download: &Arc<ActiveDownload>,
        file_path: &PathBuf,
        mut result: Result<()>,
    ) -> Result<()> {
        let active_owner_matches = {
            let active_downloads = self.active_downloads.lock().await;
            matches!(
                active_downloads.get(model_name),
                Some(current) if Arc::ptr_eq(current, active_download)
            )
        };
        if !active_owner_matches {
            log::warn!("Download owner for {} was no longer active during finalization", model_name);
            active_download.completion.send_replace(true);
            return result;
        }

        if result.is_ok() && !active_download.cancellation.is_cancelled() {
            result = self.validate_model_file(file_path).await;

            if result.is_ok() {
                let expected_min_size = WHISPER_MODEL_CATALOG
                    .iter()
                    .find(|model| model.0 == model_name)
                    .map(|model| ((model.2 as f64 * 0.9) as u64) * 1024 * 1024);

                result = match expected_min_size {
                    Some(expected_min_size) => match fs::metadata(file_path).await {
                        Ok(metadata) if metadata.len() >= expected_min_size => Ok(()),
                        Ok(metadata) => Err(anyhow!(
                            "Downloaded model file is too small: {} bytes (expected at least {} bytes)",
                            metadata.len(),
                            expected_min_size
                        )),
                        Err(e) => Err(anyhow!(
                            "Failed to read downloaded model file metadata: {}",
                            e
                        )),
                    },
                    None => Err(anyhow!(
                        "Unsupported model for download validation: {}",
                        model_name
                    )),
                };
            }
        }

        if result.is_err() && !active_download.cancellation.is_cancelled() && file_path.exists() {
            if let Err(e) = fs::remove_file(file_path).await {
                log::warn!("Failed to clean up failed download file: {}", e);
            } else {
                log::info!("Cleaned up failed download file: {}", file_path.display());
            }
        }

        let (released_active_owner, cancellation_won) = {
            let mut active_downloads = self.active_downloads.lock().await;
            match active_downloads.get(model_name) {
                Some(current) if Arc::ptr_eq(current, active_download) => {
                    if active_download.cancellation.is_cancelled() {
                        (false, true)
                    } else {
                        active_downloads.remove(model_name);
                        (true, false)
                    }
                }
                _ => {
                    log::warn!("Download owner for {} was no longer active during finalization", model_name);
                    (false, active_download.cancellation.is_cancelled())
                }
            }
        };

        if cancellation_won {
            result = Err(DownloadCancelled.into());
            if file_path.exists() {
                if let Err(e) = fs::remove_file(file_path).await {
                    log::warn!("Failed to clean up cancelled download file: {}", e);
                } else {
                    log::info!("Cleaned up cancelled download file: {}", file_path.display());
                }
            }

            let removed = {
                let mut active_downloads = self.active_downloads.lock().await;
                #[cfg(test)]
                if let Some(hook) = self.test_hook() {
                    hook.cancellation_first_lock_acquired.notify_one();
                    hook.continue_cancellation.notified().await;
                }

                let mut models = self.available_models.write().await;
                match active_downloads.get(model_name) {
                    Some(current) if Arc::ptr_eq(current, active_download) => {
                        if let Some(model_info) = models.get_mut(model_name) {
                            model_info.status = ModelStatus::Missing;
                        }
                        active_downloads.remove(model_name);
                        true
                    }
                    _ => false,
                }
            };
            if !removed {
                log::warn!("Download owner for {} was no longer active during cancellation cleanup", model_name);
            }
        } else if released_active_owner {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                if result.is_ok() {
                    model_info.status = ModelStatus::Available;
                    model_info.path = file_path.clone();
                } else {
                    model_info.status = ModelStatus::Missing;
                }
            }
        }

        active_download.completion.send_replace(true);
        result
    }

    pub async fn download_model(&self, model_name: &str, progress_callback: Option<Box<dyn Fn(u8) + Send>>) -> Result<()> {
        let base_url = crate::config::whisper_model_base_url();
        log::info!(
            "Starting download for model: {} (base URL: {})",
            model_name,
            base_url
        );

        // Filenames come from the catalog rather than a second hardcoded table, so
        // the download URL and the on-disk name used by discovery cannot drift.
        let file_name = crate::config::WHISPER_MODEL_CATALOG
            .iter()
            .find(|(name, ..)| *name == model_name)
            .map(|(_, filename, ..)| *filename)
            .ok_or_else(|| {
                anyhow!(
                    "Unsupported model: {}. Available: {}",
                    model_name,
                    crate::config::WHISPER_MODEL_CATALOG
                        .iter()
                        .map(|(name, ..)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;

        let model_url = crate::config::whisper_model_url(file_name);

        self.download_model_from_url(model_name, &model_url, progress_callback).await
    }

    async fn download_model_from_url(
        &self,
        model_name: &str,
        model_url: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        let active_download = self.reserve_active_download(model_name).await?;
        let file_path = self.models_dir.join(format!("ggml-{}.bin", model_name));

        let result = self
            .download_model_with_owner(
                model_name,
                model_url,
                &file_path,
                &active_download,
                progress_callback,
            )
            .await;

        self.finish_download(model_name, &active_download, &file_path, result)
            .await
    }

    async fn download_model_with_owner(
        &self,
        model_name: &str,
        model_url: &str,
        file_path: &PathBuf,
        active_download: &ActiveDownload,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        if active_download.cancellation.is_cancelled() {
            return Err(DownloadCancelled.into());
        }

        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir)
                .await
                .map_err(|e| anyhow!("Failed to create models directory: {}", e))?;
        }

        // Refuse before writing anything when the target volume cannot hold the
        // file. Without this the download streams into a nearly-full disk and
        // fails partway through, leaving a truncated `ggml-*.bin` that later
        // reads as a corrupt model — a much harder failure to diagnose than
        // "not enough space".
        crate::whisper_engine::download::ensure_space_for_model(&self.models_dir, model_name)?;

        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Downloading { progress: 0 };
            }
        }

        // Proxy-aware client: `Client::new()` ignores the OS proxy, which makes
        // downloads fail outright on machines that need one.
        let client = crate::http_client::client_with_proxy();
        let response = tokio::select! {
            biased;
            _ = active_download.cancellation.cancelled() => return Err(DownloadCancelled.into()),
            response = client.get(model_url).send() => response
                .map_err(|e| anyhow!("Failed to start download: {}", e))?,
        };

        if !response.status().is_success() {
            return Err(anyhow!("Download failed with status: {}", response.status()));
        }

        let total_size = response.content_length().unwrap_or(0);
        let mut file = fs::File::create(file_path)
            .await
            .map_err(|e| anyhow!("Failed to create file: {}", e))?;

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;
        let mut last_progress_report = 0u8;
        let mut last_report_time = std::time::Instant::now();

        if let Some(ref callback) = progress_callback {
            callback(0);
        }

        loop {
            let chunk_result = tokio::select! {
                biased;
                _ = active_download.cancellation.cancelled() => return Err(DownloadCancelled.into()),
                chunk_result = stream.next() => chunk_result,
            };

            let Some(chunk_result) = chunk_result else {
                break;
            };

            if active_download.cancellation.is_cancelled() {
                return Err(DownloadCancelled.into());
            }

            let chunk = chunk_result.map_err(|e| anyhow!("Failed to read chunk: {}", e))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| anyhow!("Failed to write chunk to file: {}", e))?;

            downloaded += chunk.len() as u64;
            let progress = if total_size > 0 {
                ((downloaded as f64 / total_size as f64) * 100.0) as u8
            } else {
                0
            };

            let time_since_last_report = last_report_time.elapsed().as_secs();
            if progress >= last_progress_report + 1 || progress == 100 || time_since_last_report >= 2 {
                let mut models = self.available_models.write().await;
                if let Some(model_info) = models.get_mut(model_name) {
                    model_info.status = ModelStatus::Downloading { progress };
                }

                if let Some(ref callback) = progress_callback {
                    callback(progress);
                }

                last_progress_report = progress;
                last_report_time = std::time::Instant::now();
            }
        }

        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Downloading { progress: 100 };
            }
        }

        if let Some(ref callback) = progress_callback {
            callback(100);
        }

        file.flush()
            .await
            .map_err(|e| anyhow!("Failed to flush file: {}", e))?;

        if active_download.cancellation.is_cancelled() {
            return Err(DownloadCancelled.into());
        }


        Ok(())
    }

    pub async fn cancel_download(&self, model_name: &str) -> Result<CancelDownloadOutcome> {
        self.cancel_download_with_timeout(model_name, CANCEL_DOWNLOAD_CLEANUP_TIMEOUT)
            .await
    }

    async fn cancel_download_with_timeout(
        &self,
        model_name: &str,
        cleanup_timeout: Duration,
    ) -> Result<CancelDownloadOutcome> {
        log::info!("Cancelling download for model: {}", model_name);

        let Some(active_download) = self.active_downloads.lock().await.get(model_name).cloned() else {
            return Ok(CancelDownloadOutcome::Cancelled);
        };

        active_download.cancellation.cancel();

        let mut completion = active_download.completion.subscribe();
        if !*completion.borrow() {
            match tokio::time::timeout(cleanup_timeout, completion.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    return Err(anyhow!("Download worker ended before completing cancellation cleanup"));
                }
                Err(_) => {
                    return Ok(CancelDownloadOutcome::Pending);
                }
            }
        }

        Ok(CancelDownloadOutcome::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::time::{timeout, Duration};

    #[test]
    fn prompt_builder_keeps_vocabulary_and_bounds_context() {
        // Vocabulary-only behaviour is unchanged.
        assert_eq!(
            build_initial_prompt(Some("Meetly、Parakeet"), None, Some("en")).as_deref(),
            Some("Meetly、Parakeet")
        );
        assert_eq!(build_initial_prompt(None, None, None), None);
        assert_eq!(build_initial_prompt(Some("  "), Some("  "), Some("en")), None);

        // Context follows the glossary, separated by one space.
        let prompt =
            build_initial_prompt(Some("Meetly"), Some("we discussed the budget"), Some("en"))
                .expect("prompt");
        assert_eq!(prompt, "Meetly we discussed the budget");

        // Long context is cut to its tail and the whole prompt stays under the cap.
        let long_context = "word ".repeat(200);
        let prompt =
            build_initial_prompt(Some("glossary"), Some(&long_context), Some("en")).expect("prompt");
        assert!(prompt.chars().count() <= MAX_PROMPT_CHARS);
        assert!(prompt.starts_with("glossary "));
        assert!(prompt.ends_with("word"));
    }

    #[test]
    fn prompt_builder_skips_context_for_translation() {
        let prompt =
            build_initial_prompt(Some("Meetly"), Some("previous words"), Some("auto-translate"));
        assert_eq!(prompt.as_deref(), Some("Meetly"));

        // Detection mode ("auto") still gets context; only translation drops it.
        assert_eq!(
            build_initial_prompt(None, Some("previous words"), Some("auto")).as_deref(),
            Some("previous words")
        );
    }

    fn tiny_model(models: &[ModelInfo]) -> &ModelInfo {
        models
            .iter()
            .find(|model| model.name == "tiny")
            .expect("tiny should be present in the Whisper model catalog")
    }

    #[tokio::test]
    async fn discover_models_keeps_active_downloads_downloading_before_a_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();

        let _active_download = engine.reserve_active_download("tiny").await.unwrap();
        engine.available_models.write().await.insert(
            "tiny".to_string(),
            ModelInfo {
                name: "tiny".to_string(),
                path: dir.path().join("ggml-tiny.bin"),
                size_mb: 75,
                accuracy: "Basic".to_string(),
                speed: "Fast".to_string(),
                status: ModelStatus::Downloading { progress: 42 },
                description: "test model".to_string(),
            },
        );

        let absent = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&absent).status,
            ModelStatus::Downloading { progress: 42 }
        ));

        std::fs::write(dir.path().join("ggml-tiny.bin"), b"").unwrap();
        let zero_byte = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&zero_byte).status,
            ModelStatus::Downloading { progress: 42 }
        ));

        std::fs::write(dir.path().join("ggml-tiny.bin"), [0_u8; 32]).unwrap();
        let partial = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&partial).status,
            ModelStatus::Downloading { progress: 42 }
        ));
    }

    #[tokio::test]
    async fn discover_models_uses_disk_state_after_a_download_is_no_longer_active() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();

        let model_path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&model_path, b"").unwrap();
        let missing_models = engine.discover_models().await.unwrap();
        assert!(matches!(tiny_model(&missing_models).status, ModelStatus::Missing));

        std::fs::write(&model_path, vec![0_u8; 2 * 1024 * 1024]).unwrap();
        let corrupted_models = engine.discover_models().await.unwrap();

        assert!(matches!(
            tiny_model(&corrupted_models).status,
            ModelStatus::Corrupted { .. }
        ));
    }

    #[tokio::test]
    async fn finish_download_publishes_available_after_releasing_active_owner() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");

        std::fs::write(&model_path, b"ggml\0\0\0\0").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&model_path)
            .unwrap()
            .set_len(68 * 1024 * 1024)
            .unwrap();

        engine.discover_models().await.unwrap();
        let active_download = engine.reserve_active_download("tiny").await.unwrap();
        let active_models = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&active_models).status,
            ModelStatus::Downloading { progress: 0 }
        ));

        engine
            .finish_download("tiny", &active_download, &model_path, Ok(()))
            .await
            .unwrap();

        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(matches!(
            engine
                .available_models
                .read()
                .await
                .get("tiny")
                .expect("tiny should remain cached")
                .status,
            ModelStatus::Available
        ));

        let rediscovered_models = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&rediscovered_models).status,
            ModelStatus::Available
        ));
    }

    #[tokio::test]
    async fn finish_download_rejects_invalid_header_and_cleans_file() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&model_path, b"not-a-model").unwrap();

        engine.discover_models().await.unwrap();
        let active_download = engine.reserve_active_download("tiny").await.unwrap();
        let error = engine
            .finish_download("tiny", &active_download, &model_path, Ok(()))
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("Invalid model file: missing GGML/GGUF magic number"));
        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(!model_path.exists());
        assert!(matches!(
            engine
                .available_models
                .read()
                .await
                .get("tiny")
                .expect("tiny should remain cached")
                .status,
            ModelStatus::Missing
        ));
    }

    #[tokio::test]
    async fn unsupported_download_does_not_leave_an_active_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();

        assert!(engine.download_model("not-a-model", None).await.is_err());
        assert!(!engine.active_downloads.lock().await.contains_key("not-a-model"));
    }

    #[tokio::test]
    async fn duplicate_download_reservation_is_rejected_while_the_first_owner_is_active() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();

        let _first_owner = engine.reserve_active_download("tiny").await.unwrap();
        assert!(engine.reserve_active_download("tiny").await.is_err());
    }

    #[tokio::test]
    async fn cancelled_finalization_holds_active_ownership_before_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap());
        engine.discover_models().await.unwrap();

        let active_download = engine.reserve_active_download("tiny").await.unwrap();
        active_download.cancellation.cancel();

        let hook = Arc::new(DownloadStateTestHook {
            cancellation_first_lock_acquired: tokio::sync::Notify::new(),
            continue_cancellation: tokio::sync::Notify::new(),
            discovery_active_lock_acquired: tokio::sync::Notify::new(),
            continue_discovery: tokio::sync::Notify::new(),
        });
        *engine.download_state_test_hook.lock().unwrap() = Some(Arc::clone(&hook));

        let finalization_engine = Arc::clone(&engine);
        let finalization_owner = Arc::clone(&active_download);
        let model_path = dir.path().join("ggml-tiny.bin");
        let finalization = tokio::spawn(async move {
            finalization_engine
                .finish_download(
                    "tiny",
                    &finalization_owner,
                    &model_path,
                    Err(DownloadCancelled.into()),
                )
                .await
        });

        timeout(
            Duration::from_secs(1),
            hook.cancellation_first_lock_acquired.notified(),
        )
        .await
        .expect("cancellation finalization should reach its first lock");

        let discovery_engine = Arc::clone(&engine);
        let discovery = tokio::spawn(async move { discovery_engine.discover_models().await });

        assert!(
            timeout(
                Duration::from_millis(100),
                hook.discovery_active_lock_acquired.notified(),
            )
            .await
            .is_err(),
            "discovery must wait for cancellation to release active ownership"
        );

        hook.continue_cancellation.notify_one();
        let finalization_result = timeout(Duration::from_secs(1), finalization)
            .await
            .expect("cancelled finalization should complete")
            .unwrap()
            .unwrap_err();
        assert!(is_download_cancelled(&finalization_result));

        timeout(
            Duration::from_secs(1),
            hook.discovery_active_lock_acquired.notified(),
        )
        .await
        .expect("discovery should acquire active ownership after cancellation finalizes");
        hook.continue_discovery.notify_one();

        let models = timeout(Duration::from_secs(1), discovery)
            .await
            .expect("discovery should complete")
            .unwrap()
            .unwrap();
        assert!(matches!(tiny_model(&models).status, ModelStatus::Missing));
        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
    }

    #[tokio::test]
    async fn cancelling_an_unresponsive_download_keeps_ownership_reserved() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let active_download = engine.reserve_active_download("tiny").await.unwrap();

        let outcome = engine
            .cancel_download_with_timeout("tiny", Duration::from_millis(1))
            .await
            .expect("an unresponsive download should report cancellation pending");

        assert_eq!(outcome, CancelDownloadOutcome::Pending);
        assert!(active_download.cancellation.is_cancelled());
        assert!(matches!(
            engine.active_downloads.lock().await.get("tiny"),
            Some(current) if Arc::ptr_eq(current, &active_download)
        ));
        assert!(engine.reserve_active_download("tiny").await.is_err());
    }

    #[tokio::test]
    async fn late_cancellation_preserves_a_completed_model_state() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");

        std::fs::write(&model_path, b"ggml\0\0\0\0").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&model_path)
            .unwrap()
            .set_len(68 * 1024 * 1024)
            .unwrap();

        engine.discover_models().await.unwrap();
        let active_download = engine.reserve_active_download("tiny").await.unwrap();
        engine
            .finish_download("tiny", &active_download, &model_path, Ok(()))
            .await
            .unwrap();

        assert_eq!(
            engine
                .cancel_download_with_timeout("tiny", Duration::from_millis(1))
                .await
                .unwrap(),
            CancelDownloadOutcome::Cancelled
        );
        assert!(matches!(
            tiny_model(&engine.discover_models().await.unwrap()).status,
            ModelStatus::Available
        ));
    }

    async fn stalled_http_server(
    ) -> (
        String,
        oneshot::Receiver<()>,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (headers_sent, headers_ready) = oneshot::channel();
        let (release_body, body_released) = oneshot::channel();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            socket.flush().await.unwrap();
            headers_sent.send(()).unwrap();
            let _ = body_released.await;
            let _ = socket.write_all(b"test").await;
        });

        (format!("http://{address}/model.bin"), headers_ready, release_body, server)
    }

    async fn response_http_server(
        content_length: usize,
        body: &'static [u8],
    ) -> (
        String,
        oneshot::Receiver<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_sent, request_received) = oneshot::channel();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let bytes_read = socket.read(&mut buffer).await.unwrap();
                if bytes_read == 0 {
                    break;
                }

                request.extend_from_slice(&buffer[..bytes_read]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            request_sent.send(request).unwrap();

            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            socket.flush().await.unwrap();
        });

        (format!("http://{address}/model.bin"), request_received, server)
    }

    #[tokio::test]
    async fn downloaded_model_sends_user_agent_and_rejects_undersized_valid_header() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        engine.discover_models().await.unwrap();
        let (url, request, server) = response_http_server(8, b"ggml\0\0\0\0").await;

        let error = engine.download_model_from_url("tiny", &url, None).await.unwrap_err();
        let request = String::from_utf8(request.await.unwrap()).unwrap();
        server.await.unwrap();

        assert!(error.to_string().contains("too small"));
        assert!(request.to_ascii_lowercase().contains(&format!(
            "user-agent: meetily/{}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(!dir.path().join("ggml-tiny.bin").exists());
        assert!(matches!(
            engine
                .available_models
                .read()
                .await
                .get("tiny")
                .expect("tiny should remain cached")
                .status,
            ModelStatus::Missing
        ));
    }


    #[tokio::test]
    async fn cancelling_a_stalled_download_waits_for_cleanup_before_retry_can_start() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap());
        let (url_a, headers_a, release_a, server_a) = stalled_http_server().await;

        let download_a_engine = Arc::clone(&engine);
        let download_a = tokio::spawn(async move {
            download_a_engine
                .download_model_from_url("tiny", &url_a, None)
                .await
        });

        timeout(Duration::from_secs(1), headers_a)
            .await
            .expect("first request should receive response headers")
            .unwrap();

        assert!(engine.active_downloads.lock().await.contains_key("tiny"));
        let status_while_active = engine.discover_models().await.unwrap();
        assert!(matches!(
            tiny_model(&status_while_active).status,
            ModelStatus::Downloading { progress: 0 }
        ));

        timeout(Duration::from_secs(1), engine.cancel_download("tiny"))
            .await
            .expect("cancellation should not wait for a stalled response body")
            .unwrap();

        let download_a_result = timeout(Duration::from_secs(1), download_a)
            .await
            .expect("cancelled worker should finish promptly")
            .unwrap();
        assert!(is_download_cancelled(&download_a_result.unwrap_err()));
        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(!dir.path().join("ggml-tiny.bin").exists());

        let (url_b, headers_b, release_b, server_b) = stalled_http_server().await;
        let download_b_engine = Arc::clone(&engine);
        let download_b = tokio::spawn(async move {
            download_b_engine
                .download_model_from_url("tiny", &url_b, None)
                .await
        });
        timeout(Duration::from_secs(1), headers_b)
            .await
            .expect("retry should reserve the released model slot")
            .unwrap();
        assert!(engine.active_downloads.lock().await.contains_key("tiny"));

        timeout(Duration::from_secs(1), engine.cancel_download("tiny"))
            .await
            .expect("retry cancellation should also complete promptly")
            .unwrap();
        assert!(is_download_cancelled(
            &timeout(Duration::from_secs(1), download_b)
                .await
                .expect("retry worker should finish promptly")
                .unwrap()
                .unwrap_err()
        ));

        let _ = release_a.send(());
        let _ = release_b.send(());
        let _ = server_a.await;
        let _ = server_b.await;
    }

    #[tokio::test]
    async fn incomplete_response_cleans_partial_file_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");
        engine.discover_models().await.unwrap();
        let (url, _request, server) = response_http_server(8, b"ggml").await;

        let error = engine
            .download_model_from_url("tiny", &url, None)
            .await
            .unwrap_err();
        server.await.unwrap();

        assert!(error.to_string().contains("Failed to read chunk"));
        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(!model_path.exists());
        assert!(matches!(
            engine
                .available_models
                .read()
                .await
                .get("tiny")
                .expect("tiny should remain cached")
                .status,
            ModelStatus::Missing
        ));

        let models = engine.discover_models().await.unwrap();
        assert!(matches!(tiny_model(&models).status, ModelStatus::Missing));
    }

    #[tokio::test]
    async fn terminal_download_error_releases_the_active_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(Some(dir.path().to_path_buf())).unwrap();
        let model_path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&model_path, vec![0_u8; 2 * 1024 * 1024]).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        assert!(engine
            .download_model_from_url("tiny", &format!("http://{address}/model.bin"), None)
            .await
            .is_err());
        server.await.unwrap();

        assert!(!engine.active_downloads.lock().await.contains_key("tiny"));
        assert!(!model_path.exists());
        let models = engine.discover_models().await.unwrap();
        assert!(matches!(tiny_model(&models).status, ModelStatus::Missing));
    }
}
