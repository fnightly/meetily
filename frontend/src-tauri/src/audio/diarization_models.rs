// audio/diarization_models.rs
//
// ONNX implementation of [`super::diarization::DiarizationModels`].
//
// Two models, both run through the ONNX Runtime this app already loads:
//
//   * **segmentation** — a pyannote-style powerset classifier over short sliding
//     windows of audio. Its metadata (window size, receptive field, class count)
//     is read from the ONNX file rather than hard-coded, because exports differ and
//     a wrong value silently stretches the timeline.
//   * **speaker embedding** — one vector per speech span, which
//     `super::speaker_cluster` then groups into speakers.
//
// The arrangement in `super::diarization` is already tested against a fake; this
// file is where the real tensors are wired up.

use std::path::Path;

use ndarray::{ArrayD, IxDyn};

use super::diarization::{decode_chunk, powerset_mapping, SegmentationMeta};
use super::onnx_model::{array2, array3, OnnxModel};

/// Metadata keys the segmentation export publishes.
///
/// Missing keys are fatal rather than defaulted: guessing a window size produces a
/// transcript with the wrong speaker at every timestamp.
fn read_usize(meta: &[(String, String)], key: &str) -> Result<usize, String> {
    meta.iter()
        .find(|(name, _)| name == key)
        .ok_or_else(|| format!("The segmentation model does not publish '{key}' metadata"))?
        .1
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("Segmentation metadata '{key}' is not a number: {e}"))
}

/// Read the segmentation model's self-description.
pub fn read_segmentation_meta(meta: &[(String, String)]) -> Result<SegmentationMeta, String> {
    Ok(SegmentationMeta {
        window_size: read_usize(meta, "window_size")?,
        sample_rate: read_usize(meta, "sample_rate")? as u32,
        receptive_field_size: read_usize(meta, "receptive_field_size")?,
        receptive_field_shift: read_usize(meta, "receptive_field_shift")?,
        num_classes: read_usize(meta, "num_classes")?,
        num_speakers: read_usize(meta, "num_speakers")?,
        powerset_max_classes: read_usize(meta, "powerset_max_classes")?,
    })
}

/// Offsets for one sliding window.
///
/// The reference builds these with `numpy.lib.stride_tricks.as_strided`; the
/// arithmetic is the same, just expressed as explicit offsets.
pub fn window_offsets(num_samples: usize, window_size: usize, window_shift: usize) -> Vec<usize> {
    if window_size == 0 || window_shift == 0 || num_samples < window_size {
        return Vec::new();
    }

    let count = (num_samples - window_size) / window_shift + 1;
    (0..count).map(|index| index * window_shift).collect()
}

/// Whether the audio needs a final padded window.
///
/// The trailing samples do not divide evenly by the shift, so the reference adds one
/// more window padded with silence. Without it the last fraction of a second is never
/// analysed, which can drop the final word of a meeting.
///
/// Only a remainder *smaller than the shift* needs this. A remainder equal to a whole
/// shift is already covered: the final complete window sits exactly on it.
pub fn needs_trailing_window(num_samples: usize, window_size: usize, window_shift: usize) -> bool {
    if window_shift == 0 {
        return false;
    }

    if num_samples < window_size {
        return true;
    }

    let remainder = (num_samples - window_size) % window_shift;
    remainder > 0 && remainder < window_shift
}

/// Assemble one window of exactly `window_size` samples, zero-padded past the end.
pub fn slice_window(samples: &[f32], offset: usize, window_size: usize) -> Vec<f32> {
    let mut window = vec![0.0f32; window_size];

    if offset >= samples.len() {
        return window;
    }

    let available = (samples.len() - offset).min(window_size);
    window[..available].copy_from_slice(&samples[offset..offset + available]);
    window
}

/// The segmentation model, with its metadata resolved.
pub struct SegmentationModel {
    model: OnnxModel,
    meta: SegmentationMeta,
    mapping: Vec<Vec<u8>>,
}

impl SegmentationModel {
    /// Load the model and read its self-description.
    ///
    /// The metadata comes from the ONNX file, so a new export does not need code
    /// changes and cannot be silently mis-parameterised.
    pub fn new(path: &Path, intra_threads: Option<usize>) -> Result<Self, String> {
        let model = OnnxModel::load(path, intra_threads)?;
        let meta = read_segmentation_meta(model.custom_metadata())?;
        let mapping =
            powerset_mapping(meta.num_classes, meta.num_speakers, meta.powerset_max_classes)?;

        log::info!(
            "Segmentation model: window={} samples ({:.1}s), shift={} samples, {} classes, {} local speakers",
            meta.window_size,
            meta.window_size as f64 / meta.sample_rate as f64,
            meta.window_shift(),
            meta.num_classes,
            meta.num_speakers
        );

        Ok(Self { model, meta, mapping })
    }

    pub fn meta(&self) -> &SegmentationMeta {
        &self.meta
    }

    /// Run the model over one batch of windows.
    ///
    /// `windows` is row-major `batch * window_size`. Returns row-major
    /// `batch * num_frames * num_speakers` after powerset decoding, plus the frame
    /// count.
    pub fn run_batch(&mut self, windows: &[Vec<f32>]) -> Result<(Vec<u8>, usize), String> {
        if windows.is_empty() {
            return Ok((Vec::new(), 0));
        }

        let batch = windows.len();
        let window_size = self.meta.window_size;

        // The graph expects (batch, 1, samples): a single audio channel.
        let mut flat = Vec::with_capacity(batch * window_size);
        for window in windows {
            if window.len() != window_size {
                return Err(format!(
                    "Segmentation window has {} samples but {} were expected",
                    window.len(),
                    window_size
                ));
            }
            flat.extend_from_slice(window);
        }

        let input = array3(batch, 1, window_size, flat)?;
        let output = self.model.run_single(vec![input])?;

        let shape = output.shape().to_vec();
        if shape.len() != 3 {
            return Err(format!(
                "Segmentation model returned a {:?} tensor; expected 3 dimensions",
                shape
            ));
        }

        let (out_batch, num_frames, num_classes) = (shape[0], shape[1], shape[2]);

        if out_batch != batch {
            return Err(format!(
                "Segmentation model returned {out_batch} results for a batch of {batch}"
            ));
        }

        let values: Vec<f32> = output.iter().copied().collect();

        let mut decoded = Vec::with_capacity(batch * num_frames * self.meta.num_speakers);
        for chunk in 0..batch {
            let start = chunk * num_frames * num_classes;
            let end = start + num_frames * num_classes;
            let part = decode_chunk(&values[start..end], num_frames, num_classes, &self.mapping)?;
            decoded.extend_from_slice(&part);
        }

        Ok((decoded, num_frames))
    }
}

/// The speaker embedding model.
///
/// Its input is log-mel features, not audio: the 3D-Speaker export declares
/// `(batch, frames, 80)`. Features are computed by [`super::fbank`] and then
/// mean-normalised, which the model's metadata requests via
/// `feature_normalize_type = global-mean`.
pub struct EmbeddingModel {
    model: OnnxModel,
    fbank: super::fbank::FbankExtractor,
}

impl EmbeddingModel {
    pub fn new(path: &Path, intra_threads: Option<usize>) -> Result<Self, String> {
        let model = OnnxModel::load(path, intra_threads)?;
        let fbank = super::fbank::FbankExtractor::new()?;
        Ok(Self { model, fbank })
    }

    pub fn input_names(&self) -> &[String] {
        self.model.input_names()
    }

    /// Embed a batch of equal-length audio windows.
    ///
    /// Returns one 512-dimension vector per window. The windows must all be the same
    /// length so their feature matrices stack into one tensor.
    pub fn embed(&mut self, windows: &[Vec<f32>]) -> Result<Vec<Vec<f32>>, String> {
        if windows.is_empty() {
            return Ok(Vec::new());
        }

        let expected_samples = windows[0].len();
        if expected_samples == 0 {
            return Err("Embedding windows are empty".to_string());
        }

        let frames = super::fbank::FbankExtractor::num_frames(expected_samples);
        if frames == 0 {
            return Err(format!(
                "A {expected_samples}-sample window is too short to produce a single feature frame"
            ));
        }

        let rows = windows.len();
        let mut flat = Vec::with_capacity(rows * frames * super::fbank::NUM_MEL_BINS);

        for window in windows {
            if window.len() != expected_samples {
                return Err(format!(
                    "Embedding windows must all be the same length, got {} and {expected_samples}",
                    window.len()
                ));
            }

            let mut features = self.fbank.compute(window)?;
            super::fbank::FbankExtractor::normalize_global_mean(&mut features, frames);
            flat.extend_from_slice(&features);
        }

        let input = array3(rows, frames, super::fbank::NUM_MEL_BINS, flat)?;
        let output = self.model.run_single(vec![input])?;

        let shape = output.shape().to_vec();
        if shape.len() != 2 {
            return Err(format!(
                "Embedding model returned a {:?} tensor; expected (batch, dim)",
                shape
            ));
        }

        if shape[0] != rows {
            return Err(format!(
                "Embedding model returned {} vectors for a batch of {rows}",
                shape[0]
            ));
        }

        let dim = shape[1];
        let values: Vec<f32> = output.iter().copied().collect();

        Ok((0..rows)
            .map(|row| values[row * dim..(row + 1) * dim].to_vec())
            .collect())
    }
}

/// Build a 3-D array from a flat batch, for callers that need the raw helper.
pub fn batch_tensor(
    batch: usize,
    channels: usize,
    samples: usize,
    values: Vec<f32>,
) -> Result<ArrayD<f32>, String> {
    ArrayD::from_shape_vec(IxDyn(&[batch, channels, samples]), values)
        .map_err(|e| format!("Failed to build a batch tensor: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_pairs() -> Vec<(String, String)> {
        vec![
            ("window_size".to_string(), "160000".to_string()),
            ("sample_rate".to_string(), "16000".to_string()),
            ("receptive_field_size".to_string(), "3200".to_string()),
            ("receptive_field_shift".to_string(), "160".to_string()),
            ("num_classes".to_string(), "7".to_string()),
            ("num_speakers".to_string(), "3".to_string()),
            ("powerset_max_classes".to_string(), "2".to_string()),
        ]
    }

    #[test]
    fn metadata_is_read_from_the_model() {
        let meta = read_segmentation_meta(&meta_pairs()).expect("meta");
        assert_eq!(meta.window_size, 160_000);
        assert_eq!(meta.sample_rate, 16_000);
        assert_eq!(meta.num_classes, 7);
        assert_eq!(meta.window_shift(), 16_000);
    }

    #[test]
    fn a_missing_metadata_key_is_an_error_not_a_default() {
        // Guessing a window size would stretch every timestamp silently.
        let mut pairs = meta_pairs();
        pairs.retain(|(name, _)| name != "window_size");

        let error = read_segmentation_meta(&pairs).err().expect("must fail");
        assert!(error.contains("window_size"), "got: {error}");
    }

    #[test]
    fn a_non_numeric_metadata_value_is_an_error() {
        let mut pairs = meta_pairs();
        for pair in pairs.iter_mut() {
            if pair.0 == "receptive_field_shift" {
                pair.1 = "ten".to_string();
            }
        }

        let error = read_segmentation_meta(&pairs).err().expect("must fail");
        assert!(error.contains("not a number"), "got: {error}");
    }

    #[test]
    fn window_offsets_tile_the_audio() {
        // 1000 samples, window 100, shift 50 -> offsets 0,50,...,900 (19 windows).
        let offsets = window_offsets(1000, 100, 50);
        assert_eq!(offsets.len(), 19);
        assert_eq!(offsets[0], 0);
        assert_eq!(offsets[1], 50);
        assert_eq!(*offsets.last().expect("last"), 900);
    }

    #[test]
    fn window_offsets_are_empty_when_audio_is_shorter_than_a_window() {
        assert!(window_offsets(50, 100, 50).is_empty());
    }

    #[test]
    fn window_offsets_handle_a_zero_shift_without_hanging() {
        // A zero shift would otherwise loop forever.
        assert!(window_offsets(1000, 100, 0).is_empty());
    }

    #[test]
    fn no_trailing_window_when_the_remainder_is_a_whole_shift() {
        // 1050 samples, window 100, shift 50 -> 19 windows; the last covers
        // [900,1000), and the remaining 50 samples are exactly one shift past it, so
        // they are already the end of the final window.
        assert!(!needs_trailing_window(1050, 100, 50));
    }

    #[test]
    fn a_trailing_window_is_added_when_the_audio_does_not_divide_evenly() {
        // 1075 samples leaves 25 samples past the last window's end, which the
        // sliding stride cannot reach: without a padded window the final 25 samples
        // are never analysed.
        assert!(needs_trailing_window(1075, 100, 50));
    }

    #[test]
    fn no_trailing_window_when_the_audio_divides_evenly() {
        // 1000 samples, window 100, shift 50 -> exact.
        assert!(!needs_trailing_window(1000, 100, 50));
    }

    #[test]
    fn a_trailing_window_is_needed_when_audio_is_shorter_than_one_window() {
        assert!(needs_trailing_window(50, 100, 50));
    }

    #[test]
    fn slice_window_pads_past_the_end_with_silence() {
        // Padding with silence adds no voice; truncating would lose the tail.
        let samples: Vec<f32> = (0..250).map(|i| i as f32).collect();
        let window = slice_window(&samples, 200, 100);

        assert_eq!(window.len(), 100);
        assert_eq!(window[0], 200.0);
        assert_eq!(window[49], 249.0);
        assert!(window[50..].iter().all(|v| *v == 0.0), "the tail must be silent");
    }

    #[test]
    fn slice_window_of_an_offset_past_the_end_is_all_silence() {
        let samples = vec![1.0f32; 10];
        let window = slice_window(&samples, 100, 8);
        assert_eq!(window.len(), 8);
        assert!(window.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn slice_window_takes_a_full_window_when_available() {
        let samples: Vec<f32> = (0..500).map(|i| i as f32).collect();
        let window = slice_window(&samples, 100, 100);

        assert_eq!(window.len(), 100);
        assert_eq!(window[0], 100.0);
        assert_eq!(window[99], 199.0);
    }

    #[test]
    fn batch_tensor_builds_the_requested_shape() {
        let tensor = batch_tensor(2, 1, 4, vec![0.0; 8]).expect("tensor");
        assert_eq!(tensor.shape(), &[2, 1, 4]);
    }

    #[test]
    fn batch_tensor_rejects_a_length_mismatch() {
        assert!(batch_tensor(2, 1, 4, vec![0.0; 7]).is_err());
    }
}
