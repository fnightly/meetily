//! Obsidian target: one Markdown note per meeting, written straight to disk.
//!
//! Nothing here touches the network. The note body is rendered by the frontend
//! (`buildMeetingMarkdown`), because that is where all three persisted summary
//! representations are already understood; this module owns only what a
//! webview cannot do: choosing a filesystem-safe name, generating YAML
//! frontmatter, and writing the file without destroying an existing note.

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A single path component cannot contain these on Windows, and `/` is the
/// separator on every platform, so a title containing them must be rewritten.
const ILLEGAL_COMPONENT_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Windows silently strips a trailing dot or space, so `Report.` and `Report`
/// would be the same file and one would look like an overwrite.
fn clean_component(raw: &str, max_chars: usize) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if ILLEGAL_COMPONENT_CHARS.contains(&c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();

    let trimmed = cleaned
        .trim()
        .trim_end_matches(|c| c == '.' || c == ' ')
        .trim();

    trimmed.chars().take(max_chars).collect::<String>().trim().to_string()
}

/// Windows reserves these device names even with an extension (`CON.txt`).
fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim().to_ascii_uppercase();

    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }

    // COM1..COM9 and LPT1..LPT9 (the superscript variants are not representable
    // here and are rejected by Windows anyway).
    stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.as_bytes()[3].is_ascii_digit()
}

/// A filesystem-safe single path component (a file or folder name).
pub fn sanitize_component(raw: &str) -> String {
    let cleaned = clean_component(raw, 80);

    if cleaned.is_empty() {
        "untitled-meeting".to_string()
    } else if is_reserved_name(&cleaned) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

/// Normalise an optional vault subfolder.
///
/// Returns `None` for "no subfolder". Path traversal segments are dropped rather
/// than interpreted, so a stored `../../somewhere` can never write outside the
/// vault folder.
pub fn sanitize_subfolder(raw: Option<&str>) -> Option<String> {
    let raw = raw?;

    let parts: Vec<String> = raw
        .split(['/', '\\'])
        .filter_map(|part| {
            let cleaned = clean_component(part, 60);
            if cleaned.is_empty() {
                None
            } else if is_reserved_name(&cleaned) {
                Some(format!("_{cleaned}"))
            } else {
                Some(cleaned)
            }
        })
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// The calendar date of a meeting, from its `created_at` timestamp.
///
/// Returns `None` when the value cannot be understood: a note without a date is
/// better than one dated from a guess.
pub fn meeting_date(created_at: &str) -> Option<String> {
    let raw = created_at.trim();
    if raw.is_empty() {
        return None;
    }

    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(parsed.format("%Y-%m-%d").to_string());
    }

    // Already a plain `YYYY-MM-DD`, which is what older records and hand-built
    // payloads carry.
    let head: String = raw.chars().take(10).collect();
    let looks_like_date = head.len() == 10
        && head
            .chars()
            .enumerate()
            .all(|(index, c)| if index == 4 || index == 7 { c == '-' } else { c.is_ascii_digit() });

    if looks_like_date {
        Some(head)
    } else {
        None
    }
}

/// `Title YYYY-MM-DD.md`, with each part made safe.
///
/// The date makes the name stable across re-syncs (so identical content is
/// recognised) and unique across meetings, without depending on the recording
/// folder name.
pub fn note_file_name(title: &str, date: Option<&str>) -> String {
    let mut safe_title = clean_component(title, 60);
    if safe_title.is_empty() {
        safe_title = "untitled-meeting".to_string();
    } else if is_reserved_name(&safe_title) {
        safe_title = format!("_{safe_title}");
    }

    match date.map(str::trim).filter(|date| !date.is_empty()) {
        Some(date) => {
            let safe_date: String = date
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '-')
                .take(10)
                .collect();
            if safe_date.is_empty() {
                format!("{safe_title}.md")
            } else {
                format!("{safe_title} {safe_date}.md")
            }
        }
        None => format!("{safe_title}.md"),
    }
}

/// Quote a YAML scalar.
///
/// Always double-quoted: titles and tags come from meeting text and routinely
/// contain `:`, `#`, `[` or a leading `-`, all of which change the meaning of an
/// unquoted scalar. Newlines and tabs are folded to spaces because a raw newline
/// would break the block.
fn yaml_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');

    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' | '\r' | '\t' => quoted.push(' '),
            c if c.is_control() => quoted.push(' '),
            c => quoted.push(c),
        }
    }

    quoted.push('"');
    quoted
}

/// The YAML frontmatter block, including the closing `---` and a blank line.
pub fn frontmatter(title: &str, date: Option<&str>, meeting_id: &str, tags: &[String]) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_quote(title)));

    if let Some(date) = date {
        out.push_str(&format!("date: {}\n", yaml_quote(date)));
    }

    out.push_str(&format!("meeting_id: {}\n", yaml_quote(meeting_id)));
    out.push_str("source: meetily\n");

    if !tags.is_empty() {
        out.push_str("tags:\n");
        for tag in tags {
            out.push_str(&format!("  - {}\n", yaml_quote(tag)));
        }
    }

    out.push_str("---\n\n");
    out
}

/// Substitute the template placeholders in one pass.
///
/// A one-pass scan matters: meeting titles and note bodies are user text and
/// may themselves contain `{{title}}` or `{{content}}`. Replacing token by token
/// over the whole string would expand those injected values a second time.
///
/// Returns the rendered text and whether `{{content}}` was present.
fn render_template(
    template: &str,
    title: &str,
    date: &str,
    tags: &str,
    meeting_id: &str,
    markdown: &str,
) -> (String, bool) {
    let mut out = String::with_capacity(template.len() + markdown.len());
    let mut rest = template;
    let mut had_content = false;

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];

        let Some(end) = after.find("}}") else {
            out.push_str(after);
            rest = "";
            break;
        };

        match &after[2..end] {
            "title" => out.push_str(title),
            "date" => out.push_str(date),
            "tags" => out.push_str(tags),
            "meeting_id" => out.push_str(meeting_id),
            "content" => {
                out.push_str(markdown);
                had_content = true;
            }
            // Unknown placeholders stay visible rather than silently vanishing.
            _ => out.push_str(&after[..end + 2]),
        }

        rest = &after[end + 2..];
    }

    out.push_str(rest);
    (out, had_content)
}

/// Build the complete note: generated frontmatter, then the body.
///
/// A user template replaces the body wrapper, never the frontmatter: tags and
/// the date stay machine-readable whichever template is chosen. A template
/// without `{{content}}` gets the note appended, so choosing one cannot silently
/// drop the meeting.
pub fn render_note(
    title: &str,
    date: Option<&str>,
    tags: &[String],
    meeting_id: &str,
    markdown: &str,
    template: Option<&str>,
) -> String {
    let mut note = frontmatter(title, date, meeting_id, tags);
    let markdown = markdown.trim();

    let body = match template.map(str::trim).filter(|template| !template.is_empty()) {
        Some(template) => {
            let (mut rendered, had_content) = render_template(
                template,
                title,
                date.unwrap_or(""),
                &tags.join(", "),
                meeting_id,
                markdown,
            );

            if !had_content {
                rendered.push_str("\n\n");
                rendered.push_str(markdown);
            }

            rendered
        }
        None => markdown.to_string(),
    };

    note.push_str(body.trim_end());
    note.push('\n');
    note
}

/// How a write related to what was already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteStatus {
    /// No note existed at the computed name.
    Created,
    /// A note with identical content existed; nothing was written.
    Unchanged,
    /// A note with different content existed, so a numbered name was used.
    Alternative,
}

/// Result of writing one note.
#[derive(Debug, Clone, Serialize)]
pub struct ObsidianWriteOutcome {
    /// The note that now holds this meeting.
    pub path: String,
    pub status: WriteStatus,
    /// The differing note that was left untouched, when `status` is
    /// `alternative`.
    pub existing_path: Option<String>,
}

/// Candidate file name for the nth attempt: `name.md`, `name (2).md`, ...
fn candidate_name(base: &str, index: usize) -> String {
    if index <= 1 {
        return base.to_string();
    }

    match base.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem} ({index}).{extension}"),
        _ => format!("{base} ({index})"),
    }
}

/// Write `content` into `folder`, never overwriting a note with different text.
///
/// - identical content already present: report `unchanged` and write nothing;
/// - different content at the computed name: leave it alone and try `(2)`, `(3)`,
///   ... so this meeting still syncs and the earlier note survives;
/// - `create_new` is used for the final write so a file appearing between the
///   read and the write is not clobbered either.
pub fn write_note(
    folder: &Path,
    base_name: &str,
    content: &str,
) -> Result<ObsidianWriteOutcome, String> {
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("Cannot create {}: {error}", folder.display()))?;

    let mut first_different: Option<PathBuf> = None;

    for index in 1..=1000 {
        let path = folder.join(candidate_name(base_name, index));

        match std::fs::read(&path) {
            Ok(existing) => {
                if existing == content.as_bytes() {
                    return Ok(ObsidianWriteOutcome {
                        path: path.to_string_lossy().to_string(),
                        status: WriteStatus::Unchanged,
                        existing_path: None,
                    });
                }

                if first_different.is_none() {
                    first_different = Some(path);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        file.write_all(content.as_bytes()).map_err(|error| {
                            format!("Cannot write {}: {error}", path.display())
                        })?;

                        return Ok(ObsidianWriteOutcome {
                            path: path.to_string_lossy().to_string(),
                            status: if index == 1 {
                                WriteStatus::Created
                            } else {
                                WriteStatus::Alternative
                            },
                            existing_path: first_different
                                .map(|existing| existing.to_string_lossy().to_string()),
                        });
                    }
                    // Something appeared after the read; treat it like an
                    // existing note and try the next name.
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        return Err(format!("Cannot write {}: {error}", path.display()));
                    }
                }
            }
            Err(error) => {
                return Err(format!("Cannot read {}: {error}", path.display()));
            }
        }
    }

    Err(format!(
        "Tried 1000 names in {} and every one is taken. Move or rename the old notes first.",
        folder.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illegal_characters_are_replaced() {
        assert_eq!(sanitize_component("Q3: 规划/复盘?"), "Q3- 规划-复盘-");
        assert_eq!(sanitize_component("a\u{0007}b"), "a-b");
    }

    #[test]
    fn empty_and_dot_only_titles_get_a_fallback() {
        assert_eq!(sanitize_component(""), "untitled-meeting");
        assert_eq!(sanitize_component("   "), "untitled-meeting");
        assert_eq!(sanitize_component("..."), "untitled-meeting");
    }

    #[test]
    fn trailing_dots_and_spaces_are_removed() {
        assert_eq!(sanitize_component("Report. "), "Report");
    }

    #[test]
    fn long_titles_are_truncated() {
        let title = "会".repeat(200);
        assert!(sanitize_component(&title).chars().count() <= 80);
    }

    #[test]
    fn windows_device_names_are_escaped() {
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("com1"), "_com1");
        // Not a device name: the digit is not the fourth character.
        assert_eq!(sanitize_component("COM"), "COM");
        assert_eq!(sanitize_component("console"), "console");
    }

    #[test]
    fn subfolder_components_are_sanitized_and_traversal_is_dropped() {
        assert_eq!(
            sanitize_subfolder(Some("../Meetings/2026/")),
            Some("Meetings/2026".to_string())
        );
        assert_eq!(sanitize_subfolder(Some("C:\\Obsidian\\Notes")), Some("C-/Obsidian/Notes".to_string()));
        assert_eq!(sanitize_subfolder(Some("/")), None);
        assert_eq!(sanitize_subfolder(None), None);
    }

    #[test]
    fn meeting_date_reads_rfc3339_and_plain_dates() {
        assert_eq!(
            meeting_date("2026-02-03T10:00:00.000Z"),
            Some("2026-02-03".to_string())
        );
        assert_eq!(meeting_date("2026-02-03"), Some("2026-02-03".to_string()));
        assert_eq!(meeting_date("not a date"), None);
        assert_eq!(meeting_date(""), None);
    }

    #[test]
    fn file_name_contains_the_sanitized_title_and_date() {
        assert_eq!(
            note_file_name("Q3: 规划/复盘", Some("2026-02-03")),
            "Q3- 规划-复盘 2026-02-03.md"
        );
        assert_eq!(note_file_name("Weekly Sync", None), "Weekly Sync.md");
        assert_eq!(note_file_name("", Some("2026-02-03")), "untitled-meeting 2026-02-03.md");
    }

    #[test]
    fn frontmatter_is_valid_yaml_with_quoted_values() {
        let block = frontmatter(
            "Q3: planning",
            Some("2026-02-03"),
            "meeting-1",
            &["client".to_string(), "Q3 规划".to_string()],
        );

        assert!(block.starts_with("---\n"));
        assert!(block.contains("title: \"Q3: planning\"\n"));
        assert!(block.contains("date: \"2026-02-03\"\n"));
        assert!(block.contains("meeting_id: \"meeting-1\"\n"));
        assert!(block.contains("tags:\n  - \"client\"\n  - \"Q3 规划\"\n"));
        assert!(block.trim_end().ends_with("---"));
    }

    #[test]
    fn frontmatter_escapes_quotes_backslashes_and_folds_newlines() {
        let block = frontmatter("He said \"hi\"\\", None, "m\"1", &[]);
        assert!(block.contains(r#"title: "He said \"hi\"\\""#));
        assert!(block.contains(r#"meeting_id: "m\"1""#));
        assert!(!block.contains("tags:"));
    }

    #[test]
    fn frontmatter_never_emits_a_raw_newline_inside_a_value() {
        let block = frontmatter("line one\nline two", None, "m1", &[]);
        let title_line = block.lines().find(|line| line.starts_with("title:")).unwrap();
        assert_eq!(title_line, "title: \"line one line two\"");
    }

    #[test]
    fn template_replaces_placeholders_and_keeps_frontmatter() {
        let note = render_note(
            "Weekly",
            Some("2026-02-03"),
            &["client".to_string()],
            "m1",
            "# Weekly\n\nBody",
            Some("## {{title}} ({{date}})\nTags: {{tags}}\n\n{{content}}"),
        );

        assert!(note.starts_with("---\n"));
        assert!(note.contains("## Weekly (2026-02-03)"));
        assert!(note.contains("Tags: client"));
        assert!(note.contains("Body"));
        // The default markdown heading is preserved inside the template.
        assert!(note.contains("# Weekly"));
    }

    #[test]
    fn template_without_content_still_includes_the_meeting() {
        let note = render_note("Weekly", None, &[], "m1", "# Weekly\n\nBody", Some("Journal: {{title}}"));
        assert!(note.contains("Journal: Weekly"));
        assert!(note.contains("Body"));
    }

    #[test]
    fn placeholders_inside_user_text_are_not_expanded_again() {
        // A title that looks like a placeholder must be written literally; a
        // second substitution pass would pull the note body into the title.
        let note = render_note(
            "Weekly {{content}}",
            None,
            &[],
            "m1",
            "Body",
            Some("{{title}} | {{content}}"),
        );
        assert!(note.contains("Weekly {{content}} | Body"));
    }

    #[test]
    fn unknown_template_placeholders_are_left_visible() {
        let note = render_note("Weekly", None, &[], "m1", "Body", Some("{{mood}}"));
        assert!(note.contains("{{mood}}"));
        assert!(note.contains("Body"));
    }

    #[test]
    fn writing_a_new_note_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = write_note(dir.path(), "Weekly 2026-02-03.md", "hello").unwrap();

        assert_eq!(outcome.status, WriteStatus::Created);
        assert_eq!(outcome.existing_path, None);
        assert_eq!(std::fs::read_to_string(&outcome.path).unwrap(), "hello");
    }

    #[test]
    fn writing_identical_content_is_reported_as_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "Weekly.md", "hello").unwrap();
        let outcome = write_note(dir.path(), "Weekly.md", "hello").unwrap();

        assert_eq!(outcome.status, WriteStatus::Unchanged);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn different_content_never_overwrites_the_existing_note() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_note(dir.path(), "Weekly.md", "old").unwrap();
        let second = write_note(dir.path(), "Weekly.md", "new").unwrap();

        assert_eq!(second.status, WriteStatus::Alternative);
        assert_eq!(first.path, second.existing_path.clone().unwrap());
        assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&second.path).unwrap(), "new");
        assert!(second.path.ends_with("Weekly (2).md"));
    }

    #[test]
    fn re_syncing_the_same_edited_content_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        write_note(dir.path(), "Weekly.md", "old").unwrap();
        let second = write_note(dir.path(), "Weekly.md", "new").unwrap();
        let third = write_note(dir.path(), "Weekly.md", "new").unwrap();

        // The second write created "Weekly (2).md"; the third finds it identical
        // instead of accumulating "Weekly (3).md".
        assert_eq!(third.status, WriteStatus::Unchanged);
        assert_eq!(third.path, second.path);
    }

    #[test]
    fn writing_creates_the_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("Meetings").join("2026");
        let outcome = write_note(&nested, "Weekly.md", "hello").unwrap();
        assert!(Path::new(&outcome.path).exists());
    }
}
