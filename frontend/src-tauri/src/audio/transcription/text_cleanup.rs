// audio/transcription/text_cleanup.rs
//
// Deterministic text handling for transcription output.
//
// Whisper returns segment text with padding whitespace, occasionally repeats
// punctuation, and — once a preceding-transcript prompt is supplied — can echo the
// end of that prompt at the start of the next segment. The live worker emits one
// transcript per VAD segment, so this cleanup has to happen before emission: the
// frontend and the saved transcript both read the same text.

/// Longest previous-transcript tail handed to the decoder as initial-prompt context.
///
/// Roughly 45 English words. The dictionary prompt already uses most of Whisper's
/// ~224-token prompt budget, so this is a hard ceiling rather than a target.
pub const MAX_CONTEXT_CHARS: usize = 180;

/// Overlap shorter than this is left alone: a single repeated word is normal speech
/// ("no, no"), and stripping it would silently delete real content.
const MIN_OVERLAP_WORDS: usize = 2;
const MAX_OVERLAP_WORDS: usize = 8;

/// Trim and collapse all whitespace runs (including newlines) to single spaces.
///
/// Zero-width characters are removed too: whisper.cpp occasionally emits them at
/// segment boundaries where they render as nothing but break string comparisons.
pub fn clean_segment(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for token in text.split_whitespace() {
        let token: String = token
            .chars()
            .filter(|character| !matches!(character, '​' | '﻿'))
            .collect();
        if token.is_empty() {
            continue;
        }
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        cleaned.push_str(&token);
    }
    cleaned
}

/// The last at most `max_chars` characters of a transcript, starting at a word
/// boundary when the text is whitespace-delimited.
///
/// Returns `None` for empty input or a zero budget so callers can skip prompting.
pub fn context_tail(text: &str, max_chars: usize) -> Option<String> {
    let cleaned = clean_segment(text);
    if cleaned.is_empty() || max_chars == 0 {
        return None;
    }

    let characters: Vec<char> = cleaned.chars().collect();
    if characters.len() <= max_chars {
        return Some(cleaned);
    }

    let start = characters.len() - max_chars;
    let mut tail: String = characters[start..].iter().collect();
    // Do not start mid-word: drop the partial leading word. CJK has no spaces, so
    // there is nothing to align to and the character slice is used as-is.
    if start > 0 && !characters[start - 1].is_whitespace() {
        if let Some(space) = tail.find(char::is_whitespace) {
            tail = tail[space..].trim_start().to_string();
        }
    }

    let tail = tail.trim().to_string();
    (!tail.is_empty()).then_some(tail)
}

/// Strip a duplicated prefix from `new_text` that already ends `previous`.
///
/// Requires at least two overlapping words so ordinary one-word repeats survive.
/// Used both when joining whisper segments and when the live worker receives the
/// next VAD segment, whose prompt context can leak into the decoder output.
pub fn strip_overlap_prefix(previous: Option<&str>, new_text: &str) -> String {
    let cleaned = clean_segment(new_text);
    let Some(previous) = previous else {
        return cleaned;
    };

    // CJK has no word separators, so the word-based overlap below never fires for
    // Chinese transcripts and a prompt echo would be emitted twice. Compare a
    // character prefix instead, requiring a longer overlap than a normal repeat.
    if let Some(stripped) = strip_cjk_overlap_prefix(previous, &cleaned) {
        return stripped;
    }

    let previous_words = normalized_words(previous);
    let new_words: Vec<&str> = cleaned.split_whitespace().collect();
    let max_overlap = MAX_OVERLAP_WORDS
        .min(previous_words.len())
        .min(new_words.len());
    if max_overlap < MIN_OVERLAP_WORDS {
        return cleaned;
    }

    // Normalise word-for-word: `normalized_words` drops punctuation-only tokens,
    // which would make this list shorter than `new_words` and break the slices.
    let new_normalized: Vec<String> = new_words.iter().map(|word| normalize_word(word)).collect();
    for overlap in (MIN_OVERLAP_WORDS..=max_overlap).rev() {
        if previous_words[previous_words.len() - overlap..] == new_normalized[..overlap] {
            let rest = new_words[overlap..].join(" ");
            return dedupe_leading_punctuation(previous, rest);
        }
    }

    cleaned
}

/// Append one segment to a running transcript, removing any overlap with what is
/// already there and a duplicated punctuation mark at the seam.
pub fn append_segment(result: &mut String, segment: &str) {
    let addition = strip_overlap_prefix(Some(result.as_str()), segment);
    let addition = dedupe_leading_punctuation(result, addition);
    let addition = addition.trim();
    if addition.is_empty() {
        return;
    }
    if needs_separator(result, addition) {
        result.push(' ');
    }
    result.push_str(addition);
}

/// Join whisper's per-segment text into one transcript with the same seam cleanup.
pub fn join_segments<'a>(segments: impl IntoIterator<Item = &'a str>) -> String {
    let mut joined = String::new();
    for segment in segments {
        append_segment(&mut joined, segment);
    }
    joined
}

/// Lower-cased word with surrounding punctuation removed, for overlap comparison.
fn normalize_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase()
}

/// Lower-cased words with surrounding punctuation removed, for overlap comparison.
fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3040..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
    )
}

fn strip_cjk_overlap_prefix(previous: &str, new_text: &str) -> Option<String> {
    if !new_text.chars().any(is_cjk) {
        return None;
    }

    let previous_chars: Vec<char> = previous.chars().collect();
    let new_chars: Vec<char> = new_text.chars().collect();
    let max = 40.min(previous_chars.len()).min(new_chars.len());
    const MIN_CJK_OVERLAP: usize = 4;
    if max < MIN_CJK_OVERLAP {
        return None;
    }

    for overlap in (MIN_CJK_OVERLAP..=max).rev() {
        if previous_chars[previous_chars.len() - overlap..] == new_chars[..overlap] {
            let rest: String = new_chars[overlap..].iter().collect();
            let rest = rest.trim().to_string();
            return Some(if rest.is_empty() {
                String::new()
            } else {
                dedupe_leading_punctuation(previous, rest)
            });
        }
    }

    None
}
fn needs_separator(previous: &str, addition: &str) -> bool {
    let Some(last) = previous.chars().last() else { return false; };
    let Some(first) = addition.chars().next() else { return false; };
    if matches!(last, '。' | '，' | '；' | '：' | '！' | '？' | '、' | '）' | '」' | '』') {
        return false;
    }
    if is_cjk(last) || is_cjk(first) {
        return false;
    }
    true
}
fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalize_word)
        .filter(|word| !word.is_empty())
        .collect()
}

/// Drop leading punctuation that duplicates the last punctuation mark before it.
fn dedupe_leading_punctuation(previous: &str, text: String) -> String {
    let Some(last) = previous.trim_end().chars().last() else {
        return text;
    };
    if !matches!(
        last,
        '.' | ',' | ';' | ':' | '!' | '?' | '。' | '，' | '；' | '：' | '！' | '？'
    ) {
        return text;
    }

    let mut rest = text.as_str();
    while let Some(first) = rest.chars().next() {
        if first != last {
            break;
        }
        rest = &rest[first.len_utf8()..];
    }
    rest.trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_segment_collapses_whitespace_and_zero_width_characters() {
        assert_eq!(clean_segment("  hello\n\tworld  "), "hello world");
        assert_eq!(clean_segment("one\u{200b}two"), "onetwo");
        assert_eq!(clean_segment(""), "");
        assert_eq!(clean_segment("   \n\t "), "");
    }

    #[test]
    fn join_segments_drops_a_shared_phrase_at_the_seam() {
        let joined = join_segments(["we discussed the budget", "the budget for next year"]);
        assert_eq!(joined, "we discussed the budget for next year");
    }

    #[test]
    fn join_segments_keeps_short_repeats_and_dedupes_punctuation() {
        // A single repeated word is normal speech and must survive.
        assert_eq!(join_segments(["no", "no, not that"]), "no no, not that");
        assert_eq!(join_segments(["That is done.", ". Next topic"]), "That is done. Next topic");
    }

    #[test]
    fn strip_overlap_prefix_requires_a_real_overlap() {
        assert_eq!(
            strip_overlap_prefix(Some("we discussed the budget"), "budget for next year"),
            "budget for next year"
        );
        assert_eq!(
            strip_overlap_prefix(Some("we discussed the budget"), "the budget for next year"),
            "for next year"
        );
        assert_eq!(strip_overlap_prefix(None, "  hello  "), "hello");
    }

    #[test]
    fn context_tail_is_bounded_and_starts_on_a_word() {
        let text = "one two three four five six seven eight";
        let tail = context_tail(text, 18).expect("tail");
        assert!(tail.chars().count() <= 18, "tail too long: {tail}");
        assert!(text.ends_with(&tail));
        assert!(tail.starts_with("four") || tail.starts_with("five") || tail.starts_with("six"));
        assert_eq!(context_tail("short", 180).as_deref(), Some("short"));
        assert_eq!(context_tail("   ", 180), None);
        assert_eq!(context_tail("short", 0), None);
    }

    #[test]
    fn cjk_text_is_preserved_without_spaces() {
        assert_eq!(clean_segment(" 你好 ， 世界 "), "你好 ， 世界");
        let joined = join_segments(["我们今天讨论了三件事。", "第一件是预算。"]);
        assert_eq!(joined, "我们今天讨论了三件事。第一件是预算。");
        assert_eq!(strip_overlap_prefix(Some("我们决定下周"), "我们决定下周再讨论预算"), "再讨论预算");
        assert!(!needs_separator("你好", "世界"));
        assert!(needs_separator("hello", "world"));
    }
}
