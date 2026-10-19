use std::sync::Arc;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use anyhow::Result;
use log::{debug, error, info, warn};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use super::devices::AudioDevice;
use super::recording_state::{AudioChunk, AudioError, RecordingState, DeviceType};
use super::audio_processing::{audio_to_mono, LoudnessNormalizer, NoiseSuppressionProcessor, HighPassFilter};
use super::echo_cancellation::EchoCanceller;
use super::vad::{ContinuousVadProcessor};

/// How long a silence must last before the VAD closes a speech segment, and
/// therefore how long the audio clips handed to the ASR engine are.
///
/// Live-path policy: 500ms (matches the established Meetily Pro live policy).
/// The batch paths (`import.rs` / `retranscription.rs`) use 2000ms instead —
/// they have no latency requirement, so they optimize purely for ASR request
/// length. The live path cannot: with continuous audio (e.g. a podcast played
/// as system audio) a 2000ms redemption keeps one VAD segment open
/// indefinitely, withholds live transcript emission, and overruns the
/// accumulated-speech-buffer warning threshold. Bounded live segmentation
/// during continuous speech is tracked separately in #756.
const VAD_REDEMPTION_TIME_MS: u32 = 500;

/// Ring buffer for synchronized audio mixing
/// Accumulates samples from mic and system streams until we have aligned windows
struct AudioMixerRingBuffer {
    mic_buffer: VecDeque<f32>,
    system_buffer: VecDeque<f32>,
    window_size_samples: usize,  // Fixed mixing window (e.g., 50ms)
    max_buffer_size: usize,  // Safety limit (e.g., 100ms)
}

impl AudioMixerRingBuffer {
    fn new(sample_rate: u32) -> Self {
        // Use 50ms windows for mixing
        let window_ms = 600.0;
        let window_size_samples = (sample_rate as f32 * window_ms / 1000.0) as usize;

        // CRITICAL FIX: Increase max buffer to 400ms for system audio stability
        // System audio (especially Core Audio on macOS) can have significant jitter
        // due to sample-by-sample streaming → batching → channel transmission
        // Accounts for: RNNoise buffering + Core Audio jitter + processing delays
        let max_buffer_size = window_size_samples * 8;  // 400ms (was 200ms)

        info!("🔊 Ring buffer initialized: window={}ms ({} samples), max={}ms ({} samples)",
              window_ms, window_size_samples,
              window_ms * 8.0, max_buffer_size);

        Self {
            mic_buffer: VecDeque::with_capacity(max_buffer_size),
            system_buffer: VecDeque::with_capacity(max_buffer_size),
            window_size_samples,
            max_buffer_size,
        }
    }

    fn add_samples(&mut self, device_type: DeviceType, samples: Vec<f32>) {
        // Log buffer health periodically for diagnostics
        static mut SAMPLE_COUNTER: u64 = 0;
        unsafe {
            SAMPLE_COUNTER += 1;
            if SAMPLE_COUNTER % 200 == 0 {
                debug!("📊 Ring buffer status: mic={} samples, sys={} samples (max={})",
                       self.mic_buffer.len(), self.system_buffer.len(), self.max_buffer_size);
            }
        }

        match device_type {
            DeviceType::Microphone => self.mic_buffer.extend(samples),
            DeviceType::System => self.system_buffer.extend(samples),
        }

        // CRITICAL FIX: Add warnings before dropping samples
        // This helps diagnose timing issues in production
        if self.mic_buffer.len() > self.max_buffer_size {
            warn!("⚠️ Microphone buffer overflow: {} > {} samples, dropping oldest {} samples",
                  self.mic_buffer.len(), self.max_buffer_size,
                  self.mic_buffer.len() - self.max_buffer_size);
        }
        if self.system_buffer.len() > self.max_buffer_size {
            error!("🔴 SYSTEM AUDIO BUFFER OVERFLOW: {} > {} samples, dropping {} samples - THIS CAUSES DISTORTION!",
                  self.system_buffer.len(), self.max_buffer_size,
                  self.system_buffer.len() - self.max_buffer_size);
        }

        // Safety: prevent buffer overflow (keep only last 200ms)
        while self.mic_buffer.len() > self.max_buffer_size {
            self.mic_buffer.pop_front();
        }
        while self.system_buffer.len() > self.max_buffer_size {
            self.system_buffer.pop_front();
        }
    }

    fn can_mix(&self) -> bool {
        self.mic_buffer.len() >= self.window_size_samples ||
        self.system_buffer.len() >= self.window_size_samples
    }

    fn extract_window(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.can_mix() {
            return None;
        }

        // Extract mic window with zero-padding for incomplete buffers
        // Zero-padding (silence) is preferred over last-sample-hold to prevent artifacts

        // Extract mic window (or pad with zeros if insufficient data)
        let mic_window = if self.mic_buffer.len() >= self.window_size_samples {
            // Enough mic data - drain window
            self.mic_buffer.drain(0..self.window_size_samples).collect()
        } else if !self.mic_buffer.is_empty() {
            // Some mic data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.mic_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No mic data - return silence
            vec![0.0; self.window_size_samples]
        };

        // Extract system window (or pad with zeros if insufficient data)
        let sys_window = if self.system_buffer.len() >= self.window_size_samples {
            // Enough system data - drain window
            self.system_buffer.drain(0..self.window_size_samples).collect()
        } else if !self.system_buffer.is_empty() {
            // Some system data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.system_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No system data - return silence
            vec![0.0; self.window_size_samples]
        };

        Some((mic_window, sys_window))
    }

}

/// Simple audio mixer without aggressive ducking
/// Combines mic + system audio with basic clipping prevention
struct ProfessionalAudioMixer;

impl ProfessionalAudioMixer {
    fn new(_sample_rate: u32) -> Self {
        Self
    }

    fn mix_window(&mut self, mic_window: &[f32], sys_window: &[f32]) -> Vec<f32> {
        // Handle different lengths (already padded by extract_window, but defensive)
        let max_len = mic_window.len().max(sys_window.len());
        let mut mixed = Vec::with_capacity(max_len);

        // Both sources arrive at their natural level: the microphone is already
        // normalised to -23 LUFS by the capture path, so the sum is left alone and
        // only overload is shaped.
        for i in 0..max_len {
            let mic = mic_window.get(i).copied().unwrap_or(0.0);
            let sys = sys_window.get(i).copied().unwrap_or(0.0);

            // Per-sample hard normalisation turned every overloaded peak into a
            // full-scale square wave, which is the distortion heard when loud system
            // audio mixes with the microphone. The soft knee rounds the peak instead.
            mixed.push(soft_limit(mic + sys));
        }

        mixed
    }
}

/// Average interleaved L/R pairs into a mono stream.
///
/// The internal recording keeps the untouched stereo chunks; this copy exists only so
/// VAD and transcription still see the single channel they expect.
fn downmix_interleaved_stereo(interleaved: &[f32]) -> Vec<f32> {
    interleaved
        .chunks_exact(2)
        .map(|pair| (pair[0] + pair[1]) * 0.5)
        .collect()
}

/// Duplicate a mono stream into interleaved L/R pairs.
fn mono_to_interleaved_stereo(mono: &[f32]) -> Vec<f32> {
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for &sample in mono {
        stereo.push(sample);
        stereo.push(sample);
    }
    stereo
}

/// Average every frame of an interleaved stream with `channels` channels into mono.
fn downmix_frames_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let channels = channels.max(1);
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Resample interleaved stereo by resampling each channel, returning the input
/// unchanged at 48 kHz.
fn resample_interleaved_stereo(interleaved: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return interleaved.to_vec();
    }
    let left: Vec<f32> = interleaved.iter().copied().step_by(2).collect();
    let right: Vec<f32> = interleaved.iter().copied().skip(1).step_by(2).collect();
    let left = super::audio_processing::resample_audio(&left, from, to);
    let right = super::audio_processing::resample_audio(&right, from, to);

    let frames = left.len().min(right.len());
    let mut stereo = Vec::with_capacity(frames * 2);
    for index in 0..frames {
        stereo.push(left[index]);
        stereo.push(right[index]);
    }
    stereo
}

/// Resampling parameters for one device rate.
///
/// Kept in one place so the mono and stereo persistent resamplers cannot drift apart;
/// this mirrors the choice made by `audio_processing::resample_audio`.
fn sinc_params_for_ratio(ratio: f64) -> SincInterpolationParameters {
    let (sinc_len, interpolation, oversampling) = if ratio >= 2.0 {
        (512, SincInterpolationType::Cubic, 512)
    } else if ratio >= 1.5 {
        (384, SincInterpolationType::Cubic, 384)
    } else if ratio > 1.0 {
        (256, SincInterpolationType::Linear, 256)
    } else if ratio <= 0.5 {
        (512, SincInterpolationType::Cubic, 512)
    } else {
        (384, SincInterpolationType::Linear, 384)
    };
    SincInterpolationParameters {
        sinc_len,
        f_cutoff: 0.95,
        interpolation,
        oversampling_factor: oversampling,
        window: WindowFunction::BlackmanHarris2,
    }
}

/// Simplified audio capture without broadcast channels
#[derive(Clone)]
pub struct AudioCapture {
    device: Arc<AudioDevice>,
    state: Arc<RecordingState>,
    sample_rate: u32,        // Original device sample rate
    channels: u16,
    chunk_counter: Arc<std::sync::atomic::AtomicU64>,
    device_type: DeviceType,
    needs_resampling: bool,  // Flag if resampling is required
    // CRITICAL FIX: Persistent resampler to preserve energy across chunks
    resampler: Arc<std::sync::Mutex<Option<SincFixedIn<f32>>>>,
    // Buffering for variable-size chunks → fixed-size resampler input
    resampler_input_buffer: Arc<std::sync::Mutex<Vec<f32>>>,
    resampler_chunk_size: usize,  // Fixed chunk size for resampler (512 samples)
    // Interleaved stereo system capture for internal recordings: rubato needs one
    // buffer per channel, and the output is interleaved again before sending.
    stereo_recording: bool,
    stereo_resampler: Arc<std::sync::Mutex<Option<SincFixedIn<f32>>>>,
    stereo_resampler_input: Arc<std::sync::Mutex<Vec<Vec<f32>>>>,
    // Audio enhancement processors (microphone only)
    noise_suppressor: Arc<std::sync::Mutex<Option<NoiseSuppressionProcessor>>>,
    high_pass_filter: Arc<std::sync::Mutex<Option<HighPassFilter>>>,
    // EBU R128 normalizer for microphone audio (per-device, stateful)
    normalizer: Arc<std::sync::Mutex<Option<LoudnessNormalizer>>>,
    /// Wall-clock time of the last level emit, so the meter is throttled rather than
    /// emitting on every capture callback.
    last_level_emit_ms: Arc<std::sync::atomic::AtomicU64>,
    // Note: Using global recording timestamp for synchronization
}

impl AudioCapture {
    pub fn new(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        sample_rate: u32,
        channels: u16,
        device_type: DeviceType,
        _recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
    ) -> Self {
        // CRITICAL FIX: Detect if resampling is needed
        // Pipeline expects 48kHz, but Bluetooth devices often report 8kHz, 16kHz, or 44.1kHz
        const TARGET_SAMPLE_RATE: u32 = 48000;
        let needs_resampling = sample_rate != TARGET_SAMPLE_RATE;

        // Only a system capture in a stereo internal recording produces stereo; every
        // mode that involves the microphone keeps the mono path exactly as it was.
        let stereo_recording =
            device_type == DeviceType::System && state.recording_channels() == 2;
        if stereo_recording {
            info!(
                "🎧 System capture '{}' will be recorded as interleaved stereo",
                device.name
            );
        }

        // Detect device kind (Bluetooth vs Wired) for adaptive processing
        // Use reasonable defaults for buffer size (512 samples is typical)
        let device_kind = super::device_detection::InputDeviceKind::detect(&device.name, 512, sample_rate);

        if needs_resampling {
            warn!(
                "⚠️ SAMPLE RATE MISMATCH DETECTED ⚠️"
            );
            warn!(
                "🔄 [{:?}] Audio device '{}' ({:?}) reports {} Hz (pipeline expects {} Hz)",
                device_type, device.name, device_kind, sample_rate, TARGET_SAMPLE_RATE
            );
            warn!(
                "🔄 Automatic resampling will be applied: {} Hz → {} Hz",
                sample_rate, TARGET_SAMPLE_RATE
            );

            // Log which resampling strategy will be used
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            let strategy = if ratio >= 2.0 {
                "High-quality upsampling (sinc_len=512, Cubic interpolation)"
            } else if ratio >= 1.5 {
                "Moderate upsampling (sinc_len=384, Cubic)"
            } else if ratio > 1.0 {
                "Small upsampling (sinc_len=256, Linear)"
            } else if ratio <= 0.5 {
                "Anti-aliased downsampling (sinc_len=512, Cubic)"
            } else {
                "Moderate downsampling (sinc_len=384, Linear)"
            };
            info!("   Resampling strategy: {}", strategy);
        } else {
            info!(
                "✅ [{:?}] Audio device '{}' ({:?}) uses {} Hz (matches pipeline)",
                device_type, device.name, device_kind, sample_rate
            );
        }

        // Initialize audio enhancement processors for MICROPHONE ONLY
        // System audio doesn't need enhancement (already clean)
        let (noise_suppressor, high_pass_filter, normalizer) = if matches!(device_type, DeviceType::Microphone) {
            // Initialize noise suppression (RNNoise) at 48kHz when the session asks for
            // it. Read here because the processor must exist before the first callback.
            let ns = if state.noise_suppression_enabled() {
                match NoiseSuppressionProcessor::new(TARGET_SAMPLE_RATE) {
                    Ok(processor) => {
                        info!("✅ RNNoise noise suppression ENABLED for microphone '{}' (10-15 dB reduction)", device.name);
                        Some(processor)
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to create noise suppressor: {}, continuing without noise suppression", e);
                        None
                    }
                }
            } else {
                info!("ℹ️ RNNoise noise suppression disabled for microphone '{}' (preference off)", device.name);
                None
            };

            // Initialize high-pass filter (removes rumble below 80 Hz)
            let hpf = {
                let filter = HighPassFilter::new(TARGET_SAMPLE_RATE, 80.0);
                info!("✅ High-pass filter initialized for microphone '{}' (cutoff: 80 Hz)", device.name);
                Some(filter)
            };

            // Initialize EBU R128 normalizer (professional loudness standard)
            let norm = match LoudnessNormalizer::new(1, TARGET_SAMPLE_RATE) {
                Ok(normalizer) => {
                    info!("✅ EBU R128 normalizer initialized for microphone '{}' (target: -23 LUFS)", device.name);
                    Some(normalizer)
                }
                Err(e) => {
                    warn!("⚠️ Failed to create normalizer for microphone: {}, normalization disabled", e);
                    None
                }
            };

            (ns, hpf, norm)
        } else {
            // System audio: no enhancement needed
            info!("ℹ️ System audio '{}' captured raw (no enhancement)", device.name);
            (None, None, None)
        };

        // CRITICAL FIX: Initialize persistent resampler to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification and incorrect output sizes
        // Use fixed chunk size of 512 samples with buffering for variable-size input
        const RESAMPLER_CHUNK_SIZE: usize = 512;

        let resampler = if needs_resampling && !stereo_recording {
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            let params = sinc_params_for_ratio(ratio);

            match SincFixedIn::<f32>::new(
                ratio,
                2.0,  // Maximum relative deviation
                params,
                RESAMPLER_CHUNK_SIZE,
                1,    // Mono
            ) {
                Ok(resampler) => {
                    info!("✅ Persistent resampler initialized for '{}' ({}Hz → {}Hz, chunk_size={})",
                          device.name, sample_rate, TARGET_SAMPLE_RATE, RESAMPLER_CHUNK_SIZE);
                    info!("   Buffering enabled for variable-size chunks (e.g., 320, 512, 1024, etc.)");
                    Some(resampler)
                }
                Err(e) => {
                    warn!("⚠️ Failed to create persistent resampler: {}, will use fallback", e);
                    None
                }
            }
        } else {
            None
        };

        // The persistent two-channel resampler is only useful for the stereo system
        // path; it also gets its own per-channel buffers, since rubato wants
        // deinterleaved input.
        let stereo_resampler = if needs_resampling && stereo_recording {
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            let params = sinc_params_for_ratio(ratio);
            match SincFixedIn::<f32>::new(ratio, 2.0, params, RESAMPLER_CHUNK_SIZE, 2) {
                Ok(resampler) => {
                    info!(
                        "✅ Stereo resampler initialized for '{}' ({}Hz → {}Hz, 2 channels)",
                        device.name, sample_rate, TARGET_SAMPLE_RATE
                    );
                    Some(resampler)
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to create stereo resampler: {}, will use per-chunk fallback",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        Self {
            device,
            state,
            sample_rate,
            channels,
            chunk_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            device_type,
            needs_resampling,
            resampler: Arc::new(std::sync::Mutex::new(resampler)),
            resampler_input_buffer: Arc::new(std::sync::Mutex::new(Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2))),
            resampler_chunk_size: RESAMPLER_CHUNK_SIZE,
            stereo_recording,
            stereo_resampler: Arc::new(std::sync::Mutex::new(stereo_resampler)),
            stereo_resampler_input: Arc::new(std::sync::Mutex::new(vec![
                Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2),
                Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2),
            ])),
            noise_suppressor: Arc::new(std::sync::Mutex::new(noise_suppressor)),
            high_pass_filter: Arc::new(std::sync::Mutex::new(high_pass_filter)),
            normalizer: Arc::new(std::sync::Mutex::new(normalizer)),
            last_level_emit_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            // Using global recording time for sync
        }
    }

    /// Measure this device's level and hand it to the app.
    ///
    /// Throttled to [`LEVEL_EMIT_INTERVAL_MS`] because the capture callback runs far
    /// more often than an eye can follow, and every emit crosses into the webview.
    /// The level is taken before resampling and normalisation: it answers "is this
    /// device delivering sound", and post-processing would flatter a quiet input.
    fn report_level(&self, samples: &[f32]) {
        const LEVEL_EMIT_INTERVAL_MS: u64 = 100;

        if samples.is_empty() {
            return;
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);

        let last = self
            .last_level_emit_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if now_ms.saturating_sub(last) < LEVEL_EMIT_INTERVAL_MS {
            return;
        }
        self.last_level_emit_ms
            .store(now_ms, std::sync::atomic::Ordering::Relaxed);

        let mut peak: f32 = 0.0;
        let mut sum_squares: f32 = 0.0;
        for &sample in samples {
            let magnitude = sample.abs();
            if magnitude > peak {
                peak = magnitude;
            }
            sum_squares += sample * sample;
        }
        let rms = (sum_squares / samples.len() as f32).sqrt();

        let device_type = match self.device_type {
            DeviceType::System => "system",
            DeviceType::Microphone => "input",
        };
        self.state.report_level(&self.device.name, device_type, rms, peak);
    }

    /// Process audio data directly from callback
    pub fn process_audio_data(&self, data: &[f32]) {
        // Check if still recording
        if !self.state.is_recording() {
            return;
        }

        // Internal recordings keep the system stream interleaved and stereo end to
        // end; the mixer exists to combine two sources and there is only one here.
        if self.stereo_recording {
            self.process_stereo_system_audio(data);
            return;
        }

        // Convert to mono if needed
        let mut mono_data = if self.channels > 1 {
            audio_to_mono(data, self.channels)
        } else {
            data.to_vec()
        };

        // Report the device's level before resampling and normalisation, so the meter
        // reflects what the device actually delivered.
        self.report_level(&mono_data);

        // CRITICAL FIX: Resample to 48kHz if device uses different sample rate
        // This fixes Bluetooth devices (like Sony WH-1000XM4) that report 16kHz or 44.1kHz
        // Without this, audio is sped up 3x and VAD fails
        //
        // IMPORTANT: Uses PERSISTENT resampler with BUFFERING to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification (173.5% RMS)
        // Buffering handles variable chunk sizes (320, 512, 1024, etc.) by accumulating to fixed 512-sample chunks
        const TARGET_SAMPLE_RATE: u32 = 48000;
        if self.needs_resampling {
            let before_len = mono_data.len();
            let before_rms = if !mono_data.is_empty() {
                (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
            } else {
                0.0
            };

            // Use persistent resampler with buffering to handle variable chunk sizes
            let mut resampled_output = Vec::new();
            let mut used_persistent_resampler = false;

            if let Ok(mut buffer_lock) = self.resampler_input_buffer.lock() {
                let mut resampler_failed = false;

                match self.resampler.lock() {
                    Ok(mut resampler_lock) => {
                        if let Some(ref mut resampler) = *resampler_lock {
                            used_persistent_resampler = true;

                            // Buffer only while a resampler exists. Appending
                            // unconditionally used to strand every chunk of the
                            // recording in this buffer whenever the resampler was
                            // absent (startup failure) or had just failed.
                            buffer_lock.extend_from_slice(&mono_data);

                            // Process as many complete chunks as we have
                            while buffer_lock.len() >= self.resampler_chunk_size {
                                // Extract exactly chunk_size samples
                                let chunk: Vec<f32> = buffer_lock.drain(0..self.resampler_chunk_size).collect();

                                // Rubato expects input as Vec<Vec<f32>> (one Vec per channel)
                                let waves_in = vec![chunk];

                                match resampler.process(&waves_in, None) {
                                    Ok(mut waves_out) => {
                                        if let Some(output) = waves_out.pop() {
                                            resampled_output.extend_from_slice(&output);
                                        }
                                    }
                                    Err(e) => {
                                        warn!("⚠️ Persistent resampler processing failed: {}", e);
                                        used_persistent_resampler = false;
                                        resampler_failed = true;
                                        break;
                                    }
                                }
                            }
                        }

                        // A resampler that failed once fails on every later chunk,
                        // and each attempt would leave its input behind. Drop both
                        // it and the backlog: the per-chunk fallback below then
                        // produces the audio with bounded memory.
                        if resampler_failed {
                            warn!("🔄 Falling back to per-chunk resampling; dropping the buffered resampler");
                            *resampler_lock = None;
                            buffer_lock.clear();
                        }
                    }
                    Err(_) => {
                        // A poisoned lock can never be used again, so buffered
                        // samples would accumulate for the rest of the session.
                        buffer_lock.clear();
                    }
                }
            }

            // CRITICAL: Only update mono_data if we got output from persistent resampler
            // If buffer is accumulating (< 512 samples), skip this chunk - data is safely buffered
            // and will be processed in next iteration with proper resampling
            let has_resampled_output = !resampled_output.is_empty();

            if has_resampled_output {
                mono_data = resampled_output;
            } else if !used_persistent_resampler {
                // Only fallback if persistent resampler is not available at all
                mono_data = super::audio_processing::resample_audio(
                    &mono_data,
                    self.sample_rate,
                    TARGET_SAMPLE_RATE,
                );
            } else {
                // Buffering: samples are accumulating in buffer, waiting for 512-sample chunk
                // Don't send partial/unprocessed data - return early
                // Audio is NOT lost - it's in the buffer and will be processed next iteration
                return;
            }

            // Log resampling only occasionally to avoid spam
            let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
            if chunk_id % 100 == 0 && has_resampled_output {
                let after_len = mono_data.len();
                let after_rms = if !mono_data.is_empty() {
                    (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                } else {
                    0.0
                };
                let ratio = TARGET_SAMPLE_RATE as f64 / self.sample_rate as f64;
                let rms_preservation = if before_rms > 0.0 { (after_rms / before_rms) * 100.0 } else { 100.0 };

                let buffer_size = if let Ok(buf) = self.resampler_input_buffer.lock() {
                    buf.len()
                } else {
                    0
                };

                info!(
                    "🔄 [{:?}] Persistent buffered resampler: {}Hz → {}Hz (ratio: {:.2}x)",
                    self.device_type,
                    self.sample_rate,
                    TARGET_SAMPLE_RATE,
                    ratio
                );
                info!(
                    "   Chunk {}: {} → {} samples, RMS preservation: {:.1}%, buffer: {}",
                    chunk_id,
                    before_len,
                    after_len,
                    rms_preservation,
                    buffer_size
                );
            }
        }

        // AUDIO ENHANCEMENT PIPELINE (Microphone Only)
        // Processing order is critical: high-pass → noise suppression → normalization
        // This ensures noise is removed before being amplified by the normalizer
        if matches!(self.device_type, DeviceType::Microphone) {
            // STEP 1: Apply high-pass filter to remove low-frequency rumble (< 80 Hz)
            if let Ok(mut hpf_lock) = self.high_pass_filter.lock() {
                if let Some(ref mut filter) = *hpf_lock {
                    mono_data = filter.process(&mono_data);
                }
            }

            // STEP 2: Apply RNNoise noise suppression (10-15 dB reduction) when the
            // session has it enabled.
            if self.state.noise_suppression_enabled() {
                if let Ok(mut ns_lock) = self.noise_suppressor.lock() {
                    if let Some(ref mut suppressor) = *ns_lock {
                        let before_len = mono_data.len();
                        mono_data = suppressor.process(&mono_data);
                        let after_len = mono_data.len();

                        // CRITICAL MONITORING: Track buffer health
                        let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                        if chunk_id % 100 == 0 {
                            let buffered = suppressor.buffered_samples();
                            let length_delta = (before_len as i32 - after_len as i32).abs();

                            debug!("🔇 Noise suppression health: in={}, out={}, delta={}, buffered={}, RMS={:.4}",
                                   before_len, after_len, length_delta, buffered,
                                   if !mono_data.is_empty() {
                                       (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                                   } else { 0.0 });

                            // WARN if accumulating samples (potential latency buildup)
                            if buffered > 1000 {
                                warn!("⚠️ RNNoise accumulating samples: {} buffered (potential latency issue!)",
                                      buffered);
                            }

                            // WARN if significant length mismatch
                            if length_delta > 50 {
                                warn!("⚠️ RNNoise length mismatch: input={} output={} (delta={})",
                                      before_len, after_len, length_delta);
                            }
                        }
                    }
                }
            }

            // STEP 3: Apply EBU R128 normalization (professional loudness standard)
            if let Ok(mut normalizer_lock) = self.normalizer.lock() {
                if let Some(ref mut normalizer) = *normalizer_lock {
                    mono_data = normalizer.normalize_loudness(&mono_data);

                    // Log normalization occasionally for debugging
                    let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                    if chunk_id % 200 == 0 && !mono_data.is_empty() {
                        let rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
                        let peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
                        debug!("🎤 After normalization chunk {}: RMS={:.4}, Peak={:.4}", chunk_id, rms, peak);
                    }
                }
            }

            // STEP 4: User gain, applied last so the normalizer cannot cancel it.
            // The soft limiter rounds off peaks instead of clipping them, so a hot
            // setting costs some dynamics rather than adding distortion.
            let gain = self.state.mic_gain_linear();
            if gain != 1.0 {
                for sample in mono_data.iter_mut() {
                    *sample = soft_limit(*sample * gain);
                }
            }
        }

        // Create audio chunk with stream-specific timestamp (get ID first for logging)
        let chunk_id = self.chunk_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // RAW AUDIO: No gain applied here - will be applied AFTER mixing
        // This prevents amplifying system audio bleed-through in the microphone

        // DIAGNOSTIC: Log audio levels for debugging (especially mic issues)
        // if chunk_id % 100 == 0 && !mono_data.is_empty() {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);

        //         info!("🎙️ [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //               self.device_type, chunk_id, raw_rms, raw_peak);

        //     // Warn if microphone is completely silent
        //     if matches!(self.device_type, DeviceType::Microphone) && raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ Microphone producing ZERO audio - check permissions or hardware!");
        //     }
        // }
        // else if chunk_id % 100 == 0 && matches!(self.device_type, DeviceType::System) {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
        //     info!("🔊 [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //       self.device_type, chunk_id, raw_rms, raw_peak);
            
        //     // Warn if system audio is completely silent
        //     if raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ System audio producing ZERO audio - check permissions or hardware!");
        //     }
        // }

        // Use global recording timestamp for proper synchronization
        let timestamp = self.state.get_recording_duration().unwrap_or(0.0);

        // RAW AUDIO CHUNK: No gain applied - will be mixed and gained downstream
        // Use 48kHz if we resampled, otherwise use original rate
        let audio_chunk = AudioChunk {
            data: mono_data,  // Raw audio (resampled if needed), no gain yet
            sample_rate: if self.needs_resampling { 48000 } else { self.sample_rate },
            timestamp,
            chunk_id,
            device_type: self.device_type.clone(),
        };

        // NOTE: Raw audio is NOT sent to recording saver to prevent echo
        // Only the mixed audio (from AudioPipeline) is saved to file (see pipeline.rs:726-736)
        // This ensures we only record once: mic + system properly mixed
        // Individual raw streams go only to the transcription pipeline below

        // Send to processing pipeline for transcription
        if let Err(e) = self.state.send_audio_chunk(audio_chunk) {
            // Check if this is the "pipeline not ready" error
            if e.to_string().contains("Audio pipeline not ready") {
                // This is expected during initialization, just log it as debug
                debug!("Audio pipeline not ready yet, skipping chunk {}", chunk_id);
                return;
            }

            warn!("Failed to send audio chunk: {}", e);
            // More specific error handling based on failure reason
            let error = if e.to_string().contains("channel closed") {
                AudioError::ChannelClosed
            } else if e.to_string().contains("full") {
                AudioError::BufferOverflow
            } else {
                AudioError::ProcessingFailed
            };
            self.state.report_error(error);
        } else {
            debug!("Sent audio chunk {} ({} samples)", chunk_id, data.len());
        }
    }

    /// Build and send one interleaved 48 kHz stereo chunk from the system device.
    ///
    /// Nothing else in the capture path touches this stream: stereo recording exists
    /// to keep the computer audio as it arrived, so no gain, filter or normaliser is
    /// applied here.
    fn process_stereo_system_audio(&self, data: &[f32]) {
        if data.is_empty() {
            return;
        }

        // The meter reads what the device delivered, matching the mono path.
        self.report_level(data);

        let channels = self.channels.max(1) as usize;
        let frames = data.len() / channels;
        let stereo = match channels {
            1 => mono_to_interleaved_stereo(&data[..frames]),
            2 => data[..frames * 2].to_vec(),
            // More than two channels: collapse to mono, then keep the file stereo with
            // identical channels. Mapping surround channels onto L/R would need a
            // downmix matrix this pipeline has no business inventing.
            _ => mono_to_interleaved_stereo(&downmix_frames_to_mono(
                &data[..frames * channels],
                channels,
            )),
        };

        let stereo = if self.needs_resampling {
            match self.resample_stereo(&stereo) {
                Some(resampled) => resampled,
                // The resampler is buffering a partial chunk; the rest arrives next call.
                None => return,
            }
        } else {
            stereo
        };
        if stereo.is_empty() {
            return;
        }

        let chunk_id = self
            .chunk_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let chunk = AudioChunk {
            data: stereo,
            sample_rate: 48_000,
            timestamp: self.state.get_recording_duration().unwrap_or(0.0),
            chunk_id,
            device_type: DeviceType::System,
        };

        match self.state.send_audio_chunk(chunk) {
            Ok(()) => debug!("Sent stereo system chunk {} ({} samples)", chunk_id, data.len()),
            Err(e) => {
                if !e.to_string().contains("Audio pipeline not ready") {
                    warn!("Failed to send stereo system chunk: {}", e);
                }
            }
        }
    }

    /// Resample interleaved stereo to 48 kHz through the persistent two-channel
    /// resampler, buffering whole 512-frame chunks.
    fn resample_stereo(&self, interleaved: &[f32]) -> Option<Vec<f32>> {
        let Ok(mut resampler_guard) = self.stereo_resampler.lock() else {
            return Some(resample_interleaved_stereo(interleaved, self.sample_rate, 48_000));
        };
        let Some(resampler) = resampler_guard.as_mut() else {
            // Building the persistent resampler failed at startup; per-chunk
            // resampling still produces the right samples, just with less state.
            return Some(resample_interleaved_stereo(interleaved, self.sample_rate, 48_000));
        };

        let Ok(mut buffers) = self.stereo_resampler_input.lock() else {
            return None;
        };
        for frame in interleaved.chunks_exact(2) {
            buffers[0].push(frame[0]);
            buffers[1].push(frame[1]);
        }

        let mut output = Vec::with_capacity(interleaved.len());
        let mut resampler_failed = false;

        while buffers[0].len() >= self.resampler_chunk_size {
            let chunk: Vec<Vec<f32>> = buffers
                .iter_mut()
                .map(|channel| channel.drain(0..self.resampler_chunk_size).collect())
                .collect();
            match resampler.process(&chunk, None) {
                Ok(mut waves) if waves.len() == 2 => {
                    let right = waves.pop()?;
                    let left = waves.pop()?;
                    for (left, right) in left.iter().zip(right.iter()) {
                        output.push(*left);
                        output.push(*right);
                    }
                }
                Ok(_) => {
                    resampler_failed = true;
                    break;
                }
                Err(e) => {
                    warn!("⚠️ Stereo resampler failed: {}", e);
                    resampler_failed = true;
                    break;
                }
            }
        }

        // A resampler that failed or produced the wrong shape keeps failing, and
        // each attempt leaves its input in the per-channel buffers. Drop both the
        // resampler and the backlog so the stateless fallback takes over and the
        // buffers cannot grow for the rest of the session.
        if resampler_failed {
            *resampler_guard = None;
            buffers[0].clear();
            buffers[1].clear();
            return None;
        }

        if output.is_empty() {
            None
        } else {
            Some(output)
        }
    }

    /// Handle stream errors with enhanced disconnect detection
    pub fn handle_stream_error(&self, error: cpal::StreamError) {
        error!("Audio stream error for {}: {}", self.device.name, error);

        let error_str = error.to_string().to_lowercase();

        // Enhanced error detection for device disconnection
        let audio_error = if error_str.contains("device is no longer available")
            || error_str.contains("device not found")
            || error_str.contains("device disconnected")
            || error_str.contains("no such device")
            || error_str.contains("device unavailable")
            || error_str.contains("device removed")
        {
            warn!("🔌 Device disconnect detected for: {}", self.device.name);
            AudioError::DeviceDisconnected
        } else if error_str.contains("permission") || error_str.contains("access denied") {
            AudioError::PermissionDenied
        } else if error_str.contains("channel closed") {
            AudioError::ChannelClosed
        } else if error_str.contains("stream") && error_str.contains("failed") {
            AudioError::StreamFailed
        } else {
            warn!("Unknown audio error: {}", error);
            AudioError::StreamFailed
        };

        self.state.report_error(audio_error);
    }
}

/// VAD-driven audio processing pipeline
/// Uses Voice Activity Detection to segment speech in real-time and send only speech to Whisper
pub struct AudioPipeline {
    receiver: mpsc::UnboundedReceiver<AudioChunk>,
    transcription_sender: mpsc::UnboundedSender<AudioChunk>,
    vad_processor: ContinuousVadProcessor,
    sample_rate: u32,
    chunk_id_counter: u64,
    // Performance optimization: reduce logging frequency
    last_summary_time: std::time::Instant,
    processed_chunks: u64,
    // Smart batching for audio metrics
    // PROFESSIONAL AUDIO MIXING: Ring buffer + RMS-based mixer
    ring_buffer: AudioMixerRingBuffer,
    mixer: ProfessionalAudioMixer,
    /// Present only when the session asked for echo cancellation and has a render
    /// stream to cancel; `None` keeps the microphone path exactly as it was.
    echo_canceller: Arc<std::sync::Mutex<Option<EchoCanceller>>>,
    // Recording sender for pre-mixed audio
    recording_sender_for_mixed: Option<mpsc::UnboundedSender<AudioChunk>>,
    /// Channels the recorder writes for this session, read once at construction so the
    /// hot path never consults session state.
    recording_channels: u16,
}

impl AudioPipeline {
    pub fn new(
        receiver: mpsc::UnboundedReceiver<AudioChunk>,
        transcription_sender: mpsc::UnboundedSender<AudioChunk>,
        state: Arc<RecordingState>,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
    ) -> Result<Self> {
        // Log device characteristics for adaptive buffering
        info!("🎛️ AudioPipeline initializing with device characteristics:");
        info!("   Mic: '{}' ({:?}) - Buffer: {:?}",
              mic_device_name, mic_device_kind, mic_device_kind.buffer_timeout());
        info!("   System: '{}' ({:?}) - Buffer: {:?}",
              system_device_name, system_device_kind, system_device_kind.buffer_timeout());

        // Device kind information can be used for adaptive buffering in the future
        // For now, we log it for monitoring and potential optimization
        let _ = (mic_device_name, mic_device_kind, system_device_name, system_device_kind);

        // Create VAD processor. The VAD processor handles 48kHz->16kHz resampling
        // internally.
        //
        // Redemption time is how long a silence must last before the VAD closes a
        // speech segment, so it decides how long the audio clips handed to the ASR
        // engine are. Conversational speech pauses constantly for breath and
        // mid-sentence thought, and every pause longer than this becomes a segment
        // boundary and therefore a separate transcription request.
        //
        // This was 400ms, which fragmented a 26-minute meeting into 322 requests with
        // a median length of 3.5s. Whisper is a fixed 30-second-window model: below
        // that it zero-pads the window and leans on its language-model prior, which
        // was trained on web subtitles, so short clips come back as memorised
        // boilerplate ("subscribe to the channel", "thank you") instead of speech.
        // Measured on a real recording, 47% of segment boundaries sat in the
        // 0.42-0.75s range that a longer redemption bridges.
        //
        // 500ms is the live-path policy (see the constant's doc comment). The
        // batch value (2000ms, `import.rs`/`retranscription.rs`) was tried here
        // first, but under continuous system audio it kept a VAD segment open
        // indefinitely and withheld live transcript emission, so live and batch
        // deliberately diverge. Bounded live segments under continuous speech
        // are tracked in #756.
        let vad_processor =
            ContinuousVadProcessor::new(sample_rate, VAD_REDEMPTION_TIME_MS)?;
        info!(
            "VAD-driven pipeline: segments dispatched per speech burst (redemption_time={}ms)",
            VAD_REDEMPTION_TIME_MS
        );

        // Initialize professional audio mixing components
        let ring_buffer = AudioMixerRingBuffer::new(sample_rate);
        let mixer = ProfessionalAudioMixer::new(sample_rate);

        // AEC needs a far-end stream to subtract from the microphone. The start path
        // clears the session flag in microphone-only mode; this is the backstop for
        // any future caller that sets the preference without a system stream.
        let echo_canceller = if state.echo_cancellation_enabled() {
            let canceller = EchoCanceller::new(sample_rate);
            if canceller.is_none() {
                warn!("Echo cancellation could not be initialised; recording continues without it");
            }
            canceller
        } else {
            None
        };
        let echo_canceller = Arc::new(std::sync::Mutex::new(echo_canceller));

        // Note: target_chunk_duration_ms is ignored - VAD controls segmentation now
        let _ = target_chunk_duration_ms;

        let recording_channels = state.recording_channels();
        if recording_channels == 2 {
            info!("🎧 Pipeline using the stereo internal-recording path");
        }

        Ok(Self {
            receiver,
            transcription_sender,
            vad_processor,
            sample_rate,
            chunk_id_counter: 0,
            // Performance optimization: reduce logging frequency
            last_summary_time: std::time::Instant::now(),
            processed_chunks: 0,
            ring_buffer,
            mixer,
            echo_canceller,
            recording_sender_for_mixed: None,  // Will be set by manager
            recording_channels,
        })
    }

    /// Pass the microphone window through AEC when the session enabled it.
    ///
    /// The lock is uncontended (the pipeline thread is the only user); it exists
    /// because the canceller has to be installed before the task starts and shared
    /// with the struct that runs it.
    fn cancel_echo(&self, mic_window: Vec<f32>, sys_window: &[f32]) -> Vec<f32> {
        let Ok(mut guard) = self.echo_canceller.lock() else {
            return mic_window;
        };
        match guard.as_mut() {
            Some(canceller) => canceller.process_window(&mic_window, sys_window),
            None => mic_window,
        }
    }

    /// Run the VAD-driven audio processing pipeline
    pub async fn run(mut self) -> Result<()> {
        info!("VAD-driven audio pipeline started - segments sent in real-time based on speech detection");

        // CRITICAL FIX: Continue processing until channel is closed, not based on recording state
        // This ensures ALL chunks are processed during shutdown, fixing premature meeting completion
        // Previous bug: Loop checked `while self.state.is_recording()` which caused early exit when
        // stop_recording() was called, losing flush signals and remaining chunks in the pipeline
        loop {
            // Receive audio chunks with timeout
            match tokio::time::timeout(
                std::time::Duration::from_millis(50), // Shorter timeout for responsiveness
                self.receiver.recv()
            ).await {
                Ok(Some(chunk)) => {
                    // PERFORMANCE: Check for flush signal (special chunk with ID >= u64::MAX - 10)
                    // Multiple flush signals may be sent to ensure processing
                    if chunk.chunk_id >= u64::MAX - 10 {
                        info!("📥 Received FLUSH signal #{} - flushing VAD processor", u64::MAX - chunk.chunk_id);
                        self.flush_remaining_audio()?;
                        // Continue processing to handle any remaining chunks
                        continue;
                    }

                    // PERFORMANCE OPTIMIZATION: Eliminate per-chunk logging overhead
                    // Logging in hot paths causes severe performance degradation
                    self.processed_chunks += 1;

                    // CRITICAL: Log summary only every 200 chunks OR every 60 seconds (99.5% reduction)
                    // This eliminates I/O overhead in the audio processing hot path
                    // Use performance-optimized debug macro that compiles to nothing in release builds
                    if self.processed_chunks % 200 == 0 || self.last_summary_time.elapsed().as_secs() >= 60 {
                        perf_debug!("Pipeline processed {} chunks, current chunk: {} ({} samples)",
                                   self.processed_chunks, chunk.chunk_id, chunk.data.len());
                        self.last_summary_time = std::time::Instant::now();
                    }

                    // STEP 1: A stereo internal recording bypasses the mixer: the
                    // untouched interleaved chunks go straight to the recorder, and a
                    // mono downmix feeds the ring buffer so VAD/transcription keep
                    // working as they do for every other session.
                    let stereo_system =
                        self.recording_channels == 2 && chunk.device_type == DeviceType::System;
                    let chunk_data = if stereo_system {
                        if let Some(ref sender) = self.recording_sender_for_mixed {
                            let stereo_chunk = AudioChunk {
                                data: chunk.data.clone(),
                                sample_rate: 48_000,
                                timestamp: chunk.timestamp,
                                chunk_id: chunk.chunk_id,
                                device_type: DeviceType::System,
                            };
                            let _ = sender.send(stereo_chunk);
                        }
                        downmix_interleaved_stereo(&chunk.data)
                    } else {
                        chunk.data
                    };
                    self.ring_buffer.add_samples(chunk.device_type.clone(), chunk_data);

                    // STEP 2: Mix audio in fixed windows when both streams have sufficient data
                    while self.ring_buffer.can_mix() {
                        if let Some((mic_window, sys_window)) = self.ring_buffer.extract_window() {
                            // Cancel speaker echo in the microphone before mixing:
                            // otherwise the recording contains both the system audio
                            // and its delayed, coloured copy from the room.
                            let mic_window = self.cancel_echo(mic_window, &sys_window);

                            // Simple mixing without aggressive ducking
                            let mixed_clean = self.mixer.mix_window(&mic_window, &sys_window);

                            // NO POST-GAIN NEEDED: Microphone already normalized by EBU R128 to -23 LUFS
                            // This is broadcast-standard loudness (Netflix/YouTube/Spotify level)
                            // System audio at natural levels
                            // Previous 2x gain was causing excessive limiting/distortion
                            let mixed_with_gain = mixed_clean;

                            // STEP 3: Send mixed audio for transcription (VAD + Whisper)
                            match self.vad_processor.process_audio(&mixed_with_gain) {
                                Ok(speech_segments) => {
                                    for segment in speech_segments {
                                        let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                                        if segment.samples.len() >= 800 {  // Minimum 50ms at 16kHz - matches Parakeet capability
                                            info!("📤 Sending VAD segment: {:.1}ms, {} samples",
                                                  duration_ms, segment.samples.len());

                                            let transcription_chunk = AudioChunk {
                                                data: segment.samples,
                                                sample_rate: 16000,
                                                timestamp: segment.start_timestamp_ms / 1000.0,
                                                chunk_id: self.chunk_id_counter,
                                                device_type: DeviceType::Microphone,  // Mixed audio
                                            };

                                            if let Err(e) = self.transcription_sender.send(transcription_chunk) {
                                                warn!("Failed to send VAD segment: {}", e);
                                            } else {
                                                self.chunk_id_counter += 1;
                                            }
                                        } else {
                                            debug!("⏭️ Dropping short VAD segment: {:.1}ms ({} samples < 800)",
                                                   duration_ms, segment.samples.len());
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ VAD error: {}", e);
                                }
                            }

                            // STEP 4: Send mixed audio for recording (WAV file). The
                            // stereo internal recording has no mixer output to save: its
                            // interleaved source was forwarded before the downmix.
                            if !stereo_system {
                                if let Some(ref sender) = self.recording_sender_for_mixed {
                                    let recording_chunk = AudioChunk {
                                        data: mixed_with_gain.clone(),
                                        sample_rate: self.sample_rate,
                                        timestamp: chunk.timestamp,
                                        chunk_id: self.chunk_id_counter,
                                        device_type: DeviceType::Microphone,  // Mixed audio
                                    };
                                    let _ = sender.send(recording_chunk);
                                }
                            }
                        }
                    }
                }
                Ok(None) => {
                    info!("Audio pipeline: sender closed after processing {} chunks", self.processed_chunks);
                    break;
                }
                Err(_) => {
                    // Timeout - just continue, VAD handles all segmentation
                    continue;
                }
            }
        }

        // Flush any remaining VAD segments
        self.flush_remaining_audio()?;

        info!("VAD-driven audio pipeline ended");
        Ok(())
    }

    fn flush_remaining_audio(&mut self) -> Result<()> {
        info!("Flushing remaining audio from pipeline (processed {} chunks)", self.processed_chunks);

        // Flush any remaining audio from VAD processor and send segments to transcription
        match self.vad_processor.flush() {
            Ok(final_segments) => {
                for segment in final_segments {
                    let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                    // Send segments >= 50ms (800 samples at 16kHz) - matches main pipeline filter
                    if segment.samples.len() >= 800 {
                        info!("📤 Sending final VAD segment to Whisper: {:.1}ms duration, {} samples",
                              duration_ms, segment.samples.len());

                        let transcription_chunk = AudioChunk {
                            data: segment.samples,
                            sample_rate: 16000,
                            timestamp: segment.start_timestamp_ms / 1000.0,
                            chunk_id: self.chunk_id_counter,
                            device_type: DeviceType::Microphone,
                        };

                        if let Err(e) = self.transcription_sender.send(transcription_chunk) {
                            warn!("Failed to send final VAD segment: {}", e);
                        } else {
                            self.chunk_id_counter += 1;
                        }
                    } else {
                        info!("⏭️ Skipping short final segment: {:.1}ms ({} samples < 800)",
                              duration_ms, segment.samples.len());
                    }
                }
            }
            Err(e) => {
                warn!("Failed to flush VAD processor: {}", e);
            }
        }

        Ok(())
    }

}

/// Soft knee limiter for the user-gain stage.
///
/// Below 0.9 the sample is untouched. Above it a tanh knee bends the remaining
/// headroom towards an asymptote just under full scale, so a hot gain setting
/// rounds peaks off instead of squaring them at a hard clamp. The asymptote is
/// 0.999 rather than 1.0 because f32 rounding would otherwise let a large enough
/// input land exactly on full scale, which is what the hard clamp did.
fn soft_limit(sample: f32) -> f32 {
    const KNEE: f32 = 0.9;
    const ASYMPTOTE: f32 = 0.999;
    if sample.abs() <= KNEE {
        return sample;
    }
    let sign = if sample.is_sign_negative() { -1.0 } else { 1.0 };
    let headroom = ASYMPTOTE - KNEE;
    let excess = (sample.abs() - KNEE) / headroom;
    sign * (KNEE + headroom * excess.tanh())
}

/// Simple audio pipeline manager
pub struct AudioPipelineManager {
    pipeline_handle: Option<JoinHandle<Result<()>>>,
    audio_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
}

impl AudioPipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_handle: None,
            audio_sender: None,
        }
    }

    /// Start the audio pipeline with device information for adaptive buffering
    pub fn start(
        &mut self,
        state: Arc<RecordingState>,
        transcription_sender: mpsc::UnboundedSender<AudioChunk>,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
    ) -> Result<()> {
        // Log device information for adaptive buffering
        info!("🎙️ Starting pipeline with device info:");
        info!("   Microphone: '{}' ({:?})", mic_device_name, mic_device_kind);
        info!("   System Audio: '{}' ({:?})", system_device_name, system_device_kind);

        // Create audio processing channel
        let (audio_sender, audio_receiver) = mpsc::unbounded_channel::<AudioChunk>();


        // Create and start pipeline with device information for adaptive mixing
        let mut pipeline = AudioPipeline::new(
            audio_receiver,
            transcription_sender,
            state.clone(),
            target_chunk_duration_ms,
            sample_rate,
            mic_device_name,
            mic_device_kind,
            system_device_name,
            system_device_kind,
        )?;
        state.set_audio_sender(audio_sender.clone());

        // CRITICAL FIX: Connect recording sender to receive pre-mixed audio
        // This ensures both mic AND system audio are captured in recordings
        pipeline.recording_sender_for_mixed = recording_sender;

        let handle = tokio::spawn(async move {
            pipeline.run().await
        });

        self.pipeline_handle = Some(handle);
        self.audio_sender = Some(audio_sender);

        info!("Audio pipeline manager started with mixed audio recording");
        Ok(())
    }

    /// Stop the audio pipeline
    pub async fn stop(&mut self) -> Result<()> {
        // Drop the sender to close the pipeline
        self.audio_sender = None;

        // Wait for pipeline to finish
        if let Some(handle) = self.pipeline_handle.take() {
            match handle.await {
                Ok(result) => result,
                Err(e) => {
                    error!("Pipeline task failed: {}", e);
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }

    /// Force immediate flush of accumulated audio and stop pipeline
    /// PERFORMANCE CRITICAL: Eliminates 30+ second shutdown delays
    pub async fn force_flush_and_stop(&mut self) -> Result<()> {
        info!("🚀 Force flushing pipeline - processing ALL accumulated audio immediately");

        // If we have a sender, send a special flush signal first
        if let Some(sender) = &self.audio_sender {
            // Create a special flush chunk to trigger immediate processing
            let flush_chunk = AudioChunk {
                data: vec![], // Empty data signals flush
                sample_rate: 16000,
                timestamp: 0.0,
                chunk_id: u64::MAX, // Special ID to indicate flush
                device_type: super::recording_state::DeviceType::Microphone,
            };

            if let Err(e) = sender.send(flush_chunk) {
                warn!("Failed to send flush signal: {}", e);
            } else {
                info!("📤 Sent flush signal to pipeline");

                // PERFORMANCE OPTIMIZATION: Reduced wait time from 50ms to 20ms
                // Pipeline should process flush signal very quickly
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

                // Send multiple flush signals to ensure the pipeline catches it
                // This aggressive approach eliminates shutdown delay issues
                for i in 0..3 {
                    let additional_flush = AudioChunk {
                        data: vec![],
                        sample_rate: 16000,
                        timestamp: 0.0,
                        chunk_id: u64::MAX - (i as u64),
                        device_type: super::recording_state::DeviceType::Microphone,
                    };
                    let _ = sender.send(additional_flush);
                }

                info!("📤 Sent additional flush signals for reliability");
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }

        // Now stop normally
        self.stop().await
    }
}

impl Default for AudioPipelineManager {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_vad_redemption_matches_pro_policy() {
        // Live uses the established 500ms pause policy; it does not bound
        // uninterrupted speech. Batch import/retranscription use 2000ms.
        // See #679 and #756.
        assert_eq!(VAD_REDEMPTION_TIME_MS, 500);
    }

    #[test]
    fn soft_limit_leaves_quiet_samples_untouched() {
        for sample in [-0.9, -0.5, -0.1, 0.0, 0.3, 0.9] {
            assert_eq!(soft_limit(sample), sample);
        }
    }

    #[test]
    fn soft_limit_bends_loud_samples_below_full_scale() {
        for sample in [0.901, 0.95, 1.0, 2.0, 100.0] {
            let limited = soft_limit(sample);
            assert!(limited > 0.9, "{sample} lost its signal: {limited}");
            assert!(limited < 1.0, "{sample} reached full scale: {limited}");
        }
        for sample in [-0.901, -0.95, -1.0, -2.0, -100.0] {
            let limited = soft_limit(sample);
            assert!(limited < -0.9, "{sample} lost its signal: {limited}");
            assert!(limited > -1.0, "{sample} reached full scale: {limited}");
        }
    }

    #[test]
    fn mixer_keeps_a_quiet_sum_unchanged() {
        let mut mixer = ProfessionalAudioMixer::new(48_000);
        let mic = [0.2f32, -0.3, 0.05, 0.0];
        let sys = [0.1f32, 0.1, -0.15, 0.4];

        let mixed = mixer.mix_window(&mic, &sys);

        // Below the knee the output is exactly the f32 sum, not a rounded one.
        assert_eq!(mixed.len(), mic.len());
        for ((out, mic), sys) in mixed.iter().zip(mic.iter()).zip(sys.iter()) {
            assert!((mic + sys).abs() <= 0.9, "test input crossed the knee");
            assert_eq!(*out, mic + sys, "quiet sample was altered");
        }
    }

    #[test]
    fn mixer_soft_limits_instead_of_hard_clipping() {
        // Sustained overload: the old per-sample normalisation emitted exactly ±1.0
        // for every one of these samples, i.e. a square wave.
        let mut mixer = ProfessionalAudioMixer::new(48_000);
        let mixed = mixer.mix_window(&[1.0; 64], &[1.0; 64]);

        assert_eq!(mixed.len(), 64);
        for sample in &mixed {
            assert!(sample.is_finite(), "non-finite mix output: {sample}");
            assert!(*sample < 1.0, "output hit the hard ceiling: {sample}");
            assert!(
                *sample > 0.99,
                "the limiter removed the whole signal: {sample}"
            );
        }
    }

    #[test]
    fn mixer_keeps_loud_samples_distinguishable() {
        // A hard normalisation maps every overloaded sample to the same ±1.0; the
        // soft knee has to leave the waveform's shape intact instead.
        let mut mixer = ProfessionalAudioMixer::new(48_000);
        let mic: Vec<f32> = (0..=120).map(|step| step as f32 * 0.01).collect();
        let sys = vec![0.2f32; mic.len()];

        let mixed = mixer.mix_window(&mic, &sys);
        let loud: Vec<f32> = mixed.iter().copied().filter(|s| *s > 0.95).collect();

        assert!(loud.len() > 10, "test input did not reach the knee");
        for pair in loud.windows(2) {
            assert!(
                pair[1] > pair[0],
                "flat-topped limiter output: {} -> {}",
                pair[0],
                pair[1]
            );
        }
        assert!(mixed.iter().all(|s| s.is_finite() && s.abs() < 1.0));
    }

    #[test]
    fn ring_buffer_backlog_is_bounded_and_windows_drain_it() {
        // The 600 ms mixing window keeps at most eight windows of audio; a long
        // recording must never grow the backlog past that.
        let mut ring = AudioMixerRingBuffer::new(48_000);
        let max = ring.max_buffer_size;
        assert_eq!(ring.window_size_samples, 48_000 * 600 / 1000);

        for _ in 0..200 {
            ring.add_samples(DeviceType::Microphone, vec![0.1; 480]);
            ring.add_samples(DeviceType::System, vec![0.1; 480]);
        }

        assert!(
            ring.mic_buffer.len() <= max,
            "microphone backlog grew to {}",
            ring.mic_buffer.len()
        );
        assert!(
            ring.system_buffer.len() <= max,
            "system backlog grew to {}",
            ring.system_buffer.len()
        );

        // Extracting a window consumes exactly one window from each full buffer.
        let (mic, sys) = ring.extract_window().expect("buffers are mixable");
        assert_eq!(mic.len(), ring.window_size_samples);
        assert_eq!(sys.len(), ring.window_size_samples);
        assert!(ring.mic_buffer.len() <= max);
    }

    #[test]
    fn downmix_averages_interleaved_stereo_pairs() {
        let stereo = [0.4f32, 1.0, -0.2, 0.0, 0.6, 0.2];
        let mono = downmix_interleaved_stereo(&stereo);
        let expected = [0.7f32, -0.1, 0.4];

        assert_eq!(mono.len(), expected.len());
        for (mono, expected) in mono.iter().zip(expected.iter()) {
            assert!((mono - expected).abs() < 1e-6, "{mono} != {expected}");
        }
    }

    #[test]
    fn mono_is_duplicated_into_interleaved_stereo() {
        let mono = [0.1f32, -0.2, 0.3];
        assert_eq!(
            mono_to_interleaved_stereo(&mono),
            vec![0.1, 0.1, -0.2, -0.2, 0.3, 0.3]
        );
    }

    #[test]
    fn wide_devices_downmix_to_mono_before_duplication() {
        // 4-channel device: each frame becomes the average of its channels.
        let interleaved = [1.0f32, 0.0, 0.0, 0.0, 0.2, 0.2, 0.2, 0.2];
        let mono = downmix_frames_to_mono(&interleaved, 4);

        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.25).abs() < 1e-6, "frame 0: {}", mono[0]);
        assert!((mono[1] - 0.2).abs() < 1e-6, "frame 1: {}", mono[1]);
    }

    #[test]
    fn stereo_resampling_is_a_passthrough_at_48k() {
        let stereo = [0.1f32, -0.1, 0.2, -0.2];
        assert_eq!(resample_interleaved_stereo(&stereo, 48_000, 48_000), stereo);
    }

    #[test]
    fn stereo_resampling_preserves_channel_order() {
        // The fallback resamples each channel independently; a constant left channel
        // and a silent right channel must keep that layout.
        let frames = 2_000;
        let mut stereo = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            stereo.push(0.5);
            stereo.push(0.0);
        }

        let resampled = resample_interleaved_stereo(&stereo, 44_100, 48_000);

        assert!(!resampled.is_empty());
        assert_eq!(resampled.len() % 2, 0, "output must stay interleaved");
        for pair in resampled.chunks_exact(2) {
            assert!(pair[0] > 0.4, "left channel was lost: {}", pair[0]);
            assert!(
                pair[1].abs() < 0.01,
                "left channel leaked into the right: {}",
                pair[1]
            );
        }
    }

    #[test]
    fn soft_limit_is_monotonic_and_finite() {
        let mut previous = f32::NEG_INFINITY;
        let mut sample = -4.0f32;
        while sample <= 4.0 {
            let limited = soft_limit(sample);
            assert!(limited.is_finite(), "non-finite output for {sample}");
            assert!(
                limited >= previous,
                "soft limiter went backwards at {sample}: {limited} < {previous}"
            );
            previous = limited;
            sample += 0.01;
        }
    }

    /// Record two seconds from the system default microphone and assert sound arrived.
    ///
    /// Ignored by default because it needs real hardware. Run
    /// "cargo test --lib real_microphone_probe -- --ignored --nocapture" to check the
    /// microphone capture path without launching the app.
    #[test]
    #[ignore]
    fn real_microphone_probe_receives_sound() {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::SampleFormat;
        use std::sync::{Arc, Mutex};

        let host = cpal::default_host();
        let device = host.default_input_device().expect(
            "no default input device — connect a microphone and check the Windows sound settings",
        );
        let supported = device
            .default_input_config()
            .expect("the default input device exposes no input configuration");
        println!(
            "probing '{}' ({:?}, {} Hz, {} ch)",
            device.name().unwrap_or_else(|_| "<unnamed>".to_string()),
            supported.sample_format(),
            supported.sample_rate().0,
            supported.channels()
        );

        let collected: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let stream_config = supported.config();
        let error_fn = |err| eprintln!("microphone probe stream error: {err}");

        let stream = match supported.sample_format() {
            SampleFormat::F32 => {
                let sink = collected.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend_from_slice(data);
                        }
                    },
                    error_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let sink = collected.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend(data.iter().map(|&s| s as f32 / 32768.0));
                        }
                    },
                    error_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let sink = collected.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                        }
                    },
                    error_fn,
                    None,
                )
            }
            other => panic!("microphone probe cannot handle sample format {other:?}"),
        }
        .expect("could not build an input stream for the default microphone");

        stream.play().expect("could not start the microphone stream");
        std::thread::sleep(std::time::Duration::from_secs(2));
        drop(stream);

        let samples = collected.lock().map(|s| s.clone()).unwrap_or_default();
        assert!(
            !samples.is_empty(),
            "the microphone delivered no samples at all"
        );

        let peak = samples
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!(
            peak > 1e-4,
            "the microphone delivered silence (peak {peak:e}); check the Windows microphone privacy setting and that the selected input is not muted"
        );
        println!("probe captured {} samples, peak {peak:.4}", samples.len());
    }

    /// Capture the system output through the same cpal loopback path a recording uses.
    ///
    /// Ignored by default: it needs something audible. Run
    /// "cargo test --lib real_system_loopback_probe -- --ignored --nocapture" while a
    /// tone or music is playing; the raw f32 samples are written next to the test
    /// binary so the captured waveform can be compared with the source.
    #[test]
    #[ignore]
    fn real_system_loopback_probe_captures_output() {
        use crate::audio::{get_device_and_config, AudioDevice, DeviceType as AudioDeviceKind};
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::SampleFormat;
        use std::sync::{Arc, Mutex};

        let host = cpal::default_host();
        let output = host
            .default_output_device()
            .expect("no default output device — check the Windows sound settings");
        let name = output
            .name()
            .expect("the default output device reports no name");
        println!("probing system output '{name}' through the recording loopback path");

        let device = AudioDevice::new(name, AudioDeviceKind::Output);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build a runtime for device lookup");
        let (cpal_device, config) = runtime
            .block_on(get_device_and_config(&device))
            .expect("could not open the output device for loopback capture");
        println!(
            "loopback config: {:?}, {} Hz, {} channel(s)",
            config.sample_format(),
            config.sample_rate().0,
            config.channels()
        );

        let collected: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let stream_config = config.config();
        let error_fn = |err| eprintln!("system loopback probe stream error: {err}");

        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                let sink = collected.clone();
                cpal_device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend_from_slice(data);
                        }
                    },
                    error_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let sink = collected.clone();
                cpal_device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend(data.iter().map(|&s| s as f32 / 32768.0));
                        }
                    },
                    error_fn,
                    None,
                )
            }
            SampleFormat::I32 => {
                let sink = collected.clone();
                cpal_device.build_input_stream(
                    &stream_config,
                    move |data: &[i32], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend(data.iter().map(|&s| s as f32 / 2147483648.0));
                        }
                    },
                    error_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let sink = collected.clone();
                cpal_device.build_input_stream(
                    &stream_config,
                    move |data: &[u16], _| {
                        if let Ok(mut sink) = sink.lock() {
                            sink.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                        }
                    },
                    error_fn,
                    None,
                )
            }
            other => panic!("system loopback probe cannot handle sample format {other:?}"),
        }
        .expect("could not build a loopback input stream");

        stream.play().expect("could not start the loopback stream");
        std::thread::sleep(std::time::Duration::from_secs(5));
        drop(stream);

        let samples = collected.lock().map(|s| s.clone()).unwrap_or_default();
        assert!(
            !samples.is_empty(),
            "the loopback stream delivered no samples at all"
        );
        let peak = samples
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        // The raw stream is interleaved and may be multi-channel: duration has to
        // divide by the channel count, and the waveform must be measured on one
        // channel or the interleave itself shows up as extra zero crossings.
        let channels = config.channels() as usize;
        let seconds =
            samples.len() as f32 / (config.sample_rate().0 as f32 * channels.max(1) as f32);
        let channel_zero: Vec<f32> = samples.iter().copied().step_by(channels.max(1)).collect();
        let zero_crossings = channel_zero
            .windows(2)
            .filter(|pair| pair[0].is_sign_positive() != pair[1].is_sign_positive())
            .count();

        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in &samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let probe_path = std::env::temp_dir().join("meetily-system-loopback-probe.f32");
        std::fs::write(&probe_path, &bytes).expect("write the probe samples");

        println!(
            "probe captured {} samples ({:.2}s), peak {:.4}, rms {:.4}, zero-crossing rate {:.0} Hz",
            samples.len(),
            seconds,
            peak,
            rms,
            zero_crossings as f32 / seconds / 2.0
        );
        println!("raw samples: {}", probe_path.display());

        assert!(
            peak > 1e-4,
            "loopback delivered silence (peak {peak:e}); check that something is playing"
        );
    }
}
