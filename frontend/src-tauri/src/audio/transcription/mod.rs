// audio/transcription/mod.rs
//
// Transcription module: Provider abstraction, engine management, and worker pool.

pub mod provider;
pub mod whisper_provider;
pub mod parakeet_provider;
pub mod engine;
pub mod worker;
pub mod catalog;
pub mod provider_commands;

// Re-export commonly used types
pub use catalog::{
    default_provider_id, ensure_selectable, find_provider, is_cloud, is_implemented,
    select_provider_for_language, validate_language, ProviderKind, ProviderStatus,
    SpeechProviderDescriptor, PROVIDERS,
};
pub use provider::{TranscriptionError, TranscriptionProvider, TranscriptResult};
pub use whisper_provider::WhisperProvider;
pub use parakeet_provider::ParakeetProvider;
pub use engine::{
    TranscriptionEngine,
    configured_provider_id,
    validate_transcription_model_ready,
    get_or_init_transcription_engine,
    get_or_init_transcription_engine_for,
    get_or_init_whisper
};
pub use worker::{
    start_transcription_task,
    reset_speech_detected_flag,
    TranscriptUpdate
};
