//! Tags and projects for organising meetings.

use std::collections::BTreeMap;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::state::AppState;

const MAX_LABEL_CHARS: usize = 32;
const MAX_TAGS_PER_MEETING: usize = 12;

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct MeetingLabels {
    pub meeting_id: String,
    pub tags: Vec<String>,
    pub project: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceLabels {
    pub tags: Vec<LabelCount>,
    pub projects: Vec<LabelCount>,
}

/// Trim, collapse spaces, drop a leading '#', bound the length.
pub fn clean_label(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let without_hash = collapsed.trim_start_matches(['#', '＃']).trim();
    if without_hash.is_empty() {
        return None;
    }
    Some(without_hash.chars().take(MAX_LABEL_CHARS).collect())
}

/// Clean and de-duplicate (case-insensitively) a tag list, keeping order.
pub fn clean_tags(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in raw.iter().filter_map(|t| clean_label(t)) {
        if !out.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
            out.push(tag);
        }
        if out.len() >= MAX_TAGS_PER_MEETING {
            break;
        }
    }
    out
}

pub async fn labels_for(pool: &SqlitePool, meeting_id: &str) -> Result<MeetingLabels, sqlx::Error> {
    let tags = sqlx::query_as::<_, (String,)>(
        "SELECT tag FROM meeting_tags WHERE meeting_id = ? ORDER BY created_at, tag",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(t,)| t)
    .collect();
    let project = sqlx::query_as::<_, (String,)>("SELECT project FROM meeting_projects WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?
        .map(|(p,)| p);
    Ok(MeetingLabels { meeting_id: meeting_id.to_string(), tags, project })
}

pub async fn set_tags(pool: &SqlitePool, meeting_id: &str, tags: &[String]) -> Result<MeetingLabels, sqlx::Error> {
    let tags = clean_tags(tags);
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM meeting_tags WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *tx)
        .await?;
    let now = chrono::Utc::now();
    for (i, tag) in tags.iter().enumerate() {
        // Offset timestamps keep the user's order stable on read.
        let stamp = (now + chrono::Duration::milliseconds(i as i64)).to_rfc3339();
        sqlx::query("INSERT INTO meeting_tags (meeting_id, tag, created_at) VALUES (?, ?, ?)")
            .bind(meeting_id)
            .bind(tag)
            .bind(stamp)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    labels_for(pool, meeting_id).await
}

pub async fn set_project(pool: &SqlitePool, meeting_id: &str, project: Option<&str>) -> Result<MeetingLabels, sqlx::Error> {
    match project.and_then(clean_label) {
        Some(project) => {
            sqlx::query(
                "INSERT INTO meeting_projects (meeting_id, project, updated_at) VALUES (?, ?, ?)
                 ON CONFLICT(meeting_id) DO UPDATE SET project = excluded.project, updated_at = excluded.updated_at",
            )
            .bind(meeting_id)
            .bind(project)
            .bind(super::now_rfc3339())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM meeting_projects WHERE meeting_id = ?")
                .bind(meeting_id)
                .execute(pool)
                .await?;
        }
    }
    labels_for(pool, meeting_id).await
}

pub async fn all_labels(pool: &SqlitePool) -> Result<Vec<MeetingLabels>, sqlx::Error> {
    let mut by_meeting: BTreeMap<String, MeetingLabels> = BTreeMap::new();
    for (meeting_id, tag) in sqlx::query_as::<_, (String, String)>(
        "SELECT meeting_id, tag FROM meeting_tags ORDER BY created_at, tag",
    )
    .fetch_all(pool)
    .await?
    {
        by_meeting
            .entry(meeting_id.clone())
            .or_insert_with(|| MeetingLabels { meeting_id, ..Default::default() })
            .tags
            .push(tag);
    }
    for (meeting_id, project) in
        sqlx::query_as::<_, (String, String)>("SELECT meeting_id, project FROM meeting_projects")
            .fetch_all(pool)
            .await?
    {
        by_meeting
            .entry(meeting_id.clone())
            .or_insert_with(|| MeetingLabels { meeting_id, ..Default::default() })
            .project = Some(project);
    }
    Ok(by_meeting.into_values().collect())
}

#[tauri::command]
pub async fn api_get_meeting_labels(
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingLabels, String> {
    labels_for(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to load labels: {e}"))
}

#[tauri::command]
pub async fn api_set_meeting_tags(
    meeting_id: String,
    tags: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingLabels, String> {
    set_tags(state.db_manager.pool(), &meeting_id, &tags)
        .await
        .map_err(|e| format!("Failed to save tags: {e}"))
}

#[tauri::command]
pub async fn api_set_meeting_project(
    meeting_id: String,
    project: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<MeetingLabels, String> {
    set_project(state.db_manager.pool(), &meeting_id, project.as_deref())
        .await
        .map_err(|e| format!("Failed to save project: {e}"))
}

/// Labels of every meeting, for client-side filtering of the meeting list.
#[tauri::command]
pub async fn api_list_meeting_labels(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<MeetingLabels>, String> {
    all_labels(state.db_manager.pool())
        .await
        .map_err(|e| format!("Failed to list labels: {e}"))
}

/// Distinct tags and projects with usage counts, most used first.
#[tauri::command]
pub async fn api_list_workspace_labels(
    state: tauri::State<'_, AppState>,
) -> Result<WorkspaceLabels, String> {
    let pool = state.db_manager.pool();
    let to_counts = |rows: Vec<(String, i64)>| {
        rows.into_iter().map(|(name, count)| LabelCount { name, count }).collect::<Vec<_>>()
    };
    let tags = sqlx::query_as::<_, (String, i64)>(
        "SELECT tag, COUNT(*) FROM meeting_tags GROUP BY tag ORDER BY COUNT(*) DESC, tag",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list tags: {e}"))?;
    let projects = sqlx::query_as::<_, (String, i64)>(
        "SELECT project, COUNT(*) FROM meeting_projects GROUP BY project ORDER BY COUNT(*) DESC, project",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Failed to list projects: {e}"))?;
    Ok(WorkspaceLabels { tags: to_counts(tags), projects: to_counts(projects) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_cleaned_and_deduplicated() {
        let tags = clean_tags(&[
            "#尽调".into(),
            " 尽调 ".into(),
            "Series  A".into(),
            "series a".into(),
            "  ".into(),
        ]);
        assert_eq!(tags, vec!["尽调".to_string(), "Series A".to_string()]);
        assert_eq!(clean_label("＃项目"), Some("项目".into()));
    }
}
