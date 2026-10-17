use super::defaults;
use super::types::Template;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use once_cell::sync::Lazy;
use std::sync::RwLock;

// Global storage for the bundled templates directory path
static BUNDLED_TEMPLATES_DIR: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));

// Global storage for the application data directory.
//
// Custom templates must live beside the rest of the app's data, so we learn the
// directory from the Tauri app handle at startup rather than guessing it.
static APP_DATA_DIR: Lazy<RwLock<Option<PathBuf>>> = Lazy::new(|| RwLock::new(None));

/// Set the bundled templates directory path (called once at app startup)
pub fn set_bundled_templates_dir(path: PathBuf) {
    info!("Bundled templates directory set to: {:?}", path);
    if let Ok(mut dir) = BUNDLED_TEMPLATES_DIR.write() {
        *dir = Some(path);
    }
}

/// Set the application data directory (called once at app startup).
///
/// This is the same directory the database and models live in, so custom
/// templates are discoverable alongside the user's other Meetily data.
pub fn set_app_data_dir(path: PathBuf) {
    info!("Templates app data directory set to: {:?}", path);
    if let Ok(mut dir) = APP_DATA_DIR.write() {
        *dir = Some(path);
    }
}

/// Get the user's custom templates directory path.
///
/// Prefers the application data directory supplied at startup, which is where
/// the rest of the app's data lives. It falls back to the historical
/// `dirs::data_dir()/Meetily/templates` location only if startup did not supply
/// one, so that templates written by older builds are still found.
pub fn get_custom_templates_dir() -> Option<PathBuf> {
    if let Ok(dir) = APP_DATA_DIR.read() {
        if let Some(base) = dir.as_ref() {
            return Some(base.join("templates"));
        }
    }

    // Fallback: legacy location used before the app data dir was plumbed in.
    let mut path = dirs::data_dir()?;
    path.push("Meetily");
    path.push("templates");
    Some(path)
}

/// All directories that may contain custom templates, most preferred first.
fn custom_template_dirs() -> Vec<PathBuf> {
    let mut dirs_found: Vec<PathBuf> = Vec::new();

    if let Some(primary) = get_custom_templates_dir() {
        dirs_found.push(primary);
    }

    // Keep reading the legacy location so pre-existing custom templates are not
    // silently ignored after the directory change.
    if let Some(mut legacy) = dirs::data_dir() {
        legacy.push("Meetily");
        legacy.push("templates");
        if !dirs_found.contains(&legacy) {
            dirs_found.push(legacy);
        }
    }

    dirs_found
}

/// Where newly saved custom templates are written.
pub fn custom_templates_write_dir() -> Result<PathBuf, String> {
    get_custom_templates_dir().ok_or_else(|| {
        "Unable to resolve a directory for custom templates on this platform".to_string()
    })
}

/// Convert a display name into a safe template identifier.
pub fn slugify_template_id(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = true; // avoid a leading separator

    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            slug.push('_');
            last_was_separator = true;
        }
    }

    while slug.ends_with('_') {
        slug.pop();
    }

    slug
}

/// Load a template from the bundled resources directory
///
/// # Arguments
/// * `template_id` - Template identifier (without .json extension)
///
/// # Returns
/// The template JSON content if found, None otherwise
fn load_bundled_template(template_id: &str) -> Option<String> {
    let bundled_dir = BUNDLED_TEMPLATES_DIR.read().ok()?.clone()?;
    let template_path = bundled_dir.join(format!("{}.json", template_id));

    debug!("Checking for bundled template at: {:?}", template_path);

    match std::fs::read_to_string(&template_path) {
        Ok(content) => {
            info!("Loaded bundled template '{}' from {:?}", template_id, template_path);
            Some(content)
        }
        Err(e) => {
            debug!("No bundled template '{}' found: {}", template_id, e);
            None
        }
    }
}

/// Load a template from the user's custom templates directory
///
/// # Arguments
/// * `template_id` - Template identifier (without .json extension)
///
/// # Returns
/// The template JSON content if found, None otherwise
fn load_custom_template(template_id: &str) -> Option<String> {
    for custom_dir in custom_template_dirs() {
        let template_path = custom_dir.join(format!("{}.json", template_id));
        if let Ok(content) = std::fs::read_to_string(&template_path) {
            info!("Loaded custom template '{}' from {:?}", template_id, template_path);
            return Some(content);
        }
    }

    debug!("No custom template '{}' found in any custom directory", template_id);
    None
}

/// Whether a template id resolves to a user-authored template.
pub fn is_custom_template(template_id: &str) -> bool {
    custom_template_dirs()
        .iter()
        .any(|dir| dir.join(format!("{}.json", template_id)).is_file())
}

/// Read a custom template straight from disk (no bundled/builtin fallback).
pub fn read_custom_template(template_id: &str) -> Result<Template, String> {
    let content = load_custom_template(template_id)
        .ok_or_else(|| format!("Custom template '{}' not found", template_id))?;
    validate_and_parse_template(&content)
}

/// Validate a template identifier before it is used as a file name.
///
/// Kept free of any filesystem or global state so it can be unit tested; the
/// point is that a caller-supplied id must never be able to escape the templates
/// directory or shadow a JSON file elsewhere.
pub fn validate_template_id(template_id: &str) -> Result<(), String> {
    let trimmed = template_id.trim();

    if trimmed.is_empty() {
        return Err("Template id cannot be empty".to_string());
    }
    if trimmed != template_id {
        return Err(format!(
            "Template id '{}' must not have leading or trailing whitespace",
            template_id
        ));
    }
    if template_id.contains(['/', '\\', '.']) {
        return Err(format!(
            "Template id '{}' must not contain path separators or dots",
            template_id
        ));
    }

    Ok(())
}

/// Persist a custom template, creating or overwriting it.
///
/// A template is never written over a built-in one implicitly: built-ins are
/// embedded in the binary and cannot be edited in place, so saving under a
/// built-in id is rejected and the caller is expected to pick a new name.
/// Pass `overwrite = true` to replace an existing *custom* template.
pub fn save_custom_template(
    template_id: &str,
    template: &Template,
    overwrite: bool,
) -> Result<PathBuf, String> {
    validate_template_id(template_id)?;

    template.validate()?;

    let dir = custom_templates_write_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create templates directory {}: {}", dir.display(), e))?;

    let path = dir.join(format!("{}.json", template_id));

    let already_exists = custom_template_dirs()
        .iter()
        .any(|d| d.join(format!("{}.json", template_id)).is_file());

    if already_exists && !overwrite {
        return Err(format!(
            "A custom template with id '{}' already exists. Choose another name or overwrite it.",
            template_id
        ));
    }

    if !already_exists && defaults::get_builtin_template(template_id).is_some() {
        return Err(format!(
            "'{}' is a built-in template and cannot be overwritten. Choose a different name.",
            template_id
        ));
    }

    let json = serde_json::to_string_pretty(template)
        .map_err(|e| format!("Failed to serialize template: {}", e))?;

    std::fs::write(&path, json)
        .map_err(|e| format!("Failed to write template to {}: {}", path.display(), e))?;

    info!("Saved custom template '{}' to {:?}", template_id, path);
    Ok(path)
}

/// Delete a custom template. Built-in templates cannot be deleted.
pub fn delete_custom_template(template_id: &str) -> Result<(), String> {
    let mut removed = false;

    for dir in custom_template_dirs() {
        let path = dir.join(format!("{}.json", template_id));
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete {}: {}", path.display(), e))?;
            info!("Deleted custom template '{}' at {:?}", template_id, path);
            removed = true;
        }
    }

    if !removed {
        if defaults::get_builtin_template(template_id).is_some() {
            return Err(format!(
                "'{}' is a built-in template and cannot be deleted.",
                template_id
            ));
        }
        return Err(format!("Custom template '{}' not found", template_id));
    }

    Ok(())
}

/// Pick an unused identifier derived from a display name.
pub fn unique_template_id(preferred_name: &str) -> String {
    let base = slugify_template_id(preferred_name);
    let base = if base.is_empty() { "custom_template".to_string() } else { base };

    let taken = |id: &str| -> bool {
        is_custom_template(id) || defaults::get_builtin_template(id).is_some()
    };

    if !taken(&base) {
        return base;
    }

    for n in 2..1000 {
        let candidate = format!("{}_{}", base, n);
        if !taken(&candidate) {
            return candidate;
        }
    }

    format!("{}_{}", base, std::process::id())
}

/// Whether a directory is one of the custom template directories.
pub fn is_custom_templates_dir(dir: &Path) -> bool {
    custom_template_dirs().iter().any(|d| d == dir)
}

/// Load and parse a template by identifier
///
/// This function implements a fallback strategy:
/// 1. Check user's custom templates directory
/// 2. Check bundled resources directory (app templates)
/// 3. Fall back to built-in embedded templates
/// 4. Return error if not found in any location
///
/// # Arguments
/// * `template_id` - Template identifier (e.g., "daily_standup", "standard_meeting")
///
/// # Returns
/// Parsed and validated Template struct
pub fn get_template(template_id: &str) -> Result<Template, String> {
    // The bundled file was once misspelled; keep saved selections working.
    let template_id = if template_id == "psychatric_session" { "psychiatric_session" } else { template_id };
    info!("Loading template: {}", template_id);

    // Try custom template first, then bundled, then built-in
    let json_content = if let Some(custom_content) = load_custom_template(template_id) {
        debug!("Using custom template for '{}'", template_id);
        custom_content
    } else if let Some(bundled_content) = load_bundled_template(template_id) {
        debug!("Using bundled template for '{}'", template_id);
        bundled_content
    } else if let Some(builtin_content) = defaults::get_builtin_template(template_id) {
        debug!("Using built-in template for '{}'", template_id);
        builtin_content.to_string()
    } else {
        return Err(format!(
            "Template '{}' not found. Available templates: {}",
            template_id,
            list_template_ids().join(", ")
        ));
    };

    // Parse and validate
    validate_and_parse_template(&json_content)
}

/// Validate and parse template JSON
///
/// # Arguments
/// * `json_content` - Raw JSON string
///
/// # Returns
/// Parsed and validated Template struct
pub fn validate_and_parse_template(json_content: &str) -> Result<Template, String> {
    let template: Template = serde_json::from_str(json_content)
        .map_err(|e| format!("Failed to parse template JSON: {}", e))?;

    template.validate()?;

    Ok(template)
}

/// List all available template identifiers
///
/// Returns a combined list of:
/// - Built-in template IDs
/// - Bundled template IDs (from app resources)
/// - Custom template IDs (from user's data directory)
pub fn list_template_ids() -> Vec<String> {
    let mut ids: Vec<String> = defaults::list_builtin_template_ids()
        .into_iter()
        .map(|s| s.to_string())
        .collect();

    // Add bundled templates if directory is set
    if let Ok(bundled_dir_lock) = BUNDLED_TEMPLATES_DIR.read() {
        if let Some(bundled_dir) = bundled_dir_lock.as_ref() {
            if bundled_dir.exists() {
                match std::fs::read_dir(bundled_dir) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            if let Some(filename) = entry.file_name().to_str() {
                                if filename.ends_with(".json") {
                                    let id = filename.trim_end_matches(".json").to_string();
                                    if !ids.contains(&id) {
                                        ids.push(id);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read bundled templates directory: {}", e);
                    }
                }
            }
        }
    }

    // Add custom templates from every known custom directory.
    for custom_dir in custom_template_dirs() {
        if !custom_dir.exists() {
            continue;
        }
        match std::fs::read_dir(&custom_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if let Some(filename) = entry.file_name().to_str() {
                        if filename.ends_with(".json") {
                            let id = filename.trim_end_matches(".json").to_string();
                            if !ids.contains(&id) {
                                ids.push(id);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read custom templates directory {:?}: {}", custom_dir, e);
            }
        }
    }

    ids.sort();
    ids
}

/// List all available templates with their metadata.
///
/// Returns `(id, name, description, is_custom)` tuples. `is_custom` lets the UI
/// distinguish user-authored templates (editable / deletable) from built-in ones.
pub fn list_templates_detailed() -> Vec<(String, String, String, bool)> {
    let mut templates = Vec::new();

    for id in list_template_ids() {
        match get_template(&id) {
            Ok(template) => {
                let is_custom = is_custom_template(&id);
                templates.push((id, template.name, template.description, is_custom));
            }
            Err(e) => {
                warn!("Failed to load template '{}': {}", id, e);
            }
        }
    }

    templates
}

/// List all available templates with their metadata
///
/// Returns a list of (id, name, description) tuples
pub fn list_templates() -> Vec<(String, String, String)> {
    let mut templates = Vec::new();

    for id in list_template_ids() {
        match get_template(&id) {
            Ok(template) => {
                templates.push((id, template.name, template.description));
            }
            Err(e) => {
                warn!("Failed to load template '{}': {}", id, e);
            }
        }
    }

    templates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_builtin_template() {
        let template = get_template("daily_standup");
        assert!(template.is_ok());

        let template = template.unwrap();
        assert_eq!(template.name, "Daily Standup");
        assert!(!template.sections.is_empty());
    }

    #[test]
    fn test_get_nonexistent_template() {
        let result = get_template("nonexistent_template");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_template_ids() {
        let ids = list_template_ids();
        assert!(ids.contains(&"daily_standup".to_string()));
        assert!(ids.contains(&"standard_meeting".to_string()));
    }

    #[test]
    fn test_validate_invalid_json() {
        let result = validate_and_parse_template("invalid json");
        assert!(result.is_err());
    }

    #[test]
    fn slugify_produces_filesystem_safe_ids() {
        assert_eq!(slugify_template_id("Weekly Engineering Sync"), "weekly_engineering_sync");
        assert_eq!(slugify_template_id("Q1/Q2 Review"), "q1_q2_review");
        // Separators collapse and never lead/trail.
        assert_eq!(slugify_template_id("  spaced  out  "), "spaced_out");
        assert_eq!(slugify_template_id("1:1 with Bob"), "1_1_with_bob");
        assert_eq!(slugify_template_id("ALL CAPS"), "all_caps");
    }

    #[test]
    fn slugify_of_unusable_name_is_empty() {
        assert_eq!(slugify_template_id(""), "");
        assert_eq!(slugify_template_id("---"), "");
        assert_eq!(slugify_template_id("日本語"), "");
    }

    #[test]
    fn validate_template_id_accepts_plain_ids() {
        assert!(validate_template_id("weekly_sync").is_ok());
        assert!(validate_template_id("template-2").is_ok());
        assert!(validate_template_id("abc123").is_ok());
    }

    #[test]
    fn validate_template_id_rejects_path_traversal() {
        // A dot or separator could otherwise escape the templates directory.
        assert!(validate_template_id("").is_err());
        assert!(validate_template_id("   ").is_err());
        assert!(validate_template_id("../secrets").is_err());
        assert!(validate_template_id("a/b").is_err());
        assert!(validate_template_id("a\\b").is_err());
        assert!(validate_template_id("evil.json").is_err());
        assert!(validate_template_id(" padded ").is_err());
    }

    #[test]
    fn custom_templates_write_dir_is_resolvable() {
        // Must never panic; a platform without a data dir returns an error.
        let _ = custom_templates_write_dir();
    }
}
