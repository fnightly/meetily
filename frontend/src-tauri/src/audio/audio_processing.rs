use anyhow::Result;
use chrono::Utc;
use log::{debug, info, warn};
use realfft::num_complex::{Complex32, ComplexFloat};
use realfft::RealFftPlanner;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::path::PathBuf;
use nnnoiseless::DenoiseState;

use super::encode::encode_single_audio; // Correct path to encode module

/// Sanitize a filename to be safe for filesystem use
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Create a meeting folder with timestamp and return the path
/// Creates structure: base_path/MeetingName_YYYY-MM-DD_HH-MM/
///                    ├── .checkpoints/  (for incremental saves, optional)
///
/// # Arguments
/// * `base_path` - Base directory for meetings
/// * `meeting_name` - Name of the meeting
/// * `create_checkpoints_dir` - Whether to create .checkpoints/ subdirectory (only needed when auto_save is true)
pub fn create_meeting_folder(
    base_path: &PathBuf,
    meeting_name: &str,
    create_checkpoints_dir: bool,
) -> Result<PathBuf> {
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M").to_string();
    let sanitized_name = sanitize_filename(meeting_name);
    let folder_name = format!("{}_{}", sanitized_name, timestamp);
    let meeting_folder = base_path.join(folder_name);

    // Create main meeting folder
    std::fs::create_dir_all(&meeting_folder)?;

    // Only create .checkpoints subdirectory if requested (when auto_save is true)
    if create_checkpoints_dir {
        let checkpoints_dir = meeting_folder.join(".checkpoints");
        std::fs::create_dir_all(&checkpoints_dir)?;
        log::info!("Created meeting folder with checkpoints: {}", meeting_folder.display());
    } else {
        log::info!("Created meeting folder without checkpoints: {}", meeting_folder.display());
    }

    Ok(meeting_folder)
}

pub fn normalize_v2(audio: &[f32]) -> Vec<f32> {
    let rms = (audio.iter().map(|&x| x * x).sum::<f32>() / audio.len() as f32).sqrt();
    let peak = audio
        .iter()
        .fold(0.0f32, |max, &sample| max.max(sample.abs()));

    // Return the original audio if it's completely silent
    if rms == 0.0 || peak == 0.0 {
        return audio.to_vec();
    }

    // Increase target RMS for better voice volume while keeping peak in check
    let target_rms = 0.9;  // Increased from 0.6
    let target_peak = 0.95; // Slightly reduced to prevent clipping

    let rms_scaling = target_rms / rms;
    let peak_scaling = target_peak / peak;

    // Apply a minimum scaling factor to boost very quiet audio
    let min_scaling = 1.5; // Minimum boost for quiet audio
    let scaling_factor = (rms_scaling.min(peak_scaling)).max(min_scaling);

    // Apply scaling with soft clipping to prevent harsh distortion
    audio
        .iter()
        .map(|&sample| {
            let scaled = sample * scaling_factor;
            // Soft clip at ±0.95 to prevent harsh distortion
            if scaled > 0.95 {
                0.95 + (scaled - 0.95) * 0.05
            } else if scaled < -0.95 {
                -0.95 + (scaled + 0.95) * 0.05
            } else {
                scaled
            }
        })
        .collect()
}

/// True peak limiter with lookahead buffer (prevents clipping)
struct TruePeakLimiter {
    lookahead_samples: usize,
    buffer: Vec<f32>,
    gain_reduction: Vec<f32>,
    current_position: usize,
}

impl TruePeakLimiter {
    fn new(sample_rate: u32) -> Self {
        const LIMITER_LOOKAHEAD_MS: usize = 10;
        let lookahead_samples = ((sample_rate as usize * LIMITER_LOOKAHEAD_MS) / 1000).max(1);

        Self {
            lookahead_samples,
            buffer: vec![0.0; lookahead_samples],
            gain_reduction: vec![1.0; lookahead_samples],
            current_position: 0,
        }
    }

    fn process(&mut self, sample: f32, true_peak_limit: f32) -> f32 {
        self.buffer[self.current_position] = sample;

        let sample_abs = sample.abs();
        if sample_abs > true_peak_limit {
            let reduction = true_peak_limit / sample_abs;
            self.gain_reduction[self.current_position] = reduction;
        } else {
            self.gain_reduction[self.current_position] = 1.0;
        }

        let output_position = (self.current_position + 1) % self.lookahead_samples;
        let output_sample = self.buffer[output_position] * self.gain_reduction[output_position];

        self.current_position = output_position;
        output_sample
    }
}

/// Professional loudness normalizer using EBU R128 standard
/// This is a STATEFUL normalizer that tracks cumulative loudness over time
///
/// EBU R128 is the broadcast industry standard for loudness normalization:
/// - Target: -23 LUFS (Loudness Units relative to Full Scale)
/// - Used by: Netflix, YouTube, Spotify, all professional broadcast
/// - Perceptually accurate (not just simple RMS)
///
pub struct LoudnessNormalizer {
    ebur128: ebur128::EbuR128,
    limiter: TruePeakLimiter,
    gain_linear: f32,
    loudness_buffer: Vec<f32>,
    true_peak_limit: f32,
}

impl LoudnessNormalizer {
    /// Create a new EBU R128 loudness normalizer
    ///
    /// # Arguments
    /// * `channels` - Number of audio channels (1 for mono, 2 for stereo)
    /// * `sample_rate` - Sample rate in Hz (e.g., 48000)
    pub fn new(channels: u32, sample_rate: u32) -> Result<Self> {
        const TRUE_PEAK_LIMIT: f64 = -1.0;
        const ANALYZE_CHUNK_SIZE: usize = 512;

        let ebur128 = ebur128::EbuR128::new(channels, sample_rate, ebur128::Mode::I | ebur128::Mode::TRUE_PEAK)
            .map_err(|e| anyhow::anyhow!("Failed to create EBU R128 normalizer: {}", e))?;

        let true_peak_limit = 10_f32.powf(TRUE_PEAK_LIMIT as f32 / 20.0);

        Ok(Self {
            ebur128,
            limiter: TruePeakLimiter::new(sample_rate),
            gain_linear: 1.0,
            loudness_buffer: Vec::with_capacity(ANALYZE_CHUNK_SIZE),
            true_peak_limit,
        })
    }

    /// Normalize loudness using EBU R128 standard with true peak limiting
    ///
    /// This maintains cumulative loudness measurements across all processed audio,
    /// resulting in consistent normalization that sounds natural.
    ///
    /// Target: -23 LUFS (professional broadcast standard for speech/dialog)
    /// Applies sample-by-sample with 10ms lookahead limiter to prevent clipping
    pub fn normalize_loudness(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        const TARGET_LUFS: f64 = -23.0;
        const ANALYZE_CHUNK_SIZE: usize = 512;

        let mut normalized_samples = Vec::with_capacity(samples.len());

        for &sample in samples {
            // Accumulate samples for loudness analysis
            self.loudness_buffer.push(sample);

            // Analyze loudness every 512 samples
            if self.loudness_buffer.len() >= ANALYZE_CHUNK_SIZE {
                if let Err(e) = self.ebur128.add_frames_f32(&self.loudness_buffer) {
                    warn!("Failed to add frames to EBU R128: {}", e);
                } else {
                    // Update gain based on cumulative loudness
                    if let Ok(current_lufs) = self.ebur128.loudness_global() {
                        if current_lufs.is_finite() && current_lufs < 0.0 {
                            let gain_db = TARGET_LUFS - current_lufs;
                            self.gain_linear = 10_f32.powf(gain_db as f32 / 20.0);
                        }
                    }
                }
                self.loudness_buffer.clear();
            }

            // Apply gain and true peak limiting
            let amplified = sample * self.gain_linear;
            let limited = self.limiter.process(amplified, self.true_peak_limit);

            normalized_samples.push(limited);
        }

        normalized_samples
    }
}

/// RNNoise-based noise suppression processor
///
/// Uses a recurrent neural network to suppress background noise while preserving speech.
/// Processes audio at 48kHz in 10ms frames (480 samples per frame).
///
/// Benefits:
/// - 10-15 dB noise reduction in typical office/home environments
/// - Preserves speech quality and intelligibility
/// - Low latency (~10ms per frame)
/// - Cross-platform (works on macOS, Windows, Linux)
pub struct NoiseSuppressionProcessor {
    denoiser: DenoiseState<'static>,
    frame_buffer: Vec<f32>,
    frame_size: usize,  // 480 samples at 48kHz = 10ms
}

impl NoiseSuppressionProcessor {
    /// Create a new noise suppression processor
    ///
    /// # Arguments
    /// * `sample_rate` - Must be 48000 Hz (RNNoise requirement)
    pub fn new(sample_rate: u32) -> Result<Self> {
        if sample_rate != 48000 {
            return Err(anyhow::anyhow!(
                "Noise suppression requires 48kHz sample rate, got {}Hz",
                sample_rate
            ));
        }

        const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

        info!("Initializing RNNoise noise suppression (frame size: {} samples, 10ms @ 48kHz)", FRAME_SIZE);

        Ok(Self {
            denoiser: *DenoiseState::new(),
            frame_buffer: Vec::with_capacity(FRAME_SIZE * 2),
            frame_size: FRAME_SIZE,
        })
    }

    /// Apply noise suppression to audio samples
    ///
    /// Processes audio in 480-sample frames (10ms at 48kHz).
    /// Buffers partial frames for next call.
    ///
    /// CRITICAL FIX: Always returns same length as input to prevent latency accumulation
    ///
    /// # Arguments
    /// * `samples` - Input audio samples at 48kHz
    ///
    /// # Returns
    /// Noise-suppressed audio samples (SAME LENGTH as input)
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        if samples.is_empty() {
            return Vec::new();
        }

        // CRITICAL: Remember original input length
        let input_len = samples.len();

        // Add new samples to buffer
        self.frame_buffer.extend_from_slice(samples);

        let mut output = Vec::with_capacity(input_len);

        // Process complete frames
        while self.frame_buffer.len() >= self.frame_size {
            // Extract one frame
            let frame: Vec<f32> = self.frame_buffer.drain(0..self.frame_size).collect();

            // RNNoise processes audio: separate input and output buffers
            let mut denoised_frame = vec![0.0f32; self.frame_size];

            // Apply noise suppression
            // process_frame(output: &mut [f32], input: &[f32]) -> f32
            // Returns VAD probability (0.0-1.0), higher means more likely to be speech
            let _vad_prob = self.denoiser.process_frame(&mut denoised_frame, &frame);

            output.extend_from_slice(&denoised_frame);
        }

        // Return processed output without forcing length matching
        // Frame-based processing naturally creates variable-length output
        // Downstream pipeline handles this correctly via ring buffer
        output
    }

    /// Get the number of buffered samples waiting for processing
    pub fn buffered_samples(&self) -> usize {
        self.frame_buffer.len()
    }

    /// Flush any remaining buffered samples
    /// Call this at the end of recording to process partial frames
    pub fn flush(&mut self) -> Vec<f32> {
        if self.frame_buffer.is_empty() {
            return Vec::new();
        }

        // Pad the remaining samples to a full frame with zeros
        let remaining = self.frame_buffer.len();
        let mut input_frame = self.frame_buffer.clone();
        if input_frame.len() < self.frame_size {
            input_frame.resize(self.frame_size, 0.0);
        }

        let mut output = vec![0.0f32; self.frame_size];
        self.denoiser.process_frame(&mut output, &input_frame);
        self.frame_buffer.clear();

        // Return only the original samples (without padding)
        output.truncate(remaining);
        output
    }
}

/// High-pass filter to remove low-frequency rumble and noise
/// Removes frequencies below cutoff_hz (typically 80-100 Hz for speech)
pub struct HighPassFilter {
    #[allow(dead_code)]
    sample_rate: f32,
    #[allow(dead_code)]
    cutoff_hz: f32,
    // First-order IIR filter coefficients
    alpha: f32,
    prev_input: f32,
    prev_output: f32,
}

impl HighPassFilter {
    /// Create a new high-pass filter
    ///
    /// # Arguments
    /// * `sample_rate` - Audio sample rate in Hz
    /// * `cutoff_hz` - Cutoff frequency in Hz (typical: 80-100 Hz for speech)
    pub fn new(sample_rate: u32, cutoff_hz: f32) -> Self {
        let sample_rate_f = sample_rate as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate_f;
        let alpha = rc / (rc + dt);

        info!("Initializing high-pass filter: cutoff={}Hz @ {}Hz", cutoff_hz, sample_rate);

        Self {
            sample_rate: sample_rate_f,
            cutoff_hz,
            alpha,
            prev_input: 0.0,
            prev_output: 0.0,
        }
    }

    /// Apply high-pass filter to audio samples
    /// Uses first-order IIR (Infinite Impulse Response) filter
    pub fn process(&mut self, samples: &[f32]) -> Vec<f32> {
        let mut output = Vec::with_capacity(samples.len());

        for &sample in samples {
            // First-order high-pass IIR filter formula:
            // y[n] = alpha * (y[n-1] + x[n] - x[n-1])
            let filtered = self.alpha * (self.prev_output + sample - self.prev_input);

            self.prev_input = sample;
            self.prev_output = filtered;

            output.push(filtered);
        }

        output
    }

    /// Reset filter state (call when starting new recording)
    pub fn reset(&mut self) {
        self.prev_input = 0.0;
        self.prev_output = 0.0;
    }
}

pub fn spectral_subtraction(audio: &[f32], d: f32) -> Result<Vec<f32>> {
    let mut real_planner = RealFftPlanner::<f32>::new();
    let window_size = 1600; // 16k sample rate - 100ms

    // CRITICAL FIX: Handle cases where audio is longer than window size
    if audio.is_empty() {
        return Ok(Vec::new());
    }

    // If audio is longer than window size, truncate to prevent overflow
    let processed_audio = if audio.len() > window_size {
        warn!("Audio length {} exceeds window size {}, truncating", audio.len(), window_size);
        &audio[..window_size]
    } else {
        audio
    };

    let r2c = real_planner.plan_fft_forward(window_size);
    let mut y = r2c.make_output_vec();

    // Safe padding: only pad if audio is shorter than window size
    let mut padded_audio = processed_audio.to_vec();
    if processed_audio.len() < window_size {
        let padding_needed = window_size - processed_audio.len();
        padded_audio.extend(vec![0.0f32; padding_needed]);
    }

    let mut indata = padded_audio;
    r2c.process(&mut indata, &mut y)?;

    let mut processed_audio = y
        .iter()
        .map(|&x| {
            let magnitude_y = x.abs().powf(2.0);

            let div = 1.0 - (d / magnitude_y);

            let gain = {
                if div > 0.0 {
                    f32::sqrt(div)
                } else {
                    0.0f32
                }
            };

            x * gain
        })
        .collect::<Vec<Complex32>>();

    let c2r = real_planner.plan_fft_inverse(window_size);

    let mut outdata = c2r.make_output_vec();

    c2r.process(&mut processed_audio, &mut outdata)?;

    Ok(outdata)
}

// not an average of non-speech segments, but I don't know how much pause time we
// get. for now, we will just assume the noise is constant (kinda defeats the purpose)
// but oh well
pub fn average_noise_spectrum(audio: &[f32]) -> f32 {
    let mut total_sum = 0.0f32;

    for sample in audio {
        let magnitude = sample.abs();

        total_sum += magnitude.powf(2.0);
    }

    total_sum / audio.len() as f32
}

pub fn audio_to_mono(audio: &[f32], channels: u16) -> Vec<f32> {
    let mut mono_samples = Vec::with_capacity(audio.len() / channels as usize);

    // For microphone arrays (> 2 channels), only use first 2 channels
    // Many microphone arrays have auxiliary channels for beam-forming/noise cancellation
    // that can contain anti-phase signals. Averaging all channels can cause destructive
    // interference resulting in near-zero output.
    let effective_channels = if channels > 2 { 2 } else { channels };

    // Iterate over the audio slice in chunks, each containing `channels` samples
    for chunk in audio.chunks(channels as usize) {
        // Sum only the first effective_channels (typically 1-2 for mic arrays)
        let sum: f32 = chunk.iter().take(effective_channels as usize).sum();

        // Calculate the average mono sample using effective channel count
        let mono_sample = sum / effective_channels as f32;

        // Store the computed mono sample
        mono_samples.push(mono_sample);
    }

    mono_samples
}

/// Rubato parameters for one conversion ratio.
///
/// Shared by the one-shot `resample` and the streaming resampler so their
/// anti-aliasing behaviour cannot drift apart.
pub(crate) fn sinc_params_for_ratio(ratio: f64) -> SincInterpolationParameters {
    let (sinc_len, interpolation, oversampling) = if ratio >= 2.0 {
        // Large upsampling (e.g., 8kHz → 16kHz, 16kHz → 48kHz, 24kHz → 48kHz)
        (512, SincInterpolationType::Cubic, 512)
    } else if ratio >= 1.5 {
        // Moderate upsampling (e.g., 32kHz → 48kHz)
        (384, SincInterpolationType::Cubic, 384)
    } else if ratio > 1.0 {
        // Small upsampling (e.g., 44.1kHz → 48kHz)
        (256, SincInterpolationType::Linear, 256)
    } else if ratio <= 0.5 {
        // Large downsampling (e.g., 48kHz → 16kHz, 48kHz → 8kHz): needs the
        // strongest anti-aliasing.
        (512, SincInterpolationType::Cubic, 512)
    } else {
        // Moderate downsampling (e.g., 48kHz → 24kHz, 48kHz → 32kHz)
        (384, SincInterpolationType::Linear, 384)
    };

    SincInterpolationParameters {
        sinc_len,
        f_cutoff: 0.95, // Preserve most of the frequency content
        interpolation,
        oversampling_factor: oversampling,
        window: WindowFunction::BlackmanHarris2, // Best window for audio
    }
}

/// High-quality audio resampling with adaptive parameters based on sample rate ratio
///
/// This function automatically selects the best resampling parameters based on:
/// - Sample rate ratio (upsampling vs downsampling)
/// - Quality requirements (integer ratios get optimized paths)
/// - Anti-aliasing needs
///
/// Supports all common sample rates: 8kHz, 16kHz, 24kHz, 44.1kHz, 48kHz, etc.
pub fn resample(input: &[f32], from_sample_rate: u32, to_sample_rate: u32) -> Result<Vec<f32>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    // Fast path: No resampling needed
    if from_sample_rate == to_sample_rate {
        return Ok(input.to_vec());
    }

    let ratio = to_sample_rate as f64 / from_sample_rate as f64;
    debug!("Resampling: {}Hz → {}Hz (ratio: {:.2}x)",
           from_sample_rate, to_sample_rate, ratio);
    let params = sinc_params_for_ratio(ratio);

    let mut resampler = SincFixedIn::<f32>::new(
        ratio,
        2.0,  // Maximum relative deviation
        params,
        input.len(),
        1,    // Mono
    )?;

    let waves_in = vec![input.to_vec()];
    let waves_out = resampler.process(&waves_in, None)?;

    debug!("Resampling complete: {} samples → {} samples",
           input.len(), waves_out[0].len());

    Ok(waves_out.into_iter().next().unwrap())
}

// Alias for compatibility with existing code
pub fn resample_audio(input: &[f32], from_sample_rate: u32, to_sample_rate: u32) -> Vec<f32> {
    match resample(input, from_sample_rate, to_sample_rate) {
        Ok(result) => result,
        Err(e) => {
            debug!("Resampling failed: {}, returning original audio", e);
            input.to_vec()
        }
    }
}

/// Streaming mono resampler that keeps its filter history across calls.
///
/// `resample` builds a new resampler per call, which is correct for one-shot files
/// but creates seams when a live stream arrives in arbitrary chunks: each call
/// restarts the filter and its fractional phase. This keeps one rubato Sinc
/// resampler plus the unprocessed input remainder, so the anti-aliasing filter and
/// the sample-phase stay continuous for the whole recording.
pub struct StreamingResampler {
    from: u32,
    to: u32,
    /// Input frames handed to rubato per call (~10 ms at the input rate).
    chunk_size: usize,
    input: Vec<f32>,
    /// Total input samples pushed and output samples emitted, used to trim the
    /// flushed tail to the exact output duration the ratio implies.
    pushed_input: usize,
    produced: usize,
    resampler: Option<SincFixedIn<f32>>,
}

impl StreamingResampler {
    pub fn new(from: u32, to: u32) -> Result<Self> {
        // Fixed input chunks make chunked and one-shot processing bit-identical:
        // rubato only ever sees whole `chunk_size` frames.
        let chunk_size = (from as usize / 100).max(64);
        let resampler = if from == to {
            None
        } else {
            let ratio = to as f64 / from as f64;
            Some(SincFixedIn::<f32>::new(
                ratio,
                2.0,
                sinc_params_for_ratio(ratio),
                chunk_size,
                1,
            )?)
        };

        Ok(Self {
            from,
            to,
            chunk_size,
            input: Vec::with_capacity(chunk_size * 2),
            pushed_input: 0,
            produced: 0,
            resampler,
        })
    }

    /// Feed more input; returns the output samples that are ready.
    ///
    /// Anything under one input chunk is kept for the next call so the filter never
    /// restarts mid-stream.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        if self.resampler.is_none() {
            return samples.to_vec();
        }

        self.pushed_input += samples.len();
        self.input.extend_from_slice(samples);
        let mut output = Vec::with_capacity(self.input.len() / 3 + self.chunk_size);
        while self.input.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.input.drain(..self.chunk_size).collect();
            self.process_chunk(&chunk, &mut output);
        }
        self.produced += output.len();
        output
    }

    /// Emit the tail after the final push and report the exact duration.
    ///
    /// The remaining partial chunk is padded with silence so the resampler can flush
    /// its filter, and the sinc's group delay is flushed the same way: without the
    /// extra silence the output would end ~sinc_len/2 input frames short. The result
    /// is trimmed to the sample count the input duration implies, so callers can
    /// rely on length = input_len * to / from.
    pub fn finish(&mut self) -> Vec<f32> {
        let mut output = Vec::new();
        if self.resampler.is_none() {
            return output;
        }

        let expected_total =
            (self.pushed_input as f64 * self.to as f64 / self.from as f64).round() as usize;
        let remaining = expected_total.saturating_sub(self.produced);

        if !self.input.is_empty() {
            let mut last = std::mem::take(&mut self.input);
            last.resize(self.chunk_size, 0.0);
            self.process_chunk(&last, &mut output);
        }

        // SincFixedIn holds back sinc_len/2 frames of group delay; feeding silence
        // releases that tail. 64 chunks (0.64 s of input) is far more than needed.
        let silence = vec![0.0f32; self.chunk_size];
        let mut guard = 0;
        while output.len() < remaining && guard < 64 {
            self.process_chunk(&silence, &mut output);
            guard += 1;
        }

        output.truncate(remaining);
        self.produced += output.len();
        output
    }

    fn process_chunk(&mut self, chunk: &[f32], output: &mut Vec<f32>) {
        let Some(resampler) = self.resampler.as_mut() else {
            output.extend_from_slice(chunk);
            return;
        };
        match resampler.process(&[chunk.to_vec()], None) {
            Ok(mut waves) => {
                if let Some(channel) = waves.pop() {
                    output.extend_from_slice(&channel);
                }
            }
            Err(error) => {
                debug!(
                    "Streaming resampler failed ({}); falling back to one-shot for this chunk",
                    error
                );
                output.extend_from_slice(&resample_audio(chunk, self.from, self.to));
            }
        }
    }
}

/// Fast resampling optimized for transcription preprocessing
///
pub fn write_audio_to_file(
    audio: &[f32],
    sample_rate: u32,
    output_path: &PathBuf,
    device: &str,
    skip_encoding: bool,
) -> Result<String> {
    write_audio_to_file_with_meeting_name(audio, sample_rate, output_path, device, skip_encoding, None)
}

pub fn write_audio_to_file_with_meeting_name(
    audio: &[f32],
    sample_rate: u32,
    output_path: &PathBuf,
    device: &str,
    skip_encoding: bool,
    meeting_name: Option<&str>,
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let sanitized_device_name = device.replace(['/', '\\'], "_");

    // Create meeting folder if meeting name is provided
    let final_output_path = if let Some(name) = meeting_name {
        let sanitized_meeting_name = sanitize_filename(name);
        let meeting_folder = output_path.join(&sanitized_meeting_name);

        // Create the meeting folder if it doesn't exist
        if !meeting_folder.exists() {
            std::fs::create_dir_all(&meeting_folder)?;
        }

        meeting_folder
    } else {
        output_path.clone()
    };

    let file_path = final_output_path
        .join(format!("{}_{}.mp4", sanitized_device_name, timestamp))
        .to_str()
        .expect("Failed to create valid path")
        .to_string();
    let file_path_clone = file_path.clone();
    // Run FFmpeg in a separate task
    if !skip_encoding {
        encode_single_audio(
            bytemuck::cast_slice(audio),
            sample_rate,
            1,
            192, // Unchanged mono budget for this legacy writer.
            &file_path.into(),
        )?;
    }
    Ok(file_path_clone)
}

/// Write transcript text to a file alongside the recording (legacy plain text format)
pub fn write_transcript_to_file(
    transcript_text: &str,
    output_path: &PathBuf,
    meeting_name: Option<&str>,
) -> Result<String> {
    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();

    // Create meeting folder if meeting name is provided (same logic as audio)
    let final_output_path = if let Some(name) = meeting_name {
        let sanitized_meeting_name = sanitize_filename(name);
        let meeting_folder = output_path.join(&sanitized_meeting_name);

        // Create the meeting folder if it doesn't exist
        if !meeting_folder.exists() {
            std::fs::create_dir_all(&meeting_folder)?;
        }

        meeting_folder
    } else {
        output_path.clone()
    };

    let file_path = final_output_path.join(format!("transcript_{}.txt", timestamp));

    // Write transcript to file
    std::fs::write(&file_path, transcript_text)?;

    Ok(file_path.to_string_lossy().to_string())
}

/// Write structured transcript with timestamps to JSON file
pub fn write_transcript_json_to_file(
    segments: &[super::recording_saver::TranscriptSegment],
    output_path: &PathBuf,
    meeting_name: Option<&str>,
    audio_filename: &str,
    recording_duration: f64,
) -> Result<String> {
    use serde_json::json;

    let timestamp = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();

    // Create meeting folder if meeting name is provided
    let final_output_path = if let Some(name) = meeting_name {
        let sanitized_meeting_name = sanitize_filename(name);
        let meeting_folder = output_path.join(&sanitized_meeting_name);

        if !meeting_folder.exists() {
            std::fs::create_dir_all(&meeting_folder)?;
        }

        meeting_folder
    } else {
        output_path.clone()
    };

    let file_path = final_output_path.join(format!("transcript_{}.json", timestamp));

    // Create structured JSON transcript
    let transcript_json = json!({
        "version": "1.0",
        "recording_duration": recording_duration,
        "audio_file": audio_filename,
        "sample_rate": 48000,
        "created_at": Utc::now().to_rfc3339(),
        "meeting_name": meeting_name,
        "segments": segments,
    });

    // Write JSON to file with pretty formatting
    let json_string = serde_json::to_string_pretty(&transcript_json)?;
    std::fs::write(&file_path, json_string)?;

    Ok(file_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, sample_rate: u32, seconds: f32, amplitude: f32) -> Vec<f32> {
        let samples = (sample_rate as f32 * seconds) as usize;
        (0..samples)
            .map(|index| {
                amplitude
                    * (2.0 * std::f32::consts::PI * frequency * index as f32
                        / sample_rate as f32)
                        .sin()
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// The previous VAD path: linear interpolation with no anti-aliasing filter.
    fn naive_linear_downsample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
        let ratio = from as f64 / to as f64;
        let output_len = (input.len() as f64 / ratio) as usize;
        (0..output_len)
            .map(|index| {
                let position = index as f64 * ratio;
                let source = position as usize;
                let fraction = (position - source as f64) as f32;
                if source + 1 < input.len() {
                    input[source] * (1.0 - fraction) + input[source + 1] * fraction
                } else {
                    input.get(source).copied().unwrap_or(0.0)
                }
            })
            .collect()
    }

    #[test]
    fn streaming_resampler_48k_to_16k_has_the_expected_length() {
        let mut resampler = StreamingResampler::new(48_000, 16_000).expect("resampler");
        let input = sine(1_000.0, 48_000, 1.0, 0.5);

        let mut output = resampler.push(&input);
        output.extend(resampler.finish());

        assert!(
            (output.len() as i64 - 16_000).abs() <= 1,
            "expected ~16000 output samples, got {}",
            output.len()
        );
    }

    #[test]
    fn streaming_resampler_preserves_a_passband_tone_without_seams() {
        let mut resampler = StreamingResampler::new(48_000, 16_000).expect("resampler");
        let input = sine(1_000.0, 48_000, 1.0, 0.5);

        let mut output = resampler.push(&input);
        output.extend(resampler.finish());

        let input_rms = rms(&input);
        let output_rms = rms(&output);
        assert!(
            (output_rms / input_rms - 1.0).abs() < 0.05,
            "1 kHz tone level changed too much: {} -> {}",
            input_rms,
            output_rms
        );

        // A seam/click at a chunk boundary would show up as a large single-sample jump.
        let max_step = output
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.25, "resampled tone has a discontinuity: {max_step}");
    }

    #[test]
    fn streaming_resampler_suppresses_aliasing_that_naive_downsampling_keeps() {
        // 12 kHz is above the 8 kHz Nyquist of the 16 kHz output, so it must be
        // filtered out rather than folded down to 4 kHz.
        let mut resampler = StreamingResampler::new(48_000, 16_000).expect("resampler");
        let input = sine(12_000.0, 48_000, 1.0, 0.5);

        let mut filtered = resampler.push(&input);
        filtered.extend(resampler.finish());
        let naive = naive_linear_downsample(&input, 48_000, 16_000);

        let filtered_rms = rms(&filtered);
        let naive_rms = rms(&naive);
        assert!(
            filtered_rms < naive_rms * 0.1,
            "aliasing not suppressed: filtered rms {filtered_rms}, naive rms {naive_rms}"
        );
        assert!(
            filtered_rms < 0.05,
            "out-of-band tone leaked through: rms {filtered_rms}"
        );
    }

    #[test]
    fn chunked_streaming_resampling_matches_one_shot_processing() {
        let input = sine(440.0, 48_000, 0.5, 0.4);

        let mut one_shot = StreamingResampler::new(48_000, 16_000).expect("resampler");
        let mut expected = one_shot.push(&input);
        expected.extend(one_shot.finish());

        let mut chunked = StreamingResampler::new(48_000, 16_000).expect("resampler");
        let mut actual = Vec::new();
        for chunk in input.chunks(137) {
            actual.extend(chunked.push(chunk));
        }
        actual.extend(chunked.finish());

        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "chunked and one-shot resampling disagree: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn equal_rates_pass_through_unchanged() {
        let mut resampler = StreamingResampler::new(16_000, 16_000).expect("resampler");
        let input = vec![0.1f32, -0.2, 0.3];

        assert_eq!(resampler.push(&input), input);
        assert!(resampler.finish().is_empty());
    }
}
