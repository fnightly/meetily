// audio/speaker_cluster.rs
//
// Clustering speaker embeddings into speakers.
//
// A diariser produces one embedding vector per speech segment; clustering turns
// those vectors into "there are three people in this meeting". This is our own
// algorithm and is kept dependency-free on purpose: the embedding *model* comes
// from ONNX (see the module docs below), but which vectors belong together is
// decided here, and it is the part that determines whether the transcript reads
// correctly.
//
// Runtime decision: the app already ships and dynamically loads ONNX Runtime for
// Parakeet (`ort` with `load-dynamic`), and that DLL exports the ONNX Runtime C
// API. Loading a second ONNX Runtime — which `sherpa-onnx` would bring — would
// put two copies of the same symbols in one process, a well-known source of
// runtime failures that compiles fine and only breaks during a real recording.
// So embeddings are produced through the runtime we already have, and this module
// does the clustering.
//
// Agglomerative clustering with average linkage and a cosine-distance threshold,
// which is the standard approach for speaker embeddings and, unlike k-means, does
// not need the number of speakers up front — nobody knows how many people will
// speak in a meeting.

/// One segment's embedding, in the model's own space.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentEmbedding {
    /// Index of the diarised segment this vector belongs to.
    pub segment: usize,
    pub vector: Vec<f32>,
}

/// Cosine distance below which two clusters are merged, calibrated for the
/// 3D-Speaker `eres2net` Chinese embedding model.
///
/// Measured, not guessed: clustering the upstream `0-four-speakers-zh.wav` reference
/// clip — which contains exactly four speakers — across the whole threshold range
/// gives a smooth, monotonic curve, and four clusters appear at **0.75**:
///
/// ```text
/// threshold  0.50  0.60  0.70  0.75  0.80
/// speakers      6     5     5     4     2
/// ```
///
/// The commonly cited 0.5 for this kind of model over-segments here, producing six
/// speakers for four people, because agglomerative centroid linkage and that model's
/// embedding spread differ from the reference implementation's own clustering.
///
/// This value belongs to the embedding model, so it must be re-measured if the model
/// changes. The band that yields four is roughly 0.71–0.79, which is narrow: with a
/// different recording, check the speaker count before trusting it.
pub const CALIBRATED_THRESHOLD: f32 = 0.75;

/// Clustering parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterParams {
    /// Cosine distance below which two clusters are merged.
    ///
    /// Cosine distance is `1 - cosine similarity`, so smaller means more similar.
    /// See [`CALIBRATED_THRESHOLD`] for how the default was measured.
    pub threshold: f32,
    /// Maximum number of speakers to report.
    ///
    /// A guard against a noisy recording producing dozens of "speakers". Set it high
    /// enough not to bind: a cap below the natural cluster count silently becomes a
    /// *floor*, returning that many speakers whether or not they exist.
    pub max_speakers: usize,
}

impl Default for ClusterParams {
    fn default() -> Self {
        Self {
            // The calibrated value for the embedding model this app uses, rather than
            // a generic guess. `max_speakers` is left well above the natural cluster
            // count so it does not bind and become a floor.
            threshold: CALIBRATED_THRESHOLD,
            max_speakers: 16,
        }
    }
}

/// Cosine similarity of two vectors, in `[-1, 1]`.
///
/// Returns 0.0 for a zero vector or a length mismatch rather than NaN, so a
/// degenerate embedding cannot poison the whole clustering run.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a <= 0.0 || norm_b <= 0.0 {
        return 0.0;
    }

    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Cosine distance, in `[0, 2]`. Zero means identical direction.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine_similarity(a, b)
}

/// A cluster of embeddings during the merge process.
#[derive(Debug, Clone)]
struct Cluster {
    /// Indices into the input embedding list.
    members: Vec<usize>,
}

impl Cluster {
    /// Mean vector, recomputed as members are added.
    ///
    /// Centroid linkage would also be defensible; the mean is used because it is
    /// what average-linkage distance is measured against here.
    fn centroid(&self, embeddings: &[SegmentEmbedding]) -> Vec<f32> {
        let dim = embeddings[self.members[0]].vector.len();
        let mut sum = vec![0.0f32; dim];

        for &member in &self.members {
            for (slot, value) in sum.iter_mut().zip(embeddings[member].vector.iter()) {
                *slot += value;
            }
        }

        let count = self.members.len() as f32;
        for slot in sum.iter_mut() {
            *slot /= count;
        }

        sum
    }
}

/// Cluster embeddings and return a speaker index per input embedding.
///
/// The returned `Vec<usize>` is parallel to `embeddings`, so the caller can pair
/// each embedding with its speaker directly. Speaker indices are renumbered from
/// zero in order of first appearance, which keeps "speaker 0" meaning the first
/// person heard rather than an artefact of the merge order.
pub fn cluster_embeddings(
    embeddings: &[SegmentEmbedding],
    params: ClusterParams,
) -> Vec<usize> {
    if embeddings.is_empty() {
        return Vec::new();
    }

    if embeddings.len() == 1 {
        return vec![0];
    }

    let mut clusters: Vec<Cluster> = (0..embeddings.len())
        .map(|index| Cluster {
            members: vec![index],
        })
        .collect();

    loop {
        if clusters.len() <= 1 {
            break;
        }

        // Find the closest pair of clusters.
        let mut best: Option<(usize, usize, f32)> = None;

        for i in 0..clusters.len() {
            let centroid_i = clusters[i].centroid(embeddings);
            for j in (i + 1)..clusters.len() {
                let centroid_j = clusters[j].centroid(embeddings);
                let distance = cosine_distance(&centroid_i, &centroid_j);

                let is_better = match best {
                    // Strictly closer, or equally close but an earlier pair, so the
                    // result does not depend on iteration order.
                    Some((best_i, best_j, best_distance)) => {
                        distance < best_distance
                            || (distance == best_distance && (i, j) < (best_i, best_j))
                    }
                    None => true,
                };

                if is_better {
                    best = Some((i, j, distance));
                }
            }
        }

        let Some((i, j, distance)) = best else {
            break;
        };

        // Merging reduces the number of clusters, so a *ceiling* on speakers can never
        // be met by stopping merges — stopping only ever leaves more. Two reasons to
        // merge, therefore:
        //
        //   1. the pair is closer than the threshold, so they are the same person;
        //   2. there are still more clusters than the ceiling allows, so keep merging
        //      the closest pair even though it is beyond the threshold.
        //
        // With (2) omitted, `max_speakers` acts as a floor: it returns exactly that
        // many speakers whether or not the recording contains them. That is how an
        // earlier version reported 16 speakers for a clip with four.
        let over_ceiling = params.max_speakers > 0 && clusters.len() > params.max_speakers;

        if distance > params.threshold && !over_ceiling {
            break;
        }

        // Merge j into i (j > i, so removing j does not shift i).
        let moved = clusters.remove(j);
        clusters[i].members.extend(moved.members);
    }

    // Renumber by first appearance so the labels are stable and human-orderable.
    let mut speaker_of_segment = vec![usize::MAX; embeddings.len()];
    let mut next_speaker = 0usize;

    for cluster in &clusters {
        let mut members = cluster.members.clone();
        members.sort_unstable();

        for member in members {
            speaker_of_segment[member] = next_speaker;
        }
        next_speaker += 1;
    }

    speaker_of_segment
        .into_iter()
        .map(|speaker| if speaker == usize::MAX { 0 } else { speaker })
        .collect()
}

/// Number of distinct speakers in an assignment.
pub fn count_speakers(assignment: &[usize]) -> usize {
    let mut seen: Vec<usize> = Vec::new();
    for &speaker in assignment {
        if !seen.contains(&speaker) {
            seen.push(speaker);
        }
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedding(segment: usize, vector: Vec<f32>) -> SegmentEmbedding {
        SegmentEmbedding { segment, vector }
    }

    #[test]
    fn cosine_similarity_of_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_ignores_magnitude() {
        // Embeddings are compared by direction, not length; otherwise a loud
        // segment would look unlike a quiet one from the same speaker.
        let a = vec![1.0, 0.0];
        let b = vec![5.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_opposite_vectors_is_minus_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_of_orthogonal_vectors_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn a_zero_vector_does_not_produce_nan() {
        // A degenerate embedding must not poison the whole run.
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
        assert!(!cosine_similarity(&a, &b).is_nan());
    }

    #[test]
    fn a_length_mismatch_does_not_produce_nan() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn identical_vectors_land_in_one_cluster() {
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0, 0.0]),
            embedding(1, vec![1.0, 0.0, 0.0]),
            embedding(2, vec![1.0, 0.0, 0.0]),
        ];

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());
        assert_eq!(assignment, vec![0, 0, 0]);
        assert_eq!(count_speakers(&assignment), 1);
    }

    #[test]
    fn clearly_different_vectors_land_in_separate_clusters() {
        // Orthogonal directions are maximally dissimilar, so nothing should merge.
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0]),
            embedding(1, vec![0.0, 1.0]),
        ];

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());
        assert_eq!(count_speakers(&assignment), 2);
        assert_ne!(assignment[0], assignment[1]);
    }

    #[test]
    fn two_alternating_speakers_are_separated() {
        // The realistic case: two people talking in turn.
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0, 0.0]),
            embedding(1, vec![0.0, 1.0, 0.0]),
            embedding(2, vec![0.99, 0.01, 0.0]),
            embedding(3, vec![0.01, 0.99, 0.0]),
        ];

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());

        assert_eq!(assignment[0], assignment[2], "the same speaker must group");
        assert_eq!(assignment[1], assignment[3], "the other speaker must group");
        assert_ne!(assignment[0], assignment[1]);
        assert_eq!(count_speakers(&assignment), 2);
    }

    #[test]
    fn speakers_are_numbered_by_first_appearance() {
        // "Speaker 0" should mean the first person heard, not an artefact of the
        // merge order.
        let embeddings = vec![
            embedding(0, vec![0.0, 1.0]),
            embedding(1, vec![1.0, 0.0]),
            embedding(2, vec![0.0, 1.0]),
        ];

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());
        assert_eq!(assignment[0], 0, "the first segment is speaker 0");
        assert_eq!(assignment[1], 1, "the second speaker is speaker 1");
        assert_eq!(assignment[2], 0);
    }

    #[test]
    fn a_single_embedding_is_one_speaker() {
        let embeddings = vec![embedding(0, vec![1.0, 2.0])];
        assert_eq!(cluster_embeddings(&embeddings, ClusterParams::default()), vec![0]);
    }

    #[test]
    fn no_embeddings_produces_no_assignment() {
        assert!(cluster_embeddings(&[], ClusterParams::default()).is_empty());
    }

    #[test]
    fn the_threshold_controls_how_readily_speakers_merge() {
        // Two vectors about 30 degrees apart: similar enough to merge at a loose
        // threshold, distinct enough to stay apart at a strict one.
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0]),
            embedding(1, vec![0.866, 0.5]),
        ];

        let strict = cluster_embeddings(
            &embeddings,
            ClusterParams {
                threshold: 0.05,
                max_speakers: 8,
            },
        );
        assert_eq!(count_speakers(&strict), 2, "a strict threshold keeps them apart");

        let loose = cluster_embeddings(
            &embeddings,
            ClusterParams {
                threshold: 0.9,
                max_speakers: 8,
            },
        );
        assert_eq!(count_speakers(&loose), 1, "a loose threshold merges them");
    }

    /// Eight mutually dissimilar voices: orthogonal unit vectors, so the cosine
    /// distance between any two is 1.0.
    fn eight_dissimilar() -> Vec<SegmentEmbedding> {
        (0..8)
            .map(|index| {
                let mut vector = vec![0.0f32; 8];
                vector[index] = 1.0;
                embedding(index, vector)
            })
            .collect()
    }

    #[test]
    fn the_speaker_ceiling_forces_merges_when_the_threshold_is_too_strict() {
        // At a strict threshold none of these voices merge, so the natural answer is
        // eight speakers. A ceiling of three must pull that down to three, which is the
        // only way a ceiling can be satisfied: merging is what reduces the count.
        let assignment = cluster_embeddings(
            &eight_dissimilar(),
            ClusterParams {
                threshold: 0.5,
                max_speakers: 3,
            },
        );

        assert_eq!(
            count_speakers(&assignment),
            3,
            "the ceiling must force merges down to the limit"
        );
    }

    #[test]
    fn the_speaker_ceiling_does_not_inflate_a_small_meeting() {
        // The regression this guards: treating the ceiling as a floor made eight
        // voices stay eight when the limit was eight, and would have reported sixteen
        // speakers for a four-person clip.
        let assignment = cluster_embeddings(
            &eight_dissimilar(),
            ClusterParams {
                threshold: 0.5,
                max_speakers: 20,
            },
        );

        assert_eq!(
            count_speakers(&assignment),
            8,
            "a high ceiling must not manufacture extra speakers"
        );
    }

    #[test]
    fn the_threshold_still_merges_below_the_ceiling() {
        // Identical voices merge regardless of the ceiling: the ceiling permits
        // merging, it does not prevent it.
        let identical: Vec<SegmentEmbedding> = (0..8)
            .map(|index| embedding(index, vec![1.0, 0.0]))
            .collect();

        let assignment = cluster_embeddings(
            &identical,
            ClusterParams {
                threshold: 0.5,
                max_speakers: 6,
            },
        );

        assert_eq!(
            count_speakers(&assignment),
            1,
            "identical voices must collapse to one speaker"
        );
    }

    #[test]
    fn a_zero_ceiling_is_ignored_rather_than_collapsing_everything() {
        // Zero means "no ceiling", not "no speakers". Without the guard, `len() > 0`
        // would keep merging until a single cluster remained.
        let assignment = cluster_embeddings(
            &eight_dissimilar(),
            ClusterParams {
                threshold: 0.5,
                max_speakers: 0,
            },
        );

        assert_eq!(
            count_speakers(&assignment),
            8,
            "a zero ceiling must mean unlimited, not zero"
        );
    }

    #[test]
    fn the_result_is_parallel_to_the_input() {
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0]),
            embedding(1, vec![0.0, 1.0]),
            embedding(2, vec![1.0, 0.0]),
        ];

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());
        assert_eq!(assignment.len(), embeddings.len());
    }

    #[test]
    fn clustering_is_deterministic_across_runs() {
        let embeddings = vec![
            embedding(0, vec![1.0, 0.0, 0.0]),
            embedding(1, vec![0.0, 1.0, 0.0]),
            embedding(2, vec![0.9, 0.1, 0.0]),
            embedding(3, vec![0.1, 0.9, 0.0]),
        ];

        let first = cluster_embeddings(&embeddings, ClusterParams::default());
        let second = cluster_embeddings(&embeddings, ClusterParams::default());

        assert_eq!(first, second);
    }

    #[test]
    fn the_default_is_the_calibrated_value() {
        // Not a change detector: this asserts that the default is the value measured
        // against the known four-speaker reference, so a casual edit to either one
        // without the other is caught.
        assert_eq!(ClusterParams::default().threshold, CALIBRATED_THRESHOLD);
    }

    #[test]
    fn the_default_speaker_cap_does_not_bind_for_a_small_meeting() {
        // A cap at or below the natural cluster count silently acts as a floor,
        // returning that many speakers whether or not they exist. The default must sit
        // above any plausible meeting.
        assert!(
            ClusterParams::default().max_speakers >= 8,
            "the default cap is low enough to become a floor"
        );
    }

    #[test]
    fn distinct_voices_are_not_merged_at_the_calibrated_threshold() {
        // Four orthogonal directions stand in for four speakers. They must stay apart
        // at the calibrated value, since it is a *loose* threshold (0.75 of a maximum
        // distance of 2) and being too loose would collapse the meeting into one voice.
        let embeddings: Vec<SegmentEmbedding> = (0..4)
            .map(|index| {
                let mut vector = vec![0.0f32; 4];
                vector[index] = 1.0;
                SegmentEmbedding { segment: index, vector }
            })
            .collect();

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());

        assert_eq!(
            count_speakers(&assignment),
            4,
            "four dissimilar voices collapsed at the calibrated threshold"
        );
    }

    #[test]
    fn one_voice_stays_together_at_the_calibrated_threshold() {
        // The other direction: mildly varying embeddings from the same speaker must
        // still merge, or every utterance becomes its own speaker.
        let embeddings: Vec<SegmentEmbedding> = (0..6)
            .map(|index| {
                let jitter = (index as f32) * 0.01;
                SegmentEmbedding {
                    segment: index,
                    vector: vec![1.0, jitter, 0.0],
                }
            })
            .collect();

        let assignment = cluster_embeddings(&embeddings, ClusterParams::default());

        assert_eq!(
            count_speakers(&assignment),
            1,
            "one speaker's own variation split it apart"
        );
    }

    #[test]
    fn count_speakers_counts_distinct_values() {
        assert_eq!(count_speakers(&[0, 0, 1, 1, 1]), 2);
        assert_eq!(count_speakers(&[]), 0);
        assert_eq!(count_speakers(&[3, 3, 3]), 1);
    }
}
