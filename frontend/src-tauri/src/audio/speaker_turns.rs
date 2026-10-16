// audio/speaker_turns.rs
//
// Speaker attribution: assigning a speaker to each transcript segment.
//
// Diarisation produces speaker turns — a list of `(start, end, speaker)` spans —
// while transcription produces text segments with their own audio offsets. This
// module joins the two.
//
// It is deliberately dependency-free. The diarisation itself comes from a model
// (sherpa-onnx provides offline segmentation, embedding and clustering in Rust),
// but *joining* its output to the transcript is this app's own logic and the part
// that decides whether the result reads correctly. Keeping it separate means the
// join can be tested exhaustively without a model, a GPU, or an audio file.

use serde::{Deserialize, Serialize};

/// One diarised speaker turn, in seconds from the start of the recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerTurn {
    pub start: f64,
    pub end: f64,
    /// Zero-based cluster index as produced by the diariser.
    pub speaker: u32,
}

impl SpeakerTurn {
    pub fn new(start: f64, end: f64, speaker: u32) -> Self {
        Self {
            start,
            end,
            speaker,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// A transcript segment needing a speaker, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSpan {
    pub start: f64,
    pub end: f64,
}

/// How a segment's speaker was decided, so a caller can tell a confident
/// attribution from a fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// The segment overlapped one or more turns; the best overlap won.
    Overlap,
    /// The segment landed in a gap between turns and inherited the nearest
    /// speaker, because a segment between two turns is almost always a
    /// continuation of one of them.
    Nearest,
    /// No turns were available at all.
    Unknown,
}

/// One attributed segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributionResult {
    pub speaker: Option<u32>,
    pub attribution: Attribution,
    /// Seconds of overlap with the chosen speaker, for diagnostics.
    pub overlap_seconds: f64,
}

/// Overlap between two spans, in seconds. Never negative.
pub fn overlap_seconds(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    (end - start).max(0.0)
}

/// Whether two spans share any time at all.
pub fn overlaps(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    overlap_seconds(a_start, a_end, b_start, b_end) > 0.0
}

/// Distance in seconds between two spans; zero when they overlap.
pub fn gap_seconds(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    if overlaps(a_start, a_end, b_start, b_end) {
        return 0.0;
    }

    if a_end <= b_start {
        b_start - a_end
    } else {
        a_start - b_end
    }
}

/// Total overlap per speaker for a segment.
///
/// A segment can straddle a speaker change — one person finishing a sentence and
/// another starting — so the winner is decided by total overlap rather than by
/// whichever turn happens to be examined first.
pub fn overlap_by_speaker(segment: &TranscriptSpan, turns: &[SpeakerTurn]) -> Vec<(u32, f64)> {
    let mut totals: Vec<(u32, f64)> = Vec::new();

    for turn in turns {
        let overlap = overlap_seconds(segment.start, segment.end, turn.start, turn.end);
        if overlap <= 0.0 {
            continue;
        }

        match totals.iter_mut().find(|(speaker, _)| *speaker == turn.speaker) {
            Some((_, total)) => *total += overlap,
            None => totals.push((turn.speaker, overlap)),
        }
    }

    totals
}

/// Decide the speaker for one transcript segment.
///
/// Preference order:
///   1. the speaker with the most overlap;
///   2. failing any overlap, the speaker of the nearest turn — a segment landing
///      in a short gap belongs to the turn beside it, and leaving it unattributed
///      would punch holes in the transcript;
///   3. otherwise unknown.
///
/// Ties are broken by the lower speaker index so the result is deterministic
/// across runs rather than depending on iteration order.
pub fn attribute_segment(segment: &TranscriptSpan, turns: &[SpeakerTurn]) -> AttributionResult {
    if turns.is_empty() {
        return AttributionResult {
            speaker: None,
            attribution: Attribution::Unknown,
            overlap_seconds: 0.0,
        };
    }

    let mut totals = overlap_by_speaker(segment, turns);

    if !totals.is_empty() {
        totals.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        let (speaker, overlap) = totals[0];
        return AttributionResult {
            speaker: Some(speaker),
            attribution: Attribution::Overlap,
            overlap_seconds: overlap,
        };
    }

    // No overlap: fall back to the closest turn either side.
    let mut nearest: Option<(u32, f64)> = None;

    for turn in turns {
        let gap = gap_seconds(segment.start, segment.end, turn.start, turn.end);
        nearest = match nearest {
            Some((current_speaker, current_gap)) => {
                if gap < current_gap || (gap == current_gap && turn.speaker < current_speaker) {
                    Some((turn.speaker, gap))
                } else {
                    Some((current_speaker, current_gap))
                }
            }
            None => Some((turn.speaker, gap)),
        };
    }

    match nearest {
        Some((speaker, _)) => AttributionResult {
            speaker: Some(speaker),
            attribution: Attribution::Nearest,
            overlap_seconds: 0.0,
        },
        None => AttributionResult {
            speaker: None,
            attribution: Attribution::Unknown,
            overlap_seconds: 0.0,
        },
    }
}

/// Attribute every segment, returning the speaker for each in order.
pub fn attribute_segments(
    segments: &[TranscriptSpan],
    turns: &[SpeakerTurn],
) -> Vec<AttributionResult> {
    segments
        .iter()
        .map(|segment| attribute_segment(segment, turns))
        .collect()
}

/// Number of distinct speakers in a set of turns.
pub fn speaker_count(turns: &[SpeakerTurn]) -> usize {
    let mut seen: Vec<u32> = Vec::new();
    for turn in turns {
        if !seen.contains(&turn.speaker) {
            seen.push(turn.speaker);
        }
    }
    seen.len()
}

/// Sort turns by start time.
///
/// Diarisers do not guarantee ordering, and the nearest-turn fallback depends on
/// a well-ordered list to produce a stable result.
pub fn sorted_turns(mut turns: Vec<SpeakerTurn>) -> Vec<SpeakerTurn> {
    turns.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.speaker.cmp(&b.speaker))
    });
    turns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64) -> TranscriptSpan {
        TranscriptSpan { start, end }
    }

    #[test]
    fn overlap_is_computed_for_partial_and_full_containment() {
        assert_eq!(overlap_seconds(0.0, 10.0, 2.0, 4.0), 2.0);
        assert_eq!(overlap_seconds(0.0, 10.0, 0.0, 10.0), 10.0);
        assert_eq!(overlap_seconds(2.0, 4.0, 0.0, 10.0), 2.0);
    }

    #[test]
    fn disjoint_spans_have_no_overlap() {
        assert_eq!(overlap_seconds(0.0, 1.0, 2.0, 3.0), 0.0);
        assert!(!overlaps(0.0, 1.0, 2.0, 3.0));
    }

    #[test]
    fn touching_spans_do_not_overlap() {
        // A segment ending exactly where the next begins is not an overlap.
        assert_eq!(overlap_seconds(0.0, 1.0, 1.0, 2.0), 0.0);
    }

    #[test]
    fn gap_is_zero_for_overlapping_spans() {
        assert_eq!(gap_seconds(0.0, 5.0, 3.0, 8.0), 0.0);
    }

    #[test]
    fn gap_measures_the_distance_between_disjoint_spans() {
        assert_eq!(gap_seconds(0.0, 1.0, 3.0, 4.0), 2.0);
        assert_eq!(gap_seconds(3.0, 4.0, 0.0, 1.0), 2.0);
    }

    #[test]
    fn a_segment_inside_one_turn_takes_that_speaker() {
        let turns = vec![SpeakerTurn::new(0.0, 5.0, 0), SpeakerTurn::new(5.0, 10.0, 1)];
        let result = attribute_segment(&span(1.0, 3.0), &turns);

        assert_eq!(result.speaker, Some(0));
        assert_eq!(result.attribution, Attribution::Overlap);
        assert_eq!(result.overlap_seconds, 2.0);
    }

    #[test]
    fn a_straddling_segment_takes_the_speaker_with_more_overlap() {
        // Someone finishing a sentence while the next person starts: the longer
        // half should win rather than whichever turn is listed first.
        let turns = vec![SpeakerTurn::new(0.0, 5.0, 0), SpeakerTurn::new(5.0, 10.0, 1)];

        let mostly_first = attribute_segment(&span(3.0, 7.0), &turns);
        assert_eq!(mostly_first.speaker, Some(0));

        let mostly_second = attribute_segment(&span(4.0, 9.0), &turns);
        assert_eq!(mostly_second.speaker, Some(1));
    }

    #[test]
    fn a_straddling_segment_sums_overlap_per_speaker() {
        // One speaker can own several turns; the totals must be combined rather
        // than compared turn by turn.
        let turns = vec![
            SpeakerTurn::new(0.0, 2.0, 0),
            SpeakerTurn::new(2.0, 3.0, 1),
            SpeakerTurn::new(3.0, 5.0, 0),
        ];

        let totals = overlap_by_speaker(&span(0.0, 5.0), &turns);
        let speaker_zero = totals.iter().find(|(s, _)| *s == 0).expect("speaker 0");
        let speaker_one = totals.iter().find(|(s, _)| *s == 1).expect("speaker 1");

        assert_eq!(speaker_zero.1, 4.0);
        assert_eq!(speaker_one.1, 1.0);
        assert_eq!(attribute_segment(&span(0.0, 5.0), &turns).speaker, Some(0));
    }

    #[test]
    fn a_segment_in_a_gap_inherits_the_nearest_speaker() {
        // Leaving it unattributed would punch a hole in the transcript.
        let turns = vec![SpeakerTurn::new(0.0, 4.0, 0), SpeakerTurn::new(6.0, 10.0, 1)];
        let result = attribute_segment(&span(4.5, 5.0), &turns);

        assert_eq!(result.speaker, Some(0));
        assert_eq!(result.attribution, Attribution::Nearest);
    }

    #[test]
    fn the_nearest_fallback_works_on_both_sides() {
        let turns = vec![SpeakerTurn::new(0.0, 4.0, 0), SpeakerTurn::new(6.0, 10.0, 1)];

        assert_eq!(attribute_segment(&span(4.2, 4.8), &turns).speaker, Some(0));
        assert_eq!(attribute_segment(&span(5.2, 5.8), &turns).speaker, Some(1));
    }

    #[test]
    fn no_turns_means_unknown_rather_than_a_guess() {
        let result = attribute_segment(&span(0.0, 5.0), &[]);
        assert_eq!(result.speaker, None);
        assert_eq!(result.attribution, Attribution::Unknown);
    }

    #[test]
    fn ties_are_broken_deterministically() {
        // Two speakers overlap exactly equally, so the tie-break decides the
        // answer. Only the tie-breaking rule makes this reproducible.
        let a = vec![SpeakerTurn::new(0.0, 2.0, 3), SpeakerTurn::new(0.0, 2.0, 1)];
        let b = vec![SpeakerTurn::new(0.0, 2.0, 1), SpeakerTurn::new(0.0, 2.0, 3)];

        let first = attribute_segment(&span(0.0, 2.0), &a);
        let second = attribute_segment(&span(0.0, 2.0), &b);

        // Equal overlap on both sides, so this really is exercising the tie-break.
        assert_eq!(first.overlap_seconds, 2.0);
        assert_eq!(second.overlap_seconds, 2.0);

        assert_eq!(first.speaker, second.speaker, "must be order-independent");
        assert_eq!(first.speaker, Some(1), "the lower speaker index wins");
    }

    #[test]
    fn attributing_a_list_preserves_order() {
        let turns = vec![SpeakerTurn::new(0.0, 5.0, 0), SpeakerTurn::new(5.0, 10.0, 1)];
        let segments = vec![span(0.0, 2.0), span(6.0, 8.0), span(1.0, 3.0)];

        let results = attribute_segments(&segments, &turns);
        let speakers: Vec<_> = results.iter().map(|r| r.speaker).collect();

        assert_eq!(speakers, vec![Some(0), Some(1), Some(0)]);
    }

    #[test]
    fn speaker_count_counts_distinct_speakers() {
        let turns = vec![
            SpeakerTurn::new(0.0, 1.0, 0),
            SpeakerTurn::new(1.0, 2.0, 0),
            SpeakerTurn::new(2.0, 3.0, 2),
        ];
        assert_eq!(speaker_count(&turns), 2);
        assert_eq!(speaker_count(&[]), 0);
    }

    #[test]
    fn turns_are_sorted_by_start() {
        let turns = vec![
            SpeakerTurn::new(5.0, 6.0, 1),
            SpeakerTurn::new(0.0, 1.0, 0),
            SpeakerTurn::new(2.0, 3.0, 2),
        ];

        let starts: Vec<f64> = sorted_turns(turns).iter().map(|t| t.start).collect();
        assert_eq!(starts, vec![0.0, 2.0, 5.0]);
    }

    #[test]
    fn sorting_is_stable_for_equal_starts() {
        let turns = vec![
            SpeakerTurn::new(1.0, 2.0, 5),
            SpeakerTurn::new(1.0, 2.0, 2),
        ];

        let speakers: Vec<u32> = sorted_turns(turns).iter().map(|t| t.speaker).collect();
        assert_eq!(speakers, vec![2, 5]);
    }

    #[test]
    fn a_zero_length_turn_has_no_duration() {
        assert_eq!(SpeakerTurn::new(1.0, 1.0, 0).duration(), 0.0);
        // A malformed turn must not report a negative duration.
        assert_eq!(SpeakerTurn::new(2.0, 1.0, 0).duration(), 0.0);
    }

    #[test]
    fn a_segment_with_no_temporal_extent_still_resolves() {
        // A zero-length segment cannot overlap, so it falls back to nearest.
        let turns = vec![SpeakerTurn::new(0.0, 5.0, 0)];
        let result = attribute_segment(&span(2.0, 2.0), &turns);

        assert_eq!(result.speaker, Some(0));
        assert_eq!(result.attribution, Attribution::Nearest);
    }
}
