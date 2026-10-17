//! Action items: follow-ups that outlive the summary they came from.
//!
//! Items are extracted from a summary on the frontend (which already parses
//! summary markdown) and stored here so they can be ticked off, assigned and
//! dated. Each meeting is imported once automatically; a forced re-import adds
//! only items whose text is new.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ActionItem {
    pub id: String,
    pub meeting_id: Option<String>,
    pub meeting_title: Option<String>,
    pub text: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
    pub done: bool,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionItemDraft {
    pub text: String,
    pub owner: Option<String>,
    pub due_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ActionItemPatch {
    pub text: Option<String>,
    /// `Some("")` clears the owner.
    pub owner: Option<String>,
    /// `Some("")` clears the due date.
    pub due_date: Option<String>,
    pub done: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportOutcome {
    pub imported: usize,
    pub skipped: bool,
}

const SELECT_ITEMS: &str = "SELECT a.id, a.meeting_id, m.title AS meeting_title, a.text, a.owner, a.due_date,
        a.done, a.source, a.created_at, a.updated_at, a.completed_at
     FROM action_items a LEFT JOIN meetings m ON m.id = a.meeting_id";

/// Comparison key for de-duplication: case-, space- and punctuation-insensitive.
pub fn dedupe_key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn clean_text(raw: &str) -> Option<String> {
    let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_start_matches(['-', '*', '•', '·']).trim().to_string();
    (!t.is_empty()).then(|| t.chars().take(500).collect())
}

fn clean_optional(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(60).collect())
}

pub async fn list(pool: &SqlitePool, meeting_id: Option<&str>, include_done: bool) -> Result<Vec<ActionItem>, sqlx::Error> {
    let mut sql = format!("{SELECT_ITEMS} WHERE 1 = 1");
    if meeting_id.is_some() {
        sql.push_str(" AND a.meeting_id = ?");
    }
    if !include_done {
        sql.push_str(" AND a.done = 0");
    }
    // Open first, then dated items by due date, then newest.
    sql.push_str(" ORDER BY a.done ASC, CASE WHEN a.due_date IS NULL THEN 1 ELSE 0 END, a.due_date ASC, a.created_at DESC LIMIT 500");
    let mut query = sqlx::query_as::<_, ActionItem>(&sql);
    if let Some(id) = meeting_id {
        query = query.bind(id.to_string());
    }
    query.fetch_all(pool).await
}

async fn get(pool: &SqlitePool, id: &str) -> Result<Option<ActionItem>, sqlx::Error> {
    sqlx::query_as::<_, ActionItem>(&format!("{SELECT_ITEMS} WHERE a.id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn create(
    pool: &SqlitePool,
    meeting_id: Option<&str>,
    draft: &ActionItemDraft,
    source: &str,
) -> Result<Option<ActionItem>, sqlx::Error> {
    let Some(text) = clean_text(&draft.text) else {
        return Ok(None);
    };
    let id = uuid::Uuid::new_v4().to_string();
    let now = super::now_rfc3339();
    sqlx::query(
        "INSERT INTO action_items (id, meeting_id, text, owner, due_date, done, source, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)",
    )
    .bind(&id)
    .bind(meeting_id)
    .bind(&text)
    .bind(clean_optional(draft.owner.as_deref()))
    .bind(clean_optional(draft.due_date.as_deref()))
    .bind(source)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    get(pool, &id).await
}

pub async fn update(pool: &SqlitePool, id: &str, patch: &ActionItemPatch) -> Result<Option<ActionItem>, sqlx::Error> {
    let Some(current) = get(pool, id).await? else {
        return Ok(None);
    };
    let text = patch.text.as_deref().and_then(clean_text).unwrap_or(current.text);
    let owner = match patch.owner.as_deref() {
        Some(o) => clean_optional(Some(o)),
        None => current.owner,
    };
    let due = match patch.due_date.as_deref() {
        Some(d) => clean_optional(Some(d)),
        None => current.due_date,
    };
    let done = patch.done.unwrap_or(current.done);
    let now = super::now_rfc3339();
    let completed_at = if done { current.completed_at.or_else(|| Some(now.clone())) } else { None };
    sqlx::query(
        "UPDATE action_items SET text = ?, owner = ?, due_date = ?, done = ?, completed_at = ?, updated_at = ? WHERE id = ?",
    )
    .bind(text)
    .bind(owner)
    .bind(due)
    .bind(done)
    .bind(completed_at)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    get(pool, id).await
}

pub async fn import(
    pool: &SqlitePool,
    meeting_id: &str,
    drafts: &[ActionItemDraft],
    force: bool,
) -> Result<ImportOutcome, sqlx::Error> {
    let already = sqlx::query_as::<_, (String,)>("SELECT meeting_id FROM action_item_imports WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?
        .is_some();
    if already && !force {
        return Ok(ImportOutcome { imported: 0, skipped: true });
    }
    let existing: Vec<String> = sqlx::query_as::<_, (String,)>("SELECT text FROM action_items WHERE meeting_id = ?")
        .bind(meeting_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(t,)| dedupe_key(&t))
        .collect();
    let mut seen = existing;
    let mut imported = 0;
    for draft in drafts {
        let key = dedupe_key(&draft.text);
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        if create(pool, Some(meeting_id), draft, "summary").await?.is_some() {
            imported += 1;
            seen.push(key);
        }
    }
    sqlx::query(
        "INSERT INTO action_item_imports (meeting_id, imported_at) VALUES (?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET imported_at = excluded.imported_at",
    )
    .bind(meeting_id)
    .bind(super::now_rfc3339())
    .execute(pool)
    .await?;
    Ok(ImportOutcome { imported, skipped: false })
}

#[tauri::command]
pub async fn api_list_action_items(
    meeting_id: Option<String>,
    include_done: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ActionItem>, String> {
    list(state.db_manager.pool(), meeting_id.as_deref(), include_done.unwrap_or(true))
        .await
        .map_err(|e| format!("Failed to list action items: {e}"))
}

#[tauri::command]
pub async fn api_create_action_item(
    meeting_id: Option<String>,
    item: ActionItemDraft,
    state: tauri::State<'_, AppState>,
) -> Result<ActionItem, String> {
    create(state.db_manager.pool(), meeting_id.as_deref(), &item, "manual")
        .await
        .map_err(|e| format!("Failed to create action item: {e}"))?
        .ok_or_else(|| "Action item text cannot be empty".to_string())
}

#[tauri::command]
pub async fn api_update_action_item(
    id: String,
    patch: ActionItemPatch,
    state: tauri::State<'_, AppState>,
) -> Result<ActionItem, String> {
    update(state.db_manager.pool(), &id, &patch)
        .await
        .map_err(|e| format!("Failed to update action item: {e}"))?
        .ok_or_else(|| "Action item not found".to_string())
}

#[tauri::command]
pub async fn api_delete_action_item(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    sqlx::query("DELETE FROM action_items WHERE id = ?")
        .bind(id)
        .execute(state.db_manager.pool())
        .await
        .map(|_| ())
        .map_err(|e| format!("Failed to delete action item: {e}"))
}

#[tauri::command]
pub async fn api_import_action_items(
    meeting_id: String,
    items: Vec<ActionItemDraft>,
    force: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<ImportOutcome, String> {
    import(state.db_manager.pool(), &meeting_id, &items, force.unwrap_or(false))
        .await
        .map_err(|e| format!("Failed to import action items: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_ignores_case_space_and_punctuation() {
        assert_eq!(dedupe_key("跟进 Acme 的 TS。"), dedupe_key("跟进acme的ts"));
    }

    #[test]
    fn text_is_cleaned() {
        assert_eq!(clean_text("- 发送 BP  给王总").as_deref(), Some("发送 BP 给王总"));
        assert_eq!(clean_text("  - "), None);
    }
}
