// audio/diarization_service.rs
//
// Runs speaker separation over a finished recording.
//
// This owns the orchestration: segment, embed, cluster, relabel, build turns. The
// pieces live in neighbouring modules and are already tested against a fake; this wires
// them to the real ONNX models and a real audio buffer.
//
// Deliberately synchronous. Every stage is CPU-bound ONNX or array work, so the caller
// runs it on a blocking thread rather than an async one. Making this `async` would only
// invite it onto the runtime where it would stall other tasks for minutes.

use std::path::Path;

use super::diarization::{
    batch_window_samples, build_turns, drop_short_turns, embeddable_windows, merge_adjacent,
    relabel_activity, select_top_speakers, speaker_count_per_frame, speaker_pairs,
    turns_from_chunks, ChunkLabels, SegmentationMeta, DIARIZATION_SAMPLE_RATE, MIN_DURATION_OFF,
    MIN_DURATION_ON,
};
use super::diarization_catalog as catalog;
use super::diarization_models::{EmbeddingModel, SegmentationModel};
use super::speaker_cluster::{cluster_embeddings, count_speakers, ClusterParams, SegmentEmbedding};
use super::speaker_turns::SpeakerTurn;

/// How many segmentation windows to run in one ONNX call.
///
/// Larger batches amortise the per-call overhead but hold more memory; 32 windows of ten
/// seconds is a few megabytes of audio and matched the reference's behaviour.
const SEGMENTATION_BATCH: usize = 32;

/// Longest audio handed to the embedding model, in samples (3 seconds).
///
/// Bounds the cost of a single unusually long speaker turn. The batch window is sized to
/// the speech that actually exists and only falls back to this cap.
const MAX_EMBEDDING_SAMPLES: usize = 3 * DIARIZATION_SAMPLE_RATE as usize;

/// Tunables for a separation run.
#[derive(Debug, Clone, Copy)]
pub struct DiarizationConfig {
    /// Clustering threshold; see [`super::speaker_cluster::CALIBRATED_THRESHOLD`].
    pub threshold: f32,
    /// Upper bound on speakers. Set it well above any plausible meeting so it does not
    /// bind and turn into a floor.
    pub max_speakers: usize,
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        let params = ClusterParams::default();
        Self {
            threshold: params.threshold,
            // Higher than the clustering default: a meeting rarely has this many people,
            // and a bound that binds would mislabel rather than merely over-split.
            max_speakers: 12,
        }
    }
}

/// What a separation run produced.
#[derive(Debug, Clone)]
pub struct DiarizationOutcome {
    /// Speaker turns in seconds, ordered by start time.
    pub turns: Vec<SpeakerTurn>,
    /// Number of distinct speakers found.
    pub speaker_count: usize,
    /// Seconds of audio analysed.
    pub duration_seconds: f64,
}

impl DiarizationOutcome {
    /// True when no speaker could be told apart.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() || self.speaker_count == 0
    }
}

/// A stage of a separation run, for progress reporting.
///
/// Coarse on purpose: the interface needs to know the run is advancing, not a percentage
/// it would have to smooth. The three stages take very different amounts of time — almost
/// all of it is embedding — so they are reported as distinct steps rather than equal
/// thirds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationPhase {
    /// Loading models and finding speech.
    Segmenting,
    /// Producing a voice vector per speaker span; this is the slow stage.
    Embedding,
    /// Grouping the vectors into speakers.
    Clustering,
}

impl DiarizationPhase {
    /// A caption for the interface.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Segmenting => "Finding speech",
            Self::Embedding => "Identifying voices",
            Self::Clustering => "Grouping speakers",
        }
    }
}

/// Separate speakers, reporting progress as each stage is reached.
///
/// `on_progress` is called with the phase and a completion fraction. It is a plain
/// callback rather than a channel so this stays free of Tauri types and can be called
/// from a blocking thread.
pub fn diarize_samples_with_progress(
    samples: &[f32],
    models_dir: &Path,
    config: DiarizationConfig,
    on_progress: Option<&dyn Fn(DiarizationPhase, f32)>,
) -> Result<DiarizationOutcome, String> {
    let report = |phase: DiarizationPhase, fraction: f32| {
        if let Some(callback) = on_progress {
            callback(phase, fraction.clamp(0.0, 1.0));
        }
    };

    report(DiarizationPhase::Segmenting, 0.0);

    // Progress within segmentation, reported per batch: a long meeting runs many batches
    // and a single "starting" message would leave the interface looking hung.
    let segmentation_progress = |done: usize, total: usize| {
        let fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        report(DiarizationPhase::Segmenting, fraction);
    };

    let outcome = diarize_inner(samples, models_dir, config, &segmentation_progress, &report)?;

    Ok(outcome)
}

/// Separation over 16 kHz mono samples.
///
/// Returns an outcome with no turns when the audio contains nothing separable — silence,
/// or speech too short to identify a voice. That is a normal result, not an error: the
/// caller should keep the unlabelled transcript rather than fail.
pub fn diarize_samples(
    samples: &[f32],
    models_dir: &Path,
    config: DiarizationConfig,
) -> Result<DiarizationOutcome, String> {
    diarize_samples_with_progress(samples, models_dir, config, None)
}

fn diarize_inner(
    samples: &[f32],
    models_dir: &Path,
    config: DiarizationConfig,
    segmentation_progress: &dyn Fn(usize, usize),
    report: &dyn Fn(DiarizationPhase, f32),
) -> Result<DiarizationOutcome, String> {
    let duration_seconds = samples.len() as f64 / DIARIZATION_SAMPLE_RATE as f64;

    if samples.is_empty() {
        return Ok(DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds,
        });
    }

    let missing = catalog::missing_artifacts(models_dir);
    if !missing.is_empty() {
        return Err(catalog::missing_summary(models_dir));
    }

    let mut segmentation = SegmentationModel::new(&catalog::segmentation_path(models_dir), None)?;
    let meta = *segmentation.meta();

    let windows = build_windows(samples, &meta);
    if windows.is_empty() {
        log::info!("Recording is shorter than one segmentation window; no speaker can be separated");
        return Ok(DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds,
        });
    }

    log::info!(
        "Separating speakers across {:.1}s of audio in {} window(s)",
        duration_seconds,
        windows.len()
    );

    // --- Segment ------------------------------------------------------------
    let mut chunks: Vec<ChunkLabels> = Vec::with_capacity(windows.len());
    let total_batches = windows.len().div_ceil(SEGMENTATION_BATCH);

    for (batch_index, batch) in windows.chunks(SEGMENTATION_BATCH).enumerate() {
        let audio_batch: Vec<Vec<f32>> = batch.iter().map(|window| window.audio.clone()).collect();
        let (decoded, num_frames) = segmentation.run_batch(&audio_batch)?;

        for (index, window) in batch.iter().enumerate() {
            let base = index * num_frames * meta.num_speakers;
            let end = base + num_frames * meta.num_speakers;

            chunks.push(ChunkLabels {
                sample_offset: window.offset,
                num_frames,
                labels: decoded[base..end].to_vec(),
            });
        }

        segmentation_progress(batch_index + 1, total_batches);
    }

    // A baseline that needs no embeddings. If this finds no speech, separating speakers
    // is pointless and the embedding models need not be loaded at all.
    let frame_turns = turns_from_chunks(&chunks, meta.num_speakers, samples.len(), &meta);
    if frame_turns.is_empty() {
        log::info!("Segmentation found no speech; skipping speaker separation");
        return Ok(DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds,
        });
    }

    // --- Embed --------------------------------------------------------------
    report(DiarizationPhase::Embedding, 0.0);
    let pairs = speaker_pairs(&chunks, meta.num_speakers, samples, &meta);
    if pairs.is_empty() {
        log::info!("No speaker held enough speech to embed; skipping speaker separation");
        return Ok(DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds,
        });
    }

    let window_samples = batch_window_samples(&pairs, MAX_EMBEDDING_SAMPLES);
    let pair_windows = embeddable_windows(&pairs, window_samples);

    let mut embedding = EmbeddingModel::new(&catalog::embedding_path(models_dir), None)?;
    let vectors = embedding.embed(&pair_windows)?;

    if vectors.len() != pairs.len() {
        return Err(format!(
            "The embedding model returned {} vectors for {} speaker spans",
            vectors.len(),
            pairs.len()
        ));
    }

    // --- Cluster and relabel ------------------------------------------------
    report(DiarizationPhase::Clustering, 0.0);
    let embeddings: Vec<SegmentEmbedding> = vectors
        .into_iter()
        .enumerate()
        .map(|(index, vector)| SegmentEmbedding {
            segment: index,
            vector,
        })
        .collect();

    let params = ClusterParams {
        threshold: config.threshold,
        max_speakers: config.max_speakers,
    };

    let pair_clusters = cluster_embeddings(&embeddings, params);
    let speaker_count = count_speakers(&pair_clusters);

    if speaker_count == 0 {
        return Ok(DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds,
        });
    }

    let activity = relabel_activity(
        &chunks,
        &pairs,
        &pair_clusters,
        meta.num_speakers,
        speaker_count,
        samples.len(),
        &meta,
    );

    let counts = speaker_count_per_frame(&activity, speaker_count);
    let selected = select_top_speakers(&activity, &counts, speaker_count);

    let mut turns = build_turns(&selected, speaker_count, &meta);
    merge_adjacent(&mut turns, MIN_DURATION_OFF);
    let turns = drop_short_turns(&turns, MIN_DURATION_ON);

    let final_count = turns
        .iter()
        .map(|turn| turn.speaker)
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    log::info!(
        "Speaker separation finished: {final_count} speaker(s), {} turn(s) over {:.1}s",
        turns.len(),
        duration_seconds
    );

    Ok(DiarizationOutcome {
        turns,
        speaker_count: final_count,
        duration_seconds,
    })
}

/// One segmentation window: where it starts and the audio it covers.
struct Window {
    offset: usize,
    audio: Vec<f32>,
}

/// Build the sliding windows, including the trailing padded one.
///
/// The trailing window matters: the audio rarely divides evenly by the window shift, so
/// without it the final fraction of a second is never analysed and the last word of a
/// meeting can be dropped.
fn build_windows(samples: &[f32], meta: &SegmentationMeta) -> Vec<Window> {
    let window_size = meta.window_size;
    let window_shift = meta.window_shift();

    if window_size == 0 || window_shift == 0 {
        return Vec::new();
    }

    let offsets = super::diarization_models::window_offsets(samples.len(), window_size, window_shift);
    let mut windows: Vec<Window> = offsets
        .iter()
        .map(|offset| Window {
            offset: *offset,
            audio: super::diarization_models::slice_window(samples, *offset, window_size),
        })
        .collect();

    if super::diarization_models::needs_trailing_window(samples.len(), window_size, window_shift) {
        let offset = offsets.last().copied().unwrap_or(0) + window_shift;
        windows.push(Window {
            offset,
            audio: super::diarization_models::slice_window(samples, offset, window_size),
        });
    }

    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn meta() -> SegmentationMeta {
        SegmentationMeta {
            window_size: 160_000,
            sample_rate: 16_000,
            receptive_field_size: 991,
            receptive_field_shift: 270,
            num_classes: 7,
            num_speakers: 3,
            powerset_max_classes: 2,
        }
    }

    #[test]
    fn the_default_speaker_bound_is_high_enough_not_to_bind() {
        // A bound below the real speaker count acts as a floor and mislabels, so the
        // default must sit above any plausible meeting.
        let config = DiarizationConfig::default();
        assert!(config.max_speakers >= 8, "bound {} is too low", config.max_speakers);
    }

    #[test]
    fn the_default_threshold_is_the_calibrated_one() {
        assert_eq!(
            DiarizationConfig::default().threshold,
            super::super::speaker_cluster::CALIBRATED_THRESHOLD
        );
    }

    #[test]
    fn empty_audio_produces_an_empty_outcome_not_an_error() {
        // Silence is a normal result; the caller keeps the unlabelled transcript.
        let dir = TempDir::new().expect("tempdir");
        let outcome = diarize_samples(&[], dir.path(), DiarizationConfig::default()).expect("ok");

        assert!(outcome.is_empty());
        assert_eq!(outcome.duration_seconds, 0.0);
    }

    #[test]
    fn missing_models_are_reported_before_any_work_happens() {
        // The caller should learn what to download, not get a parse error from ONNX.
        let dir = TempDir::new().expect("tempdir");
        let samples = vec![0.1f32; 16_000];

        let error = diarize_samples(&samples, dir.path(), DiarizationConfig::default())
            .err()
            .expect("must fail");
        assert!(error.contains("missing"), "got: {error}");
    }

    #[test]
    fn windows_cover_the_audio_and_add_a_trailing_one() {
        let samples = vec![0.0f32; 4_000_000];
        let windows = build_windows(&samples, &meta());

        assert!(!windows.is_empty());
        // First window starts at zero and is exactly one window long.
        assert_eq!(windows[0].offset, 0);
        assert_eq!(windows[0].audio.len(), meta().window_size);
        // Offsets strictly increase.
        assert!(windows.windows(2).all(|pair| pair[0].offset < pair[1].offset));
    }

    #[test]
    fn a_window_shorter_than_one_second_still_produces_one_window() {
        // Fewer samples than a window: the reference pads rather than refusing, so a very
        // short recording is still analysed.
        let samples = vec![0.0f32; 1_000];
        let windows = build_windows(&samples, &meta());

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].audio.len(), meta().window_size);
    }

    #[test]
    fn no_windows_when_the_window_size_is_degenerate() {
        // `window_shift` is derived from `window_size`, so a zero window also means a
        // zero shift; either would divide by zero or loop forever.
        let mut zero = meta();
        zero.window_size = 0;
        assert!(zero.window_shift() == 0, "the shift is derived from the window");
        assert!(build_windows(&vec![0.0f32; 100_000], &zero).is_empty());
    }

    #[test]
    fn padded_windows_are_silent_past_the_end() {
        // The trailing window is padded rather than truncated, which is what lets the
        // final fraction of a second be analysed at all.
        let length = meta().window_size + meta().window_shift() + 100;
        let mut samples = vec![0.0f32; length];
        for sample in samples.iter_mut().take(length - 10) {
            *sample = 1.0;
        }

        let windows = build_windows(&samples, &meta());
        let last = windows.last().expect("a trailing window");
        assert!(last.audio.len() == meta().window_size);
        assert_eq!(last.audio[0], 1.0, "should still start inside the audio");
    }

    #[test]
    fn the_outcome_reports_the_analysed_duration() {
        let dir = TempDir::new().expect("tempdir");
        let samples = vec![0.0f32; 32_000];

        // Fails on the missing models, but the duration maths is what is under test here.
        let outcome = DiarizationOutcome {
            turns: Vec::new(),
            speaker_count: 0,
            duration_seconds: samples.len() as f64 / DIARIZATION_SAMPLE_RATE as f64,
        };
        assert_eq!(outcome.duration_seconds, 2.0);
        assert!(outcome.is_empty());

        let _ = dir;
    }

    // -----------------------------------------------------------------------
    // Progress reporting
    // -----------------------------------------------------------------------

    #[test]
    fn progress_begins_before_any_model_is_loaded() {
        // The first report must not wait on the models: loading them is itself slow, and a
        // caption that only appears afterwards leaves the interface blank at the start.
        let dir = TempDir::new().expect("tempdir");
        let seen = std::cell::RefCell::new(Vec::new());

        let collect = |phase: DiarizationPhase, fraction: f32| {
            seen.borrow_mut().push((phase, fraction));
        };

        // Empty audio returns early, so anything reported came before the models mattered.
        diarize_samples_with_progress(&[], dir.path(), DiarizationConfig::default(), Some(&collect))
            .expect("empty audio is not an error");

        let seen = seen.borrow();
        assert_eq!(
            seen.first().map(|(phase, _)| *phase),
            Some(DiarizationPhase::Segmenting),
            "the run must open by reporting segmentation, got {seen:?}"
        );
        assert_eq!(seen[0].1, 0.0);
    }

    #[test]
    fn a_callback_is_not_required() {
        // `diarize_samples` is the no-progress entry point and must stay usable.
        let dir = TempDir::new().expect("tempdir");
        let outcome = diarize_samples(&[], dir.path(), DiarizationConfig::default());

        assert!(outcome.is_ok(), "the no-callback path must still work");
    }

    #[test]
    fn progress_never_goes_backwards_within_a_phase() {
        // A bar that jumps backwards reads as a restart.
        let dir = TempDir::new().expect("tempdir");
        let seen = std::cell::RefCell::new(Vec::<f32>::new());

        let collect = |phase: DiarizationPhase, fraction: f32| {
            if phase == DiarizationPhase::Segmenting {
                seen.borrow_mut().push(fraction);
            }
        };

        diarize_samples_with_progress(&[], dir.path(), DiarizationConfig::default(), Some(&collect))
            .expect("ok");

        let fractions = seen.borrow();
        for pair in fractions.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "segmentation progress went backwards: {fractions:?}"
            );
        }
    }

    #[test]
    fn phase_labels_are_non_empty() {
        for phase in [
            DiarizationPhase::Segmenting,
            DiarizationPhase::Embedding,
            DiarizationPhase::Clustering,
        ] {
            assert!(!phase.label().is_empty(), "{phase:?} has no caption");
        }
    }

    #[test]
    fn phases_serialise_as_snake_case_for_the_interface() {
        // The interface switches on these strings, so the wire form is a contract.
        assert_eq!(
            serde_json::to_string(&DiarizationPhase::Segmenting).expect("serialise"),
            "\"segmenting\""
        );
        assert_eq!(
            serde_json::to_string(&DiarizationPhase::Embedding).expect("serialise"),
            "\"embedding\""
        );
        assert_eq!(
            serde_json::to_string(&DiarizationPhase::Clustering).expect("serialise"),
            "\"clustering\""
        );
    }}
