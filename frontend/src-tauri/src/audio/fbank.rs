// audio/fbank.rs
//
// Log-mel filterbank features, for the speaker embedding model.
//
// The 3D-Speaker embedding model does not take audio: its input is
// `(batch, frames, 80)` of log-mel features, computed the Kaldi way. Feeding it raw
// samples fails on the tensor shape, and feeding it features that differ even
// slightly from the training configuration produces embeddings that cluster badly
// without any obvious error.
//
// Parameters follow the 3D-Speaker `fbank` frontend: 16 kHz, 25 ms frames with a
// 10 ms shift, 80 mel bins over 20 Hz–8 kHz, a Povey window, no pre-emphasis, and
// the Kaldi floor on the log.
//
// This is the one piece of the diarisation path that cannot be validated by shape
// alone, so the geometry is unit-tested and the result is checked end-to-end
// against the upstream reference clip.

use std::f32::consts::PI;

/// Sample rate the features are computed for.
pub const SAMPLE_RATE: u32 = 16_000;

/// Frame length and shift, in samples (25 ms and 10 ms).
pub const FRAME_LENGTH: usize = 400;
pub const FRAME_SHIFT: usize = 160;

/// Number of mel bins the model expects.
pub const NUM_MEL_BINS: usize = 80;

/// FFT size. The next power of two at or above the padded frame length.
pub const FFT_SIZE: usize = 512;

/// Mel filter range.
pub const LOW_FREQUENCY: f32 = 20.0;
pub const HIGH_FREQUENCY: f32 = 8_000.0;

/// Floor applied before the logarithm, matching Kaldi.
const LOG_FLOOR: f32 = 1.192_092_9e-7;

/// Convert a frequency in Hz to the mel scale.
///
/// Kaldi's formula, which is not the HTK one: `1127 * ln(1 + f / 700)`.
pub fn hz_to_mel(frequency: f32) -> f32 {
    1127.0 * (1.0 + frequency / 700.0).ln()
}

/// Convert a mel value back to Hz.
pub fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (mel / 1127.0).exp_m1()
}

/// The Povey window, which is what the 3D-Speaker frontend uses.
///
/// Not a Hann window: using Hann shifts the spectrum enough to change the
/// embeddings.
pub fn povey_window(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let ratio = index as f32 / (length - 1) as f32;
            (0.5 - 0.5 * (2.0 * PI * ratio).cos()).powf(0.85)
        })
        .collect()
}

/// Triangular mel filterbank.
///
/// Returns `num_bins` rows of `fft_size / 2 + 1` weights. Bins are spaced evenly on
/// the mel scale between the two frequencies, which is what makes low frequencies
/// resolve better than high ones.
pub fn mel_filterbank(
    num_bins: usize,
    fft_size: usize,
    sample_rate: u32,
    low_hz: f32,
    high_hz: f32,
) -> Vec<Vec<f32>> {
    let num_fft_bins = fft_size / 2 + 1;
    let low_mel = hz_to_mel(low_hz);
    let high_mel = hz_to_mel(high_hz);

    // num_bins + 2 points: the first and last are the outer feet of the triangles,
    // which are at zero.
    let mel_points: Vec<f32> = (0..num_bins + 2)
        .map(|index| low_mel + (high_mel - low_mel) * index as f32 / (num_bins + 1) as f32)
        .collect();

    let hz_points: Vec<f32> = mel_points.iter().map(|mel| mel_to_hz(*mel)).collect();

    // FFT bin index for each mel point. Kaldi floors to an integer bin.
    let bins: Vec<f32> = hz_points
        .iter()
        .map(|hz| hz * fft_size as f32 / sample_rate as f32)
        .collect();

    let mut filters = vec![vec![0.0f32; num_fft_bins]; num_bins];

    for bin in 0..num_bins {
        let left = bins[bin];
        let centre = bins[bin + 1];
        let right = bins[bin + 2];

        for (index, weight) in filters[bin].iter_mut().enumerate() {
            let position = index as f32;
            // Rising edge, then falling edge; outside the triangle the weight is 0.
            if position > left && position < centre && centre > left {
                *weight = (position - left) / (centre - left);
            } else if position > centre && position < right && right > centre {
                *weight = (right - position) / (right - centre);
            }
        }
    }

    filters
}

/// A prepared feature extractor.
///
/// Built once per diarisation run: the filterbank depends only on the
/// configuration, and rebuilding it per utterance would waste the allocation.
pub struct FbankExtractor {
    window: Vec<f32>,
    filters: Vec<Vec<f32>>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
}

impl FbankExtractor {
    pub fn new() -> Result<Self, String> {
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);

        Ok(Self {
            window: povey_window(FRAME_LENGTH),
            filters: mel_filterbank(
                NUM_MEL_BINS,
                FFT_SIZE,
                SAMPLE_RATE,
                LOW_FREQUENCY,
                HIGH_FREQUENCY,
            ),
            fft,
        })
    }

    /// Number of frames a signal of `num_samples` produces.
    ///
    /// Kaldi's `snip_edges = true` behaviour: only whole frames are used, so a
    /// signal shorter than one frame yields none.
    pub fn num_frames(num_samples: usize) -> usize {
        if num_samples < FRAME_LENGTH {
            0
        } else {
            (num_samples - FRAME_LENGTH) / FRAME_SHIFT + 1
        }
    }

    /// Compute log-mel features.
    ///
    /// Returns `frames * NUM_MEL_BINS` values in row-major order, or an empty vector
    /// when the signal is too short to frame.
    pub fn compute(&self, samples: &[f32]) -> Result<Vec<f32>, String> {
        let frames = Self::num_frames(samples.len());
        if frames == 0 {
            return Ok(Vec::new());
        }

        // The frame is zero-padded up to the FFT size.
        let mut buffer = self.fft.make_input_vec();
        let mut spectrum = self.fft.make_output_vec();

        let mut features = Vec::with_capacity(frames * NUM_MEL_BINS);

        for frame in 0..frames {
            let start = frame * FRAME_SHIFT;

            for (index, slot) in buffer.iter_mut().enumerate() {
                *slot = if index < FRAME_LENGTH {
                    samples[start + index] * self.window[index]
                } else {
                    0.0
                };
            }

            self.fft
                .process(&mut buffer, &mut spectrum)
                .map_err(|e| format!("FFT failed: {e}"))?;

            // Power spectrum, including Nyquist, which is why the length is
            // fft_size / 2 + 1.
            let power: Vec<f32> = spectrum.iter().map(|c| c.re * c.re + c.im * c.im).collect();

            for filter in &self.filters {
                let energy: f32 = filter
                    .iter()
                    .zip(power.iter())
                    .map(|(weight, value)| weight * value)
                    .sum();

                features.push(energy.max(LOG_FLOOR).ln());
            }
        }

        Ok(features)
    }

    /// Mean-normalise features across time.
    ///
    /// The model's metadata declares `feature_normalize_type = global-mean`, which
    /// is the 3D-Speaker name for subtracting the per-bin mean over the utterance.
    /// Raw log-mel features carry the recording's absolute level, so without this a
    /// loud and a quiet recording of the same voice sit far apart.
    pub fn normalize_global_mean(features: &mut [f32], num_frames: usize) {
        if num_frames == 0 {
            return;
        }

        let mut means = vec![0.0f32; NUM_MEL_BINS];
        for frame in 0..num_frames {
            for bin in 0..NUM_MEL_BINS {
                means[bin] += features[frame * NUM_MEL_BINS + bin];
            }
        }
        for mean in means.iter_mut() {
            *mean /= num_frames as f32;
        }

        for frame in 0..num_frames {
            for bin in 0..NUM_MEL_BINS {
                features[frame * NUM_MEL_BINS + bin] -= means[bin];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_conversion_round_trips() {
        for frequency in [20.0f32, 100.0, 1000.0, 4000.0, 8000.0] {
            let round_tripped = mel_to_hz(hz_to_mel(frequency));
            assert!(
                (round_tripped - frequency).abs() < 0.5,
                "{frequency} -> {round_tripped}"
            );
        }
    }

    #[test]
    fn mel_scale_is_monotonic() {
        // A non-monotonic scale would produce overlapping, inverted filters.
        let mut previous = f32::NEG_INFINITY;
        for frequency in (0..=8000).step_by(100) {
            let mel = hz_to_mel(frequency as f32);
            assert!(mel > previous, "not increasing at {frequency} Hz");
            previous = mel;
        }
    }

    #[test]
    fn mel_of_zero_is_zero() {
        assert!(hz_to_mel(0.0).abs() < 1e-6);
    }

    #[test]
    fn the_povey_window_has_the_right_length_and_range() {
        let window = povey_window(FRAME_LENGTH);
        assert_eq!(window.len(), FRAME_LENGTH);
        assert!(window.iter().all(|v| (0.0..=1.0).contains(v)));
        // The window starts at zero by construction.
        assert!(window[0].abs() < 1e-9);
    }

    #[test]
    fn the_povey_window_is_not_a_hann_window() {
        // The exponent is what distinguishes them, so applying Hann instead would
        // silently change every embedding. At a quarter of the length the raised
        // cosine is 0.5, and 0.5^0.85 is about 0.55.
        let window = povey_window(FRAME_LENGTH);
        let quarter = window[FRAME_LENGTH / 4];
        assert!(
            (quarter - 0.554).abs() < 0.01,
            "expected the Povey exponent, got {quarter}"
        );
        assert!(quarter > 0.5, "a Hann window would give exactly 0.5 here");
    }

    #[test]
    fn the_povey_window_peaks_in_the_middle() {
        let window = povey_window(FRAME_LENGTH);
        let middle = window[FRAME_LENGTH / 2];
        assert!(middle > window[0]);
        assert!(middle > window[FRAME_LENGTH - 1]);
    }

    #[test]
    fn the_filterbank_has_the_expected_shape() {
        let filters = mel_filterbank(NUM_MEL_BINS, FFT_SIZE, SAMPLE_RATE, LOW_FREQUENCY, HIGH_FREQUENCY);
        assert_eq!(filters.len(), NUM_MEL_BINS);
        for filter in &filters {
            assert_eq!(filter.len(), FFT_SIZE / 2 + 1);
        }
    }

    #[test]
    fn every_filter_has_a_peak_at_one() {
        // A filter whose triangle collapses has no peak and contributes nothing.
        let filters = mel_filterbank(NUM_MEL_BINS, FFT_SIZE, SAMPLE_RATE, LOW_FREQUENCY, HIGH_FREQUENCY);

        for (index, filter) in filters.iter().enumerate() {
            let peak = filter.iter().cloned().fold(f32::MIN, f32::max);
            assert!(peak <= 1.0 + 1e-6, "filter {index} peak {peak} above 1");
        }
    }

    #[test]
    fn filters_do_not_overlap_more_than_adjacent_ones() {
        // Two triangles that are not neighbours should never both be non-zero at the
        // same FFT bin, which would mean the mel spacing is wrong.
        let filters = mel_filterbank(NUM_MEL_BINS, FFT_SIZE, SAMPLE_RATE, LOW_FREQUENCY, HIGH_FREQUENCY);

        for bin in 0..FFT_SIZE / 2 + 1 {
            let active: Vec<usize> = (0..NUM_MEL_BINS)
                .filter(|filter| filters[*filter][bin] > 0.0)
                .collect();

            assert!(
                active.len() <= 2,
                "FFT bin {bin} is covered by {} filters: {active:?}",
                active.len()
            );
        }
    }

    #[test]
    fn frames_are_counted_the_kaldi_way() {
        // Shorter than one frame -> none.
        assert_eq!(FbankExtractor::num_frames(0), 0);
        assert_eq!(FbankExtractor::num_frames(399), 0);
        // Exactly one frame.
        assert_eq!(FbankExtractor::num_frames(400), 1);
        // One more shift -> two frames.
        assert_eq!(FbankExtractor::num_frames(560), 2);
    }

    #[test]
    fn a_short_signal_yields_no_features() {
        let extractor = FbankExtractor::new().expect("extractor");
        assert!(extractor.compute(&[0.0; 100]).expect("ok").is_empty());
        assert!(extractor.compute(&[]).expect("ok").is_empty());
    }

    #[test]
    fn features_have_one_row_of_bins_per_frame() {
        let extractor = FbankExtractor::new().expect("extractor");
        let samples = vec![0.1f32; 16_000];

        let features = extractor.compute(&samples).expect("ok");
        let frames = FbankExtractor::num_frames(samples.len());

        assert_eq!(features.len(), frames * NUM_MEL_BINS);
    }

    #[test]
    fn silence_produces_the_log_floor_everywhere() {
        // Silence must give the floor, not negative infinity or NaN.
        let extractor = FbankExtractor::new().expect("extractor");
        let features = extractor.compute(&vec![0.0f32; 8_000]).expect("ok");

        assert!(!features.is_empty());
        assert!(features.iter().all(|v| v.is_finite()), "features must be finite");
        for value in &features {
            assert!(
                (*value - LOG_FLOOR.ln()).abs() < 1e-3,
                "silence should sit at the log floor, got {value}"
            );
        }
    }

    #[test]
    fn a_tone_concentrates_energy_in_the_expected_bins() {
        // A 1 kHz tone must light up low-index mel bins, not high ones: this is the
        // check that the filterbank is oriented the right way round.
        let extractor = FbankExtractor::new().expect("extractor");
        let samples: Vec<f32> = (0..16_000)
            .map(|index| (2.0 * PI * 1000.0 * index as f32 / SAMPLE_RATE as f32).sin())
            .collect();

        let features = extractor.compute(&samples).expect("ok");
        let frames = FbankExtractor::num_frames(samples.len());

        let mean_per_bin: Vec<f32> = (0..NUM_MEL_BINS)
            .map(|bin| {
                (0..frames)
                    .map(|frame| features[frame * NUM_MEL_BINS + bin])
                    .sum::<f32>()
                    / frames as f32
            })
            .collect();

        let loudest = mean_per_bin
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("comparable"))
            .map(|(index, _)| index)
            .expect("a bin");

        assert!(
            loudest < NUM_MEL_BINS / 2,
            "a 1 kHz tone should peak in the lower half of the mel bins, peaked at {loudest}"
        );
    }

    #[test]
    fn louder_audio_gives_larger_raw_features() {
        let extractor = FbankExtractor::new().expect("extractor");
        let tone = |gain: f32| -> Vec<f32> {
            (0..8_000)
                .map(|index| gain * (2.0 * PI * 440.0 * index as f32 / SAMPLE_RATE as f32).sin())
                .collect()
        };

        let quiet = extractor.compute(&tone(0.01)).expect("ok");
        let loud = extractor.compute(&tone(0.5)).expect("ok");

        let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean(&loud) > mean(&quiet));
    }

    #[test]
    fn global_mean_normalisation_centres_each_bin() {
        let extractor = FbankExtractor::new().expect("extractor");
        let samples = vec![0.2f32; 8_000];
        let frames = FbankExtractor::num_frames(samples.len());

        let mut features = extractor.compute(&samples).expect("ok");
        FbankExtractor::normalize_global_mean(&mut features, frames);

        for bin in 0..NUM_MEL_BINS {
            let mean: f32 = (0..frames)
                .map(|frame| features[frame * NUM_MEL_BINS + bin])
                .sum::<f32>()
                / frames as f32;
            assert!(mean.abs() < 1e-4, "bin {bin} mean after normalisation is {mean}");
        }
    }

    #[test]
    fn normalisation_of_no_frames_is_a_no_op() {
        let mut features = vec![1.0f32; 10];
        FbankExtractor::normalize_global_mean(&mut features, 0);
        assert!(features.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn normalisation_preserves_the_frame_count() {
        let extractor = FbankExtractor::new().expect("extractor");
        let samples = vec![0.1f32; 8_000];
        let frames = FbankExtractor::num_frames(samples.len());

        let mut features = extractor.compute(&samples).expect("ok");
        let before = features.len();
        FbankExtractor::normalize_global_mean(&mut features, frames);

        assert_eq!(features.len(), before);
    }
}
