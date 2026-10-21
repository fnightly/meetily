use crate::summary::templates::{self, Template, TemplateLengthBand, TemplateSection};
use serde::{Deserialize, Serialize};
use tauri::Runtime;
use tracing::{info, warn};

/// Template metadata for UI display
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateInfo {
    /// Template identifier (e.g., "daily_standup", "standard_meeting")
    pub id: String,

    /// Display name for the template
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// Whether this is a user-authored template (editable and deletable) as
    /// opposed to one shipped with the app.
    pub is_custom: bool,
}

/// Detailed template structure for preview/debugging
#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateDetails {
    /// Template identifier
    pub id: String,

    /// Display name
    pub name: String,

    /// Description
    pub description: String,

    /// List of section titles in order
    pub sections: Vec<String>,

    /// Whether this is a user-authored template.
    pub is_custom: bool,
}

/// A full template payload used when creating or editing a custom template.
#[derive(Debug, Deserialize)]
pub struct TemplateSectionInput {
    pub title: String,
    pub instruction: String,
    pub format: String,
    #[serde(default)]
    pub item_format: Option<String>,
}

/// Result of saving a custom template.
#[derive(Debug, Serialize)]
pub struct SaveTemplateResult {
    /// Identifier the template was stored under (may be generated).
    pub id: String,
    /// Absolute path of the written file.
    pub path: String,
    /// True when an existing custom template was replaced.
    pub overwritten: bool,
}

/// Lists all available templates
///
/// Returns templates from both built-in (embedded) and custom (user data directory) sources.
/// Templates are automatically discovered - no code changes needed to add new templates.
///
/// # Returns
/// Vector of TemplateInfo with id, name, description and origin for each template
#[tauri::command]
pub async fn api_list_templates<R: Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<Vec<TemplateInfo>, String> {
    info!("api_list_templates called");

    let template_infos: Vec<TemplateInfo> = templates::list_templates_detailed()
        .into_iter()
        .map(|(id, name, description, is_custom)| TemplateInfo {
            id,
            name,
            description,
            is_custom,
        })
        .collect();

    info!("Found {} available templates", template_infos.len());

    Ok(template_infos)
}

/// Gets detailed information about a specific template
///
/// # Arguments
/// * `template_id` - Template identifier (e.g., "daily_standup")
///
/// # Returns
/// TemplateDetails with full template structure
#[tauri::command]
pub async fn api_get_template_details<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<TemplateDetails, String> {
    info!("api_get_template_details called for template_id: {}", template_id);

    let template = templates::get_template(&template_id)?;

    let section_titles: Vec<String> = template
        .sections
        .iter()
        .map(|section| section.title.clone())
        .collect();

    let details = TemplateDetails {
        id: template_id.clone(),
        name: template.name,
        description: template.description,
        sections: section_titles,
        is_custom: templates::is_custom_template(&template_id),
    };

    info!("Retrieved template details for '{}'", details.name);

    Ok(details)
}

/// Returns the full definition of a template, including every section's
/// instruction, so the editor can load an existing template.
#[tauri::command]
pub async fn api_get_template_full<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<Template, String> {
    info!("api_get_template_full called for template_id: {}", template_id);
    templates::get_template(&template_id)
}

/// Directory where custom templates are stored (for display in the UI).
#[tauri::command]
pub async fn api_get_custom_templates_dir<R: Runtime>(
    _app: tauri::AppHandle<R>,
) -> Result<String, String> {
    templates::custom_templates_write_dir().map(|p| p.to_string_lossy().to_string())
}

/// Create or update a custom template.
///
/// When `template_id` is `None` a unique id is derived from the template name.
/// Saving over a built-in template is refused; saving over an existing custom
/// template requires `overwrite = true`.
#[tauri::command]
pub async fn api_save_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: Option<String>,
    name: String,
    description: String,
    detail_level: Option<String>,
    granularity: Option<String>,
    length_bands: Option<Vec<TemplateLengthBand>>,
    sections: Vec<TemplateSectionInput>,
    overwrite: Option<bool>,
) -> Result<SaveTemplateResult, String> {
    info!(
        "api_save_template called (id: {:?}, name: {}, sections: {})",
        template_id,
        name,
        sections.len()
    );

    let template = Template {
        name: name.clone(),
        description,
        detail_level,
        granularity,
        length_bands,
        sections: sections
            .into_iter()
            .map(|s| TemplateSection {
                title: s.title,
                instruction: s.instruction,
                format: s.format,
                item_format: s.item_format,
                example_item_format: None,
            })
            .collect(),
    };

    // Validate before touching the filesystem so a bad payload cannot leave a
    // half-written template behind.
    template.validate()?;

    let overwrite = overwrite.unwrap_or(false);

    let (id, overwritten) = match template_id.filter(|id| !id.trim().is_empty()) {
        Some(id) => {
            let exists = templates::is_custom_template(&id);
            (id, exists)
        }
        None => (templates::unique_template_id(&name), false),
    };

    let path = templates::save_custom_template(&id, &template, overwrite)?;

    info!("Template '{}' saved to {:?}", id, path);

    Ok(SaveTemplateResult {
        id,
        path: path.to_string_lossy().to_string(),
        overwritten,
    })
}

/// Delete a custom template. Built-in templates are refused.
#[tauri::command]
pub async fn api_delete_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
) -> Result<(), String> {
    info!("api_delete_template called for template_id: {}", template_id);
    templates::delete_custom_template(&template_id)
}

/// Duplicate any template (built-in included) into a new custom template.
#[tauri::command]
pub async fn api_duplicate_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_id: String,
    new_name: Option<String>,
) -> Result<SaveTemplateResult, String> {
    info!("api_duplicate_template called for template_id: {}", template_id);

    let source = templates::get_template(&template_id)?;

    let name = new_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("{} (copy)", source.name));

    let duplicated = Template {
        name: name.clone(),
        description: source.description.clone(),
        detail_level: source.detail_level.clone(),
        granularity: source.granularity.clone(),
        length_bands: source.length_bands.clone(),
        sections: source.sections.clone(),
    };

    let new_id = templates::unique_template_id(&name);
    let path = templates::save_custom_template(&new_id, &duplicated, false)?;

    Ok(SaveTemplateResult {
        id: new_id,
        path: path.to_string_lossy().to_string(),
        overwritten: false,
    })
}

/// Validates a custom template JSON string
///
/// Useful for template editor UI or validation before saving custom templates
///
/// # Arguments
/// * `template_json` - Raw JSON string of the template
///
/// # Returns
/// Ok(template_name) if valid, Err(error_message) if invalid
#[tauri::command]
pub async fn api_validate_template<R: Runtime>(
    _app: tauri::AppHandle<R>,
    template_json: String,
) -> Result<String, String> {
    info!("api_validate_template called");

    match templates::validate_and_parse_template(&template_json) {
        Ok(template) => {
            info!("Template '{}' validated successfully", template.name);
            Ok(template.name)
        }
        Err(e) => {
            warn!("Template validation failed: {}", e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_templates() {
        // This test requires the templates to be embedded/available
        // In a real test environment, you might want to mock the templates module

        // For now, just verify the function compiles and runs
        // You can expand this with more specific assertions
    }

    #[tokio::test]
    async fn test_validate_template_valid() {
        let valid_json = r#"
        {
            "name": "Test Template",
            "description": "A test template",
            "sections": [
                {
                    "title": "Summary",
                    "instruction": "Provide a summary",
                    "format": "paragraph"
                }
            ]
        }"#;

        // Mock app handle would be needed for actual testing
        // For now, test the validation logic directly
        let result = templates::validate_and_parse_template(valid_json);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_template_invalid() {
        let invalid_json = "invalid json";

        let result = templates::validate_and_parse_template(invalid_json);
        assert!(result.is_err());
    }
}
