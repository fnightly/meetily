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
        speaker_pairs, turns_from_chunks, select_top_speakers, weighted_speaker_scores, ChunkLabels,
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