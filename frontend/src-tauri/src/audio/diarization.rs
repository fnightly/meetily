// audio/diarization.rs
//
// Speaker diarization: deciding who spoke when.
//
// Three stages, in order:
//
//   1. **Local segmentation** — a model marks which short local windows contain
//      speech, and whether one or two speakers are active in each.
//   2. **Speaker embedding** — for each speech region, a model produces a vector
//      representing that voice.
//   3. **Clustering** — `speaker_cluster` groups those vectors into speakers.
//
// Stage 3 is `audio/speaker_cluster.rs`. This module owns the arrangement of the
// three stages and the batching, which is the part that has to be right: the
// embedding model is fed *batches* of equal-length regions rather than one region
// at a time, because per-region inference would run the model hundreds of times
// for a meeting and dominate the whole diarisation cost.
//
// The models themselves are reached through [`DiarizationModels`] so this
// arrangement can be tested without an ONNX file, a GPU, or audio.

use super::speaker_turns::SpeakerTurn;

/// Sample rate the segmentation and embedding models expect.
pub const DIARIZATION_SAMPLE_RATE: u32 = 16_000;

/// How far the segmentation window advances, as a fraction of the window size.
///
/// The reference implementation uses 0.1. Shorter is more expensive: the number of
/// model calls is inversely proportional to this.
pub const WINDOW_SHIFT_RATIO: f64 = 0.1;

/// Minimum duration for a speech segment to be reported, in seconds.
pub const MIN_DURATION_ON: f64 = 0.3;

/// Maximum gap between two segments of the same speaker to merge them, in seconds.
pub const MIN_DURATION_OFF: f64 = 0.5;

/// Probability above which a frame counts as speech starting.
pub const ONSET_THRESHOLD: f64 = 0.5;

/// Probability below which a frame counts as speech ending.
pub const OFFSET_THRESHOLD: f64 = 0.5;

/// Frames a (chunk, speaker) pair must have before it is worth embedding.
///
/// The reference skips pairs with fewer than 10 frames — roughly 0.2 seconds —
/// because a very short span gives an unreliable voice vector that pulls clusters
/// together and mislabels speakers elsewhere in the meeting.
pub const MIN_EMBEDDING_FRAMES: usize = 10;

/// Metadata the segmentation model publishes about itself.
///
/// These come from the ONNX file's own metadata rather than being hard-coded: the
/// window size and receptive field differ between exports, and using the wrong
/// values silently produces a timeline that is stretched or offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentationMeta {
    /// Samples in one analysis window.
    pub window_size: usize,
    pub sample_rate: u32,
    /// Samples covered by one output frame.
    pub receptive_field_size: usize,
    /// Samples advanced per output frame.
    pub receptive_field_shift: usize,
    /// Number of entries in the powerset, i.e. the classifier's output classes.
    pub num_classes: usize,
    /// Number of local speakers the model can distinguish within one window.
    pub num_speakers: usize,
    /// Largest number of simultaneous speakers the powerset encodes.
    pub powerset_max_classes: usize,
}

impl SegmentationMeta {
    /// Samples the window advances between consecutive analyses.
    pub fn window_shift(&self) -> usize {
        (self.window_shift_ratio() * self.window_size as f64) as usize
    }

    /// The reference ratio, 0.1.
    pub fn window_shift_ratio(&self) -> f64 {
        WINDOW_SHIFT_RATIO
    }

    /// Seconds one output frame represents.
    pub fn frame_seconds(&self) -> f64 {
        self.receptive_field_shift as f64 / self.sample_rate as f64
    }

    /// Half a receptive field, in seconds.
    ///
    /// Frames are centred on their receptive field, so a segment's time is offset
    /// by half a field. Omitting this shifts every speaker turn earlier.
    pub fn frame_offset_seconds(&self) -> f64 {
        self.receptive_field_size as f64 / self.sample_rate as f64 * 0.5
    }
}

/// Build the powerset mapping from classifier class to the set of speakers it means.
///
/// A powerset classifier does not output "speaker 0" and "speaker 1" separately: it
/// outputs one class per possible *combination* of active speakers, so that a frame
/// with two people talking is a single class rather than two independent ones.
/// Class 0 is the empty set (silence); the single-speaker classes follow, then the
/// pairs.
///
/// Returns a `num_classes x num_speakers` matrix whose row for a class has a 1 for
/// every speaker in that class.
pub fn powerset_mapping(
    num_classes: usize,
    num_speakers: usize,
    powerset_max_classes: usize,
) -> Result<Vec<Vec<u8>>, String> {
    let mut mapping = vec![vec![0u8; num_speakers]; num_classes];
    let mut row = 1usize; // row 0 stays all-zero: no speaker active

    for size in 1..=powerset_max_classes {
        match size {
            1 => {
                for speaker in 0..num_speakers {
                    if row >= num_classes {
                        return Err(format!(
                            "Powerset has {num_classes} classes but needs more for {num_speakers} single speakers"
                        ));
                    }
                    mapping[row][speaker] = 1;
                    row += 1;
                }
            }
            2 => {
                for first in 0..num_speakers {
                    for second in (first + 1)..num_speakers {
                        if row >= num_classes {
                            return Err(format!(
                                "Powerset has {num_classes} classes but needs more for speaker pairs"
                            ));
                        }
                        mapping[row][first] = 1;
                        mapping[row][second] = 1;
                        row += 1;
                    }
                }
            }
            other => {
                // Three or more simultaneous local speakers is not something the
                // published exports encode, and inventing an encoding would produce
                // wrong speaker sets rather than an honest failure.
                return Err(format!(
                    "A powerset with {other} simultaneous speakers is not supported"
                ));
            }
        }
    }

    Ok(mapping)
}

/// Decode one frame's classifier output into per-speaker activity.
///
/// `class_scores` is the model's output for a single frame. Returns a vector with
/// one entry per speaker.
pub fn decode_frame(class_scores: &[f32], mapping: &[Vec<u8>]) -> Vec<u8> {
    let num_speakers = mapping.first().map(Vec::len).unwrap_or(0);
    if num_speakers == 0 || class_scores.is_empty() {
        return vec![0u8; num_speakers];
    }

    // Argmax over classes; ties go to the lowest class index so the result does
    // not depend on iteration order.
    let mut best_class = 0usize;
    let mut best_score = f32::NEG_INFINITY;

    for (index, score) in class_scores.iter().enumerate() {
        if index >= mapping.len() {
            break;
        }
        if *score > best_score {
            best_score = *score;
            best_class = index;
        }
    }

    mapping[best_class].clone()
}

/// Decode a whole chunk of frames.
///
/// `chunk` is row-major `num_frames * num_classes`. Returns row-major
/// `num_frames * num_speakers`.
pub fn decode_chunk(
    chunk: &[f32],
    num_frames: usize,
    num_classes: usize,
    mapping: &[Vec<u8>],
) -> Result<Vec<u8>, String> {
    if num_classes == 0 {
        return Err("The segmentation model reported zero classes".to_string());
    }

    let expected = num_frames * num_classes;
    if chunk.len() != expected {
        return Err(format!(
            "Segmentation output has {} values but {num_frames} frames x {num_classes} classes = {expected} were expected",
            chunk.len()
        ));
    }

    let num_speakers = mapping.first().map(Vec::len).unwrap_or(0);
    let mut result = Vec::with_capacity(num_frames * num_speakers);

    for frame in 0..num_frames {
        let scores = &chunk[frame * num_classes..(frame + 1) * num_classes];
        result.extend(decode_frame(scores, mapping));
    }

    Ok(result)
}

/// Merge adjacent same-speaker segments separated by at most `gap` seconds.
///
/// Repeated until stable, because merging two segments can bring a third within
/// range — the reference does the same.
pub fn merge_adjacent(regions: &mut Vec<SpeakerTurn>, gap: f64) {
    let mut changed = true;

    while changed {
        changed = false;

        let mut index = 0;
        while index + 1 < regions.len() {
            let current = &regions[index];
            let next = &regions[index + 1];

            if current.speaker != next.speaker {
                index += 1;
                continue;
            }

            // Only bridge a gap, not an overlap: overlapping turns from the same
            // speaker are already one span and need no merging.
            let separates = current.end <= next.start && current.end + gap >= next.start;
            let precedes = next.end <= current.start && next.end + gap >= current.start;

            if separates {
                regions[index] = SpeakerTurn::new(current.start, next.end, current.speaker);
                regions.remove(index + 1);
                changed = true;
            } else if precedes {
                regions[index] = SpeakerTurn::new(next.start, current.end, current.speaker);
                regions.remove(index + 1);
                changed = true;
            } else {
                index += 1;
            }
        }
    }
}

/// A `(chunk, local speaker)` pair that has enough speech to embed.
///
/// Clustering runs per pair, not per chunk: one chunk usually contains several local
/// speakers, and each needs its own voice vector before they can be matched across
/// chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerPair {
    pub chunk: usize,
    pub speaker: usize,
    /// The samples belonging to this pair, concatenated across its active frames.
    pub samples: Vec<f32>,
}

/// Samples each frame spans within a chunk.
///
/// A chunk's audio is `window_size` samples split across `num_frames` local frames, so
/// each frame owns `window_size / num_frames` samples. Using the exact ratio rather
/// than the receptive field keeps the concatenation aligned with the audio the chunk
/// analysed.
pub fn samples_per_local_frame(meta: &SegmentationMeta, num_frames: usize) -> usize {
    if num_frames == 0 {
        return 0;
    }
    meta.window_size / num_frames
}

/// Collect the audio for every `(chunk, local speaker)` pair worth embedding.
///
/// Pairs with fewer than `MIN_EMBEDDING_FRAMES` active frames are skipped: a handful
/// of frames is too little voice to characterise a speaker, and including it pulls
/// unrelated speakers into the same cluster.
///
/// `audio` is the full signal the chunks were taken from, so a chunk's frames are read
/// at `sample_offset` plus their own position.
pub fn speaker_pairs(
    chunks: &[ChunkLabels],
    num_speakers: usize,
    audio: &[f32],
    meta: &SegmentationMeta,
) -> Vec<SpeakerPair> {
    if num_speakers == 0 {
        return Vec::new();
    }

    let mut pairs = Vec::new();

    for (chunk_index, chunk) in chunks.iter().enumerate() {
        if chunk.num_frames == 0 {
            continue;
        }

        let frame_samples = samples_per_local_frame(meta, chunk.num_frames);
        if frame_samples == 0 {
            continue;
        }

        for speaker in 0..num_speakers {
            let mut active_frames = 0usize;
            for frame in 0..chunk.num_frames {
                let index = frame * num_speakers + speaker;
                if chunk.labels.get(index).copied().unwrap_or(0) != 0 {
                    active_frames += 1;
                }
            }

            if active_frames < MIN_EMBEDDING_FRAMES {
                continue;
            }

            // Concatenate only the active frames, so the embedding sees speech and
            // little else. Keeping the silent gaps would dilute the voice.
            let mut samples = Vec::with_capacity(active_frames * frame_samples);

            for frame in 0..chunk.num_frames {
                let index = frame * num_speakers + speaker;
                if chunk.labels.get(index).copied().unwrap_or(0) == 0 {
                    continue;
                }

                let start = chunk.sample_offset + frame * frame_samples;
                let end = (start + frame_samples).min(audio.len());
                if start < end {
                    samples.extend_from_slice(&audio[start..end]);
                }
            }

            if samples.is_empty() {
                continue;
            }

            pairs.push(SpeakerPair {
                chunk: chunk_index,
                speaker,
                samples,
            });
        }
    }

    pairs
}

/// Minimum audio handed to the embedding model, in samples (0.5 s at 16 kHz).
///
/// A pair's true speech can be very short — a handful of frames is a fraction of a
/// second — and an embedding from too little audio is unstable. This floor keeps the
/// input meaningful.
pub const MIN_EMBEDDING_SAMPLES: usize = (DIARIZATION_SAMPLE_RATE as usize) / 2;

/// The window length to batch a set of pairs at.
///
/// Padding every pair to a fixed length wastes most of the model's work: a pair
/// spanning ten frames holds under 0.2 s of speech, so padding it to a multiple of
/// that makes the model process many times more audio than exists. Measuring the
/// batch instead keeps the input proportional to the speech.
///
/// Trimmed to the same length, so pairs longer than this keep only their first
/// seconds — acceptable because the embedding is a summary of the voice, and the
/// alternative is padding every other pair up to the longest one.
pub fn batch_window_samples(pairs: &[SpeakerPair], cap: usize) -> usize {
    let longest = pairs
        .iter()
        .map(|pair| pair.samples.len())
        .max()
        .unwrap_or(0);

    longest.clamp(MIN_EMBEDDING_SAMPLES, cap.max(MIN_EMBEDDING_SAMPLES))
}

/// Pad or trim each pair's audio to `length` samples so it can be batched.
///
/// Padding with silence adds no voice; dropping a short pair would lose a speaker.
pub fn embeddable_windows(pairs: &[SpeakerPair], length: usize) -> Vec<Vec<f32>> {
    pairs
        .iter()
        .map(|pair| {
            let mut window = vec![0.0f32; length];
            let take = pair.samples.len().min(length);
            window[..take].copy_from_slice(&pair.samples[..take]);
            window
        })
        .collect()
}

/// Assign each pair's cluster label to every frame where that pair is active, then
/// produce global speaker turns.
///
/// A chunk's local speaker is numbered within that chunk only: local speaker 1 in one
/// chunk need not be the same person as local speaker 1 in the next. The cluster label
/// is what carries across chunks, which is why the relabelling step exists.
///
/// Returns per-frame activity on a global speaker axis, row-major
/// `global_frames * num_global_speakers`.
pub fn relabel_activity(
    chunks: &[ChunkLabels],
    pairs: &[SpeakerPair],
    pair_clusters: &[usize],
    num_local_speakers: usize,
    num_global_speakers: usize,
    num_samples: usize,
    meta: &SegmentationMeta,
) -> Vec<f32> {
    let shift = meta.receptive_field_shift;
    if shift == 0 || num_global_speakers == 0 {
        return Vec::new();
    }

    let num_frames = num_samples / shift + 1;
    let mut activity = vec![0.0f32; num_frames * num_global_speakers];

    for (index, pair) in pairs.iter().enumerate() {
        let Some(cluster) = pair_clusters.get(index).copied() else {
            continue;
        };
        if cluster >= num_global_speakers {
            continue;
        }

        let Some(chunk) = chunks.get(pair.chunk) else {
            continue;
        };
        if chunk.num_frames == 0 {
            continue;
        }

        // Same rounding as the chunk-weighting step, so frames line up between the two.
        let start = (chunk.sample_offset as f64 / shift as f64 + 0.5) as usize;

        for frame in 0..chunk.num_frames {
            let label = chunk
                .labels
                .get(frame * num_local_speakers + pair.speaker)
                .copied()
                .unwrap_or(0);
            if label == 0 {
                continue;
            }

            let global = start + frame;
            if global < num_frames {
                activity[global * num_global_speakers + cluster] += 1.0;
            }
        }
    }

    activity
}

/// Speaker activity for one chunk, as returned by the segmentation model.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkLabels {
    /// How far into the audio this chunk's window starts, in samples.
    pub sample_offset: usize,
    /// Row-major `num_frames * num_speakers`, values 0 or 1.
    pub labels: Vec<u8>,
    pub num_frames: usize,
}

/// Project each chunk's frames onto one global frame axis, averaging overlaps.
///
/// Chunks overlap — the window advances by less than its length — so a given frame
/// is covered by several chunks. Averaging rather than overwriting is what makes the
/// result independent of chunk order, and the reference divides by the same coverage
/// count.
///
/// The global frame count follows the reference: the whole audio, stepped by the
/// receptive field shift.
pub fn weighted_speaker_scores(
    chunks: &[ChunkLabels],
    num_speakers: usize,
    num_samples: usize,
    meta: &SegmentationMeta,
) -> Vec<f32> {
    if chunks.is_empty() || num_speakers == 0 {
        return Vec::new();
    }

    let shift = meta.receptive_field_shift;
    if shift == 0 {
        return Vec::new();
    }

    let num_frames = num_samples / shift + 1;

    let mut totals = vec![0.0f32; num_frames * num_speakers];
    let mut coverage = vec![0.0f32; num_frames];

    for chunk in chunks {
        if chunk.num_frames == 0 {
            continue;
        }

        // Where this chunk's first frame lands on the global axis. The +0.5 rounds
        // to nearest, matching the reference, so a chunk does not drift by a frame
        // as the offsets grow.
        let start =
            (chunk.sample_offset as f64 / shift as f64 + 0.5) as usize;

        for frame in 0..chunk.num_frames {
            let global = start + frame;
            if global >= num_frames {
                break;
            }

            let base = frame * num_speakers;
            if base + num_speakers > chunk.labels.len() {
                break;
            }

            for speaker in 0..num_speakers {
                totals[global * num_speakers + speaker] +=
                    chunk.labels[base + speaker] as f32;
            }
            coverage[global] += 1.0;
        }
    }

    for (index, total) in totals.iter_mut().enumerate() {
        let frame = index / num_speakers;
        *total /= coverage[frame].max(1e-12);
    }

    totals
}

/// Round each frame's averaged scores into a speaker count.
///
/// The reference adds 0.5 and truncates, which is rounding to nearest. A frame with
/// two speakers half the time therefore counts as one speaker, not two: the result
/// describes the typical number of simultaneous speakers, not the maximum.
pub fn speaker_count_per_frame(scores: &[f32], num_speakers: usize) -> Vec<u8> {
    if num_speakers == 0 {
        return Vec::new();
    }

    scores
        .chunks(num_speakers)
        .map(|frame| {
            let total: f32 = frame.iter().sum();
            (total + 0.5).clamp(0.0, num_speakers as f32) as u8
        })
        .collect()
}

/// Keep the `count` highest-scoring speakers in each frame.
///
/// The averaged scores are fractional and can leave more speakers above the onset
/// threshold than the frame's speaker count allows. Taking the top `count` keeps the
/// result consistent with "this many people were speaking" and stops a frame with one
/// dominant speaker from also lighting up a weakly-supported second one.
pub fn select_top_speakers(
    scores: &[f32],
    speaker_counts: &[u8],
    num_speakers: usize,
) -> Vec<f32> {
    if num_speakers == 0 {
        return Vec::new();
    }

    let num_frames = scores.len() / num_speakers;
    let mut selected = vec![0.0f32; scores.len()];

    for frame in 0..num_frames {
        let count = speaker_counts.get(frame).copied().unwrap_or(0) as usize;
        if count == 0 {
            continue;
        }

        let base = frame * num_speakers;
        let mut order: Vec<usize> = (0..num_speakers).collect();
        order.sort_by(|a, b| {
            scores[base + *b]
                .partial_cmp(&scores[base + *a])
                .unwrap_or(std::cmp::Ordering::Equal)
                // Ties resolve to the lower speaker index so the output does not
                // depend on sort stability.
                .then_with(|| a.cmp(b))
        });

        for speaker in order.into_iter().take(count.min(num_speakers)) {
            selected[base + speaker] = scores[base + speaker];
        }
    }

    selected
}

/// Build speaker turns from per-frame multi-speaker activity.
///
/// `activity` is row-major `num_frames * num_speakers` of averaged scores. Each
/// speaker's frame track is walked to find runs above the onset threshold and closed
/// below the offset threshold, then the run boundaries are converted to seconds.
///
/// The half-receptive-field offset is added because frames are centred on their
/// receptive field; omitting it shifts every turn earlier.
pub fn build_turns(
    activity: &[f32],
    num_speakers: usize,
    meta: &SegmentationMeta,
) -> Vec<SpeakerTurn> {
    if num_speakers == 0 || activity.is_empty() {
        return Vec::new();
    }

    let num_frames = activity.len() / num_speakers;
    if num_frames == 0 {
        return Vec::new();
    }

    let scale = meta.frame_seconds();
    let offset = meta.frame_offset_seconds();

    let mut turns = Vec::new();

    for speaker in 0..num_speakers {
        let value = |frame: usize| activity[frame * num_speakers + speaker];

        let mut active = value(0) > ONSET_THRESHOLD as f32;
        let mut start = 0usize;

        for frame in 1..num_frames {
            if active {
                if value(frame) < OFFSET_THRESHOLD as f32 {
                    turns.push(SpeakerTurn::new(
                        start as f64 * scale + offset,
                        frame as f64 * scale + offset,
                        speaker as u32,
                    ));
                    active = false;
                }
            } else if value(frame) > ONSET_THRESHOLD as f32 {
                start = frame;
                active = true;
            }
        }

        // A speaker still active at the end runs to the final frame.
        if active {
            turns.push(SpeakerTurn::new(
                start as f64 * scale + offset,
                (num_frames - 1) as f64 * scale + offset,
                speaker as u32,
            ));
        }
    }

    turns.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.speaker.cmp(&b.speaker))
    });

    turns
}

/// The full post-processing chain: average chunks, round, keep the top speakers,
/// build runs, merge, and drop noise.
pub fn turns_from_chunks(
    chunks: &[ChunkLabels],
    num_speakers: usize,
    num_samples: usize,
    meta: &SegmentationMeta,
) -> Vec<SpeakerTurn> {
    if num_speakers == 0 || chunks.is_empty() {
        return Vec::new();
    }

    let averaged = weighted_speaker_scores(chunks, num_speakers, num_samples, meta);
    if averaged.is_empty() {
        return Vec::new();
    }

    let counts = speaker_count_per_frame(&averaged, num_speakers);
    let selected = select_top_speakers(&averaged, &counts, num_speakers);

    let mut turns = build_turns(&selected, num_speakers, meta);
    merge_adjacent(&mut turns, MIN_DURATION_OFF);
    let turns = drop_short_turns(&turns, MIN_DURATION_ON);

    log::info!(
        "Diarization post-processing: {} frame(s) -> {} turn(s) after merging",
        counts.len(),
        turns.len()
    );

    turns
}

/// Drop turns shorter than `min_duration` seconds.
pub fn drop_short_turns(regions: &[SpeakerTurn], min_duration: f64) -> Vec<SpeakerTurn> {
    regions
        .iter()
        .filter(|turn| turn.duration() >= min_duration)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speaker: u32) -> SpeakerTurn {
        SpeakerTurn::new(start, end, speaker)
    }

    fn meta() -> SegmentationMeta {
        // Values in the shape the published pyannote 3.0 export uses.
        SegmentationMeta {
            window_size: 160_000,
            sample_rate: 16_000,
            receptive_field_size: 3_200,
            receptive_field_shift: 160,
            num_classes: 7,
            num_speakers: 3,
            powerset_max_classes: 2,
        }
    }

    #[test]
    fn window_shift_is_a_tenth_of_the_window() {
        assert_eq!(meta().window_shift(), 16_000);
    }

    #[test]
    fn frame_timing_comes_from_the_receptive_field() {
        // 160 samples at 16 kHz is 10 ms per frame.
        assert!((meta().frame_seconds() - 0.01).abs() < 1e-9);
        // Half of a 3200-sample field is 100 ms.
        assert!((meta().frame_offset_seconds() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn powerset_mapping_covers_silence_singles_and_pairs() {
        // 1 empty + 3 singles + 3 pairs = 7 classes, which is what the export has.
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        assert_eq!(mapping.len(), 7);
        assert_eq!(mapping[0], vec![0, 0, 0], "class 0 is silence");
        assert_eq!(mapping[1], vec![1, 0, 0]);
        assert_eq!(mapping[2], vec![0, 1, 0]);
        assert_eq!(mapping[3], vec![0, 0, 1]);
        assert_eq!(mapping[4], vec![1, 1, 0], "pairs follow the singles");
        assert_eq!(mapping[5], vec![1, 0, 1]);
        assert_eq!(mapping[6], vec![0, 1, 1]);
    }

    #[test]
    fn powerset_mapping_row_count_matches_the_classifier() {
        // Classes must equal 1 + s + s(s-1)/2: silence, each single speaker, each
        // unordered pair. 2 speakers -> 4, 3 -> 7, 4 -> 11.
        for (classes, speakers) in [(4usize, 2usize), (7, 3), (11, 4)] {
            let mapping = powerset_mapping(classes, speakers, 2).expect("mapping");
            assert_eq!(mapping.len(), classes);
            for row in &mapping {
                assert_eq!(row.len(), speakers);
            }
        }
    }

    #[test]
    fn powerset_mapping_rejects_a_class_count_that_cannot_fit_the_speakers() {
        // 4 classes cannot encode 3 speakers (that needs 7), so this must fail
        // rather than produce a mapping with unreachable speakers.
        assert!(powerset_mapping(4, 3, 2).is_err());
    }

    #[test]
    fn a_powerset_with_three_way_overlap_is_refused() {
        // Better an honest error than a wrong speaker set.
        let error = powerset_mapping(20, 4, 3).err().expect("must fail");
        assert!(error.contains("not supported"), "got: {error}");
    }

    #[test]
    fn too_few_classes_for_the_speakers_is_refused() {
        let error = powerset_mapping(2, 3, 2).err().expect("must fail");
        assert!(error.contains("classes"), "got: {error}");
    }

    #[test]
    fn decode_frame_picks_the_highest_scoring_class() {
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");

        // Highest score on class 4, a pair.
        let scores = [-1.0, -2.0, -3.0, -4.0, 5.0, -6.0, -7.0];
        assert_eq!(decode_frame(&scores, &mapping), vec![1, 1, 0]);

        // Highest score on class 0, silence.
        let silent = [9.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(decode_frame(&silent, &mapping), vec![0, 0, 0]);
    }

    #[test]
    fn decode_frame_breaks_ties_towards_the_lower_class() {
        // Deterministic regardless of score ordering.
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        let scores = [0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(decode_frame(&scores, &mapping), vec![0, 0, 0]);
    }

    #[test]
    fn decode_frame_ignores_scores_past_the_mapping() {
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        let scores = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 99.0];
        // The extra column must not be selected.
        assert_eq!(decode_frame(&scores, &mapping), vec![0, 0, 0]);
    }

    #[test]
    fn decode_chunk_flattens_frames_in_order() {
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        // Two frames: class 1 (speaker 0), then class 2 (speaker 1).
        let mut chunk = vec![0.0f32; 2 * 7];
        chunk[1] = 1.0;
        chunk[7 + 2] = 1.0;

        let decoded = decode_chunk(&chunk, 2, 7, &mapping).expect("decode");
        assert_eq!(decoded, vec![1, 0, 0, 0, 1, 0]);
    }

    #[test]
    fn decode_chunk_rejects_a_size_mismatch() {
        // A mismatch means the frames and classes were misread, which would shift
        // every speaker in time.
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        let error = decode_chunk(&[0.0; 10], 2, 7, &mapping).err().expect("must fail");
        assert!(error.contains("expected"), "got: {error}");
    }

    #[test]
    fn decode_chunk_rejects_zero_classes() {
        let mapping = powerset_mapping(7, 3, 2).expect("mapping");
        assert!(decode_chunk(&[], 0, 0, &mapping).is_err());
    }

    #[test]
    fn merge_joins_same_speaker_segments_across_a_short_gap() {
        let mut regions = vec![turn(0.0, 1.0, 0), turn(1.3, 2.0, 0)];
        merge_adjacent(&mut regions, MIN_DURATION_OFF);

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 0.0);
        assert_eq!(regions[0].end, 2.0);
    }

    #[test]
    fn merge_leaves_a_long_gap_alone() {
        // Beyond the gap these are two separate utterances.
        let mut regions = vec![turn(0.0, 1.0, 0), turn(5.0, 6.0, 0)];
        merge_adjacent(&mut regions, MIN_DURATION_OFF);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn merge_never_joins_different_speakers() {
        let mut regions = vec![turn(0.0, 1.0, 0), turn(1.1, 2.0, 1)];
        merge_adjacent(&mut regions, MIN_DURATION_OFF);
        assert_eq!(regions.len(), 2, "different speakers must stay separate");
    }

    #[test]
    fn merge_cascades_until_stable() {
        // Merging 1 and 2 brings 3 within range, so a single pass is not enough.
        let mut regions = vec![
            turn(0.0, 1.0, 0),
            turn(1.4, 2.0, 0),
            turn(2.4, 3.0, 0),
            turn(3.4, 4.0, 0),
        ];
        merge_adjacent(&mut regions, MIN_DURATION_OFF);

        assert_eq!(regions.len(), 1, "expected one merged span, got {regions:?}");
        assert_eq!(regions[0].end, 4.0);
    }

    #[test]
    fn merge_on_an_empty_or_single_list_is_a_no_op() {
        let mut empty: Vec<SpeakerTurn> = vec![];
        merge_adjacent(&mut empty, MIN_DURATION_OFF);
        assert!(empty.is_empty());

        let mut single = vec![turn(0.0, 1.0, 0)];
        merge_adjacent(&mut single, MIN_DURATION_OFF);
        assert_eq!(single.len(), 1);
    }

    #[test]
    fn drop_short_turns_removes_noise_segments() {
        let regions = vec![turn(0.0, 0.2, 0), turn(0.5, 2.0, 1)];
        let kept = drop_short_turns(&regions, MIN_DURATION_ON);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].speaker, 1);
    }

    #[test]
    fn drop_short_turns_keeps_a_turn_exactly_at_the_minimum() {
        let regions = vec![turn(0.0, MIN_DURATION_ON, 0)];
        assert_eq!(drop_short_turns(&regions, MIN_DURATION_ON).len(), 1);
    }

    #[test]
    fn the_frame_offset_is_positive_and_shifts_times_later() {
        // A forgotten offset would move every speaker turn earlier by half a field.
        let m = meta();
        assert!(m.frame_offset_seconds() > 0.0);
    }
}


// ---------------------------------------------------------------------------
// Post-processing tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod postprocess_tests {
    use super::*;

    fn meta() -> SegmentationMeta {
        // The values the real pyannote 3.0 export publishes.
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

    /// One chunk with frames given as `[(speaker, ...)]`.
    fn chunk(sample_offset: usize, frames: &[Vec<u8>], num_speakers: usize) -> ChunkLabels {
        let mut labels = Vec::new();
        for frame in frames {
            // Always emit exactly num_speakers values, even for a frame that lists no
            // speakers. A shorter row would be read past the end of by the code under
            // test, and the missing entries would look like inactive speakers by
            // accident rather than by construction.
            let mut row = vec![0u8; num_speakers];
            for speaker in frame {
                if (*speaker as usize) < num_speakers {
                    row[*speaker as usize] = 1;
                }
            }
            labels.extend_from_slice(&row);
        }

        ChunkLabels {
            sample_offset,
            num_frames: frames.len(),
            labels,
        }
    }

    #[test]
    fn no_chunks_produce_no_scores() {
        assert!(weighted_speaker_scores(&[], 3, 16_000, &meta()).is_empty());
    }

    #[test]
    fn a_single_chunk_maps_onto_the_global_axis_at_its_offset() {
        // Two frames of speaker 0 starting at sample 0.
        let chunks = vec![chunk(0, &[vec![0], vec![0]], 1)];
        let scores = weighted_speaker_scores(&chunks, 1, 16_000, &meta());

        // Frames land at global indices 0 and 1 (offset/shift + 0.5 = 0).
        assert_eq!(scores[0], 1.0);
        assert_eq!(scores[1], 1.0);
    }

    #[test]
    fn a_chunk_is_placed_according_to_its_sample_offset() {
        // Offset 270 samples = one receptive field shift = global frame 1.
        let chunks = vec![chunk(270, &[vec![0]], 1)];
        let scores = weighted_speaker_scores(&chunks, 1, 16_000, &meta());

        assert_eq!(scores[0], 0.0, "frame 0 must stay empty");
        assert_eq!(scores[1], 1.0, "the chunk belongs at frame 1");
    }

    #[test]
    fn overlapping_chunks_are_averaged_not_overwritten() {
        // Two chunks both claim global frame 0 for speaker 0 and speaker 1.
        // Averaging gives 0.5 each; overwriting would give 1.0 to whichever came last.
        let chunks = vec![
            chunk(0, &[vec![0]], 2),
            chunk(0, &[vec![1]], 2),
        ];
        let scores = weighted_speaker_scores(&chunks, 2, 16_000, &meta());

        assert!((scores[0] - 0.5).abs() < 1e-6, "speaker 0: {}", scores[0]);
        assert!((scores[1] - 0.5).abs() < 1e-6, "speaker 1: {}", scores[1]);
    }

    #[test]
    fn coverage_is_per_frame_not_per_speaker() {
        // Frame 0 is covered twice, frame 1 once. Frame 1 must not be diluted.
        let chunks = vec![
            chunk(0, &[vec![0], vec![0]], 1),
            chunk(0, &[vec![0]], 1),
        ];
        let scores = weighted_speaker_scores(&chunks, 1, 16_000, &meta());

        // Frame 0: (1 + 1) / 2 = 1.0. Frame 1: 1 / 1 = 1.0.
        assert_eq!(scores[0], 1.0);
        assert_eq!(scores[1], 1.0);
    }

    #[test]
    fn a_chunk_running_past_the_audio_is_truncated() {
        // The global axis must not grow to fit a chunk that overshoots.
        let target = meta();
        let num_samples = 16_000usize;
        let expected_frames = num_samples / target.receptive_field_shift + 1;

        let long = vec![vec![0u8]; 5_000];
        let chunks = vec![chunk(0, &long, 1)];
        let scores = weighted_speaker_scores(&chunks, 1, num_samples, &target);

        assert_eq!(scores.len(), expected_frames);
    }

    #[test]
    fn a_zero_shift_yields_no_scores_rather_than_hanging() {
        let mut target = meta();
        target.receptive_field_shift = 0;
        assert!(weighted_speaker_scores(&[chunk(0, &[vec![0]], 1)], 1, 16_000, &target).is_empty());
    }

    #[test]
    fn frame_counts_round_to_nearest() {
        // 1.4 speakers -> 1; 1.6 -> 2. This is what stops a frame briefly touched by
        // a second voice from being treated as two speakers throughout.
        let scores = vec![1.0, 0.4, 1.0, 0.6];
        let counts = speaker_count_per_frame(&scores, 2);
        assert_eq!(counts, vec![1, 2]);
    }

    #[test]
    fn frame_counts_are_clamped_to_the_speaker_total() {
        let scores = vec![1.0, 1.0, 1.0, 1.0];
        let counts = speaker_count_per_frame(&scores, 2);
        assert_eq!(counts, vec![2, 2]);
    }

    #[test]
    fn zero_scores_count_as_silence() {
        let scores = vec![0.0, 0.0, 0.0, 0.0];
        assert_eq!(speaker_count_per_frame(&scores, 2), vec![0, 0]);
    }

    #[test]
    fn top_speakers_keeps_only_the_requested_count() {
        // Frame 0: speaker 0 scores higher, count 1 -> speaker 1 must be cleared even
        // though it is above the onset threshold.
        let scores = vec![0.9, 0.8];
        let counts = vec![1u8];
        let selected = select_top_speakers(&scores, &counts, 2);

        assert_eq!(selected[0], 0.9);
        assert_eq!(selected[1], 0.0, "the weaker speaker must be dropped");
    }

    #[test]
    fn top_speakers_keeps_the_genuinely_highest() {
        let scores = vec![0.3, 0.95];
        let counts = vec![1u8];
        let selected = select_top_speakers(&scores, &counts, 2);

        assert_eq!(selected[0], 0.0);
        assert_eq!(selected[1], 0.95);
    }

    #[test]
    fn a_frame_with_no_speakers_selects_nothing() {
        let selected = select_top_speakers(&[1.0, 1.0], &[0u8], 2);
        assert!(selected.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn ties_in_selection_resolve_to_the_lower_speaker() {
        let selected = select_top_speakers(&[0.7, 0.7], &[1u8], 2);
        assert_eq!(selected, vec![0.7, 0.0]);
    }

    #[test]
    fn selection_preserves_the_scores_it_keeps() {
        // The value matters: `build_turns` compares it against the onset threshold.
        let selected = select_top_speakers(&[0.4, 0.9], &[2u8], 2);
        assert_eq!(selected, vec![0.4, 0.9]);
    }

    #[test]
    fn a_continuous_run_becomes_one_turn() {
        // Speaker 0 active across four frames, others silent.
        let mut activity = Vec::new();
        for _ in 0..4 {
            activity.extend_from_slice(&[1.0, 0.0]);
        }

        let turns = build_turns(&activity, 2, &meta());
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].speaker, 0);
    }

    #[test]
    fn turn_times_include_the_half_receptive_field_offset() {
        let mut activity = Vec::new();
        for _ in 0..4 {
            activity.extend_from_slice(&[1.0, 0.0]);
        }

        let target = meta();
        let turns = build_turns(&activity, 2, &target);

        let scale = target.frame_seconds();
        let offset = target.frame_offset_seconds();
        assert!((turns[0].start - offset).abs() < 1e-9, "start must be offset");
        assert!(
            (turns[0].end - (3.0 * scale + offset)).abs() < 1e-9,
            "end must be frame 3 plus offset"
        );
    }

    #[test]
    fn two_speakers_produce_separate_turns() {
        // 3 frames: speaker 0, speaker 1, speaker 0.
        let activity = vec![1.0, 0.0, 0.0, 1.0, 1.0, 0.0];
        let turns = build_turns(&activity, 2, &meta());

        assert_eq!(turns.len(), 3);
        // Sorted by start; the two speaker-0 runs are separate because speaker 1
        // interrupted.
        assert_eq!(turns[0].speaker, 0);
        assert_eq!(turns[1].speaker, 1);
        assert_eq!(turns[2].speaker, 0);
    }

    #[test]
    fn silence_produces_no_turns() {
        let activity = vec![0.0f32; 20];
        assert!(build_turns(&activity, 2, &meta()).is_empty());
    }

    #[test]
    fn a_speaker_active_at_the_end_is_closed_at_the_last_frame() {
        // Two frames, the second active: the run has no closing frame, so it ends at
        // the final frame index (1), not past the audio.
        let activity = vec![0.0, 0.0, 1.0, 0.0];
        let turns = build_turns(&activity, 2, &meta());

        assert_eq!(turns.len(), 1);
        let target = meta();
        let expected_end = 1.0 * target.frame_seconds() + target.frame_offset_seconds();
        assert!(
            (turns[0].end - expected_end).abs() < 1e-9,
            "expected {expected_end}, got {}",
            turns[0].end
        );
    }

    #[test]
    fn build_turns_orders_by_start_time() {
        // Speaker 1 starts before speaker 0 here, so the output must be reordered.
        let activity = vec![0.0, 1.0, 1.0, 1.0];
        let turns = build_turns(&activity, 2, &meta());

        assert!(turns.len() >= 2);
        assert!(turns.windows(2).all(|pair| pair[0].start <= pair[1].start));
    }

    #[test]
    fn building_turns_from_no_frames_is_empty() {
        assert!(build_turns(&[], 2, &meta()).is_empty());
        assert!(build_turns(&[1.0, 0.0], 0, &meta()).is_empty());
    }

    #[test]
    fn the_full_chain_produces_plausible_turns() {
        // One chunk: speaker 0 for 60 frames, silence for 30, speaker 1 for 60.
        let mut frames = Vec::new();
        for _ in 0..60 {
            frames.push(vec![0u8]);
        }
        for _ in 0..30 {
            frames.push(vec![]);
        }
        for _ in 0..60 {
            frames.push(vec![1u8]);
        }

        let chunks = vec![chunk(0, &frames, 3)];
        // Enough audio that the global axis covers the chunk.
        let num_samples = (frames.len() + 4) * meta().receptive_field_shift;

        let turns = turns_from_chunks(&chunks, 3, num_samples, &meta());

        assert!(turns.len() >= 2, "expected two speaker turns, got {turns:?}");
        let speakers: Vec<u32> = turns.iter().map(|turn| turn.speaker).collect();
        assert!(speakers.contains(&0), "speaker 0 missing: {turns:?}");
        assert!(speakers.contains(&1), "speaker 1 missing: {turns:?}");
    }

    #[test]
    fn the_full_chain_on_silence_produces_nothing() {
        let silence: Vec<Vec<u8>> = (0..100).map(|_| vec![]).collect();
        let chunks = vec![chunk(0, &silence, 3)];
        let num_samples = 100 * meta().receptive_field_shift;

        assert!(turns_from_chunks(&chunks, 3, num_samples, &meta()).is_empty());
    }

    #[test]
    fn the_full_chain_drops_a_single_frame_blip() {
        // One frame of speech is far below MIN_DURATION_ON and should not survive.
        let mut frames: Vec<Vec<u8>> = (0..100).map(|_| vec![]).collect();
        frames[50] = vec![0u8];

        let chunks = vec![chunk(0, &frames, 3)];
        let num_samples = 100 * meta().receptive_field_shift;

        assert!(
            turns_from_chunks(&chunks, 3, num_samples, &meta()).is_empty(),
            "a one-frame blip must be filtered out"
        );
    }

    #[test]
    fn the_full_chain_is_deterministic() {
        let mut frames = Vec::new();
        for index in 0..120 {
            frames.push(vec![(index / 40) as u8]);
        }

        let chunks = vec![chunk(0, &frames, 3)];
        let num_samples = 130 * meta().receptive_field_shift;

        let first = turns_from_chunks(&chunks, 3, num_samples, &meta());
        let second = turns_from_chunks(&chunks, 3, num_samples, &meta());

        assert_eq!(first, second);
    }
}
// ---------------------------------------------------------------------------
// Speaker-pair extraction tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod speaker_pair_tests {
    use super::*;

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

    fn chunk(sample_offset: usize, frames: &[Vec<u8>], num_speakers: usize) -> ChunkLabels {
        let mut labels = Vec::new();
        for frame in frames {
            // Always emit exactly num_speakers values, even for a frame that lists no
            // speakers. A shorter row would be read past the end of by the code under
            // test, and the missing entries would look like inactive speakers by
            // accident rather than by construction.
            let mut row = vec![0u8; num_speakers];
            for speaker in frame {
                if (*speaker as usize) < num_speakers {
                    row[*speaker as usize] = 1;
                }
            }
            labels.extend_from_slice(&row);
        }

        ChunkLabels {
            sample_offset,
            num_frames: frames.len(),
            labels,
        }
    }

    /// Audio where every sample encodes its own index, so a slice can be identified.
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|index| index as f32).collect()
    }

    #[test]
    fn samples_per_frame_is_the_window_split_evenly() {
        // 160000 samples over 400 frames is 400 samples each.
        assert_eq!(samples_per_local_frame(&meta(), 400), 400);
    }

    #[test]
    fn samples_per_frame_of_no_frames_is_zero() {
        assert_eq!(samples_per_local_frame(&meta(), 0), 0);
    }

    #[test]
    fn no_chunks_produce_no_pairs() {
        assert!(speaker_pairs(&[], 3, &ramp(1000), &meta()).is_empty());
    }

    #[test]
    fn a_short_pair_is_skipped() {
        // Four active frames is below MIN_EMBEDDING_FRAMES, so the voice cannot be
        // characterised and including it would distort clustering.
        let frames: Vec<Vec<u8>> = (0..4).map(|_| vec![0u8]).collect();
        let chunks = vec![chunk(0, &frames, 3)];

        assert!(speaker_pairs(&chunks, 3, &ramp(100_000), &meta()).is_empty());
    }

    #[test]
    fn a_long_enough_pair_is_kept() {
        let frames: Vec<Vec<u8>> = (0..MIN_EMBEDDING_FRAMES).map(|_| vec![0u8]).collect();
        let chunks = vec![chunk(0, &frames, 3)];

        let pairs = speaker_pairs(&chunks, 3, &ramp(100_000), &meta());
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].chunk, 0);
        assert_eq!(pairs[0].speaker, 0);
    }

    #[test]
    fn only_speakers_with_enough_frames_are_kept() {
        // Speaker 0 speaks for 12 frames, speaker 1 for 3.
        let mut frames: Vec<Vec<u8>> = (0..12).map(|_| vec![0u8]).collect();
        for frame in frames.iter_mut().take(3) {
            frame.push(1);
        }

        let chunks = vec![chunk(0, &frames, 3)];
        let pairs = speaker_pairs(&chunks, 3, &ramp(100_000), &meta());

        let speakers: Vec<usize> = pairs.iter().map(|pair| pair.speaker).collect();
        assert_eq!(speakers, vec![0], "speaker 1 has too few frames");
    }

    #[test]
    fn a_pair_covers_the_frames_of_one_local_speaker_only() {
        // Speaker 0 active on frames 0 and 1; frame_samples is 400 at 400 frames.
        let mut frames: Vec<Vec<u8>> = Vec::new();
        for _ in 0..MIN_EMBEDDING_FRAMES {
            frames.push(vec![0u8]);
        }

        let chunks = vec![chunk(0, &frames, 3)];
        // The buffer must cover the whole window, otherwise the read is clipped and
        // the length assertion below measures the clip rather than the logic.
        let audio = ramp(200_000);
        let pairs = speaker_pairs(&chunks, 3, &audio, &meta());

        assert_eq!(pairs.len(), 1);
        let frame_samples = samples_per_local_frame(&meta(), MIN_EMBEDDING_FRAMES);
        assert_eq!(pairs[0].samples.len(), MIN_EMBEDDING_FRAMES * frame_samples);
        // The pair starts at the beginning of the chunk's audio.
        assert_eq!(pairs[0].samples[0], audio[0]);
    }

    #[test]
    fn a_pair_respects_the_chunk_sample_offset() {
        let frames: Vec<Vec<u8>> = (0..MIN_EMBEDDING_FRAMES).map(|_| vec![0u8]).collect();
        let audio = ramp(200_000);
        let chunks = vec![chunk(8_000, &frames, 3)];

        let pairs = speaker_pairs(&chunks, 3, &audio, &meta());
        assert_eq!(pairs[0].samples[0], audio[8_000]);
    }

    #[test]
    fn silently_skipped_frames_are_not_concatenated() {
        // One more frame than the minimum, with one of them silent: the pair still has
        // enough active frames to be embedded, and the silent frame must contribute no
        // audio or the embedding is diluted with silence.
        let total_frames = MIN_EMBEDDING_FRAMES + 1;
        let mut frames: Vec<Vec<u8>> = (0..total_frames).map(|_| vec![0u8]).collect();
        frames[5] = vec![];

        let chunks = vec![chunk(0, &frames, 3)];
        let audio = ramp(200_000);
        let pairs = speaker_pairs(&chunks, 3, &audio, &meta());

        assert_eq!(pairs.len(), 1);
        let frame_samples = samples_per_local_frame(&meta(), total_frames);
        assert_eq!(
            pairs[0].samples.len(),
            MIN_EMBEDDING_FRAMES * frame_samples,
            "the silent frame must be skipped"
        );
    }

    #[test]
    fn a_chunk_running_past_the_audio_is_clipped() {
        // Reading past the buffer must not panic.
        let frames: Vec<Vec<u8>> = (0..MIN_EMBEDDING_FRAMES).map(|_| vec![0u8]).collect();
        let chunks = vec![chunk(0, &frames, 3)];
        let audio = ramp(10);

        let pairs = speaker_pairs(&chunks, 3, &audio, &meta());
        // Either nothing survives or what does is within the buffer.
        for pair in &pairs {
            assert!(pair.samples.len() <= audio.len());
        }
    }

    #[test]
    fn zero_speakers_produce_no_pairs() {
        let frames: Vec<Vec<u8>> = (0..MIN_EMBEDDING_FRAMES).map(|_| vec![0u8]).collect();
        let chunks = vec![chunk(0, &frames, 3)];
        assert!(speaker_pairs(&chunks, 0, &ramp(100_000), &meta()).is_empty());
    }

    #[test]
    fn windows_are_padded_to_one_length() {
        // Batching needs equal lengths; padding with silence adds no voice.
        let pairs = vec![
            SpeakerPair { chunk: 0, speaker: 0, samples: vec![1.0; 5] },
            SpeakerPair { chunk: 0, speaker: 1, samples: vec![2.0; 3] },
        ];

        let windows = embeddable_windows(&pairs, 8);
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().all(|window| window.len() == 8));

        assert!(windows[1][3..].iter().all(|value| *value == 0.0));
    }

    #[test]
    fn long_window_is_trimmed_not_padded() {
        let pairs = vec![SpeakerPair { chunk: 0, speaker: 0, samples: vec![7.0; 100] }];
        let windows = embeddable_windows(&pairs, 10);

        assert_eq!(windows[0].len(), 10);
        assert!(windows[0].iter().all(|value| *value == 7.0));
    }

    #[test]
    fn embedding_no_pairs_yields_no_windows() {
        assert!(embeddable_windows(&[], 16).is_empty());
    }

    #[test]
    fn the_batch_window_follows_the_longest_pair() {
        // Sizing the batch to the real speech avoids padding every pair up to a fixed
        // length the model would then process as silence.
        let pairs = vec![
            SpeakerPair { chunk: 0, speaker: 0, samples: vec![0.0; 9_000] },
            SpeakerPair { chunk: 0, speaker: 1, samples: vec![0.0; 30_000] },
        ];

        assert_eq!(batch_window_samples(&pairs, 100_000), 30_000);
    }

    #[test]
    fn the_batch_window_respects_the_floor() {
        // An embedding from a fraction of a second is unstable, so a very short pair
        // is padded up rather than embedded as-is.
        let pairs = vec![SpeakerPair { chunk: 0, speaker: 0, samples: vec![0.0; 100] }];
        assert_eq!(batch_window_samples(&pairs, 100_000), MIN_EMBEDDING_SAMPLES);
    }

    #[test]
    fn the_batch_window_respects_the_cap() {
        // Trimming the longest pair bounds the cost; the alternative is every pair
        // padded up to a single long outlier.
        let pairs = vec![SpeakerPair { chunk: 0, speaker: 0, samples: vec![0.0; 500_000] }];
        assert_eq!(batch_window_samples(&pairs, 48_000), 48_000);
    }

    #[test]
    fn the_batch_window_of_no_pairs_is_the_floor() {
        assert_eq!(batch_window_samples(&[], 48_000), MIN_EMBEDDING_SAMPLES);
    }

    #[test]
    fn a_cap_below_the_floor_still_yields_a_usable_window() {
        // A nonsensical cap must not produce a window shorter than the floor, which
        // would hand the model too little audio to embed.
        let pairs = vec![SpeakerPair { chunk: 0, speaker: 0, samples: vec![0.0; 5_000] }];
        assert_eq!(batch_window_samples(&pairs, 10), MIN_EMBEDDING_SAMPLES);
    }
}
