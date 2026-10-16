// audio/transcript_text.rs
//
// Language-aware validation of transcriber output.
//
// The existing cleanup in `whisper_engine` assumes space-delimited words: it
// splits on whitespace and bails out when fewer than three "words" remain. Chinese
// does not separate words with spaces, so an entire sentence counts as one word,
// the early return fires, and **none** of the repetition or garbage handling runs
// for Chinese output.
//
// That leaves two well-known Chinese failure modes unfiltered:
//   - punctuation-only output, which Whisper emits over silence ("。" or "。。。");
//   - repetition loops, where the decoder gets stuck repeating a phrase.
//
// The checks here work on characters, so they apply to any script.

/// Collapse runs of whitespace, matching how the transcript is stored.
pub fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Punctuation that carries no content on its own.
///
/// Includes the full-width Chinese forms, which are what actually appear in a
/// Chinese transcript.
const PUNCTUATION: &[char] = &[
    '.', ',', '!', '?', ';', ':', '\'', '"', '`', '~', '-', '_', '(', ')', '[', ']', '{', '}',
    '<', '>', '/', '\\', '|', '*', '#', '@', '$', '%', '^', '&', '+', '=',
    // Full-width and CJK punctuation.
    '。', '，', '！', '？', '；', '：', '、', '…', '—', '－', '～', '·', '「', '」', '『', '』',
    '（', '）', '《', '》', '〈', '〉', '【', '】', '〔', '〕', '“', '”', '‘', '’',
];

/// Whether a character is punctuation-only for our purposes.
pub fn is_punctuation(c: char) -> bool {
    PUNCTUATION.contains(&c) || (c.is_ascii_punctuation() && !c.is_alphanumeric())
}

/// Whether the text carries no actual content.
///
/// True for empty input and for text made up entirely of punctuation and
/// whitespace. A lone Chinese full stop is the single most common hallucination
/// over silence, so this is the check that matters most for Chinese audio.
pub fn is_content_free(text: &str) -> bool {
    text.trim().is_empty() || text.chars().all(|c| c.is_whitespace() || is_punctuation(c))
}

/// Minimum number of characters in a repeated unit.
///
/// Four because four-character phrases are idiomatic in Chinese ("谢谢大家"), and
/// requiring more would miss real loops in Chinese audio.
const MIN_REPEAT_UNIT_CHARS: usize = 4;

/// How many consecutive repeats make a phrase a decoder loop rather than speech.
const MIN_REPEAT_COUNT: usize = 3;

/// The longest repeated unit to look for, in non-whitespace characters.
const MAX_REPEAT_UNIT_CHARS: usize = 40;

/// A located repetition loop.
struct RepetitionLoop {
    /// Byte offset in the original text where the second repeat starts.
    repeat_start: usize,
    /// Byte offset in the original text just past the last repeat.
    repeat_end: usize,
}

/// Locate a consecutive repetition loop.
///
/// Matching runs over a whitespace-stripped view of the text, because whitespace
/// should not decide whether a phrase is repeating — Chinese has none, and a
/// spaced language may or may not have it between repeats. Offsets are then
/// mapped back to the original string so the caller can slice it safely, which
/// is what keeps this correct for multi-byte text.
fn find_repetition_loop(text: &str) -> Option<RepetitionLoop> {
    // (char, byte offset in the original)
    let significant: Vec<(char, usize)> = text
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(offset, c)| (c, offset))
        .collect();

    let len = significant.len();
    if len < MIN_REPEAT_UNIT_CHARS * MIN_REPEAT_COUNT {
        return None;
    }

    let max_unit = (len / MIN_REPEAT_COUNT).min(MAX_REPEAT_UNIT_CHARS);

    for unit_len in MIN_REPEAT_UNIT_CHARS..=max_unit {
        let unit: Vec<char> = significant[..unit_len].iter().map(|(c, _)| *c).collect();

        let mut repeats = 1;
        while (repeats + 1) * unit_len <= len {
            let start = repeats * unit_len;
            let matches = (0..unit_len).all(|i| significant[start + i].0 == unit[i]);
            if !matches {
                break;
            }
            repeats += 1;
        }

        if repeats >= MIN_REPEAT_COUNT {
            // The second repeat begins at significant[unit_len]; everything from
            // there through the last repeat is the redundancy.
            return Some(RepetitionLoop {
                repeat_start: significant[unit_len].1,
                repeat_end: significant[repeats * unit_len - 1].1
                    + significant[repeats * unit_len - 1].0.len_utf8(),
            });
        }
    }

    None
}

/// Whether a phrase repeats consecutively enough times to be a decoder loop.
///
/// Compared by characters, so it works for scripts without spaces — which is the
/// whole point, since Chinese does not delimit words.
pub fn has_repetition_loop(text: &str) -> bool {
    find_repetition_loop(text).is_some()
}

/// Collapse a repetition loop down to a single copy of the repeated unit.
///
/// Returns the normalised text when there is no loop, so callers can apply this
/// unconditionally. Any speech after the loop is kept.
pub fn collapse_repetition(text: &str) -> String {
    match find_repetition_loop(text) {
        None => normalize_whitespace(text),
        Some(loop_span) => {
            let mut result = String::with_capacity(text.len());
            result.push_str(&text[..loop_span.repeat_start]);
            result.push_str(&text[loop_span.repeat_end..]);
            normalize_whitespace(&result)
        }
    }
}

/// Prepare a transcript segment for storage.
///
/// Returns `None` when the segment carries no content and should be dropped.
pub fn sanitize_segment(text: &str) -> Option<String> {
    if is_content_free(text) {
        return None;
    }

    let collapsed = collapse_repetition(text);
    let normalized = normalize_whitespace(&collapsed);

    if is_content_free(&normalized) {
        return None;
    }

    Some(normalized)
}

/// Collapse adjacent duplicate segments, which Whisper emits when a phrase is
/// decoded twice across a chunk boundary.
pub fn dedupe_adjacent<'a, I>(segments: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut result: Vec<String> = Vec::new();

    for segment in segments {
        let Some(clean) = sanitize_segment(segment) else {
            continue;
        };

        // Compare on a case-folded copy: "Thank you" and "thank you" are the same
        // duplicate for this purpose.
        let is_duplicate = result
            .last()
            .is_some_and(|previous| previous.to_lowercase() == clean.to_lowercase());

        if !is_duplicate {
            result.push(clean);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_collapsed() {
        assert_eq!(normalize_whitespace("  a   b \n c "), "a b c");
        assert_eq!(normalize_whitespace(""), "");
    }

    #[test]
    fn chinese_punctuation_counts_as_punctuation() {
        for c in ['。', '，', '！', '？', '；', '：', '、', '…', '—', '（', '）', '《', '》'] {
            assert!(is_punctuation(c), "{c} should be punctuation");
        }
    }

    #[test]
    fn content_free_detects_empty_and_punctuation_only() {
        assert!(is_content_free(""));
        assert!(is_content_free("   "));
        assert!(is_content_free("."));
        assert!(is_content_free("..."));
        // The classic Chinese hallucination over silence.
        assert!(is_content_free("。"));
        assert!(is_content_free("。。。"));
        assert!(is_content_free("，。！？"));
        assert!(is_content_free(" , . ! "));
    }

    #[test]
    fn content_free_is_false_for_real_text() {
        assert!(!is_content_free("你好"));
        assert!(!is_content_free("hello"));
        assert!(!is_content_free("好。"));
        assert!(!is_content_free("2026 年"));
    }

    #[test]
    fn repetition_loop_detection_works_without_spaces() {
        // Chinese has no word boundaries, which is exactly why the existing
        // whitespace-based cleanup never fired.
        assert!(has_repetition_loop("谢谢大家谢谢大家谢谢大家"));
        assert!(has_repetition_loop("这是一个很长的重复句子这是一个很长的重复句子这是一个很长的重复句子"));
    }

    #[test]
    fn repetition_loop_detection_works_for_spaced_languages() {
        assert!(has_repetition_loop("thank you very much thank you very much thank you very much"));
    }

    #[test]
    fn ordinary_speech_is_not_a_loop() {
        // A person repeating a short word is not a decoder failure.
        assert!(!has_repetition_loop("谢谢，谢谢"));
        assert!(!has_repetition_loop("好的好的"));
        assert!(!has_repetition_loop("我们下周再讨论这个问题"));
        assert!(!has_repetition_loop(""));
    }

    #[test]
    fn collapse_keeps_one_copy_of_the_loop() {
        assert_eq!(collapse_repetition("谢谢大家谢谢大家谢谢大家"), "谢谢大家");
        assert_eq!(
            collapse_repetition("hello world hello world hello world"),
            "hello world"
        );
    }

    #[test]
    fn collapse_preserves_trailing_real_speech() {
        // Anything after the loop is genuine audio and must survive.
        let collapsed = collapse_repetition("谢谢大家谢谢大家谢谢大家然后我们继续");
        assert!(collapsed.starts_with("谢谢大家"));
        assert!(collapsed.contains("然后我们继续"), "got: {collapsed}");
    }

    #[test]
    fn collapse_leaves_normal_text_untouched() {
        let normal = "我们今天讨论了三件事";
        assert_eq!(collapse_repetition(normal), normal);

        let long = "这是一段没有任何重复的正常会议内容，我们讨论了项目进度和下一步安排。";
        assert_eq!(collapse_repetition(long), long);
    }

    #[test]
    fn sanitize_drops_punctuation_only_segments() {
        // Dropping these is what stops a silent stretch producing a transcript
        // full of full stops.
        assert_eq!(sanitize_segment("。"), None);
        assert_eq!(sanitize_segment("。。。"), None);
        assert_eq!(sanitize_segment("   "), None);
        assert_eq!(sanitize_segment(""), None);
    }

    #[test]
    fn sanitize_keeps_and_normalizes_real_segments() {
        assert_eq!(sanitize_segment("  你好  世界 "), Some("你好 世界".to_string()));
        assert_eq!(
            sanitize_segment("谢谢大家谢谢大家谢谢大家"),
            Some("谢谢大家".to_string())
        );
    }

    #[test]
    fn dedupe_removes_adjacent_repeats_across_chunks() {
        let segments = ["你好", "你好", "世界", "世界", "世界"];
        assert_eq!(dedupe_adjacent(segments), vec!["你好", "世界"]);
    }

    #[test]
    fn dedupe_is_case_insensitive() {
        assert_eq!(dedupe_adjacent(["Hello", "hello"]), vec!["Hello"]);
    }

    #[test]
    fn dedupe_drops_content_free_segments_entirely() {
        let segments = ["你好", "。", "世界"];
        assert_eq!(dedupe_adjacent(segments), vec!["你好", "世界"]);
    }

    #[test]
    fn dedupe_keeps_non_adjacent_repeats() {
        // The same phrase said again later is real speech.
        assert_eq!(dedupe_adjacent(["好的", "然后", "好的"]), vec!["好的", "然后", "好的"]);
    }

    #[test]
    fn dedupe_on_empty_input_is_empty() {
        let empty: [&str; 0] = [];
        assert!(dedupe_adjacent(empty).is_empty());
    }
}
