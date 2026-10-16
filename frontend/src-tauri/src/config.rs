/// Application configuration constants
///
/// Centralized definitions for default models and settings.
/// Used across database initialization, import, and retranscription.

/// Upstream host that serves the Whisper GGML models.
///
/// Downloads used to be hardcoded to this host, which left users behind a
/// restricted network with no way to switch to a mirror. Override it with the
/// `MEETILY_MODEL_BASE_URL` environment variable (see [`model_base_url`]).
pub const DEFAULT_MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp";

/// Host serving the Parakeet ONNX model files.
pub const DEFAULT_PARAKEET_MODEL_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx";

/// Environment variable that overrides the Whisper model download host.
pub const MODEL_BASE_URL_ENV: &str = "MEETILY_MODEL_BASE_URL";

/// Environment variable that overrides the Parakeet model download host.
pub const PARAKEET_MODEL_BASE_URL_ENV: &str = "MEETILY_PARAKEET_MODEL_BASE_URL";

/// Normalise a download base URL: trim whitespace and any trailing slash.
///
/// Keeping this pure makes the URL assembly testable without network access.
pub fn normalize_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Base URL to download Whisper models from.
///
/// Precedence: explicit `MEETILY_MODEL_BASE_URL`, then the default host. A
/// mirror only needs to serve the same `resolve/main/ggml-*.bin` layout, so
/// pointing this at `https://hf-mirror.com/ggerganov/whisper.cpp` is enough.
pub fn whisper_model_base_url() -> String {
    std::env::var(MODEL_BASE_URL_ENV)
        .ok()
        .and_then(|v| normalize_base_url(&v))
        .unwrap_or_else(|| DEFAULT_MODEL_BASE_URL.to_string())
}

/// Base URL to download the Parakeet model from — same override rules.
pub fn parakeet_model_base_url() -> String {
    std::env::var(PARAKEET_MODEL_BASE_URL_ENV)
        .ok()
        .and_then(|v| normalize_base_url(&v))
        .unwrap_or_else(|| DEFAULT_PARAKEET_MODEL_BASE_URL.to_string())
}

/// Build the download URL for a Whisper model file.
pub fn whisper_model_url(file_name: &str) -> String {
    format!("{}/resolve/main/{}", whisper_model_base_url(), file_name)
}

/// Default Whisper model for transcription when no preference is configured.
/// This is the recommended balance of accuracy and speed.
pub const DEFAULT_WHISPER_MODEL: &str = "large-v3-turbo";

/// Default Parakeet model for transcription when no preference is configured.
/// This is the quantized version optimized for speed.
pub const DEFAULT_PARAKEET_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

/// Whisper model catalog with metadata for all supported models.
/// Used by both WhisperEngine::discover_models() and discover_models_standalone().
///
/// Format: (name, filename, size_mb, accuracy, speed, description)
pub const WHISPER_MODEL_CATALOG: &[(&str, &str, u32, &str, &str, &str)] = &[
    // Standard f16 models (full precision)
    ("tiny", "ggml-tiny.bin", 74, "Decent", "Very Fast", "Fastest processing, good for real-time use"),
    ("base", "ggml-base.bin", 142, "Good", "Fast", "Good balance of speed and accuracy"),
    ("small", "ggml-small.bin", 466, "Good", "Medium", "Better accuracy, moderate speed"),
    ("medium", "ggml-medium.bin", 1463, "High", "Slow", "High accuracy for professional use"),
    ("large-v3-turbo", "ggml-large-v3-turbo.bin", 1549, "High", "Medium", "Best accuracy with improved speed"),
    ("large-v3", "ggml-large-v3.bin", 2951, "High", "Slow", "Most Accurate, latest large model"),

    // Q5_1 quantized models (balanced speed/accuracy, slightly better quality than Q5_0)
    ("tiny-q5_1", "ggml-tiny-q5_1.bin", 31, "Decent", "Very Fast", "Quantized tiny model, ~50% faster processing"),
    ("base-q5_1", "ggml-base-q5_1.bin", 57, "Good", "Fast", "Quantized base model, good speed/accuracy balance"),
    ("small-q5_1", "ggml-small-q5_1.bin", 181, "Good", "Fast", "Quantized small model, faster than f16 version"),

    // Q5_0 quantized models (balanced speed/accuracy)
    ("medium-q5_0", "ggml-medium-q5_0.bin", 514, "High", "Medium", "Quantized medium model, professional quality"),
    ("large-v3-turbo-q5_0", "ggml-large-v3-turbo-q5_0.bin", 547, "High", "Medium", "Quantized large model, best balance"),
    ("large-v3-q5_0", "ggml-large-v3-q5_0.bin", 1031, "High", "Slow", "Quantized large model, high accuracy"),
];
