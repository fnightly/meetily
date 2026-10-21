use crate::api::{MeetingDetails, MeetingTranscript};
use crate::database::models::{MeetingModel, Transcript};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqliteConnection, SqlitePool};
use tracing::{error, info};

/// Largest page of transcripts one request may load.
///
/// SQLite treats a negative LIMIT as "no limit", so an unchecked value from the
/// interface would pull the whole table into memory. The window is clamped to
/// this ceiling, and a zero or negative page size is clamped up to one row.
pub const MAX_TRANSCRIPTS_PAGE_LIMIT: i64 = 1000;

/// Clamp a renderer-supplied transcript page window to something bounded.
pub fn clamp_transcript_page(limit: i64, offset: i64) -> (i64, i64) {
    (limit.clamp(1, MAX_TRANSCRIPTS_PAGE_LIMIT), offset.max(0))
}

pub struct MeetingsRepository;

impl MeetingsRepository {
    pub async fn get_meetings(pool: &SqlitePool) -> Result<Vec<MeetingModel>, sqlx::Error> {
        let meetings =
            sqlx::query_as::<_, MeetingModel>("SELECT * FROM meetings ORDER BY created_at DESC")
                .fetch_all(pool)
                .await?;
        Ok(meetings)
    }

    pub async fn delete_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        match delete_meeting_with_transaction(&mut transaction, meeting_id).await {
            Ok(success) => {
                if success {
                    transaction.commit().await?;
                    info!(
                        "Successfully deleted meeting {} and all associated data",
                        meeting_id
                    );
                    Ok(true)
                } else {
                    transaction.rollback().await?;
                    Ok(false)
                }
            }
            Err(e) => {
                let _ = transaction.rollback().await;
                error!("Failed to delete meeting {}: {}", meeting_id, e);
                Err(e)
            }
        }
    }

    pub async fn get_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingDetails>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        // Get meeting details
        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?;

        if meeting.is_none() {
            transaction.rollback().await?;
            return Err(SqlxError::RowNotFound);
        }

        if let Some(meeting) = meeting {
            // Get all transcripts for this meeting
            let transcripts =
                sqlx::query_as::<_, Transcript>("SELECT * FROM transcripts WHERE meeting_id = ?")
                    .bind(meeting_id)
                    .fetch_all(&mut *transaction)
                    .await?;

            transaction.commit().await?;

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                    speaker: t.speaker,
                })
                .collect::<Vec<_>>();

            Ok(Some(MeetingDetails {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at.0.to_rfc3339(),
                updated_at: meeting.updated_at.0.to_rfc3339(),
                transcripts: meeting_transcripts,
            }))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }

    /// Get meeting metadata without transcripts (for pagination)
    pub async fn get_meeting_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingModel>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;

        Ok(meeting)
    }

    /// Get meeting transcripts with pagination support
    pub async fn get_meeting_transcripts_paginated(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Transcript>, i64), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        // Defense in depth: the command clamps too, but a direct repository caller
        // must not be able to ask SQLite for "-1 LIMIT = everything".
        let (limit, offset) = clamp_transcript_page(limit, offset);

        // Get total count of transcripts for this meeting
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?"
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await?;

        // Get paginated transcripts ordered by audio_start_time
        let transcripts = sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts
             WHERE meeting_id = ?
             ORDER BY audio_start_time ASC
             LIMIT ? OFFSET ?"
        )
        .bind(meeting_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        Ok((transcripts, total.0))
    }

    pub async fn update_meeting_title(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;
        if rows_affected.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let now = Utc::now();

        // Update meetings table
        let meeting_update =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;

        if meeting_update.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false); // Meeting not found
        }

        // Update transcript_chunks table
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(new_title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_page_is_clamped_to_a_bounded_window() {
        // A normal request is passed through unchanged.
        assert_eq!(clamp_transcript_page(50, 0), (50, 0));
        assert_eq!(clamp_transcript_page(1, 40), (1, 40));

        // Zero/negative limits and offsets can only mean "everything"/"before the
        // start", which must not reach SQLite as LIMIT -1.
        assert_eq!(clamp_transcript_page(0, 0), (1, 0));
        assert_eq!(clamp_transcript_page(-1, -5), (1, 0));

        // An oversized request is capped instead of loading the whole table.
        assert_eq!(
            clamp_transcript_page(i64::MAX, 10),
            (MAX_TRANSCRIPTS_PAGE_LIMIT, 10)
        );
        assert_eq!(
            clamp_transcript_page(MAX_TRANSCRIPTS_PAGE_LIMIT + 1, 0),
            (MAX_TRANSCRIPTS_PAGE_LIMIT, 0)
        );
    }

    /// In-memory schema with one row in every meeting-scoped table for two
    /// meetings, so a delete can be checked for both cleanup and over-deletion.
    async fn pool_with_child_rows() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");

        for statement in [
            "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, folder_path TEXT)",
            "CREATE TABLE transcripts (meeting_id TEXT NOT NULL)",
            "CREATE TABLE transcript_chunks (meeting_id TEXT NOT NULL)",
            "CREATE TABLE summary_processes (meeting_id TEXT NOT NULL)",
            "CREATE TABLE meeting_notes (meeting_id TEXT NOT NULL)",
            "CREATE TABLE meeting_tags (meeting_id TEXT NOT NULL, tag TEXT NOT NULL)",
            "CREATE TABLE meeting_speakers (meeting_id TEXT NOT NULL, speaker TEXT NOT NULL)",
            "CREATE TABLE meeting_projects (meeting_id TEXT NOT NULL, project TEXT NOT NULL)",
            "CREATE TABLE action_items (id TEXT NOT NULL, meeting_id TEXT)",
            "CREATE TABLE action_item_imports (meeting_id TEXT NOT NULL)",
        ] {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("create table");
        }

        for meeting_id in ["m1", "m2"] {
            for (statement, extra) in [
                (
                    "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'title', 'now', 'now')",
                    false,
                ),
                ("INSERT INTO transcripts (meeting_id) VALUES (?)", false),
                ("INSERT INTO transcript_chunks (meeting_id) VALUES (?)", false),
                ("INSERT INTO summary_processes (meeting_id) VALUES (?)", false),
                ("INSERT INTO meeting_notes (meeting_id) VALUES (?)", false),
                (
                    "INSERT INTO meeting_tags (meeting_id, tag) VALUES (?, 'tag')",
                    false,
                ),
                (
                    "INSERT INTO meeting_speakers (meeting_id, speaker) VALUES (?, 'speaker_1')",
                    false,
                ),
                (
                    "INSERT INTO meeting_projects (meeting_id, project) VALUES (?, 'project')",
                    false,
                ),
                (
                    "INSERT INTO action_items (id, meeting_id) VALUES (?, ?)",
                    true,
                ),
                ("INSERT INTO action_item_imports (meeting_id) VALUES (?)", false),
            ] {
                let mut query = sqlx::query(statement).bind(meeting_id);
                if extra {
                    query = query.bind(meeting_id);
                }
                query.execute(&pool).await.expect("insert child row");
            }
        }

        pool
    }

    async fn count_for(pool: &sqlx::SqlitePool, table: &str, meeting_id: &str) -> i64 {
        let (count,): (i64,) =
            sqlx::query_as(&format!("SELECT COUNT(*) FROM {table} WHERE meeting_id = ?"))
                .bind(meeting_id)
                .fetch_one(pool)
                .await
                .expect("count rows");
        count
    }

    #[tokio::test]
    async fn delete_meeting_removes_every_child_row_explicitly() {
        let pool = pool_with_child_rows().await;

        assert!(
            MeetingsRepository::delete_meeting(&pool, "m1")
                .await
                .expect("delete meeting"),
            "the meeting existed and must be reported deleted"
        );

        const CHILD_TABLES: &[&str] = &[
            "transcripts",
            "transcript_chunks",
            "summary_processes",
            "meeting_notes",
            "meeting_tags",
            "meeting_speakers",
            "meeting_projects",
            "action_items",
            "action_item_imports",
        ];

        for table in CHILD_TABLES {
            assert_eq!(
                count_for(&pool, table, "m1").await,
                0,
                "{table} still has rows for the deleted meeting"
            );
            assert_eq!(
                count_for(&pool, table, "m2").await,
                1,
                "{table} lost a row of another meeting"
            );
        }

        let (meetings,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .expect("count meetings");
        assert_eq!(meetings, 1, "only the deleted meeting row may be gone");
    }
}

async fn delete_meeting_with_transaction(
    transaction: &mut SqliteConnection,
    meeting_id: &str,
) -> Result<bool, SqlxError> {
    // Check if meeting exists
    let meeting_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

    if meeting_exists.is_none() {
        error!("Meeting {} not found for deletion", meeting_id);
        return Ok(false);
    }

    // Delete from related tables in proper order
    // 1. Delete from transcript_chunks
    sqlx::query("DELETE FROM transcript_chunks WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 2. Delete from summary_processes
    sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 3. Delete from transcripts
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 4. Workspace and notes data. The schema also declares ON DELETE CASCADE for
    // these tables, but deleting them explicitly keeps the operation correct if
    // foreign-key enforcement is ever off (a maintenance connection, a future
    // default change) instead of silently orphaning the rows.
    sqlx::query("DELETE FROM meeting_notes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("DELETE FROM meeting_tags WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("DELETE FROM meeting_speakers WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("DELETE FROM meeting_projects WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("DELETE FROM action_items WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("DELETE FROM action_item_imports WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 5. Finally, delete the meeting
    let result = sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    Ok(result.rows_affected() > 0)
}
