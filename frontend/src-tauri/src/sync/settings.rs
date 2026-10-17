//! Persisted settings for the Obsidian and Notion sync targets.
//!
//! Follows the storage-settings pattern: one JSON store in the app config
//! directory, every field optional so an older file keeps loading, and an
//! infallible read because a corrupt settings file must never stop the app.
//!
//! The Notion integration token is deliberately **not** here. It is a secret and
//! is stored with the other API keys (see `SettingsRepository`), so this file
//! can be read, logged or backed up without copying the token around.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

use super::obsidian::sanitize_subfolder;

/// Name of the JSON store that persists these settings.
pub const SYNC_SETTINGS_STORE: &str = "sync_settings.json";
/// Key inside that store.
pub const SYNC_SETTINGS_KEY: &str = "settings";

/// Frontmatter tags are a convenience, not a tagging system: keep the list and
/// each tag short enough to stay readable in a note.
const MAX_TAGS: usize = 20;
const MAX_TAG_CHARS: usize = 64;
/// A template is pasted text; cap it so a stray large paste cannot turn every
/// synced note into a megabyte file.
const MAX_TEMPLATE_CHARS: usize = 20_000;

/// Everything the Obsidian / Notion targets need to know that is not a secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSettings {
    /// Obsidian vault folder. `None` means "use the notes folder from storage
    /// settings", which is what the user already configured.
    #[serde(default)]
    pub obsidian_folder: Option<String>,
    /// Optional folder inside the vault, e.g. `Meetings/2026`.
    #[serde(default)]
    pub obsidian_subfolder: Option<String>,
    /// Optional note body template; placeholders are documented in the UI.
    #[serde(default)]
    pub obsidian_template: Option<String>,
    /// Tags written into the YAML frontmatter.
    #[serde(default)]
    pub obsidian_tags: Vec<String>,
    /// Notion database the pages are created in (the plain 32-character id).
    #[serde(default)]
    pub notion_database_id: Option<String>,
}

/// What the settings panel and the sync dialog render.
#[derive(Debug, Serialize)]
pub struct SyncSettingsView {
    pub settings: SyncSettings,
    /// The notes folder from storage settings, shown as the installed default.
    pub default_obsidian_folder: String,
    /// The folder notes would be written to right now (before the subfolder).
    pub effective_obsidian_folder: String,
    /// The folder notes are actually written to, including the subfolder.
    pub obsidian_notes_folder: String,
    /// Whether a Notion token is stored. The token itself is never returned.
    pub notion_token_set: bool,
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Read the persisted settings, falling back to defaults on any problem.
pub fn load_sync_settings<R: Runtime>(app: &AppHandle<R>) -> SyncSettings {
    let store = match app.store(SYNC_SETTINGS_STORE) {
        Ok(store) => store,
        Err(error) => {
            log::warn!("Failed to access sync settings store: {error}; using defaults");
            return SyncSettings::default();
        }
    };

    match store.get(SYNC_SETTINGS_KEY) {
        Some(value) => match serde_json::from_value::<SyncSettings>(value.clone()) {
            Ok(settings) => settings,
            Err(error) => {
                log::warn!("Failed to deserialize sync settings: {error}; using defaults");
                SyncSettings::default()
            }
        },
        None => SyncSettings::default(),
    }
}

/// Persist the settings, already sanitised by the caller.
pub fn save_sync_settings<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SyncSettings,
) -> Result<(), String> {
    let store = app
        .store(SYNC_SETTINGS_STORE)
        .map_err(|error| format!("Failed to access sync settings store: {error}"))?;

    let value = serde_json::to_value(settings)
        .map_err(|error| format!("Failed to serialize sync settings: {error}"))?;

    store.set(SYNC_SETTINGS_KEY, value);
    store
        .save()
        .map_err(|error| format!("Failed to persist sync settings: {error}"))?;

    // No token in this structure, so the settings are safe to log.
    log::info!(
        "Persisted sync settings: obsidian_folder={:?}, subfolder={:?}, tags={:?}, notion_database_set={}",
        settings.obsidian_folder,
        settings.obsidian_subfolder,
        settings.obsidian_tags,
        settings.notion_database_id.is_some()
    );

    Ok(())
}

/// Clean user input before it is stored.
///
/// The UI also filters input, but settings can arrive from a hand-edited store,
/// an older version, or a future caller; validating here keeps every writer
/// (and therefore every sync) on the same rules.
pub fn sanitize_settings(mut settings: SyncSettings) -> SyncSettings {
    settings.obsidian_folder = non_empty(&settings.obsidian_folder).map(str::to_string);
    settings.obsidian_subfolder = sanitize_subfolder(settings.obsidian_subfolder.as_deref());
    settings.obsidian_template = settings
        .obsidian_template
        .as_deref()
        .map(str::trim)
        .filter(|template| !template.is_empty())
        .map(|template| template.chars().take(MAX_TEMPLATE_CHARS).collect());
    settings.obsidian_tags = normalize_tags(&settings.obsidian_tags);
    settings.notion_database_id = non_empty(&settings.notion_database_id).map(str::to_string);
    settings
}

/// Normalise frontmatter tags: no leading `#`, no blanks, no duplicates.
///
/// Deduplication is case-insensitive because Obsidian treats `Meeting` and
/// `meeting` as the same tag; keeping both would show one and hide the other.
pub fn normalize_tags(raw: &[String]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for entry in raw {
        for piece in entry.split([',', '，', ';', '；']) {
            let tag = piece.trim().trim_start_matches('#').trim();
            if tag.is_empty() {
                continue;
            }

            let tag: String = tag.chars().take(MAX_TAG_CHARS).collect();
            let key = tag.to_lowercase();
            if seen.contains(&key) {
                continue;
            }

            seen.push(key);
            tags.push(tag);
            if tags.len() >= MAX_TAGS {
                return tags;
            }
        }
    }

    tags
}

/// The configured vault folder, or the notes folder from storage settings.
pub fn effective_obsidian_folder<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SyncSettings,
) -> Result<PathBuf, String> {
    if let Some(folder) = non_empty(&settings.obsidian_folder) {
        return Ok(PathBuf::from(folder));
    }

    crate::storage::load_storage_config(app).notes_folder(app)
}

/// The folder notes are written to, including the configured subfolder.
pub fn obsidian_notes_folder<R: Runtime>(
    app: &AppHandle<R>,
    settings: &SyncSettings,
) -> Result<PathBuf, String> {
    let base = effective_obsidian_folder(app, settings)?;
    Ok(match sanitize_subfolder(settings.obsidian_subfolder.as_deref()) {
        Some(subfolder) => base.join(subfolder),
        None => base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn tags_lose_hashes_and_blank_entries() {
        assert_eq!(
            normalize_tags(&tags(&["#meeting", "  ", "##notes"])),
            vec!["meeting".to_string(), "notes".to_string()]
        );
    }

    #[test]
    fn tags_are_deduplicated_case_insensitively() {
        // Obsidian folds case, so keeping both spellings would silently lose one.
        assert_eq!(
            normalize_tags(&tags(&["Meeting", "meeting", "MEETING"])),
            vec!["Meeting".to_string()]
        );
    }

    #[test]
    fn tags_split_on_ascii_and_chinese_commas() {
        assert_eq!(
            normalize_tags(&tags(&["客户，融资, Q3"])),
            vec!["客户".to_string(), "融资".to_string(), "Q3".to_string()]
        );
    }

    #[test]
    fn tags_are_capped_in_count_and_length() {
        let many: Vec<String> = (0..(MAX_TAGS + 10)).map(|i| format!("tag{i}")).collect();
        assert_eq!(normalize_tags(&many).len(), MAX_TAGS);

        let long = normalize_tags(&tags(&["x".repeat(200).as_str()]));
        assert_eq!(long[0].chars().count(), MAX_TAG_CHARS);
    }

    #[test]
    fn sanitizing_clears_blank_fields_and_unwraps_subfolders() {
        let settings = sanitize_settings(SyncSettings {
            obsidian_folder: Some("   ".to_string()),
            obsidian_subfolder: Some("../Meetings/./2026/".to_string()),
            obsidian_template: Some("  \n ".to_string()),
            obsidian_tags: tags(&["#a", "a"]),
            notion_database_id: Some("  ".to_string()),
        });

        assert_eq!(settings.obsidian_folder, None);
        assert_eq!(settings.obsidian_subfolder.as_deref(), Some("Meetings/2026"));
        assert_eq!(settings.obsidian_template, None);
        assert_eq!(settings.obsidian_tags, vec!["a".to_string()]);
        assert_eq!(settings.notion_database_id, None);
    }

    #[test]
    fn sanitizing_keeps_a_relative_subfolder_and_rejects_absolute() {
        let settings = sanitize_settings(SyncSettings {
            obsidian_subfolder: Some("E:\\Obsidian\\Meetings".to_string()),
            ..SyncSettings::default()
        });

        // The drive letter component is just a name after sanitising, so the
        // result stays inside the vault instead of escaping to another drive.
        assert_eq!(
            settings.obsidian_subfolder.as_deref(),
            Some("E-/Obsidian/Meetings")
        );
    }
}
