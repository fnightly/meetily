// dictionary.rs
//
// A user-maintained vocabulary of terms the speech model gets wrong.
//
// Borrowed from OpenWhispr's custom dictionary: dictated text is full of proper
// nouns — colleagues' names, product names, acronyms, internal jargon — that a
// speech model reliably mangles, and no general model can know them. Telling the
// model the exact spelling up front is far more reliable than correcting the text
// afterwards.
//
// Chinese makes this sharper than English: names that differ by one character are
// different people, and a model guessing at homophones produces plausible but
// wrong characters that a reader will not notice.
//
// The vocabulary is a plain term list plus optional corrections. Nothing here is
// language-specific — it helps English jargon as much as Chinese names — but the
// motivation and the examples are Chinese-first.

use serde::{Deserialize, Serialize};

/// Maximum number of entries kept.
///
/// Every entry is injected into prompts, so an unbounded list would silently
/// consume the model's context and cost. This is generous for a personal
/// vocabulary while keeping the prompt a predictable size.
pub const MAX_ENTRIES: usize = 200;

/// Maximum length of a single term, in characters.
pub const MAX_TERM_CHARS: usize = 64;

/// One vocabulary entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntry {
    /// The exact spelling to use.
    pub term: String,
    /// Optional: what the model tends to produce instead.
    ///
    /// When present the instruction becomes "heard X, write Y", which is a much
    /// stronger signal than a bare term list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misheard_as: Option<String>,
}

impl DictionaryEntry {
    pub fn term(term: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            misheard_as: None,
        }
    }

    pub fn correction(term: impl Into<String>, misheard_as: impl Into<String>) -> Self {
        Self {
            term: term.into(),
            misheard_as: Some(misheard_as.into()),
        }
    }
}

/// The user's vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDictionary {
    #[serde(default)]
    pub entries: Vec<DictionaryEntry>,
}

impl UserDictionary {
    pub fn new(entries: Vec<DictionaryEntry>) -> Self {
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Normalise a dictionary coming from the UI or disk.
    ///
    /// Trims whitespace, drops entries with an empty term, and removes exact
    /// duplicates. A blank term would render as an empty instruction, and a
    /// duplicated term wastes context.
    pub fn sanitized(mut self) -> Self {
        let mut seen: Vec<String> = Vec::new();
        let mut cleaned: Vec<DictionaryEntry> = Vec::new();

        for entry in self.entries.drain(..) {
            let term = entry.term.trim().to_string();
            if term.is_empty() {
                continue;
            }

            let misheard_as = entry
                .misheard_as
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty() && value != &term);

            // Case-insensitive dedupe: "Meetily" and "meetily" are the same term.
            let key = term.to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);

            cleaned.push(DictionaryEntry { term, misheard_as });
        }

        cleaned.truncate(MAX_ENTRIES);
        self.entries = cleaned;
        self
    }

    /// Render the vocabulary as an instruction block, or `None` when there is
    /// nothing to say.
    ///
    /// Returns `None` rather than an empty block so callers can append
    /// unconditionally without leaving a dangling heading in the prompt.
    pub fn render_instruction(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }

        let mut lines = Vec::with_capacity(self.entries.len());

        for entry in &self.entries {
            match &entry.misheard_as {
                Some(misheard) => {
                    lines.push(format!("- 听到「{misheard}」时，写作「{}」", entry.term))
                }
                None => lines.push(format!("- 「{}」", entry.term)),
            }
        }

        Some(format!(
            "\n\n自定义词典：以下专有名词必须按下述精确写法输出，不要改写、不要翻译、不要用同音字替代：\n{}",
            lines.join("\n")
        ))
    }
}

/// Sanitise one term the way `sanitized` does, for UI validation.
///
/// Returns `None` when the term is unusable.
pub fn normalize_term(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(MAX_TERM_CHARS).collect())
}

// ---------------------------------------------------------------------------
// Persistence and commands
// ---------------------------------------------------------------------------

use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

/// Store file and key, following the same convention as the storage config.
const DICTIONARY_STORE: &str = "dictionary.json";
const DICTIONARY_KEY: &str = "dictionary";

// ---------------------------------------------------------------------------
// Active vocabulary for the local engines.
//
// Cloud transcription and summaries read the dictionary through the AppHandle,
// but the Whisper/Parakeet engines run without one. The last loaded or saved
// dictionary is mirrored here so local transcription benefits too: Whisper gets
// the terms as its initial prompt, and every local engine (Parakeet has no
// prompt input) gets the "misheard as → term" corrections applied to its text.
// ---------------------------------------------------------------------------

static ACTIVE_DICTIONARY: once_cell::sync::Lazy<std::sync::RwLock<UserDictionary>> =
    once_cell::sync::Lazy::new(|| std::sync::RwLock::new(UserDictionary::default()));

/// Replace the dictionary the local engines use.
pub fn set_active_dictionary(dictionary: &UserDictionary) {
    if let Ok(mut guard) = ACTIVE_DICTIONARY.write() {
        *guard = dictionary.clone();
    }
}

/// Terms as a Whisper initial prompt, bounded so it stays well inside the
/// decoder's prompt budget (~224 tokens; CJK terms cost about one per char).
pub fn active_vocabulary_prompt() -> Option<String> {
    let guard = ACTIVE_DICTIONARY.read().ok()?;
    vocabulary_prompt(&guard, 160)
}

pub(crate) fn vocabulary_prompt(dictionary: &UserDictionary, max_chars: usize) -> Option<String> {
    let mut prompt = String::new();
    for entry in &dictionary.entries {
        let term = entry.term.trim();
        if term.is_empty() {
            continue;
        }
        let extra = term.chars().count() + if prompt.is_empty() { 0 } else { 1 };
        if prompt.chars().count() + extra > max_chars {
            break;
        }
        if !prompt.is_empty() {
            // An ideographic comma reads as natural context for Chinese and is
            // harmless for Latin terms.
            prompt.push('、');
        }
        prompt.push_str(term);
    }
    (!prompt.is_empty()).then_some(prompt)
}

/// Apply the active dictionary's corrections to engine output.
pub fn apply_active_corrections(text: &str) -> String {
    match ACTIVE_DICTIONARY.read() {
        Ok(guard) => apply_corrections(&guard, text),
        Err(_) => text.to_string(),
    }
}

/// Replace every "misheard as" variant with its exact term.
///
/// Latin variants match case-insensitively on word boundaries so "meetly" fixes
/// "Meetly" but not "meetlyness"; CJK variants match as plain substrings since
/// Chinese has no word spacing.
pub fn apply_corrections(dictionary: &UserDictionary, text: &str) -> String {
    let mut out = text.to_string();
    for entry in &dictionary.entries {
        let Some(misheard) = entry.misheard_as.as_deref().map(str::trim) else {
            continue;
        };
        if misheard.is_empty() || misheard == entry.term {
            continue;
        }
        if misheard.chars().all(|c| c.is_ascii()) {
            let pattern = format!(r"(?i)\b{}\b", regex::escape(misheard));
            if let Ok(re) = regex::Regex::new(&pattern) {
                out = re.replace_all(&out, regex::NoExpand(entry.term.as_str())).into_owned();
            }
        } else {
            out = out.replace(misheard, &entry.term);
        }
    }
    out
}


/// Read the persisted vocabulary, falling back to empty on any problem.
///
/// Infallible on purpose, matching the storage config: a missing or corrupt
/// dictionary must never stop the app from working, because an empty vocabulary
/// simply means no extra guidance in the prompt.
pub fn load_dictionary<R: Runtime>(app: &AppHandle<R>) -> UserDictionary {
    let store = match app.store(DICTIONARY_STORE) {
        Ok(store) => store,
        Err(e) => {
            log::warn!("Failed to access dictionary store: {e}; using an empty dictionary");
            return UserDictionary::default();
        }
    };

    match store.get(DICTIONARY_KEY) {
        Some(value) => match serde_json::from_value::<UserDictionary>(value.clone()) {
            Ok(dictionary) => {
                let dictionary = dictionary.sanitized();
                set_active_dictionary(&dictionary);
                dictionary
            }
            Err(e) => {
                log::warn!("Failed to deserialize dictionary: {e}; using an empty dictionary");
                UserDictionary::default()
            }
        },
        None => UserDictionary::default(),
    }
}

/// Persist the vocabulary, sanitising it first.
pub fn save_dictionary<R: Runtime>(
    app: &AppHandle<R>,
    dictionary: &UserDictionary,
) -> Result<UserDictionary, String> {
    let cleaned = dictionary.clone().sanitized();

    let store = app
        .store(DICTIONARY_STORE)
        .map_err(|e| format!("Failed to access dictionary store: {e}"))?;

    let value = serde_json::to_value(&cleaned)
        .map_err(|e| format!("Failed to serialise dictionary: {e}"))?;

    store.set(DICTIONARY_KEY, value);

    store
        .save()
        .map_err(|e| format!("Failed to save dictionary: {e}"))?;

    log::info!("Saved dictionary with {} entries", cleaned.len());
    set_active_dictionary(&cleaned);

    Ok(cleaned)
}

/// The user's custom vocabulary.
#[tauri::command]
pub async fn get_dictionary<R: Runtime>(app: AppHandle<R>) -> Result<UserDictionary, String> {
    Ok(load_dictionary(&app))
}

/// Replace the custom vocabulary.
///
/// Returns the sanitised result so the UI shows exactly what was stored rather
/// than what was typed.
#[tauri::command]
pub async fn set_dictionary<R: Runtime>(
    app: AppHandle<R>,
    dictionary: UserDictionary,
) -> Result<UserDictionary, String> {
    save_dictionary(&app, &dictionary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_dictionary_renders_nothing() {
        // Returning None lets callers append the block unconditionally without
        // leaving a dangling "自定义词典：" heading.
        assert!(UserDictionary::default().render_instruction().is_none());
        assert!(UserDictionary::default().is_empty());
    }

    #[test]
    fn a_bare_term_is_rendered_as_an_exact_spelling() {
        let dictionary = UserDictionary::new(vec![DictionaryEntry::term("Meetily")]);
        let rendered = dictionary.render_instruction().expect("instruction");

        assert!(rendered.contains("Meetily"));
        assert!(rendered.contains("自定义词典"));
    }

    #[test]
    fn a_correction_renders_both_sides() {
        // "heard X, write Y" is a much stronger signal than a bare term.
        let dictionary = UserDictionary::new(vec![DictionaryEntry::correction("张伟", "张伟伟")]);
        let rendered = dictionary.render_instruction().expect("instruction");

        assert!(rendered.contains("张伟伟"), "the misheard form must appear");
        assert!(rendered.contains("张伟"), "the correct form must appear");
        assert!(rendered.contains("听到"), "the correction phrasing is missing");
    }

    #[test]
    fn chinese_terms_are_preserved_exactly() {
        let dictionary = UserDictionary::new(vec![
            DictionaryEntry::term("腾讯会议"),
            DictionaryEntry::term("飞书"),
        ]);
        let rendered = dictionary.render_instruction().expect("instruction");

        assert!(rendered.contains("腾讯会议"));
        assert!(rendered.contains("飞书"));
    }

    #[test]
    fn sanitize_trims_and_drops_blank_terms() {
        let dictionary = UserDictionary::new(vec![
            DictionaryEntry::term("  Meetily  "),
            DictionaryEntry::term("   "),
            DictionaryEntry::term(""),
        ])
        .sanitized();

        assert_eq!(dictionary.len(), 1);
        assert_eq!(dictionary.entries[0].term, "Meetily");
    }

    #[test]
    fn sanitize_removes_exact_duplicates() {
        let dictionary = UserDictionary::new(vec![
            DictionaryEntry::term("Qwen"),
            DictionaryEntry::term("Qwen"),
        ])
        .sanitized();

        assert_eq!(dictionary.len(), 1);
    }

    #[test]
    fn sanitize_dedupes_case_insensitively() {
        let dictionary = UserDictionary::new(vec![
            DictionaryEntry::term("Meetily"),
            DictionaryEntry::term("meetily"),
        ])
        .sanitized();

        assert_eq!(dictionary.len(), 1);
        assert_eq!(dictionary.entries[0].term, "Meetily", "the first spelling wins");
    }

    #[test]
    fn sanitize_drops_a_correction_that_repeats_the_term() {
        // "heard X, write X" is noise.
        let dictionary =
            UserDictionary::new(vec![DictionaryEntry::correction("飞书", "飞书")]).sanitized();

        assert_eq!(dictionary.entries[0].misheard_as, None);
    }

    #[test]
    fn sanitize_caps_the_entry_count() {
        // An unbounded list would silently consume the model's context.
        let entries = (0..MAX_ENTRIES + 50)
            .map(|index| DictionaryEntry::term(format!("term-{index}")))
            .collect();

        let dictionary = UserDictionary::new(entries).sanitized();
        assert_eq!(dictionary.len(), MAX_ENTRIES);
    }

    #[test]
    fn sanitize_trims_the_misheard_form_too() {
        let dictionary =
            UserDictionary::new(vec![DictionaryEntry::correction("飞书", "  飞舒  ")]).sanitized();

        assert_eq!(dictionary.entries[0].misheard_as.as_deref(), Some("飞舒"));
    }

    #[test]
    fn normalize_term_rejects_blank_input() {
        assert_eq!(normalize_term(""), None);
        assert_eq!(normalize_term("   "), None);
        assert_eq!(normalize_term("  ok  "), Some("ok".to_string()));
    }

    #[test]
    fn normalize_term_truncates_overlong_input() {
        let long = "字".repeat(MAX_TERM_CHARS + 20);
        let normalized = normalize_term(&long).expect("term");
        assert_eq!(normalized.chars().count(), MAX_TERM_CHARS);
    }

    #[test]
    fn the_dictionary_round_trips_through_json() {
        // Preferences are persisted as JSON, so the shape must be stable.
        let dictionary = UserDictionary::new(vec![
            DictionaryEntry::term("Meetily"),
            DictionaryEntry::correction("张伟", "张伟伟"),
        ]);

        let json = serde_json::to_string(&dictionary).expect("serialize");
        let restored: UserDictionary = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(restored, dictionary);
    }

    #[test]
    fn a_dictionary_saved_without_the_optional_field_still_loads() {
        // `misheard_as` is optional, so older or hand-written JSON must parse.
        let json = r#"{"entries":[{"term":"Meetily"}]}"#;
        let dictionary: UserDictionary = serde_json::from_str(json).expect("deserialize");

        assert_eq!(dictionary.entries[0].misheard_as, None);
    }

    #[test]
    fn an_empty_json_object_loads_as_an_empty_dictionary() {
        let dictionary: UserDictionary = serde_json::from_str("{}").expect("deserialize");
        assert!(dictionary.is_empty());
    }
}
