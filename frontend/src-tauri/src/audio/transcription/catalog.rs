// audio/transcription/catalog.rs
//
// Pluggable speech-to-text provider catalog.
//
// Before this, the set of usable providers was implicit in `match
// config.provider.as_str()` arms scattered across `engine.rs`, `worker.rs` and
// the frontend `modelOptions` map. Adding a provider meant editing all of them,
// and nothing recorded what a provider actually supports — which is how
// Parakeet came to be labelled "Recommended" while being English-only.
//
// This module makes the provider set data: adding a provider means adding one
// entry to `PROVIDERS` and (if it should really transcribe) implementing
// `TranscriptionProvider`.

use serde::Serialize;

/// Whether a provider runs on this machine or sends audio to a third party.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Local,
    Cloud,
}

/// How far along a provider's implementation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Fully usable today.
    Available,
    /// Wired into settings, but audio is deliberately not uploaded yet.
    NotImplemented,
}

/// Static description of a speech-to-text provider.
#[derive(Debug, Clone, Serialize)]
pub struct SpeechProviderDescriptor {
    /// Stable id used in settings and config (`localWhisper`, `parakeet`, ...).
    pub id: &'static str,
    /// Name shown in the UI.
    pub display_name: &'static str,
    pub kind: ProviderKind,
    pub status: ProviderStatus,
    /// `None` means "any language". `Some(list)` are ISO 639-1 codes, where an
    /// empty list means the provider does not accept a language hint at all.
    pub supported_languages: Option<&'static [&'static str]>,
    /// True when the provider needs an API key to be configured.
    pub requires_api_key: bool,
    /// Suggested model names, used to populate the model picker.
    pub models: &'static [&'static str],
    /// Short explanation shown under the option in the UI.
    pub notes: &'static str,
    /// Where to obtain an API key (cloud providers only).
    pub api_key_url: Option<&'static str>,
}

/// The full provider catalog.
///
/// Order matters: the UI lists providers in this order, and the first
/// local provider is the default for new installs.
pub const PROVIDERS: &[SpeechProviderDescriptor] = &[
    SpeechProviderDescriptor {
        id: "parakeet",
        display_name: "Parakeet",
        kind: ProviderKind::Local,
        status: ProviderStatus::Available,
        // NeMo Parakeet TDT 0.6B is English-only and does not take a language
        // hint. Declaring that here is what stops the UI recommending it for
        // non-English meetings.
        supported_languages: Some(&["en"]),
        requires_api_key: false,
        models: &["parakeet-tdt-0.6b-v2"],
        notes: "Fast local transcription. English only — use Whisper for other languages.",
        api_key_url: None,
    },
    SpeechProviderDescriptor {
        id: "localWhisper",
        display_name: "Local Whisper",
        kind: ProviderKind::Local,
        status: ProviderStatus::Available,
        // Whisper is multilingual; `None` means no restriction.
        supported_languages: None,
        requires_api_key: false,
        models: &["tiny", "base", "small", "medium", "large-v3", "large-v3-turbo"],
        notes: "Multilingual local transcription. Slower than Parakeet.",
        api_key_url: None,
    },
    SpeechProviderDescriptor {
        id: "deepgram",
        display_name: "Deepgram",
        kind: ProviderKind::Cloud,
        status: ProviderStatus::NotImplemented,
        supported_languages: None,
        requires_api_key: true,
        models: &["nova-2", "nova-2-phonecall"],
        notes: "Cloud provider. Not wired up yet — selecting it does not upload audio.",
        api_key_url: Some("https://console.deepgram.com/"),
    },
    SpeechProviderDescriptor {
        id: "elevenLabs",
        display_name: "ElevenLabs",
        kind: ProviderKind::Cloud,
        status: ProviderStatus::NotImplemented,
        supported_languages: None,
        requires_api_key: true,
        models: &["eleven_multilingual_v2"],
        notes: "Cloud provider. Not wired up yet — selecting it does not upload audio.",
        api_key_url: Some("https://elevenlabs.io/app/settings/api-keys"),
    },
    SpeechProviderDescriptor {
        id: "groq",
        display_name: "Groq",
        kind: ProviderKind::Cloud,
        status: ProviderStatus::NotImplemented,
        supported_languages: None,
        requires_api_key: true,
        models: &["whisper-large-v3", "whisper-large-v3-turbo"],
        notes: "Cloud provider. Not wired up yet — selecting it does not upload audio.",
        api_key_url: Some("https://console.groq.com/keys"),
    },
    SpeechProviderDescriptor {
        id: "openai",
        display_name: "OpenAI",
        kind: ProviderKind::Cloud,
        status: ProviderStatus::NotImplemented,
        supported_languages: None,
        requires_api_key: true,
        models: &["whisper-1", "gpt-4o-transcribe"],
        notes: "Cloud provider. Not wired up yet — selecting it does not upload audio.",
        api_key_url: Some("https://platform.openai.com/api-keys"),
    },
];

/// Look up a provider by id.
pub fn find_provider(id: &str) -> Option<&'static SpeechProviderDescriptor> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// The provider used when nothing is configured.
pub fn default_provider_id() -> &'static str {
    "parakeet"
}

/// Where this provider sends audio, for a privacy confirmation in the UI.
pub fn is_cloud(id: &str) -> bool {
    matches!(
        find_provider(id).map(|p| p.kind),
        Some(ProviderKind::Cloud)
    )
}

/// Whether the provider can be used to transcribe right now.
pub fn is_implemented(id: &str) -> bool {
    matches!(
        find_provider(id).map(|p| p.status),
        Some(ProviderStatus::Available)
    )
}

/// Normalise a language hint (`None`/empty/"auto" all mean "let the engine decide").
fn normalized_language(language: Option<&str>) -> Option<&str> {
    language
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("auto") && !l.eq_ignore_ascii_case("auto-translate"))
}

/// Whether a provider can transcribe the given language.
///
/// Returns `Ok(())` when the provider accepts the language (including when no
/// hint is given), or an explanatory error naming a better provider. This is the
/// check that turns "Parakeet silently produces garbage for Chinese" into a
/// message the user can act on.
pub fn validate_language(provider_id: &str, language: Option<&str>) -> Result<(), String> {
    let Some(provider) = find_provider(provider_id) else {
        return Err(format!("Unknown speech provider '{}'", provider_id));
    };

    let Some(requested) = normalized_language(language) else {
        // No explicit language: the engine will auto-detect.
        return Ok(());
    };

    let Some(allowed) = provider.supported_languages else {
        return Ok(());
    };

    // Base language only: "zh-tw" should match a provider supporting "zh".
    let primary = requested
        .split(['-', '_'])
        .next()
        .unwrap_or(requested)
        .to_ascii_lowercase();

    if allowed.iter().any(|l| l.eq_ignore_ascii_case(&primary)) {
        return Ok(());
    }

    let alternatives: Vec<&str> = PROVIDERS
        .iter()
        .filter(|p| p.status == ProviderStatus::Available)
        .filter(|p| p.supported_languages.is_none())
        .map(|p| p.id)
        .collect();

    let hint = if alternatives.is_empty() {
        String::new()
    } else {
        format!(" Use one of these instead: {}.", alternatives.join(", "))
    };

    Err(format!(
        "{} does not support language '{}' (supported: {}).{}",
        provider.display_name,
        requested,
        allowed.join(", "),
        hint
    ))
}

/// Whether switching to this provider is allowed.
///
/// Cloud providers stay selectable in the UI but are refused here, which is the
/// single chokepoint that guarantees no audio is uploaded while their
/// implementation is marked `NotImplemented`.
pub fn ensure_selectable(provider_id: &str) -> Result<(), String> {
    let Some(provider) = find_provider(provider_id) else {
        return Err(format!(
            "Unknown speech provider '{}'. Available: {}",
            provider_id,
            PROVIDERS
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };

    if provider.status == ProviderStatus::NotImplemented {
        return Err(format!(
            "{} is not available yet. It is reserved in settings but does not send audio anywhere.",
            provider.display_name
        ));
    }

    Ok(())
}

/// Pick a provider that can actually handle `language`.
///
/// This exists because a user's configured provider may not support the language
/// they are about to speak: Parakeet is the default and is English-only, so a
/// Chinese meeting used to run through an English-only model and produce
/// unusable text. Rather than fail the recording (or silently degrade), fall back
/// to a provider that supports the language.
///
/// Preference order:
/// 1. the configured provider, when it accepts the language;
/// 2. the first selectable provider that accepts it.
///
/// `is_ready` lets the caller exclude providers that are not actually usable on
/// this machine (e.g. no model downloaded yet), so we never "fix" the language
/// problem by switching to a provider that cannot run.
///
/// Returns `None` when nothing can handle the language.
pub fn select_provider_for_language<F>(
    configured_id: &str,
    language: Option<&str>,
    is_ready: F,
) -> Option<&'static str>
where
    F: Fn(&str) -> bool,
{
    let is_usable = |p: &SpeechProviderDescriptor| -> bool {
        p.status == ProviderStatus::Available && is_ready(p.id)
    };

    // Preferred: keep the user's choice when it works.
    if let Some(configured) = find_provider(configured_id) {
        if is_usable(configured) && validate_language(configured.id, language).is_ok() {
            return Some(configured.id);
        }
    }

    // Fallback: first usable provider that accepts this language. Catalog order
    // puts Parakeet first, so a multilingual request naturally lands on Whisper.
    PROVIDERS
        .iter()
        .find(|p| is_usable(p) && validate_language(p.id, language).is_ok())
        .map(|p| p.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_exists_and_is_usable() {
        let id = default_provider_id();
        assert!(find_provider(id).is_some(), "default provider must exist");
        assert!(is_implemented(id), "default provider must be selectable");
    }

    #[test]
    fn every_cloud_provider_is_marked_not_implemented() {
        // The safety property: while cloud ASR is unimplemented, none of them
        // may be selectable, so no audio can leave the machine.
        for provider in PROVIDERS.iter().filter(|p| p.kind == ProviderKind::Cloud) {
            assert_eq!(
                provider.status,
                ProviderStatus::NotImplemented,
                "{} is cloud but selectable",
                provider.id
            );
            assert!(
                ensure_selectable(provider.id).is_err(),
                "{} must be refused",
                provider.id
            );
            assert!(provider.requires_api_key, "{} needs a key", provider.id);
            assert!(provider.api_key_url.is_some(), "{} needs a key URL", provider.id);
        }
    }

    #[test]
    fn local_providers_are_selectable_and_need_no_key() {
        for provider in PROVIDERS.iter().filter(|p| p.kind == ProviderKind::Local) {
            assert_eq!(provider.status, ProviderStatus::Available);
            assert!(ensure_selectable(provider.id).is_ok());
            assert!(!provider.requires_api_key, "{} needs no key", provider.id);
        }
    }

    #[test]
    fn parakeet_rejects_non_english_and_suggests_whisper() {
        let err = validate_language("parakeet", Some("zh")).expect_err("zh must be rejected");
        assert!(err.contains("English only") || err.contains("supported"), "got: {err}");
        assert!(err.contains("localWhisper"), "should point at a multilingual provider: {err}");
    }

    #[test]
    fn parakeet_accepts_english_and_any_case_or_region() {
        assert!(validate_language("parakeet", Some("en")).is_ok());
        assert!(validate_language("parakeet", Some("EN")).is_ok());
        assert!(validate_language("parakeet", Some("en-US")).is_ok());
    }

    #[test]
    fn whisper_accepts_every_language() {
        for lang in ["zh", "zh-tw", "ja", "de", "ar", "hi"] {
            assert!(
                validate_language("localWhisper", Some(lang)).is_ok(),
                "whisper should accept {lang}"
            );
        }
    }

    #[test]
    fn auto_and_empty_language_mean_no_restriction() {
        for lang in [None, Some(""), Some("  "), Some("auto"), Some("auto-translate")] {
            assert!(
                validate_language("parakeet", lang).is_ok(),
                "parakeet should accept {lang:?} as 'no hint'"
            );
        }
    }

    #[test]
    fn unknown_provider_is_reported_with_available_ids() {
        let err = ensure_selectable("nope").unwrap_err();
        assert!(err.contains("nope"));
        assert!(err.contains("parakeet"), "should list valid ids: {err}");
    }

    #[test]
    fn validate_language_on_unknown_provider_fails_cleanly() {
        assert!(validate_language("nope", Some("en")).is_err());
    }

    #[test]
    fn provider_ids_are_unique() {
        let mut ids: Vec<&str> = PROVIDERS.iter().map(|p| p.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate provider id in catalog");
    }

    #[test]
    fn is_cloud_matches_kind() {
        assert!(is_cloud("deepgram"));
        assert!(!is_cloud("parakeet"));
        assert!(!is_cloud("localWhisper"));
    }

    // --- provider auto-selection -------------------------------------------------

    #[test]
    fn select_keeps_configured_provider_when_it_supports_the_language() {
        // English + Parakeet: no reason to switch.
        assert_eq!(
            select_provider_for_language("parakeet", Some("en"), |_| true),
            Some("parakeet")
        );
    }

    #[test]
    fn select_switches_away_from_english_only_provider_for_chinese() {
        // The regression this guards: Parakeet + Chinese used to proceed and
        // produce unusable output.
        assert_eq!(
            select_provider_for_language("parakeet", Some("zh"), |_| true),
            Some("localWhisper")
        );
    }

    #[test]
    fn select_switches_for_other_non_english_languages() {
        for lang in ["ja", "ko", "de", "ar", "zh-tw"] {
            assert_eq!(
                select_provider_for_language("parakeet", Some(lang), |_| true),
                Some("localWhisper"),
                "language {lang} should fall back to Whisper"
            );
        }
    }

    #[test]
    fn select_does_not_switch_when_the_replacement_is_not_ready() {
        // Parakeet cannot do Chinese and Whisper has no model downloaded: there
        // is no valid answer, and we must not pretend otherwise.
        let ready = |id: &str| id == "parakeet";
        assert_eq!(
            select_provider_for_language("parakeet", Some("zh"), ready),
            None
        );
    }

    #[test]
    fn select_prefers_configured_provider_even_if_others_are_ready() {
        let all_ready = |_: &str| true;
        assert_eq!(
            select_provider_for_language("localWhisper", Some("zh"), all_ready),
            Some("localWhisper")
        );
    }

    #[test]
    fn select_ignores_unready_configured_provider() {
        // Configured is Whisper but its model is missing; Parakeet is ready but
        // cannot do Chinese -> no usable provider.
        let ready = |id: &str| id == "parakeet";
        assert_eq!(
            select_provider_for_language("localWhisper", Some("zh"), ready),
            None
        );
    }

    #[test]
    fn select_never_returns_an_unimplemented_provider() {
        let all_ready = |_: &str| true;
        let chosen = select_provider_for_language("deepgram", Some("en"), all_ready);
        assert_eq!(chosen, Some("parakeet"));
        assert!(is_implemented(chosen.unwrap()));
    }

    #[test]
    fn select_with_no_language_hint_keeps_the_configured_provider() {
        // `auto` means "detect", so no capability mismatch can be assumed.
        let all_ready = |_: &str| true;
        for lang in [None, Some("auto"), Some("")] {
            assert_eq!(
                select_provider_for_language("parakeet", lang, all_ready),
                Some("parakeet"),
                "language {lang:?} should not force a switch"
            );
        }
    }

    #[test]
    fn select_reports_no_provider_when_nothing_is_ready() {
        assert_eq!(
            select_provider_for_language("parakeet", Some("en"), |_| false),
            None
        );
    }
}
