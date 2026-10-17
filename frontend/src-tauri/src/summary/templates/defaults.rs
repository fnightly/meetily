/// Embedded default templates using compile-time inclusion
///
/// These templates are bundled into the binary and serve as fallbacks
/// when custom templates are not available.

/// Daily standup template for engineering/product teams
pub const DAILY_STANDUP: &str = include_str!("../../../templates/daily_standup.json");

/// Standard meeting notes template
pub const STANDARD_MEETING: &str = include_str!("../../../templates/standard_meeting.json");

/// Chinese template: zh_weekly_meeting
pub const ZH_WEEKLY_MEETING: &str = include_str!("../../../templates/zh_weekly_meeting.json");

/// Chinese template: zh_customer_interview
pub const ZH_CUSTOMER_INTERVIEW: &str = include_str!("../../../templates/zh_customer_interview.json");

/// Chinese template: zh_job_interview
pub const ZH_JOB_INTERVIEW: &str = include_str!("../../../templates/zh_job_interview.json");

/// Chinese template: zh_investment_committee
pub const ZH_INVESTMENT_COMMITTEE: &str = include_str!("../../../templates/zh_investment_committee.json");

/// Chinese template: zh_project_pitch
pub const ZH_PROJECT_PITCH: &str = include_str!("../../../templates/zh_project_pitch.json");

/// Registry of all built-in templates
///
/// Maps template identifiers to their embedded JSON content
pub fn get_builtin_templates() -> Vec<(&'static str, &'static str)> {
    vec![
        ("daily_standup", DAILY_STANDUP),
        ("standard_meeting", STANDARD_MEETING),
        ("zh_weekly_meeting", ZH_WEEKLY_MEETING),
        ("zh_customer_interview", ZH_CUSTOMER_INTERVIEW),
        ("zh_job_interview", ZH_JOB_INTERVIEW),
        ("zh_investment_committee", ZH_INVESTMENT_COMMITTEE),
        ("zh_project_pitch", ZH_PROJECT_PITCH),
    ]
}

/// Get a built-in template by identifier
///
/// # Arguments
/// * `id` - Template identifier (e.g., "daily_standup", "standard_meeting")
///
/// # Returns
/// The template JSON content if found, None otherwise
pub fn get_builtin_template(id: &str) -> Option<&'static str> {
    match id {
        "daily_standup" => Some(DAILY_STANDUP),
        "standard_meeting" => Some(STANDARD_MEETING),
        "zh_weekly_meeting" => Some(ZH_WEEKLY_MEETING),
        "zh_customer_interview" => Some(ZH_CUSTOMER_INTERVIEW),
        "zh_job_interview" => Some(ZH_JOB_INTERVIEW),
        "zh_investment_committee" => Some(ZH_INVESTMENT_COMMITTEE),
        "zh_project_pitch" => Some(ZH_PROJECT_PITCH),
        _ => None,
    }
}

/// List all built-in template identifiers
pub fn list_builtin_template_ids() -> Vec<&'static str> {
    vec![
        "daily_standup",
        "standard_meeting",
        "zh_weekly_meeting",
        "zh_customer_interview",
        "zh_job_interview",
        "zh_investment_committee",
        "zh_project_pitch",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_templates_valid_json() {
        for (id, content) in get_builtin_templates() {
            let result = serde_json::from_str::<serde_json::Value>(content);
            assert!(
                result.is_ok(),
                "Built-in template '{}' contains invalid JSON: {:?}",
                id,
                result.err()
            );
        }
    }

    #[test]
    fn test_get_builtin_template() {
        assert!(get_builtin_template("daily_standup").is_some());
        assert!(get_builtin_template("standard_meeting").is_some());
        assert!(get_builtin_template("nonexistent").is_none());
        for id in list_builtin_template_ids() {
            let template = super::super::loader::validate_and_parse_template(
                get_builtin_template(id).unwrap(),
            );
            assert!(template.is_ok(), "template {id} invalid: {:?}", template.err());
        }
    }
}
