//! Tauri command surface for syncing one meeting to Obsidian or Notion.
//!
//! Both commands are explicit: nothing is pushed on save, and nothing at all
//! reaches the network until the user presses Sync. The note body is rendered by
//! the frontend from the same persisted data as the Markdown export, so there is
//! only one definition of what a meeting note contains.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

use crate::database::repositories::setting::SettingsRepository;

use super::notion;
use super::obsidian::{self, ObsidianWriteOutcome};
use super::settings::{
    load_sync_settings, obsidian_notes_folder, save_sync_settings as persist_sync_settings,
    sanitize_settings, SyncSettings, SyncSettingsView,
};

/// One meeting's note, as prepared by the frontend.
#[derive(Debug, Clone, Deserialize)]
pub struct SyncNoteRequest {
    /// Used for the frontmatter, so a note can be traced back to its meeting.
    pub meeting_id: String,
    pub title: String,
    /// The meeting's `created_at`; only the date is used.
    pub date: String,
    /// Complete Markdown from `buildMeetingMarkdown`.
    pub markdown: String,
}

#[derive(Debug, Serialize)]
pub struct NotionSyncResult {
    pub page_id: String,
    pub page_url: Option<String>,
    pub blocks_written: usize,
    pub database_title: Option<String>,
    pub data_source_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NotionTestResult {
    pub database_title: Option<String>,
    pub data_source_name: Option<String>,
    pub title_property: String,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// The database pool, or a clear error while the app is still starting.
fn database_pool<R: Runtime>(app: &AppHandle<R>) -> Result<sqlx::SqlitePool, String> {
    app.try_state::<crate::state::AppState>()
        .map(|state| state.db_manager.pool().clone())
        .ok_or_else(|| "The meeting database is still starting up; try again in a moment.".to_string())
}

/// The stored token, trimmed; `None` when none is saved.
///
/// Never logged and never returned to the renderer — the UI only learns whether
/// one exists.
async fn stored_notion_token<R: Runtime>(app: &AppHandle<R>) -> Result<Option<String>, String> {
    let Some(state) = app.try_state::<crate::state::AppState>() else {
        return Ok(None);
    };

    SettingsRepository::get_notion_api_key(state.db_manager.pool())
        .await
        .map(|token| {
            token
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
        })
        .map_err(|error| format!("Could not read the saved Notion token: {error}"))
}

fn build_settings_view<R: Runtime>(
    app: &AppHandle<R>,
    settings: SyncSettings,
    notion_token_set: bool,
) -> Result<SyncSettingsView, String> {
    let default_folder = crate::storage::load_storage_config(app).notes_folder(app)?;
    let effective_folder = super::settings::effective_obsidian_folder(app, &settings)?;
    let notes_folder = obsidian_notes_folder(app, &settings)?;

    Ok(SyncSettingsView {
        settings,
        default_obsidian_folder: default_folder.to_string_lossy().to_string(),
        effective_obsidian_folder: effective_folder.to_string_lossy().to_string(),
        obsidian_notes_folder: notes_folder.to_string_lossy().to_string(),
        notion_token_set,
    })
}

#[tauri::command]
pub async fn get_sync_settings<R: Runtime>(app: AppHandle<R>) -> Result<SyncSettingsView, String> {
    let settings = load_sync_settings(&app);

    let notion_token_set = match stored_notion_token(&app).await {
        Ok(token) => token.is_some(),
        // The settings panel must still open if the database is mid-startup;
        // the Notion target simply looks unconfigured until it is re-read.
        Err(error) => {
            log::warn!("Could not check the saved Notion token: {error}");
            false
        }
    };

    build_settings_view(&app, settings, notion_token_set)
}

/// Persist the settings.
///
/// `notion_token` is `None` to keep the stored token, `Some("")` to remove it,
/// and `Some(value)` to replace it. The token is written through the same
/// repository as every other API key; it is never echoed back.
#[tauri::command]
pub async fn save_sync_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: SyncSettings,
    notion_token: Option<String>,
) -> Result<SyncSettingsView, String> {
    let settings = sanitize_settings(settings);
    persist_sync_settings(&app, &settings)?;

    if let Some(token) = notion_token {
        let token = token.trim().to_string();
        let pool = database_pool(&app)?;

        if token.is_empty() {
            SettingsRepository::delete_notion_api_key(&pool)
                .await
                .map_err(|error| format!("Could not remove the saved Notion token: {error}"))?;
            log::info!("Removed the saved Notion integration token");
        } else {
            SettingsRepository::save_notion_api_key(&pool, &token)
                .await
                .map_err(|error| format!("Could not save the Notion token: {error}"))?;
            // The value itself must never reach the log.
            log::info!("Saved a Notion integration token");
        }
    }

    get_sync_settings(app).await
}

#[tauri::command]
pub async fn pick_obsidian_folder<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let settings = load_sync_settings(&app);
    let current = super::settings::effective_obsidian_folder(&app, &settings).ok();

    let app_for_dialog = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let mut builder = app_for_dialog
            .dialog()
            .file()
            .set_title("Select Obsidian vault folder");

        if let Some(folder) = current.filter(|folder| folder.exists()) {
            builder = builder.set_directory(folder);
        }

        builder.blocking_pick_folder()
    })
    .await
    .map_err(|error| format!("Folder dialog task failed: {error}"))?;

    Ok(picked.map(|path| path.to_string()))
}

/// Write one meeting into the Obsidian vault.
///
/// Purely local: this command never builds an HTTP client.
#[tauri::command]
pub async fn obsidian_sync_meeting<R: Runtime>(
    app: AppHandle<R>,
    request: SyncNoteRequest,
) -> Result<ObsidianWriteOutcome, String> {
    if request.markdown.trim().is_empty() {
        return Err("This meeting has no note content to sync yet.".to_string());
    }

    let settings = load_sync_settings(&app);
    let folder = obsidian_notes_folder(&app, &settings)?;

    let date = obsidian::meeting_date(&request.date);
    let title = non_empty(Some(request.title.as_str())).unwrap_or("Untitled meeting");
    let content = obsidian::render_note(
        title,
        date.as_deref(),
        &settings.obsidian_tags,
        &request.meeting_id,
        &request.markdown,
        settings.obsidian_template.as_deref(),
    );
    let file_name = obsidian::note_file_name(title, date.as_deref());

    let outcome = tokio::task::spawn_blocking(move || {
        obsidian::write_note(&folder, &file_name, &content)
    })
    .await
    .map_err(|error| format!("Obsidian sync task failed: {error}"))??;

    log::info!(
        "Obsidian sync wrote {} ({:?})",
        outcome.path,
        outcome.status
    );

    Ok(outcome)
}

/// The token and database id, or an error that says what to configure.
async fn notion_target<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<(String, String), String> {
    let settings = load_sync_settings(app);

    let database_id = non_empty(settings.notion_database_id.as_deref())
        .ok_or_else(|| {
            "Notion is not configured: add the database ID in Settings → Sync.".to_string()
        })?
        .to_string();
    let database_id = notion::normalize_database_id(&database_id)?;

    let token = stored_notion_token(app)
        .await?
        .ok_or_else(|| {
            "Notion is not configured: add an integration token in Settings → Sync.".to_string()
        })?;

    Ok((database_id, token))
}

fn notion_client() -> reqwest::Client {
    // The page itself is quick; a long transcript can produce several append
    // calls, so the timeout is per request rather than for the whole sync.
    crate::http_client::client_with_proxy_and_timeout(Duration::from_secs(60))
}

/// Create one Notion page for the meeting, appending content in 100-block chunks.
#[tauri::command]
pub async fn notion_sync_meeting<R: Runtime>(
    app: AppHandle<R>,
    request: SyncNoteRequest,
) -> Result<NotionSyncResult, String> {
    if request.markdown.trim().is_empty() {
        return Err("This meeting has no note content to sync yet.".to_string());
    }

    let (database_id, token) = notion_target(&app).await?;
    let title = non_empty(Some(request.title.as_str())).unwrap_or("Untitled meeting");

    let blocks = notion::markdown_to_blocks(&request.markdown);
    if blocks.is_empty() {
        return Err("There is no text in this meeting note to send to Notion.".to_string());
    }

    let total: usize = blocks.len();
    let chunks = notion::chunk_blocks(blocks, notion::MAX_BLOCKS_PER_REQUEST);
    let mut chunks = chunks.into_iter();

    let client = notion_client();
    let target = notion::discover_target(&client, &token, &database_id).await?;

    let first = chunks.next().unwrap_or_default();
    let page =
        notion::create_page(&client, &token, &target, title, first).await?;

    for chunk in chunks {
        notion::append_children(&client, &token, &page.id, chunk)
            .await
            .map_err(|error| {
                // The page exists at this point, so say so: retrying from the
                // dialog creates a second page, and the user should know that.
                format!(
                    "The Notion page was created (id {}) but the rest of the note could not be \
                     added: {error}",
                    page.id
                )
            })?;
    }

    log::info!(
        "Notion sync created page {} with {} block(s) across {} request(s)",
        page.id,
        total,
        chunks_count(total)
    );

    Ok(NotionSyncResult {
        page_id: page.id,
        page_url: page.url,
        blocks_written: total,
        database_title: target.database_title,
        data_source_name: target.data_source_name,
    })
}

/// Number of create/append requests needed for `blocks`, for the log line.
fn chunks_count(blocks: usize) -> usize {
    if blocks == 0 {
        0
    } else {
        blocks.div_ceil(notion::MAX_BLOCKS_PER_REQUEST)
    }
}

/// Check the saved token and database without creating a page.
#[tauri::command]
pub async fn notion_test_connection<R: Runtime>(
    app: AppHandle<R>,
) -> Result<NotionTestResult, String> {
    let (database_id, token) = notion_target(&app).await?;
    let client = notion_client();
    let target = notion::discover_target(&client, &token, &database_id).await?;

    Ok(NotionTestResult {
        database_title: target.database_title,
        data_source_name: target.data_source_name,
        title_property: target.title_property,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_chunk_count_rounds_up() {
        assert_eq!(chunks_count(0), 0);
        assert_eq!(chunks_count(1), 1);
        assert_eq!(chunks_count(100), 1);
        assert_eq!(chunks_count(101), 2);
        assert_eq!(chunks_count(250), 3);
    }
}
