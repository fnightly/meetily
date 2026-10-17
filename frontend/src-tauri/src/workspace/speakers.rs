//! Display names for diarization labels.
//!
//! Separation writes `speaker_1`, `speaker_2`, … into each transcript row. The
//! user names them per meeting; the names are applied wherever the transcript
//! is shown, copied, exported, summarised or searched.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::state::AppState;

const MAX_NAME_CHARS: usize = 40;

pub async fn speaker_names(pool: &SqlitePool, meeting_id: &str) -> Result<BTreeMap<String, String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT speaker, display_name FROM meeting_speakers WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Normalise a user-entered name; `None` clears the name.
pub fn clean_name(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_NAME_CHARS).collect())
}

pub async fn set_speaker_name(
    pool: &SqlitePool,
    meeting_id: &str,
    speaker: &str,
    name: Option<&str>,
) -> Result<BTreeMap<String, String>, sqlx::Error> {
    match clean_name(name) {
        Some(name) => {
            sqlx::query(
                "INSERT INTO meeting_speakers (meeting_id, speaker, display_name, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT(meeting_id, speaker) DO UPDATE SET display_name = excluded.display_name, updated_at = excluded.updated_at",
            )
            .bind(meeting_id)
            .bind(speaker)
            .bind(&name)
            .bind(super::now_rfc3339())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ? AND speaker = ?")
                .bind(meeting_id)
                .bind(speaker)
                .execute(pool)
                .await?;
        }
    }
    speaker_names(pool, meeting_id).await
}

/// Human label for a stored speaker value: the user's name, else "说话人 N".
pub fn display_speaker(names: &BTreeMap<String, String>, speaker: &str) -> String {
    if let Some(name) = names.get(speaker) {
        return name.clone();
    }
    match speaker.strip_prefix("speaker_") {
        Some(n) if n.chars().all(|c| c.is_ascii_digit()) => format!("说话人 {n}"),
        _ => speaker.to_string(),
    }
}

#[tauri::command]
pub async fn api_get_speaker_names(
    meeting_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<BTreeMap<String, String>, String> {
    speaker_names(state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|e| format!("Failed to load speaker names: {e}"))
}

#[tauri::command]
pub async fn api_set_speaker_name(
    meeting_id: String,
    speaker: String,
    name: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<BTreeMap<String, String>, String> {
    if meeting_id.trim().is_empty() || speaker.trim().is_empty() {
        return Err("meeting_id and speaker are required".into());
    }
    set_speaker_name(state.db_manager.pool(), &meeting_id, &speaker, name.as_deref())
        .await
        .map_err(|e| format!("Failed to save speaker name: {e}"))
}

/// Names used before, most recent first, for quick re-use across meetings.
#[tauri::command]
pub async fn api_list_known_speaker_names(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT display_name FROM meeting_speakers GROUP BY display_name ORDER BY MAX(updated_at) DESC LIMIT 30",
    )
    .fetch_all(state.db_manager.pool())
    .await
    .map(|rows| rows.into_iter().map(|(n,)| n).collect())
    .map_err(|e| format!("Failed to list speaker names: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(clean_name(Some("  张  总 ")).as_deref(), Some("张 总"));
        assert_eq!(clean_name(Some("   ")), None);
        assert_eq!(clean_name(None), None);
        assert_eq!(clean_name(Some(&"a".repeat(80))).unwrap().len(), MAX_NAME_CHARS);
    }

    #[test]
    fn display_falls_back_to_numbered_label() {
        let mut names = BTreeMap::new();
        names.insert("speaker_1".to_string(), "王总".to_string());
        assert_eq!(display_speaker(&names, "speaker_1"), "王总");
        assert_eq!(display_speaker(&names, "speaker_2"), "说话人 2");
        assert_eq!(display_speaker(&names, "host"), "host");
    }
}
