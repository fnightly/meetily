// audio/echo_cancellation.rs
//
// Acoustic echo cancellation for the microphone when the meeting is playing
// through speakers.
//
// Without it, whatever the system stream contains is re-captured by the
// microphone and mixed back in half a second later. `sonora` is a pure-Rust port
// of the WebRTC audio processing module; only its AEC3 is used here. The capture
// path already runs a high-pass filter, EBU R128 normalisation and optional
// RNNoise, so enabling sonora's NS/AGC/HPF stages as well would process the
// microphone twice and change the sound for every existing recording.

use std::collections::VecDeque;

use log::{debug, info, warn};
use sonora::config::EchoCanceller as WebRtcEchoCanceller;
use sonora::{AudioProcessing, Config, StreamConfig};

/// The capture pipeline is fixed at 48 kHz mono; AEC3 works on 10 ms frames.
const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = (SAMPLE_RATE / 100) as usize;
const SAMPLES_PER_MS: usize = (SAMPLE_RATE / 1000) as usize;

/// Assumed echo path delay until the correlation estimate has enough history.
const DEFAULT_STREAM_DELAY_MS: i32 = 50;
/// Longest echo path considered by the estimate; AEC3 itself clamps to 0..=500 ms.
const MAX_ESTIMATED_DELAY_MS: usize = 200;
/// Microphone tail used to measure the echo path.
///
/// The mixer hands over 600 ms windows, but correlating all of one against every
/// lag multiplies the estimate cost for no better answer: the tail is produced by
/// the same echo path as the rest of the window.
const DELAY_ESTIMATE_SLICE_SAMPLES: usize = SAMPLES_PER_MS * 50;
/// Far-end history needed for the longest lag plus one estimate slice.
const RENDER_HISTORY_SAMPLES: usize =
    DELAY_ESTIMATE_SLICE_SAMPLES + MAX_ESTIMATED_DELAY_MS * SAMPLES_PER_MS;
/// A window quieter than this is not used to measure the echo path.
const AUDIBLE_RMS: f32 = 1e-3;
/// Normalised correlation below which a delay estimate is not trusted.
const MIN_DELAY_CORRELATION: f32 = 0.3;
/// Bounds the cost of retrying the estimate when the early windows are silent.
const MAX_DELAY_ESTIMATE_ATTEMPTS: usize = 30;

/// Echo canceller for one recording session.
pub struct EchoCanceller {
    apm: AudioProcessing,
    /// Far-end samples kept so the echo path can be measured against a full 200 ms
    /// of history, not just the window being processed.
    render_history: VecDeque<f32>,
    estimated_delay_ms: Option<i32>,
    delay_estimate_attempts: usize,
}

impl EchoCanceller {
    /// Build the canceller, or `None` when this session cannot use it.
    pub fn new(sample_rate: u32) -> Option<Self> {
        if sample_rate != SAMPLE_RATE {
            warn!(
                "Echo cancellation needs {} Hz, pipeline is {} Hz — continuing without it",
                SAMPLE_RATE, sample_rate
            );
            return None;
        }

        let config = Config {
            echo_canceller: Some(WebRtcEchoCanceller::default()),
            ..Default::default()
        };
        let mut apm = AudioProcessing::builder()
            .config(config)
            .capture_config(StreamConfig::new(SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(SAMPLE_RATE, 1))
            .build();

        // A desk-to-desk setup with a small buffer is a few tens of milliseconds;
        // the first window with audible far-end audio replaces this with a measured
        // delay.
        let _ = apm.set_stream_delay_ms(DEFAULT_STREAM_DELAY_MS);

        info!("🔇 Echo cancellation enabled (WebRTC AEC3, {} Hz mono)", SAMPLE_RATE);

        Some(Self {
            apm,
            render_history: VecDeque::with_capacity(RENDER_HISTORY_SAMPLES),
            estimated_delay_ms: None,
            delay_estimate_attempts: 0,
        })
    }

    /// Echo path delay measured from the first audible render window, if one was seen.
    pub fn estimated_delay_ms(&self) -> Option<i32> {
        self.estimated_delay_ms
    }

    /// Remove the far-end echo from one microphone window.
    ///
    /// The render window is the system audio the user was hearing while the
    /// microphone captured this window. Output length always matches
    /// `mic_window`; only whole 10 ms frames are processed and a shorter render
    /// window is zero-padded, so a partial window degrades to a frame of silence
    /// on the far end rather than shifting the rest of the stream.
    pub fn process_window(&mut self, mic_window: &[f32], render_window: &[f32]) -> Vec<f32> {
        self.remember_render(render_window);
        self.refine_delay(mic_window, render_window);

        let frames = mic_window.len() / FRAME_SAMPLES;
        let mut output = Vec::with_capacity(mic_window.len());

        for frame_index in 0..frames {
            let start = frame_index * FRAME_SAMPLES;
            let end = start + FRAME_SAMPLES;

            let mut render_frame = [0.0f32; FRAME_SAMPLES];
            if start < render_window.len() {
                let available = (render_window.len() - start).min(FRAME_SAMPLES);
                render_frame[..available].copy_from_slice(&render_window[start..start + available]);
            }

            // Render has to be pushed before the capture frame it lines up with.
            // AEC3 keeps the far-end frames it has received in an internal history
            // and aligns capture frame k with render frame k - delay using the
            // reported stream delay, so the frames it needs were pushed in earlier
            // iterations. Same-index interleaving is therefore the real-time order
            // the APM expects; pushing the whole render window ahead would be the
            // same samples in an order the delay setting does not describe.
            let render_src: [&[f32]; 1] = [&render_frame];
            let mut render_output = [0.0f32; FRAME_SAMPLES];
            let mut render_dst: [&mut [f32]; 1] = [&mut render_output];
            if let Err(e) = self.apm.process_render_f32(&render_src, &mut render_dst) {
                debug!("Echo cancellation render frame failed ({}), passing mic through", e);
                output.extend_from_slice(&mic_window[start..end]);
                continue;
            }

            let capture_src: [&[f32]; 1] = [&mic_window[start..end]];
            let mut capture_output = [0.0f32; FRAME_SAMPLES];
            let mut capture_dst: [&mut [f32]; 1] = [&mut capture_output];
            match self.apm.process_capture_f32(&capture_src, &mut capture_dst) {
                Ok(()) => output.extend_from_slice(&capture_output),
                Err(e) => {
                    debug!("Echo cancellation capture frame failed ({}), passing mic through", e);
                    output.extend_from_slice(&mic_window[start..end]);
                }
            }
        }

        // A trailing partial frame is passed through untouched: AEC3 only accepts
        // whole 10 ms frames, and resampling just the remainder would distort it.
        output.extend_from_slice(&mic_window[frames * FRAME_SAMPLES..]);
        output
    }

    fn remember_render(&mut self, render_window: &[f32]) {
        for &sample in render_window {
            if self.render_history.len() == RENDER_HISTORY_SAMPLES {
                self.render_history.pop_front();
            }
            self.render_history.push_back(sample);
        }
    }

    /// Measure the echo path once, on the first window where both sides carry sound.
    ///
    /// The initial 50 ms guess is only a guess: speakers, output buffering and the
    /// operating system's audio engine each add delay, and AEC3 aligns its
    /// adaptive filter from this number. One measurement is enough because the
    /// delay is a property of the setup, not of the content.
    fn refine_delay(&mut self, mic_window: &[f32], render_window: &[f32]) {
        if self.estimated_delay_ms.is_some() || self.delay_estimate_attempts >= MAX_DELAY_ESTIMATE_ATTEMPTS {
            return;
        }
        if mic_window.len() < DELAY_ESTIMATE_SLICE_SAMPLES {
            return;
        }

        // Correlate only the newest slice. Render history ends at this window's end
        // (it was just pushed), so a lag maps the microphone tail onto the render
        // that produced it without putting the whole 600 ms window through every lag.
        let mic_slice = &mic_window[mic_window.len() - DELAY_ESTIMATE_SLICE_SAMPLES..];
        if rms(render_window) < AUDIBLE_RMS || rms(mic_slice) < AUDIBLE_RMS {
            return;
        }
        if self.render_history.len() < RENDER_HISTORY_SAMPLES {
            return;
        }

        self.delay_estimate_attempts += 1;
        let Some((delay_ms, correlation)) = estimate_delay(&self.render_history, mic_slice) else {
            return;
        };
        if correlation < MIN_DELAY_CORRELATION {
            return;
        }

        if self.apm.set_stream_delay_ms(delay_ms).is_err() {
            // Only clamped values fail; the estimate is inside AEC3's range, so this
            // would mean the bound constants drifted apart.
            warn!("Echo path delay {} ms was out of the supported range", delay_ms);
            return;
        }
        self.estimated_delay_ms = Some(delay_ms);
        info!(
            "🔇 Echo path delay measured at {} ms (correlation {:.2})",
            delay_ms, correlation
        );
    }
}

/// Root-mean-square level of a window.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Find the lag whose far-end segment best matches the microphone window.
///
/// Returns the lag in milliseconds and the absolute normalised correlation, so
/// the caller can refuse a weak match instead of tuning AEC3 to noise. The
/// correlation is taken on the absolute value because a speaker can be wired with
/// inverted polarity.
fn estimate_delay(render_history: &VecDeque<f32>, mic_window: &[f32]) -> Option<(i32, f32)> {
    let window = mic_window.len();
    if window == 0 || render_history.len() < window + MAX_ESTIMATED_DELAY_MS * SAMPLES_PER_MS {
        return None;
    }

    let mic_energy: f32 = mic_window.iter().map(|s| s * s).sum();
    if mic_energy <= f32::EPSILON {
        return None;
    }

    let mut best: Option<(i32, f32)> = None;
    for delay_ms in 0..=MAX_ESTIMATED_DELAY_MS {
        let delay_samples = delay_ms * SAMPLES_PER_MS;
        let start = render_history.len() - delay_samples - window;
        let render_samples = render_history.iter().skip(start).take(window);
        let render_energy: f32 = render_samples.clone().map(|s| s * s).sum();
        if render_energy <= f32::EPSILON {
            continue;
        }
        let dot: f32 = mic_window
            .iter()
            .zip(render_history.iter().skip(start).take(window))
            .map(|(mic, render)| mic * render)
            .sum();
        let correlation = (dot / (mic_energy * render_energy).sqrt()).abs();
        if best.map_or(true, |(_, best_correlation)| correlation > best_correlation) {
            best = Some((delay_ms as i32, correlation));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mixer hands over 600 ms windows (pipeline.rs: window_ms = 600).
    const TEST_WINDOW: usize = FRAME_SAMPLES * 60;

    /// Deterministic broadband noise; no rand dependency and no per-run variation.
    struct Lcg(u64);

    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32 / 8_388_608.0) - 1.0
        }
    }

    fn energy(samples: &[f32]) -> f64 {
        samples.iter().map(|&s| (s as f64) * (s as f64)).sum()
    }

    #[test]
    fn cancels_synthetic_echo_in_local_silence() {
        const WINDOWS: usize = 6; // 3.6 s of 600 ms windows

        let mut noise = Lcg(0x5eed_1234);
        let mut render = vec![0.0f32; TEST_WINDOW * WINDOWS];
        for sample in render.iter_mut() {
            *sample = 0.3 * noise.next_f32();
        }

        // The microphone hears the render through the room, 40 ms late, plus local
        // speech during the middle second only. Everything after that is local
        // silence, so the remaining energy there is echo that was not removed.
        let echo_delay_samples = 40 * SAMPLES_PER_MS;
        let mut mic = vec![0.0f32; render.len()];
        for index in 0..mic.len() {
            let echo = if index >= echo_delay_samples {
                0.5 * render[index - echo_delay_samples]
            } else {
                0.0
            };
            let window = index / TEST_WINDOW;
            let speech = if (1..2).contains(&window) {
                0.05 * (2.0 * std::f32::consts::PI * 440.0 * index as f32 / SAMPLE_RATE as f32).sin()
            } else {
                0.0
            };
            mic[index] = echo + speech;
        }

        let mut canceller = EchoCanceller::new(SAMPLE_RATE).expect("construct AEC at 48 kHz");

        let mut raw_silence_energy = 0.0f64;
        let mut cancelled_silence_energy = 0.0f64;
        for window in 0..WINDOWS {
            let start = window * TEST_WINDOW;
            let end = start + TEST_WINDOW;
            let output = canceller.process_window(&mic[start..end], &render[start..end]);

            assert_eq!(output.len(), TEST_WINDOW, "window length changed");
            assert!(
                output.iter().all(|s| s.is_finite() && s.abs() <= 1.05),
                "AEC emitted a non-finite or out-of-range sample"
            );

            // Only the second half counts: AEC3 needs time to adapt, and its
            // residual before that says nothing about steady-state quality.
            if window >= WINDOWS / 2 {
                raw_silence_energy += energy(&mic[start..end]);
                cancelled_silence_energy += energy(&output);
            }
        }

        let delay_ms = canceller.estimated_delay_ms();
        let reduction_db = 10.0
            * (raw_silence_energy / cancelled_silence_energy.max(f64::MIN_POSITIVE)).log10();
        println!(
            "delay estimate: {:?} ms, echo reduction in local silence: {:.1} dB",
            delay_ms, reduction_db
        );

        assert!(
            delay_ms.is_some(),
            "the echo path delay was never estimated from audible render"
        );
        assert!(
            reduction_db >= 6.0,
            "AEC only removed {reduction_db:.1} dB of echo (want at least 6 dB)"
        );
    }

    #[test]
    fn process_window_preserves_length_and_trailing_remainder() {
        let mut canceller = EchoCanceller::new(SAMPLE_RATE).expect("construct AEC");

        let mic: Vec<f32> = (0..1000)
            .map(|index| 0.1 * (index as f32 * 0.05).sin())
            .collect();
        let render = vec![0.05f32; 700];

        let output = canceller.process_window(&mic, &render);

        assert_eq!(output.len(), mic.len(), "output length must match input");
        let whole_frames = (mic.len() / FRAME_SAMPLES) * FRAME_SAMPLES;
        assert_eq!(
            &output[whole_frames..],
            &mic[whole_frames..],
            "a trailing partial frame must pass through unchanged"
        );
        assert!(output.iter().all(|s| s.is_finite() && s.abs() <= 1.05));
    }

    #[test]
    fn delay_estimate_finds_the_synthetic_echo_path() {
        const MIC_LEN: usize = DELAY_ESTIMATE_SLICE_SAMPLES;
        const HISTORY_LEN: usize = MIC_LEN + MAX_ESTIMATED_DELAY_MS * SAMPLES_PER_MS;

        let mut noise = Lcg(0xabcd_4321);
        let mut history = VecDeque::with_capacity(HISTORY_LEN);
        for _ in 0..HISTORY_LEN {
            history.push_back(0.3 * noise.next_f32());
        }

        let delay_ms = 40;
        let delay_samples = delay_ms * SAMPLES_PER_MS;
        let mic: Vec<f32> = history
            .iter()
            .skip(HISTORY_LEN - delay_samples - MIC_LEN)
            .take(MIC_LEN)
            .map(|sample| 0.5 * sample)
            .collect();

        let (estimated_ms, correlation) =
            estimate_delay(&history, &mic).expect("estimate from correlated windows");

        assert_eq!(estimated_ms, delay_ms as i32);
        assert!(correlation > 0.9, "weak correlation {correlation:.2}");
    }
}
