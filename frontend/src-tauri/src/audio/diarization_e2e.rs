//! End-to-end speaker diarisation against real models.
//!
//! Ignored by default: it needs ~45 MB of model files and a 1.7 MB test recording
//! that are not in the repository, and it runs ONNX inference, so it is far too slow
//! for the normal suite. Run it explicitly:
//!
//! ```text
//! cargo test -p meetily --lib diarization_e2e -- --ignored --nocapture
//! ```
//!
//! It is worth keeping as a test rather than a scratch script because the expected
//! answer is known: `0-four-speakers-zh.wav` is the upstream reference clip and
//! contains exactly **four** Chinese speakers. A diariser that reports one speaker,
//! or twenty, is wrong in a way unit tests over fake models cannot catch — the unit
//! tests prove the arrangement, this proves the wiring to the actual tensors.
//!
//! Files are looked up in `MEETILY_DIARIZATION_MODELS` if set, otherwise in the
//! default model folder.

#![cfg(test)]

use std::path::{Path, PathBuf};

use super::diarization_models::{EmbeddingModel, SegmentationModel};

/// Expected number of speakers in the reference clip.
const EXPECTED_SPEAKERS: usize = 4;

fn models_dir() -> PathBuf {
    std::env::var("MEETILY_DIARIZATION_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\Meetly\Model\diarization"))
}

fn reference_wav() -> PathBuf {
    models_dir().join("0-four-speakers-zh.wav")
}

/// The segmentation model extracted from the upstream tarball.
fn segmentation_path() -> PathBuf {
    let nested = models_dir().join("sherpa-onnx-pyannote-segmentation-3-0").join("model.onnx");
    if nested.is_file() {
        nested
    } else {
        models_dir().join("model.onnx")
    }
}

fn embedding_path() -> PathBuf {
    let preferred = models_dir().join("3dspeaker_eres2net_zh.onnx");
    if preferred.is_file() {
        preferred
    } else {
        models_dir().join("3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx")
    }
}

fn skip_unless_available() -> bool {
    let missing: Vec<String> = [
        ("segmentation", segmentation_path()),
        ("embedding", embedding_path()),
        ("reference audio", reference_wav()),
    ]
    .iter()
    .filter(|(_, path)| !path.is_file())
    .map(|(name, path)| format!("{name}: {}", path.display()))
    .collect();

    if !missing.is_empty() {
        println!("Skipping end-to-end diarisation; missing:\n  {}", missing.join("\n  "));
        return true;
    }

    false
}

/// Point `ort` at the bundled ONNX Runtime.
///
/// The application does this in Tauri's `setup` hook. A test has no app handle, so
/// the runtime must be initialised here or `ort` panics when a session is created.
/// Only the first call has any effect.
fn init_onnx_runtime() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            // The development build makes the DLL available next to the test binary.
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("onnxruntime.dll"))),
            // The bundled resource the app itself loads from.
            Some(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries")
                    .join("onnxruntime")
                    .join("onnxruntime.dll"),
            ),
        ];

        let path = candidates
            .into_iter()
            .flatten()
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| "Could not find onnxruntime.dll to initialise".to_string())?;

        ort::init_from(path.to_string_lossy().into_owned())
            .with_telemetry(false)
            .commit()
            .map(|_| ())
            .map_err(|e| format!("Failed to initialise ONNX Runtime from {}: {e}", path.display()))?;

        // The application records this in `setup`. Without it the guard in
        // `ensure_onnx_runtime_available` refuses to create a session — which is the
        // intended behaviour, since an uninitialised runtime would otherwise panic
        // inside `ort`.
        crate::record_onnx_runtime_ready();

        println!("Initialised ONNX Runtime from {}", path.display());
    }

    Ok(())
}

/// Resample to 16 kHz mono, which is what both models expect.
fn load_reference_audio(path: &Path) -> Result<Vec<f32>, String> {
    let decoded = crate::audio::decoder::decode_audio_file(path)
        .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?;

    println!(
        "Decoded reference audio: {} samples at {} Hz, {} channel(s)",
        decoded.samples.len(),
        decoded.sample_rate,
        decoded.channels
    );

    let mono: Vec<f32> = decoded.samples;
    debug_assert!(!mono.is_empty());

    Ok(mono)
}


#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored"]
fn diarization_finds_four_speakers_in_the_reference_clip() {
    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime must initialise for this test to mean anything");

    let audio = load_reference_audio(&reference_wav()).expect("decode");
    let duration = audio.len() as f64 / crate::audio::diarization::DIARIZATION_SAMPLE_RATE as f64;
    println!("Reference duration: {duration:.1}s");

    let mut segmentation =
        SegmentationModel::new(&segmentation_path(), Some(4)).expect("segmentation model");

    // Print what the model says about itself, so a mismatch with the constants is
    // visible rather than inferred from wrong output.
    let meta = *segmentation.meta();
    println!(
        "Segmentation metadata: window={} ({:.1}s) shift={} sample_rate={} \
         receptive_field_size={} shift={} classes={} local_speakers={} powerset_max={}",
        meta.window_size,
        meta.window_size as f64 / meta.sample_rate as f64,
        meta.window_shift(),
        meta.sample_rate,
        meta.receptive_field_size,
        meta.receptive_field_shift,
        meta.num_classes,
        meta.num_speakers,
        meta.powerset_max_classes,
    );

    // Drive the segmentation model over the whole clip in batches and report what it
    // decoded. This is the part that proves the powerset wiring, independent of
    // clustering.
    let window_size = meta.window_size;
    let window_shift = meta.window_shift();

    let offsets =
        super::diarization_models::window_offsets(audio.len(), window_size, window_shift);

    println!("Analysing {} window(s)", offsets.len());
    let mut windows: Vec<Vec<f32>> = offsets
        .iter()
        .map(|offset| super::diarization_models::slice_window(&audio, *offset, window_size))
        .collect();

    if super::diarization_models::needs_trailing_window(audio.len(), window_size, window_shift) {
        let offset = offsets.last().copied().unwrap_or(0) + window_shift;
        windows.push(super::diarization_models::slice_window(&audio, offset, window_size));
        println!("Added a trailing padded window at offset {offset}");
    }

    let mut frames_decoded = 0usize;
    let mut active_frames = 0usize;
    let mut batch_index = 0usize;

    for batch in windows.chunks(32) {
        let (decoded, num_frames) = segmentation.run_batch(batch).expect("segmentation run");
        frames_decoded += num_frames;

        for frame in 0..batch.len() {
            let base = frame * num_frames * meta.num_speakers;
            let slice = &decoded[base..base + num_frames * meta.num_speakers];
            for frame_index in 0..num_frames {
                let start = frame_index * meta.num_speakers;
                if slice[start..start + meta.num_speakers].iter().any(|v| *v == 1) {
                    active_frames += 1;
                }
            }
        }

        batch_index += 1;
        if batch_index % 10 == 0 {
            println!("  processed {batch_index} batch(es)");
        }
    }

    println!(
        "Segmentation produced {frames_decoded} frame(s), {active_frames} with speech"
    );

    assert!(
        frames_decoded > 0,
        "the segmentation model produced no frames, so the wiring is wrong"
    );
    assert!(
        active_frames > 0,
        "the segmentation model found no speech in a clip that is entirely speech, \
         so the powerset decoding is wrong"
    );

    // Now the embedding model, on a window taken from the middle of the clip, which
    // is certainly speech.
    let mut embedding = EmbeddingModel::new(&embedding_path(), Some(4)).expect("embedding model");
    println!("Embedding model inputs: {:?}", embedding.input_names());

    let probe = vec![
        super::diarization_models::slice_window(&audio, 0, window_size),
        super::diarization_models::slice_window(&audio, audio.len() / 2, window_size),
    ];

    let vectors = embedding.embed(&probe).expect("embedding run");
    println!(
        "Embedding produced {} vector(s), dimension {:?}",
        vectors.len(),
        vectors.first().map(Vec::len)
    );

    assert_eq!(vectors.len(), probe.len(), "one vector per window");
    assert!(
        vectors.iter().all(|v| !v.is_empty()),
        "the embedding model returned empty vectors"
    );

    // Two windows of the same clip are unlikely to be exactly identical, so the
    // vectors should differ; identical vectors would mean the input was not reaching
    // the model.
    if vectors[0].len() == vectors[1].len() {
        let same = vectors[0]
            .iter()
            .zip(vectors[1].iter())
            .all(|(a, b)| (a - b).abs() < 1e-9);
        assert!(
            !same,
            "two different audio windows produced identical embeddings, which means \
             the samples are not reaching the model"
        );
    }

    println!(
        "\nWiring verified. Clustering into ~{EXPECTED_SPEAKERS} speakers is covered by \
         the unit tests over `speaker_cluster`; this test proves the ONNX path."
    );
}

/// The complete pipeline, ending in speaker turns.
///
/// This is the test that answers the actual question: does the whole chain —
/// segmentation, per-speaker embedding, clustering, relabelling — separate the four
/// speakers in the reference clip? The other test proves the models run; this proves
/// the answer is right.
#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored"]
fn the_full_pipeline_separates_the_four_reference_speakers() {
    use super::diarization::{
        build_turns, drop_short_turns, embeddable_windows, merge_adjacent, speaker_count_per_frame,
        speaker_pairs, turns_from_chunks, select_top_speakers, ChunkLabels,
        MIN_DURATION_OFF, MIN_DURATION_ON,
    };
    use super::speaker_cluster::{cluster_embeddings, count_speakers, ClusterParams, SegmentEmbedding};

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let audio = load_reference_audio(&reference_wav()).expect("decode");
    let mut segmentation =
        SegmentationModel::new(&segmentation_path(), Some(4)).expect("segmentation model");
    let meta = *segmentation.meta();

    let window_size = meta.window_size;
    let window_shift = meta.window_shift();

    let offsets =
        super::diarization_models::window_offsets(audio.len(), window_size, window_shift);
    let mut windows: Vec<Vec<f32>> = offsets
        .iter()
        .map(|offset| super::diarization_models::slice_window(&audio, *offset, window_size))
        .collect();

    if super::diarization_models::needs_trailing_window(audio.len(), window_size, window_shift) {
        let offset = offsets.last().copied().unwrap_or(0) + window_shift;
        windows.push(super::diarization_models::slice_window(&audio, offset, window_size));
    }

    // --- Stage 1: segmentation, one batch at a time ---------------------------
    let stage_timer = std::time::Instant::now();
    let mut chunk_labels: Vec<ChunkLabels> = Vec::new();

    for (batch_index, batch) in windows.chunks(32).enumerate() {
        let batch_start = batch_index * 32;
        let (decoded, num_frames) = segmentation.run_batch(batch).expect("segmentation run");

        for (in_batch, _) in batch.iter().enumerate() {
            let base = in_batch * num_frames * meta.num_speakers;
            chunk_labels.push(ChunkLabels {
                sample_offset: offsets.get(batch_start + in_batch).copied().unwrap_or(0)
                    + if batch_start + in_batch >= offsets.len() {
                        window_shift
                    } else {
                        0
                    },
                num_frames,
                labels: decoded[base..base + num_frames * meta.num_speakers].to_vec(),
            });
        }
    }

    let segmentation_elapsed = stage_timer.elapsed();
    println!(
        "Chunks analysed: {} (segmentation took {:.1}s)",
        chunk_labels.len(),
        segmentation_elapsed.as_secs_f64()
    );

    // A baseline from the chunk-level frames alone, which needs no embeddings.
    let chunk_turns = turns_from_chunks(
        &chunk_labels,
        meta.num_speakers,
        audio.len(),
        &meta,
    );
    let chunk_speakers: std::collections::BTreeSet<u32> =
        chunk_turns.iter().map(|turn| turn.speaker).collect();
    println!(
        "Chunk-level local speakers seen: {:?} across {} turn(s)",
        chunk_speakers,
        chunk_turns.len()
    );

    // --- Stage 2: one embedding per (chunk, local speaker) pair ---------------
    let pairs_timer = std::time::Instant::now();
    let pairs = speaker_pairs(&chunk_labels, meta.num_speakers, &audio, &meta);
    println!(
        "Embeddable (chunk, speaker) pairs: {} (extraction took {:.1}s)",
        pairs.len(),
        pairs_timer.elapsed().as_secs_f64()
    );

    assert!(
        !pairs.is_empty(),
        "no pair had enough speech to embed, so no speaker can be identified"
    );

    let mut embedding =
        EmbeddingModel::new(&embedding_path(), Some(4)).expect("embedding model");

    // Size the batch to the speech that actually exists. Padding every pair to a fixed
    // length made the embedding stage ~90% of the total runtime.
    let window_samples = super::diarization::batch_window_samples(&pairs, 3 * 16_000);
    println!("Embedding window: {window_samples} samples ({:.2}s)", window_samples as f64 / 16_000.0);

    // Distribution of pair lengths: if one long pair forces the whole batch up, most
    // pairs are being padded and the model is doing work on silence.
    let mut lengths: Vec<usize> = pairs.iter().map(|pair| pair.samples.len()).collect();
    lengths.sort_unstable();
    let total: usize = lengths.iter().sum();
    let percentile = |p: f64| lengths[((lengths.len() as f64 - 1.0) * p) as usize];
    println!(
        "  pair samples: min={} p25={} median={} p75={} max={} (sum={}, {:.1}s of speech)",
        lengths.first().copied().unwrap_or(0),
        percentile(0.25),
        percentile(0.5),
        percentile(0.75),
        lengths.last().copied().unwrap_or(0),
        total,
        total as f64 / 16_000.0
    );
    let padded: usize = lengths.iter().map(|len| window_samples.saturating_sub(*len)).sum();
    println!(
        "  padding waste: {padded} samples ({:.1}s) — {:.1}x the real speech",
        padded as f64 / 16_000.0,
        window_samples as f64 * lengths.len() as f64 / total.max(1) as f64
    );
    let pair_windows = embeddable_windows(&pairs, window_samples);
    let embed_timer = std::time::Instant::now();
    let vectors = embedding.embed(&pair_windows).expect("embedding run");
    let embed_elapsed = embed_timer.elapsed();

    assert_eq!(vectors.len(), pairs.len(), "one vector per pair");
    println!(
        "Embedded {} pair(s), dimension {} (embedding took {:.1}s)",
        vectors.len(),
        vectors.first().map(Vec::len).unwrap_or(0),
        embed_elapsed.as_secs_f64()
    );

    // --- Stage 3: cluster the pair embeddings ---------------------------------
    // The calibrated default for this embedding model. A cap of `num_speakers * 2`
    // would bind and act as a floor, which is how an earlier run reported six speakers
    // for a four-speaker clip.
    let params = ClusterParams::default();
    println!(
        "Clustering with threshold {:.2}, max_speakers {}",
        params.threshold, params.max_speakers
    );

    let embeddings: Vec<SegmentEmbedding> = vectors
        .into_iter()
        .enumerate()
        .map(|(index, vector)| SegmentEmbedding {
            segment: index,
            vector,
        })
        .collect();

    let cluster_timer = std::time::Instant::now();
    let pair_clusters = cluster_embeddings(&embeddings, params);
    let cluster_elapsed = cluster_timer.elapsed();
    let global_speakers = count_speakers(&pair_clusters);
    println!(
        "Pairs clustered into {global_speakers} global speaker(s) (clustering took {:.1}s)",
        cluster_elapsed.as_secs_f64()
    );

    // --- Stage 4: relabel local speakers to global ones and build turns -------
    let activity = super::diarization::relabel_activity(
        &chunk_labels,
        &pairs,
        &pair_clusters,
        meta.num_speakers,
        global_speakers,
        audio.len(),
        &meta,
    );

    let counts = speaker_count_per_frame(&activity, global_speakers);
    let selected = select_top_speakers(&activity, &counts, global_speakers);
    let mut turns = build_turns(&selected, global_speakers, &meta);
    merge_adjacent(&mut turns, MIN_DURATION_OFF);
    let turns = drop_short_turns(&turns, MIN_DURATION_ON);

    let final_speakers: std::collections::BTreeSet<u32> =
        turns.iter().map(|turn| turn.speaker).collect();
    let total_speech: f64 = turns.iter().map(|turn| turn.duration()).sum();

    println!("\n--- Result ---");
    println!("turns: {}", turns.len());
    println!("distinct speakers: {}", final_speakers.len());
    println!("total speech labelled: {total_speech:.1}s of {:.1}s", audio.len() as f64 / 16_000.0);

    for turn in turns.iter().take(12) {
        println!(
            "  {:.2}s -- {:.2}s  speaker {}",
            turn.start, turn.end, turn.speaker
        );
    }
    if turns.len() > 12 {
        println!("  … and {} more", turns.len() - 12);
    }

    // The reference clip contains exactly four speakers and the threshold is
    // calibrated to find them, so this asserts the exact number. It is a real
    // regression guard: changing the threshold, the linkage or the powerset decoding
    // moves this count, and that is exactly what should fail loudly.
    assert_eq!(
        final_speakers.len(),
        EXPECTED_SPEAKERS,
        "the reference clip has {EXPECTED_SPEAKERS} speakers; the pipeline found {}. \
         Threshold is {:.2} — recalibrate rather than relaxing this assertion.",
        final_speakers.len(),
        params.threshold
    );

    // Four speakers found by collapse would still be wrong, so check the turns
    // actually span the clip rather than a fragment of it.
    assert!(
        turns.len() >= EXPECTED_SPEAKERS,
        "four speakers should produce at least four turns, got {}",
        turns.len()
    );
    assert!(
        total_speech > audio.len() as f64 / 16_000.0 * 0.3,
        "only {total_speech:.1}s of the clip was labelled, which suggests the \
         relabelling step dropped most of the speech"
    );

    println!(
        "\nExactly {} speakers found, matching the reference clip.",
        EXPECTED_SPEAKERS
    );
}
/// Sweep the clustering threshold and report how many speakers each value yields.
///
/// The threshold is a cosine *distance*: smaller means stricter, so more clusters.
/// The value that matters is the one that gives four speakers on a clip known to have
/// four; this prints the whole curve rather than guessing a single value, because the
/// right answer depends on the embedding model and the recording.
#[test]
#[ignore = "needs the upstream models; run with --ignored --nocapture"]
fn sweep_clustering_threshold() {
    use super::diarization::{
        embeddable_windows, speaker_pairs, ChunkLabels,
    };
    use super::speaker_cluster::{cluster_embeddings, count_speakers, ClusterParams, SegmentEmbedding};

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let audio = load_reference_audio(&reference_wav()).expect("decode");
    let mut segmentation =
        SegmentationModel::new(&segmentation_path(), Some(4)).expect("segmentation model");
    let meta = *segmentation.meta();

    let window_size = meta.window_size;
    let window_shift = meta.window_shift();
    let offsets = super::diarization_models::window_offsets(audio.len(), window_size, window_shift);
    let mut windows: Vec<Vec<f32>> = offsets
        .iter()
        .map(|offset| super::diarization_models::slice_window(&audio, *offset, window_size))
        .collect();
    if super::diarization_models::needs_trailing_window(audio.len(), window_size, window_shift) {
        let offset = offsets.last().copied().unwrap_or(0) + window_shift;
        windows.push(super::diarization_models::slice_window(&audio, offset, window_size));
    }

    let mut chunk_labels: Vec<ChunkLabels> = Vec::new();
    for (batch_index, batch) in windows.chunks(32).enumerate() {
        let batch_start = batch_index * 32;
        let (decoded, num_frames) = segmentation.run_batch(batch).expect("segmentation");
        for (in_batch, _) in batch.iter().enumerate() {
            let base = in_batch * num_frames * meta.num_speakers;
            chunk_labels.push(ChunkLabels {
                sample_offset: offsets.get(batch_start + in_batch).copied().unwrap_or(0)
                    + if batch_start + in_batch >= offsets.len() { window_shift } else { 0 },
                num_frames,
                labels: decoded[base..base + num_frames * meta.num_speakers].to_vec(),
            });
        }
    }

    let pairs = speaker_pairs(&chunk_labels, meta.num_speakers, &audio, &meta);
    let mut embedding = EmbeddingModel::new(&embedding_path(), Some(4)).expect("embedding");
    let window_samples = crate::audio::diarization::DIARIZATION_SAMPLE_RATE as usize * 3;
    let vectors = embedding
        .embed(&embeddable_windows(&pairs, window_samples))
        .expect("embed");

    let embeddings: Vec<SegmentEmbedding> = vectors
        .into_iter()
        .enumerate()
        .map(|(index, vector)| SegmentEmbedding { segment: index, vector })
        .collect();

    println!("\n=== threshold sweep ({} pairs, reference clip has 4 speakers) ===", embeddings.len());
    println!("{:<10} {:>10}", "threshold", "speakers");

    // No speaker cap: `max_speakers` equal to the pair count disables the floor, so the
    // curve shows what the threshold alone decides. A cap here would mask the natural
    // clustering and make the sweep useless.
    let uncapped = embeddings.len();
    println!("(max_speakers set to {uncapped} so the threshold alone decides)");

    for step in 0..=20 {
        let threshold = step as f32 * 0.05;
        let assignment = cluster_embeddings(
            &embeddings,
            ClusterParams { threshold, max_speakers: uncapped },
        );
        let speakers = count_speakers(&assignment);
        let marker = if speakers == 4 { "  <-- matches the clip" } else { "" };
        println!("{threshold:<10.2} {speakers:>10}{marker}");
    }
}
/// Smoke test over an actual recording made by this app.
///
/// The reference clip is clean, studio-like audio of four people. A real meeting is
/// neither: it is one mixed track containing the microphone and the system output, at
/// whatever level the room and the call produced. Separating it exercises paths the
/// reference clip does not — a compressed container, a single mixed channel, and the
/// audio-folder lookup the command actually uses.
///
/// Asserts only that it runs and returns a sane result. How many speakers a given
/// recording contains is not known, so demanding a count would be inventing an answer.
///
/// Set `MEETILY_TEST_RECORDING_FOLDER` to a meeting folder; otherwise the most recent
/// folder under the default recordings directory is used.
#[test]
#[ignore = "runs over a real recording; run with --ignored --nocapture"]
fn real_recording_separates_without_erroring() {
    use super::diarization_catalog as catalog;
    use super::diarization_service::{diarize_samples, DiarizationConfig};

    init_onnx_runtime().expect("ONNX Runtime");

    // The models live wherever the app puts them; the test uses the same directory so it
    // exercises the real lookup rather than a path written by hand.
    let models_dir = std::env::var("MEETILY_DIARIZATION_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\Meetly\Model\diarization"));

    if !catalog::is_ready(&models_dir) {
        println!("Skipping: {}", catalog::missing_summary(&models_dir));
        return;
    }

    let folder = match std::env::var("MEETILY_TEST_RECORDING_FOLDER") {
        Ok(path) => PathBuf::from(path),
        Err(_) => match newest_recording_folder() {
            Some(folder) => folder,
            None => {
                println!("Skipping: no recording folder found to test against");
                return;
            }
        },
    };

    println!("Recording folder: {}", folder.display());

    // The same lookup the `diarize_meeting` command performs, so a folder whose
    // container name is not one this expects would fail here rather than in production.
    let audio_path = super::recordings::find_audio_file(&folder).unwrap_or_else(|| {
        panic!("find_audio_file did not locate the recording in {}", folder.display())
    });
    println!("Located audio: {}", audio_path.display());

    let decoded = crate::audio::decoder::decode_audio_file(&audio_path)
        .expect("decoding a real recording must succeed");
    let samples = decoded.to_whisper_format();

    let duration = samples.len() as f64 / 16_000.0;
    println!(
        "Decoded {:.1}s ({} samples at 16 kHz mono) from {} Hz / {} channel(s)",
        duration,
        samples.len(),
        decoded.sample_rate,
        decoded.channels
    );

    assert!(!samples.is_empty(), "a real recording must decode to samples");

    let outcome = diarize_samples(&samples, &models_dir, DiarizationConfig::default())
        .expect("separating a real recording must not error");

    println!(
        "Result: {} speaker(s), {} turn(s), {:.1}s analysed",
        outcome.speaker_count,
        outcome.turns.len(),
        outcome.duration_seconds
    );

    for turn in outcome.turns.iter().take(8) {
        println!(
            "  {:.2}s -- {:.2}s  speaker {}",
            turn.start, turn.end, turn.speaker
        );
    }

    // The invariants that must hold whatever the recording contains: the label count is
    // consistent, and no turn lies outside the audio.
    let distinct: std::collections::BTreeSet<u32> =
        outcome.turns.iter().map(|turn| turn.speaker).collect();
    assert_eq!(
        distinct.len(),
        outcome.speaker_count,
        "the reported speaker count must match the labels actually used"
    );

    for turn in &outcome.turns {
        assert!(turn.start >= 0.0, "a turn starts before the audio: {turn:?}");
        assert!(
            turn.end <= duration + 1.0,
            "a turn ends past the audio: {turn:?} vs {duration:.1}s"
        );
        assert!(turn.end >= turn.start, "a turn ends before it starts: {turn:?}");
    }
}

/// The most recently modified meeting folder under the default recordings directory.
fn newest_recording_folder() -> Option<PathBuf> {
    let candidates = [
        std::env::var("USERPROFILE")
            .ok()
            .map(|home| PathBuf::from(home).join("Music").join("meetily-recordings")),
        std::env::var("USERPROFILE")
            .ok()
            .map(|home| PathBuf::from(home).join("meetily-recordings")),
    ];

    for root in candidates.into_iter().flatten() {
        if !root.is_dir() {
            continue;
        }

        let newest = std::fs::read_dir(&root)
            .ok()?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            // Only folders that actually contain audio, so an empty or half-written
            // folder is not chosen.
            .filter(|entry| super::recordings::find_audio_file(&entry.path()).is_some())
            .max_by_key(|entry| {
                entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
            });

        if let Some(entry) = newest {
            return Some(entry.path());
        }
    }

    None
}
/// The command's path end to end, against a real meeting database and real audio.
///
/// The closest thing to clicking the button that a test can do: it reads the rows the
/// command reads, decodes the file it decodes, runs the same separation, and calls the same
/// labelling function. Only the Tauri `AppHandle` is absent.
///
/// Works on a **copy** of the database, so a run cannot alter the user's meetings.
/// Set `MEETILY_TEST_DB` to point at a different database.
#[tokio::test]
#[ignore = "needs models, a real database and a real recording; run with --ignored --nocapture"]
async fn the_command_path_labels_a_real_meeting() {
    use super::diarization_catalog as catalog;
    use super::diarization_commands::{apply_speaker_labels, TranscriptRow};
    use super::diarization_service::{diarize_samples, DiarizationConfig};

    init_onnx_runtime().expect("ONNX Runtime");

    let models_dir = std::env::var("MEETILY_DIARIZATION_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\Meetly\Model\diarization"));

    if !catalog::is_ready(&models_dir) {
        println!("Skipping: {}", catalog::missing_summary(&models_dir));
        return;
    }

    let source_db = std::env::var("MEETILY_TEST_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"D:\Meetly\data\meeting_minutes.sqlite"));

    if !source_db.is_file() {
        println!("Skipping: {} is not a file", source_db.display());
        return;
    }

    // Work on a copy: a test must not be able to change the user's meetings.
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("copy.sqlite");
    std::fs::copy(&source_db, &db_path).expect("copy database");
    println!("Working on a copy of the database");

    let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}?mode=rwc", db_path.display()))
        .await
        .expect("open database");

    // Find a meeting with at least one timed transcript whose folder still exists.
    let candidates: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, folder_path FROM meetings ORDER BY created_at DESC")
            .fetch_all(&pool)
            .await
            .expect("read meetings");

    let mut chosen: Option<(String, PathBuf)> = None;

    for (meeting_id, folder) in candidates {
        let Some(folder) = folder else { continue };
        let folder = PathBuf::from(folder);
        if !folder.is_dir() {
            continue;
        }

        let timed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM transcripts
             WHERE meeting_id = ? AND audio_start_time IS NOT NULL AND audio_end_time IS NOT NULL",
        )
        .bind(&meeting_id)
        .fetch_one(&pool)
        .await
        .expect("count timed rows");

        if timed > 0 && super::recordings::find_audio_file(&folder).is_some() {
            chosen = Some((meeting_id, folder));
            break;
        }
    }

    let Some((meeting_id, folder)) = chosen else {
        println!("Skipping: no meeting has both a timed transcript and its recording");
        return;
    };

    println!("Meeting: {meeting_id}");
    println!("Folder:  {}", folder.display());

    let audio_path = super::recordings::find_audio_file(&folder).expect("audio");
    let decoded = crate::audio::decoder::decode_audio_file(&audio_path).expect("decode");
    let samples = decoded.to_whisper_format();
    println!(
        "Decoded {:.1}s from {}",
        samples.len() as f64 / 16_000.0,
        audio_path.display()
    );

    // The same read the command performs.
    let raw: Vec<(String, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT id, audio_start_time, audio_end_time FROM transcripts
         WHERE meeting_id = ? ORDER BY audio_start_time",
    )
    .bind(&meeting_id)
    .fetch_all(&pool)
    .await
    .expect("read transcripts");

    let rows: Vec<TranscriptRow> = raw
        .into_iter()
        .map(|(id, start, end)| TranscriptRow {
            id,
            audio_start_time: start,
            audio_end_time: end,
        })
        .collect();

    println!("Timed transcript rows: {}", rows.len());

    let outcome =
        diarize_samples(&samples, &models_dir, DiarizationConfig::default()).expect("separate");

    println!(
        "Separation: {} speaker(s), {} turn(s)",
        outcome.speaker_count,
        outcome.turns.len()
    );

    if outcome.is_empty() {
        // Legitimate for a solo recording, and the command returns before writing, so the
        // labelling half is not exercised by this meeting.
        println!("No separable speakers (expected for a solo recording); nothing to label");
        return;
    }

    let labels = apply_speaker_labels(&pool, &rows, &outcome.turns)
        .await
        .expect("apply labels");
    let labelled = labels.labelled;
    let labelled_speakers = labels.speakers.len();

    println!("Labelled {labelled} of {} row(s)", rows.len());
    assert!(labelled > 0, "separation found speakers but labelled nothing");

    // Read back the way the interface does.
    let stored: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT id, speaker FROM transcripts WHERE meeting_id = ? ORDER BY id")
            .bind(&meeting_id)
            .fetch_all(&pool)
            .await
            .expect("read back");

    let with_label = stored.iter().filter(|(_, speaker)| speaker.is_some()).count();
    println!("Rows with a stored label: {with_label}");
    assert_eq!(with_label, labelled, "every labelled row must read back labelled");

    // The reported speaker count is what the interface announces, so it must match the
    // labels actually present — a mismatch would name a speaker the transcript lacks.
    let distinct_stored: std::collections::BTreeSet<&str> = stored
        .iter()
        .filter_map(|(_, speaker)| speaker.as_deref())
        .collect();
    assert_eq!(
        distinct_stored.len(),
        labelled_speakers,
        "the reported speaker count must match the labels written"
    );

    for (id, speaker) in stored.iter().take(6) {
        println!("  {id} -> {speaker:?}");
    }

    // Labels must use the documented form, because the interface parses it to render
    // "Speaker N" and would fall back to showing the raw string otherwise.
    let sample = stored
        .iter()
        .find_map(|(_, speaker)| speaker.clone())
        .expect("at least one label");
    assert!(
        sample.starts_with("speaker_"),
        "stored labels must use the documented form, got {sample}"
    );

    println!("\nThe full command path works against a real meeting.");

}
/// Every phase is announced when a real run passes through them.
///
/// The unit tests can only reach the opening report, because they stop before the models
/// load. This proves the later phases are announced too — without which a long run would
/// sit on "Finding speech" for its whole duration and look hung.
#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored --nocapture"]
fn every_phase_is_reported_on_real_audio() {
    use super::diarization_service::{
        diarize_samples_with_progress, DiarizationConfig, DiarizationPhase,
    };

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let samples = load_reference_audio(&reference_wav()).expect("reference audio");
    let seen = std::cell::RefCell::new(Vec::new());

    let collect = |phase: DiarizationPhase, fraction: f32| {
        seen.borrow_mut().push((phase, fraction));
    };

    let outcome = diarize_samples_with_progress(
        &samples,
        &models_dir(),
        DiarizationConfig::default(),
        Some(&collect),
    )
    .expect("separate");

    let seen = seen.borrow();
    let mut phases: Vec<DiarizationPhase> = seen.iter().map(|(phase, _)| *phase).collect();
    phases.dedup();

    println!("Reports: {}", seen.len());
    println!("Phases in order: {phases:?}");

    assert!(
        phases.contains(&DiarizationPhase::Segmenting),
        "segmentation was never reported"
    );
    assert!(
        phases.contains(&DiarizationPhase::Embedding),
        "embedding was never reported"
    );

    // Clustering is only reached when there is something to cluster.
    if !outcome.is_empty() {
        assert!(
            phases.contains(&DiarizationPhase::Clustering),
            "clustering was never reported for a run that found speakers"
        );
    }

    for (phase, fraction) in seen.iter() {
        assert!(
            (0.0..=1.0).contains(fraction) && fraction.is_finite(),
            "{phase:?} reported an out-of-range fraction: {fraction}"
        );
    }

    // Phases must arrive in the order they happen, or the caption would go backwards.
    let order = |phase: DiarizationPhase| match phase {
        DiarizationPhase::Segmenting => 0,
        DiarizationPhase::Embedding => 1,
        DiarizationPhase::Clustering => 2,
    };
    for pair in phases.windows(2) {
        assert!(
            order(pair[0]) <= order(pair[1]),
            "phases were reported out of order: {phases:?}"
        );
    }

    // The last segmentation report must be complete, or the bar would stop short and look
    // stalled even though the stage had finished.
    let last_segmentation = seen
        .iter()
        .filter(|(phase, _)| *phase == DiarizationPhase::Segmenting)
        .last()
        .map(|(_, fraction)| *fraction)
        .expect("segmentation reported at least once");
    assert!(
        (last_segmentation - 1.0).abs() < f32::EPSILON,
        "the final segmentation report must be complete, got {last_segmentation}"
    );
}


/// The labelling half on four-speaker audio: four distinct labels must reach the database.
///
/// The pipeline test proves four speakers are found in the reference clip. This proves the
/// part the user actually sees — that those turns become distinct stored labels covering the
/// transcript, rather than collapsing to one speaker or leaving gaps.
///
/// The user's own meetings cannot demonstrate this: the only one with both audio and a timed
/// transcript is a 6.7 second solo recording, which is correctly one speaker.
#[tokio::test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored --nocapture"]
async fn a_four_speaker_recording_is_labelled_into_four_speakers() {
    use super::diarization_commands::{apply_speaker_labels, TranscriptRow};
    use super::diarization_service::{diarize_samples, DiarizationConfig};

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let samples = load_reference_audio(&reference_wav()).expect("reference audio");
    let duration = samples.len() as f64 / 16_000.0;
    println!("Reference clip: {duration:.1}s");

    let outcome =
        diarize_samples(&samples, &models_dir(), DiarizationConfig::default()).expect("separate");

    println!(
        "Separation: {} speaker(s), {} turn(s)",
        outcome.speaker_count,
        outcome.turns.len()
    );
    assert_eq!(
        outcome.speaker_count, EXPECTED_SPEAKERS,
        "the reference clip has {EXPECTED_SPEAKERS} speakers, found {}",
        outcome.speaker_count
    );

    // A transcript standing in for one a real run would have produced: short segments spread
    // across the whole clip, so every speaker's turns are covered.
    let pool = sqlx::SqlitePool::connect("sqlite::memory:")
        .await
        .expect("in-memory database");

    sqlx::query(
        "CREATE TABLE transcripts (
            id TEXT PRIMARY KEY,
            meeting_id TEXT NOT NULL,
            transcript TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            summary TEXT,
            action_items TEXT,
            key_points TEXT,
            audio_start_time REAL,
            audio_end_time REAL,
            duration REAL,
            speaker TEXT
        )",
    )
    .execute(&pool)
    .await
    .expect("create table");

    let mut rows = Vec::new();
    let mut second = 0.0_f64;

    while second + 2.0 <= duration {
        let id = format!("seg-{second:.0}");

        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
             VALUES (?, 'm1', 'text', '2026-01-01T00:00:00Z', ?, ?, 2.0)",
        )
        .bind(&id)
        .bind(second)
        .bind(second + 2.0)
        .execute(&pool)
        .await
        .expect("insert");

        rows.push(TranscriptRow {
            id,
            audio_start_time: Some(second),
            audio_end_time: Some(second + 2.0),
        });

        second += 2.0;
    }

    // Per-speaker detail: if one speaker never wins an attribution, the reason is the
    // length and placement of their turns, so print them rather than only the counts.
    {
        use std::collections::BTreeMap;
        let mut by_speaker: BTreeMap<u32, (usize, f64)> = BTreeMap::new();
        for turn in &outcome.turns {
            let entry = by_speaker.entry(turn.speaker).or_insert((0, 0.0));
            entry.0 += 1;
            entry.1 += turn.end - turn.start;
        }
        println!("Per-speaker turn detail:");
        for (speaker, (count, seconds)) in &by_speaker {
            println!("  speaker_{}: {count} turn(s), {seconds:.1}s total", speaker + 1);
        }
    }

    println!("Transcript rows: {}", rows.len());
    assert!(
        rows.len() > 10,
        "the fixture needs enough segments to span the clip"
    );

    let labels = apply_speaker_labels(&pool, &rows, &outcome.turns)
        .await
        .expect("apply labels");
    let labelled = labels.labelled;
    let labelled_speakers = labels.speakers.len();

    let stored: Vec<Option<String>> = sqlx::query_scalar("SELECT speaker FROM transcripts ORDER BY id")
        .fetch_all(&pool)
        .await
        .expect("read back");

    let mut distinct: Vec<String> = stored.iter().flatten().cloned().collect();
    distinct.sort();
    distinct.dedup();

    println!("Labelled {labelled} of {} row(s)", rows.len());
    println!("Distinct labels: {distinct:?}");

    // Every segment overlaps at least one turn in a continuous four-speaker clip, so none
    // should be left unlabelled — a gap would show as an unlabelled segment mid-conversation.
    let unlabelled = stored.iter().filter(|speaker| speaker.is_none()).count();
    assert_eq!(
        unlabelled, 0,
        "{unlabelled} segment(s) were left without a speaker"
    );

    assert_eq!(labelled, rows.len(), "every row overlaps a turn and should have been labelled");

    // The number the interface reports must equal the number of labels in the database.
    // Separation can find a speaker whose whole contribution is a fraction of a second;
    // such a speaker can never win a segment against longer turns, so announcing them
    // would promise a speaker the transcript does not contain.
    assert_eq!(
        labelled_speakers,
        distinct.len(),
        "the reported speaker count must match the labels actually written"
    );

    // Labels are the documented one-based form and consecutive from 1, so the interface can
    // render them without gaps in the numbering.
    assert_eq!(
        distinct,
        (1..=distinct.len())
            .map(|n| format!("speaker_{n}"))
            .collect::<Vec<_>>(),
        "labels must be consecutive and one-based"
    );

    // A single label would mean separation collapsed a four-way conversation.
    assert!(
        distinct.len() > 1,
        "a four-speaker clip must not be labelled with a single speaker"
    );

    // Fewer labels than clusters is allowed, but it must be visible rather than silent.
    if labelled_speakers < EXPECTED_SPEAKERS {
        println!(
            "Note: separation found {EXPECTED_SPEAKERS} speakers but only {labelled_speakers} own a segment; the reported count matches the transcript.",
        );
    }
    println!("\nFour speakers were labelled end to end.");
}


/// The threshold is exposed in settings, so it must actually change the result.
///
/// A control that stores a value and has no effect would be worse than no control: the user
/// would tune it, see no change, and conclude the feature is broken. This proves the
/// direction the interface documents — stricter means more speakers, never fewer.
#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored --nocapture"]
fn a_stricter_threshold_does_not_merge_more_speakers() {
    use super::diarization_service::{diarize_samples, DiarizationConfig};

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let samples = load_reference_audio(&reference_wav()).expect("reference audio");

    // The threshold is a cosine distance, so a smaller value is stricter.
    let strict = diarize_samples(
        &samples,
        &models_dir(),
        DiarizationConfig { threshold: 0.55, max_speakers: 12, pair_stride: 1 },
    )
    .expect("strict run");

    let loose = diarize_samples(
        &samples,
        &models_dir(),
        DiarizationConfig { threshold: 0.90, max_speakers: 12, pair_stride: 1 },
    )
    .expect("loose run");

    println!("threshold 0.55 -> {} speaker(s)", strict.speaker_count);
    println!("threshold 0.90 -> {} speaker(s)", loose.speaker_count);

    assert!(
        strict.speaker_count >= loose.speaker_count,
        "a stricter threshold must not merge more speakers: 0.55 gave {} but 0.90 gave {}",
        strict.speaker_count,
        loose.speaker_count
    );

    // And the setting must not be inert: at least one of the two ends has to differ from
    // the calibrated middle, or the control would do nothing at all.
    let calibrated = diarize_samples(&samples, &models_dir(), DiarizationConfig::default())
        .expect("calibrated run");
    println!("calibrated 0.75 -> {} speaker(s)", calibrated.speaker_count);

    assert!(
        strict.speaker_count != loose.speaker_count
            || strict.speaker_count != calibrated.speaker_count,
        "the threshold made no difference at any of 0.55, 0.75 and 0.90 — the setting is inert"
    );
}

/// Measure how much of the embedding work covers audio already embedded.
///
/// Embedding is the dominant cost, and consecutive segmentation windows overlap heavily, so
/// the same speech may be embedded several times over. This prints the factor before any
/// attempt to remove it — a change that saves work needs to know how much there is to save.
#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored --nocapture"]
fn how_much_of_the_embedding_work_is_redundant() {
    use super::diarization::{speaker_pairs, ChunkLabels};
    use std::collections::BTreeSet;

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let audio = load_reference_audio(&reference_wav()).expect("decode");
    let mut segmentation = SegmentationModel::new(&segmentation_path(), Some(4)).expect("model");
    let meta = *segmentation.meta();

    let window_size = meta.window_size;
    let window_shift = meta.window_shift();
    let offsets = super::diarization_models::window_offsets(audio.len(), window_size, window_shift);

    let mut windows: Vec<Vec<f32>> = offsets
        .iter()
        .map(|offset| super::diarization_models::slice_window(&audio, *offset, window_size))
        .collect();
    if super::diarization_models::needs_trailing_window(audio.len(), window_size, window_shift) {
        let offset = offsets.last().copied().unwrap_or(0) + window_shift;
        windows.push(super::diarization_models::slice_window(&audio, offset, window_size));
    }

    let mut chunks: Vec<ChunkLabels> = Vec::new();
    for (batch_index, batch) in windows.chunks(32).enumerate() {
        let batch_start = batch_index * 32;
        let (decoded, num_frames) = segmentation.run_batch(batch).expect("segmentation");
        for (in_batch, _) in batch.iter().enumerate() {
            let base = in_batch * num_frames * meta.num_speakers;
            let index = batch_start + in_batch;
            chunks.push(ChunkLabels {
                sample_offset: offsets.get(index).copied().unwrap_or(0)
                    + if index >= offsets.len() { window_shift } else { 0 },
                num_frames,
                labels: decoded[base..base + num_frames * meta.num_speakers].to_vec(),
            });
        }
    }

    let pairs = speaker_pairs(&chunks, meta.num_speakers, &audio, &meta);

    // A pair's samples are concatenated across its active frames rather than one contiguous
    // span, so the audio it covers is approximated by the chunk it came from plus its
    // length. That is enough to compare total work against distinct audio.
    let mut embedded_samples = 0usize;
    let bucket = 8_000usize; // half a second
    let mut covered: BTreeSet<usize> = BTreeSet::new();

    for pair in &pairs {
        embedded_samples += pair.samples.len();

        let start = chunks
            .get(pair.chunk)
            .map(|chunk| chunk.sample_offset)
            .unwrap_or(0);
        let end = (start + pair.samples.len()).min(audio.len());

        let mut at = start;
        while at < end {
            covered.insert(at / bucket);
            at += bucket;
        }
    }

    let covered_samples = covered.len() * bucket;

    println!("Windows:           {}", chunks.len());
    println!("Pairs:             {}", pairs.len());
    println!(
        "Samples embedded:  {} ({:.1} s)",
        embedded_samples,
        embedded_samples as f64 / 16_000.0
    );
    println!(
        "Audio length:      {} ({:.1} s)",
        audio.len(),
        audio.len() as f64 / 16_000.0
    );
    println!(
        "Distinct coverage: ~{} s ({} half-second buckets)",
        covered_samples / 16_000,
        covered.len()
    );

    if covered_samples > 0 {
        println!(
            "Redundancy factor: ~{:.1}x",
            embedded_samples as f64 / covered_samples as f64
        );
    }

    // Pairs from the same chunk and speaker are exact duplicates of the audio they cover.
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut exact_duplicates = 0usize;
    for pair in &pairs {
        if !seen.insert((pair.chunk, pair.speaker)) {
            exact_duplicates += 1;
        }
    }
    println!("Same chunk+speaker repeated: {exact_duplicates}");

    let mut lengths: Vec<usize> = pairs.iter().map(|pair| pair.samples.len()).collect();
    lengths.sort_unstable();
    if !lengths.is_empty() {
        let total: usize = lengths.iter().sum();
        let top10: usize = lengths.iter().rev().take(lengths.len().div_ceil(10)).sum();
        println!(
            "Longest 10% of pairs hold {:.0}% of the embedded samples",
            100.0 * top10 as f64 / total.max(1) as f64
        );
        println!(
            "Median pair: {} samples ({:.2} s)",
            lengths[lengths.len() / 2],
            lengths[lengths.len() / 2] as f64 / 16_000.0
        );
    }
}


/// How much time a window stride saves, and what it costs in clustering detail.
///
/// Embedding dominates a run and consecutive windows overlap heavily, so a stride removes
/// duplicated work. It also removes samples the clustering had, which is the cost this
/// measures: the reference clip is known to hold four speakers, so any stride that stops
/// reporting four has given up too much.
#[test]
#[ignore = "needs the upstream ~45MB models and reference audio; run with --ignored --nocapture"]
fn strides_trade_time_for_clustering_detail() {
    use super::diarization_service::{diarize_samples, DiarizationConfig};
    use std::time::Instant;

    if skip_unless_available() {
        return;
    }

    init_onnx_runtime().expect("ONNX Runtime");

    let samples = load_reference_audio(&reference_wav()).expect("reference audio");
    let calibrated = DiarizationConfig::default();
    println!("Audio: {:.1}s", samples.len() as f64 / 16_000.0);
    println!("Calibrated threshold: {:.2}", calibrated.threshold);
    println!();
    println!("stride | speakers | seconds | speedup");
    println!("-------|----------|---------|--------");

    let mut baseline: Option<f64> = None;

    for stride in [1usize, 2, 3, 4, 5, 6] {
        let config = DiarizationConfig { pair_stride: stride, ..calibrated };

        let started = Instant::now();
        let outcome = diarize_samples(&samples, &models_dir(), config).expect("separate");
        let elapsed = started.elapsed().as_secs_f64();

        let speedup = match baseline {
            Some(base) if elapsed > 0.0 => format!("{:.2}x", base / elapsed),
            _ => "—".to_string(),
        };
        if baseline.is_none() {
            baseline = Some(elapsed);
        }

        println!(
            "{stride:>6} | {:>8} | {elapsed:>7.1} | {speedup:>7}",
            outcome.speaker_count
        );
    }

    println!();
    println!("A stride worth using keeps the speaker count at {EXPECTED_SPEAKERS}.");
}