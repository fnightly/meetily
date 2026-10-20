//! Persistence for user-authored meeting notes.
//!
//! The `meeting_notes` table has existed in the migrations for a while but had
//! no read or write path in Rust. This repository is that path.
//!
//! Notes are stored in two representations:
//! - `notes_json`: the structured block document the editor works with.
//! - `notes_markdown`: a derived plain-text rendering, kept so exporting and
//!   searching do not need to understand the editor's format.
//!
//! Exactly one row exists per meeting.

use chrono::Utc;
use log::info as log_info;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// A single content block in a meeting note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NoteBlock {
    /// A run of plain text.
    Text { text: String },
    /// A bullet list item.
    Bullet { text: String },
    /// A heading; `level` is 1-6.
    Heading { text: String, level: u8 },
    /// An image stored on disk. `path` is absolute.
    Image {
        path: String,
        #[serde(default)]
        alt: String,
    },
}

/// The full note document for one meeting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingNote {
    pub meeting_id: String,
    #[serde(default)]
    pub blocks: Vec<NoteBlock>,
    pub created_at: String,
    pub updated_at: String,
    /// True when the meeting already had a note before this load.
    #[serde(default)]
    pub exists: bool,
}

/// Payload used when writing a note; timestamps are managed by the repository.
#[derive(Debug, Clone, Deserialize)]
pub struct NoteInput {
    #[serde(default)]
    pub blocks: Vec<NoteBlock>,
    /// Optional explicit markdown; when omitted it is derived from `blocks`.
    #[serde(default)]
    pub markdown: Option<String>,
}

pub struct MeetingNotesRepository;

impl MeetingNotesRepository {
    /// Load a meeting's note, returning an empty document when none exists yet.
    ///
    /// Returning an empty note rather than `None` keeps the caller simple: a
    /// meeting without notes is a normal state, not an error.
    pub async fn get(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<MeetingNote, sqlx::Error> {
        let row: Option<(Option<String>, Option<String>, String, String)> = sqlx::query_as(
            "SELECT notes_json, notes_markdown, created_at, updated_at
             FROM meeting_notes WHERE meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?;

        let Some((notes_json, notes_markdown, created_at, updated_at)) = row else {
            let now = Utc::now().to_rfc3339();
            return Ok(MeetingNote {
                meeting_id: meeting_id.to_string(),
                blocks: Vec::new(),
                created_at: now.clone(),
                updated_at: now,
                exists: false,
            });
        };

        // Prefer the structured document; fall back to rendering the stored
        // markdown if the JSON is absent or unreadable, so a note is never lost
        // to a format problem.
        let blocks = notes_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<NoteBlock>>(json).ok())
            .unwrap_or_else(|| {
                notes_markdown
                    .as_deref()
                    .map(markdown_to_blocks)
                    .unwrap_or_default()
            });

        Ok(MeetingNote {
            meeting_id: meeting_id.to_string(),
            blocks,
            created_at,
            updated_at,
            exists: true,
        })
    }

    /// Create or replace a meeting's note.
    pub async fn save(
        pool: &SqlitePool,
        meeting_id: &str,
        input: &NoteInput,
    ) -> Result<MeetingNote, sqlx::Error> {
        let json = serde_json::to_string(&input.blocks).unwrap_or_else(|_| "[]".to_string());
        let markdown = input
            .markdown
            .clone()
            .unwrap_or_else(|| blocks_to_markdown(&input.blocks));

        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO meeting_notes (meeting_id, notes_markdown, notes_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(meeting_id) DO UPDATE SET
                notes_markdown = excluded.notes_markdown,
                notes_json = excluded.notes_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(meeting_id)
        .bind(&markdown)
        .bind(&json)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await?;

        log_info!(
            "Saved meeting note for {} ({} blocks, {} chars of markdown)",
            meeting_id,
            input.blocks.len(),
            markdown.len()
        );

        Self::get(pool, meeting_id).await
    }

    /// Remove a meeting's note. Returns whether a row was deleted.
    pub async fn delete(pool: &SqlitePool, meeting_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM meeting_notes WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Whether a meeting has a note with any content.
    pub async fn has_content(pool: &SqlitePool, meeting_id: &str) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM meeting_notes
             WHERE meeting_id = ?
               AND (COALESCE(notes_markdown, '') <> '' OR COALESCE(notes_json, '') NOT IN ('', '[]'))",
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await?;
        Ok(count.0 > 0)
    }

    /// Absolute paths of the images referenced by a meeting's note.
    ///
    /// Used when a meeting is deleted: the note row is removed by the database
    /// cascade, but the image files live in the notes folder and would otherwise
    /// be orphaned. Only image blocks carry files; text is stored in the row.
    pub async fn image_paths(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        Ok(Self::get(pool, meeting_id)
            .await?
            .blocks
            .into_iter()
            .filter_map(|block| match block {
                NoteBlock::Image { path, .. } => Some(path),
                _ => None,
            })
            .collect())
    }
}

/// Render blocks to markdown, so notes stay readable outside the app.
pub fn blocks_to_markdown(blocks: &[NoteBlock]) -> String {
    let mut out = String::new();

    for block in blocks {
        match block {
            NoteBlock::Heading { text, level } => {
                let hashes = "#".repeat((*level).clamp(1, 6) as usize);
                out.push_str(&format!("{} {}\n\n", hashes, text.trim()));
            }
            NoteBlock::Bullet { text } => {
                out.push_str(&format!("- {}\n", text.trim()));
            }
            NoteBlock::Text { text } => {
                out.push_str(&format!("{}\n\n", text.trim()));
            }
            NoteBlock::Image { path, alt } => {
                out.push_str(&format!("![{}]({})\n\n", alt, path));
            }
        }
    }

    out.trim_end().to_string()
}

/// Best-effort reconstruction of blocks from markdown.
///
/// Only used as a recovery path when the structured JSON is unavailable, so it
/// deliberately handles just the shapes [`blocks_to_markdown`] emits.
pub fn markdown_to_blocks(markdown: &str) -> Vec<NoteBlock> {
    let mut blocks = Vec::new();

    for raw_line in markdown.lines() {
        let line = raw_line.trim_end();

        if line.trim().is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("###### ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 6 });
        } else if let Some(rest) = line.strip_prefix("##### ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 5 });
        } else if let Some(rest) = line.strip_prefix("#### ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 4 });
        } else if let Some(rest) = line.strip_prefix("### ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 3 });
        } else if let Some(rest) = line.strip_prefix("## ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 2 });
        } else if let Some(rest) = line.strip_prefix("# ") {
            blocks.push(NoteBlock::Heading { text: rest.to_string(), level: 1 });
        } else if let Some(rest) = line.strip_prefix("- ") {
            blocks.push(NoteBlock::Bullet { text: rest.to_string() });
        } else if let Some(image) = parse_markdown_image(line.trim()) {
            blocks.push(image);
        } else {
            blocks.push(NoteBlock::Text { text: line.to_string() });
        }
    }

    blocks
}

/// Parse `![alt](path)` into an image block.
fn parse_markdown_image(line: &str) -> Option<NoteBlock> {
    let rest = line.strip_prefix("![")?;
    let (alt, rest) = rest.split_once("](")?;
    let path = rest.strip_suffix(')')?;

    if path.is_empty() {
        return None;
    }

    Some(NoteBlock::Image {
        path: path.to_string(),
        alt: alt.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_round_trips_through_blocks() {
        let blocks = vec![
            NoteBlock::Heading { text: "Agenda".into(), level: 2 },
            NoteBlock::Text { text: "First topic".into() },
            NoteBlock::Bullet { text: "Ship it".into() },
            NoteBlock::Image { path: "/tmp/diagram.png".into(), alt: "diagram".into() },
        ];

        let markdown = blocks_to_markdown(&blocks);
        assert!(markdown.contains("## Agenda"));
        assert!(markdown.contains("- Ship it"));
        assert!(markdown.contains("![diagram](/tmp/diagram.png)"));

        let restored = markdown_to_blocks(&markdown);
        assert_eq!(restored, blocks);
    }

    #[test]
    fn image_without_alt_is_preserved() {
        let blocks = vec![NoteBlock::Image { path: "/tmp/a.png".into(), alt: String::new() }];
        let restored = markdown_to_blocks(&blocks_to_markdown(&blocks));
        assert_eq!(restored, blocks);
    }

    #[test]
    fn empty_note_renders_empty_markdown() {
        assert_eq!(blocks_to_markdown(&[]), "");
        assert!(markdown_to_blocks("").is_empty());
    }

    #[test]
    fn heading_level_is_clamped_when_rendering() {
        let blocks = vec![NoteBlock::Heading { text: "deep".into(), level: 9 }];
        assert_eq!(blocks_to_markdown(&blocks), "###### deep");
    }

    #[test]
    fn markdown_parse_ignores_blank_lines() {
        let blocks = markdown_to_blocks("\n\nhello\n\n");
        assert_eq!(blocks, vec![NoteBlock::Text { text: "hello".into() }]);
    }

    #[test]
    fn malformed_image_markdown_falls_back_to_text() {
        // An unterminated image must not be silently dropped.
        let blocks = markdown_to_blocks("![alt](/tmp/a.png");
        assert_eq!(blocks, vec![NoteBlock::Text { text: "![alt](/tmp/a.png".into() }]);
    }

    #[tokio::test]
    async fn image_paths_extracts_images_from_structured_and_markdown_notes() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory database");
        sqlx::query(
            "CREATE TABLE meeting_notes (
                meeting_id TEXT PRIMARY KEY,
                notes_markdown TEXT,
                notes_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create meeting_notes table");

        let block_note = NoteInput {
            blocks: vec![
                NoteBlock::Text { text: "hello".into() },
                NoteBlock::Image { path: "/notes/images/a.png".into(), alt: "a".into() },
            ],
            markdown: None,
        };
        MeetingNotesRepository::save(&pool, "m-structured", &block_note)
            .await
            .expect("save note");

        let structured = MeetingNotesRepository::image_paths(&pool, "m-structured")
            .await
            .expect("image paths");
        assert_eq!(structured, vec!["/notes/images/a.png".to_string()]);

        // A note stored only as markdown (the recovery path) still exposes images.
        // notes_json is NULL, not "[]": a readable empty document takes precedence
        // over the markdown fallback.
        sqlx::query(
            "INSERT INTO meeting_notes (meeting_id, notes_markdown, notes_json, created_at, updated_at)
             VALUES ('m-markdown', '![b](/notes/images/b.png)', NULL, 'x', 'x')",
        )
        .execute(&pool)
        .await
        .expect("insert legacy note");

        let legacy = MeetingNotesRepository::image_paths(&pool, "m-markdown")
            .await
            .expect("legacy image paths");
        assert_eq!(legacy, vec!["/notes/images/b.png".to_string()]);

        // A meeting without a note is not an error and yields nothing to delete.
        assert!(MeetingNotesRepository::image_paths(&pool, "missing")
            .await
            .expect("missing note")
            .is_empty());
    }
}
