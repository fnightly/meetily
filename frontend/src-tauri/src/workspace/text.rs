//! Small text helpers shared by search and Q&A: case-insensitive matching on
//! characters (byte offsets differ between a string and its lowercase form for
//! some scripts), snippets, and query-term extraction for mixed Chinese/English.

/// Lowercase one character the way the matcher compares it.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Character index of the first case-insensitive occurrence of `needle`.
pub fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay: Vec<char> = haystack.chars().map(fold).collect();
    let pat: Vec<char> = needle.chars().map(fold).collect();
    if pat.is_empty() || pat.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - pat.len()).find(|&i| hay[i..i + pat.len()] == pat[..])
}

/// Number of non-overlapping case-insensitive occurrences.
pub fn count_ci(haystack: &str, needle: &str) -> usize {
    let hay: Vec<char> = haystack.chars().map(fold).collect();
    let pat: Vec<char> = needle.chars().map(fold).collect();
    if pat.is_empty() || pat.len() > hay.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + pat.len() <= hay.len() {
        if hay[i..i + pat.len()] == pat[..] {
            count += 1;
            i += pat.len();
        } else {
            i += 1;
        }
    }
    count
}

/// A window of text around the first match, with ellipses where it was cut.
pub fn snippet_around(text: &str, needle: &str, radius: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    let Some(at) = find_ci(&flat, needle) else {
        return truncate_chars(&flat, radius * 2);
    };
    let start = at.saturating_sub(radius);
    let end = (at + needle.chars().count() + radius).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Escape a value for `LIKE ? ESCAPE '\'`.
pub fn like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for c in value.chars() {
        if matches!(c, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push('%');
    escaped
}

/// An FTS5 phrase query for a literal string.
pub fn fts_phrase(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

const EN_STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "what", "who", "when",
    "where", "why", "how", "is", "are", "was", "were", "did", "do", "does", "we", "our", "you",
    "they", "it", "that", "this", "about", "last", "week", "meeting", "meetings", "any", "which",
    "be", "been", "have", "has", "had", "at", "by", "from", "as", "can", "could", "should", "will",
];

const ZH_STOP_BIGRAMS: &[&str] = &[
    "什么", "怎么", "我们", "你们", "他们", "是否", "一下", "哪些", "多少", "这个", "那个", "上周",
    "本周", "这周", "今天", "昨天", "会议", "开会", "时候", "是不", "不是", "有没", "没有", "一个",
    "的是", "了吗", "是什", "么时", "如何", "哪个", "谁负", "提到", "讨论", "说了", "关于", "情况",
];

/// Search terms for a natural-language question.
///
/// Chinese has no spaces, so each CJK run contributes its overlapping bigrams
/// (plus the whole run when it is short enough to be a name or term); Latin
/// text contributes its non-stopword words. Longer terms come first because
/// they are more selective.
pub fn question_terms(question: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut push = |t: String| {
        if !t.is_empty() && !terms.contains(&t) {
            terms.push(t);
        }
    };
    let mut cjk_run = String::new();
    let mut word = String::new();
    let flush_cjk = |run: &mut String, push: &mut dyn FnMut(String)| {
        let chars: Vec<char> = run.chars().collect();
        if chars.len() >= 2 && chars.len() <= 4 {
            let whole: String = chars.iter().collect();
            if !ZH_STOP_BIGRAMS.contains(&whole.as_str()) {
                push(whole);
            }
        }
        if chars.len() > 2 {
            for pair in chars.windows(2) {
                let bigram: String = pair.iter().collect();
                if !ZH_STOP_BIGRAMS.contains(&bigram.as_str()) {
                    push(bigram);
                }
            }
        }
        run.clear();
    };
    let flush_word = |word: &mut String, push: &mut dyn FnMut(String)| {
        let lower = word.to_lowercase();
        if lower.chars().count() >= 2 && !EN_STOPWORDS.contains(&lower.as_str()) {
            push(lower);
        }
        word.clear();
    };
    for c in question.chars() {
        if is_cjk(c) {
            if !word.is_empty() {
                flush_word(&mut word, &mut push);
            }
            cjk_run.push(c);
        } else if c.is_alphanumeric() {
            if !cjk_run.is_empty() {
                flush_cjk(&mut cjk_run, &mut push);
            }
            word.push(c);
        } else {
            if !cjk_run.is_empty() {
                flush_cjk(&mut cjk_run, &mut push);
            }
            if !word.is_empty() {
                flush_word(&mut word, &mut push);
            }
        }
    }
    if !cjk_run.is_empty() {
        flush_cjk(&mut cjk_run, &mut push);
    }
    if !word.is_empty() {
        flush_word(&mut word, &mut push);
    }
    terms.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    terms.truncate(16);
    terms
}

/// Recording-relative `[mm:ss]`, or the wall-clock timestamp when unknown.
pub fn clock(seconds: Option<f64>, fallback: &str) -> String {
    match seconds {
        Some(s) if s >= 0.0 => {
            let total = s as u64;
            format!("{:02}:{:02}", total / 60, total % 60)
        }
        _ => fallback.to_string(),
    }
}

/// The visible markdown of a stored summary result (JSON or legacy text).
pub fn summary_markdown(raw: &str) -> Option<String> {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return (!raw.trim().is_empty()).then(|| raw.to_string()),
    };
    if let Some(md) = value.get("markdown").and_then(|v| v.as_str()) {
        return (!md.trim().is_empty()).then(|| md.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_insensitive_matching_counts_chars() {
        assert_eq!(find_ci("Hello Meetily", "meetily"), Some(6));
        assert_eq!(find_ci("讨论了估值和条款", "估值"), Some(3));
        assert_eq!(count_ci("估值 估值 估", "估值"), 2);
        assert_eq!(find_ci("abc", ""), None);
    }

    #[test]
    fn snippet_has_ellipses_when_cut() {
        let text = "前面有很多很多的内容然后我们讨论了估值的问题后面还有很多内容";
        let s = snippet_around(text, "估值", 4);
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("估值"));
    }

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("50%_x"), "%50\\%\\_x%");
    }

    #[test]
    fn question_terms_mix_chinese_and_english() {
        let terms = question_terms("上周和 Acme 客户谈的价格是多少？");
        assert!(terms.contains(&"acme".to_string()));
        assert!(terms.contains(&"价格".to_string()));
        assert!(terms.contains(&"客户".to_string()));
        assert!(!terms.contains(&"多少".to_string()));
        assert!(!terms.contains(&"上周".to_string()));
    }

    #[test]
    fn summary_markdown_reads_json_field() {
        assert_eq!(summary_markdown(r###"{"markdown":"## 决议"}"###).as_deref(), Some("## 决议"));
        assert_eq!(summary_markdown(r#"{"other":1}"#), None);
    }
}
