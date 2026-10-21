use serde::{Deserialize, Serialize};

/// Represents a single section in a meeting template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateSection {
    /// Section title (e.g., "Summary", "Action Items")
    pub title: String,

    /// Instruction for the LLM on what to extract/include
    pub instruction: String,

    /// Format type: "paragraph", "list", or "string"
    pub format: String,

    /// Optional markdown formatting hint for list items (e.g., table structure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_format: Option<String>,

    /// Alternative formatting hint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example_item_format: Option<String>,
}

/// Represents a complete meeting template
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemplateLengthBand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to_minutes: Option<u32>,
    pub target_words: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// Template display name
    pub name: String,

    /// Brief description of the template's purpose
    pub description: String,

    /// How much prose each section should contain: "brief", "standard" or
    /// "detailed". Older templates omit it and get the standard behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_level: Option<String>,

    /// How finely discussion should be split into points: "coarse", "balanced"
    /// or "fine". Older templates omit it and get the balanced behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_bands: Option<Vec<TemplateLengthBand>>,

    /// List of sections in the template
    pub sections: Vec<TemplateSection>,
}

impl Template {
    /// Validates the template structure
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("Template name cannot be empty".to_string());
        }

        if self.description.is_empty() {
            return Err("Template description cannot be empty".to_string());
        }

        if self.sections.is_empty() {
            return Err("Template must have at least one section".to_string());
        }

        if let Some(level) = self.detail_level.as_deref() {
            match level {
                "brief" | "standard" | "detailed" => {},
                other => return Err(format!(
                    "Invalid detail_level '{}'. Must be 'brief', 'standard' or 'detailed'",
                    other
                )),
            }
        }

        if let Some(granularity) = self.granularity.as_deref() {
            match granularity {
                "coarse" | "balanced" | "fine" => {},
                other => return Err(format!(
                    "Invalid granularity '{}'. Must be 'coarse', 'balanced' or 'fine'",
                    other
                )),
            }
        }

        if let Some(bands) = self.length_bands.as_ref() {
            if bands.is_empty() {
                return Err("Length bands cannot be empty when length_bands is set".to_string());
            }
            let mut previous_bound: Option<u32> = None;
            for (i, band) in bands.iter().enumerate() {
                if band.target_words == 0 {
                    return Err(format!("Length band {} must target more than zero words", i + 1));
                }
                if let Some(bound) = band.up_to_minutes {
                    if bound == 0 {
                        return Err(format!("Length band {} cannot end at zero minutes", i + 1));
                    }
                    if previous_bound.is_some_and(|previous| bound < previous) {
                        return Err("Length band minute limits must be in ascending order".to_string());
                    }
                    previous_bound = Some(bound);
                }
            }
        }

        for (i, section) in self.sections.iter().enumerate() {
            if section.title.is_empty() {
                return Err(format!("Section {} has empty title", i));
            }

            if section.instruction.is_empty() {
                return Err(format!("Section '{}' has empty instruction", section.title));
            }

            match section.format.as_str() {
                "paragraph" | "list" | "string" => {},
                other => return Err(format!(
                    "Section '{}' has invalid format '{}'. Must be 'paragraph', 'list', or 'string'",
                    section.title, other
                )),
            }
        }

        Ok(())
    }

    /// Generates a clean markdown template structure
    pub fn to_markdown_structure(&self) -> String {
        let mut markdown = String::from("# <Add Title here>\n\n");

        for section in &self.sections {
            markdown.push_str(&format!("**{}**\n\n", section.title));
        }

        markdown
    }

    /// Generates section-specific instructions for the LLM
    pub fn to_section_instructions(&self) -> String {
        let mut instructions = String::from(
            "- **For the main title (`# [AI-Generated Title]`):** Analyze the entire transcript and create a concise, descriptive title for the meeting.\n"
        );

        // Template-wide breadth controls. Written as instructions rather than
        // separate parameters so they apply to every section and every model,
        // including custom templates and cloud providers.
        match self.detail_level.as_deref().unwrap_or("standard") {
            "brief" => instructions.push_str(
                "- **Overall detail level:** brief. Prefer short, information-dense sentences. Omit background, repetition and examples; keep each section to the essential facts.\n"
            ),
            "detailed" => instructions.push_str(
                "- **Overall detail level:** detailed. Explain each point with the context, reasoning and any caveats the transcript supports. Do not invent facts; when the transcript is ambiguous, say so instead of expanding.\n"
            ),
            _ => instructions.push_str(
                "- **Overall detail level:** standard. Cover each section with enough context to be useful, without repeating the transcript.\n"
            ),
        }

        match self.granularity.as_deref().unwrap_or("balanced") {
            "coarse" => instructions.push_str(
                "- **Overall granularity:** coarse. Group related points into a few themes instead of listing every remark; only keep details that change a decision or next step.\n"
            ),
            "fine" => instructions.push_str(
                "- **Overall granularity:** fine. Break the discussion into individual points, decisions, open questions and owners. Keep distinct topics separate rather than merging them.\n"
            ),
            _ => instructions.push_str(
                "- **Overall granularity:** balanced. Keep distinct decisions and next steps separate, and group small remarks into one point.\n"
            ),
        }

        for section in &self.sections {
            instructions.push_str(&format!(
                "- **For the '{}' section:** {}.\n",
                section.title, section.instruction
            ));

            // Add item format instructions if present
            let item_format = section.item_format.as_ref()
                .or(section.example_item_format.as_ref());

            if let Some(format) = item_format {
                instructions.push_str(&format!(
                    "  - Items in this section should follow the format: `{}`.\n",
                    format
                ));
            }
        }

        instructions
    }

    fn select_length_band(&self, minutes: Option<f64>) -> Option<&TemplateLengthBand> {
        let bands = self.length_bands.as_ref()?;
        if bands.is_empty() {
            return None;
        }
        match minutes {
            Some(value) => bands
                .iter()
                .find(|band| band.up_to_minutes.is_none() || value <= band.up_to_minutes.unwrap() as f64)
                .or_else(|| bands.last()),
            None => bands
                .iter()
                .find(|band| band.up_to_minutes.is_none())
                .or_else(|| bands.last()),
        }
    }

    pub fn length_instruction_for_minutes(&self, minutes: Option<f64>) -> Option<String> {
        let band = self.select_length_band(minutes)?;
        let duration = minutes
            .map(|value| format!(" for a {:.0}-minute meeting", value.ceil()))
            .unwrap_or_default();
        Some(format!(
            "- **Overall length target:** aim for about {} words{}. Keep each section proportional to its importance; do not pad or truncate mechanically.
",
            band.target_words, duration
        ))
    }

    pub fn to_section_instructions_with_length(&self, minutes: Option<f64>) -> String {
        let mut instructions = self.to_section_instructions();
        if let Some(length_instruction) = self.length_instruction_for_minutes(minutes) {
            instructions.push_str(&length_instruction);
        }
        instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_template() {
        let template = Template {
            name: "Test Template".to_string(),
            description: "A test template".to_string(),
            detail_level: None,
            granularity: None,
            length_bands: None,
            sections: vec![
                TemplateSection {
                    title: "Summary".to_string(),
                    instruction: "Provide a summary".to_string(),
                    format: "paragraph".to_string(),
                    item_format: None,
                    example_item_format: None,
                },
            ],
        };

        assert!(template.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_name() {
        let template = Template {
            name: "".to_string(),
            description: "A test template".to_string(),
            detail_level: None,
            granularity: None,
            length_bands: None,
            sections: vec![],
        };

        assert!(template.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_format() {
        let template = Template {
            name: "Test".to_string(),
            description: "Test".to_string(),
            detail_level: None,
            granularity: None,
            length_bands: None,
            sections: vec![
                TemplateSection {
                    title: "Test".to_string(),
                    instruction: "Test".to_string(),
                    format: "invalid".to_string(),
                    item_format: None,
                    example_item_format: None,
                },
            ],
        };

        assert!(template.validate().is_err());
    }

    fn valid_template(detail: Option<&str>, granularity: Option<&str>) -> Template {
        Template {
            name: "Test".to_string(),
            description: "Test".to_string(),
            detail_level: detail.map(str::to_string),
            granularity: granularity.map(str::to_string),
            length_bands: None,
            sections: vec![TemplateSection {
                title: "Summary".to_string(),
                instruction: "Summarise".to_string(),
                format: "paragraph".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        }
    }

    #[test]
    fn detail_level_and_granularity_are_validated_and_prompted() {
        let template = valid_template(Some("detailed"), Some("fine"));
        assert!(template.validate().is_ok());
        let instructions = template.to_section_instructions();
        assert!(instructions.contains("Overall detail level:** detailed"));
        assert!(instructions.contains("Overall granularity:** fine"));

        assert!(valid_template(Some("verbose"), None).validate().is_err());
        assert!(valid_template(None, Some("tiny")).validate().is_err());
    }

    #[test]
    fn length_bands_are_validated_and_selected_by_duration() {
        let mut template = valid_template(None, None);
        template.length_bands = Some(vec![
            TemplateLengthBand { up_to_minutes: Some(15), target_words: 250 },
            TemplateLengthBand { up_to_minutes: Some(45), target_words: 600 },
            TemplateLengthBand { up_to_minutes: None, target_words: 1000 },
        ]);
        assert!(template.validate().is_ok());
        assert!(template
            .length_instruction_for_minutes(Some(10.0))
            .expect("short band")
            .contains("about 250 words"));
        assert!(template
            .length_instruction_for_minutes(Some(30.0))
            .expect("medium band")
            .contains("about 600 words"));
        assert!(template
            .length_instruction_for_minutes(Some(120.0))
            .expect("long band")
            .contains("about 1000 words"));
        assert!(template
            .to_section_instructions_with_length(Some(30.0))
            .contains("Overall length target"));

        template.length_bands = Some(vec![]);
        assert!(template.validate().is_err());
        template.length_bands = Some(vec![TemplateLengthBand {
            up_to_minutes: Some(10),
            target_words: 0,
        }]);
        assert!(template.validate().is_err());
        template.length_bands = Some(vec![
            TemplateLengthBand { up_to_minutes: Some(45), target_words: 600 },
            TemplateLengthBand { up_to_minutes: Some(15), target_words: 250 },
        ]);
        assert!(template.validate().is_err());
    }
}
